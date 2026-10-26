//! Xilem UI semantic palette (`StudioPalette`) and pixmap conversion for `kine-studio`.
//!
//! Headless theme tokens (`StudioThemeMode`, `AccentPreset`, `UiDensity`, `StudioSettings`,
//! `StudioAnimState`) are defined in `kine_session::theme` and re-exported here.

pub use kine_session::theme::*;

#[cfg(feature = "xilem-ui")]
use kine_render::RasterPixmap;
#[cfg(feature = "xilem-ui")]
use xilem::Color;

/// Bundled UI face. The font is registered in `run_studio` through `Xilem::with_font`.
#[cfg(feature = "xilem-ui")]
pub const UI_FONT: &str = "Inter Variable";
/// Bundled mono face for timecode, ids, property values and telemetry.
#[cfg(feature = "xilem-ui")]
pub const MONO_FONT: &str = "JetBrains Mono";
/// Google Material Symbols Outlined (ligature icon font).
#[cfg(feature = "xilem-ui")]
pub const ICON_FONT: &str = "Material Symbols Outlined";

/// Convert a rendered [`RasterPixmap`] into a Xilem/Masonry [`xilem::masonry::peniko::ImageData`].
#[cfg(feature = "xilem-ui")]
pub fn pixmap_to_xilem_image(pixmap: &RasterPixmap) -> xilem::masonry::peniko::ImageData {
    use std::sync::Arc;
    use xilem::masonry::peniko::{Blob, ImageAlphaType, ImageData, ImageFormat};
    let w = u32::from(pixmap.width());
    let h = u32::from(pixmap.height());
    let mut bytes = Vec::with_capacity((w as usize) * (h as usize) * 4);
    for px in pixmap.data() {
        bytes.extend_from_slice(&[px.r, px.g, px.b, px.a]);
    }
    ImageData {
        data: Blob::new(Arc::new(bytes)),
        format: ImageFormat::Rgba8,
        alpha_type: ImageAlphaType::AlphaPremultiplied,
        width: w,
        height: h,
    }
}

/// Complete semantic palette for Kine Studio (Gruvbox Material dark/light/soft + accent presets).
#[cfg(feature = "xilem-ui")]
#[derive(Debug, Clone, Copy)]
pub struct StudioPalette {
    /// Window frame behind the floating cards.
    pub bg_shell: Color,
    /// Panel card surface.
    pub bg_surface: Color,
    /// Recessed field / inspector body.
    pub bg_inset: Color,
    /// Raised row, popover, active tab.
    pub bg_raised: Color,
    /// Modal / settings surface.
    pub bg_elevated: Color,
    /// Preview well behind the rendered frame.
    pub bg_stage: Color,

    /// Default 1px separator.
    pub border: Color,
    /// Card / input outline.
    pub border_strong: Color,
    /// Accent outline for selected cards.
    pub border_accent: Color,

    /// Primary ink.
    pub text: Color,
    /// Secondary ink.
    pub text_soft: Color,
    /// Muted labels and metadata.
    pub muted: Color,
    /// Disabled ink.
    pub off: Color,

    /// Primary signal colour.
    pub accent: Color,
    /// Hovered signal colour.
    pub accent_hover: Color,
    /// Signal colour used as text on a surface.
    pub accent_ink: Color,
    /// Ink painted on an accent fill.
    pub on_accent: Color,
    /// Translucent accent wash for toggled controls.
    pub accent_wash: Color,

    /// Selection blue (deliberately distinct from accent).
    pub selection: Color,
    pub selection_wash: Color,
    /// Playhead ink.
    pub playhead: Color,

    pub danger: Color,
    pub danger_wash: Color,
    pub warning: Color,
    pub warning_wash: Color,

    /// Media / video kind colour.
    pub media: Color,
    /// Container / layout node kind colour.
    pub container: Color,
    /// Audio track violet.
    pub audio: Color,
    pub audio_wash: Color,

    /// Neutral timeline clip fill.
    pub clip: Color,
    pub clip_active: Color,
    pub clip_shader: Color,

    /// Hover wash (neutral overlay).
    pub hover: Color,
    /// Press / selected row wash.
    pub press: Color,
    /// Modal scrim.
    pub scrim: Color,
}

