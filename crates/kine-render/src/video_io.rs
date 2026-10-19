//! Video Input & Frame-Sequence I/O for Kine:
//! * Pure-Rust decoders for `.y4m` (`YUV4MPEG2`), `.gif` (`GIF87a`/`GIF89a` with
//!   variable-width LZW), `.kvid` (Kine Indexed Lossless RGBA Video), and image
//!   sequence directories (`.png`, `.qoi`, `.bmp`, `.ppm`, `.pam`, `.tga`).
//! * Procedural test-pattern video generators (`video:smpte`, `video:plasma`,
//!   `video:grid`) for zero-file testing and calibration.
//! * Optional `ffmpeg` pipe demuxing for compressed container inputs (`.mp4`,
//!   `.mkv`, `.webm`, `.mov`) when `ffmpeg` is installed on the host.
//! * Cached timeline frame sampler (`VideoCache`) supporting `start`,
//!   `trim_start`, `trim_end`, `playback_rate`, and `loop_video`.

use crate::formats::decode_any_image;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;

/// A decoded video clip held in memory as straight-alpha RGBA8 frames.
#[derive(Debug, Clone)]
pub struct DecodedVideoClip {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Straight RGBA8 buffers (`width * height * 4` bytes per frame).
    pub frames_rgba: Vec<Vec<u8>>,
    /// Optional cumulative end timestamps (in seconds) for variable-frame-rate clips (e.g. GIF).
    pub cumulative_times: Option<Vec<f64>>,
}

impl DecodedVideoClip {
    /// Total duration of the clip in seconds.
    pub fn duration_seconds(&self) -> f64 {
        if let Some(times) = &self.cumulative_times {
            if let Some(&last) = times.last() {
                if last > 0.0 {
                    return last;
                }
            }
        }
        if self.fps_num == 0 {
            return 0.0;
        }
        (self.frames_rgba.len() as f64) * f64::from(self.fps_den.max(1)) / f64::from(self.fps_num)
    }

    /// Sample the RGBA8 frame for `scene_time` (in seconds) given node timing parameters.
    pub fn sample_frame(
        &self,
        scene_time: f64,
        start: f64,
        trim_start: f64,
        trim_end: Option<f64>,
        playback_rate: f64,
        loop_video: bool,
    ) -> Option<&[u8]> {
        if self.frames_rgba.is_empty() || scene_time < start {
            return None;
        }
        let rate = if playback_rate.is_finite() && playback_rate > 0.0 {
            playback_rate
        } else {
            1.0
        };
        let clip_dur = self.duration_seconds().max(1e-6);
        let eff_start = trim_start.clamp(0.0, clip_dur);
        let eff_end = trim_end
            .unwrap_or(clip_dur)
            .clamp(eff_start + 1e-6, clip_dur);
        let window = (eff_end - eff_start).max(1e-6);

        let elapsed = (scene_time - start) * rate;
        let source_t = if loop_video {
            eff_start + (elapsed % window)
        } else if elapsed >= window {
            eff_end - 1e-6
        } else {
            eff_start + elapsed
        };

        if let Some(times) = &self.cumulative_times {
            let idx = times
                .iter()
                .position(|&t_end| source_t < t_end)
                .unwrap_or(self.frames_rgba.len() - 1);
            return Some(&self.frames_rgba[idx]);
        }

        let fps = f64::from(self.fps_num.max(1)) / f64::from(self.fps_den.max(1));
        let idx = ((source_t.max(0.0) * fps).floor() as usize).min(self.frames_rgba.len() - 1);
        Some(&self.frames_rgba[idx])
    }
}

/// Cache of decoded video clips keyed by source path or URI.
#[derive(Debug, Default, Clone)]
pub struct VideoCache {
    clips: HashMap<String, Arc<DecodedVideoClip>>,
}

impl VideoCache {
    pub fn new() -> Self {
        Self {
            clips: HashMap::new(),
        }
    }

    pub fn get_or_load(&mut self, src: &str) -> Result<Arc<DecodedVideoClip>, String> {
        if let Some(existing) = self.clips.get(src) {
            return Ok(Arc::clone(existing));
        }
        let decoded = Arc::new(decode_video_source(src)?);
        self.clips.insert(src.to_string(), Arc::clone(&decoded));
        Ok(decoded)
    }

