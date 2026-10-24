//! Frame → video/animation: render each frame, encode it, mux into the
//! requested container (`MP4`, `MKV`, `WebM`, `Y4M`, `GIF89a`, `KVID`, or raw
//! `RGBA8`/`I420`/`NV12` streams), plus direct video-to-video transcoding.

use crate::engine::{Engine, EngineError, SceneHandle};
use kine_audio::{
    AudioClip, AudioTrackSpec, BiquadFilterSpec, CompressorSpec, FadeCurve,
};
use kine_encode::{
    encode_kvid, encode_raw_frame, Av1Encoder, EncoderSettings, RawPixelFormat, VvcEncoder, Y4mEncoder,
};
use kine_mux::{AudioSpec, EbmlDocType, MkvMuxer, Muxer};
use kine_render::{encode_animated_gif, pixmap_to_straight_rgba};
use std::io::Cursor;
use std::path::Path;

/// Output container or stream format for multi-frame video/animation renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoContainer {
    /// ISO-BMFF `.mp4` (`av01` / `avc1` + `dfLa` FLAC).
    Mp4,
    /// Matroska `.mkv` (`V_AV1` / `V_MPEG4/ISO/AVC` + `A_FLAC`).
    Mkv,
    /// WebM `.webm` (`V_AV1` + `A_FLAC`).
    Webm,
    /// Uncompressed `YUV4MPEG2` `.y4m` (BT.709 4:2:0 limited range).
    Y4m,
    /// Animated looping `GIF89a` `.gif`.
    Gif,
    /// Kine Indexed Lossless RGBA Video `.kvid` (QOI-compressed frames + O(1) index).
    Kvid,
    /// Uncompressed packed 32-bit RGBA8 stream (`.rgba` / `.raw`).
    RawRgba,
    /// Uncompressed planar BT.709 4:2:0 stream (`.yuv` / `.i420`).
    RawI420,
    /// Uncompressed semi-planar BT.709 4:2:0 stream (`.nv12`).
    RawNv12,
}

impl Default for VideoContainer {
    fn default() -> Self {
        Self::Mp4
    }
}

impl VideoContainer {
    pub fn from_path(path: &Path) -> Option<Self> {
        let ext = path.extension()?.to_str()?;
        Self::from_extension(ext)
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "mp4" | "m4v" => Some(Self::Mp4),
            "mkv" => Some(Self::Mkv),
            "webm" => Some(Self::Webm),
            "y4m" => Some(Self::Y4m),
            "gif" => Some(Self::Gif),
            "kvid" => Some(Self::Kvid),
            "rgba" | "raw" => Some(Self::RawRgba),
            "yuv" | "i420" | "yuv420p" => Some(Self::RawI420),
            "nv12" => Some(Self::RawNv12),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Mp4 => "mp4",
            Self::Mkv => "mkv",
            Self::Webm => "webm",
            Self::Y4m => "y4m",
            Self::Gif => "gif",
            Self::Kvid => "kvid",
            Self::RawRgba => "rgba",
            Self::RawI420 => "i420",
            Self::RawNv12 => "nv12",
        }
    }
}

/// Which encoder produces the video track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoEncoder {
    Av1Software,
    /// Software H.266 / VVC (ITU-T H.266 | ISO/IEC 23090-3) encoder.
    H266Software,
    #[cfg(feature = "hwencode")]
    H264Hardware,
    #[cfg(feature = "zero-copy")]
    H264ZeroCopy,
}

impl VideoEncoder {
    pub fn name(self) -> &'static str {
        match self {
            Self::Av1Software => "av1 (rav1e, software)",
            Self::H266Software => "h266 / vvc (software)",
            #[cfg(feature = "hwencode")]
            Self::H264Hardware => "h264 (hardware, NV12 upload)",
            #[cfg(feature = "zero-copy")]
            Self::H264ZeroCopy => "h264 (hardware, zero-copy GPU render)",
        }
    }

    pub fn is_available(self) -> bool {
        match self {
            Self::Av1Software | Self::H266Software => true,
            #[cfg(feature = "hwencode")]
            Self::H264Hardware => {
                matches!(kine_mux::hwencode::h264_encoder_available(), Ok(true))
            }
            #[cfg(feature = "zero-copy")]
            Self::H264ZeroCopy => {
                kine_render::gpu_adapter_present()
                    && matches!(kine_mux::hwencode::h264_encoder_available(), Ok(true))
            }
        }
    }
}

