use crate::aiff::decode_aiff_bytes;
use crate::synth::synthesize_uri;
use crate::{AudioClip, AudioError};
use std::path::Path;
use symphonia::core::codecs::audio::AudioDecoderOptions;
use symphonia::core::codecs::CodecParameters;
use symphonia::core::errors::Error as SymphoniaError;
use symphonia::core::formats::probe::Hint;
use symphonia::core::formats::{FormatOptions, TrackType};
use symphonia::core::io::{MediaSource, MediaSourceStream, MediaSourceStreamOptions};
use symphonia::core::meta::MetadataOptions;

/// Decodes any container `symphonia` is built with (wav/flac/mp3/ogg/aac/mp4/mkv/caf),
/// plus AIFF (`FORM`/`AIFF`) and procedural `synth:...` URIs, into interleaved `f32`.
pub fn decode_file(path: &Path) -> Result<AudioClip, AudioError> {
    if let Some(s) = path.to_str() {
        if let Some(idx) = s.find("synth:") {
            return synthesize_uri(&s[idx..], 3.0, 48_000, 2);
        }
    }
    let bytes = std::fs::read(path)?;
    let hint = path
        .extension()
        .and_then(|ext| ext.to_str())
        .map(str::to_ascii_lowercase);
    decode_bytes(&bytes, hint.as_deref())
}

/// Decode an audio container from an in-memory byte slice.
pub fn decode_bytes(bytes: &[u8], extension_hint: Option<&str>) -> Result<AudioClip, AudioError> {
    if bytes.len() >= 12 && &bytes[0..4] == b"FORM" && &bytes[8..12] == b"AIFF" {
        return decode_aiff_bytes(bytes);
    }
    let cursor = std::io::Cursor::new(bytes.to_vec());
    decode_source(Box::new(cursor), extension_hint)
}

/// Shared decode path over any `MediaSource`.
pub(crate) fn decode_source(
    source: Box<dyn MediaSource + '_>,
    extension_hint: Option<&str>,
) -> Result<AudioClip, AudioError> {
    let mss = MediaSourceStream::new(source, MediaSourceStreamOptions::default());
    let mut hint = Hint::new();
    if let Some(ext) = extension_hint {
        hint.with_extension(ext);
    }

    let mut reader = symphonia::default::get_probe()
        .probe(
            &hint,
            mss,
            FormatOptions::default(),
            MetadataOptions::default(),
        )
        .map_err(decode_error)?;

    let (track_id, params) = {
        let track = reader
            .default_track(TrackType::Audio)
            .ok_or(AudioError::NoAudioTrack)?;
        let params = match &track.codec_params {
            Some(CodecParameters::Audio(params)) => params.clone(),
            _ => return Err(AudioError::NoAudioTrack),
        };
        (track.id, params)
    };

    let mut decoder = symphonia::default::get_codecs()
        .make_audio_decoder(&params, &AudioDecoderOptions::default())
        .map_err(decode_error)?;

    let mut samples: Vec<f32> = Vec::new();
    let mut packet_samples: Vec<f32> = Vec::new();
    let mut sample_rate = params.sample_rate.unwrap_or(0);
    let mut channels = params
        .channels
        .as_ref()
        .map(|c| c.count())
        .unwrap_or(0);

    loop {
        let packet = match reader.next_packet() {
            Ok(Some(packet)) => packet,
            Ok(None) => break,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(err) => return Err(decode_error(err)),
        };
        if packet.track_id != track_id {
            continue;
        }

        let decoded = match decoder.decode(&packet) {
            Ok(decoded) => decoded,
            Err(SymphoniaError::DecodeError(_)) => continue,
            Err(SymphoniaError::IoError(err))
                if err.kind() == std::io::ErrorKind::UnexpectedEof =>
            {
                break
            }
            Err(err) => return Err(decode_error(err)),
        };

        let spec = decoded.spec();
        sample_rate = spec.rate();
        channels = spec.channels().count();
        packet_samples.clear();
        decoded.copy_to_vec_interleaved(&mut packet_samples);
        samples.extend_from_slice(&packet_samples);
    }

    if channels == 0 || channels > u16::MAX as usize {
        return Err(AudioError::Decode(format!(
            "decoded channel count {channels} is not representable"
        )));
    }
    if sample_rate == 0 {
        return Err(AudioError::Decode("stream declares no sample rate".into()));
    }

    let clip = AudioClip::new(sample_rate, channels as u16, samples);
    clip.validate()?;
    Ok(clip)
}

fn decode_error(err: SymphoniaError) -> AudioError {
    AudioError::Decode(err.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::write_wav;

    #[test]
    fn missing_file_is_a_typed_io_error() {
        let err = decode_file(Path::new("/nonexistent/kine-audio/nope.wav")).unwrap_err();
        assert!(matches!(err, AudioError::Io(_)), "got {err:?}");
    }

    #[test]
    fn malformed_input_does_not_panic() {
        let garbage = vec![0x00u8; 64];
        let cursor = std::io::Cursor::new(garbage);
        let err = decode_source(Box::new(cursor), Some("wav")).unwrap_err();
        assert!(matches!(err, AudioError::Decode(_)), "got {err:?}");
    }

    #[test]
    fn decodes_a_wav_we_wrote() {
        let rate = 8_000u32;
        let frames = 800usize;
        let samples: Vec<f32> = (0..frames)
            .map(|i| (2.0 * std::f32::consts::PI * 440.0 * i as f32 / rate as f32).sin())
            .collect();
        let clip = AudioClip::new(rate, 1, samples.clone());

        let dir = std::env::temp_dir().join("kine-audio-decode-test");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("sine.wav");
        write_wav(&path, &clip).unwrap();

        let decoded = decode_file(&path).unwrap();
        assert_eq!(decoded.sample_rate, rate);
        assert_eq!(decoded.channels, 1);
        assert_eq!(decoded.frame_count(), frames);

        for (a, b) in decoded.samples.iter().zip(clip.samples.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }

    #[test]
    fn decodes_synth_uri_directly() {
        let clip = decode_file(Path::new("synth:sine:440:0.25")).unwrap();
        assert_eq!(clip.sample_rate, 48_000);
        assert_eq!(clip.frame_count(), 12_000);
    }
}