    pub fn insert(&mut self, src: impl Into<String>, clip: DecodedVideoClip) {
        self.clips.insert(src.into(), Arc::new(clip));
    }

    /// Insert an already-decoded clip without copying its frames.
    pub fn insert_shared(&mut self, src: impl Into<String>, clip: Arc<DecodedVideoClip>) {
        self.clips.insert(src.into(), clip);
    }

    /// Shared handles to every cached clip, cheap to mirror into another cache.
    pub fn shared_clips(&self) -> Vec<(String, Arc<DecodedVideoClip>)> {
        self.clips
            .iter()
            .map(|(src, clip)| (src.clone(), Arc::clone(clip)))
            .collect()
    }
}

/// Decode any supported video source path or URI into a [`DecodedVideoClip`].
pub fn decode_video_source(src: &str) -> Result<DecodedVideoClip, String> {
    if src.starts_with("video:") {
        return synthesize_procedural_video(src);
    }
    if let Some(dir_str) = src.strip_prefix("seq:") {
        return decode_image_sequence_dir(Path::new(dir_str), 30, 1);
    }

    let path = Path::new(src);
    if path.is_dir() {
        return decode_image_sequence_dir(path, 30, 1);
    }

    let bytes = std::fs::read(path).map_err(|e| format!("read video {src:?}: {e}"))?;
    decode_video_bytes(&bytes, Some(path))
}

/// Decode a video clip from raw bytes (with optional file path for container fallback).
pub fn decode_video_bytes(
    bytes: &[u8],
    source_path: Option<&Path>,
) -> Result<DecodedVideoClip, String> {
    if bytes.starts_with(b"YUV4MPEG2") {
        return decode_y4m_clip(bytes);
    }
    if bytes.starts_with(b"KVID\x01\x00\x00\x00") {
        return decode_kvid_clip(bytes);
    }
    if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        return decode_gif_clip(bytes);
    }
    // Single still frame fallback (PNG, QOI, BMP, PAM, PPM, TGA)
    if let Ok((w, h, rgba)) = decode_any_image(bytes) {
        return Ok(DecodedVideoClip {
            width: w,
            height: h,
            fps_num: 30,
            fps_den: 1,
            frames_rgba: vec![rgba],
            cumulative_times: None,
        });
    }
    // External container demux/decode via ffmpeg -> YUV4MPEG2 pipe when available on host
    if let Some(p) = source_path {
        if let Ok(out) = std::process::Command::new("ffmpeg")
            .args(["-v", "error", "-i"])
            .arg(p)
            .args(["-f", "yuv4mpegpipe", "-pix_fmt", "yuv420p", "-"])
            .output()
        {
            if out.status.success() && out.stdout.starts_with(b"YUV4MPEG2") {
                return decode_y4m_clip(&out.stdout);
            }
        }
    }
    Err(
        "unsupported video input format (native decoders support .y4m, .kvid, .gif, image sequence directories, and video:* URIs)"
            .into(),
    )
}

/// Decode an image sequence directory (`0000.png`, `0001.qoi`, etc.) sorted lexicographically.
pub fn decode_image_sequence_dir(
    dir: &Path,
    fps_num: u32,
    fps_den: u32,
) -> Result<DecodedVideoClip, String> {
    let entries = std::fs::read_dir(dir)
        .map_err(|e| format!("read sequence directory {}: {e}", dir.display()))?;
    let mut paths: Vec<std::path::PathBuf> = entries
        .filter_map(|e| e.ok().map(|d| d.path()))
        .filter(|p| {
            p.is_file()
                && p.extension()
                    .and_then(|e| e.to_str())
                    .is_some_and(|ext| {
                        matches!(
                            ext.to_ascii_lowercase().as_str(),
                            "png" | "qoi" | "bmp" | "ppm" | "pam" | "tga"
                        )
                    })
        })
        .collect();
    paths.sort();

    if paths.is_empty() {
        return Err(format!(
            "image sequence directory {} contains no raster frames (.png/.qoi/.bmp/.ppm/.pam/.tga)",
            dir.display()
        ));
    }

    let mut width = 0u32;
    let mut height = 0u32;
    let mut frames_rgba = Vec::with_capacity(paths.len());

    for p in &paths {
        let bytes = std::fs::read(p).map_err(|e| format!("read frame {}: {e}", p.display()))?;
        let (w, h, rgba) = decode_any_image(&bytes)
            .map_err(|e| format!("decode frame {}: {e}", p.display()))?;
        if width == 0 {
            width = w;
            height = h;
        } else if w != width || h != height {
            return Err(format!(
                "frame {} has dimensions {w}x{h}, expected {width}x{height}",
                p.display()
            ));
        }
        frames_rgba.push(rgba);
    }

    Ok(DecodedVideoClip {
        width,
        height,
        fps_num: fps_num.max(1),
        fps_den: fps_den.max(1),
        frames_rgba,
        cumulative_times: None,
    })
}

