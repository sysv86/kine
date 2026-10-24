//! External CLI Agent Tool Suite (`studio_*`) and Element Context Builder
//! (`build_element_agent_prompt`) for Kine Studio.
//!
//! Instead of embedding an in-app LLM chat wrapper, Kine Studio exposes its
//! entire in-process [`StudioState`] and [`kine_server::Engine`] to external
//! CLI agents (Claude Code / Opus 5.5, Codex / GPT-6 Astra, Gemini CLI / Argon 4,
//! OpenCode, etc.) via a real-time TCP bridge and CLI subcommands (`kine agent ...`).
//! Every mutating tool checkpoints `StudioState::undo_stack`, synchronizes the
//! in-process engine, and performs read-back verification (`stage: "verified"`).

use crate::state::{node_at_path_mut, StudioError, StudioState};
use crate::timeline::{node_def_badge, node_def_display_label};
use kine_core::NodeId;
use kine_render::FrameFormat;
use kine_script::{
    AnimationDef, AudioDef, AudioTrackDef, EdgesDef, GradientDef, KeyframeDef, NodeDef, SceneDef,
    SizeDef,
};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;
use std::path::Path;

/// Structured receipt returned by every `studio_*` tool.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentToolResult {
    pub ok: bool,
    /// `"verified"`, `"saved"`, or `"refused"`.
    pub stage: &'static str,
    pub tool: &'static str,
    pub summary: String,
    pub payload: Value,
}

impl AgentToolResult {
    pub fn verified(tool: &'static str, summary: impl Into<String>, payload: Value) -> Self {
        Self {
            ok: true,
            stage: "verified",
            tool,
            summary: summary.into(),
            payload,
        }
    }

    pub fn saved(tool: &'static str, summary: impl Into<String>, payload: Value) -> Self {
        Self {
            ok: true,
            stage: "saved",
            tool,
            summary: summary.into(),
            payload,
        }
    }

    pub fn refused(tool: &'static str, reason: impl Into<String>) -> Self {
        let msg = reason.into();
        Self {
            ok: false,
            stage: "refused",
            tool,
            summary: msg.clone(),
            payload: json!({ "error": msg }),
        }
    }
}

/// Geometry & 2D transform patch for `studio_transform` (supports both absolute values and relative nudges).
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TransformPatch {
    pub x: Option<f32>,
    pub y: Option<f32>,
    pub width: Option<f32>,
    pub height: Option<f32>,
    pub translate_x: Option<f32>,
    pub translate_y: Option<f32>,
    pub scale: Option<f32>,
    pub scale_x: Option<f32>,
    pub scale_y: Option<f32>,
    pub rotation: Option<f32>,
    /// Relative position delta (`dx`, `dy`) in pixels.
    pub dx: Option<f32>,
    pub dy: Option<f32>,
    /// Relative size delta (`dw`, `dh`) in pixels.
    pub dw: Option<f32>,
    pub dh: Option<f32>,
    /// Relative translation delta (`dtx`, `dty`) in pixels.
    pub dtx: Option<f32>,
    pub dty: Option<f32>,
    /// Relative uniform scale delta.
    pub dscale: Option<f32>,
    /// Relative rotation delta in degrees.
    pub drot: Option<f32>,
}

/// `studio_status` / `studio_context` — Complete live Studio snapshot for external CLI agents.
pub fn studio_status(state: &StudioState) -> AgentToolResult {
    let active_comp = state.compositions.get(state.active_composition);
    let comp_list: Vec<Value> = state
        .compositions
        .iter()
        .enumerate()
        .map(|(idx, c)| {
            json!({
                "index": idx,
                "id": c.id,
                "title": c.title,
                "subtitle": c.subtitle,
                "sourcePath": c.source_path,
                "active": idx == state.active_composition,
            })
        })
        .collect();

    let selected_snap = state.inspect_selected_node();
    let node_count = state.node_paths().len();

    AgentToolResult::verified(
        "studio_status",
        format!(
            "Studio active · {} ({}x{} @ {:.0}fps) · playhead {:.2}s (F{}/{}) · {} nodes · selected {:?}",
            active_comp.map(|c| c.title.as_str()).unwrap_or("Composition"),
            state.scene_def.width,
            state.scene_def.height,
            state.fps,
            state.current_time,
            state.current_frame_index(),
            state.total_frames(),
            node_count,
            state.selected_node.map(|n| n.0),
        ),
        json!({
            "workspaceRoot": state.workspace_root,
            "activeCompositionIndex": state.active_composition,
            "activeComposition": active_comp.map(|c| json!({
                "id": c.id,
                "title": c.title,
                "subtitle": c.subtitle,
                "sourcePath": c.source_path,
            })),
            "compositions": comp_list,
            "width": state.scene_def.width,
            "height": state.scene_def.height,
            "fps": state.fps,
            "duration": state.duration,
            "currentTime": state.current_time,
            "frame": state.current_frame_index(),
            "totalFrames": state.total_frames(),
            "timecode": state.formatted_timecode(),
            "isPlaying": state.is_playing,
            "loopEnabled": state.loop_enabled,
            "playbackRate": state.playback_rate,
            "audioMuted": state.audio_muted,
            "audioVolume": state.audio_volume,
            "backend": state.engine.backend().name(),
            "gpuAvailable": state.engine.gpu_available(),
            "lastRenderMs": state.last_render_ms,
            "frameRevision": state.frame_revision,
            "nodeCount": node_count,
            "selectedNode": state.selected_node.map(|n| n.0),
            "selectedInspector": selected_snap,
            "undoDepth": state.undo_stack.len(),
            "redoDepth": state.redo_stack.len(),
            "findingsCount": state.inspect_findings.len(),
            "findings": state.inspect_findings,
            "bridgeAddress": state.bridge_listen_addr(),
            "watchedFile": state.watched_scene_path(),
        }),
    )
}

/// `studio_selection` — Read the currently selected node in Studio along with its structured edit context.
pub fn studio_selection(state: &StudioState, custom_request: Option<&str>) -> AgentToolResult {
    let Some(node_id) = state.selected_node else {
        return AgentToolResult::verified(
            "studio_selection",
            "No node currently selected in Studio",
            json!({
                "selectedNode": null,
                "currentTime": state.current_time,
                "frame": state.current_frame_index(),
            }),
        );
    };

    let Some(def) = state.node_def(node_id).cloned() else {
        return AgentToolResult::refused(
            "studio_selection",
            format!("selected node {node_id} no longer exists in scene"),
        );
    };

    let rect = state.node_abs_rect(node_id);
    let path = state.path_for_node(node_id).unwrap_or_default();
    let badge = node_def_badge(&def);
    let label = node_def_display_label(node_id, &def);
    let inspector = state.inspect_selected_node();
    let prompt = build_element_agent_prompt(state, node_id, custom_request.unwrap_or(""));

    AgentToolResult::verified(
        "studio_selection",
        format!("Selected {node_id} [{badge}] ({label}) @ {:.2}s", state.current_time),
        json!({
            "selectedNode": node_id.0,
            "badge": badge,
            "label": label,
            "path": path,
            "bounds": rect,
            "currentTime": state.current_time,
            "frame": state.current_frame_index(),
            "inspector": inspector,
            "authored": def,
            "agentContextPrompt": prompt,
        }),
    )
}

/// 1. `studio_look` — Capture the current rendered frame (`PNG` base64), scene tree, and inspection report.
pub fn studio_look(state: &mut StudioState) -> AgentToolResult {
    let frame_idx = u64::from(state.current_frame_index());
    let b64 = match state.engine.render_frame_png_b64(state.scene_handle, frame_idx) {
        Ok(s) => s,
        Err(e) => return AgentToolResult::refused("studio_look", e.to_string()),
    };

    let nodes: Vec<Value> = state
        .node_paths()
        .into_iter()
        .filter_map(|(id, path)| {
            let def = state.node_def(id)?;
            let rect = state.node_abs_rect(id);
            Some(json!({
                "id": id.0,
                "depth": path.len(),
                "path": path,
                "kind": node_def_badge(def),
                "label": node_def_display_label(id, def),
                "bounds": rect.map(|r| json!({
                    "x": r.x, "y": r.y, "width": r.width, "height": r.height
                })),
            }))
        })
        .collect();

    AgentToolResult::verified(
        "studio_look",
        format!(
            "Captured frame {} ({:.2}s) · {}x{} · {} nodes",
            frame_idx,
            state.current_time,
            state.scene_def.width,
            state.scene_def.height,
            nodes.len()
        ),
        json!({
            "width": state.scene_def.width,
            "height": state.scene_def.height,
            "currentTime": state.current_time,
            "frame": frame_idx,
            "fps": state.fps,
            "duration": state.duration,
            "selectedNode": state.selected_node.map(|n| n.0),
            "findingsCount": state.inspect_findings.len(),
            "nodes": nodes,
            "dataUrl": format!("data:image/png;base64,{b64}"),
        }),
    )
}

/// `studio_tree` — Return the hierarchical scene layer tree with bounds, visibility, and animation counts.
pub fn studio_tree(state: &StudioState) -> AgentToolResult {
    let rows = state.build_layer_tree_rows();
    let detailed: Vec<Value> = state
        .node_paths()
        .into_iter()
        .filter_map(|(id, path)| {
            let def = state.node_def(id)?;
            let rect = state.node_abs_rect(id);
            let visible = !def.opacity.is_some_and(|o| o <= 0.001);
            let anim_props: Vec<String> = def
                .animate
                .as_ref()
                .map(|m| m.keys().cloned().collect())
                .unwrap_or_default();
            Some(json!({
                "nodeId": id.0,
                "depth": path.len(),
                "path": path,
                "badge": node_def_badge(def),
                "kind": def.kind.as_deref().unwrap_or("container"),
                "label": node_def_display_label(id, def),
                "visible": visible,
                "selected": state.selected_node == Some(id),
                "bounds": rect,
                "animatedProperties": anim_props,
            }))
        })
        .collect();

    AgentToolResult::verified(
        "studio_tree",
        format!("Scene hierarchy: {} total nodes ({} visible rows)", detailed.len(), rows.len()),
        json!({
            "totalNodes": detailed.len(),
            "selectedNode": state.selected_node.map(|n| n.0),
            "nodes": detailed,
            "layerRows": rows,
        }),
    )
}

