//! Rip the CD audio soundtrack, from a CD image or with `cdparanoia`.
//!
//! The DOS CDs carry the music as red book audio tracks (track 2 onwards).
//! From a `.cue`/`.bin` image the tracks are simply copied out. From a
//! physical CD the device is found from the mount point of the data
//! directory, which is only implemented for Linux (like the original on
//! non-FreeBSD Unix).

use std::path::{Path, PathBuf};
use std::process::Command;

use crate::cdimage::CdImage;

/// Names of the CD audio tracks, in track order starting at track 2.
pub const MUSIC_NAMES: &[&str] = &[
    "Human Battle 1",
    "Human Battle 2",
    "Human Battle 3",
    "Human Battle 4",
    "Human Battle 5",
    "Human Briefing",
    "Human Victory",
    "Human Defeat",
    "Orc Battle 1",
    "Orc Battle 2",
    "Orc Battle 3",
    "Orc Battle 4",
    "Orc Battle 5",
    "Orc Briefing",
    "Orc Victory",
    "Orc Defeat",
    // Only on the expansion CD.
    "I'm a Medieval Man",
];

/// Extra tracks of the Battle.net edition.
pub const BNE_MUSIC_NAMES: &[&str] = &["Human Battle 6", "Orc Battle 6"];

/// Walk up from `dir` to the root of the file system it lives on.
#[cfg(unix)]
fn find_mount_dir(dir: &Path) -> Option<PathBuf> {
    use std::os::unix::fs::MetadataExt;

    let mut last =
        std::fs::canonicalize(dir).map_err(|e| eprintln!("Error: Cannot resolve {}: {e}", dir.display())).ok()?;
    let md = std::fs::metadata(&last).ok()?;
    let (mut dev, mut ino) = (md.dev(), md.ino());
    loop {
        let Some(parent) = last.parent().map(Path::to_path_buf) else {
            return Some(last);
        };
        let md =
            std::fs::metadata(&parent).map_err(|e| eprintln!("Error: Cannot stat {}: {e}", parent.display())).ok()?;
        if md.dev() != dev || md.ino() == ino {
            return Some(last);
        }
        if parent == Path::new("/") {
            return Some(parent);
        }
        (dev, ino) = (md.dev(), md.ino());
        last = parent;
    }
}

#[cfg(not(unix))]
fn find_mount_dir(_dir: &Path) -> Option<PathBuf> {
    None
}

/// Undo the octal escapes (`\040` for space, ...) used in `/proc/mounts`.
#[cfg(any(target_os = "linux", test))]
fn unescape_mount_field(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'\\' && i + 3 < b.len() && b[i + 1..i + 4].iter().all(|c| (b'0'..=b'7').contains(c)) {
            out.push((b[i + 1] - b'0') * 64 + (b[i + 2] - b'0') * 8 + (b[i + 3] - b'0'));
            i += 4;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(target_os = "linux")]
fn find_device(mount_dir: &Path) -> Option<String> {
    let mounts = std::fs::read_to_string("/proc/self/mounts")
        .map_err(|e| eprintln!("Error: Cannot open file /proc/self/mounts: {e}"))
        .ok()?;
    mounts.lines().find_map(|line| {
        let mut fields = line.split_whitespace();
        let dev = unescape_mount_field(fields.next()?);
        let dir = unescape_mount_field(fields.next()?);
        (Path::new(&dir) == mount_dir).then_some(dev)
    })
}

#[cfg(not(target_os = "linux"))]
fn find_device(_mount_dir: &Path) -> Option<String> {
    None
}

fn cdparanoia(args: &[&str]) -> bool {
    match Command::new("cdparanoia").args(args).status() {
        Ok(s) => s.success(),
        Err(e) => {
            eprintln!("Error: Cannot exec cdparanoia: {e}");
            false
        }
    }
}

/// Rip the soundtrack of the CD mounted at `data_dir` into
/// `dest_dir/<name>.wav`. Returns true if at least one track was ripped.
pub fn rip_music(expansion_cd: bool, data_dir: &Path, dest_dir: &Path) -> bool {
    let Some(mount_dir) = find_mount_dir(data_dir) else {
        eprintln!("Error: Cannot find mount dir");
        return false;
    };
    let Some(dev) = find_device(&mount_dir) else {
        eprintln!("Error: Cannot find device for mount dir {}", mount_dir.display());
        return false;
    };
    println!("Found CD-ROM device: {dev}");

    if !cdparanoia(&["-d", &dev, "-v", "-Q"]) {
        return false;
    }

    let tracks = if expansion_cd { MUSIC_NAMES.len() } else { MUSIC_NAMES.len() - 1 };
    let mut count = 0;
    for (i, name) in MUSIC_NAMES.iter().take(tracks).enumerate() {
        let track = (i + 2).to_string();
        let file = dest_dir.join(format!("{name}.wav"));
        if cdparanoia(&["-d", &dev, "-v", &track, &file.to_string_lossy()]) {
            count += 1;
        }
    }
    count > 0
}

/// "Rip" the soundtrack from the audio tracks of a CD image into
/// `dest_dir/<name>.wav`. Returns true if at least one track was saved.
pub fn rip_music_from_image(expansion_cd: bool, image: &CdImage, dest_dir: &Path) -> bool {
    let tracks = if expansion_cd { MUSIC_NAMES.len() } else { MUSIC_NAMES.len() - 1 };
    let mut count = 0;
    for (i, name) in MUSIC_NAMES.iter().take(tracks).enumerate() {
        let number = i as u32 + 2;
        let Some(track) = image.tracks.iter().find(|t| t.number == number && t.audio) else {
            println!("No audio track {number} ({name}) in the CD image");
            continue;
        };
        let file = dest_dir.join(format!("{name}.wav"));
        println!(
            "Saving audio track {number} ({}:{:02}) as \"{}\"",
            track.seconds() / 60,
            track.seconds() % 60,
            file.display()
        );
        match image.save_audio_track(number, &file) {
            Ok(()) => count += 1,
            Err(e) => eprintln!("Error: {e:#}"),
        }
    }
    count > 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_mount_points() {
        assert_eq!(unescape_mount_field("/media/WAR\\0402"), "/media/WAR 2");
        assert_eq!(unescape_mount_field("/plain"), "/plain");
    }
}