/// Write a sequence of rendered frames into a directory (`frame_0000.<ext>`, ...).
pub fn write_image_sequence_dir(
    dir: &Path,
    frames: &[vello_cpu::Pixmap],
    format: crate::formats::FrameFormat,
) -> Result<Vec<std::path::PathBuf>, String> {
    std::fs::create_dir_all(dir)
        .map_err(|e| format!("create sequence dir {}: {e}", dir.display()))?;
    let ext = match format {
        crate::formats::FrameFormat::Png => "png",
        crate::formats::FrameFormat::Qoi => "qoi",
        crate::formats::FrameFormat::Bmp => "bmp",
        crate::formats::FrameFormat::Ppm => "ppm",
        crate::formats::FrameFormat::Pam => "pam",
        crate::formats::FrameFormat::Tga => "tga",
        crate::formats::FrameFormat::Svg => "svg",
    };
    let mut written = Vec::with_capacity(frames.len());
    for (i, pixmap) in frames.iter().enumerate() {
        let bytes = crate::formats::encode_pixmap(pixmap, format)?;
        let path = dir.join(format!("frame_{i:04}.{ext}"));
        std::fs::write(&path, &bytes)
            .map_err(|e| format!("write frame {}: {e}", path.display()))?;
        written.push(path);
    }
    Ok(written)
}

// ── Procedural Video Generators (`video:smpte`, `video:plasma`, `video:grid`) ─

