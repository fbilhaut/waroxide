//! Read-only access to CD images: `.cue`/`.bin` pairs and plain `.iso`
//! files.
//!
//! The first data track is read as an ISO 9660 file system (file names are
//! matched case-insensitively, without the `;1` version suffix), and audio
//! tracks can be saved as WAV files. This lets the extractor work directly
//! from the image a DOSBox install uses, including the CD audio soundtrack.

use std::cell::RefCell;
use std::collections::HashMap;
use std::fs::File;
use std::io::{BufWriter, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use anyhow::{bail, ensure, Context, Result};

use crate::bytes::{le16, le32, slice};

const USER_DATA: u64 = 2048;
const RAW_SECTOR: u64 = 2352;
const SYNC: [u8; 12] = [0, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0xFF, 0];
/// CD audio is 44.1 kHz, 16-bit, stereo: 75 sectors of 2352 bytes a second.
const FRAMES_PER_SECOND: u64 = 75;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Track {
    pub number: u32,
    pub audio: bool,
    file: PathBuf,
    /// Byte offset of the track's first sector (its `INDEX 01`) in `file`.
    start: u64,
    sector_size: u64,
    /// Offset of the 2048 user bytes inside a data sector.
    data_offset: u64,
    pub sectors: u64,
}

impl Track {
    /// Length in seconds (audio tracks).
    pub fn seconds(&self) -> u64 {
        self.sectors / FRAMES_PER_SECOND
    }
}

#[derive(Clone, Debug)]
struct Entry {
    name: String,
    lba: u32,
    size: u32,
    dir: bool,
}

pub struct CdImage {
    path: PathBuf,
    pub tracks: Vec<Track>,
    data: Track,
    root: Entry,
    dirs: RefCell<HashMap<u32, Vec<Entry>>>,
}

fn sector_layout(mode: &str) -> Result<(u64, u64, bool)> {
    Ok(match mode.to_ascii_uppercase().as_str() {
        "AUDIO" => (RAW_SECTOR, 0, true),
        "MODE1/2352" => (RAW_SECTOR, 16, false),
        "MODE1/2048" | "MODE2/2048" => (USER_DATA, 0, false),
        "MODE2/2352" => (RAW_SECTOR, 24, false),
        "MODE2/2336" => (2336, 8, false),
        other => bail!("unsupported track mode {other}"),
    })
}

/// `mm:ss:ff` (frames are 1/75 s) to a sector number.
fn msf(s: &str) -> Result<u64> {
    let p: Vec<u64> =
        s.split(':').map(str::parse).collect::<Result<_, _>>().with_context(|| format!("bad time {s}"))?;
    ensure!(p.len() == 3, "bad time {s}");
    Ok((p[0] * 60 + p[1]) * FRAMES_PER_SECOND + p[2])
}

/// Split a cue line into words, keeping quoted strings together.
fn words(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut chars = line.trim().chars().peekable();
    while let Some(&c) = chars.peek() {
        if c.is_whitespace() {
            chars.next();
        } else if c == '"' {
            chars.next();
            out.push(chars.by_ref().take_while(|&c| c != '"').collect());
        } else {
            let mut w = String::new();
            while let Some(&c) = chars.peek().filter(|c| !c.is_whitespace()) {
                w.push(c);
                chars.next();
            }
            out.push(w);
        }
    }
    out
}

/// Find `name` in `dir`, ignoring case (cue sheets often get the case of
/// the `.bin` wrong).
fn find_file(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    std::fs::read_dir(dir)
        .ok()?
        .flatten()
        .map(|e| e.path())
        .find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().eq_ignore_ascii_case(name)))
}

/// A `TRACK` as written in the cue sheet.
struct CueTrack {
    file: PathBuf,
    number: u32,
    mode: String,
    index0: Option<u64>,
    index1: Option<u64>,
}

