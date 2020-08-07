use std::io::{self, Write};

use crate::error::FormatError;
use crate::prim::{ReadCursor, WriteCursor};

/// A skip list reader.
///
/// A skip list reader fundamentally supports one option: it can efficiently
/// searched an ordered sequence of values. It does this by associating values
/// with offsets. The idea is that if you have a given value (say a doc ID),
/// then a skip list can very quickly produce an offset associated with that
/// value. In the case of Nakala, values are typically doc IDs and offsets are
/// positions in an index that correspond to the beginning of a block of doc
/// IDs. When running conjunction queries, even a single infrequent term can
/// make skipping through a frequent term very fast.
///
/// The `'a` lifetime parameter refers to the lifetime of the slice of bytes
/// that this skip reader reads from.
#[derive(Debug)]
pub struct Reader<'a> {
    /// The region of bytes from which to read skip entries.
    cursor: ReadCursor<'a>,
    /// The start offset (into `bytes`) of the first skip in level 0.
    start: usize,
    /// The number of levels in this skip list. A skip list with zero levels
    /// is an empty list.
    levels: usize,
    /// The minimum value indexed by this skip list.
    min: u32,
    /// The maximum value indexed by this skip list.
    max: u32,
}

impl<'a> Reader<'a> {
    /// Create a new reader for a skip list. The cursor given should be
    /// positioned at the beginning of the skip list, but it may continue
    /// beyond the skip list. That is, for a correctly encoded skip list, this
    /// cursor will never read beyond the skip list.
    pub fn new(cursor: ReadCursor<'a>) -> Result<Reader<'a>, FormatError> {
        let levels = cursor.read_usize_le().map_err(|e| {
            FormatError::err(e).context("failed to read skip list levels")
        })?;
        let min = cursor.read_u32_le().map_err(|e| {
            FormatError::err(e).context("failed to read min skip list value")
        })?;
        let max = cursor.read_u32_le().map_err(|e| {
            FormatError::err(e).context("failed to read max skip list value")
        })?;
        let start = cursor.pos();
        Ok(Reader { cursor, levels, min, max, start })
    }

    /// Return the number of levels in this skip list.
    pub fn levels(&self) -> usize {
        self.levels
    }

    /// Return the minimum value indexed by this skip list.
    pub fn min(&self) -> u32 {
        self.min
    }

    /// Return the maximum value indexed by this skip list.
    pub fn max(&self) -> u32 {
        self.max
    }

    /// Return the skip offset associated with the given value.
    ///
    /// If the given value is greater than any skip value indexed by this skip
    /// list, then this returns `None`. Note that an empty skip list always has
    /// zero levels and a max value of `0`.
    ///
    /// If there was a problem reading the skip list, then an error is
    /// returned. Errors only occur when the skip list is malformed.
    pub fn skip_to(
        &self,
        target_value: u32,
    ) -> Result<Option<usize>, FormatError> {
        if self.levels() == 0 || target_value > self.max() {
            return Ok(None);
        }

        let mut level = 0;
        let last_level = self.levels() - 1;
        // The starting position is derived from the position of the cursor
        // at construction, so it can never be invalid.
        self.cursor.set_pos(self.start).unwrap();
        loop {
            // val is the maximum value indexed by this parent and all of its
            // children ...
            let val = self.read_target_value()?;
            if target_value > val {
                // ... so if our target value is bigger than it, then we should
                // move to the next sibling ...
                if level == last_level {
                    // ... if we're on the last level, then the sibling is
                    // adjacent to this skip block, once we read (and discard)
                    // this block's offset.
                    let _ = self.read_last_level_offset()?;
                } else {
                    // ... otherwise, we know that there is no skip block
                    // beneath this one, so we should move to the next sibling,
                    // which is the next u32LE.
                    let next_sibling = self.read_next_sibling()?;
                    self.cursor.set_pos(next_sibling).map_err(|e| {
                        FormatError::err(e).context(
                            "failed to set skip position to next sibling",
                        )
                    })?;
                }
            } else {
                // ... otherwise, the skip block we want is either this one
                // (in case we are at the last level) or beneath this one.
                if level == last_level {
                    return Ok(Some(self.read_last_level_offset()?));
                } else {
                    // The skip block beneath this one is adjacent, so just
                    // read and discard the sibling offset to move to the next
                    // block.
                    let _ = self.read_next_sibling()?;
                    level += 1;
                }
            }
        }
    }

    /// Reads the target value at the beginning of this skip entry. The cursor
    /// must be positioned at the beginning of a skip entry.
    fn read_target_value(&self) -> Result<u32, FormatError> {
        self.cursor.read_u32_le().map_err(|e| {
            FormatError::err(e).context("failed to read skip target value")
        })
    }

    /// Reads the next sibling offset encoded at the current position.
    ///
    /// If the sibling offset is invalid, then this returns an error.
    fn read_next_sibling(&self) -> Result<usize, FormatError> {
        let next_sibling = self.cursor.read_usize_le().map_err(|e| {
            FormatError::err(e).context("failed to read next sibling offset")
        })?;
        // The reader should never move backwards as a result of skipping to
        // the next sibling in this level. We insert this check for better
        // error handling and to prevent potential infinite loops on a
        // malformed index.
        //
        // A next_sibling of 0 indicates that this is the final skip entry, and
        // so has no next sibling.
        if next_sibling != 0 && next_sibling < self.cursor.pos() {
            bail_format!(
                "invalid sibling offset {} at position {}",
                next_sibling,
                self.cursor.pos(),
            );
        }
        Ok(next_sibling)
    }

    /// Reads the offset encoded in the current block, which must be a block
    /// on the last level. The cursor should be positioned just after the
    /// target value for the last block.
    fn read_last_level_offset(&self) -> Result<usize, FormatError> {
        self.cursor.read_usize_le().map_err(|e| {
            FormatError::err(e)
                .context("failed to read last level skip offset")
        })
    }
}

/// A writer that can write skip lists to a Nakala index.
///
/// While the primary purpose of a skip list in Nakala is to make doc id list
/// traversal faster in some cases, this implementation is not tied to doc ids
/// and can be used with any ordered sequence of `u32` values.
///
/// Skip lists produced by this writer are not amenable to subsequent
/// modification.
///
/// Note that a `SkipWriter` requires buffering the skip list in memory.
/// Because of this, it may be beneficial to reuse skip lists to amortize the
/// cost of allocating said memory.
#[derive(Clone, Debug)]
pub struct Writer {
    /// An in memory buffer for writing the skip list.
    ///
    /// We use this in memory buffer because we need to be able to write
    /// sibling offsets in previously written skip entries.
    buf: WriteCursor<Vec<u8>>,
    /// A sequence of skip values and offsets associated with each value.
    ///
    /// For doc ids, each skip in this sequence corresponds to a *block* of
    /// doc ids. The value indexed in the skip list is the last doc id in that
    /// block, while the offset corresponds to the start of a doc id block that
    /// is or precedes the block containing the aforementioned last doc id.
    skips: Vec<Skip>,
    /// The minimum value indexed by this skip list. This value is derived from
    /// the values recorded in the `skips` sequence.
    min: u32,
    /// The maximum value indexed by this skip list. This value is derived from
    /// the values recorded in the `skips` sequence.
    max: u32,
    /// The number of skips in the lowest level. Each increasing level skips
    /// multiplicatively more entries.
    ///
    /// For example, if the skip_size is 4, then the lowest level will consist
    /// of entries for every 4 entries in the `skips` sequence. The second
    /// lowest level will consist of entries for every 16 entries. The next
    /// is 64 entries, and so on.
    skip_size: usize,
    /// A map where the keys are levels (`0` being the top most level that
    /// skips the most entries) and the values correspond to the number of
    /// entries that are skipped at this level.
    levels: Vec<usize>,
    /// A map where the keys are levels and the values are optional offsets
    /// into `buf` corresponding to the level's previous sibling. The offset
    /// points at a location where a u32LE can be written, where that number
    /// should point to the beginning of its subsequent sibling.
    ///
    /// If not previous sibling exists (yet) for a particular level, then there
    /// is no offset for it.
    ///
    /// Note that these offsets are only applicable for parent levels. The
    /// entries in the lowest level are written contiguous, so they do not need
    /// explicit sibling pointers.
    prev_sibling_level_hole: Vec<Option<usize>>,
}

/// An explicitly buffered writer for skip lists.
///
/// This is a snapshot of some state produced by `SkipWriter`, from which the
/// actual skip list can be written.
#[derive(Debug)]
struct BufWriter<'a> {
    buf: &'a mut WriteCursor<Vec<u8>>,
    skips: &'a [Skip],
    skip_size: usize,
    levels: &'a [usize],
    min: u32,
    max: u32,
    prev_sibling_level_hole: &'a mut [Option<usize>],
}

