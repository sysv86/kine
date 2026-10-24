//! Newline-delimited JSON-RPC 2.0 over TCP. One connection carries one live
//! agent session.

use crate::engine::{Engine, SceneHandle};
use crate::video::mix_scene_audio;
use kine_render::{FrameFormat, RenderBackend};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;

pub fn dispatch(engine: &mut Engine, req: &Value) -> Value {
    let id = req.get("id").cloned().unwrap_or(Value::Null);
    match dispatch_inner(engine, req) {
        Ok(value) => json!({"jsonrpc": "2.0", "id": id, "result": value}),
        Err(e) => json!({"jsonrpc": "2.0", "id": id, "error": e}),
    }
}

fn dispatch_inner(engine: &mut Engine, req: &Value) -> Result<Value, Value> {
    let method = req.get("method").and_then(Value::as_str).unwrap_or("");
    let params = req.get("params").cloned().unwrap_or(Value::Null);

    match method {
        "ping" => Ok(json!("pong")),

        "doctor" => serde_json::to_value(engine.doctor()).map_err(engine_err),

        "types" => Ok(json!({"dts": kine_script::typescript_dts()})),

        "modules" => {
            let dir = params
                .get("dir")
                .and_then(Value::as_str)
                .map(PathBuf::from)
                .unwrap_or_else(|| {
                    std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
                });
            let registry = kine_script::ModuleRegistry::discover(&dir)
                .map_err(|e| json_err(&e.to_string()))?;
            let list: Vec<Value> = registry
                .iter()
                .map(|(name, module)| {
                    json!({
                        "name": name,
                        "kind": module.meta.kind,
                        "description": module.meta.description,
                        "params": module.meta.params,
                        "entry": module.scene_path,
                    })
                })
                .collect();
            Ok(json!({
                "root": registry.root().map(|p| p.to_string_lossy().into_owned()),
                "count": registry.len(),
                "modules": list,
            }))
        }

        "set_backend" => {
            let name = params
                .get("backend")
                .and_then(Value::as_str)
                .ok_or_else(|| json_err("params.backend (auto|gpu|cpu) required"))?;
            let backend = RenderBackend::parse(name)
                .ok_or_else(|| json_err("unknown backend (expected auto, gpu or cpu)"))?;
            engine.set_backend(backend);
            Ok(json!({"backend": backend.name()}))
        }

        "load_scene" => {
            let scene = params
                .get("scene")
                .and_then(Value::as_str)
                .ok_or_else(|| json_err("params.scene (JSON or TS string) required"))?;
            let handle = engine.load_scene_json(scene).map_err(engine_err)?;
            Ok(json!({"scene": handle.0}))
        }

        "free_scene" => {
            let handle = scene_handle(&params)?;
            let freed = engine.free_scene(handle);
            Ok(json!({"freed": freed}))
        }

        "render_frame" => {
            let handle = scene_handle(&params)?;
            let format = params
                .get("format")
                .and_then(Value::as_str)
                .and_then(FrameFormat::from_extension)
                .unwrap_or(FrameFormat::Png);

            if let Some(time) = params.get("time").and_then(Value::as_f64) {
                let frame_idx = params
                    .get("frame")
                    .and_then(Value::as_u64)
                    .map(|f| f as u32)
                    .unwrap_or_else(|| (time * 30.0).round().max(0.0) as u32);
                return engine
                    .render_frame_at_with_index(handle, time, frame_idx)
                    .map_err(engine_err)
                    .and_then(|pixmap| {
                        kine_render::encode_pixmap(&pixmap, format)
                            .map(|bytes| json!({"png_base64": b64_encode(&bytes), "format": format!("{format:?}")}))
                            .map_err(|e| json!({"code": -32603, "message": e}))
                    });
            }
            let frame = params.get("frame").and_then(Value::as_u64).unwrap_or(0);
            engine
                .render_frame_format(handle, frame, format)
                .map(|bytes| json!({"png_base64": b64_encode(&bytes)}))
                .map_err(engine_err)
        }

        "render_strip" => {
            let handle = scene_handle(&params)?;
            let frames = params.get("frames").and_then(Value::as_u64).unwrap_or(6) as u32;
            let fps = params.get("fps").and_then(Value::as_f64).unwrap_or(30.0);
            engine
                .render_strip_png(handle, frames, fps)
                .map(|png| json!({"png_base64": b64_encode(&png)}))
                .map_err(engine_err)
        }

        "render_onion" => {
            let handle = scene_handle(&params)?;
            let frames = params.get("frames").and_then(Value::as_u64).unwrap_or(6) as u32;
            let fps = params.get("fps").and_then(Value::as_f64).unwrap_or(30.0);
            engine
                .render_onion_png(handle, frames, fps)
                .map(|png| json!({"png_base64": b64_encode(&png)}))
                .map_err(engine_err)
        }

        "render_video" => {
            let handle = scene_handle(&params)?;
            let mut opts = crate::video::VideoOptions::default();
            if let Some(f) = params.get("frames").and_then(Value::as_u64) {
                opts.frames = f;
            }
            if let Some(fps) = params.get("fps").and_then(Value::as_u64) {
                opts.fps_num = fps as u32;
            }
            if let Some(fmt) = params.get("format").and_then(Value::as_str) {
                if let Some(c) = crate::video::VideoContainer::from_extension(fmt) {
                    opts.container = c;
                }
            }
            let report = crate::video::render_video(engine, handle, &opts).map_err(engine_err)?;
            Ok(json!({
                "container": report.container.name(),
                "width": report.width,
                "height": report.height,
                "frames": report.frames,
                "samples": report.samples,
                "keyframes": report.keyframes,
                "distinct_frames": report.distinct_frames,
                "audio_frames": report.audio_frames,
                "video_base64": b64_encode(&report.bytes),
            }))
        }

        "video_info" => {
            let src = params
                .get("src")
                .and_then(Value::as_str)
                .ok_or_else(|| json_err("params.src required"))?;
            let clip = kine_render::decode_video_source(src).map_err(engine_err)?;
            Ok(json!({
                "width": clip.width,
                "height": clip.height,
                "fps_num": clip.fps_num,
                "fps_den": clip.fps_den,
                "frames": clip.frames_rgba.len(),
                "duration_seconds": clip.duration_seconds(),
            }))
        }

        "audio_metrics" => {
            let handle = scene_handle(&params)?;
            let seconds = params.get("seconds").and_then(Value::as_f64).unwrap_or(2.0);
            let mixed = mix_scene_audio(engine, handle, seconds).map_err(engine_err)?;
            match mixed {
                Some(clip) => Ok(serde_json::to_value(clip.metrics()).unwrap_or(Value::Null)),
                None => Ok(json!({"has_audio": false})),
            }
        }

        "inspect" => {
            let handle = scene_handle(&params)?;
            engine
                .inspect(handle)
                .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
                .map_err(engine_err)
        }

        "snapshot" => {
            let handle = scene_handle(&params)?;
            let frame = params.get("frame").and_then(Value::as_u64).unwrap_or(0);
            let approved_b64 = params
                .get("approved_png_base64")
                .and_then(Value::as_str)
                .ok_or_else(|| json_err("params.approved_png_base64 required"))?;
            let approved = base64_decode(approved_b64)?;
            let threshold = params
                .get("threshold")
                .and_then(Value::as_f64)
                .unwrap_or(0.001);
            engine
                .snapshot_diff(handle, frame, &approved, threshold)
                .map(|r| serde_json::to_value(r).unwrap_or(Value::Null))
                .map_err(engine_err)
        }

        "timeline" => {
            let handle = scene_handle(&params)?;
            engine.timeline(handle).map_err(engine_err)
        }

        other => Err(json!({
            "code": -32601,
            "message": format!("method not found: {other}"),
        })),
    }
}

