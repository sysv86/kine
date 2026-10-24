//! Kine Studio design tokens, themes, accent presets, density metrics,
//! settings model, and UI animation interpolators.
//!
//! Grounded in `crates/kine-studio/UI_GUIDELINES.md`:
//! - Surface ladder: Graphite Dark (default), Warm Paper (light), Carbon Obsidian (OLED).
//! - Signal accents: Kine Mint, Cinema Amber, Broadcast Cyan, Studio Violet, Signal Crimson.
//! - Density scale: Compact, Comfortable, Spacious (`4px` spatial grid).
//! - Smooth frame-rate-independent UI animation state (`StudioAnimState`).

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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

/// Floating panel card radius (default Comfortable metric).
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
pub const TRACK_GUTTER: f64 = 216.0;
/// Timeline track row height.
pub const TRACK_H: f64 = 32.0;
/// Timeline ruler height.
pub const RULER_H: f64 = 24.0;

/// Studio surface theme mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum StudioThemeMode {
    GraphiteDark,
    WarmPaper,
    CarbonObsidian,
}

impl StudioThemeMode {
    pub const ALL: [Self; 3] = [Self::GraphiteDark, Self::WarmPaper, Self::CarbonObsidian];

    pub const fn label(self) -> &'static str {
        match self {
            Self::GraphiteDark => "Graphite Dark",
            Self::WarmPaper => "Warm Paper",
            Self::CarbonObsidian => "Carbon Obsidian",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::GraphiteDark => "Graphite",
            Self::WarmPaper => "Paper",
            Self::CarbonObsidian => "Obsidian",
        }
    }

    pub const fn is_dark(self) -> bool {
        !matches!(self, Self::WarmPaper)
    }

    pub const fn next(self) -> Self {
        match self {
            Self::GraphiteDark => Self::CarbonObsidian,
            Self::CarbonObsidian => Self::WarmPaper,
            Self::WarmPaper => Self::GraphiteDark,
        }
    }
}

/// Primary signal accent color preset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum AccentPreset {
    KineMint,
    CinemaAmber,
    BroadcastCyan,
    StudioViolet,
    SignalCrimson,
}

impl AccentPreset {
    pub const ALL: [Self; 5] = [
        Self::KineMint,
        Self::CinemaAmber,
        Self::BroadcastCyan,
        Self::StudioViolet,
        Self::SignalCrimson,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::KineMint => "Kine Mint",
            Self::CinemaAmber => "Cinema Amber",
            Self::BroadcastCyan => "Broadcast Cyan",
            Self::StudioViolet => "Studio Violet",
            Self::SignalCrimson => "Signal Crimson",
        }
    }

    pub const fn hex_preview(self) -> &'static str {
        match self {
            Self::KineMint => "#3de4a8",
            Self::CinemaAmber => "#f5a623",
            Self::BroadcastCyan => "#00e3ff",
            Self::StudioViolet => "#9b72f2",
            Self::SignalCrimson => "#ff6b6b",
        }
    }
}

/// UI layout density tier following the 4px spatial grid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum UiDensity {
    Compact,
    Comfortable,
    Spacious,
}

impl UiDensity {
    pub const ALL: [Self; 3] = [Self::Compact, Self::Comfortable, Self::Spacious];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Compact => "Compact",
            Self::Comfortable => "Comfortable",
            Self::Spacious => "Spacious",
        }
    }

    pub const fn metrics(self) -> DensityMetrics {
        match self {
            Self::Compact => DensityMetrics {
                shell_gap: 4.0,
                panel_radius: 8.0,
                header_h: 36.0,
                tab_h: 26.0,
                row_h: 26.0,
                track_h: 28.0,
                subtrack_h: 22.0,
                left_dock_w: 236.0,
                right_dock_w: 304.0,
                track_gutter_w: 196.0,
                cascade_indent: 12.0,
                panel_pad: 6.0,
            },
            Self::Comfortable => DensityMetrics {
                shell_gap: 6.0,
                panel_radius: 10.0,
                header_h: 40.0,
                tab_h: 28.0,
                row_h: 30.0,
                track_h: 32.0,
                subtrack_h: 24.0,
                left_dock_w: 256.0,
                right_dock_w: 328.0,
                track_gutter_w: 216.0,
                cascade_indent: 14.0,
                panel_pad: 8.0,
            },
            Self::Spacious => DensityMetrics {
                shell_gap: 8.0,
                panel_radius: 12.0,
                header_h: 44.0,
                tab_h: 32.0,
                row_h: 34.0,
                track_h: 36.0,
                subtrack_h: 28.0,
                left_dock_w: 280.0,
                right_dock_w: 356.0,
                track_gutter_w: 236.0,
                cascade_indent: 18.0,
                panel_pad: 10.0,
            },
        }
    }
}

