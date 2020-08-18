This is a "scratch" document that was primarily used during initial development
of Nakala. It keeps track of constraints, architecture, design and nasty
implementation details that I used while writing the code. The two different
use cases I had in mind driving everything were:

* To serve as the name index for imdb-rename, which just needs to index short
  name strings. Notably though, multiple names can be indexed for the same ID.
  This requires true relevance ranking via a TF-IDF scheme.
* Accelerating ripgrep searches on huge corpora. This should not use any
  relevance ranking at all, and requires yielding the complete set of document
  IDs.

Of course, I tried to keep generality in mind as well, as I've used IR systems
in many other contexts too. And tried to think about how others might use this
code, but didn't step too far outside my comfort zone.

## Novelty prelude

Before diving into details, it should be noted that almost no technique
described in this document is novel. Maybe the _combination_ of these things is
novel, but it's hard to say. Nakala makes use of write-ahead logging,
copy-on-write and file locking to deal with concurrency. These techniques have
been used in databases since forever. And in particular, I've taken a lot of
inspiration from material I've read on Lucene, SQLite and LMDB, as all of those
things support concurrency in some form.

One thing that I did think was novel was the use of advisory locks to discover
stale readers (or merges that have been ungracefully killed). But alas, after
coming up with this idea and looking to see how other embedded databases to
deal with this, it does indeed seem that LMDB uses this same technique.
(Although, curiously only on Unix and not Windows, even though I believe the
technique should also work on Windows.) With that said, I still have never seen
this technique explicitly discussed. But lots of things never are in this
field, seemingly. Either it's just not commonly known, or more likely, everyone
in the field thinks it's "obvious." (If you can't tell, I was greatly miffed by
a lot of the elitist attitudes I came across in my research.)

In general, this sort of information has been extremely difficult to find. In
doing research, I read lots of code, comments, papers and mailing list posts.
Frequently, folks would refer to "techniques" that have been used "since
forever," and yet, I found it difficult to find authoritative sources on them.
They are almost always buried in platform specific details of how concurrency
primitives work, or particular interpretations of standards like POSIX, or
experiments on implementations of said standard or even extrapolations from
hardware itself. (The author of LMDB has written at length about how writes
below a certain number of bytes are guaranteed to be atomic at the hardware
level by HDDs/SSDs for instance, and then proceeds to rely on those properties
in the implementation of LMDB. How does one actually come across this sort of
information _and_ be confident enough to rely on it? `¯\_(ツ)_/¯`.)


## Things Nakala should NOT do

We really want to try to strike a balance between being simple enough that
Nakala is flexible and embeddable in other applications, but heavy enough that
it actually handles most of the complexity in many common use cases. Striking
this balance in an IR engine is hard. Still, there are some things we can rule
out fairly easily:

* Nakala will not handle tokenization beyond simplistic approaches. Nakala
  might provide some standard whitespace or ngram tokenizers, but it is
  otherwise up to the caller to plug in more sophisticated things like language
  aware stemmers or lemmatizers.
* Nakala's query infrastructure should generally be limited to boolean term
  queries.
* Nakala will not have a notion of "fields" or any such thing. Every document
  is just a multi-set of terms. In this sense, there is no schema as there is
  only one field for every document. (Technically, Nakala does associated a
  user supplied identifier with each document, and this is represented as its
  own field in a specialized way.)
* Similar to the above, Nakala will strictly be an index. It will not store
  document content.
