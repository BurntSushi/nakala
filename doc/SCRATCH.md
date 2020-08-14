This is a "scratch" document that was primarily used during initial development
of Nakala. It keeps track of higher level constraints that I used to drive the
implementation, along with lots of other bits. The two different use cases I
had in mind driving this were:

* To serve as the name index for imdb-rename, which just needs to index short
  name strings. Notably though, multiple names can be indexed for the same ID.
  This requires true relevance ranking via a TF-IDF scheme.
* Accelerating ripgrep searches on huge corpora. This should not use any
  relevance ranking at all, and requires yielding the complete set of document
  IDs.


## Things Nakala should NOT do

This is no easy task. We really want to try to strike a balance between being
simple enough that Nakala is flexible and embeddable in other applications, but
heavy enough that it actually handles most of the complexity in many common use
cases. Striking this balance in an IR engine is hard. Still, there are some
things we can rule out fairly easily:

* Nakala will not handle tokenization beyond simplistic approaches. Nakala
  might provide some standard whitespace or ngram tokenizers, but it is
  otherwise up to the caller to plug in more sophisticated things like language
  aware stemmers or lemmatizers.
* Nakalas query infrastructure should generally be limited to boolean term
  queries.
* Nakala will not have a notion of "fields" or any such thing. Every document
  is just a multi-set of terms. In this sense, there is no schema as there is
  only one field for every document.
* Similar to the above, Nakala will strictly be an index. It will not store
  document context.
* The only things in the index will be document frequencies. Positions will not
  be indexed. This means phrase queries will not be possible... For that
  reason, I strongly suspect this constraint may be relaxed in a future
  version, but we leave it out of the initial version. (Because phrase queries
  are supremely useful.)

These alone draw a fairly bright dividing line between Nakala and more full
fledged IR systems like Lucene. Nakala is built for simpler tasks, and we
acknowledge that more complex tasks are better solved by other tools.

## Things Nakala should do

We've established some things that Nakala won't do, but what are some things
that it should do?

* It should handle the complexity of deletes in addition to scaling writes
  using Lucene's segment merging approach. Nakala should not _force_ merges on
  you though. That is, it should still be possible to create a single segment
  index without any merges. Handling merges and deletes is a phenomenal source
  of complexity, but it can be relatively contained and doesn't really lend
  itself to being terribly customized. For that reason, it makes sense for
  Nakala to own it.
* Nakala will permit associating an arbitrary byte sequence with any document.
  The byte sequence must not have any constraints imposed on it. This implies
  that Nakala will maintain a mapping between user supplied IDs and internal
  doc IDs. (Where doc IDs are scoped to a single segment, are u32s and are
  monotonically increasing.)
* As a corollary to the above, the Unit of Retrieval will be the user supplied
  ID.
* It is possible to associate multiple multi-sets with the same user supplied
  ID, however, it will not be possible to distinguish which of those multi-sets
  matched. (This is useful for imdb-rename's name index where many films have
  aliases.)
* Nakala should have a pluggable "file system abstraction," such that it can
  have its index read or written to from anywhere. That is, it shouldn't tied
  to an actual file system. With that said, the typical way to read a Nakala
  index will be through memory maps. It's not clear how useful this abstraction
  will be though, since it will probably operate at the segment level and
  segments can get quite large.
* Nakala should provide a way to search the index not only without relevance
  ranking, but without the overhead of relevance ranking. This should likely
  be both an index time and a query time setting. That is, it should be
  possible to build an index without document frequencies and it should be
  possible to search an index with document frequencies but without relevance
  ranking.

## Handling identifiers

Figuring out how to handle identifiers properly is surprisingly complicated due
to the varying constraints and access patterns. To start with, Nakala will have
two different identifiers in play:

1. Identifiers supplied by the caller, which are called "user IDs." A user ID
   is an arbitrary byte sequence that Nakala treats as entirely opaque. A user
   ID is the Unit of Retrieval when executing a search and is therefore the
   primary way in which a search result can be tied back to the actual thing
   that has been indexed.
