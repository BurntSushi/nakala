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
