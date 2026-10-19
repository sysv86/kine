//! Authoring schema (the interchange format between the TS layer and Rust)
//! and its conversion into a layout-ready [`kine_core::Scene`].

use kine_core::{
    BoxShadow, Color, Gradient, GradientStop, NodeId, NodeKind, ObjectFit, Scene, StrokeStyle,
    TextAlign, Transform2D,
};
use serde::{Deserialize, Serialize};

#[derive(Debug, thiserror::Error)]
pub enum AuthoringError {
    #[error("invalid color: {0:?} (expected #rgb, #rrggbb or #rrggbbaa)")]
    InvalidColor(String),

    #[error(
        "unknown node kind: {0:?} (expected container, rect, circle, path, image, video, shader or text)"
    )]
    UnknownKind(String),

    #[error("unknown {field} value: {value:?}")]
    UnknownEnum { field: &'static str, value: String },

    #[error("text node is missing `content`")]
    MissingContent,

    #[error("path node is missing `d` (SVG path data)")]
    MissingPathData,

    #[error("image node is missing `src`")]
    MissingImageSrc,

    #[error("video node is missing `src`")]
    MissingVideoSrc,

    #[error("shader node is missing `shader` (the WGSL source)")]
    MissingShader,

    #[error(
        "unknown animated property: {0:?} (expected offsetX, offsetY, width, height, opacity, translateX, translateY, scale, scaleX, scaleY, rotation, cornerRadius or fontSize)"
    )]
    UnknownAnimProperty(String),

    #[error("animation on {property:?} has end <= start ({start} >= {end})")]
    EmptyAnimationRange {
        property: &'static str,
        start: f64,
        end: f64,
    },

    #[error(
        "unknown easing: {0:?} (expected linear, easeIn, easeOut, easeInOut, spring, steps(N) or cubic-bezier(x1,y1,x2,y2))"
    )]
    UnknownEasing(String),
}

/// A full scene description as authored by a script.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SceneDef {
    pub width: u32,
    pub height: u32,
    pub root: NodeDef,
    /// Optional audio bed.
    #[serde(default)]
    pub audio: Option<AudioDef>,
}

/// The audio side of a scene.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioDef {
    /// Sample rate of the mixed track.
    #[serde(default = "default_sample_rate")]
    pub sample_rate: u32,
    /// Channels of the mixed track.
    #[serde(default = "default_channels")]
    pub channels: u16,
    /// Optional target peak normalization in dBFS (e.g. `-1.0`).
    #[serde(default)]
    pub normalize_peak_db: Option<f32>,
    #[serde(default)]
    pub tracks: Vec<AudioTrackDef>,
}

fn default_sample_rate() -> u32 {
    48_000
}

fn default_channels() -> u16 {
    2
}

/// Parametric EQ / filter band authored on an audio track.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EqBandDef {
    /// `"lowpass"`, `"highpass"`, `"bandpass"`, `"notch"`, `"peaking"`, `"lowshelf"`, or `"highshelf"`.
    pub kind: String,
    pub freq_hz: f32,
    #[serde(default = "default_eq_q")]
    pub q: f32,
    #[serde(default)]
    pub gain_db: f32,
}

fn default_eq_q() -> f32 {
    0.707
}

/// Dynamic range compressor authored on an audio track.
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompressorDef {
    #[serde(default = "default_comp_threshold")]
    pub threshold_db: f32,
    #[serde(default = "default_comp_ratio")]
    pub ratio: f32,
    #[serde(default = "default_comp_attack")]
    pub attack_ms: f32,
    #[serde(default = "default_comp_release")]
    pub release_ms: f32,
    #[serde(default)]
    pub makeup_db: f32,
}

fn default_comp_threshold() -> f32 {
    -18.0
}
fn default_comp_ratio() -> f32 {
    3.0
}
fn default_comp_attack() -> f32 {
    10.0
}
fn default_comp_release() -> f32 {
    120.0
}

/// One audio source placed on the timeline.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AudioTrackDef {
    /// Path to the source file (relative to the scene file) or `synth:...` URI.
    pub src: String,
    /// Gain in decibels; `0` is unity.
    #[serde(default)]
    pub gain_db: f32,
    /// Where the track starts on the timeline, in seconds.
    #[serde(default)]
    pub start: f64,
    /// Trim from the beginning of the source clip, in seconds.
    #[serde(default)]
    pub trim_start: f64,
    /// Optional end timestamp in the source clip, in seconds.
    #[serde(default)]
    pub trim_end: Option<f64>,
    /// Whether the clip loops continuously until the end of the mix.
    #[serde(default, alias = "loop")]
    pub loop_track: bool,
    /// Constant-power stereo pan in `[-1.0, 1.0]`.
    #[serde(default)]
    pub pan: f32,
    /// Fade lengths in seconds.
    #[serde(default)]
    pub fade_in: f64,
    #[serde(default)]
    pub fade_out: f64,
    /// Fade curve (`"linear"`, `"equalPower"`, `"sCurve"`, `"exponential"`).
    #[serde(default)]
    pub fade_curve: Option<String>,
    /// Optional high-pass rumble filter cutoff in Hz.
    #[serde(default)]
    pub highpass_hz: Option<f32>,
    /// Optional low-pass filter cutoff in Hz.
    #[serde(default)]
    pub lowpass_hz: Option<f32>,
    /// Optional parametric EQ / filter chain.
    #[serde(default)]
    pub eq: Option<Vec<EqBandDef>>,
    /// Optional dynamic range compressor.
    #[serde(default)]
    pub compressor: Option<CompressorDef>,
    /// Sidechain ducking amount in dB (e.g. `-10` ducks background music by 10 dB when voice speaks).
    #[serde(default)]
    pub duck_db: f32,
    /// Threshold in dBFS at which foreground tracks trigger ducking.
    #[serde(default)]
    pub duck_threshold_db: Option<f32>,
    #[serde(default)]
    pub muted: bool,
}

