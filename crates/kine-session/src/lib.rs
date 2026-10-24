//! `kine-session` — Headless session state, NLE timeline model, property inspector,
//! live file watcher, and real-time external CLI agent TCP bridge synchronized
//! in-process with [`kine_server::Engine`].
//!
//! Decoupled from `kine-studio` so `kine-cli` and headless agents compile with
//! zero desktop GUI (`xilem` / `winit` / Wayland / X11) dependencies.

pub mod agent_tools;
pub mod bridge;
pub mod catalog;
pub mod inspector;
pub mod state;
pub mod theme;
pub mod timeline;

pub use agent_tools::{
    agent_tools_json_schema, build_element_agent_prompt, dispatch_agent_tool,
    studio_add_animation, studio_add_keyframe, studio_add_node, studio_align_node,
    studio_audio, studio_delete_animation, studio_delete_keyframe, studio_delete_node,
    studio_duplicate_node, studio_frame, studio_inspect, studio_look, studio_reorder_node,
    studio_seek, studio_select, studio_selection, studio_set_shader, studio_set_style,
    studio_set_text, studio_shift_time, studio_split, studio_status, studio_timeline,
    studio_toggle_visibility, studio_transform, studio_transport, studio_tree,
    studio_update_animation, AgentToolResult, TransformPatch,
};
pub use bridge::{
    bridge_call, bridge_watch, discover_bridge_addr, BridgeDiscoveryManifest, BridgeEvent,
    SceneFileWatcher, StudioBridgeServer, BRIDGE_MANIFEST_FILENAME, DEFAULT_BRIDGE_HOST,
    DEFAULT_BRIDGE_PORT,
};
pub use catalog::{
    build_catalog_node, default_assets, default_compositions, starter_kinetic_scene,
    starter_nle_av_scene, starter_shader_scene, AssetEntry, AssetKind, CatalogItem,
    CompositionEntry, CATALOG_ITEMS,
};
pub use inspector::{LayerTreeRow, NodeInspectorSnapshot};
pub use state::{
    LeftDockTab, RenderJobStatus, RenderQueueJob, RightDockTab, StudioError, StudioState, UndoEntry,
};
pub use theme::{
    sample_audio_meter_levels, AccentPreset, DensityMetrics, SettingsCategory, StudioAnimState,
    StudioSettings, StudioThemeMode, UiDensity, ViewportChannelMode, CONTROL_H, HEADER_H,
    PANEL_RADIUS, ROW_H, RULER_H, SHELL_GAP, TAB_H, TRACK_GUTTER, TRACK_H,
};
pub use timeline::{
    generate_ruler_ticks, node_def_badge, node_def_display_label, TimelineKeyframeMarker,
    TimelineTick, TimelineTrack, TimelineTrackKind,
};

#[cfg(test)]
mod tests {
    use super::*;
    use kine_core::NodeId;
    use kine_script::AnimationDef;
    use serde_json::json;

    #[test]
    fn studio_initializes_in_process_and_renders_starter_frame() {
        let state = StudioState::new(None);
        assert_eq!(state.scene_def.width, 640);
        assert_eq!(state.scene_def.height, 360);
        assert_eq!(state.last_raw_pixmap.width(), 640);
        assert_eq!(state.last_raw_pixmap.height(), 360);
        assert!(state.frame_revision >= 1);
        assert!(state.inspect_findings.is_empty());
        assert!(state.dark_theme);
        assert!(state.topology_lens_active);
        assert_eq!(state.settings.theme_mode, StudioThemeMode::GraphiteDark);
        assert_eq!(state.settings.accent_preset, AccentPreset::KineMint);
        assert_eq!(state.settings.density, UiDensity::Comfortable);
    }

    #[test]
    fn node_paths_match_engine_scene_ids_and_support_undo_redo() {
        let mut state = StudioState::new(None);
        let paths = state.node_paths();
        assert_eq!(paths.len(), 7); // #0 root + 6 starter children (#1..=#6)

        // Mutate headline #4 via studio_set_text
        let res = studio_set_text(&mut state, 4, "SYNCED IN PROCESS", Some("Rename headline"));
        assert!(res.ok);
        assert_eq!(res.stage, "verified");
        assert_eq!(
            state.node_def(NodeId(4)).and_then(|d| d.content.as_deref()),
            Some("SYNCED IN PROCESS")
        );
        assert_eq!(state.undo_stack.len(), 1);

        // Undo restores original headline
        let undone = state.undo();
        assert_eq!(undone.as_deref(), Some("Rename headline"));
        assert_eq!(
            state.node_def(NodeId(4)).and_then(|d| d.content.as_deref()),
            Some("KINE STUDIO")
        );

        // Redo reapplies headline
        let redone = state.redo();
        assert_eq!(redone.as_deref(), Some("Rename headline"));
        assert_eq!(
            state.node_def(NodeId(4)).and_then(|d| d.content.as_deref()),
            Some("SYNCED IN PROCESS")
        );
    }