#[cfg(feature = "xilem-ui")]
impl StudioPalette {
    /// Gruvbox Material Dark (medium contrast).
    pub const fn graphite() -> Self {
        Self {
            bg_shell: Color::from_rgb8(40, 40, 40),
            bg_surface: Color::from_rgb8(50, 48, 47),
            bg_inset: Color::from_rgb8(29, 32, 33),
            bg_raised: Color::from_rgb8(60, 56, 54),
            bg_elevated: Color::from_rgb8(80, 73, 69),
            bg_stage: Color::from_rgb8(29, 32, 33),

            border: Color::from_rgba8(168, 153, 132, 40),
            border_strong: Color::from_rgba8(168, 153, 132, 70),
            border_accent: Color::from_rgba8(169, 182, 101, 130),

            text: Color::from_rgb8(221, 199, 161),
            text_soft: Color::from_rgb8(212, 190, 152),
            muted: Color::from_rgb8(168, 153, 132),
            off: Color::from_rgb8(146, 131, 116),

            accent: Color::from_rgb8(169, 182, 101),
            accent_hover: Color::from_rgb8(184, 196, 111),
            accent_ink: Color::from_rgb8(169, 182, 101),
            on_accent: Color::from_rgb8(40, 40, 40),
            accent_wash: Color::from_rgba8(169, 182, 101, 40),

            selection: Color::from_rgb8(125, 174, 163),
            selection_wash: Color::from_rgba8(125, 174, 163, 40),
            playhead: Color::from_rgb8(221, 199, 161),

            danger: Color::from_rgb8(234, 105, 98),
            danger_wash: Color::from_rgba8(234, 105, 98, 36),
            warning: Color::from_rgb8(216, 166, 87),
            warning_wash: Color::from_rgba8(216, 166, 87, 34),

            media: Color::from_rgb8(125, 174, 163),
            container: Color::from_rgb8(216, 166, 87),
            audio: Color::from_rgb8(211, 134, 155),
            audio_wash: Color::from_rgba8(211, 134, 155, 110),

            clip: Color::from_rgb8(60, 56, 54),
            clip_active: Color::from_rgb8(74, 92, 58),
            clip_shader: Color::from_rgb8(47, 62, 68),

            hover: Color::from_rgba8(221, 199, 161, 18),
            press: Color::from_rgba8(221, 199, 161, 30),
            scrim: Color::from_rgba8(29, 32, 33, 185),
        }
    }

    /// Gruvbox Material Soft (lighter dark surfaces).
    pub const fn obsidian() -> Self {
        Self {
            bg_shell: Color::from_rgb8(50, 48, 47),
            bg_surface: Color::from_rgb8(60, 56, 54),
            bg_inset: Color::from_rgb8(40, 40, 40),
            bg_raised: Color::from_rgb8(80, 73, 69),
            bg_elevated: Color::from_rgb8(102, 92, 84),
            bg_stage: Color::from_rgb8(29, 32, 33),

            border: Color::from_rgba8(168, 153, 132, 44),
            border_strong: Color::from_rgba8(168, 153, 132, 76),
            border_accent: Color::from_rgba8(169, 182, 101, 136),

            text: Color::from_rgb8(221, 199, 161),
            text_soft: Color::from_rgb8(212, 190, 152),
            muted: Color::from_rgb8(168, 153, 132),
            off: Color::from_rgb8(146, 131, 116),

            accent: Color::from_rgb8(169, 182, 101),
            accent_hover: Color::from_rgb8(189, 201, 118),
            accent_ink: Color::from_rgb8(169, 182, 101),
            on_accent: Color::from_rgb8(40, 40, 40),
            accent_wash: Color::from_rgba8(169, 182, 101, 44),

            selection: Color::from_rgb8(125, 174, 163),
            selection_wash: Color::from_rgba8(125, 174, 163, 44),
            playhead: Color::from_rgb8(221, 199, 161),

            danger: Color::from_rgb8(234, 105, 98),
            danger_wash: Color::from_rgba8(234, 105, 98, 40),
            warning: Color::from_rgb8(216, 166, 87),
            warning_wash: Color::from_rgba8(216, 166, 87, 38),

            media: Color::from_rgb8(125, 174, 163),
            container: Color::from_rgb8(216, 166, 87),
            audio: Color::from_rgb8(211, 134, 155),
            audio_wash: Color::from_rgba8(211, 134, 155, 118),

            clip: Color::from_rgb8(80, 73, 69),
            clip_active: Color::from_rgb8(92, 108, 74),
            clip_shader: Color::from_rgb8(62, 78, 84),

            hover: Color::from_rgba8(221, 199, 161, 20),
            press: Color::from_rgba8(221, 199, 161, 34),
            scrim: Color::from_rgba8(29, 32, 33, 205),
        }
    }

