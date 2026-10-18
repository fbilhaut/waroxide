//! PUD map files: parsing and conversion to Stratagus map scripts.
//!
//! A PUD is a sequence of sections, each a 4-byte tag and a `u32` length.
//! The conversion produces a `.smp` presentation file (players, size,
//! description) and a `.sms` setup file (tiles, units, resources), both
//! gzip-compressed Lua.

use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

use anyhow::{bail, Context, Result};
use flate2::write::GzEncoder;
use flate2::Compression;

use crate::bytes::{cstr, le16, le32, slice, u8_at};

pub const PLAYER_MAX: usize = 16;

const VERSION: &str = "1.0";

const PLAYER_NEUTRAL: u8 = 2;
const PLAYER_NOBODY: u8 = 3;

const UNIT_HUMAN_OIL_PLATFORM: u8 = 0x56;
const UNIT_ORC_OIL_PLATFORM: u8 = 0x57;
const UNIT_GOLD_MINE: u8 = 0x5C;
const UNIT_OIL_PATCH: u8 = 0x5D;
const UNIT_HUMAN_START: u8 = 0x5E;
const UNIT_ORC_START: u8 = 0x5F;

const PLAYER_TYPES: [&str; 8] = ["", "", "neutral", "nobody", "computer", "person", "rescue-passive", "rescue-active"];

const RACE_NAMES: [&str; 3] = ["human", "orc", "neutral"];

const TILESET_NAMES: [&str; 4] = ["summer", "winter", "wasteland", "swamp"];

/// Stratagus unit script names indexed by PUD unit type. Empty names are
/// types Wargus does not define.
#[rustfmt::skip]
const UNIT_SCRIPT_NAMES: &[&str] = &[
    "unit-footman", "unit-grunt",
    "unit-peasant", "unit-peon",
    "unit-ballista", "unit-catapult",
    "unit-knight", "unit-ogre",
    "unit-archer", "unit-axethrower",
    "unit-mage", "unit-death-knight",
    "unit-paladin", "unit-ogre-mage",
    "unit-dwarves", "unit-goblin-sappers",
    "unit-attack-peasant", "unit-attack-peon",
    "unit-ranger", "unit-berserker",
    "unit-female-hero", "unit-evil-knight",
    "unit-flying-angel", "unit-fad-man",
    "unit-white-mage", "unit-beast-cry",
    "unit-human-oil-tanker", "unit-orc-oil-tanker",
    "unit-human-transport", "unit-orc-transport",
    "unit-human-destroyer", "unit-orc-destroyer",
    "unit-battleship", "unit-ogre-juggernaught",
    "", "unit-fire-breeze",
    "", "",
    "unit-human-submarine", "unit-orc-submarine",
    "unit-balloon", "unit-zeppelin",
    "unit-gryphon-rider", "unit-dragon",
    "unit-knight-rider",
    "unit-eye-of-vision",
    "unit-arthor-literios",
    "unit-quick-blade",
    "",
    "unit-double-head",
    "unit-wise-man",
    "unit-ice-bringer",
    "unit-man-of-light",
    "unit-sharp-axe",
    "",
    "unit-skeleton",
    "unit-daemon",
    "unit-critter",
    "unit-farm", "unit-pig-farm",
    "unit-human-barracks", "unit-orc-barracks",
    "unit-church", "unit-altar-of-storms",
    "unit-human-watch-tower", "unit-orc-watch-tower",
    "unit-stables", "unit-ogre-mound",
    "unit-inventor", "unit-alchemist",
    "unit-gryphon-aviary", "unit-dragon-roost",
    "unit-human-shipyard", "unit-orc-shipyard",
    "unit-town-hall", "unit-great-hall",
    "unit-elven-lumber-mill", "unit-troll-lumber-mill",
    "unit-human-foundry", "unit-orc-foundry",
    "unit-mage-tower", "unit-temple-of-the-damned",
    "unit-human-blacksmith", "unit-orc-blacksmith",
    "unit-human-refinery", "unit-orc-refinery",
    "unit-human-oil-platform", "unit-orc-oil-platform",
    "unit-keep", "unit-stronghold",
    "unit-castle", "unit-fortress",
    "unit-gold-mine",
    "unit-oil-patch",
    "unit-human-start-location", "unit-orc-start-location",
    "unit-human-guard-tower", "unit-orc-guard-tower",
    "unit-human-cannon-tower", "unit-orc-cannon-tower",
    "unit-circle-of-power",
    "unit-dark-portal",
    "unit-runestone",
    "unit-human-wall", "unit-orc-wall",
];

