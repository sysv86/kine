//! AV1 and H.264 encoding through an external FFmpeg build.
//!
//! Kine does not ship Rust codec implementations.  Both video codecs are
//! encoded by a recent FFmpeg binary:
//!
//! * AV1: `libsvtav1` (preferred) or `libaom-av1` — both permissive;
//! * H.264: `libopenh264` (preferred) or `libx264`.
//!
//! FFmpeg is an external process, not a linked dependency.  The binary is
//! resolved from `KINE_FFMPEG`, `.tools/ffmpeg/bin/ffmpeg` (the path used by
//! `scripts/install-ffmpeg.sh`), or `PATH`.
//!
//! Output is an elementary stream: low-overhead OBU for AV1 and Annex-B for
//! H.264. `kine-mux` turns those into ISO-BMFF/Matroska samples.

use crate::color::rgba_to_i420;
use crate::settings::EncoderSettings;
use crate::{AccessUnit, EncodeError};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::thread::JoinHandle;

/// Video codec encoded by FFmpeg.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FfmpegCodec {
    Av1,
    H264,
}

impl FfmpegCodec {
    pub fn label(self) -> &'static str {
        match self {
            Self::Av1 => "AV1",
            Self::H264 => "H.264",
        }
    }
}

#[derive(Debug, Clone)]
struct FfmpegInfo {
    path: PathBuf,
    version: String,
    av1: Option<&'static str>,
    h264: Option<&'static str>,
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os("KINE_FFMPEG") {
        out.push(PathBuf::from(path));
    }
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join(".tools/ffmpeg/bin").join(exe));
    }
    if let Ok(current) = std::env::current_exe() {
        for ancestor in current.ancestors().skip(1).take(6) {
            out.push(ancestor.join(".tools/ffmpeg/bin").join(exe));
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for dir in std::env::split_paths(&path) {
            out.push(dir.join(exe));
        }
    }
    out
}

fn probe() -> Option<FfmpegInfo> {
    for path in candidate_paths() {
        if !path.is_file() {
            continue;
        }
        let Ok(output) = Command::new(&path)
            .args(["-hide_banner", "-encoders"])
            .output()
        else {
            continue;
        };
        let text = String::from_utf8_lossy(&output.stdout);
        let has = |name: &str| text.lines().any(|line| line.contains(name));
        let av1 = ["libsvtav1", "libaom-av1"]
            .into_iter()
            .find(|name| has(name));
        let h264 = ["libopenh264", "libx264"]
            .into_iter()
            .find(|name| has(name));
        if av1.is_none() && h264.is_none() {
            continue;
        }
        let version = Command::new(&path)
            .args(["-hide_banner", "-version"])
            .output()
            .ok()
            .and_then(|out| {
                String::from_utf8_lossy(&out.stdout)
                    .lines()
                    .next()
                    .map(str::to_owned)
            })
            .unwrap_or_else(|| "ffmpeg (version unknown)".to_string());
        return Some(FfmpegInfo {
            path,
            version,
            av1,
            h264,
        });
    }
    None
}

fn ffmpeg_info() -> Option<&'static FfmpegInfo> {
    static INFO: OnceLock<Option<FfmpegInfo>> = OnceLock::new();
    INFO.get_or_init(probe).as_ref()
}

/// Whether an FFmpeg binary with an encoder for `codec` is available.
pub fn ffmpeg_available(codec: FfmpegCodec) -> bool {
    ffmpeg_info()
        .map(|info| match codec {
            FfmpegCodec::Av1 => info.av1.is_some(),
            FfmpegCodec::H264 => info.h264.is_some(),
        })
        .unwrap_or(false)
}

/// Encoder name FFmpeg will use for `codec`, if available.
pub fn ffmpeg_encoder_name(codec: FfmpegCodec) -> Option<&'static str> {
    let info = ffmpeg_info()?;
    match codec {
        FfmpegCodec::Av1 => info.av1,
        FfmpegCodec::H264 => info.h264,
    }
}

