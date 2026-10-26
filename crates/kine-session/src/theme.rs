//! Headless Studio design tokens, theme modes, accent presets, density metrics,
//! settings model, and UI animation interpolators.
//!
//! Separated from Xilem widget rendering so `kine-session` and `kine-cli` compile
//! without pulling in `xilem` or `winit`.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

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
            Self::GraphiteDark => "Gruvbox Dark",
            Self::WarmPaper => "Gruvbox Light",
            Self::CarbonObsidian => "Gruvbox Soft",
        }
    }

    pub const fn short_label(self) -> &'static str {
        match self {
            Self::GraphiteDark => "Dark",
            Self::WarmPaper => "Light",
            Self::CarbonObsidian => "Soft",
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
            Self::KineMint => "Gruvbox Green",
            Self::CinemaAmber => "Gruvbox Yellow",
            Self::BroadcastCyan => "Gruvbox Aqua",
            Self::StudioViolet => "Gruvbox Purple",
            Self::SignalCrimson => "Gruvbox Red",
        }
    }

    pub const fn hex_preview(self) -> &'static str {
        match self {
            Self::KineMint => "#a9b665",
            Self::CinemaAmber => "#d8a657",
            Self::BroadcastCyan => "#89b482",
            Self::StudioViolet => "#d3869b",
            Self::SignalCrimson => "#ea6962",
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
        left_visible: bool,
        right_visible: bool,
        open_sections: &std::collections::BTreeSet<String>,
        speed_mult: f32,
        reduced_motion: bool,
    ) -> bool {
        if reduced_motion || speed_mult <= 0.05 {
            self.smooth_playhead = target_playhead;
            self.settings_modal_t = if settings_open { 1.0 } else { 0.0 };
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