pub fn synthesize_procedural_video(uri: &str) -> Result<DecodedVideoClip, String> {
    // Syntax: `video:<kind>[:WxH[:fps[:seconds]]]`
    let rest = uri.strip_prefix("video:").unwrap_or(uri);
    let parts: Vec<&str> = rest.split(':').collect();
    let kind = parts.first().copied().unwrap_or("smpte");
    let (width, height) = if let Some(dim) = parts.get(1) {
        if let Some((w_s, h_s)) = dim.split_once('x') {
            (
                w_s.parse::<u32>().unwrap_or(160).clamp(8, 1920),
                h_s.parse::<u32>().unwrap_or(90).clamp(8, 1080),
            )
        } else {
            (160, 90)
        }
    } else {
        (160, 90)
    };
    let fps_num = parts
        .get(2)
        .and_then(|s| s.parse::<u32>().ok())
        .unwrap_or(30)
        .clamp(1, 120);
    let seconds = parts
        .get(3)
        .and_then(|s| s.parse::<f64>().ok())
        .unwrap_or(2.0)
        .clamp(0.1, 30.0);
    let total_frames = ((seconds * f64::from(fps_num)).round() as usize).max(1);

    let mut frames_rgba = Vec::with_capacity(total_frames);
    let w = width as usize;
    let h = height as usize;

    const BARS: [[u8; 3]; 7] = [
        [192, 192, 192], // 75% White
        [192, 192, 0],   // Yellow
        [0, 192, 192],   // Cyan
        [0, 192, 0],     // Green
        [192, 0, 192],   // Magenta
        [192, 0, 0],     // Red
        [0, 0, 192],     // Blue
    ];

    for f in 0..total_frames {
        let t = (f as f32) / (fps_num as f32);
        let mut buf = vec![0u8; w * h * 4];
        match kind {
            "plasma" => {
                for y in 0..h {
                    let ny = (y as f32) / (h as f32);
                    for x in 0..w {
                        let nx = (x as f32) / (w as f32);
                        let v1 = (nx * 6.0 + t * 4.0).sin();
                        let v2 = (ny * 6.0 - t * 3.0).cos();
                        let v3 = ((nx + ny) * 5.0 + t * 2.5).sin();
                        let r = ((v1 * 0.5 + 0.5) * 255.0).round() as u8;
                        let g = ((v2 * 0.5 + 0.5) * 255.0).round() as u8;
                        let b = ((v3 * 0.5 + 0.5) * 255.0).round() as u8;
                        let o = (y * w + x) * 4;
                        buf[o] = r;
                        buf[o + 1] = g;
                        buf[o + 2] = b;
                        buf[o + 3] = 255;
                    }
                }
            }
            "grid" => {
                let shift_x = ((t * 48.0).round() as usize) % 16;
                let shift_y = ((t * 24.0).round() as usize) % 16;
                for y in 0..h {
                    for x in 0..w {
                        let gx = (x + shift_x) % 16 == 0;
                        let gy = (y + shift_y) % 16 == 0;
                        let c = if gx || gy {
                            [79, 209, 197, 255]
                        } else {
                            [16, 24, 40, 255]
                        };
                        let o = (y * w + x) * 4;
                        buf[o..o + 4].copy_from_slice(&c);
                    }
                }
            }
            _ => {
                // SMPTE color bars + moving scan marker in bottom quarter
                let split_y = (h * 3) / 4;
                let marker_x = ((f * 7) % w.max(1)) as i32;
                for y in 0..h {
                    for x in 0..w {
                        let o = (y * w + x) * 4;
                        if y < split_y {
                            let bar_idx = (x * 7 / w.max(1)).min(6);
                            let c = BARS[bar_idx];
                            buf[o] = c[0];
                            buf[o + 1] = c[1];
                            buf[o + 2] = c[2];
                            buf[o + 3] = 255;
                        } else {
                            let dist = (x as i32 - marker_x).abs();
                            let v = if dist <= 4 {
                                240u8
                            } else {
                                ((x * 255) / w.max(1)) as u8
                            };
                            buf[o] = v;
                            buf[o + 1] = v;
                            buf[o + 2] = v;
                            buf[o + 3] = 255;
                        }
                    }
                }
            }
        }
        frames_rgba.push(buf);
    }

    Ok(DecodedVideoClip {
        width,
        height,
        fps_num,
        fps_den: 1,
        frames_rgba,
        cumulative_times: None,
    })
}

// ── YUV4MPEG2 (`.y4m`) Decoder ──────────────────────────────────────────────

#[inline]
fn yuv_bt709_to_rgba(y_byte: u8, cb_byte: u8, cr_byte: u8) -> [u8; 4] {
    let y = (f32::from(y_byte.clamp(16, 235)) - 16.0) * (1.0 / 219.0);
    let cb = (f32::from(cb_byte) - 128.0) * (1.0 / 224.0);
    let cr = (f32::from(cr_byte) - 128.0) * (1.0 / 224.0);
    let r = (y + 1.5748 * cr).clamp(0.0, 1.0);
    let g = (y - 0.1873243 * cb - 0.4681243 * cr).clamp(0.0, 1.0);
    let b = (y + 1.8556 * cb).clamp(0.0, 1.0);
    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        255,
    ]
}

