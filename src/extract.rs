//! The `wartool` extraction driver.
//!
//! Detects which Warcraft II release is in the source (a directory or a CD
//! image, see [`Source`]), then walks
//! the extraction plan ([`TODO`]) converting every archive entry into the
//! files Wargus loads, and finally writes `scripts/wc2-config.lua`.
//!
//! The output layout and file formats are those of the original wartool
//! 3.3.3, so data extracted by either tool is interchangeable.

use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::Path;
use std::process::Command;

use anyhow::{bail, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::archive::Archive;
use crate::bytes::{cstr, le16};
use crate::cd::CdType;
use crate::control::Kind;
use crate::gfx;
use crate::image::{convert_palette, save_png, Indexed, Region};
use crate::pud::pud_to_stratagus;
use crate::rip_music::{rip_music, rip_music_from_image, BNE_MUSIC_NAMES, MUSIC_NAMES};
use crate::source::Source;
use crate::tables::{EXPANSION_PUDS, GROUPED_GRAPHICS, NAMES, ORIGINAL_PUDS, TODO};
use crate::text::convert_string;

/// Version of the extracted data format. The launcher compares it with the
/// `extracted` marker to decide whether to re-extract, so it must stay in
/// sync with the original wartool this port is compatible with.
pub const VERSION: &str = "3.3.3";

pub const NAME_LINE: &str = concat!(
    "wartool V3.3.3 for Stratagus, (c) 1998-2022 by The Stratagus Project.\n",
    "  Written by Lutz Sammer, Nehal Mistry, Jimmy Salmon, Pali Rohar and cybermind.\n",
    "  Rust port by the orcrust project.\n",
    "  https://wargus.github.io"
);

const TILESET_PATH: &str = "graphics/tilesets";
const GRAPHICS_PATH: &str = "graphics";
const PUD_PATH: &str = ".";
const FONT_PATH: &str = "graphics/ui/fonts";
const CURSOR_PATH: &str = "graphics/ui";
const SOUND_PATH: &str = "sounds";
const MUSIC_PATH: &str = "music";
const TEXT_PATH: &str = "campaigns";

/// Where archives from an expansion CD are copied, so a later
/// re-extraction can pick them up together with the original CD.
pub const EXPANSION_SUBDIR: &str = "wargus.exp.data";

/// Marker telling the launcher the archives were copied into the data dir.
pub const REEXTRACT_MARKER_FILE: &str = "data_copied.wargus";

/// Size of `REZDAT.WAR` on the expansion CD, and of `War Resources` on the
/// Mac expansion CD.
const DOS_EXPANSION_REZDAT_SIZE: u64 = 2_811_086;
const MAC_EXPANSION_RESOURCES_SIZE: u64 = 2_876_978;

/// Index of the first campaign level text in [`TODO`]; the campaign
/// objective files are named after those entries.
const FIRST_LEVEL_TEXT: usize = 6;

pub struct Options {
    /// `Some(true)` for `-e`, `Some(false)` for `-n`, `None` to autodetect.
    pub expansion: Option<bool>,
    /// `-v`: convert the videos (needs ffmpeg; skipped if it is missing).
    pub video: bool,
    /// `-r`: rip the CD audio tracks (from a CD image, or with cdparanoia
    /// from the real CD).
    pub rip: bool,
    /// Directory with the archives, or a CD image (`.cue`, `.bin`, `.iso`).
    pub archive_dir: String,
    pub dest_dir: String,
    /// Command line arguments (without the program name), used to run the
    /// extractor again on a copied expansion CD.
    pub args: Vec<String>,
    /// Index of `archive_dir` in `args`.
    pub archive_arg: usize,
}

fn exists(path: &str) -> bool {
    Path::new(path).exists()
}

/// Create every directory leading to `path`.
fn check_path(path: &str) {
    if let Some(parent) = Path::new(path).parent() {
        let _ = fs::create_dir_all(parent);
    }
}

fn write_gz(path: &str, data: &[u8]) -> Result<()> {
    let file = File::create(path).with_context(|| format!("Can't open {path}"))?;
    let mut gz = GzEncoder::new(BufWriter::new(file), Compression::best());
    gz.write_all(data)?;
    gz.finish()?.flush()?;
    Ok(())
}

/// Archive names of the Mac release.
fn mac_name(file: &str) -> &str {
    match file {
        "rezdat.war" => "War Resources",
        "strdat.war" => "War Strings",
        "maindat.war" => "War Data",
        "snddat.war" => "War Music",
        "muddat.cud" => "War Movies",
        "sfxdat.sud" => "War Sounds",
        other => other,
    }
}

fn detect_cd(src: &Source, archive_dir: &str, expansion_opt: i32) -> Result<(CdType, i32)> {
    let mut cd = CdType::default();
    let mut expansion_cd = expansion_opt;

    for (tome, label) in
        [("support/tomes/tome.1", ""), ("Support/TOMES/TOME.1", " Captialized"), ("SUPPORT/TOMES/TOME.1", " Uppercase")]
    {
        if src.exists(&format!("{archive_dir}/{tome}")) {
            println!("Detected BNE CD{label}");
            bail!(
                "The Battle.net edition is not supported by this version of the extractor tool \
                 yet (it needs MPQ support). Please use the DOS CD data."
            );
        }
    }

    let mut rez = format!("{archive_dir}/rezdat.war");
    let mut strdat = format!("{archive_dir}/strdat.war");
    if !src.exists(&rez) {
        rez = format!("{archive_dir}/REZDAT.WAR");
        strdat = format!("{archive_dir}/STRDAT.WAR");
        cd |= CdType::UPPER;
    }

    if !src.exists(&rez) {
        cd |= CdType::MAC | CdType::US;
        let Some(size) = src.size(&format!("{archive_dir}/War Resources")) else {
            // Signalled to the caller, which may retry on a copied expansion.
            return Ok((cd, i32::MIN));
        };
        if expansion_cd == -1 || (expansion_cd != 1 && size != MAC_EXPANSION_RESOURCES_SIZE) {
            println!("Detected original MAC CD");
        } else {
            println!("Detected expansion MAC CD");
            cd |= CdType::EXPANSION;
        }
        return Ok((cd, expansion_cd));
    }

    let strdat_size = src.size(&strdat).unwrap_or(0);
    if src.size(&rez) != Some(DOS_EXPANSION_REZDAT_SIZE) {
        expansion_cd = 0;
        let detected = match strdat_size {
            51550 => Some((CdType::US, "US original DOS CD")),
            53874 => Some((CdType::ITALIAN, "Italian original DOS CD")),
            55014 => Some((CdType::SPANISH, "Spanish original DOS CD")),
            55724 => Some((CdType::GERMAN, "German original DOS CD")),
            51451 => Some((CdType::UK, "UK/Australian original DOS CD")),
            52883 => Some((CdType::PORTUGUESE, "Portuguese original DOS CD")),
            55079 => Some((CdType::FRENCH, "French original DOS CD")),
            52152 => Some((CdType::RUSSIAN, "Russian SPK DOS CD")),
            _ => None,
        };
        match detected {
            Some((flag, label)) => {
                println!("Detected {label}");
                cd |= flag;
            }
            None => {
                println!("Could not detect CD version:");
                println!("Defaulting to German original DOS CD");
                cd |= CdType::GERMAN;
            }
        }
    } else if strdat_size == 74422 {
        println!("Detected Russian expansion DOS CD");
        cd |= CdType::EXPANSION | CdType::RUSSIAN;
    } else {
        println!("Detected expansion DOS CD");
        cd |= CdType::EXPANSION | CdType::US;
    }
    Ok((cd, expansion_cd))
}

struct Extractor {
    src: Source,
    archive_dir: String,
    dir: String,
    cd: CdType,
    expansion: bool,
    archive: Option<Archive>,
    /// Unit / command names, 1-based (index 0 is unused).
    unit_names: Vec<String>,
    game_font_width: usize,
    /// Raw palette 27, shared by the credits background and its cursor.
    pal27: Option<Vec<u8>>,
}

impl Extractor {
    fn entry(&self, index: i32) -> Result<Vec<u8>> {
        let archive = self.archive.as_ref().context("no archive is open")?;
        let index = usize::try_from(index).with_context(|| format!("invalid archive entry {index}"))?;
        archive.entry(index)
    }

    fn palette(&self, index: i32) -> Result<[u8; 768]> {
        Ok(convert_palette(&self.entry(index)?))
    }

    /// `<dest>/<rel>`, with its parent directories created.
    fn out(&self, rel: &str) -> String {
        let path = format!("{}/{rel}", self.dir);
        check_path(&path);
        path
    }

    /// PNG write failures are reported but not fatal, like the original.
    fn png(&self, path: &str, img: &Indexed, region: Region, pal: &[u8; 768], transparent: bool) {
        if let Err(e) = save_png(Path::new(path), img, region, pal, transparent) {
            println!("{path}: {e:#}");
        }
    }

    fn png_whole(&self, path: &str, img: &Indexed, pal: &[u8; 768], transparent: bool) {
        self.png(path, img, Region::whole(img), pal, transparent);
    }

    fn convert_rgb(&self, file: &str, rgbe: i32) -> Result<()> {
        let raw = self.entry(rgbe)?;
        let mut rgb = raw.clone();
        for b in rgb.iter_mut().take(768) {
            *b <<= 2;
        }
        let path = self.out(&format!("{TILESET_PATH}/{file}.rgb"));
        fs::write(&path, &rgb).with_context(|| format!("Can't open {path}"))?;

        let pal = convert_palette(&raw);
        let mut capitalized = file.to_string();
        if let Some(first) = capitalized.get_mut(..1) {
            first.make_ascii_uppercase();
        }
        let mut gimp = format!("GIMP Palette\n# Stratagus {capitalized} -- GIMP Palette file\n");
        for (i, c) in pal.chunks(3).enumerate() {
            gimp += &format!("{} {} {}\t#{i}\n", c[0], c[1], c[2]);
        }
        let path = self.out(&format!("{TILESET_PATH}/{file}.gimp"));
        fs::write(&path, gimp).with_context(|| format!("Can't open {path}"))?;
        Ok(())
    }

    fn convert_tileset(&self, file: &str, pale: i32, mege: i32, mine: i32, mape: i32) -> Result<()> {
        let pal = self.palette(pale)?;
        let mega = self.entry(mege)?;
        let mini = self.entry(mine)?;
        self.entry(mape)?; // map table, not needed for the image
        let img = gfx::convert_tileset(&mini, &mega)?;
        self.png_whole(&self.out(&format!("{TILESET_PATH}/{file}.png")), &img, &pal, true);
        Ok(())
    }

    fn convert_gfx(&self, file: &str, pale: i32, gfxe: i32, gfxe2: i32, start2: i32) -> Result<()> {
        let pal = self.palette(pale)?;
        let gfx = self.entry(gfxe)?;
        let alt = if gfxe2 != 0 { Some(self.entry(gfxe2)?) } else { None };
        let img = gfx::convert_graphic(true, &gfx, alt.as_deref(), start2.max(0) as usize)?;
        self.png_whole(&self.out(&format!("{GRAPHICS_PATH}/{file}.png")), &img, &pal, true);
        Ok(())
    }

    fn convert_gfu(&self, file: &str, pale: i32, gfue: i32) -> Result<()> {
        let pal = self.palette(pale)?;
        let img = gfx::convert_graphic(false, &self.entry(gfue)?, None, 0)?;
        self.png_whole(&self.out(&format!("{GRAPHICS_PATH}/{file}.png")), &img, &pal, true);
        Ok(())
    }

    /// Cut a UI sheet into the named pieces of [`GROUPED_GRAPHICS`].
    fn convert_grouped_gfu(&self, path: &str, pale: i32, gfue: i32, glist: i32) -> Result<()> {
        let mut pal = self.palette(pale)?;
        let img = gfx::convert_graphic(false, &self.entry(gfue)?, None, 0)?;
        let group = GROUPED_GRAPHICS.get(glist as usize).context("unknown graphic group")?;
        for (i, gg) in group.iter().enumerate() {
            // The original CD has fewer widgets than the expansion.
            if (gg.y + gg.height) as usize > img.height {
                break;
            }
            // Widgets from the 4th on use another palette.
            if i == 3 && path.contains("widgets") {
                pal = self.palette(14)?;
            }
            let region =
                Region { x: gg.x as usize, y: gg.y as usize, width: gg.width as usize, height: gg.height as usize };
            self.png(&self.out(&format!("{GRAPHICS_PATH}/{path}/{}.png", gg.name)), &img, region, &pal, true);
        }
        Ok(())
    }

    fn convert_pud(&self, file: &str, pude: i32) -> Result<()> {
        let data = self.entry(pude)?;
        let path = self.out(&format!("{PUD_PATH}/{file}"));
        let (outdir, _) = path.rsplit_once('/').unwrap_or((".", ""));
        let name = file.rsplit('/').next().unwrap_or(file);
        pud_to_stratagus(&data, name, outdir)
    }

    /// Convert the PUDs that are loose files on the CD rather than archive
    /// entries.
    fn convert_file_puds(&self, list: &[&str]) -> Result<()> {
        let upper = self.cd.any(CdType::UPPER);
        for &entry in list {
            let rel = if upper { entry.to_uppercase() } else { entry.to_string() };
            let path = format!("{}/{rel}", self.archive_dir);
            if !self.src.exists(&path) {
                continue;
            }
            let Ok(data) = self.src.read(&path) else { return Ok(()) };

            let mut base = rel.rsplit('/').next().unwrap_or(&rel).to_string();
            if let Some(p) = base.find(if upper { ".PUD" } else { ".pud" }) {
                base.truncate(p);
            }
            let mut outdir = match entry.find("puds/") {
                Some(p) => format!("{}/maps/{}", self.dir, &entry[p + 5..]),
                None => format!("{}/maps/", self.dir),
            };
            if let Some(p) = outdir.rfind('/') {
                outdir.truncate(p);
            }
            let _ = fs::create_dir_all(&outdir);
            pud_to_stratagus(&data, &base, &outdir)?;
        }
        Ok(())
    }

    fn convert_font(&mut self, file: &str, pale: i32, fnte: i32) -> Result<()> {
        let pal = self.palette(pale)?;
        let russian = self.cd.any(CdType::RUSSIAN);
        let mut img = gfx::convert_font(&self.entry(fnte)?, russian)?;
        if file == "game" {
            self.game_font_width = img.width / 15;
        }
        if russian {
            gfx::fix_russian_font(&mut img);
        }
        self.png_whole(&self.out(&format!("{FONT_PATH}/{file}.png")), &img, &pal, true);
        Ok(())
    }

    fn convert_image(&mut self, file: &str, mut pale: i32, mut imge: i32, nw: i32, nh: i32) -> Result<()> {
        // The Mac expansion CD has 7 extra entries before these.
        if self.cd.any(CdType::MAC) {
            if (94..=103).contains(&imge) {
                imge += 7;
            }
            if pale == 93 {
                pale += 7;
            }
        }
        let raw_pal = self.entry(pale)?;
        if pale == 27 && imge == 28 {
            self.pal27 = Some(raw_pal.clone());
        }
        let mut img = match gfx::convert_image(&self.entry(imge)?) {
            Ok(img) => img,
            Err(e) => {
                println!(
                    "Please report this bug, could not extract image: file={file} pale={pale} imge={imge} nw={nw} nh={nh} mac={}",
                    self.cd.any(CdType::MAC) as i32
                );
                return Err(e.context("This version of the CD is not supported"));
            }
        };
        if nw > 0 && nh > 0 {
            img = gfx::resize(&img, nw as usize, nh as usize);
        }
        self.png_whole(&self.out(&format!("{GRAPHICS_PATH}/{file}.png")), &img, &convert_palette(&raw_pal), false);
        Ok(())
    }

    fn convert_cursor(&self, file: &str, pale: i32, cure: i32) -> Result<()> {
        // The credits arrow uses the credits background palette.
        let raw_pal = match (&self.pal27, pale, cure) {
            (Some(p), 27, 314) => p.clone(),
            _ => self.entry(pale)?,
        };
        let img = gfx::convert_cursor(&self.entry(cure)?)?;
        self.png_whole(&self.out(&format!("{CURSOR_PATH}/{file}.png")), &img, &convert_palette(&raw_pal), true);
        Ok(())
    }

    fn convert_wav(&self, file: &str, wave: i32) -> Result<()> {
        let data = self.entry(wave)?;
        write_gz(&self.out(&format!("{SOUND_PATH}/{file}.wav.gz")), &data)
    }

    fn convert_xmi(&self, file: &str, xmi: i32) -> Result<()> {
        let data = self.entry(xmi)?;
        let Some(mid) = crate::xmi2mid::xmi_to_mid(&data) else {
            println!("Can't convert music {file}: not a valid XMI file");
            return Ok(());
        };
        let path = self.out(&format!("{MUSIC_PATH}/{file}.mid"));
        fs::write(&path, mid).with_context(|| format!("Can't open {path}"))
    }

    fn convert_text(&self, file: &str, mut txte: i32, ofs: i32) -> Result<()> {
        // The German, UK and Russian original CDs lack one entry before these.
        if !self.cd.any(CdType::EXPANSION) && self.cd.any(CdType::GERMAN | CdType::UK | CdType::RUSSIAN) {
            txte -= 1;
        }
        if self.cd.any(CdType::MAC) && txte >= 99 {
            txte += 6;
        }
        let data = self.entry(txte)?;
        let text =
            data.get(ofs.max(0) as usize..).with_context(|| format!("text entry {txte} is shorter than {ofs}"))?;
        let mut out = convert_string(text, self.cd.code_page()).into_bytes();
        out.push(0);
        write_gz(&self.out(&format!("{TEXT_PATH}/{file}.txt.gz")), &out)
    }

    /// Load the unit names used to name sprite files (see [`Self::parse_string`]).
    fn setup_names(&mut self) -> Result<()> {
        let n = le16(NAMES, 0)? as usize;
        self.unit_names = vec![String::new(); n.max(1)];
        for u in 1..n {
            let offset = le16(NAMES, u * 2)? as usize;
            self.unit_names[u] = convert_string(cstr(NAMES, offset), self.cd.code_page());
        }
        Ok(())
    }

    /// Expand `%N` (unit name N) and `%-N` (unit name N without its first
    /// word) in a file name, lower-cased with spaces and dashes as `_`.
    fn parse_string(&self, input: &str) -> Result<String> {
        let mut out = String::new();
        let mut rest = input;
        while let Some(p) = rest.find('%') {
            out += &rest[..p];
            rest = &rest[p + 1..];
            let skip_word = rest.starts_with('-');
            if skip_word {
                rest = &rest[1..];
            }
            let digits = rest.bytes().take_while(u8::is_ascii_digit).count();
            let index: usize = rest[..digits].parse().with_context(|| format!("bad name reference in {input}"))?;
            rest = &rest[digits..];
            let mut name = self
                .unit_names
                .get(index)
                .filter(|_| index > 0)
                .with_context(|| format!("unknown unit name {index} in {input}"))?
                .as_str();
            if skip_word {
                name = name
                    .split_once(' ')
                    .map(|(_, r)| r)
                    .with_context(|| format!("unit name {index} has a single word"))?;
            }
            out.extend(name.chars().map(|c| match c {
                '-' | ' ' => '_',
                c => c.to_ascii_lowercase(),
            }));
        }
        out += rest;
        Ok(out)
    }

    fn campaigns_create(&self) -> Result<()> {
        // Campaign data is in different spots for different CDs.
        let (expansion, txte, ofs) = if self.cd.any(CdType::EXPANSION) {
            (true, 54, 236)
        } else {
            let txte = if self.cd.any(CdType::UK | CdType::GERMAN | CdType::RUSSIAN) { 53 } else { 54 };
            let ofs = if self.cd.any(CdType::SPANISH) { 172 } else { 140 };
            (false, txte, ofs)
        };
        let mut data = self.entry(txte)?;
        data.push(0);
        self.campaigns_load_data(&data, expansion, ofs)
    }

    /// Write `campaigns/<level>_c2.sms` with each level's title and
    /// objectives. The table holds all objectives (human and orc levels
    /// interleaved), then, after some unrelated strings, all titles.
    fn campaigns_load_data(&self, data: &[u8], expansion: bool, offset: usize) -> Result<()> {
        #[derive(Clone, Default)]
        struct Level<'a> {
            title: &'a [u8],
            objectives: Vec<&'a [u8]>,
        }
        let count = if expansion { 52 } else { 28 };
        let mut levels = vec![[Level::default(), Level::default()]; count / 2];

        let mut cur = offset;
        for l in 0..count {
            let s = cstr(data, cur);
            levels[l / 2][l % 2].objectives = s.split(|&b| b == b'\n').take(9).collect();
            cur += s.len() + 1;
        }

        // Skip to the level titles ("I. ...").
        let at = |i: usize| data.get(i).copied().unwrap_or(0);
        while at(cur) != 0 && at(cur) != b'I' && at(cur + 1) != b'.' {
            cur += cstr(data, cur).len() + 1;
        }
        for l in 0..count {
            let s = cstr(data, cur);
            levels[l / 2][l % 2].title = s;
            cur += s.len() + 1;
        }

        let cp = self.cd.code_page();
        for (n, pair) in levels.iter().enumerate() {
            for (race, level) in pair.iter().enumerate() {
                let name = TODO[FIRST_LEVEL_TEXT + 2 * n + race].name();
                let path = self.out(&format!("{TEXT_PATH}/{name}_c2.sms"));
                let objectives: Vec<String> =
                    level.objectives.iter().map(|o| format!("\"{}\"", convert_string(o, cp))).collect();
                let body = format!(
                    "title = \"{}\"\nobjectives = {{{}}}\n",
                    convert_string(level.title, cp),
                    objectives.join(",")
                );
                if fs::write(&path, body).is_err() {
                    println!("Cannot Write File (Skipping Level: {path})");
                }
            }
        }
        Ok(())
    }

    /// Smacker video to Ogg Theora with ffmpeg. Skipped without ffmpeg; the
    /// game runs without its videos.
    fn convert_video(&self, file: &str, video: i32) -> Result<()> {
        if !ffmpeg_available() {
            return Ok(());
        }
        let smk = self.out(&format!("{file}.smk"));
        let ogv = self.out(&format!("{file}.ogv"));
        let backup = format!("{ogv}.bak");
        fs::write(&smk, self.entry(video)?).with_context(|| format!("Can't open {smk}"))?;

        let args = [
            "-y",
            "-i",
            smk.as_str(),
            "-codec:v",
            "libtheora",
            "-qscale:v",
            "31",
            "-codec:a",
            "libvorbis",
            "-qscale:a",
            "15",
            "-pix_fmt",
            "yuv420p",
            ogv.as_str(),
        ];
        println!("ffmpeg {}", args.join(" "));

        let had_output = Path::new(&ogv).exists();
        if had_output {
            let _ = fs::rename(&ogv, &backup);
        }
        let ok = Command::new("ffmpeg").args(args).status().map(|s| s.success()).unwrap_or(false);
        if ok {
            if had_output {
                let _ = fs::remove_file(&backup);
            }
        } else {
            let _ = fs::remove_file(&ogv);
            if had_output {
                let _ = fs::rename(&backup, &ogv);
            } else {
                println!("Can't convert video {file} to ogv format. Is ffmpeg installed in PATH?");
            }
        }
        let _ = fs::remove_file(&smk);
        Ok(())
    }

    /// Copy music ripped beforehand into `<cd>/music`. Returns how many OGG
    /// files were copied.
    fn copy_music(&self) -> usize {
        let mut oggs = 0;
        for name in MUSIC_NAMES {
            let Some((src, ext)) = ["wav", "ogg"]
                .iter()
                .map(|ext| (format!("{}/music/{name}.{ext}", self.archive_dir), *ext))
                .find(|(p, _)| self.src.exists(p))
            else {
                continue;
            };
            println!("Found ripped music file \"{}\"", self.src.describe(&src));
            let dst = self.out(&format!("{MUSIC_PATH}/{name}.{ext}"));
            if !exists(&dst) {
                let copied = match self.src {
                    Source::Dir => fs::copy(&src, &dst).is_ok(),
                    Source::Image(_) => self.src.read(&src).and_then(|d| Ok(fs::write(&dst, d)?)).is_ok(),
                };
                if copied && ext == "ogg" {
                    oggs += 1;
                }
            }
        }
        oggs
    }

    /// Convert ripped WAV music to OGG. Returns (WAV files found, converted).
    fn convert_music(&self) -> (usize, usize) {
        let bne: &[&str] = if self.cd.any(CdType::BNE) { BNE_MUSIC_NAMES } else { &[] };
        let (mut found, mut converted) = (0, 0);
        for name in MUSIC_NAMES.iter().chain(bne) {
            let wav = self.out(&format!("{MUSIC_PATH}/{name}.wav"));
            if !exists(&wav) {
                continue;
            }
            found += 1;
            let ogg = format!("{}/{MUSIC_PATH}/{name}.ogg", self.dir);
            println!("Encoding \"{name}\" to Ogg Vorbis");
            match crate::ogg::encode_wav(Path::new(&wav), Path::new(&ogg)) {
                Ok(()) => {
                    let _ = fs::remove_file(&wav);
                    converted += 1;
                }
                Err(e) => {
                    let _ = fs::remove_file(&ogg);
                    println!("Can't convert wav sound {name} to ogg format: {e:#}");
                }
            }
        }
        (found, converted)
    }

    /// Keep a copy of the archive in the data directory so the launcher can
    /// re-extract without the CD.
    fn copy_archive(&self, partial: &str) -> Result<()> {
        let mut target = format!("{}/", self.dir);
        if self.cd.any(CdType::EXPANSION) && !self.cd.any(CdType::BNE) {
            target += EXPANSION_SUBDIR;
            target += "/";
        }
        target += partial;
        let source = format!("{}/{partial}", self.archive_dir);
        if exists(&target) || (matches!(self.src, Source::Dir) && source == target) {
            return Ok(());
        }
        check_path(&target);
        let shown = self.src.describe(&source);
        match self.src {
            Source::Dir => fs::copy(&source, &target).map(|_| ()).map_err(anyhow::Error::from),
            Source::Image(_) => self.src.read(&source).and_then(|d| Ok(fs::write(&target, d)?)),
        }
        .with_context(|| format!("Cannot copy {shown} to {target}"))?;
        println!("Copied {shown}->{target}");
        Ok(())
    }

    fn write_config(&self, music_extension: &str) -> Result<()> {
        let mut cfg = String::new();
        cfg += "wargus.tales = false\n";
        cfg += &format!("wargus.expansion = {}\n", self.expansion);
        cfg += &format!("wargus.music_extension = \"{music_extension}\"\n");
        cfg += &format!("wargus.bne = {}\n", self.cd.any(CdType::BNE));
        cfg += &format!("wargus.game_font_width = {}\n", self.game_font_width);
        cfg += if self.cd.any(CdType::RUSSIAN) { "SetFontCodePage(866)\n" } else { "SetFontCodePage(437)\n" };
        cfg += "InGameStrings = {\n";
        for name in self.unit_names.iter().skip(1).rev() {
            cfg += &format!("   [[{name}]],\n");
        }
        cfg += "}\n";
        let path = self.out("scripts/wc2-config.lua");
        fs::write(&path, cfg).with_context(|| format!("Can't open {path}"))
    }

    fn run_plan(&mut self, video: bool) -> Result<()> {
        let bne = self.cd.any(CdType::BNE);
        for c in TODO {
            let skip = (!self.expansion && c.version & 2 != 0)
                || (self.expansion && c.version == 3)
                || (bne && c.version & 4 != 0)
                || (!bne && c.version & 8 != 0);
            if skip {
                continue;
            }
            let file = if self.cd.any(CdType::MAC) { mac_name(c.name()) } else { c.name() };
            let (a1, a2, a3, a4) = (c.arg1, c.arg2, c.arg3, c.arg4);
            match c.kind {
                Kind::Archive => {
                    self.archive = None;
                    let file = if self.cd.any(CdType::UPPER) { file.to_uppercase() } else { file.to_string() };
                    let path = format!("{}/{file}", self.archive_dir);
                    println!("Archive \"{}\"", self.src.describe(&path));
                    let id = u16::try_from(a1).context("invalid archive id")?;
                    self.archive = Some(Archive::from_bytes(self.src.read(&path)?, id)?);
                    self.copy_archive(&file)?;
                }
                Kind::Mpq => bail!("Error - not a BNE disk: this version of the CD is not supported"),
                Kind::Rgb => self.convert_rgb(file, a1)?,
                Kind::Tileset => self.convert_tileset(file, a1, a2, a3, a4)?,
                Kind::Gfx => self.convert_gfx(&self.parse_string(file)?, a1, a2, a3, a4)?,
                Kind::Gfu => self.convert_gfu(file, a1, a2)?,
                Kind::GroupedGfu => self.convert_grouped_gfu(file, a1, a2, a3)?,
                Kind::Pud => self.convert_pud(file, a1)?,
                Kind::Font => self.convert_font(file, 2, a1)?,
                Kind::Image => self.convert_image(file, a1, a2, a3, a4)?,
                Kind::Cursor => self.convert_cursor(file, a1, a2)?,
                Kind::Midi => self.convert_xmi(file, a1)?,
                Kind::Wav if self.archive.is_some() => self.convert_wav(file, a1)?,
                Kind::Text if !bne => self.convert_text(file, a1, a2)?,
                Kind::Setup => self.setup_names()?,
                Kind::Video if video => self.convert_video(file, a1)?,
                Kind::Campaign if self.archive.is_some() => self.campaigns_create()?,
                Kind::Wav | Kind::Text | Kind::Video | Kind::Campaign => {}
            }
        }
        Ok(())
    }
}

