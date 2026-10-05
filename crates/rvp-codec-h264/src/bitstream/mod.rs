//! Bitstream layer: NAL units, RBSP escaping, bit reader and bit writer, Exp-Golomb codes.
//!
//! Nothing here depends on the decoder; an encoder uses [`BitWriter`] and [`nal::write_nal`] the same way
//! the decoder uses [`BitReader`] and [`nal::unescape`].

pub mod nal;
pub mod reader;
pub mod writer;

pub use nal::{NalHeader, NalUnitType};
pub use reader::BitReader;
pub use writer::BitWriter;