/// Computed geometric layout tokens for a [`UiDensity`] level.
#[derive(Debug, Clone, Copy)]
pub struct DensityMetrics {
    pub shell_gap: f64,
    pub panel_radius: f64,
    pub header_h: f64,
    pub tab_h: f64,
    pub row_h: f64,
    pub track_h: f64,
    pub subtrack_h: f64,
    pub left_dock_w: f64,
    pub right_dock_w: f64,
    pub track_gutter_w: f64,
    pub cascade_indent: f64,
    pub panel_pad: f64,
}

/// Active category tab inside the Studio Settings window.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SettingsCategory {
    Appearance,
    EditorViewport,
    TimelinePlayback,
    EngineExport,
    Shortcuts,
}

impl SettingsCategory {
    pub const ALL: [Self; 5] = [
        Self::Appearance,
        Self::EditorViewport,
        Self::TimelinePlayback,
        Self::EngineExport,
        Self::Shortcuts,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Appearance => "Appearance & UI",
            Self::EditorViewport => "Editor & Viewport",
            Self::TimelinePlayback => "Timeline & Audio",
            Self::EngineExport => "Engine & Export",
            Self::Shortcuts => "Keybindings",
        }
    }

    pub const fn icon(self) -> &'static str {
        match self {
            Self::Appearance => "palette",
            Self::EditorViewport => "crop_free",
            Self::TimelinePlayback => "movie_edit",
            Self::EngineExport => "memory",
            Self::Shortcuts => "keyboard",
        }
    }
}

/// Viewport color channel inspection filter.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ViewportChannelMode {
    Rgb,
    Red,
    Green,
    Blue,
    Alpha,
    Luma,
}

impl ViewportChannelMode {
    pub const ALL: [Self; 6] = [
        Self::Rgb,
        Self::Red,
        Self::Green,
        Self::Blue,
        Self::Alpha,
        Self::Luma,
    ];

    pub const fn badge(self) -> &'static str {
        match self {
            Self::Rgb => "RGB",
            Self::Red => "R",
            Self::Green => "G",
            Self::Blue => "B",
            Self::Alpha => "ALPHA",
            Self::Luma => "LUMA",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Rgb => Self::Red,
            Self::Red => Self::Green,
            Self::Green => Self::Blue,
            Self::Blue => Self::Alpha,
            Self::Alpha => Self::Luma,
            Self::Luma => Self::Rgb,
        }
    }
}

/// Persistent UI & Editor settings for Kine Studio.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StudioSettings {
    // ── Appearance & UI ────────────────────────────────────────────────────
    pub theme_mode: StudioThemeMode,
    pub accent_preset: AccentPreset,
    pub density: UiDensity,
    pub ui_scale: f32,
    pub animation_speed: f32,
    pub reduced_motion: bool,
    pub show_cascade_guides: bool,
    pub show_status_bar: bool,
    pub left_dock_visible: bool,

    // ── Editor & Viewport ──────────────────────────────────────────────────
    pub default_canvas_width: u32,
    pub default_canvas_height: u32,
    pub show_checkerboard_alpha: bool,
    pub show_center_crosshair: bool,
    pub nudge_step_px: f32,
    pub nudge_large_px: f32,
    pub code_auto_apply: bool,
    pub code_font_size: f32,

    // ── Timeline & Playback ────────────────────────────────────────────────
    pub default_fps: f64,
    pub default_duration: f64,
    pub timeline_show_subtracks: bool,
    pub timeline_show_waveforms: bool,
    pub scrub_audio_preview: bool,
    pub snap_tolerance_sec: f64,

    // ── Engine & Export ────────────────────────────────────────────────────
    pub prefer_gpu_backend: bool,
    pub export_quantizer: u8,
    pub export_speed: u8,
    pub auto_save_scene: bool,
}

impl Default for StudioSettings {
    fn default() -> Self {
        Self {
            theme_mode: StudioThemeMode::GraphiteDark,
            accent_preset: AccentPreset::KineMint,
            density: UiDensity::Comfortable,
            ui_scale: 1.0,
            animation_speed: 1.0,
            reduced_motion: false,
            show_cascade_guides: true,
            show_status_bar: true,
            left_dock_visible: true,
            default_canvas_width: 640,
            default_canvas_height: 360,
            show_checkerboard_alpha: false,
            show_center_crosshair: true,
            nudge_step_px: 10.0,
            nudge_large_px: 32.0,
            code_auto_apply: false,
            code_font_size: 11.0,
            default_fps: 30.0,
            default_duration: 3.0,
            timeline_show_subtracks: true,
            timeline_show_waveforms: true,
            scrub_audio_preview: true,
            snap_tolerance_sec: 0.04,
            prefer_gpu_backend: true,
            export_quantizer: 90,
            export_speed: 8,
            auto_save_scene: false,
        }
    }
}