pub fn decode_y4m_clip(bytes: &[u8]) -> Result<DecodedVideoClip, String> {
    let Some(header_end) = bytes.iter().position(|&b| b == b'\n') else {
        return Err("missing YUV4MPEG2 header newline".into());
    };
    let header_str = std::str::from_utf8(&bytes[..header_end])
        .map_err(|_| "invalid UTF-8 in YUV4MPEG2 header".to_string())?;
    if !header_str.starts_with("YUV4MPEG2") {
        return Err("stream does not start with YUV4MPEG2".into());
    }

    let mut width = 0u32;
    let mut height = 0u32;
    let mut fps_num = 30u32;
    let mut fps_den = 1u32;
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
        return Err("invalid Y4M dimensions".into());
    }

    let w = width as usize;
    let h = height as usize;
    let y_len = w * h;
    let (cw, ch, planes) = if colorspace.starts_with("mono") {
        (0usize, 0usize, 0usize)
    } else if colorspace.starts_with("444") {
        (w, h, 2usize)
    } else if colorspace.starts_with("422") {
        (width.div_ceil(2) as usize, h, 2usize)
    } else {
        (width.div_ceil(2) as usize, height.div_ceil(2) as usize, 2usize)
    };
    let c_len = cw * ch;
    let payload_len = y_len + c_len * planes;

    let mut pos = header_end + 1;
    let mut frames_rgba = Vec::new();
    while pos < bytes.len() {
        let rem = &bytes[pos..];
        if !rem.starts_with(b"FRAME") {
            break;
        }
        let Some(nl) = rem.iter().position(|&b| b == b'\n') else {
            break;
        };
        pos += nl + 1;
        if pos + payload_len > bytes.len() {
            break;
        }
        let y_plane = &bytes[pos..pos + y_len];
        let mut rgba = vec![0u8; w * h * 4];
        if planes == 0 {
            for (i, &yb) in y_plane.iter().enumerate() {
                let v = ((f32::from(yb.clamp(16, 235)) - 16.0) * (255.0 / 219.0)).round() as u8;
                rgba[i * 4..i * 4 + 4].copy_from_slice(&[v, v, v, 255]);
            }
        } else {
            let u_plane = &bytes[pos + y_len..pos + y_len + c_len];
            let v_plane = &bytes[pos + y_len + c_len..pos + y_len + c_len * 2];
            let x_shift = if colorspace.starts_with("444") { 0 } else { 1 };
            let y_shift = if colorspace.starts_with("444") || colorspace.starts_with("422") {
                0
            } else {
                1
            };
            for row in 0..h {
                let cy = (row >> y_shift).min(ch.saturating_sub(1));
                for col in 0..w {
                    let cx = (col >> x_shift).min(cw.saturating_sub(1));
                    let yv = y_plane[row * w + col];
                    let uv = u_plane[cy * cw + cx];
                    let vv = v_plane[cy * cw + cx];
                    let px = yuv_bt709_to_rgba(yv, uv, vv);
                    let dst = (row * w + col) * 4;
                    rgba[dst..dst + 4].copy_from_slice(&px);
                }
            }
        }
        frames_rgba.push(rgba);
        pos += payload_len;
    }

    if frames_rgba.is_empty() {
        return Err("Y4M stream contains no frames".into());
    }

    Ok(DecodedVideoClip {
        width,
        height,
        fps_num,
        fps_den,
        frames_rgba,
        cumulative_times: None,
    })
}

// ── `.kvid` Decoder ─────────────────────────────────────────────────────────