/// How to encode a video.
#[derive(Debug, Clone, Copy)]
pub struct VideoOptions {
    pub encoder: VideoEncoder,
    pub container: VideoContainer,
    pub fps_num: u32,
    pub fps_den: u32,
    pub frames: u64,
    pub quantizer: usize,
    pub speed: u8,
    pub keyframe_interval: u32,
    pub bitrate: u64,
}

impl Default for VideoOptions {
    fn default() -> Self {
        Self {
            encoder: VideoEncoder::Av1Software,
            container: VideoContainer::Mp4,
            fps_num: 30,
            fps_den: 1,
            frames: 60,
            quantizer: 100,
            speed: 9,
            keyframe_interval: 60,
            bitrate: 0,
        }
    }
}

impl VideoOptions {
    pub fn duration_seconds(&self) -> f64 {
        self.frames as f64 * f64::from(self.fps_den) / f64::from(self.fps_num)
    }

    pub fn frame_time(&self, frame: u64) -> f64 {
        frame as f64 * f64::from(self.fps_den) / f64::from(self.fps_num)
    }
}

struct PreparedAudio {
    flac: kine_audio::FlacOutput,
}

/// Decode, filter, compress, duck, and mix the scene's audio bed into a
/// standalone [`AudioClip`]. Returns `Ok(None)` when the scene declares no audio.
pub fn mix_scene_audio(
    engine: &Engine,
    handle: SceneHandle,
    total_seconds: f64,
) -> Result<Option<AudioClip>, EngineError> {
    let Some((def, base_dir)) = engine.audio_def(handle) else {
        return Ok(None);
    };
    if def.tracks.is_empty() {
        return Ok(None);
    }

    let mut specs = Vec::new();
    for track in &def.tracks {
        let clip = if track.src.starts_with("synth:") {
            kine_audio::synthesize_uri(&track.src, total_seconds, def.sample_rate, def.channels)
                .map_err(|e| EngineError::Video(format!("audio synth {:?}: {e}", track.src)))?
        } else {
            let path = base_dir.join(&track.src);
            kine_audio::decode_file(&path)
                .map_err(|e| EngineError::Video(format!("audio track {:?}: {e}", path.display())))?
        };

        let mut filters = Vec::new();
        if let Some(hp) = track.highpass_hz {
            filters.push(BiquadFilterSpec::HighPass {
                cutoff_hz: hp,
                q: 0.707,
            });
        }
        if let Some(lp) = track.lowpass_hz {
            filters.push(BiquadFilterSpec::LowPass {
                cutoff_hz: lp,
                q: 0.707,
            });
        }
        if let Some(eq_bands) = &track.eq {
            for band in eq_bands {
                let spec = match band.kind.to_ascii_lowercase().as_str() {
                    "lowpass" | "lp" => BiquadFilterSpec::LowPass {
                        cutoff_hz: band.freq_hz,
                        q: band.q,
                    },
                    "highpass" | "hp" => BiquadFilterSpec::HighPass {
                        cutoff_hz: band.freq_hz,
                        q: band.q,
                    },
                    "bandpass" | "bp" => BiquadFilterSpec::BandPass {
                        center_hz: band.freq_hz,
                        q: band.q,
                    },
                    "notch" => BiquadFilterSpec::Notch {
                        center_hz: band.freq_hz,
                        q: band.q,
                    },
                    "lowshelf" | "ls" => BiquadFilterSpec::LowShelf {
                        cutoff_hz: band.freq_hz,
                        gain_db: band.gain_db,
                    },
                    "highshelf" | "hs" => BiquadFilterSpec::HighShelf {
                        cutoff_hz: band.freq_hz,
                        gain_db: band.gain_db,
                    },
                    _ => BiquadFilterSpec::PeakingEq {
                        center_hz: band.freq_hz,
                        q: band.q,
                        gain_db: band.gain_db,
                    },
                };
                filters.push(spec);
            }
        }

        let compressor = track.compressor.map(|c| CompressorSpec {
            threshold_db: c.threshold_db,
            ratio: c.ratio,
            attack_ms: c.attack_ms,
            release_ms: c.release_ms,
            makeup_db: c.makeup_db,
        });

        let fade_curve = track
            .fade_curve
            .as_deref()
            .and_then(FadeCurve::parse)
            .unwrap_or(FadeCurve::Linear);

        specs.push(AudioTrackSpec {
            clip,
            gain: db_to_gain(track.gain_db),
            start: track.start,
            trim_start: track.trim_start,
            trim_end: track.trim_end,
            loop_track: track.loop_track,
            pan: track.pan,
            fade_in: track.fade_in,
            fade_out: track.fade_out,
            fade_curve,
            filters,
            compressor,
            duck_db: track.duck_db,
            duck_threshold_db: track.duck_threshold_db.unwrap_or(-28.0),
            muted: track.muted,
        });
    }

    let mut mixed: AudioClip = kine_audio::mix(&specs, total_seconds, def.sample_rate, def.channels)
        .map_err(|e| EngineError::Video(format!("audio mix: {e}")))?;

    if let Some(target_peak_db) = def.normalize_peak_db {
        mixed.normalize_peak(target_peak_db);
    }

    Ok(Some(mixed))
}