    #[test]
    fn timeline_tracks_keyframes_split_duplicate_and_delete_work() {
        let mut state = StudioState::new(None);
        let initial_tracks = state.build_timeline_tracks();
        assert!(initial_tracks.len() >= 10);

        // Add keyframe at playhead on #2
        state.selected_node = Some(NodeId(2));
        state.active_anim_property = "opacity".to_string();
        state.seek_to(0.45, false);
        state.timeline_add_keyframe_at_playhead().unwrap();

        let kf_times = state.sorted_keyframe_times(Some(NodeId(2)));
        assert!(kf_times.iter().any(|t| (*t - 0.45).abs() < 1e-3));

        // Duplicate #3
        state.selected_node = Some(NodeId(3));
        let dup_id = state.timeline_duplicate_selected().unwrap();
        assert_eq!(state.node_paths().len(), 8);

        // Delete the duplicated node
        state.selected_node = Some(dup_id);
        state.timeline_delete_selected().unwrap();
        assert_eq!(state.node_paths().len(), 7);
    }

    #[test]
    fn cascading_tree_collapse_and_multi_section_inspector_work() {
        let mut state = StudioState::new(None);
        assert_eq!(state.build_layer_tree_rows().len(), 7);

        // Collapsing root #0 hides its 6 children in the layer tree & timeline
        state.toggle_node_collapsed(NodeId(0));
        assert_eq!(state.build_layer_tree_rows().len(), 1);

        // Expanding root #0 restores all children
        state.toggle_node_collapsed(NodeId(0));
        assert_eq!(state.build_layer_tree_rows().len(), 7);

        // Multi-open inspector sections
        assert!(state.is_inspector_section_open("layout"));
        assert!(state.is_inspector_section_open("style"));
        state.toggle_inspector_section("layout");
        assert!(!state.is_inspector_section_open("layout"));
        state.set_all_inspector_sections(true);
        assert!(state.is_inspector_section_open("stroke"));
    }