/// AI scripts indexed by the PUD `AIPL` value.
#[rustfmt::skip]
const AI_TYPE_NAMES: &[&str] = &[
    "wc2-land-attack", "wc2-passive",
    "orc-03", "hum-04", "orc-04", "hum-05", "orc-05", "hum-06", "orc-06",
    "hum-07", "orc-07", "hum-08", "orc-08", "hum-09", "orc-09", "hum-10",
    "orc-10", "hum-11", "orc-11", "hum-12", "orc-12", "hum-13", "orc-13",
    "hum-14-orange", "orc-14-blue",
    "wc2-sea-attack", "wc2-air-attack",
    "hum-14-red", "hum-14-white", "hum-14-black", "orc-14-green", "orc-14-white",
    // expansion scenarios
    "orc-exp-4", "orc-exp-5", "orc-exp-7a", "orc-exp-9", "orc-exp-10", "orc-exp-12",
    "orc-exp-6a", "orc-exp-6b", "orc-exp-11a", "orc-exp-11b",
    "hum-exp-2a", "hum-exp-2b", "hum-exp-2c", "hum-exp-3a", "hum-exp-3b", "hum-exp-3c",
    "hum-exp-4a", "hum-exp-4b", "hum-exp-4c", "hum-exp-5a", "hum-exp-5b", "hum-exp-5c",
    "hum-exp-5d", "hum-exp-6a", "hum-exp-6b", "hum-exp-6c", "hum-exp-6d", "hum-exp-8a",
    "hum-exp-8b", "hum-exp-8c", "hum-exp-9a", "hum-exp-9b", "hum-exp-9c", "hum-exp-9d",
    "hum-exp-10a", "hum-exp-10b", "hum-exp-10c", "hum-exp-11a", "hum-exp-11b",
    "hum-exp-12a", "orc-exp-5b", "hum-exp-7a", "hum-exp-7b", "hum-exp-7c",
    "orc-exp-12a", "orc-exp-12b", "orc-exp-12c", "orc-exp-12d", "orc-exp-2",
    "orc-exp-7b", "orc-exp-3",
];

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Unit {
    pub x: u16,
    pub y: u16,
    pub kind: u8,
    pub player: u8,
    pub data: u16,
}

#[derive(Clone, Debug, Default)]
pub struct PudData {
    /// Raw description bytes (not re-encoded, like the original).
    pub description: Vec<u8>,
    pub players: [u8; PLAYER_MAX],
    pub races: [u8; PLAYER_MAX],
    pub tileset: u8,
    pub width: usize,
    pub height: usize,
    pub tiles: Vec<u16>,
    /// Hit points for wall tiles, 0 elsewhere.
    pub values: Vec<i32>,
    pub units: Vec<Unit>,
    pub ai: [u8; PLAYER_MAX],
    pub gold: [u16; PLAYER_MAX],
    pub lumber: [u16; PLAYER_MAX],
    pub oil: [u16; PLAYER_MAX],
    pub start_x: [u16; PLAYER_MAX],
    pub start_y: [u16; PLAYER_MAX],
}

fn per_player_u16(data: &[u8], at: usize) -> Result<[u16; PLAYER_MAX]> {
    let mut out = [0; PLAYER_MAX];
    for (i, v) in out.iter_mut().enumerate() {
        *v = le16(data, at + i * 2)?;
    }
    Ok(out)
}

fn per_player_u8(data: &[u8], at: usize) -> Result<[u8; PLAYER_MAX]> {
    Ok(slice(data, at, PLAYER_MAX)?.try_into().unwrap())
}