* The only things in the index will be document frequencies. Positions will not
  be indexed. This means phrase queries will not be possible... For that
  reason, I strongly suspect this constraint may be relaxed in a future
  version, but we leave it out of the initial version. (Because phrase queries
  are supremely useful, but increase the complexity of the implementation.)

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
  have its index read or written to from anywhere. That is, it shouldn't be
  tied to an actual file system. With that said, the typical way to read a
  Nakala index will be through memory maps. It's not clear how useful this
  abstraction will be though, since it will probably operate at the segment
  level and segments can get quite large. (In reviewing this document, it is
  awfully tempting to forgo this abstraction layer initially, since designing
  it seems like a lot of work _and_ it's insidiously complicated.)
* Nakala should provide a way to search the index not only without relevance
  ranking, but without the overhead of relevance ranking. This should likely
  be both an index time and a query time setting. That is, it should be
  possible to build an index without document frequencies and it should be
  possible to search an index with document frequencies but without relevance
  ranking.

The other major design point that Nakala could have occupied---and I did
carefully consider---is a library for writing a single segment. This is
tempting because this design space only requires focusing on the IR bits and
the file format, and _not_ on anything concurrency related. Why? Because only
one thread can feasibly write to a segment, and once it's written, it can never
be mutated due to its representation. So at that point, any number of threads
or processes can read from it without synchronization.

While it's plausible I may build such a standalone library, it's likely to end
up as `nakala-core`, where `nakala` itself handles everything else. The problem
with only offering that is that it isn't broadly useful. It really only applies
to some specific niches (imdb-rename being one of them), and in order to get
it to work in a more scalable fashion, you really need to wind up building a
lot of the infrastructure that `nakala` could have done for you. Finally, on
top of all of that, ripgrep's use case absolutely requires building out that
infrastructure, so I must do it anyway. We might as well package it up and
make it generally useful.

Plus, ever since I started learning about information retrieval ~six years ago,
I've always wanted to build an IR system. But I wanted to do something
different than what Lucene has done. Nakala isn't _that_ different than Lucene,
but I think it will occupy enough of a niche for it to be successful. Time will
tell.

## Handling identifiers

Figuring out how to handle identifiers properly is surprisingly complicated due
to the varying constraints and access patterns. To start with, Nakala will have
two different identifiers in play:

1. Identifiers supplied by the caller, which are called "user IDs." A user ID
   is an arbitrary byte sequence that Nakala treats as entirely opaque. A user
   ID is the Unit of Retrieval when executing a search and is therefore the
   primary way in which a search result can be tied back to the actual thing
   that has been indexed.
2. Internal document identifiers called "doc IDs." A doc ID is unique only
   within a particular segment. Doc IDs are controlled and allocated by Nakala
   in order to retain certain properties, such as being a dense sequence of
   monotonically increasing integers. Doc IDs are u32 identifiers that make
   up the posting lists for each term (in addition to term frequencies).
   Therefore, each segment can index a maximum of 2^32 separate documents.

(Nakala will have other identifiers in play as well, such as transaction IDs
and segment IDs, but these are a bit more mundane.)

When a user indexes a document, they must provide at least two things: a user
ID for the document and a sequence of terms to index. Since doc IDs make up the
posting lists and user IDs are the things we return, it follows that we must
have a mapping from a doc ID to its corresponding user ID.

Moreover, since Nakala will support deleting documents, the only sensible way
to implement deletes is by requiring the caller to provide the user ID of the
document they want to delete. Therefore, there must be some way of mapping a
user ID to its corresponding doc ID. (Not quite the _only_ sensible way.
Another approach would be to permit callers to delete a document corresponding
to a search, but this is quite restrictive and provides no easy foreign key to
delete a document. However, we can (easily) provide this in addition to deletes
by user ID.)

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
costly in and of itself, it will also need to be a synchronized check of some kind
to prevent races from sneaking in a duplicate ID. This in turn would make the
check even more expensive. Without this requirement, indexing documents in each
segment is a completely independent process that only needs to be synchronized
once a segment is ready to join the index. _With_ this requirement, we
completely thwart our ability to permit callers to use multiple writers at once
without blocking each other for the vast majority of the writing process.

(2) seems feasible on its face, if not a cheap cop-out. However, if we assume
that user IDs are unique and design our index format around that, then we would
run into trouble if that assumption were violated. For example, if two
different segments contain the same user ID and we went to merge them, then the
merge would either need to fail or drop one of the duplicate documents. The
former essentially makes the index broken since any two segments should be
mergeable. The latter is undesirable since it silently drops data.

(3) is really the only good choice IMO if we want to permit truly concurrent
writers. Moreover, it's actually useful. For example, in the case of
imdb-rename, it's quite handy to be able to index multiple names under the same
identifier since many movies and TV shows have aliases. Being able to search
all of those aliases as if they were one document and having them deduplicated
at search time in favor of the "best" match is _exactly_ what you want.

One could argue that (3) is somewhat of a cop-out, because if you do care about
uniqueness, it generally puts the onus on the caller to manage that themselves.
Otherwise, the caller could end up _accidentally_ indexing duplicate documents
when what they really wanted was a constraint violation telling them that the
ID already exists. Other than the considerable benefits of this approach
mentioned above, one final justification is that Nakala is very deliberately
not a document store. The _only_ thing it can do is return an ID. This ID, in
many cases, is almost always going to be associated with something else in some
other system of record. And _that_ system of record can manage uniqueness.

We now need to design an index format that supports this functionality without
violating any of our constraints. We will use two different per-segment
structures to deal with this:

* An FST that maps each user ID to an inclusive range of doc IDs. Every doc ID
  in the range corresponds to a document that has been indexed for that user
  ID.
* A map from doc ID to a range of doc IDs that are equivalent. This is
  structured as a single contiguous block of `N * u64LEs`, where `N` is the
  total number of documents in a segment. Each doc ID corresponds to an offset
  into this map. The value for each doc ID corresponds precisely to a range in
  the FST above.

(N.B. In the case where a segment does not have any duplicates, we can omit the
second map entirely since every range is just the doc ID repeated in the upper
and lower 32 bits of the u64LE. This is easy to detect because of the state we
buffer while writing before actually writing the segment. See below.)

The "trick" to this scheme is that regardless of the order in which documents
are indexed, doc IDs are assigned to user IDs in a monotonically increasing
order. We do this by:

* When a new document is indexed, we generate a temporary doc ID and assign it
  to the user ID given for that document. We record this mapping and permit
  multiple temporary doc IDs to be assigned to a single user ID.
* Once it comes time to create the segment in storage, we sort the user IDs in
  lexicographic order.
* Iterate over the user IDs in lexicographic order. For each user ID, iterate
  over each temporary doc ID. For each temporary doc ID, generate a
  monotonically increasing real doc ID (using our iteration order) and
  associated it with the user ID. The resulting mapping is exactly the same
  size as the `userID->tempDocID` map.
* Record a mapping from temporary doc ID to real doc ID during the previous
  step.
* When writing the posting lists, map the temporary doc IDs recorded in the
  postings in memory to their real doc IDs.

(N.B. If every user ID maps to a single temporary doc ID, we still need to do
the dance above, since we still need doc IDs to be allocated in monotonically
increasing order with respect to the lexicographic order of user IDs. This is
what gives us the "superpower" to do `docID->userID` reverse lookups on the
FST mentioned below.)

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

(N.B. There is a little more to this, as deletes will need to make their way
through the transaction log first. See below.)

For the most part, (2) is fairly simple to accommodate. When two or more
segments are merged, we simply omit writing doc IDs to the postings that have
been marked as deleted in its corresponding segment.

(1) is the trickier part to handle. Firstly, since deletes are recorded via
their doc ID in their respective segments and since doc IDs are internal to
Nakala, it follows that the only reasonable way for a caller to delete a
document is to delete it by its user ID. (We could in theory provide handles to
a specific doc ID in a specific segment as part of search results, but omitting
a way to delete a document by its user ID seems like a critical failing.) This
means we need a way to quickly map a user ID to every doc ID it is associated
with in every segment. This is where the FST mapping user IDs to doc IDs
described in the section on identifiers comes in handy.

Since deletes are recorded by doc ID and since doc IDs are assigned in a
contiguous and dense fashion, it follows that we can record deletes using a
bitset. So we only need ceil(N/8) bytes of space to record them. However, this
does somewhat complicate concurrent processes deleting documents. If each byte
records the deletion status of 8 documents, then multiple processes must
synchronize before writing to this file to avoid a race that accidentally
squashes a document delete. While at this point in time it's not clear how much
of a performance issue it would be to synchronize in this case, my current plan
to avoid the synchronization problem entirely by using a full byte to record
the deletion status for each document. Then, multiple processes can mark
documents deleted concurrently with zero synchronization. While this does waste
space, it is very little waste compared the segment itself and other ID
mappings we already need to store.

The only other problem remaining is how to handle the fact that deletes are one
of the few "mutable" aspects of Nakala index. (With the other being the current
list of active segments.) Since Nakala wants to permit multiple processes (or
threads) to index new documents at the same time, this implies that deleting
documents require some kind of inter-process synchronization. To achieve this,
we'll use a combination of a transaction log to initially record the deletes
and file locking to eventually write tombstones to the corresponding segment.

We'll explaining all of the synchronization issues in more detail later, but
one of the nastiest variants of this is what happens when a document is deleted
in a segment that has been merged into another segment. If we did nothing, we
would write the _old_ segment ID of the document into the transaction log, but
since that segment is no longer active _and_ we have no way of mapping the old
docID to its corresponding docID in the merged segment, then we would wind up
losing the delete. Which is bad. We wind up solving this by building another ID
map when we merge segments, which maps doc IDs in their old segments to the doc
IDs in the merged segment. This wastes space, but the mapping can be deleted
once it is known that there are no active handles to any of the old segments.

## Merging segments

One of the main constraints of segment merging is that it should be able to
happen with ~constant heap memory. In my mind's eye, this is almost achievable,
but not quite. One of the key things that makes this mostly possible is that
multiple FSTs can be unioned together in a streaming fashion from the index,
and then also streamed to disk in a new FST that has all of the keys from the
union. This means we can seamlessly merge the term index and the userID ->
docID FST index described above. Moreover, since doc IDs are unique to each
segment, we can treat all of them as referring to distinct documents and simply
allocate new doc IDs as needed. When a single user ID is mapped to multiple
doc IDs, even if this happens in multiple segments, then those duplicates are
all contiguous because of how we allocated the doc IDs, so those can all be
handled in a streaming fashion as well. So too can document frequencies, which
are stored in blocks adjacent to their corresponding doc IDs.

Other than things like headers, footers and miscellaneous attributes, that
pretty much covers everything except for the skip list generated for the
posting list for each term. There's really no way that I can see to stream
these to disk as we need to know all of the skip entries in order to partition
them into levels. Moreover, to avoid buffering the serialized bytes of skip
entries, we need to write them in reverse. (Because each skip entry contains a
pointer to its next sibling.) With that said, each skip entry is a u32 doc ID
with a usize offset, so it's in practice quite small. Moreover, we only create
a skip entry for each block of doc IDs and each block can contain many doc IDs.
So the number of skip entries is quite small. Still, its size _is_ proportional
to a part of the index size, but to my knowledge, it is the only such thing.
(We could do away with skip lists since skipping entire blocks is itself quite
fast, but my guess is that they are too important for speeding up conjunction
queries to throw them away.)

There is one other critical aspect of merging that is necessary to make ACID
in Nakala work in the face of concurrency. The ACID part of this is described
in more detail in the transaction log and synchronization section below, but
suffice it to say, it is necessary for a merged segment to provide a way to map
doc IDs from their old segment to their new doc IDs in the merged segment. This
map must also include deleted document IDs for it to provide quick random
access lookups from the old ID namespace.

The map can be generated as we merge the userID -> docID FSTs, and it can be
streamed to disk. The map should have a header indicating the segment IDs that
have been merged (encoded as a length prefixed block of pairs of u64LEs, where
the first value in each pair is the segment ID and the second is the file
offset into the segment block mapping old to new doc IDs). Following that is a
sequence of blocks whose length is equal to the number of segments. Each block
contains a contiguous sequence of new doc IDs (as u32LEs), with the offset
of each being an old doc ID. (If an old doc ID has been deleted, then `0` is
encoded at that position, which serves as a sentinel value since we don't
permit a doc ID to be `0`.) Lookups are constant assuming you have a map from
segment ID to the segment block index in this file, which can be built from the
header and stored in heap memory very easily. At that point, you just need to
read the four bytes starting at `segment offset + 4*oldDocID` to get the new
doc ID (or the `0` sentinel if it has been deleted).

This map can be streamed to disk since we visit old doc IDs in a streaming
lexicographic fashion. As we allocate new doc IDs, they can be written to this
map. The main downside of this strategy is that while we visit doc IDs in order
across all merged segments, we will jump around each segment map in the file.
So we'll have to write a doc ID in one block, and then the next doc ID will
likely be written in another block. We could avoid seek time overhead using
things like `pwrite` (and the equivalent on Windows), but we still wind up
with a syscall for every doc ID, which is a non-starter. At that point, we can
either use a mutable memory map or simply create a buffer for each segment
block and write it out as we go using `pwrite` and such (which permits
efficiently using a single file descriptor). The mutable memory map would be
deliciously simple though, since the OS would handle the buffering for us.

This might seem like a lot of trouble to go through, but it's necessary for
ACID, since otherwise we might lose deletes. This could occur if a merge is
committed while an old Nakala handle deletes a document from one of the
segments that got merged. Once that delete gets committed, Nakala has to detect
that its ID is not in an active segment and instead map it to one that is in an
active segment. In fact, in theory, the mapping might need to go through
multiple merges if a Nakala handle has been open for long enough!

On top of all of that, creating this mapping at merge time is necessary
_anyway_. Namely, we must maintain a map at some point from old doc ID to new
doc ID, as we will use this to write the new posting list of the merged
segment. All of the old doc IDs must be mapped to their new doc ID in the
merged segment. The only thing ACID does is force us to push this mapping to
disk. Conveniently, this also means that we aren't tantilized into shoving this
mapping into heap memory. We can simply memory map the file and use it directly
as the doc ID map when writing the merged posting lists.

## Transaction log and synchronization

A Nakala index is formed of zero or more segments. Every search request must be
run against every segment, and results must be merged. Over time, as the
number of segments increases, they are merged together into bigger segments.
This architecture makes it possible to use limited memory to create small
segments at first, and gradually scale by merging segments as search latencies
start to slow down due to the number of segments. (Where merging an arbitrary
number of segments takes *close* to constant heap memory.)

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
segments. (Permitting multiple writers is just as much about figuring out
concurrency as it is setting policy. For example, Nakala doesn't require that
every document have a unique user ID. This is critical for permitting multiple
writers that don't block each other.)

The primary way in which we approach this problem is through a transaction
log. The transaction log is the single point of truth about which segments are
live at any particular point in time. Each entry includes a timestamp from
the system and each entry has a unique transaction ID that is monotonically
increasing. (N.B. The timestamp is only recorded for debugging/info purposes.
As of now, Nakala never relies on timestamps for any sort of correctness.)
Either many readers can be reading the log at the same time, or only one writer
can. To minimize contention, we ensure that all such actions are quick, and
in particular, do not require that every search first consult the transaction
log. (It may sound like we are giving up on our ideal of multiple concurrent
writers, but as we'll see, the lock time required here is very small and
mostly just amounts to some small book-keeping. Most of the writer's work is
done without a lock held on the transaction log and thus does not block other
writers in other threads _or_ other processes.)

The types of log entries are as follows:

* AddSegment(segmentID) - This indicates that a segment with the given ID has
  been added to the index. The segment ID can be used to derive the location of
  the segment in storage. Adding a new segment to the log fails if there
  already exists a live segment with the given ID (or an inactive segment that
  hasn't been removed from the transaction log yet).
* StartMerge(segmentID...) - This indicates that one or more segments have
  begun the process of being merged. The purpose of this log is to instruct
  other processes that some set of segments are involved in a merge. This helps
  reduce repetitive work. Adding a StartMerge log fails if it contains a
  segment ID that is involved in an active merge. Because of that failure
  condition, `StartMerge` also serves as _permission_ to start a merge
  involving a segment while being sure that it isn't just doing wasted work by
  racing with some other process.
* CancelMerge(segmentID...) - This indicates that a prior StartMerge has been
  stopped without finishing. The segment IDs in StartMerge remain active and
  are now eligible for merging again. This can be explicitly committed by an
  in-progress merge that has been cancelled for some reason. (Whether by the
  caller or a signal handler trying to clean up resources used by the process.)
* EndMerge(newSegmentID, oldSegmentID...) - This indicates that a merge has
  finished and a new segment has been added to the index. The old segment ID
  list must correspond to a previously recorded StartMerge log, and now point
  to segments that are no longer in the index. Adding an EndMerge log fails if
  there is no corresponding StartMerge log, or if the new segment ID already
  exists, or if any of the old segment IDs are no longer active or if a
  CancelMerge operation took place. In general, none of these failure
  conditions should ever occur (except perhaps CancelMerge in exceptional
  circumstances), so they are mostly just sanity checks.
* DeleteDocuments(segmentDocID...) - This indicates that one or more documents
  have been deleted from the index by recording a document's internal unique
  ID. (Which corresponds to the pair of segment ID and doc ID.) Searches must
  remove documents that are recorded in this log entry from their results
  before returning to that caller.

From this log, it should be clear how to compute the active set of segments
at any particular time. Since adding a new entry to this log should generally
be quite cheap, and to keep things simple, we require that there can only be
one writer to the log at any particular time. Each write will check that the
new log entry is valid to add and will fail if not, based on the state of the
log at the time of the write. We can enforce a single writer via file locking
(described in its own section below).

There are a few remaining hiccups here.

Firstly, when a StartMerge is added to the log, we have no guarantee that a
corresponding CancelMerge or EndMerge will be added. The merge process might
die unexpectedly for example. There are only two ways that I can see to solve
this.

1. One is to impose a timeout of sorts, where if a StartMerge hasn't completed
   within a designated interval, then a CancelMerge log entry is added to the
   log (or perhaps, equivalently, the StartMerge entry is simply removed).
   The main downside of a timeout is that if it's too short, we'll wind up
   doing a lot of work that winds up being wasted. And if the timeout is too
   long, then it could potentially cause merging to be delayed if a process
   dies unexpectedly. It's probably better to use a timeout that is too long,
   since in theory, a process dying unexpectedly should be a somewhat rare
   event.
2. When a StartMerge log entry is added, we use that log entry's ID to create a
   new file and then acquire an OS-native advisory lock on that file. When the
   merge process is done, it can unlock and remove the file. If the merge
   process dies unexpectedly, then the OS will automatically unlock the file.
   At that point, checking whether a merge has been cancelled or not can be
   determined by trying to acquire a lock on the file created. If the lock
   cannot be acquired, then at least we know that the process responsible for
   the merge is still running. It's plausible that we may still want to combine
   this with some kind of timeout though, in case the process itself is hung.

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
transaction log, or alternatively use the OS-native advisory lock technique.
(Since we cannot depend on a reader unmarking itself. It might be ungracefully
killed without the chance to unmark itself.)

The next section will discuss file locking, which is how we'll implement the
above procedure in a way that is safe for multiple simultaneous readers and
writers.

## File locking and index structure

It turns out that file locking is a total and complete pit of despair. There's
a lot of good material out there bemoaning how awful it is, but I found this to
be a good high level summary: https://gavv.github.io/articles/file-locks/

### Prelude

I'd like to quickly summarize the options available to us:

* Windows - It has its own locks via `LockFile` and they are non-advisory. But
  this is OK for the purposes of Nakala. The main thing is that they appear
  to work, support byte range locking, enjoy broad support and the OS will
  unlock them if the process holding the lock dies (which is critical for our
  strategy of dealing with ungracefully killed processes). On Windows at least,
  this seems like our tool of choice.
* POSIX fcntl locks - Broken beyond belief, but generally enjoy broad support.
  They do support byte range locking, but the main way in which they are broken
  is that they are only process scoped. So if we have multiple Nakala index
  handles in the same process but in different threads, then we can't use POSIX
  locks to synchronize between them. (SQLite manages to pull this off by using
  truly global state and managing a reference count itself. But I don't think
  it's worth digging into that rabbit hole.)
* BSD flock - This also appears to enjoy broad support, but doesn't support
  byte range locking. It does have shared/exclusive locking support though and
  may be used for synchronization between threads of the same process.
  Unfortunately, it does not appear to work correctly over NFS. Namely, it
  gets emulated via POSIX fcntl locks, which means they aren't appropriate for
  synchronization between threads in the same process. Using flock almost seems
  like a fool's errand, because you get easily lulled into a false sense of
  security only to get tripped up big time when using NFS.
* Linux open file description locks - Available on Linux only, support byte
  ranges, can be used to synchronize across threads and is claimed to work on
  NFS. This is the dream scenario, but it's Linux only.
* Create new file/directory locks - This is the old school "create a lock file"
  approach that is supposed to work everywhere. Although, it seems NFS (perhaps
  only older versions) have problems with the `O_EXCL` technique, which might
  make `mkdir` our best option. (`mkdir` is what SQLite uses.) The
  downside with this style of locking is that it either requires a timeout or
  being okay with manual user intervention to remove a lock file if a Nakala
  process dies unexpectedly. The other downside is that it can only be used for
  exclusive locking and of course has no byte range locking support.

(N.B. It is plausible that other synchronization primitives could be useful as
well. For example, shared memory pthread mutexes or shared memory semaphores.
However, these do not allow us to eliminate file locking in all cases and some
specific guarantees around what happens to them when their owning process dies
seem hard to come by. On top of all of that, these shared memory primitives
seem rarely used in other embedded databases, although I don't know exactly why
that is.)

My reaction to learning all of this was to think _really_ hard about what would
be the most minimal set of assumptions one could make in order to make Nakala
work well and fast. That is, the less we assume about the correctness and
availability of file locking, the simpler the implementation. I'm not sure if I
succeeded at that, but I did my best.

So, do we _really_ need locking? Yes, I think we do. A design goal of
Nakala that I am absolutely unwavering on is that it should _just work_ with
multiple simultaneous readers and writers from multiple processes or threads.
Having that freedom is intoxicating. It's like writing Rust code. You know the
compiler will generally prevent you from making the worst kinds of mistakes,
so you actually wind up feeling liberated. If we had no locking at all, then
it's possible we might be able to support one writer and many readers and we'd
have to tell the user that they are responsible for ensuring there is only one
writer. Which is annoying. We could add a tiny amount of locking for enforcing
the single writer rule, which would make this better. But a single writer means
1) you can't use multiple processes to index data, 2) you need to be a bit more
careful and 3) the Nakala API probably needs distinct `Writer` and `Reader`
types. (N.B. "single" writer in this context means that while a writer is open
in memory in one process, no other process can open a writer. If we did use
the simplistic locking scheme for this, it would correspond to very coarsely
grained locking.)

OK, so maybe we really do need locking, but could we get by with just old
school `O_EXCL` file locks? I actually think we could, but there are two major
problems with this approach as I see it:

1. This approach would necessite either timeouts to deal with locks that were
   not cleaned up by their creator, or otherwise require users to manually
   remove rogue lock files. (Most likely, we would make timeouts an optional
   feature.) Namely, with OS-native advisory locks, the locks are automatically
   unlocked if the process dies (or the corresponding file descriptor is closed
   for any reason).
2. They are really only good for exclusive locks. Which means we can't do
   something like, "lock these specific bytes in this segment tombstone file,"
   which would permit multiple processes to delete documents from the same
   segment simultaneously. Although, above we concluded that we should just
   avoid synchronization here altogether and use a full byte for each document.

At this point in time, it's hard to say with much certaintly how important (2)
is, but my instinct is that we really should reduce contention as much as
possible. (1) feels a bit more important, and in particular, advisory locks
feel like a pretty elegant solution to the problem of detecting when a reader
is inactive or when a merge has completed. And in particular, the alternative
to (1) is pretty grisly: timeouts and/or manual intervention if a process dies
at the wrong time.

OK, so maybe old school file locks aren't a great idea, but could we at least
pick _one_ method of file locking and use that? Unfortunately, I don't think
so. But answering that is trickier, so I think it would make sense to walk
through the individual use cases for file locking. To do that, I want to
describe my vision for Nakala's index structure in storage.

### Index structure

While Nakala is designed to be embeddable and therefore doesn't technically
require a file system to operate, its primary storage engine will of course be
one that works in a typical directory hierarchy on a file system. To that end,
we'll assume that particular implementation to discuss index structure, but
keep in mind that all of this is (hopefully) abstracted from Nakala's point of
view. Due to how complicated file locking is, it is necessary to think deeply
about that specific implementation, design the abstraction around that and hope
that it's sufficient for other use cases. (And if it isn't, we can evolve.)

OK, so here's what I'm thinking the index structure will look like. A Nakala
index will be a single directory with the following tree. Names in braces
indicate variables. Files that are transient or are otherwise only intended for
use in synchronization are in brackets. File names that end with a `?` imply
that they are only used on some platforms/environments.

  {index-name}/
    transaction.log
    [transaction.log.rollback]
    [transaction.log.lock]? (non-Linux/Windows)
    segment.{segmentID}.idx
    segment.{segmentID}.tomb
    [segment.{segmentID}.idmap] (ensuring ACID for deletes)
    merges/
      {transactionID}.active
    handles/
      {handleID}.active

Sadly, my hope to use the simplest possible file locking scheme kind of failed,
because the above structure requires the use of four different lock APIs.
Although, only two of them are used for any particular platform at a given
time:

* On Linux, its open file description locks are used in all cases. Explicit
  lock files are never needed.
* On Windows, its native `LockFileEx` APIs are used. As with Linux, explicit
  lock files are never needed.
* On all other platforms (including macOS), the old school `O_EXCL` lock file
  approach is used for `transaction.log.lock`, while POSIX `fcntl` locks are
  used for merges/{transactionID}.log` and `handles/{handleID.log}`.
  (N.B. Perhaps we could use POSIX `fcntl` locks on the transaction log by
  using the Nakala handle ID for the byte range offset?)

TODO: Revise this section. We really probably want `flock` on macOS for the
transaction log, so that we can support creating many readers simultaneously.
It should be possible to use feature-sniffing at runtime to determine if the
`flock` implementation is broken, and perhaps fall back to old school lock
files.

The last bullet point deserves a bit more explanation. POSIX locks are okay in
this case since only a single thread will ever hold locks on these files at
any given point in time, so we never need to support synchronization between
threads. In fact, the locks held on the merge and handle logs are specifically
only to indicate when the ongoing work represented by those logs is complete.
(Which occurs when the lock is released, whether gracefully or not.) Finally,
the use of old school lock files for the transaction log and for tombstones is
unfortunate, since it will limit concurrency to exactly one handle at any given
point in time. This is bad but perhaps not as bad as it sounds: the critical
section of each lock should be very small.

We'll go through the reasons for using old school lock files on
non-Linux/Windows by explaining why the other techniques don't work:

* POSIX fcntl locks don't work (for the transaction log or the tombstones)
  because they only permit process-to-process synchronization. But we want
  to be able to have multiple (possibly completely independent) handles in a
  process running simultaneously. Those handles need to synchronize somehow,
  and using the file system is the ideal way because we already need such
  a mechanism in place for process level synchronization. It is in theory
  possible to use POSIX locks here---after all, SQLite uses them---but it would
  require sharing global state that manages these locks across all Nakala
  handles in a given process. It's a big rabbit hole. And that alone doesn't
  solve the byte range aspect for POSIX locks, which we would want for writing
  tombstones.
* BSD flock could potentially be used to fight half the battle. If we were on a
  local file system in a non-Linux/Windows environment, then flock's
  shared/exclusive mechanism would come in handy with the transaction log.
  Because then we could use reader locks any time we just need to read the log.
  But flock doesn't really help us with synchronizing on the tombstone files
  since it doesn't support byte-range locking, which we need in order to be
  able to synchronize writing bits for individual doc IDs. (One alternative
  here would be to change the tombstone format to use one byte per doc ID
  rather than one bit, which I think should let us concurrently mutate bytes
  without synchronization since the file has a fixed size. Or so I think.)
  Moreover, BSD flock couldn't be used in either scenario when running on
  NFS since it degrades to POSIX locks and we just explained why those don't
  work. However, we could at least use flock on non-Linux/Windows and non-NFS
  environments for the transaction log.

The only other two mechanisms are specific to one platform (Windows and Linux).
So the only one left is old school lock files. Whether or not we pursue the
special cases mentioned above should depend on benchmarks motivating the extra
work. And also use cases. For example, how often are people putting an
extremely heavy load on a Nakala index on macOS?


### Index synchronization

In this section, we'll go over the particulars of synchronization in more
detail. Most of the synchronization isn't too difficult to describe:

* The transaction log can only have one writer at any point in time. It may
  have multiple readers.
* Determining whether a merge is complete or not requires synchronizing on the
  active merge files.
* Similarly, determining whether a handle has stopped reading the index or not
  requires synchronizing on the active handle files. We do also require that
  the `O_EXCL` trick works for creating active handle files, as this is used to
  guarantee that the ID of the handle is unique.
* Creating a segment file also requires the `O_EXCL` trick. Like active handle
  files, we use this to guarantee that we've generated a unique segment ID.
  Although in the case of segments, we also must check that the segment ID does
  not appear anywhere in the transaction log to avoid reusing a segment that is
  in the process of being removed due to compaction.
* Creating the index itself. Creating the index requires running a `mkdir`
  command successfully, which will guarantee that it is the only Nakala process
  writing to files inside of that directory. (If another Nakala process tries
  to create an index at the same directory at the same time, then they will
  race and one of them will fail.)

It sounds like there isn't much synchronization and that things should
therefore be pretty simple! But, it's the interaction between all of these that
breeds complexity. For example, there is quite a bit of interaction between the
transaction log and the active merge and handle files. Similarly, there is a
lot of logic for determining when to actually write tombstones since they are
first recorded in the log.

For clarity, it's worth pointing out some things that *don't* require
synchronization:

* Writing to a segment index is only ever done by a single thread. There are
  never multiple writers to a single segment. Nakala scales as Lucene does:
  multiple writers write to different segments. Only later are things
  (optionally) merged together.
* Reading from a segment index also doesn't require synchronization, as once a
  segment index has been written, it is never modified again.
* Reading/writing tombstones to segments needs no synchronization between the
  deletion status of each document is represented by a single byte. So writing
  a tombstone always means writing the byte `0xFF`. Even if two processes race,
  it doesn't matter which wins, so long as `0xFF` gets written.

The top-most form of synchronization is on the transaction log. What we
want here is a shared/exclusive lock, where reading acquires a shared lock
and writing acquires an exclusive lock. In all cases, the critical section
of one of these locks should be specifically designed to be as quick as
possible. (Some careful attention is paid to this a bit later for handling log
compaction.) I think the easiest way to start here is to explain what happens
when a handle to the index is opened. To be clear, a handle can be used for
either reading or writing. It can be long lived and cheaply shared between
threads inside a single process.

When a handle is first opened, it acquires a shared lock on the transaction
log. This lock should be capable of synchronizing with both other processes and
with other threads within the same process. (So that rules out POSIX locks.)
Once a shared lock is obtained, it makes note of the ID of the most recent
entry in the log. The handle then assigns itself an ID (say, from the current
timestamp) and attempts to create a new file with `O_EXCL` set with the name
`{logid}\_{handleid}`. If it doesn't succeed, then the handle should generate a
new ID and try again. If it does, then the handle should acquire an exclusive
lock on this file. The handle should then read the contents of the transaction
log into memory (explained in the next paragraph). Once done, the handle can
drop the shared lock on the transaction log. This technique makes it possible
to create many readers simultaneously with little to no contention.

Reading the transaction log into memory should result in roughly this
structure:

    struct Snapshot {
      // The unique transaction ID corresponding to this snapshot.
      id: TransactionID,
      // Handles to segments that are live according to the log.
      live_segments: Map<SegmentID, Segment>,
      // IDs of segments that are no longer live but still occur in the log.
      // This happens after a successful merge and before a log compaction.
      dead_segments: Set<SegmentID>,
      // A collection of ongoing segment merges. This information can be used
      // to smartly choose the next segments that should be merged.
      merges_in_progress: Map<TransactionID, Set<SegmentID>>,
      // All deleted document IDs, grouped by segment, that have been recorded
      // in the log up to this point.
      deleted_docs: Map<SegmentID, Set<DocID>>,
    }

This in-memory structure represents a "snapshot" of the index at a particular
point in time. So long as the handle for this snapshot remains open and is not
updated to a new snapshot, querying with this snapshot will continue to work
correctly and should always produce deterministic results, including with
respect to deleted.

Other than generating a segment ID, no synchronization is needed when indexing
new documents until they need to be committed. Namely, writing a new segment
can only happen within a single thread, and since the segment ID is unique, no
other process will be writing to it. We can guarantee uniqueness almost via the
same technique that we used to generate a handle ID, but that would make it
possible to use an ID of a segment that has been removed but is still recorded
in the log (e.g., log+index compaction is in progress or log+index compaction
failed part way through). In particular, a subsequent compaction run after
a crash might try to delete the old segment even though it had already been
removed from the file system, which could wind up deleting our new segment.
We can fix this by checking whether the generated ID for the segment exists
anywhere in the transaction log. If it doesn't, and since we've already claimed
the ID in storage (which means it cannot possibly be in a future log entry
that we haven't seen yet), it follows that even if it did correspond to an old
segment, it is no longer in the transaction log and therefore won't be tampered
with.

BREADCRUMBS: Touch on active merge and handle files. This should lead into the
next section about compaction, since compaction needs to know the position of
the oldest reader in the log. (But also can't compact past a `StartMerge`
without a corresponding `CancelMerge`/`FinishMerge`.

### Log and index compaction

BREADCRUMBS: This is Nakala's "checkpoint" process.


## Durability

BREADCRUMBS: https://danluu.com/file-consistency/ --- main gist is that we have
to checksum everything and ensure there is always a recovering strategy if we
crash during I/O. So everything needs to end in one last atomic action that
either makes things visible or indicates to a future Nakala handle that there
has been corruption and it should automatically recover.