/// A single skip entry.
#[derive(Clone, Debug)]
struct Skip {
    /// The skip value.
    value: u32,
    /// The offset associated with this skip value.
    offset: usize,
}

impl Writer {
    /// Create a new skip writer with a default skip size.
    pub fn new() -> Writer {
        Writer::with_skip_size(4)
    }

    /// Create a new skip writer with the given skip size.
    pub fn with_skip_size(skip_size: usize) -> Writer {
        Writer {
            buf: WriteCursor::new(vec![]),
            skips: vec![],
            skip_size: skip_size,
            levels: vec![],
            min: 0,
            max: 0,
            prev_sibling_level_hole: vec![],
        }
    }

    /// Add a new value that can be skipped to.
    ///
    /// The offset given should correspond to a pointer to something that can
    /// be used to resolve to the given value. Generally, `value` corresponds
    /// to the last value in a compressed block of values and the `offset`
    /// corresponds to the beginning of that block.
    ///
    /// This panics if values are not added in increasing order.
    pub fn add(&mut self, value: u32, offset: usize) {
        assert!(value == 0 || value > self.max);
        if self.min == 0 {
            self.min = value;
        }
        self.max = value;
        self.skips.push(Skip { value, offset });
    }

    /// Reset this writer such that it can be reused.
    pub fn clear(&mut self) {
        self.buf.clear();
        self.skips.clear();
        self.levels.clear();
        self.min = 0;
        self.max = 0;
        self.prev_sibling_level_hole.clear();
    }

