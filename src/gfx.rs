//! Decoders for the Warcraft II graphic formats: sprite sheets (RLE and
//! uncompressed), tilesets, fonts, plain images and cursors.
//!
//! All decoders produce an [`Indexed`] image laid out exactly like the
//! original extractor did, so the PNGs match what the Wargus scripts expect.

use anyhow::{bail, Result};

use crate::bytes::{le16, le32, slice, u8_at};
use crate::image::{Indexed, TRANSPARENT};

/// Tiles per row in the generated tileset images.
pub const TILES_PER_ROW: usize = 16;

/// Frames per row for RLE sprite sheets with at least that many frames.
const FRAMES_PER_ROW: usize = 5;

/// Header shared by sprite files: frame count and maximum frame size.
const SPRITE_HEADER: usize = 6;

struct Frame {
    xoff: usize,
    yoff: usize,
    width: usize,
    height: usize,
    offset: u32,
}

fn frame(data: &[u8], index: usize) -> Result<Frame> {
    let p = SPRITE_HEADER + index * 8;
    let h = slice(data, p, 4)?;
    Ok(Frame {
        xoff: h[0] as usize,
        yoff: h[1] as usize,
        width: h[2] as usize,
        height: h[3] as usize,
        offset: le32(data, p + 4)?,
    })
}

/// Decode an RLE (`gfx`) frame. Each row is a run list: `0x80|n` skips `n`
/// transparent pixels, `0x40|n` repeats the next byte `n` times, otherwise
/// `n` literal bytes follow.
fn decode_gfx_frame(index: usize, data: &[u8], img: &mut Indexed, base: usize, pitch: usize) -> Result<()> {
    let f = frame(data, index)?;
    let rows = f.offset as usize;
    let dp = base + f.xoff + f.yoff * pitch;
    for h in 0..f.height {
        let mut sp = rows + le16(data, rows + h * 2)? as usize;
        let mut w = 0;
        while w < f.width {
            let ctrl = u8_at(data, sp)?;
            sp += 1;
            let at = dp + h * pitch + w;
            if ctrl & 0x80 != 0 {
                let n = (ctrl & 0x7F) as usize;
                (0..n).for_each(|i| img.put(at + i, TRANSPARENT));
                w += n;
            } else if ctrl & 0x40 != 0 {
                let n = (ctrl & 0x3F) as usize;
                let v = u8_at(data, sp)?;
                sp += 1;
                (0..n).for_each(|i| img.put(at + i, v));
                w += n;
            } else {
                let n = (ctrl & 0x3F) as usize;
                for (i, &v) in slice(data, sp, n)?.iter().enumerate() {
                    img.put(at + i, v);
                }
                sp += n;
                w += n;
            }
        }
    }
    Ok(())
}

/// Decode an uncompressed (`gfu`) frame. The high bit of the offset adds 256
/// to the frame width.
fn decode_gfu_frame(index: usize, data: &[u8], img: &mut Indexed, base: usize, pitch: usize) -> Result<()> {
    let f = frame(data, index)?;
    let (width, offset) = if f.offset & 0x8000_0000 != 0 {
        (f.width + 256, (f.offset & 0x7FFF_FFFF) as usize)
    } else {
        (f.width, f.offset as usize)
    };
    let mut dp = base + f.xoff + f.yoff * pitch;
    let mut sp = offset;
    for _ in 0..f.height {
        for (i, &v) in slice(data, sp, width)?.iter().enumerate() {
            img.put(dp + i, v);
        }
        dp += pitch;
        sp += width;
    }
    Ok(())
}