pub fn decode_kvid_clip(bytes: &[u8]) -> Result<DecodedVideoClip, String> {
    if bytes.len() < 32 || &bytes[0..8] != b"KVID\x01\x00\x00\x00" {
        return Err("invalid KVID header".into());
    }
    let width = u32::from_le_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    let height = u32::from_le_bytes([bytes[12], bytes[13], bytes[14], bytes[15]]);
    let fps_num = u32::from_le_bytes([bytes[16], bytes[17], bytes[18], bytes[19]]).max(1);
    let fps_den = u32::from_le_bytes([bytes[20], bytes[21], bytes[22], bytes[23]]).max(1);
    let frame_count = u32::from_le_bytes([bytes[24], bytes[25], bytes[26], bytes[27]]) as usize;
    let index_end = 32 + frame_count * 12;
    if width == 0 || height == 0 || bytes.len() < index_end {
        return Err("truncated KVID container".into());
    }

    let total_px = (width as usize) * (height as usize);
    let mut frames_rgba = Vec::with_capacity(frame_count);

    for i in 0..frame_count {
        let b = 32 + i * 12;
        let offset = u64::from_le_bytes([
            bytes[b],
            bytes[b + 1],
            bytes[b + 2],
            bytes[b + 3],
            bytes[b + 4],
            bytes[b + 5],
            bytes[b + 6],
            bytes[b + 7],
        ]) as usize;
        let len = u32::from_le_bytes([bytes[b + 8], bytes[b + 9], bytes[b + 10], bytes[b + 11]])
            as usize;
        if offset + len > bytes.len() {
            return Err("truncated KVID frame payload".into());
        }
        // Reconstruct a QOI stream around the raw QOI op chunk so decode_qoi handles it
        let mut qoi_buf = Vec::with_capacity(14 + len + 8);
        qoi_buf.extend_from_slice(b"qoif");
        qoi_buf.extend_from_slice(&width.to_be_bytes());
        qoi_buf.extend_from_slice(&height.to_be_bytes());
        qoi_buf.push(4);
        qoi_buf.push(0);
        qoi_buf.extend_from_slice(&bytes[offset..offset + len]);
        qoi_buf.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
        let (_, _, rgba) = crate::formats::decode_qoi(&qoi_buf)?;
        if rgba.len() == total_px * 4 {
            frames_rgba.push(rgba);
        }
    }

    if frames_rgba.is_empty() {
        return Err("KVID container has no frames".into());
    }

    Ok(DecodedVideoClip {
        width,
        height,
        fps_num,
        fps_den,
        frames_rgba,
        cumulative_times: None,
    })
}

// ── Animated GIF87a / GIF89a Decoder (Variable-Width LZW) ───────────────────

