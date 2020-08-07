use crate::error::FormatError;

/// https://developers.google.com/protocol-buffers/docs/encoding#varints
pub fn read_u64(bytes: &[u8]) -> Result<(u64, usize), FormatError> {
    if bytes.is_empty() {
        bail_format!("no bytes to read varu64");
    }

    let mut n: u64 = 0;
    let mut shift: u32 = 0;
    for (i, &b) in bytes.iter().enumerate() {
        if b < 0b1000_0000 {
            return match (b as u64).checked_shl(shift) {
                None => bail_format!("varu64 has too many continuation bytes"),
                Some(b) => Ok((n | b, i + 1)),
            };
        }
        match ((b as u64) & 0b0111_1111).checked_shl(shift) {
            None => bail_format!("varu64 has too many bytes"),
            Some(b) => n |= b,
        }
        shift += 7;
    }
    Err(FormatError::msg("unexpected EOF when reading varu64"))
}

/// https://developers.google.com/protocol-buffers/docs/encoding#varints
pub fn write_u64<W: std::io::Write>(
    mut wtr: W,
    mut n: u64,
) -> Result<usize, FormatError> {
    let mut i = 0;
    while n >= 0b1000_0000 {
        wtr.write_all(&[(n as u8) | 0b1000_0000]).map_err(|e| {
            FormatError::err(e)
                .context("failed to write varint continuation byte")
        })?;
        n >>= 7;
        i += 1;
    }
    wtr.write_all(&[n as u8]).map_err(|e| {
        FormatError::err(e).context("failed to write final varint byte")
    })?;
    Ok(i + 1)
}
