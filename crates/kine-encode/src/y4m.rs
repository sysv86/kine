//! Lossless `YUV4MPEG2` (`.y4m`) stream writer and reader for BT.709 planar
//! frames (`4:2:0`, `4:2:2`, `4:4:4`, and `mono`).

use crate::color::{i420_to_rgba, i444_to_rgba, rgba_to_i420, I420Frame};
use crate::EncodeError;
use std::io::Write;

/// Decoded `YUV4MPEG2` video clip in memory.
#[derive(Debug, Clone)]
pub struct Y4mVideo {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub colorspace: String,
    /// Straight RGBA8 frames (`width * height * 4` bytes per frame).
    pub frames_rgba: Vec<Vec<u8>>,
}

impl Y4mVideo {
    /// Total duration of the decoded clip in seconds.
    pub fn duration_seconds(&self) -> f64 {
        if self.fps_num == 0 {
            return 0.0;
        }
        (self.frames_rgba.len() as f64) * f64::from(self.fps_den.max(1)) / f64::from(self.fps_num)
    }

    /// Sample the frame corresponding to `time_sec` (seconds).
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

/// Decode a `YUV4MPEG2` (`.y4m`) byte stream into a [`Y4mVideo`].
pub fn decode_y4m(bytes: &[u8]) -> Result<Y4mVideo, EncodeError> {
    let Some(header_end) = bytes.iter().position(|&b| b == b'\n') else {
        return Err(EncodeError::InvalidConfig("missing YUV4MPEG2 header newline".into()));
    };
    let header_str = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| EncodeError::InvalidConfig("invalid UTF-8 in YUV4MPEG2 header".into()))?;
    if !header_str.starts_with("YUV4MPEG2") {
        return Err(EncodeError::InvalidConfig(
            "stream does not start with YUV4MPEG2 magic".into(),
        ));
    }

    let mut width: u32 = 0;
    let mut height: u32 = 0;
    let mut fps_num: u32 = 30;
    let mut fps_den: u32 = 1;
    let mut colorspace = "420".to_string();

    for token in header_str.split_whitespace().skip(1) {
        if let Some(w) = token.strip_prefix('W') {
            width = w.parse().unwrap_or(0);
        } else if let Some(h) = token.strip_prefix('H') {
            height = h.parse().unwrap_or(0);
        } else if let Some(f) = token.strip_prefix('F') {
            if let Some((n, d)) = f.split_once(':') {
                fps_num = n.parse().unwrap_or(30).max(1);
                fps_den = d.parse().unwrap_or(1).max(1);
            }
        } else if let Some(c) = token.strip_prefix('C') {
            colorspace = c.to_ascii_lowercase();
        }
    }

    if width == 0 || height == 0 {
        return Err(EncodeError::InvalidConfig(
            "YUV4MPEG2 header missing valid W/H dimensions".into(),
        ));
    }

    let w = width as usize;
    let h = height as usize;
    let y_len = w * h;
    let (cw, ch, chroma_planes) = if colorspace.starts_with("mono") {
        (0usize, 0usize, 0usize)
    } else if colorspace.starts_with("444") {
        (w, h, 2usize)
    } else if colorspace.starts_with("422") {
        (width.div_ceil(2) as usize, h, 2usize)
    } else {
        // Default 4:2:0 (`420`, `420jpeg`, `420mpeg2`, `420paldv`)
        (width.div_ceil(2) as usize, height.div_ceil(2) as usize, 2usize)
    };
    let c_len = cw * ch;
    let frame_payload_len = y_len + c_len * chroma_planes;

    let mut pos = header_end + 1;
    let mut frames_rgba = Vec::new();

    while pos < bytes.len() {
        let rem = &bytes[pos..];
        if !rem.starts_with(b"FRAME") {
            break;
        }
        let Some(frame_hdr_end) = rem.iter().position(|&b| b == b'\n') else {
            return Err(EncodeError::Rejected("truncated Y4M FRAME header".into()));
        };
        pos += frame_hdr_end + 1;
        if pos + frame_payload_len > bytes.len() {
            return Err(EncodeError::Rejected("truncated Y4M frame planes".into()));
        }

        let y_slice = &bytes[pos..pos + y_len];
        let rgba = if chroma_planes == 0 {
            let mut out = vec![255u8; w * h * 4];
            for (i, &yb) in y_slice.iter().enumerate() {
                let v = ((f32::from(yb.clamp(16, 235)) - 16.0) * (255.0 / 219.0)).round() as u8;
                out[i * 4] = v;
                out[i * 4 + 1] = v;
                out[i * 4 + 2] = v;
                out[i * 4 + 3] = 255;
            }
            out
        } else if colorspace.starts_with("444") {
            let u_slice = &bytes[pos + y_len..pos + y_len + c_len];
            let v_slice = &bytes[pos + y_len + c_len..pos + y_len + c_len * 2];
            i444_to_rgba(y_slice, u_slice, v_slice, width, height)
        } else if colorspace.starts_with("422") {
            let u_slice = &bytes[pos + y_len..pos + y_len + c_len];
            let v_slice = &bytes[pos + y_len + c_len..pos + y_len + c_len * 2];
            // Downsample 4:2:2 vertically by 2 into I420 for unified conversion
            let ch_420 = height.div_ceil(2) as usize;
            let mut u_420 = vec![128u8; cw * ch_420];
            let mut v_420 = vec![128u8; cw * ch_420];
            for cy in 0..ch_420 {
                let r0 = (cy * 2).min(h - 1);
                let r1 = (cy * 2 + 1).min(h - 1);
                for cx in 0..cw {
                    u_420[cy * cw + cx] =
                        ((u16::from(u_slice[r0 * cw + cx]) + u16::from(u_slice[r1 * cw + cx])) / 2)
                            as u8;
                    v_420[cy * cw + cx] =
                        ((u16::from(v_slice[r0 * cw + cx]) + u16::from(v_slice[r1 * cw + cx])) / 2)
                            as u8;
                }
            }
            let frame = I420Frame {
                width,
                height,
                y: y_slice.to_vec(),
                u: u_420,
                v: v_420,
            };
            i420_to_rgba(&frame)
        } else {
            let u_slice = &bytes[pos + y_len..pos + y_len + c_len];
            let v_slice = &bytes[pos + y_len + c_len..pos + y_len + c_len * 2];
            let frame = I420Frame {
                width,
                height,
                y: y_slice.to_vec(),
                u: u_slice.to_vec(),
                v: v_slice.to_vec(),
            };
            i420_to_rgba(&frame)
        };

        frames_rgba.push(rgba);
        pos += frame_payload_len;
    }

