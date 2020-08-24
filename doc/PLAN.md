# Plan

This is a "scratch" planning document that was primarily used during initial
development of Nakala. It keeps track of constraints, architecture, design
and nasty implementation details that I used while writing the code. The two
different use cases I had in mind driving everything were:

* To serve as the name index for imdb-rename, which just needs to index short
  name strings. Notably though, multiple names can be indexed for the same ID.
  This requires true relevance ranking via a TF-IDF scheme.
* Accelerating ripgrep searches on huge corpora. This should not use any
  relevance ranking at all, and requires yielding the complete set of document
  IDs.

Of course, I tried to keep generality in mind as well, as I've used IR systems
in many other contexts too. And tried to think about how others might use this
code, but didn't step too far outside my comfort zone.

## Table of Contents

* [Novelty](#novelty)
* [Things Nakala should NOT do](#things-nakala-should-not-do)
* [Things Nakala should do](#things-nakala-should-do)
* [Handling identifiers](#handling-identifiers)
* [Handling deletes](#handling-deletes)
* [Merging segments](#merging-segments)
* [Transaction log](#transaction-log)
* [Generating unique identifiers](#generating-unique-identifiers)
* [File locking and index structure](#file-locking-and-index-structure)
    * [File locking prelude](#file-locking-prelude)
    * [Index structure](#index-structure)
* [Index synchronization](#index-synchronization)
    * [Synchronization at a glance](#synchronization-at-a-glance)
    * [Motivating compaction](#motivating-compaction)
    * [Log and index compaction explained](#log-and-index-compaction-explained)
* [ACID](#acid)
    * [Atomicity](#atomicity)
    * [Consistency](#consistency)
    * [Isolation](#isolation)
        * [Stopping short of full serializability](#stopping-short-of-full-serializability)
    * [Durability](#durability)
        * [fsync](#fsync)
        * [On disk format](#on-disk-format)
        * [Recovery](#recovery)

## Novelty

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

Also, if push came to shove, we could stream the skip list to disk as well, and
then read it back once we're ready to serialize it. But I don't think it will
use a lot of memory in practice.

## Transaction log

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
  finished and a new segment has been added to the index. The old segment
  ID list must correspond to a previously recorded StartMerge log, and now
  point to segments that are no longer in the index. Adding an EndMerge log
  fails if there is no corresponding StartMerge log, or if the new segment ID
  already exists, or if any of the old segment IDs are no longer active or
  if a CancelMerge operation took place. In general, none of these failure
  conditions should ever occur (except perhaps CancelMerge in exceptional
  circumstances), so they are mostly just sanity checks. If any of the old
  segment IDs have been referenced by a DeleteDocuments entry since the
  corresponding StartMerge entry, then those IDs must be remapped and included
  as a preceding DeleteDocuments entry in the same transaction.
* DeleteDocuments(segmentDocID...) - This indicates that one or more documents
  have been deleted from the index by recording a document's internal unique
  ID. (Which corresponds to the pair of segment ID and doc ID.) Searches must
  remove documents that are recorded in this log entry from their results
  before returning to that caller. If any of the segments referenced in this
  entry are no longer live, then the IDs must be remapped to their new
  identifiers before the entry is committed.

From this log, it should be somewhat clear how to compute the active set of
segments at any particular time. Since adding a new entry to this log should
generally be quite cheap, and to keep things simple, we require that there can
only be one writer to the log at any particular time. Each write will check
that the new log entry is valid to add and will fail if not, based on the state
of the log at the time of the write. We can enforce a single writer via file
locking (described in its own section below). In addition to checks, some
entries (like DeleteDocuments and EndMerge) may require some updating if there
were log entries created since its snapshot and at the time of commit. (For
example, if a long running Nakala handle deletes a document in a segment that
was merged after the Nakala handle was opened.)

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


## Generating unique identifiers

Regretably, I found precious little material on how to generate unique
identifiers in the context of an embedded database. It's likely I'm not looking
in the right places, but search results were flooded with how to generate
unique IDs in a cluster environment. Many other search results were roughly
equivalent to "just use UUIDs." While UUIDs are in practice unique, they also
tend to use up more space than we need. Also, we are not in a cluster
environment and so it is not terribly burdensome to generate our own
identifiers, although it is subtle.

For motivation, we use unique identifiers for generating segments and handle
IDs. (Note that handle IDs and transaction IDs are not the same. Handle IDs are
assigned once a transaction is open, but a transaction ID is assigned only at
commit time. At commit time, an exclusive lock is required, and so generating
an ID in this context merely requires looking at the ID of the most recently
committed transaction.) It would also be nice to provide ID generation to users
of the Nakala library for arbitrary purposes, in case they don't have some
external source of identifiers, although that seems unlikely.

Obviously, generating unique identifiers requires some kind of disk
persistence, and this is why it's tricky. We need to ensure that we never
hand out a duplicate ID and we also need to ensure that we never corrupt our
ID generation state, even in the face of application crashes or abrupt power
failure. Since writing to disk and ensuring its safe even from abrupt power
failure is costly, we must also amortize this operation. That is, whenever we
read the next ID from disk, we will actually read a block of IDs. The process
can then freely allocate from that block until it runs out, and only then, does
the process need to go back to disk.

So here's what we do. The file for the ID generation will store the start of
the next block to allocate four times:

    u64LE           u64LE           u64LE           u64LE
    canonical       error           recovery        error

Whenever a new block is requested, we follow the process below:

1. Acquire an exclusive lock on the ID file.
2. Read the next block from the file and keep it in memory. This must check
   that all four u64LEs in the file are equivalent. If they aren't, then
   initiate a recovery process (explained below) and start again.
3. Compute the start of the next block, and store it in `next`.
4. Perform the following I/O operations:
    a. pwrite(file, [next, next], 0)
    b. fsync(file)
    c. pwrite(file, [next, next], 16)
    d. fsync(file)
5. Upon success, return the block read in (2), which is the block immediately
   preceding `next`.

The key here is that we write the ID in such a way where any interruption will
never leave us in a state that we 1) can't recover from and 2) will ever hand
out the same block twice. (2) is covered since we don't return the next block
until we have confirmed success from the OS that the next block has been made
durable on disk.

(1) is a bit harder to explain. The idea here is that we write the ID twice in
the file (first the canonical and then the recovery), but each time we write
it, we also include an error checking mechanism to ensure the write succeeded.
In this case, the error checking mechanism is to simply write the next block
again. When reading the file, the error check will fail if the `error` is not
equivalent to the `primary`.

Error checking can fail either when we read the `canonical` or the `recovery`
number. If error checking fails when reading `canonical`, then `recovery`
will never fail error checking because the only way for the `canonical` error
check to fail is if 4a or 4b failed and 4c did not run. Similarly, if error
checking fails when reading `recovery`, then `canonical` will never fail error
checking because the only way for the `recover` error check to fail is if 4c
or 4d failed, which implies that both 4a and 4b succeeded, since 4c can only
execute if 4b succeeded. The `fsync` call in 4b is critical here: it forces
that 4a will be made durabile in disk before ever attempting 4c. Therefore, it
is impossible for both the `canonical` and `recovery` numbers to fail the error
check.

This property leads to the following recovery mechanism (this process is only
invoked when one of the error checks fails):

1. If `primary` could not be read, then it must be possible to read `recover`
   such that `recover` refers to next block of IDs that has never been returned
   to the user (if it were, then writing `primary` would have succeeded). Thus,
   copy `recovery` to `primary` (as in (4) above) and restart ID generation.
2. If `recovery` could not be read, then it must be possible to read `primary`
   such that `primary` refers to the block _after_ the next block of IDs that
   should be returned to the user. Thus, subtract a block from `primary` and
   write that to the file as in (4) above and restart ID generation.

In this way, no matter where a crash or power failure occurs, we can always
generate unique IDs and be guaranteed to never lose our state.

The only thing remaining is to address the initial state of ID generation.
Generally speaking, the initial conditions are satisfied by virtue of having a
valid index. That is, there will be a (yet to be determined, likely the
presence of some file) indicator that an index has been fully created that
all other accesses will recognize. If this indicator doesn't exist, then the
index is not assumed to be created and thus no ID generation can occur. If the
indicator does exist, then by virtue of it existing, any ID generation file
will have been made durable to disk with a correct format.

If it is possible for arbitrary ID generators to exist, then the initial
conditions for an ID generator file can occur at any time. We address this by
detecting the special case of a zero file size. If the ID generator file as
zero size, then the handle accessing the file should acquire an exclusive lock
and initialize the ID generator (by writing 4 u64LEs as per (4) above). Even if
some other process initially created the ID generator, all ID generator
initialization is the same. The other process that created the file will simply
detect that the file has already been initialized once it acquired the
exclusive lock. (This is just an idea. Perhaps it would be simple to just
acquire an exclusive lock on the transaction log first, which would serialize
ID generator creation and would probably be much simpler.)

It's worth noting that this scheme is potentially more complex than necessary.
Namely, it's likely that writing such a small number of bytes (32 in this case)
will always be guaranteed to be atomic. That is, it will either completely
succeed or completely fail. Despite this guarantee being mentioned by various
folks with a lot of credibility, it's not clear where this guarantee actually
comes from, whether it really exists or whether it will always exist.
Therefore, we do not assume the existence of atomic writes under a small
number.


## File locking and index structure

It turns out that file locking is a total and complete pit of despair. There's
a lot of good material out there bemoaning how awful it is, but I found this to
be a good high level summary: https://gavv.github.io/articles/file-locks/

This section mostly serves as a bit of background reading before the next
section on index synchronization.

### File locking prelude

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


## Index synchronization

This section discuss how Nakala synchronizes multiple simultaneous readers and
writers in detail. This is effectively how Nakala provides atomicity and
isolation, and also partially how consistency is maintained. (Durability is
discussed later as a separate topic from synchronization.)

We first give a higher level overview of synchronization and then dive into
log+index compaction to explain it in more depth.

### Synchronization at a glance

In this section, we'll go over more of the particulars of synchronization, but
we'll remain at a somewhat high level. The next section on log+index compaction
will drive it home in even more detail, since the existence of compaction is
the main thing driving the need for synchronization in the first place.

At a low level, most of the synchronization Nakala needs to do isn't too
difficult to describe:

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
respect to deleted documents.

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

At this point, we've covered synchronization on the log at a very high level in
addition to the synchronization required to produce a segment file (which boils
down to generated a unique segment ID). We've also mostly addressed index
creation itself, since `mkdir` will return an error if the index already
exists. The only synchronization issues remaining to address at a high level is
how to determine when merges are complete or when Nakala handles have become
stale.

Both of these things use the same technique, which we alluded to above in our
discussion of file locking. Namely, both use a single file in storage to
indicate that either a merge has started or a Nakala handle has been opened. In
both cases, a lock is acquired on this file immediately. Once the merge has
completed or when the Nakala handle is closed, the lock on this file is
released and the file is removed. Of course, we cannot rely on graceful
destruction of resources, which is where the file lock comes in handy. If the
process holding the lock dies without cleaning itself up, the lock will
automatically be released. This makes it possible for log+index compaction to
later discover a stale handle or a merge that will never complete.

Detecting these cases is critical:

* We need to know if a merge will never complete because otherwise Nakala will
  refuse to allow the segments involved in that merge to participate in another
  merge. This avoids doing redundant work, and of course, allowing a segment to
  get merged into two other segments simultaneously would violate the integrity
  of the index. If we can determine that a merge has been stopped, then we can
  record this and free up the participating segments correctly without any
  wasted work. The only other alternative to this would be some form of timeout
  mechanism, which would permit us to preserve correctness, but might
  occasionally result in wasted work.
* Similarly, it is critical to be able to reliably know the _oldest_ active
  Nakala handle. This knowledge is what permits us to perform log+index
  compaction, since if we know that the oldest Nakala handle is not reading a
  segment that has since been merged into another segment, then we therefore
  know that no future Nakala handle will ever read that segment. Thus, the
  segment can be removed and the corresponding entry in the transaction log can
  be removed. If we couldn't reliably detect the oldest _active_ Nakala handle,
  then it would prevent us from compacting the index. As with merges, the only
  alternative to using locking is some kind of timeout mechanism, which would
  let us guarantee correctness, but at the expense of being either delayed in
  detecting stale handles, or worse, deleting segments that a handle is still
  trying to read from and is otherwise simply taking a long time.

There are of course still some failure modes here. For example, a caller might
keep a handle open for a long time (think about a network process) which will
prevent compaction. There isn't too much to be done about this, although it
seems likely that we'll want some kind of auto-update mechanism where the
handle updates itself to the latest transaction log entry. (Whether that's
based on time or on every action, I'm not sure.) Either way, this failure mode
isn't specific the locking strategy described above. It could happen with the
timeout strategy too. The difference is that the timeout strategy is even more
error prone. Avoiding the error proneness of the timeout strategy is especially
important for Nakala handles in an embedded database, since I anticipate that a
common access pattern will be something like:

* Start process.
* Open Nakala index.
* Get handle.
* Run search.
* Print results.
* Exit process.

In most runs, exiting the process should hopefully include closing the handle.
At minimum, we can do this inside a Rust destructor proactive, so the caller
doesn't even need to be counted on to do it. But, destructors aren't guaranteed
to run. The caller might call `process::exit(0)` for example. Or perhaps the
user will `^C` the process, and the caller might not have a signal handler
installed to properly clean up resources. Or any number of other scenarios. I
expect this sort of thing to happen a lot, so having a reliable means of
detecting this sort of case makes things a lot nicer. With a timeout mechanism,
all of these cases would necessarily devolve to "compaction must wait N minutes
until it is sure that the handle is stale, and then can compact past it." With
the locking strategy, staleness can be detected instantly.

That about sums up Nakala's need for synchronization at a reasonably high
level. The next section will discuss compaction, which is really at the core of
why Nakala needs synchronization at all.

### Motivating compaction

Compaction refers to the process of reducing the size of the transaction log,
reducing number of segments in storage, and, to a lesser extent, removing the
oldDocID -> newDocID map present in merged segments. The fundamental way in
which compaction works is pretty much how any other kind of garbage collection
works: it detects what is no longer being used and removes it. The main
problems in this process come from 1) reliably and correctly detecting that
something is no longer used and 2) synchronizing its removal.

Popping up a level, why do we need compaction? We need compaction because
otherwise the index would grow without bound. Specifically, due to how segment
files are written and the nature of inverted indices, it just isn't feasible to
update them in place. Moreover, even if we could devise such a mutable system
with similar read latency and storage overhead, we might not want to because of
the benefits of immutability. Namely, immutability _reduces_ the need for
synchronization since once a segment index is written, nothing else will _ever_
write to it. Therefore, no synchronization is ever needed to read from segment
files, which further implies that writers to an index will never block readers
to an index. This is a supremely desirable quality that dramatically improves
the scaling properties of a database.

The downside of immutability is that it takes up lots of space. Since you
aren't mutating an existing structure when writing a new document, you wind up
needing to build a whole new structure just to accomodate it. Moreover, if you
only write one document at a time, instead of batching them, then each such
document gets its own segment. This is gratuitously wasteful of space, but is
the price we pay for immutability. Therefore, in order to avoid wasting lots of
space, compaction allows us to reclaim it. _Additionally_, and just as
critically, if we grew the number of segments without any compaction, then
search requests would need to search ever more segments, which will keep
increasing the latency of search requests. Therefore, compaction is not only
about saving space, but also about improving search latency!

This technique of using immutability to avoid synchronization costs is
sometimes called Multi-Version Concurrency Control (MVCC) in the context
of database design. But the technique itself is really broader than that.
Another name for it is persistent data structures, which are commonly found in
functional languages due to their preference for immutability over mutability.
There is somewhat of an art to building these sorts of data structures because
you almost always have to balance time and space delicately. Of course, the
implementation details vary greatly depending on whether they are on-disk
structures or in-memory structures. (For in-memory structures, Chris Okasaki's
"Purely Functional Data Structures" is a gem.)

Databases like PostgreSQL, SQLite (in WAL mode) and LMBD (and many more!)
all do MVCC (or MVCC-like in the case of SQLite?), although their techniques
differ. SQLite is closer to the scheme described in this document. LMDB, being
quite a bit more specialized, has a really nice trick where it reuses blocks of
space on disk before writing new blocks, and avoids the need for a transaction
log altogether.

To complete the motivation, it is critical to note that compaction is a
critical reason for the complexity of synchronization in Nakala. If Nakala did
no compaction of its immutable segments, then Nakala could _almost_ get away
without any synchronization at all. Deleting documents could be done by writing
tombstones to segments directly (using one byte per docID, which therefore
wouldn't need synchronization), and new segments could be created via creating
files with the `O_EXCL` flag, which would guarantee uniqueness. To find all the
segments in the index at any point in time, Nakala would just need to list the
index directory contents and pick out the completed segment files. The only
problem with this approach is that we'd give up isolation with respect to
deleted documents. (It would permit a "read uncommitted" isolation level.) We
could get back to a "repeatable read" isolation level by re-introducing a
transaction log, but with lighter synchronization requirements:

* There would still need to be a shared/exclusive locking mechanism for the
  transaction log. We would need this to guarantee atomic additions to the
  file. (In theory, appending a small amount of data to a file is supposed to
  be atomic on its own, but there are conflicting reports on the Internet as to
  whether this is truly guaranteed or not.)
* When documents are deleted, they are recorded in the transaction log as
  described above. For efficiency, we could periodically write tombstones to
  segments (using 1 byte per doc ID, so that doesn't require synchronization
  either) and then record this act in the log. Nakala then wouldn't need to
  load all deleted doc IDs into memory---it could drop the ones that have been
  written to storage.
* We would need to track active Nakala handles, such that we only wrote
  tombstones for deletions that occur _before_ the oldest active reader.
  Otherwise, it would be possible to observe a delete mid-way through a
  transaction, leading to a phantom read.

So, with compaction well motivated, we can move on to describing the actual
process in more detail.

### Log and index compaction explained

This section attempts to describe the _full_ process of compaction, including
both the log and the index. To briefly recap, compaction is the process of
deleting things from storage that are no longer in use. This consists of three
things:

1. Segments that have been merged, and where no active Nakala handles exist
   that are using a snapshot of the index before the merge.
2. The ID map discussed above (in the section on merging) that is produced only
   for segments that are the result of a merge, once all segments that formed
   the merged segment have been removed in (1).
3. Any transaction log entry that occurs before the oldest active Nakala
   handle and is no longer needed. We'll address each possible entry for
   completeness:
     * `AddSegment` can be removed if it has been merged into another segment
       and that merge occurred before the oldest active handle's snapshot.
       Removing an `AddSegment` must coincide with removing the segment index
       from storage.
     * `StartMerge` can be removed if its corresponding `CancelMerge` or
       `EndMerge` exists and was committed before the oldest active handle's
       snapshot. If a `CancelMerge` was found, that it can be removed as well.
       Removing this entry does not correspond to removing anything from
       storage.
     * `CancelMerge` can be only be removed when its corresponding `StartMerge`
       entry is removed.
     * `EndMerge` can be removed in similar circumstances as `AddSegment`.
       Namely, if the segment it produced has itself been merged into another
       segment and that merge occurred before the oldest active handle's
       snapshot. Removing an `EndMerge` must coincide with removing the segment
       index from storage. Note that an `EndMerge` can include re-mapped
       deleted doc IDs that were committed after the corresponding
       `StartMerge`, however, if this segment has already been merged into
       another, then those deleted doc IDs were necessarily omitted from the
       merged segment and can thus be removed from the log. An `EndMerge` entry
       might also be _rewritten_ to omit its deleted doc IDs (if it has any)
       using the same procedure as `DeleteDocuments`.
     * `DeleteDocuments` can be removed if it occurs before the oldest active
       handle and once all of its doc IDs either reference segments that no
       longer exist or have been written as tombstones to their corresponding
       segment. (Note that writing tombstones occurs as part of this compaction
       process.)
   Note that the above means that a transaction entry `B` that occurs after `A`
   may actually be removed before `A` is, leaving "gaps" in the log. But this
   is okay.

As you can see, most of the complexity of compaction actually involves deciding
what to do with each log entry. One of the key complexities of the transaction
log is ensuring we don't drop deletes or allow them to refer to things that no
longer exist (at the time of commit). This can happen in a few different ways
if we aren't careful.

For example, let's say `A` and `B` refer to two distinct open transactions
(also called "handles"). Consider that `A` and `B` were opened at the same
transaction `x`. `A` deletes document `foo` while `B` merges the segment `foo`
was in into another segment. If `A` commits before `B`, then the merged segment
won't incorporate the delete, since the snapshot it used to perform the merge
occurred before the delete. Thus, when `B` is committed, the `EndMerge` entry
must include the doc IDs for deletes that occurred after `x` in any segment
that participated in the merge (where the IDs are remapped into the merged
segment's ID space). Conversely, if `B` commits before `A`, then `A`'s delete
will refer to a segment that is no longer live. Thus, subsequent snapshots will
not observe the delete since they will never look at the segment it was deleted
from because it isn't live. Thus, when `A` is committed, the `DeleteDocuments`
entry must have all of its IDs that refer to segments that are no longer live
remapped to their new identifiers before the entry is committed. Failing to
account for both of these cases results in a delete that is ignored.

The other thing worth touching on here is tombstoning. Tombstoning is the
responsibility of the compaction process itself, since tombstoning is how we
move deleted doc IDs out of the transaction log and into storage. This is
advantageous because checking if a document is deleted using storage means we
don't have to keep growing heap memory without bound in order to check whether
a document is deleted or not. Writing tombstones to segments makes it possible
to organize an on disk structure for efficient checks. The transaction log does
not, because the deleted doc IDs are scattered throughout different log entries
instead of organized in one place for each segment.

Tombstoning occurs whenever we examine a `DeleteDocuments` entry or an
`EndMerge` entry with remapped deleted doc IDs that occurs before the oldest
active handle. It looks like this:

* For each deleted doc ID, check the in-memory snapshot for whether it
  references a segment ID that is live. If it does, then write a tombstone for
  it.
* `fsync` the tombstones. (We talk about durability in more detail in the
  [durability](#durability) section.)
* Once all of the tombstones are written, either remove the `DeleteDocuments`
  entry or rewrite the `EndMerge` entry without its remapped delete doc IDs.

We note that this is safe because if the segment is no longer live in the
snapshot, then the segment must have been merged into another segment. Since
merges handle document deletes correctly (even in cases where the deletes occur
during a merge), it follows that the deleted doc ID is either recorded in a
subsequent `EndMerge` entry in its remapped form or absent entirely from the
merged segment (if the merge began after the delete was committed). We note
that the deleted doc ID cannot be tombstoned anywhere---even in its remapped
form---because that can only happen during compaction and compaction hasn't
visited that far in the log yet. In the case where the deleted doc ID refers to
a live segment, then our `fsync` on the corresponding tombstone file ensures it
is durable in storage.

Other than dealing with durability in the face of crashes (another tricky
complication), I believe that just about covers the guts of compaction. So now
we can more succinctly write out the sequence of steps:

* When compaction starts, acquire an exclusive lock to the transaction log
  and open a handle, such that it points to the most recent transaction.
* Discover the oldest active handle by listing the handles directory and
  sorting its contents by transaction ID in ascending order. Then attempt to
  acquire an exclusive lock on the first one. If it succeeds, remove the file
  and continue until all files have been remove or until acquiring a lock
  fails. The former case isn't technically possible, since compaction is itself
  a handle. In the latter case, the file at which acquiring a lock fails
  implies the handle is still active and is thus the oldest handle (since
  transaction IDs are monotonically increasing).
* Open a new transaction log for writing the compacted form.
* For each entry in the current log, use the entry-specific process described
  above to determine whether to leave it, remove it or rewrite it.
* If there is any action associated with removing or rewriting the entry (such
  as deleting segments from storage or writing tombstones), do that now.
* Once the action is complete and durable, either omit writing the entry to the
  new log or rewrite it, as appropriate.
* Upon completion, replace the old log with the new one. (Again, we are waving
  are hands a bit here, since the details of how exactly we do this are the
  domain of crash proof durability.)

There are some potential deviations we may make here, for performance reasons,
but they do complicate the procedure somewhat. (So we'll likely start without
them.) Namely, writing tombstones could be quite I/O heavy and expensive,
especially since we require the `fsync` and since it is random I/O, not
sequential I/O (to support a random access pattern). So if there are a lot of
tombstones to write, it is likely not appropriate to hold an exclusive lock on
the log file for that entire time.

We can limit the time we hold the lock by making compaction incremental.
Namely, when we come across an entry that requires writing a lot of tombstones,
then we can drop the exclusive lock and write the tombstones without any
synchronization whatsoever. We can record that we did this in memory (and if we
crash, it's okay to repeat the work) and reacquire the lock once the tombstones
are finished writing to disk. Then we simply restart the compaction process,
but skipping over the process of writing tombstones if it's required.

Of course, this introduces its own problems. For example, this could
potentially result in pathological behavior where compaction never completes if
deletes keep getting added during the time in which the exclusive lock is not
held. We could resolve this with a heuristic: if the compaction process has
been repeated more than N times, then stop dropping the lock and prioritize
completing compaction. Although, if deletes are truly streaming in that
quickly, then its likely we will be spending a lot of time in compaction, so
there will probably be other things to tweak.


## ACID

This section discusses how Nakala upholds atomicity, consistency, isolation and
durability (ACID). Many of these properties have already been discussed
above. This section tries to more specifically address them in order to make
the guarantees that Nakala provides clearer.


### Atomicity

Atomicity refers to the property that a change to Nakala's index either happens
completely or not at all. From the user's perspective, this means that if they
open a transaction, add some documents, delete some documents and then commit
it, then either _all_ of the writes and the deletes will be executed
successfully or _none_ of them will.

Nakala generally guarantees atomicy via its transaction log. Namely, once the
caller is ready to commit some writes, or deletes or a merge completes, then
Nakala will lock the transaction log and write all necessary entries for that
single transaction. That transaction includes a checksum, which means that if
writing the transaction fails part way through, then reading the transaction
can detect the corruption and drop it without sacrificing the integrity of the
rest of the index.

Similarly, if a transaction fails part way through before ever getting to the
commit phase, and since the transaction log is the single point of truth for
what is included in the snapshot of an index, it follows that no partial
results are ever observed.

### Consistency

Consistency refers to the property that every change made to a Nakala index is
consistent with the invariants guaranteed by Nakala. Nakala has it somewhat
easy on this front compared with other databases, especially relational
databases. Namely, Nakala doesn't really impose any constraints on its data,
and in particular, does not require that user IDs be unique. That is, a single
user ID may refer to multiple documents in the index. (They are deduplicated at
search time.)

Nakala _does_ protect itself against potential bugs by checking invariants in
the transaction log. For example, adding a new segment will fail if its ID
already refers to an existing segment, but this failure scenario should never
occur because our ID generation guarantees a unique ID. Similar constraints
exist for merges.

### Isolation

Isolation refers to the property that when concurrent transactions are
executed, the resulting state of the database would be no different than if
each of those transactions were executed serially, one after the other, in any
order.

In practice, guaranteeing such a strong level of isolation (usually referred to
as "serializable") is not something that all databases provide, or at least,
typically only provide through some sort of explicit configuration. (For
example, in PostgreSQL, one must explicitly enable the "serializable" isolation
level to get the strictest semantics. It is not enabled by default.)

At a high level, Nakala provides a "repeatable read" level of isolation, and
does not actually support full serializability. This means that dirty reads,
non-repeatable reads and phantom reads are not possible. To be clear, we use
the same
[definitions as PostgreSQL does](https://www.postgresql.org/docs/9.5/transaction-iso.html)
for those terms:

* A **dirty read** is a read of data written by a concurrent _uncommitted_
  transaction.
* A **non-repeatable read** occurs when data changes from read to read within
  the same transaction. That is, the data was modified by some other committed
  transaction, and that change has now been observed within a single search.
* A **phantom read** is like a non-repeatable read, but over the course of an
  entire transaction. It occurs when two distinct reads (e.g., two different
  searches) that are otherwise identical return two different results sets.
  That is, a single transaction sees data committed by a concurrent
  transaction.

Nakala prevents all of these things by using a snapshot of the index at the
time the transaction was opened. For the most part, the trickiest aspect of
this is in handling deletes. Much of this has already been addressed as part of
the sections above that explain deletes and the transaction log in more detail.
But in essence, deletes are first recorded in the transaction log and then
later moved to tombstones in their corresponding segment files once all open
transactions are using snapshots _after_ the deletion transaction. This means
that snapshots taken before the deletion transaction will never see those
deletes, because we are careful not to eagerly create tombstones for them.

Otherwise, the "repeatable read" isolation level is generally guaranteed by
virtue of the fact that segments are never mutated once they have been created.

With that said, a longer discussion about why Nakala stops here instead of
guaranteeing full serializability is worth having, because it helps us
understand the limitations of Nakala.

#### Stopping short of full serializability

Full serializability means that among concurrent transactions, executing them
serially (one after the other with no overlap) in every order would have an
equivalent outcome. This is a high bar to clear, and Nakala does not. Moreover,
there is no way to turn it on.

First, we should discuss how Nakala falls short. Deletes are the primary thing
that cause problems. If Nakala did not support deletes, then it would trivially
satisfy full serializability because documents would only ever be added (and
multiple documents with the same user ID are allowed).

Before explaining the example, it is important to briefly summarize how deletes
work. The normal way to delete a document is to delete all documents associated
with a particular user ID. The delete executes by searching the index for every
segmentID-docID pair in the index associated with that user ID, and then
records all of those pairs in the transaction log. (Only later, during log
compaction, are tombstones to each individual segment written.) The critical
part of this process is that user IDs themselves are never recorded. They are
only used to lookup the segmentID-docID pairs.

The simplest example of failing full serializability is when we have two
concurrent transactions `A` and `B`, where `A` creates a new segment containing
a document with a user ID `foo`, and where `B` deletes all documents containing
user ID `foo`. If both transactions are opened at the same time (i.e., they use
the same snapshot of the index), then `B`'s delete will only apply to documents
available in `B`'s snapshot of the index. That is, if you instead applied `A`,
committed, then started `B`, then `B`'s delete would include the document
associated with `foo` that was added in `A`. But if you reverse the order,
running `B` first, then the `foo` document that was added in `A` will not be
deleted.

Nakala will allow both of these transactions to commit without error, but full
serializability would require Nakala to detect this type of scenario and either
prevent one of the transactions from being committed or causing the results of
the transactions to be consistent with respect to ordering.

Detection is tricky, because deletes are only recorded via their
segmentID-docID pairs and _not_ their user IDs. So for example, if `A` were
committed first, then for `B` to detect that there is a new document that must
be deleted, then `B` would need to re-run the search for `foo` on the segment
introduced by `A`. Similarly, if `B` were committed first, then `A` would need
to do the same: re-run the search for `foo` on its segment and mark any
matching documents as deleted.

The only possible way to implement this, that I see, is to record both the user
IDs of deletes in addition to the segmentID-docID pairs, and then run the
detection logic mentioned above. While there is nothing impossible about this,
it can be quite expensive from a storage perspective and from a computation
perspective, and likely means holding an exclusive lock on the transaction log
for longer than we want. (Unless we do an incremental dance where we release
the lock, make progress, acquire the lock and check whether our progress is
enough.) But this makes an already complex thing even more complex. On the
bright side, once log compaction runs, we can drop the user IDs, since they are
only needed for resolving serializability conflicts.

It's likely that this is the sort of thing that seems daunting to try to
accomplish initially, but may not be too difficult to add once the full system
is built.

A similar version of this problem is discussed in the section above on
log+index compaction, but in that case, it was a delete being executed
concurrently with a merge, and the failure mode there is that a delete is
actually outright ignored, even though the document existed in the snapshot of
the transaction that executed the delete. This is a categorically different
kind of problem from the serializability problem we just described. The former
results in a delete that reports a successful deletion of a specific
segmentID-docID pair that winds up getting dropped. The latter never reports
any incorrect results; there's just an inconsistency with respect to
concurrency.

Other than the add/delete inconsistency, I do not think there are any other
serializability concerns in Nakala. Namely, as long as deletes aren't dropped
when a delete/merge occurs simultaneously, there are no serializability
problems because a merge never adds new documents. I believe (though not 100%
convinced) that a serializability problem requires that both an add and a
delete occur, and in particular, where the delete occurs via a user ID.

### Durability

Durability refers to the property that if Nakala reports that a commit is
successful, then any kind of crash (including abrupt power failure) will not
cause the loss of the data included in that commit. Durability does not,
however, guarantee that data that hasn't been committed or fails to commit is
retained. In fact, there is a strict correspondence: if a commit succeeds, then
that data is durable, and if a commit does not succeed then the data is not
part of the index---not even partially (which is also part of the atomicity
guarantee).

Generally speaking, there are three important aspects to ensuring durability:

1. The strategy employed to ensure that data is truly written to disk. Namely,
   because of caching, passing data to `write` to a file does *not* necessarily
   write it to disk. Instead, the data is likely put into a cache somewhere and
   written to disk later. The typical way to ensure durability is to call
   `fsync` on Unix platforms.
2. The design of the format on disk must make it possible to detect that there
   is some kind of corruption. Usually this means baking some kind of error
   detection (like a checksum) into the data on disk.
3. The recovery process that occurs when data on disk has been corrupted. That
   is, it is detected and the index is returned to a non-corrupt state.

We'll discuss each of these in more detail below, but Dan Luu's
[article on file consistency](https://danluu.com/file-consistency/)
is about as close to required reading as it gets. (And if you're doubly
curious, read the linked papers.)

#### fsync

First up is the `fsync` strategy. This is important to cover for two reasons.
Firstly, getting `fsync` correct is notoriously difficult, primarily because
getting it wrong can be hard to observe without proper testing infrastructure.
You tend to only see things not work when disaster occurs and disaster occurs
relatively infrequently. Secondly, many database systems either omit `fsync`
calls or provide a way to disable them through configuration. (Where the main
benefit of this is performance, since it yields control of I/O to the OS, which
presumably will schedule such things more efficiently.) For example, SQLite
provides such options with its `synchronous` pragma. When `fsync` calls are
omitted or reduced, then durability is typically guaranteed in the face of
application crashes, but not in the face of abrupt power loss.

As for Nakala, its default configuration will always be fully durable, even in
the face of power loss, although it may expose options like SQLite that permit
toggling it if performance is more important. The crucial bit here is that the
default configuration should be safe in all cases, even at the expense of
performance.

For those reading the Nakala code---while this isn't a perfect analogy---it
helps to think of `fsync` as a memory barrier, except it occurs with respect to
I/O. While strictly speaking it is something that forces data to be written to
disk, in terms of reasoning about concurrency, it helps to think about it as
something that imposes an _ordering_ between two I/O operations. Generally
speaking, two `write` calls may technically occur in any order, so if
durability (or atomicity) _requires_ one to be made durable before the other
may proceed, then an `fsync` call (or equivalent) is required.

#### On disk format

There are two primary error checking techniques we use in our on-disk format to
guarantee durability:

1. In very simple cases, where error checking only needs to occur on a single
   number, we simply write the same number twice. If we ended up with a partial
   write, then the first and second numbers won't be equivalent and the error
   checking will fail. (This technique is used in ID generation.)
2. In other cases, we use a CRC32 checksum. This is principally used in the
   transaction log.

Namely, the transaction log is a sequence of length-prefixed transactions,
where each transaction contains one or more entries. The data in each doesn't
matter too much for our purposes, but the end of each transaction includes a
checksum for the preceding bytes of that transaction. Thus, if a transaction
is only partially written, it will be possible to detect this via a checksum
failure.

It's worth noting that there are many places where checksums _aren't_ used, or
rather, where they are not necessary for durability. For example, while each
segment includes a checksum, those checksums are never used for durability.
They are only ever used for checking the integrity of the index, perhaps due to
some other form of corruption. The reason why we don't need checksums for
durability in segments is because segments are immutable. That is, before we
write a commit a transaction referencing a segment, we first make sure the
segment is durable, and only then do we start writing to the transaction log.
Since we never subsequently mutate it, the presence of the transaction itself
is a receipt that the segment is durable.

The index wide configuration file is handled similarly to the transaction log,
since it can be mutated. In this case though, there is only one checksum for
the entire configuration file, since it is generally small.

#### Recovery

Recovery refers to the process by which opening a Nakala index will "self heal"
itself in case there was an application crash or an abrupt power failure in the
middle of a write.

Most of the writing that Nakala does (segment files) is not subject to this
consideration, which simplifies things considerably. Namely, if a crash occurs
while writing a segment, then it follows that it was never committed and thus
there are no durability guarantees in play. (We do however omit details of how
to discover and remove incomplete segment files. This is "just" a space
optimization rather than a correctness issue.)

Thus, the main place where recovery matters is the transaction log. The
transaction log is the single source of truth for which segments are "live" in
the index, and thus, which segments must be consulted when executing a search.

Given the fact that each transaction is checksummed, it is trivial to determine
where the log has been corrupted. And thus, it is also possible to determine
which transactions are still valid: every transaction up to the corrupt one.
The corrupt transaction implies that it was not fully written, thus the commit
never reported success to the caller and thus, that transaction (and everything
after it) can "simply" be deleted from the log.

In practice, things are a touch more complicated than this, particularly with
respect to compaction. Namely, log compaction requires rewriting the log,
potentially in its entirely. This rewriting could fail at _any_ point, and
thus, rewriting the log in place could result in catastrophic data loss. There
are a couple different ways to approach it, but one simple way that I intend to
implement is:

1. When compaction starts, copy the existing log to `transaction.log.backup`.
2. Truncate `transaction.log` and rewrite it, in accordance with compaction.
3. Once compaction completes, make the transaction log durable and then finally
   remove the `transaction.log.backup`. This ordering is important, since the
   removal of the backup log indicates the completion of compaction. Thus, we
   must make sure the log is durable before removing the backup.

And then, our recovery process is the detection of `transaction.log.backup`. If
it exists when one opens a handle, then it necessarily implies that compaction
experienced a crash part way through. (If compaction is in progress, then it
holds an exclusive lock, and thus no other process will try to read the log.)
When this condition occurs, a recovery process that copies the
`transaction.log.backup` back to `transaction.log` must be complete. Once done,
things may resume as normal. (In this case, no data is lost. It's possible that
compaction has written some tombstones or removed some segments that are no
longer being used, but both of these things are idempotent. They will simply be
retried.)