impl PudData {
    pub fn parse(data: &[u8]) -> Result<Self> {
        let mut pud = PudData::default();
        let section = |at: usize| -> Result<(&[u8], usize)> { Ok((slice(data, at, 4)?, le32(data, at + 4)? as usize)) };

        let (tag, len) = section(0)?;
        if tag != b"TYPE" || cstr(data, 8) != b"WAR2 MAP" {
            bail!("not a Warcraft II map");
        }
        let mut at = 8 + len;

        while at < data.len() {
            let (tag, len) = section(at)?;
            at += 8;
            match tag {
                b"DESC" => {
                    pud.description = match cstr(slice(data, at, len)?, 0) {
                        b"" => b"(unnamed)".to_vec(),
                        d => d.to_vec(),
                    };
                }
                b"OWNR" => pud.players = per_player_u8(data, at)?,
                b"ERA " | b"ERAX" => pud.tileset = u8_at(data, at)?,
                b"DIM " => {
                    pud.width = le16(data, at)? as usize;
                    pud.height = le16(data, at + 2)? as usize;
                }
                b"SIDE" => pud.races = per_player_u8(data, at)?,
                b"SGLD" => pud.gold = per_player_u16(data, at)?,
                b"SLBR" => pud.lumber = per_player_u16(data, at)?,
                b"SOIL" => pud.oil = per_player_u16(data, at)?,
                b"AIPL" => pud.ai = per_player_u8(data, at)?,
                b"MTXM" => {
                    let n = pud.width * pud.height;
                    pud.tiles = Vec::with_capacity(n);
                    pud.values = Vec::with_capacity(n);
                    for i in 0..n {
                        let v = le16(data, at + i * 2)?;
                        pud.tiles.push(v);
                        pud.values.push(if is_wall(v) { 40 } else { 0 });
                    }
                }
                b"UNIT" => {
                    pud.units.clear();
                    for i in 0..len / 8 {
                        let u = slice(data, at + i * 8, 8)?;
                        let unit = Unit {
                            x: u16::from_le_bytes([u[0], u[1]]),
                            y: u16::from_le_bytes([u[2], u[3]]),
                            kind: u[4],
                            player: u[5],
                            data: u16::from_le_bytes([u[6], u[7]]),
                        };
                        if unit.kind == UNIT_HUMAN_START || unit.kind == UNIT_ORC_START {
                            if let Some(p) = pud.start_x.get_mut(unit.player as usize) {
                                *p = unit.x;
                                pud.start_y[unit.player as usize] = unit.y;
                            }
                        }
                        pud.units.push(unit);
                    }
                }
                // Not needed by Stratagus.
                b"VER " | b"UDTA" | b"ALOW" | b"UGRD" | b"SQM " | b"OILM" | b"REGM" | b"SIGN" => {}
                _ => {
                    println!("unknown section: {}, {len}", String::from_utf8_lossy(tag));
                    bail!("unknown PUD section");
                }
            }
            at += len;
        }
        Ok(pud)
    }

    /// The `.smp` presentation script. `setup` is the path the map setup is
    /// loaded from, only written for campaign maps.
    pub fn write_smp<W: Write + ?Sized>(&self, out: &mut W, setup: Option<&str>) -> std::io::Result<()> {
        writeln!(out, "-- Stratagus Map Presentation")?;
        writeln!(out, "-- File generated automatically from pudconvert V{VERSION}")?;
        writeln!(out)?;

        let types: Vec<String> =
            self.players.iter().map(|&p| format!("\"{}\"", PLAYER_TYPES.get(p as usize).unwrap_or(&""))).collect();
        writeln!(out, "DefinePlayerTypes({})", types.join(","))?;

        let player_count = self.players.iter().filter(|&&p| p != PLAYER_NOBODY && p != PLAYER_NEUTRAL).count();
        out.write_all(b"PresentMap(\"")?;
        out.write_all(&self.description)?;
        writeln!(out, "\", {player_count}, {}, {}, 1)", self.width, self.height)?;

        if let Some(setup) = setup {
            writeln!(out, "DefineMapSetup(\"{setup}\")")?;
        }
        Ok(())
    }