/// Decode a sprite file into a sheet.
///
/// * `rle == true` (unit/building graphics): every frame gets a cell of the
///   declared maximum size, 5 cells per row once there are 5+ frames.
/// * `rle == false` (UI graphics): frames are stacked vertically in cells
///   sized to the actual extent of all frames.
///
/// `alt` / `alt_start` substitute frames from a second file starting at a
/// given index (used for workers carrying resources).
pub fn convert_graphic(rle: bool, data: &[u8], alt: Option<&[u8]>, alt_start: usize) -> Result<Indexed> {
    let count = le16(data, 0)? as usize;
    let max_width = le16(data, 2)? as usize;
    let max_height = le16(data, 4)? as usize;

    let (cell_w, cell_h, per_row, cells) = if rle {
        if count < FRAMES_PER_ROW {
            (max_width, max_height, 1, count)
        } else {
            (max_width, max_height, FRAMES_PER_ROW, count.div_ceil(FRAMES_PER_ROW) * FRAMES_PER_ROW)
        }
    } else {
        let (mut w, mut h) = (0, 0);
        for i in 0..count {
            let f = frame(data, i)?;
            let width = f.width + if f.offset & 0x8000_0000 != 0 { 256 } else { 0 };
            w = w.max(f.xoff + width);
            h = h.max(f.yoff + f.height);
        }
        (w, h, 1, count)
    };

    let pitch = cell_w * per_row;
    let mut img = Indexed::new(pitch, cell_h * (cells / per_row), TRANSPARENT);
    for i in 0..count {
        let base = cell_w * (i % per_row) + cell_h * pitch * (i / per_row);
        if !rle {
            decode_gfu_frame(i, data, &mut img, base, pitch)?;
        } else if let (Some(alt), true) = (alt, i >= alt_start) {
            decode_gfx_frame(i, alt, &mut img, base, pitch)?;
        } else {
            decode_gfx_frame(i, data, &mut img, base, pitch)?;
        }
    }
    Ok(img)
}

/// Assemble a tileset image from its mini-tiles (8x8 pixel blocks) and
/// mega-tiles (4x4 mini-tile references with flip bits). The first 16 tiles
/// are the fog of war, stored as raw 32x32 images.
pub fn convert_tileset(mini: &[u8], mega: &[u8]) -> Result<Indexed> {
    let count = mega.len() / 32;
    let width = TILES_PER_ROW * 32;
    let mut img = Indexed::new(width, count.div_ceil(TILES_PER_ROW) * 32, 0);

    for i in 0..count {
        let tx = i % TILES_PER_ROW;
        let ty = i / TILES_PER_ROW;
        if i < 16 {
            for y in 0..32 {
                let src = slice(mini, i * 32 * 32 + y * 32, 32)?;
                let dst = tx * 32 + (ty * 32 + y) * width;
                img.pixels[dst..dst + 32].copy_from_slice(src);
            }
            continue;
        }
        for y in 0..4 {
            for x in 0..4 {
                let r = le16(mega, i * 32 + (x + y * 4) * 2)? as usize;
                let src = (r & 0xFFFC) * 16;
                let (flip_x, flip_y) = (r & 2 != 0, r & 1 != 0);
                for py in 0..8 {
                    for px in 0..8 {
                        let sy = if flip_y { 7 - py } else { py };
                        let sx = if flip_x { 7 - px } else { px };
                        let v = u8_at(mini, src + sy * 8 + sx)?;
                        img.put((py + (y + ty * 4) * 8) * width + (x + tx * 4) * 8 + px, v);
                    }
                }
            }
        }
    }
    Ok(img)
}

