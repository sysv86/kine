use crate::{AudioClip, AudioError};
use std::path::Path;

/// WAV sample encoding format for export.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavSampleFormat {
    /// 16-bit signed integer PCM (universal default).
    Pcm16,
    /// 24-bit signed integer PCM (studio mastering).
    Pcm24,
    /// 32-bit IEEE floating-point PCM (lossless headroom).
    Float32,
}

/// Writes `clip` as a 16-bit PCM WAV file.
pub fn write_wav(path: &Path, clip: &AudioClip) -> Result<(), AudioError> {
    write_wav_with_format(path, clip, WavSampleFormat::Pcm16)
}

/// Writes `clip` as a WAV file with explicit sample format (16-bit, 24-bit, or 32-bit float).
pub fn write_wav_with_format(
    path: &Path,
    clip: &AudioClip,
    format: WavSampleFormat,
) -> Result<(), AudioError> {
    let bytes = encode_wav_bytes(clip, format)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Encode `clip` into an in-memory WAV file buffer.
pub fn encode_wav_bytes(
    clip: &AudioClip,
    format: WavSampleFormat,
) -> Result<Vec<u8>, AudioError> {
    clip.validate()?;
    let (bits_per_sample, sample_format) = match format {
        WavSampleFormat::Pcm16 => (16, hound::SampleFormat::Int),
        WavSampleFormat::Pcm24 => (24, hound::SampleFormat::Int),
        WavSampleFormat::Float32 => (32, hound::SampleFormat::Float),
    };

    let spec = hound::WavSpec {
        channels: clip.channels,
        sample_rate: clip.sample_rate,
        bits_per_sample,
        sample_format,
    };
    let mut cursor = std::io::Cursor::new(Vec::new());
    {
        let mut writer = hound::WavWriter::new(&mut cursor, spec)?;
        match format {
            WavSampleFormat::Pcm16 => {
                for &sample in &clip.samples {
                    writer.write_sample(to_i16(sample))?;
                }
            }
            WavSampleFormat::Pcm24 => {
                for &sample in &clip.samples {
                    writer.write_sample(to_i24(sample))?;
                }
            }
            WavSampleFormat::Float32 => {
                for &sample in &clip.samples {
                    writer.write_sample(if sample.is_finite() { sample } else { 0.0 })?;
                }
            }
        }
        writer.finalize()?;
    }
    Ok(cursor.into_inner())
}

/// Reads a WAV file into an [`AudioClip`], converting integer PCM to `f32` in
/// nominal `-1.0..=1.0`.
pub fn read_wav(path: &Path) -> Result<AudioClip, AudioError> {
    let mut reader = hound::WavReader::open(path)?;
    let spec = reader.spec();
    let mut samples: Vec<f32> = Vec::new();

    match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, 32) => {
            for sample in reader.samples::<f32>() {
                samples.push(sample?);
            }
        }
        (hound::SampleFormat::Int, 8) => {
            for sample in reader.samples::<i8>() {
                samples.push(sample? as f32 / 128.0);
            }
        }
        (hound::SampleFormat::Int, 16) => {
            for sample in reader.samples::<i16>() {
                samples.push(sample? as f32 / 32_768.0);
            }
        }
        (hound::SampleFormat::Int, 24) => {
            for sample in reader.samples::<i32>() {
                samples.push(sample? as f32 / 8_388_608.0);
            }
        }
        (hound::SampleFormat::Int, 32) => {
            for sample in reader.samples::<i32>() {
                samples.push(sample? as f32 / 2_147_483_648.0);
            }
        }
        (format, bits) => {
            return Err(AudioError::Decode(format!(
                "unsupported WAV layout: {format:?} at {bits} bits"
            )))
        }
    }

    let clip = AudioClip::new(spec.sample_rate, spec.channels, samples);
    clip.validate()?;
    Ok(clip)
}

/// float → i16 with the asymmetrically-clamped PCM range.
fn to_i16(sample: f32) -> i16 {
    if !sample.is_finite() {
        return 0;
    }
    (sample * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16
}

/// float → 24-bit signed PCM stored in `i32`.
fn to_i24(sample: f32) -> i32 {
    if !sample.is_finite() {
        return 0;
    }
    (sample * 8_388_607.0)
        .round()
        .clamp(-8_388_608.0, 8_388_607.0) as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_path(name: &str) -> std::path::PathBuf {
        let dir = std::env::temp_dir().join("kine-audio-wav-test");
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    #[test]
    fn wav_round_trip_preserves_geometry_and_values() {
        let frames = 500usize;
        let samples: Vec<f32> = (0..frames * 2)
            .map(|i| ((i as f32) / 1000.0).sin() * 0.75)
            .collect();
        let clip = AudioClip::new(22_050, 2, samples);

        let path = temp_path("roundtrip.wav");
        write_wav(&path, &clip).unwrap();
        let read = read_wav(&path).unwrap();

        assert_eq!(read.sample_rate, 22_050);
        assert_eq!(read.channels, 2);
        assert_eq!(read.frame_count(), frames);
        for (a, b) in read.samples.iter().zip(clip.samples.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn wav_24bit_and_float32_round_trip() {
        let clip = AudioClip::new(48_000, 1, vec![0.12345, -0.54321, 0.99]);
        let path24 = temp_path("rt24.wav");
        write_wav_with_format(&path24, &clip, WavSampleFormat::Pcm24).unwrap();
        let read24 = read_wav(&path24).unwrap();
        for (a, b) in read24.samples.iter().zip(clip.samples.iter()) {
            assert!((a - b).abs() < 1e-5);
        }

        let path32f = temp_path("rt32f.wav");
        write_wav_with_format(&path32f, &clip, WavSampleFormat::Float32).unwrap();
        let read32f = read_wav(&path32f).unwrap();
        assert_eq!(read32f.samples, clip.samples);
    }

    #[test]
    fn writing_a_degenerate_clip_is_rejected() {
        let broken = AudioClip::new(48_000, 2, vec![0.0; 3]);
        assert!(matches!(
            write_wav(&temp_path("broken.wav"), &broken).unwrap_err(),
            AudioError::InvalidClip(_)
        ));
    }

    #[test]
    fn reading_a_missing_file_is_a_typed_error() {
        assert!(matches!(
            read_wav(&temp_path("does-not-exist.wav")).unwrap_err(),
            AudioError::Wav(_)
        ));
    }
}
