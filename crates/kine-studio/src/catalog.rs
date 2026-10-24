//! Starter compositions, project scene discovery, and the insertable block /
//! WGSL shader preset catalog for Kine Studio.

use kine_script::{
    AnimationDef, AudioDef, AudioTrackDef, EdgesDef, GradientDef, GradientStopDef, KeyframeDef,
    NodeDef, SceneDef, SizeDef,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// An available scene composition in the Left Dock's `Compositions` panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompositionEntry {
    pub id: String,
    pub title: String,
    pub subtitle: String,
    pub source_path: Option<PathBuf>,
    pub scene_def: SceneDef,
}

/// A discovered or built-in media/font asset in the Left Dock's `Assets` panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AssetEntry {
    pub name: String,
    pub kind: AssetKind,
    pub uri: String,
    pub details: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AssetKind {
    Font,
    Image,
    Video,
    Audio,
    Shader,
}

impl AssetKind {
    pub const fn badge(self) -> &'static str {
        match self {
            Self::Font => "FONT",
            Self::Image => "IMG",
            Self::Video => "VID",
            Self::Audio => "AUD",
            Self::Shader => "WGSL",
        }
    }
}

/// An insertable node or shader preset in the Left Dock's `Catalog` panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CatalogItem {
    pub id: &'static str,
    pub category: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub const CATALOG_ITEMS: &[CatalogItem] = &[
    CatalogItem {
        id: "rect-card",
        category: "Primitives",
        label: "Rounded Card (Rect)",
        description: "Filled rectangle with rounded corners, subtle border, and drop shadow.",
    },
    CatalogItem {
        id: "circle-badge",
        category: "Primitives",
        label: "Circle Accent Badge",
        description: "Circular shape with animated scale pulse.",
    },
    CatalogItem {
        id: "text-headline",
        category: "Typography",
        label: "Kinetic Headline Text",
        description: "JetBrains Mono headline with slide-up + opacity entrance animation.",
    },
    CatalogItem {
        id: "svg-path-star",
        category: "Vector",
        label: "SVG Vector Star",
        description: "Crisp Bezier/polygon vector path rendered via vello.",
    },
    CatalogItem {
        id: "image-checkerboard",
        category: "Media",
        label: "Procedural Image (Checkerboard)",
        description: "Built-in pattern:checkerboard raster image node.",
    },
    CatalogItem {
        id: "video-testbars",
        category: "Media",
        label: "Procedural Video (SMPTE Bars)",
        description: "Sampled timeline video clip (video:testbars) with trim & rate controls.",
    },
    CatalogItem {
        id: "shader-film-grain",
        category: "WGSL Shaders",
        label: "Shader: Film Grain",
        description: "Hash-noise film grain post-process pass over underlying layers.",
    },
    CatalogItem {
        id: "shader-vignette",
        category: "WGSL Shaders",
        label: "Shader: Vignette",
        description: "Smooth radial edge darkening WGSL fragment shader.",
    },
    CatalogItem {
        id: "shader-chromatic",
        category: "WGSL Shaders",
        label: "Shader: Chromatic Aberration",
        description: "RGB channel offset lens dispersion WGSL pass.",
    },
    CatalogItem {
        id: "shader-scanlines",
        category: "WGSL Shaders",
        label: "Shader: CRT Scanlines",
        description: "Horizontal CRT scanline modulation WGSL pass.",
    },
    CatalogItem {
        id: "audio-synth-chime",
        category: "Audio DSP",
        label: "Audio Track: 440 Hz Sine Bed",
        description: "Synthesized sine wave audio track with fade-in/out and highpass filter.",
    },
];