fn prepare_audio(
    engine: &Engine,
    handle: SceneHandle,
    total_seconds: f64,
) -> Result<Option<PreparedAudio>, EngineError> {
    let Some(mixed) = mix_scene_audio(engine, handle, total_seconds)? else {
        return Ok(None);
    };
    let flac = kine_audio::encode_flac(&mixed, 16)
        .map_err(|e| EngineError::Video(format!("flac encode: {e}")))?;
    Ok(Some(PreparedAudio { flac }))
}

fn db_to_gain(db: f32) -> f32 {
    if db <= -100.0 {
        0.0
    } else {
        10f32.powf(db / 20.0)
    }
}

/// What a completed render produced.
#[derive(Debug, Clone)]
pub struct VideoReport {
    /// The container or stream bytes.
    pub bytes: Vec<u8>,
    pub encoder: VideoEncoder,
    pub container: VideoContainer,
    pub audio_frames: u64,
    pub audio_samples: u64,
    pub width: u32,
    pub height: u32,
    pub frames: u64,
    pub samples: u64,
    pub keyframes: u64,
    pub distinct_frames: u64,
}

/// Unified container sink supporting MP4, Matroska (`.mkv`), and WebM (`.webm`).
enum ContainerSink {
    Mp4(Muxer<Cursor<Vec<u8>>>),
    Mkv(MkvMuxer<Cursor<Vec<u8>>>),
}

impl ContainerSink {
    fn new(
        container: VideoContainer,
        codec: kine_mux::Codec,
        width: u32,
        height: u32,
        fps_num: u32,
        fps_den: u32,
        audio: Option<&PreparedAudio>,
    ) -> Result<Self, EngineError> {
        match container {
            VideoContainer::Mkv | VideoContainer::Webm => {
                let doc_type = if container == VideoContainer::Webm {
                    EbmlDocType::Webm
                } else {
                    EbmlDocType::Matroska
                };
                let muxer = match audio {
                    Some(prepared) => MkvMuxer::with_audio(
                        Cursor::new(Vec::new()),
                        doc_type,
                        codec,
                        width,
                        height,
                        fps_num,
                        fps_den,
                        &AudioSpec::Flac {
                            streaminfo: prepared.flac.streaminfo.clone(),
                        },
                        prepared.flac.sample_rate,
                        prepared.flac.channels,
                    )
                    .map_err(|e| EngineError::Video(e.to_string()))?,
                    None => MkvMuxer::new(
                        Cursor::new(Vec::new()),
                        doc_type,
                        codec,
                        width,
                        height,
                        fps_num,
                        fps_den,
                    ),
                };
                Ok(Self::Mkv(muxer))
            }
            _ => {
                let muxer = match audio {
                    Some(prepared) => Muxer::with_audio(
                        Cursor::new(Vec::new()),
                        codec,
                        width,
                        height,
                        fps_num,
                        fps_den,
                        &AudioSpec::Flac {
                            streaminfo: prepared.flac.streaminfo.clone(),
                        },
                        prepared.flac.sample_rate,
                        prepared.flac.channels,
                    )
                    .map_err(|e| EngineError::Video(e.to_string()))?,
                    None => Muxer::new(Cursor::new(Vec::new()), codec, width, height, fps_num, fps_den),
                };
                Ok(Self::Mp4(muxer))
            }
        }
    }

