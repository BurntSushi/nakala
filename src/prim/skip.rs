use std::io::{self, Write};

use crate::error::{FormatContext, FormatError};
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
    /// Create a new reader for a skip list. The cursor given may begin
    /// anywhere, but must end immediately where the skip list ends.
    pub fn new(cursor: ReadCursor<'a>) -> Result<Reader<'a>, FormatError> {
        // start (u64) + levels (u64) + min (u32) + max (u32)
        cursor.set_pos_rev(8 + 8 + 4 + 4)?;
        let start = cursor
            .read_usize_le()
            .context("failed to read skip start offset")?;
        let levels = cursor
            .read_usize_le()
            .context("failed to read skip list levels")?;
        let min = cursor
            .read_u32_le()
            .context("failed to read min skip list value")?;
        let max = cursor
            .read_u32_le()
            .context("failed to read max skip list value")?;
        // Ensure that the starting position is correct.
        cursor.set_pos(start).context("invalid skip list start offset")?;
        Ok(Reader { cursor, start, levels, min, max })
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
        // Constructor of this reader checks that the start offset is valid,
        // so this will always exceed.
        self.cursor.set_pos(self.start).expect("already verified");
        loop {
            // val is the maximum value indexed by this parent and all of its
            // children ...
            let val = self.read_target_value()?;
            if target_value > val {
                // ... so if our target value is bigger than it, then we should
                // move to the next sibling ...
                if level == last_level {
                    // ... if we're on the last level, then the sibling is
                    // encoded after the offset, so read past the offset to
                    // get the sibling.
                    let _ = self.read_last_level_offset()?;
                    let next_sibling = self.read_next_sibling()?;
                    self.cursor.set_pos(next_sibling).context(
                        "failed to set skip position to next sibling",
                    )?;
                } else {
                    // ... otherwise, we know that there is no skip block
                    // beneath this one, so we should move to the next sibling,
                    // which is the next u32LE.
                    let next_sibling = self.read_next_sibling()?;
                    self.cursor.set_pos(next_sibling).context(
                        "failed to set skip position to next sibling",
                    )?;
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
        self.cursor.read_u32_le().context("failed to read skip target value")
    }

    /// Reads the next sibling offset encoded at the current position.
    ///
    /// If the sibling offset is invalid, then this returns an error.
    fn read_next_sibling(&self) -> Result<usize, FormatError> {
        let next_sibling = self
            .cursor
            .read_usize_le()
            .context("failed to read next sibling offset")?;
        // The reader should never move backwards as a result of skipping to
        // the next sibling in this level. We insert this check for better
        // error handling and to prevent potential infinite loops on a
        // malformed index.
        //
        // A next_sibling of 0 indicates that this is the final skip entry, and
        // so has no next sibling.
        if next_sibling != 0 && next_sibling >= self.cursor.pos() {
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
        self.cursor
            .read_usize_le()
            .context("failed to read last level skip offset")
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
#[derive(Clone, Debug)]
pub struct Writer {
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
    /// corresponding to the level's next sibling. The offset points at a
    /// location where a u32LE can be written, where that number should point
    /// to the beginning of its subsequent sibling.
    ///
    /// A "next" sibling doesn't exist for the last skip entries, in which
    /// case, a special offset of `0` is written.
    next_sibling_hole: Vec<Option<usize>>,
}

/// The actual implementation of writing a skip list.
///
/// This is a snapshot of some state produced by `Writer`, from which the
/// actual skip list can be written. It exists because the public `Writer`
/// type collects all of the skip entries, but the `WriterImp` is actually
/// responsible for serializing them to bytes once we're ready.
///
/// It also amortizes a few allocations, but this probably isn't
/// necessary, since they are proportional to the number of levels in the skip
/// list (which should always be very small). Otherwise, this streams the
/// skip entries to the underlying writer by writing them in reverse. Writing
/// them in reverse is necessary because each entry points to its next sibling
/// in sequence. If we didn't write them in reverse, then we'd either need
/// to buffer everything or require a seekable writer to backfill the sibling
/// positions.
///
/// TODO: Since the number of levels is fixed, and everything in the skip
/// list is of fixed size, it seems like it should be possible to compute the
/// sibling offsets without needing to explicitly store them... But I was not
/// smart enough to figure this out.
#[derive(Debug)]
struct WriterImp<'a, W> {
    wtr: &'a mut WriteCursor<W>,
    skips: &'a [Skip],
    skip_size: usize,
    levels: &'a [usize],
    min: u32,
    max: u32,
    next_sibling_hole: &'a mut [Option<usize>],
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
            skips: vec![],
            skip_size: skip_size,
            levels: vec![],
            min: 0,
            max: 0,
            next_sibling_hole: vec![],
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
        self.skips.clear();
        self.levels.clear();
        self.min = 0;
        self.max = 0;
        self.next_sibling_hole.clear();
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
        self.writer(wtr).write()?;
        self.clear();
        Ok(())
    }

    /// Create a skip writer that can write a skip list to any writer by first
    /// buffering the skip list.
    ///
    /// Buffering is necessary to backfill offsets to subsequent siblings
    /// within the same level.
    fn writer<'a, W: io::Write>(
        &'a mut self,
        wtr: &'a mut WriteCursor<W>,
    ) -> WriterImp<'a, W> {
        // This method does all the house-keeping work before we can start
        // writing our skip list.
        self.levels.clear();
        self.next_sibling_hole.clear();

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
            self.next_sibling_hole.push(None);
            level_length *= self.skip_size;
            num_skip_values /= self.skip_size;
        }
        // We built the levels backwards above, so right the ship. Level `0`
        // is the top-most level (where a sibling traversal at level 0 skips
        // the most values).
        self.levels.reverse();
        WriterImp {
            wtr,
            skips: &self.skips,
            skip_size: self.skip_size,
            levels: &self.levels,
            min: self.min,
            max: self.max,
            next_sibling_hole: &mut self.next_sibling_hole,
        }
    }
}

impl<'a, W: io::Write> WriterImp<'a, W> {
    /// Write this skip list to the given writer.
    fn write(&mut self) -> Result<(), FormatError> {
        // start marks the position at which the top-most skip entry is
        // encoded. We actually encode our skip entries backwards, since each
        // skip entry needs to point to its next sibling in sequence. If we
        // don't encode the entries backwards, then we'd need to buffer the
        // entire skip list in order to backfill the sibling offsets.
        let mut start = if self.levels.is_empty() { Some(0) } else { None };
        for (i, skip) in self.skips.iter().enumerate().rev() {
            for (level, &skip_every) in self.levels.iter().enumerate() {
                if i % skip_every != 0 {
                    continue;
                }
                if i == 0 && level == 0 {
                    start = Some(self.wtr.pos());
                }
                self.write_skip(i, level)?;
            }
        }
        self.wtr
            .write_usize_le(start.expect("first skip is always visited"))
            .context("failed to write skip start")?;
        self.wtr
            .write_usize_le(self.levels.len())
            .context("failed to write skip levels")?;
        self.wtr.write_u32_le(self.min).context("failed to write skip min")?;
        self.wtr.write_u32_le(self.max).context("failed to write skip min")?;
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

        let entry_start = self.wtr.pos();

        let skip_value = self.skip_value(skip_index, level);
        self.wtr
            .write_u32_le(skip_value)
            .context("failed to write skip value")?;
        // Only the last level actually associates skip values with offsets.
        if level == self.last_level() {
            self.wtr
                .write_usize_le(self.skips[skip_index].offset)
                .context("failed to write last level skip offset")?;
        }
        self.wtr
            .write_usize_le(self.next_sibling(level))
            .context("failed to write next sibling offset")?;

        self.next_sibling_hole[level] = Some(entry_start);
        Ok(())
    }

    /// Return the next sibling offset that has been recorded for the given
    /// level. If no sibling offset has been recorded, then `0` is returned.
    fn next_sibling(&self, level: usize) -> usize {
        self.next_sibling_hole[level].unwrap_or(0)
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