/// Build the `NodeDef` for a catalog item so it can be inserted into the active scene.
pub fn build_catalog_node(item_id: &str, canvas_w: u32, canvas_h: u32) -> Option<NodeDef> {
    let cw = canvas_w as f32;
    let ch = canvas_h as f32;
    match item_id {
        "rect-card" => {
            let mut anim = BTreeMap::new();
            anim.insert(
                "opacity".to_string(),
                AnimationDef {
                    from: 0.0,
                    to: 1.0,
                    start: 0.0,
                    end: 0.8,
                    keyframes: None,
                    easing: Some("easeOut".to_string()),
                },
            );
            Some(NodeDef {
                kind: Some("rect".to_string()),
                fill: Some("#1e293b".to_string()),
                corner_radius: Some(16.0),
                stroke_color: Some("#38bdf8".to_string()),
                stroke_width: Some(2.0),
                shadow_color: Some("#00000088".to_string()),
                shadow_blur: Some(18.0),
                shadow_offset_y: Some(6.0),
                position: Some("absolute".to_string()),
                inset: Some(EdgesDef {
                    left: Some((cw * 0.2).round()),
                    top: Some((ch * 0.25).round()),
                    ..Default::default()
                }),
                size: Some(SizeDef {
                    width: Some((cw * 0.6).round().max(160.0)),
                    height: Some((ch * 0.45).round().max(100.0)),
                }),
                animate: Some(anim),
                ..Default::default()
            })
        }
        "circle-badge" => {
            let mut anim = BTreeMap::new();
            anim.insert(
                "scale".to_string(),
                AnimationDef {
                    from: 0.5,
                    to: 1.0,
                    start: 0.0,
                    end: 1.2,
                    keyframes: None,
                    easing: Some("spring(180, 18)".to_string()),
                },
            );
            Some(NodeDef {
                kind: Some("circle".to_string()),
                fill: Some("#f43f5e".to_string()),
                radius: Some(42.0),
                position: Some("absolute".to_string()),
                inset: Some(EdgesDef {
                    left: Some((cw * 0.5 - 42.0).round()),
                    top: Some((ch * 0.5 - 42.0).round()),
                    ..Default::default()
                }),
                size: Some(SizeDef {
                    width: Some(84.0),
                    height: Some(84.0),
                }),
                animate: Some(anim),
                ..Default::default()
            })
        }
        "text-headline" => {
            let mut anim = BTreeMap::new();
            anim.insert(
                "translateY".to_string(),
                AnimationDef {
                    from: 24.0,
                    to: 0.0,
                    start: 0.0,
                    end: 1.0,
                    keyframes: None,
                    easing: Some("easeOutCubic".to_string()),
                },
            );
            anim.insert(
                "opacity".to_string(),
                AnimationDef {
                    from: 0.0,
                    to: 1.0,
                    start: 0.0,
                    end: 0.7,
                    keyframes: None,
                    easing: Some("easeOut".to_string()),
                },
            );
            Some(NodeDef {
                kind: Some("text".to_string()),
                content: Some("KINE STUDIO // XILEM".to_string()),
                font_family: Some("JetBrains Mono".to_string()),
                font_size: Some(28.0),
                font_weight: Some(700),
                color: Some("#f8fafc".to_string()),
                align: Some("center".to_string()),
                position: Some("absolute".to_string()),
                inset: Some(EdgesDef {
                    left: Some(40.0),
                    top: Some((ch * 0.42).round()),
                    ..Default::default()
                }),
                size: Some(SizeDef {
                    width: Some((cw - 80.0).max(160.0)),
                    height: Some(48.0),
                }),
                animate: Some(anim),
                ..Default::default()
            })
        }
        "svg-path-star" => Some(NodeDef {
            kind: Some("path".to_string()),
            d: Some(
                "M 40 4 L 50 28 L 76 28 L 55 44 L 63 68 L 40 53 L 17 68 L 25 44 L 4 28 L 30 28 Z"
                    .to_string(),
            ),
            fill: Some("#fbbf24".to_string()),
            stroke_color: Some("#f59e0b".to_string()),
            stroke_width: Some(2.0),
            position: Some("absolute".to_string()),
            inset: Some(EdgesDef {
                left: Some(48.0),
                top: Some(48.0),
                ..Default::default()
            }),
            size: Some(SizeDef {
                width: Some(80.0),
                height: Some(80.0),
            }),
            ..Default::default()
        }),
        "image-checkerboard" => Some(NodeDef {
            kind: Some("image".to_string()),
            src: Some("pattern:checkerboard".to_string()),
            object_fit: Some("cover".to_string()),
            corner_radius: Some(12.0),
            position: Some("absolute".to_string()),
            inset: Some(EdgesDef {
                left: Some(32.0),
                top: Some(32.0),
                ..Default::default()
            }),
            size: Some(SizeDef {
                width: Some(160.0),
                height: Some(120.0),
            }),
            ..Default::default()
        }),
        "video-testbars" => Some(NodeDef {
            kind: Some("video".to_string()),
            src: Some("video:testbars".to_string()),
            object_fit: Some("cover".to_string()),
            corner_radius: Some(10.0),
            start: Some(0.0),
            trim_start: Some(0.0),
            playback_rate: Some(1.0),
            loop_video: Some(true),
            position: Some("absolute".to_string()),
            inset: Some(EdgesDef {
                left: Some(48.0),
                top: Some(48.0),
                ..Default::default()
            }),
            size: Some(SizeDef {
                width: Some(240.0),
                height: Some(135.0),
            }),
            ..Default::default()
        }),
        "shader-film-grain" => {
            let mut uniforms = serde_json::Map::new();
            uniforms.insert("amount".to_string(), serde_json::json!(0.08));
            uniforms.insert("seed".to_string(), serde_json::json!(1.0));
            Some(NodeDef {
                kind: Some("shader".to_string()),
                shader: Some(SHADER_FILM_GRAIN.to_string()),
                uniforms: Some(uniforms),
                blend: Some("normal".to_string()),
                position: Some("absolute".to_string()),
                inset: Some(EdgesDef {
                    left: Some(0.0),
                    top: Some(0.0),
                    ..Default::default()
                }),
                size: Some(SizeDef {
                    width: Some(cw),
                    height: Some(ch),
                }),
                ..Default::default()
            })
        }
        "shader-vignette" => {
            let mut uniforms = serde_json::Map::new();
            uniforms.insert("strength".to_string(), serde_json::json!(0.55));
            uniforms.insert("softness".to_string(), serde_json::json!(0.40));
            Some(NodeDef {
                kind: Some("shader".to_string()),
                shader: Some(SHADER_VIGNETTE.to_string()),
                uniforms: Some(uniforms),
                blend: Some("normal".to_string()),
                position: Some("absolute".to_string()),
                inset: Some(EdgesDef {
                    left: Some(0.0),
                    top: Some(0.0),
                    ..Default::default()
                }),
                size: Some(SizeDef {
                    width: Some(cw),
                    height: Some(ch),
                }),
                ..Default::default()
            })
        }
        "shader-chromatic" => {
            let mut uniforms = serde_json::Map::new();
            uniforms.insert("offset".to_string(), serde_json::json!(0.006));
            Some(NodeDef {
                kind: Some("shader".to_string()),
                shader: Some(SHADER_CHROMATIC.to_string()),
                uniforms: Some(uniforms),
                blend: Some("normal".to_string()),
                position: Some("absolute".to_string()),
                inset: Some(EdgesDef {
                    left: Some(0.0),
                    top: Some(0.0),
                    ..Default::default()
                }),
                size: Some(SizeDef {
                    width: Some(cw),
                    height: Some(ch),
                }),
                ..Default::default()
            })
        }
        "shader-scanlines" => {
            let mut uniforms = serde_json::Map::new();
            uniforms.insert("density".to_string(), serde_json::json!(180.0));
            uniforms.insert("opacity".to_string(), serde_json::json!(0.18));
            Some(NodeDef {
                kind: Some("shader".to_string()),
                shader: Some(SHADER_SCANLINES.to_string()),
                uniforms: Some(uniforms),
                blend: Some("normal".to_string()),
                position: Some("absolute".to_string()),
                inset: Some(EdgesDef {
                    left: Some(0.0),
                    top: Some(0.0),
                    ..Default::default()
                }),
                size: Some(SizeDef {
                    width: Some(cw),
                    height: Some(ch),
                }),
                ..Default::default()
            })
        }
        _ => None,
    }
}

