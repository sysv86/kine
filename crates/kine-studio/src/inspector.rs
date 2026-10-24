//! Design / Property Inspector & Layers Tree operations for Kine Studio.
//!
//! Mirrors Hyperframes Studio's `PropertyPanel.tsx`, `PropertyFieldRow.tsx`, and
//! `LayersPanel.tsx`: layout geometry (`X`, `Y`, `W`, `H`), 2D transforms,
//! appearance (fill, stroke, shadow, corner radius), typography, WGSL shader
//! uniform reflection, keyframe & easing controls, and Z-ordered layer tree
//! management.

use crate::state::{node_at_path_mut, StudioError, StudioState};
use crate::timeline::{node_def_badge, node_def_display_label};
use kine_core::{NodeId, Rect};
use kine_script::{AnimationDef, EdgesDef, KeyframeDef, SizeDef};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

/// Snapshot of a single node's authored + evaluated properties for the Right Dock's `Design` tab.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInspectorSnapshot {
    pub node_id: NodeId,
    pub kind_badge: &'static str,
    pub label: String,
    pub computed_rect: Rect,
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
    pub opacity: f32,
    pub translate_x: f32,
    pub translate_y: f32,
    pub scale_x: f32,
    pub scale_y: f32,
    pub rotation_deg: f32,
    pub fill: Option<String>,
    pub corner_radius: f32,
    pub stroke_color: Option<String>,
    pub stroke_width: f32,
    pub shadow_color: Option<String>,
    pub shadow_blur: f32,
    pub text_content: Option<String>,
    pub font_family: Option<String>,
    pub font_size: Option<f32>,
    pub font_weight: Option<u16>,
    pub text_color: Option<String>,
    pub text_align: Option<String>,
    pub shader_blend: Option<String>,
    pub shader_uniforms: Vec<(String, f32)>,
    pub animations: Vec<(String, AnimationDef)>,
}

/// One row in the Right Dock's `Layers` tree panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LayerTreeRow {
    pub node_id: NodeId,
    pub depth: usize,
    pub badge: &'static str,
    pub label: String,
    pub visible: bool,
    pub selected: bool,
    pub can_move_up: bool,
    pub can_move_down: bool,
}

