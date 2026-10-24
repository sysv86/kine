//! Native Rust **Xilem 0.4.0** desktop GUI for Kine Studio (Linux, Windows, macOS)
//! with **Hyperframes Studio** visual design & UX parity.
//!
//! Features:
//! - **Graphite (Dark) & Paper (Light) Theme Engine** (`StudioPalette` from `theme.rs`).
//! - **10px Rounded Dock Cards & 6px Shell Gutters** (`dock_card_shell` + `dock_tab_pill`).
//! - **Recessed Dark Preview Stage** with composition breadcrumb, 8-point selection chrome,
//!   WebMCP Topology Lens (`#3ce6ac`), Rule-of-Thirds grid, Safe Guides, and live Stereo
//!   L/R Audio Peak Meters (`progress_bar`).
//! - **Two-Column Proportional NLE Timeline**: Fixed 220px left track header gutter +
//!   proportional horizontal clip bars (Teal/Mint visual clips, Violet audio clips,
//!   Cyan WGSL shader clips) with clickable `◆` keyframe diamonds and zoom slider.
//! - **Collapsible Flat Property Inspector (`PropertyPanelFlat`)**: Header with kind badge,
//!   visibility toggle, prompt copy, and clear selection; collapsible accordion sections
//!   with 1-line live summaries (`Layout`, `Style`, `Typography`, `Motion`, `Shaders & FX`);
//!   and a pinned footer (`Ask AI Agent` + `Auto-KF Record`).

use crate::catalog::CATALOG_ITEMS;
use crate::state::{LeftDockTab, RenderJobStatus, RightDockTab, StudioState};
use crate::theme::sample_audio_meter_levels;
#[cfg(feature = "xilem-ui")]
use crate::theme::{
    dock_card_shell, dock_tab_pill, flat_group_header, kind_badge_chip, StudioPalette,
};
use crate::timeline::{generate_ruler_ticks, TimelineTrackKind};
use kine_core::NodeId;
use kine_render::FrameFormat;
use std::path::Path;

#[cfg(feature = "xilem-ui")]
use xilem::{
    masonry::{
        layout::AsUnit as _,
        parley::style::FontWeight,
        peniko::{ImageBrush, ObjectFit},
    },
    style::Style as _,
    view::{
        checkbox, flex_col, flex_row, image, label, portal, progress_bar, prose, sized_box, slider,
        split, text_button, text_input, Axis, CrossAxisAlignment, FlexExt as _, FlexSpacer,
    },
    winit::error::EventLoopError,
    EventLoop, WidgetView, WindowOptions, Xilem,
};

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
    let pal = StudioPalette::for_mode(state.dark_theme);
    let header = header_bar_view(state, pal);
    let left_dock = dock_card_shell(left_dock_view(state, pal), pal);
    let center_workspace = center_workspace_view(state, pal);
    let right_dock = dock_card_shell(right_dock_view(state, pal), pal);

    let main_body = split(
        left_dock,
        split(center_workspace, right_dock)
            .split_axis(Axis::Horizontal)
            .split_point(if state.inspector_visible { 0.70 } else { 0.98 }),
    )
    .split_axis(Axis::Horizontal)
    .split_point(0.21);

    flex_col((header, main_body.flex(1.0))).background_color(pal.bg_shell)
}

