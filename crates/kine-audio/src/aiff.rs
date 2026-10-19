//! Pure-Rust AIFF (Audio Interchange File Format, `FORM`/`AIFF`) 16-bit big-endian
//! PCM reader and writer.

use crate::{AudioClip, AudioError};
use std::path::Path;

/// Encode `clip` as a standard 16-bit big-endian AIFF byte buffer.
pub fn encode_aiff(clip: &AudioClip) -> Result<Vec<u8>, AudioError> {
    clip.validate()?;
    let frames = clip.frame_count() as u32;
    let channels = clip.channels;
    let pcm_bytes_len = (clip.samples.len() * 2) as u32;
    // COMM chunk is 18 bytes payload; SSND chunk is 8 bytes (offset + blockSize) + pcm_bytes
    let ssnd_chunk_len = 8 + pcm_bytes_len;
    let form_payload_len = 4 + (8 + 18) + (8 + ssnd_chunk_len + (ssnd_chunk_len & 1));

    let mut out = Vec::with_capacity((8 + form_payload_len) as usize);
    out.extend_from_slice(b"FORM");
    out.extend_from_slice(&form_payload_len.to_be_bytes());
    out.extend_from_slice(b"AIFF");

    // COMM chunk
    out.extend_from_slice(b"COMM");
    out.extend_from_slice(&18u32.to_be_bytes());
    out.extend_from_slice(&channels.to_be_bytes());
    out.extend_from_slice(&frames.to_be_bytes());
    out.extend_from_slice(&16u16.to_be_bytes()); // 16 bits per sample
    out.extend_from_slice(&encode_ieee_extended_80(clip.sample_rate as f64));

    // SSND chunk
    out.extend_from_slice(b"SSND");
    out.extend_from_slice(&ssnd_chunk_len.to_be_bytes());
    out.extend_from_slice(&0u32.to_be_bytes()); // offset
    out.extend_from_slice(&0u32.to_be_bytes()); // blockSize
    for &s in &clip.samples {
        let q = if !s.is_finite() {
            0i16
        } else {
            (s * 32_767.0).round().clamp(-32_768.0, 32_767.0) as i16
        };
        out.extend_from_slice(&q.to_be_bytes());
    }
    if ssnd_chunk_len & 1 == 1 {
        out.push(0);
    }
    Ok(out)
}

/// Write `clip` to disk as a 16-bit PCM AIFF file.
pub fn write_aiff(path: &Path, clip: &AudioClip) -> Result<(), AudioError> {
    let bytes = encode_aiff(clip)?;
    std::fs::write(path, bytes)?;
    Ok(())
}

