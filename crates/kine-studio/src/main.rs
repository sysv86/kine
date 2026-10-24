//! `kine-studio` — Standalone desktop binary for Kine Studio (Xilem UI).

use kine_render::RenderBackend;
use kine_studio::StudioState;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

fn usage() -> &'static str {
    "kine-studio — Native Rust Xilem visual editor, NLE timeline & AI Agent Studio for Kine

USAGE:
  kine-studio [--scene <scene.ts|scene.json>]
              [--workspace <dir>]
              [--backend auto|gpu|cpu]
              [--play]
              [--headless-check]
"
}

fn main() -> ExitCode {
    if cfg!(debug_assertions) {
        eprintln!(
            "kine-studio: warning: this is an unoptimized debug build; \
             preview playback will stutter. Use `cargo run --release` or scripts/studio.sh."
        );
    }
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--help" || a == "-h") {
        print!("{}", usage());
        return ExitCode::SUCCESS;
    }

    let flag = |name: &str| -> Option<String> {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
            .cloned()
    };
    let has_flag = |name: &str| -> bool { args.iter().any(|a| a == name) };

    let workspace = flag("--workspace")
        .map(PathBuf::from)
        .or_else(|| std::env::current_dir().ok());
    let mut state = StudioState::new(workspace);

    if let Some(b_name) = flag("--backend") {
        let Some(backend) = RenderBackend::parse(&b_name) else {
            eprintln!("unknown backend {b_name:?} (expected auto, gpu or cpu)");
            return ExitCode::FAILURE;
        };
        state.engine.set_backend(backend);
        let _ = state.render_current_frame();
    }

    if let Some(scene_path) = flag("--scene") {
        if let Err(e) = state.load_scene_file(Path::new(&scene_path)) {
            eprintln!("kine-studio: failed to load {scene_path}: {e}");
            return ExitCode::FAILURE;
        }
    }

    if has_flag("--play") {
        state.is_playing = true;
    }

    if has_flag("--headless-check") {
        let look = kine_studio::studio_look(&mut state);
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": look.ok,
                "stage": look.stage,
                "summary": look.summary,
                "width": state.scene_def.width,
                "height": state.scene_def.height,
                "duration": state.duration,
                "tracks": state.build_timeline_tracks().len(),
                "compositions": state.compositions.len(),
            }))
            .unwrap()
        );
        return if look.ok {
            ExitCode::SUCCESS
        } else {
            ExitCode::FAILURE
        };
    }

    #[cfg(feature = "xilem-ui")]
    {
        if let Err(e) = kine_studio::run_studio(state) {
            eprintln!("kine-studio window error: {e}");
            return ExitCode::FAILURE;
        }
        ExitCode::SUCCESS
    }

    #[cfg(not(feature = "xilem-ui"))]
    {
        eprintln!("kine-studio was built without feature `xilem-ui`; run with `--headless-check` or rebuild with default features.");
        ExitCode::FAILURE
    }
}
