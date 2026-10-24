//! NLE Timeline & Node Track model and editing operations for Kine Studio.
//!
//! Mirrors Hyperframes Studio's `TimelinePane`, `TimelineToolbar`, and
//! `Timeline.tsx`: time ruler tick generation, multi-track node/audio lanes,
//! keyframe diamonds, playhead scrubbing, clip/keyframe splitting, time
//! shifting, ripple edit, and keyframe navigation.

use crate::state::{node_at_path_mut, StudioError, StudioState};
use kine_core::{NodeId, NodeKind};
use kine_script::{AnimationDef, KeyframeDef, NodeDef};
use std::collections::BTreeMap;

/// Classification of a track row in the bottom NLE timeline.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TimelineTrackKind {
    SceneNode {
        node_id: NodeId,
        depth: usize,
        kind_badge: &'static str,
    },
    AnimationProperty {
        node_id: NodeId,
        property: String,
        easing: String,
    },
    AudioTrack {
        track_index: usize,
    },
}

/// A diamond keyframe marker along an animation track lane.
#[derive(Debug, Clone, PartialEq)]
pub struct TimelineKeyframeMarker {
    pub node_id: NodeId,
    pub property: String,
    pub time: f64,
    pub value: f32,
}

/// One horizontal lane in the bottom NLE timeline & node editor.
#[derive(Debug, Clone)]
pub struct TimelineTrack {
    pub kind: TimelineTrackKind,
    pub label: String,
    pub subtitle: String,
    pub start_sec: f64,
    pub end_sec: f64,
    pub selected: bool,
    pub muted_or_hidden: bool,
    pub keyframes: Vec<TimelineKeyframeMarker>,
}

/// A major or minor tick on the timeline time ruler.
#[derive(Debug, Clone)]
pub struct TimelineTick {
    pub time_sec: f64,
    pub x_px: f32,
    pub is_major: bool,
    pub label: Option<String>,
}

/// Generate adaptive time ruler ticks based on composition duration and zoom (`zoom_pps`).
pub fn generate_ruler_ticks(duration: f64, zoom_pps: f32) -> Vec<TimelineTick> {
    let dur = duration.max(1.0);
    let pps = f64::from(zoom_pps.clamp(30.0, 600.0));
    let major_step = if pps >= 240.0 {
        0.25
    } else if pps >= 110.0 {
        0.5
    } else {
        1.0
    };
    let minor_step = major_step / 2.0;

    let mut ticks = Vec::new();
    let total_steps = ((dur / minor_step).ceil() as usize).saturating_add(1);
    for i in 0..=total_steps {
        let t = (i as f64) * minor_step;
        if t > dur + 1e-6 {
            break;
        }
        let is_major = (i % 2) == 0;
        let label = if is_major {
            Some(format!("{t:.2}s"))
        } else {
            None
        };
        ticks.push(TimelineTick {
            time_sec: t,
            x_px: (t * pps) as f32,
            is_major,
            label,
        });
    }
    ticks
}

