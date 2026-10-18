//! The extraction plan: one [`Control`] per file to produce.
//!
//! The plan itself ([`crate::tables::TODO`]) is generated from the original
//! `wartool.h` so that archive indices stay exactly as they were.

/// What to do with an entry. The `arg*` fields of [`Control`] mean
/// different things depending on the kind; see each variant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Kind {
    /// Load the unit name table used to name sprite files.
    Setup,
    /// Open an archive. `arg1`: archive id.
    Archive,
    /// `arg1..4`: palette, mega-tiles, mini-tiles, map entries.
    Tileset,
    /// Palette to `.rgb` + GIMP palette. `arg1`: palette entry.
    Rgb,
    /// RLE sprites. `arg1`: palette, `arg2`: sprite, `arg3`/`arg4`:
    /// alternative sprite and first frame taken from it.
    Gfx,
    /// Uncompressed sprites. `arg1`: palette, `arg2`: sprite.
    Gfu,
    /// Uncompressed sprites cut into named pieces. `arg3`: group index.
    GroupedGfu,
    /// PUD map. `arg1`: entry.
    Pud,
    /// Font. `arg1`: entry.
    Font,
    /// Image. `arg1`: palette, `arg2`: image, `arg3`/`arg4`: resize to.
    Image,
    /// Sound. `arg1`: entry.
    Wav,
    /// XMI music. `arg1`: entry.
    Midi,
    /// Text. `arg1`: entry, `arg2`: offset into it.
    Text,
    /// Cursor. `arg1`: palette, `arg2`: cursor.
    Cursor,
    /// Smacker video. `arg1`: entry.
    Video,
    /// Campaign titles and objectives.
    Campaign,
    /// File inside a Battle.net MPQ archive.
    Mpq,
}

/// One step of the extraction plan.
///
/// `version` restricts where the entry applies:
/// `0` everywhere, bit `2` expansion only, `3` original only, bit `4` DOS
/// only, bit `8` Battle.net only.
#[derive(Clone, Copy, Debug)]
pub struct Control {
    pub kind: Kind,
    pub version: u32,
    pub file: Option<&'static str>,
    pub arg1: i32,
    pub arg2: i32,
    pub arg3: i32,
    pub arg4: i32,
    pub mpq_file: Option<&'static str>,
    pub arc_file: Option<&'static str>,
}

impl Control {
    /// Output name (empty for the few entries that have none).
    pub fn name(&self) -> &'static str {
        self.file.unwrap_or("")
    }
}
