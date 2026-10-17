//! Native `.aseprite` reader/writer (Aseprite 1.3 file format), builder, renderer and strict validator.
//!
//! * [`read`] / [`write`]: lossless typed model ([`AseDoc`]). Chunks that cannot be parsed strictly (unknown type,
//!   trailing bytes, bad zlib) are kept verbatim as [`Chunk::Unknown`], so read -> write keeps every chunk.
//!   Files we wrote are reproduced byte for byte (fixed zlib level, canonical reserved bytes).
//! * [`AseBuilder`]: what `export` uses to create documents.
//! * [`validate`]: independent byte level checker.
//! * [`render`]: cel / layer / flattened pixel access for verification.

mod build;
mod model;
mod props;
mod read;
mod render;
mod validate;
mod write;

pub use build::{AseBuilder, SliceSpec};
pub use model::*;
pub use read::read;
pub use render::*;
pub use validate::{validate, Problem};
pub use write::{write, ZLIB_LEVEL};

pub const HEADER_MAGIC: u16 = 0xA5E0;
pub const FRAME_MAGIC: u16 = 0xF1FA;

pub const CHUNK_OLD_PALETTE_256: u16 = 0x0004;
pub const CHUNK_OLD_PALETTE_64: u16 = 0x0011;
pub const CHUNK_LAYER: u16 = 0x2004;
pub const CHUNK_CEL: u16 = 0x2005;
pub const CHUNK_CEL_EXTRA: u16 = 0x2006;
pub const CHUNK_COLOR_PROFILE: u16 = 0x2007;
pub const CHUNK_EXTERNAL_FILES: u16 = 0x2008;
pub const CHUNK_MASK: u16 = 0x2016;
pub const CHUNK_PATH: u16 = 0x2017;
pub const CHUNK_TAGS: u16 = 0x2018;
pub const CHUNK_PALETTE: u16 = 0x2019;
pub const CHUNK_USER_DATA: u16 = 0x2020;
pub const CHUNK_SLICE: u16 = 0x2022;
pub const CHUNK_TILESET: u16 = 0x2023;
