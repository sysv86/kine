//! kine — non-interactive CLI over the engine.

use kine_audio::{SynthSpec, WavSampleFormat, Waveform};
use kine_platform::{resolve_output, write_output, PlatformInfo};
use kine_render::{FrameFormat, RenderBackend};
use kine_server::{Engine, SceneHandle, VideoContainer, VideoEncoder, VideoOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

const BUNDLED_FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

fn usage() -> &'static str {
    "kine — Video-as-Code engine

USAGE:
  kine new        --out <scene.json>          write a starter scene template
  kine types      [--out <kine.d.ts>]         emit TypeScript .d.ts schema for the DSL
  kine modules    [--dir <project>]           list ready-made modules from <project>/Modules
  kine doctor                                 print engine, GPU/CPU, format & host diagnostics
  kine host                                   print platform/storage policy (JSON)
  kine tone       --out <file.wav|file.flac|file.aiff>
                  [--wave sine|square|saw|triangle|white-noise|pink-noise]
                  [--hz 440] [--sweep-to HZ] [--seconds 2] [--gain 0.4] [--bits 16|24|32f]
  kine audio-mix  --scene <scene> --out <mix.wav|mix.flac|mix.aiff>
                  [--seconds 3] [--bits 16|24|32f]
  kine video-info --in <video.y4m|video.gif|video.kvid|dir|video:...>
  kine transcode  --in <input> --out <output|->
                  [--format mp4|mkv|webm|y4m|gif|kvid|rgba|i420|nv12]
                  [--fps 30] [--quality Q] [--speed 0-10]
  kine shader     (--file <shader.wgsl> | --inline <code>)
                                              validate WGSL now, print its bindings
  kine render     --scene <scene>
                  --out <file|dir/|->
                  [--format mp4|mkv|webm|y4m|gif|kvid|rgba|i420|nv12|png|qoi|bmp|ppm|pam|tga]
                  [--seq] [--frame N] [--frames N] [--fps 30] [--quality Q] [--speed 0-10]
                  [--encoder av1|h264-hw|h264-zero-copy] [--bitrate BPS]
                  [--backend auto|gpu|cpu]
  kine strip      --scene <scene> --out <strip.png> [--frames 6] [--fps 30]
  kine onion      --scene <scene> --out <onion.png> [--frames 6] [--fps 30]
  kine timeline   --scene <scene>
  kine inspect    --scene <scene>
  kine check      --scene <scene> [--against <golden.png>] [--frame N] [--threshold R]
  kine snapshot   --scene <scene.json> --against <golden.png> [--frame N] [--threshold R]
                  [--backend auto|gpu|cpu]
  kine studio     [--scene <scene>] [--workspace <dir>] [--backend auto|gpu|cpu]
                  [--headless-check]          launch the native Xilem Kine Studio GUI
  kine serve      [--listen 127.0.0.1:7700]
"
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let cmd = args.first().map(String::as_str).unwrap_or("");
    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has_flag = |name: &str| -> bool { args.iter().any(|a| a == name) };
    let flag_or = |name: &str, default: &str| flag(name).unwrap_or_else(|| default.to_string());

    let platform = PlatformInfo::detect();

    let mut engine = Engine::new();
    engine.load_font("JetBrains Mono", BUNDLED_FONT.to_vec());

    if let Some(b_name) = flag("--backend") {
        let Some(backend) = RenderBackend::parse(&b_name) else {
            eprintln!("unknown backend {b_name:?} (expected auto, gpu or cpu)");
            return ExitCode::FAILURE;
        };
        engine.set_backend(backend);
    }

    match cmd {
        "new" => {
            let Some(out) = flag("--out") else {
                eprintln!("new requires --out\n{}", usage());
                return ExitCode::FAILURE;
            };
            let json = starter_scene_json();
            if let Err(e) = std::fs::write(&out, json) {
                eprintln!("write {out}: {e}");
                return ExitCode::FAILURE;
            }
            eprintln!("wrote {out}");
            ExitCode::SUCCESS
        }

        "types" => {
            let dts = kine_script::typescript_dts();
            if let Some(out) = flag("--out") {
                if let Err(e) = std::fs::write(&out, dts) {
                    eprintln!("write {out}: {e}");
                    return ExitCode::FAILURE;
                }
                eprintln!("wrote {out}");
            } else {
                print!("{dts}");
            }
            ExitCode::SUCCESS
        }

        "tone" => {
            let Some(out) = flag("--out") else {
                eprintln!("tone requires --out\n{}", usage());
                return ExitCode::FAILURE;
            };
            let out = PathBuf::from(&out);
            if let Some(parent) = out.parent() {
                if !parent.as_os_str().is_empty() {
                    let _ = std::fs::create_dir_all(parent);
                }
            }
            let wave_str = flag("--wave").or_else(|| flag("--waveform")).unwrap_or_else(|| "sine".to_string());
            let Some(waveform) = Waveform::parse(&wave_str) else {
                eprintln!("unknown waveform {wave_str:?}");
                return ExitCode::FAILURE;
            };
            let hz: f32 = flag_or("--hz", "440").parse().unwrap_or(440.0);
            let sweep_to: Option<f32> = flag("--sweep-to")
                .or_else(|| flag("--sweep-to-hz"))
                .and_then(|s| s.parse().ok());
            let seconds: f64 = flag_or("--seconds", "2").parse().unwrap_or(2.0);
            let gain: f32 = flag_or("--gain", "0.4").parse().unwrap_or(0.4);
            let bits = flag("--bits").or_else(|| flag("--bit-depth")).unwrap_or_else(|| "16".to_string());
            let channels: u16 = if has_flag("--stereo") { 2 } else { 1 };

            let spec = SynthSpec {
                waveform,
                frequencies_hz: vec![hz],
                sweep_to_hz: sweep_to,
                duration_seconds: seconds,
                sample_rate: 48_000,
                channels,
                gain,
                ..SynthSpec::default()
            };
            let clip = match kine_audio::synthesize(&spec) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("tone synth error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(e) = write_audio_file(&out, &clip, &bits) {
                eprintln!("write {}: {e}", out.display());
                return ExitCode::FAILURE;
            }
            eprintln!(
                "wrote {} — {:?} {hz} Hz, {seconds}s, 48000 Hz {}ch",
                out.display(),
                waveform,
                channels
            );
            ExitCode::SUCCESS
        }

        "audio-mix" => {
            let (Some(scene_path), Some(out)) = (flag("--scene"), flag("--out")) else {
                eprintln!("audio-mix requires --scene and --out\n{}", usage());
                return ExitCode::FAILURE;
            };
            let seconds: f64 = flag_or("--seconds", "3").parse().unwrap_or(3.0);
            let bits = flag("--bits").or_else(|| flag("--bit-depth")).unwrap_or_else(|| "16".to_string());
            let out = resolve_output(Path::new(&out), &platform);
            let handle = load(&mut engine, &scene_path);
            match kine_server::mix_scene_audio(&engine, handle, seconds) {
                Ok(Some(clip)) => {
                    if let Err(e) = write_audio_file(&out, &clip, &bits) {
                        eprintln!("write {}: {e}", out.display());
                        return ExitCode::FAILURE;
                    }
                    let m = clip.metrics();
                    eprintln!(
                        "wrote {} — {} Hz, {} ch, {:.2}s (peak {:.1} dBFS, RMS {:.1} dBFS, ~{:.1} LUFS)",
                        out.display(),
                        clip.sample_rate,
                        clip.channels,
                        clip.duration_seconds(),
                        m.peak_db,
                        m.rms_db,
                        m.lufs_approx
                    );
                    ExitCode::SUCCESS
                }
                Ok(None) => {
                    eprintln!("scene declares no audio tracks");
                    ExitCode::FAILURE
                }
                Err(e) => {
                    eprintln!("audio-mix error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "video-info" => {
            let Some(input) = flag("--in") else {
                eprintln!("video-info requires --in\n{}", usage());
                return ExitCode::FAILURE;
            };
            match kine_server::decode_video_source(&input) {
                Ok(clip) => {
                    let info = serde_json::json!({
                        "src": input,
                        "width": clip.width,
                        "height": clip.height,
                        "fps_num": clip.fps_num,
                        "fps_den": clip.fps_den,
                        "frames": clip.frames_rgba.len(),
                        "duration_seconds": clip.duration_seconds(),
                    });
                    println!("{}", serde_json::to_string_pretty(&info).unwrap());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("video-info error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "transcode" => {
            let (Some(input), Some(out_raw)) = (flag("--in"), flag("--out")) else {
                eprintln!("transcode requires --in and --out\n{}", usage());
                return ExitCode::FAILURE;
            };
            let clip = match kine_server::decode_video_source(&input) {
                Ok(c) => c,
                Err(e) => {
                    eprintln!("transcode input error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            let is_stdout = out_raw == "-";
            let out_path = if is_stdout {
                PathBuf::from("-")
            } else {
                resolve_output(Path::new(&out_raw), &platform)
            };
            let container = flag("--format")
                .and_then(|f| VideoContainer::from_extension(&f))
                .or_else(|| VideoContainer::from_path(&out_path))
                .unwrap_or(VideoContainer::Mp4);

            let mut opts = VideoOptions::default();
            opts.container = container;
            opts.frames = flag("--frames")
                .and_then(|s| s.parse().ok())
                .unwrap_or(clip.frames_rgba.len() as u64);
            opts.fps_num = flag("--fps")
                .and_then(|s| s.parse().ok())
                .unwrap_or(clip.fps_num);
            opts.fps_den = clip.fps_den.max(1);
            opts.quantizer = flag_or("--quality", "100").parse().unwrap_or(100);
            opts.speed = flag_or("--speed", "9").parse().unwrap_or(9);

            match kine_server::transcode_video_clip(&clip, &opts) {
                Ok(report) => {
                    if is_stdout {
                        if let Err(e) = std::io::stdout().write_all(&report.bytes) {
                            eprintln!("stdout write error: {e}");
                            return ExitCode::FAILURE;
                        }
                    } else if let Err(e) = write_output(&out_path, &report.bytes) {
                        eprintln!("write {}: {e}", out_path.display());
                        return ExitCode::FAILURE;
                    }
                    eprintln!(
                        "transcoded {} -> {} ({}x{}, {} frames, {})",
                        input,
                        out_raw,
                        report.width,
                        report.height,
                        report.frames,
                        report.container.name()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("transcode error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "host" => {
            println!("{}", serde_json::to_string_pretty(&platform).unwrap());
            ExitCode::SUCCESS
        }

        "shader" => {
            let source = match (flag("--file"), flag("--inline")) {
                (Some(path), _) => read_file(&path),
                (None, Some(code)) => code,
                _ => {
                    eprintln!("shader requires --file or --inline\n{}", usage());
                    return ExitCode::FAILURE;
                }
            };
            match kine_shader::validate(&source) {
                Ok(module) => {
                    println!("{}", serde_json::to_string_pretty(&module).unwrap());
                    eprintln!(
                        "ok: {} entry point(s), {} binding(s)",
                        module.entry_points.len(),
                        module.uniforms.len() + module.textures.len() + module.samplers.len()
                    );
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("{}", e.diagnostic());
                    if let Some(line) = e.line() {
                        eprintln!("at line {line}");
                    }
                    ExitCode::from(2)
                }
            }
        }

        "modules" => {
            let dir = flag("--dir")
                .map(PathBuf::from)
                .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
            match kine_script::ModuleRegistry::discover(&dir) {
                Ok(registry) => {
                    let list: Vec<serde_json::Value> = registry
                        .iter()
                        .map(|(name, module)| {
                            serde_json::json!({
                                "name": name,
                                "kind": module.meta.kind,
                                "description": module.meta.description,
                                "params": module.meta.params,
                                "entry": module.scene_path,
                            })
                        })
                        .collect();
                    let out = serde_json::json!({
                        "root": registry.root().map(|p| p.to_string_lossy().into_owned()),
                        "count": registry.len(),
                        "modules": list,
                    });
                    println!("{}", serde_json::to_string_pretty(&out).unwrap());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("modules error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "doctor" => {
            let doc = engine.doctor();
            println!("{}", serde_json::to_string_pretty(&doc).unwrap());
            ExitCode::SUCCESS
        }

        "timeline" => {
            let Some(scene_path) = flag("--scene") else {
                eprintln!("timeline requires --scene\n{}", usage());
                return ExitCode::FAILURE;
            };
            let handle = load(&mut engine, &scene_path);
            match engine.timeline(handle) {
                Ok(value) => {
                    println!("{}", serde_json::to_string_pretty(&value).unwrap());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("timeline error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "strip" => {
            let (Some(scene_path), Some(out)) = (flag("--scene"), flag("--out")) else {
                eprintln!("strip requires --scene and --out\n{}", usage());
                return ExitCode::FAILURE;
            };
            let frames: u32 = flag_or("--frames", "6").parse().unwrap_or(6);
            let fps: f64 = flag_or("--fps", "30").parse().unwrap_or(30.0);
            let out = resolve_output(Path::new(&out), &platform);
            let handle = load(&mut engine, &scene_path);
            match engine.render_strip_png(handle, frames, fps) {
                Ok(png) => {
                    if let Err(e) = write_output(&out, &png) {
                        eprintln!("write {}: {e}", out.display());
                        return ExitCode::FAILURE;
                    }
                    eprintln!("wrote {} ({frames} frames)", out.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("strip error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "onion" => {
            let (Some(scene_path), Some(out)) = (flag("--scene"), flag("--out")) else {
                eprintln!("onion requires --scene and --out\n{}", usage());
                return ExitCode::FAILURE;
            };
            let frames: u32 = flag_or("--frames", "6").parse().unwrap_or(6);
            let fps: f64 = flag_or("--fps", "30").parse().unwrap_or(30.0);
            let out = resolve_output(Path::new(&out), &platform);
            let handle = load(&mut engine, &scene_path);
            match engine.render_onion_png(handle, frames, fps) {
                Ok(png) => {
                    if let Err(e) = write_output(&out, &png) {
                        eprintln!("write {}: {e}", out.display());
                        return ExitCode::FAILURE;
                    }
                    eprintln!("wrote {} ({frames} frames)", out.display());
                    ExitCode::SUCCESS
                }
                Err(e) => {
                    eprintln!("onion error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "render" => {
            let (Some(scene_path), Some(out_raw)) = (flag("--scene"), flag("--out")) else {
                eprintln!("render requires --scene and --out\n{}", usage());
                return ExitCode::FAILURE;
            };
            let is_stdout = out_raw == "-";
            let is_seq_dir = has_flag("--seq")
                || out_raw.ends_with('/')
                || Path::new(&out_raw).is_dir();
            let out = if is_stdout {
                PathBuf::from("-")
            } else {
                resolve_output(Path::new(&out_raw), &platform)
            };
            let handle = load(&mut engine, &scene_path);
            let explicit_fmt = flag("--format");

            if is_seq_dir {
                let frames_count: u64 = flag_or("--frames", "60").parse().unwrap_or(60);
                let fps: f64 = flag_or("--fps", "30").parse::<f64>().unwrap_or(30.0).max(1.0);
                let frame_fmt = match frame_format_for(&out, explicit_fmt.as_deref()) {
                    Ok(fmt) => fmt,
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::FAILURE;
                    }
                };
                let mut pixmaps = Vec::with_capacity(frames_count as usize);
                for f in 0..frames_count {
                    let t = (f as f64) / fps;
                    match engine.render_frame_at_with_index(handle, t, f as u32) {
                        Ok(px) => pixmaps.push(px),
                        Err(e) => {
                            eprintln!("sequence render error at frame {f}: {e}");
                            return ExitCode::FAILURE;
                        }
                    }
                }
                match kine_server::write_image_sequence_dir(&out, &pixmaps, frame_fmt) {
                    Ok(paths) => {
                        eprintln!(
                            "wrote {} frames ({:?}) into {}",
                            paths.len(),
                            frame_fmt,
                            out.display()
                        );
                        return ExitCode::SUCCESS;
                    }
                    Err(e) => {
                        eprintln!("write sequence error: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }

            let video_container = explicit_fmt
                .as_deref()
                .and_then(VideoContainer::from_extension)
                .or_else(|| VideoContainer::from_path(&out));

            if let Some(container) = video_container {
                let mut opts = VideoOptions::default();
                opts.container = container;
                opts.frames = flag_or("--frames", "60").parse().unwrap_or(60);
                opts.fps_num = flag_or("--fps", "30").parse().unwrap_or(30);
                opts.quantizer = flag_or("--quality", "100").parse().unwrap_or(100);
                opts.speed = flag_or("--speed", "9").parse().unwrap_or(9);
                opts.keyframe_interval = flag_or("--keyframe-interval", "60")
                    .parse()
                    .unwrap_or(60);
                opts.bitrate = flag_or("--bitrate", "0").parse().unwrap_or(0);

                let requested = flag_or("--encoder", "av1");
                match requested.to_ascii_lowercase().as_str() {
                    "av1" | "software" => opts.encoder = VideoEncoder::Av1Software,
                    #[cfg(feature = "hwencode")]
                    "h264-hw" | "hardware" | "h264" => {
                        opts.encoder = VideoEncoder::H264Hardware
                    }
                    #[cfg(not(feature = "hwencode"))]
                    "h264-hw" | "hardware" | "h264" => {
                        eprintln!(
                            "this build has no hardware encoder; rebuild with --features hwencode"
                        );
                        return ExitCode::FAILURE;
                    }
                    #[cfg(feature = "zero-copy")]
                    "h264-zero-copy" | "zero-copy" | "zerocopy" => {
                        opts.encoder = VideoEncoder::H264ZeroCopy
                    }
                    #[cfg(not(feature = "zero-copy"))]
                    "h264-zero-copy" | "zero-copy" | "zerocopy" => {
                        eprintln!(
                            "this build has no zero-copy encoder; rebuild with --features zero-copy"
                        );
                        return ExitCode::FAILURE;
                    }
                    other => {
                        eprintln!(
                            "unknown encoder {other:?} (expected av1, h264-hw or h264-zero-copy)"
                        );
                        return ExitCode::FAILURE;
                    }
                }

                if !opts.encoder.is_available() {
                    eprintln!(
                        "encoder {} is not available on this host: it needs a GPU adapter with \
                         hardware video encode support (Vulkan Video on Linux, VideoToolbox on macOS)",
                        opts.encoder.name(),
                    );
                    return ExitCode::FAILURE;
                }

                eprintln!(
                    "rendering {} frames at {}/{} fps ({:.2}s) -> {} ({})",
                    opts.frames,
                    opts.fps_num,
                    opts.fps_den,
                    opts.duration_seconds(),
                    opts.container.name(),
                    opts.encoder.name()
                );
                match kine_server::render_video(&mut engine, handle, &opts) {
                    Ok(report) => {
                        if is_stdout {
                            if let Err(e) = std::io::stdout().write_all(&report.bytes) {
                                eprintln!("stdout write error: {e}");
                                return ExitCode::FAILURE;
                            }
                        } else if let Err(e) = write_output(&out, &report.bytes) {
                            eprintln!("write {}: {e}", out.display());
                            return ExitCode::FAILURE;
                        }
                        eprintln!(
                            "wrote {} — {}x{}, {} frames, {} samples, {} keyframe(s), {} distinct frame(s)",
                            out_raw,
                            report.width,
                            report.height,
                            report.frames,
                            report.samples,
                            report.keyframes,
                            report.distinct_frames
                        );
                        if report.audio_frames > 0 {
                            eprintln!(
                                "audio: {} frames, {} samples per channel",
                                report.audio_frames, report.audio_samples
                            );
                        }
                        if report.distinct_frames <= 1 {
                            eprintln!(
                                "note: every frame is identical — the scene has no animation and \
                                 no time-varying effect layer"
                            );
                        }
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("video render error: {e}");
                        ExitCode::FAILURE
                    }
                }
            } else {
                let frame: u64 = flag_or("--frame", "0").parse().unwrap_or(0);
                let format = match frame_format_for(&out, explicit_fmt.as_deref()) {
                    Ok(fmt) => fmt,
                    Err(e) => {
                        eprintln!("{e}");
                        return ExitCode::FAILURE;
                    }
                };
                match engine.render_frame_format(handle, frame, format) {
                    Ok(bytes) => {
                        if is_stdout {
                            if let Err(e) = std::io::stdout().write_all(&bytes) {
                                eprintln!("stdout write error: {e}");
                                return ExitCode::FAILURE;
                            }
                        } else if let Err(e) = write_output(&out, &bytes) {
                            eprintln!("write {}: {e}", out.display());
                            return ExitCode::FAILURE;
                        }
                        eprintln!("wrote {out_raw}");
                        ExitCode::SUCCESS
                    }
                    Err(e) => {
                        eprintln!("render error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
        }

        "inspect" => {
            let Some(scene_path) = flag("--scene") else {
                eprintln!("inspect requires --scene\n{}", usage());
                return ExitCode::FAILURE;
            };
            let handle = load(&mut engine, &scene_path);
            match engine.inspect(handle) {
                Ok(report) => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap());
                    if report.has_errors() {
                        ExitCode::from(2)
                    } else {
                        ExitCode::SUCCESS
                    }
                }
                Err(e) => {
                    eprintln!("inspect error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "check" => {
            let Some(scene_path) = flag("--scene") else {
                eprintln!("check requires --scene\n{}", usage());
                return ExitCode::FAILURE;
            };
            let handle = load(&mut engine, &scene_path);
            let inspect_report = match engine.inspect(handle) {
                Ok(r) => r,
                Err(e) => {
                    eprintln!("check inspect error: {e}");
                    return ExitCode::FAILURE;
                }
            };
            if let Err(e) = kine_server::mix_scene_audio(&engine, handle, 0.5) {
                eprintln!("check audio error: {e}");
                return ExitCode::from(2);
            }
            if inspect_report.has_errors() {
                println!("{}", serde_json::to_string_pretty(&inspect_report).unwrap());
                return ExitCode::from(2);
            }
            if let Some(golden) = flag("--against") {
                let frame: u64 = flag_or("--frame", "0").parse().unwrap_or(0);
                let threshold: f64 = flag_or("--threshold", "0.001").parse().unwrap_or(0.001);
                let golden = resolve_output(Path::new(&golden), &platform);
                let approved = read_file_bytes(&golden.to_string_lossy());
                match engine.snapshot_diff(handle, frame, &approved, threshold) {
                    Ok(diff) => {
                        println!(
                            "{}",
                            serde_json::to_string_pretty(&serde_json::json!({
                                "inspect": inspect_report,
                                "snapshot": diff,
                            }))
                            .unwrap()
                        );
                        return if diff.passed {
                            ExitCode::SUCCESS
                        } else {
                            ExitCode::from(3)
                        };
                    }
                    Err(e) => {
                        eprintln!("check snapshot error: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            println!("{}", serde_json::to_string_pretty(&inspect_report).unwrap());
            ExitCode::SUCCESS
        }

        "snapshot" => {
            let (Some(scene_path), Some(golden)) = (flag("--scene"), flag("--against")) else {
                eprintln!("snapshot requires --scene and --against\n{}", usage());
                return ExitCode::FAILURE;
            };
            let frame: u64 = flag_or("--frame", "0").parse().unwrap_or(0);
            let threshold: f64 = flag_or("--threshold", "0.001").parse().unwrap_or(0.001);
            let golden = resolve_output(Path::new(&golden), &platform);
            let approved = read_file_bytes(&golden.to_string_lossy());
            let handle = load(&mut engine, &scene_path);
            match engine.snapshot_diff(handle, frame, &approved, threshold) {
                Ok(report) => {
                    println!("{}", serde_json::to_string_pretty(&report).unwrap());
                    if report.passed {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::from(3)
                    }
                }
                Err(e) => {
                    eprintln!("snapshot error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        "studio" => {
            let workspace = flag("--workspace")
                .map(PathBuf::from)
                .or_else(|| std::env::current_dir().ok());
            let mut studio_state = kine_studio::StudioState::new(workspace);
            studio_state.engine.set_backend(engine.backend());
            let _ = studio_state.render_current_frame();

            if let Some(scene_path) = flag("--scene") {
                if let Err(e) = studio_state.load_scene_file(Path::new(&scene_path)) {
                    eprintln!("studio load error ({scene_path}): {e}");
                    return ExitCode::FAILURE;
                }
            }

            if has_flag("--headless-check") {
                let look = kine_studio::studio_look(&mut studio_state);
                println!(
                    "{}",
                    serde_json::to_string_pretty(&serde_json::json!({
                        "ok": look.ok,
                        "stage": look.stage,
                        "summary": look.summary,
                        "width": studio_state.scene_def.width,
                        "height": studio_state.scene_def.height,
                        "duration": studio_state.duration,
                        "tracks": studio_state.build_timeline_tracks().len(),
                        "compositions": studio_state.compositions.len(),
                    }))
                    .unwrap()
                );
                return if look.ok {
                    ExitCode::SUCCESS
                } else {
                    ExitCode::FAILURE
                };
            }

            #[cfg(feature = "studio-ui")]
            {
                match kine_studio::run_studio(studio_state) {
                    Ok(()) => ExitCode::SUCCESS,
                    Err(e) => {
                        eprintln!("studio error: {e}");
                        ExitCode::FAILURE
                    }
                }
            }
            #[cfg(not(feature = "studio-ui"))]
            {
                eprintln!("kine built without `studio-ui` feature; use `--headless-check` or rebuild with default features.");
                ExitCode::FAILURE
            }
        }

        "serve" => {
            let addr = flag_or("--listen", "127.0.0.1:7700");
            match kine_server::rpc::serve(&addr, &mut engine) {
                Ok(()) => ExitCode::SUCCESS,
                Err(e) => {
                    eprintln!("serve error: {e}");
                    ExitCode::FAILURE
                }
            }
        }

        _ => {
            eprintln!("{}", usage());
            ExitCode::FAILURE
        }
    }
}

fn write_audio_file(
    out: &Path,
    clip: &kine_audio::AudioClip,
    bits: &str,
) -> Result<(), kine_audio::AudioError> {
    let ext = out
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("wav")
        .to_ascii_lowercase();
    match ext.as_str() {
        "flac" => {
            let b: u16 = if bits == "24" { 24 } else { 16 };
            kine_audio::write_flac(out, clip, b)
        }
        "aiff" | "aif" => kine_audio::write_aiff(out, clip),
        _ => {
            let fmt = match bits {
                "24" => WavSampleFormat::Pcm24,
                "32f" | "32" | "float" => WavSampleFormat::Float32,
                _ => WavSampleFormat::Pcm16,
            };
            kine_audio::write_wav_with_format(out, clip, fmt)
        }
    }
}

fn read_file(path: &str) -> String {
    String::from_utf8_lossy(&read_file_bytes(path)).into_owned()
}

fn read_file_bytes(path: &str) -> Vec<u8> {
    std::fs::read(path).unwrap_or_else(|e| {
        eprintln!("read {path}: {e}");
        std::process::exit(1);
    })
}

/// Resolve the still-frame format from `--format` or the output extension.
///
/// An explicit but unsupported format is an error: writing PNG bytes into a
/// `.txt` file (or any other unknown extension) would look like success.
fn frame_format_for(out: &Path, explicit: Option<&str>) -> Result<FrameFormat, String> {
    if let Some(name) = explicit {
        return FrameFormat::from_extension(name)
            .ok_or_else(|| format!("unknown frame format {name:?} (png, qoi, bmp, ppm, pam, tga)"));
    }
    match out.extension().and_then(|e| e.to_str()) {
        Some(ext) => FrameFormat::from_extension(ext)
            .ok_or_else(|| format!("unknown frame format {ext:?} (png, qoi, bmp, ppm, pam, tga)")),
        None => Ok(FrameFormat::Png),
    }
}

fn load(engine: &mut Engine, path: &str) -> SceneHandle {
    let source = read_file(path);
    let base = Path::new(path)
        .parent()
        .filter(|p| !p.as_os_str().is_empty())
        .map(Path::to_path_buf)
        .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));

    // The engine handles both JSON and TypeScript; routing scripts through it
    // keeps `module("name", props)` working, since it discovers `<base>/Modules`
    // before evaluating the scene body.
    engine
        .load_scene_json_with_base(&source, &base)
        .unwrap_or_else(|e| {
            eprintln!("scene load error ({path}): {e}");
            std::process::exit(1);
        })
}

fn starter_scene_json() -> String {
    let def = serde_json::json!({
        "width": 1280,
        "height": 720,
        "root": {
            "kind": "container",
            "size": { "width": 1280, "height": 720 },
            "children": [
                {
                    "kind": "rect",
                    "fill": "#121422",
                    "position": "absolute",
                    "inset": { "top": 0, "left": 0, "right": 0, "bottom": 0 }
                },
                {
                    "kind": "text",
                    "content": "Kine — video as code",
                    "fontFamily": "JetBrains Mono",
                    "fontSize": 48,
                    "color": "#ebf0ff",
                    "align": "center",
                    "size": { "width": 1280, "height": 80 },
                    "position": "absolute",
                    "inset": { "top": 320, "left": 0 },
                    "animate": {
                        "offsetY": { "from": 60, "to": 0, "start": 0.0, "end": 0.8, "easing": "spring" }
                    }
                },
                {
                    "kind": "rect",
                    "fill": "#f2b880",
                    "cornerRadius": 3,
                    "position": "absolute",
                    "size": { "width": 96, "height": 6 },
                    "inset": { "top": 280, "left": 592 },
                    "animate": {
                        "width": { "from": 0, "to": 96, "start": 0.2, "end": 1.0, "easing": "easeOut" }
                    }
                }
            ]
        }
    });
    serde_json::to_string_pretty(&def).unwrap()
}
