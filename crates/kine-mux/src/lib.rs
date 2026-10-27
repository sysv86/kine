//! Container muxing for Kine:
//! * **MP4 (`.mp4`)** via `muxfin` (ISO-BMFF, `av01` + `avc1` + `dfLa` FLAC).
//! * **Matroska (`.mkv`) & WebM (`.webm`)** via [`mkv::MkvMuxer`] (EBML v1 /
//!   Matroska v4, `V_AV1` + `V_MPEG4/ISO/AVC` + `A_FLAC`).
//!
//! Kine never encodes video itself; the elementary streams come from the
//! external FFmpeg backend in `kine-encode`.

pub mod mkv;

pub use mkv::{EbmlDocType, MkvMuxer};

use muxfin::api::{AudioCodec, Muxer as Mp4MuxerInner, MuxerBuilder, VideoCodec};
use muxfin::time::{EncodedSample, SampleTime};
use std::cell::RefCell;
use std::io::Write;
use std::rc::Rc;

/// MP4 video timescale used by this engine (standard 90 kHz).
pub const VIDEO_TIMESCALE: i64 = 90_000;

/// Sample duration for a constant frame rate, in timescale ticks.
pub fn frame_duration(fps_num: u32, fps_den: u32) -> u32 {
    (VIDEO_TIMESCALE as u64 * u64::from(fps_den) / u64::from(fps_num)) as u32
}

/// Convert a frame index to a 90 kHz timestamp for a rational frame rate.
pub fn frame_pts(frame: u64, fps_num: u32, fps_den: u32) -> i64 {
    (frame * VIDEO_TIMESCALE as u64 * u64::from(fps_den) / u64::from(fps_num)) as i64
}

#[derive(Debug, thiserror::Error)]
pub enum MuxError {
    #[error("muxfin error: {0}")]
    Muxfin(#[from] muxfin::api::MuxerError),

    #[error("io error: {0}")]
    Io(#[from] std::io::Error),

    #[error("sink still shared after finalize")]
    SinkShared,

    #[error("audio track configuration is invalid: {0}")]
    BadAudioConfig(String),
}

struct Slot<W: Write>(Rc<RefCell<W>>);

impl<W: Write> Write for Slot<W> {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.borrow_mut().write(buf)
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.0.borrow_mut().flush()
    }
}

/// Audio track to mux alongside the video.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AudioSpec {
    /// FLAC: lossless, one MP4/MKV sample per FLAC frame.
    /// The 34-byte STREAMINFO payload comes from the encoder.
    Flac { streaminfo: Vec<u8> },
}

/// Video codec the muxer writes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Codec {
    /// H.264 / AVC in Annex-B form (converted to `avcC` + length-prefixed NALs).
    H264,
    /// AV1 in low-overhead OBU form.
    Av1,
}

impl Codec {
    fn mp4_video_codec(self) -> VideoCodec {
        match self {
            Self::H264 => VideoCodec::H264,
            Self::Av1 => VideoCodec::Av1,
        }
    }
}

/// MP4 writer for a single video track (and optional audio track).
pub struct Muxer<W: Write> {
    sink: Rc<RefCell<W>>,
    inner: Option<Mp4MuxerInner<Slot<W>>>,
    fps_num: u32,
    fps_den: u32,
    audio_sample_rate: Option<u32>,
}

impl<W: Write> Muxer<W> {
    /// Create a muxer for a constant-frame-rate track of the given codec.
    pub fn new(sink: W, codec: Codec, width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        let sink = Rc::new(RefCell::new(sink));
        let framerate = f64::from(fps_num) / f64::from(fps_den);
        let inner = MuxerBuilder::new(Slot(Rc::clone(&sink)))
            .video(codec.mp4_video_codec(), width, height, framerate)
            .build()
            .expect("builder configuration is valid by construction");
        Self {
            sink,
            inner: Some(inner),
            fps_num,
            fps_den,
            audio_sample_rate: None,
        }
    }

    /// Create a muxer with an audio track.
    pub fn with_audio(
        sink: W,
        codec: Codec,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        audio: &AudioSpec,
        sample_rate: u32,
        channels: u16,
    ) -> Result<Self, MuxError> {
        let AudioSpec::Flac { streaminfo } = audio;
        if streaminfo.len() != 34 {
            return Err(MuxError::BadAudioConfig(format!(
                "STREAMINFO must be 34 bytes, got {}",
                streaminfo.len()
            )));
        }
        let sink = Rc::new(RefCell::new(sink));
        let framerate = f64::from(fps_num) / f64::from(fps_den);
        let inner = MuxerBuilder::new(Slot(Rc::clone(&sink)))
            .video(codec.mp4_video_codec(), width, height, framerate)
            .audio(AudioCodec::Flac, sample_rate, channels)
            .with_flac_streaminfo(streaminfo.clone())
            .build()
            .map_err(|e| MuxError::BadAudioConfig(e.to_string()))?;
        Ok(Self {
            sink,
            inner: Some(inner),
            fps_num,
            fps_den,
            audio_sample_rate: Some(sample_rate),
        })
    }