    fn write_audio(&mut self, audio: Option<&PreparedAudio>) -> Result<u64, EngineError> {
        let Some(prepared) = audio else {
            return Ok(0);
        };
        let mut pts = 0u64;
        let mut frames = 0u64;
        for frame in &prepared.flac.frames {
            match self {
                Self::Mp4(m) => m
                    .push_audio_frame(pts, &frame.data, frame.samples)
                    .map_err(|e| EngineError::Video(format!("audio mux: {e}")))?,
                Self::Mkv(m) => m
                    .push_audio_frame(pts, &frame.data, frame.samples)
                    .map_err(|e| EngineError::Video(format!("audio mux: {e}")))?,
            }
            pts += u64::from(frame.samples);
            frames += 1;
        }
        Ok(frames)
    }

    fn push_video(&mut self, frame: u64, data: &[u8], keyframe: bool) -> Result<(), EngineError> {
        match self {
            Self::Mp4(m) => m
                .push_av1_obu(frame, data, keyframe)
                .map_err(|e| EngineError::Video(e.to_string())),
            Self::Mkv(m) => m
                .push_video(frame, data, keyframe)
                .map_err(|e| EngineError::Video(e.to_string())),
        }
    }

    fn finalize(self) -> Result<Vec<u8>, EngineError> {
        match self {
            Self::Mp4(m) => Ok(m
                .finalize()
                .map_err(|e| EngineError::Video(e.to_string()))?
                .into_inner()),
            Self::Mkv(m) => Ok(m
                .finalize()
                .map_err(|e| EngineError::Video(e.to_string()))?
                .into_inner()),
        }
    }
}