    if frames_rgba.is_empty() {
        return Err(EncodeError::NoOutput);
    }

    Ok(Y4mVideo {
        width,
        height,
        fps_num,
        fps_den,
        colorspace,
        frames_rgba,
    })
}

/// Streaming `YUV4MPEG2` (`.y4m`) writer.
pub struct Y4mEncoder<W: Write> {
    sink: W,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    header_written: bool,
    frames: u64,
}

impl<W: Write> Y4mEncoder<W> {
    pub fn new(
        sink: W,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
    ) -> Result<Self, EncodeError> {
        if width == 0 || height == 0 {
            return Err(EncodeError::InvalidConfig(
                "Y4M width and height must be non-zero".into(),
            ));
        }
        if fps_num == 0 || fps_den == 0 {
            return Err(EncodeError::InvalidConfig(
                "Y4M frame rate must be non-zero".into(),
            ));
        }
        Ok(Self {
            sink,
            width,
            height,
            fps_num,
            fps_den,
            header_written: false,
            frames: 0,
        })
    }

    fn ensure_header(&mut self) -> Result<(), EncodeError> {
        if !self.header_written {
            let header = format!(
                "YUV4MPEG2 W{} H{} F{}:{} Ip A1:1 C420mpeg2 XYSCSS=420MPEG2 XCOLORRANGE=LIMITED\n",
                self.width, self.height, self.fps_num, self.fps_den
            );
            self.sink
                .write_all(header.as_bytes())
                .map_err(|e| EncodeError::Rejected(format!("y4m write header: {e}")))?;
            self.header_written = true;
        }
        Ok(())
    }

    /// Convert one premultiplied RGBA8 frame to BT.709 limited-range I420 and
    /// write it as a `FRAME\n` record.
    pub fn write_rgba(&mut self, rgba: &[u8]) -> Result<(), EncodeError> {
        let expected = (self.width as usize) * (self.height as usize) * 4;
        if rgba.len() != expected {
            return Err(EncodeError::FrameSize {
                actual: rgba.len(),
                expected,
                width: self.width,
                height: self.height,
            });
        }
        let i420 = rgba_to_i420(rgba, self.width, self.height);
        self.write_i420(&i420)
    }

    /// Write an already-converted [`I420Frame`] as a `FRAME\n` record.
    pub fn write_i420(&mut self, frame: &I420Frame) -> Result<(), EncodeError> {
        self.ensure_header()?;
        self.sink
            .write_all(b"FRAME\n")
            .map_err(|e| EncodeError::Rejected(format!("y4m write frame tag: {e}")))?;
        self.sink
            .write_all(&frame.y)
            .map_err(|e| EncodeError::Rejected(format!("y4m write Y plane: {e}")))?;
        self.sink
            .write_all(&frame.u)
            .map_err(|e| EncodeError::Rejected(format!("y4m write U plane: {e}")))?;
        self.sink
            .write_all(&frame.v)
            .map_err(|e| EncodeError::Rejected(format!("y4m write V plane: {e}")))?;
        self.frames += 1;
        Ok(())
    }

    pub fn frames_written(&self) -> u64 {
        self.frames
    }

    pub fn finish(mut self) -> Result<W, EncodeError> {
        self.ensure_header()?;
        self.sink
            .flush()
            .map_err(|e| EncodeError::Rejected(format!("y4m flush: {e}")))?;
        Ok(self.sink)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn writes_and_decodes_valid_yuv4mpeg2_stream() {
        let mut enc = Y4mEncoder::new(Vec::new(), 4, 4, 30, 1).unwrap();
        let rgba1: Vec<u8> = [200u8, 200, 200, 255].repeat(4 * 4);
        let rgba2: Vec<u8> = [40u8, 40, 40, 255].repeat(4 * 4);
        enc.write_rgba(&rgba1).unwrap();
        enc.write_rgba(&rgba2).unwrap();
        assert_eq!(enc.frames_written(), 2);
        let bytes = enc.finish().unwrap();
        assert!(bytes.starts_with(b"YUV4MPEG2 W4 H4 F30:1 Ip A1:1 C420mpeg2"));

        let decoded = decode_y4m(&bytes).unwrap();
        assert_eq!(decoded.width, 4);
        assert_eq!(decoded.height, 4);
        assert_eq!(decoded.fps_num, 30);
        assert_eq!(decoded.fps_den, 1);
        assert_eq!(decoded.frames_rgba.len(), 2);
        assert!((i32::from(decoded.frames_rgba[0][0]) - 200).abs() <= 2);
        assert!((i32::from(decoded.frames_rgba[1][0]) - 40).abs() <= 2);
    }
}
