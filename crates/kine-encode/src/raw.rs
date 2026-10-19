//! Uncompressed raw video streams (`RGBA8`, `RGB24`, `I420`, `NV12`) and
//! `.kvid` (Kine Indexed Lossless Video container with O(1) random frame seek
//! and full 8-bit alpha channel preservation).

use crate::color::{i420_to_rgba, nv12_to_rgba, rgba_to_i420, rgba_to_nv12, I420Frame};
use crate::EncodeError;

/// Uncompressed raw video pixel formats for pipe/stream input and output.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawPixelFormat {
    /// Straight or premultiplied 32-bit RGBA (`width * height * 4` bytes per frame).
    Rgba8,
    /// Packed 24-bit RGB (`width * height * 3` bytes per frame).
    Rgb24,
    /// Planar BT.709 limited-range 4:2:0 (`Y`, `U`, `V` planes).
    I420,
    /// Semi-planar BT.709 limited-range 4:2:0 (`Y` plane + interleaved `UV` plane).
    Nv12,
}

impl RawPixelFormat {
    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "rgba" | "rgba8" | "raw" => Some(Self::Rgba8),
            "rgb" | "rgb24" => Some(Self::Rgb24),
            "i420" | "yuv420p" | "yuv" => Some(Self::I420),
            "nv12" => Some(Self::Nv12),
            _ => None,
        }
    }

    pub fn frame_byte_len(self, width: u32, height: u32) -> usize {
        let w = width as usize;
        let h = height as usize;
        let cw = width.div_ceil(2) as usize;
        let ch = height.div_ceil(2) as usize;
        match self {
            Self::Rgba8 => w * h * 4,
            Self::Rgb24 => w * h * 3,
            Self::I420 | Self::Nv12 => w * h + cw * ch * 2,
        }
    }
}

/// Convert one RGBA8 frame (`width * height * 4`) into a raw pixel buffer.
pub fn encode_raw_frame(
    rgba: &[u8],
    width: u32,
    height: u32,
    fmt: RawPixelFormat,
) -> Result<Vec<u8>, EncodeError> {
    let expected = (width as usize) * (height as usize) * 4;
    if rgba.len() != expected {
        return Err(EncodeError::FrameSize {
            actual: rgba.len(),
            expected,
            width,
            height,
        });
    }
    match fmt {
        RawPixelFormat::Rgba8 => Ok(rgba.to_vec()),
        RawPixelFormat::Rgb24 => {
            let mut rgb = Vec::with_capacity((width as usize) * (height as usize) * 3);
            for px in rgba.chunks_exact(4) {
                rgb.extend_from_slice(&[px[0], px[1], px[2]]);
            }
            Ok(rgb)
        }
        RawPixelFormat::I420 => {
            let f = rgba_to_i420(rgba, width, height);
            let mut out = Vec::with_capacity(f.y.len() + f.u.len() + f.v.len());
            out.extend_from_slice(&f.y);
            out.extend_from_slice(&f.u);
            out.extend_from_slice(&f.v);
            Ok(out)
        }
        RawPixelFormat::Nv12 => Ok(rgba_to_nv12(rgba, width, height)),
    }
}

