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

/// Complete semantic palette for Kine Studio (Graphite / Paper / Obsidian + AccentPreset).
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
    /// Warm Graphite, the primary studio theme.
    pub const fn graphite() -> Self {
        Self {
            bg_shell: Color::from_rgb8(18, 17, 15),
            bg_surface: Color::from_rgb8(32, 30, 27),
            bg_inset: Color::from_rgb8(23, 22, 20),
            bg_raised: Color::from_rgb8(43, 40, 37),
            bg_elevated: Color::from_rgb8(53, 49, 45),
            bg_stage: Color::from_rgb8(13, 12, 11),

            border: Color::from_rgba8(255, 255, 255, 20),
            border_strong: Color::from_rgba8(255, 255, 255, 38),
            border_accent: Color::from_rgba8(61, 228, 168, 115),

            text: Color::from_rgb8(245, 243, 241),
            text_soft: Color::from_rgb8(216, 214, 211),
            muted: Color::from_rgb8(162, 159, 156),
            off: Color::from_rgb8(116, 114, 111),

            accent: Color::from_rgb8(61, 228, 168),
            accent_hover: Color::from_rgb8(84, 245, 184),
            accent_ink: Color::from_rgb8(65, 230, 170),
            on_accent: Color::from_rgb8(0, 28, 17),
            accent_wash: Color::from_rgba8(61, 228, 168, 38),

            selection: Color::from_rgb8(75, 163, 210),
            selection_wash: Color::from_rgba8(75, 163, 210, 40),
            playhead: Color::from_rgb8(245, 243, 241),

            danger: Color::from_rgb8(255, 156, 148),
            danger_wash: Color::from_rgba8(255, 92, 82, 34),
            warning: Color::from_rgb8(255, 185, 0),
            warning_wash: Color::from_rgba8(255, 185, 0, 30),

            media: Color::from_rgb8(0, 227, 255),
            container: Color::from_rgb8(245, 166, 35),
            audio: Color::from_rgb8(155, 114, 242),
            audio_wash: Color::from_rgba8(96, 67, 141, 110),

            clip: Color::from_rgb8(46, 54, 50),
            clip_active: Color::from_rgb8(38, 88, 70),
            clip_shader: Color::from_rgb8(30, 70, 88),

            hover: Color::from_rgba8(255, 255, 255, 18),
            press: Color::from_rgba8(255, 255, 255, 30),
            scrim: Color::from_rgba8(8, 8, 10, 185),
        }
    }

    /// Carbon Obsidian, ultra-deep high-contrast OLED dark theme.
    pub const fn obsidian() -> Self {
        Self {
            bg_shell: Color::from_rgb8(8, 8, 10),
            bg_surface: Color::from_rgb8(18, 18, 22),
            bg_inset: Color::from_rgb8(12, 12, 15),
            bg_raised: Color::from_rgb8(28, 28, 34),
            bg_elevated: Color::from_rgb8(38, 38, 46),
            bg_stage: Color::from_rgb8(5, 5, 7),

            border: Color::from_rgba8(255, 255, 255, 22),
            border_strong: Color::from_rgba8(255, 255, 255, 44),
            border_accent: Color::from_rgba8(61, 228, 168, 130),

            text: Color::from_rgb8(250, 250, 252),
            text_soft: Color::from_rgb8(224, 224, 230),
            muted: Color::from_rgb8(158, 160, 170),
            off: Color::from_rgb8(108, 110, 120),

            accent: Color::from_rgb8(61, 228, 168),
            accent_hover: Color::from_rgb8(92, 248, 190),
            accent_ink: Color::from_rgb8(72, 236, 176),
            on_accent: Color::from_rgb8(0, 24, 14),
            accent_wash: Color::from_rgba8(61, 228, 168, 42),

            selection: Color::from_rgb8(56, 189, 248),
            selection_wash: Color::from_rgba8(56, 189, 248, 44),
            playhead: Color::from_rgb8(255, 255, 255),

            danger: Color::from_rgb8(255, 128, 120),
            danger_wash: Color::from_rgba8(255, 82, 72, 38),
            warning: Color::from_rgb8(255, 196, 24),
            warning_wash: Color::from_rgba8(255, 196, 24, 34),

            media: Color::from_rgb8(34, 211, 238),
            container: Color::from_rgb8(251, 191, 36),
            audio: Color::from_rgb8(168, 130, 255),
            audio_wash: Color::from_rgba8(109, 76, 178, 120),

            clip: Color::from_rgb8(34, 42, 46),
            clip_active: Color::from_rgb8(32, 84, 68),
            clip_shader: Color::from_rgb8(26, 64, 86),

            hover: Color::from_rgba8(255, 255, 255, 20),
            press: Color::from_rgba8(255, 255, 255, 34),
            scrim: Color::from_rgba8(4, 4, 6, 205),
        }
    }

    /// Warm Paper, a real daylight variant (not an inverted dark theme).
    pub const fn paper() -> Self {
        Self {
            bg_shell: Color::from_rgb8(236, 234, 229),
            bg_surface: Color::from_rgb8(253, 251, 248),
            bg_inset: Color::from_rgb8(244, 242, 238),
            bg_raised: Color::from_rgb8(255, 255, 255),
            bg_elevated: Color::from_rgb8(247, 245, 240),
            bg_stage: Color::from_rgb8(24, 23, 21),

            border: Color::from_rgba8(0, 0, 0, 22),
            border_strong: Color::from_rgba8(0, 0, 0, 42),
            border_accent: Color::from_rgba8(1, 130, 91, 120),

            text: Color::from_rgb8(24, 23, 22),
            text_soft: Color::from_rgb8(68, 67, 65),
            muted: Color::from_rgb8(105, 103, 100),
            off: Color::from_rgb8(142, 140, 136),

            accent: Color::from_rgb8(1, 130, 91),
            accent_hover: Color::from_rgb8(2, 113, 79),
            accent_ink: Color::from_rgb8(0, 108, 76),
            on_accent: Color::from_rgb8(255, 255, 255),
            accent_wash: Color::from_rgba8(1, 130, 91, 30),

            selection: Color::from_rgb8(0, 106, 165),
            selection_wash: Color::from_rgba8(0, 106, 165, 30),
            playhead: Color::from_rgb8(220, 38, 38),

            danger: Color::from_rgb8(204, 51, 54),
            danger_wash: Color::from_rgba8(204, 51, 54, 28),
            warning: Color::from_rgb8(142, 84, 0),
            warning_wash: Color::from_rgba8(245, 166, 35, 34),

            media: Color::from_rgb8(0, 113, 135),
            container: Color::from_rgb8(161, 97, 0),
            audio: Color::from_rgb8(96, 67, 141),
            audio_wash: Color::from_rgba8(96, 67, 141, 34),

            clip: Color::from_rgb8(222, 233, 229),
            clip_active: Color::from_rgb8(178, 226, 208),
            clip_shader: Color::from_rgb8(206, 234, 246),

            hover: Color::from_rgba8(0, 0, 0, 14),
            press: Color::from_rgba8(0, 0, 0, 28),
            scrim: Color::from_rgba8(15, 14, 12, 130),
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
            AccentPreset::KineMint => {}
            AccentPreset::CinemaAmber => {
                if dark {
                    p.accent = Color::from_rgb8(245, 166, 35);
                    p.accent_hover = Color::from_rgb8(255, 184, 64);
                    p.accent_ink = Color::from_rgb8(247, 178, 59);
                    p.on_accent = Color::from_rgb8(28, 16, 0);
                    p.accent_wash = Color::from_rgba8(245, 166, 35, 42);
                    p.border_accent = Color::from_rgba8(245, 166, 35, 120);
                } else {
                    p.accent = Color::from_rgb8(180, 105, 0);
                    p.accent_hover = Color::from_rgb8(155, 88, 0);
                    p.accent_ink = Color::from_rgb8(160, 92, 0);
                    p.on_accent = Color::from_rgb8(255, 255, 255);
                    p.accent_wash = Color::from_rgba8(180, 105, 0, 30);
                    p.border_accent = Color::from_rgba8(180, 105, 0, 120);
                }
            }
            AccentPreset::BroadcastCyan => {
                if dark {
                    p.accent = Color::from_rgb8(0, 227, 255);
                    p.accent_hover = Color::from_rgb8(71, 236, 255);
                    p.accent_ink = Color::from_rgb8(38, 231, 255);
                    p.on_accent = Color::from_rgb8(0, 24, 30);
                    p.accent_wash = Color::from_rgba8(0, 227, 255, 40);
                    p.border_accent = Color::from_rgba8(0, 227, 255, 120);
                } else {
                    p.accent = Color::from_rgb8(0, 118, 148);
                    p.accent_hover = Color::from_rgb8(0, 98, 124);
                    p.accent_ink = Color::from_rgb8(0, 104, 132);
                    p.on_accent = Color::from_rgb8(255, 255, 255);
                    p.accent_wash = Color::from_rgba8(0, 118, 148, 30);
                    p.border_accent = Color::from_rgba8(0, 118, 148, 120);
                }
            }
            AccentPreset::StudioViolet => {
                if dark {
                    p.accent = Color::from_rgb8(155, 114, 242);
                    p.accent_hover = Color::from_rgb8(177, 143, 247);
                    p.accent_ink = Color::from_rgb8(168, 130, 245);
                    p.on_accent = Color::from_rgb8(18, 8, 36);
                    p.accent_wash = Color::from_rgba8(155, 114, 242, 42);
                    p.border_accent = Color::from_rgba8(155, 114, 242, 120);
                } else {
                    p.accent = Color::from_rgb8(104, 62, 196);
                    p.accent_hover = Color::from_rgb8(86, 48, 170);
                    p.accent_ink = Color::from_rgb8(92, 52, 180);
                    p.on_accent = Color::from_rgb8(255, 255, 255);
                    p.accent_wash = Color::from_rgba8(104, 62, 196, 30);
                    p.border_accent = Color::from_rgba8(104, 62, 196, 120);
                }
            }
            AccentPreset::SignalCrimson => {
                if dark {
                    p.accent = Color::from_rgb8(255, 107, 107);
                    p.accent_hover = Color::from_rgb8(255, 135, 135);
                    p.accent_ink = Color::from_rgb8(255, 122, 122);
                    p.on_accent = Color::from_rgb8(32, 4, 4);
                    p.accent_wash = Color::from_rgba8(255, 107, 107, 42);
                    p.border_accent = Color::from_rgba8(255, 107, 107, 120);
                } else {
                    p.accent = Color::from_rgb8(198, 40, 40);
                    p.accent_hover = Color::from_rgb8(172, 30, 30);
                    p.accent_ink = Color::from_rgb8(180, 34, 34);
                    p.on_accent = Color::from_rgb8(255, 255, 255);
                    p.accent_wash = Color::from_rgba8(198, 40, 40, 30);
                    p.border_accent = Color::from_rgba8(198, 40, 40, 120);
                }
            }
        }
        p
    }
}

