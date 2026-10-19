//! GPU rasterizer parity for node kinds the GPU path draws itself.
//!
//! `image()` and `video()` nodes used to be silently skipped by the GPU
//! backend, so the frame showed whatever was behind them. These tests render
//! the same scene through both backends and require identical pixels.

#![cfg(feature = "gpu")]

use kine_server::{Engine, RenderBackend};

fn scene_json() -> String {
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
                    "kind": "image",
                    "src": "pattern:checkerboard",
                    "objectFit": "cover",
                    "cornerRadius": 6,
                    "size": { "width": 48, "height": 32 },
                    "position": "absolute",
                    "inset": { "top": 0, "left": 0 }
                },
                {
                    "kind": "video",
                    "src": "video:smpte:96x64:30:1.0",
                    "objectFit": "contain",
                    "loop": true,
                    "size": { "width": 96, "height": 64 },
                    "position": "absolute",
                    "inset": { "top": 0, "left": 0 }
                }
            ]
        }
    })
    .to_string()
}

fn render_with(backend: RenderBackend, time: f64) -> Vec<u8> {
    let mut engine = Engine::new();
    engine.set_backend(backend);
    let handle = engine.load_scene_json(&scene_json()).expect("scene");
    engine
        .render_frame_at(handle, time)
        .expect("frame")
        .data_as_u8_slice()
        .to_vec()
}

#[test]
fn image_and_video_nodes_match_between_backends() {
    if !kine_render::gpu::gpu_adapter_present() {
        eprintln!("no GPU adapter: skipping parity check");
        return;
    }

    for time in [0.0, 0.35] {
        let cpu = render_with(RenderBackend::Cpu, time);
        let gpu = render_with(RenderBackend::Gpu, time);
        assert_eq!(
            cpu, gpu,
            "image()/video() must rasterize identically on both backends at t={time}"
        );
    }
}

#[test]
fn gpu_draws_a_video_that_changes_over_time() {
    if !kine_render::gpu::gpu_adapter_present() {
        eprintln!("no GPU adapter: skipping parity check");
        return;
    }

    let first = render_with(RenderBackend::Gpu, 0.0);
    let later = render_with(RenderBackend::Gpu, 0.35);
    assert_ne!(
        first, later,
        "a video() node must show a different frame later in the clip"
    );
}