/// `studio_timeline` — Return all NLE timeline tracks, property sub-lanes, keyframes, and audio tracks.
pub fn studio_timeline(state: &StudioState) -> AgentToolResult {
    let tracks: Vec<Value> = state
        .build_timeline_tracks()
        .into_iter()
        .map(|t| {
            let kind_json = match t.kind {
                crate::timeline::TimelineTrackKind::SceneNode {
                    node_id,
                    depth,
                    kind_badge,
                } => json!({
                    "type": "sceneNode",
                    "nodeId": node_id.0,
                    "depth": depth,
                    "badge": kind_badge,
                }),
                crate::timeline::TimelineTrackKind::AnimationProperty {
                    node_id,
                    property,
                    easing,
                } => json!({
                    "type": "animationProperty",
                    "nodeId": node_id.0,
                    "property": property,
                    "easing": easing,
                }),
                crate::timeline::TimelineTrackKind::AudioTrack { track_index } => json!({
                    "type": "audioTrack",
                    "trackIndex": track_index,
                }),
            };
            let kfs: Vec<Value> = t
                .keyframes
                .into_iter()
                .map(|k| {
                    json!({
                        "nodeId": k.node_id.0,
                        "property": k.property,
                        "time": k.time,
                        "value": k.value,
                    })
                })
                .collect();
            json!({
                "kind": kind_json,
                "label": t.label,
                "subtitle": t.subtitle,
                "startSec": t.start_sec,
                "endSec": t.end_sec,
                "selected": t.selected,
                "mutedOrHidden": t.muted_or_hidden,
                "keyframes": kfs,
            })
        })
        .collect();

    let all_keyframe_times = state.sorted_keyframe_times(None);
    AgentToolResult::verified(
        "studio_timeline",
        format!(
            "Timeline: {} track lanes · {:.2}s duration @ {:.0}fps · {} keyframe timestamps",
            tracks.len(),
            state.duration,
            state.fps,
            all_keyframe_times.len()
        ),
        json!({
            "duration": state.duration,
            "fps": state.fps,
            "currentTime": state.current_time,
            "frame": state.current_frame_index(),
            "totalFrames": state.total_frames(),
            "keyframeTimes": all_keyframe_times,
            "tracks": tracks,
        }),
    )
}

/// 2. `studio_select` — Select or clear a scene node (`NodeId`).
pub fn studio_select(state: &mut StudioState, node_id: Option<u64>) -> AgentToolResult {
    let Some(raw_id) = node_id else {
        state.selected_node = None;
        let _ = state.render_current_frame();
        state.notify_status("CLI Bridge: cleared node selection");
        return AgentToolResult::verified(
            "studio_select",
            "Cleared node selection",
            json!({ "selectedNode": null }),
        );
    };

    let target = NodeId(raw_id);
    if state.node_def(target).is_none() {
        return AgentToolResult::refused(
            "studio_select",
            format!("node #{raw_id} does not exist in the active scene"),
        );
    }

    state.selected_node = Some(target);
    let _ = state.render_current_frame();
    let snap = state.inspect_selected_node();
    state.notify_status(format!("CLI Bridge: selected node #{raw_id}"));
    AgentToolResult::verified(
        "studio_select",
        format!("Selected node #{raw_id}"),
        json!({
            "selectedNode": raw_id,
            "inspector": snap,
        }),
    )
}

/// 3. `studio_seek` — Seek the in-process playhead to `time_seconds` and rasterize immediately.
pub fn studio_seek(state: &mut StudioState, time_seconds: f64) -> AgentToolResult {
    if !time_seconds.is_finite() || time_seconds < 0.0 {
        return AgentToolResult::refused(
            "studio_seek",
            "time_seconds must be a non-negative number",
        );
    }
    state.seek_to(time_seconds, false);
    state.notify_status(format!(
        "CLI Bridge: seeked to {:.2}s (F{})",
        state.current_time,
        state.current_frame_index()
    ));
    AgentToolResult::verified(
        "studio_seek",
        format!(
            "Seeked playhead to {:.3}s (frame {})",
            state.current_time,
            state.current_frame_index()
        ),
        json!({
            "currentTime": state.current_time,
            "frame": state.current_frame_index(),
            "duration": state.duration,
        }),
    )
}

/// 4. `studio_frame` — Seek to an exact `frame_index` (`frame_index / fps`).
pub fn studio_frame(state: &mut StudioState, frame_index: u32) -> AgentToolResult {
    let time = f64::from(frame_index) / state.fps.max(1.0);
    state.seek_to(time, false);
    state.notify_status(format!(
        "CLI Bridge: stepped to frame {} ({:.2}s)",
        state.current_frame_index(),
        state.current_time
    ));
    AgentToolResult::verified(
        "studio_frame",
        format!(
            "Stepped to frame {} ({:.3}s)",
            state.current_frame_index(),
            state.current_time
        ),
        json!({
            "frame": state.current_frame_index(),
            "currentTime": state.current_time,
            "totalFrames": state.total_frames(),
        }),
    )
}

/// `studio_transport` — Control live Studio playback, rate, looping, FPS, audio monitoring, and timeline flags.
pub fn studio_transport(state: &mut StudioState, args: &Value) -> AgentToolResult {
    if let Some(action) = args.get("action").and_then(Value::as_str) {
        match action.to_ascii_lowercase().as_str() {
            "play" => state.is_playing = true,
            "pause" => state.is_playing = false,
            "toggle" => state.is_playing = !state.is_playing,
            "stop" => {
                state.is_playing = false;
                state.seek_to(0.0, false);
            }
            "step" => {
                let delta = args
                    .get("deltaFrames")
                    .or_else(|| args.get("delta"))
                    .and_then(Value::as_i64)
                    .unwrap_or(1) as i32;
                state.step_frames(delta);
            }
            "prev_keyframe" | "prev-keyframe" => state.timeline_jump_prev_keyframe(),
            "next_keyframe" | "next-keyframe" => state.timeline_jump_next_keyframe(),
            other => {
                return AgentToolResult::refused(
                    "studio_transport",
                    format!("unknown transport action: {other}"),
                )
            }
        }
    }

    if let Some(lp) = args.get("loop").and_then(Value::as_bool) {
        state.loop_enabled = lp;
    }
    if let Some(rate) = args.get("rate").or_else(|| args.get("playbackRate")).and_then(Value::as_f64) {
        state.playback_rate = rate.clamp(0.1, 4.0);
    }
    if let Some(fps) = args.get("fps").and_then(Value::as_f64) {
        state.fps = fps.clamp(1.0, 240.0);
        state.settings.default_fps = state.fps;
    }
    if let Some(muted) = args.get("muted").and_then(Value::as_bool) {
        state.audio_muted = muted;
    }
    if let Some(vol) = args.get("volume").and_then(Value::as_f64) {
        state.audio_volume = (vol as f32).clamp(0.0, 1.0);
    }
    if let Some(snap) = args.get("snap").and_then(Value::as_bool) {
        state.timeline_snap_enabled = snap;
    }
    if let Some(akf) = args.get("autoKeyframe").and_then(Value::as_bool) {
        state.auto_keyframe = akf;
    }
    if let Some(ripple) = args.get("ripple").or_else(|| args.get("rippleEdit")).and_then(Value::as_bool) {
        state.ripple_edit = ripple;
    }

    let _ = state.render_current_frame();
    state.notify_status(format!(
        "CLI Bridge: transport {} @ {:.2}s ({:.0}fps, {:.2}x)",
        if state.is_playing { "PLAYING" } else { "PAUSED" },
        state.current_time,
        state.fps,
        state.playback_rate
    ));

    AgentToolResult::verified(
        "studio_transport",
        format!(
            "Transport {} · {:.2}s (F{}/{}) · {:.0}fps · rate {:.2}x · loop={}",
            if state.is_playing { "playing" } else { "paused" },
            state.current_time,
            state.current_frame_index(),
            state.total_frames(),
            state.fps,
            state.playback_rate,
            state.loop_enabled
        ),
        json!({
            "isPlaying": state.is_playing,
            "currentTime": state.current_time,
            "frame": state.current_frame_index(),
            "totalFrames": state.total_frames(),
            "fps": state.fps,
            "playbackRate": state.playback_rate,
            "loopEnabled": state.loop_enabled,
            "audioMuted": state.audio_muted,
            "audioVolume": state.audio_volume,
            "snap": state.timeline_snap_enabled,
            "autoKeyframe": state.auto_keyframe,
            "rippleEdit": state.ripple_edit,
        }),
    )
}

/// 5. `studio_inspect` — Inspect a specific node (or the active selection) and return static findings.
pub fn studio_inspect(state: &mut StudioState, node_id: Option<u64>) -> AgentToolResult {
    let target = node_id
        .map(NodeId)
        .or(state.selected_node)
        .unwrap_or(NodeId(0));

    let Some(def) = state.node_def(target).cloned() else {
        return AgentToolResult::refused(
            "studio_inspect",
            format!("node {target} not found in scene"),
        );
    };
    let rect = state.node_abs_rect(target);
    let path = state.path_for_node(target);
    let report = state.engine.inspect(state.scene_handle).ok();

    let prev_sel = state.selected_node;
    state.selected_node = Some(target);
    let inspector_snap = state.inspect_selected_node();
    state.selected_node = prev_sel;

    AgentToolResult::verified(
        "studio_inspect",
        format!("Inspected node {target}"),
        json!({
            "nodeId": target.0,
            "path": path,
            "bounds": rect,
            "inspector": inspector_snap,
            "authored": def,
            "findings": report.map(|r| r.findings).unwrap_or_default(),
        }),
    )
}