/// Gradient fill authored in TS/JSON.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradientDef {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub angle: Option<f32>,
    pub stops: Vec<GradientStopDef>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GradientStopDef {
    pub offset: f32,
    pub color: String,
}

/// One node in the authored tree.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeDef {
    #[serde(default)]
    pub kind: Option<String>,
    #[serde(default)]
    pub children: Option<Vec<NodeDef>>,

    // ── visual / transform ──────────────────────────────────────────────────
    #[serde(default)]
    pub opacity: Option<f32>,
    #[serde(default)]
    pub translate_x: Option<f32>,
    #[serde(default)]
    pub translate_y: Option<f32>,
    #[serde(default)]
    pub scale: Option<f32>,
    #[serde(default)]
    pub scale_x: Option<f32>,
    #[serde(default)]
    pub scale_y: Option<f32>,
    #[serde(default)]
    pub rotation: Option<f32>,

    // ── rect / circle / path / image ────────────────────────────────────────
    #[serde(default)]
    pub fill: Option<String>,
    #[serde(default)]
    pub corner_radius: Option<f32>,
    #[serde(default)]
    pub radius: Option<f32>,
    #[serde(default)]
    pub d: Option<String>,
    #[serde(default)]
    pub src: Option<String>,
    #[serde(default)]
    pub object_fit: Option<String>,
    #[serde(default)]
    pub start: Option<f64>,
    #[serde(default)]
    pub trim_start: Option<f64>,
    #[serde(default)]
    pub trim_end: Option<f64>,
    #[serde(default)]
    pub playback_rate: Option<f64>,
    #[serde(default, alias = "loop")]
    pub loop_video: Option<bool>,
    #[serde(default)]
    pub stroke_color: Option<String>,
    #[serde(default)]
    pub stroke_width: Option<f32>,
    #[serde(default)]
    pub shadow_color: Option<String>,
    #[serde(default)]
    pub shadow_blur: Option<f32>,
    #[serde(default)]
    pub shadow_offset_x: Option<f32>,
    #[serde(default)]
    pub shadow_offset_y: Option<f32>,
    #[serde(default)]
    pub gradient: Option<GradientDef>,

    // ── shader ─────────────────────────────────────────────────────────────
    #[serde(default)]
    pub shader: Option<String>,
    #[serde(default)]
    pub uniforms: Option<serde_json::Map<String, serde_json::Value>>,
    #[serde(default)]
    pub blend: Option<String>,

    // ── text ───────────────────────────────────────────────────────────────
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub font_family: Option<String>,
    #[serde(default)]
    pub font_size: Option<f32>,
    #[serde(default)]
    pub font_weight: Option<u16>,
    #[serde(default)]
    pub color: Option<String>,
    #[serde(default)]
    pub align: Option<String>,

    // ── layout ─────────────────────────────────────────────────────────────
    #[serde(default)]
    pub size: Option<SizeDef>,
    #[serde(default)]
    pub min_size: Option<SizeDef>,
    #[serde(default)]
    pub max_size: Option<SizeDef>,
    #[serde(default)]
    pub inset: Option<EdgesDef>,
    #[serde(default)]
    pub margin: Option<EdgesDef>,
    #[serde(default)]
    pub padding: Option<EdgesDef>,
    #[serde(default)]
    pub position: Option<String>,
    #[serde(default)]
    pub flex_direction: Option<String>,
    #[serde(default)]
    pub flex_wrap: Option<String>,
    #[serde(default)]
    pub flex_grow: Option<f32>,
    #[serde(default)]
    pub flex_shrink: Option<f32>,
    #[serde(default)]
    pub flex_basis: Option<f32>,
    #[serde(default)]
    pub justify_content: Option<String>,
    #[serde(default)]
    pub align_items: Option<String>,
    #[serde(default)]
    pub align_self: Option<String>,
    #[serde(default)]
    pub gap: Option<f32>,
    #[serde(default)]
    pub aspect_ratio: Option<f32>,
    #[serde(default)]
    pub overflow: Option<String>,

    #[serde(default)]
    pub animate: Option<std::collections::BTreeMap<String, AnimationDef>>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KeyframeDef {
    pub time: f64,
    pub value: f32,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AnimationDef {
    #[serde(default)]
    pub from: f32,
    #[serde(default)]
    pub to: f32,
    #[serde(default)]
    pub start: f64,
    #[serde(default = "default_animation_end")]
    pub end: f64,
    #[serde(default)]
    pub keyframes: Option<Vec<KeyframeDef>>,
    #[serde(default)]
    pub easing: Option<String>,
}

fn default_animation_end() -> f64 {
    1.0
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SizeDef {
    pub width: Option<f32>,
    pub height: Option<f32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EdgesDef {
    pub top: Option<f32>,
    pub left: Option<f32>,
    pub right: Option<f32>,
    pub bottom: Option<f32>,
}

/// Parse `#rgb`, `#rgba`, `#rrggbb` or `#rrggbbaa`.
pub fn parse_color(s: &str) -> Result<Color, AuthoringError> {
    let bad = || AuthoringError::InvalidColor(s.to_string());
    let hex = s.strip_prefix('#').ok_or_else(bad)?;
    let nib = |c: char| c.to_digit(16).map(|d| d as u8).ok_or_else(bad);
    let chars: Vec<char> = hex.chars().collect();
    match chars.len() {
        3 => {
            let mut it = chars.iter().map(|c| nib(*c));
            let r = it.next().unwrap()? * 17;
            let g = it.next().unwrap()? * 17;
            let b = it.next().unwrap()? * 17;
            Ok(Color::rgb(r, g, b))
        }
        4 => {
            let mut it = chars.iter().map(|c| nib(*c));
            let r = it.next().unwrap()? * 17;
            let g = it.next().unwrap()? * 17;
            let b = it.next().unwrap()? * 17;
            let a = it.next().unwrap()? * 17;
            Ok(Color::rgba(r, g, b, a))
        }
        6 | 8 => {
            let byte = |i: usize| -> Result<u8, AuthoringError> {
                Ok(nib(chars[i])? * 16 + nib(chars[i + 1])?)
            };
            let r = byte(0)?;
            let g = byte(2)?;
            let b = byte(4)?;
            let a = if chars.len() == 8 { byte(6)? } else { 255 };
            Ok(Color::rgba(r, g, b, a))
        }
        _ => Err(bad()),
    }
}

fn parse_blend(s: &str) -> Result<kine_core::ShaderBlend, AuthoringError> {
    match s.to_ascii_lowercase().as_str() {
        "normal" | "over" => Ok(kine_core::ShaderBlend::Normal),
        "add" | "plus" => Ok(kine_core::ShaderBlend::Add),
        "multiply" | "mul" => Ok(kine_core::ShaderBlend::Multiply),
        "screen" => Ok(kine_core::ShaderBlend::Screen),
        other => Err(AuthoringError::UnknownEnum {
            field: "blend",
            value: other.to_string(),
        }),
    }
}

fn parse_align(s: &str) -> Result<TextAlign, AuthoringError> {
    match s.to_ascii_lowercase().as_str() {
        "left" | "start" => Ok(TextAlign::Left),
        "center" => Ok(TextAlign::Center),
        "right" | "end" => Ok(TextAlign::Right),
        other => Err(AuthoringError::UnknownEnum {
            field: "align",
            value: other.to_string(),
        }),
    }
}

fn parse_object_fit(s: Option<&str>) -> Result<ObjectFit, AuthoringError> {
    match s.unwrap_or("cover").to_ascii_lowercase().as_str() {
        "cover" => Ok(ObjectFit::Cover),
        "contain" => Ok(ObjectFit::Contain),
        "fill" | "stretch" => Ok(ObjectFit::Fill),
        other => Err(AuthoringError::UnknownEnum {
            field: "objectFit",
            value: other.to_string(),
        }),
    }
}

fn parse_gradient(def: &GradientDef) -> Result<Gradient, AuthoringError> {
    let mut stops = Vec::with_capacity(def.stops.len());
    for s in &def.stops {
        stops.push(GradientStop {
            offset: s.offset.clamp(0.0, 1.0),
            color: parse_color(&s.color)?,
        });
    }
    stops.sort_by(|a, b| a.offset.total_cmp(&b.offset));
    match def.kind.as_deref().unwrap_or("linear").to_ascii_lowercase().as_str() {
        "linear" => Ok(Gradient::Linear {
            angle_deg: def.angle.unwrap_or(0.0),
            stops,
        }),
        "radial" => Ok(Gradient::Radial { stops }),
        other => Err(AuthoringError::UnknownEnum {
            field: "gradient.kind",
            value: other.to_string(),
        }),
    }
}

pub fn scene_from_def(def: SceneDef) -> Result<Scene, AuthoringError> {
    let mut scene = Scene::new(def.width, def.height);
    let root_id = scene.root;
    apply_node(&mut scene, root_id, &def.root, true)?;
    Ok(scene)
}

fn apply_node(
    scene: &mut Scene,
    id: NodeId,
    def: &NodeDef,
    is_root: bool,
) -> Result<(), AuthoringError> {
    let kind_name = def
        .kind
        .as_deref()
        .unwrap_or("container")
        .to_ascii_lowercase();

    let def_for_layout = def.clone();
    scene.with_layout(id, move |l| apply_layout(l, &def_for_layout));

    if is_root {
        // Root keeps its full-canvas size unless overridden.
    }

    let stroke = match (&def.stroke_color, def.stroke_width) {
        (Some(c), Some(w)) if w > 0.0 => Some(StrokeStyle {
            color: parse_color(c)?,
            width: w,
        }),
        (Some(c), None) => Some(StrokeStyle {
            color: parse_color(c)?,
            width: 1.0,
        }),
        _ => None,
    };
    let shadow = match &def.shadow_color {
        Some(c) => Some(BoxShadow {
            color: parse_color(c)?,
            blur: def.shadow_blur.unwrap_or(12.0).max(0.0),
            offset_x: def.shadow_offset_x.unwrap_or(0.0),
            offset_y: def.shadow_offset_y.unwrap_or(4.0),
        }),
        None => None,
    };
    let gradient = def.gradient.as_ref().map(parse_gradient).transpose()?;
    let base_scale = def.scale.unwrap_or(1.0);
    let transform = Transform2D {
        translate_x: def.translate_x.unwrap_or(0.0),
        translate_y: def.translate_y.unwrap_or(0.0),
        scale_x: def.scale_x.unwrap_or(base_scale),
        scale_y: def.scale_y.unwrap_or(base_scale),
        rotation_deg: def.rotation.unwrap_or(0.0),
    };
    let clip = def
        .overflow
        .as_deref()
        .is_some_and(|o| o.eq_ignore_ascii_case("hidden") || o.eq_ignore_ascii_case("clip"));

    if let Some(node) = scene.nodes.get_mut(&id) {
        node.opacity = def.opacity.unwrap_or(1.0).clamp(0.0, 1.0);
        node.transform = transform;
        node.stroke = stroke;
        node.shadow = shadow;
        node.gradient = gradient;
        node.clip = clip;
    }

    match kind_name.as_str() {
        "container" => {}
        "rect" => {
            let fill = def
                .fill
                .as_deref()
                .map(parse_color)
                .transpose()?
                .unwrap_or(Color::BLACK);
            if let Some(node) = scene.nodes.get_mut(&id) {
                node.kind = NodeKind::Rect {
                    fill,
                    corner_radius: def.corner_radius.unwrap_or(0.0),
                };
            }
        }
        "circle" | "ellipse" => {
            let fill = def
                .fill
                .as_deref()
                .map(parse_color)
                .transpose()?
                .unwrap_or(Color::WHITE);
            if let Some(node) = scene.nodes.get_mut(&id) {
                node.kind = NodeKind::Circle {
                    fill,
                    radius: def.radius.or(def.corner_radius),
                };
            }
        }
        "path" | "svg" => {
            let d = def
                .d
                .clone()
                .or_else(|| def.content.clone())
                .ok_or(AuthoringError::MissingPathData)?;
            let fill = def.fill.as_deref().map(parse_color).transpose()?;
            if let Some(node) = scene.nodes.get_mut(&id) {
                node.kind = NodeKind::Path { d, fill };
            }
        }
        "image" | "img" => {
            let src = def.src.clone().ok_or(AuthoringError::MissingImageSrc)?;
            let fit = parse_object_fit(def.object_fit.as_deref())?;
            if let Some(node) = scene.nodes.get_mut(&id) {
                node.kind = NodeKind::Image {
                    src,
                    fit,
                    corner_radius: def.corner_radius.unwrap_or(0.0),
                };
            }
        }
        "video" | "vid" => {
            let src = def.src.clone().ok_or(AuthoringError::MissingVideoSrc)?;
            let fit = parse_object_fit(def.object_fit.as_deref())?;
            if let Some(node) = scene.nodes.get_mut(&id) {
                node.kind = NodeKind::Video {
                    src,
                    fit,
                    corner_radius: def.corner_radius.unwrap_or(0.0),
                    start: def.start.unwrap_or(0.0),
                    trim_start: def.trim_start.unwrap_or(0.0),
                    trim_end: def.trim_end,
                    playback_rate: def.playback_rate.unwrap_or(1.0),
                    loop_video: def.loop_video.unwrap_or(false),
                };
            }
        }
        "shader" => {
            let source = def.shader.clone().ok_or(AuthoringError::MissingShader)?;
            let blend = match def.blend.as_deref() {
                Some(b) => parse_blend(b)?,
                None => kine_core::ShaderBlend::Normal,
            };
            if let Some(node) = scene.nodes.get_mut(&id) {
                node.kind = NodeKind::Shader {
                    source,
                    uniforms: def.uniforms.clone().unwrap_or_default(),
                    blend,
                };
            }
        }
        "text" => {
            let content = def.content.clone().ok_or(AuthoringError::MissingContent)?;
            let color = def
                .color
                .as_deref()
                .or(def.fill.as_deref())
                .map(parse_color)
                .transpose()?
                .unwrap_or(Color::WHITE);
            let align = match def.align.as_deref() {
                Some(a) => parse_align(a)?,
                None => TextAlign::Left,
            };
            if let Some(node) = scene.nodes.get_mut(&id) {
                node.kind = NodeKind::Text {
                    content,
                    font_family: def
                        .font_family
                        .clone()
                        .unwrap_or_else(|| "JetBrains Mono".to_string()),
                    font_size: def.font_size.unwrap_or(16.0),
                    font_weight: def.font_weight.unwrap_or(400),
                    color,
                    align,
                };
            }
        }
        other => return Err(AuthoringError::UnknownKind(other.to_string())),
    }

    if let Some(tracks) = &def.animate {
        for (name, spec) in tracks {
            let property = kine_core::AnimProperty::parse(name)
                .ok_or_else(|| AuthoringError::UnknownAnimProperty(name.clone()))?;
            let easing = parse_easing(spec.easing.as_deref())?;

            let keyframes = if let Some(kfs) = &spec.keyframes {
                if kfs.len() < 2 {
                    return Err(AuthoringError::EmptyAnimationRange {
                        property: property.name(),
                        start: 0.0,
                        end: 0.0,
                    });
                }
                kfs.iter()
                    .map(|k| kine_core::Keyframe {
                        time: k.time,
                        value: f64::from(k.value),
                    })
                    .collect()
            } else {
                if spec.end <= spec.start {
                    return Err(AuthoringError::EmptyAnimationRange {
                        property: property.name(),
                        start: spec.start,
                        end: spec.end,
                    });
                }
                vec![
                    kine_core::Keyframe {
                        time: spec.start,
                        value: f64::from(spec.from),
                    },
                    kine_core::Keyframe {
                        time: spec.end,
                        value: f64::from(spec.to),
                    },
                ]
            };

            let track = kine_core::KeyframeTrack::new(keyframes, easing);
            let base = authored_value(def, property);
            scene.add_animation(kine_core::NodeAnimation {
                node: id,
                property,
                track,
                base,
            });
        }
    }

    if let Some(children) = &def.children {
        for child in children {
            let child_id = scene.add_node(id, NodeKind::Container);
            apply_node(scene, child_id, child, false)?;
        }
    }

    Ok(())
}

fn authored_value(def: &NodeDef, property: kine_core::AnimProperty) -> f32 {
    use kine_core::AnimProperty;
    match property {
        AnimProperty::OffsetX => def.inset.as_ref().and_then(|e| e.left).unwrap_or(0.0),
        AnimProperty::OffsetY => def.inset.as_ref().and_then(|e| e.top).unwrap_or(0.0),
        AnimProperty::Width => def.size.as_ref().and_then(|s| s.width).unwrap_or(0.0),
        AnimProperty::Height => def.size.as_ref().and_then(|s| s.height).unwrap_or(0.0),
        AnimProperty::Opacity => 0.0,
        AnimProperty::TranslateX => def.translate_x.unwrap_or(0.0),
        AnimProperty::TranslateY => def.translate_y.unwrap_or(0.0),
        AnimProperty::Scale | AnimProperty::ScaleX | AnimProperty::ScaleY => 0.0,
        AnimProperty::Rotation => def.rotation.unwrap_or(0.0),
        AnimProperty::CornerRadius => def.corner_radius.unwrap_or(0.0),
        AnimProperty::FontSize => def.font_size.unwrap_or(0.0),
    }
}

fn parse_easing(name: Option<&str>) -> Result<kine_core::Easing, AuthoringError> {
    let raw = name.unwrap_or("linear").trim();
    let lower = raw.to_ascii_lowercase();
    match lower.as_str() {
        "linear" => return Ok(kine_core::Easing::Linear),
        "easein" | "ease-in" => return Ok(kine_core::Easing::EaseIn),
        "easeout" | "ease-out" => return Ok(kine_core::Easing::EaseOut),
        "easeinout" | "ease-in-out" => return Ok(kine_core::Easing::EaseInOut),
        "easeoutcubic" | "ease-out-cubic" => {
            return Ok(kine_core::Easing::CubicBezier {
                x1: 0.33,
                y1: 1.0,
                x2: 0.68,
                y2: 1.0,
            })
        }
        "easeinoutcubic" | "ease-in-out-cubic" => {
            return Ok(kine_core::Easing::CubicBezier {
                x1: 0.65,
                y1: 0.0,
                x2: 0.35,
                y2: 1.0,
            })
        }
        "spring" => {
            return Ok(kine_core::Easing::Spring {
                stiffness: 180.0,
                damping: 22.0,
            })
        }
        _ => {}
    }

    if let Some(args_str) = lower
        .strip_prefix("spring(")
        .and_then(|s| s.strip_suffix(')'))
    {
        let nums: Result<Vec<f64>, _> = args_str
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect();
        if let Ok(v) = nums {
            if v.len() == 2 {
                return Ok(kine_core::Easing::Spring {
                    stiffness: v[0],
                    damping: v[1],
                });
            }
        }
    }

    if let Some(args_str) = lower
        .strip_prefix("cubic-bezier(")
        .or_else(|| lower.strip_prefix("cubicbezier("))
        .and_then(|s| s.strip_suffix(')'))
    {
        let nums: Result<Vec<f64>, _> = args_str
            .split(',')
            .map(|p| p.trim().parse::<f64>())
            .collect();
        if let Ok(v) = nums {
            if v.len() == 4 {
                return Ok(kine_core::Easing::CubicBezier {
                    x1: v[0],
                    y1: v[1],
                    x2: v[2],
                    y2: v[3],
                });
            }
        }
    }

    if let Some(args_str) = lower
        .strip_prefix("steps(")
        .and_then(|s| s.strip_suffix(')'))
    {
        if let Ok(n) = args_str.trim().parse::<u32>() {
            return Ok(kine_core::Easing::Steps { steps: n.max(1) });
        }
    }

    Err(AuthoringError::UnknownEasing(raw.to_string()))
}

fn apply_layout(l: &mut taffy::Style, def: &NodeDef) {
    if let Some(size) = &def.size {
        if let Some(w) = size.width {
            l.size.width = taffy::Dimension::length(w);
        }
        if let Some(h) = size.height {
            l.size.height = taffy::Dimension::length(h);
        }
    }
    if let Some(min_size) = &def.min_size {
        if let Some(w) = min_size.width {
            l.min_size.width = taffy::LengthPercentageAuto::length(w);
        }
        if let Some(h) = min_size.height {
            l.min_size.height = taffy::LengthPercentageAuto::length(h);
        }
    }
    if let Some(max_size) = &def.max_size {
        if let Some(w) = max_size.width {
            l.max_size.width = taffy::LengthPercentageAuto::length(w);
        }
        if let Some(h) = max_size.height {
            l.max_size.height = taffy::LengthPercentageAuto::length(h);
        }
    }
    if let Some(inset) = &def.inset {
        l.inset = taffy::Rect {
            top: inset
                .top
                .map_or(taffy::LengthPercentageAuto::auto(), taffy::LengthPercentageAuto::length),
            left: inset
                .left
                .map_or(taffy::LengthPercentageAuto::auto(), taffy::LengthPercentageAuto::length),
            right: inset
                .right
                .map_or(taffy::LengthPercentageAuto::auto(), taffy::LengthPercentageAuto::length),
            bottom: inset
                .bottom
                .map_or(taffy::LengthPercentageAuto::auto(), taffy::LengthPercentageAuto::length),
        };
    }
    if let Some(margin) = &def.margin {
        l.margin = taffy::Rect {
            top: margin
                .top
                .map_or(taffy::LengthPercentageAuto::length(0.0), taffy::LengthPercentageAuto::length),
            left: margin
                .left
                .map_or(taffy::LengthPercentageAuto::length(0.0), taffy::LengthPercentageAuto::length),
            right: margin
                .right
                .map_or(taffy::LengthPercentageAuto::length(0.0), taffy::LengthPercentageAuto::length),
            bottom: margin
                .bottom
                .map_or(taffy::LengthPercentageAuto::length(0.0), taffy::LengthPercentageAuto::length),
        };
    }
    if let Some(padding) = &def.padding {
        l.padding = taffy::Rect {
            top: padding
                .top
                .map_or(taffy::LengthPercentage::length(0.0), taffy::LengthPercentage::length),
            left: padding
                .left
                .map_or(taffy::LengthPercentage::length(0.0), taffy::LengthPercentage::length),
            right: padding
                .right
                .map_or(taffy::LengthPercentage::length(0.0), taffy::LengthPercentage::length),
            bottom: padding
                .bottom
                .map_or(taffy::LengthPercentage::length(0.0), taffy::LengthPercentage::length),
        };
    }
    if let Some(pos) = &def.position {
        l.position = if pos.eq_ignore_ascii_case("absolute") {
            taffy::Position::Absolute
        } else {
            taffy::Position::Relative
        };
    }
    if let Some(dir) = &def.flex_direction {
        l.flex_direction = match dir.to_ascii_lowercase().as_str() {
            "row" => taffy::FlexDirection::Row,
            "column" => taffy::FlexDirection::Column,
            "row-reverse" => taffy::FlexDirection::RowReverse,
            "column-reverse" => taffy::FlexDirection::ColumnReverse,
            _ => l.flex_direction,
        };
    }
    if let Some(wrap) = &def.flex_wrap {
        l.flex_wrap = match wrap.to_ascii_lowercase().as_str() {
            "wrap" => taffy::FlexWrap::Wrap,
            "wrap-reverse" => taffy::FlexWrap::WrapReverse,
            "nowrap" | "no-wrap" => taffy::FlexWrap::NoWrap,
            _ => l.flex_wrap,
        };
    }
    if let Some(grow) = def.flex_grow {
        l.flex_grow = grow;
    }
    if let Some(shrink) = def.flex_shrink {
        l.flex_shrink = shrink;
    }
    if let Some(basis) = def.flex_basis {
        l.flex_basis = taffy::Dimension::length(basis);
    }
    if let Some(ar) = def.aspect_ratio {
        l.aspect_ratio = Some(ar);
    }
    if let Some(jc) = &def.justify_content {
        l.justify_content = Some(match jc.to_ascii_lowercase().as_str() {
            "start" | "flex-start" => taffy::JustifyContent::START,
            "center" => taffy::JustifyContent::CENTER,
            "end" | "flex-end" => taffy::JustifyContent::END,
            "space-between" => taffy::JustifyContent::SPACE_BETWEEN,
            "space-around" => taffy::JustifyContent::SPACE_AROUND,
            "space-evenly" => taffy::JustifyContent::SPACE_EVENLY,
            _ => return,
        });
    }
    if let Some(ai) = &def.align_items {
        l.align_items = Some(match ai.to_ascii_lowercase().as_str() {
            "start" | "flex-start" => taffy::AlignItems::START,
            "center" => taffy::AlignItems::CENTER,
            "end" | "flex-end" => taffy::AlignItems::END,
            "stretch" => taffy::AlignItems::STRETCH,
            _ => return,
        });
    }
    if let Some(als) = &def.align_self {
        l.align_self = Some(match als.to_ascii_lowercase().as_str() {
            "start" | "flex-start" => taffy::AlignSelf::START,
            "center" => taffy::AlignSelf::CENTER,
            "end" | "flex-end" => taffy::AlignSelf::END,
            "stretch" => taffy::AlignSelf::STRETCH,
            _ => return,
        });
    }
    if let Some(gap) = def.gap {
        l.gap = taffy::Size {
            width: taffy::LengthPercentage::length(gap),
            height: taffy::LengthPercentage::length(gap),
        };
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex_colors() {
        assert_eq!(parse_color("#1a2b3c").unwrap(), Color::rgb(0x1a, 0x2b, 0x3c));
        assert_eq!(parse_color("#eef").unwrap(), Color::rgb(0xee, 0xee, 0xff));
        assert_eq!(
            parse_color("#11223344").unwrap(),
            Color::rgba(0x11, 0x22, 0x33, 0x44)
        );
    }

    #[test]
    fn rejects_bad_colors() {
        assert!(matches!(parse_color("#xyz"), Err(AuthoringError::InvalidColor(_))));
        assert!(matches!(parse_color("red"), Err(AuthoringError::InvalidColor(_))));
        assert!(matches!(parse_color("#12345"), Err(AuthoringError::InvalidColor(_))));
    }

    #[test]
    fn unknown_kind_is_an_error() {
        let def = SceneDef {
            width: 10,
            height: 10,
            audio: None,
            root: NodeDef {
                kind: Some("sparkle".into()),
                ..Default::default()
            },
        };
        assert!(matches!(
            scene_from_def(def),
            Err(AuthoringError::UnknownKind(_))
        ));
    }

    #[test]
    fn an_audio_bed_round_trips_through_the_schema() {
        let json = r##"{
            "width": 64, "height": 64,
            "root": { "kind": "rect", "fill": "#000000" },
            "audio": {
                "sampleRate": 44100,
                "channels": 2,
                "normalizePeakDb": -1.0,
                "tracks": [
                    { "src": "voice.wav", "gainDb": -3, "start": 0.5, "fadeIn": 0.2, "highpassHz": 80 },
                    { "src": "music.flac", "loop": true, "pan": -0.25, "duckDb": -10, "muted": true }
                ]
            }
        }"##;
        let def: SceneDef = serde_json::from_str(json).unwrap();
        let audio = def.audio.expect("audio block");
        assert_eq!(audio.sample_rate, 44_100);
        assert_eq!(audio.channels, 2);
        assert_eq!(audio.normalize_peak_db, Some(-1.0));
        assert_eq!(audio.tracks.len(), 2);
        assert_eq!(audio.tracks[0].src, "voice.wav");
        assert_eq!(audio.tracks[0].highpass_hz, Some(80.0));
        assert!(audio.tracks[1].loop_track);
        assert!((audio.tracks[1].duck_db + 10.0).abs() < f32::EPSILON);
        assert!(audio.tracks[1].muted);
    }

    #[test]
    fn a_scene_without_audio_has_none() {
        let json = r##"{ "width": 8, "height": 8, "root": {} }"##;
        let def: SceneDef = serde_json::from_str(json).unwrap();
        assert!(def.audio.is_none());
    }

    #[test]
    fn animation_tracks_are_registered_and_evaluated() {
        let json = r##"{
            "width": 100, "height": 100,
            "root": {
                "kind": "rect", "fill": "#123456",
                "size": { "width": 20, "height": 20 },
                "inset": { "top": 50 },
                "animate": {
                    "offsetY": { "from": 40, "to": 0, "start": 0.0, "end": 1.0 },
                    "width": { "from": 0, "to": 60, "start": 0.0, "end": 1.0, "easing": "easeOut" }
                }
            }
        }"##;
        let def: SceneDef = serde_json::from_str(json).unwrap();
        let mut scene = scene_from_def(def).unwrap();
        assert!(scene.is_animated());
        assert_eq!(scene.animations.len(), 2);

        scene.apply_time(0.0);
        assert_eq!(
            scene.nodes[&scene.root].layout.inset.top,
            taffy::LengthPercentageAuto::length(90.0),
            "base 50 + 40"
        );

        scene.apply_time(1.0);
        assert_eq!(
            scene.nodes[&scene.root].layout.inset.top,
            taffy::LengthPercentageAuto::length(50.0),
            "base 50 + 0"
        );
        assert_eq!(
            scene.nodes[&scene.root].layout.size.width,
            taffy::Dimension::length(80.0),
            "base 20 + 60"
        );

        scene.apply_time(0.5);
        let first = scene.nodes[&scene.root].layout.inset.top;
        scene.apply_time(0.5);
        let second = scene.nodes[&scene.root].layout.inset.top;
        assert_eq!(first, second, "apply_time must be idempotent");
    }

    #[test]
    fn a_bad_animation_is_rejected() {
        let unknown = r##"{ "width": 8, "height": 8, "root": { "animate": { "colour": { "from": 0, "to": 1 } } } }"##;
        let def: SceneDef = serde_json::from_str(unknown).unwrap();
        assert!(matches!(
            scene_from_def(def),
            Err(AuthoringError::UnknownAnimProperty(_))
        ));

        let empty = r##"{ "width": 8, "height": 8, "root": { "animate": { "width": { "from": 0, "to": 1, "start": 2.0, "end": 2.0 } } } }"##;
        let def: SceneDef = serde_json::from_str(empty).unwrap();
        assert!(matches!(
            scene_from_def(def),
            Err(AuthoringError::EmptyAnimationRange { .. })
        ));

        let easing = r##"{ "width": 8, "height": 8, "root": { "animate": { "width": { "from": 0, "to": 1, "easing": "bouncy" } } } }"##;
        let def: SceneDef = serde_json::from_str(easing).unwrap();
        assert!(matches!(
            scene_from_def(def),
            Err(AuthoringError::UnknownEasing(_))
        ));
    }

    #[test]
    fn a_shader_node_carries_its_source_and_uniforms() {
        let json = r##"{
            "width": 64, "height": 64,
            "root": {
                "kind": "shader",
                "shader": "@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> { return vec4<f32>(uv, 0.0, 1.0); }",
                "uniforms": { "speed": 2.0 },
                "blend": "add"
            }
        }"##;
        let def: SceneDef = serde_json::from_str(json).unwrap();
        let scene = scene_from_def(def).unwrap();
        match &scene.nodes[&scene.root].kind {
            NodeKind::Shader {
                source,
                uniforms,
                blend,
            } => {
                assert!(source.contains("@fragment"));
                assert_eq!(uniforms.get("speed").unwrap().as_f64(), Some(2.0));
                assert_eq!(*blend, kine_core::ShaderBlend::Add);
            }
            other => panic!("expected a shader node, got {other:?}"),
        }
    }

    #[test]
    fn a_shader_node_without_source_is_an_error() {
        let json = r##"{ "width": 8, "height": 8, "root": { "kind": "shader" } }"##;
        let def: SceneDef = serde_json::from_str(json).unwrap();
        assert!(matches!(
            scene_from_def(def),
            Err(AuthoringError::MissingShader)
        ));
    }

    #[test]
    fn builds_nested_scene() {
        let json = r##"{
            "width": 100, "height": 50,
            "root": {
                "kind": "container",
                "children": [
                    { "kind": "rect", "fill": "#102030", "size": {"width": 20, "height": 10} },
                    { "kind": "text", "content": "hi", "fontSize": 12, "color": "#fff" }
                ]
            }
        }"##;
        let def: SceneDef = serde_json::from_str(json).unwrap();
        let mut scene = scene_from_def(def).unwrap();
        assert_eq!(scene.nodes.len(), 3);
        scene.compute_layout().unwrap();
        let text = scene
            .nodes
            .values()
            .find(|n| matches!(n.kind, NodeKind::Text { .. }))
            .unwrap();
        match &text.kind {
            NodeKind::Text { font_size, .. } => assert_eq!(*font_size, 12.0),
            _ => unreachable!(),
        }
    }
}