/// Decode a concatenated raw video byte stream into a sequence of RGBA8 frames.
pub fn decode_raw_stream(
    bytes: &[u8],
    width: u32,
    height: u32,
    fmt: RawPixelFormat,
) -> Result<Vec<Vec<u8>>, EncodeError> {
    if width == 0 || height == 0 {
        return Err(EncodeError::InvalidConfig(
            "raw video dimensions must be non-zero".into(),
        ));
    }
    let frame_len = fmt.frame_byte_len(width, height);
    if frame_len == 0 || bytes.len() < frame_len {
        return Err(EncodeError::Rejected(format!(
            "raw stream too short: {} bytes < 1 frame ({frame_len} bytes)",
            bytes.len()
        )));
    }
    let w = width as usize;
    let h = height as usize;
    let cw = width.div_ceil(2) as usize;
    let ch = height.div_ceil(2) as usize;
    let y_len = w * h;
    let c_len = cw * ch;

    let mut frames = Vec::with_capacity(bytes.len() / frame_len);
    for chunk in bytes.chunks_exact(frame_len) {
        let rgba = match fmt {
            RawPixelFormat::Rgba8 => chunk.to_vec(),
            RawPixelFormat::Rgb24 => {
                let mut out = Vec::with_capacity(w * h * 4);
                for px in chunk.chunks_exact(3) {
                    out.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                out
            }
            RawPixelFormat::I420 => {
                let f = I420Frame {
                    width,
                    height,
                    y: chunk[..y_len].to_vec(),
                    u: chunk[y_len..y_len + c_len].to_vec(),
                    v: chunk[y_len + c_len..y_len + c_len * 2].to_vec(),
                };
                i420_to_rgba(&f)
            }
            RawPixelFormat::Nv12 => {
                nv12_to_rgba(chunk, width, height).map_err(EncodeError::Rejected)?
            }
        };
        frames.push(rgba);
    }
    Ok(frames)
}

// ── `.kvid` Indexed Lossless RGBA Video Container ───────────────────────────
//
// Binary layout:
// - 0..8:   Magic `b"KVID\x01\x00\x00\x00"`
// - 8..12:  width (u32 LE)
// - 12..16: height (u32 LE)
// - 16..20: fps_num (u32 LE)
// - 20..24: fps_den (u32 LE)
// - 24..28: frame_count (u32 LE)
// - 28..32: flags (u32 LE, 0 = QOI lossless RGBA8 payload per frame)
// - 32..(32 + frame_count * 12): frame index entries `[offset: u64 LE, len: u32 LE]`
// - payload: QOI-compressed RGBA8 frames

const KVID_MAGIC: &[u8; 8] = b"KVID\x01\x00\x00\x00";

/// Decoded `.kvid` video clip with lossless RGBA8 frames.
#[derive(Debug, Clone)]
pub struct KvidVideo {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frames_rgba: Vec<Vec<u8>>,
}

impl KvidVideo {
    pub fn duration_seconds(&self) -> f64 {
        if self.fps_num == 0 {
            return 0.0;
        }
        (self.frames_rgba.len() as f64) * f64::from(self.fps_den.max(1)) / f64::from(self.fps_num)
    }

    pub fn frame_at_time(&self, time_sec: f64, loop_video: bool) -> Option<&[u8]> {
        if self.frames_rgba.is_empty() {
            return None;
        }
        let fps = f64::from(self.fps_num.max(1)) / f64::from(self.fps_den.max(1));
        let raw_idx = (time_sec.max(0.0) * fps).floor() as usize;
        let idx = if loop_video {
            raw_idx % self.frames_rgba.len()
        } else {
            raw_idx.min(self.frames_rgba.len() - 1)
        };
        Some(&self.frames_rgba[idx])
    }
}

#[inline]
fn qoi_hash(r: u8, g: u8, b: u8, a: u8) -> usize {
    (usize::from(r) * 3 + usize::from(g) * 5 + usize::from(b) * 7 + usize::from(a) * 11) % 64
}

fn encode_rgba_qoi_chunk(rgba: &[u8]) -> Vec<u8> {
    let total_px = rgba.len() / 4;
    let mut out = Vec::with_capacity(rgba.len() / 2 + 16);
    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0u8, 0u8, 255u8];
    let mut run = 0u8;

    for (px_i, px) in rgba.chunks_exact(4).enumerate() {
        let cur = [px[0], px[1], px[2], px[3]];
        if cur == prev {
            run += 1;
            if run == 62 || px_i + 1 == total_px {
                out.push(0xC0 | (run - 1));
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(0xC0 | (run - 1));
            run = 0;
        }
        let hash = qoi_hash(cur[0], cur[1], cur[2], cur[3]);
        if index[hash] == cur {
            out.push(hash as u8);
        } else {
            index[hash] = cur;
            if cur[3] == prev[3] {
                let vr = cur[0].wrapping_sub(prev[0]) as i8;
                let vg = cur[1].wrapping_sub(prev[1]) as i8;
                let vb = cur[2].wrapping_sub(prev[2]) as i8;
                let vg_r = vr.wrapping_sub(vg);
                let vg_b = vb.wrapping_sub(vg);
                if (-2..=1).contains(&vr) && (-2..=1).contains(&vg) && (-2..=1).contains(&vb) {
                    out.push(
                        0x40 | (((vr + 2) as u8) << 4)
                            | (((vg + 2) as u8) << 2)
                            | ((vb + 2) as u8),
                    );
                } else if (-8..=7).contains(&vg_r)
                    && (-32..=31).contains(&vg)
                    && (-8..=7).contains(&vg_b)
                {
                    out.push(0x80 | ((vg + 32) as u8));
                    out.push((((vg_r + 8) as u8) << 4) | ((vg_b + 8) as u8));
                } else {
                    out.extend_from_slice(&[0xFE, cur[0], cur[1], cur[2]]);
                }
            } else {
                out.extend_from_slice(&[0xFF, cur[0], cur[1], cur[2], cur[3]]);
            }
        }
        prev = cur;
    }
    out
}

fn decode_rgba_qoi_chunk(bytes: &[u8], total_px: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(total_px * 4);
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0u8, 0u8, 255u8];
    let mut run = 0u8;
    let mut pos = 0usize;
    let end = bytes.len();

    for _ in 0..total_px {
        if run > 0 {
            run -= 1;
        } else if pos < end {
            let b1 = bytes[pos];
            pos += 1;
            if b1 == 0xFE {
                if pos + 3 <= end {
                    px[0] = bytes[pos];
                    px[1] = bytes[pos + 1];
                    px[2] = bytes[pos + 2];
                    pos += 3;
                }
            } else if b1 == 0xFF {
                if pos + 4 <= end {
                    px[0] = bytes[pos];
                    px[1] = bytes[pos + 1];
                    px[2] = bytes[pos + 2];
                    px[3] = bytes[pos + 3];
                    pos += 4;
                }
            } else {
                match b1 & 0xC0 {
                    0x00 => {
                        px = index[(b1 & 0x3F) as usize];
                    }
                    0x40 => {
                        px[0] = px[0].wrapping_add(((b1 >> 4) & 0x03).wrapping_sub(2));
                        px[1] = px[1].wrapping_add(((b1 >> 2) & 0x03).wrapping_sub(2));
                        px[2] = px[2].wrapping_add((b1 & 0x03).wrapping_sub(2));
                    }
                    0x80 => {
                        if pos < end {
                            let b2 = bytes[pos];
                            pos += 1;
                            let vg = (b1 & 0x3F).wrapping_sub(32);
                            px[0] = px[0]
                                .wrapping_add(vg.wrapping_sub(8).wrapping_add((b2 >> 4) & 0x0F));
                            px[1] = px[1].wrapping_add(vg);
                            px[2] =
                                px[2].wrapping_add(vg.wrapping_sub(8).wrapping_add(b2 & 0x0F));
                        }
                    }
                    0xC0 => {
                        run = b1 & 0x3F;
                    }
                    _ => unreachable!(),
                }
            }
            index[qoi_hash(px[0], px[1], px[2], px[3])] = px;
        }
        out.extend_from_slice(&px);
    }
    out
}