    /// Gruvbox Material Light.
    pub const fn paper() -> Self {
        Self {
            bg_shell: Color::from_rgb8(235, 219, 178),
            bg_surface: Color::from_rgb8(242, 229, 188),
            bg_inset: Color::from_rgb8(224, 209, 168),
            bg_raised: Color::from_rgb8(249, 245, 215),
            bg_elevated: Color::from_rgb8(249, 245, 215),
            bg_stage: Color::from_rgb8(40, 40, 40),

            border: Color::from_rgba8(101, 71, 53, 28),
            border_strong: Color::from_rgba8(101, 71, 53, 50),
            border_accent: Color::from_rgba8(108, 120, 46, 120),

            text: Color::from_rgb8(101, 71, 53),
            text_soft: Color::from_rgb8(81, 64, 54),
            muted: Color::from_rgb8(124, 111, 100),
            off: Color::from_rgb8(168, 153, 132),

            accent: Color::from_rgb8(108, 120, 46),
            accent_hover: Color::from_rgb8(125, 138, 55),
            accent_ink: Color::from_rgb8(92, 104, 38),
            on_accent: Color::from_rgb8(242, 229, 188),
            accent_wash: Color::from_rgba8(108, 120, 46, 30),

            selection: Color::from_rgb8(69, 112, 122),
            selection_wash: Color::from_rgba8(69, 112, 122, 30),
            playhead: Color::from_rgb8(193, 74, 74),

            danger: Color::from_rgb8(193, 74, 74),
            danger_wash: Color::from_rgba8(193, 74, 74, 28),
            warning: Color::from_rgb8(180, 113, 9),
            warning_wash: Color::from_rgba8(180, 113, 9, 30),

            media: Color::from_rgb8(69, 112, 122),
            container: Color::from_rgb8(180, 113, 9),
            audio: Color::from_rgb8(148, 94, 128),
            audio_wash: Color::from_rgba8(148, 94, 128, 34),

            clip: Color::from_rgb8(224, 209, 168),
            clip_active: Color::from_rgb8(200, 211, 160),
            clip_shader: Color::from_rgb8(201, 219, 224),

            hover: Color::from_rgba8(101, 71, 53, 14),
            press: Color::from_rgba8(101, 71, 53, 28),
            scrim: Color::from_rgba8(40, 40, 40, 130),
        }
    }

    pub const fn for_mode(dark: bool) -> Self {
        if dark {
            Self::graphite()
        } else {
            Self::paper()
        }
    }

