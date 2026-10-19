//! QuickJS evaluation of the authoring DSL.

use crate::authoring::{scene_from_def, SceneDef};
use crate::transpile::transpile_ts;
use crate::ScriptError;
use kine_core::Scene;
use rquickjs::{Context, Runtime};

/// Typed-DSL helpers injected before user code.
pub const PRELUDE: &str = r##"
const scene = (width, height, root, opts = {}) => ({ width, height, root, ...opts });
const container = (opts = {}, children = []) => ({ kind: "container", children, ...opts });
const rect = (opts = {}) => ({ kind: "rect", ...opts });
const circle = (opts = {}) => ({ kind: "circle", ...opts });
const path = (d, opts = {}) => ({ kind: "path", d, ...opts });
const image = (src, opts = {}) => ({ kind: "image", src, ...opts });
const video = (src, opts = {}) => ({ kind: "video", src, ...opts });
const text = (content, opts = {}) => ({ kind: "text", content, ...opts });
const shader = (wgsl, opts = {}) => ({ kind: "shader", shader: wgsl, ...opts });
const linearGradient = (angle, stops) => ({ kind: "linear", angle, stops });
const radialGradient = (stops) => ({ kind: "radial", stops });
const spring = (stiffness = 180, damping = 22) => `spring(${stiffness}, ${damping})`;
const cubicBezier = (x1, y1, x2, y2) => `cubic-bezier(${x1}, ${y1}, ${x2}, ${y2})`;
const steps = (n) => `steps(${n})`;
const presets = {
  filmGrain: (uniforms = { amount: 0.08, tint: [1.0, 1.0, 1.0] }, opts = {}) =>
    shader("preset:filmGrain", { uniforms, ...opts }),
  vignette: (uniforms = { intensity: 0.45, smoothness: 1.8 }, opts = {}) =>
    shader("preset:vignette", { uniforms, ...opts }),
  chromaticAberration: (uniforms = { shift: 0.004 }, opts = {}) =>
    shader("preset:chromaticAberration", { uniforms, ...opts }),
  colorGrade: (uniforms = { contrast: 1.08, saturation: 1.1, brightness: 0.0, gamma: 1.0 }, opts = {}) =>
    shader("preset:colorGrade", { uniforms, ...opts }),
  scanlines: (uniforms = { density: 0.5, opacity: 0.15 }, opts = {}) =>
    shader("preset:scanlines", { uniforms, ...opts }),
};
const rng = (seed = 1) => {
  let a = (seed >>> 0) || 1;
  return () => {
    let t = (a += 0x6D2B79F5);
    t = Math.imul(t ^ (t >>> 15), t | 1);
    t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
    return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
  };
};
"##;

pub fn eval_scene_def_js(js_src: &str) -> Result<SceneDef, ScriptError> {
    let program = format!("{PRELUDE}\nJSON.stringify((function(){{\n{js_src}\n}})())");

    let runtime = Runtime::new().map_err(|e| ScriptError::Runtime(e.to_string()))?;
    let context = Context::full(&runtime).map_err(|e| ScriptError::Runtime(e.to_string()))?;

    let evaluated: Result<Option<String>, rquickjs::Error> =
        context.with(|ctx| ctx.eval(program));

    let json = match evaluated {
        Ok(Some(s)) => s,
        Ok(None) => return Err(ScriptError::NoReturn),
        Err(e) => return Err(ScriptError::Runtime(e.to_string())),
    };

    let trimmed = json.trim();
    if !trimmed.starts_with('{') {
        return Err(ScriptError::NotAnObject(trimmed.chars().take(64).collect()));
    }

    serde_json::from_str(&json).map_err(|e| ScriptError::Json(e.to_string()))
}

pub fn eval_scene_def_ts(ts_src: &str) -> Result<SceneDef, ScriptError> {
    let js = transpile_ts(ts_src)?;
    eval_scene_def_js(&js)
}

pub fn eval_scene_js(js_src: &str) -> Result<Scene, ScriptError> {
    Ok(scene_from_def(eval_scene_def_js(js_src)?)?)
}