pub fn decode_gif_clip(bytes: &[u8]) -> Result<DecodedVideoClip, String> {
    if bytes.len() < 13 || (&bytes[0..6] != b"GIF89a" && &bytes[0..6] != b"GIF87a") {
        return Err("invalid GIF header".into());
    }
    let width = u16::from_le_bytes([bytes[6], bytes[7]]) as u32;
    let height = u16::from_le_bytes([bytes[8], bytes[9]]) as u32;
    if width == 0 || height == 0 {
        return Err("zero-sized GIF".into());
    }
    let packed = bytes[10];
    let has_gct = (packed & 0x80) != 0;
    let gct_size = 1usize << ((packed & 0x07) + 1);
    let mut pos = 13usize;

    let mut global_palette = [[0u8; 3]; 256];
    if has_gct {
        if pos + gct_size * 3 > bytes.len() {
            return Err("truncated GIF Global Color Table".into());
        }
        for i in 0..gct_size {
            global_palette[i] = [
                bytes[pos + i * 3],
                bytes[pos + i * 3 + 1],
                bytes[pos + i * 3 + 2],
            ];
        }
        pos += gct_size * 3;
    }

    let mut canvas = vec![0u8; (width as usize) * (height as usize) * 4];
    let mut frames_rgba = Vec::new();
    let mut cumulative_times = Vec::new();
    let mut cur_time = 0.0f64;

    let mut delay_cs = 5u16;
    let mut transparent_idx: Option<u8> = None;
    let mut disposal = 0u8;

    while pos < bytes.len() {
        let block_id = bytes[pos];
        pos += 1;
        match block_id {
            0x3B => break, // Trailer
            0x21 => {
                // Extension
                if pos >= bytes.len() {
                    break;
                }
                let ext_label = bytes[pos];
                pos += 1;
                if ext_label == 0xF9 && pos + 6 <= bytes.len() && bytes[pos] == 4 {
                    // Graphic Control Extension
                    let gce_packed = bytes[pos + 1];
                    disposal = (gce_packed >> 2) & 0x07;
                    let has_trans = (gce_packed & 0x01) != 0;
                    delay_cs = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]);
                    let trans_color = bytes[pos + 4];
                    transparent_idx = if has_trans { Some(trans_color) } else { None };
                    pos += 6;
                } else {
                    // Skip sub-blocks
                    while pos < bytes.len() {
                        let sub_len = bytes[pos] as usize;
                        pos += 1;
                        if sub_len == 0 {
                            break;
                        }
                        pos = (pos + sub_len).min(bytes.len());
                    }
                }
            }
            0x2C => {
                // Image Descriptor
                if pos + 9 > bytes.len() {
                    break;
                }
                let img_left = u16::from_le_bytes([bytes[pos], bytes[pos + 1]]) as usize;
                let img_top = u16::from_le_bytes([bytes[pos + 2], bytes[pos + 3]]) as usize;
                let img_w = u16::from_le_bytes([bytes[pos + 4], bytes[pos + 5]]) as usize;
                let img_h = u16::from_le_bytes([bytes[pos + 6], bytes[pos + 7]]) as usize;
                let img_packed = bytes[pos + 8];
                pos += 9;

                let has_lct = (img_packed & 0x80) != 0;
                let lct_size = 1usize << ((img_packed & 0x07) + 1);
                let mut palette = global_palette;
                if has_lct {
                    if pos + lct_size * 3 > bytes.len() {
                        break;
                    }
                    for i in 0..lct_size {
                        palette[i] = [
                            bytes[pos + i * 3],
                            bytes[pos + i * 3 + 1],
                            bytes[pos + i * 3 + 2],
                        ];
                    }
                    pos += lct_size * 3;
                }

                if pos >= bytes.len() {
                    break;
                }
                let min_code_size = bytes[pos];
                pos += 1;
                let mut compressed = Vec::new();
                while pos < bytes.len() {
                    let sub_len = bytes[pos] as usize;
                    pos += 1;
                    if sub_len == 0 {
                        break;
                    }
                    let end = (pos + sub_len).min(bytes.len());
                    compressed.extend_from_slice(&bytes[pos..end]);
                    pos = end;
                }

                let indices = lzw_decode_gif(&compressed, min_code_size, img_w * img_h);
                let prev_canvas = if disposal == 3 {
                    Some(canvas.clone())
                } else {
                    None
                };

                for py in 0..img_h {
                    let cy = img_top + py;
                    if cy >= height as usize {
                        continue;
                    }
                    for px in 0..img_w {
                        let cx = img_left + px;
                        if cx >= width as usize {
                            continue;
                        }
                        let idx = indices.get(py * img_w + px).copied().unwrap_or(0);
                        if Some(idx) == transparent_idx {
                            continue;
                        }
                        let rgb = palette[idx as usize];
                        let dst = (cy * (width as usize) + cx) * 4;
                        canvas[dst] = rgb[0];
                        canvas[dst + 1] = rgb[1];
                        canvas[dst + 2] = rgb[2];
                        canvas[dst + 3] = 255;
                    }
                }

                frames_rgba.push(canvas.clone());
                let dt = (f64::from(delay_cs.max(2)) / 100.0).max(0.01);
                cur_time += dt;
                cumulative_times.push(cur_time);

                if disposal == 2 {
                    for py in 0..img_h {
                        let cy = img_top + py;
                        if cy >= height as usize {
                            continue;
                        }
                        for px in 0..img_w {
                            let cx = img_left + px;
                            if cx >= width as usize {
                                continue;
                            }
                            let dst = (cy * (width as usize) + cx) * 4;
                            canvas[dst..dst + 4].copy_from_slice(&[0, 0, 0, 0]);
                        }
                    }
                } else if let Some(prev) = prev_canvas {
                    canvas = prev;
                }

                transparent_idx = None;
                disposal = 0;
            }
            _ => break,
        }
    }

    if frames_rgba.is_empty() {
        return Err("GIF contains no decodable frames".into());
    }

    let avg_fps = if cur_time > 0.0 {
        ((frames_rgba.len() as f64) / cur_time).round().clamp(1.0, 120.0) as u32
    } else {
        20
    };

    Ok(DecodedVideoClip {
        width,
        height,
        fps_num: avg_fps,
        fps_den: 1,
        frames_rgba,
        cumulative_times: Some(cumulative_times),
    })
}