    /// The `.sms` setup script.
    pub fn write_sms<W: Write + ?Sized>(&self, out: &mut W) -> std::io::Result<()> {
        writeln!(out, "-- Stratagus Map Setup")?;
        writeln!(out, "-- File generated automatically from pudconvert V{VERSION}")?;
        writeln!(out)?;

        for i in 0..PLAYER_MAX - 1 {
            if self.players[i] == PLAYER_NOBODY {
                continue;
            }
            writeln!(out, "SetStartView({i}, {}, {})", self.start_x[i], self.start_y[i])?;
            writeln!(out, "SetPlayerData({i}, \"Resources\", \"gold\", {})", self.gold[i])?;
            writeln!(out, "SetPlayerData({i}, \"Resources\", \"wood\", {})", self.lumber[i])?;
            writeln!(out, "SetPlayerData({i}, \"Resources\", \"oil\", {})", self.oil[i])?;
            writeln!(
                out,
                "SetPlayerData({i}, \"RaceName\", \"{}\")",
                RACE_NAMES.get(self.races[i] as usize).unwrap_or(&"")
            )?;
            if self.players[i] != PLAYER_NEUTRAL {
                let ai = AI_TYPE_NAMES.get(self.ai[i] as usize).copied().unwrap_or_else(|| {
                    eprintln!("Unknown AI type {} for player {i}, using {}", self.ai[i], AI_TYPE_NAMES[0]);
                    AI_TYPE_NAMES[0]
                });
                writeln!(out, "SetAiType({i}, \"{ai}\")")?;
            }
        }
        writeln!(out, "SetPlayerData({}, \"RaceName\", \"neutral\")", PLAYER_MAX - 1)?;
        writeln!(out)?;

        let tileset = TILESET_NAMES.get(self.tileset as usize).copied().unwrap_or_else(|| {
            eprintln!("Unknown tileset {}, using {}", self.tileset, TILESET_NAMES[0]);
            TILESET_NAMES[0]
        });
        writeln!(out, "LoadTileModels(\"scripts/tilesets/{tileset}.lua\")")?;
        writeln!(out)?;

        for y in 0..self.height {
            for x in 0..self.width {
                let i = y * self.width + x;
                let (tile, value) = (self.tiles.get(i).copied().unwrap_or(0), self.values.get(i).copied().unwrap_or(0));
                writeln!(out, "SetTile({tile}, {x}, {y}, {value})")?;
            }
        }

        writeln!(out)?;
        writeln!(out, "local unit")?;

        for u in &self.units {
            if u.kind == UNIT_HUMAN_START || u.kind == UNIT_ORC_START {
                continue;
            }
            let name = UNIT_SCRIPT_NAMES.get(u.kind as usize).copied().unwrap_or("");
            if name.len() <= 1 {
                eprintln!("Skipping unknown unit type {} for player {} at [{}, {}]", u.kind, u.player, u.x, u.y);
                continue;
            }
            writeln!(out, "unit = CreateUnit(\"{name}\", {}, {{{}, {}}})", u.player, u.x, u.y)?;
            if matches!(u.kind, UNIT_HUMAN_OIL_PLATFORM | UNIT_ORC_OIL_PLATFORM | UNIT_GOLD_MINE | UNIT_OIL_PATCH) {
                writeln!(out, "SetResourcesHeld(unit, {})", u.data as i32 * 2500)?;
            }
        }
        Ok(())
    }
}

fn is_wall(v: u16) -> bool {
    let orc = (v & 0xFFF0) == 0x00A0 || (v & 0xFFF0) == 0x00C0 || (v & 0xFF00) == 0x0900;
    let human = (v & 0x00F0) == 0x0090 || (v & 0xFFF0) == 0x00B0 || (v & 0xFF00) == 0x0800;
    orc || human
}

/// The `DefineMapSetup` path for campaign maps: the part of the setup file
/// path from `./campaigns` on, with `.sms` turned into `_c.sms` and the
/// `.gz` suffix dropped.
fn campaign_setup_path(sms_path: &str) -> Option<String> {
    let start = sms_path.find("./campaigns")?;
    let rel = sms_path[start..].replace('\\', "/");
    let ext = rel.find(".sms")?;
    Some(format!("{}_c.sms", &rel[..ext]))
}

fn write_gz(path: &str, level: Compression, write: impl FnOnce(&mut dyn Write) -> std::io::Result<()>) -> Result<()> {
    let file = File::create(path).with_context(|| format!("cannot open {path}"))?;
    let mut gz = GzEncoder::new(BufWriter::new(file), level);
    write(&mut gz)?;
    gz.finish()?.flush()?;
    Ok(())
}

/// Convert PUD `data` to `<outdir>/<name>.smp.gz` and `<outdir>/<name>.sms.gz`.
pub fn pud_to_stratagus(data: &[u8], name: &str, outdir: &str) -> Result<()> {
    let pud = PudData::parse(data).context("invalid pud data")?;
    let smp_path = format!("{outdir}/{name}.smp.gz");
    let sms_path = format!("{outdir}/{name}.sms.gz");

    let setup = if sms_path.contains("campaigns") { campaign_setup_path(&sms_path) } else { None };
    write_gz(&smp_path, Compression::default(), |out| pud.write_smp(out, setup.as_deref()))?;
    write_gz(&sms_path, Compression::default(), |out| pud.write_sms(out))?;
    Ok(())
}

