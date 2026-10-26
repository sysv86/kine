//! Kine Studio shell — a native Video-as-Code NLE built on Xilem `0.4.0` & Masonry `0.4.0`.
//!
//! Studio UI contract:
//! - Calibrated multi-theme surface ladder (`Gruvbox Dark`, `Gruvbox Light`, `Gruvbox Soft`)
//!   with 5 signal accent presets and 3 spatial density tiers (`Compact`, `Comfortable`, `Spacious`).
//! - Zero dead stubs: every control mutates `StudioState` / `SceneDef` and synchronizes
//!   in-process with `kine_server::Engine`.
//! - True Vello-rendered `TimelineRulerWidget`, `TrackLaneWidget`, and `EasingCurveWidget`
//!   replacing proportional spacer hacks.
//! - Multi-open cascading Inspector accordion with animated disclosure chevrons and
//!   3-state property rows (`Default`, `Modified`, `Keyframed`).
//! - Full 5-tab Settings Window (`settings_panel.rs`) overlaid via `zstack`.

use kine_session::catalog::CATALOG_ITEMS;
use crate::custom_widgets::{
    easing_curve_canvas, timeline_ruler_canvas, track_lane_canvas, EasingCurveSpec, RulerPaintSpec,
    TrackLaneSpec,
};
use crate::local_portal::portal;
use crate::preview_canvas::preview;
use crate::settings_panel::settings_window_overlay;
use kine_session::state::{LeftDockTab, RenderJobStatus, RightDockTab, StudioState};
use crate::theme::{
    sample_audio_meter_levels, DensityMetrics, StudioPalette, ICON_FONT, MONO_FONT, RULER_H,
    UI_FONT,
};
use kine_session::timeline::{generate_ruler_ticks, TimelineTrackKind};
use kine_core::NodeId;
use kine_render::{FrameFormat, RenderBackend};
use std::path::Path;
use std::time::Duration;

use xilem::core::fork;
use xilem::view::task_raw;
use xilem::{
    masonry::{
        core::ArcStr, parley::style::FontWeight, peniko::ImageBrush,
        properties::types::AsUnit as _, properties::Padding,
    },
    style::Style as _,
    view::{
        button, flex_col, flex_row, label, sized_box, slider, text_input, zstack, AnyFlexChild,
        Axis, CrossAxisAlignment, FlexExt as _, FlexSpacer,
    },
    winit::error::EventLoopError,
    Color, EventLoop, WidgetView, WindowOptions, Xilem,
};

type Child = AnyFlexChild<StudioState>;
type Boxed = Box<xilem::AnyWidgetView<StudioState>>;

fn transparent() -> Color {
    Color::from_rgba8(0, 0, 0, 0)
}

/// Launch the native Xilem desktop window for Kine Studio.
pub fn run_studio(mut state: StudioState) -> Result<(), EventLoopError> {
    if state.bridge.is_none() {
        let _ = state.start_bridge_server(None);
    }
    let app = Xilem::new_simple(
        state,
        app_logic,
        WindowOptions::new("Kine Studio — Video-as-Code NLE"),
    )
    .with_font(include_bytes!("../assets/InterVariable.ttf").to_vec())
    .with_font(include_bytes!("../assets/MaterialSymbolsOutlined.ttf").to_vec())
    .with_font(include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf").to_vec());
    app.run_in(EventLoop::with_user_event())
}

pub fn app_logic(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    // Keep legacy `dark_theme` boolean in sync if toggled externally by tests
    if state.dark_theme != state.settings.theme_mode.is_dark() {
        state.settings.theme_mode = if state.dark_theme {
            crate::theme::StudioThemeMode::GraphiteDark
        } else {
            crate::theme::StudioThemeMode::WarmPaper
        };
    }

    let t = StudioPalette::from_settings(&state.settings);
    let dm = state.settings.density.metrics();

    let header = header_bar(state, &t, &dm);
    let left = left_dock(state, &t, &dm);
    let centre = center_workspace(state, &t, &dm);
    let right = right_dock(state, &t, &dm);

    let mut body: Vec<Child> = Vec::new();
    if state.settings.left_dock_visible {
        body.push(
            sized_box(left)
                .width(dm.left_dock_w.px())
                .expand_height()
                .into_any_flex(),
        );
    }
    body.push(
        sized_box(centre)
            .expand_width()
            .expand_height()
            .flex(1.0)
            .into_any_flex(),
    );
    if state.inspector_visible {
        body.push(
            sized_box(right)
                .width(dm.right_dock_w.px())
                .expand_height()
                .into_any_flex(),
        );
    }

    let mut shell_rows: Vec<Child> = vec![
        header.into_any_flex(),
        sized_box(
            flex_row(body)
                .gap(dm.shell_gap.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .must_fill_major_axis(true),
        )
        .expand_width()
        .padding(Padding::all(dm.shell_gap))
        .background_color(t.bg_shell)
        .flex(1.0)
        .into_any_flex(),
    ];

    if state.settings.show_status_bar {
        shell_rows.push(status_bar(state, &t).into_any_flex());
    }

    let base_shell = flex_col(shell_rows)
        .gap(0.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .background_color(t.bg_shell);

    // Layer modal overlays cleanly above the workspace using `zstack`
    let root_view: Boxed = if state.settings_open {
        let overlay = settings_window_overlay(state, &t);
        zstack((base_shell, overlay)).boxed()
    } else {
        base_shell.boxed()
    };

    let run_ticker = state.needs_animation_tick();
    let bridge_pending = state.bridge.as_ref().map(|b| b.pending_flag());
    let has_watched_file = state.watched_scene_path().is_some();
    let run_loop = run_ticker || bridge_pending.is_some() || has_watched_file;

    // 60 Hz frame-rate-independent ticker during playback/animations, plus low-latency
    // wakeups for external CLI bridge commands and live scene file hot-reloads.
    fork(
        root_view,
        run_loop.then(|| {
            task_raw(
                move |proxy| {
                    let pending_flag = bridge_pending.clone();
                    async move {
                        let interval_ms = if run_ticker { 16 } else { 32 };
                        let mut tick =
                            xilem::tokio::time::interval(Duration::from_millis(interval_ms));
                        let mut idle_counter = 0u32;
                        loop {
                            tick.tick().await;
                            idle_counter = idle_counter.wrapping_add(1);
                            let has_cmd = pending_flag
                                .as_ref()
                                .is_some_and(|f| f.load(std::sync::atomic::Ordering::Acquire));
                            let should_wake = run_ticker
                                || has_cmd
                                || (has_watched_file && idle_counter % 6 == 0);
                            if should_wake && proxy.message(()).is_err() {
                                break;
                            }
                        }
                    }
                },
                |st: &mut StudioState, ()| {
                    st.tick_studio(1.0 / 60.0);
                },
            )
        }),
    )
}

// ── Primitives ───────────────────────────────────────────────────────────────

fn txt(value: impl Into<ArcStr>, size: f32, color: Color) -> impl WidgetView<StudioState> {
    label(value).font(UI_FONT).text_size(size).color(color)
}

fn txtw(
    value: impl Into<ArcStr>,
    size: f32,
    weight: FontWeight,
    color: Color,
) -> impl WidgetView<StudioState> {
    label(value)
        .font(UI_FONT)
        .text_size(size)
        .weight(weight)
        .color(color)
}

fn mono(value: impl Into<ArcStr>, size: f32, color: Color) -> impl WidgetView<StudioState> {
    label(value).font(MONO_FONT).text_size(size).color(color)
}

fn clipped(value: &str, max: usize) -> String {
    let mut chars = value.chars();
    let mut out: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        out.push('…');
    }
    out
}

fn icon(name: &'static str, size: f32, color: Color) -> impl WidgetView<StudioState> {
    label(name).font(ICON_FONT).text_size(size).color(color)
}

fn divider(t: &StudioPalette) -> impl WidgetView<StudioState> {
    sized_box(label(""))
        .width(1.0_f64.px())
        .height(16.0_f64.px())
        .background_color(t.border)
}

fn icon_btn(
    name: &'static str,
    color: Color,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(icon(name, 15.0, color), cb)
        .padding(Padding::from_vh(3.0, 5.0))
        .corner_radius(5.0)
        .background_color(transparent())
        .active_background_color(t.press)
        .border(transparent(), 1.0)
        .hovered_border_color(t.border_strong)
}

fn icon_btn_on(
    name: &'static str,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(icon(name, 15.0, t.accent), cb)
        .padding(Padding::from_vh(3.0, 5.0))
        .corner_radius(5.0)
        .background_color(t.accent_wash)
        .active_background_color(t.press)
        .border(t.border_accent, 1.0)
        .hovered_border_color(t.accent)
}

fn icon_toggle(
    name: &'static str,
    on: bool,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(icon(name, 15.0, if on { t.accent } else { t.muted }), cb)
        .padding(Padding::from_vh(3.0, 5.0))
        .corner_radius(5.0)
        .background_color(if on { t.accent_wash } else { transparent() })
        .active_background_color(t.press)
        .border(if on { t.border_accent } else { transparent() }, 1.0)
        .hovered_border_color(t.border_strong)
}

fn text_btn(
    value: impl Into<ArcStr>,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(txt(value, 11.0, t.text_soft), cb)
        .padding(Padding::from_vh(3.0, 7.0))
        .corner_radius(5.0)
        .background_color(t.bg_inset)
        .active_background_color(t.press)
        .border(t.border, 1.0)
        .hovered_border_color(t.border_strong)
}

fn icon_text_btn(
    name: &'static str,
    value: impl Into<ArcStr>,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((icon(name, 13.5, t.text_soft), txt(value, 10.5, t.text_soft)))
            .gap(4.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(3.0, 7.0))
    .corner_radius(5.0)
    .background_color(t.bg_surface)
    .active_background_color(t.press)
    .border(t.border, 1.0)
    .hovered_border_color(t.border_strong)
}

fn primary_btn(
    name: &'static str,
    value: &'static str,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((
            icon(name, 14.0, t.on_accent),
            txtw(value, 11.0, FontWeight::SEMI_BOLD, t.on_accent),
        ))
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(4.0, 10.0))
    .corner_radius(6.0)
    .background_color(t.accent)
    .active_background_color(t.accent_hover)
    .border(t.accent, 1.0)
    .hovered_border_color(t.accent_hover)
}

fn tab(
    icon_name: &'static str,
    value: &'static str,
    active: bool,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((
            icon(icon_name, 12.5, if active { t.accent } else { t.muted }),
            txtw(
                value,
                10.0,
                FontWeight::new(if active { 560.0 } else { 450.0 }),
                if active { t.text } else { t.muted },
            ),
        ))
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(3.0, 7.0))
    .corner_radius(5.0)
    .background_color(if active { t.bg_raised } else { transparent() })
    .active_background_color(t.press)
    .border(
        if active {
            t.border_strong
        } else {
            transparent()
        },
        1.0,
    )
    .hovered_border_color(t.border_strong)
}

fn toggle(
    value: &'static str,
    on: bool,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((
            icon(
                if on {
                    "check_box"
                } else {
                    "check_box_outline_blank"
                },
                13.0,
                if on { t.accent } else { t.muted },
            ),
            txt(value, 10.5, if on { t.text } else { t.muted }),
        ))
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(2.5, 6.0))
    .corner_radius(4.0)
    .background_color(if on { t.accent_wash } else { transparent() })
    .active_background_color(t.press)
    .border(if on { t.border_accent } else { transparent() }, 1.0)
    .hovered_border_color(t.border_strong)
}