fn scene_handle(params: &Value) -> Result<SceneHandle, Value> {
    params
        .get("scene")
        .and_then(Value::as_u64)
        .map(SceneHandle)
        .ok_or_else(|| json_err("params.scene (u64) required"))
}

fn json_err(msg: &str) -> Value {
    json!({"code": -32602, "message": msg})
}

fn engine_err(e: impl std::fmt::Display) -> Value {
    json!({"code": -32603, "message": e.to_string()})
}

fn b64_encode(bytes: &[u8]) -> String {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    B64.encode(bytes)
}

fn base64_decode(s: &str) -> Result<Vec<u8>, Value> {
    use base64::engine::general_purpose::STANDARD as B64;
    use base64::Engine as _;
    B64.decode(s)
        .map_err(|e| json_err(&format!("invalid base64: {e}")))
}

pub fn serve(addr: &str, engine: &mut Engine) -> std::io::Result<()> {
    let listener = TcpListener::bind(addr)?;
    eprintln!("kine-server listening on {addr}");
    for stream in listener.incoming() {
        match stream {
            Ok(stream) => {
                let _ = handle_connection(engine, stream);
            }
            Err(e) => eprintln!("accept error: {e}"),
        }
    }
    Ok(())
}

fn handle_connection(engine: &mut Engine, stream: TcpStream) -> std::io::Result<()> {
    let mut reader = BufReader::new(stream.try_clone()?);
    let mut writer = stream;
    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line)?;
        if n == 0 {
            return Ok(());
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }
        let response = match serde_json::from_str::<Value>(trimmed) {
            Ok(req) => dispatch(engine, &req),
            Err(e) => json!({
                "jsonrpc": "2.0",
                "id": null,
                "error": {"code": -32700, "message": format!("parse error: {e}")},
            }),
        };
        writeln!(writer, "{response}")?;
        writer.flush()?;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::engine::Engine;

    const FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

    fn engine() -> Engine {
        let mut e = Engine::new();
        e.load_font("JetBrains Mono", FONT.to_vec());
        e
    }

    fn scene_json() -> String {
        serde_json::json!({
            "width": 64,
            "height": 64,
            "root": {
                "kind": "container",
                "size": { "width": 64, "height": 64 },
                "children": [{
                    "kind": "rect",
                    "fill": "#c81e1e",
                    "cornerRadius": 8,
                    "size": { "width": 32, "height": 32 }
                }]
            }
        })
        .to_string()
    }

    #[test]
    fn rpc_round_trip() {
        let mut engine = engine();

        let pong = dispatch(&mut engine, &json!({"jsonrpc":"2.0","id":1,"method":"ping"}));
        assert_eq!(pong["result"], json!("pong"));

        let types = dispatch(&mut engine, &json!({"jsonrpc":"2.0","id":9,"method":"types"}));
        assert!(types["result"]["dts"].as_str().unwrap().contains("declare global"));

        let load = dispatch(
            &mut engine,
            &json!({"jsonrpc":"2.0","id":2,"method":"load_scene","params":{"scene": scene_json()}}),
        );
        let scene_id = load["result"]["scene"].as_u64().unwrap();

        let frame = dispatch(
            &mut engine,
            &json!({"jsonrpc":"2.0","id":3,"method":"render_frame","params":{"scene": scene_id,"frame":0}}),
        );
        let png_b64 = frame["result"]["png_base64"].as_str().unwrap();
        assert!(!png_b64.is_empty());

        let inspect = dispatch(
            &mut engine,
            &json!({"jsonrpc":"2.0","id":4,"method":"inspect","params":{"scene": scene_id}}),
        );
        assert!(inspect["result"]["findings"].as_array().unwrap().is_empty());

        let snap = dispatch(
            &mut engine,
            &json!({"jsonrpc":"2.0","id":5,"method":"snapshot","params":{
                "scene": scene_id, "frame": 0, "approved_png_base64": png_b64, "threshold": 0.001
            }}),
        );
        assert_eq!(snap["result"]["passed"], json!(true), "snapshot report: {snap}");

        let unknown = dispatch(&mut engine, &json!({"jsonrpc":"2.0","id":6,"method":"nope"}));
        assert_eq!(unknown["error"]["code"], json!(-32601), "method-not-found code");

        let bad_params = dispatch(
            &mut engine,
            &json!({"jsonrpc":"2.0","id":7,"method":"render_frame","params":{}}),
        );
        assert_eq!(bad_params["error"]["code"], json!(-32602), "invalid-params code");
    }
}