/// Convenience wrapper reading the PUD from a file.
pub fn convert_file(input: &Path, name: &str, outdir: &str) -> Result<()> {
    let data = std::fs::read(input).with_context(|| format!("error reading from file: {}", input.display()))?;
    pud_to_stratagus(&data, name, outdir)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sec(tag: &[u8; 4], body: &[u8]) -> Vec<u8> {
        let mut v = tag.to_vec();
        v.extend((body.len() as u32).to_le_bytes());
        v.extend(body);
        v
    }

    fn sample() -> Vec<u8> {
        let mut d = sec(b"TYPE", b"WAR2 MAP\0\0");
        d.extend(sec(b"VER ", &[0x11, 0]));
        let mut desc = b"Test map".to_vec();
        desc.resize(32, 0);
        d.extend(sec(b"DESC", &desc));
        let mut owners = [PLAYER_NOBODY; 16];
        owners[0] = 5;
        owners[1] = 4;
        owners[15] = PLAYER_NEUTRAL;
        d.extend(sec(b"OWNR", &owners));
        d.extend(sec(b"ERAX", &[1, 0]));
        d.extend(sec(b"DIM ", &[2, 0, 1, 0]));
        let mut side = [0u8; 16];
        side[1] = 1;
        side[15] = 2;
        d.extend(sec(b"SIDE", &side));
        let gold: Vec<u8> = (0..16u16).flat_map(|i| (i * 100).to_le_bytes()).collect();
        d.extend(sec(b"SGLD", &gold));
        d.extend(sec(b"AIPL", &[1; 16]));
        d.extend(sec(b"MTXM", &[0x10, 0x00, 0xA3, 0x00]));
        let mut units = Vec::new();
        units.extend([3, 0, 4, 0, UNIT_HUMAN_START, 0, 0, 0]);
        units.extend([1, 0, 0, 0, UNIT_GOLD_MINE, 15, 4, 0]);
        units.extend([0, 0, 0, 0, 0x22, 1, 0, 0]); // unknown type
        d.extend(sec(b"UNIT", &units));
        d
    }

    #[test]
    fn parses_sections() {
        let pud = PudData::parse(&sample()).unwrap();
        assert_eq!(pud.description, b"Test map");
        assert_eq!((pud.width, pud.height, pud.tileset), (2, 1, 1));
        assert_eq!(pud.values, vec![0, 40]);
        assert_eq!((pud.start_x[0], pud.start_y[0]), (3, 4));
        assert_eq!(pud.units.len(), 3);
    }

    #[test]
    fn writes_scripts() {
        let pud = PudData::parse(&sample()).unwrap();
        let mut smp = Vec::new();
        pud.write_smp(&mut smp, Some("./campaigns/human/level01h_c.sms")).unwrap();
        let smp = String::from_utf8(smp).unwrap();
        assert!(smp.contains(
            "DefinePlayerTypes(\"person\",\"computer\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"nobody\",\"neutral\")\n"
        ));
        assert!(smp.contains("PresentMap(\"Test map\", 2, 2, 1, 1)\n"));
        assert!(smp.ends_with("DefineMapSetup(\"./campaigns/human/level01h_c.sms\")\n"));

        let mut sms = Vec::new();
        pud.write_sms(&mut sms).unwrap();
        let sms = String::from_utf8(sms).unwrap();
        assert!(sms.contains("SetStartView(0, 3, 4)\nSetPlayerData(0, \"Resources\", \"gold\", 0)\n"));
        assert!(sms.contains("SetPlayerData(1, \"RaceName\", \"orc\")\nSetAiType(1, \"wc2-passive\")\n"));
        assert!(!sms.contains("SetStartView(15"));
        assert!(sms.contains(
            "SetPlayerData(15, \"RaceName\", \"neutral\")\n\nLoadTileModels(\"scripts/tilesets/winter.lua\")\n\n"
        ));
        assert!(sms.contains("SetTile(16, 0, 0, 0)\nSetTile(163, 1, 0, 40)\n\nlocal unit\n"));
        assert!(sms.ends_with("unit = CreateUnit(\"unit-gold-mine\", 15, {1, 0})\nSetResourcesHeld(unit, 10000)\n"));
    }

    #[test]
    fn campaign_setup_paths() {
        assert_eq!(
            campaign_setup_path("data/./campaigns/human/level01h.sms.gz").as_deref(),
            Some("./campaigns/human/level01h_c.sms")
        );
        assert_eq!(campaign_setup_path("data/campaigns/x.sms.gz"), None);
    }

    #[test]
    fn rejects_bad_maps() {
        assert!(PudData::parse(b"TYPE\x0a\0\0\0NOT A MAP\0").is_err());
        let mut d = sample();
        d.extend(sec(b"ZZZZ", &[]));
        assert!(PudData::parse(&d).is_err());
    }
}