/// Encode a sequence of RGBA8 frames into an indexed `.kvid` container.
pub fn encode_kvid(
    frames_rgba: &[Vec<u8>],
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
) -> Result<Vec<u8>, EncodeError> {
    if width == 0 || height == 0 {
        return Err(EncodeError::InvalidConfig(
            "KVID width and height must be non-zero".into(),
        ));
    }
    if frames_rgba.is_empty() {
        return Err(EncodeError::NoOutput);
    }
    let expected = (width as usize) * (height as usize) * 4;
    let mut compressed_frames = Vec::with_capacity(frames_rgba.len());
    for f in frames_rgba {
        if f.len() != expected {
            return Err(EncodeError::FrameSize {
                actual: f.len(),
                expected,
                width,
                height,
            });
        }
        compressed_frames.push(encode_rgba_qoi_chunk(f));
    }

    let frame_count = compressed_frames.len() as u32;
    let header_and_index_size = 32 + (compressed_frames.len() * 12);
    let total_payload: usize = compressed_frames.iter().map(Vec::len).sum();
    let mut out = Vec::with_capacity(header_and_index_size + total_payload);

    out.extend_from_slice(KVID_MAGIC);
    out.extend_from_slice(&width.to_le_bytes());
    out.extend_from_slice(&height.to_le_bytes());
    out.extend_from_slice(&fps_num.max(1).to_le_bytes());
    out.extend_from_slice(&fps_den.max(1).to_le_bytes());
    out.extend_from_slice(&frame_count.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes()); // flags = 0 (QOI chunks)

    let mut cur_offset = header_and_index_size as u64;
    for chunk in &compressed_frames {
        let len = chunk.len() as u32;
        out.extend_from_slice(&cur_offset.to_le_bytes());
        out.extend_from_slice(&len.to_le_bytes());
        cur_offset += u64::from(len);
    }

    for chunk in &compressed_frames {
        out.extend_from_slice(chunk);
    }

    Ok(out)
}