pub fn parse_cue(cue: &Path) -> Result<Vec<Track>> {
    let text = std::fs::read(cue).with_context(|| format!("cannot read {}", cue.display()))?;
    let text = String::from_utf8_lossy(&text);
    let dir = cue.parent().unwrap_or(Path::new("."));

    let mut raw: Vec<CueTrack> = Vec::new();
    let mut file: Option<PathBuf> = None;
    for line in text.lines() {
        let w = words(line);
        match w.first().map(|s| s.to_ascii_uppercase()).as_deref() {
            Some("FILE") => {
                let name = w.get(1).context("FILE without a name")?;
                file =
                    Some(find_file(dir, name).with_context(|| format!("{name} (from {}) not found", cue.display()))?);
            }
            Some("TRACK") => {
                let f = file.clone().context("TRACK before FILE")?;
                let n = w.get(1).and_then(|n| n.parse().ok()).context("bad TRACK number")?;
                raw.push(CueTrack {
                    file: f,
                    number: n,
                    mode: w.get(2).cloned().unwrap_or_default(),
                    index0: None,
                    index1: None,
                });
            }
            Some("INDEX") => {
                let t = raw.last_mut().context("INDEX before TRACK")?;
                let at = msf(w.get(2).context("INDEX without a time")?)?;
                match w.get(1).map(String::as_str) {
                    Some("00") => t.index0 = Some(at),
                    Some("01") => t.index1 = Some(at),
                    _ => {}
                }
            }
            _ => {}
        }
    }

    let mut tracks = Vec::new();
    for (i, t) in raw.iter().enumerate() {
        let (sector_size, data_offset, audio) = sector_layout(&t.mode)?;
        let first = t.index1.with_context(|| format!("track {} has no INDEX 01", t.number))?;
        // A track ends where the next one in the same file starts (its
        // pregap included); the last one at the end of the file.
        let end = match raw.get(i + 1).filter(|n| n.file == t.file) {
            Some(next) => next.index0.or(next.index1).unwrap_or(first),
            None => std::fs::metadata(&t.file)?.len() / sector_size,
        };
        tracks.push(Track {
            number: t.number,
            audio,
            file: t.file.clone(),
            start: first * sector_size,
            sector_size,
            data_offset,
            sectors: end.saturating_sub(first),
        });
    }
    Ok(tracks)
}

/// A lone `.bin`/`.iso`: one data track, raw (2352) or cooked (2048).
fn single_track(path: &Path) -> Result<Vec<Track>> {
    let len = std::fs::metadata(path)?.len();
    let mut head = [0u8; 16];
    File::open(path)?.read_exact(&mut head).ok();
    let (sector_size, data_offset) = if head[..12] == SYNC && len % RAW_SECTOR == 0 {
        (RAW_SECTOR, if head[15] == 2 { 24 } else { 16 })
    } else {
        (USER_DATA, 0)
    };
    Ok(vec![Track {
        number: 1,
        audio: false,
        file: path.to_path_buf(),
        start: 0,
        sector_size,
        data_offset,
        sectors: len / sector_size,
    }])
}

fn parse_entry(rec: &[u8]) -> Result<Entry> {
    let name_len = rec[32] as usize;
    let raw = slice(rec, 33, name_len)?;
    let name = match raw {
        [0] => ".".to_string(),
        [1] => "..".to_string(),
        _ => {
            let s = String::from_utf8_lossy(raw);
            let s = s.split(';').next().unwrap_or("");
            s.strip_suffix('.').unwrap_or(s).to_string()
        }
    };
    Ok(Entry { name, lba: le32(rec, 2)?, size: le32(rec, 10)?, dir: rec[25] & 2 != 0 })
}