/// `studio_add_node` — Insert a new node, catalog block, or project module into the scene hierarchy.
pub fn studio_add_node(state: &mut StudioState, args: &Value) -> AgentToolResult {
    // 1. Check if a catalog preset ID was requested (`catalogId` or `preset` starting with catalog prefix)
    if let Some(cat_id) = args
        .get("catalogId")
        .or_else(|| args.get("catalog_id"))
        .and_then(Value::as_str)
    {
        match state.insert_catalog_item(cat_id) {
            Ok(new_id) => {
                let bounds = state.node_abs_rect(new_id);
                return AgentToolResult::verified(
                    "studio_add_node",
                    format!("Inserted catalog preset '{cat_id}' as {new_id}"),
                    json!({
                        "nodeId": new_id.0,
                        "catalogId": cat_id,
                        "bounds": bounds,
                    }),
                );
            }
            Err(e) => return AgentToolResult::refused("studio_add_node", e.to_string()),
        }
    }

    // 2. Check if a project module was requested
    if let Some(mod_name) = args.get("module").and_then(Value::as_str) {
        let props = args.get("props").cloned().unwrap_or_else(|| json!({}));
        let node = match state.module_registry.instantiate(mod_name, props) {
            Ok(n) => n,
            Err(e) => return AgentToolResult::refused("studio_add_node", e.to_string()),
        };
        let parent_id = NodeId(
            args.get("parentId")
                .or_else(|| args.get("parent_id"))
                .and_then(Value::as_u64)
                .unwrap_or(0),
        );
        let Some(parent_path) = state.path_for_node(parent_id) else {
            return AgentToolResult::refused(
                "studio_add_node",
                format!("parent node {parent_id} not found"),
            );
        };
        let label = format!("Agent: insert module '{mod_name}' under {parent_id}");
        if let Err(e) = state.commit_mutation(label, move |scene_def| {
            let parent = node_at_path_mut(&mut scene_def.root, &parent_path)
                .ok_or(StudioError::NodeNotFound(parent_id))?;
            parent.children.get_or_insert_with(Vec::new).push(node);
            Ok(())
        }) {
            return AgentToolResult::refused("studio_add_node", e.to_string());
        }
        let new_id = state
            .node_paths()
            .last()
            .map(|(id, _)| *id)
            .unwrap_or(NodeId(0));
        state.selected_node = Some(new_id);
        let _ = state.render_current_frame();
        return AgentToolResult::verified(
            "studio_add_node",
            format!("Inserted module '{mod_name}' as {new_id}"),
            json!({
                "nodeId": new_id.0,
                "module": mod_name,
                "bounds": state.node_abs_rect(new_id),
            }),
        );
    }

    // 3. Construct `NodeDef` from `args.node` or shorthand properties
    let mut node_def: NodeDef = if let Some(node_obj) = args.get("node") {
        match serde_json::from_value(node_obj.clone()) {
            Ok(d) => d,
            Err(e) => {
                return AgentToolResult::refused(
                    "studio_add_node",
                    format!("invalid 'node' definition: {e}"),
                )
            }
        }
    } else {
        let kind_raw = args
            .get("kind")
            .and_then(Value::as_str)
            .unwrap_or("rect")
            .trim();
        if let Some(cat_id) = kind_raw.strip_prefix("catalog:") {
            return studio_add_node(state, &json!({ "catalogId": cat_id }));
        }
        let kind = kind_raw.to_ascii_lowercase();
        let mut def = NodeDef {
            kind: Some(kind.clone()),
            ..Default::default()
        };

        let x = args.get("x").and_then(Value::as_f64).map(|v| v as f32);
        let y = args.get("y").and_then(Value::as_f64).map(|v| v as f32);
        if x.is_some() || y.is_some() {
            def.position = Some("absolute".to_string());
            def.inset = Some(EdgesDef {
                left: Some(x.unwrap_or(48.0)),
                top: Some(y.unwrap_or(48.0)),
                ..Default::default()
            });
        }

        let w = args.get("width").and_then(Value::as_f64).map(|v| v as f32);
        let h = args.get("height").and_then(Value::as_f64).map(|v| v as f32);
        if w.is_some() || h.is_some() {
            def.size = Some(SizeDef {
                width: w.or(Some(160.0)),
                height: h.or(Some(96.0)),
            });
        } else if matches!(kind.as_str(), "rect" | "container" | "image" | "video") {
            def.size = Some(SizeDef {
                width: Some(180.0),
                height: Some(100.0),
            });
        } else if kind == "circle" {
            let r = args
                .get("radius")
                .and_then(Value::as_f64)
                .map(|v| v as f32)
                .unwrap_or(48.0);
            def.radius = Some(r);
            def.size = Some(SizeDef {
                width: Some(r * 2.0),
                height: Some(r * 2.0),
            });
        }

        if let Some(fill) = args.get("fill").and_then(Value::as_str) {
            def.fill = Some(fill.to_string());
        } else if matches!(kind.as_str(), "rect" | "circle") {
            def.fill = Some("#3de4a8".to_string());
        }

        if let Some(color) = args.get("color").and_then(Value::as_str) {
            def.color = Some(color.to_string());
        } else if kind == "text" {
            def.color = Some("#f5f3f1".to_string());
        }

        if let Some(text) = args
            .get("text")
            .or_else(|| args.get("content"))
            .and_then(Value::as_str)
        {
            def.content = Some(text.to_string());
        } else if kind == "text" {
            def.content = Some("NEW TEXT".to_string());
        }

        if let Some(fs) = args.get("fontSize").and_then(Value::as_f64) {
            def.font_size = Some(fs as f32);
        } else if kind == "text" {
            def.font_size = Some(28.0);
        }

        if let Some(fw) = args.get("fontWeight").and_then(Value::as_u64) {
            def.font_weight = Some(fw as u16);
        }
        if let Some(align) = args.get("align").and_then(Value::as_str) {
            def.align = Some(align.to_string());
        }
        if let Some(cr) = args.get("cornerRadius").and_then(Value::as_f64) {
            def.corner_radius = Some(cr as f32);
        }
        if let Some(r) = args.get("radius").and_then(Value::as_f64) {
            def.radius = Some(r as f32);
        }
        if let Some(op) = args.get("opacity").and_then(Value::as_f64) {
            def.opacity = Some((op as f32).clamp(0.0, 1.0));
        }
        if let Some(src) = args.get("src").and_then(Value::as_str) {
            def.src = Some(src.to_string());
        } else if kind == "image" {
            def.src = Some("pattern:checkerboard".to_string());
        } else if kind == "video" {
            def.src = Some("video:testbars".to_string());
        }
        if let Some(d) = args.get("d").and_then(Value::as_str) {
            def.d = Some(d.to_string());
        } else if kind == "path" {
            def.d = Some("M 0 0 L 120 0 L 60 100 Z".to_string());
        }
        if let Some(sh) = args
            .get("shader")
            .or_else(|| args.get("preset"))
            .and_then(Value::as_str)
        {
            def.shader = Some(sh.to_string());
        } else if kind == "shader" {
            def.shader = Some("preset:film-grain".to_string());
        }
        if let Some(blend) = args.get("blend").and_then(Value::as_str) {
            def.blend = Some(blend.to_string());
        }
        if let Some(u) = args.get("uniforms").and_then(Value::as_object) {
            def.uniforms = Some(u.clone());
        }
        def
    };

    if node_def.kind.is_none() {
        node_def.kind = Some("rect".to_string());
    }

    let parent_id = NodeId(
        args.get("parentId")
            .or_else(|| args.get("parent_id"))
            .and_then(Value::as_u64)
            .unwrap_or(0),
    );
    let Some(parent_path) = state.path_for_node(parent_id) else {
        return AgentToolResult::refused(
            "studio_add_node",
            format!("parent node {parent_id} not found in scene"),
        );
    };
    let insert_idx = args.get("index").and_then(Value::as_u64).map(|i| i as usize);
    let kind_label = node_def.kind.clone().unwrap_or_else(|| "node".to_string());
    let undo_label = args
        .get("undoLabel")
        .and_then(Value::as_str)
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: add {kind_label} under {parent_id}"));

    let parent_path_clone = parent_path.clone();
    let mut inserted_child_idx = 0usize;
    let node_to_insert = node_def.clone();

    if let Err(e) = state.commit_mutation(undo_label, |scene_def| {
        let parent = node_at_path_mut(&mut scene_def.root, &parent_path_clone)
            .ok_or(StudioError::NodeNotFound(parent_id))?;
        let children = parent.children.get_or_insert_with(Vec::new);
        let idx = insert_idx.unwrap_or(children.len()).min(children.len());
        inserted_child_idx = idx;
        children.insert(idx, node_to_insert);
        Ok(())
    }) {
        return AgentToolResult::refused("studio_add_node", e.to_string());
    }

    let mut target_path = parent_path;
    target_path.push(inserted_child_idx);
    let new_id = state
        .node_paths()
        .into_iter()
        .find(|(_, p)| *p == target_path)
        .map(|(id, _)| id)
        .unwrap_or(NodeId(0));

    state.selected_node = Some(new_id);
    let _ = state.render_current_frame();
    let bounds = state.node_abs_rect(new_id);

    AgentToolResult::verified(
        "studio_add_node",
        format!("Added {kind_label} node as {new_id} under {parent_id}"),
        json!({
            "nodeId": new_id.0,
            "parentId": parent_id.0,
            "path": target_path,
            "bounds": bounds,
            "node": state.node_def(new_id),
        }),
    )
}

/// `studio_delete_node` — Delete a non-root node from the scene hierarchy.
pub fn studio_delete_node(
    state: &mut StudioState,
    node_id: u64,
    undo_label: Option<&str>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    if target == NodeId(0) {
        return AgentToolResult::refused(
            "studio_delete_node",
            "cannot delete root container node #0",
        );
    }
    let Some(path) = state.path_for_node(target) else {
        return AgentToolResult::refused("studio_delete_node", format!("node #{node_id} not found"));
    };
    let Some((&child_idx, parent_path)) = path.split_last() else {
        return AgentToolResult::refused("studio_delete_node", "invalid root path");
    };
    let parent_path = parent_path.to_vec();
    let label = undo_label
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: delete node #{node_id}"));

    if let Err(e) = state.commit_mutation(label, move |scene_def| {
        let parent = node_at_path_mut(&mut scene_def.root, &parent_path)
            .ok_or(StudioError::NodeNotFound(target))?;
        let children = parent
            .children
            .as_mut()
            .ok_or(StudioError::NodeNotFound(target))?;
        if child_idx >= children.len() {
            return Err(StudioError::NodeNotFound(target));
        }
        children.remove(child_idx);
        Ok(())
    }) {
        return AgentToolResult::refused("studio_delete_node", e.to_string());
    }

    if state.selected_node == Some(target) {
        state.selected_node = Some(NodeId(0));
    }
    let _ = state.render_current_frame();

    AgentToolResult::verified(
        "studio_delete_node",
        format!("Deleted node #{node_id} ({} nodes remaining)", state.node_paths().len()),
        json!({
            "deletedNodeId": node_id,
            "remainingNodes": state.node_paths().len(),
        }),
    )
}

/// `studio_duplicate_node` — Duplicate `node_id` as a sibling and select the copy.
pub fn studio_duplicate_node(state: &mut StudioState, node_id: u64) -> AgentToolResult {
    let target = NodeId(node_id);
    if target == NodeId(0) {
        return AgentToolResult::refused(
            "studio_duplicate_node",
            "cannot duplicate root container node #0",
        );
    }
    state.selected_node = Some(target);
    match state.timeline_duplicate_selected() {
        Ok(new_id) => AgentToolResult::verified(
            "studio_duplicate_node",
            format!("Duplicated #{node_id} -> {new_id}"),
            json!({
                "sourceNodeId": node_id,
                "newNodeId": new_id.0,
                "bounds": state.node_abs_rect(new_id),
            }),
        ),
        Err(e) => AgentToolResult::refused("studio_duplicate_node", e.to_string()),
    }
}

/// `studio_reorder_node` — Move `node_id` up (`delta < 0`) or down (`delta > 0`) in sibling Z-order.
pub fn studio_reorder_node(
    state: &mut StudioState,
    node_id: u64,
    delta: isize,
) -> AgentToolResult {
    let target = NodeId(node_id);
    match state.layer_move_z_order(target, delta) {
        Ok(()) => AgentToolResult::verified(
            "studio_reorder_node",
            format!("Reordered #{node_id} by {delta:+} -> path {:?}", state.path_for_node(target)),
            json!({
                "nodeId": node_id,
                "delta": delta,
                "newPath": state.path_for_node(target),
            }),
        ),
        Err(e) => AgentToolResult::refused("studio_reorder_node", e.to_string()),
    }
}

