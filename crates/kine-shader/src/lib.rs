//! WGSL shaders written by the agent, validated **without a GPU** and executable
//! on both GPU (`wgpu`) and CPU (`vm`).
//!
//! Responsibilities:
//!
//! * [`validate`] — parse + validate + reflect an agent-authored module (or
//!   a built-in [`presets`] identifier such as `"preset:vignette"`).
//! * [`PRELUDE`] / [`wrap`] — the engine supplies the binding declarations
//!   (`kine_frame`, `kine_input`, `kine_sampler`) so the agent cannot get the
//!   bind group layout wrong; it only writes the fragment entry point.
//! * [`UniformPack`] — turn the agent's JSON uniform values into bytes using the
//!   offsets naga computed, without the agent knowing about std140 rules.
//! * [`vm::CpuShaderProgram`] — execute the validated WGSL AST directly on CPU
//!   threads when no GPU adapter is used.
//! * [`presets`] — built-in WGSL post-processing shaders (`filmGrain`, `vignette`,
//!   `chromaticAberration`, `colorGrade`, `scanlines`).

pub mod presets;
pub mod reflect;
pub mod uniforms;
pub mod vm;

pub use presets::{
    resolve_preset, CHROMATIC_ABERRATION_WGSL, COLOR_GRADE_WGSL, FILM_GRAIN_WGSL, SCANLINES_WGSL,
    VIGNETTE_WGSL,
};
pub use reflect::{
    EntryPoint, ShaderModule, ShaderStage, TextureBinding, UniformBinding, UniformMember,
    UniformType,
};
pub use uniforms::{UniformPack, UniformPackError};
pub use vm::{CpuBlendMode, CpuShaderProgram};

/// Declarations the engine injects ahead of the agent's source.
pub const PRELUDE: &str = r#"// ── injected by Kine ────────────────────────────────────────────────────
struct KineFrame {
    resolution: vec2<f32>,
    time: f32,
    frame: f32,
};

@group(0) @binding(0) var<uniform> kine_frame: KineFrame;
@group(0) @binding(1) var kine_input: texture_2d<f32>;
@group(0) @binding(2) var kine_sampler: sampler;

// Expected entry point:
//   @fragment
//   fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32>
// `uv` is in [0,1] with the origin at the top-left; `kine_input` holds the
// scene rendered so far for this frame.
// ─────────────────────────────────────────────────────────────────────────
"#;

/// The fragment entry point the engine binds to.
pub const ENTRY_POINT: &str = "main";

fn prelude_prefix() -> String {
    format!("{PRELUDE}\n")
}

/// Prepend the engine prelude to an agent-authored shader body (or resolved preset).
pub fn wrap(agent_source: &str) -> String {
    let resolved = presets::resolve_preset(agent_source);
    format!("{}{resolved}\n", prelude_prefix())
}

#[derive(Debug, thiserror::Error)]
pub enum ShaderError {
    #[error("WGSL syntax error at line {line}: {message}")]
    Parse {
        message: String,
        diagnostic: String,
        line: u32,
        column: u32,
    },

    #[error("WGSL validation error at line {line}: {message}")]
    Validate {
        message: String,
        diagnostic: String,
        line: u32,
        column: u32,
    },

    #[error(
        "shader has no fragment entry point named `{ENTRY_POINT}` — expected exactly:\n  \
         @fragment\n  fn {ENTRY_POINT}(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32>"
    )]
    MissingEntryPoint,

    #[error(
        "fragment entry point `{ENTRY_POINT}` has the wrong signature: {reason}. Expected:\n  \
         @fragment\n  fn {ENTRY_POINT}(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32>"
    )]
    BadEntryPointSignature { reason: String },
}

impl ShaderError {
    pub fn line(&self) -> Option<u32> {
        match self {
            Self::Parse { line, .. } | Self::Validate { line, .. } => Some(*line),
            Self::MissingEntryPoint | Self::BadEntryPointSignature { .. } => None,
        }
    }

    pub fn diagnostic(&self) -> String {
        match self {
            Self::Parse { diagnostic, .. } | Self::Validate { diagnostic, .. } => {
                diagnostic.clone()
            }
            other => other.to_string(),
        }
    }
}

fn prelude_line_count() -> u32 {
    prelude_prefix().lines().count() as u32
}

pub(crate) fn relocate(err: ShaderError, agent_source: &str) -> ShaderError {
    let resolved = presets::resolve_preset(agent_source);
    let offset = prelude_line_count();
    let (message, line, column, wrap): (
        String,
        u32,
        u32,
        fn(String, String, u32, u32) -> ShaderError,
    ) = match err {
        ShaderError::Parse {
            message,
            line,
            column,
            ..
        } => (message, line, column, |message, diagnostic, line, column| {
            ShaderError::Parse {
                message,
                diagnostic,
                line,
                column,
            }
        }),
        ShaderError::Validate {
            message,
            line,
            column,
            ..
        } => (message, line, column, |message, diagnostic, line, column| {
            ShaderError::Validate {
                message,
                diagnostic,
                line,
                column,
            }
        }),
        other => return other,
    };

    let agent_line = line.saturating_sub(offset).max(1);
    let agent_column = if line <= offset { 1 } else { column };
    let diagnostic = excerpt(resolved, agent_line, agent_column, &message);
    wrap(message, diagnostic, agent_line, agent_column)
}