/// Render a scene to a video/animation container or raw stream in memory.
pub fn render_video(
    engine: &mut Engine,
    handle: SceneHandle,
    opts: &VideoOptions,
) -> Result<VideoReport, EngineError> {
    if opts.frames == 0 {
        return Err(EngineError::Video("frame count must be greater than zero".into()));
    }
    if opts.fps_num == 0 || opts.fps_den == 0 {
        return Err(EngineError::Video("frame rate must be greater than zero".into()));
    }

    let (width, height) = {
        let scene = engine.scene(handle)?;
        (scene.width, scene.height)
    };

    match opts.container {
        VideoContainer::Y4m => return render_video_y4m(engine, handle, opts, width, height),
        VideoContainer::Gif => return render_video_gif(engine, handle, opts, width, height),
        VideoContainer::Kvid => return render_video_kvid(engine, handle, opts, width, height),
        VideoContainer::RawRgba => {
            return render_video_raw(engine, handle, opts, width, height, RawPixelFormat::Rgba8)
        }
        VideoContainer::RawI420 => {
            return render_video_raw(engine, handle, opts, width, height, RawPixelFormat::I420)
        }
        VideoContainer::RawNv12 => {
            return render_video_raw(engine, handle, opts, width, height, RawPixelFormat::Nv12)
        }
        VideoContainer::Mp4 | VideoContainer::Mkv | VideoContainer::Webm => {}
    }

    #[cfg(feature = "hwencode")]
    if opts.encoder == VideoEncoder::H264Hardware {
        return render_video_h264_hardware(engine, handle, opts, width, height);
    }
    #[cfg(feature = "zero-copy")]
    if opts.encoder == VideoEncoder::H264ZeroCopy {
        return render_video_zero_copy(engine, handle, opts, width, height);
    }

    if opts.encoder == VideoEncoder::H266Software && opts.container == VideoContainer::Webm {
        return Err(EngineError::Video(
            "WebM container only supports AV1/VP9 video; use .mp4 or .mkv for H.266/VVC".into(),
        ));
    }

    let mut settings = EncoderSettings::new(width, height, opts.fps_num, opts.fps_den);
    settings.quantizer = opts.quantizer;
    settings.speed = opts.speed;
    settings.keyframe_interval = opts.keyframe_interval;

    let mux_codec = if opts.encoder == VideoEncoder::H266Software {
        kine_mux::Codec::H266
    } else {
        kine_mux::Codec::Av1
    };

    let audio = prepare_audio(engine, handle, opts.duration_seconds())?;
    let mut sink = ContainerSink::new(
        opts.container,
        mux_codec,
        width,
        height,
        opts.fps_num,
        opts.fps_den,
        audio.as_ref(),
    )?;
    let audio_frames = sink.write_audio(audio.as_ref())?;
    let mut samples = 0u64;
    let mut keyframes = 0u64;
    let mut distinct = 0u64;
    let mut previous: Option<Vec<u8>> = None;

    if opts.encoder == VideoEncoder::H266Software {
        let mut encoder = VvcEncoder::new(settings).map_err(|e| EngineError::Video(e.to_string()))?;
        for frame in 0..opts.frames {
            let time = opts.frame_time(frame);
            let pixmap = engine.render_frame_at_with_index(handle, time, frame as u32)?;
            let rgba = pixmap.data_as_u8_slice();

            if previous.as_deref() != Some(rgba) {
                distinct += 1;
                previous = Some(rgba.to_vec());
            }

            let out = encoder
                .encode_rgba(rgba)
                .map_err(|e| EngineError::Video(e.to_string()))?;
            for unit in out.units {
                if unit.keyframe {
                    keyframes += 1;
                }
                sink.push_video(samples, &unit.data, unit.keyframe)?;
                samples += 1;
            }
        }
        let tail = encoder
            .finish()
            .map_err(|e| EngineError::Video(e.to_string()))?;
        for unit in tail.units {
            if unit.keyframe {
                keyframes += 1;
            }
            sink.push_video(samples, &unit.data, unit.keyframe)?;
            samples += 1;
        }
    } else {
        let mut encoder = Av1Encoder::new(settings).map_err(|e| EngineError::Video(e.to_string()))?;
        for frame in 0..opts.frames {
            let time = opts.frame_time(frame);
            let pixmap = engine.render_frame_at_with_index(handle, time, frame as u32)?;
            let rgba = pixmap.data_as_u8_slice();

            if previous.as_deref() != Some(rgba) {
                distinct += 1;
                previous = Some(rgba.to_vec());
            }

            let out = encoder
                .encode_rgba(rgba)
                .map_err(|e| EngineError::Video(e.to_string()))?;
            for unit in out.units {
                if unit.keyframe {
                    keyframes += 1;
                }
                sink.push_video(samples, &unit.data, unit.keyframe)?;
                samples += 1;
            }
        }
        let tail = encoder
            .finish()
            .map_err(|e| EngineError::Video(e.to_string()))?;
        for unit in tail.units {
            if unit.keyframe {
                keyframes += 1;
            }
            sink.push_video(samples, &unit.data, unit.keyframe)?;
            samples += 1;
        }
    }

    if samples == 0 {
        return Err(EngineError::Video("encoder produced no samples".into()));
    }

    let bytes = sink.finalize()?;

    Ok(VideoReport {
        bytes,
        encoder: if opts.encoder == VideoEncoder::H266Software {
            VideoEncoder::H266Software
        } else {
            VideoEncoder::Av1Software
        },
        container: opts.container,
        audio_frames,
        audio_samples: audio
            .as_ref()
            .map(|a| a.flac.total_samples())
            .unwrap_or(0),
        width,
        height,
        frames: opts.frames,
        samples,
        keyframes,
        distinct_frames: distinct,
    })
}

