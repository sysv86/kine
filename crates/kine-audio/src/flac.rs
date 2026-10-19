use crate::{AudioClip, AudioError};
use flacenc::bitsink::ByteSink;
use flacenc::component::{BitRepr, Stream};
use flacenc::error::Verify;
use flacenc::source::MemSource;
use std::path::Path;

/// Frames per FLAC block.
const BLOCK_SIZE: usize = 4096;

/// One coded FLAC frame, which becomes one MP4/MKV audio sample.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlacFrame {
    /// The frame's bytes, header included.
    pub data: Vec<u8>,
    /// Samples per channel in this frame.
    pub samples: u32,
}

/// Encoded FLAC, shaped for the MP4 `dfLa`/`STREAMINFO` requirement: the raw
/// 34-byte STREAMINFO block *payload* (without the `fLaC` magic or metadata
/// header) plus one complete FLAC frame per element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FlacOutput {
    /// 34-byte STREAMINFO block payload, for the MP4 `dfLa`/`STREAMINFO` requirement.
    pub streaminfo: Vec<u8>,
    /// One FLAC frame per element: the MP4 audio samples, in order.
    pub frames: Vec<FlacFrame>,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
}

impl FlacOutput {
    /// Total samples per channel across all frames.
    pub fn total_samples(&self) -> u64 {
        self.frames.iter().map(|f| u64::from(f.samples)).sum()
    }

    /// Assemble a complete standalone `.flac` file (`fLaC` magic + STREAMINFO
    /// metadata block + all coded frames).
    pub fn to_file_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(
            42 + self.frames.iter().map(|f| f.data.len()).sum::<usize>(),
        );
        bytes.extend_from_slice(b"fLaC");
        bytes.push(0x80); // last-metadata-block flag (0x80) | block type STREAMINFO (0)
        bytes.extend_from_slice(&[0, 0, 34]); // 24-bit big-endian length = 34
        bytes.extend_from_slice(&self.streaminfo);
        for frame in &self.frames {
            bytes.extend_from_slice(&frame.data);
        }
        bytes
    }
}

/// Write `clip` as a standalone `.flac` file on disk.
pub fn write_flac(path: &Path, clip: &AudioClip, bits_per_sample: u16) -> Result<(), AudioError> {
    let encoded = encode_flac(clip, bits_per_sample)?;
    std::fs::write(path, encoded.to_file_bytes())?;
    Ok(())
}

/// Encodes `clip` to FLAC at `bits_per_sample` (4..=32, per the FLAC spec).
pub fn encode_flac(clip: &AudioClip, bits_per_sample: u16) -> Result<FlacOutput, AudioError> {
    clip.validate()?;
    if !(4..=32).contains(&bits_per_sample) {
        return Err(AudioError::Flac(format!(
            "bit depth {bits_per_sample} is outside FLAC's 4..=32 range"
        )));
    }
    if clip.frame_count() == 0 {
        return Err(AudioError::InvalidClip(
            "cannot FLAC-encode an empty clip".into(),
        ));
    }

    let signal: Vec<i32> = clip
        .samples
        .iter()
        .map(|&sample| quantize(sample, bits_per_sample))
        .collect();
    let source = MemSource::from_samples(
        &signal,
        clip.channels as usize,
        bits_per_sample as usize,
        clip.sample_rate as usize,
    );

    let mut encoder = flacenc::config::Encoder::default();
    encoder.multithread = false;
    let config = match encoder.into_verified() {
        Ok(config) => config,
        Err((_, err)) => return Err(AudioError::Flac(err.to_string())),
    };

    let stream = flacenc::encode_with_fixed_block_size(&config, source, BLOCK_SIZE)
        .map_err(|err| AudioError::Flac(err.to_string()))?;

    let streaminfo = extract_streaminfo(&stream)?;
    let mut frames = Vec::with_capacity(stream.frame_count());
    for index in 0..stream.frame_count() {
        let frame = stream
            .frame(index)
            .ok_or_else(|| AudioError::Flac(format!("stream is missing frame {index}")))?;
        let mut sink = ByteSink::new();
        frame
            .write(&mut sink)
            .map_err(|err| AudioError::Flac(err.to_string()))?;
        let samples = u32::try_from(frame.header().block_size()).map_err(|_| {
            AudioError::Flac("a FLAC frame claims more samples than a u32 holds".to_string())
        })?;
        frames.push(FlacFrame {
            data: sink.as_slice().to_vec(),
            samples,
        });
    }

    Ok(FlacOutput {
        streaminfo,
        frames,
        sample_rate: clip.sample_rate,
        channels: clip.channels,
        bits_per_sample,
    })
}