/// `studio_toggle_visibility` — Toggle or explicitly set visibility on `node_id`.
pub fn studio_toggle_visibility(
    state: &mut StudioState,
    node_id: u64,
    visible: Option<bool>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    if state.node_def(target).is_none() {
        return AgentToolResult::refused(
            "studio_toggle_visibility",
            format!("node #{node_id} not found"),
        );
    }
    let res = if let Some(vis) = visible {
        let next_op = if vis { 1.0 } else { 0.0 };
        state.mutate_node(
            target,
            format!("Agent: {} #{node_id}", if vis { "show" } else { "hide" }),
            move |def| {
                def.opacity = Some(next_op);
                Ok(())
            },
        )
    } else {
        state.layer_toggle_visibility(target)
    };

    match res {
        Ok(()) => {
            let now_vis = state
                .node_def(target)
                .map_or(true, |d| !d.opacity.is_some_and(|o| o <= 0.001));
            AgentToolResult::verified(
                "studio_toggle_visibility",
                format!("Node #{node_id} visibility is now {now_vis}"),
                json!({
                    "nodeId": node_id,
                    "visible": now_vis,
                }),
            )
        }
        Err(e) => AgentToolResult::refused("studio_toggle_visibility", e.to_string()),
    }
}

/// `studio_align_node` — Align `node_id` within the composition canvas.
pub fn studio_align_node(
    state: &mut StudioState,
    node_id: u64,
    horizontal: Option<&str>,
    vertical: Option<&str>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    if state.node_def(target).is_none() {
        return AgentToolResult::refused("studio_align_node", format!("node #{node_id} not found"));
    }
    let prev = state.selected_node;
    state.selected_node = Some(target);
    let res = state.inspector_align_selected(horizontal, vertical);
    state.selected_node = prev;
    match res {
        Ok(()) => {
            let bounds = state.node_abs_rect(target);
            AgentToolResult::verified(
                "studio_align_node",
                format!(
                    "Aligned #{node_id} (h={:?}, v={:?}) -> {:?}",
                    horizontal, vertical, bounds
                ),
                json!({
                    "nodeId": node_id,
                    "horizontal": horizontal,
                    "vertical": vertical,
                    "bounds": bounds,
                }),
            )
        }
        Err(e) => AgentToolResult::refused("studio_align_node", e.to_string()),
    }
}

/// 6. `studio_set_text` — Update a `Text` node's content and verify shaped output in-process.
pub fn studio_set_text(
    state: &mut StudioState,
    node_id: u64,
    text: &str,
    undo_label: Option<&str>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    let Some(existing) = state.node_def(target) else {
        return AgentToolResult::refused("studio_set_text", format!("node #{node_id} not found"));
    };
    if existing.kind.as_deref().unwrap_or("container") != "text" {
        return AgentToolResult::refused(
            "studio_set_text",
            format!("node #{node_id} is not a text node"),
        );
    }

    let label = undo_label
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: set text on #{node_id}"));
    let new_text = text.to_string();

    if let Err(e) = state.mutate_node(target, label, move |def| {
        def.content = Some(new_text);
        Ok(())
    }) {
        return AgentToolResult::refused("studio_set_text", e.to_string());
    }

    let verified_text = state
        .node_def(target)
        .and_then(|d| d.content.clone())
        .unwrap_or_default();
    let bounds = state.node_abs_rect(target);

    AgentToolResult::verified(
        "studio_set_text",
        format!("Updated text on #{node_id} to {verified_text:?}"),
        json!({
            "nodeId": node_id,
            "content": verified_text,
            "bounds": bounds,
        }),
    )
}

/// 7. `studio_set_style` — Mutate visual/style/layout properties on `node_id` and verify in-process.
pub fn studio_set_style(
    state: &mut StudioState,
    node_id: u64,
    changes: &Map<String, Value>,
    undo_label: Option<&str>,
) -> AgentToolResult {
    if changes.is_empty() {
        return AgentToolResult::refused("studio_set_style", "changes map cannot be empty");
    }
    let target = NodeId(node_id);
    if state.node_def(target).is_none() {
        return AgentToolResult::refused("studio_set_style", format!("node #{node_id} not found"));
    }

    let label = undo_label
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: update styles on #{node_id}"));
    let patch = changes.clone();

    if let Err(e) = state.mutate_node(target, label, move |def| {
        for (k, v) in &patch {
            match k.as_str() {
                "fill" | "backgroundColor" => {
                    if let Some(s) = v.as_str() {
                        def.fill = Some(s.to_string());
                    }
                }
                "color" => {
                    if let Some(s) = v.as_str() {
                        if def.kind.as_deref() == Some("text") {
                            def.color = Some(s.to_string());
                        } else {
                            def.fill = Some(s.to_string());
                        }
                    }
                }
                "opacity" => {
                    if let Some(n) = v.as_f64() {
                        def.opacity = Some((n as f32).clamp(0.0, 1.0));
                    }
                }
                "cornerRadius" | "borderRadius" => {
                    if let Some(n) = v.as_f64() {
                        def.corner_radius = Some((n as f32).max(0.0));
                    }
                }
                "radius" => {
                    if let Some(n) = v.as_f64() {
                        def.radius = Some((n as f32).max(1.0));
                    }
                }
                "strokeColor" | "borderColor" => {
                    if let Some(s) = v.as_str() {
                        def.stroke_color = Some(s.to_string());
                    }
                }
                "strokeWidth" | "borderWidth" => {
                    if let Some(n) = v.as_f64() {
                        def.stroke_width = Some((n as f32).max(0.0));
                    }
                }
                "shadowColor" => {
                    if let Some(s) = v.as_str() {
                        def.shadow_color = Some(s.to_string());
                    }
                }
                "shadowBlur" => {
                    if let Some(n) = v.as_f64() {
                        def.shadow_blur = Some((n as f32).max(0.0));
                    }
                }
                "shadowOffsetX" => {
                    if let Some(n) = v.as_f64() {
                        def.shadow_offset_x = Some(n as f32);
                    }
                }
                "shadowOffsetY" => {
                    if let Some(n) = v.as_f64() {
                        def.shadow_offset_y = Some(n as f32);
                    }
                }
                "fontFamily" => {
                    if let Some(s) = v.as_str() {
                        def.font_family = Some(s.to_string());
                    }
                }
                "fontSize" => {
                    if let Some(n) = v.as_f64() {
                        def.font_size = Some((n as f32).max(4.0));
                    }
                }
                "fontWeight" => {
                    if let Some(n) = v.as_u64() {
                        def.font_weight = Some(n as u16);
                    }
                }
                "align" | "textAlign" => {
                    if let Some(s) = v.as_str() {
                        def.align = Some(s.to_string());
                    }
                }
                "blend" | "mixBlendMode" => {
                    if let Some(s) = v.as_str() {
                        def.blend = Some(s.to_string());
                    }
                }
                "objectFit" => {
                    if let Some(s) = v.as_str() {
                        def.object_fit = Some(s.to_string());
                    }
                }
                "overflow" => {
                    if let Some(s) = v.as_str() {
                        def.overflow = Some(s.to_string());
                    }
                }
                "flexDirection" => {
                    if let Some(s) = v.as_str() {
                        def.flex_direction = Some(s.to_string());
                    }
                }
                "justifyContent" => {
                    if let Some(s) = v.as_str() {
                        def.justify_content = Some(s.to_string());
                    }
                }
                "alignItems" => {
                    if let Some(s) = v.as_str() {
                        def.align_items = Some(s.to_string());
                    }
                }
                "gap" => {
                    if let Some(n) = v.as_f64() {
                        def.gap = Some((n as f32).max(0.0));
                    }
                }
                "gradient" => {
                    if let Ok(g) = serde_json::from_value::<GradientDef>(v.clone()) {
                        def.gradient = Some(g);
                    }
                }
                _ => {}
            }
        }
        Ok(())
    }) {
        return AgentToolResult::refused("studio_set_style", e.to_string());
    }

    let updated = state.node_def(target).cloned();
    AgentToolResult::verified(
        "studio_set_style",
        format!("Verified {} style change(s) on #{node_id}", changes.len()),
        json!({
            "nodeId": node_id,
            "appliedKeys": changes.keys().collect::<Vec<_>>(),
            "node": updated,
            "bounds": state.node_abs_rect(target),
        }),
    )
}

/// 8. `studio_transform` — Mutate layout bounds (`x`, `y`, `width`, `height`) or 2D transform
/// (`translate_x`, `translate_y`, `scale`, `rotation`) or relative deltas and verify read-back `abs_pos`.
pub fn studio_transform(
    state: &mut StudioState,
    node_id: u64,
    patch: TransformPatch,
    undo_label: Option<&str>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    if state.node_def(target).is_none() {
        return AgentToolResult::refused("studio_transform", format!("node #{node_id} not found"));
    }

    let before_rect = state
        .node_abs_rect(target)
        .unwrap_or_else(|| kine_core::Rect::new(0.0, 0.0, 100.0, 100.0));
    let label = undo_label
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: transform #{node_id}"));
    let p = patch.clone();

    if let Err(e) = state.mutate_node(target, label, move |def| {
        if p.x.is_some() || p.y.is_some() || p.dx.is_some() || p.dy.is_some() {
            if def.position.is_none() {
                def.position = Some("absolute".to_string());
            }
            let inset = def.inset.get_or_insert_with(EdgesDef::default);
            if let Some(x) = p.x {
                inset.left = Some(x);
            } else if let Some(dx) = p.dx {
                inset.left = Some(inset.left.unwrap_or(before_rect.x) + dx);
            }
            if let Some(y) = p.y {
                inset.top = Some(y);
            } else if let Some(dy) = p.dy {
                inset.top = Some(inset.top.unwrap_or(before_rect.y) + dy);
            }
        }
        if p.width.is_some() || p.height.is_some() || p.dw.is_some() || p.dh.is_some() {
            let size = def.size.get_or_insert_with(SizeDef::default);
            if let Some(w) = p.width {
                size.width = Some(w.max(1.0));
            } else if let Some(dw) = p.dw {
                size.width = Some((size.width.unwrap_or(before_rect.width) + dw).max(1.0));
            }
            if let Some(h) = p.height {
                size.height = Some(h.max(1.0));
            } else if let Some(dh) = p.dh {
                size.height = Some((size.height.unwrap_or(before_rect.height) + dh).max(1.0));
            }
        }
        if let Some(tx) = p.translate_x {
            def.translate_x = Some(tx);
        } else if let Some(dtx) = p.dtx {
            def.translate_x = Some(def.translate_x.unwrap_or(0.0) + dtx);
        }
        if let Some(ty) = p.translate_y {
            def.translate_y = Some(ty);
        } else if let Some(dty) = p.dty {
            def.translate_y = Some(def.translate_y.unwrap_or(0.0) + dty);
        }
        if let Some(s) = p.scale {
            def.scale = Some(s);
            def.scale_x = Some(s);
            def.scale_y = Some(s);
        } else if let Some(ds) = p.dscale {
            let next = (def.scale.unwrap_or(1.0) + ds).max(0.01);
            def.scale = Some(next);
            def.scale_x = Some(next);
            def.scale_y = Some(next);
        }
        if let Some(sx) = p.scale_x {
            def.scale_x = Some(sx);
        }
        if let Some(sy) = p.scale_y {
            def.scale_y = Some(sy);
        }
        if let Some(rot) = p.rotation {
            def.rotation = Some(rot);
        } else if let Some(drot) = p.drot {
            def.rotation = Some(def.rotation.unwrap_or(0.0) + drot);
        }
        Ok(())
    }) {
        return AgentToolResult::refused("studio_transform", e.to_string());
    }

    let after_rect = state.node_abs_rect(target);
    AgentToolResult::verified(
        "studio_transform",
        format!("Transformed #{node_id} -> bounds {after_rect:?}"),
        json!({
            "nodeId": node_id,
            "beforeBounds": before_rect,
            "afterBounds": after_rect,
            "patch": patch,
        }),
    )
}

