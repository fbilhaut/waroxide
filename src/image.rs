//! 8-bit indexed images, palettes and PNG output.

use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use anyhow::{bail, Context, Result};

/// Palette index the original game treats as transparent once written to PNG.
pub const TRANSPARENT: u8 = 255;

/// An 8-bit paletted image stored row by row.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Indexed {
    pub width: usize,
    pub height: usize,
    pub pixels: Vec<u8>,
}

impl Indexed {
    pub fn new(width: usize, height: usize, fill: u8) -> Self {
        Self { width, height, pixels: vec![fill; width * height] }
    }

    /// Store `v` at linear index `idx`, ignoring writes past the end of the
    /// buffer (the C code would corrupt memory there).
    #[inline]
    pub fn put(&mut self, idx: usize, v: u8) {
        if let Some(p) = self.pixels.get_mut(idx) {
            *p = v;
        }
    }
}

/// Rectangle of an image to write out.
#[derive(Clone, Copy, Debug)]
pub struct Region {
    pub x: usize,
    pub y: usize,
    pub width: usize,
    pub height: usize,
}

impl Region {
    pub fn whole(img: &Indexed) -> Self {
        Self { x: 0, y: 0, width: img.width, height: img.height }
    }
}

/// Convert a raw 6-bit VGA palette to 8-bit (each component `<< 2`).
/// The result always has 256 entries; short palettes are zero-padded.
pub fn convert_palette(raw: &[u8]) -> [u8; 768] {
    let mut pal = [0u8; 768];
    for (dst, src) in pal.iter_mut().zip(raw) {
        *dst = src << 2;
    }
    pal
}

/// Write `region` of `img` as an indexed PNG.
///
/// Rows are addressed linearly with the image width as pitch, like the
/// original: a region wider than the image wraps into the following row.
/// With `transparent`, index 0 is remapped to 255 and 255 is the only fully
/// transparent palette entry.
pub fn save_png(path: &Path, img: &Indexed, region: Region, palette: &[u8; 768], transparent: bool) -> Result<()> {
    let Region { x, y, width, height } = region;
    if width == 0 || height == 0 {
        bail!("{}: refusing to write an empty {width}x{height} image", path.display());
    }
    let mut buf = Vec::with_capacity(width * height);
    for row in 0..height {
        let start = (y + row) * img.width + x;
        match img.pixels.get(start..start + width) {
            Some(line) => buf.extend_from_slice(line),
            None => bail!("{}: region {region:?} exceeds the {}x{} image", path.display(), img.width, img.height),
        }
    }
    if transparent {
        for p in buf.iter_mut().filter(|p| **p == 0) {
            *p = TRANSPARENT;
        }
    }

    let file = File::create(path).with_context(|| format!("Can't open file {}", path.display()))?;
    let mut enc = png::Encoder::new(BufWriter::new(file), width as u32, height as u32);
    enc.set_color(png::ColorType::Indexed);
    enc.set_depth(png::BitDepth::Eight);
    enc.set_palette(palette.to_vec());
    if transparent {
        let mut trns = vec![0xFF; 256];
        trns[TRANSPARENT as usize] = 0;
        enc.set_trns(trns);
    }
    enc.set_compression(png::Compression::Best);
    let mut writer = enc.write_header()?;
    writer.write_image_data(&buf)?;
    writer.finish()?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_is_scaled_and_padded() {
        let pal = convert_palette(&[1, 63, 0x41]);
        assert_eq!(&pal[..3], &[4, 252, 4]);
        assert!(pal[3..].iter().all(|&c| c == 0));
    }

    #[test]
    fn writes_transparent_region() {
        let dir = std::env::temp_dir().join(format!("orcrust-png-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.png");
        let mut img = Indexed::new(4, 2, 7);
        img.pixels[5] = 0;
        let region = Region { x: 1, y: 0, width: 2, height: 2 };
        save_png(&path, &img, region, &[0; 768], true).unwrap();

        let decoder = png::Decoder::new(File::open(&path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut out = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut out).unwrap();
        assert_eq!((info.width, info.height), (2, 2));
        assert_eq!(&out[..4], &[7, 7, 255, 7]);
        assert_eq!(reader.info().trns.as_ref().unwrap()[255], 0);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
