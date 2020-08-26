use std::cell::Cell;
use std::convert::TryInto;
use std::ops::Range;

use fst::raw::Fst;

use crate::error::{FormatContext, FormatError};
use crate::prim::varint;

/// A cursor wraps a slice of bytes and associates it with a mutable position
/// into that slice.
///
/// The cursor then provides convenience methods for reading from that byte
/// slice while automatically incrementing the position. In particular, a
/// cursor provides panic safe routines for deserializing structures from
/// a byte slice. That is, callers may freely provide positions and inputs
/// generated from untrusted external data and rely on the cursor to return an
/// error instead of panicking if the position or input is invalid.
///
/// If necessary, callers can move the cursor backwards to a previous position.
#[derive(Clone, Debug)]
pub struct Cursor<B> {
    bytes: B,
    pos: Cell<usize>,
}

impl<B: AsRef<[u8]>> Cursor<B> {
    /// Create a new cursor for a byte slice.
    pub fn new(bytes: B) -> Cursor<B> {
        Cursor { bytes, pos: Cell::new(0) }
    }

    /// Return the remaining bytes in this cursor. That is, all bytes in this
    /// cursor starting from the cursor's current position.
    pub fn as_slice(&self) -> &[u8] {
        &self.bytes.as_ref()[self.pos()..]
    }

    /// Returns the number of bytes remaining in this cursor.
    pub fn len(&self) -> usize {
        self.as_slice().len()
    }

    /// Returns true if and only if this cursor has been completely consumed.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Return the current position of this cursor.
    pub fn pos(&self) -> usize {
        self.pos.get()
    }

    /// Explicitly set the absolute position of this cursor.
    ///
    /// Note that this is only necessary for special cases. Namely, all of the
    /// `read` methods on a cursor will automatically increment the position
    /// for you.
    ///
    /// This returns an error if the given position is out of bounds. It will
    /// never panic.
    pub fn set_pos(&self, pos: usize) -> Result<(), FormatError> {
        if pos > self.bytes.as_ref().len() {
            bail_format!("expected pos <= {}, but got {}", self.len(), pos);
        }
        self.pos.set(pos);
        Ok(())
    }

    /// Explicitly set the absolute position of this cursor, counting backwards
    /// from the end of the underlying bytes.
    ///
    /// This returns an error if the given position is out of bounds. It will
    /// never panic.
    pub fn set_pos_rev(&self, rpos: usize) -> Result<(), FormatError> {
        let len = self.bytes.as_ref().len();
        match len.checked_sub(rpos) {
            Some(pos) => self.set_pos(pos),
            None => bail_format!("expected rpos <= {}, but got {}", len, rpos),
        }
    }

    /// Increment this cursor's position by the number of bytes given.
    ///
    /// If this would set the position of the cursor beyond the end of the
    /// underlying bytes, then this returns an error.
    pub fn inc(&self, amount: usize) -> Result<(), FormatError> {
        match self.pos().checked_add(amount) {
            Some(new) => self.set_pos(new),
            None => bail_format!("invalid amount increase ({})", amount),
        }
    }

    /// Return a new cursor whose position is always set to `0` such that `0`
    /// corresponds to the current position of this cursor.
    pub fn zero(&self) -> Cursor<&[u8]> {
        Cursor { bytes: &self.as_slice()[self.pos()..], pos: Cell::new(0) }
    }

    /// Return a sub-range of this cursor.
    ///
    /// The range given should be in terms of the unconsumed bytes in this
    /// cursor. The position of this cursor remains unchanged and independent
    /// of the position of the cursor returned. Initially, both this cursor and
    /// the returned cursor will have the same absolute position. The only
    /// difference between the two cursors is that the one returned may have
    /// fewer bytes than this one, depending on the range.
    ///
    /// To consume a range of bytes, use `read_range`.
    ///
    /// If the given range is not valid, then this returns an error.
    pub fn range(
        &self,
        r: Range<usize>,
    ) -> Result<Cursor<&[u8]>, FormatError> {
        if self.as_slice().get(r.clone()).is_none() {
            bail_format!("invalid cursor range {:?}", r);
        }
        Ok(Cursor {
            bytes: &self.as_slice()[..r.end],
            pos: Cell::new(self.pos() + r.start),
        })
    }

