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
* Nakala will permit associating an arbitrary `u64` with any document. The
  `u64` must not have any constraints imposed on it. This implies that Nakala
  will maintain a mapping between user supplied IDs and internal doc IDs.
  (Where doc IDs are scoped to a single segment, are u32s and are monotonically
  increasing.)
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

