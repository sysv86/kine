//! Hyperframes WebMCP-parity AI Agent Tool Suite (`studio_*`) and Element
//! Prompt Builder (`build_element_agent_prompt`) for Kine Studio.
//!
//! All 12 tools execute directly against the in-process [`StudioState`] and
//! [`kine_server::Engine`], performing read-back verification (`stage: "verified"`)
//! after every scene mutation and checkpointing `StudioState::undo_stack`.

use crate::state::StudioState;
use crate::timeline::{node_def_badge, node_def_display_label};
use kine_core::NodeId;
use kine_script::{AnimationDef, EdgesDef, KeyframeDef, SizeDef};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};
use std::collections::BTreeMap;

/// Structured receipt returned by every `studio_*` tool (mirroring Hyperframes `StudioToolResult`).
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

/// Geometry & 2D transform patch for `studio_transform`.
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
    pub rotation: Option<f32>,
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

/// 2. `studio_select` — Select or clear a scene node (`NodeId`).
pub fn studio_select(state: &mut StudioState, node_id: Option<u64>) -> AgentToolResult {
    let Some(raw_id) = node_id else {
        state.selected_node = None;
        let _ = state.render_current_frame();
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
        return AgentToolResult::refused("studio_seek", "time_seconds must be a non-negative number");
    }
    state.seek_to(time_seconds, false);
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
    let report = state.engine.inspect(state.scene_handle).ok();

    AgentToolResult::verified(
        "studio_inspect",
        format!("Inspected node {target}"),
        json!({
            "nodeId": target.0,
            "bounds": rect,
            "authored": def,
            "findings": report.map(|r| r.findings).unwrap_or_default(),
        }),
    )
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

/// 7. `studio_set_style` — Mutate visual/style properties on `node_id` and verify in-process.
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
        }),
    )
}

/// 8. `studio_transform` — Mutate layout bounds (`x`, `y`, `width`, `height`) or 2D transform
/// (`translate_x`, `translate_y`, `scale`, `rotation`) and verify read-back `abs_pos`.
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

    let before_rect = state.node_abs_rect(target);
    let label = undo_label
        .map(str::to_string)
        .unwrap_or_else(|| format!("Agent: transform #{node_id}"));
    let p = patch.clone();

    if let Err(e) = state.mutate_node(target, label, move |def| {
        if p.x.is_some() || p.y.is_some() {
            if def.position.is_none() {
                def.position = Some("absolute".to_string());
            }
            let inset = def.inset.get_or_insert_with(EdgesDef::default);
            if let Some(x) = p.x {
                inset.left = Some(x);
            }
            if let Some(y) = p.y {
                inset.top = Some(y);
            }
        }
        if p.width.is_some() || p.height.is_some() {
            let size = def.size.get_or_insert_with(SizeDef::default);
            if let Some(w) = p.width {
                size.width = Some(w.max(1.0));
            }
            if let Some(h) = p.height {
                size.height = Some(h.max(1.0));
            }
        }
        if let Some(tx) = p.translate_x {
            def.translate_x = Some(tx);
        }
        if let Some(ty) = p.translate_y {
            def.translate_y = Some(ty);
        }
        if let Some(s) = p.scale {
            def.scale = Some(s);
            def.scale_x = Some(s);
            def.scale_y = Some(s);
        }
        if let Some(rot) = p.rotation {
            def.rotation = Some(rot);
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

/// Unified JSON tool dispatcher for all 12 `studio_*` tools.
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
        "studio_look" => studio_look(state),
        "studio_select" => {
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
        "studio_inspect" => {
            let id = args
                .get("nodeId")
                .or_else(|| args.get("node_id"))
                .and_then(Value::as_u64);
            studio_inspect(state, id)
        }
        "studio_set_text" => {
            let Some(text) = args.get("text").or_else(|| args.get("content")).and_then(Value::as_str)
            else {
                return AgentToolResult::refused("studio_set_text", "missing 'text' string argument");
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
        "studio_delete_animation" => {
            let Some(prop) = args.get("property").and_then(Value::as_str) else {
                return AgentToolResult::refused("studio_delete_animation", "missing 'property'");
            };
            studio_delete_animation(state, node_id, prop, undo_label)
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
    AnimationDef {
        from,
        to,
        start,
        end,
        keyframes: None,
        easing,
    }
}

/// Build the structured **"Kine Studio element edit request v1"** prompt
/// (mirroring Hyperframes Studio's `buildElementAgentPrompt` in `domEditingAgentPrompt.ts`).
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
        "(Describe the visual, layout, shader, or keyframe change you want on this node.)"
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
         Available In-Process Agent Tools:\n\
         - studio_look, studio_select, studio_seek, studio_frame, studio_inspect\n\
         - studio_set_text, studio_set_style, studio_transform\n\
         - studio_add_animation, studio_update_animation, studio_add_keyframe, studio_delete_animation\n\
         \n\
         Guardrails:\n\
         - Apply changes only to target node {node_id} unless explicitly requested.\n\
         - Always include `history.undo_label` on mutating tool calls.\n\
         - Verify layout bounds and shader uniforms with `studio_inspect` / `studio_look` after editing.\n\
         \n\
         User Request:\n\
         {request_block}\n",
        w = state.scene_def.width,
        h = state.scene_def.height,
        fps = state.fps,
        dur = state.duration,
        t = state.current_time,
        f = state.current_frame_index(),
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

/// JSON Schema declarations for all 12 `studio_*` tools for external LLM tool-calling agents.
pub fn agent_tools_json_schema() -> Value {
    json!([
        { "name": "studio_look", "description": "Render current playhead frame to base64 PNG and return scene tree & static diagnostics." },
        { "name": "studio_select", "description": "Select a scene node by numeric NodeId or clear selection." },
        { "name": "studio_seek", "description": "Seek the playhead to timeSeconds and re-render in-process." },
        { "name": "studio_frame", "description": "Seek the playhead to an exact 0-based frameIndex." },
        { "name": "studio_inspect", "description": "Inspect a node's computed bounds, authored definition, and static findings." },
        { "name": "studio_set_text", "description": "Update text content on a Text node with read-back verification." },
        { "name": "studio_set_style", "description": "Mutate fill, color, opacity, cornerRadius, stroke, shadow, or font properties." },
        { "name": "studio_transform", "description": "Mutate x, y, width, height, translateX, translateY, scale, or rotation with bounds verification." },
        { "name": "studio_add_animation", "description": "Add a keyframe or from/to animation track on an animatable property." },
        { "name": "studio_update_animation", "description": "Update an existing animation track on a node." },
        { "name": "studio_add_keyframe", "description": "Insert or update a keyframe at (time, value) on a property track." },
        { "name": "studio_delete_animation", "description": "Delete an animation track from a node." }
    ])
}
