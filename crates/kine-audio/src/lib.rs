//! Pure-Rust audio engine for Kine: decode (WAV, FLAC, MP3, Ogg Vorbis, AAC,
//! ALAC, CAF, MP4, MKV, AIFF, procedural `synth:` URIs), resample (`rubato`
//! windowed sinc), DSP (RBJ biquad EQ filters, feed-forward compression,
//! sidechain voiceover ducking, loudness metering), mix (trim, loop,
//! constant-power stereo pan, shaped fades, soft-clipping) and export (FLAC,
//! 16/24/32f WAV, AIFF).

mod aiff;
mod clip;
mod decode;
pub mod dsp;
mod flac;
mod mix;
mod resample;
pub mod synth;
mod wav;

pub use aiff::{decode_aiff_bytes, encode_aiff, read_aiff, write_aiff};
pub use clip::AudioClip;
pub use decode::{decode_bytes, decode_file};
pub use dsp::{
    apply_biquad, apply_compressor, compute_ducking_envelope, AudioMetrics, BiquadFilterSpec,
    CompressorSpec,
};
pub use flac::{encode_flac, write_flac, FlacFrame, FlacOutput};
pub use mix::{mix, AudioTrackSpec, FadeCurve};
pub use resample::resample;
pub use synth::{synthesize, synthesize_uri, SynthSpec, Waveform};
pub use wav::{encode_wav_bytes, read_wav, write_wav, write_wav_with_format, WavSampleFormat};

/// Everything that can go wrong while decoding, resampling, mixing or encoding.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The path could not be opened or written.
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    /// The container could not be probed or its audio could not be decoded.
    #[error("decode error: {0}")]
    Decode(String),

    /// The container probed fine but exposes no decodable audio track.
    #[error("no decodable audio track")]
    NoAudioTrack,

    /// A clip, track spec or mix geometry is self-inconsistent.
    #[error("invalid clip: {0}")]
    InvalidClip(String),

    /// The requested mix length rounds down to zero frames.
    #[error("mix produced zero frames")]
    EmptyMix,

    /// `flacenc` failed to encode or produced something we refuse to slice.
    #[error("flac error: {0}")]
    Flac(String),

    /// The FLAC header was not the `fLaC` + STREAMINFO(34) layout we rely on.
    #[error("malformed flac streaminfo: {0}")]
    BadStreamInfo(String),

    /// `rubato` could not be constructed or run for the requested ratio.
    #[error("resample error: {0}")]
    Resample(String),

    /// Reading or writing a WAV file failed.
    #[error("wav error: {0}")]
    Wav(#[from] hound::Error),
}
