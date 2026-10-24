//! `kine-studio` — Native Rust **Xilem** visual studio, NLE timeline, node
//! inspector, and AI agent loop synchronized in-process with the Kine engine
//! across **Linux**, **Windows**, and **macOS**.

pub mod agent_tools;
pub mod catalog;
pub mod chat;
pub mod inspector;
pub mod state;
pub mod theme;
pub mod timeline;
pub mod ui;

pub use agent_tools::{
    agent_tools_json_schema, build_element_agent_prompt, dispatch_agent_tool, studio_add_animation,
    studio_add_keyframe, studio_delete_animation, studio_frame, studio_inspect, studio_look,
    studio_seek, studio_select, studio_set_style, studio_set_text, studio_transform,
    studio_update_animation, AgentToolResult, TransformPatch,
};
pub use catalog::{
    build_catalog_node, default_assets, default_compositions, starter_kinetic_scene,
    starter_nle_av_scene, starter_shader_scene, AssetEntry, AssetKind, CatalogItem,
    CompositionEntry, CATALOG_ITEMS,
};
pub use chat::{AgentChatMessage, AgentChatRole, StudioChatSession};
pub use inspector::{LayerTreeRow, NodeInspectorSnapshot};
pub use state::{
    LeftDockTab, RenderJobStatus, RenderQueueJob, RightDockTab, StudioError, StudioState, UndoEntry,
};
pub use theme::sample_audio_meter_levels;
#[cfg(feature = "xilem-ui")]
pub use theme::StudioPalette;
pub use timeline::{
    generate_ruler_ticks, node_def_badge, node_def_display_label, TimelineKeyframeMarker,
    TimelineTick, TimelineTrack, TimelineTrackKind,
};
#[cfg(feature = "xilem-ui")]
pub use ui::{app_logic, run_studio};

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
    fn chat_natural_language_and_json_tool_calls_mutate_scene() {
        let mut state = StudioState::new(None);
        state.selected_node = Some(NodeId(4));

        let receipts = state.submit_chat_prompt("set text to AI DRIVEN KINE");
        assert!(!receipts.is_empty());
        assert_eq!(
            state.node_def(NodeId(4)).and_then(|d| d.content.as_deref()),
            Some("AI DRIVEN KINE")
        );

        let json_call = json!({
            "tool": "studio_seek",
            "args": { "time": 1.25 }
        })
        .to_string();
        let seek_receipts = state.submit_chat_prompt(&json_call);
        assert_eq!(seek_receipts.len(), 1);
        assert!((state.current_time - 1.25).abs() < 1e-4);
    }

    #[test]
    fn audio_meter_sampling_respects_mute_and_volume() {
        let (l, r) = sample_audio_meter_levels(true, false, 0.85, 0.5);
        assert!(l > 0.0 && r > 0.0);
        let (ml, mr) = sample_audio_meter_levels(true, true, 0.85, 0.5);
        assert_eq!((ml, mr), (0.0, 0.0));
    }
}