fn extract_streaminfo(stream: &Stream) -> Result<Vec<u8>, AudioError> {
    let mut sink = ByteSink::new();
    stream
        .write(&mut sink)
        .map_err(|err| AudioError::Flac(err.to_string()))?;
    let bytes = sink.as_slice();

    if bytes.len() < 42 {
        return Err(AudioError::BadStreamInfo(format!(
            "flac stream is only {} bytes",
            bytes.len()
        )));
    }
    if &bytes[0..4] != b"fLaC" {
        return Err(AudioError::BadStreamInfo(
            "stream does not start with the fLaC magic".into(),
        ));
    }
    let block_type = bytes[4] & 0x7f;
    let length =
        ((bytes[5] as usize) << 16) | ((bytes[6] as usize) << 8) | bytes[7] as usize;
    if block_type != 0 {
        return Err(AudioError::BadStreamInfo(format!(
            "first metadata block is type {block_type}, expected STREAMINFO (0)"
        )));
    }
    if length != 34 {
        return Err(AudioError::BadStreamInfo(format!(
            "STREAMINFO block is {length} bytes, expected 34"
        )));
    }

    let mut payload = bytes[8..42].to_vec();
    let max_block = [payload[2], payload[3]];
    payload[0..2].copy_from_slice(&max_block);

    Ok(payload)
}

fn quantize(sample: f32, bits: u16) -> i32 {
    if !sample.is_finite() {
        return 0;
    }
    let scale = (1i64 << (bits - 1)) as f32;
    (sample * scale).round().clamp(-scale, scale - 1.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::decode::decode_source;

    fn sine(rate: u32, channels: u16, hz: f32, frames: usize) -> AudioClip {
        let mut samples = Vec::with_capacity(frames * channels as usize);
        for i in 0..frames {
            let v = 0.8 * (2.0 * std::f32::consts::PI * hz * i as f32 / rate as f32).sin();
            for _ in 0..channels {
                samples.push(v);
            }
        }
        AudioClip::new(rate, channels, samples)
    }

    #[test]
    fn mono_sine_round_trips_through_symphonia() {
        let clip = sine(48_000, 1, 1_000.0, 24_000);
        let output = encode_flac(&clip, 16).unwrap();

        assert!(!output.frames.is_empty(), "must emit at least one FLAC frame");
        assert_eq!(
            output.total_samples(),
            clip.frame_count() as u64,
            "frame durations must sum to the clip length"
        );
        let block = output.frames[0].samples;
        for frame in &output.frames[..output.frames.len() - 1] {
            assert_eq!(frame.samples, block, "all but the last frame are full blocks");
        }
        assert_eq!(output.streaminfo.len(), 34);
        assert_ne!(&output.streaminfo[0..2], b"fL");
        assert_ne!(&output.streaminfo[0..4], b"fLaC");
        assert_eq!(output.sample_rate, 48_000);
        assert_eq!(output.channels, 1);
        assert_eq!(output.bits_per_sample, 16);

        let bytes = output.to_file_bytes();
        let decoded = decode_source(Box::new(std::io::Cursor::new(bytes)), Some("flac")).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.frame_count(), clip.frame_count());

        for (a, b) in decoded.samples.iter().zip(clip.samples.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn stereo_sine_round_trips_through_symphonia() {
        let clip = sine(48_000, 2, 1_000.0, 24_000);
        let output = encode_flac(&clip, 16).unwrap();
        assert_eq!(output.channels, 2);
        assert!(!output.frames.is_empty());
        assert_eq!(output.total_samples(), clip.frame_count() as u64);

        let bytes = output.to_file_bytes();
        let decoded = decode_source(Box::new(std::io::Cursor::new(bytes)), Some("flac")).unwrap();
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.frame_count(), clip.frame_count());
        for (a, b) in decoded.samples.iter().zip(clip.samples.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn frame_count_covers_every_source_frame() {
        let clip = sine(48_000, 1, 440.0, 10_000);
        let output = encode_flac(&clip, 16).unwrap();
        assert_eq!(output.frames.len(), 3);

        let bytes = output.to_file_bytes();
        let decoded = decode_source(Box::new(std::io::Cursor::new(bytes)), Some("flac")).unwrap();
        assert_eq!(decoded.frame_count(), 10_000);
    }

    #[test]
    fn encoding_is_byte_for_byte_deterministic() {
        let clip = sine(48_000, 2, 1_000.0, 8_192);
        let first = encode_flac(&clip, 16).unwrap();
        let second = encode_flac(&clip, 16).unwrap();
        assert_eq!(first, second);
    }

    #[test]
    fn rejects_empty_clip_and_bad_depth() {
        let empty = AudioClip::new(48_000, 1, Vec::new());
        assert!(matches!(
            encode_flac(&empty, 16).unwrap_err(),
            AudioError::InvalidClip(_)
        ));

        let clip = sine(48_000, 1, 440.0, 128);
        assert!(matches!(
            encode_flac(&clip, 3).unwrap_err(),
            AudioError::Flac(_)
        ));
    }
}