pub const SHADER_FILM_GRAIN: &str = r#"struct Params { amount: f32, seed: f32 };
@group(1) @binding(0) var<uniform> p: Params;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let h = fract(sin(dot(uv + vec2<f32>(kine_frame.time * 0.73 + p.seed, kine_frame.frame * 0.17), vec2<f32>(12.9898, 78.233))) * 43758.5453);
    let n = (h - 0.5) * p.amount;
    return vec4<f32>(clamp(base.rgb + vec3<f32>(n), vec3<f32>(0.0), vec3<f32>(1.0)), base.a);
}
"#;

pub const SHADER_VIGNETTE: &str = r#"struct Params { strength: f32, softness: f32 };
@group(1) @binding(0) var<uniform> p: Params;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let d = distance(uv, vec2<f32>(0.5, 0.5));
    let vig = smoothstep(0.8 - p.softness * 0.4, 0.2, d * (1.0 + p.strength));
    return vec4<f32>(base.rgb * vig, base.a);
}
"#;

pub const SHADER_CHROMATIC: &str = r#"struct Params { offset: f32 };
@group(1) @binding(0) var<uniform> p: Params;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let dir = (uv - vec2<f32>(0.5, 0.5)) * p.offset;
    let r = textureSample(kine_input, kine_sampler, clamp(uv + dir, vec2<f32>(0.0), vec2<f32>(1.0))).r;
    let g = textureSample(kine_input, kine_sampler, uv).g;
    let b = textureSample(kine_input, kine_sampler, clamp(uv - dir, vec2<f32>(0.0), vec2<f32>(1.0))).b;
    let a = textureSample(kine_input, kine_sampler, uv).a;
    return vec4<f32>(r, g, b, a);
}
"#;