pub fn eval_scene_ts(ts_src: &str) -> Result<Scene, ScriptError> {
    Ok(scene_from_def(eval_scene_def_ts(ts_src)?)?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn evaluates_a_plain_scene() {
        let ts = r##"
            return scene(320, 200,
              container({ size: { width: 320, height: 200 } }, [
                rect({ fill: "#121829", size: { width: 200, height: 80 } }),
                text("script ok", { fontSize: 24, color: "#eef", align: "center" })
              ])
            );
        "##;
        let scene = eval_scene_ts(ts).expect("scene");
        assert_eq!(scene.width, 320);
        assert_eq!(scene.nodes.len(), 3);
    }

    #[test]
    fn evaluates_circle_path_image_video_presets_and_seeded_rng() {
        let ts = r##"
            const rand = rng(42);
            const r = Math.round(rand() * 20) + 10;
            return scene(200, 200,
              container({ size: { width: 200, height: 200 } }, [
                circle({ fill: "#ff0055", radius: r, size: { width: 40, height: 40 } }),
                path("M 0 0 L 20 0 L 10 20 Z", { fill: "#00ffaa", size: { width: 20, height: 20 } }),
                image("pattern:checkerboard", { size: { width: 32, height: 32 }, objectFit: "cover" }),
                video("video:smpte:64x36:30:1.0", { size: { width: 64, height: 36 }, loop: true, playbackRate: 1.5 }),
                presets.vignette({ intensity: 0.4, smoothness: 1.5 }, { size: { width: 200, height: 200 } }),
              ])
            );
        "##;
        let scene1 = eval_scene_ts(ts).expect("scene1");
        let scene2 = eval_scene_ts(ts).expect("scene2");
        assert_eq!(scene1.nodes.len(), 6);
        assert!(scene1.has_videos());
        assert_eq!(
            serde_json::to_value(&scene1).unwrap(),
            serde_json::to_value(&scene2).unwrap(),
            "seeded rng(42) must produce identical scenes"
        );
    }

    #[test]
    fn scene_options_carry_an_audio_bed() {
        let ts = r##"
            return scene(64, 64,
              container({ size: { width: 64, height: 64 } }, []),
              {
                audio: {
                  sampleRate: 48000,
                  channels: 1,
                  tracks: [{ src: "voice.wav", gainDb: -6, start: 0.5 }]
                }
              }
            );
        "##;
        let def = eval_scene_def_ts(ts).expect("definition");
        let audio = def.audio.expect("audio block survives evaluation");
        assert_eq!(audio.sample_rate, 48_000);
        assert_eq!(audio.tracks.len(), 1);
        assert_eq!(audio.tracks[0].src, "voice.wav");
        assert!((audio.tracks[0].start - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn a_shader_layer_can_be_authored_inline() {
        let ts = r##"
            return scene(64, 64,
              container({ size: { width: 64, height: 64 } }, [
                shader(`
                  @fragment
                  fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
                      let t = kine_frame.time;
                      return textureSample(kine_input, kine_sampler, uv) * vec4<f32>(t, 1.0, 1.0, 1.0);
                  }
                `, { blend: "add", uniforms: { amount: 0.5 } })
              ])
            );
        "##;
        let scene = eval_scene_ts(ts).expect("scene");
        let shader_node = scene
            .nodes
            .values()
            .find(|n| matches!(n.kind, kine_core::NodeKind::Shader { .. }))
            .expect("shader node");
        match &shader_node.kind {
            kine_core::NodeKind::Shader { source, .. } => assert!(source.contains("textureSample")),
            _ => unreachable!(),
        }
    }

    #[test]
    fn returning_a_number_is_not_a_scene() {
        let ts = "return 42;";
        let err = eval_scene_ts(ts).unwrap_err();
        assert!(matches!(err, ScriptError::NotAnObject(_)), "got {err:?}");
    }

    #[test]
    fn syntax_error_is_reported_not_panicking() {
        let ts = "return scene(1, 2, {";
        let err = eval_scene_ts(ts).unwrap_err();
        assert!(matches!(err, ScriptError::Transpile(_)), "got {err:?}");
    }

    #[test]
    fn runtime_exception_is_reported() {
        let ts = r#"throw new Error("boom");"#;
        let err = eval_scene_ts(ts).unwrap_err();
        assert!(matches!(err, ScriptError::Runtime(_)), "got {err:?}");
    }
}
