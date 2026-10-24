//! Hyperframes Studio visual design tokens & styled Xilem widget builders for Kine Studio.
//!
//! Directly translates the **Graphite (Dark)** and **Paper (Light)** OKLCH/sRGB
//! palettes, 10px dock card geometry, 6px shell gutters, and semantic track colors
//! from Hyperframes Studio (`packages/studio/src/styles/theme.css` & `dock.css`).

#[cfg(feature = "xilem-ui")]
use xilem::{
    masonry::parley::style::FontWeight,
    style::Style as _,
    view::{flex_row, label, sized_box, text_button, CrossAxisAlignment},
    Color, WidgetView,
};

/// Complete semantic color palette for Kine Studio (Graphite Dark or Paper Light).
#[cfg(feature = "xilem-ui")]
#[derive(Debug, Clone, Copy)]
pub struct StudioPalette {
    /// Outer window shell background (`--color-bg-0`).
    pub bg_shell: Color,
    /// Dock card surface (`--color-surface`).
    pub bg_surface: Color,
    /// Recessed inset surface for open inspector sections & timeline gutter (`--color-input` / `--timeline-gutter-bg`).
    pub bg_inset: Color,
    /// Raised surface for active tabs, headers, and pills (`--color-raised`).
    pub bg_raised: Color,
    /// Recessed dark preview stage well (stays dark in both themes, matching Hyperframes `PreviewPane`).
    pub bg_preview_well: Color,
    /// Subtle hairline border (`--color-border`).
    pub border_hairline: Color,
    /// Stronger card/control border (`--color-border-strong`).
    pub border_strong: Color,
    /// Primary high-contrast ink (`--color-text-0`).
    pub text_primary: Color,
    /// Secondary body ink (`--color-text-2`).
    pub text_secondary: Color,
    /// Muted label/timecode ink (`--color-text-muted`).
    pub text_muted: Color,
    /// Hyperframes signature mint accent (`--color-accent` `#3ce6ac`).
    pub accent_mint: Color,
    /// Subtle translucent mint wash for active toggles (`--color-on`).
    pub accent_mint_wash: Color,
    /// Brand cyan accent (`--color-brand-gradient-from` `#06e3fa`).
    pub accent_cyan: Color,
    /// Selection sky blue (`--color-selection` `#4ba3d2`).
    pub selection_sky: Color,
    /// Danger / Auto-keyframe recording coral (`--color-danger` `#ef4444`).
    pub danger_coral: Color,
    /// Warning amber (`--color-container` `#f5a623`).
    pub warning_amber: Color,
    /// Visual timeline clip fill (`--timeline-clip-bg`).
    pub clip_visual_bg: Color,
    /// Selected visual timeline clip fill (`--timeline-clip-bg-active`).
    pub clip_visual_active: Color,
    /// Audio timeline clip violet fill (`--timeline-clip-audio-bg`).
    pub clip_audio_bg: Color,
    /// WGSL shader timeline clip teal/cyan fill.
    pub clip_shader_bg: Color,
}

#[cfg(feature = "xilem-ui")]
impl StudioPalette {
    /// Default **Graphite Dark** palette from Hyperframes Studio (`theme.css`).
    pub const fn graphite_dark() -> Self {
        Self {
            bg_shell: Color::from_rgb8(24, 24, 26),
            bg_surface: Color::from_rgb8(31, 31, 34),
            bg_inset: Color::from_rgb8(20, 20, 22),
            bg_raised: Color::from_rgb8(42, 42, 46),
            bg_preview_well: Color::from_rgb8(10, 10, 12),
            border_hairline: Color::from_rgb8(46, 46, 52),
            border_strong: Color::from_rgb8(63, 63, 70),
            text_primary: Color::from_rgb8(244, 244, 245),
            text_secondary: Color::from_rgb8(212, 212, 216),
            text_muted: Color::from_rgb8(152, 152, 160),
            accent_mint: Color::from_rgb8(60, 230, 172),
            accent_mint_wash: Color::from_rgb8(22, 58, 47),
            accent_cyan: Color::from_rgb8(6, 227, 250),
            selection_sky: Color::from_rgb8(75, 163, 210),
            danger_coral: Color::from_rgb8(239, 68, 68),
            warning_amber: Color::from_rgb8(245, 166, 35),
            clip_visual_bg: Color::from_rgb8(38, 52, 49),
            clip_visual_active: Color::from_rgb8(28, 82, 66),
            clip_audio_bg: Color::from_rgb8(88, 56, 150),
            clip_shader_bg: Color::from_rgb8(24, 74, 96),
        }
    }