/// Decode a `FONT` file into a sheet of 15 glyphs per row, starting at
/// character 32 (31 for the Russian release, to keep its last letter).
pub fn convert_font(data: &[u8], russian: bool) -> Result<Indexed> {
    const PER_ROW: usize = 15;
    let first = if russian { 31 } else { 32 };
    let Some(count) = (u8_at(data, 5)? as usize).checked_sub(first) else {
        bail!("font declares fewer than {first} characters");
    };
    let max_w = u8_at(data, 6)? as usize;
    let max_h = u8_at(data, 7)? as usize;
    let iw = max_w * PER_ROW;
    let mut img = Indexed::new(iw, count.div_ceil(PER_ROW) * max_h, TRANSPARENT);

    for i in 0..count {
        let offset = le32(data, 8 + i * 4)? as usize;
        if offset == 0 {
            continue;
        }
        let g = slice(data, offset, 4)?;
        let (width, height, xoff, yoff) = (g[0] as usize, g[1] as usize, g[2] as usize, g[3] as usize);
        let dp = (i % PER_ROW) * max_w + (i / PER_ROW) * iw * max_h + xoff + yoff * iw;
        let mut sp = offset + 4;
        let (mut w, mut h) = (0usize, 0usize);
        // Each byte: skip count in the high 5 bits, colour in the low 3.
        'glyph: loop {
            let ctrl = u8_at(data, sp)? as usize;
            sp += 1;
            w += ctrl >> 3;
            while w >= width {
                w -= width;
                h += 1;
                if h >= height {
                    break 'glyph;
                }
            }
            img.put(dp + h * iw + w, (ctrl & 7) as u8);
            w += 1;
            if w >= width {
                w -= width;
                h += 1;
                if h >= height {
                    break;
                }
            }
        }
    }
    Ok(img)
}

/// The Russian SPK release keeps some lowercase letters in odd places;
/// move them where Stratagus expects them.
pub fn fix_russian_font(img: &mut Indexed) {
    let w = img.width;
    let fw = w / 15;
    let fh = img.height / 14;
    let mut copy = |src_row: usize, src_col: usize, dst_row: usize, dst_col: usize, chars: usize| {
        for i in 0..fh {
            let si = w * (src_row * fh + i) + fw * src_col;
            let di = w * (dst_row * fh + i) + fw * dst_col;
            for j in 0..chars * fw {
                if let Some(&v) = img.pixels.get(si + j) {
                    img.put(di + j, v);
                }
            }
        }
    };
    copy(12, 12, 9, 9, 3);
    copy(13, 0, 9, 12, 3);
    copy(13, 3, 10, 0, 10);
}

/// Plain image: `u16` width, `u16` height, raw pixels.
pub fn convert_image(data: &[u8]) -> Result<Indexed> {
    let width = le16(data, 0)? as usize;
    let height = le16(data, 2)? as usize;
    if width == 0 || height == 0 {
        bail!("image has an empty size {width}x{height}");
    }
    let pixels = slice(data, 4, width * height)?.to_vec();
    Ok(Indexed { width, height, pixels })
}

/// Nearest-neighbour resize, with the same rounding as the original.
pub fn resize(img: &Indexed, nw: usize, nh: usize) -> Indexed {
    if img.width == nw && img.height == nh {
        return img.clone();
    }
    let (ow, oh) = (img.width, img.height);
    let mut out = Vec::with_capacity(nw * nh);
    for i in 0..nh {
        for j in 0..nw {
            out.push(img.pixels[i * oh / nh * ow + j * ow / nw]);
        }
    }
    Indexed { width: nw, height: nh, pixels: out }
}

