//! End-to-end video output (MP4, MKV, WebM, Y4M, GIF, KVID, Raw) and video input
//! (`NodeKind::Video`, `decode_video_source`, `transcode_video_clip`) tests.

use kine_server::{Engine, VideoContainer, VideoEncoder, VideoOptions};
use std::sync::atomic::{AtomicUsize, Ordering};

fn scratch_dir(tag: &str) -> std::path::PathBuf {
    static COUNTER: AtomicUsize = AtomicUsize::new(0);
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("kine-{tag}-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("scratch dir");
    dir
}

const FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

fn animated_scene_json() -> String {
    serde_json::json!({
        "width": 96,
        "height": 64,
        "root": {
            "kind": "container",
            "size": { "width": 96, "height": 64 },
            "children": [
                {
                    "kind": "rect",
                    "fill": "#101828",
                    "position": "absolute",
                    "inset": { "top": 0, "left": 0, "right": 0, "bottom": 0 }
                },
                {
                    "kind": "rect",
                    "fill": "#4fd1c5",
                    "size": { "width": 40, "height": 16 },
                    "position": "absolute",
                    "inset": { "top": 4, "left": 8 },
                    "animate": {
                        "offsetY": { "from": 0, "to": 40, "start": 0.0, "end": 1.0 }
                    }
                }
            ]
        }
    })
    .to_string()
}

fn static_scene_json() -> String {
    serde_json::json!({
        "width": 96,
        "height": 64,
        "root": {
            "kind": "container",
            "size": { "width": 96, "height": 64 },
            "children": [
                {
                    "kind": "rect",
                    "fill": "#101828",
                    "position": "absolute",
                    "inset": { "top": 0, "left": 0, "right": 0, "bottom": 0 }
                },
                {
                    "kind": "rect",
                    "fill": "#4fd1c5",
                    "size": { "width": 40, "height": 16 },
                    "position": "absolute",
                    "inset": { "top": 4, "left": 8 }
                }
            ]
        }
    })
    .to_string()
}

fn engine_with(json: &str) -> (Engine, kine_server::SceneHandle) {
    let mut engine = Engine::new();
    engine.load_font("JetBrains Mono", FONT.to_vec());
    let handle = engine.load_scene_json(json).expect("scene");
    (engine, handle)
}

fn has(bytes: &[u8], needle: &[u8]) -> bool {
    bytes.windows(needle.len()).any(|w| w == needle)
}

#[test]
fn renders_an_animated_scene_to_mp4() {
    let (mut engine, handle) = engine_with(&animated_scene_json());
    assert!(engine.is_animated(handle).unwrap());

    let opts = VideoOptions {
        frames: 12,
        fps_num: 12,
        fps_den: 1,
        quantizer: 140,
        speed: 10,
        keyframe_interval: 12,
        ..Default::default()
    };
    let report = kine_server::render_video(&mut engine, handle, &opts).expect("video");

    assert!(has(&report.bytes, b"ftyp"), "missing ftyp");
    assert!(has(&report.bytes, b"moov"), "missing moov");
    assert!(has(&report.bytes, b"mdat"), "missing mdat");
    assert!(has(&report.bytes, b"av01"), "missing av01 sample entry");

    assert_eq!(report.width, 96);
    assert_eq!(report.height, 64);
    assert_eq!(report.frames, 12);
    assert!(report.samples >= 12);
    assert!(report.keyframes >= 1);
    assert!(report.distinct_frames > 1);
}

#[test]
fn renders_animated_scene_to_all_containers_and_raw_streams() {
    let (mut engine, handle) = engine_with(&animated_scene_json());

    for container in [
        VideoContainer::Mkv,
        VideoContainer::Webm,
        VideoContainer::Y4m,
        VideoContainer::Gif,
        VideoContainer::Kvid,
        VideoContainer::RawRgba,
        VideoContainer::RawI420,
        VideoContainer::RawNv12,
    ] {
        let opts = VideoOptions {
            frames: 4,
            fps_num: 10,
            fps_den: 1,
            quantizer: 160,
            speed: 10,
            keyframe_interval: 4,
            container,
            ..Default::default()
        };
        let report =
            kine_server::render_video(&mut engine, handle, &opts).expect("container render");
        assert_eq!(report.container, container);
        assert_eq!(report.frames, 4);
        match container {
            VideoContainer::Mkv => {
                assert_eq!(&report.bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);
                assert!(has(&report.bytes, b"matroska"));
            }
            VideoContainer::Webm => {
                assert_eq!(&report.bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);
                assert!(has(&report.bytes, b"webm"));
            }
            VideoContainer::Y4m => {
                assert!(report.bytes.starts_with(b"YUV4MPEG2 W96 H64 F10:1 Ip"));
            }
            VideoContainer::Gif => {
                assert!(report.bytes.starts_with(b"GIF89a"));
                assert_eq!(report.bytes.last().copied(), Some(0x3B));
            }
            VideoContainer::Kvid => {
                assert!(report.bytes.starts_with(b"KVID\x01\x00\x00\x00"));
                let decoded = kine_server::decode_video_bytes(&report.bytes, None).unwrap();
                assert_eq!(decoded.width, 96);
                assert_eq!(decoded.height, 64);
                assert_eq!(decoded.frames_rgba.len(), 4);
            }
            VideoContainer::RawRgba => {
                assert_eq!(report.bytes.len(), 96 * 64 * 4 * 4);
            }
            VideoContainer::RawI420 | VideoContainer::RawNv12 => {
                assert_eq!(report.bytes.len(), (96 * 64 * 3 / 2) * 4);
            }
            VideoContainer::Mp4 => unreachable!(),
        }
    }
}

#[test]
fn video_input_node_reads_rendered_y4m_gif_and_kvid_files() {
    let dir = scratch_dir("video-io");
    let (mut src_engine, src_handle) = engine_with(&animated_scene_json());

    for (ext, container) in [
        ("clip.y4m", VideoContainer::Y4m),
        ("clip.gif", VideoContainer::Gif),
        ("clip.kvid", VideoContainer::Kvid),
    ] {
        let opts = VideoOptions {
            frames: 6,
            fps_num: 12,
            fps_den: 1,
            container,
            ..Default::default()
        };
        let report = kine_server::render_video(&mut src_engine, src_handle, &opts).unwrap();
        let file_path = dir.join(ext);
        std::fs::write(&file_path, &report.bytes).unwrap();

        let composite_json = serde_json::json!({
            "width": 96,
            "height": 64,
            "root": {
                "kind": "container",
                "size": { "width": 96, "height": 64 },
                "children": [{
                    "kind": "video",
                    "src": ext,
                    "objectFit": "cover",
                    "loop": true,
                    "size": { "width": 96, "height": 64 },
                    "position": "absolute",
                    "inset": { "top": 0, "left": 0 }
                }]
            }
        })
        .to_string();

        let mut comp_engine = Engine::new();
        let comp_handle = comp_engine
            .load_scene_json_with_base(&composite_json, &dir)
            .unwrap();
        assert!(comp_engine.is_animated(comp_handle).unwrap());
        let f0 = comp_engine.render_frame_at(comp_handle, 0.0).unwrap();
        let f3 = comp_engine.render_frame_at(comp_handle, 0.35).unwrap();
        assert_ne!(
            f0.data_as_u8_slice(),
            f3.data_as_u8_slice(),
            "composited video input {ext} must change over time"
        );
    }

    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_static_scene_reports_that_nothing_moved() {
    let (mut engine, handle) = engine_with(&static_scene_json());
    assert!(!engine.is_animated(handle).unwrap());

    let opts = VideoOptions {
        frames: 4,
        fps_num: 12,
        fps_den: 1,
        quantizer: 160,
        speed: 10,
        keyframe_interval: 4,
        ..Default::default()
    };
    let report = kine_server::render_video(&mut engine, handle, &opts).expect("video");
    assert_eq!(report.distinct_frames, 1);
}

#[test]
fn time_drives_the_render() {
    let (mut engine, handle) = engine_with(&animated_scene_json());
    let first = engine.render_frame_at(handle, 0.0).unwrap();
    let later = engine.render_frame_at(handle, 1.0).unwrap();
    assert_ne!(first.data_as_u8_slice(), later.data_as_u8_slice());

    let again = engine.render_frame_at(handle, 0.0).unwrap();
    assert_eq!(first.data_as_u8_slice(), again.data_as_u8_slice());
}

#[test]
fn zero_frames_is_rejected() {
    let (mut engine, handle) = engine_with(&static_scene_json());
    let opts = VideoOptions {
        frames: 0,
        ..Default::default()
    };
    let err = kine_server::render_video(&mut engine, handle, &opts).unwrap_err();
    assert!(err.to_string().contains("frame count"), "got: {err}");
}

// ── Audio ───────────────────────────────────────────────────────────────────

fn write_tone(dir: &std::path::Path, name: &str, seconds: f64, gain: f32) -> std::path::PathBuf {
    let rate = 48_000u32;
    let count = (seconds * f64::from(rate)) as usize;
    let mut samples = Vec::with_capacity(count);
    for i in 0..count {
        let t = i as f32 / rate as f32;
        samples.push(gain * (2.0 * std::f32::consts::PI * 440.0 * t).sin());
    }
    let clip = kine_audio::AudioClip {
        sample_rate: rate,
        channels: 1,
        samples,
    };
    let path = dir.join(name);
    kine_audio::write_wav(&path, &clip).expect("write wav");
    path
}

fn scene_with_audio(audio: serde_json::Value) -> String {
    serde_json::json!({
        "width": 96,
        "height": 64,
        "root": {
            "kind": "container",
            "size": { "width": 96, "height": 64 },
            "children": [{
                "kind": "rect",
                "fill": "#101828",
                "position": "absolute",
                "inset": { "top": 0, "left": 0, "right": 0, "bottom": 0 }
            }]
        },
        "audio": audio
    })
    .to_string()
}

#[test]
fn a_scene_audio_bed_lands_in_the_mp4() {
    let dir = scratch_dir("audio-scene");
    write_tone(&dir, "tone.wav", 1.0, 0.5);

    let json = scene_with_audio(serde_json::json!({
        "sampleRate": 48000,
        "channels": 1,
        "tracks": [{ "src": "tone.wav", "gainDb": 0.0 }]
    }));

    let mut engine = Engine::new();
    engine.load_font("JetBrains Mono", FONT.to_vec());
    let handle = engine
        .load_scene_json_with_base(&json, &dir)
        .expect("scene with audio");

    let opts = VideoOptions {
        frames: 6,
        fps_num: 12,
        fps_den: 1,
        quantizer: 160,
        speed: 10,
        keyframe_interval: 6,
        ..Default::default()
    };
    let report = kine_server::render_video(&mut engine, handle, &opts).expect("video");

    assert!(report.audio_frames > 0, "no audio frames were written");
    assert_eq!(report.audio_samples, 24_000);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn synth_tracks_and_sidechain_ducking_work_without_external_files() {
    let json = scene_with_audio(serde_json::json!({
        "sampleRate": 48000,
        "channels": 2,
        "normalizePeakDb": -1.0,
        "tracks": [
            {
                "src": "synth:sine:440:0.4",
                "gainDb": -2.0,
                "start": 0.1,
                "fadeIn": 0.05,
                "fadeOut": 0.05,
                "fadeCurve": "sCurve",
                "pan": 0.0,
                "eq": [{ "kind": "highpass", "freqHz": 100.0, "q": 0.707 }]
            },
            {
                "src": "synth:chord:220,277.18,329.63:0.3",
                "gainDb": -6.0,
                "loop": true,
                "duckDb": -12.0,
                "duckThresholdDb": -35.0,
                "pan": -0.25,
                "eq": [{ "kind": "lowpass", "freqHz": 4000.0, "q": 0.707 }]
            }
        ]
    }));

    let (engine, handle) = engine_with(&json);
    let clip = kine_server::mix_scene_audio(&engine, handle, 0.5)
        .expect("mix")
        .expect("has audio");
    let metrics = clip.metrics();
    assert_eq!(clip.channels, 2);
    assert_eq!(clip.frame_count(), 24_000);
    assert!(
        (metrics.peak_db - (-1.0)).abs() < 0.25,
        "peak_db = {}",
        metrics.peak_db
    );
}

#[test]
fn a_missing_audio_source_is_an_error_not_a_silent_drop() {
    let json = scene_with_audio(serde_json::json!({
        "tracks": [{ "src": "does-not-exist.wav" }]
    }));
    let mut engine = Engine::new();
    engine.load_font("JetBrains Mono", FONT.to_vec());
    let handle = engine
        .load_scene_json_with_base(&json, std::path::Path::new("/nonexistent"))
        .expect("scene loads; audio is resolved at render time");

    let opts = VideoOptions {
        frames: 2,
        fps_num: 12,
        fps_den: 1,
        ..Default::default()
    };
    let err = kine_server::render_video(&mut engine, handle, &opts).unwrap_err();
    assert!(err.to_string().contains("does-not-exist.wav"));
}

#[test]
fn a_scene_without_audio_writes_no_audio_track() {
    let (mut engine, handle) = engine_with(&static_scene_json());
    let opts = VideoOptions {
        frames: 3,
        fps_num: 12,
        fps_den: 1,
        ..Default::default()
    };
    let report = kine_server::render_video(&mut engine, handle, &opts).expect("video");
    assert_eq!(report.audio_frames, 0);
    assert_eq!(report.audio_samples, 0);
}

#[test]
fn renders_an_animated_scene_to_av1_mp4_and_mkv() {
    if !kine_server::VideoEncoder::Av1Ffmpeg.is_available() {
        eprintln!("skipping AV1 test: FFmpeg with an AV1 encoder is not available");
        return;
    }
    let (mut engine, handle) = engine_with(&animated_scene_json());

    let mp4_opts = VideoOptions {
        encoder: VideoEncoder::Av1Ffmpeg,
        container: VideoContainer::Mp4,
        frames: 8,
        fps_num: 24,
        fps_den: 1,
        quantizer: 80,
        speed: 9,
        keyframe_interval: 4,
        ..Default::default()
    };
    let mp4_report = kine_server::render_video(&mut engine, handle, &mp4_opts).expect("av1 mp4");
    assert_eq!(mp4_report.encoder, VideoEncoder::Av1Ffmpeg);
    assert_eq!(mp4_report.frames, 8);
    assert_eq!(mp4_report.samples, 8);
    assert!(mp4_report.keyframes >= 1);
    assert!(has(&mp4_report.bytes, b"ftyp"));
    assert!(has(&mp4_report.bytes, b"av01"), "missing av01 sample entry");
    assert!(has(&mp4_report.bytes, b"av1C"), "missing av1C config box");
    assert!(has(&mp4_report.bytes, b"mdat"));
    assert!(has(&mp4_report.bytes, b"moov"));

    let mkv_opts = VideoOptions {
        encoder: VideoEncoder::Av1Ffmpeg,
        container: VideoContainer::Mkv,
        frames: 6,
        fps_num: 24,
        fps_den: 1,
        quantizer: 80,
        speed: 9,
        keyframe_interval: 3,
        ..Default::default()
    };
    let mkv_report = kine_server::render_video(&mut engine, handle, &mkv_opts).expect("av1 mkv");
    assert_eq!(&mkv_report.bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);
    assert!(has(&mkv_report.bytes, b"V_AV1"));
}

#[test]
fn renders_an_animated_scene_to_h264_mp4_and_mkv() {
    if !kine_server::VideoEncoder::H264Ffmpeg.is_available() {
        eprintln!("skipping H.264 test: FFmpeg with an H.264 encoder is not available");
        return;
    }
    let (mut engine, handle) = engine_with(&animated_scene_json());

    let mp4_opts = VideoOptions {
        encoder: VideoEncoder::H264Ffmpeg,
        container: VideoContainer::Mp4,
        frames: 8,
        fps_num: 24,
        fps_den: 1,
        quantizer: 80,
        speed: 9,
        keyframe_interval: 4,
        ..Default::default()
    };
    let mp4_report = kine_server::render_video(&mut engine, handle, &mp4_opts).expect("h264 mp4");
    assert_eq!(mp4_report.encoder, VideoEncoder::H264Ffmpeg);
    assert_eq!(mp4_report.frames, 8);
    assert_eq!(mp4_report.samples, 8);
    assert!(mp4_report.keyframes >= 1);
    assert!(has(&mp4_report.bytes, b"ftyp"));
    assert!(has(&mp4_report.bytes, b"avc1"), "missing avc1 sample entry");
    assert!(has(&mp4_report.bytes, b"avcC"), "missing avcC config box");
    assert!(has(&mp4_report.bytes, b"mdat"));
    assert!(has(&mp4_report.bytes, b"moov"));

    let mkv_opts = VideoOptions {
        encoder: VideoEncoder::H264Ffmpeg,
        container: VideoContainer::Mkv,
        frames: 6,
        fps_num: 24,
        fps_den: 1,
        quantizer: 80,
        speed: 9,
        keyframe_interval: 3,
        ..Default::default()
    };
    let mkv_report = kine_server::render_video(&mut engine, handle, &mkv_opts).expect("h264 mkv");
    assert_eq!(&mkv_report.bytes[0..4], &[0x1A, 0x45, 0xDF, 0xA3]);
    assert!(has(&mkv_report.bytes, b"V_MPEG4/ISO/AVC"));
}
