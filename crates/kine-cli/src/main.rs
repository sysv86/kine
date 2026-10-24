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
                  [--bridge <127.0.0.1:7701>] [--no-bridge] [--auto-save] [--play]
                  [--selection] [--context] [--headless-check]
                                              launch Kine Studio or query live selection/context
  kine agent      <subcommand> [options]      control a live Kine Studio window (or --scene file)
                  status | context | selection [--prompt] | look [--out <png>] |
                  tree | timeline | inspect [--node <id>] | select [--node <id>|--clear] |
                  seek --time <s> | frame --frame <n> | play | pause | toggle | stop |
                  step [--delta <n>] | transport [...] | resize --width <w> --height <h> |
                  add [...] | delete [--node <id>] | duplicate [--node <id>] |
                  reorder [--node <id>] --delta <n> | visibility [--node <id>] |
                  align [--node <id>] [--h <mode>] [--v <mode>] |
                  set-text [--node <id>] --text <str> | set-style [--node <id>] [...] |
                  transform [--node <id>] [...] | shader [--node <id>] [...] |
                  audio [...] | animate [--node <id>] --prop <p> [...] |
                  keyframe [--node <id>] --prop <p> --time <s> --value <v> |
                  delete-keyframe [--node <id>] --prop <p> --time <s> |
                  delete-anim [--node <id>] --prop <p> | shift [--node <id>] --delta <s> |
                  split [--node <id>] | undo | redo | history | load --scene <path> |
                  reload | apply (--file <f>|--inline <s>) | save [--out <path>] |
                  export [--format <fmt>] [--out <path>] | call --tool <name> [--args <json>] |
                  batch (--file <f>|--inline <json>) | schema | watch
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

            if has_flag("--selection") || has_flag("--context") {
                let addr = flag("--bridge")
                    .or_else(|| flag("--addr"))
                    .unwrap_or_else(|| kine_studio::discover_bridge_addr(workspace.as_deref()));
                let tool = if has_flag("--selection") {
                    "studio_selection"
                } else {
                    "studio_status"
                };
                match kine_studio::bridge_call(&addr, tool, serde_json::json!({})) {
                    Ok(res) => {
                        println!("{}", serde_json::to_string_pretty(&res).unwrap());
                        return if res.ok { ExitCode::SUCCESS } else { ExitCode::FAILURE };
                    }
                    Err(e) => {
                        eprintln!("studio bridge error: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }

            let mut studio_state = kine_studio::StudioState::new(workspace);
            studio_state.engine.set_backend(engine.backend());
            let _ = studio_state.render_current_frame();

            if let Some(scene_path) = flag("--scene") {
                if let Err(e) = studio_state.load_scene_file(Path::new(&scene_path)) {
                    eprintln!("studio load error ({scene_path}): {e}");
                    return ExitCode::FAILURE;
                }
            }

            if has_flag("--auto-save") {
                studio_state.settings.auto_save_scene = true;
            }
            if has_flag("--play") {
                studio_state.is_playing = true;
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

            if !has_flag("--no-bridge") {
                let explicit_addr = flag("--bridge");
                match studio_state.start_bridge_server(explicit_addr.as_deref()) {
                    Ok(bound) => eprintln!("kine studio: CLI agent bridge listening on {bound}"),
                    Err(e) => eprintln!("kine studio: warning: could not bind CLI bridge: {e}"),
                }
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

        "agent" | "bridge" => run_agent_cli(&args[1..], engine.backend()),

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


fn run_agent_cli(sub_args: &[String], backend: RenderBackend) -> ExitCode {
    let subcmd = sub_args.first().map(String::as_str).unwrap_or("status");
    if subcmd == "--help" || subcmd == "-h" || subcmd == "help" {
        print!("{}", usage());
        return ExitCode::SUCCESS;
    }

    let flag = |name: &str| -> Option<String> {
        sub_args
            .iter()
            .position(|a| a == name)
            .and_then(|i| sub_args.get(i + 1))
            .cloned()
    };
    let has_flag = |name: &str| -> bool { sub_args.iter().any(|a| a == name) };
    let flag_f64 = |name: &str| -> Option<f64> { flag(name).and_then(|s| s.parse::<f64>().ok()) };
    let flag_u64 = |name: &str| -> Option<u64> {
        flag(name).and_then(|s| s.trim_start_matches('#').parse::<u64>().ok())
    };
    let flag_i64 = |name: &str| -> Option<i64> { flag(name).and_then(|s| s.parse::<i64>().ok()) };
    let flag_bool = |name: &str| -> Option<bool> {
        match flag(name).as_deref() {
            Some("true" | "1" | "yes" | "on") => Some(true),
            Some("false" | "0" | "no" | "off") => Some(false),
            _ => {
                if has_flag(name) {
                    Some(true)
                } else {
                    None
                }
            }
        }
    };

    let workspace = flag("--workspace")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let addr = flag("--addr")
        .or_else(|| flag("--bridge"))
        .unwrap_or_else(|| kine_studio::discover_bridge_addr(workspace.as_deref()));

    if subcmd == "schema" || subcmd == "tools" {
        println!(
            "{}",
            serde_json::to_string_pretty(&kine_studio::agent_tools_json_schema()).unwrap()
        );
        return ExitCode::SUCCESS;
    }

    if subcmd == "watch" || subcmd == "subscribe" {
        eprintln!("watching live Kine Studio events on {addr} (Ctrl-C to stop)...");
        match kine_studio::bridge_watch(&addr, |ev| {
            println!("{}", serde_json::to_string(&ev).unwrap_or_default());
            true
        }) {
            Ok(()) => return ExitCode::SUCCESS,
            Err(e) => {
                eprintln!("agent watch error: {e}");
                return ExitCode::FAILURE;
            }
        }
    }

    let node_id_opt = flag_u64("--node").or_else(|| flag_u64("--id"));
    let undo_label_opt = flag("--undo-label").or_else(|| flag("--reason"));

    let mut map = serde_json::Map::new();
    if let Some(nid) = node_id_opt {
        map.insert("nodeId".to_string(), serde_json::json!(nid));
    }
    if let Some(lbl) = undo_label_opt {
        map.insert("undoLabel".to_string(), serde_json::json!(lbl));
    }

    let tool_name: String = match subcmd {
        "status" | "context" => "studio_status".to_string(),
        "selection" => {
            if let Some(req) = flag("--request").or_else(|| flag("--prompt")) {
                map.insert("request".to_string(), serde_json::json!(req));
            }
            "studio_selection".to_string()
        }
        "look" => {
            if let Some(out) = flag("--out") {
                map.insert("outPath".to_string(), serde_json::json!(out));
            }
            "studio_look".to_string()
        }
        "tree" | "layers" => "studio_tree".to_string(),
        "timeline" => "studio_timeline".to_string(),
        "inspect" => "studio_inspect".to_string(),
        "select" => {
            if has_flag("--clear") {
                map.insert("clear".to_string(), serde_json::json!(true));
            }
            "studio_select".to_string()
        }
        "seek" => {
            if let Some(t) = flag_f64("--time").or_else(|| flag_f64("--sec")) {
                map.insert("time".to_string(), serde_json::json!(t));
            }
            "studio_seek".to_string()
        }
        "frame" => {
            if let Some(f) = flag_u64("--frame") {
                map.insert("frame".to_string(), serde_json::json!(f));
            }
            "studio_frame".to_string()
        }
        "play" | "pause" | "toggle" | "stop" => {
            map.insert("action".to_string(), serde_json::json!(subcmd));
            "studio_transport".to_string()
        }
        "step" => {
            map.insert("action".to_string(), serde_json::json!("step"));
            if let Some(d) = flag_i64("--delta").or_else(|| flag_i64("--frames")) {
                map.insert("deltaFrames".to_string(), serde_json::json!(d));
            }
            "studio_transport".to_string()
        }
        "transport" => {
            if let Some(act) = flag("--action") {
                map.insert("action".to_string(), serde_json::json!(act));
            }
            if let Some(fps) = flag_f64("--fps") {
                map.insert("fps".to_string(), serde_json::json!(fps));
            }
            if let Some(rate) = flag_f64("--rate") {
                map.insert("rate".to_string(), serde_json::json!(rate));
            }
            if let Some(lp) = flag_bool("--loop") {
                map.insert("loop".to_string(), serde_json::json!(lp));
            }
            if let Some(m) = flag_bool("--muted") {
                map.insert("muted".to_string(), serde_json::json!(m));
            }
            if let Some(v) = flag_f64("--volume") {
                map.insert("volume".to_string(), serde_json::json!(v));
            }
            if let Some(s) = flag_bool("--snap") {
                map.insert("snap".to_string(), serde_json::json!(s));
            }
            if let Some(r) = flag_bool("--ripple") {
                map.insert("ripple".to_string(), serde_json::json!(r));
            }
            "studio_transport".to_string()
        }
        "resize" => {
            if let Some(w) = flag_u64("--width") {
                map.insert("width".to_string(), serde_json::json!(w));
            }
            if let Some(h) = flag_u64("--height") {
                map.insert("height".to_string(), serde_json::json!(h));
            }
            "studio_resize_canvas".to_string()
        }
        "add" | "add-node" => {
            if let Some(raw_json) = flag("--json") {
                match serde_json::from_str::<serde_json::Value>(&raw_json) {
                    Ok(v) => {
                        map.insert("node".to_string(), v);
                    }
                    Err(e) => {
                        eprintln!("invalid --json for add: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            if let Some(cat) = flag("--catalog").or_else(|| flag("--preset")) {
                map.insert("catalogId".to_string(), serde_json::json!(cat));
            }
            if let Some(m) = flag("--module") {
                map.insert("module".to_string(), serde_json::json!(m));
            }
            if let Some(props_str) = flag("--props") {
                if let Ok(p) = serde_json::from_str::<serde_json::Value>(&props_str) {
                    map.insert("props".to_string(), p);
                }
            }
            if let Some(parent) = flag_u64("--parent") {
                map.insert("parentId".to_string(), serde_json::json!(parent));
            }
            if let Some(idx) = flag_u64("--index") {
                map.insert("index".to_string(), serde_json::json!(idx));
            }
            if let Some(k) = flag("--kind") {
                map.insert("kind".to_string(), serde_json::json!(k));
            }
            if let Some(t) = flag("--text").or_else(|| flag("--content")) {
                map.insert("text".to_string(), serde_json::json!(t));
            }
            if let Some(f) = flag("--fill") {
                map.insert("fill".to_string(), serde_json::json!(f));
            }
            if let Some(c) = flag("--color") {
                map.insert("color".to_string(), serde_json::json!(c));
            }
            if let Some(x) = flag_f64("--x") {
                map.insert("x".to_string(), serde_json::json!(x));
            }
            if let Some(y) = flag_f64("--y") {
                map.insert("y".to_string(), serde_json::json!(y));
            }
            if let Some(w) = flag_f64("--width") {
                map.insert("width".to_string(), serde_json::json!(w));
            }
            if let Some(h) = flag_f64("--height") {
                map.insert("height".to_string(), serde_json::json!(h));
            }
            if let Some(r) = flag_f64("--radius") {
                map.insert("radius".to_string(), serde_json::json!(r));
            }
            if let Some(cr) = flag_f64("--corner-radius") {
                map.insert("cornerRadius".to_string(), serde_json::json!(cr));
            }
            if let Some(op) = flag_f64("--opacity") {
                map.insert("opacity".to_string(), serde_json::json!(op));
            }
            if let Some(fs) = flag_f64("--font-size") {
                map.insert("fontSize".to_string(), serde_json::json!(fs));
            }
            if let Some(src_uri) = flag("--src") {
                map.insert("src".to_string(), serde_json::json!(src_uri));
            }
            if let Some(sh) = flag("--shader") {
                map.insert("shader".to_string(), serde_json::json!(sh));
            }
            if let Some(d) = flag("--d") {
                map.insert("d".to_string(), serde_json::json!(d));
            }
            "studio_add_node".to_string()
        }
        "delete" | "delete-node" => "studio_delete_node".to_string(),
        "duplicate" | "duplicate-node" => "studio_duplicate_node".to_string(),
        "reorder" => {
            if let Some(d) = flag_i64("--delta") {
                map.insert("delta".to_string(), serde_json::json!(d));
            }
            "studio_reorder_node".to_string()
        }
        "visibility" => {
            if let Some(v) = flag_bool("--visible") {
                map.insert("visible".to_string(), serde_json::json!(v));
            }
            "studio_toggle_visibility".to_string()
        }
        "align" => {
            if let Some(h) = flag("--h").or_else(|| flag("--horizontal")) {
                map.insert("horizontal".to_string(), serde_json::json!(h));
            }
            if let Some(v) = flag("--v").or_else(|| flag("--vertical")) {
                map.insert("vertical".to_string(), serde_json::json!(v));
            }
            "studio_align_node".to_string()
        }
        "set-text" => {
            if let Some(t) = flag("--text").or_else(|| flag("--content")) {
                map.insert("text".to_string(), serde_json::json!(t));
            }
            "studio_set_text".to_string()
        }
        "set-style" | "style" => {
            let mut changes = serde_json::Map::new();
            if let Some(raw_json) = flag("--json") {
                match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw_json) {
                    Ok(parsed) => changes.extend(parsed),
                    Err(e) => {
                        eprintln!("invalid --json for set-style: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            if let Some(v) = flag("--fill") {
                changes.insert("fill".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag("--color") {
                changes.insert("color".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag_f64("--opacity") {
                changes.insert("opacity".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag_f64("--corner-radius") {
                changes.insert("cornerRadius".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag_f64("--radius") {
                changes.insert("radius".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag("--stroke-color") {
                changes.insert("strokeColor".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag_f64("--stroke-width") {
                changes.insert("strokeWidth".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag("--shadow-color") {
                changes.insert("shadowColor".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag_f64("--shadow-blur") {
                changes.insert("shadowBlur".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag("--font-family") {
                changes.insert("fontFamily".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag_f64("--font-size") {
                changes.insert("fontSize".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag_u64("--font-weight") {
                changes.insert("fontWeight".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag("--align") {
                changes.insert("align".to_string(), serde_json::json!(v));
            }
            if let Some(v) = flag("--blend") {
                changes.insert("blend".to_string(), serde_json::json!(v));
            }
            map.insert("changes".to_string(), serde_json::Value::Object(changes));
            "studio_set_style".to_string()
        }
        "transform" => {
            for (cli_flag, key) in [
                ("--x", "x"),
                ("--y", "y"),
                ("--width", "width"),
                ("--height", "height"),
                ("--tx", "translateX"),
                ("--ty", "translateY"),
                ("--scale", "scale"),
                ("--scale-x", "scaleX"),
                ("--scale-y", "scaleY"),
                ("--rotation", "rotation"),
                ("--dx", "dx"),
                ("--dy", "dy"),
                ("--dw", "dw"),
                ("--dh", "dh"),
                ("--dtx", "dtx"),
                ("--dty", "dty"),
                ("--dscale", "dscale"),
                ("--drot", "drot"),
            ] {
                if let Some(val) = flag_f64(cli_flag) {
                    map.insert(key.to_string(), serde_json::json!(val));
                }
            }
            "studio_transform".to_string()
        }
        "shader" => {
            if let Some(preset) = flag("--preset") {
                map.insert("preset".to_string(), serde_json::json!(preset));
            } else if let Some(file) = flag("--file") {
                map.insert("shader".to_string(), serde_json::json!(read_file(&file)));
            } else if let Some(inline) = flag("--inline").or_else(|| flag("--shader")) {
                map.insert("shader".to_string(), serde_json::json!(inline));
            }
            if let Some(b) = flag("--blend") {
                map.insert("blend".to_string(), serde_json::json!(b));
            }
            if let Some(u_str) = flag("--uniforms") {
                if let Ok(u_val) = serde_json::from_str::<serde_json::Value>(&u_str) {
                    map.insert("uniforms".to_string(), u_val);
                }
            }
            "studio_set_shader".to_string()
        }
        "audio" => {
            if let Some(act) = flag("--action") {
                map.insert("action".to_string(), serde_json::json!(act));
            }
            if let Some(idx) = flag_u64("--index").or_else(|| flag_u64("--track")) {
                map.insert("trackIndex".to_string(), serde_json::json!(idx));
            }
            if let Some(src_uri) = flag("--src") {
                map.insert("src".to_string(), serde_json::json!(src_uri));
            }
            if let Some(g) = flag_f64("--gain-db") {
                map.insert("gainDb".to_string(), serde_json::json!(g));
            }
            if let Some(p) = flag_f64("--pan") {
                map.insert("pan".to_string(), serde_json::json!(p));
            }
            if let Some(s) = flag_f64("--start") {
                map.insert("start".to_string(), serde_json::json!(s));
            }
            if let Some(te) = flag_f64("--trim-end") {
                map.insert("trimEnd".to_string(), serde_json::json!(te));
            }
            if let Some(fi) = flag_f64("--fade-in") {
                map.insert("fadeIn".to_string(), serde_json::json!(fi));
            }
            if let Some(fo) = flag_f64("--fade-out") {
                map.insert("fadeOut".to_string(), serde_json::json!(fo));
            }
            if let Some(m) = flag_bool("--muted") {
                map.insert("muted".to_string(), serde_json::json!(m));
            }
            "studio_audio".to_string()
        }
        "animate" | "add-anim" | "update-anim" => {
            if let Some(p) = flag("--prop").or_else(|| flag("--property")) {
                map.insert("property".to_string(), serde_json::json!(p));
            }
            if let Some(f) = flag_f64("--from") {
                map.insert("from".to_string(), serde_json::json!(f));
            }
            if let Some(t) = flag_f64("--to") {
                map.insert("to".to_string(), serde_json::json!(t));
            }
            if let Some(s) = flag_f64("--start") {
                map.insert("start".to_string(), serde_json::json!(s));
            }
            if let Some(e) = flag_f64("--end") {
                map.insert("end".to_string(), serde_json::json!(e));
            }
            if let Some(ease) = flag("--easing") {
                map.insert("easing".to_string(), serde_json::json!(ease));
            }
            "studio_add_animation".to_string()
        }
        "keyframe" | "add-keyframe" => {
            if let Some(p) = flag("--prop").or_else(|| flag("--property")) {
                map.insert("property".to_string(), serde_json::json!(p));
            }
            if let Some(t) = flag_f64("--time") {
                map.insert("time".to_string(), serde_json::json!(t));
            }
            if let Some(v) = flag_f64("--value") {
                map.insert("value".to_string(), serde_json::json!(v));
            }
            if let Some(ease) = flag("--easing") {
                map.insert("easing".to_string(), serde_json::json!(ease));
            }
            "studio_add_keyframe".to_string()
        }
        "delete-keyframe" => {
            if let Some(p) = flag("--prop").or_else(|| flag("--property")) {
                map.insert("property".to_string(), serde_json::json!(p));
            }
            if let Some(t) = flag_f64("--time") {
                map.insert("time".to_string(), serde_json::json!(t));
            }
            "studio_delete_keyframe".to_string()
        }
        "delete-anim" | "delete-animation" => {
            if let Some(p) = flag("--prop").or_else(|| flag("--property")) {
                map.insert("property".to_string(), serde_json::json!(p));
            }
            "studio_delete_animation".to_string()
        }
        "shift" => {
            if let Some(d) = flag_f64("--delta") {
                map.insert("delta".to_string(), serde_json::json!(d));
            }
            if let Some(r) = flag_bool("--ripple") {
                map.insert("ripple".to_string(), serde_json::json!(r));
            }
            "studio_shift_time".to_string()
        }
        "split" => {
            if let Some(t) = flag_f64("--time") {
                map.insert("time".to_string(), serde_json::json!(t));
            }
            "studio_split".to_string()
        }
        "undo" => "studio_undo".to_string(),
        "redo" => "studio_redo".to_string(),
        "history" => "studio_history".to_string(),
        "load" => {
            if let Some(p) = flag("--scene").or_else(|| flag("--path")) {
                map.insert("path".to_string(), serde_json::json!(p));
            }
            "studio_load_scene".to_string()
        }
        "reload" => "studio_reload".to_string(),
        "apply" => {
            let source = match (flag("--file"), flag("--inline")) {
                (Some(path), _) => read_file(&path),
                (None, Some(code)) => code,
                _ => {
                    eprintln!("agent apply requires --file <scene.ts|json> or --inline <source>");
                    return ExitCode::FAILURE;
                }
            };
            map.insert("source".to_string(), serde_json::json!(source));
            "studio_apply_source".to_string()
        }
        "save" => {
            if let Some(out) = flag("--out").or_else(|| flag("--path")) {
                map.insert("path".to_string(), serde_json::json!(out));
            }
            "studio_save_scene".to_string()
        }
        "export" => {
            if let Some(fmt) = flag("--format") {
                map.insert("format".to_string(), serde_json::json!(fmt));
            }
            if let Some(out) = flag("--out") {
                map.insert("out".to_string(), serde_json::json!(out));
            }
            "studio_export".to_string()
        }
        "call" => {
            let Some(t) = flag("--tool") else {
                eprintln!("agent call requires --tool <studio_*>");
                return ExitCode::FAILURE;
            };
            if let Some(raw_args) = flag("--args") {
                match serde_json::from_str::<serde_json::Map<String, serde_json::Value>>(&raw_args) {
                    Ok(parsed) => map.extend(parsed),
                    Err(e) => {
                        eprintln!("invalid --args JSON: {e}");
                        return ExitCode::FAILURE;
                    }
                }
            }
            t
        }
        "batch" => {
            let raw = match (flag("--file"), flag("--inline")) {
                (Some(path), _) => read_file(&path),
                (None, Some(inline)) => inline,
                _ => {
                    eprintln!("agent batch requires --file <calls.json> or --inline <json>");
                    return ExitCode::FAILURE;
                }
            };
            match serde_json::from_str::<serde_json::Value>(&raw) {
                Ok(serde_json::Value::Array(arr)) => {
                    map.insert("calls".to_string(), serde_json::Value::Array(arr));
                }
                Ok(obj) => {
                    if let Some(arr) = obj.get("calls").and_then(|v| v.as_array()) {
                        map.insert("calls".to_string(), serde_json::Value::Array(arr.clone()));
                    } else {
                        eprintln!("agent batch JSON must be an array or {{"calls": [...]}}");
                        return ExitCode::FAILURE;
                    }
                }
                Err(e) => {
                    eprintln!("invalid batch JSON: {e}");
                    return ExitCode::FAILURE;
                }
            }
            "studio_batch".to_string()
        }
        other => {
            if other.starts_with("studio_") {
                other.to_string()
            } else {
                eprintln!("unknown agent subcommand {other:?}
{}", usage());
                return ExitCode::FAILURE;
            }
        }
    };

    let args_val = serde_json::Value::Object(map);

    // Offline / headless direct scene mode when `--scene` is supplied (except for `agent load --scene`)
    if subcmd != "load" {
        if let Some(scene_path) = flag("--scene") {
            let mut state = kine_studio::StudioState::new(workspace);
            state.engine.set_backend(backend);
            if let Err(e) = state.load_scene_file(Path::new(&scene_path)) {
                eprintln!("agent load error ({scene_path}): {e}");
                return ExitCode::FAILURE;
            }
            let res = kine_studio::dispatch_agent_tool(&mut state, &tool_name, &args_val);
            if res.ok && (has_flag("--save") || flag("--save-out").is_some()) {
                let save_target = flag("--save-out").unwrap_or(scene_path);
                if let Err(e) = state.save_scene_def_to_disk(Some(Path::new(&save_target))) {
                    eprintln!("agent save error ({save_target}): {e}");
                    return ExitCode::FAILURE;
                }
            }
            if subcmd == "selection" && has_flag("--prompt") {
                if let Some(p) = res.payload.get("agentContextPrompt").and_then(|v| v.as_str()) {
                    println!("{p}");
                    return ExitCode::SUCCESS;
                }
            }
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
            return if res.ok { ExitCode::SUCCESS } else { ExitCode::FAILURE };
        }
    }

    // Live TCP Bridge mode against a running Kine Studio window
    match kine_studio::bridge_call(&addr, &tool_name, args_val) {
        Ok(res) => {
            if subcmd == "selection" && has_flag("--prompt") {
                if let Some(p) = res.payload.get("agentContextPrompt").and_then(|v| v.as_str()) {
                    println!("{p}");
                    return ExitCode::SUCCESS;
                }
            }
            println!("{}", serde_json::to_string_pretty(&res).unwrap());
            if res.ok {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}