/// Is ffmpeg (needed for the videos) on `PATH`?
fn ffmpeg_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| {
        let ok = Command::new("ffmpeg").arg("-version").output().is_ok_and(|o| o.status.success());
        if !ok {
            println!("ffmpeg not found: skipping the videos");
        }
        ok
    })
}

/// If the archives of an expansion CD were copied next to the data (see
/// [`EXPANSION_SUBDIR`]), run the extractor again on them. Only returns if
/// there is nothing to do.
fn extract_implicit_expansion(src: &Source, archive_dir: &str, opts: &Options) -> Result<()> {
    println!("Extracting from expansion subdir");
    if !matches!(src, Source::Dir) || archive_dir.contains(EXPANSION_SUBDIR) {
        // Already extracting from the copy; don't recurse.
        return Ok(());
    }
    let subdir = format!("{archive_dir}/{EXPANSION_SUBDIR}");
    if !exists(&subdir) {
        return Ok(());
    }
    let mut args = opts.args.clone();
    args[opts.archive_arg] = subdir;
    let exe = std::env::current_exe().context("cannot locate the wartool executable")?;
    std::io::stdout().flush()?;
    replace_process(Command::new(exe).args(&args))
        .context("an error occurred trying to extract from the expansion archives")
}

#[cfg(unix)]
fn replace_process(cmd: &mut Command) -> Result<()> {
    use std::os::unix::process::CommandExt;
    Err(cmd.exec().into())
}