impl StudioState {
    /// Extract all hierarchical node lanes, animated property sub-lanes, and audio tracks
    /// for rendering in the bottom NLE timeline.
    pub fn build_timeline_tracks(&self) -> Vec<TimelineTrack> {
        let mut tracks = Vec::new();
        let comp_dur = self.duration.max(1.0);

        for (node_id, path) in self.node_paths() {
            let Some(def) = self.node_def(node_id) else {
                continue;
            };
            let depth = path.len();
            let badge = node_def_badge(def);
            let label = node_def_display_label(node_id, def);
            let (start_sec, end_sec, all_kfs) =
                node_time_span_and_keyframes(node_id, def, comp_dur);
            let hidden = def.opacity.is_some_and(|o| o <= 0.001);
            let selected = self.selected_node == Some(node_id);

            let anim_count = def.animate.as_ref().map_or(0, |m| m.len());
            let subtitle = if anim_count > 0 {
                format!("{start_sec:.2}s–{end_sec:.2}s · {anim_count} anim")
            } else {
                format!("0.00s–{comp_dur:.2}s · static")
            };

            tracks.push(TimelineTrack {
                kind: TimelineTrackKind::SceneNode {
                    node_id,
                    depth,
                    kind_badge: badge,
                },
                label,
                subtitle,
                start_sec,
                end_sec,
                selected,
                muted_or_hidden: hidden,
                keyframes: all_kfs,
            });

            // When a node is selected (or has animations), emit its property sub-tracks.
            if let Some(anim_map) = &def.animate {
                for (prop, spec) in anim_map {
                    let (p_start, p_end, kfs) = animation_spec_markers(node_id, prop, spec);
                    let easing = spec.easing.clone().unwrap_or_else(|| "linear".to_string());
                    tracks.push(TimelineTrack {
                        kind: TimelineTrackKind::AnimationProperty {
                            node_id,
                            property: prop.clone(),
                            easing: easing.clone(),
                        },
                        label: prop.clone(),
                        subtitle: format!("{p_start:.2}s..{p_end:.2}s ({easing})"),
                        start_sec: p_start,
                        end_sec: p_end,
                        selected: selected && self.active_anim_property == *prop,
                        muted_or_hidden: hidden,
                        keyframes: kfs,
                    });
                }
            }
        }

        if let Some(audio) = &self.scene_def.audio {
            for (idx, tr) in audio.tracks.iter().enumerate() {
                let start = tr.start.max(0.0);
                let end = tr.trim_end.unwrap_or(comp_dur).max(start + 0.1);
                tracks.push(TimelineTrack {
                    kind: TimelineTrackKind::AudioTrack { track_index: idx },
                    label: format!("AUDIO #{idx} · {}", tr.src),
                    subtitle: format!(
                        "{start:.2}s–{end:.2}s · {:+.1} dB{}",
                        tr.gain_db,
                        if tr.muted { " [MUTED]" } else { "" }
                    ),
                    start_sec: start,
                    end_sec: end,
                    selected: false,
                    muted_or_hidden: tr.muted,
                    keyframes: Vec::new(),
                });
            }
        }

        tracks
    }

    /// Collect all sorted keyframe timestamps on the currently selected node (or entire scene).
    pub fn sorted_keyframe_times(&self, target_node: Option<NodeId>) -> Vec<f64> {
        let mut times = Vec::new();
        let Ok(scene) = self.engine.scene(self.scene_handle) else {
            return times;
        };
        for anim in &scene.animations {
            if target_node.is_none() || target_node == Some(anim.node) {
                for kf in &anim.track.keyframes {
                    if kf.time.is_finite() {
                        times.push(kf.time);
                    }
                }
            }
        }
        times.sort_by(|a, b| a.total_cmp(b));
        times.dedup_by(|a, b| (*a - *b).abs() < 1e-4);
        times
    }

    /// Jump playhead to the previous keyframe on the selected node (or scene).
    pub fn timeline_jump_prev_keyframe(&mut self) {
        let times = self.sorted_keyframe_times(self.selected_node);
        let prev = times
            .into_iter()
            .rev()
            .find(|&t| t < self.current_time - 1e-4)
            .unwrap_or(0.0);
        self.seek_to(prev, false);
    }

    /// Jump playhead to the next keyframe on the selected node (or scene).
    pub fn timeline_jump_next_keyframe(&mut self) {
        let times = self.sorted_keyframe_times(self.selected_node);
        let next = times
            .into_iter()
            .find(|&t| t > self.current_time + 1e-4)
            .unwrap_or(self.duration);
        self.seek_to(next, false);
    }

    /// Insert or update a keyframe at `self.current_time` on `self.selected_node`
    /// for `self.active_anim_property` (toolbar `[+ Keyframe (K)]`).
    pub fn timeline_add_keyframe_at_playhead(&mut self) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let prop = if self.active_anim_property.trim().is_empty() {
            "opacity".to_string()
        } else {
            self.active_anim_property.trim().to_string()
        };
        let t = self.current_time;
        let eval_val = self.sample_property_value(node_id, &prop);
        let easing = self.active_easing_input.clone();