impl CdImage {
    /// Open a `.cue`, or a `.bin`/`.iso` (using a `.cue` next to it if any).
    pub fn open(path: &Path) -> Result<Self> {
        let ext = path.extension().map(|e| e.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        let tracks = if ext == "cue" {
            parse_cue(path)?
        } else {
            let cue = path.with_extension("cue");
            let cue = path
                .parent()
                .and_then(|d| find_file(d, &cue.file_name()?.to_string_lossy()))
                .filter(|c| c.extension().is_some_and(|e| e.eq_ignore_ascii_case("cue")));
            match cue {
                Some(cue) => parse_cue(&cue)?,
                None => single_track(path)?,
            }
        };
        let data = tracks.iter().find(|t| !t.audio).cloned().context("the image has no data track")?;

        let mut image = CdImage {
            path: path.to_path_buf(),
            tracks,
            data,
            root: Entry { name: String::new(), lba: 0, size: 0, dir: true },
            dirs: RefCell::default(),
        };
        image.root = image.find_root()?;
        Ok(image)
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    /// Read `count` user-data sectors of the data track starting at `lba`.
    fn read_sectors(&self, lba: u64, bytes: u64) -> Result<Vec<u8>> {
        let t = &self.data;
        let count = bytes.div_ceil(USER_DATA);
        ensure!(lba + count <= t.sectors, "read past the end of the data track (sector {lba}+{count})");
        let mut f = File::open(&t.file).with_context(|| format!("cannot open {}", t.file.display()))?;
        let mut out = Vec::with_capacity((count * USER_DATA) as usize);
        if t.sector_size == USER_DATA {
            f.seek(SeekFrom::Start(t.start + lba * USER_DATA))?;
            out.resize((count * USER_DATA) as usize, 0);
            f.read_exact(&mut out)?;
        } else {
            // Read raw sectors in chunks and keep the user data of each.
            let mut buf = vec![0u8; (t.sector_size * 64) as usize];
            f.seek(SeekFrom::Start(t.start + lba * t.sector_size))?;
            let mut left = count;
            while left > 0 {
                let n = left.min(64);
                let chunk = &mut buf[..(n * t.sector_size) as usize];
                f.read_exact(chunk)?;
                for s in chunk.chunks(t.sector_size as usize) {
                    out.extend_from_slice(&s[t.data_offset as usize..(t.data_offset + USER_DATA) as usize]);
                }
                left -= n;
            }
        }
        out.truncate(bytes as usize);
        Ok(out)
    }

    fn find_root(&self) -> Result<Entry> {
        for sector in 16..32 {
            let d = self.read_sectors(sector, USER_DATA)?;
            ensure!(&d[1..6] == b"CD001", "the data track is not an ISO 9660 file system");
            match d[0] {
                1 => {
                    ensure!(le16(&d, 128)? as u64 == USER_DATA, "unsupported ISO 9660 block size");
                    return parse_entry(&d[156..156 + 34]);
                }
                255 => break,
                _ => {}
            }
        }
        bail!("no ISO 9660 primary volume descriptor found")
    }

    fn list(&self, dir: &Entry) -> Result<Vec<Entry>> {
        if let Some(e) = self.dirs.borrow().get(&dir.lba) {
            return Ok(e.clone());
        }
        let data = self.read_sectors(dir.lba as u64, dir.size as u64)?;
        let mut entries = Vec::new();
        let mut at = 0;
        while at < data.len() {
            let len = data[at] as usize;
            if len == 0 {
                // Records never cross sectors: skip the padding.
                at = (at / USER_DATA as usize + 1) * USER_DATA as usize;
                continue;
            }
            let e = parse_entry(slice(&data, at, len)?)?;
            if e.name != "." && e.name != ".." {
                entries.push(e);
            }
            at += len;
        }
        self.dirs.borrow_mut().insert(dir.lba, entries.clone());
        Ok(entries)
    }

    fn lookup(&self, path: &str) -> Option<Entry> {
        let mut stack: Vec<Entry> = vec![self.root.clone()];
        for part in path.split(['/', '\\']) {
            match part {
                "" | "." => {}
                ".." => {
                    if stack.len() > 1 {
                        stack.pop();
                    }
                }
                name => {
                    let cur = stack.last()?;
                    if !cur.dir {
                        return None;
                    }
                    let next = self.list(cur).ok()?.into_iter().find(|e| e.name.eq_ignore_ascii_case(name))?;
                    stack.push(next);
                }
            }
        }
        stack.pop()
    }

    pub fn exists(&self, path: &str) -> bool {
        self.lookup(path).is_some()
    }

    pub fn size(&self, path: &str) -> Option<u64> {
        self.lookup(path).filter(|e| !e.dir).map(|e| e.size as u64)
    }

    pub fn read(&self, path: &str) -> Result<Vec<u8>> {
        let e =
            self.lookup(path).filter(|e| !e.dir).with_context(|| format!("{path}: no such file in the CD image"))?;
        self.read_sectors(e.lba as u64, e.size as u64)
    }

    /// Save audio track `number` as a 16-bit stereo 44.1 kHz WAV file.
    pub fn save_audio_track(&self, number: u32, dest: &Path) -> Result<()> {
        let t = self
            .tracks
            .iter()
            .find(|t| t.number == number && t.audio)
            .with_context(|| format!("no audio track {number}"))?;
        let len = t.sectors * RAW_SECTOR;
        let data_len = u32::try_from(len).context("audio track too long for a WAV file")?;

        let mut src = File::open(&t.file)?;
        src.seek(SeekFrom::Start(t.start))?;
        let mut out = BufWriter::new(File::create(dest).with_context(|| format!("cannot create {}", dest.display()))?);
        out.write_all(b"RIFF")?;
        out.write_all(&(36 + data_len).to_le_bytes())?;
        out.write_all(b"WAVEfmt ")?;
        out.write_all(&16u32.to_le_bytes())?;
        out.write_all(&1u16.to_le_bytes())?; // PCM
        out.write_all(&2u16.to_le_bytes())?; // channels
        out.write_all(&44100u32.to_le_bytes())?;
        out.write_all(&(44100u32 * 4).to_le_bytes())?; // bytes per second
        out.write_all(&4u16.to_le_bytes())?; // block align
        out.write_all(&16u16.to_le_bytes())?; // bits per sample
        out.write_all(b"data")?;
        out.write_all(&data_len.to_le_bytes())?;
        let copied = std::io::copy(&mut src.take(len), &mut out)?;
        ensure!(copied == len, "audio track {number} is truncated");
        out.flush()?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("orcrust-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A tiny ISO 9660 image: /DATA/FILE.TXT containing "hello".
    fn iso() -> Vec<u8> {
        let mut img = vec![0u8; 24 * 2048];
        let rec = |lba: u32, size: u32, dir: bool, name: &[u8]| {
            let mut r = vec![0u8; 33];
            r[2..6].copy_from_slice(&lba.to_le_bytes());
            r[10..14].copy_from_slice(&size.to_le_bytes());
            r[25] = if dir { 2 } else { 0 };
            r[32] = name.len() as u8;
            r.extend(name);
            if r.len() % 2 == 1 {
                r.push(0);
            }
            r[0] = r.len() as u8;
            r
        };
        let pvd = 16 * 2048;
        img[pvd] = 1;
        img[pvd + 1..pvd + 6].copy_from_slice(b"CD001");
        img[pvd + 128..pvd + 130].copy_from_slice(&2048u16.to_le_bytes());
        let root = rec(20, 2048, true, &[0]);
        img[pvd + 156..pvd + 156 + 34].copy_from_slice(&root);
        img[17 * 2048] = 255;
        img[17 * 2048 + 1..17 * 2048 + 6].copy_from_slice(b"CD001");

        let mut root_dir = rec(20, 2048, true, &[0]);
        root_dir.extend(rec(20, 2048, true, &[1]));
        root_dir.extend(rec(21, 2048, true, b"DATA"));
        img[20 * 2048..20 * 2048 + root_dir.len()].copy_from_slice(&root_dir);
        let data_dir = rec(22, 5, false, b"FILE.TXT;1");
        img[21 * 2048..21 * 2048 + data_dir.len()].copy_from_slice(&data_dir);
        img[22 * 2048..22 * 2048 + 5].copy_from_slice(b"hello");
        img
    }

    /// Wrap 2048-byte sectors into raw MODE1/2352 sectors.
    fn raw(iso: &[u8]) -> Vec<u8> {
        iso.chunks(2048)
            .flat_map(|s| {
                let mut r = SYNC.to_vec();
                r.extend([0, 0, 0, 1]);
                r.extend(s);
                r.resize(2352, 0);
                r
            })
            .collect()
    }

    #[test]
    fn reads_iso_files() {
        let dir = temp_dir("iso");
        let path = dir.join("t.iso");
        std::fs::write(&path, iso()).unwrap();
        let img = CdImage::open(&path).unwrap();
        assert_eq!(img.read("data/file.txt").unwrap(), b"hello");
        assert_eq!(img.size("/DATA/FILE.TXT"), Some(5));
        assert!(img.exists("DATA/../DATA/FILE.TXT"));
        assert!(!img.exists("DATA/OTHER"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn reads_bin_cue_with_audio() {
        let dir = temp_dir("cue");
        let mut bin = raw(&iso());
        let audio: Vec<u8> = (0..2352 * 3).map(|i| i as u8).collect();
        bin.extend(&audio);
        std::fs::write(dir.join("game.bin"), &bin).unwrap();
        std::fs::write(
            dir.join("game.cue"),
            "FILE \"GAME.BIN\" BINARY\n  TRACK 01 MODE1/2352\n    INDEX 01 00:00:00\n  TRACK 02 AUDIO\n    PREGAP 00:02:00\n    INDEX 01 00:00:24\n",
        )
        .unwrap();

        let img = CdImage::open(&dir.join("game.cue")).unwrap();
        assert_eq!(img.tracks.len(), 2);
        assert_eq!(img.tracks[1].sectors, 3);
        assert_eq!(img.read("DATA/FILE.TXT").unwrap(), b"hello");

        let wav = dir.join("t.wav");
        img.save_audio_track(2, &wav).unwrap();
        let w = std::fs::read(&wav).unwrap();
        assert_eq!(&w[..4], b"RIFF");
        assert_eq!(&w[44..], &audio[..]);

        // Opening the .bin finds the .cue next to it.
        assert_eq!(CdImage::open(&dir.join("game.bin")).unwrap().tracks.len(), 2);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn cue_words() {
        assert_eq!(words("FILE \"My Game.bin\" BINARY"), ["FILE", "My Game.bin", "BINARY"]);
        assert_eq!(msf("01:02:03").unwrap(), 4653);
    }
}
