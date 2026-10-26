//! H.266 / VVC encoding through an external FFmpeg build.
//!
//! Kine no longer ships a custom VVC encoder.  The H.266 path delegates to a
//! recent FFmpeg binary built with `libvvenc` (BSD-3-Clause), the Fraunhofer
//! VVC encoder.  FFmpeg is an external process, not a linked dependency.
//!
//! The binary is resolved from `KINE_FFMPEG`, then `.tools/ffmpeg/bin/ffmpeg`
//! under the current directory, then `ffmpeg` on `PATH`.  `scripts/install-ffmpeg.sh`
//! fetches the latest LGPL BtbN build, which includes `libvvenc`.

use crate::color::rgba_to_i420;
use crate::{AccessUnit, EncodeError, EncoderSettings};
use std::io::{BufWriter, Read, Write};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::OnceLock;
use std::thread::JoinHandle;

#[derive(Debug, Clone)]
struct FfmpegInfo {
    path: PathBuf,
    version: String,
}

fn candidate_paths() -> Vec<PathBuf> {
    let mut out = Vec::new();
    if let Some(path) = std::env::var_os("KINE_FFMPEG") {
        out.push(PathBuf::from(path));
    }
    if let Ok(cwd) = std::env::current_dir() {
        out.push(cwd.join(".tools/ffmpeg/bin/ffmpeg"));
        out.push(cwd.join(".tools/ffmpeg/bin/ffmpeg.exe"));
    }
    // Test binaries run with a crate-local working directory; walk up to the
    // workspace root so `.tools/ffmpeg` is still found.
    if let Ok(exe) = std::env::current_exe() {
        for ancestor in exe.ancestors().skip(1).take(6) {
            out.push(ancestor.join(".tools/ffmpeg/bin/ffmpeg"));
            out.push(ancestor.join(".tools/ffmpeg/bin/ffmpeg.exe"));
        }
    }
    let exe = if cfg!(windows) {
        "ffmpeg.exe"
    } else {
        "ffmpeg"
    };
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
        if !text.lines().any(|line| line.contains("libvvenc")) {
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
        return Some(FfmpegInfo { path, version });
    }
    None
}

fn ffmpeg_info() -> Option<&'static FfmpegInfo> {
    static INFO: OnceLock<Option<FfmpegInfo>> = OnceLock::new();
    INFO.get_or_init(probe).as_ref()
}

/// Whether an FFmpeg binary with the `libvvenc` encoder is available.
pub fn ffmpeg_vvc_available() -> bool {
    ffmpeg_info().is_some()
}

/// The FFmpeg version string used for H.266 encoding, if available.
pub fn ffmpeg_vvc_version() -> Option<&'static str> {
    ffmpeg_info().map(|info| info.version.as_str())
}

/// Path of the FFmpeg binary used for H.266 encoding, if available.
pub fn ffmpeg_vvc_path() -> Option<&'static std::path::Path> {
    ffmpeg_info().map(|info| info.path.as_path())
}

/// Streaming H.266/VVC encoder backed by `ffmpeg -c:v libvvenc`.
pub struct FfmpegVvcEncoder {
    settings: EncoderSettings,
    child: Option<Child>,
    stdin: Option<BufWriter<ChildStdin>>,
    stdout: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    stderr: Option<JoinHandle<std::io::Result<Vec<u8>>>>,
    frames_sent: u64,
}

impl FfmpegVvcEncoder {
    pub fn new(settings: EncoderSettings) -> Result<Self, EncodeError> {
        if settings.width == 0 || settings.height == 0 {
            return Err(EncodeError::InvalidConfig(
                "width and height must be non-zero".to_string(),
            ));
        }
        if !settings.width.is_multiple_of(2) || !settings.height.is_multiple_of(2) {
            return Err(EncodeError::InvalidConfig(format!(
                "H.266 4:2:0 needs even dimensions, got {}x{}",
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
                "FFmpeg with libvvenc was not found; run scripts/install-ffmpeg.sh \
                 or set KINE_FFMPEG to a build with --enable-libvvenc"
                    .to_string(),
            )
        })?;

        let qp = 4 + (settings.quantizer.min(255) * 59 / 255);
        let preset = match settings.speed {
            9..=u8::MAX => "faster",
            7..=8 => "fast",
            4..=6 => "medium",
            2..=3 => "slow",
            _ => "slower",
        };

        let size = format!("{}x{}", settings.width, settings.height);
        let rate = format!("{}/{}", settings.fps_num, settings.fps_den);
        let mut child = Command::new(&info.path)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "rawvideo",
                "-pixel_format",
                "yuv420p",
                "-video_size",
                &size,
                "-framerate",
                &rate,
                "-i",
                "pipe:0",
                "-an",
                "-c:v",
                "libvvenc",
                "-preset",
                preset,
                "-qp",
                &qp.to_string(),
                "-vvenc-params",
                "IntraPeriod=1",
                "-pix_fmt",
                "yuv420p10le",
                "-f",
                "vvc",
                "pipe:1",
            ])
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