    /// **Paper Light** palette from Hyperframes Studio (`data-theme="paper"` in `theme.css`).
    pub const fn paper_light() -> Self {
        Self {
            bg_shell: Color::from_rgb8(236, 235, 231),
            bg_surface: Color::from_rgb8(250, 249, 246),
            bg_inset: Color::from_rgb8(242, 240, 236),
            bg_raised: Color::from_rgb8(255, 255, 255),
            bg_preview_well: Color::from_rgb8(14, 14, 17),
            border_hairline: Color::from_rgb8(218, 216, 210),
            border_strong: Color::from_rgb8(196, 193, 186),
            text_primary: Color::from_rgb8(28, 27, 26),
            text_secondary: Color::from_rgb8(72, 70, 66),
            text_muted: Color::from_rgb8(118, 115, 108),
            accent_mint: Color::from_rgb8(16, 138, 98),
            accent_mint_wash: Color::from_rgb8(214, 245, 233),
            accent_cyan: Color::from_rgb8(8, 145, 178),
            selection_sky: Color::from_rgb8(37, 118, 172),
            danger_coral: Color::from_rgb8(220, 38, 38),
            warning_amber: Color::from_rgb8(202, 118, 12),
            clip_visual_bg: Color::from_rgb8(212, 236, 228),
            clip_visual_active: Color::from_rgb8(178, 226, 208),
            clip_audio_bg: Color::from_rgb8(224, 212, 248),
            clip_shader_bg: Color::from_rgb8(206, 234, 246),
        }
    }

    pub const fn for_mode(dark: bool) -> Self {
        if dark {
            Self::graphite_dark()
        } else {
            Self::paper_light()
        }
    }
}

/// Build a dock tab pill with an active top-rounded highlight (`dock.css` `.hf-dock-tab-fill`).
#[cfg(feature = "xilem-ui")]
pub fn dock_tab_pill<State, F>(
    title: &str,
    is_active: bool,
    pal: StudioPalette,
    on_click: F,
) -> impl WidgetView<State> + use<State, F>
where
    State: 'static,
    F: Fn(&mut State) + Send + Sync + 'static,
{
    let text_str = if is_active {
        format!("● {title}")
    } else {
        title.to_string()
    };
    let bg = if is_active {
        pal.accent_mint_wash
    } else {
        pal.bg_shell
    };
    let border = if is_active {
        pal.accent_mint
    } else {
        pal.border_hairline
    };
    sized_box(text_button(text_str, on_click))
        .padding(2.0)
        .background_color(bg)
        .border(border, 1.0)
        .corner_radius(6.0)
}

/// Build a compact semantic badge chip (`TEXT`, `RECT`, `SHDR`, `VID`, `AUD`).
#[cfg(feature = "xilem-ui")]
pub fn kind_badge_chip<State>(
    badge: &str,
    pal: StudioPalette,
) -> impl WidgetView<State> + use<State>
where
    State: 'static,
{
    let (fg, bg) = match badge {
        "TEXT" => (pal.accent_mint, pal.accent_mint_wash),
        "SHDR" | "WGSL" => (pal.accent_cyan, pal.clip_shader_bg),
        "VID" | "IMG" => (pal.selection_sky, pal.bg_raised),
        "AUD" => (Color::from_rgb8(216, 180, 254), pal.clip_audio_bg),
        "RECT" | "CIRC" | "PATH" => (pal.warning_amber, pal.bg_raised),
        _ => (pal.text_secondary, pal.bg_raised),
    };
    sized_box(
        label(badge.to_string())
            .weight(FontWeight::BOLD)
            .text_size(10.0)
            .color(fg),
    )
    .padding(3.0)
    .background_color(bg)
    .border(fg, 1.0)
    .corner_radius(4.0)
}

/// Build a collapsible `PropertyPanelFlat` group header with chevron, title, and live summary.
#[cfg(feature = "xilem-ui")]
pub fn flat_group_header<State, F>(
    title: &str,
    summary: &str,
    is_open: bool,
    pal: StudioPalette,
    on_toggle: F,
) -> impl WidgetView<State> + use<State, F>
where
    State: 'static,
    F: Fn(&mut State) + Send + Sync + 'static,
{
    let chevron = if is_open { "▾" } else { "▸" };
    let btn_label = format!("{chevron} {title}");
    let bg = if is_open {
        pal.bg_raised
    } else {
        pal.bg_surface
    };
    flex_row((
        text_button(btn_label, on_toggle),
        label(summary.to_string())
            .text_size(11.0)
            .color(if is_open {
                pal.accent_mint
            } else {
                pal.text_muted
            }),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(6.0)
    .background_color(bg)
    .border(pal.border_hairline, 1.0)
}

/// Wrap a dock panel inside Hyperframes' signature rounded card with a 3px outer shell gutter.
#[cfg(feature = "xilem-ui")]
pub fn dock_card_shell<State, V>(
    inner: V,
    pal: StudioPalette,
) -> impl WidgetView<State> + use<State, V>
where
    State: 'static,
    V: WidgetView<State>,
{
    sized_box(
        sized_box(inner)
            .expand()
            .background_color(pal.bg_surface)
            .border(pal.border_strong, 1.0)
            .corner_radius(10.0),
    )
    .expand()
    .padding(3.0)
    .background_color(pal.bg_shell)
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