fn render_video_y4m(
    engine: &mut Engine,
    handle: SceneHandle,
    opts: &VideoOptions,
    width: u32,
    height: u32,
) -> Result<VideoReport, EngineError> {
    let mut y4m = Y4mEncoder::new(Vec::new(), width, height, opts.fps_num, opts.fps_den)
        .map_err(|e| EngineError::Video(e.to_string()))?;
    let mut distinct = 0u64;
    let mut previous: Option<Vec<u8>> = None;

    for frame in 0..opts.frames {
        let time = opts.frame_time(frame);
        let pixmap = engine.render_frame_at_with_index(handle, time, frame as u32)?;
        let rgba = pixmap.data_as_u8_slice();
        if previous.as_deref() != Some(rgba) {
            distinct += 1;
            previous = Some(rgba.to_vec());
        }
        y4m.write_rgba(rgba)
            .map_err(|e| EngineError::Video(e.to_string()))?;
    }
    let bytes = y4m.finish().map_err(|e| EngineError::Video(e.to_string()))?;
    Ok(VideoReport {
        bytes,
        encoder: VideoEncoder::Av1Software,
        container: VideoContainer::Y4m,
        audio_frames: 0,
        audio_samples: 0,
        width,
        height,
        frames: opts.frames,
        samples: opts.frames,
        keyframes: opts.frames,
        distinct_frames: distinct,
    })
}

fn render_video_gif(
    engine: &mut Engine,
    handle: SceneHandle,
    opts: &VideoOptions,
    width: u32,
    height: u32,
) -> Result<VideoReport, EngineError> {
    let mut frames_vec = Vec::with_capacity(opts.frames as usize);
    let mut distinct = 0u64;
    let mut previous: Option<Vec<u8>> = None;

    for frame in 0..opts.frames {
        let time = opts.frame_time(frame);
        let pixmap = engine.render_frame_at_with_index(handle, time, frame as u32)?;
        let rgba = pixmap.data_as_u8_slice();
        if previous.as_deref() != Some(rgba) {
            distinct += 1;
            previous = Some(rgba.to_vec());
        }
        frames_vec.push(pixmap);
    }
    let bytes = encode_animated_gif(&frames_vec, opts.fps_num, opts.fps_den)
        .map_err(EngineError::Video)?;
    Ok(VideoReport {
        bytes,
        encoder: VideoEncoder::Av1Software,
        container: VideoContainer::Gif,
        audio_frames: 0,
        audio_samples: 0,
        width,
        height,
        frames: opts.frames,
        samples: opts.frames,
        keyframes: opts.frames,
        distinct_frames: distinct,
    })
}

fn render_video_kvid(
    engine: &mut Engine,
    handle: SceneHandle,
    opts: &VideoOptions,
    width: u32,
    height: u32,
) -> Result<VideoReport, EngineError> {
    let mut frames_rgba = Vec::with_capacity(opts.frames as usize);
    let mut distinct = 0u64;
    let mut previous: Option<Vec<u8>> = None;

    for frame in 0..opts.frames {
        let time = opts.frame_time(frame);
        let pixmap = engine.render_frame_at_with_index(handle, time, frame as u32)?;
        let straight = pixmap_to_straight_rgba(&pixmap);
        if previous.as_deref() != Some(&straight[..]) {
            distinct += 1;
            previous = Some(straight.clone());
        }
        frames_rgba.push(straight);
    }
    let bytes = encode_kvid(&frames_rgba, width, height, opts.fps_num, opts.fps_den)
        .map_err(|e| EngineError::Video(e.to_string()))?;
    Ok(VideoReport {
        bytes,
        encoder: VideoEncoder::Av1Software,
        container: VideoContainer::Kvid,
        audio_frames: 0,
        audio_samples: 0,
        width,
        height,
        frames: opts.frames,
        samples: opts.frames,
        keyframes: opts.frames,
        distinct_frames: distinct,
    })
}

fn render_video_raw(
    engine: &mut Engine,
    handle: SceneHandle,
    opts: &VideoOptions,
    width: u32,
    height: u32,
    fmt: RawPixelFormat,
) -> Result<VideoReport, EngineError> {
    let frame_len = fmt.frame_byte_len(width, height);
    let mut bytes = Vec::with_capacity(frame_len * (opts.frames as usize));
    let mut distinct = 0u64;
    let mut previous: Option<Vec<u8>> = None;

    for frame in 0..opts.frames {
        let time = opts.frame_time(frame);
        let pixmap = engine.render_frame_at_with_index(handle, time, frame as u32)?;
        let rgba = pixmap.data_as_u8_slice();
        if previous.as_deref() != Some(rgba) {
            distinct += 1;
            previous = Some(rgba.to_vec());
        }
        let encoded = encode_raw_frame(rgba, width, height, fmt)
            .map_err(|e| EngineError::Video(e.to_string()))?;
        bytes.extend_from_slice(&encoded);
    }
    Ok(VideoReport {
        bytes,
        encoder: VideoEncoder::Av1Software,
        container: opts.container,
        audio_frames: 0,
        audio_samples: 0,
        width,
        height,
        frames: opts.frames,
        samples: opts.frames,
        keyframes: opts.frames,
        distinct_frames: distinct,
    })
}