    /// Close the input, wait for FFmpeg, and split the Annex-B output into
    /// access units.
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
                "ffmpeg produced an empty H.266 stream".to_string(),
            ));
        }
        split_access_units(&stdout)
    }
}

impl Drop for FfmpegVvcEncoder {
    fn drop(&mut self) {
        self.stdin.take();
        if let Some(mut child) = self.child.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

/// Split an Annex-B VVC stream into access units.
///
/// FFmpeg/libvvenc emits an AUD per picture, so that is the primary boundary.
/// For streams without AUDs, each VCL NAL starts a new access unit (the
/// supported encoder emits one slice per picture).
pub fn split_access_units(data: &[u8]) -> Result<Vec<AccessUnit>, EncodeError> {
    let nals = parse_annexb(data);
    if nals.is_empty() {
        return Err(EncodeError::NoOutput);
    }
    let has_aud = nals.iter().any(|(_, _, ty)| *ty == 20);
    let mut units = Vec::new();
    let mut current = Vec::new();
    let mut current_has_vcl = false;

    if has_aud {
        for (start, end, ty) in nals {
            // Only split at an AUD once the current access unit actually has a
            // picture; leading parameter sets stay with the first picture.
            if ty == 20 && current_has_vcl && !current.is_empty() {
                units.push(build_unit(&current));
                current.clear();
                current_has_vcl = false;
            }
            current.extend_from_slice(&data[start..end]);
            current_has_vcl |= ty <= 11;
        }
        if !current.is_empty() {
            units.push(build_unit(&current));
        }
    } else {
        for (start, end, ty) in nals {
            current.extend_from_slice(&data[start..end]);
            if ty <= 11 {
                units.push(build_unit(&current));
                current.clear();
            }
        }
        if !current.is_empty() {
            units.push(build_unit(&current));
        }
    }

    if units.is_empty() {
        return Err(EncodeError::NoOutput);
    }
    Ok(units)
}

fn build_unit(data: &[u8]) -> AccessUnit {
    let keyframe = parse_annexb(data)
        .iter()
        .any(|(_, _, ty)| matches!(ty, 7 | 8 | 9));
    AccessUnit {
        data: data.to_vec(),
        keyframe,
    }
}

/// Locate Annex-B NAL units: `(start_of_start_code, end, nal_unit_type)`.
fn parse_annexb(data: &[u8]) -> Vec<(usize, usize, u8)> {
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
        if header + 1 < end {
            let ty = (data[header + 1] >> 3) & 0x1F;
            out.push((start, end, ty));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn nal(ty: u8, payload: &[u8]) -> Vec<u8> {
        let mut out = vec![0, 0, 0, 1, 0, ty << 3 | 1];
        out.extend_from_slice(payload);
        out
    }

    #[test]
    fn splits_on_access_unit_delimiters() {
        let mut stream = Vec::new();
        stream.extend(nal(20, &[]));
        stream.extend(nal(15, &[1, 2]));
        stream.extend(nal(8, &[3]));
        stream.extend(nal(20, &[]));
        stream.extend(nal(1, &[4]));
        let units = split_access_units(&stream).unwrap();
        assert_eq!(units.len(), 2);
        assert!(units[0].keyframe);
        assert!(!units[1].keyframe);
    }

    #[test]
    fn splits_without_aud_on_vcl_nals() {
        let mut stream = Vec::new();
        stream.extend(nal(15, &[1]));
        stream.extend(nal(7, &[2]));
        stream.extend(nal(1, &[3]));
        let units = split_access_units(&stream).unwrap();
        assert_eq!(units.len(), 2);
        assert!(units[0].keyframe);
        assert!(!units[1].keyframe);
    }
}
