//! Dedicated Settings Window & Modal Overlay (`settings_panel.rs`) for Kine Studio.
//!
//! Provides a 5-category tabbed configuration interface:
//! 1. `Appearance & UI`: Surface theme (`Gruvbox Dark`, `Gruvbox Light`, `Gruvbox Soft`),
//!    Signal Accent preset (`Gruvbox Green`, `Gruvbox Yellow`, `Gruvbox Aqua`, `Gruvbox Purple`, `Gruvbox Red`),
//!    UI Density (`Compact`, `Comfortable`, `Spacious`), UI Animation speed, Reduced Motion,
//!    Cascade Guide Lines, Left/Right Dock visibility, and Status Bar visibility.
//! 2. `Editor & Viewport`: Active canvas dimensions (`640×360`, `1280×720`, `1920×1080`, `1080×1080`, `1080×1920`),
//!    Viewport channel inspection mode (`RGB`, `R`, `G`, `B`, `ALPHA`, `LUMA`), Safe Guides,
//!    Rule-of-Thirds Grid, Center Crosshair, Topology Lens, Nudge step sizes, and Code Editor font size.
//! 3. `Timeline & Audio`: Composition frame rate (`24`, `30`, `60` fps), Snap-to-Frame/Keyframe,
//!    Snap tolerance, Auto-Keyframe, Ripple Edit, Animated Property Sub-tracks, Audio Waveforms,
//!    Live Audio Peak Meters, and Master Volume.
//! 4. `Engine & Export`: Hardware/Software Render Backend (`Auto`, `GPU`, `CPU`), Default Export Container
//!    (`MP4`, `MKV`, `WebM`, `GIF`, `PNG`), AV1 Quantizer (`Q`), Encoder Speed preset (`0–10`),
//!    and Settings JSON persistence (`Save Settings` / `Reset Defaults`).
//! 5. `Keybindings`: Complete reference of studio transport, timeline, viewport, and node shortcuts.

use crate::local_portal::portal;
use kine_session::state::StudioState;
use crate::theme::{
    AccentPreset, SettingsCategory, StudioPalette, StudioThemeMode, UiDensity,
    ViewportChannelMode, ICON_FONT, MONO_FONT, UI_FONT,
};
use kine_render::RenderBackend;
use xilem::{
    masonry::{
        core::ArcStr, parley::style::FontWeight, properties::types::AsUnit as _,
        properties::Padding,
    },
    style::Style as _,
    view::{
        button, flex_col, flex_row, label, sized_box, slider, AnyFlexChild, CrossAxisAlignment,
        FlexExt as _, FlexSpacer,
    },
    Color, WidgetView,
};

type Child = AnyFlexChild<StudioState>;
type Boxed = Box<xilem::AnyWidgetView<StudioState>>;

fn transparent() -> Color {
    Color::from_rgba8(0, 0, 0, 0)
}

fn s_txt(value: impl Into<ArcStr>, size: f32, color: Color) -> impl WidgetView<StudioState> {
    label(value).font(UI_FONT).text_size(size).color(color)
}