/// Decode an AIFF (`FORM`/`AIFF`) byte buffer into an [`AudioClip`].
pub fn decode_aiff_bytes(bytes: &[u8]) -> Result<AudioClip, AudioError> {
    if bytes.len() < 12 || &bytes[0..4] != b"FORM" || &bytes[8..12] != b"AIFF" {
        return Err(AudioError::Decode("not a valid FORM/AIFF header".into()));
    }
    let mut pos = 12usize;
    let mut channels: Option<u16> = None;
    let mut frames: Option<usize> = None;
    let mut bits_per_sample: Option<u16> = None;
    let mut sample_rate: Option<u32> = None;
    let mut ssnd_payload: Option<&[u8]> = None;

    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_be_bytes([
            bytes[pos + 4],
            bytes[pos + 5],
            bytes[pos + 6],
            bytes[pos + 7],
        ]) as usize;
        pos += 8;
        if pos + len > bytes.len() {
            break;
        }
        let chunk = &bytes[pos..pos + len];
        if id == b"COMM" && chunk.len() >= 18 {
            channels = Some(u16::from_be_bytes([chunk[0], chunk[1]]));
            frames = Some(u32::from_be_bytes([chunk[2], chunk[3], chunk[4], chunk[5]]) as usize);
            bits_per_sample = Some(u16::from_be_bytes([chunk[6], chunk[7]]));
            let mut ext = [0u8; 10];
            ext.copy_from_slice(&chunk[8..18]);
            sample_rate = Some(decode_ieee_extended_80(&ext).round().max(1.0) as u32);
        } else if id == b"SSND" && chunk.len() >= 8 {
            let offset = u32::from_be_bytes([chunk[0], chunk[1], chunk[2], chunk[3]]) as usize;
            let start = 8 + offset;
            if start <= chunk.len() {
                ssnd_payload = Some(&chunk[start..]);
            }
        }
        pos += len + (len & 1);
    }

    let ch = channels.ok_or_else(|| AudioError::Decode("AIFF missing COMM chunk".into()))?;
    let total_frames = frames.unwrap_or(0);
    let bits = bits_per_sample.unwrap_or(16);
    let rate = sample_rate.unwrap_or(48_000);
    let data = ssnd_payload.ok_or_else(|| AudioError::Decode("AIFF missing SSND chunk".into()))?;

    let total_samples = total_frames * (ch as usize);
    let mut samples = Vec::with_capacity(total_samples);
    match bits {
        16 => {
            for pair in data.chunks_exact(2).take(total_samples) {
                let v = i16::from_be_bytes([pair[0], pair[1]]);
                samples.push(f32::from(v) / 32_768.0);
            }
        }
        8 => {
            for &b in data.iter().take(total_samples) {
                samples.push(f32::from(b as i8) / 128.0);
            }
        }
        24 => {
            for tri in data.chunks_exact(3).take(total_samples) {
                let raw = ((i32::from(tri[0] as i8)) << 16)
                    | (i32::from(tri[1]) << 8)
                    | i32::from(tri[2]);
                samples.push((raw as f32) / 8_388_608.0);
            }
        }
        other => {
            return Err(AudioError::Decode(format!(
                "unsupported AIFF bit depth: {other}"
            )))
        }
    }

    let clip = AudioClip::new(rate, ch, samples);
    clip.validate()?;
    Ok(clip)
}

/// Read an AIFF file from disk.
pub fn read_aiff(path: &Path) -> Result<AudioClip, AudioError> {
    let bytes = std::fs::read(path)?;
    decode_aiff_bytes(&bytes)
}

fn encode_ieee_extended_80(value: f64) -> [u8; 10] {
    if value <= 0.0 || !value.is_finite() {
        return [0u8; 10];
    }
    let mut v = value;
    let mut exp: i32 = 16383;
    while v >= 2.0 {
        v *= 0.5;
        exp += 1;
    }
    while v < 1.0 {
        v *= 2.0;
        exp -= 1;
    }
    let mantissa = (v * ((1u64 << 63) as f64)).round() as u64;
    let mut out = [0u8; 10];
    out[0..2].copy_from_slice(&(exp as u16).to_be_bytes());
    out[2..10].copy_from_slice(&mantissa.to_be_bytes());
    out
}

fn decode_ieee_extended_80(bytes: &[u8; 10]) -> f64 {
    let exp = (u16::from_be_bytes([bytes[0], bytes[1]]) & 0x7FFF) as i32;
    let mantissa = u64::from_be_bytes([
        bytes[2], bytes[3], bytes[4], bytes[5], bytes[6], bytes[7], bytes[8], bytes[9],
    ]);
    if exp == 0 && mantissa == 0 {
        return 0.0;
    }
    let f = (mantissa as f64) / ((1u64 << 63) as f64);
    f * 2.0f64.powi(exp - 16383)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn aiff_round_trip_preserves_samples() {
        let clip = AudioClip::new(48_000, 2, vec![0.0, 0.25, -0.5, 0.75, -0.25, 0.1]);
        let bytes = encode_aiff(&clip).unwrap();
        let decoded = decode_aiff_bytes(&bytes).unwrap();
        assert_eq!(decoded.sample_rate, 48_000);
        assert_eq!(decoded.channels, 2);
        assert_eq!(decoded.frame_count(), 3);
        for (a, b) in decoded.samples.iter().zip(clip.samples.iter()) {
            assert!((a - b).abs() < 1e-3, "{a} vs {b}");
        }
    }
}