#[cfg(feature = "xilem-ui")]
fn header_bar_view(
    state: &mut StudioState,
    pal: StudioPalette,
) -> impl WidgetView<StudioState> + use<> {
    let comp_name = state
        .compositions
        .get(state.active_composition)
        .map(|c| c.title.clone())
        .unwrap_or_else(|| "Untitled Composition".to_string());
    let backend_label = format!("⚡ {}", state.engine.backend().name().to_uppercase());
    let res_label = format!(
        "{}×{} · {:.0}fps",
        state.scene_def.width, state.scene_def.height, state.fps
    );
    let (lint_label, lint_color) = if state.inspect_findings.is_empty() {
        ("✓ 0 issues".to_string(), pal.accent_mint)
    } else {
        (
            format!("⚠ {} findings", state.inspect_findings.len()),
            pal.warning_amber,
        )
    };
    let theme_label = if state.dark_theme {
        "☾ Graphite"
    } else {
        "☀ Paper"
    };
    let banner = state.status_banner.clone();

    flex_row((
        label("✦ KINE // STUDIO")
            .weight(FontWeight::BOLD)
            .text_size(13.0)
            .color(pal.accent_mint),
        label("|").text_size(12.0).color(pal.border_strong),
        label(comp_name)
            .weight(FontWeight::SEMI_BOLD)
            .text_size(12.0)
            .color(pal.text_primary),
        label(res_label).text_size(11.0).color(pal.text_muted),
        label(lint_label)
            .weight(FontWeight::SEMI_BOLD)
            .text_size(11.0)
            .color(lint_color),
        FlexSpacer::Flex(1.0),
        label(banner).text_size(11.0).color(pal.text_muted),
        FlexSpacer::Flex(1.0),
        text_button("📷 Capture PNG", |st: &mut StudioState| {
            let _ = st.capture_current_frame(FrameFormat::Png, Path::new("studio-capture.png"));
        }),
        text_button("◇ SVG", |st: &mut StudioState| {
            let _ = st.capture_current_frame(FrameFormat::Svg, Path::new("studio-capture.svg"));
        }),
        text_button(
            if state.inspector_visible {
                "◧ Inspector: ON"
            } else {
                "◧ Inspector: OFF"
            },
            |st: &mut StudioState| {
                st.inspector_visible = !st.inspector_visible;
            },
        ),
        text_button(theme_label, |st: &mut StudioState| {
            st.dark_theme = !st.dark_theme;
            st.status_banner = format!(
                "Switched theme to {}",
                if st.dark_theme {
                    "Graphite (Dark)"
                } else {
                    "Paper (Light)"
                }
            );
        }),
        text_button(backend_label, |st: &mut StudioState| {
            st.cycle_render_backend();
        }),
        text_button("✦ Ask Agent", |st: &mut StudioState| {
            st.right_tab = RightDockTab::AiAgent;
            st.inspector_visible = true;
            let _ = st.generate_selected_element_prompt("");
        }),
        sized_box(text_button("▶ Export", |st: &mut StudioState| {
            st.right_tab = RightDockTab::Renders;
            st.inspector_visible = true;
            let _ = st.run_export_job();
        }))
        .background_color(pal.accent_mint_wash)
        .border(pal.accent_mint, 1.0)
        .corner_radius(6.0),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(6.0)
    .background_color(pal.bg_surface)
    .border(pal.border_strong, 1.0)
}

#[cfg(feature = "xilem-ui")]
fn left_dock_view(
    state: &mut StudioState,
    pal: StudioPalette,
) -> impl WidgetView<StudioState> + use<> {
    let cur_tab = state.left_tab;
    let tab_strip = flex_row((
        dock_tab_pill(
            LeftDockTab::Compositions.label(),
            cur_tab == LeftDockTab::Compositions,
            pal,
            |st: &mut StudioState| st.left_tab = LeftDockTab::Compositions,
        ),
        dock_tab_pill(
            LeftDockTab::Assets.label(),
            cur_tab == LeftDockTab::Assets,
            pal,
            |st: &mut StudioState| st.left_tab = LeftDockTab::Assets,
        ),
        dock_tab_pill(
            LeftDockTab::Code.label(),
            cur_tab == LeftDockTab::Code,
            pal,
            |st: &mut StudioState| st.left_tab = LeftDockTab::Code,
        ),
        dock_tab_pill(
            LeftDockTab::Catalog.label(),
            cur_tab == LeftDockTab::Catalog,
            pal,
            |st: &mut StudioState| st.left_tab = LeftDockTab::Catalog,
        ),
    ))
    .padding(6.0)
    .background_color(pal.bg_inset)
    .border(pal.border_hairline, 1.0);

    let mut items: Vec<_> = Vec::new();
    match state.left_tab {
        LeftDockTab::Compositions => {
            for (idx, comp) in state.compositions.iter().enumerate() {
                let is_active = idx == state.active_composition;
                let badge = if idx == 0 { "MASTER" } else { "COMP" };
                let title = if is_active {
                    format!("● {}", comp.title)
                } else {
                    format!("○ {}", comp.title)
                };
                let sub = comp.subtitle.clone();
                items.push(
                    flex_col((
                        flex_row((
                            kind_badge_chip(badge, pal),
                            text_button(title, move |st: &mut StudioState| {
                                let _ = st.select_composition(idx);
                            }),
                        ))
                        .cross_axis_alignment(CrossAxisAlignment::Center),
                        label(sub).text_size(11.0).color(pal.text_muted),
                    ))
                    .padding(8.0)
                    .background_color(if is_active {
                        pal.accent_mint_wash
                    } else {
                        pal.bg_surface
                    })
                    .border(
                        if is_active {
                            pal.accent_mint
                        } else {
                            pal.border_hairline
                        },
                        1.0,
                    )
                    .corner_radius(8.0),
                );
            }
        }
        LeftDockTab::Assets => {
            for asset in &state.assets {
                let badge = asset.kind.badge();
                let name = asset.name.clone();
                let uri = asset.uri.clone();
                let details = format!("{} · {}", asset.uri, asset.details);
                items.push(
                    flex_col((
                        flex_row((
                            kind_badge_chip(badge, pal),
                            text_button(name, move |st: &mut StudioState| {
                                st.status_banner = format!("Asset URI ready: {uri}");
                            }),
                        ))
                        .cross_axis_alignment(CrossAxisAlignment::Center),
                        label(details).text_size(11.0).color(pal.text_muted),
                    ))
                    .padding(7.0)
                    .background_color(pal.bg_surface)
                    .border(pal.border_hairline, 1.0)
                    .corner_radius(8.0),
                );
            }
        }
        LeftDockTab::Code => {
            let src_buf = state.source_buffer.clone();
            items.push(
                flex_col((
                    flex_row((
                        label("Scene AST Source (JSON / TypeScript)")
                            .weight(FontWeight::BOLD)
                            .text_size(12.0)
                            .color(pal.text_primary),
                        FlexSpacer::Flex(1.0),
                        text_button("▶ Apply to Engine", |st: &mut StudioState| {
                            if let Err(e) = st.apply_source_buffer() {
                                st.status_banner = e.to_string();
                            }
                        }),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                    text_input(src_buf.clone(), |st: &mut StudioState, next: String| {
                        st.source_buffer = next;
                    }),
                    prose(src_buf),
                ))
                .padding(8.0)
                .background_color(pal.bg_inset)
                .border(pal.border_hairline, 1.0)
                .corner_radius(8.0),
            );
        }
        LeftDockTab::Catalog => {
            for item in CATALOG_ITEMS {
                let item_id = item.id;
                let cat_badge = if item.category.contains("WGSL") {
                    "WGSL"
                } else if item.category.contains("Audio") {
                    "AUD"
                } else if item.category.contains("Typography") {
                    "TEXT"
                } else {
                    "RECT"
                };
                let btn_label = format!("+ {}", item.label);
                let desc = format!("{} — {}", item.category, item.description);
                items.push(
                    flex_col((
                        flex_row((
                            kind_badge_chip(cat_badge, pal),
                            text_button(btn_label, move |st: &mut StudioState| {
                                let _ = st.insert_catalog_item(item_id);
                            }),
                        ))
                        .cross_axis_alignment(CrossAxisAlignment::Center),
                        label(desc).text_size(11.0).color(pal.text_muted),
                    ))
                    .padding(7.0)
                    .background_color(pal.bg_surface)
                    .border(pal.border_hairline, 1.0)
                    .corner_radius(8.0),
                );
            }
        }
    }

    flex_col((tab_strip, portal(flex_col(items).padding(8.0)).flex(1.0)))
        .background_color(pal.bg_surface)
}

#[cfg(feature = "xilem-ui")]
fn center_workspace_view(
    state: &mut StudioState,
    pal: StudioPalette,
) -> impl WidgetView<StudioState> + use<> {
    let preview = dock_card_shell(preview_pane_view(state, pal), pal);
    let timeline = dock_card_shell(timeline_pane_view(state, pal), pal);

    split(preview, timeline)
        .split_axis(Axis::Vertical)
        .split_point(0.54)
}

#[cfg(feature = "xilem-ui")]
fn preview_pane_view(
    state: &mut StudioState,
    pal: StudioPalette,
) -> impl WidgetView<StudioState> + use<> {
    let comp_title = state
        .compositions
        .get(state.active_composition)
        .map(|c| c.title.as_str())
        .unwrap_or("Composition");
    let sel_crumb = if let Some(snap) = state.inspect_selected_node() {
        format!(
            "{} ▸ {} ({:.0}×{:.0} px)",
            comp_title, snap.label, snap.computed_rect.width, snap.computed_rect.height
        )
    } else {
        format!("{comp_title} ▸ (No selection)")
    };

    // Top Breadcrumb & Viewport Guides Bar (Hyperframes PreviewPane + PreviewZoomOverlay)
    let viewport_hud = flex_row((
        label(sel_crumb)
            .weight(FontWeight::SEMI_BOLD)
            .text_size(11.5)
            .color(pal.text_primary),
        FlexSpacer::Flex(1.0),
        checkbox(
            "Bounds",
            state.show_bounding_boxes,
            |st: &mut StudioState, v: bool| {
                st.show_bounding_boxes = v;
                let _ = st.render_current_frame();
            },
        ),
        checkbox(
            "Topology Lens",
            state.topology_lens_active,
            |st: &mut StudioState, v: bool| {
                st.topology_lens_active = v;
                let _ = st.render_current_frame();
            },
        ),
        checkbox(
            "Grid 3×3",
            state.show_grid_overlay,
            |st: &mut StudioState, v: bool| {
                st.show_grid_overlay = v;
                let _ = st.render_current_frame();
            },
        ),
        checkbox(
            "Safe Area",
            state.show_safe_guides,
            |st: &mut StudioState, v: bool| {
                st.show_safe_guides = v;
                let _ = st.render_current_frame();
            },
        ),
        text_button("−", |st: &mut StudioState| {
            st.viewport_zoom = (st.viewport_zoom - 0.25).clamp(0.25, 4.0);
        }),
        text_button(
            format!("{:.0}%", state.viewport_zoom * 100.0),
            |st: &mut StudioState| {
                st.viewport_zoom = 1.0;
            },
        ),
        text_button("+", |st: &mut StudioState| {
            st.viewport_zoom = (st.viewport_zoom + 0.25).clamp(0.25, 4.0);
        }),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(6.0)
    .background_color(pal.bg_inset)
    .border(pal.border_hairline, 1.0);

    let brush = ImageBrush::new(state.frame_image_data.clone());
    let canvas_stage = sized_box(image(brush).fit(ObjectFit::Contain))
        .expand()
        .padding(14.0)
        .background_color(pal.bg_preview_well);

    // Optional live stereo L/R Audio Meter Strip at the bottom of the preview well
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
    let meter_strip = if state.show_audio_meters && has_audio {
        flex_row((
            label("AUDIO L").text_size(10.0).color(pal.accent_mint),
            progress_bar(Some(f64::from(lvl_l))).flex(1.0),
            label("R").text_size(10.0).color(pal.accent_mint),
            progress_bar(Some(f64::from(lvl_r))).flex(1.0),
            label(if state.audio_muted {
                "MUTED".to_string()
            } else {
                format!("{:+.1} dBFS", (lvl_l.max(0.01)).log10() * 20.0)
            })
            .text_size(10.0)
            .color(pal.text_muted),
        ))
        .cross_axis_alignment(CrossAxisAlignment::Center)
        .padding(4.0)
        .background_color(pal.bg_inset)
    } else {
        flex_row((label("").text_size(1.0),)).padding(0.0).background_color(pal.bg_inset)
    };

    let transport = transport_controls_view(state, pal);

    flex_col((viewport_hud, canvas_stage.flex(1.0), meter_strip, transport))
        .background_color(pal.bg_preview_well)
}

#[cfg(feature = "xilem-ui")]
fn transport_controls_view(
    state: &mut StudioState,
    pal: StudioPalette,
) -> impl WidgetView<StudioState> + use<> {
    let play_label = if state.is_playing { "⏸ Pause" } else { "▶ Play" };
    let timecode = state.formatted_timecode();
    let dur = state.duration.max(0.1);
    let cur = state.current_time.clamp(0.0, dur);
    let speed_label = format!("{:.2}x", state.playback_rate);

    flex_row((
        text_button("⏮", |st: &mut StudioState| {
            st.is_playing = false;
            st.seek_to(0.0, false);
        }),
        text_button("◀ -1f", |st: &mut StudioState| {
            st.step_frames(-1);
        }),
        sized_box(text_button(play_label, |st: &mut StudioState| {
            st.is_playing = !st.is_playing;
            if st.is_playing {
                st.tick_playback(1.0 / st.fps.max(1.0));
            }
        }))
        .background_color(pal.accent_mint_wash)
        .border(pal.accent_mint, 1.0)
        .corner_radius(6.0),
        text_button("+1f ▶", |st: &mut StudioState| {
            st.step_frames(1);
        }),
        text_button("⏭", |st: &mut StudioState| {
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
    .background_color(pal.bg_surface)
    .border(pal.border_hairline, 1.0)
}

#[cfg(feature = "xilem-ui")]
fn timeline_pane_view(
    state: &mut StudioState,
    pal: StudioPalette,
) -> impl WidgetView<StudioState> + use<> {
    let undo_label = format!("↶ ({})", state.undo_stack.len());
    let redo_label = format!("↷ ({})", state.redo_stack.len());
    let zoom_pct = ((state.timeline_zoom_pps / 140.0) * 100.0).round() as u32;
    let zoom_val = f64::from(state.timeline_zoom_pps.clamp(40.0, 400.0));

    // Hyperframes TimelineToolbar parity: History | Snap & Ripple & Meters | Keyframe & Auto-KF | Split/Dup/Del | Zoom Slider + Fit
    let toolbar = flex_row((
        text_button(undo_label, |st: &mut StudioState| {
            let _ = st.undo();
        }),
        text_button(redo_label, |st: &mut StudioState| {
            let _ = st.redo();
        }),
        label("|").text_size(11.0).color(pal.border_strong),
        checkbox(
            "🧲 Snap (N)",
            state.timeline_snap_enabled,
            |st: &mut StudioState, v: bool| {
                st.timeline_snap_enabled = v;
            },
        ),
        checkbox(
            "≋ Ripple",
            state.ripple_edit,
            |st: &mut StudioState, v: bool| {
                st.ripple_edit = v;
            },
        ),
        checkbox(
            "📊 Meters",
            state.show_audio_meters,
            |st: &mut StudioState, v: bool| {
                st.show_audio_meters = v;
            },
        ),
        label("|").text_size(11.0).color(pal.border_strong),
        text_button("◀ KF", |st: &mut StudioState| {
            st.timeline_jump_prev_keyframe();
        }),
        sized_box(text_button("◆ +KF (K)", |st: &mut StudioState| {
            let _ = st.timeline_add_keyframe_at_playhead();
        }))
        .background_color(pal.accent_mint_wash)
        .border(pal.accent_mint, 1.0)
        .corner_radius(5.0),
        text_button("KF ▶", |st: &mut StudioState| {
            st.timeline_jump_next_keyframe();
        }),
        checkbox(
            "● Auto-KF",
            state.auto_keyframe,
            |st: &mut StudioState, v: bool| {
                st.auto_keyframe = v;
            },
        ),
        label("|").text_size(11.0).color(pal.border_strong),
        text_button("✂ Split (S)", |st: &mut StudioState| {
            let _ = st.timeline_split_at_playhead();
        }),
        text_button("⧉ Dup", |st: &mut StudioState| {
            let _ = st.timeline_duplicate_selected();
        }),
        text_button("⌫ Del", |st: &mut StudioState| {
            let _ = st.timeline_delete_selected();
        }),
        FlexSpacer::Flex(1.0),
        text_button("Fit", |st: &mut StudioState| {
            st.timeline_zoom_pps = 140.0;
        }),
        slider(40.0, 360.0, zoom_val, |st: &mut StudioState, v: f64| {
            st.timeline_zoom_pps = v as f32;
        }),
        label(format!("{zoom_pct}%"))
            .text_size(11.0)
            .color(pal.text_muted),
    ))
    .cross_axis_alignment(CrossAxisAlignment::Center)
    .padding(5.0)
    .background_color(pal.bg_inset)
    .border(pal.border_hairline, 1.0);

    // Sticky Two-Column Time Ruler: Left 230px gutter header + Right proportional tick ruler with Playhead badge
    let ticks = generate_ruler_ticks(state.duration, state.timeline_zoom_pps);
    let mut tick_buttons: Vec<_> = Vec::new();
    let playhead_sec = state.current_time;
    tick_buttons.push(
        sized_box(
            label(format!("▼ {:.2}s", playhead_sec))
                .weight(FontWeight::BOLD)
                .text_size(10.5)
                .color(pal.accent_mint),
        )
        .padding(2.0)
        .background_color(pal.accent_mint_wash)
        .border(pal.accent_mint, 1.0)
        .corner_radius(4.0),
    );
    for tick in ticks.into_iter().filter(|t| t.is_major) {
        let t_sec = tick.time_sec;
        let lbl = tick.label.unwrap_or_else(|| format!("{t_sec:.1}s"));
        tick_buttons.push(text_button(lbl, move |st: &mut StudioState| {
            st.seek_to(t_sec, false);
        }));
    }

    let ruler_bar = flex_row((
        sized_box(
            label(format!(
                "TRACKS & NODES · F{:03}",
                state.current_frame_index()
            ))
            .weight(FontWeight::BOLD)
            .text_size(10.5)
            .color(pal.text_muted),
        )
        .width(230.px())
        .padding(4.0)
        .background_color(pal.bg_inset),
        flex_row(tick_buttons)
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .flex(1.0)
            .padding(3.0)
            .background_color(pal.bg_surface),
    ))
    .border(pal.border_strong, 1.0);

    // Two-Column NLE Track Lanes: Left 230px Gutter + Right Proportional Clip Pill Bars
    let pps = f64::from(state.timeline_zoom_pps.clamp(40.0, 360.0));
    let tracks = state.build_timeline_tracks();
    let mut track_rows: Vec<_> = Vec::new();

    for track in tracks {
        let is_selected = track.selected;
        let left_px = ((track.start_sec.max(0.0)) * pps).round().clamp(0.0, 640.0);
        let span_sec = (track.end_sec - track.start_sec).max(0.25);
        let bar_w_px = (span_sec * pps).round().clamp(90.0, 780.0);

        // Build clickable keyframe diamond chips inside the clip bar
        let mut kf_chips: Vec<_> = Vec::new();
        for kf in &track.keyframes {
            let kf_t = kf.time;
            let kf_v = kf.value;
            let chip_lbl = format!("◆{kf_t:.2}s ({kf_v:.0})");
            kf_chips.push(text_button(chip_lbl, move |st: &mut StudioState| {
                st.seek_to(kf_t, false);
            }));
        }

        match track.kind {
            TimelineTrackKind::SceneNode {
                node_id,
                depth,
                kind_badge,
            } => {
                let nid = node_id;
                let indent = "  ".repeat(depth);
                let gutter_title = format!("{indent}{}", track.label);
                let is_shader = kind_badge == "SHDR";
                let (clip_bg, clip_border) = if is_selected {
                    (pal.clip_visual_active, pal.accent_mint)
                } else if is_shader {
                    (pal.clip_shader_bg, pal.accent_cyan)
                } else {
                    (pal.clip_visual_bg, pal.border_strong)
                };

                let left_gutter = sized_box(
                    flex_row((
                        kind_badge_chip(kind_badge, pal),
                        text_button(gutter_title, move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            let _ = st.render_current_frame();
                        }),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                )
                .width(230.px())
                .padding(4.0)
                .background_color(if is_selected {
                    pal.bg_raised
                } else {
                    pal.bg_inset
                })
                .border(pal.border_hairline, 1.0);

                let clip_pill = sized_box(
                    flex_row((
                        label(format!(
                            "{} ({:.2}s–{:.2}s)",
                            track.label, track.start_sec, track.end_sec
                        ))
                        .weight(FontWeight::SEMI_BOLD)
                        .text_size(11.0)
                        .color(pal.text_primary),
                        FlexSpacer::Flex(1.0),
                        flex_row(kf_chips),
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
                    .padding(4.0),
                )
                .width(bar_w_px.px())
                .background_color(clip_bg)
                .border(clip_border, 1.0)
                .corner_radius(8.0);

                let right_lane = flex_row((
                    FlexSpacer::Fixed(left_px.px()),
                    clip_pill,
                    FlexSpacer::Flex(1.0),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .flex(1.0)
                .padding(3.0)
                .background_color(pal.bg_surface);

                track_rows.push(flex_row((left_gutter, right_lane)).border(pal.border_hairline, 1.0));
            }
            TimelineTrackKind::AnimationProperty {
                node_id,
                property,
                easing,
            } => {
                let nid = node_id;
                let prop = property.clone();
                let left_gutter = sized_box(
                    flex_row((
                        label("   ↳").text_size(11.0).color(pal.accent_mint),
                        text_button(property.clone(), move |st: &mut StudioState| {
                            st.selected_node = Some(nid);
                            st.active_anim_property = prop.clone();
                            let _ = st.render_current_frame();
                        }),
                        label(format!("({easing})"))
                            .text_size(10.0)
                            .color(pal.text_muted),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                )
                .width(230.px())
                .padding(3.0)
                .background_color(pal.bg_inset)
                .border(pal.border_hairline, 1.0);

                let kf_bar = sized_box(
                    flex_row((
                        label(format!("{:.2}s..{:.2}s", track.start_sec, track.end_sec))
                            .text_size(10.5)
                            .color(pal.accent_mint),
                        FlexSpacer::Flex(1.0),
                        flex_row(kf_chips),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(3.0),
                )
                .width(bar_w_px.px())
                .background_color(pal.bg_inset)
                .border(pal.accent_mint, 1.0)
                .corner_radius(6.0);

                let right_lane = flex_row((
                    FlexSpacer::Fixed(left_px.px()),
                    kf_bar,
                    FlexSpacer::Flex(1.0),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .flex(1.0)
                .padding(2.0)
                .background_color(pal.bg_surface);

                track_rows.push(flex_row((left_gutter, right_lane)).border(pal.border_hairline, 1.0));
            }
            TimelineTrackKind::AudioTrack { track_index } => {
                let left_gutter = sized_box(
                    flex_row((
                        kind_badge_chip("AUD", pal),
                        text_button(track.label.clone(), move |st: &mut StudioState| {
                            if let Some(audio) = &mut st.scene_def.audio {
                                if let Some(tr) = audio.tracks.get_mut(track_index) {
                                    tr.muted = !tr.muted;
                                }
                            }
                        }),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center),
                )
                .width(230.px())
                .padding(4.0)
                .background_color(pal.bg_inset)
                .border(pal.border_hairline, 1.0);

                // Hyperframes violet audio pill (`--timeline-clip-audio-bg`) with simulated waveform bars
                let audio_pill = sized_box(
                    flex_row((
                        label(format!("♫ {} · ▁▃▅▇▃▂▄▆█▆▄▂", track.subtitle))
                            .weight(FontWeight::SEMI_BOLD)
                            .text_size(11.0)
                            .color(pal.text_primary),
                        FlexSpacer::Flex(1.0),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(5.0),
                )
                .width(bar_w_px.px())
                .background_color(pal.clip_audio_bg)
                .border(pal.border_strong, 1.0)
                .corner_radius(10.0);

                let right_lane = flex_row((
                    FlexSpacer::Fixed(left_px.px()),
                    audio_pill,
                    FlexSpacer::Flex(1.0),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .flex(1.0)
                .padding(3.0)
                .background_color(pal.bg_surface);

                track_rows.push(flex_row((left_gutter, right_lane)).border(pal.border_hairline, 1.0));
            }
        }
    }

    flex_col((
        toolbar,
        ruler_bar,
        portal(flex_col(track_rows)).flex(1.0),
    ))
    .background_color(pal.bg_surface)
}

#[cfg(feature = "xilem-ui")]
fn right_dock_view(
    state: &mut StudioState,
    pal: StudioPalette,
) -> impl WidgetView<StudioState> + use<> {
    let cur_tab = state.right_tab;
    let tab_strip = flex_row((
        dock_tab_pill(
            RightDockTab::Design.label(),
            cur_tab == RightDockTab::Design,
            pal,
            |st: &mut StudioState| st.right_tab = RightDockTab::Design,
        ),
        dock_tab_pill(
            RightDockTab::Layers.label(),
            cur_tab == RightDockTab::Layers,
            pal,
            |st: &mut StudioState| st.right_tab = RightDockTab::Layers,
        ),
        dock_tab_pill(
            RightDockTab::AiAgent.label(),
            cur_tab == RightDockTab::AiAgent,
            pal,
            |st: &mut StudioState| st.right_tab = RightDockTab::AiAgent,
        ),
        dock_tab_pill(
            RightDockTab::Renders.label(),
            cur_tab == RightDockTab::Renders,
            pal,
            |st: &mut StudioState| st.right_tab = RightDockTab::Renders,
        ),
    ))
    .padding(6.0)
    .background_color(pal.bg_inset)
    .border(pal.border_hairline, 1.0);

    let mut sections: Vec<_> = Vec::new();
    match state.right_tab {
        RightDockTab::Design => {
            if let Some(snap) = state.inspect_selected_node() {
                let nid = snap.node_id;
                let open_grp = state.open_inspector_group.clone();
                let show_all = open_grp == "all";

                // 1. PropertyPanelFlatHeader (Kind badge + Name + Vis + Copy Prompt + Clear + All toggle)
                sections.push(
                    flex_row((
                        kind_badge_chip(snap.kind_badge, pal),
                        label(snap.label.clone())
                            .weight(FontWeight::BOLD)
                            .text_size(12.5)
                            .color(pal.text_primary),
                        FlexSpacer::Flex(1.0),
                        text_button("👁", move |st: &mut StudioState| {
                            let _ = st.layer_toggle_visibility(nid);
                        }),
                        text_button("📋", |st: &mut StudioState| {
                            let _ = st.generate_selected_element_prompt("");
                            st.right_tab = RightDockTab::AiAgent;
                        }),
                        text_button(
                            if show_all { " Accordion " } else { " Expand All " },
                            |st: &mut StudioState| {
                                st.open_inspector_group = if st.open_inspector_group == "all" {
                                    "layout".to_string()
                                } else {
                                    "all".to_string()
                                };
                            },
                        ),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(8.0)
                    .background_color(pal.bg_raised)
                    .border(pal.border_hairline, 1.0)
                    .corner_radius(8.0),
                );

                // 2. Collapsible Group: Layout & Alignment
                let layout_open = show_all || open_grp == "layout";
                let layout_summary = format!(
                    "{:.0},{:.0} · {:.0}×{:.0} px",
                    snap.computed_rect.x,
                    snap.computed_rect.y,
                    snap.computed_rect.width,
                    snap.computed_rect.height
                );
                sections.push(
                    sized_box(flat_group_header(
                        "Layout & Transform",
                        &layout_summary,
                        layout_open,
                        pal,
                        |st: &mut StudioState| {
                            st.open_inspector_group = if st.open_inspector_group == "layout" {
                                String::new()
                            } else {
                                "layout".to_string()
                            };
                        },
                    ))
                    .expand_width(),
                );
                if layout_open {
                    sections.push(
                        flex_col((
                            flex_row((
                                text_button("◰ Left", |st: &mut StudioState| {
                                    let _ = st.inspector_align_selected(Some("left"), None);
                                }),
                                text_button("◫ Center H", |st: &mut StudioState| {
                                    let _ = st.inspector_align_selected(Some("center"), None);
                                }),
                                text_button("◳ Right", |st: &mut StudioState| {
                                    let _ = st.inspector_align_selected(Some("right"), None);
                                }),
                                text_button("⬓ Middle V", |st: &mut StudioState| {
                                    let _ = st.inspector_align_selected(None, Some("middle"));
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
                        .padding(8.0)
                        .background_color(pal.bg_inset)
                        .border(pal.border_hairline, 1.0),
                    );
                }

                // 3. Collapsible Group: Style & Appearance
                let style_open = show_all || open_grp == "style";
                let cur_color = snap
                    .fill
                    .clone()
                    .or_else(|| snap.text_color.clone())
                    .unwrap_or_else(|| "#3ce6ac".to_string());
                let style_summary = format!(
                    "fill {} · {:.0}% · r {:.0}px",
                    cur_color,
                    snap.opacity * 100.0,
                    snap.corner_radius
                );
                sections.push(
                    sized_box(flat_group_header(
                        "Style & Appearance",
                        &style_summary,
                        style_open,
                        pal,
                        |st: &mut StudioState| {
                            st.open_inspector_group = if st.open_inspector_group == "style" {
                                String::new()
                            } else {
                                "style".to_string()
                            };
                        },
                    ))
                    .expand_width(),
                );
                if style_open {
                    let op = f64::from(snap.opacity);
                    sections.push(
                        flex_col((
                            label(format!("Opacity: {:.0}%", snap.opacity * 100.0))
                                .text_size(11.0)
                                .color(pal.text_secondary),
                            slider(0.0, 1.0, op, |st: &mut StudioState, v: f64| {
                                let _ = st.inspector_set_opacity(v as f32);
                            }),
                            text_input(cur_color, |st: &mut StudioState, hex: String| {
                                let _ = st.inspector_set_fill(&hex);
                            }),
                            flex_row((
                                text_button("#3ce6ac Mint", |st: &mut StudioState| {
                                    let _ = st.inspector_set_fill("#3ce6ac");
                                }),
                                text_button("#06e3fa Cyan", |st: &mut StudioState| {
                                    let _ = st.inspector_set_fill("#06e3fa");
                                }),
                                text_button("#f43f5e Rose", |st: &mut StudioState| {
                                    let _ = st.inspector_set_fill("#f43f5e");
                                }),
                                text_button("#f59e0b Amber", |st: &mut StudioState| {
                                    let _ = st.inspector_set_fill("#f59e0b");
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
                        .padding(8.0)
                        .background_color(pal.bg_inset)
                        .border(pal.border_hairline, 1.0),
                    );
                }

                // 4. Collapsible Group: Typography (for Text nodes)
                if let Some(txt) = snap.text_content {
                    let text_open = show_all || open_grp == "text";
                    let text_summary =
                        format!("\"{}\" · {:.0}px", txt, snap.font_size.unwrap_or(16.0));
                    sections.push(
                        sized_box(flat_group_header(
                            "Typography",
                            &text_summary,
                            text_open,
                            pal,
                            |st: &mut StudioState| {
                                st.open_inspector_group = if st.open_inspector_group == "text" {
                                    String::new()
                                } else {
                                    "text".to_string()
                                };
                            },
                        ))
                        .expand_width(),
                    );
                    if text_open {
                        sections.push(
                            flex_col((
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
                            .padding(8.0)
                            .background_color(pal.bg_inset)
                            .border(pal.border_hairline, 1.0),
                        );
                    }
                }

                // 5. Collapsible Group: Motion & Easing Curve
                let motion_open = show_all || open_grp == "motion";
                let active_prop = state.active_anim_property.clone();
                let motion_summary = format!(
                    "{} track(s) · {} ({})",
                    snap.animations.len(),
                    active_prop,
                    state.active_easing_input
                );
                sections.push(
                    sized_box(flat_group_header(
                        "Motion & Easing",
                        &motion_summary,
                        motion_open,
                        pal,
                        |st: &mut StudioState| {
                            st.open_inspector_group = if st.open_inspector_group == "motion" {
                                String::new()
                            } else {
                                "motion".to_string()
                            };
                        },
                    ))
                    .expand_width(),
                );
                if motion_open {
                    sections.push(
                        flex_col((
                            flex_row((
                                text_button("translateX", |st: &mut StudioState| {
                                    st.active_anim_property = "translateX".to_string();
                                }),
                                text_button("translateY", |st: &mut StudioState| {
                                    st.active_anim_property = "translateY".to_string();
                                }),
                                text_button("opacity", |st: &mut StudioState| {
                                    st.active_anim_property = "opacity".to_string();
                                }),
                                text_button("scale", |st: &mut StudioState| {
                                    st.active_anim_property = "scale".to_string();
                                }),
                                text_button("rotation", |st: &mut StudioState| {
                                    st.active_anim_property = "rotation".to_string();
                                }),
                            )),
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
                                    let _ = st.inspector_set_easing(&p, "spring(180, 18)");
                                }),
                            )),
                        ))
                        .padding(8.0)
                        .background_color(pal.bg_inset)
                        .border(pal.border_hairline, 1.0),
                    );
                }

                // 5b. Collapsible Group: WGSL Shaders & Grade (Hyperframes FlatEffectsSection parity)
                let fx_open = show_all || open_grp == "fx" || !snap.shader_uniforms.is_empty();
                let fx_summary = if !snap.shader_uniforms.is_empty() {
                    format!(
                        "{} · {} uniform(s)",
                        snap.shader_blend.as_deref().unwrap_or("normal"),
                        snap.shader_uniforms.len()
                    )
                } else {
                    "Quick attach WGSL post-FX".to_string()
                };
                sections.push(
                    sized_box(flat_group_header(
                        "WGSL Shaders & Grade",
                        &fx_summary,
                        fx_open,
                        pal,
                        |st: &mut StudioState| {
                            st.open_inspector_group = if st.open_inspector_group == "fx" {
                                String::new()
                            } else {
                                "fx".to_string()
                            };
                        },
                    ))
                    .expand_width(),
                );
                if fx_open {
                    let mut fx_rows: Vec<_> = Vec::new();
                    for (u_name, u_val) in &snap.shader_uniforms {
                        let key = u_name.clone();
                        let val_f64 = f64::from(*u_val);
                        let max_range = if key == "density" { 360.0 } else { 2.0 };
                        fx_rows.push(
                            flex_row((
                                label(format!("{u_name}: {u_val:.3}"))
                                    .text_size(11.0)
                                    .color(pal.accent_cyan),
                                slider(0.0, max_range, val_f64, move |st: &mut StudioState, v: f64| {
                                    let _ = st.inspector_set_shader_uniform_f32(&key, v as f32);
                                })
                                .flex(1.0),
                            ))
                            .cross_axis_alignment(CrossAxisAlignment::Center),
                        );
                    }
                    fx_rows.push(flex_row((
                        text_button("+ Vignette", |st: &mut StudioState| {
                            let _ = st.insert_catalog_item("shader-vignette");
                        }),
                        text_button("+ Film Grain", |st: &mut StudioState| {
                            let _ = st.insert_catalog_item("shader-film-grain");
                        }),
                        text_button("+ Chromatic", |st: &mut StudioState| {
                            let _ = st.insert_catalog_item("shader-chromatic");
                        }),
                        text_button("+ Scanlines", |st: &mut StudioState| {
                            let _ = st.insert_catalog_item("shader-scanlines");
                        }),
                    )));
                    sections.push(
                        flex_col(fx_rows)
                            .padding(8.0)
                            .background_color(pal.bg_inset)
                            .border(pal.border_hairline, 1.0),
                    );
                }

                // 6. Pinned PropertyPanelFlatFooter (Ask AI Agent + Auto-KF Recording Toggle)
                sections.push(
                    flex_row((
                        sized_box(text_button(
                            format!("✦ Ask AI Agent about {nid}"),
                            |st: &mut StudioState| {
                                let _ = st.generate_selected_element_prompt("");
                                st.right_tab = RightDockTab::AiAgent;
                            },
                        ))
                        .background_color(pal.accent_mint_wash)
                        .border(pal.accent_mint, 1.0)
                        .corner_radius(6.0),
                        FlexSpacer::Flex(1.0),
                        text_button(
                            if state.auto_keyframe {
                                "● Auto-KF: REC"
                            } else {
                                "○ Auto-KF: OFF"
                            },
                            |st: &mut StudioState| {
                                st.auto_keyframe = !st.auto_keyframe;
                            },
                        ),
                    ))
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(8.0)
                    .background_color(pal.bg_raised)
                    .border(pal.border_strong, 1.0)
                    .corner_radius(8.0),
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
                        kind_badge_chip(row.badge, pal),
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
                    .cross_axis_alignment(CrossAxisAlignment::Center)
                    .padding(6.0)
                    .background_color(if row.selected {
                        pal.accent_mint_wash
                    } else {
                        pal.bg_surface
                    })
                    .border(
                        if row.selected {
                            pal.accent_mint
                        } else {
                            pal.border_hairline
                        },
                        1.0,
                    )
                    .corner_radius(6.0),
                );
            }
        }
        RightDockTab::AiAgent => {
            let sel_id = state.selected_node.unwrap_or(NodeId(0));
            let draft = state.chat.input_draft.clone();
            sections.push(
                flex_col((
                    label(format!(
                        "✦ WebMCP Agent Studio · Target {sel_id} · {}",
                        state.chat.provider_model
                    ))
                    .weight(FontWeight::BOLD)
                    .text_size(12.0)
                    .color(pal.accent_mint),
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
                        sized_box(text_button("▶ Send to Agent", |st: &mut StudioState| {
                            let _ = st.submit_current_chat_draft();
                        }))
                        .background_color(pal.accent_mint_wash)
                        .border(pal.accent_mint, 1.0)
                        .corner_radius(6.0),
                        text_button("📋 Prompt v1", |st: &mut StudioState| {
                            let _ = st.generate_selected_element_prompt("");
                        }),
                        text_button("/look", |st: &mut StudioState| {
                            let _ = st.submit_chat_prompt("/look");
                        }),
                        text_button("+ Film Grain", |st: &mut StudioState| {
                            let _ = st.submit_chat_prompt("add film grain");
                        }),
                    )),
                ))
                .padding(8.0)
                .background_color(pal.bg_raised)
                .border(pal.border_strong, 1.0)
                .corner_radius(8.0),
            );

            if let Some(prompt_v1) = &state.last_copied_agent_prompt {
                sections.push(
                    flex_col((
                        label("Kine Studio Element Edit Request v1:")
                            .weight(FontWeight::BOLD)
                            .text_size(11.0)
                            .color(pal.accent_mint),
                        prose(prompt_v1.clone()),
                    ))
                    .padding(8.0)
                    .background_color(pal.bg_inset)
                    .border(pal.accent_mint, 1.0)
                    .corner_radius(8.0),
                );
            }

            for msg in &state.chat.messages {
                let header = format!("[{}]", msg.role.badge());
                sections.push(
                    flex_col((
                        label(header)
                            .weight(FontWeight::BOLD)
                            .text_size(11.0)
                            .color(pal.accent_cyan),
                        prose(msg.content.clone()),
                    ))
                    .padding(7.0)
                    .background_color(pal.bg_surface)
                    .border(pal.border_hairline, 1.0)
                    .corner_radius(6.0),
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
                        .color(pal.text_primary),
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
                    sized_box(text_button(
                        "▶ Start In-Process Render",
                        |st: &mut StudioState| {
                            let _ = st.run_export_job();
                        },
                    ))
                    .background_color(pal.accent_mint_wash)
                    .border(pal.accent_mint, 1.0)
                    .corner_radius(6.0),
                ))
                .padding(8.0)
                .background_color(pal.bg_raised)
                .border(pal.border_strong, 1.0)
                .corner_radius(8.0),
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
                        .color(pal.text_primary),
                        label(format!(
                            "{} · {} bytes -> {}",
                            status_str,
                            job.bytes_written,
                            job.output_path.display()
                        ))
                        .text_size(11.0)
                        .color(pal.accent_mint),
                    ))
                    .padding(7.0)
                    .background_color(pal.bg_surface)
                    .border(pal.border_hairline, 1.0)
                    .corner_radius(6.0),
                );
            }
        }
    }

    flex_col((tab_strip, portal(flex_col(sections).padding(8.0)).flex(1.0)))
        .background_color(pal.bg_surface)
}