#[cfg(not(unix))]
fn replace_process(cmd: &mut Command) -> Result<()> {
    let status = cmd.status()?;
    std::process::exit(status.code().unwrap_or(-1));
}

/// Run a complete extraction.
pub fn run(opts: &Options) -> Result<()> {
    let src = Source::open(&opts.archive_dir)?;
    // Paths inside an image are relative to its root.
    let mut archive_dir = if src.image().is_some() { String::new() } else { opts.archive_dir.clone() };
    // Accept the CD root as well as its DATA directory.
    for (sub, rez) in [("data", "rezdat.war"), ("DATA", "REZDAT.WAR")] {
        if src.exists(&format!("{archive_dir}/{sub}/{rez}")) {
            archive_dir = format!("{archive_dir}/{sub}");
            break;
        }
    }
    let dir = opts.dest_dir.clone();

    if let Ok(version) = fs::read_to_string(format!("{dir}/extracted")) {
        if version.lines().next() == Some(VERSION) {
            println!("Note: Data is already extracted in Dir \"{dir}\" with this version of wartool");
        }
    }

    let expansion_opt = match opts.expansion {
        Some(true) => 1,
        Some(false) => -1,
        None => 0,
    };
    if let Some(img) = src.image() {
        let audio = img.tracks.iter().filter(|t| t.audio).count();
        println!("Reading CD image \"{}\" ({} track(s), {audio} audio)", img.path().display(), img.tracks.len());
    }
    let (cd, expansion_cd) = detect_cd(&src, &archive_dir, expansion_opt)?;
    if expansion_cd == i32::MIN {
        // Nothing found: maybe this is a data dir holding a copied expansion CD.
        extract_implicit_expansion(&src, &archive_dir, opts)?;
        println!("Could not find Warcraft 2 Data");
        bail!(
            "Could not find Warcraft 2 data in folder. Make sure you have selected the DATA \
             directory of the DOS version, or the root of the Battle.net CD."
        );
    }
    let expansion = !(expansion_cd == -1 || (expansion_cd != 1 && !cd.any(CdType::EXPANSION)));

    println!("Extract from \"{}\" to \"{dir}\"", src.describe(&archive_dir));
    println!("Please be patient, the data may take a couple of minutes to extract...");

    let mut ex = Extractor {
        src,
        archive_dir: archive_dir.clone(),
        dir: dir.clone(),
        cd,
        expansion,
        archive: None,
        unit_names: Vec::new(),
        game_font_width: 0,
        pal27: None,
    };
    ex.run_plan(opts.video)?;

    ex.convert_file_puds(ORIGINAL_PUDS)?;
    ex.convert_file_puds(EXPANSION_PUDS)?;

    let copied_oggs = ex.copy_music();
    if opts.rip && !cd.any(CdType::BNE) {
        let music_dir = format!("{dir}/{MUSIC_PATH}");
        let _ = fs::create_dir_all(&music_dir);
        match ex.src.image() {
            Some(img) => rip_music_from_image(expansion, img, Path::new(&music_dir)),
            None => rip_music(expansion, Path::new(&archive_dir), Path::new(&music_dir)),
        };
    }
    let (wavs, converted) = ex.convert_music();
    // The original wrote ".wav" whenever nothing was converted, even with no
    // ripped music at all; fall back to the extracted MIDI files instead.
    let music_extension = if converted > 0 || copied_oggs > 0 {
        ".ogg"
    } else if wavs > 0 {
        ".wav"
    } else {
        ".mid"
    };
    ex.archive = None;

    ex.write_config(music_extension)?;
    fs::write(ex.out("extracted"), VERSION).context("Can't write the extracted marker")?;
    fs::write(ex.out(REEXTRACT_MARKER_FILE), "data copied for re-extraction")
        .context("Can't write the re-extraction marker")?;

    println!("Done.");
    extract_implicit_expansion(&ex.src, &archive_dir, opts)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn extractor() -> Extractor {
        let mut ex = Extractor {
            src: Source::Dir,
            archive_dir: String::new(),
            dir: String::new(),
            cd: CdType::US,
            expansion: false,
            archive: None,
            unit_names: Vec::new(),
            game_font_width: 0,
            pal27: None,
        };
        ex.setup_names().unwrap();
        ex
    }

    #[test]
    fn unit_names_expand_in_file_names() {
        let ex = extractor();
        assert_eq!(ex.parse_string("human/units/%2").unwrap(), "human/units/footman");
        assert_eq!(ex.parse_string("human/units/%16").unwrap(), "human/units/dwarven_demolition_squad");
        assert_eq!(ex.parse_string("orc/units/%-31").unwrap(), "orc/units/transport");
        assert_eq!(ex.parse_string("human/units/%-28_empty").unwrap(), "human/units/oil_tanker_empty");
        assert_eq!(ex.parse_string("plain").unwrap(), "plain");
        assert!(ex.parse_string("x/%9999").is_err());
    }

    #[test]
    fn campaign_level_names_line_up() {
        assert_eq!(TODO[FIRST_LEVEL_TEXT].name(), "human/level01h");
        assert_eq!(TODO[FIRST_LEVEL_TEXT + 1].name(), "orc/level01o");
        assert_eq!(TODO[FIRST_LEVEL_TEXT + 51].name(), "orc-exp/levelx12o");
    }

    #[test]
    fn campaign_objectives() {
        let dir = std::env::temp_dir().join(format!("orcrust-campaign-{}", std::process::id()));
        let mut ex = extractor();
        ex.dir = dir.to_string_lossy().into_owned();

        let mut data = b"padding\0".to_vec();
        for l in 0..28 {
            data.extend(format!("-Objective {l}\n-Second\0").bytes());
        }
        data.extend(b"unrelated\0");
        for l in 0..28 {
            data.extend(format!("{}. Title {l}\0", if l == 0 { "I" } else { "X" }).bytes());
        }
        data.push(0);
        ex.campaigns_load_data(&data, false, 8).unwrap();

        let orc2 = fs::read_to_string(dir.join("campaigns/orc/level02o_c2.sms")).unwrap();
        assert_eq!(orc2, "title = \"X. Title 3\"\nobjectives = {\"-Objective 3\",\"-Second\"}\n");
        fs::remove_dir_all(dir).unwrap();
    }
}
