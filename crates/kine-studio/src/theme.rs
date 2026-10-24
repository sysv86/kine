//! Kine Studio design tokens.
//!
//! The values are a deliberate blend of two open-source references:
//! - OpenCode Desktop (`anomalyco/opencode`, MIT): inset shell, floating panel cards,
//!   neutral-only chrome, neutral alpha state washes.
//! - HyperFrames Studio (`heygen-com/hyperframes`, Apache-2.0): warm-graphite surface ladder,
//!   mint signal colour, editor density constants and timeline geometry.
//!
//! Everything is precomputed as `Color` values. Xilem 0.4 has no `color-mix()` or CSS
//! inheritance, so each role is resolved here once and passed down as a `Copy` token set.

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

/// Floating panel card radius.
pub const PANEL_RADIUS: f64 = 10.0;
/// Gap between panel cards and shell edge.
pub const SHELL_GAP: f64 = 6.0;
/// Header height.
pub const HEADER_H: f64 = 40.0;
/// Dock tab height.
pub const TAB_H: f64 = 28.0;
/// Default control height.
pub const CONTROL_H: f64 = 28.0;
/// Compact inspector row height.
pub const ROW_H: f64 = 30.0;
/// Timeline track header width.
pub const TRACK_GUTTER: f64 = 184.0;
/// Timeline track row height.
pub const TRACK_H: f64 = 34.0;
/// Timeline ruler height.
pub const RULER_H: f64 = 24.0;

/// Complete semantic palette for Kine Studio (Graphite / Paper).
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

    /// Kine mint signal colour.
    pub accent: Color,
    /// Hovered mint.
    pub accent_hover: Color,
    /// Mint used as text on a dark surface.
    pub accent_ink: Color,
    /// Ink painted on a mint fill.
    pub on_accent: Color,
    /// Translucent mint wash for toggled controls.
    pub accent_wash: Color,

    /// Selection blue (deliberately distinct from mint).
    pub selection: Color,
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
    /// Warm Graphite, the primary theme.
    pub const fn graphite() -> Self {
        Self {
            bg_shell: Color::from_rgb8(18, 17, 15),
            bg_surface: Color::from_rgb8(32, 30, 27),
            bg_inset: Color::from_rgb8(26, 25, 23),
            bg_raised: Color::from_rgb8(47, 44, 41),
            bg_stage: Color::from_rgb8(14, 13, 12),

            border: Color::from_rgba8(255, 255, 255, 20),
            border_strong: Color::from_rgba8(255, 255, 255, 36),
            border_accent: Color::from_rgba8(61, 228, 168, 110),

            text: Color::from_rgb8(245, 243, 241),
            text_soft: Color::from_rgb8(226, 224, 222),
            muted: Color::from_rgb8(184, 182, 180),
            off: Color::from_rgb8(151, 149, 147),

            accent: Color::from_rgb8(61, 228, 168),
            accent_hover: Color::from_rgb8(84, 245, 184),
            accent_ink: Color::from_rgb8(65, 230, 170),
            on_accent: Color::from_rgb8(0, 28, 17),
            accent_wash: Color::from_rgba8(61, 228, 168, 38),

            selection: Color::from_rgb8(75, 163, 210),
            playhead: Color::from_rgb8(245, 243, 241),

            danger: Color::from_rgb8(255, 156, 148),
            danger_wash: Color::from_rgba8(255, 92, 82, 34),
            warning: Color::from_rgb8(255, 185, 0),
            warning_wash: Color::from_rgba8(255, 185, 0, 30),

            media: Color::from_rgb8(0, 227, 255),
            container: Color::from_rgb8(245, 166, 35),
            audio: Color::from_rgb8(150, 111, 214),
            audio_wash: Color::from_rgba8(96, 67, 141, 120),

            clip: Color::from_rgb8(49, 57, 53),
            clip_active: Color::from_rgb8(40, 92, 73),
            clip_shader: Color::from_rgb8(30, 70, 88),

            hover: Color::from_rgba8(255, 255, 255, 18),
            press: Color::from_rgba8(255, 255, 255, 30),
            scrim: Color::from_rgba8(0, 0, 0, 160),
        }
    }

    /// Warm Paper, a real light variant (not an inverted dark theme).
    pub const fn paper() -> Self {
        Self {
            bg_shell: Color::from_rgb8(236, 234, 229),
            bg_surface: Color::from_rgb8(253, 251, 248),
            bg_inset: Color::from_rgb8(246, 244, 240),
            bg_raised: Color::from_rgb8(255, 255, 255),
            bg_stage: Color::from_rgb8(24, 23, 21),

            border: Color::from_rgba8(0, 0, 0, 22),
            border_strong: Color::from_rgba8(0, 0, 0, 40),
            border_accent: Color::from_rgba8(1, 130, 91, 120),

            text: Color::from_rgb8(24, 23, 22),
            text_soft: Color::from_rgb8(73, 72, 71),
            muted: Color::from_rgb8(101, 100, 98),
            off: Color::from_rgb8(128, 127, 125),

            accent: Color::from_rgb8(1, 130, 91),
            accent_hover: Color::from_rgb8(2, 113, 79),
            accent_ink: Color::from_rgb8(0, 101, 71),
            on_accent: Color::from_rgb8(255, 255, 255),
            accent_wash: Color::from_rgba8(1, 130, 91, 30),

            selection: Color::from_rgb8(0, 106, 165),
            playhead: Color::from_rgb8(24, 23, 22),

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
            scrim: Color::from_rgba8(0, 0, 0, 110),
        }
    }

    pub const fn for_mode(dark: bool) -> Self {
        if dark {
            Self::graphite()
        } else {
            Self::paper()
        }
    }
}

/// Compute simulated stereo L/R audio peak levels in `[0.0, 1.0]` at `current_time`
/// for the live Audio Meter Strip when the composition has an active audio bed.
pub fn sample_audio_meter_levels(
    has_audio: bool,
    muted: bool,
    volume: f32,
    current_time: f64,
) -> (f32, f32) {
    if !has_audio || muted || volume <= 0.001 {
        return (0.0, 0.0);
    }
    let env_l = (0.65 + 0.30 * (current_time * 9.5).sin() as f32).clamp(0.05, 1.0);
    let env_r = (0.62 + 0.32 * (current_time * 11.3 + 0.8).cos() as f32).clamp(0.05, 1.0);
    (
        (env_l * volume).clamp(0.0, 1.0),
        (env_r * volume).clamp(0.0, 1.0),
    )
}