/// `studio_set_shader` — Set or update WGSL shader source/preset, blend mode, and uniforms on `node_id`.
pub fn studio_set_shader(state: &mut StudioState, node_id: u64, args: &Value) -> AgentToolResult {
    let target = NodeId(node_id);
    if state.node_def(target).is_none() {
        return AgentToolResult::refused("studio_set_shader", format!("node #{node_id} not found"));
    }

    let shader_src = args
        .get("shader")
        .or_else(|| args.get("preset"))
        .or_else(|| args.get("source"))
        .and_then(Value::as_str)
        .map(|s| {
            if !s.contains('@') && !s.starts_with("preset:") {
                format!("preset:{s}")
            } else {
                s.to_string()
            }
        });

    if let Some(ref src) = shader_src {
        if let Err(e) = kine_shader::validate(src) {
            return AgentToolResult::refused(
                "studio_set_shader",
                format!("WGSL validation failed: {}", e.diagnostic()),
            );
        }
    }

    let blend_opt = args.get("blend").and_then(Value::as_str).map(str::to_string);
    let uniforms_opt = args.get("uniforms").and_then(Value::as_object).cloned();

    let src_clone = shader_src.clone();
    let blend_clone = blend_opt.clone();
    let uni_clone = uniforms_opt.clone();

    if let Err(e) = state.mutate_node(
        target,
        format!("Agent: update WGSL shader on #{node_id}"),
        move |def| {
            def.kind = Some("shader".to_string());
            if let Some(s) = src_clone {
                def.shader = Some(s);
            } else if def.shader.is_none() {
                def.shader = Some("preset:film-grain".to_string());
            }
            if let Some(b) = blend_clone {
                def.blend = Some(b);
            }
            if let Some(u) = uni_clone {
                let map = def.uniforms.get_or_insert_with(Map::new);
                for (k, v) in u {
                    map.insert(k, v);
                }
            }
            Ok(())
        },
    ) {
        return AgentToolResult::refused("studio_set_shader", e.to_string());
    }

    let updated = state.node_def(target).cloned();
    AgentToolResult::verified(
        "studio_set_shader",
        format!("Verified WGSL shader on #{node_id}"),
        json!({
            "nodeId": node_id,
            "node": updated,
        }),
    )
}

