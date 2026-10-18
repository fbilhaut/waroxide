//! Blizzard `.WAR` archives (`MAINDAT.WAR`, `REZDAT.WAR`, `STRDAT.WAR`, ...).
//!
//! Layout: a `u32` magic (`0x19`), a `u16` entry count, a `u16` archive id,
//! then one `u32` offset per entry. Each entry starts with a `u32` whose high
//! byte holds flags (`0x00` stored, `0x20` LZ-compressed) and whose low 24
//! bits hold the uncompressed length.

use std::fs;
use std::path::Path;

use anyhow::{bail, Context, Result};

use crate::bytes::{le16, le32, slice, u8_at};

/// What the original extractor substitutes for entries with a bad offset:
/// a fake header `{1, 1, 1}` that decodes to a single `0x01` byte.
const EMPTY_ENTRY: [u8; 1] = [1];

pub struct Archive {
    data: Vec<u8>,
    /// Offset of each entry header, `None` for entries that were rejected.
    entries: Vec<Option<usize>>,
}

impl Archive {
    /// Read an archive from disk and check it has the expected id.
    pub fn open(path: &Path, kind: u16) -> Result<Self> {
        let data = fs::read(path).with_context(|| format!("Can't open file {}", path.display()))?;
        Self::from_bytes(data, kind)
    }

    pub fn from_bytes(data: Vec<u8>, kind: u16) -> Result<Self> {
        let magic = le32(&data, 0)?;
        if magic != 0x19 {
            bail!("Wrong magic {magic:08x}, expected 00000019: this version of the data is not supported");
        }
        let count = le16(&data, 4)? as usize;
        let id = le16(&data, 6)?;
        if id != kind {
            bail!("Wrong type {id:08x}, expected {kind:08x}: this version of the data is not supported");
        }

        let size = data.len();
        let mut entries = Vec::with_capacity(count);
        for i in 0..count {
            let offset = le32(&data, 8 + i * 4)? as usize;
            if offset + 4 >= size {
                println!("Ignore entry {i} in archive (invalid offset)");
                entries.push(None);
                continue;
            }
            let header = le32(&data, offset)?;
            let length = (header & 0x00FF_FFFF) as usize;
            if header >> 24 == 0x00 && offset + length >= size {
                println!("Ignore entry {i} in archive (invalid uncompressed length)");
                entries.push(None);
                continue;
            }
            entries.push(Some(offset));
        }
        Ok(Self { data, entries })
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Extract and decompress entry `index`.
    pub fn entry(&self, index: usize) -> Result<Vec<u8>> {
        match self.entries.get(index) {
            None => bail!("archive has no entry {index} (it has {})", self.entries.len()),
            Some(None) => Ok(EMPTY_ENTRY.to_vec()),
            Some(Some(offset)) => decompress(&self.data[*offset..]).with_context(|| format!("archive entry {index}")),
        }
    }
}

/// Decode one entry, starting at its length/flags header.
pub fn decompress(src: &[u8]) -> Result<Vec<u8>> {
    let header = le32(src, 0)?;
    let flags = header >> 24;
    let length = (header & 0x00FF_FFFF) as usize;
    match flags {
        0x00 => Ok(slice(src, 4, length)?.to_vec()),
        0x20 => lz_decompress(&src[4..], length),
        _ => bail!("Unknown flags {flags:x}: this version of the data is not supported"),
    }
}

/// LZSS variant with a 4 KiB ring buffer. Each control byte describes 8
/// items: a set bit is a literal byte, a clear bit is a `u16` back-reference
/// (12-bit ring offset, 4-bit length minus 3).
fn lz_decompress(src: &[u8], out_len: usize) -> Result<Vec<u8>> {
    let mut out = Vec::with_capacity(out_len);
    let mut ring = [0u8; 4096];
    let mut ri = 0usize;
    let mut sp = 0usize;

    while out.len() < out_len {
        let mut ctrl = u8_at(src, sp)?;
        sp += 1;
        for _ in 0..8 {
            if ctrl & 1 != 0 {
                let b = u8_at(src, sp)?;
                sp += 1;
                out.push(b);
                ring[ri & 0xFFF] = b;
                ri += 1;
            } else {
                let r = le16(src, sp)? as usize;
                sp += 2;
                for pos in (r & 0xFFF..).take((r >> 12) + 3) {
                    let b = ring[pos & 0xFFF];
                    ring[ri & 0xFFF] = b;
                    ri += 1;
                    out.push(b);
                    if out.len() == out_len {
                        break;
                    }
                }
            }
            if out.len() == out_len {
                break;
            }
            ctrl >>= 1;
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn archive_with(entries: &[Vec<u8>], kind: u16) -> Vec<u8> {
        let mut data = Vec::new();
        data.extend(0x19u32.to_le_bytes());
        data.extend((entries.len() as u16).to_le_bytes());
        data.extend(kind.to_le_bytes());
        let mut offset = 8 + entries.len() * 4;
        for e in entries {
            data.extend((offset as u32).to_le_bytes());
            offset += e.len();
        }
        for e in entries {
            data.extend(e);
        }
        data.extend([0u8; 4]); // padding so the last entry is not at EOF
        data
    }

    #[test]
    fn stored_entry() {
        let mut e = vec![3, 0, 0, 0x00];
        e.extend(b"abc");
        let a = Archive::from_bytes(archive_with(&[e], 7), 7).unwrap();
        assert_eq!(a.entry(0).unwrap(), b"abc");
        assert!(a.entry(1).is_err());
    }

    #[test]
    fn compressed_entry() {
        // "abcabcabc": 3 literals, then a back-reference of length 6 to ring
        // offset 0.
        let mut e = vec![9, 0, 0, 0x20];
        e.push(0b0000_0111);
        e.extend(b"abc");
        e.extend(((6u16 - 3) << 12).to_le_bytes()); // length 6, ring offset 0
        let a = Archive::from_bytes(archive_with(&[e], 1), 1).unwrap();
        assert_eq!(a.entry(0).unwrap(), b"abcabcabc");
    }

    #[test]
    fn rejects_wrong_type_and_magic() {
        let data = archive_with(&[], 1);
        assert!(Archive::from_bytes(data.clone(), 2).is_err());
        let mut bad = data;
        bad[0] = 0x18;
        assert!(Archive::from_bytes(bad, 1).is_err());
    }

    #[test]
    fn invalid_offset_becomes_empty_entry() {
        let mut data = archive_with(&[vec![0, 0, 0, 0]], 1);
        data[8..12].copy_from_slice(&0xFFFFu32.to_le_bytes());
        let a = Archive::from_bytes(data, 1).unwrap();
        assert_eq!(a.entry(0).unwrap(), vec![1]);
    }
}