impl StudioState {
    /// Build a complete `NodeInspectorSnapshot` for `self.selected_node` (or root `#0`).
    pub fn inspect_selected_node(&self) -> Option<NodeInspectorSnapshot> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let def = self.node_def(node_id)?;
        let rt_node = self.runtime_node(node_id);
        let computed_rect = rt_node
            .and_then(|n| n.abs_pos)
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 0.0, 0.0));

        let x = def
            .inset
            .as_ref()
            .and_then(|i| i.left)
            .unwrap_or(computed_rect.x);
        let y = def
            .inset
            .as_ref()
            .and_then(|i| i.top)
            .unwrap_or(computed_rect.y);
        let width = def
            .size
            .as_ref()
            .and_then(|s| s.width)
            .unwrap_or(computed_rect.width);
        let height = def
            .size
            .as_ref()
            .and_then(|s| s.height)
            .unwrap_or(computed_rect.height);

        let opacity = rt_node.map_or(def.opacity.unwrap_or(1.0), |n| n.opacity);
        let (tx, ty, sx, sy, rot) = if let Some(rt) = rt_node {
            (
                rt.transform.translate_x,
                rt.transform.translate_y,
                rt.transform.scale_x,
                rt.transform.scale_y,
                rt.transform.rotation_deg,
            )
        } else {
            let s = def.scale.unwrap_or(1.0);
            (
                def.translate_x.unwrap_or(0.0),
                def.translate_y.unwrap_or(0.0),
                def.scale_x.unwrap_or(s),
                def.scale_y.unwrap_or(s),
                def.rotation.unwrap_or(0.0),
            )
        };

        let mut shader_uniforms = Vec::new();
        if let Some(u_map) = &def.uniforms {
            for (k, v) in u_map {
                if let Some(num) = v.as_f64() {
                    shader_uniforms.push((k.clone(), num as f32));
                }
            }
        }

        let animations = def
            .animate
            .as_ref()
            .map(|m| m.iter().map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();

        Some(NodeInspectorSnapshot {
            node_id,
            kind_badge: node_def_badge(def),
            label: node_def_display_label(node_id, def),
            computed_rect,
            x,
            y,
            width,
            height,
            opacity,
            translate_x: tx,
            translate_y: ty,
            scale_x: sx,
            scale_y: sy,
            rotation_deg: rot,
            fill: def.fill.clone(),
            corner_radius: def.corner_radius.or(def.radius).unwrap_or(0.0),
            stroke_color: def.stroke_color.clone(),
            stroke_width: def.stroke_width.unwrap_or(0.0),
            shadow_color: def.shadow_color.clone(),
            shadow_blur: def.shadow_blur.unwrap_or(0.0),
            text_content: def.content.clone(),
            font_family: def.font_family.clone(),
            font_size: def.font_size,
            font_weight: def.font_weight,
            text_color: def.color.clone(),
            text_align: def.align.clone(),
            shader_blend: def.blend.clone(),
            shader_uniforms,
            animations,
        })
    }

    /// Build the hierarchical list of rows for the Right Dock's `Layers` panel.
    pub fn build_layer_tree_rows(&self) -> Vec<LayerTreeRow> {
        let paths = self.node_paths();
        let mut rows = Vec::with_capacity(paths.len());

        for (node_id, path) in &paths {
            let Some(def) = self.node_def(*node_id) else {
                continue;
            };
            let visible = !def.opacity.is_some_and(|o| o <= 0.001);
            let (can_up, can_down) = if let Some((&child_idx, parent_path)) = path.split_last() {
                let sib_len = self
                    .node_paths()
                    .iter()
                    .find(|(_, p)| p.as_slice() == parent_path)
                    .and_then(|(pid, _)| self.node_def(*pid))
                    .and_then(|pdef| pdef.children.as_ref())
                    .map_or(0, |c| c.len());
                (child_idx > 0, child_idx + 1 < sib_len)
            } else {
                (false, false)
            };

            rows.push(LayerTreeRow {
                node_id: *node_id,
                depth: path.len(),
                badge: node_def_badge(def),
                label: node_def_display_label(*node_id, def),
                visible,
                selected: self.selected_node == Some(*node_id),
                can_move_up: can_up,
                can_move_down: can_down,
            });
        }

        rows
    }

    /// Nudge selected node's `inset.left` and `inset.top` by `(dx, dy)` pixels.
    pub fn inspector_nudge_position(&mut self, dx: f32, dy: f32) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let cur_rect = self
            .node_abs_rect(node_id)
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 100.0, 100.0));
        self.mutate_node(
            node_id,
            format!("Move {node_id} by ({dx:+.0}, {dy:+.0})"),
            move |def| {
                if def.position.is_none() {
                    def.position = Some("absolute".to_string());
                }
                let inset = def.inset.get_or_insert_with(EdgesDef::default);
                let base_x = inset.left.unwrap_or(cur_rect.x);
                let base_y = inset.top.unwrap_or(cur_rect.y);
                inset.left = Some(base_x + dx);
                inset.top = Some(base_y + dy);
                Ok(())
            },
        )
    }

    /// Nudge selected node's `size.width` and `size.height` by `(dw, dh)` pixels.
    pub fn inspector_nudge_size(&mut self, dw: f32, dh: f32) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let cur_rect = self
            .node_abs_rect(node_id)
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 100.0, 100.0));
        self.mutate_node(
            node_id,
            format!("Resize {node_id} by ({dw:+.0}, {dh:+.0})"),
            move |def| {
                let size = def.size.get_or_insert_with(SizeDef::default);
                let base_w = size.width.unwrap_or(cur_rect.width);
                let base_h = size.height.unwrap_or(cur_rect.height);
                size.width = Some((base_w + dw).max(4.0));
                size.height = Some((base_h + dh).max(4.0));
                Ok(())
            },
        )
    }

    /// Align selected node within the canvas (`left`, `center`, `right` / `top`, `middle`, `bottom`).
    pub fn inspector_align_selected(
        &mut self,
        horizontal: Option<&str>,
        vertical: Option<&str>,
    ) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        if node_id == NodeId(0) {
            return Ok(());
        }
        let cw = self.scene_def.width as f32;
        let ch = self.scene_def.height as f32;
        let rect = self
            .node_abs_rect(node_id)
            .unwrap_or_else(|| Rect::new(0.0, 0.0, 100.0, 100.0));
        let h_mode = horizontal.map(str::to_string);
        let v_mode = vertical.map(str::to_string);

        self.mutate_node(node_id, format!("Align {node_id} in canvas"), move |def| {
            def.position = Some("absolute".to_string());
            let inset = def.inset.get_or_insert_with(EdgesDef::default);
            if let Some(h) = h_mode.as_deref() {
                inset.left = Some(match h {
                    "left" => 0.0,
                    "center" => ((cw - rect.width) * 0.5).round(),
                    "right" => (cw - rect.width).max(0.0).round(),
                    _ => inset.left.unwrap_or(rect.x),
                });
            }
            if let Some(v) = v_mode.as_deref() {
                inset.top = Some(match v {
                    "top" => 0.0,
                    "middle" | "center" => ((ch - rect.height) * 0.5).round(),
                    "bottom" => (ch - rect.height).max(0.0).round(),
                    _ => inset.top.unwrap_or(rect.y),
                });
            }
            Ok(())
        })
    }

    /// Set opacity (`0.0..=1.0`) on the selected node. If `self.auto_keyframe` is true and
    /// `opacity` is already animated on this node, records a keyframe at `self.current_time`.
    pub fn inspector_set_opacity(&mut self, opacity: f32) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let clamped = opacity.clamp(0.0, 1.0);
        let auto_kf = self.auto_keyframe;
        let t = self.current_time;

        self.mutate_node(
            node_id,
            format!("Set {node_id} opacity to {clamped:.2}"),
            move |def| {
                def.opacity = Some(clamped);
                if auto_kf {
                    if let Some(anim_map) = &mut def.animate {
                        if let Some(spec) = anim_map.get_mut("opacity") {
                            upsert_keyframe_in_spec(spec, t, clamped);
                        }
                    }
                }
                Ok(())
            },
        )
    }

    /// Adjust rotation in degrees on the selected node.
    pub fn inspector_nudge_rotation(&mut self, delta_deg: f32) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let auto_kf = self.auto_keyframe;
        let t = self.current_time;

        self.mutate_node(
            node_id,
            format!("Rotate {node_id} by {delta_deg:+.1} deg"),
            move |def| {
                let next = def.rotation.unwrap_or(0.0) + delta_deg;
                def.rotation = Some(next);
                if auto_kf {
                    if let Some(anim_map) = &mut def.animate {
                        if let Some(spec) = anim_map.get_mut("rotation") {
                            upsert_keyframe_in_spec(spec, t, next);
                        }
                    }
                }
                Ok(())
            },
        )
    }

    /// Adjust uniform scale on the selected node.
    pub fn inspector_nudge_scale(&mut self, delta_scale: f32) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let auto_kf = self.auto_keyframe;
        let t = self.current_time;

        self.mutate_node(
            node_id,
            format!("Scale {node_id} by {delta_scale:+.2}"),
            move |def| {
                let next = (def.scale.unwrap_or(1.0) + delta_scale).clamp(0.05, 8.0);
                def.scale = Some(next);
                def.scale_x = Some(next);
                def.scale_y = Some(next);
                if auto_kf {
                    if let Some(anim_map) = &mut def.animate {
                        if let Some(spec) = anim_map.get_mut("scale") {
                            upsert_keyframe_in_spec(spec, t, next);
                        }
                    }
                }
                Ok(())
            },
        )
    }

    /// Reset 2D transform (`translate`, `scale`, `rotation`) on the selected node.
    pub fn inspector_reset_transform(&mut self) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        self.mutate_node(node_id, format!("Reset transform on {node_id}"), |def| {
            def.translate_x = Some(0.0);
            def.translate_y = Some(0.0);
            def.scale = Some(1.0);
            def.scale_x = Some(1.0);
            def.scale_y = Some(1.0);
            def.rotation = Some(0.0);
            Ok(())
        })
    }

    /// Set hex fill/text color (`#rrggbb` or `#rrggbbaa`) on the selected node.
    pub fn inspector_set_fill(&mut self, hex_color: &str) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let _ = kine_script::parse_color(hex_color)?;
        let color_str = hex_color.to_string();

        self.mutate_node(
            node_id,
            format!("Set {node_id} color to {color_str}"),
            move |def| {
                if def.kind.as_deref() == Some("text") {
                    def.color = Some(color_str);
                } else {
                    def.fill = Some(color_str);
                }
                Ok(())
            },
        )
    }

    /// Adjust corner radius on the selected node.
    pub fn inspector_nudge_corner_radius(&mut self, delta: f32) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        self.mutate_node(
            node_id,
            format!("Adjust {node_id} corner radius by {delta:+.1}"),
            move |def| {
                let cur = def.corner_radius.or(def.radius).unwrap_or(0.0);
                let next = (cur + delta).max(0.0);
                def.corner_radius = Some(next);
                if def.kind.as_deref() == Some("circle") {
                    def.radius = Some(next.max(2.0));
                }
                Ok(())
            },
        )
    }

    /// Update text content on a `Text` node.
    pub fn inspector_set_text_content(&mut self, content: &str) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let text = content.to_string();
        self.mutate_node(
            node_id,
            format!("Update text on {node_id}"),
            move |def| {
                def.content = Some(text);
                Ok(())
            },
        )
    }

    /// Adjust font size on a `Text` node.
    pub fn inspector_nudge_font_size(&mut self, delta: f32) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        self.mutate_node(
            node_id,
            format!("Adjust font size on {node_id} by {delta:+.1}"),
            move |def| {
                let cur = def.font_size.unwrap_or(16.0);
                def.font_size = Some((cur + delta).clamp(6.0, 200.0));
                Ok(())
            },
        )
    }

    /// Set text alignment (`left`, `center`, `right`) on a `Text` node.
    pub fn inspector_set_text_align(&mut self, align: &str) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let align_str = align.to_string();
        self.mutate_node(
            node_id,
            format!("Set text align on {node_id} to {align_str}"),
            move |def| {
                def.align = Some(align_str);
                Ok(())
            },
        )
    }

    /// Update the easing curve on `property` for the selected node.
    pub fn inspector_set_easing(&mut self, property: &str, easing: &str) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let prop = property.to_string();
        let ease = easing.to_string();
        self.active_easing_input = ease.clone();

        self.mutate_node(
            node_id,
            format!("Set easing on {node_id}.{prop} to {ease}"),
            move |def| {
                let anim_map = def.animate.get_or_insert_with(BTreeMap::new);
                if let Some(spec) = anim_map.get_mut(&prop) {
                    spec.easing = Some(ease);
                }
                Ok(())
            },
        )
    }

    /// Update a scalar `f32` WGSL uniform on a `Shader` node.
    pub fn inspector_set_shader_uniform_f32(
        &mut self,
        uniform_name: &str,
        value: f32,
    ) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let key = uniform_name.to_string();
        self.mutate_node(
            node_id,
            format!("Set shader uniform {key} = {value:.3} on {node_id}"),
            move |def| {
                let map = def.uniforms.get_or_insert_with(serde_json::Map::new);
                map.insert(key, serde_json::json!(value));
                Ok(())
            },
        )
    }

    /// Toggle visibility (`opacity = 0.0` vs `1.0`) of `node_id` in the Layers panel.
    pub fn layer_toggle_visibility(&mut self, node_id: NodeId) -> Result<(), StudioError> {
        let cur_vis = self
            .node_def(node_id)
            .map_or(true, |d| !d.opacity.is_some_and(|o| o <= 0.001));
        let next_opacity = if cur_vis { 0.0 } else { 1.0 };
        self.mutate_node(
            node_id,
            format!(
                "{} layer {node_id}",
                if cur_vis { "Hide" } else { "Show" }
            ),
            move |def| {
                def.opacity = Some(next_opacity);
                Ok(())
            },
        )
    }

    /// Move `node_id` up (`delta = -1`) or down (`delta = +1`) among its siblings in Z-order.
    pub fn layer_move_z_order(&mut self, node_id: NodeId, delta: isize) -> Result<(), StudioError> {
        if node_id == NodeId(0) {
            return Ok(());
        }
        let path = self
            .path_for_node(node_id)
            .ok_or(StudioError::NodeNotFound(node_id))?;
        let (&child_idx, parent_path) = path
            .split_last()
            .ok_or(StudioError::NodeNotFound(node_id))?;
        let parent_path = parent_path.to_vec();

        self.commit_mutation(
            format!("Reorder layer {node_id} ({delta:+})"),
            move |scene_def| {
                let parent = node_at_path_mut(&mut scene_def.root, &parent_path)
                    .ok_or(StudioError::NodeNotFound(node_id))?;
                let children = parent
                    .children
                    .as_mut()
                    .ok_or(StudioError::NodeNotFound(node_id))?;
                let new_idx = (child_idx as isize) + delta;
                if new_idx < 0 || (new_idx as usize) >= children.len() {
                    return Ok(());
                }
                children.swap(child_idx, new_idx as usize);
                Ok(())
            },
        )
    }
}

fn upsert_keyframe_in_spec(spec: &mut AnimationDef, time: f64, value: f32) {
    let mut kfs = spec.keyframes.take().unwrap_or_else(|| {
        vec![
            KeyframeDef {
                time: spec.start,
                value: spec.from,
            },
            KeyframeDef {
                time: spec.end.max(spec.start + 0.1),
                value: spec.to,
            },
        ]
    });
    if let Some(existing) = kfs.iter_mut().find(|k| (k.time - time).abs() < 1e-3) {
        existing.value = value;
    } else {
        kfs.push(KeyframeDef { time, value });
    }
    kfs.sort_by(|a, b| a.time.total_cmp(&b.time));
    spec.start = kfs.first().map_or(0.0, |k| k.time);
    spec.end = kfs.last().map_or(1.0, |k| k.time);
    spec.keyframes = Some(kfs);
}
