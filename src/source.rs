//! Where the extractor reads the original game files from: a directory
//! (CD, mounted image, or install) or a CD image file.

use std::path::Path;

use anyhow::{Context, Result};

use crate::cdimage::CdImage;

pub enum Source {
    /// Paths are file system paths.
    Dir,
    /// Paths are inside the image's ISO 9660 file system.
    Image(Box<CdImage>),
}

impl Source {
    /// A CD image if `path` is a file, else a directory.
    pub fn open(path: &str) -> Result<Self> {
        if Path::new(path).is_file() {
            let image = CdImage::open(Path::new(path)).with_context(|| format!("cannot read CD image {path}"))?;
            Ok(Source::Image(Box::new(image)))
        } else {
            Ok(Source::Dir)
        }
    }

    pub fn exists(&self, path: &str) -> bool {
        match self {
            Source::Dir => Path::new(path).exists(),
            Source::Image(img) => img.exists(path),
        }
    }

    pub fn size(&self, path: &str) -> Option<u64> {
        match self {
            Source::Dir => std::fs::metadata(path).ok().filter(|m| m.is_file()).map(|m| m.len()),
            Source::Image(img) => img.size(path),
        }
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        match self {
            Source::Dir => std::fs::read(path).with_context(|| format!("Can't open file {path}")),
            Source::Image(img) => img.read(path),
        }
    }

    /// `path` as shown to the user.
    pub fn describe(&self, path: &str) -> String {
        match self {
            Source::Dir => path.to_string(),
            Source::Image(img) => format!("{}:{}", img.path().display(), path.trim_start_matches('/')),
        }
    }

    pub fn image(&self) -> Option<&CdImage> {
        match self {
            Source::Dir => None,
            Source::Image(img) => Some(img),
        }
    }
}
