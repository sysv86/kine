//! Native Rust **Xilem 0.4.0** desktop GUI for Kine Studio (Linux, Windows, macOS).
//!
//! Layout mirrors **Hyperframes Studio** (`EditorShell.tsx`):
//! - **Top Header Bar**: Project/composition badge, status banner, Capture PNG/SVG,
//!   Backend switcher (`AUTO`/`CPU`/`GPU`), Ask AI Agent modal toggle, Inspector
//!   toggle, and Render trigger.
//! - **Left Dock**: `Compositions`, `Assets`, `Code (TS/JSON)`, and `Catalog & FX`.
//! - **Center Workspace**:
//!   - **Viewport Preview Pane**: Zero-copy `ImageData` canvas rendered in-process
//!     by `kine_server::Engine`, bounding-box & safe-guide overlays, zoom controls.
//!   - **Player Controls (Transport Bar)**: Play/Pause, step `|<` `<` `>` `>|`,
//!     time/frame toggle, seek slider, speed `0.5x`–`2.0x`, loop & mute toggles.
//!   - **Bottom NLE Timeline & Nodes**: Timeline toolbar (`Undo`, `Redo`, `Snap`,
//!     `Auto-KF`, `+ Keyframe (K)`, `Split @ Playhead (S)`, `Duplicate`, `Delete`,
//!     zoom slider), adaptive time ruler, and hierarchical node/property/audio lanes
//!     with keyframe diamonds.
//! - **Right Dock**: `Design` (Property Inspector), `Layers` (Z-order tree),
//!   `AI Agent` (Interactive chat + `studio_*` tool loop + Prompt v1 builder),
//!   and `Renders` (Multi-format export queue).

use crate::catalog::CATALOG_ITEMS;
use crate::state::{LeftDockTab, RenderJobStatus, RightDockTab, StudioState};
use crate::timeline::{generate_ruler_ticks, TimelineTrackKind};
use kine_core::NodeId;
use kine_render::FrameFormat;
use std::path::Path;

#[cfg(feature = "xilem-ui")]
use xilem::{
    masonry::{
        parley::style::FontWeight,
        peniko::{ImageBrush, ObjectFit},
    },
    style::Style as _,
    view::{
        checkbox, flex_col, flex_row, image, label, portal, prose, sized_box, slider, split,
        text_button, text_input, Axis, CrossAxisAlignment, FlexExt as _, FlexSpacer,
        MainAxisAlignment,
    },
    winit::error::EventLoopError,
    Color, EventLoop, WidgetView, WindowOptions, Xilem,
};

#[cfg(feature = "xilem-ui")]
const BG_ROOT: Color = Color::from_rgb8(11, 15, 25);
#[cfg(feature = "xilem-ui")]
const BG_PANEL: Color = Color::from_rgb8(17, 24, 39);
#[cfg(feature = "xilem-ui")]
const BG_CARD: Color = Color::from_rgb8(30, 41, 59);
#[cfg(feature = "xilem-ui")]
const BG_CANVAS: Color = Color::from_rgb8(7, 10, 18);
#[cfg(feature = "xilem-ui")]
const BORDER_SUBTLE: Color = Color::from_rgb8(51, 65, 85);
#[cfg(feature = "xilem-ui")]
const TEXT_PRIMARY: Color = Color::from_rgb8(248, 250, 252);
#[cfg(feature = "xilem-ui")]
const TEXT_MUTED: Color = Color::from_rgb8(148, 163, 184);
#[cfg(feature = "xilem-ui")]
const ACCENT_CYAN: Color = Color::from_rgb8(56, 189, 248);
#[cfg(feature = "xilem-ui")]
const ACCENT_ROSE: Color = Color::from_rgb8(244, 63, 94);
#[cfg(feature = "xilem-ui")]
const ACCENT_EMERALD: Color = Color::from_rgb8(16, 185, 129);

/// Launch the native Xilem desktop window for Kine Studio.
#[cfg(feature = "xilem-ui")]
pub fn run_studio(state: StudioState) -> Result<(), EventLoopError> {
    let app = Xilem::new_simple(
        state,
        app_logic,
        WindowOptions::new("Kine Studio — Video-as-Code NLE & AI Agent Studio (Xilem)"),
    );
    app.run_in(EventLoop::with_user_event())
}

