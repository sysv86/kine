//! Software video encoding, decoding, and uncompressed/lossless stream formats, in pure Rust.
//!
//! Colour handling is **BT.709 limited range** for 8-bit SDR, with the matrix
//! applied explicitly rather than assumed from a library default.

pub mod av1;
pub mod color;
pub mod raw;
pub mod y4m;

pub use av1::{Av1Encoder, EncoderSettings};
pub use color::{
    i420_to_rgba, i444_to_rgba, nv12_to_rgba, rgba_to_i420, rgba_to_nv12, I420Frame,
};
pub use raw::{
    decode_kvid, decode_raw_stream, encode_kvid, encode_raw_frame, KvidVideo, RawPixelFormat,
};
pub use y4m::{decode_y4m, Y4mEncoder, Y4mVideo};

#[derive(Debug, thiserror::Error)]
pub enum EncodeError {
    #[error("invalid encoder configuration: {0}")]
    InvalidConfig(String),

    #[error("frame has {actual} bytes, expected {expected} for {width}x{height} RGBA")]
    FrameSize {
        actual: usize,
        expected: usize,
        width: u32,
        height: u32,
    },

    #[error("encoder rejected the frame: {0}")]
    Rejected(String),

    #[error("encoder produced no output")]
    NoOutput,
}

/// One coded access unit, ready for the muxer.
#[derive(Debug, Clone)]
pub struct AccessUnit {
    /// Coded bytes: OBU stream for AV1, Annex-B for H.264.
    pub data: Vec<u8>,
    /// True when this unit is a sync sample (IDR / AV1 keyframe).
    pub keyframe: bool,
}

/// Result of pushing one frame: the access units that became available.
#[derive(Debug, Default)]
pub struct EncodeOutput {
    /// Access units in stream order.
    pub units: Vec<AccessUnit>,
    /// Whether the *first* unit produced here is a keyframe.
    pub starts_keyframe: bool,
}

impl EncodeOutput {
    pub fn is_empty(&self) -> bool {
        self.units.is_empty()
    }
}