        self.mutate_node(
            node_id,
            format!("Add keyframe on {node_id}.{prop} @ {t:.2}s"),
            move |def| {
                let anim_map = def.animate.get_or_insert_with(BTreeMap::new);
                let entry = anim_map
                    .entry(prop.clone())
                    .or_insert_with(|| AnimationDef {
                        from: eval_val,
                        to: eval_val,
                        start: 0.0,
                        end: (t + 1.0).max(1.0),
                        keyframes: Some(vec![
                            KeyframeDef {
                                time: 0.0,
                                value: eval_val,
                            },
                            KeyframeDef {
                                time: (t + 1.0).max(1.0),
                                value: eval_val,
                            },
                        ]),
                        easing: Some(easing.clone()),
                    });

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

                if let Some(existing) = kfs.iter_mut().find(|k| (k.time - t).abs() < 1e-3) {
                    existing.value = eval_val;
                } else {
                    kfs.push(KeyframeDef {
                        time: t,
                        value: eval_val,
                    });
                }
                kfs.sort_by(|a, b| a.time.total_cmp(&b.time));
                if kfs.len() < 2 {
                    kfs.push(KeyframeDef {
                        time: (t + 0.5).max(0.5),
                        value: eval_val,
                    });
                }
                entry.start = kfs.first().map_or(0.0, |k| k.time);
                entry.end = kfs.last().map_or(1.0, |k| k.time);
                entry.keyframes = Some(kfs);
                Ok(())
            },
        )
    }

    /// Split the selected node's animation or video clip at `self.current_time`
    /// (toolbar `[Split @ Playhead (S)]`).
    pub fn timeline_split_at_playhead(&mut self) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let t = self.current_time;

        // If selected node is a video clip, split it into two sequential video clips at playhead.
        let is_video = self
            .node_def(node_id)
            .and_then(|d| d.kind.as_deref())
            .is_some_and(|k| k.eq_ignore_ascii_case("video") || k.eq_ignore_ascii_case("vid"));

        if is_video && node_id != NodeId(0) {
            let path = self
                .path_for_node(node_id)
                .ok_or(StudioError::NodeNotFound(node_id))?;
            if let Some((&child_idx, parent_path)) = path.split_last() {
                let parent_path = parent_path.to_vec();
                return self.commit_mutation(
                    format!("Split video clip {node_id} @ {t:.2}s"),
                    move |scene_def| {
                        let parent = node_at_path_mut(&mut scene_def.root, &parent_path)
                            .ok_or(StudioError::NodeNotFound(node_id))?;
                        let children = parent
                            .children
                            .as_mut()
                            .ok_or(StudioError::NodeNotFound(node_id))?;
                        let orig = children
                            .get_mut(child_idx)
                            .ok_or(StudioError::NodeNotFound(node_id))?;
                        let orig_start = orig.start.unwrap_or(0.0);
                        let split_point = t.max(orig_start + 0.1);
                        let orig_end = orig.trim_end;

                        let mut second_half = orig.clone();
                        orig.trim_end = Some(split_point);
                        second_half.start = Some(split_point);
                        second_half.trim_start =
                            Some(orig.trim_start.unwrap_or(0.0) + (split_point - orig_start));
                        second_half.trim_end = orig_end;

                        children.insert(child_idx + 1, second_half);
                        Ok(())
                    },
                );
            }
        }

        // Otherwise, insert a midpoint keyframe across all animated tracks on the selected node.
        self.timeline_add_keyframe_at_playhead()
    }

    /// Shift all animation/clip start and end times on `self.selected_node` by `delta_sec`.
    /// When `self.ripple_edit` is enabled, subsequent sibling nodes are also shifted.
    pub fn timeline_shift_selected(&mut self, delta_sec: f64) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        let path = self
            .path_for_node(node_id)
            .ok_or(StudioError::NodeNotFound(node_id))?;
        let ripple = self.ripple_edit;

        self.commit_mutation(
            format!("Shift {node_id} timing by {delta_sec:+.2}s"),
            move |scene_def| {
                if ripple && !path.is_empty() {
                    let (&child_idx, parent_path) = path.split_last().unwrap();
                    if let Some(parent) = node_at_path_mut(&mut scene_def.root, parent_path) {
                        if let Some(children) = &mut parent.children {
                            for child in children.iter_mut().skip(child_idx) {
                                shift_node_def_times(child, delta_sec);
                            }
                            return Ok(());
                        }
                    }
                }
                let target = node_at_path_mut(&mut scene_def.root, &path)
                    .ok_or(StudioError::NodeNotFound(node_id))?;
                shift_node_def_times(target, delta_sec);
                Ok(())
            },
        )
    }

    /// Duplicate the selected node as a sibling in `self.scene_def`.
    pub fn timeline_duplicate_selected(&mut self) -> Result<NodeId, StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        if node_id == NodeId(0) {
            return Err(StudioError::Message(
                "cannot duplicate the root container node".to_string(),
            ));
        }
        let path = self
            .path_for_node(node_id)
            .ok_or(StudioError::NodeNotFound(node_id))?;
        let (&child_idx, parent_path) = path
            .split_last()
            .ok_or(StudioError::NodeNotFound(node_id))?;
        let parent_path = parent_path.to_vec();
        let parent_path_for_insert = parent_path.clone();

        self.commit_mutation(format!("Duplicate node {node_id}"), move |scene_def| {
            let parent = node_at_path_mut(&mut scene_def.root, &parent_path_for_insert)
                .ok_or(StudioError::NodeNotFound(node_id))?;
            let children = parent
                .children
                .as_mut()
                .ok_or(StudioError::NodeNotFound(node_id))?;
            let mut copy = children
                .get(child_idx)
                .cloned()
                .ok_or(StudioError::NodeNotFound(node_id))?;

            // Offset the duplicate slightly so it is visually distinct in the viewport.
            if let Some(inset) = &mut copy.inset {
                if let Some(l) = &mut inset.left {
                    *l += 16.0;
                }
                if let Some(t) = &mut inset.top {
                    *t += 16.0;
                }
            }
            children.insert(child_idx + 1, copy);
            Ok(())
        })?;

        let mut dup_path = parent_path;
        dup_path.push(child_idx + 1);
        let new_id = self
            .node_paths()
            .into_iter()
            .find(|(_, p)| *p == dup_path)
            .map(|(id, _)| id)
            .unwrap_or(node_id);
        self.selected_node = Some(new_id);
        let _ = self.render_current_frame();
        Ok(new_id)
    }

    /// Delete the currently selected node from `self.scene_def` (unless it is the root node).
    pub fn timeline_delete_selected(&mut self) -> Result<(), StudioError> {
        let node_id = self.selected_node.unwrap_or(NodeId(0));
        if node_id == NodeId(0) {
            return Err(StudioError::Message(
                "cannot delete the root container node".to_string(),
            ));
        }
        let path = self
            .path_for_node(node_id)
            .ok_or(StudioError::NodeNotFound(node_id))?;
        let (&child_idx, parent_path) = path
            .split_last()
            .ok_or(StudioError::NodeNotFound(node_id))?;
        let parent_path = parent_path.to_vec();

        self.commit_mutation(format!("Delete node {node_id}"), move |scene_def| {
            let parent = node_at_path_mut(&mut scene_def.root, &parent_path)
                .ok_or(StudioError::NodeNotFound(node_id))?;
            let children = parent
                .children
                .as_mut()
                .ok_or(StudioError::NodeNotFound(node_id))?;
            if child_idx >= children.len() {
                return Err(StudioError::NodeNotFound(node_id));
            }
            children.remove(child_idx);
            Ok(())
        })?;

        self.selected_node = Some(NodeId(0));
        let _ = self.render_current_frame();
        Ok(())
    }

    /// Sample the current runtime value of an animatable property on `node_id`.
    pub fn sample_property_value(&self, node_id: NodeId, prop: &str) -> f32 {
        let Some(node) = self.runtime_node(node_id) else {
            return 0.0;
        };
        let rect = node
            .abs_pos
            .unwrap_or(kine_core::Rect::new(0.0, 0.0, 0.0, 0.0));
        match prop {
            "offsetX" => rect.x,
            "offsetY" => rect.y,
            "width" => rect.width,
            "height" => rect.height,
            "opacity" => node.opacity,
            "translateX" => node.transform.translate_x,
            "translateY" => node.transform.translate_y,
            "scale" | "scaleX" => node.transform.scale_x,
            "scaleY" => node.transform.scale_y,
            "rotation" => node.transform.rotation_deg,
            "cornerRadius" => match &node.kind {
                NodeKind::Rect { corner_radius, .. }
                | NodeKind::Image { corner_radius, .. }
                | NodeKind::Video { corner_radius, .. } => *corner_radius,
                _ => 0.0,
            },
            "fontSize" => match &node.kind {
                NodeKind::Text { font_size, .. } => *font_size,
                _ => 16.0,
            },
            _ => 0.0,
        }
    }
}