/// Decode a `.kvid` byte buffer into a [`KvidVideo`].
pub fn decode_kvid(bytes: &[u8]) -> Result<KvidVideo, EncodeError> {
    if bytes.len() < 32 || &bytes[0..8] != KVID_MAGIC {
        return Err(EncodeError::InvalidConfig("invalid KVID header magic".into()));
    }
    let width = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let height = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let fps_num = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]).max(1);
    let fps_den = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]).max(1);
    let frame_count = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as usize;

    let index_end = 32 + frame_count * 12;
    if width == 0 || height == 0 || bytes.len() < index_end {
        return Err(EncodeError::Rejected("truncated KVID frame table".into()));
    }

    let total_px = (width as usize) * (height as usize);
    let mut frames_rgba = Vec::with_capacity(frame_count);
    for i in 0..frame_count {
        let base = 32 + i * 12;
        let offset = u64::from_le_bytes([
            bytes[base],
            bytes[base + 1],
            bytes[base + 2],
            bytes[base + 3],
            bytes[base + 4],
            bytes[base + 5],
            bytes[base + 6],
            bytes[base + 7],
        ]) as usize;
        let len = u32::from_le_bytes([
            bytes[base + 8],
            bytes[base + 9],
            bytes[base + 10],
            bytes[base + 11],
        ]) as usize;
        if offset + len > bytes.len() {
            return Err(EncodeError::Rejected(format!(
                "truncated KVID frame {i} at offset {offset}+{len}"
            )));
        }
        let rgba = decode_rgba_qoi_chunk(&bytes[offset..offset + len], total_px);
        frames_rgba.push(rgba);
    }

    Ok(KvidVideo {
        width,
        height,
        fps_num,
        fps_den,
        frames_rgba,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kvid_round_trips_rgba_with_alpha_losslessly() {
        let f0 = vec![
            10, 20, 30, 128, 40, 50, 60, 255, 70, 80, 90, 0, 100, 110, 120, 200,
        ];
        let f1 = vec![
            250, 120, 30, 255, 250, 120, 30, 255, 12, 34, 56, 78, 90, 91, 92, 93,
        ];
        let encoded = encode_kvid(&[f0.clone(), f1.clone()], 2, 2, 24, 1).unwrap();
        let decoded = decode_kvid(&encoded).unwrap();
        assert_eq!(decoded.width, 2);
        assert_eq!(decoded.height, 2);
        assert_eq!(decoded.fps_num, 24);
        assert_eq!(decoded.frames_rgba.len(), 2);
        assert_eq!(decoded.frames_rgba[0], f0);
        assert_eq!(decoded.frames_rgba[1], f1);
    }

    #[test]
    fn raw_formats_encode_and_decode() {
        let f0 = vec![200u8; 4 * 4 * 4];
        for fmt in [
            RawPixelFormat::Rgba8,
            RawPixelFormat::Rgb24,
            RawPixelFormat::I420,
            RawPixelFormat::Nv12,
        ] {
            let raw = encode_raw_frame(&f0, 4, 4, fmt).unwrap();
            let decoded = decode_raw_stream(&raw, 4, 4, fmt).unwrap();
            assert_eq!(decoded.len(), 1);
            assert_eq!(decoded[0].len(), 4 * 4 * 4);
        }
    }
}