    /// Queue one encoded audio frame.
    pub fn push_audio_frame(
        &mut self,
        pts_samples: u64,
        data: &[u8],
        samples: u32,
    ) -> Result<(), MuxError> {
        if self.audio_sample_rate.is_none() {
            return Err(MuxError::BadAudioConfig(
                "this muxer was created without an audio track".to_string(),
            ));
        }
        if samples == 0 {
            return Err(MuxError::BadAudioConfig(
                "an audio frame must contain at least one sample".to_string(),
            ));
        }
        let pts = i64::try_from(pts_samples)
            .map_err(|_| MuxError::BadAudioConfig("audio pts overflow".to_string()))?;
        let sample = EncodedSample {
            data,
            timing: SampleTime::new(pts, pts, samples)?,
            is_sync: true,
        };
        self.inner
            .as_mut()
            .expect("muxer finalized")
            .write_audio_sample(sample)?;
        Ok(())
    }

    /// Create a muxer for a constant-frame-rate H.264 track.
    pub fn h264(sink: W, width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self::new(sink, Codec::H264, width, height, fps_num, fps_den)
    }

    /// Create a muxer for a constant-frame-rate AV1 track.
    pub fn av1(sink: W, width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self::new(sink, Codec::Av1, width, height, fps_num, fps_den)
    }

    /// Queue one AV1 access unit (OBU stream).
    pub fn push_av1_obu(
        &mut self,
        frame: u64,
        data: &[u8],
        keyframe: bool,
    ) -> Result<(), MuxError> {
        self.push_video_sample(frame, data, keyframe)
    }

    /// Queue one encoded video sample for the muxer's declared [`Codec`].
    pub fn push_video_sample(
        &mut self,
        frame: u64,
        data: &[u8],
        keyframe: bool,
    ) -> Result<(), MuxError> {
        let pts = frame_pts(frame, self.fps_num, self.fps_den);
        let sample = EncodedSample {
            data,
            timing: SampleTime::new(pts, pts, frame_duration(self.fps_num, self.fps_den))?,
            is_sync: keyframe,
        };
        self.inner
            .as_mut()
            .expect("muxer finalized")
            .write_video_sample(sample)?;
        Ok(())
    }

    /// Queue one H.264 Annex-B access unit.
    pub fn push_h264_annexb(
        &mut self,
        frame: u64,
        data: &[u8],
        keyframe: bool,
    ) -> Result<(), MuxError> {
        self.push_video_sample(frame, data, keyframe)
    }

    /// Finalize and return the sink containing the complete MP4 file.
    pub fn finalize(mut self) -> Result<W, MuxError> {
        if let Some(inner) = self.inner.take() {
            inner.finish()?;
        }
        Rc::try_unwrap(self.sink)
            .map_err(|_| MuxError::SinkShared)?
            .into_inner()
            .pipe(Ok)
    }
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}
impl<T> Pipe for T {}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    fn ffmpeg() -> Option<&'static str> {
        if let Some(path) = std::env::var_os("KINE_FFMPEG") {
            return Some(Box::leak(
                path.to_string_lossy().into_owned().into_boxed_str(),
            ));
        }
        let candidate = Command::new("ffmpeg")
            .arg("-version")
            .output()
            .ok()
            .filter(|out| out.status.success())
            .map(|_| "ffmpeg");
        candidate
    }

    fn encode_h264_frame(ffmpeg: &str, frame_index: i64) -> Option<Vec<u8>> {
        let out = Command::new(ffmpeg)
            .args([
                "-hide_banner",
                "-loglevel",
                "error",
                "-f",
                "lavfi",
                "-i",
                "color=c=blue:s=128x96:d=1",
                "-frames:v",
                "1",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-pix_fmt",
                "yuv420p",
                "-x264-params",
                &format!("keyint=1:scenecut=0:offset={frame_index}"),
                "-f",
                "h264",
                "-",
            ])
            .output()
            .ok()?;
        if !out.status.success() || out.stdout.is_empty() {
            return None;
        }
        Some(out.stdout)
    }

    #[test]
    fn muxes_h264_annexb_into_mp4() {
        let Some(ffmpeg) = ffmpeg() else {
            eprintln!("ffmpeg unavailable — skipping (test-only tooling)");
            return;
        };
        let Some(first) = encode_h264_frame(ffmpeg, 0) else {
            eprintln!("ffmpeg/libx264 unavailable — skipping (test-only tooling)");
            return;
        };
        let mut muxer = Muxer::h264(Vec::new(), 128, 96, 30, 1);
        muxer.push_h264_annexb(0, &first, true).unwrap();
        for i in 1..4u64 {
            let frame = encode_h264_frame(ffmpeg, i as i64).expect("ffmpeg worked before");
            muxer.push_h264_annexb(i, &frame, false).unwrap();
        }
        let mp4 = muxer.finalize().unwrap();

        let has = |needle: &[u8]| mp4.windows(needle.len()).any(|w| w == needle);
        assert!(has(b"ftyp"), "no ftyp box in {} bytes", mp4.len());
        assert!(has(b"moov"), "no moov box");
        assert!(has(b"mdat"), "no mdat box");
    }

    #[test]
    fn frame_pts_matches_timescale() {
        assert_eq!(frame_pts(0, 30, 1), 0);
        assert_eq!(frame_pts(1, 30, 1), 3000);
        assert_eq!(frame_pts(30, 30, 1), 90_000);
        assert_eq!(frame_pts(24, 24, 1), 90_000);
        assert_eq!(frame_duration(30, 1), 3000);
    }
}