/// Cursor: `u16` hot spot x/y, `u16` width/height, raw pixels with 0 as
/// transparent.
pub fn convert_cursor(data: &[u8]) -> Result<Indexed> {
    let width = le16(data, 4)? as usize;
    let height = le16(data, 6)? as usize;
    let pixels = slice(data, 8, width * height)?.iter().map(|&p| if p == 0 { TRANSPARENT } else { p }).collect();
    Ok(Indexed { width, height, pixels })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sprite(frames: &[(u8, u8, u8, u8, u32)], max_w: u16, max_h: u16, body: &[u8]) -> Vec<u8> {
        let mut d = Vec::new();
        d.extend((frames.len() as u16).to_le_bytes());
        d.extend(max_w.to_le_bytes());
        d.extend(max_h.to_le_bytes());
        for &(x, y, w, h, off) in frames {
            d.extend([x, y, w, h]);
            d.extend(off.to_le_bytes());
        }
        d.extend(body);
        d
    }

    #[test]
    fn rle_frame() {
        // One 3x1 frame at (1,0) in a 4x1 cell: transparent 1, repeat 2 x 9.
        let body_at = 6 + 8;
        let rows = [2u8, 0]; // row 0 starts 2 bytes after the row table
        let mut body = rows.to_vec();
        body.extend([0x81, 0x42, 9]);
        let d = sprite(&[(1, 0, 3, 1, body_at as u32)], 4, 1, &body);
        let img = convert_graphic(true, &d, None, 0).unwrap();
        assert_eq!((img.width, img.height), (4, 1));
        assert_eq!(img.pixels, vec![255, 255, 9, 9]);
    }

    #[test]
    fn rle_sheet_layout() {
        // 6 empty frames -> 5 per row, 2 rows.
        let frames = vec![(0, 0, 0, 0, 0); 6];
        let d = sprite(&frames, 2, 3, &[]);
        let img = convert_graphic(true, &d, None, 0).unwrap();
        assert_eq!((img.width, img.height), (10, 6));
    }

    #[test]
    fn gfu_frames_stack_vertically() {
        let off = 6 + 16;
        let body = [1, 2, 3, 4, 5, 6];
        let d = sprite(&[(0, 0, 2, 1, off), (1, 1, 2, 2, off + 2)], 0, 0, &body);
        let img = convert_graphic(false, &d, None, 0).unwrap();
        assert_eq!((img.width, img.height), (3, 6));
        assert_eq!(img.pixels[..3], [1, 2, 255]);
        // second frame: base = 3*3 (one cell down), then +1,+1
        assert_eq!(img.pixels[9 + 4..9 + 6], [3, 4]);
    }

    #[test]
    fn tileset_flips_minitiles() {
        let mut mini = vec![0u8; 16 * 1024 + 64];
        for (i, p) in mini[16 * 1024..].iter_mut().enumerate() {
            *p = i as u8;
        }
        let mut mega = vec![0u8; 17 * 32];
        // tile 16, minitile (0,0): mini-tile index 256 (=16*1024/64), flip x
        let r = (256u16 << 2) | 2;
        mega[16 * 32..16 * 32 + 2].copy_from_slice(&r.to_le_bytes());
        let img = convert_tileset(&mini, &mega).unwrap();
        assert_eq!(img.width, 512);
        assert_eq!(img.height, 64);
        let row1 = 32 * 512;
        assert_eq!(img.pixels[row1..row1 + 8], [7, 6, 5, 4, 3, 2, 1, 0]);
    }

    #[test]
    fn font_glyph() {
        let mut d = b"FONT ".to_vec();
        d.extend([33, 2, 2]); // one glyph, max 2x2
        d.extend(12u32.to_le_bytes());
        d.extend([2, 2, 0, 0]); // 2x2 glyph
        d.extend([0x01, 0x0A]); // colour 1 at (0,0); skip 1, colour 2 at (0,1)
        d.extend([0x03]); // colour 3 at (1,1), finishes glyph
        let img = convert_font(&d, false).unwrap();
        assert_eq!((img.width, img.height), (30, 2));
        assert_eq!(img.pixels[0], 1);
        assert_eq!(img.pixels[30], 2);
        assert_eq!(img.pixels[31], 3);
    }

    #[test]
    fn resize_nearest() {
        let img = Indexed { width: 2, height: 1, pixels: vec![1, 2] };
        assert_eq!(resize(&img, 4, 2).pixels, vec![1, 1, 2, 2, 1, 1, 2, 2]);
    }

    #[test]
    fn cursor_zero_is_transparent() {
        let mut d = vec![0u8; 4];
        d.extend(2u16.to_le_bytes());
        d.extend(1u16.to_le_bytes());
        d.extend([0, 5]);
        assert_eq!(convert_cursor(&d).unwrap().pixels, vec![255, 5]);
    }
}