    /// Like range, but advance the position of this cursor past the range
    /// returned. The cursor returned will be positioned at the beginning of
    /// the range (i.e., the position before consuming the range).
    pub fn read_range(
        &self,
        r: Range<usize>,
    ) -> Result<Cursor<&[u8]>, FormatError> {
        let cursor = self.range(r)?;
        self.inc(cursor.len())?;
        Ok(cursor)
    }

    /// Read a slice of bytes from this cursor corresponding to the given
    /// range.
    ///
    /// (This is like `read_range`, except it returns raw bytes instead of
    /// another cursor.)
    pub fn read_slice(&self, r: Range<usize>) -> Result<&[u8], FormatError> {
        let bytes = match self.as_slice().get(r.clone()) {
            None => bail_format!("invalid slice range {:?}", r),
            Some(bytes) => bytes,
        };
        self.inc(bytes.len())?;
        Ok(bytes)
    }

    /// Read a 16-bit little endian integer.
    pub fn read_u16_le(&self) -> Result<u16, FormatError> {
        // OK since read_slice guarantees exactly 2 bytes or an error.
        let bytes: [u8; 2] = self.read_slice(0..2)?.try_into().unwrap();
        Ok(u16::from_le_bytes(bytes))
    }

    /// Read a 32-bit little endian integer.
    pub fn read_u32_le(&self) -> Result<u32, FormatError> {
        // OK since read_slice guarantees exactly 4 bytes or an error.
        let bytes: [u8; 4] = self.read_slice(0..4)?.try_into().unwrap();
        Ok(u32::from_le_bytes(bytes))
    }

    /// Read a 64-bit little endian integer.
    pub fn read_u64_le(&self) -> Result<u64, FormatError> {
        // OK since read_slice guarantees exactly 8 bytes or an error.
        let bytes: [u8; 8] = self.read_slice(0..8)?.try_into().unwrap();
        Ok(u64::from_le_bytes(bytes))
    }

    /// Read a 64-bit little endian integer as a usize. If the integer doesn't
    /// fit in a usize, then return an error.
    pub fn read_usize_le(&self) -> Result<usize, FormatError> {
        // OK since read_slice guarantees exactly 8 bytes or an error.
        let bytes: [u8; 8] = self.read_slice(0..8)?.try_into().unwrap();
        to_usize(u64::from_le_bytes(bytes))
    }

    /// Read a variable length u64.
    ///
    /// The format of the integer matches Google's protocol buffer varint.
    ///
    /// See: https://developers.google.com/protocol-buffers/docs/encoding#varints
    pub fn read_varu64(&self) -> Result<u64, FormatError> {
        let (n, nread) = varint::read_u64(self.as_slice())?;
        self.inc(nread)?;
        Ok(n)
    }

    /// Read a variable length usize.
    ///
    /// The format of the integer matches Google's protocol buffer varint.
    ///
    /// See: https://developers.google.com/protocol-buffers/docs/encoding#varints
    pub fn read_varusize(&self) -> Result<usize, FormatError> {
        self.read_varu64().and_then(to_usize)
    }

    /// Read a length prefixed byte string. The length prefix should be a
    /// varint.
    pub fn read_prefixed_bytes(&self) -> Result<&[u8], FormatError> {
        let len = self.read_varusize()?;
        self.read_slice(0..len)
    }

    /// Read a length prefixed UTF-8 encoded string, where the length prefix is
    /// a varint. This returns an error if the bytes are invalid UTF-8.
    pub fn read_prefixed_str(&self) -> Result<&str, FormatError> {
        let bytes = self.read_prefixed_bytes()?;
        std::str::from_utf8(bytes)
            .context("invalid UTF-8 in fixed length string")
    }

    /// Interpret the remaining bytes in this cursor as an FST and return it.
    pub fn read_fst(&self) -> Result<Fst<&[u8]>, FormatError> {
        let fst = Fst::new(self.as_slice()).context("could not read FST")?;
        self.inc(self.as_slice().len())?;
        Ok(fst)
    }
}

/// Convert the given u64 to a usize. If the u64 is too big, then an error
/// is returned.
fn to_usize(n: u64) -> Result<usize, FormatError> {
    if n > usize::MAX as u64 {
        bail_format!("integer {} exceeds maximum usize {}", n, usize::MAX);
    }
    Ok(n as usize)
}