/// Transcode an already-decoded [`kine_render::DecodedVideoClip`] into any
/// [`VideoContainer`] format (`MP4`, `MKV`, `WebM`, `Y4M`, `GIF`, `KVID`, or raw).
pub fn transcode_video_clip(
    clip: &kine_render::DecodedVideoClip,
    opts: &VideoOptions,
) -> Result<VideoReport, EngineError> {
    if clip.frames_rgba.is_empty() {
        return Err(EngineError::Video("input video clip has no frames".into()));
    }
    let mut engine = Engine::new();
    let mut scene = kine_core::Scene::new(clip.width, clip.height);
    let vid = scene.add_node(
        scene.root,
        kine_core::NodeKind::Video {
            src: "in-memory://transcode".into(),
            fit: kine_core::ObjectFit::Fill,
            corner_radius: 0.0,
            start: 0.0,
            trim_start: 0.0,
            trim_end: None,
            playback_rate: 1.0,
            loop_video: false,
        },
    );
    scene.with_layout(vid, |l| {
        l.position = taffy::Position::Absolute;
        l.size = taffy::Size {
            width: taffy::Dimension::length(clip.width as f32),
            height: taffy::Dimension::length(clip.height as f32),
        };
    });
    let handle = engine.insert_scene(scene);
    engine.preload_video_clip("in-memory://transcode", clip.clone());
    render_video(&mut engine, handle, opts)
}

#[cfg(feature = "hwencode")]
fn render_video_h264_hardware(
    engine: &mut Engine,
    handle: SceneHandle,
    opts: &VideoOptions,
    width: u32,
    height: u32,
) -> Result<VideoReport, EngineError> {
    use kine_mux::hwencode::{video_capable_device, HwEncodeConfig, HwH264Encoder};

    let mut config = HwEncodeConfig::new(width, height, opts.fps_num, opts.fps_den);
    config.keyframe_interval = opts.keyframe_interval;
    if opts.bitrate != 0 {
        config.bitrate = opts.bitrate;
    }

    let (device, _queue) =
        video_capable_device().map_err(|e| EngineError::Video(e.to_string()))?;

    let mut encoder =
        HwH264Encoder::new(&device, config).map_err(|e| EngineError::Video(e.to_string()))?;

    let audio = prepare_audio(engine, handle, opts.duration_seconds())?;
    let mut sink = ContainerSink::new(
        opts.container,
        kine_mux::Codec::H264,
        width,
        height,
        opts.fps_num,
        opts.fps_den,
        audio.as_ref(),
    )?;
    let audio_frames = sink.write_audio(audio.as_ref())?;

    let mut samples = 0u64;
    let mut keyframes = 0u64;
    let mut distinct = 0u64;
    let mut previous: Option<Vec<u8>> = None;

    for frame in 0..opts.frames {
        let time = opts.frame_time(frame);
        let pixmap = engine.render_frame_at_with_index(handle, time, frame as u32)?;
        let rgba = pixmap.data_as_u8_slice();

        if previous.as_deref() != Some(rgba) {
            distinct += 1;
            previous = Some(rgba.to_vec());
        }

        let nv12 = kine_encode::rgba_to_nv12(rgba, width, height);
        let chunks = encoder
            .encode_nv12(&nv12, Some(frame), frame == 0)
            .map_err(|e| EngineError::Video(e.to_string()))?;
        for chunk in chunks {
            if chunk.keyframe {
                keyframes += 1;
            }
            sink.push_video(samples, &chunk.data, chunk.keyframe)?;
            samples += 1;
        }
    }

    for chunk in encoder
        .flush()
        .map_err(|e| EngineError::Video(e.to_string()))?
    {
        if chunk.keyframe {
            keyframes += 1;
        }
        sink.push_video(samples, &chunk.data, chunk.keyframe)?;
        samples += 1;
    }

    if samples == 0 {
        return Err(EngineError::Video("encoder produced no samples".into()));
    }

    let bytes = sink.finalize()?;

    Ok(VideoReport {
        bytes,
        encoder: VideoEncoder::H264Hardware,
        container: opts.container,
        audio_frames,
        audio_samples: audio
            .as_ref()
            .map(|a| a.flac.total_samples())
            .unwrap_or(0),
        width,
        height,
        frames: opts.frames,
        samples,
        keyframes,
        distinct_frames: distinct,
    })
}

