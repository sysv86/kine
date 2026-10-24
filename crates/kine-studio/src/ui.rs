//! Kine Studio shell — a warm-graphite native NLE built on Xilem 0.4.
//!
//! Layout contract:
//! - 40px header inside a `#12110f` frame.
//! - 6px gutters; left dock 248px, right inspector 320px, centre split preview/timeline.
//! - Every region is a 10px-radius card with a 1px white-alpha rim, never a heavy shadow.
//! - Chrome type is 10–12px Inter; timecode/ids/values use JetBrains Mono.
//! - Icons are bundled Material Symbols ligatures, never emoji.

use crate::catalog::CATALOG_ITEMS;
use crate::local_portal::portal;
use crate::preview_canvas::preview;
use crate::state::{LeftDockTab, RenderJobStatus, RightDockTab, StudioState};
use crate::theme::{
    sample_audio_meter_levels, StudioPalette, HEADER_H, ICON_FONT, MONO_FONT, PANEL_RADIUS,
    SHELL_GAP, TAB_H, TRACK_GUTTER, TRACK_H, UI_FONT,
};
use crate::timeline::{generate_ruler_ticks, TimelineTrackKind};
use kine_render::{FrameFormat, RenderBackend};
use std::path::Path;

use std::time::Duration;
use xilem::core::fork;
use xilem::view::task;
use xilem::{
    masonry::{
        core::ArcStr, parley::style::FontWeight, peniko::ImageBrush,
        properties::types::AsUnit as _, properties::Padding,
    },
    style::Style as _,
    view::{
        button, flex_col, flex_row, label, sized_box, slider, text_input, AnyFlexChild, Axis,
        CrossAxisAlignment, FlexExt as _, FlexSpacer,
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
pub fn run_studio(state: StudioState) -> Result<(), EventLoopError> {
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
    let t = StudioPalette::for_mode(state.dark_theme);
    let header = header_bar(state, &t);
    let status = status_bar(state, &t);
    let left = left_dock(state, &t);
    let centre = center_workspace(state, &t);
    let right = right_dock(state, &t);

    // Fixed chrome rails, fluid stage. Sidebars keep a predictable editor width;
    // the centre pane absorbs every window resize. Hiding the inspector collapses
    // the right rail completely.
    let mut body: Vec<Child> = vec![
        sized_box(left)
            .width(236.0_f64.px())
            .expand_height()
            .into_any_flex(),
        sized_box(centre)
            .expand_width()
            .expand_height()
            .flex(1.0)
            .into_any_flex(),
    ];
    if state.inspector_visible {
        body.push(
            sized_box(right)
                .width(300.0_f64.px())
                .expand_height()
                .into_any_flex(),
        );
    }

    let ui = flex_col(vec![
        header.into_any_flex(),
        sized_box(
            flex_row(body)
                .gap(SHELL_GAP.px())
                .cross_axis_alignment(CrossAxisAlignment::Fill)
                .must_fill_major_axis(true),
        )
        .expand_width()
        .padding(Padding::all(SHELL_GAP))
        .background_color(t.bg_shell)
        .flex(1.0)
        .into_any_flex(),
        status.into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .background_color(t.bg_shell);

    // A 30 Hz playback clock only while transport is running. The task is removed
    // from the tree when paused, so a static editor performs no idle rebuilds.
    fork(
        ui,
        state.is_playing.then(|| {
            task(
                |proxy| async move {
                    let mut tick = xilem::tokio::time::interval(Duration::from_millis(33));
                    loop {
                        tick.tick().await;
                        if proxy.message(()).is_err() {
                            break;
                        }
                    }
                },
                |st: &mut StudioState, ()| {
                    st.tick_playback(1.0 / 30.0);
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
    button(icon(name, 16.0, color), cb)
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
    button(icon(name, 16.0, t.accent), cb)
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
    button(txt(value, 12.0, t.text_soft), cb)
        .padding(Padding::from_vh(3.0, 8.0))
        .corner_radius(5.0)
        .background_color(transparent())
        .active_background_color(t.press)
        .border(transparent(), 1.0)
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
        flex_row((icon(name, 14.0, t.text_soft), txt(value, 11.0, t.text_soft)))
            .gap(4.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(3.0, 7.0))
    .corner_radius(5.0)
    .background_color(transparent())
    .active_background_color(t.press)
    .border(transparent(), 1.0)
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
            icon(name, 15.0, t.on_accent),
            txtw(value, 12.0, FontWeight::MEDIUM, t.on_accent),
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
    value: &'static str,
    active: bool,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        txtw(
            value,
            10.0,
            FontWeight::new(if active { 530.0 } else { 440.0 }),
            if active { t.text } else { t.muted },
        ),
        cb,
    )
    .padding(Padding::from_vh(3.0, 6.0))
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
            txt(value, 11.0, if on { t.text } else { t.muted }),
        ))
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(2.0, 6.0))
    .corner_radius(4.0)
    .background_color(if on { t.accent_wash } else { transparent() })
    .active_background_color(t.press)
    .border(if on { t.border_accent } else { transparent() }, 1.0)
    .hovered_border_color(t.border_strong)
}

fn badge(value: impl Into<ArcStr>, fg: Color, bg: Color) -> impl WidgetView<StudioState> {
    sized_box(txtw(value, 8.5, FontWeight::new(650.0), fg))
        .padding(Padding::from_vh(1.0, 4.0))
        .background_color(bg)
        .border(fg, 1.0)
        .corner_radius(3.0)
}

fn panel(t: &StudioPalette, body: impl WidgetView<StudioState>) -> impl WidgetView<StudioState> {
    let t = *t;
    sized_box(body)
        .background_color(t.bg_surface)
        .border(t.border_strong, 1.0)
        .corner_radius(PANEL_RADIUS)
}

fn tab_strip(children: Vec<Child>, t: &StudioPalette) -> impl WidgetView<StudioState> {
    sized_box(
        flex_row(children)
            .gap(2.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height((TAB_H + 6.0).px())
    .padding(Padding::from_vh(3.0, 4.0))
    .background_color(t.bg_surface)
    .border(transparent(), 1.0)
}

fn field(
    value: impl Into<String>,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState, String) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    text_input(value.into(), cb)
        .padding(Padding::from_vh(3.0, 7.0))
        .background_color(t.bg_inset)
        .border(t.border, 1.0)
        .corner_radius(5.0)
}

fn num_cell(
    name: &'static str,
    value: impl Into<ArcStr>,
    t: &StudioPalette,
) -> impl WidgetView<StudioState> {
    let t = *t;
    sized_box(flex_col((txt(name, 8.5, t.muted), mono(value, 9.5, t.text_soft))).gap(0.0_f64.px()))
        .padding(Padding::from_vh(2.0, 5.0))
        .background_color(t.bg_surface)
        .border(t.border, 1.0)
        .corner_radius(4.0)
}

fn row_label(value: &'static str, t: &StudioPalette) -> impl WidgetView<StudioState> {
    txt(value, 10.0, t.muted)
}

// ── Header ───────────────────────────────────────────────────────────────────

fn header_bar(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let comp = state
        .compositions
        .get(state.active_composition)
        .map(|c| clipped(&c.title, 24))
        .unwrap_or_else(|| "Untitled Composition".to_string());
    let (_status_icon, status_text, status_color) = if state.inspect_findings.is_empty() {
        ("check_circle", "0 issues".to_string(), t.accent)
    } else {
        (
            "warning",
            format!("{} findings", state.inspect_findings.len()),
            t.warning,
        )
    };
    let theme_icon = if state.dark_theme {
        "dark_mode"
    } else {
        "light_mode"
    };
    let inspector_icon = "view_sidebar";

    sized_box(
        flex_row(vec![
            flex_row((
                icon("auto_awesome", 16.0, t.accent),
                txtw("KINE", 12.0, FontWeight::new(650.0), t.text),
                txtw("STUDIO", 8.5, FontWeight::new(600.0), t.muted),
            ))
            .gap(5.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .into_any_flex(),
            divider(t).into_any_flex(),
            txtw(comp, 11.0, FontWeight::new(500.0), t.text).into_any_flex(),
            badge(status_text, status_color, t.bg_inset).into_any_flex(),
            FlexSpacer::Flex(1.0).into_any_flex(),
            icon_btn("photo_camera", t.text_soft, t, |st: &mut StudioState| {
                let _ = st.capture_current_frame(FrameFormat::Png, Path::new("studio-capture.png"));
            })
            .into_any_flex(),
            icon_btn("download", t.text_soft, t, |st: &mut StudioState| {
                let _ = st.capture_current_frame(FrameFormat::Svg, Path::new("studio-capture.svg"));
            })
            .into_any_flex(),
            divider(t).into_any_flex(),
            if state.inspector_visible {
                icon_btn_on(inspector_icon, t, |st: &mut StudioState| {
                    st.inspector_visible = false;
                })
                .into_any_flex()
            } else {
                icon_btn(inspector_icon, t.text_soft, t, |st: &mut StudioState| {
                    st.inspector_visible = true;
                })
                .into_any_flex()
            },
            icon_btn(theme_icon, t.text_soft, t, |st: &mut StudioState| {
                st.dark_theme = !st.dark_theme;
                st.status_banner = format!(
                    "Theme: {}",
                    if st.dark_theme { "Graphite" } else { "Paper" }
                );
            })
            .into_any_flex(),
            icon_btn("bolt", t.text_soft, t, |st: &mut StudioState| {
                st.cycle_render_backend();
                st.status_banner = format!("Backend: {}", st.engine.backend().name());
            })
            .into_any_flex(),
            icon_btn_on("smart_toy", t, |st: &mut StudioState| {
                st.right_tab = RightDockTab::AiAgent;
                st.inspector_visible = true;
                let _ = st.generate_selected_element_prompt("");
            })
            .into_any_flex(),
            primary_btn("ios_share", "Export", t, |st: &mut StudioState| {
                st.right_tab = RightDockTab::Renders;
                st.inspector_visible = true;
                let _ = st.run_export_job();
            })
            .into_any_flex(),
        ])
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(HEADER_H.px())
    .padding(Padding::from_vh(0.0, 10.0))
    .background_color(t.bg_shell)
    .border(t.border, 1.0)
}

fn status_bar(state: &StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let t = *t;
    let ready = state.inspect_findings.is_empty();
    let dot = if ready { t.accent } else { t.warning };
    let status_label = if ready {
        "READY".to_string()
    } else {
        format!("{} FINDINGS", state.inspect_findings.len())
    };
    let frame = state.current_frame_index();
    let total = (state.duration * state.fps).round().max(1.0) as u32;
    let gpu_active =
        !matches!(state.engine.backend(), RenderBackend::Cpu) && state.engine.gpu_available();
    let backend = if gpu_active { "GPU" } else { "CPU" };
    let scene = state
        .compositions
        .get(state.active_composition)
        .map(|c| {
            c.source_path
                .as_ref()
                .map(|p| p.display().to_string())
                .unwrap_or_else(|| format!("composition:{}", c.id))
        })
        .unwrap_or_default();
    sized_box(
        flex_row(vec![
            sized_box(label(""))
                .width(5.0_f64.px())
                .height(5.0_f64.px())
                .background_color(dot)
                .corner_radius(3.0)
                .into_any_flex(),
            txtw(status_label, 9.0, FontWeight::new(600.0), dot).into_any_flex(),
            mono(clipped(&scene, 42), 9.0, t.off).into_any_flex(),
            FlexSpacer::Flex(1.0).into_any_flex(),
            mono(
                format!(
                    "{}×{}  ·  {:.0} fps  ·  F{:03} / {:03}",
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
                if gpu_active { t.accent_ink } else { t.off },
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

// ── Left dock ────────────────────────────────────────────────────────────────

fn left_dock(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let cur = state.left_tab;
    let tabs = tab_strip(
        vec![
            tab("Comps", cur == LeftDockTab::Compositions, t, |st| {
                st.left_tab = LeftDockTab::Compositions
            })
            .into_any_flex(),
            tab("Assets", cur == LeftDockTab::Assets, t, |st| {
                st.left_tab = LeftDockTab::Assets
            })
            .into_any_flex(),
            tab("Code", cur == LeftDockTab::Code, t, |st| {
                st.left_tab = LeftDockTab::Code
            })
            .into_any_flex(),
            tab("Catalog", cur == LeftDockTab::Catalog, t, |st| {
                st.left_tab = LeftDockTab::Catalog
            })
            .into_any_flex(),
        ],
        t,
    );

    let content: Boxed = match state.left_tab {
        LeftDockTab::Compositions => composition_list(state, t),
        LeftDockTab::Assets => asset_list(state, t),
        LeftDockTab::Code => code_panel(state, t),
        LeftDockTab::Catalog => catalog_panel(state, t),
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
    .corner_radius(PANEL_RADIUS)
}

fn composition_list(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let mut rows: Vec<Child> = Vec::new();
    for (idx, comp) in state.compositions.iter().enumerate() {
        let active = idx == state.active_composition;
        let title = clipped(&comp.title, 22);
        let sub = clipped(&comp.subtitle, 30);
        let thumb_color = match idx % 3 {
            0 => t.clip_active,
            1 => t.clip_shader,
            _ => t.audio_wash,
        };
        rows.push(
            button(
                flex_row((
                    sized_box(label(""))
                        .width(52.0_f64.px())
                        .height(30.0_f64.px())
                        .background_color(thumb_color)
                        .border(t.border, 1.0)
                        .corner_radius(4.0),
                    flex_col((
                        txtw(title, 11.0, FontWeight::new(500.0), t.text),
                        txt(sub, 9.5, t.muted),
                    ))
                    .gap(1.0_f64.px())
                    .flex(1.0),
                ))
                .gap(8.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                move |st: &mut StudioState| {
                    let _ = st.select_composition(idx);
                },
            )
            .padding(Padding::from_vh(4.0, 6.0))
            .corner_radius(6.0)
            .background_color(if active { t.bg_raised } else { transparent() })
            .active_background_color(t.press)
            .border(
                if active {
                    t.border_accent
                } else {
                    transparent()
                },
                1.0,
            )
            .hovered_border_color(t.border_strong)
            .into_any_flex(),
        );
    }
    flex_col(rows)
        .gap(3.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(8.0))
        .boxed()
}

fn asset_list(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let mut rows: Vec<Child> = Vec::new();
    for asset in &state.assets {
        let name = clipped(&asset.name, 22);
        let uri = asset.uri.clone();
        let details = clipped(&asset.details, 32);
        let kind = asset.kind.badge();
        rows.push(
            button(
                flex_col((
                    flex_row((
                        badge(kind, t.text, t.bg_raised),
                        txtw(name, 11.0, FontWeight::new(500.0), t.text),
                        FlexSpacer::Flex(1.0),
                    ))
                    .gap(5.0_f64.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    mono(uri.clone(), 8.5, t.muted),
                    txt(details, 9.0, t.muted),
                ))
                .gap(2.0_f64.px()),
                move |st: &mut StudioState| {
                    st.status_banner = format!("Asset ready: {uri}");
                },
            )
            .padding(Padding::from_vh(4.0, 7.0))
            .corner_radius(5.0)
            .background_color(transparent())
            .active_background_color(t.press)
            .border(transparent(), 1.0)
            .hovered_border_color(t.border_strong)
            .into_any_flex(),
        );
    }
    flex_col(rows)
        .gap(2.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(8.0))
        .boxed()
}

fn code_panel(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let src = state.source_buffer.clone();
    flex_col(vec![
        flex_row((
            txtw("SCENE SOURCE", 9.0, FontWeight::new(600.0), t.muted),
            FlexSpacer::Flex(1.0),
            icon_text_btn("play_arrow", "Apply", t, |st: &mut StudioState| {
                if let Err(e) = st.apply_source_buffer() {
                    st.status_banner = e.to_string();
                }
            }),
        ))
        .gap(6.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .into_any_flex(),
        field(src, t, |st: &mut StudioState, next: String| {
            st.source_buffer = next;
        })
        .into_any_flex(),
        portal(
            sized_box(txt(state.source_buffer.clone(), 11.0, t.text_soft))
                .padding(Padding::all(6.0))
                .background_color(t.bg_inset),
        )
        .flex(1.0)
        .into_any_flex(),
    ])
    .gap(6.0_f64.px())
    .must_fill_major_axis(true)
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::all(8.0))
    .boxed()
}

fn catalog_panel(_state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let mut rows: Vec<Child> = Vec::new();
    for item in CATALOG_ITEMS {
        let id = item.id;
        let label = item.label;
        let desc = item.description;
        rows.push(
            button(
                flex_col((
                    flex_row((
                        icon("add", 13.0, t.accent),
                        txtw(label, 11.0, FontWeight::new(500.0), t.text),
                    ))
                    .gap(4.0_f64.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    txt(desc, 9.0, t.muted),
                ))
                .gap(1.0_f64.px()),
                move |st: &mut StudioState| {
                    let _ = st.insert_catalog_item(id);
                },
            )
            .padding(Padding::from_vh(3.0, 6.0))
            .corner_radius(5.0)
            .background_color(transparent())
            .active_background_color(t.press)
            .border(transparent(), 1.0)
            .hovered_border_color(t.border_strong)
            .into_any_flex(),
        );
    }
    flex_col(rows)
        .gap(1.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(6.0))
        .boxed()
}

// ── Centre: preview + timeline ───────────────────────────────────────────────

fn center_workspace(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let preview = panel(t, preview_pane(state, t));
    let timeline = panel(t, timeline_pane(state, t));
    xilem::view::split(preview, timeline)
        .split_axis(Axis::Vertical)
        .split_point(0.57)
}

fn preview_pane(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let comp = state
        .compositions
        .get(state.active_composition)
        .map(|c| c.title.clone())
        .unwrap_or_else(|| "Composition".to_string());
    let selected = state
        .inspect_selected_node()
        .map(|s| s.label)
        .unwrap_or_else(|| "No selection".to_string());
    let zoom_pct = format!("{:.0}%", state.viewport_zoom * 100.0);

    let breadcrumb = flex_row((
        icon("movie", 13.0, t.media),
        txtw(comp, 11.0, FontWeight::new(500.0), t.text_soft),
        icon("chevron_right", 12.0, t.muted),
        txt(selected, 10.0, t.muted),
        FlexSpacer::Flex(1.0),
        icon_btn("grid_3x3", t.text_soft, t, |st| {
            st.show_grid_overlay = !st.show_grid_overlay;
            let _ = st.render_current_frame();
        }),
        icon_btn("crop_free", t.text_soft, t, |st| {
            st.show_safe_guides = !st.show_safe_guides;
            let _ = st.render_current_frame();
        }),
        icon_btn("select_all", t.text_soft, t, |st| {
            st.show_bounding_boxes = !st.show_bounding_boxes;
            let _ = st.render_current_frame();
        }),
        divider(t),
        icon_btn("remove", t.text_soft, t, |st| {
            st.viewport_zoom = (st.viewport_zoom - 0.25).clamp(0.25, 4.0);
        }),
        mono(zoom_pct, 9.5, t.muted),
        icon_btn("add", t.text_soft, t, |st| {
            st.viewport_zoom = (st.viewport_zoom + 0.25).clamp(0.25, 4.0);
        }),
    ))
    .gap(4.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(Padding::from_vh(0.0, 8.0));

    let stage = sized_box(preview(
        ImageBrush::new(state.frame_image_data.clone()),
        t.bg_stage,
        t.border_strong,
    ))
    .expand()
    .padding(Padding::all(12.0))
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
        mono(timecode, 10.0, t.text_soft).into_any_flex(),
        FlexSpacer::Flex(1.0).into_any_flex(),
        icon_btn("skip_previous", t.text_soft, t, |st| {
            st.is_playing = false;
            st.seek_to(0.0, false);
        })
        .into_any_flex(),
        icon_btn("chevron_left", t.text_soft, t, |st| {
            st.step_frames(-1);
        })
        .into_any_flex(),
        sized_box(icon_btn(play_icon, t.accent, t, |st: &mut StudioState| {
            st.is_playing = !st.is_playing;
        }))
        .background_color(t.accent_wash)
        .border(t.border_accent, 1.0)
        .corner_radius(5.0)
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
        icon_toggle("volume_off", state.audio_muted, t, |st| {
            st.audio_muted = !st.audio_muted;
        })
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

fn timeline_pane(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
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
        icon_toggle("graphic_eq", state.show_audio_meters, t, |st| {
            st.show_audio_meters = !st.show_audio_meters;
        })
        .into_any_flex(),
        divider(t).into_any_flex(),
        icon_btn("chevron_left", t.text_soft, t, |st| {
            st.timeline_jump_prev_keyframe();
        })
        .into_any_flex(),
        icon_btn("diamond", t.accent, t, |st| {
            let _ = st.timeline_add_keyframe_at_playhead();
        })
        .into_any_flex(),
        icon_btn("chevron_right", t.text_soft, t, |st| {
            st.timeline_jump_next_keyframe();
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
        icon_btn("delete", t.text_soft, t, |st| {
            let _ = st.timeline_delete_selected();
        })
        .into_any_flex(),
        FlexSpacer::Flex(1.0).into_any_flex(),
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

    let ruler = timeline_ruler(state, t);
    let tracks = timeline_tracks(state, t);

    flex_col(vec![
        toolbar.into_any_flex(),
        ruler.into_any_flex(),
        portal(tracks).flex(1.0).into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .must_fill_major_axis(true)
    .cross_axis_alignment(CrossAxisAlignment::Fill)
}

fn timeline_ruler(state: &StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let ticks = generate_ruler_ticks(state.duration, state.timeline_zoom_pps);
    let mut labels: Vec<Child> = Vec::new();
    for tick in ticks.into_iter().filter(|t| t.is_major).take(12) {
        let label = tick
            .label
            .unwrap_or_else(|| format!("{:.1}s", tick.time_sec));
        labels.push(
            sized_box(mono(label, 9.0, t.muted))
                .flex(1.0)
                .into_any_flex(),
        );
    }
    flex_row(vec![
        sized_box(txtw("TRACKS", 9.0, FontWeight::new(600.0), t.muted))
            .width(TRACK_GUTTER.px())
            .padding(Padding::from_vh(0.0, 8.0))
            .into_any_flex(),
        flex_row(labels).gap(0.0_f64.px()).flex(1.0).into_any_flex(),
    ])
    .gap(0.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(Padding::from_vh(3.0, 0.0))
    .background_color(t.bg_inset)
    .border(t.border, 1.0)
}

fn timeline_tracks(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let tracks = state.build_timeline_tracks();
    let duration = state.duration.max(0.1);
    let mut rows: Vec<Child> = Vec::new();
    for track in tracks {
        let selected = track.selected;
        match track.kind.clone() {
            TimelineTrackKind::SceneNode {
                node_id,
                depth,
                kind_badge,
            } => {
                let nid = node_id;
                let is_shader = kind_badge == "SHDR";
                let clip_color = if selected {
                    t.clip_active
                } else if is_shader {
                    t.clip_shader
                } else {
                    t.clip
                };
                let clip_border = if selected { t.accent } else { t.border_strong };
                let label = track.label.clone();
                let time = format!(
                    "{:.2}s – {:.2}s",
                    track.start_sec.max(0.0),
                    track.end_sec.max(0.0)
                );
                let kf_count = track.keyframes.len();
                rows.push(
                    track_row(
                        &track,
                        t,
                        depth,
                        selected,
                        move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            let _ = st.render_current_frame();
                        },
                        clip_color,
                        clip_border,
                        label,
                        time,
                        kf_count,
                        duration,
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
                let label = clipped(&format!("{property}  ·  {easing}"), 24);
                let time = format!(
                    "{:.2}s – {:.2}s",
                    track.start_sec.max(0.0),
                    track.end_sec.max(0.0)
                );
                let kf_count = track.keyframes.len();
                let clip_color = t.bg_raised;
                rows.push(
                    tree_row(
                        t,
                        "subdirectory_arrow_right",
                        t.accent,
                        label,
                        time,
                        kf_count,
                        duration,
                        track.start_sec,
                        track.end_sec,
                        clip_color,
                        t.border,
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
                let label = track.label.clone();
                let subtitle = track.subtitle.clone();
                let time = format!(
                    "{:.2}s – {:.2}s",
                    track.start_sec.max(0.0),
                    track.end_sec.max(0.0)
                );
                rows.push(
                    tree_row(
                        t,
                        "music_note",
                        t.audio,
                        format!("{label} · {subtitle}"),
                        time,
                        0,
                        duration,
                        track.start_sec,
                        track.end_sec,
                        t.audio_wash,
                        t.audio,
                        move |st: &mut StudioState| {
                            if let Some(audio) = &mut st.scene_def.audio {
                                if let Some(tr) = audio.tracks.get_mut(idx) {
                                    tr.muted = !tr.muted;
                                }
                            }
                        },
                    )
                    .into_any_flex(),
                );
            }
        }
    }
    rows.push(
        sized_box(
            flex_row((
                sized_box(label("")).width(TRACK_GUTTER.px()),
                flex_row((icon("add", 12.0, t.muted), txt("Add track", 9.5, t.muted)))
                    .gap(4.0_f64.px())
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(Padding::from_vh(0.0, 8.0)),
            ))
            .gap(0.0_f64.px()),
        )
        .height(28.0_f64.px())
        .background_color(t.bg_inset)
        .border(t.border, 1.0)
        .into_any_flex(),
    );
    flex_col(rows)
        .gap(0.0_f64.px())
        .background_color(t.bg_surface)
}

#[allow(clippy::too_many_arguments)]
fn track_row(
    track: &crate::timeline::TimelineTrack,
    t: &StudioPalette,
    depth: usize,
    selected: bool,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
    clip_color: Color,
    clip_border: Color,
    clip_label: String,
    time: String,
    kf_count: usize,
    duration: f64,
) -> impl WidgetView<StudioState> {
    let indent = 6.0 + depth as f64 * 10.0;
    let start = track.start_sec.max(0.0);
    let end = track.end_sec.max(start + 0.25);
    let total = duration.max(0.1);
    let pre = (start / total).clamp(0.01, 0.95);
    let span = ((end - start) / total).clamp(0.03, 0.95);
    let post = (1.0 - pre - span).clamp(0.01, 0.95);
    let t = *t;

    flex_row(vec![
        sized_box(
            flex_row((
                sized_box(label("")).width(indent.px()).height(1.0_f64.px()),
                icon("segment", 12.0, if selected { t.accent } else { t.muted }),
                txt(
                    clip_label.clone(),
                    10.0,
                    if selected { t.text } else { t.text_soft },
                ),
            ))
            .gap(5.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .width(TRACK_GUTTER.px())
        .height(TRACK_H.px())
        .padding(Padding::from_vh(0.0, 6.0))
        .background_color(if selected { t.bg_raised } else { t.bg_inset })
        .border(t.border, 1.0)
        .into_any_flex(),
        sized_box(
            flex_row(vec![
                sized_box(label("")).flex(pre).into_any_flex(),
                sized_box(
                    button(
                        flex_col((
                            sized_box(label("")).height(2.0_f64.px()).background_color(
                                if selected { t.accent } else { t.border_strong },
                            ),
                            flex_row((
                                txt(clip_label.clone(), 9.5, t.text),
                                FlexSpacer::Flex(1.0),
                                if kf_count > 0 {
                                    flex_row((
                                        icon("diamond", 9.0, t.accent),
                                        mono(format!("{kf_count}"), 8.0, t.accent),
                                    ))
                                    .gap(2.0_f64.px())
                                    .into_any_flex()
                                } else {
                                    sized_box(label("")).into_any_flex()
                                },
                                mono(time, 8.5, t.muted),
                            ))
                            .gap(5.0_f64.px())
                            .cross_axis_alignment(CrossAxisAlignment::Center)
                            .padding(Padding::from_vh(2.0, 5.0)),
                        ))
                        .gap(0.0_f64.px()),
                        cb,
                    )
                    .padding(Padding::all(0.0))
                    .corner_radius(6.0)
                    .background_color(clip_color)
                    .active_background_color(t.press)
                    .border(if selected { t.selection } else { clip_border }, 1.0)
                    .hovered_border_color(t.accent),
                )
                .height(26.0_f64.px())
                .flex(span)
                .into_any_flex(),
                sized_box(label("")).flex(post).into_any_flex(),
            ])
            .gap(0.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .height(TRACK_H.px())
        .padding(Padding::from_vh(0.0, 4.0))
        .background_color(t.bg_surface)
        .border(t.border, 1.0)
        .flex(1.0)
        .into_any_flex(),
    ])
    .gap(0.0_f64.px())
}

#[allow(clippy::too_many_arguments)]
fn tree_row(
    t: &StudioPalette,
    icon_name: &'static str,
    icon_color: Color,
    clip_label: String,
    time: String,
    kf_count: usize,
    duration: f64,
    start: f64,
    end: f64,
    clip_color: Color,
    clip_border: Color,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    let total = duration.max(0.1);
    let pre = (start.max(0.0) / total).clamp(0.01, 0.95);
    let span = ((end.max(start + 0.25) - start) / total).clamp(0.03, 0.95);
    let post = (1.0 - pre - span).clamp(0.01, 0.95);
    flex_row(vec![
        sized_box(
            flex_row((
                icon(icon_name, 12.0, icon_color),
                txt(clip_label, 10.0, t.text_soft),
            ))
            .gap(5.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .width(TRACK_GUTTER.px())
        .height(28.0_f64.px())
        .padding(Padding::from_vh(0.0, 6.0))
        .background_color(t.bg_inset)
        .border(t.border, 1.0)
        .into_any_flex(),
        sized_box(
            flex_row(vec![
                sized_box(label("")).flex(pre).into_any_flex(),
                sized_box(
                    button(
                        flex_row((
                            if kf_count > 0 {
                                mono(format!("◆ {kf_count}"), 8.0, icon_color).into_any_flex()
                            } else {
                                sized_box(label("")).into_any_flex()
                            },
                            FlexSpacer::Flex(1.0),
                            mono(time, 8.5, t.muted),
                        ))
                        .cross_axis_alignment(CrossAxisAlignment::Center),
                        cb,
                    )
                    .padding(Padding::from_vh(1.0, 5.0))
                    .corner_radius(5.0)
                    .background_color(clip_color)
                    .active_background_color(t.press)
                    .border(clip_border, 1.0),
                )
                .height(20.0_f64.px())
                .flex(span)
                .into_any_flex(),
                sized_box(label("")).flex(post).into_any_flex(),
            ])
            .gap(0.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
        )
        .height(28.0_f64.px())
        .padding(Padding::from_vh(0.0, 6.0))
        .background_color(t.bg_surface)
        .border(t.border, 1.0)
        .flex(1.0)
        .into_any_flex(),
    ])
    .gap(0.0_f64.px())
}

// ── Right dock ───────────────────────────────────────────────────────────────

fn right_dock(state: &mut StudioState, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let cur = state.right_tab;
    let tabs = tab_strip(
        vec![
            tab("Design", cur == RightDockTab::Design, t, |st| {
                st.right_tab = RightDockTab::Design
            })
            .into_any_flex(),
            tab("Layers", cur == RightDockTab::Layers, t, |st| {
                st.right_tab = RightDockTab::Layers
            })
            .into_any_flex(),
            tab("Agent", cur == RightDockTab::AiAgent, t, |st| {
                st.right_tab = RightDockTab::AiAgent
            })
            .into_any_flex(),
            tab("Renders", cur == RightDockTab::Renders, t, |st| {
                st.right_tab = RightDockTab::Renders
            })
            .into_any_flex(),
        ],
        t,
    );
    let content: Boxed = match state.right_tab {
        RightDockTab::Design => design_panel(state, t),
        RightDockTab::Layers => layers_panel(state, t),
        RightDockTab::AiAgent => agent_panel(state, t),
        RightDockTab::Renders => renders_panel(state, t),
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
    .corner_radius(PANEL_RADIUS)
}

fn design_panel(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let Some(snap) = state.inspect_selected_node() else {
        return empty_state(
            t,
            "select",
            "Nothing selected",
            "Select a layer or a node in the timeline to edit its properties.",
        );
    };
    let nid = snap.node_id;
    let open = state.open_inspector_group.clone();
    let show_all = open == "all";
    let mut kids: Vec<Child> = Vec::new();

    kids.push(
        flex_row((
            badge(snap.kind_badge, t.text, t.bg_raised),
            txtw(
                clipped(&snap.label, 18),
                12.0,
                FontWeight::new(600.0),
                t.text,
            ),
            mono(format!("#{nid}"), 9.0, t.muted),
            FlexSpacer::Flex(1.0),
            icon_btn("visibility", t.text_soft, t, move |st: &mut StudioState| {
                let _ = st.layer_toggle_visibility(nid);
            }),
            icon_btn("content_copy", t.text_soft, t, |st: &mut StudioState| {
                let _ = st.generate_selected_element_prompt("");
                st.right_tab = RightDockTab::AiAgent;
            }),
        ))
        .gap(6.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(8.0, 8.0))
        .background_color(t.bg_raised)
        .border(t.border, 1.0)
        .into_any_flex(),
    );

    let layout_open = show_all || open == "layout";
    kids.push(
        section_header(
            "Layout & Transform",
            format!(
                "{:.0}×{:.0}",
                snap.computed_rect.width, snap.computed_rect.height
            ),
            layout_open,
            t,
            |st: &mut StudioState| {
                st.open_inspector_group = if st.open_inspector_group == "layout" {
                    String::new()
                } else {
                    "layout".to_string()
                };
            },
        )
        .into_any_flex(),
    );
    if layout_open {
        kids.push(
            flex_col(vec![
                flex_row(vec![
                    num_cell("X", format!("{:.0}", snap.computed_rect.x), t)
                        .flex(1.0)
                        .into_any_flex(),
                    num_cell("Y", format!("{:.0}", snap.computed_rect.y), t)
                        .flex(1.0)
                        .into_any_flex(),
                    num_cell("W", format!("{:.0}", snap.computed_rect.width), t)
                        .flex(1.0)
                        .into_any_flex(),
                    num_cell("H", format!("{:.0}", snap.computed_rect.height), t)
                        .flex(1.0)
                        .into_any_flex(),
                ])
                .gap(3.0_f64.px())
                .into_any_flex(),
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
                    icon_btn("align_vertical_center", t.text_soft, t, |st| {
                        let _ = st.inspector_align_selected(None, Some("middle"));
                    })
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(2.0_f64.px())
                .into_any_flex(),
                flex_row(vec![
                    icon_btn("remove", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_position(-10.0, 0.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("add", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_position(10.0, 0.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("remove", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_position(0.0, -10.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("add", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_position(0.0, 10.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(2.0_f64.px())
                .into_any_flex(),
                flex_row(vec![
                    icon_btn("remove", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_size(-16.0, 0.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("add", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_size(16.0, 0.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("remove", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_size(0.0, -16.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                    icon_btn("add", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_size(0.0, 16.0);
                    })
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(2.0_f64.px())
                .into_any_flex(),
                flex_row((
                    icon_btn("rotate_left", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_rotation(-15.0);
                    }),
                    icon_btn("rotate_right", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_rotation(15.0);
                    }),
                    icon_btn("zoom_out", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_scale(-0.1);
                    }),
                    icon_btn("zoom_in", t.text_soft, t, |st| {
                        let _ = st.inspector_nudge_scale(0.1);
                    }),
                    icon_btn("restart_alt", t.text_soft, t, |st| {
                        let _ = st.inspector_reset_transform();
                    }),
                ))
                .gap(2.0_f64.px())
                .into_any_flex(),
            ])
            .gap(4.0_f64.px())
            .padding(Padding::all(6.0))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }

    let style_open = show_all || open == "style";
    let cur_color = snap
        .fill
        .clone()
        .or_else(|| snap.text_color.clone())
        .unwrap_or_else(|| "#3de4a8".to_string());
    kids.push(
        section_header(
            "Style",
            format!("{} · {:.0}%", cur_color, snap.opacity * 100.0),
            style_open,
            t,
            |st: &mut StudioState| {
                st.open_inspector_group = if st.open_inspector_group == "style" {
                    String::new()
                } else {
                    "style".to_string()
                };
            },
        )
        .into_any_flex(),
    );
    if style_open {
        let opacity = f64::from(snap.opacity);
        kids.push(
            flex_col(vec![
                flex_row((
                    row_label("Opacity", t),
                    FlexSpacer::Flex(1.0),
                    mono(format!("{:.0}%", snap.opacity * 100.0), 10.0, t.text_soft),
                ))
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
                    swatch(t.accent, "#3de4a8", t),
                    swatch(t.selection, "#4ba3d2", t),
                    swatch(t.danger, "#ff5c52", t),
                    swatch(t.warning, "#ffb900", t),
                    swatch(t.media, "#00e3ff", t),
                    swatch(t.container, "#f5a623", t),
                ))
                .gap(4.0_f64.px())
                .into_any_flex(),
                flex_row((
                    icon_text_btn("remove", "Radius", t, |st| {
                        let _ = st.inspector_nudge_corner_radius(-4.0);
                    }),
                    icon_text_btn("add", "Radius", t, |st| {
                        let _ = st.inspector_nudge_corner_radius(4.0);
                    }),
                ))
                .gap(2.0_f64.px())
                .into_any_flex(),
            ])
            .gap(4.0_f64.px())
            .padding(Padding::all(6.0))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }

    if let Some(text_content) = snap.text_content.clone() {
        let text_open = show_all || open == "text";
        kids.push(
            section_header(
                "Typography",
                format!(
                    "\"{text_content}\"  ·  {:.0}px",
                    snap.font_size.unwrap_or(16.0)
                ),
                text_open,
                t,
                |st: &mut StudioState| {
                    st.open_inspector_group = if st.open_inspector_group == "text" {
                        String::new()
                    } else {
                        "text".to_string()
                    };
                },
            )
            .into_any_flex(),
        );
        if text_open {
            kids.push(
                flex_col(vec![
                    field(text_content, t, |st: &mut StudioState, next: String| {
                        let _ = st.inspector_set_text_content(&next);
                    })
                    .into_any_flex(),
                    flex_row((
                        icon_text_btn("text_decrease", "Size −", t, |st| {
                            let _ = st.inspector_nudge_font_size(-2.0);
                        }),
                        icon_text_btn("text_increase", "Size +", t, |st| {
                            let _ = st.inspector_nudge_font_size(2.0);
                        }),
                    ))
                    .gap(2.0_f64.px())
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
                    .gap(2.0_f64.px())
                    .into_any_flex(),
                ])
                .gap(4.0_f64.px())
                .padding(Padding::all(6.0))
                .background_color(t.bg_inset)
                .border(t.border, 1.0)
                .into_any_flex(),
            );
        }
    }

    let motion_open = show_all || open == "motion";
    kids.push(
        section_header(
            "Motion",
            format!("{} tracks", snap.animations.len()),
            motion_open,
            t,
            |st: &mut StudioState| {
                st.open_inspector_group = if st.open_inspector_group == "motion" {
                    String::new()
                } else {
                    "motion".to_string()
                };
            },
        )
        .into_any_flex(),
    );
    if motion_open {
        kids.push(
            flex_col(vec![
                flex_row((
                    chip_btn("translateX", t),
                    chip_btn("translateY", t),
                    chip_btn("opacity", t),
                    chip_btn("scale", t),
                ))
                .gap(2.0_f64.px())
                .into_any_flex(),
                flex_row((
                    icon_text_btn("linear_scale", "linear", t, |st| {
                        let p = st.active_anim_property.clone();
                        let _ = st.inspector_set_easing(&p, "linear");
                    }),
                    icon_text_btn("speed", "easeOut", t, |st| {
                        let p = st.active_anim_property.clone();
                        let _ = st.inspector_set_easing(&p, "easeOut");
                    }),
                    icon_text_btn("bolt", "spring", t, |st| {
                        let p = st.active_anim_property.clone();
                        let _ = st.inspector_set_easing(&p, "spring(180, 18)");
                    }),
                ))
                .gap(2.0_f64.px())
                .into_any_flex(),
            ])
            .gap(4.0_f64.px())
            .padding(Padding::all(6.0))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }

    let fx_open = show_all || open == "fx" || !snap.shader_uniforms.is_empty();
    kids.push(
        section_header(
            "Shaders & Grade",
            format!("{} fx", snap.shader_uniforms.len()),
            fx_open,
            t,
            |st: &mut StudioState| {
                st.open_inspector_group = if st.open_inspector_group == "fx" {
                    String::new()
                } else {
                    "fx".to_string()
                };
            },
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
                    txt(name.clone(), 10.0, t.muted),
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
            ))
            .gap(2.0_f64.px())
            .into_any_flex(),
        );
        kids.push(
            flex_col(fx)
                .gap(3.0_f64.px())
                .padding(Padding::all(6.0))
                .background_color(t.bg_inset)
                .border(t.border, 1.0)
                .into_any_flex(),
        );
    }

    kids.push(
        flex_row((
            primary_btn("smart_toy", "Ask", t, move |st: &mut StudioState| {
                let _ = st.generate_selected_element_prompt("");
                st.right_tab = RightDockTab::AiAgent;
            }),
            FlexSpacer::Flex(1.0),
            toggle("Auto-KF", state.auto_keyframe, t, |st| {
                st.auto_keyframe = !st.auto_keyframe;
            }),
        ))
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::all(6.0))
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

fn layers_panel(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let mut rows: Vec<Child> = Vec::new();
    for row in state.build_layer_tree_rows() {
        let nid = row.node_id;
        let indent = 8.0 + row.depth as f64 * 10.0;
        rows.push(
            flex_row((
                sized_box(label("")).width(indent.px()).height(1.0_f64.px()),
                icon(
                    if row.visible {
                        "visibility"
                    } else {
                        "visibility_off"
                    },
                    13.0,
                    if row.visible { t.muted } else { t.danger },
                ),
                badge(row.badge, t.text, t.bg_raised),
                button(
                    txt(
                        row.label.clone(),
                        10.5,
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
                .hovered_border_color(t.border_strong)
                .flex(1.0),
                icon_btn("arrow_upward", t.text_soft, t, move |st| {
                    let _ = st.layer_move_z_order(nid, -1);
                }),
                icon_btn("arrow_downward", t.text_soft, t, move |st| {
                    let _ = st.layer_move_z_order(nid, 1);
                }),
            ))
            .gap(3.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .padding(Padding::from_vh(4.0, 4.0))
            .background_color(if row.selected {
                t.bg_raised
            } else {
                transparent()
            })
            .border(t.border, 1.0)
            .into_any_flex(),
        );
    }
    flex_col(rows)
        .gap(1.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(4.0))
        .boxed()
}

fn agent_panel(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let draft = state.chat.input_draft.clone();
    let model = state.chat.provider_model.clone();
    let mut kids: Vec<Child> = Vec::new();
    kids.push(
        flex_row((
            icon("smart_toy", 14.0, t.accent),
            txtw("WEBMCP AGENT", 9.0, FontWeight::new(600.0), t.accent),
            FlexSpacer::Flex(1.0),
            mono(model, 8.5, t.muted),
        ))
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(Padding::from_vh(5.0, 6.0))
        .background_color(t.bg_inset)
        .border(t.border, 1.0)
        .into_any_flex(),
    );
    kids.push(
        field(draft, t, |st: &mut StudioState, next: String| {
            st.chat.input_draft = next;
        })
        .into_any_flex(),
    );
    kids.push(
        flex_row((
            primary_btn("send", "Send", t, |st: &mut StudioState| {
                let _ = st.submit_current_chat_draft();
            }),
            icon_text_btn("content_copy", "Prompt", t, |st| {
                let _ = st.generate_selected_element_prompt("");
            }),
            icon_text_btn("search", "/look", t, |st| {
                let _ = st.submit_chat_prompt("/look");
            }),
        ))
        .gap(3.0_f64.px())
        .into_any_flex(),
    );
    if let Some(prompt) = &state.last_copied_agent_prompt {
        kids.push(
            sized_box(txt(prompt.clone(), 9.5, t.muted))
                .padding(Padding::all(6.0))
                .background_color(t.bg_inset)
                .border(t.border, 1.0)
                .corner_radius(5.0)
                .into_any_flex(),
        );
    }
    for msg in &state.chat.messages {
        kids.push(
            flex_col((
                txtw(
                    format!("[{}]", msg.role.badge()),
                    8.5,
                    FontWeight::new(600.0),
                    t.accent,
                ),
                txt(msg.content.clone(), 10.0, t.text_soft),
            ))
            .gap(3.0_f64.px())
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
        .padding(Padding::all(7.0))
        .boxed()
}

fn renders_panel(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let out = state.export_output_path.clone();
    let fmt = state.export_format.to_uppercase();
    let mut kids: Vec<Child> = Vec::new();
    kids.push(
        flex_row((
            txtw("RENDER QUEUE", 9.0, FontWeight::new(600.0), t.muted),
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
        ))
        .gap(2.0_f64.px())
        .into_any_flex(),
    );
    kids.push(
        field(out, t, |st: &mut StudioState, next: String| {
            st.export_output_path = next;
        })
        .into_any_flex(),
    );
    kids.push(
        primary_btn("play_arrow", "Render", t, |st: &mut StudioState| {
            let _ = st.run_export_job();
        })
        .into_any_flex(),
    );
    for job in state.render_queue.iter().rev().take(8) {
        let status = match &job.status {
            RenderJobStatus::Completed => "done".to_string(),
            RenderJobStatus::Failed(err) => format!("failed: {err}"),
        };
        kids.push(
            flex_col((
                flex_row((
                    mono(format!("#{}", job.id), 9.0, t.muted),
                    txt(job.composition_title.clone(), 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    mono(job.format.to_uppercase(), 8.5, t.accent_ink),
                ))
                .gap(4.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                txt(
                    format!(
                        "{} frames · {} · {} bytes",
                        job.frames, status, job.bytes_written
                    ),
                    9.0,
                    t.muted,
                ),
                mono(job.output_path.display().to_string(), 8.5, t.off),
            ))
            .gap(1.0_f64.px())
            .padding(Padding::all(5.0))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .corner_radius(5.0)
            .into_any_flex(),
        );
    }
    flex_col(kids)
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill)
        .padding(Padding::all(7.0))
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
        txtw(title, 12.0, FontWeight::new(500.0), t.text_soft).into_any_flex(),
        txt(help, 10.0, t.muted).into_any_flex(),
    ])
    .gap(4.0_f64.px())
    .padding(Padding::all(20.0))
    .boxed()
}

fn section_header(
    title: &'static str,
    summary: impl Into<ArcStr>,
    open: bool,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((
            icon(
                if open { "expand_more" } else { "chevron_right" },
                14.0,
                t.muted,
            ),
            txtw(title, 10.5, FontWeight::new(600.0), t.text_soft),
            FlexSpacer::Flex(1.0),
            mono(summary, 8.5, t.muted),
        ))
        .gap(4.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(5.0, 8.0))
    .background_color(t.bg_surface)
    .active_background_color(t.press)
    .border(t.border, 1.0)
    .hovered_border_color(t.border_strong)
}

fn chip_btn(value: &'static str, t: &StudioPalette) -> impl WidgetView<StudioState> {
    let t = *t;
    button(txt(value, 9.0, t.muted), move |st: &mut StudioState| {
        st.active_anim_property = value.to_string();
    })
    .padding(Padding::from_vh(2.0, 5.0))
    .corner_radius(4.0)
    .background_color(t.bg_inset)
    .active_background_color(t.press)
    .border(t.border, 1.0)
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
    .padding(Padding::all(1.0))
    .corner_radius(4.0)
    .background_color(t.bg_raised)
    .active_background_color(t.press)
    .border(t.border, 1.0)
    .hovered_border_color(t.text)
}
