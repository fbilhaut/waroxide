//! Flags describing which release of Warcraft II is being extracted.

use std::ops::{BitOr, BitOrAssign};

use crate::text::CodePage;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct CdType(u32);

impl CdType {
    pub const MAC: Self = Self(1);
    pub const EXPANSION: Self = Self(1 << 1);
    pub const US: Self = Self(1 << 4);
    pub const SPANISH: Self = Self(1 << 5);
    pub const GERMAN: Self = Self(1 << 6);
    /// Also the Australian release.
    pub const UK: Self = Self(1 << 7);
    pub const ITALIAN: Self = Self(1 << 8);
    pub const PORTUGUESE: Self = Self(1 << 9);
    pub const FRENCH: Self = Self(1 << 10);
    pub const RUSSIAN: Self = Self(1 << 11);
    /// File names on the CD are upper case.
    pub const UPPER: Self = Self(1 << 13);
    /// Battle.net edition.
    pub const BNE: Self = Self(1 << 14);
    /// Battle.net edition with a capitalised `Support` folder.
    pub const BNE_CAPS: Self = Self(1 << 15);
    /// Battle.net edition with an upper case `SUPPORT` folder.
    pub const BNE_UPPER: Self = Self(1 << 16);

    /// True if any of the flags in `other` are set.
    pub fn any(self, other: Self) -> bool {
        self.0 & other.0 != 0
    }

    pub fn code_page(self) -> CodePage {
        if self.any(Self::RUSSIAN) {
            CodePage::Cp866
        } else if self.any(Self::BNE) {
            CodePage::Cp1252
        } else {
            CodePage::Cp437
        }
    }
}

impl BitOr for CdType {
    type Output = Self;
    fn bitor(self, rhs: Self) -> Self {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for CdType {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}