    /// Resolve the complete palette from [`StudioSettings`].
    pub const fn from_settings(settings: &StudioSettings) -> Self {
        let mut p = match settings.theme_mode {
            StudioThemeMode::GraphiteDark => Self::graphite(),
            StudioThemeMode::WarmPaper => Self::paper(),
            StudioThemeMode::CarbonObsidian => Self::obsidian(),
        };
        let dark = settings.theme_mode.is_dark();
        match settings.accent_preset {
            AccentPreset::KineMint => {
                if dark {
                    p.accent = Color::from_rgb8(169, 182, 101);
                    p.accent_hover = Color::from_rgb8(184, 196, 111);
                    p.accent_ink = Color::from_rgb8(169, 182, 101);
                    p.on_accent = Color::from_rgb8(40, 40, 40);
                    p.accent_wash = Color::from_rgba8(169, 182, 101, 42);
                    p.border_accent = Color::from_rgba8(169, 182, 101, 130);
                } else {
                    p.accent = Color::from_rgb8(108, 120, 46);
                    p.accent_hover = Color::from_rgb8(125, 138, 55);
                    p.accent_ink = Color::from_rgb8(92, 104, 38);
                    p.on_accent = Color::from_rgb8(242, 229, 188);
                    p.accent_wash = Color::from_rgba8(108, 120, 46, 30);
                    p.border_accent = Color::from_rgba8(108, 120, 46, 120);
                }
            }
            AccentPreset::CinemaAmber => {
                if dark {
                    p.accent = Color::from_rgb8(216, 166, 87);
                    p.accent_hover = Color::from_rgb8(226, 181, 112);
                    p.accent_ink = Color::from_rgb8(216, 166, 87);
                    p.on_accent = Color::from_rgb8(40, 40, 40);
                    p.accent_wash = Color::from_rgba8(216, 166, 87, 42);
                    p.border_accent = Color::from_rgba8(216, 166, 87, 120);
                } else {
                    p.accent = Color::from_rgb8(180, 113, 9);
                    p.accent_hover = Color::from_rgb8(195, 128, 25);
                    p.accent_ink = Color::from_rgb8(160, 100, 8);
                    p.on_accent = Color::from_rgb8(242, 229, 188);
                    p.accent_wash = Color::from_rgba8(180, 113, 9, 30);
                    p.border_accent = Color::from_rgba8(180, 113, 9, 120);
                }
            }
            AccentPreset::BroadcastCyan => {
                if dark {
                    p.accent = Color::from_rgb8(137, 180, 130);
                    p.accent_hover = Color::from_rgb8(153, 194, 146);
                    p.accent_ink = Color::from_rgb8(137, 180, 130);
                    p.on_accent = Color::from_rgb8(40, 40, 40);
                    p.accent_wash = Color::from_rgba8(137, 180, 130, 40);
                    p.border_accent = Color::from_rgba8(137, 180, 130, 120);
                } else {
                    p.accent = Color::from_rgb8(76, 122, 93);
                    p.accent_hover = Color::from_rgb8(92, 138, 108);
                    p.accent_ink = Color::from_rgb8(64, 108, 80);
                    p.on_accent = Color::from_rgb8(242, 229, 188);
                    p.accent_wash = Color::from_rgba8(76, 122, 93, 30);
                    p.border_accent = Color::from_rgba8(76, 122, 93, 120);
                }
            }
            AccentPreset::StudioViolet => {
                if dark {
                    p.accent = Color::from_rgb8(211, 134, 155);
                    p.accent_hover = Color::from_rgb8(221, 153, 171);
                    p.accent_ink = Color::from_rgb8(211, 134, 155);
                    p.on_accent = Color::from_rgb8(40, 40, 40);
                    p.accent_wash = Color::from_rgba8(211, 134, 155, 42);
                    p.border_accent = Color::from_rgba8(211, 134, 155, 120);
                } else {
                    p.accent = Color::from_rgb8(148, 94, 128);
                    p.accent_hover = Color::from_rgb8(164, 110, 144);
                    p.accent_ink = Color::from_rgb8(132, 80, 114);
                    p.on_accent = Color::from_rgb8(242, 229, 188);
                    p.accent_wash = Color::from_rgba8(148, 94, 128, 30);
                    p.border_accent = Color::from_rgba8(148, 94, 128, 120);
                }
            }
            AccentPreset::SignalCrimson => {
                if dark {
                    p.accent = Color::from_rgb8(234, 105, 98);
                    p.accent_hover = Color::from_rgb8(240, 130, 124);
                    p.accent_ink = Color::from_rgb8(234, 105, 98);
                    p.on_accent = Color::from_rgb8(40, 40, 40);
                    p.accent_wash = Color::from_rgba8(234, 105, 98, 42);
                    p.border_accent = Color::from_rgba8(234, 105, 98, 120);
                } else {
                    p.accent = Color::from_rgb8(193, 74, 74);
                    p.accent_hover = Color::from_rgb8(208, 90, 90);
                    p.accent_ink = Color::from_rgb8(176, 62, 62);
                    p.on_accent = Color::from_rgb8(242, 229, 188);
                    p.accent_wash = Color::from_rgba8(193, 74, 74, 30);
                    p.border_accent = Color::from_rgba8(193, 74, 74, 120);
                }
            }
        }
        p
    }
}