    /// Write this skip list to the given writer.
    ///
    /// If successful, the internal state of this writer will have been
    /// flushed, and is immediately ready for reuse to create additional
    /// skip lists.
    ///
    /// Note that `write` should be called once for every skip list produced.
    /// It is not possible to incrementally write a skip list.
    pub fn write<W: io::Write>(
        &mut self,
        wtr: &mut WriteCursor<W>,
    ) -> Result<(), FormatError> {
        self.buf_writer().write_to(wtr)?;
        self.clear();
        Ok(())
    }

    /// Create a skip writer that can write a skip list to any writer by first
    /// buffering the skip list.
    ///
    /// Buffering is necessary to backfill offsets to subsequent siblings
    /// within the same level.
    fn buf_writer(&mut self) -> BufWriter {
        // This method does all the house-keeping work before we can start
        // writing our skip list.
        self.buf.clear();
        self.levels.clear();
        self.prev_sibling_level_hole.clear();

        // The number of skip values is the number of remaining values provided
        // by the user to index by the skip list. The number of skip values
        // decreases by a factor of self.skip_size in each iteration.
        let mut num_skip_values = self.skips.len();
        // This is the number of skip values in each level. The following loop
        // builds levels backwards, such that level N (the first level built)
        // is the lowest level in the skip list. Each sibling traversal in a
        // level corresponds to skipping `level_length` skip values.
        let mut level_length = self.skip_size;

        num_skip_values /= self.skip_size;
        while num_skip_values > 0 {
            self.levels.push(level_length);
            self.prev_sibling_level_hole.push(None);
            level_length *= self.skip_size;
            num_skip_values /= self.skip_size;
        }
        // We built the levels backwards above, so right the ship. Level `0`
        // is the top-most level (where a sibling traversal at level 0 skips
        // the most values).
        self.levels.reverse();
        BufWriter {
            buf: &mut self.buf,
            skips: &self.skips,
            skip_size: self.skip_size,
            levels: &self.levels,
            min: self.min,
            max: self.max,
            prev_sibling_level_hole: &mut self.prev_sibling_level_hole,
        }
    }
}