2. Internal identifiers called "doc IDs." A doc ID is unique only within a
   particular segment. Doc IDs are controlled and allocated by Nakala in order
   to retain certain properties, such as being a dense sequence of
   monotonically increasing integers. Doc IDs are u32 identifiers that make up
   the posting lists for each term (in addition to term frequencies).
   Therefore, each segment can index a maximum of 2^32 separate documents.

When a user indexes a document, they must provide at least two things: a user
ID for the document and a sequence of terms to index. Since doc IDs make up the
posting lists and user IDs are the things we return, it follows that we must
have a mapping from a doc ID to its corresponding user ID.

Moreover, since Nakala will support deleting documents, the only sensible way
to implement deletes is by requiring the caller to provide the user ID of the
document they want to delete. Therefore, there must be some way of mapping a
user ID to its corresponding doc ID.

Orthogonally, we also must decide how to deal with (or enforce) the uniqueness
of user IDs. We have a few choices:

1. We could require them to be globally unique across the entire index.
2. We could _assume_ they are unique and force the caller to guarantee it.
3. We could permit multiple documents to be indexed under the same user ID. We
   would treat each such document as distinct internally (i.e., they would each
   get their own doc ID), but would deduplicate them at search time and return
   only the best matching document.

(1) is quite difficult to do, since it requires doing some kind of index wide
lookup for every ID of every document that is indexed. Other than this being
costly in and of itself, it will also need to a synchronized check of some kind
to prevent races from sneaking in a duplicate ID. This in turn would make the
check even more expensive. Without this requirement, indexing documents in each
segment is a completely independent process that only needs to be synchronized
once a segment is ready to join the index.

(2) seems feasible on its face, if not a cheap cop-out. However, if we assume
that user IDs are unique and design our index format around that, then we would
run into trouble if that assumption were violated. For example, if two
different segments contain the same user ID and we went to merge them, then the
merge would either need to fail or drop one of the duplicate documents. The
former essentially makes the index broken since any two segments should be
mergeable. The latter is undesirable since it silently drops data.

(3) is really the only good choice IMO. Moreover, it's actually useful. For
example, in the case of imdb-rename, it's quite handy to be able to index
multiple names under the same identifier since many movies and TV shows have
aliases. Being able to search all of those aliases as if they were one document
and having them deduplicated at search time in favor of the "best" match is
_exactly_ what you want.

We now need to design an index format that supports this functionality without
violating any of our constraints. We will use two different per-segment
structures to deal with this:

* An FST that maps each user ID to a range of doc IDs. Every doc ID in the
  range corresponds to a document that has been indexed for that user ID.
* A map from doc ID to a range of doc IDs that are equivalent. This is
  structured as a single contiguous block of `N * u64LEs`, where `N` is the
  total number of documents in a segment. Each doc ID corresponds to an offset
  into this map. The value for each doc ID corresponds precisely to a range in
  the FST above.

The "trick" to this scheme is that regardless of the order in which documents
are indexed, doc IDs are assigned to user IDs in a monotonically increasing
order. We do this by:

* When a new document is indexed, we generate a temporary doc ID and assign it
  to the user ID given for that document. We record this mapping and permit
  multiple temporary doc IDs to be assigned to a single user ID.
* Once it comes time to create the segment, we sort the user IDs in
  lexicographic order.
* Iterate over the user IDs in lexicographic order. For each user ID, iterate
  over each temporary doc ID. For each temporary doc ID, generate a
  monotonically increasing real doc ID and associated it with the user ID.
* Record a mapping from temporary doc ID to real doc ID during the previous
  step.
* When writing the posting lists, map the temporary doc IDs recorded in the
  postings in memory to their real doc IDs.

This scheme permits us to create monotonically increasing ranges of doc IDs
that all map to the same user ID. This in turn permits the aforementioned FST
to provide a map from user ID to doc IDs and also the reverse direction of doc
ID to user ID. (In the special case of an FST with monotonically increasing
values, one can find the key given a value.)