/// FFmpeg version string, if available.
pub fn ffmpeg_version() -> Option<&'static str> {
    ffmpeg_info().map(|info| info.version.as_str())
}

/// Path of the FFmpeg binary, if available.
pub fn ffmpeg_path() -> Option<&'static std::path::Path> {
    ffmpeg_info().map(|info| info.path.as_path())
}

/// Streaming encoder for AV1 or H.264 backed by FFmpeg.
pub struct FfmpegEncoder {
    settings: EncoderSettings,
    codec: FfmpegCodec,
    child: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    stdout: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    stderr: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    frames_sent: u64,
}

impl FfmpegEncoder {
    pub fn new(settings: EncoderSettings, codec: FfmpegCodec) -> Result<Self, EncodeError> {
        if settings.width == 0 || settings.height == 0 {
            return Err(EncodeError::InvalidConfig(
                "width and height must be non-zero".to_string(),
            ));
        }
        if !settings.width.is_multiple_of(2) || !settings.height.is_multiple_of(2) {
            return Err(EncodeError::InvalidConfig(format!(
                "{codec:?} 4:2:0 needs even dimensions, got {}x{}",
                settings.width, settings.height
            )));
        }
        if settings.fps_num == 0 || settings.fps_den == 0 {
            return Err(EncodeError::InvalidConfig(
                "fps must be non-zero".to_string(),
            ));
        }
        let info = ffmpeg_info().ok_or_else(|| {
            EncodeError::Rejected(
                "FFmpeg with an AV1/H.264 encoder was not found; run \
                 scripts/install-ffmpeg.sh or set KINE_FFMPEG"
                    .to_string(),
            )
        })?;
        let encoder = match codec {
            FfmpegCodec::Av1 => info.av1.ok_or_else(|| {
                EncodeError::Rejected(
                    "this FFmpeg build has no AV1 encoder (libsvtav1 or libaom-av1)".to_string(),
                )
            })?,
            FfmpegCodec::H264 => info.h264.ok_or_else(|| {
                EncodeError::Rejected(
                    "this FFmpeg build has no H.264 encoder (libopenh264/libx264)".to_string(),
                )
            })?,
        };

        let gop = settings.keyframe_interval.max(1);
        let crf = 1 + (settings.quantizer.min(255) * 51 / 255);
        let av1_crf = 1 + (settings.quantizer.min(255) * 62 / 255);
        let size = format!("{}x{}", settings.width, settings.height);
        let rate = format!("{}/{}", settings.fps_num, settings.fps_den);

        let mut args: Vec<String> = vec![
            "-hide_banner".into(),
            "-loglevel".into(),
            "error".into(),
            "-f".into(),
            "rawvideo".into(),
            "-pixel_format".into(),
            "yuv420p".into(),
            "-video_size".into(),
            size,
            "-framerate".into(),
            rate,
            "-i".into(),
            "pipe:0".into(),
            "-an".into(),
            "-c:v".into(),
            encoder.into(),
        ];

        match codec {
            FfmpegCodec::Av1 => {
                match encoder {
                    "libsvtav1" => {
                        // Kine speed 0..=10 maps to SVT-AV1 preset 3..=13
                        // (13 is fastest); the slowest SVT presets are rarely
                        // useful for a deterministic render loop.
                        let preset = (3 + u32::from(settings.speed)).min(13);
                        args.extend(["-preset".into(), preset.to_string()]);
                        if settings.bitrate > 0 {
                            args.extend(["-b:v".into(), settings.bitrate.to_string()]);
                        } else {
                            args.extend(["-crf".into(), av1_crf.to_string()]);
                        }
                        args.extend(["-g".into(), gop.to_string()]);
                    }
                    "libaom-av1" => {
                        let cpu = u32::from(settings.speed).min(8);
                        if settings.bitrate > 0 {
                            args.extend([
                                "-b:v".into(),
                                settings.bitrate.to_string(),
                                "-cpu-used".into(),
                                cpu.to_string(),
                            ]);
                        } else {
                            args.extend([
                                "-crf".into(),
                                av1_crf.to_string(),
                                "-b:v".into(),
                                "0".into(),
                                "-cpu-used".into(),
                                cpu.to_string(),
                            ]);
                        }
                        args.extend(["-g".into(), gop.to_string()]);
                    }
                    other => {
                        return Err(EncodeError::Rejected(format!(
                            "unsupported AV1 encoder {other}"
                        )));
                    }
                }
                args.extend(["-f".into(), "obu".into(), "pipe:1".into()]);
            }
            FfmpegCodec::H264 => {
                match encoder {
                    "libopenh264" => {
                        if settings.bitrate > 0 {
                            args.extend([
                                "-rc_mode".into(),
                                "bitrate".into(),
                                "-b:v".into(),
                                settings.bitrate.to_string(),
                            ]);
                        } else {
                            // OpenH264 has no CRF switch; derive a target
                            // bitrate from quality and resolution.
                            let pixels_per_second = u64::from(settings.width)
                                * u64::from(settings.height)
                                * u64::from(settings.fps_num)
                                / u64::from(settings.fps_den).max(1);
                            let q = settings.quantizer.min(255) as u64;
                            let bpp = 0.02 + 0.12 * (1.0 - q as f64 / 255.0);
                            let bitrate = ((pixels_per_second as f64 * bpp) as u64)
                                .clamp(100_000, 100_000_000);
                            args.extend([
                                "-rc_mode".into(),
                                "bitrate".into(),
                                "-b:v".into(),
                                bitrate.to_string(),
                            ]);
                        }
                        args.extend(["-g".into(), gop.to_string()]);
                    }
                    "libx264" => {
                        let preset = match settings.speed {
                            0..=1 => "veryslow",
                            2..=3 => "slower",
                            4 => "slow",
                            5..=6 => "medium",
                            7..=8 => "fast",
                            9 => "faster",
                            _ => "ultrafast",
                        };
                        args.extend([
                            "-crf".into(),
                            crf.to_string(),
                            "-preset".into(),
                            preset.into(),
                            "-g".into(),
                            gop.to_string(),
                        ]);
                    }
                    other => {
                        return Err(EncodeError::Rejected(format!(
                            "unsupported H.264 encoder {other}"
                        )));
                    }
                }
                args.extend(["-f".into(), "h264".into(), "pipe:1".into()]);
            }
        }

        let mut child = Command::new(&info.path)
            .args(&args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| EncodeError::Rejected(format!("failed to start ffmpeg: {e}")))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| EncodeError::Rejected("ffmpeg stdin unavailable".to_string()))?;
        let mut stdout = child
            .stdout
            .take()
            .ok_or_else(|| EncodeError::Rejected("ffmpeg stdout unavailable".to_string()))?;
        let mut stderr = child
            .stderr
            .take()
            .ok_or_else(|| EncodeError::Rejected("ffmpeg stderr unavailable".to_string()))?;
        let stdout_reader = std::thread::spawn(move || {
            let mut data = Vec::new();
            stdout.read_to_end(&mut data).map(|_| data)
        });
        let stderr_reader = std::thread::spawn(move || {
            let mut data = Vec::new();
            stderr.read_to_end(&mut data).map(|_| data)
        });