/// Top-level reactive Xilem view builder for `StudioState`.
#[cfg(feature = "xilem-ui")]
pub fn app_logic(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let header = header_bar_view(state);
    let left_dock = left_dock_view(state);
    let center_workspace = center_workspace_view(state);
    let right_dock = right_dock_view(state);

    let main_body = split(
        left_dock,
        split(center_workspace, right_dock)
            .split_axis(Axis::Horizontal)
            .split_point(if state.inspector_visible { 0.72 } else { 0.98 }),
    )
    .split_axis(Axis::Horizontal)
    .split_point(0.22);

    flex_col((header, main_body.flex(1.0))).background_color(BG_ROOT)
}

#[cfg(feature = "xilem-ui")]
fn header_bar_view(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let comp_name = state
        .compositions
        .get(state.active_composition)
        .map(|c| c.title.clone())
        .unwrap_or_else(|| "Untitled Composition".to_string());
    let backend_label = format!("Backend: {}", state.engine.backend().name().to_uppercase());
    let res_label = format!(
        "{}x{} @ {:.0}fps",
        state.scene_def.width, state.scene_def.height, state.fps
    );
    let findings_label = if state.inspect_findings.is_empty() {
        "Inspect: 0 issues".to_string()
    } else {
        format!("Inspect: {} finding(s)", state.inspect_findings.len())
    };
    let banner = state.status_banner.clone();

    flex_row((
        label("KINE STUDIO")
            .weight(FontWeight::BOLD)
            .text_size(14.0)
            .color(ACCENT_CYAN),
        label(comp_name).weight(FontWeight::SEMI_BOLD).text_size(13.0).color(TEXT_PRIMARY),
        label(res_label).text_size(12.0).color(TEXT_MUTED),
        label(findings_label).text_size(12.0).color(ACCENT_EMERALD),
        FlexSpacer::Flex(1.0),
        label(banner).text_size(11.0).color(TEXT_MUTED),
        FlexSpacer::Flex(1.0),
        text_button(backend_label, |st: &mut StudioState| {
            st.cycle_render_backend();
        }),
        text_button("Capture PNG", |st: &mut StudioState| {
            let _ = st.capture_current_frame(FrameFormat::Png, Path::new("studio-capture.png"));
        }),
        text_button("Capture SVG", |st: &mut StudioState| {
            let _ = st.capture_current_frame(FrameFormat::Svg, Path::new("studio-capture.svg"));
        }),
        text_button("Ask AI Agent", |st: &mut StudioState| {
            st.right_tab = RightDockTab::AiAgent;
            st.inspector_visible = true;
            let _ = st.generate_selected_element_prompt("");
        }),
        text_button(
            if state.inspector_visible {
                "Hide Dock"
            } else {
                "Show Dock"
            },
            |st: &mut StudioState| {
                st.inspector_visible = !st.inspector_visible;
            },
        ),
        text_button("Render Export", |st: &mut StudioState| {
            st.right_tab = RightDockTab::Renders;
            st.inspector_visible = true;
            let _ = st.run_export_job();
        }),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(8.0)
    .background_color(BG_PANEL)
    .border(BORDER_SUBTLE, 1.0)
}

#[cfg(feature = "xilem-ui")]
fn left_dock_view(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let tab_bar = flex_row((
        text_button(LeftDockTab::Compositions.label(), |st: &mut StudioState| {
            st.left_tab = LeftDockTab::Compositions;
        }),
        text_button(LeftDockTab::Assets.label(), |st: &mut StudioState| {
            st.left_tab = LeftDockTab::Assets;
        }),
        text_button(LeftDockTab::Code.label(), |st: &mut StudioState| {
            st.left_tab = LeftDockTab::Code;
        }),
        text_button(LeftDockTab::Catalog.label(), |st: &mut StudioState| {
            st.left_tab = LeftDockTab::Catalog;
        }),
    ))
    .padding(6.0)
    .background_color(BG_CARD);

    let mut items: Vec<_> = Vec::new();
    match state.left_tab {
        LeftDockTab::Compositions => {
            for (idx, comp) in state.compositions.iter().enumerate() {
                let is_active = idx == state.active_composition;
                let title = if is_active {
                    format!("● {}", comp.title)
                } else {
                    format!("○ {}", comp.title)
                };
                let sub = comp.subtitle.clone();
                items.push(
                    flex_col((
                        text_button(title, move |st: &mut StudioState| {
                            let _ = st.select_composition(idx);
                        }),
                        label(sub).text_size(11.0).color(TEXT_MUTED),
                    ))
                    .padding(6.0)
                    .background_color(if is_active { BG_CARD } else { BG_PANEL })
                    .border(BORDER_SUBTLE, 1.0)
                    .corner_radius(6.0),
                );
            }
        }
        LeftDockTab::Assets => {
            for asset in &state.assets {
                let header = format!("[{}] {}", asset.kind.badge(), asset.name);
                let uri = asset.uri.clone();
                let details = format!("{} · {}", asset.uri, asset.details);
                items.push(
                    flex_col((
                        text_button(header, move |st: &mut StudioState| {
                            st.status_banner = format!("Asset URI ready: {uri}");
                        }),
                        label(details).text_size(11.0).color(TEXT_MUTED),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0)
                    .corner_radius(6.0),
                );
            }
        }
        LeftDockTab::Code => {
            let src_buf = state.source_buffer.clone();
            items.push(
                flex_col((
                    flex_row((
                        label("Scene Definition (JSON / TS)")
                            .weight(FontWeight::BOLD)
                            .text_size(12.0)
                            .color(TEXT_PRIMARY),
                        FlexSpacer::Flex(1.0),
                        text_button("Apply to Engine", |st: &mut StudioState| {
                            if let Err(e) = st.apply_source_buffer() {
                                st.status_banner = e.to_string();
                            }
                        }),
                    )),
                    text_input(src_buf.clone(), |st: &mut StudioState, next: String| {
                        st.source_buffer = next;
                    }),
                    prose(src_buf),
                ))
                .padding(6.0)
                .background_color(BG_PANEL)
                .border(BORDER_SUBTLE, 1.0)
                .corner_radius(6.0),
            );
        }
        LeftDockTab::Catalog => {
            for item in CATALOG_ITEMS {
                let item_id = item.id;
                let btn_label = format!("+ [{}] {}", item.category, item.label);
                let desc = item.description.to_string();
                items.push(
                    flex_col((
                        text_button(btn_label, move |st: &mut StudioState| {
                            let _ = st.insert_catalog_item(item_id);
                        }),
                        label(desc).text_size(11.0).color(TEXT_MUTED),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0)
                    .corner_radius(6.0),
                );
            }
        }
    }

    flex_col((tab_bar, portal(flex_col(items).padding(8.0)).flex(1.0)))
        .background_color(BG_PANEL)
        .border(BORDER_SUBTLE, 1.0)
}

#[cfg(feature = "xilem-ui")]
fn center_workspace_view(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let preview = preview_pane_view(state);
    let timeline = timeline_pane_view(state);

    split(preview, timeline)
        .split_axis(Axis::Vertical)
        .split_point(0.56)
}

#[cfg(feature = "xilem-ui")]
fn preview_pane_view(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let viewport_toolbar = flex_row((
        label(format!(
            "Viewport · Zoom {:.0}% · Rev #{}",
            state.viewport_zoom * 100.0,
            state.frame_revision
        ))
        .text_size(12.0)
        .color(TEXT_MUTED),
        FlexSpacer::Flex(1.0),
        checkbox(
            "Bounds Overlay",
            state.show_bounding_boxes,
            |st: &mut StudioState, v: bool| {
                st.show_bounding_boxes = v;
                let _ = st.render_current_frame();
            },
        ),
        checkbox(
            "Safe Guides",
            state.show_safe_guides,
            |st: &mut StudioState, v: bool| {
                st.show_safe_guides = v;
                let _ = st.render_current_frame();
            },
        ),
        text_button("Zoom -", |st: &mut StudioState| {
            st.viewport_zoom = (st.viewport_zoom - 0.25).clamp(0.25, 4.0);
        }),
        text_button("100%", |st: &mut StudioState| {
            st.viewport_zoom = 1.0;
        }),
        text_button("Zoom +", |st: &mut StudioState| {
            st.viewport_zoom = (st.viewport_zoom + 0.25).clamp(0.25, 4.0);
        }),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(6.0)
    .background_color(BG_PANEL);

    let brush = ImageBrush::new(state.frame_image_data.clone());
    let canvas_box = sized_box(image(brush).fit(ObjectFit::Contain))
        .expand()
        .padding(12.0)
        .background_color(BG_CANVAS);

    let transport = transport_controls_view(state);

    flex_col((viewport_toolbar, canvas_box.flex(1.0), transport)).background_color(BG_CANVAS)
}

#[cfg(feature = "xilem-ui")]
fn transport_controls_view(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let play_label = if state.is_playing { "⏸ Pause" } else { "▶ Play" };
    let timecode = state.formatted_timecode();
    let dur = state.duration.max(0.1);
    let cur = state.current_time.clamp(0.0, dur);
    let speed_label = format!("{:.2}x", state.playback_rate);

    flex_row((
        text_button("|<", |st: &mut StudioState| {
            st.is_playing = false;
            st.seek_to(0.0, false);
        }),
        text_button("< -1f", |st: &mut StudioState| {
            st.step_frames(-1);
        }),
        text_button(play_label, |st: &mut StudioState| {
            st.is_playing = !st.is_playing;
            if st.is_playing {
                st.tick_playback(1.0 / st.fps.max(1.0));
            }
        }),
        text_button("+1f >", |st: &mut StudioState| {
            st.step_frames(1);
        }),
        text_button(">|", |st: &mut StudioState| {
            st.is_playing = false;
            let end = st.duration;
            st.seek_to(end, false);
        }),
        text_button(timecode, |st: &mut StudioState| {
            st.show_frames = !st.show_frames;
        }),
        slider(0.0, dur, cur, |st: &mut StudioState, val: f64| {
            st.seek_to(val, true);
        })
        .flex(1.0),
        text_button(speed_label, |st: &mut StudioState| {
            st.playback_rate = match st.playback_rate {
                r if (r - 0.5).abs() < 0.01 => 1.0,
                r if (r - 1.0).abs() < 0.01 => 1.5,
                r if (r - 1.5).abs() < 0.01 => 2.0,
                _ => 0.5,
            };
        }),
        checkbox("Loop", state.loop_enabled, |st: &mut StudioState, v: bool| {
            st.loop_enabled = v;
        }),
        checkbox("Mute", state.audio_muted, |st: &mut StudioState, v: bool| {
            st.audio_muted = v;
        }),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(6.0)
    .background_color(BG_PANEL)
    .border(BORDER_SUBTLE, 1.0)
}

#[cfg(feature = "xilem-ui")]
fn timeline_pane_view(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let undo_label = format!("↶ Undo ({})", state.undo_stack.len());
    let redo_label = format!("↷ Redo ({})", state.redo_stack.len());

    let toolbar = flex_row((
        text_button(undo_label, |st: &mut StudioState| {
            let _ = st.undo();
        }),
        text_button(redo_label, |st: &mut StudioState| {
            let _ = st.redo();
        }),
        text_button("◀ Prev KF", |st: &mut StudioState| {
            st.timeline_jump_prev_keyframe();
        }),
        text_button("◆ +Keyframe (K)", |st: &mut StudioState| {
            let _ = st.timeline_add_keyframe_at_playhead();
        }),
        text_button("Next KF ▶", |st: &mut StudioState| {
            st.timeline_jump_next_keyframe();
        }),
        text_button("✂ Split @ Playhead", |st: &mut StudioState| {
            let _ = st.timeline_split_at_playhead();
        }),
        text_button("⧉ Duplicate", |st: &mut StudioState| {
            let _ = st.timeline_duplicate_selected();
        }),
        text_button("⌫ Delete", |st: &mut StudioState| {
            let _ = st.timeline_delete_selected();
        }),
        checkbox(
            "Snap (N)",
            state.timeline_snap_enabled,
            |st: &mut StudioState, v: bool| {
                st.timeline_snap_enabled = v;
            },
        ),
        checkbox(
            "Auto-KF",
            state.auto_keyframe,
            |st: &mut StudioState, v: bool| {
                st.auto_keyframe = v;
            },
        ),
        checkbox(
            "Ripple",
            state.ripple_edit,
            |st: &mut StudioState, v: bool| {
                st.ripple_edit = v;
            },
        ),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(6.0)
    .background_color(BG_CARD);

    // Time Ruler bar
    let ticks = generate_ruler_ticks(state.duration, state.timeline_zoom_pps);
    let mut tick_buttons: Vec<_> = Vec::new();
    for tick in ticks.into_iter().filter(|t| t.is_major) {
        let t_sec = tick.time_sec;
        let lbl = tick.label.unwrap_or_else(|| format!("{t_sec:.1}s"));
        tick_buttons.push(text_button(lbl, move |st: &mut StudioState| {
            st.seek_to(t_sec, false);
        }));
    }
    let ruler_row = flex_row(tick_buttons)
        .padding(4.0)
        .background_color(BG_PANEL)
        .border(BORDER_SUBTLE, 1.0);

    // Track & Node lanes
    let tracks = state.build_timeline_tracks();
    let mut track_rows: Vec<_> = Vec::new();
    for track in tracks {
        let is_selected = track.selected;
        let row_bg = if is_selected { BG_CARD } else { BG_PANEL };
        let kf_summary: String = if track.keyframes.is_empty() {
            "—".to_string()
        } else {
            track
                .keyframes
                .iter()
                .map(|k| format!("◆{:.2}s={:.1}", k.time, k.value))
                .collect::<Vec<_>>()
                .join("  ")
        };

        match track.kind {
            TimelineTrackKind::SceneNode { node_id, .. } => {
                let nid = node_id;
                track_rows.push(
                    flex_row((
                        text_button(track.label.clone(), move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            let _ = st.render_current_frame();
                        }),
                        label(track.subtitle).text_size(11.0).color(TEXT_MUTED),
                        FlexSpacer::Flex(1.0),
                        label(kf_summary).text_size(11.0).color(ACCENT_CYAN),
                        text_button("-0.1s", move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            let _ = st.timeline_shift_selected(-0.1);
                        }),
                        text_button("+0.1s", move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            let _ = st.timeline_shift_selected(0.1);
                        }),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(5.0)
                    .background_color(row_bg)
                    .border(BORDER_SUBTLE, 1.0),
                );
            }
            TimelineTrackKind::AnimationProperty {
                node_id, property, ..
            } => {
                let nid = node_id;
                let prop = property.clone();
                track_rows.push(
                    flex_row((
                        text_button(track.label.clone(), move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            st.active_anim_property = prop.clone();
                            let _ = st.render_current_frame();
                        }),
                        label(track.subtitle).text_size(11.0).color(TEXT_MUTED),
                        FlexSpacer::Flex(1.0),
                        label(kf_summary).text_size(11.0).color(ACCENT_EMERALD),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(4.0)
                    .background_color(BG_ROOT)
                    .border(BORDER_SUBTLE, 1.0),
                );
            }
            TimelineTrackKind::AudioTrack { track_index } => {
                track_rows.push(
                    flex_row((
                        text_button(track.label.clone(), move |st: &mut StudioState| {
                            if let Some(audio) = &mut st.scene_def.audio {
                                if let Some(tr) = audio.tracks.get_mut(track_index) {
                                    tr.muted = !tr.muted;
                                }
                            }
                        }),
                        label(track.subtitle).text_size(11.0).color(TEXT_MUTED),
                        FlexSpacer::Flex(1.0),
                        label("DSP Audio Bed").text_size(11.0).color(ACCENT_ROSE),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(5.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0),
                );
            }
        }
    }

    flex_col((
        toolbar,
        ruler_row,
        portal(flex_col(track_rows).padding(6.0)).flex(1.0),
    ))
    .background_color(BG_PANEL)
    .border(BORDER_SUBTLE, 1.0)
}

#[cfg(feature = "xilem-ui")]
fn right_dock_view(state: &mut StudioState) -> impl WidgetView<StudioState> + use<> {
    let tab_bar = flex_row((
        text_button(RightDockTab::Design.label(), |st: &mut StudioState| {
            st.right_tab = RightDockTab::Design;
        }),
        text_button(RightDockTab::Layers.label(), |st: &mut StudioState| {
            st.right_tab = RightDockTab::Layers;
        }),
        text_button(RightDockTab::AiAgent.label(), |st: &mut StudioState| {
            st.right_tab = RightDockTab::AiAgent;
        }),
        text_button(RightDockTab::Renders.label(), |st: &mut StudioState| {
            st.right_tab = RightDockTab::Renders;
        }),
    ))
    .padding(6.0)
    .background_color(BG_CARD);

    let mut sections: Vec<_> = Vec::new();
    match state.right_tab {
        RightDockTab::Design => {
            if let Some(snap) = state.inspect_selected_node() {
                sections.push(
                    flex_col((
                        label(format!("Selected: {}", snap.label))
                            .weight(FontWeight::BOLD)
                            .text_size(13.0)
                            .color(ACCENT_CYAN),
                        label(format!(
                            "Computed Box: X={:.1} Y={:.1} W={:.1} H={:.1}",
                            snap.computed_rect.x,
                            snap.computed_rect.y,
                            snap.computed_rect.width,
                            snap.computed_rect.height
                        ))
                        .text_size(11.0)
                        .color(TEXT_MUTED),
                        flex_row((
                            text_button("Align Left", |st: &mut StudioState| {
                                let _ = st.inspector_align_selected(Some("left"), None);
                            }),
                            text_button("Center H", |st: &mut StudioState| {
                                let _ = st.inspector_align_selected(Some("center"), None);
                            }),
                            text_button("Center V", |st: &mut StudioState| {
                                let _ = st.inspector_align_selected(None, Some("middle"));
                            }),
                            text_button("Align Right", |st: &mut StudioState| {
                                let _ = st.inspector_align_selected(Some("right"), None);
                            }),
                        )),
                        flex_row((
                            text_button("X -10", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_position(-10.0, 0.0);
                            }),
                            text_button("X +10", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_position(10.0, 0.0);
                            }),
                            text_button("Y -10", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_position(0.0, -10.0);
                            }),
                            text_button("Y +10", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_position(0.0, 10.0);
                            }),
                        )),
                        flex_row((
                            text_button("W -16", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_size(-16.0, 0.0);
                            }),
                            text_button("W +16", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_size(16.0, 0.0);
                            }),
                            text_button("H -16", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_size(0.0, -16.0);
                            }),
                            text_button("H +16", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_size(0.0, 16.0);
                            }),
                        )),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0),
                );

                // Transform & Opacity section
                let op = f64::from(snap.opacity);
                sections.push(
                    flex_col((
                        label(format!(
                            "Transform · Opacity {:.0}% · Rot {:.1}° · Scale {:.2}x",
                            snap.opacity * 100.0,
                            snap.rotation_deg,
                            snap.scale_x
                        ))
                        .weight(FontWeight::BOLD)
                        .text_size(12.0)
                        .color(TEXT_PRIMARY),
                        slider(0.0, 1.0, op, |st: &mut StudioState, v: f64| {
                            let _ = st.inspector_set_opacity(v as f32);
                        }),
                        flex_row((
                            text_button("Rot -15°", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_rotation(-15.0);
                            }),
                            text_button("Rot +15°", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_rotation(15.0);
                            }),
                            text_button("Scale -0.1", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_scale(-0.1);
                            }),
                            text_button("Scale +0.1", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_scale(0.1);
                            }),
                            text_button("Reset", |st: &mut StudioState| {
                                let _ = st.inspector_reset_transform();
                            }),
                        )),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0),
                );

                // Appearance & Color Swatches
                let cur_color = snap
                    .fill
                    .or(snap.text_color)
                    .unwrap_or_else(|| "#38bdf8".to_string());
                sections.push(
                    flex_col((
                        label(format!(
                            "Appearance · Color {cur_color} · Radius {:.1}px",
                            snap.corner_radius
                        ))
                        .weight(FontWeight::BOLD)
                        .text_size(12.0)
                        .color(TEXT_PRIMARY),
                        text_input(cur_color, |st: &mut StudioState, hex: String| {
                            let _ = st.inspector_set_fill(&hex);
                        }),
                        flex_row((
                            text_button("#38bdf8 Sky", |st: &mut StudioState| {
                                let _ = st.inspector_set_fill("#38bdf8");
                            }),
                            text_button("#f43f5e Rose", |st: &mut StudioState| {
                                let _ = st.inspector_set_fill("#f43f5e");
                            }),
                            text_button("#10b981 Emerald", |st: &mut StudioState| {
                                let _ = st.inspector_set_fill("#10b981");
                            }),
                            text_button("#f8fafc White", |st: &mut StudioState| {
                                let _ = st.inspector_set_fill("#f8fafc");
                            }),
                        )),
                        flex_row((
                            text_button("Radius -4", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_corner_radius(-4.0);
                            }),
                            text_button("Radius +4", |st: &mut StudioState| {
                                let _ = st.inspector_nudge_corner_radius(4.0);
                            }),
                        )),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0),
                );

                // Typography section (when Text node)
                if let Some(txt) = snap.text_content {
                    sections.push(
                        flex_col((
                            label(format!(
                                "Typography · {:.0}px",
                                snap.font_size.unwrap_or(16.0)
                            ))
                            .weight(FontWeight::BOLD)
                            .text_size(12.0)
                            .color(TEXT_PRIMARY),
                            text_input(txt, |st: &mut StudioState, next: String| {
                                let _ = st.inspector_set_text_content(&next);
                            }),
                            flex_row((
                                text_button("Size -2", |st: &mut StudioState| {
                                    let _ = st.inspector_nudge_font_size(-2.0);
                                }),
                                text_button("Size +2", |st: &mut StudioState| {
                                    let _ = st.inspector_nudge_font_size(2.0);
                                }),
                                text_button("Left", |st: &mut StudioState| {
                                    let _ = st.inspector_set_text_align("left");
                                }),
                                text_button("Center", |st: &mut StudioState| {
                                    let _ = st.inspector_set_text_align("center");
                                }),
                                text_button("Right", |st: &mut StudioState| {
                                    let _ = st.inspector_set_text_align("right");
                                }),
                            )),
                        ))
                        .padding(6.0)
                        .background_color(BG_PANEL)
                        .border(BORDER_SUBTLE, 1.0),
                    );
                }

                // Keyframe & Easing section
                let active_prop = state.active_anim_property.clone();
                sections.push(
                    flex_col((
                        label(format!("Animation Track · Property: {active_prop}"))
                            .weight(FontWeight::BOLD)
                            .text_size(12.0)
                            .color(ACCENT_EMERALD),
                        flex_row((
                            text_button("linear", |st: &mut StudioState| {
                                let p = st.active_anim_property.clone();
                                let _ = st.inspector_set_easing(&p, "linear");
                            }),
                            text_button("easeOut", |st: &mut StudioState| {
                                let p = st.active_anim_property.clone();
                                let _ = st.inspector_set_easing(&p, "easeOut");
                            }),
                            text_button("easeOutCubic", |st: &mut StudioState| {
                                let p = st.active_anim_property.clone();
                                let _ = st.inspector_set_easing(&p, "easeOutCubic");
                            }),
                            text_button("spring", |st: &mut StudioState| {
                                let p = st.active_anim_property.clone();
                                let _ = st.inspector_set_easing(&p, "spring(180, 20)");
                            }),
                        )),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0),
                );
            }
        }
        RightDockTab::Layers => {
            for row in state.build_layer_tree_rows() {
                let nid = row.node_id;
                let indent = "  ".repeat(row.depth);
                let vis_icon = if row.visible { "👁" } else { "🚫" };
                let title = format!("{indent}{vis_icon} {}", row.label);
                sections.push(
                    flex_row((
                        text_button(title, move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            let _ = st.render_current_frame();
                        }),
                        FlexSpacer::Flex(1.0),
                        text_button("Vis", move |st: &mut StudioState| {
                            let _ = st.layer_toggle_visibility(nid);
                        }),
                        text_button("↑", move |st: &mut StudioState| {
                            let _ = st.layer_move_z_order(nid, -1);
                        }),
                        text_button("↓", move |st: &mut StudioState| {
                            let _ = st.layer_move_z_order(nid, 1);
                        }),
                    ))
                    .padding(5.0)
                    .background_color(if row.selected { BG_CARD } else { BG_PANEL })
                    .border(BORDER_SUBTLE, 1.0),
                );
            }
        }
        RightDockTab::AiAgent => {
            let sel_id = state.selected_node.unwrap_or(NodeId(0));
            let draft = state.chat.input_draft.clone();
            sections.push(
                flex_col((
                    label(format!(
                        "AI Agent Studio Loop · Target {sel_id} · {}",
                        state.chat.provider_model
                    ))
                    .weight(FontWeight::BOLD)
                    .text_size(12.0)
                    .color(ACCENT_CYAN),
                    checkbox(
                        "Attach selected node context",
                        state.chat.attach_selected_node,
                        |st: &mut StudioState, v: bool| {
                            st.chat.attach_selected_node = v;
                        },
                    ),
                    text_input(draft, |st: &mut StudioState, next: String| {
                        st.chat.input_draft = next;
                    }),
                    flex_row((
                        text_button("Send to Agent", |st: &mut StudioState| {
                            let _ = st.submit_current_chat_draft();
                        }),
                        text_button("Build Element Prompt v1", |st: &mut StudioState| {
                            let _ = st.generate_selected_element_prompt("");
                        }),
                        text_button("/look", |st: &mut StudioState| {
                            let _ = st.submit_chat_prompt("/look");
                        }),
                        text_button("Add Film Grain", |st: &mut StudioState| {
                            let _ = st.submit_chat_prompt("add film grain");
                        }),
                    )),
                ))
                .padding(6.0)
                .background_color(BG_CARD)
                .border(BORDER_SUBTLE, 1.0),
            );

            if let Some(prompt_v1) = &state.last_copied_agent_prompt {
                sections.push(
                    flex_col((
                        label("Element Edit Request v1 (Ready for Agent / Clipboard):")
                            .weight(FontWeight::BOLD)
                            .text_size(11.0)
                            .color(ACCENT_EMERALD),
                        prose(prompt_v1.clone()),
                    ))
                    .padding(6.0)
                    .background_color(BG_ROOT)
                    .border(BORDER_SUBTLE, 1.0),
                );
            }

            for msg in &state.chat.messages {
                let header = format!("[{}]", msg.role.badge());
                sections.push(
                    flex_col((
                        label(header)
                            .weight(FontWeight::BOLD)
                            .text_size(11.0)
                            .color(ACCENT_CYAN),
                        prose(msg.content.clone()),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0),
                );
            }
        }
        RightDockTab::Renders => {
            let out_path = state.export_output_path.clone();
            let cur_fmt = state.export_format.clone();
            sections.push(
                flex_col((
                    label(format!("Render Queue · Format: {}", cur_fmt.to_uppercase()))
                        .weight(FontWeight::BOLD)
                        .text_size(12.0)
                        .color(TEXT_PRIMARY),
                    flex_row((
                        text_button("MP4", |st: &mut StudioState| {
                            st.export_format = "mp4".to_string();
                            st.export_output_path = "studio-render.mp4".to_string();
                        }),
                        text_button("MKV", |st: &mut StudioState| {
                            st.export_format = "mkv".to_string();
                            st.export_output_path = "studio-render.mkv".to_string();
                        }),
                        text_button("WebM", |st: &mut StudioState| {
                            st.export_format = "webm".to_string();
                            st.export_output_path = "studio-render.webm".to_string();
                        }),
                        text_button("GIF", |st: &mut StudioState| {
                            st.export_format = "gif".to_string();
                            st.export_output_path = "studio-render.gif".to_string();
                        }),
                        text_button("PNG", |st: &mut StudioState| {
                            st.export_format = "png".to_string();
                            st.export_output_path = "studio-frame.png".to_string();
                        }),
                    )),
                    text_input(out_path, |st: &mut StudioState, next: String| {
                        st.export_output_path = next;
                    }),
                    text_button("▶ Start In-Process Render", |st: &mut StudioState| {
                        let _ = st.run_export_job();
                    }),
                ))
                .padding(6.0)
                .background_color(BG_CARD)
                .border(BORDER_SUBTLE, 1.0),
            );

            for job in state.render_queue.iter().rev() {
                let status_str = match &job.status {
                    RenderJobStatus::Completed => "COMPLETED".to_string(),
                    RenderJobStatus::Failed(err) => format!("FAILED: {err}"),
                };
                sections.push(
                    flex_col((
                        label(format!(
                            "#{} [{}] {} ({} frames @ {}fps)",
                            job.id,
                            job.format.to_uppercase(),
                            job.composition_title,
                            job.frames,
                            job.fps
                        ))
                        .weight(FontWeight::BOLD)
                        .text_size(12.0)
                        .color(TEXT_PRIMARY),
                        label(format!(
                            "{} · {} bytes -> {}",
                            status_str,
                            job.bytes_written,
                            job.output_path.display()
                        ))
                        .text_size(11.0)
                        .color(ACCENT_EMERALD),
                    ))
                    .padding(6.0)
                    .background_color(BG_PANEL)
                    .border(BORDER_SUBTLE, 1.0),
                );
            }
        }
    }

    flex_col((tab_bar, portal(flex_col(sections).padding(8.0)).flex(1.0)))
        .background_color(BG_PANEL)
        .border(BORDER_SUBTLE, 1.0)
}