fn shift_node_def_times(def: &mut NodeDef, delta_sec: f64) {
    if let Some(start) = &mut def.start {
        *start = (*start + delta_sec).max(0.0);
    }
    if let Some(trim_end) = &mut def.trim_end {
        *trim_end = (*trim_end + delta_sec).max(0.1);
    }
    if let Some(anim_map) = &mut def.animate {
        for spec in anim_map.values_mut() {
            let span = (spec.end - spec.start).max(0.05);
            let new_start = (spec.start + delta_sec).max(0.0);
            spec.start = new_start;
            spec.end = new_start + span;
            if let Some(kfs) = &mut spec.keyframes {
                for kf in kfs.iter_mut() {
                    kf.time = (kf.time + delta_sec).max(0.0);
                }
            }
        }
    }
}

pub fn node_def_badge(def: &NodeDef) -> &'static str {
    match def
        .kind
        .as_deref()
        .unwrap_or("container")
        .to_ascii_lowercase()
        .as_str()
    {
        "container" => "CONT",
        "rect" => "RECT",
        "circle" | "ellipse" => "CIRC",
        "path" | "svg" => "PATH",
        "image" | "img" => "IMG",
        "video" | "vid" => "VID",
        "shader" => "SHDR",
        "text" => "TEXT",
        _ => "NODE",
    }
}