        Ok(Self {
            settings,
            codec,
            child: Some(child),
            stdin: Some(BufWriter::new(stdin)),
            stdout: Some(stdout_reader),
            stderr: Some(stderr_reader),
            frames_sent: 0,
        })
    }

    pub fn settings(&self) -> &EncoderSettings {
        &self.settings
    }

    pub fn frames_sent(&self) -> u64 {
        self.frames_sent
    }

    /// Send one RGBA8 frame (`width * height * 4` bytes) to FFmpeg.
    pub fn encode_rgba(&mut self, rgba: &[u8]) -> Result<(), EncodeError> {
        let expected = (self.settings.width as usize) * (self.settings.height as usize) * 4;
        if rgba.len() != expected {
            return Err(EncodeError::FrameSize {
                actual: rgba.len(),
                expected,
                width: self.settings.width,
                height: self.settings.height,
            });
        }
        let frame = rgba_to_i420(rgba, self.settings.width, self.settings.height);
        let stdin = self
            .stdin
            .as_mut()
            .ok_or_else(|| EncodeError::Rejected("ffmpeg stdin is closed".to_string()))?;
        stdin
            .write_all(&frame.y)
            .and_then(|_| stdin.write_all(&frame.u))
            .and_then(|_| stdin.write_all(&frame.v))
            .and_then(|_| stdin.flush())
            .map_err(|e| EncodeError::Rejected(format!("writing to ffmpeg: {e}")))?;
        self.frames_sent += 1;
        Ok(())
    }

    /// Close the input, wait for FFmpeg, and split the output into access units.
    pub fn finish(mut self) -> Result<Vec<AccessUnit>, EncodeError> {
        drop(self.stdin.take());
        let status = self
            .child
            .take()
            .ok_or_else(|| EncodeError::Rejected("ffmpeg process is gone".to_string()))?
            .wait()
            .map_err(|e| EncodeError::Rejected(format!("waiting for ffmpeg: {e}")))?;
        let stdout = self
            .stdout
            .take()
            .ok_or_else(|| EncodeError::Rejected("ffmpeg stdout reader is gone".to_string()))?
            .join()
            .map_err(|_| EncodeError::Rejected("ffmpeg stdout reader panicked".to_string()))?
            .map_err(|e| EncodeError::Rejected(format!("reading ffmpeg stdout: {e}")))?;
        let stderr = self
            .stderr
            .take()
            .ok_or_else(|| EncodeError::Rejected("ffmpeg stderr reader is gone".to_string()))?
            .join()
            .map_err(|_| EncodeError::Rejected("ffmpeg stderr reader panicked".to_string()))?
            .map_err(|e| EncodeError::Rejected(format!("reading ffmpeg stderr: {e}")))?;

        if !status.success() {
            let message = String::from_utf8_lossy(&stderr);
            return Err(EncodeError::Rejected(format!(
                "ffmpeg exited with {status}: {}",
                message.trim()
            )));
        }
        if stdout.is_empty() {
            return Err(EncodeError::Rejected(
                "ffmpeg produced an empty video stream".to_string(),
            ));
        }
        match self.codec {
            FfmpegCodec::Av1 => split_av1_access_units(&stdout, self.settings.keyframe_interval),
            FfmpegCodec::H264 => split_h264_access_units(&stdout),
        }
    }
}

