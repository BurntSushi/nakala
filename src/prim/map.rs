use std::collections::BTreeMap;
use std::convert::TryInto;
use std::sync::{Arc, Mutex};

use crate::error::{FormatContext, FormatError};
use crate::prim::ReadCursor;

/// The restricted set of values that a map can contain.
///
/// This may be expanded. It exists so that there is a defined closed set of
/// supported types. Each type has a tag that is included in the serialized
/// value, which makes it possible to deserialize a map without knowing the
/// type of its data a priori.
#[derive(Debug)]
enum Value {
    Bytes(Arc<[u8]>),
    String(Arc<str>),
    OffsetRange(usize, usize),
    Int(u64),
}

/// A writer for serializing a map from UTF-8 encoded string keys to a
/// restricted set of possible values to raw bytes.
#[derive(Debug)]
pub struct Writer {
    map: BTreeMap<String, Value>,
}

/// A reader for lazily deserializing a map from UTF-8 encoded string keys to a
/// restricted set of possible values.
///
/// "Lazy" in this context means that the inital act of reading a map from raw
/// bytes is a cheap constant time option. The actual entries in the map aren't
/// deserialized until they are explicitly requested. Once deserialized, they
/// are cached internally in a thread-safe manner.
///
/// While Nakala doesn't typically use big maps, Nakala does permit callers to
/// store their own entries, which means there is no strict bound. Moreover,
/// I have vague plans to add more entries to this map for debugging or metric
/// recording purposes. To that end, while some keys in a map may always be
/// necessary to deserialize in order to read a segment, it will never be the
/// case that all the keys will be. Since opening a segment should generally
/// be as cheap as possible, we follow through with that here by using a "lazy"
/// strategy.
#[derive(Debug)]
pub struct Reader<'a> {
    /// The bytes from which to read the map.
    cursor: ReadCursor<'a>,
    /// The position (into `cursor`) at which the encoding of the offsets of
    /// the map's entries starts. The offsets are a contiguous range of u64LEs
    /// that point to each key, where offsets are in the same order as the keys
    /// (which is lexicographic).
    ///
    /// This makes it possible to run a binary search on the map by using
    /// the offsets, since they are a fixed size.
    start: usize,
    /// The number of entries in this map.
    len: usize,
    /// Cached values that have been deserialized.
    cache: Arc<Mutex<BTreeMap<String, Value>>>,
}

impl<'a> Reader<'a> {
    /// Create a new map reader from the given cursor. The end of the cursor
    /// should correspond to the end of the serialized bytes for the map.
    pub fn new(cursor: ReadCursor<'a>) -> Result<Reader<'a>, FormatError> {
        // start (u64) + len (u64)
        cursor
            .set_pos_rev(8 + 8)
            .context("failed to position to end of map")?;
        let start = cursor
            .read_usize_le()
            .context("failed to read map start offset")?;
        let len =
            cursor.read_usize_le().context("failed to read map length")?;
        // Ensure that the starting position is correct.
        cursor.set_pos(start).context("invalid map start offset")?;
        Ok(Reader {
            cursor,
            start,
            len,
            cache: Arc::new(Mutex::new(BTreeMap::new())),
        })
    }

    /// Search for an entry in this map corresponding to the given key. If one
    /// does not exist, then None is returned. If the map is improperly
    /// encoded, then this returns an error.
    fn binary_search(
        &self,
        needle: &[u8],
    ) -> Result<Option<&[u8]>, FormatError> {
        let offsets = self.read_key_offsets()?;
        // since each offset is a u64LE
        assert_eq!(offsets.len() % 8, 0, "offsets must be a multiple of 8");

        let (mut left, mut right) = (0, (offsets.len() / 8) / 2);
        while left < right {
            let mid = (left + right) / 2;
            offsets.set_pos(mid * 8).context("failed to set map midpoint")?;
            let offset = offsets
                .read_usize_le()
                .context("failed to read map offset")?;
            let key = self.read_key_at(offset)?;
            if needle < key {
                right = mid;
            } else if needle > key {
                left = mid + 1;
            } else {
                return Ok(Some(key));
            }
        }
        Ok(None)
    }

    /// Read the map key associated with the entry starting at the given
    /// offset.
    fn read_key_at(&self, offset: usize) -> Result<&[u8], FormatError> {
        self.cursor.set_pos(offset).context("invalid map key offset")?;
        self.cursor.read_prefixed_bytes()
    }

    /// Read the map value starting at the current position. If there was a
    /// problem reading the value, then an error is returned.
    fn read_value(&self) -> Result<Value, FormatError> {
        let tag = self
            .cursor
            .read_u16_le()
            .context("failed to read map value type tag")?;
        match tag {
            1 => {
                let bytes = self
                    .cursor
                    .read_prefixed_bytes()
                    .context("failed to read map value 'bytes'")?;
                Ok(Value::Bytes(Arc::from(bytes)))
            }
            2 => {
                let string = self
                    .cursor
                    .read_prefixed_str()
                    .context("failed to read map value 'string'")?;
                Ok(Value::String(Arc::from(string)))
            }
            3 => {
                let start = self
                    .cursor
                    .read_usize_le()
                    .context("failed to read map value 'start of range'")?;
                let end = self
                    .cursor
                    .read_usize_le()
                    .context("failed to read map value 'end of range'")?;
                Ok(Value::OffsetRange(start, end))
            }
            4 => {
                let n = self
                    .cursor
                    .read_u64_le()
                    .context("failed to read map value 'int'")?;
                Ok(Value::Int(n))
            }
            unk => bail_format!("unknown map value type tag: {}", unk),
        }
    }

    /// Return a cursor corresponding to the sequence of u64LE offsets in this
    /// map. There is one offset for each key in the map, and the order of the
    /// offsets corresponds to the order of the map entries. Each offset points
    /// to the start of a map entry.
    ///
    /// The cursor returned is zeroed at the beginning of the offsets and ends
    /// at the end of the offsets.
    fn read_key_offsets(&self) -> Result<ReadCursor<'a>, FormatError> {
        // Will always succeed since we check this at construction.
        self.cursor.set_pos(self.start).unwrap();
        // Each offset is a u64LE and there are `self.len` of them.
        let offsets = self
            .cursor
            .read_range(0..(8 * self.len))
            .context("failed to read map key offsets")?
            .zero();
        // It'd be nicer if we could just return a &[u64] here, but that would
        // require dealing with safety by ensuring correct alignment. Instead
        // we just grin and bare dealing with &[u8] directly.
        Ok(offsets)
    }
}
