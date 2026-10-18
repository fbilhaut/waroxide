//! Bounds-checked little-endian readers.
//!
//! The original C code reads raw pointers and trusts the data; here every
//! read is checked so a truncated or corrupt file produces an error instead
//! of undefined behaviour.

use anyhow::{anyhow, Result};

fn out_of_bounds(offset: usize, len: usize, size: usize) -> anyhow::Error {
    anyhow!("read of {len} byte(s) at offset {offset} is out of bounds (data size {size})")
}

/// `len` bytes starting at `offset`.
pub fn slice(data: &[u8], offset: usize, len: usize) -> Result<&[u8]> {
    offset.checked_add(len).and_then(|end| data.get(offset..end)).ok_or_else(|| out_of_bounds(offset, len, data.len()))
}

pub fn u8_at(data: &[u8], offset: usize) -> Result<u8> {
    data.get(offset).copied().ok_or_else(|| out_of_bounds(offset, 1, data.len()))
}

pub fn le16(data: &[u8], offset: usize) -> Result<u16> {
    let b = slice(data, offset, 2)?;
    Ok(u16::from_le_bytes([b[0], b[1]]))
}

pub fn le32(data: &[u8], offset: usize) -> Result<u32> {
    let b = slice(data, offset, 4)?;
    Ok(u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
}

/// NUL-terminated string starting at `offset` (without the terminator).
/// Stops at the end of `data` if no NUL is found.
pub fn cstr(data: &[u8], offset: usize) -> &[u8] {
    let tail = data.get(offset..).unwrap_or(&[]);
    let end = tail.iter().position(|&b| b == 0).unwrap_or(tail.len());
    &tail[..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_little_endian() {
        let d = [0x34, 0x12, 0x78, 0x56];
        assert_eq!(le16(&d, 0).unwrap(), 0x1234);
        assert_eq!(le32(&d, 0).unwrap(), 0x5678_1234);
        assert!(le32(&d, 1).is_err());
        assert!(slice(&d, usize::MAX, 2).is_err());
    }

    #[test]
    fn reads_c_strings() {
        let d = b"abc\0def";
        assert_eq!(cstr(d, 0), b"abc");
        assert_eq!(cstr(d, 4), b"def");
        assert_eq!(cstr(d, 99), b"");
    }
}