While this might seem like we are buffering a lot in memory at index time, it
is actually negligible. Namely, the entire posting list for the segment also
needs to be buffered into memory, which will dwarf the size of the ID mapping.
(The way Nakala "scales" is by implementing a segment merge algorithm that
doesn't require storing any whole segment in memory at once. But when writing a
segment initially, one will typically cap the size of the segment based on how
much memory you're willing to use.)

This scheme also works very nicely with merging. Since we don't enforce
uniqueness, there are never any conflicts that could prevent merging. All we
need to do is stream a union over all of the user ID FSTs in each segment that
we're merging. When two or more segments have the same user ID, we simply merge
their doc ID ranges with a fresh allocation. Since everything is visited in
lexicographic order, we can preserve the monotonicity of the doc ID ranges.
And regenerating the map from doc ID to doc ID ranges is also trivial.

## Merging segments

One of the main constraints of segment merging is that it should be able to
happen with ~constant heap memory. In my mind's eye, this is nearly achievable.
One of the key things that makes this possible is that multiple FSTs can be
unioned together in a streaming fashion from the index. This means we can
seamlessly merge the term index and the userID -> docID FST index described
above. Moreover, since doc IDs are unique to each segment, we can treat all of
them as referring to distinct documents and simply allocate new doc IDs as
needed. When a single user ID is mapped to multiple doc IDs, even if this
happens in multiple segments, then those duplicates are all contiguous because
of how we allocated the doc IDs, so those can all be handled in a streaming
fashion as well. So to can document frequencies, which are stored in a separate
stream from doc IDs, but in correspondence.

Other than things like headers, footers and miscellaneous attributes, that
pretty much covers everything except for the skip list generated for the
posting list for each term. There's really no way that I can see to stream
these to disk as we need to know all of the skip entries in order to partition
them into levels. Moreover, to avoid buffering the serialized bytes of skip
entries, we need to write them in reverse. (Because each skip entry contains a
pointer to its next sibling.) With that said, each skip extra is a u32 doc ID
with a usize offset, so it's in practice quite small. Moreover, we only create
a skip entry for each block of doc IDs and each block can contain many doc IDs.
So the number of skip entries is quite small. Still, its size _is_ proportional
to a part of the index size, but to my knowledge, it is the only such thing.
(We could do away with skip lists since skipping entire blocks is itself quite
fast, but my guess is that they are too important for speeding up conjunction
queries to throw them away.)

## Handling deletes

Deletes are a significant problem in the Lucene-style index format that Nakala
uses. The main problem is that the index is designed to be immutable, so you
can't just remove documents from the index in place. The only sensible approach
is to use a method called tombstoning. This basically involves two steps:

1. When the caller deletes a document, we record the doc ID of the document
   that has been deleted in its corresponding segment. Subsequent search
   requests consult the recorded deletes and omit results matching the deleted
   documents.
2. At some convenient point in the future, physically remove the documents from
   the index. In the Lucene-style of indexing, this happens when segments are
   merged. (That is, a merged segment will always start with zero deleted
   documents.)

For the most part, (2) is fairly simple to accommodate. When two or more
segments are merged, we simply omit writing doc IDs to the postings that have
been marked as deleted in its corresponding segment.

(1) is the trickier part to handle. Firstly, since deletes are recorded via
their doc ID in their respective segments and since doc IDs are internal to
Nakala, it follows that the only reasonable way for a caller to delete a
document is to delete it by its user ID. (We could in theory provide handles to
a specific doc ID in a specific segment as part of search results, but omitting
a way to delete a document by its user ID seems like a critical failing.) This
means we need a way to quickly map a user ID to every doc ID its associated
with in every segment. This is where the FST mapping user IDs to doc IDs
described in the section on identifiers comes in handy.

Since deletes are recorded by doc ID and since doc IDs are assigned in a
contiguous and dense fashion, it follows that we can record deletes using a
bitset. So we only need ceil(N/8) bytes of space to record them.

The only other problem remaining is how to handle the fact that deletes are one
of the few "mutable" aspects of Nakala index. (With the other being the current
list of active segments.) Since Nakala wants to permit multiple processes (or
threads) to index new documents at the same time, this implies that deleting
documents require some kind of inter-process synchronization. We will use the
same techniques here that we use to synchronize the addition of segments to the
index.

## Transaction log and synchronization

A Nakala index is formed of zero or more segments. Every search request must be
run against every segment, and results must be merged. Over time, as the
number of segments increases, they are merged together into bigger segments.
This architecture makes it possible to use limited memory to create small
segments at first, and gradually scale by merging segments at search latencies
start to slow down due to the number of segments.

One thing required by this architecture is the ability to know which segments
are active at any given point in time. Namely, segments that have been merged
should no longer be searched (instead, only the merged segment should be
consulted). A key additional constraint here is that the index should present a
consistent view of the world during a search request. That is, search requests
should not spontaneously fail because we decided to remove a segment that it
was still using. And of course, all of this must be robust with respect to
multiple writers and readers using the index at the same time. Permitting
multiple writers is quite important, since the vast majority of the work of
indexing new documents can be performed in parallel by simply creating distinct
segments.

The primary way in which we approach this problem is through a transaction log.
The transaction log is the single point of truth about which segments are live
at any particular point in time. The transaction log has only a few primitives.
Each primitive includes a timestamp.

* AddSegment(segmentID) - This indicates that a segment with the given ID has
  been added to the index. The segment ID can be used to derive the location of
  the segment in storage. Adding a new segment to the log faisl if there
  already exists a live segment with the given ID.
* StartMerge(segmentID...) - This indicates that one or more segments have
  begun the process of being merged. The purpose of this log is to instruct
  other processes that some set of segments are involved in a merge. This helps
  reduce repetitive work. Adding a StartMerge log fails if it contains a
  segment ID that is involved in an active merge.
* CancelMerge(segmentID...) - This indicates that a prior StartMerge has been
  stopped without finishing. The segment IDs in StartMerge remain active and
  are now eligible for merging again.
* EndMerge(newSegmentID, oldSegmentID...) - This indicates that a merge has
  finished and a new segment has been added to the index. The old segment ID
  list must correspond to a previously recorded StartMerge log, and now point
  to segments that are no longer in the index. Adding an EndMerge log fails if
  there is no corresponding StartMerge log, or if the new segment ID already
  exists, or if any of the old segment IDs are no longer active or if a
  CancelMerge operation took place.
* Checkpoint(...) - Records the current set of active segments along with any
  outstanding StartMerge logs with no corresponding EndMerge logs. The purpose
  of a checkpoint is that one does not need to read any previous logs to
  determine the active set of segments.

From this log, it should be clear how to compute the active set of segments at
any particular time. Since adding a new entry to this log should generally be
quite cheap, and to keep things simple, we require that there can only be one
writer to the log at any particular time. Each write will check that the new
log is valid to add and will fail if not, based on the state of the log at the
time of the write. We can enforce a single writer via file locking (described
in its own section below).

There are a few remaining hiccups here.

Firstly, when a StartMerge is added to the log, we have no guarantee that a
corresponding CancelMerge or EndMerge will be added. The merge process might
die unexpectedly for example. The only real choice here as far as I can see
is to cause an implicit CancelMerge to appear in the log if it or an EndMerge
isn't committed within N time of the StartMerge. At that point, the segments
will become available for merging again. If the merge just took longer than the
timeout, then the resulting EndMerge log would fail to add because of the
previously committed CancelMerge log. This is of course a bit wasteful, but it
keeps the index coherent. In terms of waste, we should just try hard to avoid
getting into scenarios where we don't commit an EndMerge in time. (In theory,
we could introduce a ProgressMerge primtiive that indicates the merge is still
running, which could reset the StartMerge timeout, but we should try to get by
without that extra complexity.)

One remaining hiccup here is to figure out when it's okay to actually delete
old segments from storage. The only way to do this is to know whether there are
any processes still reading a particular segment. It's possible to track this
by simply recording the position in the transaction log at which a reader
started a search. For any particular position in the log, we can compute the
precise set of active segments. Thus, if we know that all extant readers are
passed a certain point in the log, we could compute which segments are both no
longer active and no longer being read from.

The main problem here is of course tracking readers. Firstly, this would
require a reader to write something to storage so that it can be seen by other
processes. Secondly, if a reader is required to mark its position in the log,
then it must also be required to unmark itself, which of course means that'll
need to introduce another timeout dance like we did for merges above in the
transaction log. (Since we cannot depend on a reader unmarking itself. It might
be ungracefully killed without the chance to unmark itself.)