#[cfg(feature = "zero-copy")]
fn render_video_zero_copy(
    engine: &mut Engine,
    handle: SceneHandle,
    opts: &VideoOptions,
    width: u32,
    height: u32,
) -> Result<VideoReport, EngineError> {
    use kine_mux::hwencode::{
        request_video_capable_device, HwEncodeConfig, HwH264TextureEncoder,
    };
    use kine_render::gpu::{GpuRenderer, Nv12Converter};

    let mut config = HwEncodeConfig::new(width, height, opts.fps_num, opts.fps_den);
    config.keyframe_interval = opts.keyframe_interval;
    if opts.bitrate != 0 {
        config.bitrate = opts.bitrate;
    }

    let adapter =
        kine_render::gpu::request_adapter().map_err(|e| EngineError::Video(e.to_string()))?;
    let (device, queue) = request_video_capable_device(
        &adapter,
        wgpu::Features::empty(),
        wgpu::Limits::downlevel_defaults(),
    )
    .map_err(|e| EngineError::Video(e.to_string()))?;

    let mut renderer =
        GpuRenderer::from_device(device.clone(), queue.clone(), width as u16, height as u16)
            .map_err(|e| EngineError::Video(e.to_string()))?;
    renderer.sync_fonts_from(engine.shaper());

    let converter = Nv12Converter::new(&device).map_err(|e| EngineError::Video(e.to_string()))?;
    let mut encoder = HwH264TextureEncoder::new(&device, &queue, config)
        .map_err(|e| EngineError::Video(e.to_string()))?;

    let audio = prepare_audio(engine, handle, opts.duration_seconds())?;
    let mut sink = ContainerSink::new(
        opts.container,
        kine_mux::Codec::H264,
        width,
        height,
        opts.fps_num,
        opts.fps_den,
        audio.as_ref(),
    )?;
    let audio_frames = sink.write_audio(audio.as_ref())?;

    let mut samples = 0u64;
    let mut keyframes = 0u64;

    for frame in 0..opts.frames {
        let time = opts.frame_time(frame);
        let rendered = {
            let scene = engine.scene_at(handle, time)?;
            renderer
                .render_to_texture_at(scene, time as f32, frame as u32)
                .map_err(|e| EngineError::Video(e.to_string()))?
        };
        let source = rendered.create_view(&wgpu::TextureViewDescriptor::default());

        let target = encoder
            .begin_frame()
            .map_err(|e| EngineError::Video(e.to_string()))?;
        converter.convert(&device, &queue, &source, target);

        let chunks = encoder
            .submit_frame(Some(frame), frame == 0)
            .map_err(|e| EngineError::Video(e.to_string()))?;
        for chunk in chunks {
            if chunk.keyframe {
                keyframes += 1;
            }
            sink.push_video(samples, &chunk.data, chunk.keyframe)?;
            samples += 1;
        }
    }

    let tail = encoder
        .flush()
        .map_err(|e| EngineError::Video(e.to_string()))?;
    for chunk in tail {
        if chunk.keyframe {
            keyframes += 1;
        }
        sink.push_video(samples, &chunk.data, chunk.keyframe)?;
        samples += 1;
    }

    if samples == 0 {
        return Err(EngineError::Video("encoder produced no samples".into()));
    }

    let bytes = sink.finalize()?;

    Ok(VideoReport {
        bytes,
        encoder: VideoEncoder::H264ZeroCopy,
        container: opts.container,
        audio_frames,
        audio_samples: audio
            .as_ref()
            .map(|a| a.flac.total_samples())
            .unwrap_or(0),
        width,
        height,
        frames: opts.frames,
        samples,
        keyframes,
        distinct_frames: 0,
    })
}