pub const SHADER_SCANLINES: &str = r#"struct Params { density: f32, opacity: f32 };
@group(1) @binding(0) var<uniform> p: Params;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let wave = 0.5 + 0.5 * sin(uv.y * p.density * 3.14159265);
    let factor = 1.0 - p.opacity * (1.0 - wave);
    return vec4<f32>(base.rgb * factor, base.a);
}
"#;

/// Built-in starter compositions plus any `.json` / `.ts` scenes discovered under `examples/`.
pub fn default_compositions(workspace_root: Option<&Path>) -> Vec<CompositionEntry> {
    let mut list = vec![
        CompositionEntry {
            id: "kinetic-hero".to_string(),
            title: "01 // Kinetic Hero Title".to_string(),
            subtitle: "640x360 · 30fps · Multi-track keyframe animation + gradient card".to_string(),
            source_path: None,
            scene_def: starter_kinetic_scene(),
        },
        CompositionEntry {
            id: "shader-postfx".to_string(),
            title: "02 // WGSL Post-FX Pipeline".to_string(),
            subtitle: "640x360 · 30fps · Live WGSL Vignette + Film Grain shader stack".to_string(),
            source_path: None,
            scene_def: starter_shader_scene(),
        },
        CompositionEntry {
            id: "nle-av-mix".to_string(),
            title: "03 // NLE Video + Synth Audio Bed".to_string(),
            subtitle: "640x360 · 30fps · Video clip node + DSP audio track".to_string(),
            source_path: None,
            scene_def: starter_nle_av_scene(),
        },
    ];

    if let Some(root) = workspace_root {
        let examples_dir = root.join("examples");
        if let Ok(entries) = std::fs::read_dir(&examples_dir) {
            let mut paths: Vec<PathBuf> = entries
                .filter_map(|e| e.ok().map(|d| d.path()))
                .filter(|p| {
                    p.extension()
                        .and_then(|s| s.to_str())
                        .is_some_and(|ext| ext == "ts" || ext == "json")
                })
                .collect();
            paths.sort();
            for path in paths {
                if let Ok(src) = std::fs::read_to_string(&path) {
                    let parsed = if src.trim_start().starts_with('{') {
                        serde_json::from_str::<SceneDef>(&src).ok()
                    } else {
                        kine_script::eval_scene_def_ts(&src).ok()
                    };
                    if let Some(scene_def) = parsed {
                        let stem = path
                            .file_stem()
                            .and_then(|s| s.to_str())
                            .unwrap_or("example")
                            .to_string();
                        list.push(CompositionEntry {
                            id: format!("example-{stem}"),
                            title: format!("Example // {stem}"),
                            subtitle: format!(
                                "{}x{} · {}",
                                scene_def.width,
                                scene_def.height,
                                path.file_name()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("scene")
                            ),
                            source_path: Some(path),
                            scene_def,
                        });
                    }
                }
            }
        }
    }

    list
}

/// Default asset library shown in the Left Dock `Assets` tab.
pub fn default_assets() -> Vec<AssetEntry> {
    vec![
        AssetEntry {
            name: "JetBrains Mono Regular".to_string(),
            kind: AssetKind::Font,
            uri: "font:JetBrains Mono".to_string(),
            details: "Bundled TrueType font · monospace · weights 400–700".to_string(),
        },
        AssetEntry {
            name: "Procedural Checkerboard".to_string(),
            kind: AssetKind::Image,
            uri: "pattern:checkerboard".to_string(),
            details: "Deterministic RGBA8 calibration pattern".to_string(),
        },
        AssetEntry {
            name: "SMPTE Testbars Clip".to_string(),
            kind: AssetKind::Video,
            uri: "video:testbars".to_string(),
            details: "Procedural multi-frame video source · 30fps".to_string(),
        },
        AssetEntry {
            name: "440 Hz Concert A Sine".to_string(),
            kind: AssetKind::Audio,
            uri: "synth:sine:440:2.5:0.35".to_string(),
            details: "48 kHz stereo synthesized audio source".to_string(),
        },
        AssetEntry {
            name: "Pink Noise Ambient Bed".to_string(),
            kind: AssetKind::Audio,
            uri: "synth:pink-noise:220:3.0:0.15".to_string(),
            details: "48 kHz filtered pink noise bed".to_string(),
        },
        AssetEntry {
            name: "Film Grain (WGSL)".to_string(),
            kind: AssetKind::Shader,
            uri: "shader:filmGrain".to_string(),
            details: "Uniforms: amount (f32), seed (f32)".to_string(),
        },
        AssetEntry {
            name: "Vignette (WGSL)".to_string(),
            kind: AssetKind::Shader,
            uri: "shader:vignette".to_string(),
            details: "Uniforms: strength (f32), softness (f32)".to_string(),
        },
    ]
}

