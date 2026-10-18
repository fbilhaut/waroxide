//! Conversion of the game's 8-bit strings to UTF-8.

use crate::codepages::{CP1252, CP437, CP866};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CodePage {
    /// DOS CDs.
    Cp437,
    /// Battle.net edition.
    Cp1252,
    /// Russian SPK CDs.
    Cp866,
}

impl CodePage {
    fn high_half(self) -> &'static [char; 128] {
        match self {
            CodePage::Cp437 => &CP437,
            CodePage::Cp1252 => &CP1252,
            CodePage::Cp866 => &CP866,
        }
    }
}

/// Convert `input` up to its first NUL byte (or its end) to UTF-8.
pub fn convert_string(input: &[u8], cp: CodePage) -> String {
    let table = cp.high_half();
    input
        .iter()
        .take_while(|&&b| b != 0)
        .map(|&b| if b < 0x80 { b as char } else { table[(b - 0x80) as usize] })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_code_pages() {
        assert_eq!(convert_string(b"Ok\x82\0junk", CodePage::Cp437), "Oké");
        assert_eq!(convert_string(b"\x80", CodePage::Cp1252), "€");
        assert_eq!(convert_string(b"\x80", CodePage::Cp866), "А");
    }
}