fn excerpt(source: &str, line: u32, column: u32, message: &str) -> String {
    let text = source
        .lines()
        .nth(line.saturating_sub(1) as usize)
        .unwrap_or("");
    let gutter = line.to_string();
    let pad = " ".repeat(gutter.len());
    let caret_pad = " ".repeat(column.saturating_sub(1) as usize);
    format!(
        "error: {message}\n  {pad}--> scene.wgsl:{line}:{column}\n  {pad}|\n  {gutter} | {text}\n  {pad}| {caret_pad}^"
    )
}

/// Validate an agent-authored shader (or built-in `preset:...` identifier).
pub fn validate(agent_source: &str) -> Result<ShaderModule, ShaderError> {
    let resolved = presets::resolve_preset(agent_source);
    validate_full(&wrap(resolved)).map_err(|e| relocate(e, resolved))
}

/// Validate complete WGSL (prelude already included) and enforce the effect
/// contract.
pub fn validate_full(source: &str) -> Result<ShaderModule, ShaderError> {
    let module = parse_and_validate(source)?;
    reflect::check_fragment_signature(&module)?;
    reflect::reflect_ir(&module)
}

/// Reflect any valid WGSL module without enforcing the fragment contract.
pub fn reflect_source(source: &str) -> Result<ShaderModule, ShaderError> {
    let module = parse_and_validate(source)?;
    reflect::reflect_ir(&module)
}

pub(crate) fn parse_and_validate(source: &str) -> Result<naga::Module, ShaderError> {
    let diagnostic_source = source;
    let module = naga::front::wgsl::parse_str(source).map_err(|e| {
        let loc = e.location(diagnostic_source);
        ShaderError::Parse {
            message: e.message().to_string(),
            diagnostic: e.emit_to_string(diagnostic_source),
            line: loc.map_or(0, |l| l.line_number),
            column: loc.map_or(0, |l| l.line_position),
        }
    })?;

    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|e| {
        let loc = e.location(diagnostic_source);
        ShaderError::Validate {
            message: e.as_inner().to_string(),
            diagnostic: e.emit_to_string(diagnostic_source),
            line: loc.map_or(0, |l| l.line_number),
            column: loc.map_or(0, |l| l.line_position),
        }
    })?;

    Ok(module)
}

#[cfg(test)]
mod tests {
    use super::*;

    const GOOD: &str = r#"
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    var color = textureSample(kine_input, kine_sampler, uv);
    color.r += kine_frame.time * 0.0;
    return color;
}
"#;

    #[test]
    fn accepts_a_well_formed_effect() {
        let module = validate(GOOD).expect("should validate");
        assert!(module.fragment_entry_points().any(|e| e.name == ENTRY_POINT));
        assert!(module.uniforms.iter().any(|u| u.name == "kine_frame"));
        assert!(module.textures.iter().any(|t| t.name == "kine_input"));
        assert!(module.samplers.iter().any(|s| s.name == "kine_sampler"));
        let frame = module
            .uniforms
            .iter()
            .find(|u| u.name == "kine_frame")
            .unwrap();
        assert_eq!(frame.size, 16);
    }

    #[test]
    fn reports_a_syntax_error_with_a_line() {
        let bad = "\n\n@fragment\nfn main( -> vec4<f32> { return vec4<f32>(1.0); }\n";
        let err = validate(bad).unwrap_err();
        assert!(matches!(err, ShaderError::Parse { .. }), "got {err:?}");
        let line = err.line().expect("syntax errors carry a line");
        assert_eq!(line, 4, "diagnostic: {}", err.diagnostic());
        assert!(err.diagnostic().contains("fn main"), "{}", err.diagnostic());
    }

    #[test]
    fn reports_a_type_error() {
        let bad = r#"
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let v: vec4<f32> = vec3<f32>(1.0, 0.0, 0.0);
    return v;
}
"#;
        let err = validate(bad).unwrap_err();
        assert!(
            matches!(err, ShaderError::Validate { .. } | ShaderError::Parse { .. }),
            "got {err:?}"
        );
        assert!(err.line().is_some());
        assert!(err.diagnostic().contains("vec4"), "{}", err.diagnostic());
    }

    #[test]
    fn missing_entry_point_gets_an_actionable_message() {
        let no_entry = "fn helper() -> f32 { return 1.0; }";
        let err = validate(no_entry).unwrap_err();
        assert!(matches!(err, ShaderError::MissingEntryPoint));
        let msg = err.to_string();
        assert!(msg.contains("@fragment"), "{msg}");
        assert!(msg.contains("uv: vec2<f32>"), "{msg}");
    }

    #[test]
    fn wrong_entry_point_signature_is_explained() {
        let wrong = r#"
@fragment
fn main() -> @location(0) vec4<f32> { return vec4<f32>(1.0); }
"#;
        let err = validate(wrong).unwrap_err();
        assert!(
            matches!(err, ShaderError::BadEntryPointSignature { .. }),
            "got {err:?}"
        );
    }

    #[test]
    fn agents_can_declare_their_own_uniforms_at_group_one() {
        let src = r#"
struct Params {
    speed: f32,
    tint: vec4<f32>,
};
@group(1) @binding(0) var<uniform> params: Params;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = kine_frame.time * params.speed;
    return textureSample(kine_input, kine_sampler, uv + vec2<f32>(t, 0.0)) * params.tint;
}
"#;
        let module = validate(src).expect("should validate");
        let params = module
            .uniforms
            .iter()
            .find(|u| u.name == "params")
            .expect("agent uniform reflected");
        assert_eq!(params.group, 1);
        assert_eq!(params.binding, 0);
        let names: Vec<&str> = params.members.iter().map(|m| m.name.as_str()).collect();
        assert_eq!(names, vec!["speed", "tint"]);
        assert_eq!(params.members[0].offset, 0);
        assert_eq!(params.members[1].offset, 16);
    }
}
