//! Built-in WGSL post-processing shader presets (`filmGrain`, `vignette`,
//! `chromaticAberration`, `colorGrade`, `scanlines`).
//!
//! Validated on CPU via `naga` and executable on both `GpuRenderer` (`wgpu`) and
//! `FrameRenderer` (`CpuShaderProgram`).

/// Procedural film grain with warm/cool tinting (`amount: f32`, `tint: vec3<f32>`).
pub const FILM_GRAIN_WGSL: &str = r#"
struct Grain {
    amount: f32,
    tint: vec3<f32>,
};
@group(1) @binding(0) var<uniform> grain: Grain;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let n = (hash(uv * kine_frame.resolution + kine_frame.frame) - 0.5) * grain.amount;
    return vec4<f32>(clamp(base.rgb * grain.tint + vec3<f32>(n), vec3<f32>(0.0), vec3<f32>(1.0)), base.a);
}
"#;

/// Radial optical vignette (`intensity: f32`, `smoothness: f32`).
pub const VIGNETTE_WGSL: &str = r#"
struct Vignette {
    intensity: f32,
    smoothness: f32,
};
@group(1) @binding(0) var<uniform> params: Vignette;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let d = length(uv - vec2<f32>(0.5, 0.5)) * 1.4142135;
    let factor = clamp(1.0 - pow(d, max(params.smoothness, 0.5)) * params.intensity, 0.0, 1.0);
    return vec4<f32>(base.rgb * factor, base.a);
}
"#;

/// Radial RGB channel split (`shift: f32`).
pub const CHROMATIC_ABERRATION_WGSL: &str = r#"
struct Aberration {
    shift: f32,
};
@group(1) @binding(0) var<uniform> params: Aberration;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dir = (uv - vec2<f32>(0.5, 0.5)) * params.shift;
    let r = textureSample(kine_input, kine_sampler, clamp(uv + dir, vec2<f32>(0.0), vec2<f32>(1.0))).r;
    let g_a = textureSample(kine_input, kine_sampler, uv);
    let b = textureSample(kine_input, kine_sampler, clamp(uv - dir, vec2<f32>(0.0), vec2<f32>(1.0))).b;
    return vec4<f32>(r, g_a.g, b, g_a.a);
}
"#;

/// Color grading pass (`contrast: f32`, `saturation: f32`, `brightness: f32`, `gamma: f32`).
pub const COLOR_GRADE_WGSL: &str = r#"
struct Grade {
    contrast: f32,
    saturation: f32,
    brightness: f32,
    gamma: f32,
};
@group(1) @binding(0) var<uniform> params: Grade;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    var rgb = (base.rgb - vec3<f32>(0.5)) * params.contrast + vec3<f32>(0.5) + vec3<f32>(params.brightness);
    let luma = dot(rgb, vec3<f32>(0.2126, 0.7152, 0.0722));
    rgb = mix(vec3<f32>(luma), rgb, params.saturation);
    let g = max(params.gamma, 0.1);
    rgb = pow(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), vec3<f32>(1.0 / g));
    return vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), base.a);
}
"#;

/// CRT / broadcast horizontal scanlines (`density: f32`, `opacity: f32`).
pub const SCANLINES_WGSL: &str = r#"
struct Scanlines {
    density: f32,
    opacity: f32,
};
@group(1) @binding(0) var<uniform> params: Scanlines;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let wave = 0.5 + 0.5 * sin(uv.y * kine_frame.resolution.y * params.density * 3.14159265);
    let factor = 1.0 - params.opacity * (1.0 - wave);
    return vec4<f32>(base.rgb * factor, base.a);
}
"#;

/// Resolve a named preset (`"preset:vignette"` or `"vignette"`) to its WGSL source.
pub fn resolve_preset(name_or_wgsl: &str) -> &str {
    let trimmed = name_or_wgsl.trim();
    let key = trimmed.strip_prefix("preset:").unwrap_or(trimmed);
    match key {
        "filmGrain" | "film-grain" | "grain" => FILM_GRAIN_WGSL,
        "vignette" => VIGNETTE_WGSL,
        "chromaticAberration" | "chromatic-aberration" | "aberration" => CHROMATIC_ABERRATION_WGSL,
        "colorGrade" | "color-grade" | "grade" => COLOR_GRADE_WGSL,
        "scanlines" => SCANLINES_WGSL,
        _ => name_or_wgsl,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_builtin_shader_presets_validate_through_naga() {
        for preset in [
            FILM_GRAIN_WGSL,
            VIGNETTE_WGSL,
            CHROMATIC_ABERRATION_WGSL,
            COLOR_GRADE_WGSL,
            SCANLINES_WGSL,
        ] {
            crate::validate(preset).expect("preset must be valid WGSL");
        }
    }
}