fn s_txtw(
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

fn s_mono(value: impl Into<ArcStr>, size: f32, color: Color) -> impl WidgetView<StudioState> {
    label(value).font(MONO_FONT).text_size(size).color(color)
}

fn s_icon(name: &'static str, size: f32, color: Color) -> impl WidgetView<StudioState> {
    label(name).font(ICON_FONT).text_size(size).color(color)
}

/// Build the animated Settings Window overlay (`ZStack` top layer).
pub fn settings_window_overlay(
    state: &mut StudioState,
    t: &StudioPalette,
) -> impl WidgetView<StudioState> {
    let t = *t;
    let active_cat = state.settings_category;

    // Left category sidebar inside settings modal
    let mut nav_items: Vec<Child> = Vec::new();
    nav_items.push(
        sized_box(s_txtw(
            "STUDIO PREFERENCES",
            8.5,
            FontWeight::SEMI_BOLD,
            t.muted,
        ))
        .padding(Padding::from_vh(4.0, 8.0))
        .into_any_flex(),
    );

    for cat in SettingsCategory::ALL {
        let is_active = cat == active_cat;
        nav_items.push(
            button(
                flex_row((
                    s_icon(cat.icon(), 14.0, if is_active { t.accent } else { t.muted }),
                    s_txtw(
                        cat.label(),
                        10.5,
                        if is_active {
                            FontWeight::SEMI_BOLD
                        } else {
                            FontWeight::NORMAL
                        },
                        if is_active { t.text } else { t.text_soft },
                    ),
                    FlexSpacer::Flex(1.0),
                ))
                .gap(7.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                move |st: &mut StudioState| {
                    st.settings_category = cat;
                },
            )
            .padding(Padding::from_vh(6.0, 9.0))
            .corner_radius(6.0)
            .background_color(if is_active {
                t.accent_wash
            } else {
                transparent()
            })
            .active_background_color(t.press)
            .border(
                if is_active {
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

    nav_items.push(FlexSpacer::Flex(1.0).into_any_flex());

    // Footer actions inside left nav: Save JSON & Reset Defaults
    nav_items.push(
        button(
            flex_row((
                s_icon("save", 13.0, t.on_accent),
                s_txtw("Save Config", 10.0, FontWeight::SEMI_BOLD, t.on_accent),
            ))
            .gap(5.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            |st: &mut StudioState| {
                if let Err(e) = st.save_settings_to_disk() {
                    st.notify_status(e.to_string());
                }
            },
        )
        .padding(Padding::from_vh(5.0, 10.0))
        .corner_radius(6.0)
        .background_color(t.accent)
        .active_background_color(t.accent_hover)
        .border(t.accent, 1.0)
        .into_any_flex(),
    );

    nav_items.push(
        button(
            flex_row((
                s_icon("restart_alt", 13.0, t.muted),
                s_txt("Reset Defaults", 9.5, t.muted),
            ))
            .gap(5.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Center),
            |st: &mut StudioState| {
                st.reset_settings_to_defaults();
            },
        )
        .padding(Padding::from_vh(4.0, 10.0))
        .corner_radius(6.0)
        .background_color(t.bg_inset)
        .active_background_color(t.press)
        .border(t.border, 1.0)
        .hovered_border_color(t.border_strong)
        .into_any_flex(),
    );

    let sidebar = sized_box(
        flex_col(nav_items)
            .gap(3.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .width(184.0_f64.px())
    .expand_height()
    .padding(Padding::all(10.0))
    .background_color(t.bg_inset)
    .border(t.border, 1.0);

    let pane_body: Boxed = match active_cat {
        SettingsCategory::Appearance => appearance_tab(state, &t),
        SettingsCategory::EditorViewport => editor_viewport_tab(state, &t),
        SettingsCategory::TimelinePlayback => timeline_playback_tab(state, &t),
        SettingsCategory::EngineExport => engine_export_tab(state, &t),
        SettingsCategory::Shortcuts => shortcuts_tab(&t),
    };

    let modal_header = sized_box(
        flex_row(vec![
            s_icon("tune", 16.0, t.accent).into_any_flex(),
            s_txtw(
                "Kine Studio Settings — UI & Editor Configuration",
                12.0,
                FontWeight::SEMI_BOLD,
                t.text,
            )
            .into_any_flex(),
            FlexSpacer::Flex(1.0).into_any_flex(),
            s_mono(
                format!(
                    "{} · {} · {}",
                    state.settings.theme_mode.short_label(),
                    state.settings.accent_preset.label(),
                    state.settings.density.label()
                ),
                9.0,
                t.muted,
            )
            .into_any_flex(),
            button(
                flex_row((
                    s_icon("close", 14.0, t.text_soft),
                    s_txtw("Close", 10.0, FontWeight::MEDIUM, t.text_soft),
                ))
                .gap(4.0_f64.px())
                .cross_axis_alignment(CrossAxisAlignment::Center),
                |st: &mut StudioState| {
                    st.settings_open = false;
                },
            )
            .padding(Padding::from_vh(3.0, 8.0))
            .corner_radius(5.0)
            .background_color(t.bg_inset)
            .active_background_color(t.press)
            .border(t.border_strong, 1.0)
            .hovered_border_color(t.accent)
            .into_any_flex(),
        ])
        .gap(8.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
    )
    .height(38.0_f64.px())
    .padding(Padding::from_vh(0.0, 12.0))
    .background_color(t.bg_raised)
    .border(t.border, 1.0);

    let dialog_card = sized_box(
        flex_col(vec![
            modal_header.into_any_flex(),
            flex_row(vec![
                sidebar.into_any_flex(),
                portal(pane_body).flex(1.0).into_any_flex(),
            ])
            .gap(0.0_f64.px())
            .cross_axis_alignment(CrossAxisAlignment::Fill)
            .must_fill_major_axis(true)
            .flex(1.0)
            .into_any_flex(),
        ])
        .gap(0.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .width(740.0_f64.px())
    .height(480.0_f64.px())
    .background_color(t.bg_surface)
    .border(t.border_strong, 1.5)
    .corner_radius(12.0);

    sized_box(
        flex_col(vec![
            FlexSpacer::Flex(1.0).into_any_flex(),
            flex_row(vec![
                FlexSpacer::Flex(1.0).into_any_flex(),
                dialog_card.into_any_flex(),
                FlexSpacer::Flex(1.0).into_any_flex(),
            ])
            .into_any_flex(),
            FlexSpacer::Flex(1.0).into_any_flex(),
        ])
        .must_fill_major_axis(true),
    )
    .expand()
    .background_color(t.scrim)
}

fn appearance_tab(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let cur_theme = state.settings.theme_mode;
    let cur_accent = state.settings.accent_preset;
    let cur_density = state.settings.density;
    let anim_speed = f64::from(state.settings.animation_speed);

    let mut theme_btns: Vec<Child> = Vec::new();
    for mode in StudioThemeMode::ALL {
        let on = mode == cur_theme;
        theme_btns.push(
            option_pill(mode.label(), on, t, move |st: &mut StudioState| {
                st.set_theme_mode(mode);
            })
            .flex(1.0)
            .into_any_flex(),
        );
    }

    let mut accent_btns: Vec<Child> = Vec::new();
    for preset in AccentPreset::ALL {
        let on = preset == cur_accent;
        accent_btns.push(
            option_pill(preset.label(), on, t, move |st: &mut StudioState| {
                st.settings.accent_preset = preset;
                st.notify_status(format!("Signal accent set to {}", preset.label()));
            })
            .flex(1.0)
            .into_any_flex(),
        );
    }

    let mut density_btns: Vec<Child> = Vec::new();
    for den in UiDensity::ALL {
        let on = den == cur_density;
        density_btns.push(
            option_pill(den.label(), on, t, move |st: &mut StudioState| {
                st.settings.density = den;
                st.notify_status(format!("UI layout density set to {}", den.label()));
            })
            .flex(1.0)
            .into_any_flex(),
        );
    }

    flex_col(vec![
        setting_group_card(
            "Surface Theme",
            "Calibrated surface hierarchy following the Kine Studio UI Guideline.",
            flex_row(theme_btns).gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "Signal Accent Color",
            "Primary interactive highlight for playhead badges, keyframes, and active controls.",
            flex_row(accent_btns).gap(4.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "Workspace Density & 4px Grid",
            "Adjusts panel widths, track lane heights, inspector row spacing, and cascade indents.",
            flex_row(density_btns).gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "Motion & Cascading Hierarchy",
            "Configure 60Hz ease-out-quint transitions, reduced-motion accessibility, and tree guide lines.",
            flex_col(vec![
                flex_row((
                    s_txt("Transition Speed Multiplier", 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    s_mono(
                        format!("{:.1}×", state.settings.animation_speed),
                        9.5,
                        t.accent_ink,
                    ),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(0.2, 2.5, anim_speed, |st: &mut StudioState, v: f64| {
                    st.settings.animation_speed = v as f32;
                })
                .into_any_flex(),
                flex_row(vec![
                    setting_toggle(
                        "Reduced Motion",
                        state.settings.reduced_motion,
                        t,
                        |st: &mut StudioState| {
                            st.settings.reduced_motion = !st.settings.reduced_motion;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Cascading Tree Guides",
                        state.settings.show_cascade_guides,
                        t,
                        |st: &mut StudioState| {
                            st.settings.show_cascade_guides = !st.settings.show_cascade_guides;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(6.0_f64.px())
                .into_any_flex(),
                flex_row(vec![
                    setting_toggle(
                        "Show Left Navigator Dock",
                        state.settings.left_dock_visible,
                        t,
                        |st: &mut StudioState| {
                            st.settings.left_dock_visible = !st.settings.left_dock_visible;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Show Right Inspector Dock",
                        state.inspector_visible,
                        t,
                        |st: &mut StudioState| {
                            st.inspector_visible = !st.inspector_visible;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Show Telemetry Status Bar",
                        state.settings.show_status_bar,
                        t,
                        |st: &mut StudioState| {
                            st.settings.show_status_bar = !st.settings.show_status_bar;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(6.0_f64.px())
                .into_any_flex(),
            ])
            .gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
    ])
    .gap(8.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::all(12.0))
    .boxed()
}

fn editor_viewport_tab(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let cur_w = state.scene_def.width;
    let cur_h = state.scene_def.height;
    let cur_chan = state.viewport_channel;

    let presets: [(&str, u32, u32); 5] = [
        ("640×360 (nHD)", 640, 360),
        ("1280×720 (HD)", 1280, 720),
        ("1920×1080 (FHD)", 1920, 1080),
        ("1080×1080 (1:1)", 1080, 1080),
        ("1080×1920 (9:16)", 1080, 1920),
    ];
    let mut res_btns: Vec<Child> = Vec::new();
    for (lbl, w, h) in presets {
        let on = cur_w == w && cur_h == h;
        res_btns.push(
            option_pill(lbl, on, t, move |st: &mut StudioState| {
                st.settings.default_canvas_width = w;
                st.settings.default_canvas_height = h;
                let _ = st.resize_canvas(w, h);
            })
            .flex(1.0)
            .into_any_flex(),
        );
    }

    let mut chan_btns: Vec<Child> = Vec::new();
    for mode in ViewportChannelMode::ALL {
        let on = mode == cur_chan;
        chan_btns.push(
            option_pill(mode.badge(), on, t, move |st: &mut StudioState| {
                st.viewport_channel = mode;
                let _ = st.render_current_frame();
            })
            .flex(1.0)
            .into_any_flex(),
        );
    }

    let nudge_step = f64::from(state.settings.nudge_step_px);
    let code_size = f64::from(state.settings.code_font_size);

    flex_col(vec![
        setting_group_card(
            "Composition Canvas Resolution",
            "Immediately resizes the active composition stage and updates default resolution for new scenes.",
            flex_row(res_btns).gap(4.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "Viewport Channel Isolation",
            "Inspect individual color channels, premultiplied alpha matte, or BT.601 luminance.",
            flex_row(chan_btns).gap(4.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "Stage Overlays & Reticles",
            "Toggle broadcast safe margins (5% action / 10% title), rule-of-thirds grid, and selection rings.",
            flex_col(vec![
                flex_row(vec![
                    setting_toggle(
                        "Selection Bounding Box",
                        state.show_bounding_boxes,
                        t,
                        |st: &mut StudioState| {
                            st.show_bounding_boxes = !st.show_bounding_boxes;
                            let _ = st.render_current_frame();
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Topology Lens (Green Ring)",
                        state.topology_lens_active,
                        t,
                        |st: &mut StudioState| {
                            st.topology_lens_active = !st.topology_lens_active;
                            let _ = st.render_current_frame();
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(6.0_f64.px())
                .into_any_flex(),
                flex_row(vec![
                    setting_toggle(
                        "Broadcast Safe Guides",
                        state.show_safe_guides,
                        t,
                        |st: &mut StudioState| {
                            st.show_safe_guides = !st.show_safe_guides;
                            let _ = st.render_current_frame();
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Rule-of-Thirds Grid",
                        state.show_grid_overlay,
                        t,
                        |st: &mut StudioState| {
                            st.show_grid_overlay = !st.show_grid_overlay;
                            let _ = st.render_current_frame();
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Center Crosshair",
                        state.settings.show_center_crosshair,
                        t,
                        |st: &mut StudioState| {
                            st.settings.show_center_crosshair = !st.settings.show_center_crosshair;
                            let _ = st.render_current_frame();
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(6.0_f64.px())
                .into_any_flex(),
            ])
            .gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "Inspector Step & Code Editor",
            "Configure spatial nudge increment and TypeScript/JSON source editor font size.",
            flex_col(vec![
                flex_row((
                    s_txt("Inspector Nudge Step (px)", 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    s_mono(
                        format!("{:.0} px", state.settings.nudge_step_px),
                        9.5,
                        t.accent_ink,
                    ),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(1.0, 64.0, nudge_step, |st: &mut StudioState, v: f64| {
                    st.settings.nudge_step_px = v.round() as f32;
                })
                .into_any_flex(),
                flex_row((
                    s_txt("Code Editor Font Size", 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    s_mono(
                        format!("{:.1} pt", state.settings.code_font_size),
                        9.5,
                        t.accent_ink,
                    ),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(9.0, 16.0, code_size, |st: &mut StudioState, v: f64| {
                    st.settings.code_font_size = v as f32;
                })
                .into_any_flex(),
            ])
            .gap(5.0_f64.px()),
            t,
        )
        .into_any_flex(),
    ])
    .gap(8.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::all(12.0))
    .boxed()
}

fn timeline_playback_tab(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let fps_options: [f64; 4] = [24.0, 30.0, 50.0, 60.0];
    let mut fps_btns: Vec<Child> = Vec::new();
    for f in fps_options {
        let on = (state.fps - f).abs() < 0.1;
        fps_btns.push(
            option_pill(
                match f as u32 {
                    24 => "24 fps (Cinema)",
                    30 => "30 fps (NTSC/Web)",
                    50 => "50 fps (PAL)",
                    _ => "60 fps (High-Rate)",
                },
                on,
                t,
                move |st: &mut StudioState| {
                    st.fps = f;
                    st.settings.default_fps = f;
                    let _ = st.render_current_frame();
                    st.notify_status(format!("Composition frame rate set to {f:.0} fps"));
                },
            )
            .flex(1.0)
            .into_any_flex(),
        );
    }

    let snap_tol = state.settings.snap_tolerance_sec;
    let vol = f64::from(state.audio_volume);

    flex_col(vec![
        setting_group_card(
            "Composition Frame Rate (FPS)",
            "Controls timeline frame quantization, playhead stepping, and video export frame rate.",
            flex_row(fps_btns).gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "NLE Timeline Behavior & Cascading Sub-Tracks",
            "Configure magnetic keyframe snapping, automatic keyframe recording, and ripple time shifting.",
            flex_col(vec![
                flex_row(vec![
                    setting_toggle(
                        "Magnetic Snap (Keyframes & Ticks)",
                        state.timeline_snap_enabled,
                        t,
                        |st: &mut StudioState| {
                            st.timeline_snap_enabled = !st.timeline_snap_enabled;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Auto-Keyframe on Edit",
                        state.auto_keyframe,
                        t,
                        |st: &mut StudioState| {
                            st.auto_keyframe = !st.auto_keyframe;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Ripple Edit Mode",
                        state.ripple_edit,
                        t,
                        |st: &mut StudioState| {
                            st.ripple_edit = !st.ripple_edit;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(6.0_f64.px())
                .into_any_flex(),
                flex_row(vec![
                    setting_toggle(
                        "Show Animated Property Sub-Tracks",
                        state.settings.timeline_show_subtracks,
                        t,
                        |st: &mut StudioState| {
                            st.settings.timeline_show_subtracks =
                                !st.settings.timeline_show_subtracks;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Draw Audio Waveforms in Lanes",
                        state.settings.timeline_show_waveforms,
                        t,
                        |st: &mut StudioState| {
                            st.settings.timeline_show_waveforms =
                                !st.settings.timeline_show_waveforms;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Timecode as Frame Numbers",
                        state.show_frames,
                        t,
                        |st: &mut StudioState| {
                            st.show_frames = !st.show_frames;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(6.0_f64.px())
                .into_any_flex(),
                flex_row((
                    s_txt("Magnetic Snap Tolerance", 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    s_mono(format!("{:.0} ms", snap_tol * 1000.0), 9.5, t.accent_ink),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(0.01, 0.15, snap_tol, |st: &mut StudioState, v: f64| {
                    st.settings.snap_tolerance_sec = v;
                })
                .into_any_flex(),
            ])
            .gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "Audio Monitoring & Peak Meters",
            "Control master monitoring level, mute state, and stereo VU meter strip in the transport bar.",
            flex_col(vec![
                flex_row((
                    s_txt("Master Audio Gain", 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    s_mono(
                        format!("{:.0}%", state.audio_volume * 100.0),
                        9.5,
                        t.accent_ink,
                    ),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(0.0, 1.0, vol, |st: &mut StudioState, v: f64| {
                    st.audio_volume = v as f32;
                })
                .into_any_flex(),
                flex_row(vec![
                    setting_toggle(
                        "Mute Master Audio",
                        state.audio_muted,
                        t,
                        |st: &mut StudioState| {
                            st.audio_muted = !st.audio_muted;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                    setting_toggle(
                        "Show Stereo Peak Meters",
                        state.show_audio_meters,
                        t,
                        |st: &mut StudioState| {
                            st.show_audio_meters = !st.show_audio_meters;
                        },
                    )
                    .flex(1.0)
                    .into_any_flex(),
                ])
                .gap(6.0_f64.px())
                .into_any_flex(),
            ])
            .gap(5.0_f64.px()),
            t,
        )
        .into_any_flex(),
    ])
    .gap(8.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::all(12.0))
    .boxed()
}

fn engine_export_tab(state: &mut StudioState, t: &StudioPalette) -> Boxed {
    let cur_backend = state.engine.backend();
    let gpu_avail = state.engine.gpu_available();
    let quant = f64::from(state.settings.export_quantizer);
    let speed = f64::from(state.settings.export_speed);

    let backend_btns: Vec<Child> = vec![
        option_pill(
            "Auto (GPU Primary)",
            matches!(cur_backend, RenderBackend::Auto),
            t,
            |st: &mut StudioState| {
                st.engine.set_backend(RenderBackend::Auto);
                let _ = st.render_current_frame();
            },
        )
        .flex(1.0)
        .into_any_flex(),
        option_pill(
            "CPU (vello_cpu + WGSL VM)",
            matches!(cur_backend, RenderBackend::Cpu),
            t,
            |st: &mut StudioState| {
                st.engine.set_backend(RenderBackend::Cpu);
                let _ = st.render_current_frame();
            },
        )
        .flex(1.0)
        .into_any_flex(),
        option_pill(
            if gpu_avail {
                "GPU (wgpu + vello_gpu)"
            } else {
                "GPU (Unavailable on Host)"
            },
            matches!(cur_backend, RenderBackend::Gpu),
            t,
            |st: &mut StudioState| {
                if st.engine.gpu_available() {
                    st.engine.set_backend(RenderBackend::Gpu);
                    let _ = st.render_current_frame();
                } else {
                    st.notify_status("Hardware GPU adapter not detected; using deterministic CPU");
                }
            },
        )
        .flex(1.0)
        .into_any_flex(),
    ];

    flex_col(vec![
        setting_group_card(
            "In-Process Rasterization & Shader Backend",
            "Select between hardware wgpu/vello_gpu rasterization and the deterministic vello_cpu + Naga WGSL VM.",
            flex_row(backend_btns).gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
        setting_group_card(
            "AV1 Video Encoder Quality & Speed",
            "Controls rav1e quantizer (lower = higher bitrate/fidelity) and speed preset (0 = slowest, 10 = fastest).",
            flex_col(vec![
                flex_row((
                    s_txt("AV1 Quantizer (Q)", 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    s_mono(
                        format!("Q{}", state.settings.export_quantizer),
                        9.5,
                        t.accent_ink,
                    ),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(20.0, 160.0, quant, |st: &mut StudioState, v: f64| {
                    st.settings.export_quantizer = v.round() as u8;
                })
                .into_any_flex(),
                flex_row((
                    s_txt("Encoder Speed Preset", 10.0, t.text_soft),
                    FlexSpacer::Flex(1.0),
                    s_mono(
                        format!("Speed {}", state.settings.export_speed),
                        9.5,
                        t.accent_ink,
                    ),
                ))
                .cross_axis_alignment(CrossAxisAlignment::Center)
                .into_any_flex(),
                slider(1.0, 10.0, speed, |st: &mut StudioState, v: f64| {
                    st.settings.export_speed = v.round() as u8;
                })
                .into_any_flex(),
            ])
            .gap(6.0_f64.px()),
            t,
        )
        .into_any_flex(),
    ])
    .gap(8.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::all(12.0))
    .boxed()
}

fn shortcuts_tab(t: &StudioPalette) -> Boxed {
    let bindings: [(&str, &str); 10] = [
        ("Play / Pause Transport", "Space"),
        ("Step Backward / Forward 1 Frame", "Left / Right"),
        ("Jump to Start / End of Composition", "Home / End"),
        ("Insert Keyframe at Playhead", "K"),
        ("Split Clip or Track at Playhead", "S"),
        ("Duplicate Selected Scene Node", "Ctrl/Cmd + D"),
        ("Delete Selected Scene Node", "Backspace / Delete"),
        ("Undo / Redo Scene Transaction", "Ctrl/Cmd + Z / Y"),
        ("Masonry Widget Tree Inspector", "F11"),
        ("Masonry Debug Layout Rectangles", "F12"),
    ];
    let mut rows: Vec<Child> = Vec::new();
    for (action, key) in bindings {
        rows.push(
            flex_row((
                s_txt(action, 10.5, t.text_soft),
                FlexSpacer::Flex(1.0),
                sized_box(s_mono(key, 9.5, t.accent_ink))
                    .padding(Padding::from_vh(2.0, 8.0))
                    .background_color(t.bg_raised)
                    .border(t.border_strong, 1.0)
                    .corner_radius(4.0),
            ))
            .cross_axis_alignment(CrossAxisAlignment::Center)
            .padding(Padding::from_vh(5.0, 8.0))
            .background_color(t.bg_inset)
            .border(t.border, 1.0)
            .corner_radius(5.0)
            .into_any_flex(),
        );
    }

    flex_col(vec![setting_group_card(
        "Keyboard Shortcuts & Masonry Diagnostics",
        "Standard NLE transport, keyframe authoring, and Xilem/Masonry runtime inspection keys.",
        flex_col(rows).gap(4.0_f64.px()),
        t,
    )
    .into_any_flex()])
    .gap(8.0_f64.px())
    .cross_axis_alignment(CrossAxisAlignment::Fill)
    .padding(Padding::all(12.0))
    .boxed()
}

fn setting_group_card(
    title: &'static str,
    subtitle: &'static str,
    body: impl WidgetView<StudioState>,
    t: &StudioPalette,
) -> impl WidgetView<StudioState> {
    let t = *t;
    sized_box(
        flex_col((
            s_txtw(title, 11.0, FontWeight::SEMI_BOLD, t.text),
            s_txt(subtitle, 9.5, t.muted),
            body,
        ))
        .gap(6.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Fill),
    )
    .padding(Padding::all(10.0))
    .background_color(t.bg_inset)
    .border(t.border, 1.0)
    .corner_radius(8.0)
}

fn option_pill(
    label_text: &'static str,
    active: bool,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((
            s_icon(
                if active {
                    "radio_button_checked"
                } else {
                    "radio_button_unchecked"
                },
                13.0,
                if active { t.accent } else { t.muted },
            ),
            s_txtw(
                label_text,
                10.0,
                if active {
                    FontWeight::SEMI_BOLD
                } else {
                    FontWeight::NORMAL
                },
                if active { t.text } else { t.text_soft },
            ),
        ))
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(5.0, 8.0))
    .corner_radius(6.0)
    .background_color(if active { t.accent_wash } else { t.bg_surface })
    .active_background_color(t.press)
    .border(if active { t.border_accent } else { t.border }, 1.0)
    .hovered_border_color(t.border_strong)
}

fn setting_toggle(
    label_text: &'static str,
    on: bool,
    t: &StudioPalette,
    cb: impl Fn(&mut StudioState) + Send + Sync + 'static,
) -> impl WidgetView<StudioState> {
    let t = *t;
    button(
        flex_row((
            s_icon(
                if on {
                    "check_box"
                } else {
                    "check_box_outline_blank"
                },
                14.0,
                if on { t.accent } else { t.muted },
            ),
            s_txt(label_text, 10.0, if on { t.text } else { t.text_soft }),
        ))
        .gap(5.0_f64.px())
        .cross_axis_alignment(CrossAxisAlignment::Center),
        cb,
    )
    .padding(Padding::from_vh(5.0, 8.0))
    .corner_radius(6.0)
    .background_color(if on { t.accent_wash } else { t.bg_surface })
    .active_background_color(t.press)
    .border(if on { t.border_accent } else { t.border }, 1.0)
    .hovered_border_color(t.border_strong)
}
