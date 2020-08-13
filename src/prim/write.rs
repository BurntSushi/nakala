use std::convert::TryInto;
use std::io::{self, Write};

use crate::error::{FormatContext, FormatError};
use crate::prim::varint;

/// Wraps any writer and records the current position in the writer.
///
/// The position recorded always corresponds to the position that the next byte
/// would be written to.
///
/// This is distinct from the standard library's `std::io::Cursor` in that it
/// works with any arbitrary writer, and provides utility methods for writing
/// common primitives found in the Nakala index format.
///
/// The `W` type parameter refers to the type of the underlying writer.
#[derive(Clone, Debug)]
pub struct Cursor<W> {
    wtr: W,
    pos: usize,
}

impl<W: io::Write> Cursor<W> {
    /// Create a new cursor for the given writer.
    pub fn new(wtr: W) -> Cursor<W> {
        Cursor { wtr, pos: 0 }
    }

    /// Return the current position of this writer.
    pub fn pos(&self) -> usize {
        self.pos
    }

    /// Unwrap the cursor and return the inner writer.
    pub fn into_inner(self) -> W {
        self.wtr
    }

    /// Return a reference to the inner writer.
    pub fn get_ref(&self) -> &W {
        &self.wtr
    }

    /// Return a mutable reference to the inner writer.
    ///
    /// Note that care must be taken when mutating the inner writer since it
    /// could cause the position of this cursor to become incorrect.
    pub fn get_mut(&mut self) -> &mut W {
        &mut self.wtr
    }

    /// Write a 32-bit little endian integer.
    pub fn write_u32_le(&mut self, n: u32) -> Result<(), FormatError> {
        self.write_all(&n.to_le_bytes()).context("failed to write a u32LE")
    }

    /// Write a 64-bit little endian integer.
    pub fn write_u64_le(&mut self, n: u64) -> Result<(), FormatError> {
        self.write_all(&n.to_le_bytes()).context("failed to write a u64LE")
    }

    /// Writes a usize as a 64-bit little endian integer.
    pub fn write_usize_le(&mut self, n: usize) -> Result<(), FormatError> {
        self.write_u64_le(n as u64)
    }

    /// Writes a variable length u64.
    ///
    /// The format of the integer matches Google's protocol buffer varint.
    ///
    /// See: https://developers.google.com/protocol-buffers/docs/encoding#varints
    pub fn write_varu64(&mut self, n: u64) -> Result<(), FormatError> {
        varint::write_u64(self, n)?;
        Ok(())
    }

    /// Writes a variable length usize.
    ///
    /// The format of the integer matches Google's protocol buffer varint.
    ///
    /// See: https://developers.google.com/protocol-buffers/docs/encoding#varints
    pub fn write_varusize(&mut self, n: usize) -> Result<(), FormatError> {
        self.write_varu64(n as u64)
    }

    /// Writes a length prefixed byte string. The length prefix is written as
    /// a varint.
    pub fn write_prefixed_bytes(
        &mut self,
        bytes: &[u8],
    ) -> Result<(), FormatError> {
        self.write_varusize(bytes.len())?;
        self.write_all(bytes).context("failed to write prefixed bytes")?;
        Ok(())
    }

    /// Writes a length prefixed UTF-8 string. The length prefix is written as
    /// a varint.
    pub fn write_prefixed_str(
        &mut self,
        string: &str,
    ) -> Result<(), FormatError> {
        self.write_prefixed_bytes(string.as_bytes())
    }
}

impl Cursor<Vec<u8>> {
    /// Clear this cursor's inner writer and reset its position.
    pub fn clear(&mut self) {
        self.wtr.clear();
        self.pos = 0;
    }
}

impl<W: io::Write> io::Write for Cursor<W> {
    fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
        let n = self.wtr.write(buf)?;
        let new = match self.pos().checked_add(n) {
            Some(new) => new,
            None => {
                let msg = format!(
                    "adding {} bytes to position {} causes overflow",
                    n,
                    self.pos(),
                );
                return Err(io::Error::new(io::ErrorKind::Other, msg));
            }
        };
        self.pos = new;
        Ok(n)
    }

    fn flush(&mut self) -> io::Result<()> {
        self.wtr.flush()
    }
}