impl Drop for FfmpegEncoder {
    fn drop(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Split a low-overhead OBU stream into AV1 temporal units.
pub fn split_av1_access_units(
    data: &[u8],
    keyframe_interval: u32,
) -> Result<Vec<AccessUnit>, EncodeError> {
    let obus = parse_obus(data);
    if obus.is_empty() {
        return Err(EncodeError::NoOutput);
    }
    let mut units = Vec::new();
    let mut current = Vec::new();
    for (start, _, end, obu_type) in obus {
        if obu_type == 2 && !current.is_empty() {
            units.push(current);
            current = Vec::new();
        }
        current.extend_from_slice(&data[start..end]);
    }
    if !current.is_empty() {
        units.push(current);
    }

    // Prefer the AV1 frame header (`frame_type == KEY_FRAME`); fall back to the
    // intra period when a unit has no parseable frame OBU.
    let gop = keyframe_interval.max(1) as usize;
    Ok(units
        .into_iter()
        .enumerate()
        .map(|(index, data)| {
            let keyframe = av1_unit_is_keyframe(&data).unwrap_or(index == 0 || index % gop == 0);
            AccessUnit { data, keyframe }
        })
        .collect())
}

/// Split an H.264 Annex-B stream into picture access units.
pub fn split_h264_access_units(data: &[u8]) -> Result<Vec<AccessUnit>, EncodeError> {
    let nals = parse_annexb(data);
    if nals.is_empty() {
        return Err(EncodeError::NoOutput);
    }
    let mut units = Vec::new();
    let mut current = Vec::new();
    let mut current_has_vcl = false;
    for (start, header, end, nal_type) in nals {
        let is_vcl = matches!(nal_type, 1..=5);
        let first_mb = if is_vcl {
            h264_first_mb(&data[header..end])
        } else {
            0
        };
        if is_vcl && first_mb == 0 && current_has_vcl && !current.is_empty() {
            units.push(build_h264_unit(&current));
            current = Vec::new();
            current_has_vcl = false;
        }
        current.extend_from_slice(&data[start..end]);
        current_has_vcl |= is_vcl;
    }
    if !current.is_empty() {
        units.push(build_h264_unit(&current));
    }
    if units.is_empty() {
        return Err(EncodeError::NoOutput);
    }
    Ok(units)
}

fn build_h264_unit(data: &[u8]) -> AccessUnit {
    let keyframe = parse_annexb(data)
        .iter()
        .any(|(_, _, _, nal_type)| *nal_type == 5);
    AccessUnit {
        data: data.to_vec(),
        keyframe,
    }
}

/// Locate Annex-B NAL units: `(start_of_start_code, payload_start, end, nal_unit_type)`.
fn parse_annexb(data: &[u8]) -> Vec<(usize, usize, usize, u8)> {
    let mut starts = Vec::new();
    let mut i = 0usize;
    while i + 3 <= data.len() {
        if data[i] == 0 && data[i + 1] == 0 && data[i + 2] == 1 {
            starts.push((i, 3));
            i += 3;
        } else if i + 4 <= data.len()
            && data[i] == 0
            && data[i + 1] == 0
            && data[i + 2] == 0
            && data[i + 3] == 1
        {
            starts.push((i, 4));
            i += 4;
        } else {
            i += 1;
        }
    }
    let mut out = Vec::with_capacity(starts.len());
    for (idx, &(start, sc_len)) in starts.iter().enumerate() {
        let header = start + sc_len;
        let end = starts
            .get(idx + 1)
            .map(|&(next, _)| next)
            .unwrap_or(data.len());
        if header < end {
            out.push((start, header, end, data[header] & 0x1F));
        }
    }
    out
}

/// `first_mb_in_slice` (ue(v)) of an H.264 slice NAL, after EPB removal.
fn h264_first_mb(nal: &[u8]) -> u32 {
    if nal.is_empty() {
        return 0;
    }
    let mut rbsp = Vec::with_capacity(nal.len());
    let mut zeros = 0u32;
    for &byte in &nal[1..] {
        if zeros >= 2 && byte == 3 {
            zeros = 0;
            continue;
        }
        rbsp.push(byte);
        zeros = if byte == 0 { zeros + 1 } else { 0 };
    }
    let mut leading = 0u32;
    let mut pos = 0usize;
    let mut read_bit = || {
        let bit = if pos / 8 < rbsp.len() {
            (rbsp[pos / 8] >> (7 - pos % 8)) & 1
        } else {
            0
        };
        pos += 1;
        bit
    };
    while read_bit() == 0 {
        leading += 1;
        if leading > 31 {
            return 0;
        }
    }
    if leading == 0 {
        return 0;
    }
    let mut value = 1u32 << leading;
    for _ in 0..leading {
        value = (value << 1) | u32::from(read_bit() != 0);
    }
    // The loop above includes an extra shift for the leading one; recompute
    // cleanly.
    let mut pos2 = 0usize;
    let mut leading2 = 0u32;
    let mut next = || {
        let bit = if pos2 / 8 < rbsp.len() {
            (rbsp[pos2 / 8] >> (7 - pos2 % 8)) & 1
        } else {
            0
        };
        pos2 += 1;
        bit
    };
    while next() == 0 {
        leading2 += 1;
        if leading2 > 31 {
            return 0;
        }
    }
    if leading2 == 0 {
        return 0;
    }
    let mut value2 = 0u32;
    for _ in 0..leading2 {
        value2 = (value2 << 1) | u32::from(next() != 0);
    }
    ((1u32 << leading2) - 1) + value2
}

/// Read `show_existing_frame` / `frame_type` from the first frame OBU of a
/// temporal unit. Returns `None` when the unit has no frame header.
fn av1_unit_is_keyframe(data: &[u8]) -> Option<bool> {
    for (_, payload_start, end, obu_type) in parse_obus(data) {
        if obu_type != 3 && obu_type != 6 {
            continue;
        }
        let payload = data.get(payload_start..end)?;
        let first = *payload.first()?;
        // show_existing_frame(1) then frame_type(2).
        if first & 0x80 != 0 {
            return Some(false);
        }
        return Some((first >> 5) & 0x03 == 0);
    }
    None
}

/// Locate AV1 OBUs: `(start, payload_start, end, obu_type)`.
fn parse_obus(data: &[u8]) -> Vec<(usize, usize, usize, u8)> {
    let mut out = Vec::new();
    let mut i = 0usize;
    while i < data.len() {
        let start = i;
        let header = data[i];
        i += 1;
        let obu_type = (header >> 3) & 0x0F;
        let extension = (header >> 2) & 1;
        let has_size = (header >> 1) & 1;
        if extension == 1 {
            i += 1;
        }
        let mut size = None;
        if has_size == 1 {
            let mut value = 0usize;
            let mut shift = 0u32;
            loop {
                let Some(&byte) = data.get(i) else {
                    return out;
                };
                i += 1;
                value |= usize::from(byte & 0x7F) << shift;
                if byte & 0x80 == 0 {
                    break;
                }
                shift += 7;
                if shift > 28 {
                    return out;
                }
            }
            size = Some(value);
        }
        let payload_start = i;
        let end = match size {
            Some(size) => (i + size).min(data.len()),
            None => data.len(),
        };
        out.push((start, payload_start, end, obu_type));
        if size.is_none() {
            break;
        }
        i = end;
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(ty: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1, ty];
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn splits_h264_pictures() {
        let mut stream = Vec::new();
        stream.extend(nal(7, &[1]));
        stream.extend(nal(8, &[2]));
        stream.extend(nal(5, &[0x88])); // first_mb_in_slice = 0
        stream.extend(nal(1, &[0x88]));
        let units = split_h264_access_units(&stream).unwrap();
        assert_eq!(units.len(), 2);
        assert!(units[0].keyframe);
        assert!(!units[1].keyframe);
    }

    #[test]
    fn splits_av1_temporal_units() {
        // temporal delimiter (type 2), sequence header (type 1), frame (type 6)
        let make = |ty: u8, payload: &[u8]| {
            let mut out = vec![(ty << 3) | 0b010]; // has_size_field
            out.push(payload.len() as u8);
            out.extend_from_slice(payload);
            out
        };
        let mut stream = Vec::new();
        stream.extend(make(2, &[]));
        stream.extend(make(1, &[1]));
        stream.extend(make(6, &[2]));
        stream.extend(make(2, &[]));
        stream.extend(make(6, &[0x20]));
        let units = split_av1_access_units(&stream, 1).unwrap();
        assert_eq!(units.len(), 2);
        // Frame payload 2 starts with 0b000_00010: show_existing=0, frame_type=0.
        assert!(units[0].keyframe);
        // 0b001_00000: show_existing=0, frame_type=1 (INTER_FRAME).
        assert!(!units[1].keyframe);
    }
}