pub fn node_def_display_label(id: NodeId, def: &NodeDef) -> String {
    let badge = node_def_badge(def);
    if let Some(text) = &def.content {
        let short: String = text.chars().take(24).collect();
        return format!("{id} [{badge}] \"{short}\"");
    }
    if let Some(src) = &def.src {
        return format!("{id} [{badge}] {src}");
    }
    if def.shader.is_some() {
        let blend = def.blend.as_deref().unwrap_or("normal");
        return format!("{id} [{badge}] WGSL ({blend})");
    }
    if let Some(fill) = &def.fill {
        return format!("{id} [{badge}] {fill}");
    }
    format!("{id} [{badge}]")
}

fn node_time_span_and_keyframes(
    node_id: NodeId,
    def: &NodeDef,
    comp_dur: f64,
) -> (f64, f64, Vec<TimelineKeyframeMarker>) {
    let mut min_t = f64::INFINITY;
    let mut max_t = 0.0f64;
    let mut markers = Vec::new();

    if let Some(start) = def.start {
        min_t = min_t.min(start);
        max_t = max_t.max(def.trim_end.unwrap_or(comp_dur));
    }

    if let Some(anim_map) = &def.animate {
        for (prop, spec) in anim_map {
            let (s, e, mut kfs) = animation_spec_markers(node_id, prop, spec);
            min_t = min_t.min(s);
            max_t = max_t.max(e);
            markers.append(&mut kfs);
        }
    }

    if !min_t.is_finite() {
        min_t = 0.0;
        max_t = comp_dur;
    } else if max_t <= min_t {
        max_t = (min_t + 0.5).min(comp_dur);
    }

    (min_t, max_t, markers)
}

fn animation_spec_markers(
    node_id: NodeId,
    prop: &str,
    spec: &AnimationDef,
) -> (f64, f64, Vec<TimelineKeyframeMarker>) {
    if let Some(kfs) = &spec.keyframes {
        let start = kfs.first().map_or(spec.start, |k| k.time);
        let end = kfs.last().map_or(spec.end, |k| k.time);
        let list = kfs
            .iter()
            .map(|k| TimelineKeyframeMarker {
                node_id,
                property: prop.to_string(),
                time: k.time,
                value: k.value,
            })
            .collect();
        (start, end, list)
    } else {
        let list = vec![
            TimelineKeyframeMarker {
                node_id,
                property: prop.to_string(),
                time: spec.start,
                value: spec.from,
            },
            TimelineKeyframeMarker {
                node_id,
                property: prop.to_string(),
                time: spec.end,
                value: spec.to,
            },
        ];
        (spec.start, spec.end, list)
    }
}
