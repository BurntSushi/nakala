/*!
A module for low-level I/O primitives.

Low level I/O primitives form the building blocks of the on-disk Nakala index
format. They're responsible for encoding integers, varints, length prefixed
strings and more.
*/

pub use read::Cursor as ReadCursor;
pub use write::Cursor as WriteCursor;

mod map;
mod read;
mod skip;
mod varint;
mod write;
