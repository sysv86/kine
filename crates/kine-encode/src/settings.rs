//! Shared encoder configuration.

/// Settings for the video encoders.
#[derive(Debug, Clone, Copy)]
pub struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Quality knob (`0..=255`, lower is better quality). The CLI exposes it as
    /// `--quality`; FFmpeg maps it to CRF/QP or a target bitrate.
    pub quantizer: usize,
    /// Encoder speed preset (`0..=10`, 10 is fastest).
    pub speed: u8,
    /// Keyframe cadence in frames.
    pub keyframe_interval: u32,
    /// Optional target bitrate in bits per second; `0` uses quality mode.
    pub bitrate: u64,
}

impl EncoderSettings {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            width,
            height,
            fps_num,
            fps_den,
            quantizer: 100,
            speed: 9,
            keyframe_interval: 60,
            bitrate: 0,
        }
    }
}