/// `studio_audio` — Add, update, or remove DSP audio tracks on the composition.
pub fn studio_audio(state: &mut StudioState, args: &Value) -> AgentToolResult {
    let action = args
        .get("action")
        .and_then(Value::as_str)
        .unwrap_or("add")
        .to_ascii_lowercase();

    match action.as_str() {
        "add" => {
            let src = args
                .get("src")
                .and_then(Value::as_str)
                .unwrap_or("synth:sine:440:2.0:0.3")
                .to_string();
            let gain_db = args.get("gainDb").and_then(Value::as_f64).unwrap_or(-4.0) as f32;
            let pan = args.get("pan").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let start = args.get("start").and_then(Value::as_f64).unwrap_or(0.0);
            let trim_start = args.get("trimStart").and_then(Value::as_f64).unwrap_or(0.0);
            let trim_end = args.get("trimEnd").and_then(Value::as_f64);
            let fade_in = args.get("fadeIn").and_then(Value::as_f64).unwrap_or(0.1);
            let fade_out = args.get("fadeOut").and_then(Value::as_f64).unwrap_or(0.2);
            let highpass_hz = args.get("highpassHz").and_then(Value::as_f64).map(|v| v as f32);
            let lowpass_hz = args.get("lowpassHz").and_then(Value::as_f64).map(|v| v as f32);
            let duck_db = args.get("duckDb").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let muted = args.get("muted").and_then(Value::as_bool).unwrap_or(false);
            let loop_track = args.get("loop").and_then(Value::as_bool).unwrap_or(false);

            let src_clone = src.clone();
            if let Err(e) = state.commit_mutation(format!("Agent: add audio track {src}"), move |def| {
                let audio = def.audio.get_or_insert_with(|| AudioDef {
                    sample_rate: 48_000,
                    channels: 2,
                    normalize_peak_db: Some(-1.0),
                    tracks: Vec::new(),
                });
                audio.tracks.push(AudioTrackDef {
                    src: src_clone,
                    gain_db,
                    start,
                    trim_start,
                    trim_end,
                    loop_track,
                    pan,
                    fade_in,
                    fade_out,
                    fade_curve: Some("equalPower".to_string()),
                    highpass_hz,
                    lowpass_hz,
                    eq: None,
                    compressor: None,
                    duck_db,
                    duck_threshold_db: None,
                    muted,
                });
                Ok(())
            }) {
                return AgentToolResult::refused("studio_audio", e.to_string());
            }
            let count = state
                .scene_def
                .audio
                .as_ref()
                .map_or(0, |a| a.tracks.len());
            AgentToolResult::verified(
                "studio_audio",
                format!("Added audio track #{}: {}", count.saturating_sub(1), src),
                json!({
                    "trackIndex": count.saturating_sub(1),
                    "totalTracks": count,
                    "audio": state.scene_def.audio,
                }),
            )
        }
        "update" => {
            let idx = args
                .get("trackIndex")
                .or_else(|| args.get("index"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let args_clone = args.clone();
            if let Err(e) =
                state.commit_mutation(format!("Agent: update audio track #{idx}"), move |def| {
                    let audio = def.audio.as_mut().ok_or_else(|| {
                        StudioError::Message("scene has no audio tracks".to_string())
                    })?;
                    let tr = audio.tracks.get_mut(idx).ok_or_else(|| {
                        StudioError::Message(format!("audio track #{idx} out of bounds"))
                    })?;
                    if let Some(s) = args_clone.get("src").and_then(Value::as_str) {
                        tr.src = s.to_string();
                    }
                    if let Some(g) = args_clone.get("gainDb").and_then(Value::as_f64) {
                        tr.gain_db = (g as f32).clamp(-60.0, 18.0);
                    }
                    if let Some(p) = args_clone.get("pan").and_then(Value::as_f64) {
                        tr.pan = (p as f32).clamp(-1.0, 1.0);
                    }
                    if let Some(st) = args_clone.get("start").and_then(Value::as_f64) {
                        tr.start = st.max(0.0);
                    }
                    if let Some(te) = args_clone.get("trimEnd").and_then(Value::as_f64) {
                        tr.trim_end = Some(te.max(0.0));
                    }
                    if let Some(fi) = args_clone.get("fadeIn").and_then(Value::as_f64) {
                        tr.fade_in = fi.max(0.0);
                    }
                    if let Some(fo) = args_clone.get("fadeOut").and_then(Value::as_f64) {
                        tr.fade_out = fo.max(0.0);
                    }
                    if let Some(m) = args_clone.get("muted").and_then(Value::as_bool) {
                        tr.muted = m;
                    }
                    Ok(())
                })
            {
                return AgentToolResult::refused("studio_audio", e.to_string());
            }
            AgentToolResult::verified(
                "studio_audio",
                format!("Updated audio track #{idx}"),
                json!({
                    "trackIndex": idx,
                    "audio": state.scene_def.audio,
                }),
            )
        }
        "remove" | "delete" => {
            let idx = args
                .get("trackIndex")
                .or_else(|| args.get("index"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            if let Err(e) =
                state.commit_mutation(format!("Agent: remove audio track #{idx}"), move |def| {
                    let Some(audio) = &mut def.audio else {
                        return Err(StudioError::Message("scene has no audio tracks".to_string()));
                    };
                    if idx >= audio.tracks.len() {
                        return Err(StudioError::Message(format!(
                            "audio track #{idx} out of bounds"
                        )));
                    }
                    audio.tracks.remove(idx);
                    Ok(())
                })
            {
                return AgentToolResult::refused("studio_audio", e.to_string());
            }
            AgentToolResult::verified(
                "studio_audio",
                format!("Removed audio track #{idx}"),
                json!({ "removedTrackIndex": idx, "audio": state.scene_def.audio }),
            )
        }
        "clear" => {
            if let Err(e) = state.commit_mutation("Agent: clear audio tracks", |def| {
                def.audio = None;
                Ok(())
            }) {
                return AgentToolResult::refused("studio_audio", e.to_string());
            }
            AgentToolResult::verified("studio_audio", "Cleared all audio tracks", json!({}))
        }
        other => AgentToolResult::refused(
            "studio_audio",
            format!("unknown audio action: {other} (expected add, update, remove, clear)"),
        ),
    }
}

/// 9. `studio_add_animation` — Add an animation track to `node_id` and verify evaluation.
pub fn studio_add_animation(
    state: &mut StudioState,
    node_id: u64,
    property: &str,
    spec: AnimationDef,
    undo_label: Option<&str>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    if kine_core::AnimProperty::parse(property).is_none() {
        return AgentToolResult::refused(
            "studio_add_animation",
            format!("unsupported animatable property {property:?}"),
        );
    }

    let prop_name = property.to_string();
    let label = undo_label
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: add animation {prop_name} on #{node_id}"));
    let spec_clone = spec.clone();

    if let Err(e) = state.mutate_node(target, label, move |def| {
        let anim_map = def.animate.get_or_insert_with(BTreeMap::new);
        anim_map.insert(prop_name, spec_clone);
        Ok(())
    }) {
        return AgentToolResult::refused("studio_add_animation", e.to_string());
    }

    AgentToolResult::verified(
        "studio_add_animation",
        format!(
            "Added animation track '{property}' ({:.2}s..{:.2}s) on #{node_id}",
            spec.start, spec.end
        ),
        json!({
            "nodeId": node_id,
            "property": property,
            "animation": spec,
            "compositionDuration": state.duration,
        }),
    )
}

/// 10. `studio_update_animation` — Update an existing animation track on `node_id`.
pub fn studio_update_animation(
    state: &mut StudioState,
    node_id: u64,
    property: &str,
    spec: AnimationDef,
    undo_label: Option<&str>,
) -> AgentToolResult {
    studio_add_animation(
        state,
        node_id,
        property,
        spec,
        undo_label.or(Some("Agent: update animation track")),
    )
}

/// 11. `studio_add_keyframe` — Insert or update a keyframe at `time` on `node_id.property`.
pub fn studio_add_keyframe(
    state: &mut StudioState,
    node_id: u64,
    property: &str,
    time: f64,
    value: f32,
    easing: Option<&str>,
    undo_label: Option<&str>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    if kine_core::AnimProperty::parse(property).is_none() {
        return AgentToolResult::refused(
            "studio_add_keyframe",
            format!("unsupported animatable property {property:?}"),
        );
    }
    if !time.is_finite() || time < 0.0 {
        return AgentToolResult::refused("studio_add_keyframe", "time must be non-negative");
    }

    let prop_name = property.to_string();
    let ease_opt = easing.map(str::to_string);
    let label = undo_label.map(str::to_string).unwrap_or_else(|| {
        format!("Agent: keyframe {prop_name}={value:.2} @ {time:.2}s on #{node_id}")
    });

    if let Err(e) = state.mutate_node(target, label, move |def| {
        let anim_map = def.animate.get_or_insert_with(BTreeMap::new);
        let entry = anim_map.entry(prop_name).or_insert_with(|| AnimationDef {
            from: value,
            to: value,
            start: 0.0,
            end: (time + 1.0).max(1.0),
            keyframes: Some(vec![
                KeyframeDef { time: 0.0, value },
                KeyframeDef {
                    time: (time + 1.0).max(1.0),
                    value,
                },
            ]),
            easing: ease_opt.clone().or_else(|| Some("easeOut".to_string())),
        });

        if let Some(e_str) = ease_opt {
            entry.easing = Some(e_str);
        }

        let mut kfs = entry.keyframes.take().unwrap_or_else(|| {
            vec![
                KeyframeDef {
                    time: entry.start,
                    value: entry.from,
                },
                KeyframeDef {
                    time: entry.end.max(entry.start + 0.1),
                    value: entry.to,
                },
            ]
        });

        if let Some(existing) = kfs.iter_mut().find(|k| (k.time - time).abs() < 1e-3) {
            existing.value = value;
        } else {
            kfs.push(KeyframeDef { time, value });
        }
        kfs.sort_by(|a, b| a.time.total_cmp(&b.time));
        if kfs.len() < 2 {
            kfs.push(KeyframeDef {
                time: (time + 0.5).max(0.5),
                value,
            });
        }
        entry.start = kfs.first().map_or(0.0, |k| k.time);
        entry.end = kfs.last().map_or(1.0, |k| k.time);
        entry.keyframes = Some(kfs);
        Ok(())
    }) {
        return AgentToolResult::refused("studio_add_keyframe", e.to_string());
    }

    AgentToolResult::verified(
        "studio_add_keyframe",
        format!("Verified keyframe on #{node_id}.{property} @ {time:.2}s = {value:.2}"),
        json!({
            "nodeId": node_id,
            "property": property,
            "time": time,
            "value": value,
        }),
    )
}

/// `studio_delete_keyframe` — Delete a keyframe near `time` on `node_id.property`.
pub fn studio_delete_keyframe(
    state: &mut StudioState,
    node_id: u64,
    property: &str,
    time: f64,
) -> AgentToolResult {
    let target = NodeId(node_id);
    let prop_name = property.to_string();
    if let Err(e) = state.mutate_node(
        target,
        format!("Agent: delete keyframe on #{node_id}.{property} @ {time:.2}s"),
        move |def| {
            let Some(anim_map) = &mut def.animate else {
                return Err(StudioError::Message("node has no animations".to_string()));
            };
            let Some(spec) = anim_map.get_mut(&prop_name) else {
                return Err(StudioError::Message(format!(
                    "no animation track for '{prop_name}'"
                )));
            };
            if let Some(kfs) = &mut spec.keyframes {
                kfs.retain(|k| (k.time - time).abs() > 1e-2);
                if kfs.len() < 2 {
                    anim_map.remove(&prop_name);
                } else {
                    spec.start = kfs.first().map_or(0.0, |k| k.time);
                    spec.end = kfs.last().map_or(1.0, |k| k.time);
                }
            } else {
                anim_map.remove(&prop_name);
            }
            Ok(())
        },
    ) {
        return AgentToolResult::refused("studio_delete_keyframe", e.to_string());
    }

    AgentToolResult::verified(
        "studio_delete_keyframe",
        format!("Deleted keyframe on #{node_id}.{property} @ {time:.2}s"),
        json!({
            "nodeId": node_id,
            "property": property,
            "time": time,
        }),
    )
}

/// 12. `studio_delete_animation` — Remove an animation track from `node_id`.
pub fn studio_delete_animation(
    state: &mut StudioState,
    node_id: u64,
    property: &str,
    undo_label: Option<&str>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    let prop_name = property.to_string();
    let label = undo_label
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: delete animation {prop_name} on #{node_id}"));

    if let Err(e) = state.mutate_node(target, label, move |def| {
        if let Some(anim_map) = &mut def.animate {
            anim_map.remove(&prop_name);
        }
        Ok(())
    }) {
        return AgentToolResult::refused("studio_delete_animation", e.to_string());
    }

    AgentToolResult::verified(
        "studio_delete_animation",
        format!("Deleted animation track '{property}' on #{node_id}"),
        json!({
            "nodeId": node_id,
            "deletedProperty": property,
        }),
    )
}

/// `studio_shift_time` — Shift node timing by `delta_sec` (with optional `ripple` edit).
pub fn studio_shift_time(
    state: &mut StudioState,
    node_id: u64,
    delta_sec: f64,
    ripple: Option<bool>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    let prev_sel = state.selected_node;
    let prev_ripple = state.ripple_edit;
    state.selected_node = Some(target);
    if let Some(r) = ripple {
        state.ripple_edit = r;
    }
    let res = state.timeline_shift_selected(delta_sec);
    state.selected_node = prev_sel;
    state.ripple_edit = prev_ripple;

    match res {
        Ok(()) => AgentToolResult::verified(
            "studio_shift_time",
            format!("Shifted #{node_id} timing by {delta_sec:+.2}s"),
            json!({
                "nodeId": node_id,
                "deltaSec": delta_sec,
                "ripple": ripple.unwrap_or(prev_ripple),
                "duration": state.duration,
            }),
        ),
        Err(e) => AgentToolResult::refused("studio_shift_time", e.to_string()),
    }
}

/// `studio_split` — Split clip or insert midpoint keyframe on `node_id` at `time_sec` (or playhead).
pub fn studio_split(
    state: &mut StudioState,
    node_id: u64,
    time_sec: Option<f64>,
) -> AgentToolResult {
    let target = NodeId(node_id);
    if let Some(t) = time_sec {
        state.seek_to(t, false);
    }
    let prev_sel = state.selected_node;
    state.selected_node = Some(target);
    let res = state.timeline_split_at_playhead();
    state.selected_node = prev_sel;
    match res {
        Ok(()) => AgentToolResult::verified(
            "studio_split",
            format!("Split #{node_id} @ {:.2}s", state.current_time),
            json!({
                "nodeId": node_id,
                "splitTime": state.current_time,
                "nodeCount": state.node_paths().len(),
            }),
        ),
        Err(e) => AgentToolResult::refused("studio_split", e.to_string()),
    }
}

/// Unified JSON tool dispatcher for all `studio_*` tools.
pub fn dispatch_agent_tool(
    state: &mut StudioState,
    tool_name: &str,
    args: &Value,
) -> AgentToolResult {
    let default_node = state.selected_node.map_or(0, |n| n.0);
    let node_id = args
        .get("nodeId")
        .or_else(|| args.get("node_id"))
        .and_then(Value::as_u64)
        .unwrap_or(default_node);
    let undo_label = args
        .get("history")
        .and_then(|h| h.get("undo_label"))
        .or_else(|| args.get("undoLabel"))
        .and_then(Value::as_str);

    match tool_name {
        "studio_status" | "studio_context" => studio_status(state),
        "studio_selection" => {
            let req = args.get("request").or_else(|| args.get("prompt")).and_then(Value::as_str);
            studio_selection(state, req)
        }
        "studio_look" => {
            if let Some(out_path) = args.get("outPath").or_else(|| args.get("out")).and_then(Value::as_str) {
                if let Err(e) = state.capture_current_frame(FrameFormat::Png, Path::new(out_path)) {
                    return AgentToolResult::refused("studio_look", e.to_string());
                }
            }
            studio_look(state)
        }
        "studio_tree" | "studio_layers" => studio_tree(state),
        "studio_timeline" => studio_timeline(state),
        "studio_select" => {
            if args.get("clear").and_then(Value::as_bool) == Some(true) {
                return studio_select(state, None);
            }
            let id = args
                .get("nodeId")
                .or_else(|| args.get("node_id"))
                .and_then(Value::as_u64);
            studio_select(state, id)
        }
        "studio_seek" => {
            let t = args
                .get("time")
                .or_else(|| args.get("timeSeconds"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            studio_seek(state, t)
        }
        "studio_frame" => {
            let f = args
                .get("frame")
                .or_else(|| args.get("frameIndex"))
                .and_then(Value::as_u64)
                .unwrap_or(0) as u32;
            studio_frame(state, f)
        }
        "studio_transport" => studio_transport(state, args),
        "studio_resize_canvas" | "studio_resize" => {
            let w = args
                .get("width")
                .and_then(Value::as_u64)
                .unwrap_or(u64::from(state.scene_def.width)) as u32;
            let h = args
                .get("height")
                .and_then(Value::as_u64)
                .unwrap_or(u64::from(state.scene_def.height)) as u32;
            match state.resize_canvas(w, h) {
                Ok(()) => AgentToolResult::verified(
                    "studio_resize_canvas",
                    format!("Resized canvas to {}x{}", state.scene_def.width, state.scene_def.height),
                    json!({
                        "width": state.scene_def.width,
                        "height": state.scene_def.height,
                    }),
                ),
                Err(e) => AgentToolResult::refused("studio_resize_canvas", e.to_string()),
            }
        }
        "studio_inspect" => {
            let id = args
                .get("nodeId")
                .or_else(|| args.get("node_id"))
                .and_then(Value::as_u64);
            studio_inspect(state, id)
        }
        "studio_add_node" => studio_add_node(state, args),
        "studio_delete_node" => studio_delete_node(state, node_id, undo_label),
        "studio_duplicate_node" => studio_duplicate_node(state, node_id),
        "studio_reorder_node" => {
            let delta = args.get("delta").and_then(Value::as_i64).unwrap_or(-1) as isize;
            studio_reorder_node(state, node_id, delta)
        }
        "studio_toggle_visibility" => {
            let vis = args.get("visible").and_then(Value::as_bool);
            studio_toggle_visibility(state, node_id, vis)
        }
        "studio_align_node" => {
            let h = args.get("horizontal").or_else(|| args.get("h")).and_then(Value::as_str);
            let v = args.get("vertical").or_else(|| args.get("v")).and_then(Value::as_str);
            studio_align_node(state, node_id, h, v)
        }
        "studio_set_text" => {
            let Some(text) = args
                .get("text")
                .or_else(|| args.get("content"))
                .and_then(Value::as_str)
            else {
                return AgentToolResult::refused(
                    "studio_set_text",
                    "missing 'text' string argument",
                );
            };
            studio_set_text(state, node_id, text, undo_label)
        }
        "studio_set_style" => {
            let Some(changes) = args.get("changes").and_then(Value::as_object) else {
                return AgentToolResult::refused(
                    "studio_set_style",
                    "missing 'changes' object argument",
                );
            };
            studio_set_style(state, node_id, changes, undo_label)
        }
        "studio_transform" => {
            let patch: TransformPatch = serde_json::from_value(args.clone()).unwrap_or_default();
            studio_transform(state, node_id, patch, undo_label)
        }
        "studio_set_shader" => studio_set_shader(state, node_id, args),
        "studio_audio" => studio_audio(state, args),
        "studio_add_animation" => {
            let Some(prop) = args.get("property").and_then(Value::as_str) else {
                return AgentToolResult::refused("studio_add_animation", "missing 'property'");
            };
            let spec = parse_animation_def_from_args(args);
            studio_add_animation(state, node_id, prop, spec, undo_label)
        }
        "studio_update_animation" => {
            let Some(prop) = args.get("property").and_then(Value::as_str) else {
                return AgentToolResult::refused("studio_update_animation", "missing 'property'");
            };
            let spec = parse_animation_def_from_args(args);
            studio_update_animation(state, node_id, prop, spec, undo_label)
        }
        "studio_add_keyframe" => {
            let Some(prop) = args.get("property").and_then(Value::as_str) else {
                return AgentToolResult::refused("studio_add_keyframe", "missing 'property'");
            };
            let time = args
                .get("time")
                .and_then(Value::as_f64)
                .unwrap_or(state.current_time);
            let value = args.get("value").and_then(Value::as_f64).unwrap_or(0.0) as f32;
            let easing = args.get("easing").and_then(Value::as_str);
            studio_add_keyframe(state, node_id, prop, time, value, easing, undo_label)
        }
        "studio_delete_keyframe" => {
            let Some(prop) = args.get("property").and_then(Value::as_str) else {
                return AgentToolResult::refused("studio_delete_keyframe", "missing 'property'");
            };
            let time = args
                .get("time")
                .and_then(Value::as_f64)
                .unwrap_or(state.current_time);
            studio_delete_keyframe(state, node_id, prop, time)
        }
        "studio_delete_animation" => {
            let Some(prop) = args.get("property").and_then(Value::as_str) else {
                return AgentToolResult::refused("studio_delete_animation", "missing 'property'");
            };
            studio_delete_animation(state, node_id, prop, undo_label)
        }
        "studio_shift_time" => {
            let delta = args
                .get("delta")
                .or_else(|| args.get("deltaSec"))
                .and_then(Value::as_f64)
                .unwrap_or(0.0);
            let ripple = args.get("ripple").and_then(Value::as_bool);
            studio_shift_time(state, node_id, delta, ripple)
        }
        "studio_split" => {
            let t = args.get("time").and_then(Value::as_f64);
            studio_split(state, node_id, t)
        }
        "studio_undo" => match state.undo() {
            Some(lbl) => AgentToolResult::verified(
                "studio_undo",
                format!("Undid: {lbl}"),
                json!({
                    "undoneLabel": lbl,
                    "undoDepth": state.undo_stack.len(),
                    "redoDepth": state.redo_stack.len(),
                }),
            ),
            None => AgentToolResult::refused("studio_undo", "undo stack is empty"),
        },
        "studio_redo" => match state.redo() {
            Some(lbl) => AgentToolResult::verified(
                "studio_redo",
                format!("Redid: {lbl}"),
                json!({
                    "redoneLabel": lbl,
                    "undoDepth": state.undo_stack.len(),
                    "redoDepth": state.redo_stack.len(),
                }),
            ),
            None => AgentToolResult::refused("studio_redo", "redo stack is empty"),
        },
        "studio_history" => {
            let undo_labels: Vec<&str> = state.undo_stack.iter().map(|e| e.label.as_str()).collect();
            let redo_labels: Vec<&str> = state.redo_stack.iter().map(|e| e.label.as_str()).collect();
            AgentToolResult::verified(
                "studio_history",
                format!("History: {} undo, {} redo entries", undo_labels.len(), redo_labels.len()),
                json!({
                    "undoCount": undo_labels.len(),
                    "redoCount": redo_labels.len(),
                    "undoStack": undo_labels,
                    "redoStack": redo_labels,
                }),
            )
        }
        "studio_load_scene" => {
            let Some(path_str) = args
                .get("path")
                .or_else(|| args.get("scene"))
                .and_then(Value::as_str)
            else {
                return AgentToolResult::refused("studio_load_scene", "missing 'path' argument");
            };
            match state.load_scene_file(Path::new(path_str)) {
                Ok(()) => AgentToolResult::verified(
                    "studio_load_scene",
                    format!("Loaded scene {}", path_str),
                    json!({
                        "path": path_str,
                        "width": state.scene_def.width,
                        "height": state.scene_def.height,
                        "duration": state.duration,
                        "nodeCount": state.node_paths().len(),
                    }),
                ),
                Err(e) => AgentToolResult::refused("studio_load_scene", e.to_string()),
            }
        }
        "studio_reload" => match state.reload_watched_scene_from_disk(true) {
            Ok(reloaded_path) => AgentToolResult::verified(
                "studio_reload",
                format!("Reloaded scene from {}", reloaded_path.display()),
                json!({
                    "path": reloaded_path,
                    "frameRevision": state.frame_revision,
                    "nodeCount": state.node_paths().len(),
                }),
            ),
            Err(e) => AgentToolResult::refused("studio_reload", e.to_string()),
        },
        "studio_apply_source" => {
            let Some(source) = args
                .get("source")
                .or_else(|| args.get("code"))
                .and_then(Value::as_str)
            else {
                return AgentToolResult::refused(
                    "studio_apply_source",
                    "missing 'source' (TypeScript or JSON string) argument",
                );
            };
            state.source_buffer = source.to_string();
            match state.apply_source_buffer() {
                Ok(()) => AgentToolResult::verified(
                    "studio_apply_source",
                    format!(
                        "Applied scene source ({}x{}, {} nodes)",
                        state.scene_def.width,
                        state.scene_def.height,
                        state.node_paths().len()
                    ),
                    json!({
                        "width": state.scene_def.width,
                        "height": state.scene_def.height,
                        "duration": state.duration,
                        "nodeCount": state.node_paths().len(),
                        "findings": state.inspect_findings,
                    }),
                ),
                Err(e) => AgentToolResult::refused("studio_apply_source", e.to_string()),
            }
        }
        "studio_save_scene" => {
            let out_opt = args
                .get("path")
                .or_else(|| args.get("out"))
                .and_then(Value::as_str)
                .map(Path::new);
            match state.save_scene_def_to_disk(out_opt) {
                Ok(saved_path) => AgentToolResult::saved(
                    "studio_save_scene",
                    format!("Saved scene JSON to {}", saved_path.display()),
                    json!({ "path": saved_path }),
                ),
                Err(e) => AgentToolResult::refused("studio_save_scene", e.to_string()),
            }
        }
        "studio_switch_composition" => {
            if let Some(idx) = args.get("index").and_then(Value::as_u64) {
                match state.select_composition(idx as usize) {
                    Ok(()) => AgentToolResult::verified(
                        "studio_switch_composition",
                        format!("Switched to composition #{}", idx),
                        json!({ "activeComposition": state.active_composition }),
                    ),
                    Err(e) => AgentToolResult::refused("studio_switch_composition", e.to_string()),
                }
            } else if let Some(query) = args
                .get("title")
                .or_else(|| args.get("id"))
                .and_then(Value::as_str)
            {
                let q = query.to_ascii_lowercase();
                if let Some(idx) = state.compositions.iter().position(|c| {
                    c.id.to_ascii_lowercase() == q || c.title.to_ascii_lowercase().contains(&q)
                }) {
                    let _ = state.select_composition(idx);
                    AgentToolResult::verified(
                        "studio_switch_composition",
                        format!("Switched to composition #{idx}"),
                        json!({ "activeComposition": idx }),
                    )
                } else {
                    AgentToolResult::refused(
                        "studio_switch_composition",
                        format!("no composition matching {query:?}"),
                    )
                }
            } else {
                AgentToolResult::refused(
                    "studio_switch_composition",
                    "provide 'index' or 'title'/'id'",
                )
            }
        }
        "studio_new_composition" => match state.create_new_composition() {
            Ok(idx) => {
                if let (Some(w), Some(h)) = (
                    args.get("width").and_then(Value::as_u64),
                    args.get("height").and_then(Value::as_u64),
                ) {
                    let _ = state.resize_canvas(w as u32, h as u32);
                }
                if let Some(title) = args.get("title").and_then(Value::as_str) {
                    if let Some(c) = state.compositions.get_mut(idx) {
                        c.title = title.to_string();
                    }
                }
                AgentToolResult::verified(
                    "studio_new_composition",
                    format!("Created composition #{idx}"),
                    json!({
                        "activeComposition": idx,
                        "width": state.scene_def.width,
                        "height": state.scene_def.height,
                    }),
                )
            }
            Err(e) => AgentToolResult::refused("studio_new_composition", e.to_string()),
        },
        "studio_export" => {
            if let Some(fmt) = args.get("format").and_then(Value::as_str) {
                state.export_format = fmt.to_string();
            }
            if let Some(out) = args
                .get("out")
                .or_else(|| args.get("outputPath"))
                .and_then(Value::as_str)
            {
                state.export_output_path = out.to_string();
            }
            match state.run_export_job() {
                Ok(job) => AgentToolResult::saved(
                    "studio_export",
                    format!(
                        "Exported {} ({} frames, {} B) -> {}",
                        job.format,
                        job.frames,
                        job.bytes_written,
                        job.output_path.display()
                    ),
                    serde_json::to_value(job).unwrap_or(Value::Null),
                ),
                Err(e) => AgentToolResult::refused("studio_export", e.to_string()),
            }
        }
        "studio_batch" => {
            let Some(calls) = args.get("calls").and_then(Value::as_array) else {
                return AgentToolResult::refused(
                    "studio_batch",
                    "missing 'calls' array of {tool, args} objects",
                );
            };
            let mut results = Vec::with_capacity(calls.len());
            let mut all_ok = true;
            for call in calls {
                let Some(t_name) = call.get("tool").and_then(Value::as_str) else {
                    all_ok = false;
                    results.push(AgentToolResult::refused("studio_batch", "missing 'tool' name"));
                    continue;
                };
                let t_args = call.get("args").cloned().unwrap_or_else(|| call.clone());
                let res = dispatch_agent_tool(state, t_name, &t_args);
                if !res.ok {
                    all_ok = false;
                }
                results.push(res);
            }
            let summary = format!(
                "Executed {} batch tool call(s) (all_ok={all_ok})",
                results.len()
            );
            AgentToolResult {
                ok: all_ok,
                stage: if all_ok { "verified" } else { "refused" },
                tool: "studio_batch",
                summary,
                payload: json!({ "results": results }),
            }
        }
        other => AgentToolResult::refused(
            "unknown_tool",
            format!("unknown studio agent tool: {other}"),
        ),
    }
}

fn parse_animation_def_from_args(args: &Value) -> AnimationDef {
    let from = args.get("from").and_then(Value::as_f64).unwrap_or(0.0) as f32;
    let to = args.get("to").and_then(Value::as_f64).unwrap_or(1.0) as f32;
    let start = args.get("start").and_then(Value::as_f64).unwrap_or(0.0);
    let end = args
        .get("end")
        .and_then(Value::as_f64)
        .unwrap_or((start + 1.0).max(1.0));
    let easing = args
        .get("easing")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| Some("easeOut".to_string()));
    let keyframes = args
        .get("keyframes")
        .and_then(|v| serde_json::from_value::<Vec<KeyframeDef>>(v.clone()).ok());
    AnimationDef {
        from,
        to,
        start,
        end,
        keyframes,
        easing,
    }
}

/// Build the structured **"Kine Studio element edit request v1"** context block
/// for external CLI agents (`kine agent selection --prompt`).
pub fn build_element_agent_prompt(
    state: &StudioState,
    node_id: NodeId,
    user_request: &str,
) -> String {
    let comp_title = state
        .compositions
        .get(state.active_composition)
        .map(|c| c.title.as_str())
        .unwrap_or("Composition");
    let def = state.node_def(node_id).cloned().unwrap_or_default();
    let rect = state
        .node_abs_rect(node_id)
        .unwrap_or_else(|| kine_core::Rect::new(0.0, 0.0, 0.0, 0.0));
    let path = state.path_for_node(node_id).unwrap_or_default();
    let badge = node_def_badge(&def);
    let label = node_def_display_label(node_id, &def);

    let anim_json = def
        .animate
        .as_ref()
        .and_then(|a| serde_json::to_string_pretty(a).ok())
        .unwrap_or_else(|| "{}".to_string());

    let request_trimmed = user_request.trim();
    let request_block = if request_trimmed.is_empty() {
        "(Inspect or mutate this node from your external CLI agent via `kine agent ...`.)"
    } else {
        request_trimmed
    };

    format!(
        "Kine Studio element edit request v1\n\
         \n\
         Composition: {comp_title} ({w}x{h} @ {fps:.0}fps, duration {dur:.2}s, playhead {t:.2}s / frame {f})\n\
         Target Node: {node_id} [{badge}] ({label})\n\
         Scene Tree Path: {path:?}\n\
         Computed Layout Bounds (px): x={rx:.1}, y={ry:.1}, width={rw:.1}, height={rh:.1}\n\
         \n\
         Authored Style & Transform:\n\
         - kind: {kind}\n\
         - fill / color: {fill:?} / {color:?}\n\
         - opacity: {opacity:.2}\n\
         - cornerRadius: {cr:?}\n\
         - transform: translate=({tx:.1}, {ty:.1}), scale={scale:.2}, rotation={rot:.1}deg\n\
         - textContent: {text:?}\n\
         \n\
         Timeline Animations:\n\
         {anim_json}\n\
         \n\
         External CLI Agent Bridge Commands:\n\
         - kine agent status | selection | look | tree | timeline | inspect --node {nid}\n\
         - kine agent set-text --node {nid} --text \"...\"\n\
         - kine agent set-style --node {nid} --fill \"#3de4a8\" --opacity 0.9\n\
         - kine agent transform --node {nid} --x {rx:.0} --y {ry:.0} --width {rw:.0} --height {rh:.0}\n\
         - kine agent animate --node {nid} --prop opacity --from 0 --to 1 --start 0 --end 1.2 --easing easeOut\n\
         - kine agent keyframe --node {nid} --prop translateX --time {t:.2} --value 40\n\
         \n\
         Guardrails:\n\
         - Apply changes only to target node {node_id} unless explicitly requested.\n\
         - Verify layout bounds and shader uniforms with `kine agent inspect` / `kine agent look` after editing.\n\
         \n\
         User Request:\n\
         {request_block}\n",
        w = state.scene_def.width,
        h = state.scene_def.height,
        fps = state.fps,
        dur = state.duration,
        t = state.current_time,
        f = state.current_frame_index(),
        nid = node_id.0,
        rx = rect.x,
        ry = rect.y,
        rw = rect.width,
        rh = rect.height,
        kind = def.kind.as_deref().unwrap_or("container"),
        fill = def.fill,
        color = def.color,
        opacity = def.opacity.unwrap_or(1.0),
        cr = def.corner_radius.or(def.radius),
        tx = def.translate_x.unwrap_or(0.0),
        ty = def.translate_y.unwrap_or(0.0),
        scale = def.scale.unwrap_or(1.0),
        rot = def.rotation.unwrap_or(0.0),
        text = def.content,
    )
}

/// JSON Schema declarations for all `studio_*` bridge tools for external CLI agents (Opus 5.5, GPT-6 Astra, Gemini Argon 4).
pub fn agent_tools_json_schema() -> Value {
    json!([
        { "name": "studio_status", "description": "Return full live Studio state (composition, playhead, canvas, selection, findings, undo depth, bridge address)." },
        { "name": "studio_selection", "description": "Return the currently selected node in Studio with computed bounds, authored NodeDef, and structured agent prompt." },
        { "name": "studio_look", "description": "Render current playhead frame to base64 PNG (or save to outPath) and return scene tree & static diagnostics." },
        { "name": "studio_tree", "description": "Return hierarchical scene layer tree with node bounds, visibility, Z-order, and animated properties." },
        { "name": "studio_timeline", "description": "Return all NLE timeline tracks, animated property sub-tracks, keyframes, and audio tracks." },
        { "name": "studio_select", "description": "Select a scene node by numeric NodeId or clear selection." },
        { "name": "studio_seek", "description": "Seek the playhead to timeSeconds and re-render in-process." },
        { "name": "studio_frame", "description": "Seek the playhead to an exact 0-based frameIndex." },
        { "name": "studio_transport", "description": "Control playback transport (play, pause, toggle, stop, step), loop, rate, fps, audio mute/volume, and timeline flags." },
        { "name": "studio_resize_canvas", "description": "Resize active composition canvas (width, height)." },
        { "name": "studio_inspect", "description": "Inspect a node's computed bounds, authored definition, inspector snapshot, and static findings." },
        { "name": "studio_add_node", "description": "Insert a new node (rect, circle, text, shader, image, video, path, container), catalog preset, or project module under parentId." },
        { "name": "studio_delete_node", "description": "Delete a non-root node from the scene." },
        { "name": "studio_duplicate_node", "description": "Duplicate a non-root node as a sibling and select the copy." },
        { "name": "studio_reorder_node", "description": "Move a node up or down in sibling Z-order." },
        { "name": "studio_toggle_visibility", "description": "Show or hide a node in the scene." },
        { "name": "studio_align_node", "description": "Align a node horizontally (left, center, right) and/or vertically (top, middle, bottom) in the canvas." },
        { "name": "studio_set_text", "description": "Update text content on a Text node with read-back verification." },
        { "name": "studio_set_style", "description": "Mutate fill, color, opacity, cornerRadius, stroke, shadow, font, flex layout, or gradient properties." },
        { "name": "studio_transform", "description": "Mutate absolute or relative x, y, width, height, translateX, translateY, scale, or rotation with bounds verification." },
        { "name": "studio_set_shader", "description": "Validate and attach WGSL shader source or preset, blend mode, and uniform values to a node." },
        { "name": "studio_audio", "description": "Add, update, remove, or clear DSP audio bed tracks." },
        { "name": "studio_add_animation", "description": "Add a keyframe or from/to animation track on an animatable property." },
        { "name": "studio_update_animation", "description": "Update an existing animation track on a node." },
        { "name": "studio_add_keyframe", "description": "Insert or update a keyframe at (time, value) on a property track." },
        { "name": "studio_delete_keyframe", "description": "Delete a keyframe at time on a property track." },
        { "name": "studio_delete_animation", "description": "Delete an animation track from a node." },
        { "name": "studio_shift_time", "description": "Shift node timing by delta seconds (with optional ripple edit)." },
        { "name": "studio_split", "description": "Split a video clip or insert a midpoint keyframe at playhead." },
        { "name": "studio_undo", "description": "Undo the most recent studio mutation." },
        { "name": "studio_redo", "description": "Redo the most recently undone studio mutation." },
        { "name": "studio_history", "description": "List undo and redo history stacks." },
        { "name": "studio_load_scene", "description": "Load a .ts or .json scene file from disk into the live Studio." },
        { "name": "studio_reload", "description": "Reload the active scene file from disk." },
        { "name": "studio_apply_source", "description": "Compile and apply raw TypeScript DSL or JSON scene source directly in Studio." },
        { "name": "studio_save_scene", "description": "Save the active SceneDef AST as pretty-printed JSON to disk." },
        { "name": "studio_switch_composition", "description": "Switch the active composition by index, id, or title." },
        { "name": "studio_new_composition", "description": "Create and select a new composition." },
        { "name": "studio_export", "description": "Run an in-process render/export job (mp4, mkv, webm, gif, png) and record it in the Render Queue." },
        { "name": "studio_batch", "description": "Execute a batch of multiple studio_* tool calls atomically in a single round-trip." }
    ])
}