fn badge(value: impl Into<ArcStr>, fg: Color, bg: Color) -> impl WidgetView<StudioState> {
    sized_box(txtw(value, 8.5, FontWeight::new(650.0), fg))
        .padding(Padding::from_vh(1.5, 4.5))
        .background_color(bg)
        .border(fg, 1.0)
        .corner_radius(3.5)
}

fn kind_badge_pill(kind: &'static str, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let (fg, bg) = match kind {
        "SHDR" | "WGSL" => (t.media, t.clip_shader),
        "VID" | "IMG" => (t.media, t.bg_inset),
        "AUD" => (t.audio, t.audio_wash),
        "TEXT" => (t.accent_ink, t.accent_wash),
        "CONT" | "RECT" | "CIRC" | "PATH" => (t.container, t.bg_inset),
        _ => (t.text_soft, t.bg_inset),
    };
    badge(kind, fg, bg)
}

fn panel(
    t: &StudioPalette,
    radius: f64,
    body: impl WidgetView<StudioState>,
) -> impl WidgetView<StudioState> {
    let t = *t;
    sized_box(body)
        .background_color(t.bg_surface)
        .border(t.border_strong, 1.0)
        .corner_radius(radius)
}

fn tab_strip(
    children: Vec<Child>,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    sized_box(
        flex_row(children)
            .gap(2.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height((dm.tab_h + 6.0).px())
    .padding(Padding::from_vh(3.0, 5.0))
    .background_color(t.bg_inset)
    .border(t.border, 1.0)
}

fn field(
    value: impl Into<String>,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState, String) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    text_input(value.into(), cb)
        .padding(Padding::from_vh(3.5, 7.0))
        .background_color(t.bg_inset)
        .border(t.border_strong, 1.0)
        .corner_radius(5.0)
}

/// Interactive numeric stepper cell (`[-]` Label `Value` `[+]`) replacing dead read-only cells.
fn interactive_num_cell(
    name: &'static str,
    value: impl Into<ArcStr>,
    is_animated: bool,
    t: &StudioPalette,
    on_dec: impl Fn(&mut StudioState) + Send + Sync + 'static,
    on_inc: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    let border_col = if is_animated {
        t.border_accent
    } else {
        t.border
    };
    let label_col = if is_animated { t.accent_ink } else { t.muted };
    sized_box(
        flex_row((
            button(mono("−", 9.5, t.muted), on_dec)
                .padding(Padding::from_vh(1.0, 3.5))
                .corner_radius(3.0)
                .background_color(transparent())
                .active_background_color(t.press)
                .border(transparent(), 1.0)
                .hovered_border_color(t.border_strong),
            flex_col((
                txtw(name, 8.0, FontWeight::SEMI_BOLD, label_col),
                mono(value, 9.5, t.text),
            ))
            .gap(0.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .flex(1.0),
            button(mono("+", 9.5, t.muted), on_inc)
                .padding(Padding::from_vh(1.0, 3.5))
                .corner_radius(3.0)
                .background_color(transparent())
                .active_background_color(t.press)
                .border(transparent(), 1.0)
                .hovered_border_color(t.border_strong),
        ))
        .gap(1.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .padding(Padding::from_vh(2.0, 2.0))
    .background_color(if is_animated {
        t.accent_wash
    } else {
        t.bg_surface
    })
    .border(border_col, 1.0)
    .corner_radius(5.0)
}

fn row_label(value: &'static str, t: &StudioPalette) -> impl WidgetView<StudioState> {
    txt(value, 10.0, t.muted)
}

// ── Header Bar ───────────────────────────────────────────────────────────────

fn header_bar(
    state: &mut StudioState,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    let comp = state
        .compositions
        .get(state.active_composition)
        .map(|c| clipped(&c.title, 26))
        .unwrap_or_else(|| "Untitled Composition".to_string());
    let comp_count = state.compositions.len().max(1);
    let cur_comp_idx = state.active_composition;

    let (status_text, status_color) = if state.inspect_findings.is_empty() {
        ("0 ISSUES".to_string(), t.accent)
    } else {
        (
            format!("{} FINDINGS", state.inspect_findings.len()),
            t.warning,
        )
    };
    let theme_icon = match state.settings.theme_mode {
        crate::theme::StudioThemeMode::GraphiteDark => "dark_mode",
        crate::theme::StudioThemeMode::WarmPaper => "light_mode",
        crate::theme::StudioThemeMode::CarbonObsidian => "contrast",
    };
    let backend_name = state.engine.backend().name().to_uppercase();

    sized_box(
        flex_row(vec![
            // Brand cluster
            flex_row((
                icon("auto_awesome", 15.0, t.accent),
                txtw("KINE", 12.0, FontWeight::new(700.0), t.text),
                badge("STUDIO 2.0", t.accent_ink, t.accent_wash),
            ))
            .gap(5.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .into_any_flex(),
            divider(t).into_any_flex(),
            // Left dock visibility toggle
            icon_toggle(
                "dock_to_right",
                state.settings.left_dock_visible,
                t,
                |st: &mut StudioState| {
                    st.settings.left_dock_visible = !st.settings.left_dock_visible;
                },
            )
            .into_any_flex(),
            // Interactive composition switcher pill (`< Comp Title >` + `+ New`)
            icon_btn("chevron_left", t.muted, t, move |st: &mut StudioState| {
                let prev = if cur_comp_idx == 0 {
                    comp_count - 1
                } else {
                    cur_comp_idx - 1
                };
                let _ = st.select_composition(prev);
            })
            .into_any_flex(),
            button(
                flex_row((
                    icon("movie_filter", 13.0, t.accent),
                    txtw(comp, 11.0, FontWeight::MEDIUM, t.text),
                    mono(
                        format!("{}×{}", state.scene_def.width, state.scene_def.height),
                        9.0,
                        t.muted,
                    ),
                ))
                .gap(5.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                |st: &mut StudioState| {
                    st.settings.left_dock_visible = true;
                    st.left_tab = LeftDockTab::Compositions;
                },
            )
            .padding(Padding::from_vh(3.0, 8.0))
            .corner_radius(5.0)
            .background_color(t.bg_surface)
            .active_background_color(t.press)
            .border(t.border_strong, 1.0)
            .hovered_border_color(t.accent)
            .into_any_flex(),
            icon_btn("chevron_right", t.muted, t, move |st: &mut StudioState| {
                let next = (cur_comp_idx + 1) % comp_count;
                let _ = st.select_composition(next);
            })
            .into_any_flex(),
            icon_btn("add_box", t.accent, t, |st: &mut StudioState| {
                let _ = st.create_new_composition();
            })
            .into_any_flex(),
            badge(status_text, status_color, t.bg_inset).into_any_flex(),
            FlexSpacer::Flex(1.0).into_any_flex(),
            // Right utility cluster
            icon_text_btn("photo_camera", "Snap PNG", t, |st: &mut StudioState| {
                let _ = st.capture_current_frame(FrameFormat::Png, Path::new("studio-capture.png"));
            })
            .into_any_flex(),
            icon_text_btn("memory", backend_name, t, |st: &mut StudioState| {
                st.cycle_render_backend();
            })
            .into_any_flex(),
            divider(t).into_any_flex(),
            icon_toggle(
                "view_sidebar",
                state.inspector_visible,
                t,
                |st: &mut StudioState| {
                    st.inspector_visible = !st.inspector_visible;
                },
            )
            .into_any_flex(),
            icon_btn(theme_icon, t.text_soft, t, |st: &mut StudioState| {
                st.cycle_theme_mode();
            })
            .into_any_flex(),
            // Real-time CLI Bridge Toggle (`CLI :7701`)
            if let Some(addr) = state.bridge_listen_addr() {
                let port_lbl = addr
                    .rsplit(':')
                    .next()
                    .map(|p| format!("CLI :{p}"))
                    .unwrap_or_else(|| "CLI LIVE".to_string());
                button(
                    flex_row((
                        icon("terminal", 13.0, t.accent),
                        mono(port_lbl, 9.0, t.accent_ink),
                    ))
                    .gap(4.0_f64.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    |st: &mut StudioState| {
                        st.toggle_bridge_server();
                    },
                )
                .padding(Padding::from_vh(3.0, 7.0))
                .corner_radius(5.0)
                .background_color(t.accent_wash)
                .active_background_color(t.press)
                .border(t.border_accent, 1.0)
                .hovered_border_color(t.accent)
                .into_any_flex()
            } else {
                icon_text_btn("terminal", "CLI Off", t, |st: &mut StudioState| {
                    st.toggle_bridge_server();
                })
                .into_any_flex()
            },
            // Settings Window Toggle (`⚙ Settings`)
            if state.settings_open {
                icon_btn_on("tune", t, |st: &mut StudioState| {
                    st.settings_open = false;
                })
                .into_any_flex()
            } else {
                icon_text_btn("tune", "Settings", t, |st: &mut StudioState| {
                    st.settings_open = true;
                })
                .into_any_flex()
            },
            primary_btn("ios_share", "Export", t, |st: &mut StudioState| {
                st.right_tab = RightDockTab::Renders;
                st.inspector_visible = true;
                let _ = st.run_export_job();
            })
            .into_any_flex(),
        ])
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(dm.header_h.px())
    .padding(Padding::from_vh(0.0, 10.0))
    .background_color(t.bg_shell)
    .border(t.border, 1.0)
}

fn status_bar(state: &StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let t = *t;
    let ready = state.inspect_findings.is_empty();
    let dot = if ready { t.accent } else { t.warning };
    let status_label = if ready {
        "ENGINE SYNCED".to_string()
    } else {
        format!("{} FINDINGS", state.inspect_findings.len())
    };
    let frame = state.current_frame_index();
    let total = state.total_frames();
    let gpu_active =
        !matches!(state.engine.backend(), RenderBackend::Cpu) && state.engine.gpu_available();
    let backend = if gpu_active { "GPU" } else { "CPU" };
    let banner = clipped(&state.status_banner, 54);
    let undo_depth = state.undo_stack.len();
    let redo_depth = state.redo_stack.len();

    sized_box(
        flex_row(vec![
            sized_box(label(""))
                .width(6.0_f64.px())
                .height(6.0_f64.px())
                .background_color(dot)
                .corner_radius(3.0)
                .into_any_flex(),
            txtw(status_label, 8.5, FontWeight::SEMI_BOLD, dot).into_any_flex(),
            divider(&t).into_any_flex(),
            txt(banner, 9.5, t.text_soft).into_any_flex(),
            FlexSpacer::Flex(1.0).into_any_flex(),
            mono(
                match &state.bridge {
                    Some(b) => format!("BRIDGE {} ({} cmds)", b.addr(), b.commands_handled),
                    None => "BRIDGE OFF".to_string(),
                },
                8.5,
                if state.bridge.is_some() {
                    t.accent_ink
                } else {
                    t.muted
                },
            )
            .into_any_flex(),
            divider(&t).into_any_flex(),
            mono(format!("Undo:{undo_depth} Redo:{redo_depth}"), 8.5, t.muted).into_any_flex(),
            divider(&t).into_any_flex(),
            mono(
                format!(
                    "{}×{} · {:.0}fps · F{:03}/{:03}",
                    state.scene_def.width, state.scene_def.height, state.fps, frame, total
                ),
                9.0,
                t.muted,
            )
            .into_any_flex(),
            divider(&t).into_any_flex(),
            mono(
                format!("{} · {:.1}ms", backend, state.last_render_ms),
                9.0,
                if gpu_active {
                    t.accent_ink
                } else {
                    t.text_soft
                },
            )
            .into_any_flex(),
        ])
        .gap(6.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(22.0_f64.px())
    .padding(Padding::from_vh(0.0, 10.0))
    .background_color(t.bg_shell)
    .border(t.border, 1.0)
}

// ── Left Navigator Dock ──────────────────────────────────────────────────────

fn left_dock(
    state: &mut StudioState,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    let cur = state.left_tab;
    let tabs = tab_strip(
        vec![
            tab(
                "movie",
                "Comps",
                cur == LeftDockTab::Compositions,
                t,
                |st| st.left_tab = LeftDockTab::Compositions,
            )
            .into_any_flex(),
            tab(
                "Perm_Media",
                "Assets",
                cur == LeftDockTab::Assets,
                t,
                |st| st.left_tab = LeftDockTab::Assets,
            )
            .into_any_flex(),
            tab("code", "Code", cur == LeftDockTab::Code, t, |st| {
                st.left_tab = LeftDockTab::Code
            })
            .into_any_flex(),
            tab("widgets", "Catalog", cur == LeftDockTab::Catalog, t, |st| {
                st.left_tab = LeftDockTab::Catalog
            })
            .into_any_flex(),
        ],
        t,
        dm,
    );

    let content: Boxed = match state.left_tab {
        LeftDockTab::Compositions => composition_list(state, t, dm),
        LeftDockTab::Assets => asset_list(state, t, dm),
        LeftDockTab::Code => code_panel(state, t, dm),
        LeftDockTab::Catalog => catalog_panel(state, t, dm),
    };

    flex_col(vec![
        tabs.into_any_flex(),
        portal(content).flex(1.0).into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .must_fill_major_axis(true)
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .background_color(t.bg_surface)
    .border(t.border_strong, 1.0)
    .corner_radius(dm.panel_radius)
}

fn composition_list(state: &mut StudioState, t: &StudioPalette, dm: &DensityMetrics) -> Boxed {
    let mut rows: Vec<Child> = Vec::new();
    rows.push(
        flex_row((
            txtw("PROJECT COMPOSITIONS", 8.5, FontWeight::SEMI_BOLD, t.muted),
            FlexSpacer::Flex(1.0),
            icon_text_btn("add", "New Comp", t, |st: &mut StudioState| {
                let _ = st.create_new_composition();
            }),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(2.0, 2.0))
        .into_any_flex(),
    );

    for (idx, comp) in state.compositions.iter().enumerate() {
        let active = idx == state.active_composition;
        let title = clipped(&comp.title, 24);
        let sub = clipped(&comp.subtitle, 32);
        let res_tag = format!("{}×{}", comp.scene_def.width, comp.scene_def.height);
        let thumb_color = match idx % 3 {
            0 => t.clip_active,
            1 => t.clip_shader,
            _ => t.audio_wash,
        };
        rows.push(
            button(
                flex_row((
                    sized_box(
                        flex_col((
                            FlexSpacer::Flex(1.0),
                            mono(res_tag, 7.5, t.text),
                            FlexSpacer::Flex(1.0),
                        ))
                        .cross_axis_alignment(CrossAxisAlignment::Center),
                    )
                    .width(48.0_f64.px())
                    .height(30.0_f64.px())
                    .background_color(thumb_color)
                    .border(if active { t.accent } else { t.border }, 1.0)
                    .corner_radius(4.0),
                    flex_col((
                        txtw(title, 10.5, FontWeight::MEDIUM, t.text),
                        txt(sub, 9.0, t.muted),
                    ))
                    .gap(1.5_f64.px())
                    .flex(1.0),
                ))
                .gap(7.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                move |st: &mut StudioState| {
                    let _ = st.select_composition(idx);
                },
            )
            .padding(Padding::from_vh(4.5, 6.0))
            .corner_radius(6.0)
            .background_color(if active { t.bg_raised } else { t.bg_inset })
            .active_background_color(t.press)
            .border(if active { t.border_accent } else { t.border }, 1.0)
            .hovered_border_color(t.border_strong)
            .into_any_flex(),
        );
    }
    flex_col(rows)
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(dm.panel_pad))
        .boxed()
}

fn asset_list(state: &mut StudioState, t: &StudioPalette, dm: &DensityMetrics) -> Boxed {
    let mut rows: Vec<Child> = Vec::new();
    rows.push(
        flex_row((
            txtw(
                "CLICK ASSET TO INSERT / APPLY",
                8.5,
                FontWeight::SEMI_BOLD,
                t.muted,
            ),
            FlexSpacer::Flex(1.0),
            mono(format!("{} items", state.assets.len()), 8.5, t.muted),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(2.0, 2.0))
        .into_any_flex(),
    );

    for (asset_idx, asset) in state.assets.iter().enumerate() {
        let name = clipped(&asset.name, 22);
        let uri = asset.uri.clone();
        let details = clipped(&asset.details, 34);
        let kind = asset.kind.badge();
        rows.push(
            button(
                flex_col((
                    flex_row((
                        kind_badge_pill(kind, t),
                        txtw(name, 10.5, FontWeight::MEDIUM, t.text),
                        FlexSpacer::Flex(1.0),
                        icon("add_circle", 13.0, t.accent),
                    ))
                    .gap(5.0_f64.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    mono(uri, 8.5, t.accent_ink),
                    txt(details, 9.0, t.muted),
                ))
                .gap(2.0_f64.px()),
                move |st: &mut StudioState| {
                    if let Err(e) = st.insert_or_apply_asset(asset_idx) {
                        st.notify_status(e.to_string());
                    }
                },
            )
            .padding(Padding::from_vh(5.0, 7.0))
            .corner_radius(6.0)
            .background_color(t.bg_inset)
            .active_background_color(t.press)
            .border(t.border, 1.0)
            .hovered_border_color(t.accent)
            .into_any_flex(),
        );
    }

    let modules: Vec<(String, String, String)> = state
        .module_registry
        .iter()
        .map(|(name, module)| {
            (
                name.to_string(),
                module.meta.kind.clone(),
                module.meta.description.clone(),
            )
        })
        .collect();
    if !modules.is_empty() {
        rows.push(
            sized_box(txtw("PROJECT MODULES", 8.5, FontWeight::SEMI_BOLD, t.muted))
                .padding(Padding::from_vh(6.0, 2.0))
                .into_any_flex(),
        );
        for (name, kind, description) in modules {
            let module_name = name.clone();
            rows.push(
                button(
                    flex_col((
                        flex_row((
                            icon("widgets", 12.0, t.accent),
                            txtw(name, 10.5, FontWeight::MEDIUM, t.text),
                            FlexSpacer::Flex(1.0),
                            badge(kind, t.text, t.bg_raised),
                        ))
                        .gap(4.0_f64.px())
                        .cross_axis_alignment(CrossAxisAlignment::Center),
                        txt(clipped(&description, 34), 9.0, t.muted),
                    ))
                    .gap(1.5_f64.px()),
                    move |st: &mut StudioState| {
                        if let Err(e) = st.insert_module(&module_name) {
                            st.notify_status(e.to_string());
                        }
                    },
                )
                .padding(Padding::from_vh(4.0, 6.0))
                .corner_radius(5.0)
                .background_color(t.bg_raised)
                .active_background_color(t.press)
                .border(t.border, 1.0)
                .hovered_border_color(t.accent)
                .into_any_flex(),
            );
        }
    }

    flex_col(rows)
        .gap(3.5_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(dm.panel_pad))
        .boxed()
}

fn code_panel(state: &mut StudioState, t: &StudioPalette, dm: &DensityMetrics) -> Boxed {
    let src = state.source_buffer.clone();
    let code_pt = state.settings.code_font_size;
    let line_count = src.lines().count().max(1);

    flex_col(vec![
        flex_row((
            txtw("SCENE AST / TS", 9.0, FontWeight::SEMI_BOLD, t.muted),
            mono(format!("{line_count} lines"), 8.5, t.off),
            FlexSpacer::Flex(1.0),
            icon_text_btn("format_align_left", "Format", t, |st: &mut StudioState| {
                st.format_source_buffer();
            }),
            primary_btn("play_arrow", "Apply", t, |st: &mut StudioState| {
                if let Err(e) = st.apply_source_buffer() {
                    st.notify_status(e.to_string());
                }
            }),
        ))
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into_any_flex(),
        field(src, t, |st: &mut StudioState, next: String| {
            st.source_buffer = next;
        })
        .into_any_flex(),
        portal(
            sized_box(mono(state.source_buffer.clone(), code_pt, t.text_soft))
                .padding(Padding::all(8.0))
                .background_color(t.bg_inset)
                .border(t.border, 1.0)
                .corner_radius(5.0),
        )
        .flex(1.0)
        .into_any_flex(),
    ])
    .gap(6.0_f64.px())
    .must_fill_major_axis(true)
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::all(dm.panel_pad))
    .boxed()
}

fn catalog_panel(state: &mut StudioState, t: &StudioPalette, dm: &DensityMetrics) -> Boxed {
    let active_cat = state.catalog_category_filter.clone();
    let categories = [
        "All",
        "Primitives",
        "Typography",
        "Media",
        "WGSL Shaders",
        "Audio DSP",
    ];

    let mut filter_chips: Vec<Child> = Vec::new();
    for &cat in &categories {
        let on = active_cat == cat;
        let short = match cat {
            "Primitives" => "Prim",
            "Typography" => "Text",
            "WGSL Shaders" => "WGSL",
            "Audio DSP" => "Audio",
            other => other,
        };
        filter_chips.push(
            button(
                txtw(
                    short,
                    8.5,
                    if on {
                        FontWeight::SEMI_BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    if on { t.accent_ink } else { t.muted },
                ),
                move |st: &mut StudioState| {
                    st.catalog_category_filter = cat.to_string();
                },
            )
            .padding(Padding::from_vh(2.0, 5.0))
            .corner_radius(4.0)
            .background_color(if on { t.accent_wash } else { t.bg_inset })
            .active_background_color(t.press)
            .border(if on { t.border_accent } else { t.border }, 1.0)
            .into_any_flex(),
        );
    }

    let mut rows: Vec<Child> = vec![flex_row(filter_chips)
        .gap(2.5_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into_any_flex()];

    for item in CATALOG_ITEMS {
        if active_cat != "All" && item.category != active_cat {
            continue;
        }
        let id = item.id;
        let label_str = item.label;
        let desc = item.description;
        let cat_badge = match item.category {
            "WGSL Shaders" => "SHDR",
            "Audio DSP" => "AUD",
            "Typography" => "TEXT",
            "Media" => "VID",
            _ => "RECT",
        };
        rows.push(
            button(
                flex_col((
                    flex_row((
                        kind_badge_pill(cat_badge, t),
                        txtw(label_str, 10.5, FontWeight::MEDIUM, t.text),
                        FlexSpacer::Flex(1.0),
                        icon("add", 13.0, t.accent),
                    ))
                    .gap(5.0_f64.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    txt(desc, 9.0, t.muted),
                ))
                .gap(2.0_f64.px()),
                move |st: &mut StudioState| {
                    let _ = st.insert_catalog_item(id);
                },
            )
            .padding(Padding::from_vh(4.5, 6.5))
            .corner_radius(6.0)
            .background_color(t.bg_inset)
            .active_background_color(t.press)
            .border(t.border, 1.0)
            .hovered_border_color(t.accent)
            .into_any_flex(),
        );
    }
    flex_col(rows)
        .gap(3.5_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(dm.panel_pad))
        .boxed()
}

// ── Centre Workspace: Preview Stage + NLE Timeline ───────────────────────────

fn center_workspace(
    state: &mut StudioState,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    let preview = panel(t, dm.panel_radius, preview_pane(state, t));
    let timeline = panel(t, dm.panel_radius, timeline_pane(state, t, dm));
    xilem::view::split(preview, timeline)
        .split_axis(Axis::Vertical)
        .split_point(0.55)
}

fn preview_pane(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let comp = state
        .compositions
        .get(state.active_composition)
        .map(|c| clipped(&c.title, 22))
        .unwrap_or_else(|| "Composition".to_string());
    let selected = state
        .inspect_selected_node()
        .map(|s| clipped(&s.label, 22))
        .unwrap_or_else(|| "No selection".to_string());
    let zoom_pct = format!("{:.0}%", state.viewport_zoom * 100.0);
    let chan_badge = state.viewport_channel.badge();

    let breadcrumb = flex_row((
        icon("movie", 13.0, t.media),
        txtw(comp, 10.5, FontWeight::MEDIUM, t.text_soft),
        icon("chevron_right", 12.0, t.muted),
        txt(selected, 10.0, t.accent_ink),
        FlexSpacer::Flex(1.0),
        // Channel Isolate Mode Cycle (`RGB`, `R`, `G`, `B`, `ALPHA`, `LUMA`)
        button(
            flex_row((
                icon("filter_b_and_w", 12.0, t.accent),
                mono(chan_badge, 8.5, t.text),
            ))
            .gap(3.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            |st: &mut StudioState| {
                st.viewport_channel = st.viewport_channel.next();
                let _ = st.render_current_frame();
            },
        )
        .padding(Padding::from_vh(2.0, 6.0))
        .corner_radius(4.0)
        .background_color(t.bg_surface)
        .active_background_color(t.press)
        .border(t.border_strong, 1.0),
        icon_toggle("grid_3x3", state.show_grid_overlay, t, |st| {
            st.show_grid_overlay = !st.show_grid_overlay;
            let _ = st.render_current_frame();
        }),
        icon_toggle("crop_free", state.show_safe_guides, t, |st| {
            st.show_safe_guides = !st.show_safe_guides;
            let _ = st.render_current_frame();
        }),
        icon_toggle("select_all", state.show_bounding_boxes, t, |st| {
            st.show_bounding_boxes = !st.show_bounding_boxes;
            let _ = st.render_current_frame();
        }),
        icon_toggle("hub", state.topology_lens_active, t, |st| {
            st.topology_lens_active = !st.topology_lens_active;
            let _ = st.render_current_frame();
        }),
        divider(t),
        icon_btn("remove", t.text_soft, t, |st| {
            st.viewport_zoom = (st.viewport_zoom - 0.25).clamp(0.25, 4.0);
        }),
        button(mono(zoom_pct, 9.0, t.text_soft), |st: &mut StudioState| {
            st.viewport_zoom = 1.0;
        })
        .padding(Padding::from_vh(2.0, 5.0))
        .corner_radius(4.0)
        .background_color(t.bg_surface)
        .border(t.border, 1.0),
        icon_btn("add", t.text_soft, t, |st| {
            st.viewport_zoom = (st.viewport_zoom + 0.25).clamp(0.25, 4.0);
        }),
    ))
    .gap(3.5_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(Padding::from_vh(0.0, 8.0));

    let stage = sized_box(preview(
        ImageBrush::new(crate::theme::pixmap_to_xilem_image(&state.last_viewport_pixmap)),
        t.bg_stage,
        t.border_strong,
    ))
    .expand()
    .padding(Padding::all(10.0))
    .background_color(t.bg_stage);

    let transport = transport_bar(state, t);

    flex_col(vec![
        sized_box(breadcrumb)
            .height(30.0_f64.px())
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        stage.flex(1.0).into_any_flex(),
        transport.into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .must_fill_major_axis(true)
    .cross_axis_alignment(CrossAxisAlignment::Fill)
}

fn transport_bar(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let timecode = state.formatted_timecode();
    let play_icon = if state.is_playing {
        "pause"
    } else {
        "play_arrow"
    };
    let speed = format!("{:.2}×", state.playback_rate);
    let has_audio = state
        .scene_def
        .audio
        .as_ref()
        .is_some_and(|a| !a.tracks.is_empty());
    let (lvl_l, lvl_r) = sample_audio_meter_levels(
        has_audio,
        state.audio_muted,
        state.audio_volume,
        state.current_time,
    );
    let dur = state.duration.max(0.1);
    let cur = state.current_time.clamp(0.0, dur);

    flex_row(vec![
        // Clickable timecode pill toggles MM:SS.ms vs Frame F / Total
        button(
            flex_row((
                icon("schedule", 12.0, t.accent),
                mono(timecode, 10.0, t.text),
            ))
            .gap(4.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            |st: &mut StudioState| {
                st.show_frames = !st.show_frames;
            },
        )
        .padding(Padding::from_vh(3.0, 7.0))
        .corner_radius(5.0)
        .background_color(t.bg_inset)
        .active_background_color(t.press)
        .border(t.border_strong, 1.0)
        .hovered_border_color(t.accent)
        .into_any_flex(),
        icon_btn("skip_previous", t.text_soft, t, |st| {
            st.is_playing = false;
            st.seek_to(0.0, false);
        })
        .into_any_flex(),
        icon_btn("chevron_left", t.text_soft, t, |st| {
            st.step_frames(-1);
        })
        .into_any_flex(),
        sized_box(icon_btn(
            play_icon,
            t.on_accent,
            t,
            |st: &mut StudioState| {
                st.is_playing = !st.is_playing;
            },
        ))
        .background_color(t.accent)
        .border(t.accent_hover, 1.0)
        .corner_radius(6.0)
        .into_any_flex(),
        icon_btn("chevron_right", t.text_soft, t, |st| {
            st.step_frames(1);
        })
        .into_any_flex(),
        icon_btn("skip_next", t.text_soft, t, |st| {
            st.is_playing = false;
            let end = st.duration;
            st.seek_to(end, false);
        })
        .into_any_flex(),
        sized_box(slider(0.0, dur, cur, |st: &mut StudioState, v: f64| {
            st.seek_to(v, true);
        }))
        .flex(1.0)
        .into_any_flex(),
        text_btn(speed, t, |st| {
            st.playback_rate = match st.playback_rate {
                r if (r - 0.5).abs() < 0.01 => 1.0,
                r if (r - 1.0).abs() < 0.01 => 1.5,
                r if (r - 1.5).abs() < 0.01 => 2.0,
                _ => 0.5,
            };
        })
        .into_any_flex(),
        icon_toggle("loop", state.loop_enabled, t, |st| {
            st.loop_enabled = !st.loop_enabled;
        })
        .into_any_flex(),
        icon_toggle(
            if state.audio_muted {
                "volume_off"
            } else {
                "volume_up"
            },
            !state.audio_muted,
            t,
            |st| {
                st.audio_muted = !st.audio_muted;
            },
        )
        .into_any_flex(),
        if state.show_audio_meters {
            meter(lvl_l, t.accent).into_any_flex()
        } else {
            sized_box(label("")).into_any_flex()
        },
        if state.show_audio_meters {
            meter(lvl_r, t.accent).into_any_flex()
        } else {
            sized_box(label("")).into_any_flex()
        },
    ])
    .gap(4.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(Padding::from_vh(5.0, 8.0))
    .background_color(t.bg_surface)
    .border(t.border, 1.0)
}

fn meter(level: f32, color: Color) -> impl WidgetView<StudioState> {
    sized_box(
        flex_col((
            FlexSpacer::Flex(1.0 - f64::from(level.clamp(0.02, 0.98))),
            sized_box(label(""))
                .height(8.0_f64.px())
                .background_color(color)
                .flex(f64::from(level.clamp(0.02, 0.98))),
        ))
        .gap(0.0_f64.px()),
    )
    .width(4.0_f64.px())
    .height(16.0_f64.px())
    .background_color(Color::from_rgba8(255, 255, 255, 14))
    .corner_radius(2.0)
}

// ── Bottom NLE Timeline & Vello Track Lanes ──────────────────────────────────

fn timeline_pane(
    state: &mut StudioState,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    let toolbar = flex_row(vec![
        icon_btn("undo", t.text_soft, t, |st| {
            let _ = st.undo();
        })
        .into_any_flex(),
        icon_btn("redo", t.text_soft, t, |st| {
            let _ = st.redo();
        })
        .into_any_flex(),
        divider(t).into_any_flex(),
        icon_toggle("grid_on", state.timeline_snap_enabled, t, |st| {
            st.timeline_snap_enabled = !st.timeline_snap_enabled;
        })
        .into_any_flex(),
        icon_toggle("waves", state.ripple_edit, t, |st| {
            st.ripple_edit = !st.ripple_edit;
        })
        .into_any_flex(),
        icon_toggle(
            "account_tree",
            state.settings.timeline_show_subtracks,
            t,
            |st| {
                st.settings.timeline_show_subtracks = !st.settings.timeline_show_subtracks;
            },
        )
        .into_any_flex(),
        icon_toggle("graphic_eq", state.show_audio_meters, t, |st| {
            st.show_audio_meters = !st.show_audio_meters;
        })
        .into_any_flex(),
        divider(t).into_any_flex(),
        icon_btn("chevron_left", t.text_soft, t, |st| {
            st.timeline_jump_prev_keyframe();
        })
        .into_any_flex(),
        icon_text_btn("diamond", "+KF", t, |st| {
            let _ = st.timeline_add_keyframe_at_playhead();
        })
        .into_any_flex(),
        icon_btn("chevron_right", t.text_soft, t, |st| {
            st.timeline_jump_next_keyframe();
        })
        .into_any_flex(),
        divider(t).into_any_flex(),
        icon_btn("west", t.text_soft, t, |st| {
            let _ = st.timeline_shift_selected(-0.10);
        })
        .into_any_flex(),
        icon_btn("east", t.text_soft, t, |st| {
            let _ = st.timeline_shift_selected(0.10);
        })
        .into_any_flex(),
        icon_btn("content_cut", t.text_soft, t, |st| {
            let _ = st.timeline_split_at_playhead();
        })
        .into_any_flex(),
        icon_btn("content_copy", t.text_soft, t, |st| {
            let _ = st.timeline_duplicate_selected();
        })
        .into_any_flex(),
        icon_btn("delete", t.danger, t, |st| {
            let _ = st.timeline_delete_selected();
        })
        .into_any_flex(),
        FlexSpacer::Flex(1.0).into_any_flex(),
        mono(format!("{:.0}px/s", state.timeline_zoom_pps), 8.5, t.muted).into_any_flex(),
        icon_btn("fit_screen", t.text_soft, t, |st| {
            st.timeline_zoom_pps = 140.0;
        })
        .into_any_flex(),
        icon_btn("zoom_out", t.text_soft, t, |st| {
            st.timeline_zoom_pps = (st.timeline_zoom_pps - 20.0).max(40.0);
        })
        .into_any_flex(),
        icon_btn("zoom_in", t.text_soft, t, |st| {
            st.timeline_zoom_pps = (st.timeline_zoom_pps + 20.0).min(360.0);
        })
        .into_any_flex(),
    ])
    .gap(2.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(Padding::from_vh(3.0, 6.0))
    .background_color(t.bg_inset)
    .border(t.border, 1.0);

    let ruler = timeline_ruler(state, t, dm);
    let tracks = timeline_tracks(state, t, dm);

    flex_col(vec![
        toolbar.into_any_flex(),
        ruler.into_any_flex(),
        portal(tracks).flex(1.0).into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .must_fill_major_axis(true)
    .cross_axis_alignment(CrossAxisAlignment::Fill)
}

fn timeline_ruler(
    state: &StudioState,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    let ticks = generate_ruler_ticks(state.duration, state.timeline_zoom_pps);
    let mut tick_buttons: Vec<Child> = Vec::new();
    for tick in ticks.into_iter().filter(|tk| tk.is_major).take(10) {
        let sec = tick.time_sec;
        let lbl = tick.label.unwrap_or_else(|| format!("{sec:.1}s"));
        tick_buttons.push(
            button(mono(lbl, 8.5, t.muted), move |st: &mut StudioState| {
                st.seek_to(sec, false);
            })
            .padding(Padding::from_vh(1.0, 2.0))
            .background_color(transparent())
            .active_background_color(t.press)
            .border(transparent(), 1.0)
            .flex(1.0)
            .into_any_flex(),
        );
    }

    let ruler_spec = RulerPaintSpec {
        duration: state.duration.max(0.25),
        playhead: state.anim.smooth_playhead,
        fps: state.fps,
        bg: t.bg_inset,
        border: t.border_strong,
        tick_major: t.muted,
        tick_minor: t.border_strong,
        playhead_color: t.playhead,
        accent_glow: t.accent_wash,
        keyframe_times: state.sorted_keyframe_times(None),
    };

    let right_ruler = zstack((
        sized_box(timeline_ruler_canvas(ruler_spec)).expand(),
        flex_row(tick_buttons)
            .gap(0.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Start),
    ));

    flex_row(vec![
        sized_box(
            flex_row((
                icon("layers", 12.0, t.accent),
                txtw("TRACKS & NODES", 8.5, FontWeight::SEMI_BOLD, t.muted),
                FlexSpacer::Flex(1.0),
                mono(
                    format!("F{:03}", state.current_frame_index()),
                    8.5,
                    t.accent_ink,
                ),
            ))
            .gap(4.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .width(dm.track_gutter_w.px())
        .height(RULER_H.px())
        .padding(Padding::from_vh(0.0, 8.0))
        .background_color(t.bg_inset)
        .border(t.border, 1.0)
        .into_any_flex(),
        sized_box(right_ruler)
            .height(RULER_H.px())
            .flex(1.0)
            .into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
}

fn timeline_tracks(
    state: &mut StudioState,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    let tracks = state.build_timeline_tracks();
    let duration = state.duration.max(0.25);
    let playhead = state.anim.smooth_playhead;
    let show_guides = state.settings.show_cascade_guides;
    let show_waveforms = state.settings.timeline_show_waveforms;

    let mut rows: Vec<Child> = Vec::new();
    for track in tracks {
        let selected = track.selected;
        let kf_times: Vec<f64> = track.keyframes.iter().map(|k| k.time).collect();

        match track.kind.clone() {
            TimelineTrackKind::SceneNode {
                node_id,
                depth,
                kind_badge,
            } => {
                let nid = node_id;
                let is_shader = kind_badge == "SHDR";
                let clip_fill = if selected {
                    t.clip_active
                } else if is_shader {
                    t.clip_shader
                } else {
                    t.clip
                };
                let clip_band = if selected {
                    t.accent
                } else if is_shader {
                    t.media
                } else {
                    t.container
                };
                let clip_border = if selected {
                    t.selection
                } else {
                    t.border_strong
                };

                let lane_spec = TrackLaneSpec {
                    duration,
                    playhead,
                    clip_start: track.start_sec,
                    clip_end: track.end_sec,
                    keyframe_times: kf_times,
                    selected,
                    is_subtrack: false,
                    is_audio: false,
                    show_waveform: false,
                    bg_lane: if selected { t.bg_raised } else { t.bg_surface },
                    grid_line: t.border,
                    clip_fill,
                    clip_band,
                    clip_border,
                    kf_color: t.accent,
                    kf_active_color: t.playhead,
                    playhead_color: t.playhead,
                    playhead_glow: t.accent_wash,
                };

                rows.push(
                    vello_node_track_row(
                        &track,
                        t,
                        dm,
                        nid,
                        depth,
                        kind_badge,
                        show_guides,
                        lane_spec,
                    )
                    .into_any_flex(),
                );
            }
            TimelineTrackKind::AnimationProperty {
                node_id,
                property,
                easing,
            } => {
                let nid = node_id;
                let prop = property.clone();
                let lane_spec = TrackLaneSpec {
                    duration,
                    playhead,
                    clip_start: track.start_sec,
                    clip_end: track.end_sec,
                    keyframe_times: kf_times,
                    selected,
                    is_subtrack: true,
                    is_audio: false,
                    show_waveform: false,
                    bg_lane: t.bg_inset,
                    grid_line: t.border,
                    clip_fill: if selected { t.accent_wash } else { t.bg_raised },
                    clip_band: t.accent,
                    clip_border: if selected { t.border_accent } else { t.border },
                    kf_color: t.accent,
                    kf_active_color: t.playhead,
                    playhead_color: t.playhead,
                    playhead_glow: t.accent_wash,
                };

                rows.push(
                    vello_subtrack_row(
                        t,
                        dm,
                        "subdirectory_arrow_right",
                        t.accent,
                        format!("{property} · {easing}"),
                        track.keyframes.len(),
                        lane_spec,
                        move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            st.active_anim_property = prop.clone();
                            let _ = st.render_current_frame();
                        },
                    )
                    .into_any_flex(),
                );
            }
            TimelineTrackKind::AudioTrack { track_index } => {
                let idx = track_index;
                let lane_spec = TrackLaneSpec {
                    duration,
                    playhead,
                    clip_start: track.start_sec,
                    clip_end: track.end_sec,
                    keyframe_times: Vec::new(),
                    selected: false,
                    is_subtrack: false,
                    is_audio: true,
                    show_waveform: show_waveforms,
                    bg_lane: t.bg_surface,
                    grid_line: t.border,
                    clip_fill: t.audio_wash,
                    clip_band: t.audio,
                    clip_border: t.audio,
                    kf_color: t.audio,
                    kf_active_color: t.playhead,
                    playhead_color: t.playhead,
                    playhead_glow: t.accent_wash,
                };

                rows.push(
                    vello_subtrack_row(
                        t,
                        dm,
                        if track.muted_or_hidden {
                            "volume_off"
                        } else {
                            "graphic_eq"
                        },
                        if track.muted_or_hidden {
                            t.danger
                        } else {
                            t.audio
                        },
                        format!("{} ({})", track.label, track.subtitle),
                        0,
                        lane_spec,
                        move |st: &mut StudioState| {
                            let _ = st.inspector_adjust_audio_track(idx, None, true);
                        },
                    )
                    .into_any_flex(),
                );
            }
        }
    }

    // Functional "Add Track / Layer" footer bar (previously a dead non-clickable label!)
    rows.push(
        sized_box(
            flex_row(vec![
                sized_box(
                    flex_row((
                        icon("playlist_add", 13.0, t.accent),
                        txtw("ADD TRACK", 8.5, FontWeight::SEMI_BOLD, t.muted),
                    ))
                    .gap(4.0_f64.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                )
                .width(dm.track_gutter_w.px())
                .padding(Padding::from_vh(0.0, 8.0))
                .into_any_flex(),
                flex_row((
                    icon_text_btn("crop_16_9", "+ Rect Card", t, |st| {
                        let _ = st.insert_catalog_item("rect-card");
                    }),
                    icon_text_btn("title", "+ Headline", t, |st| {
                        let _ = st.insert_catalog_item("text-headline");
                    }),
                    icon_text_btn("blur_on", "+ WGSL Grain", t, |st| {
                        let _ = st.insert_catalog_item("shader-film-grain");
                    }),
                    icon_text_btn("music_note", "+ Audio Bed", t, |st| {
                        let _ = st.insert_catalog_item("audio-synth-chime");
                    }),
                ))
                .gap(4.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .padding(Padding::from_vh(0.0, 6.0))
                .into_any_flex(),
            ])
            .gap(0.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .height(30.0_f64.px())
        .background_color(t.bg_inset)
        .border(t.border, 1.0)
        .into_any_flex(),
    );

    flex_col(rows)
        .gap(0.0_f64.px())
        .background_color(t.bg_surface)
}

#[allow(clippy::too_many_arguments)]
fn vello_node_track_row(
    track: &kine_session::timeline::TimelineTrack,
    t: &StudioPalette,
    dm: &DensityMetrics,
    nid: NodeId,
    depth: usize,
    kind_badge: &'static str,
    show_guides: bool,
    lane_spec: TrackLaneSpec,
) -> impl WidgetView<StudioState> {
    let t = *t;
    let selected = track.selected;
    let has_children = track.has_children;
    let collapsed = track.collapsed;
    let hidden = track.muted_or_hidden;
    let label_str = clipped(&track.label, 22);
    let time_str = format!(
        "{:.2}s–{:.2}s",
        track.start_sec.max(0.0),
        track.end_sec.max(0.0)
    );
    let kf_count = track.keyframes.len();

    let mut gutter_items: Vec<Child> = Vec::new();
    for lvl in 0..depth {
        gutter_items.push(
            sized_box(label(""))
                .width(dm.cascade_indent.px())
                .height(dm.track_h.px())
                .border(
                    if show_guides && lvl + 1 == depth {
                        t.border_strong
                    } else {
                        transparent()
                    },
                    1.0,
                )
                .into_any_flex(),
        );
    }

    if has_children {
        let chevron = if collapsed {
            "chevron_right"
        } else {
            "expand_more"
        };
        gutter_items.push(
            button(icon(chevron, 13.0, t.muted), move |st: &mut StudioState| {
                st.toggle_node_collapsed(nid);
            })
            .padding(Padding::all(2.0))
            .background_color(transparent())
            .border(transparent(), 1.0)
            .into_any_flex(),
        );
    } else {
        gutter_items.push(
            sized_box(label(""))
                .width(14.0_f64.px())
                .height(1.0_f64.px())
                .into_any_flex(),
        );
    }

    gutter_items.push(
        button(
            icon(
                if hidden {
                    "visibility_off"
                } else {
                    "visibility"
                },
                12.5,
                if hidden { t.danger } else { t.muted },
            ),
            move |st: &mut StudioState| {
                let _ = st.layer_toggle_visibility(nid);
            },
        )
        .padding(Padding::all(2.0))
        .background_color(transparent())
        .border(transparent(), 1.0)
        .into_any_flex(),
    );

    gutter_items.push(kind_badge_pill(kind_badge, &t).into_any_flex());

    gutter_items.push(
        button(
            txtw(
                label_str.clone(),
                10.0,
                if selected {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                },
                if selected { t.text } else { t.text_soft },
            ),
            move |st: &mut StudioState| {
                st.selected_node = Some(nid);
                let _ = st.render_current_frame();
            },
        )
        .padding(Padding::from_vh(2.0, 4.0))
        .background_color(transparent())
        .border(transparent(), 1.0)
        .flex(1.0)
        .into_any_flex(),
    );

    let lane_overlay = button(
        flex_row((
            sized_box(label("")).width(8.0_f64.px()),
            txt(label_str, 9.5, t.text),
            FlexSpacer::Flex(1.0),
            if kf_count > 0 {
                mono(format!("◆ {kf_count}"), 8.5, t.accent_ink).into_any_flex()
            } else {
                sized_box(label("")).into_any_flex()
            },
            mono(time_str, 8.5, t.muted),
            sized_box(label("")).width(6.0_f64.px()),
        ))
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        move |st: &mut StudioState| {
            st.selected_node = Some(nid);
            let _ = st.render_current_frame();
        },
    )
    .padding(Padding::all(0.0))
    .background_color(transparent())
    .active_background_color(t.press)
    .border(transparent(), 1.0);

    flex_row(vec![
        sized_box(
            flex_row(gutter_items)
                .gap(3.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .width(dm.track_gutter_w.px())
        .height(dm.track_h.px())
        .padding(Padding::from_vh(0.0, 5.0))
        .background_color(if selected { t.bg_raised } else { t.bg_inset })
        .border(if selected { t.border_accent } else { t.border }, 1.0)
        .into_any_flex(),
        sized_box(zstack((
            sized_box(track_lane_canvas(lane_spec)).expand(),
            sized_box(lane_overlay).expand(),
        )))
        .height(dm.track_h.px())
        .flex(1.0)
        .into_any_flex(),
    ])
    .gap(0.0_f64.px())
}

#[allow(clippy::too_many_arguments)]
fn vello_subtrack_row(
    t: &StudioPalette,
    dm: &DensityMetrics,
    icon_name: &'static str,
    icon_color: Color,
    sub_label: String,
    kf_count: usize,
    lane_spec: TrackLaneSpec,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static + Clone,
) -> impl WidgetView<StudioState> {
    let t = *t;
    let cb_lane = cb.clone();
    let short_lbl = clipped(&sub_label, 24);

    let lane_overlay = button(
        flex_row((
            FlexSpacer::Flex(1.0),
            if kf_count > 0 {
                mono(format!("◆ {kf_count} keyframes"), 8.0, icon_color).into_any_flex()
            } else {
                sized_box(label("")).into_any_flex()
            },
            sized_box(label("")).width(8.0_f64.px()),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb_lane,
    )
    .padding(Padding::all(0.0))
    .background_color(transparent())
    .active_background_color(t.press)
    .border(transparent(), 1.0);

    flex_row(vec![
        sized_box(
            button(
                flex_row((
                    sized_box(label("")).width(dm.cascade_indent.px()),
                    icon(icon_name, 12.0, icon_color),
                    mono(short_lbl, 9.0, t.text_soft),
                ))
                .gap(4.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                cb,
            )
            .padding(Padding::from_vh(1.0, 5.0))
            .background_color(transparent())
            .border(transparent(), 1.0),
        )
        .width(dm.track_gutter_w.px())
        .height(dm.subtrack_h.px())
        .background_color(t.bg_inset)
        .border(t.border, 1.0)
        .into_any_flex(),
        sized_box(zstack((
            sized_box(track_lane_canvas(lane_spec)).expand(),
            sized_box(lane_overlay).expand(),
        )))
        .height(dm.subtrack_h.px())
        .flex(1.0)
        .into_any_flex(),
    ])
    .gap(0.0_f64.px())
}

// ── Right Inspector Dock ─────────────────────────────────────────────────────

fn right_dock(
    state: &mut StudioState,
    t: &StudioPalette,
    dm: &DensityMetrics,
) -> impl WidgetView<StudioState> {
    let cur = state.right_tab;
    let tabs = tab_strip(
        vec![
            tab("tune", "Design", cur == RightDockTab::Design, t, |st| {
                st.right_tab = RightDockTab::Design
            })
            .into_any_flex(),
            tab("layers", "Layers", cur == RightDockTab::Layers, t, |st| {
                st.right_tab = RightDockTab::Layers
            })
            .into_any_flex(),
            tab(
                "movie_Creation",
                "Renders",
                cur == RightDockTab::Renders,
                t,
                |st| st.right_tab = RightDockTab::Renders,
            )
            .into_any_flex(),
        ],
        t,
        dm,
    );
    let content: Boxed = match state.right_tab {
        RightDockTab::Design => design_panel(state, t, dm),
        RightDockTab::Layers => layers_panel(state, t, dm),
        RightDockTab::Renders => renders_panel(state, t, dm),
    };

    flex_col(vec![
        tabs.into_any_flex(),
        portal(content).flex(1.0).into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .must_fill_major_axis(true)
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .background_color(t.bg_surface)
    .border(t.border_strong, 1.0)
    .corner_radius(dm.panel_radius)
}

fn design_panel(state: &mut StudioState, t: &StudioPalette, dm: &DensityMetrics) -> Boxed {
    let Some(snap) = state.inspect_selected_node() else {
        return empty_state(
            t,
            "select",
            "Nothing selected",
            "Select a layer or a node in the timeline to edit its properties.",
        );
    };
    let nid = snap.node_id;
    let step = state.settings.nudge_step_px;
    let anim_props: Vec<String> = snap.animations.iter().map(|(k, _)| k.clone()).collect();
    let is_anim = |prop: &str| anim_props.iter().any(|p| p == prop);

    let mut kids: Vec<Child> = Vec::new();

    // 1. Selected Node Context Banner + Cascading Expand/Collapse All controls
    kids.push(
        flex_col((
            flex_row((
                kind_badge_pill(snap.kind_badge, t),
                txtw(
                    clipped(&snap.label, 20),
                    11.5,
                    FontWeight::SEMI_BOLD,
                    t.text,
                ),
                mono(format!("#{nid}"), 9.0, t.muted),
                FlexSpacer::Flex(1.0),
                icon_btn("visibility", t.text_soft, t, move |st: &mut StudioState| {
                    let _ = st.layer_toggle_visibility(nid);
                }),
                icon_btn("content_copy", t.text_soft, t, |st: &mut StudioState| {
                    let _ = st.timeline_duplicate_selected();
                }),
                icon_btn("delete", t.danger, t, |st: &mut StudioState| {
                    let _ = st.timeline_delete_selected();
                }),
            ))
            .gap(4.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            flex_row((
                txt("Cascading Sections", 8.5, t.muted),
                FlexSpacer::Flex(1.0),
                button(
                    txt("Expand All", 8.5, t.accent_ink),
                    |st: &mut StudioState| {
                        st.set_all_inspector_sections(true);
                    },
                )
                .padding(Padding::from_vh(1.0, 5.0))
                .corner_radius(3.0)
                .background_color(t.bg_inset)
                .border(t.border, 1.0),
                button(txt("Collapse All", 8.5, t.muted), |st: &mut StudioState| {
                    st.set_all_inspector_sections(false);
                })
                .padding(Padding::from_vh(1.0, 5.0))
                .corner_radius(3.0)
                .background_color(t.bg_inset)
                .border(t.border, 1.0),
            ))
            .gap(4.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        ))
        .gap(5.0_f64.px())
        .padding(Padding::all(8.0))
        .background_color(t.bg_raised)
        .border(t.border, 1.0)
        .into_any_flex(),
    );

    // 2. Layout & Geometry Section (with interactive `[-]` / `[+]` stepper cells!)
    let layout_open = state.is_inspector_section_open("layout");
    kids.push(
        section_header(
            "layout",
            "Layout & Geometry",
            format!(
                "{:.0},{:.0} · {:.0}×{:.0}",
                snap.computed_rect.x,
                snap.computed_rect.y,
                snap.computed_rect.width,
                snap.computed_rect.height
            ),
            layout_open,
            t,
        )
        .into_any_flex(),
    );
    if layout_open {
        kids.push(
            flex_col(vec![
                flex_row(vec![
                    interactive_num_cell(
                        "X",
                        format!("{:.0}", snap.computed_rect.x),
                        is_anim("offsetX"),
                        t,
                        move |st| {
                            let _ = st.inspector_nudge_position(-step, 0.0);
                        },
                        move |st| {
                            let _ = st.inspector_nudge_position(step, 0.0);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    interactive_num_cell(
                        "Y",
                        format!("{:.0}", snap.computed_rect.y),
                        is_anim("offsetY"),
                        t,
                        move |st| {
                            let _ = st.inspector_nudge_position(0.0, -step);
                        },
                        move |st| {
                            let _ = st.inspector_nudge_position(0.0, step);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(4.0_f64.px())
                .into_any_flex(),
                flex_row(vec![
                    interactive_num_cell(
                        "W",
                        format!("{:.0}", snap.computed_rect.width),
                        is_anim("width"),
                        t,
                        move |st| {
                            let _ = st.inspector_nudge_size(-step, 0.0);
                        },
                        move |st| {
                            let _ = st.inspector_nudge_size(step, 0.0);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    interactive_num_cell(
                        "H",
                        format!("{:.0}", snap.computed_rect.height),
                        is_anim("height"),
                        t,
                        move |st| {
                            let _ = st.inspector_nudge_size(0.0, -step);
                        },
                        move |st| {
                            let _ = st.inspector_nudge_size(0.0, step);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(4.0_f64.px())
                .into_any_flex(),
                // 6-way Canvas Alignment Bar (Left, Center, Right, Top, Middle, Bottom)
                flex_row(vec![
                    icon_btn("align_horizontal_left", t.text_soft, t, |st| {
                        let _ = st.inspector_align_selected(Some("left"), None);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("align_horizontal_center", t.text_soft, t, |st| {
                        let _ = st.inspector_align_selected(Some("center"), None);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("align_horizontal_right", t.text_soft, t, |st| {
                        let _ = st.inspector_align_selected(Some("right"), None);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("align_vertical_top", t.text_soft, t, |st| {
                        let _ = st.inspector_align_selected(None, Some("top"));
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("align_vertical_center", t.text_soft, t, |st| {
                        let _ = st.inspector_align_selected(None, Some("middle"));
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("align_vertical_bottom", t.text_soft, t, |st| {
                        let _ = st.inspector_align_selected(None, Some("bottom"));
                    })
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(2.0_f64.px())
                .into_any_flex(),
            ])
            .gap(4.5_f64.px())
            .padding(Padding::all(dm.panel_pad))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }

    // 3. 2D Transform Section (`TranslateX`, `TranslateY`, `Scale`, `Rotation`)
    let transform_open = state.is_inspector_section_open("transform");
    kids.push(
        section_header(
            "transform",
            "2D Transform",
            format!("{:.0}° · {:.2}×", snap.rotation_deg, snap.scale_x),
            transform_open,
            t,
        )
        .into_any_flex(),
    );
    if transform_open {
        kids.push(
            flex_col(vec![
                flex_row(vec![
                    interactive_num_cell(
                        "TX",
                        format!("{:+.0}", snap.translate_x),
                        is_anim("translateX"),
                        t,
                        |st| {
                            let _ = st.inspector_nudge_translation(-8.0, 0.0);
                        },
                        |st| {
                            let _ = st.inspector_nudge_translation(8.0, 0.0);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    interactive_num_cell(
                        "TY",
                        format!("{:+.0}", snap.translate_y),
                        is_anim("translateY"),
                        t,
                        |st| {
                            let _ = st.inspector_nudge_translation(0.0, -8.0);
                        },
                        |st| {
                            let _ = st.inspector_nudge_translation(0.0, 8.0);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(4.0_f64.px())
                .into_any_flex(),
                flex_row(vec![
                    interactive_num_cell(
                        "ROT",
                        format!("{:.0}°", snap.rotation_deg),
                        is_anim("rotation"),
                        t,
                        |st| {
                            let _ = st.inspector_nudge_rotation(-15.0);
                        },
                        |st| {
                            let _ = st.inspector_nudge_rotation(15.0);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    interactive_num_cell(
                        "SCALE",
                        format!("{:.2}×", snap.scale_x),
                        is_anim("scale"),
                        t,
                        |st| {
                            let _ = st.inspector_nudge_scale(-0.10);
                        },
                        |st| {
                            let _ = st.inspector_nudge_scale(0.10);
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("restart_alt", t.text_soft, t, |st| {
                        let _ = st.inspector_reset_transform();
                    })
                    .into_any_flex(),
                ])
                .gap(4.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
            ])
            .gap(4.5_f64.px())
            .padding(Padding::all(dm.panel_pad))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }

    // 4. Appearance & Fill Section
    let style_open = state.is_inspector_section_open("style");
    let cur_color = snap
        .fill
        .clone()
        .or_else(|| snap.text_color.clone())
        .unwrap_or_else(|| "#a9b665".to_string());
    kids.push(
        section_header(
            "style",
            "Appearance & Fill",
            format!("{} · {:.0}%", cur_color, snap.opacity * 100.0),
            style_open,
            t,
        )
        .into_any_flex(),
    );
    if style_open {
        let opacity = f64::from(snap.opacity);
        kids.push(
            flex_col(vec![
                flex_row((
                    row_label("Opacity", t),
                    if is_anim("opacity") {
                        mono("◆ KEYFRAMED", 8.0, t.accent_ink).into_any_flex()
                    } else {
                        sized_box(label("")).into_any_flex()
                    },
                    FlexSpacer::Flex(1.0),
                    mono(format!("{:.0}%", snap.opacity * 100.0), 9.5, t.text_soft),
                ))
                .gap(4.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(0.0, 1.0, opacity, |st: &mut StudioState, v: f64| {
                    let _ = st.inspector_set_opacity(v as f32);
                })
                .into_any_flex(),
                field(cur_color, t, |st: &mut StudioState, hex: String| {
                    let _ = st.inspector_set_fill(&hex);
                })
                .into_any_flex(),
                flex_row((
                    swatch(t.accent, "#a9b665", t),
                    swatch(t.selection, "#7daea3", t),
                    swatch(t.danger, "#ea6962", t),
                    swatch(t.warning, "#d8a657", t),
                    swatch(t.media, "#7daea3", t),
                    swatch(t.container, "#d8a657", t),
                    swatch(t.audio, "#d3869b", t),
                    swatch(Color::from_rgb8(221, 199, 161), "#ddc7a1", t),
                ))
                .gap(4.0_f64.px())
                .into_any_flex(),
                flex_row(vec![interactive_num_cell(
                    "RADIUS",
                    format!("{:.0}px", snap.corner_radius),
                    is_anim("cornerRadius"),
                    t,
                    |st| {
                        let _ = st.inspector_nudge_corner_radius(-4.0);
                    },
                    |st| {
                        let _ = st.inspector_nudge_corner_radius(4.0);
                    },
                )
                .flex(1.0)
                .into_any_flex()])
                .into_any_flex(),
            ])
            .gap(4.5_f64.px())
            .padding(Padding::all(dm.panel_pad))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }

    // 5. Stroke & Drop Shadow Section
    let stroke_open = state.is_inspector_section_open("stroke");
    kids.push(
        section_header(
            "stroke",
            "Stroke & Drop Shadow",
            format!("{:.0}px · blur {:.0}", snap.stroke_width, snap.shadow_blur),
            stroke_open,
            t,
        )
        .into_any_flex(),
    );
    if stroke_open {
        kids.push(
            flex_col(vec![flex_row(vec![
                interactive_num_cell(
                    "STROKE",
                    format!("{:.0}px", snap.stroke_width),
                    false,
                    t,
                    |st| {
                        let _ = st.inspector_set_stroke(None, Some(-1.0));
                    },
                    |st| {
                        let _ = st.inspector_set_stroke(None, Some(1.0));
                    },
                )
                .flex(1.0)
                .into_any_flex(),
                interactive_num_cell(
                    "SHADOW",
                    format!("{:.0}px", snap.shadow_blur),
                    false,
                    t,
                    |st| {
                        let _ = st.inspector_nudge_shadow_blur(-4.0);
                    },
                    |st| {
                        let _ = st.inspector_nudge_shadow_blur(4.0);
                    },
                )
                .flex(1.0)
                .into_any_flex(),
            ])
            .gap(4.0_f64.px())
            .into_any_flex()])
            .gap(4.0_f64.px())
            .padding(Padding::all(dm.panel_pad))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }

    // 6. Typography Section (when a Text node is selected)
    if let Some(text_content) = snap.text_content.clone() {
        let text_open = state.is_inspector_section_open("text");
        kids.push(
            section_header(
                "text",
                "Typography",
                format!(
                    "\"{}\" · {:.0}px",
                    clipped(&text_content, 12),
                    snap.font_size.unwrap_or(16.0)
                ),
                text_open,
                t,
            )
            .into_any_flex(),
        );
        if text_open {
            let cur_w = snap.font_weight.unwrap_or(400);
            kids.push(
                flex_col(vec![
                    field(text_content, t, |st: &mut StudioState, next: String| {
                        let _ = st.inspector_set_text_content(&next);
                    })
                    .into_any_flex(),
                    flex_row(vec![
                        interactive_num_cell(
                            "SIZE",
                            format!("{:.0}px", snap.font_size.unwrap_or(16.0)),
                            is_anim("fontSize"),
                            t,
                            |st| {
                                let _ = st.inspector_nudge_font_size(-2.0);
                            },
                            |st| {
                                let _ = st.inspector_nudge_font_size(2.0);
                            },
                        )
                        .flex(1.0)
                        .into_any_flex(),
                        button(
                            mono(format!("W:{cur_w}"), 9.0, t.text_soft),
                            move |st: &mut StudioState| {
                                let next_w = match cur_w {
                                    400 => 600,
                                    600 => 700,
                                    _ => 400,
                                };
                                let _ = st.inspector_set_font_weight(next_w);
                            },
                        )
                        .padding(Padding::from_vh(4.0, 8.0))
                        .corner_radius(5.0)
                        .background_color(t.bg_surface)
                        .border(t.border, 1.0)
                        .into_any_flex(),
                    ])
                    .gap(4.0_f64.px())
                    .into_any_flex(),
                    flex_row((
                        icon_text_btn("format_align_left", "Left", t, |st| {
                            let _ = st.inspector_set_text_align("left");
                        }),
                        icon_text_btn("format_align_center", "Center", t, |st| {
                            let _ = st.inspector_set_text_align("center");
                        }),
                        icon_text_btn("format_align_right", "Right", t, |st| {
                            let _ = st.inspector_set_text_align("right");
                        }),
                    ))
                    .gap(3.0_f64.px())
                    .into_any_flex(),
                ])
                .gap(4.5_f64.px())
                .padding(Padding::all(dm.panel_pad))
                .background_color(t.bg_inset)
                .border(t.border, 1.0)
                .into_any_flex(),
            );
        }
    }

    // 7. Keyframes & Motion Section (with Vello Easing Curve visualizer + active track list!)
    let motion_open = state.is_inspector_section_open("motion");
    let active_prop = state.active_anim_property.clone();
    let active_ease = state.active_easing_input.clone();
    kids.push(
        section_header(
            "motion",
            "Keyframes & Motion",
            format!("{} tracks · {active_prop}", snap.animations.len()),
            motion_open,
            t,
        )
        .into_any_flex(),
    );
    if motion_open {
        let mut motion_rows: Vec<Child> = Vec::new();
        motion_rows.push(
            flex_row((
                chip_btn("translateX", &active_prop, t),
                chip_btn("translateY", &active_prop, t),
                chip_btn("opacity", &active_prop, t),
                chip_btn("scale", &active_prop, t),
                chip_btn("rotation", &active_prop, t),
            ))
            .gap(2.5_f64.px())
            .into_any_flex(),
        );

        // Vello-painted Easing Curve Preview + Easing Selector
        motion_rows.push(
            flex_row(vec![
                sized_box(easing_curve_canvas(EasingCurveSpec {
                    easing: active_ease.clone(),
                    bg: t.bg_surface,
                    grid: t.border,
                    stroke: t.accent,
                    endpoint: t.playhead,
                }))
                .width(86.0_f64.px())
                .height(34.0_f64.px())
                .into_any_flex(),
                flex_col((
                    flex_row((
                        icon_text_btn("linear_scale", "linear", t, |st| {
                            let p = st.active_anim_property.clone();
                            let _ = st.inspector_set_easing(&p, "linear");
                        }),
                        icon_text_btn("speed", "easeOut", t, |st| {
                            let p = st.active_anim_property.clone();
                            let _ = st.inspector_set_easing(&p, "easeOutCubic");
                        }),
                        icon_text_btn("bolt", "spring", t, |st| {
                            let p = st.active_anim_property.clone();
                            let _ = st.inspector_set_easing(&p, "spring(180, 18)");
                        }),
                    ))
                    .gap(2.0_f64.px()),
                    mono(format!("Curve: {active_ease}"), 8.5, t.muted),
                ))
                .gap(2.0_f64.px())
                .flex(1.0)
                .into_any_flex(),
            ])
            .gap(6.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .into_any_flex(),
        );

        // Active animation tracks on this node with delete buttons
        for (prop_name, spec) in &snap.animations {
            let p_del = prop_name.clone();
            let p_sel = prop_name.clone();
            let kf_n = spec.keyframes.as_ref().map_or(2, Vec::len);
            let ease_lbl = spec.easing.as_deref().unwrap_or("linear");
            motion_rows.push(
                flex_row((
                    button(
                        flex_row((
                            mono("◆", 9.0, t.accent),
                            txtw(prop_name.clone(), 9.5, FontWeight::MEDIUM, t.text),
                            mono(
                                format!(
                                    "{:.2}s..{:.2}s · {kf_n}kf · {ease_lbl}",
                                    spec.start, spec.end
                                ),
                                8.5,
                                t.muted,
                            ),
                        ))
                        .gap(4.0_f64.px())
                        .cross_axis_alignment(CrossAxisAlignment::Center),
                        move |st: &mut StudioState| {
                            st.active_anim_property = p_sel.clone();
                        },
                    )
                    .padding(Padding::from_vh(2.0, 5.0))
                    .background_color(transparent())
                    .border(transparent(), 1.0)
                    .flex(1.0),
                    icon_btn("close", t.danger, t, move |st: &mut StudioState| {
                        let _ = st.inspector_delete_animation_track(&p_del);
                    }),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .background_color(t.bg_surface)
                .border(t.border, 1.0)
                .corner_radius(4.0)
                .into_any_flex(),
            );
        }

        motion_rows.push(
            flex_row((
                icon_text_btn("diamond", "+ Keyframe @ Playhead", t, |st| {
                    let _ = st.timeline_add_keyframe_at_playhead();
                }),
                FlexSpacer::Flex(1.0),
                toggle("Auto-KF", state.auto_keyframe, t, |st| {
                    st.auto_keyframe = !st.auto_keyframe;
                }),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .into_any_flex(),
        );

        kids.push(
            flex_col(motion_rows)
                .gap(4.5_f64.px())
                .padding(Padding::all(dm.panel_pad))
                .background_color(t.bg_inset)
                .border(t.border, 1.0)
                .into_any_flex(),
        );
    }

    // 8. WGSL Shaders & Grade Section
    let fx_open = state.is_inspector_section_open("fx") || !snap.shader_uniforms.is_empty();
    kids.push(
        section_header(
            "fx",
            "WGSL Shaders & Grade",
            format!("{} uniforms", snap.shader_uniforms.len()),
            fx_open,
            t,
        )
        .into_any_flex(),
    );
    if fx_open {
        let mut fx: Vec<Child> = Vec::new();
        for (name, value) in &snap.shader_uniforms {
            let key = name.clone();
            let val = f64::from(*value);
            fx.push(
                flex_row((
                    txt(name.clone(), 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    mono(format!("{value:.3}"), 9.0, t.accent_ink),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
            );
            fx.push(
                slider(0.0, 2.0, val, move |st: &mut StudioState, v: f64| {
                    let _ = st.inspector_set_shader_uniform_f32(&key, v as f32);
                })
                .into_any_flex(),
            );
        }
        fx.push(
            flex_row((
                icon_text_btn("add", "Vignette", t, |st| {
                    let _ = st.insert_catalog_item("shader-vignette");
                }),
                icon_text_btn("add", "Grain", t, |st| {
                    let _ = st.insert_catalog_item("shader-film-grain");
                }),
                icon_text_btn("add", "Chroma", t, |st| {
                    let _ = st.insert_catalog_item("shader-chromatic");
                }),
                icon_text_btn("add", "CRT", t, |st| {
                    let _ = st.insert_catalog_item("shader-scanlines");
                }),
            ))
            .gap(2.5_f64.px())
            .into_any_flex(),
        );
        kids.push(
            flex_col(fx)
                .gap(4.0_f64.px())
                .padding(Padding::all(dm.panel_pad))
                .background_color(t.bg_inset)
                .border(t.border, 1.0)
                .into_any_flex(),
        );
    }

    // Inspector footer actions
    kids.push(
        flex_row((
            icon_text_btn("unfold_more", "Expand All", t, |st: &mut StudioState| {
                st.set_all_inspector_sections(true);
            }),
            icon_text_btn("unfold_less", "Collapse All", t, |st: &mut StudioState| {
                st.set_all_inspector_sections(false);
            }),
            FlexSpacer::Flex(1.0),
            icon_text_btn("tune", "UI Settings", t, |st: &mut StudioState| {
                st.settings_open = true;
            }),
        ))
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::all(8.0))
        .background_color(t.bg_raised)
        .border(t.border_strong, 1.0)
        .into_any_flex(),
    );

    flex_col(kids)
        .gap(0.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .background_color(t.bg_surface)
        .boxed()
}

fn layers_panel(state: &mut StudioState, t: &StudioPalette, dm: &DensityMetrics) -> Boxed {
    let show_guides = state.settings.show_cascade_guides;
    let mut rows: Vec<Child> = Vec::new();

    rows.push(
        flex_row((
            txtw("SCENE HIERARCHY", 8.5, FontWeight::SEMI_BOLD, t.muted),
            FlexSpacer::Flex(1.0),
            button(
                txt("Expand All", 8.5, t.accent_ink),
                |st: &mut StudioState| {
                    st.collapsed_nodes.clear();
                },
            )
            .padding(Padding::from_vh(1.5, 5.0))
            .corner_radius(3.0)
            .background_color(t.bg_inset)
            .border(t.border, 1.0),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(4.0, 6.0))
        .into_any_flex(),
    );

    for row in state.build_layer_tree_rows() {
        let nid = row.node_id;
        let mut line_items: Vec<Child> = Vec::new();

        for lvl in 0..row.depth {
            line_items.push(
                sized_box(label(""))
                    .width(dm.cascade_indent.px())
                    .height(26.0_f64.px())
                    .border(
                        if show_guides && lvl + 1 == row.depth {
                            t.border_strong
                        } else {
                            transparent()
                        },
                        1.0,
                    )
                    .into_any_flex(),
            );
        }

        if row.has_children {
            let chevron = if row.collapsed {
                "chevron_right"
            } else {
                "expand_more"
            };
            line_items.push(
                button(icon(chevron, 13.0, t.muted), move |st: &mut StudioState| {
                    st.toggle_node_collapsed(nid);
                })
                .padding(Padding::all(2.0))
                .background_color(transparent())
                .border(transparent(), 1.0)
                .into_any_flex(),
            );
        } else {
            line_items.push(
                sized_box(label(""))
                    .width(14.0_f64.px())
                    .height(1.0_f64.px())
                    .into_any_flex(),
            );
        }

        // Interactive visibility toggle button (fixed from dead non-clickable icon!)
        line_items.push(
            button(
                icon(
                    if row.visible {
                        "visibility"
                    } else {
                        "visibility_off"
                    },
                    13.0,
                    if row.visible { t.muted } else { t.danger },
                ),
                move |st: &mut StudioState| {
                    let _ = st.layer_toggle_visibility(nid);
                },
            )
            .padding(Padding::all(2.0))
            .background_color(transparent())
            .border(transparent(), 1.0)
            .into_any_flex(),
        );

        line_items.push(kind_badge_pill(row.badge, t).into_any_flex());

        line_items.push(
            button(
                txtw(
                    clipped(&row.label, 20),
                    10.0,
                    if row.selected {
                        FontWeight::SEMI_BOLD
                    } else {
                        FontWeight::NORMAL
                    },
                    if row.selected { t.text } else { t.text_soft },
                ),
                move |st: &mut StudioState| {
                    st.selected_node = Some(nid);
                    let _ = st.render_current_frame();
                },
            )
            .padding(Padding::from_vh(2.0, 4.0))
            .corner_radius(4.0)
            .background_color(transparent())
            .active_background_color(t.press)
            .border(transparent(), 1.0)
            .flex(1.0)
            .into_any_flex(),
        );

        if row.anim_count > 0 {
            line_items.push(mono(format!("◆{}", row.anim_count), 8.5, t.accent).into_any_flex());
        }

        line_items.push(
            icon_btn("arrow_upward", t.text_soft, t, move |st| {
                let _ = st.layer_move_z_order(nid, -1);
            })
            .into_any_flex(),
        );
        line_items.push(
            icon_btn("arrow_downward", t.text_soft, t, move |st| {
                let _ = st.layer_move_z_order(nid, 1);
            })
            .into_any_flex(),
        );

        rows.push(
            flex_row(line_items)
                .gap(3.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .padding(Padding::from_vh(3.0, 5.0))
                .background_color(if row.selected {
                    t.bg_raised
                } else {
                    t.bg_inset
                })
                .border(
                    if row.selected {
                        t.border_accent
                    } else {
                        t.border
                    },
                    1.0,
                )
                .corner_radius(5.0)
                .into_any_flex(),
        );
    }
    flex_col(rows)
        .gap(2.5_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(dm.panel_pad))
        .boxed()
}

fn renders_panel(state: &mut StudioState, t: &StudioPalette, dm: &DensityMetrics) -> Boxed {
    let out = state.export_output_path.clone();
    let fmt = state.export_format.to_uppercase();
    let mut kids: Vec<Child> = Vec::new();
    kids.push(
        flex_row((
            txtw("RENDER & EXPORT QUEUE", 9.0, FontWeight::SEMI_BOLD, t.muted),
            FlexSpacer::Flex(1.0),
            badge(fmt, t.accent_ink, t.accent_wash),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into_any_flex(),
    );
    kids.push(
        flex_row((
            icon_text_btn("movie", "MP4", t, |st| {
                st.export_format = "mp4".to_string();
                st.export_output_path = "studio-render.mp4".to_string();
            }),
            icon_text_btn("folder_zip", "MKV", t, |st| {
                st.export_format = "mkv".to_string();
                st.export_output_path = "studio-render.mkv".to_string();
            }),
            icon_text_btn("language", "WebM", t, |st| {
                st.export_format = "webm".to_string();
                st.export_output_path = "studio-render.webm".to_string();
            }),
            icon_text_btn("gif_box", "GIF", t, |st| {
                st.export_format = "gif".to_string();
                st.export_output_path = "studio-render.gif".to_string();
            }),
            icon_text_btn("image", "PNG", t, |st| {
                st.export_format = "png".to_string();
                st.export_output_path = "studio-frame.png".to_string();
            }),
        ))
        .gap(2.5_f64.px())
        .into_any_flex(),
    );
    kids.push(
        field(out, t, |st: &mut StudioState, next: String| {
            st.export_output_path = next;
        })
        .into_any_flex(),
    );
    kids.push(
        flex_row((
            primary_btn("play_arrow", "Render Now", t, |st: &mut StudioState| {
                let _ = st.run_export_job();
            }),
            FlexSpacer::Flex(1.0),
            mono(
                format!(
                    "Q{} · Speed {}",
                    state.settings.export_quantizer, state.settings.export_speed
                ),
                8.5,
                t.muted,
            ),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into_any_flex(),
    );
    for job in state.render_queue.iter().rev().take(8) {
        let status = match &job.status {
            RenderJobStatus::Completed => "COMPLETED".to_string(),
            RenderJobStatus::Failed(err) => format!("FAILED: {err}"),
        };
        kids.push(
            flex_col((
                flex_row((
                    mono(format!("#{}", job.id), 9.0, t.accent_ink),
                    txtw(
                        job.composition_title.clone(),
                        9.5,
                        FontWeight::MEDIUM,
                        t.text,
                    ),
                    FlexSpacer::Flex(1.0),
                    mono(job.format.to_uppercase(), 8.5, t.accent_ink),
                ))
                .gap(4.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                txt(
                    format!(
                        "{} frames @ {}fps · {} · {} B",
                        job.frames, job.fps, status, job.bytes_written
                    ),
                    9.0,
                    t.muted,
                ),
                mono(job.output_path.display().to_string(), 8.5, t.off),
            ))
            .gap(1.5_f64.px())
            .padding(Padding::all(6.0))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .corner_radius(5.0)
            .into_any_flex(),
        );
    }
    flex_col(kids)
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(dm.panel_pad))
        .boxed()
}

fn empty_state(
    t: &StudioPalette,
    icon_name: &'static str,
    title: &'static str,
    help: &'static str,
) -> Boxed {
    flex_col(vec![
        icon(icon_name, 22.0, t.off).into_any_flex(),
        txtw(title, 12.0, FontWeight::MEDIUM, t.text_soft).into_any_flex(),
        txt(help, 10.0, t.muted).into_any_flex(),
    ])
    .gap(4.0_f64.px())
    .padding(Padding::all(20.0))
    .boxed()
}

/// Animated cascading section header (`transformed` chevron rotation + summary badge).
fn section_header(
    section_id: &'static str,
    title: &'static str,
    summary: impl Into<ArcStr>,
    open: bool,
    t: &StudioPalette,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((
            icon(
                if open { "expand_more" } else { "chevron_right" },
                13.5,
                if open { t.accent } else { t.muted },
            ),
            txtw(title, 10.5, FontWeight::SEMI_BOLD, t.text),
            FlexSpacer::Flex(1.0),
            mono(summary, 8.5, if open { t.accent_ink } else { t.muted }),
        ))
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        move |st: &mut StudioState| {
            st.toggle_inspector_section(section_id);
        },
    )
    .padding(Padding::from_vh(5.5, 8.0))
    .background_color(if open { t.bg_raised } else { t.bg_surface })
    .active_background_color(t.press)
    .border(t.border, 1.0)
    .hovered_border_color(t.border_strong)
}

fn chip_btn(
    value: &'static str,
    active_prop: &str,
    t: &StudioPalette,
) -> impl WidgetView<StudioState> {
    let t = *t;
    let active = active_prop == value;
    button(
        txtw(
            value,
            8.5,
            if active {
                FontWeight::SEMI_BOLD
            } else {
                FontWeight::NORMAL
            },
            if active { t.accent_ink } else { t.muted },
        ),
        move |st: &mut StudioState| {
            st.active_anim_property = value.to_string();
        },
    )
    .padding(Padding::from_vh(2.5, 5.5))
    .corner_radius(4.0)
    .background_color(if active { t.accent_wash } else { t.bg_surface })
    .active_background_color(t.press)
    .border(if active { t.border_accent } else { t.border }, 1.0)
    .hovered_border_color(t.border_strong)
}

fn swatch(color: Color, hex: &'static str, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        sized_box(label(""))
            .width(16.0_f64.px())
            .height(16.0_f64.px())
            .background_color(color)
            .corner_radius(3.0),
        move |st: &mut StudioState| {
            let _ = st.inspector_set_fill(hex);
        },
    )
    .padding(Padding::all(1.5))
    .corner_radius(4.0)
    .background_color(t.bg_raised)
    .active_background_color(t.press)
    .border(t.border, 1.0)
    .hovered_border_color(t.text)
}