/// Real-time UI animation & transition state (interpolated at 60Hz when active).
#[derive(Debug, Clone)]
pub struct StudioAnimState {
    /// Smoothly interpolated playhead position for visual rendering in `TimelineCanvas`.
    pub smooth_playhead: f64,
    /// Settings modal entrance progress `[0.0, 1.0]`.
    pub settings_modal_t: f32,
    /// Ask Agent modal entrance progress `[0.0, 1.0]`.
    pub agent_modal_t: f32,
    /// Left dock expansion progress `[0.0, 1.0]`.
    pub left_dock_t: f32,
    /// Right inspector dock expansion progress `[0.0, 1.0]`.
    pub right_dock_t: f32,
    /// Per-section disclosure expansion progress `[0.0, 1.0]`.
    pub section_open_t: BTreeMap<String, f32>,
    /// Toast notification banner opacity/slide timer `[0.0, 1.0]`.
    pub toast_timer: f32,
    /// Monotonic UI animation phase for subtle active indicators (e.g. playhead pulse).
    pub pulse_phase: f32,
}

impl Default for StudioAnimState {
    fn default() -> Self {
        let mut sections = BTreeMap::new();
        sections.insert("layout".to_string(), 1.0);
        sections.insert("transform".to_string(), 1.0);
        sections.insert("style".to_string(), 1.0);
        sections.insert("stroke".to_string(), 0.0);
        sections.insert("text".to_string(), 1.0);
        sections.insert("motion".to_string(), 1.0);
        sections.insert("fx".to_string(), 1.0);
        sections.insert("audio".to_string(), 0.0);
        Self {
            smooth_playhead: 0.5,
            settings_modal_t: 0.0,
            agent_modal_t: 0.0,
            left_dock_t: 1.0,
            right_dock_t: 1.0,
            section_open_t: sections,
            toast_timer: 0.0,
            pulse_phase: 0.0,
        }
    }
}

impl StudioAnimState {
    /// Evaluate `ease_out_quint`: `1 - (1 - t)^5` for crisp desktop transitions.
    pub fn ease_out_quint(t: f32) -> f32 {
        let clamped = t.clamp(0.0, 1.0);
        1.0 - (1.0 - clamped).powi(5)
    }

    /// Advance all active UI transitions by `dt` seconds. Returns `true` if any transition is still in flight.
    pub fn step(
        &mut self,
        dt: f32,
        target_playhead: f64,
        settings_open: bool,
        agent_modal_open: bool,
        left_visible: bool,
        right_visible: bool,
        open_sections: &std::collections::BTreeSet<String>,
        speed_mult: f32,
        reduced_motion: bool,
    ) -> bool {
        if reduced_motion || speed_mult <= 0.05 {
            self.smooth_playhead = target_playhead;
            self.settings_modal_t = if settings_open { 1.0 } else { 0.0 };
            self.agent_modal_t = if agent_modal_open { 1.0 } else { 0.0 };
            self.left_dock_t = if left_visible { 1.0 } else { 0.0 };
            self.right_dock_t = if right_visible { 1.0 } else { 0.0 };
            for (k, val) in &mut self.section_open_t {
                *val = if open_sections.contains(k) { 1.0 } else { 0.0 };
            }
            self.toast_timer = (self.toast_timer - dt).max(0.0);
            return self.toast_timer > 0.0;
        }

        let rate = (14.0 * speed_mult * dt).clamp(0.05, 0.95);
        let mut active = false;

        let playhead_diff = target_playhead - self.smooth_playhead;
        if playhead_diff.abs() > 1e-4 {
            self.smooth_playhead += playhead_diff * f64::from((rate * 1.4).min(1.0));
            active = true;
        } else {
            self.smooth_playhead = target_playhead;
        }

        let step_towards = |cur: &mut f32, target: f32| -> bool {
            let diff = target - *cur;
            if diff.abs() > 0.005 {
                *cur += diff * rate;
                true
            } else {
                *cur = target;
                false
            }
        };

        active |= step_towards(&mut self.settings_modal_t, if settings_open { 1.0 } else { 0.0 });
        active |= step_towards(&mut self.agent_modal_t, if agent_modal_open { 1.0 } else { 0.0 });
        active |= step_towards(&mut self.left_dock_t, if left_visible { 1.0 } else { 0.0 });
        active |= step_towards(&mut self.right_dock_t, if right_visible { 1.0 } else { 0.0 });

        for (k, val) in &mut self.section_open_t {
            let tgt = if open_sections.contains(k) { 1.0 } else { 0.0 };
            active |= step_towards(val, tgt);
        }

        if self.toast_timer > 0.0 {
            self.toast_timer = (self.toast_timer - dt * 0.45).max(0.0);
            active = true;
        }

        self.pulse_phase = (self.pulse_phase + dt * 3.5) % std::f32::consts::TAU;
        active
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