/// Default starter scene loaded when Kine Studio starts without `--scene`.
pub fn starter_kinetic_scene() -> SceneDef {
    let mut card_anim = BTreeMap::new();
    card_anim.insert(
        "translateY".to_string(),
        AnimationDef {
            from: 36.0,
            to: 0.0,
            start: 0.0,
            end: 1.2,
            keyframes: Some(vec![
                KeyframeDef {
                    time: 0.0,
                    value: 36.0,
                },
                KeyframeDef {
                    time: 0.6,
                    value: -4.0,
                },
                KeyframeDef {
                    time: 1.2,
                    value: 0.0,
                },
            ]),
            easing: Some("easeOutCubic".to_string()),
        },
    );
    card_anim.insert(
        "opacity".to_string(),
        AnimationDef {
            from: 0.0,
            to: 1.0,
            start: 0.0,
            end: 0.8,
            keyframes: None,
            easing: Some("easeOut".to_string()),
        },
    );

    let mut badge_anim = BTreeMap::new();
    badge_anim.insert(
        "scale".to_string(),
        AnimationDef {
            from: 0.4,
            to: 1.0,
            start: 0.1,
            end: 1.4,
            keyframes: None,
            easing: Some("spring(180, 18)".to_string()),
        },
    );

    let mut title_anim = BTreeMap::new();
    title_anim.insert(
        "translateX".to_string(),
        AnimationDef {
            from: -28.0,
            to: 0.0,
            start: 0.15,
            end: 1.1,
            keyframes: None,
            easing: Some("easeOutCubic".to_string()),
        },
    );
    title_anim.insert(
        "opacity".to_string(),
        AnimationDef {
            from: 0.1,
            to: 1.0,
            start: 0.15,
            end: 0.95,
            keyframes: None,
            easing: Some("easeOut".to_string()),
        },
    );

    let mut bar_anim = BTreeMap::new();
    bar_anim.insert(
        "width".to_string(),
        AnimationDef {
            from: 0.0,
            to: 360.0,
            start: 0.2,
            end: 2.4,
            keyframes: None,
            easing: Some("easeInOutCubic".to_string()),
        },
    );

    SceneDef {
        width: 640,
        height: 360,
        audio: Some(AudioDef {
            sample_rate: 48_000,
            channels: 2,
            normalize_peak_db: Some(-1.0),
            tracks: vec![AudioTrackDef {
                src: "synth:sine:440:2.4:0.25".to_string(),
                gain_db: -6.0,
                start: 0.0,
                trim_start: 0.0,
                trim_end: Some(2.4),
                loop_track: false,
                pan: 0.0,
                fade_in: 0.15,
                fade_out: 0.35,
                fade_curve: Some("equalPower".to_string()),
                highpass_hz: Some(80.0),
                lowpass_hz: None,
                eq: None,
                compressor: None,
                duck_db: 0.0,
                duck_threshold_db: None,
                muted: false,
            }],
        }),
        root: NodeDef {
            kind: Some("container".to_string()),
            size: Some(SizeDef {
                width: Some(640.0),
                height: Some(360.0),
            }),
            children: Some(vec![
                // #1 Background Rect with linear gradient
                NodeDef {
                    kind: Some("rect".to_string()),
                    fill: Some("#0b0f19".to_string()),
                    gradient: Some(GradientDef {
                        kind: Some("linear".to_string()),
                        angle: Some(135.0),
                        stops: vec![
                            GradientStopDef {
                                offset: 0.0,
                                color: "#090d16".to_string(),
                            },
                            GradientStopDef {
                                offset: 1.0,
                                color: "#1e1b4b".to_string(),
                            },
                        ],
                    }),
                    position: Some("absolute".to_string()),
                    inset: Some(EdgesDef {
                        top: Some(0.0),
                        left: Some(0.0),
                        ..Default::default()
                    }),
                    size: Some(SizeDef {
                        width: Some(640.0),
                        height: Some(360.0),
                    }),
                    ..Default::default()
                },
                // #2 Hero Card Rect
                NodeDef {
                    kind: Some("rect".to_string()),
                    fill: Some("#111827".to_string()),
                    corner_radius: Some(18.0),
                    stroke_color: Some("#38bdf8".to_string()),
                    stroke_width: Some(2.0),
                    shadow_color: Some("#00000099".to_string()),
                    shadow_blur: Some(24.0),
                    shadow_offset_y: Some(8.0),
                    position: Some("absolute".to_string()),
                    inset: Some(EdgesDef {
                        left: Some(60.0),
                        top: Some(64.0),
                        ..Default::default()
                    }),
                    size: Some(SizeDef {
                        width: Some(520.0),
                        height: Some(232.0),
                    }),
                    animate: Some(card_anim),
                    ..Default::default()
                },
                // #3 Pulsing Status Circle Badge
                NodeDef {
                    kind: Some("circle".to_string()),
                    fill: Some("#22d3ee".to_string()),
                    radius: Some(18.0),
                    position: Some("absolute".to_string()),
                    inset: Some(EdgesDef {
                        left: Some(96.0),
                        top: Some(104.0),
                        ..Default::default()
                    }),
                    size: Some(SizeDef {
                        width: Some(36.0),
                        height: Some(36.0),
                    }),
                    animate: Some(badge_anim),
                    ..Default::default()
                },
                // #4 Headline Text
                NodeDef {
                    kind: Some("text".to_string()),
                    content: Some("KINE STUDIO".to_string()),
                    font_family: Some("JetBrains Mono".to_string()),
                    font_size: Some(32.0),
                    font_weight: Some(700),
                    color: Some("#f8fafc".to_string()),
                    align: Some("left".to_string()),
                    position: Some("absolute".to_string()),
                    inset: Some(EdgesDef {
                        left: Some(148.0),
                        top: Some(102.0),
                        ..Default::default()
                    }),
                    size: Some(SizeDef {
                        width: Some(400.0),
                        height: Some(44.0),
                    }),
                    animate: Some(title_anim),
                    ..Default::default()
                },
                // #5 Subtitle Text
                NodeDef {
                    kind: Some("text".to_string()),
                    content: Some("IN-PROCESS XILEM + VELLO ENGINE".to_string()),
                    font_family: Some("JetBrains Mono".to_string()),
                    font_size: Some(16.0),
                    font_weight: Some(400),
                    color: Some("#94a3b8".to_string()),
                    align: Some("left".to_string()),
                    position: Some("absolute".to_string()),
                    inset: Some(EdgesDef {
                        left: Some(96.0),
                        top: Some(164.0),
                        ..Default::default()
                    }),
                    size: Some(SizeDef {
                        width: Some(440.0),
                        height: Some(28.0),
                    }),
                    ..Default::default()
                },
                // #6 Animated Progress Bar Rect
                NodeDef {
                    kind: Some("rect".to_string()),
                    fill: Some("#38bdf8".to_string()),
                    corner_radius: Some(4.0),
                    position: Some("absolute".to_string()),
                    inset: Some(EdgesDef {
                        left: Some(96.0),
                        top: Some(224.0),
                        ..Default::default()
                    }),
                    size: Some(SizeDef {
                        width: Some(80.0),
                        height: Some(10.0),
                    }),
                    animate: Some(bar_anim),
                    ..Default::default()
                },
            ]),
            ..Default::default()
        },
    }
}

pub fn starter_shader_scene() -> SceneDef {
    let mut base = starter_kinetic_scene();
    if let Some(children) = &mut base.root.children {
        if let Some(shader_node) = build_catalog_node("shader-vignette", base.width, base.height) {
            children.push(shader_node);
        }
        if let Some(grain_node) = build_catalog_node("shader-film-grain", base.width, base.height) {
            children.push(grain_node);
        }
    }
    base
}

pub fn starter_nle_av_scene() -> SceneDef {
    let mut base = starter_kinetic_scene();
    if let Some(children) = &mut base.root.children {
        if let Some(video_node) = build_catalog_node("video-testbars", base.width, base.height) {
            children.push(video_node);
        }
    }
    base
}