impl<'a> BufWriter<'a> {
    /// Write this skip list to the given writer.
    fn write_to<W: io::Write>(
        &mut self,
        wtr: &mut WriteCursor<W>,
    ) -> Result<(), FormatError> {
        wtr.write_usize_le(self.levels.len())?;
        wtr.write_u32_le(self.min)?;
        wtr.write_u32_le(self.max)?;
        // If we didn't add any levels, then a skip list is not needed.
        if self.levels.is_empty() {
            return Ok(());
        }
        for (i, skip) in self.skips.iter().enumerate() {
            for (level, &skip_every) in self.levels.iter().enumerate() {
                if i % skip_every != 0 {
                    continue;
                }
                self.fill_prev_sibling_hole(wtr.pos(), level);
                self.write_skip(i, level)?;
            }
        }
        wtr.write_all(self.buf.get_ref()).map_err(|e| {
            FormatError::err(e).context("failed to write skip list to writer")
        })?;
        Ok(())
    }

    /// Write a single skip entry (given by `skip_index`) at a particular
    /// `level`.
    ///
    /// Note that the given skip index must be a multiple of the corresponding
    /// level's skip size.
    fn write_skip(
        &mut self,
        skip_index: usize,
        level: usize,
    ) -> Result<(), FormatError> {
        assert!(skip_index % self.skip_every(level) == 0);

        let skip_value = self.skip_value(skip_index, level);
        self.buf.write_u32_le(skip_value)?;
        if level == self.last_level() {
            self.buf.write_usize_le(self.skips[skip_index].offset)?;
        } else {
            self.prev_sibling_level_hole[level] = Some(self.buf.pos());
            self.buf.write_usize_le(0)?;
        }
        Ok(())
    }

    /// Set the sibling offset for the previous sibling of the given level.
    ///
    /// The offset set here permits quickly skipping along the siblings of a
    /// level.
    ///
    /// `wtr_position` should be the position of the underlying writer, which
    /// is used to determine the offset to write.
    fn fill_prev_sibling_hole(&mut self, wtr_position: usize, level: usize) {
        if level >= self.last_level() {
            return;
        }
        if let Some(at) = self.prev_sibling_level_hole[level] {
            let pos = wtr_position + self.buf.pos();
            (&mut self.buf.get_mut()[at..])
                .write_all(&pos.to_le_bytes())
                .unwrap();
        }
    }

    /// Return the skip value for the given skip (`skip_index`) and `level`.
    ///
    /// The skip value corresponds to he maximum skip value in all children
    /// of this level. Readers will move to the next sibling in the same level
    /// when their value is greater than a particular skip value.
    fn skip_value(&self, skip_index: usize, level: usize) -> u32 {
        let skip_every = self.skip_every(level);
        if skip_index + skip_every >= self.skips.len() {
            // This case happens when there is no subsequent sibling of this
            // level. In this case, this is the last stop, so the target value
            // should be the maximum to make the reader's job easier.
            self.max
        } else if level == self.last_level() {
            self.skips[skip_index].value
        } else {
            // Parent skips would like to index a value equivalent to the
            // maximum value of its children. This maximum value corresponds to
            // the last skip before the next sibling in this level.
            let last_in_level = skip_index + skip_every - self.skip_size;
            match self.skips.get(last_in_level) {
                None => self.max,
                Some(ref skip) => skip.value,
            }
        }
    }