fn lzw_decode_gif(data: &[u8], min_code_size: u8, expected_len: usize) -> Vec<u8> {
    let min_bits = min_code_size.clamp(2, 8) as u32;
    let clear_code = 1u16 << min_bits;
    let eoi_code = clear_code + 1;

    let mut code_size = min_bits + 1;
    let mut next_code = eoi_code + 1;
    let mut prefix = [0u16; 4096];
    let mut suffix = [0u8; 4096];
    let mut length = [0u16; 4096];
    for i in 0..clear_code {
        suffix[i as usize] = i as u8;
        length[i as usize] = 1;
    }

    let mut out = Vec::with_capacity(expected_len);
    let mut bit_pos = 0usize;
    let total_bits = data.len() * 8;
    let mut prev_code: Option<u16> = None;
    let mut stack = [0u8; 4096];

    while bit_pos + (code_size as usize) <= total_bits && out.len() < expected_len {
        let mut code = 0u32;
        for b in 0..(code_size as usize) {
            let bit_idx = bit_pos + b;
            let byte = data[bit_idx >> 3];
            let bit = (byte >> (bit_idx & 7)) & 1;
            code |= u32::from(bit) << b;
        }
        bit_pos += code_size as usize;
        let code = code as u16;

        if code == clear_code {
            code_size = min_bits + 1;
            next_code = eoi_code + 1;
            prev_code = None;
            continue;
        }
        if code == eoi_code {
            break;
        }

        let mut cur = if code < next_code {
            code
        } else if let Some(prev) = prev_code {
            prev
        } else {
            continue;
        };

        let mut sp = 0usize;
        while cur >= clear_code && sp < 4095 {
            stack[sp] = suffix[cur as usize];
            sp += 1;
            cur = prefix[cur as usize];
        }
        let first_char = (cur as u8).min(255);
        stack[sp] = first_char;
        sp += 1;

        if let Some(prev) = prev_code {
            if code >= next_code && sp > 0 {
                // KwKwK special case
                for i in (0..sp).rev() {
                    out.push(stack[i]);
                }
                out.push(first_char);
            } else {
                for i in (0..sp).rev() {
                    out.push(stack[i]);
                }
            }
            if (next_code as usize) < 4096 {
                prefix[next_code as usize] = prev;
                suffix[next_code as usize] = first_char;
                length[next_code as usize] = length[prev as usize].saturating_add(1);
                next_code += 1;
                if next_code == (1u16 << code_size) && code_size < 12 {
                    code_size += 1;
                }
            }
        } else {
            out.push(first_char);
        }
        prev_code = Some(code);
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello_cpu::color::PremulRgba8;
    use vello_cpu::Pixmap;

    #[test]
    fn gif_encoder_and_decoder_round_trip_frames() {
        let mut p1 = Pixmap::new(8, 8);
        let mut p2 = Pixmap::new(8, 8);
        for px in p1.data_mut() {
            *px = PremulRgba8 {
                r: 255,
                g: 0,
                b: 0,
                a: 255,
            };
        }
        for px in p2.data_mut() {
            *px = PremulRgba8 {
                r: 0,
                g: 255,
                b: 0,
                a: 255,
            };
        }
        let gif_bytes = crate::formats::encode_animated_gif(&[p1, p2], 10, 1).unwrap();
        let clip = decode_gif_clip(&gif_bytes).unwrap();
        assert_eq!(clip.width, 8);
        assert_eq!(clip.height, 8);
        assert_eq!(clip.frames_rgba.len(), 2);

        let f0 = clip.sample_frame(0.01, 0.0, 0.0, None, 1.0, false).unwrap();
        assert!(f0[0] > 200 && f0[1] < 30, "frame 0 should be red: {:?}", &f0[0..4]);
        let f1 = clip.sample_frame(0.15, 0.0, 0.0, None, 1.0, false).unwrap();
        assert!(f1[1] > 200 && f1[0] < 30, "frame 1 should be green: {:?}", &f1[0..4]);
    }

    #[test]
    fn procedural_video_uris_generate_animated_frames() {
        for uri in ["video:smpte:32x24:15:0.4", "video:plasma:32x24:15:0.4", "video:grid:32x24:15:0.4"] {
            let clip = decode_video_source(uri).unwrap();
            assert_eq!(clip.width, 32);
            assert_eq!(clip.height, 24);
            assert_eq!(clip.frames_rgba.len(), 6);
            assert_ne!(clip.frames_rgba[0], clip.frames_rgba[1]);
        }
    }
}
