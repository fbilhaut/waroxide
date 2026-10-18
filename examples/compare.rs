//! Compare two extracted data directories, e.g. one produced by the
//! original C++ wartool and one by this port:
//!
//!     cargo run --release --example compare -- <reference-dir> <candidate-dir>
//!
//! PNGs are compared by decoded content (size, palette, transparency,
//! pixels) and `.gz` files by decompressed content, so differences in
//! compression settings or gzip headers are ignored. Everything else is
//! compared byte for byte.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn files(root: &Path) -> BTreeSet<PathBuf> {
    let mut out = BTreeSet::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.insert(path.strip_prefix(root).unwrap().to_path_buf());
            }
        }
    }
    out
}

struct Png {
    width: u32,
    height: u32,
    palette: Option<Vec<u8>>,
    trns: Option<Vec<u8>>,
    pixels: Vec<u8>,
}

fn read_png(path: &Path) -> Result<Png, String> {
    let decoder = png::Decoder::new(File::open(path).map_err(|e| e.to_string())?);
    let mut reader = decoder.read_info().map_err(|e| e.to_string())?;
    let mut pixels = vec![0; reader.output_buffer_size()];
    let frame = reader.next_frame(&mut pixels).map_err(|e| e.to_string())?;
    pixels.truncate(frame.buffer_size());
    let info = reader.info();
    Ok(Png {
        width: frame.width,
        height: frame.height,
        palette: info.palette.as_ref().map(|p| p.to_vec()),
        trns: info.trns.as_ref().map(|t| t.to_vec()),
        pixels,
    })
}

fn gunzip(path: &Path) -> Result<Vec<u8>, String> {
    let mut out = Vec::new();
    flate2::read::GzDecoder::new(File::open(path).map_err(|e| e.to_string())?)
        .read_to_end(&mut out)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// Describe how `a` and `b` differ, or `None` if they match.
fn diff(a: &Path, b: &Path) -> Option<String> {
    let ext = a.extension().and_then(|e| e.to_str()).unwrap_or("");
    match ext {
        "png" => {
            let (x, y) = match (read_png(a), read_png(b)) {
                (Ok(x), Ok(y)) => (x, y),
                (x, y) => return Some(format!("decode: {:?} / {:?}", x.err(), y.err())),
            };
            if (x.width, x.height) != (y.width, y.height) {
                return Some(format!("size {}x{} vs {}x{}", x.width, x.height, y.width, y.height));
            }
            if x.palette != y.palette {
                return Some("palette differs".into());
            }
            // libpng writes a full 256-entry tRNS; trailing opaque entries are
            // equivalent to a shorter chunk.
            let norm = |t: Option<Vec<u8>>| {
                t.map(|mut t| {
                    while t.last() == Some(&0xFF) {
                        t.pop();
                    }
                    t
                })
            };
            if norm(x.trns) != norm(y.trns) {
                return Some("transparency differs".into());
            }
            let n = x.pixels.iter().zip(&y.pixels).filter(|(p, q)| p != q).count();
            (n > 0).then(|| format!("{n} pixel(s) differ"))
        }
        _ => {
            let read = |p: &Path| if ext == "gz" { gunzip(p) } else { fs::read(p).map_err(|e| e.to_string()) };
            match (read(a), read(b)) {
                (Ok(x), Ok(y)) if x == y => None,
                (Ok(x), Ok(y)) => {
                    let first = x.iter().zip(&y).position(|(p, q)| p != q).unwrap_or(x.len().min(y.len()));
                    Some(format!("content differs ({} vs {} bytes, first difference at {first})", x.len(), y.len()))
                }
                (x, y) => Some(format!("read: {:?} / {:?}", x.err(), y.err())),
            }
        }
    }
}

fn main() -> ExitCode {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    let [reference, candidate] = args.as_slice() else {
        eprintln!("usage: compare <reference-dir> <candidate-dir>");
        return ExitCode::from(2);
    };
    let (ra, ca) = (files(reference), files(candidate));

    let mut problems = 0;
    for f in ra.difference(&ca) {
        println!("missing   {}", f.display());
        problems += 1;
    }
    for f in ca.difference(&ra) {
        println!("extra     {}", f.display());
        problems += 1;
    }
    let common: Vec<_> = ra.intersection(&ca).collect();
    for f in &common {
        if let Some(d) = diff(&reference.join(f), &candidate.join(f)) {
            println!("different {}: {d}", f.display());
            problems += 1;
        }
    }
    println!(
        "{} files compared, {} only in reference, {} only in candidate, {problems} problem(s)",
        common.len(),
        ra.difference(&ca).count(),
        ca.difference(&ra).count()
    );
    if problems == 0 {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}
