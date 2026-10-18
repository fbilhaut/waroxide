//! Rust port of the Wargus tools.
//!
//! Wargus lets you play Warcraft II on the Stratagus engine. This crate
//! re-implements the native parts of Wargus:
//!
//! * [`extract`]: the `wartool` data extractor, which converts the original
//!   `.WAR` archives into PNG / WAV / MIDI / Lua files Stratagus can load,
//! * [`pud`]: the PUD map converter (also exposed as `pudconvert`),
//! * the `waroxide` game launcher binary.
//!
//! The format decoders live in their own modules and are independent from
//! the extraction driver, so they can be reused or tested in isolation.

pub mod archive;
pub mod bytes;
pub mod cd;
pub mod cdimage;
pub mod codepages;
pub mod control;
pub mod extract;
pub mod gfx;
pub mod image;
pub mod ogg;
pub mod pud;
pub mod rip_music;
pub mod source;
pub mod tables;
pub mod text;
pub mod xmi2mid;