    #[test]
    fn settings_persistence_asset_insertion_and_animation_tick_work() {
        let root = std::env::temp_dir().join(format!("kine-session-settings-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let mut state = StudioState::new(Some(root.clone()));
        state.set_theme_mode(StudioThemeMode::CarbonObsidian);
        state.settings.accent_preset = AccentPreset::CinemaAmber;
        state.settings.density = UiDensity::Compact;
        let saved_path = state.save_settings_to_disk().unwrap();
        assert!(saved_path.exists());

        // Reloading from workspace restores saved settings
        let reloaded = StudioState::new(Some(root.clone()));
        assert_eq!(reloaded.settings.theme_mode, StudioThemeMode::CarbonObsidian);
        assert_eq!(reloaded.settings.accent_preset, AccentPreset::CinemaAmber);
        assert_eq!(reloaded.settings.density, UiDensity::Compact);

        // Clicking an image asset inserts a node into the scene
        let before_nodes = state.node_paths().len();
        state.insert_or_apply_asset(1).unwrap(); // pattern:checkerboard
        assert_eq!(state.node_paths().len(), before_nodes + 1);

        // UI animation interpolation progresses smoothly
        state.settings_open = true;
        state.tick_studio(1.0 / 60.0);
        assert!(state.anim.settings_modal_t > 0.0);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn agent_tools_transform_style_animation_and_prompt_v1_verify_in_process() {
        let mut state = StudioState::new(None);

        let look = studio_look(&mut state);
        assert!(look.ok);
        assert!(look
            .payload
            .get("dataUrl")
            .and_then(|v| v.as_str())
            .is_some_and(|s| s.starts_with("data:image/png;base64,")));

        let tr = studio_transform(
            &mut state,
            2,
            TransformPatch {
                x: Some(80.0),
                y: Some(72.0),
                width: Some(480.0),
                height: Some(200.0),
                ..Default::default()
            },
            Some("Resize hero card"),
        );
        assert!(tr.ok);
        let bounds = state.node_abs_rect(NodeId(2)).unwrap();
        assert!((bounds.width - 480.0).abs() < 1e-3);
        assert!((bounds.height - 200.0).abs() < 1e-3);

        let anim = studio_add_animation(
            &mut state,
            3,
            "rotation",
            AnimationDef {
                from: 0.0,
                to: 180.0,
                start: 0.0,
                end: 1.5,
                keyframes: None,
                easing: Some("easeInOut".to_string()),
            },
            None,
        );
        assert!(anim.ok);

        let kf = studio_add_keyframe(&mut state, 3, "rotation", 0.75, 90.0, Some("linear"), None);
        assert!(kf.ok);

        let prompt = build_element_agent_prompt(&state, NodeId(2), "Add a subtle cyan glow");
        assert!(prompt.contains("Kine Studio element edit request v1"));
        assert!(prompt.contains("Add a subtle cyan glow"));
    }

    #[test]
    fn live_tcp_bridge_and_file_watcher_synchronize_external_cli_agent_edits() {
        let root = std::env::temp_dir().join(format!("kine-session-bridge-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();

        let mut state = StudioState::new(Some(root.clone()));
        let addr = state.start_bridge_server(Some("127.0.0.1:0")).unwrap();

        // Spawn a background client thread that talks to Studio over TCP while the main thread polls
        let addr_client = addr.clone();
        let client_handle = std::thread::spawn(move || {
            let status = bridge_call(&addr_client, "studio_status", json!({})).unwrap();
            assert!(status.ok);

            let set_txt = bridge_call(
                &addr_client,
                "studio_set_text",
                json!({ "nodeId": 4, "text": "EXTERNAL CLI AGENT" }),
            )
            .unwrap();
            assert!(set_txt.ok);

            let add_rect = bridge_call(
                &addr_client,
                "studio_add_node",
                json!({
                    "kind": "rect",
                    "x": 120.0,
                    "y": 80.0,
                    "width": 200.0,
                    "height": 90.0,
                    "fill": "#00e3ff"
                }),
            )
            .unwrap();
            assert!(add_rect.ok);
        });

        // Poll bridge on main studio state until client finishes
        for _ in 0..50 {
            state.poll_bridge_and_watcher();
            if client_handle.is_finished() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        client_handle.join().unwrap();

        assert_eq!(
            state.node_def(NodeId(4)).and_then(|d| d.content.as_deref()),
            Some("EXTERNAL CLI AGENT")
        );
        assert_eq!(state.node_paths().len(), 8);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn studio_loads_a_module_scene_and_inserts_a_module() {
        let root = std::env::temp_dir().join(format!("kine-session-modules-{}", std::process::id()));
        let module_dir = root.join("Modules").join("badge");
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&module_dir).unwrap();
        std::fs::write(
            module_dir.join("module.json"),
            r#"{"kind":"widget","params":{"title":"DEFAULT"}}"#,
        )
        .unwrap();
        std::fs::write(
            module_dir.join("scene.ts"),
            r#"return (props) => text(props.title || "NONE", { fontSize: 12 });"#,
        )
        .unwrap();
        std::fs::write(
            root.join("scene.ts"),
            r#"return scene(64, 64, container({ size: { width: 64, height: 64 } }, [
                module("badge", { title: "LIVE" })
            ]));"#,
        )
        .unwrap();

        let mut state = StudioState::new(Some(root.clone()));
        state.load_scene_file(&root.join("scene.ts")).unwrap();
        let before = state.scene_def.root.children.as_ref().map_or(0, Vec::len);
        assert!(before > 0, "module scene must produce children");

        state.insert_module("badge").unwrap();
        let after = state.scene_def.root.children.as_ref().map_or(0, Vec::len);
        assert_eq!(after, before + 1);
        assert_eq!(state.module_registry.len(), 1);

        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn audio_meter_sampling_respects_mute_and_volume() {
        let (l, r) = sample_audio_meter_levels(true, false, 0.85, 0.5);
        assert!(l > 0.0 && r > 0.0);
        let (ml, mr) = sample_audio_meter_levels(true, true, 0.85, 0.5);
        assert_eq!((ml, mr), (0.0, 0.0));
    }
}