    /// Return the skip size for the given level. This size corresponds to the
    /// number of skip values that can be skipped by traversing siblings at the
    /// given level.
    fn skip_every(&self, level: usize) -> usize {
        self.levels[level]
    }

    /// Return the last level in this skip list.
    ///
    /// The last level is special in a couple ways because it doesn't have any
    /// children, and instead points to the actual offsets associated with a
    /// particular skip value. Additionally, skips in the last level are
    /// written contiguously, and therefore do not have any sibling offsets.
    fn last_level(&self) -> usize {
        self.levels.len() - 1
    }
}

#[cfg(test)]
mod tests {
    use super::{Reader, Writer};
    use crate::prim::{ReadCursor, WriteCursor};

    fn serialize(wtr: &mut Writer) -> Vec<u8> {
        let mut cursor = WriteCursor::new(vec![]);
        wtr.write(&mut cursor).unwrap();
        cursor.into_inner()
    }

    fn deserialize(bytes: &[u8]) -> Reader<'_> {
        Reader::new(ReadCursor::new(bytes)).unwrap()
    }

    fn skip_to(rdr: &Reader<'_>, value: u32) -> Option<usize> {
        rdr.skip_to(value).unwrap()
    }

    #[test]
    fn basic() {
        let mut wtr = Writer::new();
        for i in 0..100 {
            wtr.add((i + 1) * 10, i as usize);
        }
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 3);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 1000);

        assert_eq!(skip_to(&rdr, 0), Some(0));
        assert_eq!(skip_to(&rdr, 1), Some(0));
        assert_eq!(skip_to(&rdr, 2), Some(0));
        assert_eq!(skip_to(&rdr, 10), Some(0));
        assert_eq!(skip_to(&rdr, 11), Some(4));
        assert_eq!(skip_to(&rdr, 19), Some(4));
        assert_eq!(skip_to(&rdr, 20), Some(4));
        assert_eq!(skip_to(&rdr, 21), Some(4));
        assert_eq!(skip_to(&rdr, 50), Some(4));
        assert_eq!(skip_to(&rdr, 51), Some(8));
        assert_eq!(skip_to(&rdr, 90), Some(8));
        assert_eq!(skip_to(&rdr, 91), Some(12));
        assert_eq!(skip_to(&rdr, 130), Some(12));
        assert_eq!(skip_to(&rdr, 131), Some(16));
        assert_eq!(skip_to(&rdr, 610), Some(60));
        assert_eq!(skip_to(&rdr, 611), Some(64));
        assert_eq!(skip_to(&rdr, 930), Some(92));
        assert_eq!(skip_to(&rdr, 931), Some(96));
        assert_eq!(skip_to(&rdr, 970), Some(96));
        assert_eq!(skip_to(&rdr, 971), Some(96));
        assert_eq!(skip_to(&rdr, 1000), Some(96));
        assert_eq!(skip_to(&rdr, 1001), None);
    }

    #[test]
    fn empty() {
        let mut wtr = Writer::with_skip_size(2);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 0);
        assert_eq!(rdr.min(), 0);
        assert_eq!(rdr.max(), 0);

        assert_eq!(skip_to(&rdr, 0), None);
        assert_eq!(skip_to(&rdr, 1), None);
    }

    #[test]
    fn no_levels_non_empty() {
        let mut wtr = Writer::with_skip_size(2);
        wtr.add(10, 0);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 0);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 10);

        assert_eq!(skip_to(&rdr, 0), None);
        assert_eq!(skip_to(&rdr, 1), None);
    }

    #[test]
    fn one_level1() {
        let mut wtr = Writer::with_skip_size(2);
        wtr.add(10, 0);
        wtr.add(20, 1);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 1);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 20);

        assert_eq!(skip_to(&rdr, 0), Some(0));
        assert_eq!(skip_to(&rdr, 10), Some(0));
        assert_eq!(skip_to(&rdr, 11), Some(0));
        assert_eq!(skip_to(&rdr, 20), Some(0));
        assert_eq!(skip_to(&rdr, 21), None);
    }

    #[test]
    fn one_level2() {
        let mut wtr = Writer::with_skip_size(2);
        wtr.add(10, 0);
        wtr.add(20, 1);
        wtr.add(30, 2);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 1);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 30);

        assert_eq!(skip_to(&rdr, 0), Some(0));
        assert_eq!(skip_to(&rdr, 10), Some(0));
        assert_eq!(skip_to(&rdr, 11), Some(2));
        assert_eq!(skip_to(&rdr, 20), Some(2));
        assert_eq!(skip_to(&rdr, 21), Some(2));
        assert_eq!(skip_to(&rdr, 30), Some(2));
        assert_eq!(skip_to(&rdr, 31), None);
    }

    #[test]
    fn two_level1() {
        let mut wtr = Writer::with_skip_size(2);
        wtr.add(10, 0);
        wtr.add(20, 1);
        wtr.add(30, 2);
        wtr.add(40, 3);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 2);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 40);

        assert_eq!(skip_to(&rdr, 0), Some(0));
        assert_eq!(skip_to(&rdr, 10), Some(0));
        assert_eq!(skip_to(&rdr, 11), Some(2));
        assert_eq!(skip_to(&rdr, 20), Some(2));
        assert_eq!(skip_to(&rdr, 21), Some(2));
        assert_eq!(skip_to(&rdr, 30), Some(2));
        assert_eq!(skip_to(&rdr, 31), Some(2));
        assert_eq!(skip_to(&rdr, 40), Some(2));
        assert_eq!(skip_to(&rdr, 41), None);
    }

    #[test]
    fn two_level2() {
        let mut wtr = Writer::with_skip_size(2);
        wtr.add(10, 0);
        wtr.add(20, 1);
        wtr.add(30, 2);
        wtr.add(40, 3);
        wtr.add(50, 4);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 2);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 50);

        assert_eq!(skip_to(&rdr, 0), Some(0));
        assert_eq!(skip_to(&rdr, 10), Some(0));
        assert_eq!(skip_to(&rdr, 11), Some(2));
        assert_eq!(skip_to(&rdr, 20), Some(2));
        assert_eq!(skip_to(&rdr, 21), Some(2));
        assert_eq!(skip_to(&rdr, 30), Some(2));
        assert_eq!(skip_to(&rdr, 31), Some(4));
        assert_eq!(skip_to(&rdr, 40), Some(4));
        assert_eq!(skip_to(&rdr, 41), Some(4));
        assert_eq!(skip_to(&rdr, 50), Some(4));
        assert_eq!(skip_to(&rdr, 51), None);
    }

    #[test]
    fn repeated() {
        let mut wtr = Writer::with_skip_size(2);
        wtr.add(10, 0);
        wtr.add(20, 1);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 1);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 20);

        assert_eq!(skip_to(&rdr, 0), Some(0));
        assert_eq!(skip_to(&rdr, 10), Some(0));
        assert_eq!(skip_to(&rdr, 11), Some(0));
        assert_eq!(skip_to(&rdr, 20), Some(0));
        assert_eq!(skip_to(&rdr, 21), None);

        // The call to write in `as_reader` above should have reset the writer.
        // So just repeat the same as above.
        wtr.add(10, 0);
        wtr.add(20, 1);
        let bytes = serialize(&mut wtr);
        let rdr = deserialize(&bytes);

        assert_eq!(rdr.levels(), 1);
        assert_eq!(rdr.min(), 10);
        assert_eq!(rdr.max(), 20);

        assert_eq!(skip_to(&rdr, 0), Some(0));
        assert_eq!(skip_to(&rdr, 10), Some(0));
        assert_eq!(skip_to(&rdr, 11), Some(0));
        assert_eq!(skip_to(&rdr, 20), Some(0));
        assert_eq!(skip_to(&rdr, 21), None);
    }
}
