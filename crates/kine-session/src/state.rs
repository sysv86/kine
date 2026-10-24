//! In-process `StudioState` synchronizing `kine_server::Engine`, authored
//! `kine_script::SceneDef`, undo/redo history, transport clock, settings,
//! UI animations, and render jobs inside a single process without IPC.

use crate::bridge::{SceneFileWatcher, StudioBridgeServer};
use crate::catalog::{
    build_catalog_node, default_assets, default_compositions, starter_kinetic_scene, AssetEntry,
    AssetKind, CompositionEntry,
};
use crate::theme::{
    SettingsCategory, StudioAnimState, StudioSettings, StudioThemeMode, ViewportChannelMode,
};
use kine_core::{NodeId, Rect, Scene};
use kine_platform::{resolve_output, write_output, PlatformInfo};
use kine_render::{FrameFormat, RasterPixmap, RenderBackend};
use kine_script::{AudioDef, AudioTrackDef, EdgesDef, NodeDef, SceneDef, SizeDef};
use kine_server::{
    render_video, Engine, Finding, SceneHandle, VideoContainer, VideoEncoder, VideoOptions,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

const BUNDLED_FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

/// Left dock tabs (matching Hyperframes Studio's `StudioLeftPanels`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LeftDockTab {
    Compositions,
    Assets,
    Code,
    Catalog,
}

impl LeftDockTab {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Compositions => "Compositions",
            Self::Assets => "Assets",
            Self::Code => "Code (TS/JSON)",
            Self::Catalog => "Catalog & FX",
        }
    }
}

/// Right dock tabs (matching Hyperframes Studio's `StudioRightPanels`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RightDockTab {
    Design,
    Layers,
    Renders,
}

impl RightDockTab {
    pub const fn label(self) -> &'static str {
        match self {
            Self::Design => "Design",
            Self::Layers => "Layers",
            Self::Renders => "Renders",
        }
    }
}

/// One reversible snapshot in the persistent edit history stack.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UndoEntry {
    pub label: String,
    pub scene_def: SceneDef,
    pub selected_node: Option<NodeId>,
}

/// Status of an export job in the Render Queue panel.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RenderJobStatus {
    Completed,
    Failed(String),
}

/// A completed or failed export entry in the `Renders` dock panel.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RenderQueueJob {
    pub id: u64,
    pub composition_title: String,
    pub format: String,
    pub output_path: PathBuf,
    pub frames: u32,
    pub fps: u32,
    pub bytes_written: usize,
    pub status: RenderJobStatus,
}

#[derive(Debug, thiserror::Error)]
pub enum StudioError {
    #[error("engine error: {0}")]
    Engine(#[from] kine_server::engine::EngineError),

    #[error("authoring error: {0}")]
    Authoring(#[from] kine_script::AuthoringError),

    #[error("node {0} not found in scene definition")]
    NodeNotFound(NodeId),

    #[error("{0}")]
    Message(String),
}

/// Central studio state living in the same process as [`kine_server::Engine`].
pub struct StudioState {
    /// In-process Kine Engine (shared fonts, video cache, CPU/GPU renderers).
    pub engine: Engine,
    /// Handle of the currently active scene inside `engine`.
    pub scene_handle: SceneHandle,
    /// Authored AST representation synchronized 1:1 with `scene_handle`.
    pub scene_def: SceneDef,
    /// Pretty-printed JSON or TypeScript source buffer for the Code editor tab.
    pub source_buffer: String,
    /// Last code editor diagnostic or status message.
    pub status_banner: String,

    // ── Project & Docks ────────────────────────────────────────────────────
    pub workspace_root: Option<PathBuf>,
    pub compositions: Vec<CompositionEntry>,
    pub active_composition: usize,
    pub assets: Vec<AssetEntry>,
    /// Ready-made modules discovered in the current project's `Modules/` folder.
    pub module_registry: kine_script::ModuleRegistry,
    /// Directory whose `Modules/` library is currently loaded.
    module_dir: Option<PathBuf>,
    pub left_tab: LeftDockTab,
    pub right_tab: RightDockTab,
    pub inspector_visible: bool,

    // ── Settings Window & UI Animation State ───────────────────────────────
    pub settings: StudioSettings,
    pub settings_open: bool,
    pub settings_category: SettingsCategory,
    pub anim: StudioAnimState,
    pub open_inspector_sections: BTreeSet<String>,
    pub catalog_category_filter: String,
    pub viewport_channel: ViewportChannelMode,

    // ── Selection & Undo/Redo ──────────────────────────────────────────────
    pub selected_node: Option<NodeId>,
    pub active_anim_property: String,
    pub active_easing_input: String,
    pub undo_stack: Vec<UndoEntry>,
    pub redo_stack: Vec<UndoEntry>,

    // ── Transport & Playback Clock ─────────────────────────────────────────
    pub current_time: f64,
    pub fps: f64,
    pub duration: f64,
    pub is_playing: bool,
    pub loop_enabled: bool,
    pub playback_rate: f64,
    pub show_frames: bool,
    pub audio_muted: bool,
    pub audio_volume: f32,

    // ── Timeline Controls ──────────────────────────────────────────────────
    pub timeline_zoom_pps: f32,
    pub timeline_snap_enabled: bool,
    pub auto_keyframe: bool,
    pub ripple_edit: bool,

    // ── Theme & Flat Inspector UX State (Hyperframes Parity) ───────────────
    pub dark_theme: bool,
    pub open_inspector_group: String,
    pub collapsed_nodes: BTreeSet<u64>,

    // ── Viewport & Raster Cache ────────────────────────────────────────────
    pub viewport_zoom: f32,
    pub show_bounding_boxes: bool,
    pub show_safe_guides: bool,
    pub show_grid_overlay: bool,
    pub topology_lens_active: bool,
    pub show_audio_meters: bool,
    pub last_raw_pixmap: RasterPixmap,
    pub last_viewport_pixmap: RasterPixmap,
    pub frame_revision: u64,
    /// Wall-clock time of the last preview rasterization, for the status bar.
    pub last_render_ms: f32,
    pub inspect_findings: Vec<Finding>,

    // ── External CLI Agent Bridge & Live File Watcher ──────────────────────
    pub bridge: Option<StudioBridgeServer>,
    pub file_watcher: SceneFileWatcher,

    // ── Render Queue ───────────────────────────────────────────────────────
    pub export_format: String,
    pub export_output_path: String,
    pub render_queue: Vec<RenderQueueJob>,
    next_job_id: u64,
}

impl Default for StudioState {
    fn default() -> Self {
        Self::new(None)
    }
}

impl StudioState {
    /// Initialize Kine Studio with an in-process [`Engine`] and optional workspace directory.
    pub fn new(workspace_root: Option<PathBuf>) -> Self {
        let mut engine = Engine::new();
        engine.load_font("JetBrains Mono", BUNDLED_FONT.to_vec());

        let module_base = workspace_root
            .clone()
            .unwrap_or_else(|| std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")));
        let module_registry =
            kine_script::ModuleRegistry::discover(&module_base).unwrap_or_default();
        let compositions = default_compositions(workspace_root.as_deref());
        let initial_def = compositions
            .first()
            .map(|c| c.scene_def.clone())
            .unwrap_or_else(starter_kinetic_scene);

        let initial_scene = kine_script::scene_from_def(initial_def.clone())
            .unwrap_or_else(|_| Scene::new(640, 360));
        let scene_handle = engine.insert_scene(initial_scene);
        let source_buffer =
            serde_json::to_string_pretty(&initial_def).unwrap_or_else(|_| "{}".to_string());

        let raw_px = RasterPixmap::new(initial_def.width as u16, initial_def.height as u16);

        let mut open_sections = BTreeSet::new();
        open_sections.insert("layout".to_string());
        open_sections.insert("transform".to_string());
        open_sections.insert("style".to_string());
        open_sections.insert("text".to_string());
        open_sections.insert("motion".to_string());
        open_sections.insert("fx".to_string());

        let settings = Self::load_settings_from_disk(workspace_root.as_deref()).unwrap_or_default();
        let dark_theme = settings.theme_mode.is_dark();

        let mut state = Self {
            engine,
            scene_handle,
            scene_def: initial_def,
            source_buffer,
            status_banner: "Ready · In-process Engine synchronized".to_string(),
            workspace_root,
            compositions,
            active_composition: 0,
            assets: default_assets(),
            module_registry,
            module_dir: Some(module_base),
            left_tab: LeftDockTab::Compositions,
            right_tab: RightDockTab::Design,
            inspector_visible: true,
            settings,
            settings_open: false,
            settings_category: SettingsCategory::Appearance,
            anim: StudioAnimState::default(),
            open_inspector_sections: open_sections,
            catalog_category_filter: "All".to_string(),
            viewport_channel: ViewportChannelMode::Rgb,
            selected_node: Some(NodeId(4)),
            active_anim_property: "translateX".to_string(),
            active_easing_input: "easeOutCubic".to_string(),
            undo_stack: Vec::new(),
            redo_stack: Vec::new(),
            current_time: 0.5,
            fps: 30.0,
            duration: 3.0,
            is_playing: false,
            loop_enabled: true,
            playback_rate: 1.0,
            show_frames: false,
            audio_muted: false,
            audio_volume: 0.85,
            timeline_zoom_pps: 140.0,
            timeline_snap_enabled: true,
            auto_keyframe: true,
            ripple_edit: false,
            dark_theme,
            open_inspector_group: "layout".to_string(),
            collapsed_nodes: BTreeSet::new(),
            viewport_zoom: 1.0,
            show_bounding_boxes: true,
            show_safe_guides: false,
            show_grid_overlay: false,
            topology_lens_active: true,
            show_audio_meters: true,
            last_raw_pixmap: raw_px.clone(),
            last_viewport_pixmap: raw_px,
            frame_revision: 0,
            last_render_ms: 0.0,
            inspect_findings: Vec::new(),
            bridge: None,
            file_watcher: SceneFileWatcher::default(),
            export_format: "mp4".to_string(),
            export_output_path: "studio-render.mp4".to_string(),
            render_queue: Vec::new(),
            next_job_id: 1,
        };

        let _ = state.sync_scene_to_engine();
        state
    }

    // ── Settings Persistence & Theme Control ───────────────────────────────

    fn settings_file_path(workspace_root: Option<&Path>) -> PathBuf {
        let base = workspace_root
            .map(Path::to_path_buf)
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."));
        base.join("studio-settings.json")
    }

    fn load_settings_from_disk(workspace_root: Option<&Path>) -> Option<StudioSettings> {
        let path = Self::settings_file_path(workspace_root);
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }

    /// Save `self.settings` to `studio-settings.json` in the workspace root.
    pub fn save_settings_to_disk(&mut self) -> Result<PathBuf, StudioError> {
        let path = Self::settings_file_path(self.workspace_root.as_deref());
        let json = serde_json::to_string_pretty(&self.settings)
            .map_err(|e| StudioError::Message(format!("serialize settings: {e}")))?;
        std::fs::write(&path, json)
            .map_err(|e| StudioError::Message(format!("write {}: {e}", path.display())))?;
        self.notify_status(format!("Saved studio settings to {}", path.display()));
        Ok(path)
    }

    /// Reset all studio settings back to factory defaults.
    pub fn reset_settings_to_defaults(&mut self) {
        self.settings = StudioSettings::default();
        self.dark_theme = self.settings.theme_mode.is_dark();
        self.fps = self.settings.default_fps;
        let _ = self.render_current_frame();
        self.notify_status("Reset all UI & Editor settings to defaults");
    }

    /// Set the active `StudioThemeMode` and synchronize `self.dark_theme`.
    pub fn set_theme_mode(&mut self, mode: StudioThemeMode) {
        self.settings.theme_mode = mode;
        self.dark_theme = mode.is_dark();
        self.notify_status(format!("Theme switched to {}", mode.label()));
    }

    /// Cycle through `GraphiteDark` -> `CarbonObsidian` -> `WarmPaper`.
    pub fn cycle_theme_mode(&mut self) {
        let next = self.settings.theme_mode.next();
        self.set_theme_mode(next);
    }

    /// Toggle whether an inspector section is expanded in the cascading accordion.
    pub fn toggle_inspector_section(&mut self, section_id: &str) {
        if self.open_inspector_sections.contains(section_id) {
            self.open_inspector_sections.remove(section_id);
            if self.open_inspector_group == section_id {
                self.open_inspector_group.clear();
            }
        } else {
            self.open_inspector_sections.insert(section_id.to_string());
            self.open_inspector_group = section_id.to_string();
        }
    }

    /// Check whether an inspector section is currently open.
    pub fn is_inspector_section_open(&self, section_id: &str) -> bool {
        self.open_inspector_group == "all" || self.open_inspector_sections.contains(section_id)
    }

    /// Expand or collapse all cascading inspector sections at once.
    pub fn set_all_inspector_sections(&mut self, expand: bool) {
        self.open_inspector_sections.clear();
        if expand {
            for id in [
                "layout",
                "transform",
                "style",
                "stroke",
                "text",
                "motion",
                "fx",
                "audio",
            ] {
                self.open_inspector_sections.insert(id.to_string());
            }
            self.open_inspector_group = "all".to_string();
        } else {
            self.open_inspector_group.clear();
        }
    }

    /// Set a status message and trigger the animated notification toast timer.
    pub fn notify_status(&mut self, msg: impl Into<String>) {
        self.status_banner = msg.into();
        self.anim.toast_timer = 1.0;
    }

    /// Returns `true` if either playback or a UI transition animation needs a 60Hz tick.
    pub fn needs_animation_tick(&self) -> bool {
        if self.is_playing {
            return true;
        }
        if self.settings.reduced_motion {
            return false;
        }
        let target_settings = if self.settings_open { 1.0 } else { 0.0 };
        let target_left = if self.settings.left_dock_visible {
            1.0
        } else {
            0.0
        };
        let target_right = if self.inspector_visible { 1.0 } else { 0.0 };

        (self.anim.smooth_playhead - self.current_time).abs() > 1e-3
            || (self.anim.settings_modal_t - target_settings).abs() > 0.01
            || (self.anim.left_dock_t - target_left).abs() > 0.01
            || (self.anim.right_dock_t - target_right).abs() > 0.01
            || self.anim.toast_timer > 0.01
    }

    /// Step playback, UI transition animations, external CLI bridge commands, and live file watcher.
    pub fn tick_studio(&mut self, dt_real_seconds: f64) {
        let _ = self.poll_bridge_and_watcher();
        if self.is_playing {
            self.tick_playback(dt_real_seconds);
        }
        self.anim.step(
            dt_real_seconds as f32,
            self.current_time,
            self.settings_open,
            self.settings.left_dock_visible,
            self.inspector_visible,
            &self.open_inspector_sections,
            self.settings.animation_speed,
            self.settings.reduced_motion,
        );
    }

    // ── Project & Modules ──────────────────────────────────────────────────

    /// Base directory used to discover `Modules/` for the active project.
    fn module_base(&self) -> PathBuf {
        if let Some(dir) = &self.module_dir {
            return dir.clone();
        }
        if let Some(root) = &self.workspace_root {
            return root.clone();
        }
        std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."))
    }

    /// Reload the project module library from `base/Modules`.
    pub fn reload_modules(&mut self, base: &Path) -> Result<(), StudioError> {
        self.module_registry = kine_script::ModuleRegistry::discover(base)
            .map_err(|e| StudioError::Message(e.to_string()))?;
        self.module_dir = Some(base.to_path_buf());
        Ok(())
    }

    /// Instantiate a ready-made module and append it to the scene root.
    pub fn insert_module(&mut self, name: &str) -> Result<(), StudioError> {
        let node = self
            .module_registry
            .instantiate(name, serde_json::json!({}))
            .map_err(|e| StudioError::Message(e.to_string()))?;
        self.commit_mutation(format!("Insert module {name}"), move |def| {
            def.root.children.get_or_insert_with(Vec::new).push(node);
            Ok(())
        })?;
        self.notify_status(format!("Inserted module {name}"));
        Ok(())
    }

    /// Insert or apply a built-in/discovered `AssetEntry` into the active scene.
    pub fn insert_or_apply_asset(&mut self, asset_index: usize) -> Result<(), StudioError> {
        let Some(asset) = self.assets.get(asset_index).cloned() else {
            return Err(StudioError::Message(format!(
                "asset index {asset_index} out of bounds"
            )));
        };
        match asset.kind {
            AssetKind::Font => {
                let font_name = asset
                    .uri
                    .strip_prefix("font:")
                    .unwrap_or(&asset.name)
                    .to_string();
                let target = self.selected_node.unwrap_or(NodeId(0));
                let is_text = self
                    .node_def(target)
                    .and_then(|d| d.kind.as_deref())
                    .is_some_and(|k| k.eq_ignore_ascii_case("text"));
                if is_text {
                    self.mutate_node(
                        target,
                        format!("Set font family to {font_name}"),
                        move |d| {
                            d.font_family = Some(font_name);
                            Ok(())
                        },
                    )?;
                    self.notify_status(format!("Applied font {} to {target}", asset.name));
                } else {
                    let _ = self.insert_catalog_item("text-headline")?;
                    self.notify_status(format!("Inserted kinetic headline using {}", asset.name));
                }
            }
            AssetKind::Image => {
                let uri = asset.uri.clone();
                let cw = self.scene_def.width as f32;
                let ch = self.scene_def.height as f32;
                self.commit_mutation(format!("Insert image asset {}", asset.name), move |def| {
                    let children = def.root.children.get_or_insert_with(Vec::new);
                    children.push(NodeDef {
                        kind: Some("image".to_string()),
                        src: Some(uri),
                        object_fit: Some("cover".to_string()),
                        corner_radius: Some(12.0),
                        position: Some("absolute".to_string()),
                        inset: Some(EdgesDef {
                            left: Some((cw * 0.25).round()),
                            top: Some((ch * 0.22).round()),
                            ..Default::default()
                        }),
                        size: Some(SizeDef {
                            width: Some(200.0),
                            height: Some(140.0),
                        }),
                        ..Default::default()
                    });
                    Ok(())
                })?;
                if let Some((last_id, _)) = self.node_paths().last() {
                    self.selected_node = Some(*last_id);
                }
                let _ = self.render_current_frame();
                self.notify_status(format!("Inserted image asset '{}'", asset.name));
            }
            AssetKind::Video => {
                let _ = self.insert_catalog_item("video-testbars")?;
                self.notify_status(format!("Inserted video clip '{}'", asset.name));
            }
            AssetKind::Audio => {
                let uri = asset.uri.clone();
                let name = asset.name.clone();
                self.commit_mutation(format!("Add audio bed {name}"), move |def| {
                    let audio = def.audio.get_or_insert_with(|| AudioDef {
                        sample_rate: 48_000,
                        channels: 2,
                        normalize_peak_db: Some(-1.0),
                        tracks: Vec::new(),
                    });
                    audio.tracks.push(AudioTrackDef {
                        src: uri,
                        gain_db: -5.0,
                        start: 0.0,
                        trim_start: 0.0,
                        trim_end: Some(2.8),
                        loop_track: false,
                        pan: 0.0,
                        fade_in: 0.12,
                        fade_out: 0.30,
                        fade_curve: Some("equalPower".to_string()),
                        highpass_hz: Some(80.0),
                        lowpass_hz: None,
                        eq: None,
                        compressor: None,
                        duck_db: 0.0,
                        duck_threshold_db: None,
                        muted: false,
                    });
                    Ok(())
                })?;
                self.notify_status(format!("Added DSP audio track '{}'", asset.name));
            }
            AssetKind::Shader => {
                let preset_id = if asset.uri.contains("vignette") {
                    "shader-vignette"
                } else {
                    "shader-film-grain"
                };
                let _ = self.insert_catalog_item(preset_id)?;
                self.notify_status(format!("Inserted WGSL shader pass '{}'", asset.name));
            }
        }
        Ok(())
    }

    /// Create a fresh composition using `self.settings.default_canvas_width/height` and select it.
    pub fn create_new_composition(&mut self) -> Result<usize, StudioError> {
        let idx = self.compositions.len() + 1;
        let w = self.settings.default_canvas_width.clamp(128, 3840);
        let h = self.settings.default_canvas_height.clamp(128, 2160);
        let mut def = starter_kinetic_scene();
        def.width = w;
        def.height = h;
        def.root.size = Some(SizeDef {
            width: Some(w as f32),
            height: Some(h as f32),
        });

        let entry = CompositionEntry {
            id: format!("comp-{idx:02}"),
            title: format!("{idx:02} // Studio Composition"),
            subtitle: format!(
                "{w}x{h} · {:.0}fps · Custom scene",
                self.settings.default_fps
            ),
            source_path: None,
            scene_def: def,
        };
        self.compositions.push(entry);
        let new_idx = self.compositions.len() - 1;
        self.select_composition(new_idx)?;
        Ok(new_idx)
    }

    /// Resize the active composition canvas (`width`, `height`) inside an undoable transaction.
    pub fn resize_canvas(&mut self, width: u32, height: u32) -> Result<(), StudioError> {
        let w = width.clamp(64, 3840);
        let h = height.clamp(64, 2160);
        self.commit_mutation(format!("Resize canvas to {w}x{h}"), move |def| {
            def.width = w;
            def.height = h;
            if let Some(sz) = &mut def.root.size {
                sz.width = Some(w as f32);
                sz.height = Some(h as f32);
            }
            Ok(())
        })
    }

    /// Format `self.source_buffer` as canonical pretty-printed JSON from `self.scene_def`.
    pub fn format_source_buffer(&mut self) {
        if let Ok(pretty) = serde_json::to_string_pretty(&self.scene_def) {
            self.source_buffer = pretty;
            self.notify_status("Formatted scene source buffer (JSON AST)");
        }
    }

    /// Load a `.ts` or `.json` scene file from disk into the studio.
    pub fn load_scene_file(&mut self, path: &Path) -> Result<(), StudioError> {
        let content = std::fs::read_to_string(path)
            .map_err(|e| StudioError::Message(format!("read {}: {e}", path.display())))?;
        let base = path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(Path::to_path_buf)
            .unwrap_or_else(|| self.module_base());
        self.reload_modules(&base)?;
        let def: SceneDef = if content.trim_start().starts_with('{') {
            serde_json::from_str(&content)
                .map_err(|e| StudioError::Message(format!("parse JSON {}: {e}", path.display())))?
        } else {
            self.module_registry
                .eval_scene_ts(&content)
                .map_err(|e| StudioError::Message(format!("eval TS {}: {e}", path.display())))?
        };
        self.scene_def = def.clone();
        self.source_buffer =
            serde_json::to_string_pretty(&self.scene_def).unwrap_or_else(|_| content);

        let title = path
            .file_stem()
            .and_then(|s| s.to_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| path.display().to_string());
        let subtitle = format!(
            "{}x{} · loaded from {}",
            def.width,
            def.height,
            path.display()
        );
        if let Some(index) = self
            .compositions
            .iter()
            .position(|entry| entry.source_path.as_deref() == Some(path))
        {
            self.compositions[index].title = format!("File // {title}");
            self.compositions[index].subtitle = subtitle;
            self.compositions[index].scene_def = def.clone();
            self.active_composition = index;
        } else {
            self.compositions.push(CompositionEntry {
                id: format!("file-{}", self.compositions.len()),
                title: format!("File // {title}"),
                subtitle,
                source_path: Some(path.to_path_buf()),
                scene_def: def.clone(),
            });
            self.active_composition = self.compositions.len() - 1;
        }

        self.undo_stack.clear();
        self.redo_stack.clear();
        self.current_time = 0.0;
        self.anim.smooth_playhead = 0.0;
        self.selected_node = Some(NodeId(0));
        self.sync_scene_to_engine()?;
        self.file_watcher.watch(Some(path.to_path_buf()), Some(&base));
        self.notify_status(format!("Loaded scene {}", path.display()));
        Ok(())
    }

    /// Switch to one of the compositions in `self.compositions`.
    pub fn select_composition(&mut self, index: usize) -> Result<(), StudioError> {
        let Some(entry) = self.compositions.get(index).cloned() else {
            return Err(StudioError::Message(format!(
                "composition index {index} out of range"
            )));
        };
        self.active_composition = index;
        if let Some(base) = entry
            .source_path
            .as_ref()
            .and_then(|path| path.parent())
            .map(Path::to_path_buf)
        {
            let _ = self.reload_modules(&base);
        }
        self.scene_def = entry.scene_def;
        self.source_buffer =
            serde_json::to_string_pretty(&self.scene_def).unwrap_or_else(|_| "{}".to_string());
        self.undo_stack.clear();
        self.redo_stack.clear();
        self.current_time = 0.0;
        self.anim.smooth_playhead = 0.0;
        self.selected_node = Some(NodeId(0));
        self.sync_scene_to_engine()?;
        let base = self.module_base();
        self.file_watcher.watch(entry.source_path.clone(), Some(&base));
        self.notify_status(format!("Switched to composition: {}", entry.title));
        Ok(())
    }

    /// Rebuild `kine_core::Scene` from `self.scene_def`, update the in-process
    /// `Engine`, run static inspection, and rasterize the current playhead frame.
    pub fn sync_scene_to_engine(&mut self) -> Result<(), StudioError> {
        let new_scene = kine_script::scene_from_def(self.scene_def.clone())?;
        let old_handle = self.scene_handle;
        let new_handle = self.engine.insert_scene(new_scene);
        self.scene_handle = new_handle;
        let _ = self.engine.free_scene(old_handle);

        self.duration = self.compute_composition_duration();
        if self.current_time > self.duration {
            self.current_time = self.duration;
        }
        self.source_buffer =
            serde_json::to_string_pretty(&self.scene_def).unwrap_or_else(|_| "{}".to_string());

        if let Some(comp) = self.compositions.get_mut(self.active_composition) {
            comp.scene_def = self.scene_def.clone();
        }

        if let Ok(report) = self.engine.inspect(self.scene_handle) {
            self.inspect_findings = report.findings;
        }

        // Validate that selected_node still exists in the rebuilt scene.
        if let Some(sel) = self.selected_node {
            let exists = self
                .engine
                .scene(self.scene_handle)
                .map(|s| s.nodes.contains_key(&sel))
                .unwrap_or(false);
            if !exists {
                self.selected_node = Some(NodeId(0));
            }
        }

        self.render_current_frame()
    }

    /// Re-evaluate animations & layout at `self.current_time` and rasterize the frame in-process.
    pub fn render_current_frame(&mut self) -> Result<(), StudioError> {
        let started = std::time::Instant::now();
        let frame_idx = self.current_frame_index();
        let pixmap = self.engine.render_frame_at_with_index(
            self.scene_handle,
            self.current_time,
            frame_idx,
        )?;
        let viewport_px = self.compose_viewport_overlay(&pixmap);
        self.last_raw_pixmap = pixmap;
        self.last_viewport_pixmap = viewport_px;
        self.frame_revision = self.frame_revision.wrapping_add(1);
        self.last_render_ms = started.elapsed().as_secs_f32() * 1000.0;
        Ok(())
    }

    /// Draw interactive bounding boxes, resize handles, channel isolation, and safe-area guides onto a copy of the frame.
    fn compose_viewport_overlay(&self, raw: &RasterPixmap) -> RasterPixmap {
        let w = raw.width();
        let h = raw.height();
        let mut out = RasterPixmap::new(w, h);
        out.data_mut().copy_from_slice(raw.data());

        // Apply channel inspection filter if non-RGB
        if self.viewport_channel != ViewportChannelMode::Rgb {
            for px in out.data_mut() {
                match self.viewport_channel {
                    ViewportChannelMode::Rgb => {}
                    ViewportChannelMode::Red => {
                        px.g = 0;
                        px.b = 0;
                    }
                    ViewportChannelMode::Green => {
                        px.r = 0;
                        px.b = 0;
                    }
                    ViewportChannelMode::Blue => {
                        px.r = 0;
                        px.g = 0;
                    }
                    ViewportChannelMode::Alpha => {
                        let a = px.a;
                        px.r = a;
                        px.g = a;
                        px.b = a;
                        px.a = 255;
                    }
                    ViewportChannelMode::Luma => {
                        let y = ((u32::from(px.r) * 299
                            + u32::from(px.g) * 587
                            + u32::from(px.b) * 114)
                            / 1000) as u8;
                        px.r = y;
                        px.g = y;
                        px.b = y;
                    }
                }
            }
        }

        if self.show_safe_guides {
            // 10% title safe and 5% action safe reticles
            let action_rect = Rect::new(
                f32::from(w) * 0.05,
                f32::from(h) * 0.05,
                f32::from(w) * 0.90,
                f32::from(h) * 0.90,
            );
            draw_rect_outline(&mut out, action_rect, [148, 163, 184, 110], 1, true);
            let title_rect = Rect::new(
                f32::from(w) * 0.10,
                f32::from(h) * 0.10,
                f32::from(w) * 0.80,
                f32::from(h) * 0.80,
            );
            draw_rect_outline(&mut out, title_rect, [56, 189, 248, 90], 1, true);
        }

        if self.show_grid_overlay {
            let fw = f32::from(w);
            let fh = f32::from(h);
            // Rule of thirds + optional center crosshair
            draw_rect_outline(
                &mut out,
                Rect::new(fw / 3.0, 0.0, fw / 3.0, fh),
                [244, 244, 245, 55],
                1,
                true,
            );
            draw_rect_outline(
                &mut out,
                Rect::new(0.0, fh / 3.0, fw, fh / 3.0),
                [244, 244, 245, 55],
                1,
                true,
            );
            if self.settings.show_center_crosshair {
                let cx = (fw * 0.5).round() as i32;
                let cy = (fh * 0.5).round() as i32;
                for d in -10..=10 {
                    blend_pixel(&mut out, cx + d, cy, [61, 228, 168, 130]);
                    blend_pixel(&mut out, cx, cy + d, [61, 228, 168, 130]);
                }
            }
        }

        if self.show_bounding_boxes {
            if let Some(sel_id) = self.selected_node {
                if sel_id != NodeId(0) {
                    if let Some(rect) = self.node_abs_rect(sel_id) {
                        let ring_color = if self.topology_lens_active {
                            [60, 230, 172, 255] // Hyperframes Mint (#3ce6ac)
                        } else {
                            [75, 163, 210, 255] // Hyperframes Selection Sky (#4ba3d2)
                        };
                        draw_rect_outline(&mut out, rect, ring_color, 2, false);
                        draw_corner_handles(&mut out, rect, ring_color);
                    }
                }
            }
        }

        out
    }

    /// Hit-test a canvas pixel coordinate `(canvas_x, canvas_y)` and select the topmost matching node.
    pub fn hit_test_and_select(&mut self, canvas_x: f32, canvas_y: f32) -> Option<NodeId> {
        let mut hit = NodeId(0);
        for (nid, _) in self.node_paths().into_iter().skip(1) {
            let visible = self
                .node_def(nid)
                .map_or(true, |d| !d.opacity.is_some_and(|o| o <= 0.001));
            if !visible {
                continue;
            }
            if let Some(r) = self.node_abs_rect(nid) {
                if canvas_x >= r.x
                    && canvas_x <= r.x + r.width
                    && canvas_y >= r.y
                    && canvas_y <= r.y + r.height
                {
                    hit = nid;
                }
            }
        }
        self.selected_node = Some(hit);
        let _ = self.render_current_frame();
        Some(hit)
    }

    /// Compute total composition duration across keyframe tracks, video clips, and audio tracks.
    pub fn compute_composition_duration(&self) -> f64 {
        let mut max_sec = self
            .engine
            .scene(self.scene_handle)
            .map(|s| s.duration_seconds())
            .unwrap_or(0.0);

        if let Some(audio) = &self.scene_def.audio {
            for track in &audio.tracks {
                let end = track.trim_end.unwrap_or(track.start + 2.5);
                if end > max_sec {
                    max_sec = end;
                }
            }
        }

        max_sec.max(self.settings.default_duration.max(1.0))
    }

    /// Current 0-based frame index at `self.current_time` and `self.fps`.
    pub fn current_frame_index(&self) -> u32 {
        (self.current_time * self.fps.max(1.0)).round().max(0.0) as u32
    }

    /// Total frame count in the composition.
    pub fn total_frames(&self) -> u32 {
        (self.duration * self.fps.max(1.0)).round().max(1.0) as u32
    }

    /// Format `self.current_time` as either `Frame F / Total` or `MM:SS.ms`.
    pub fn formatted_timecode(&self) -> String {
        if self.show_frames {
            format!(
                "F {:03} / {:03}",
                self.current_frame_index(),
                self.total_frames()
            )
        } else {
            let total_secs = self.current_time.max(0.0);
            let mins = (total_secs / 60.0).floor() as u32;
            let secs = total_secs % 60.0;
            let dur_mins = (self.duration / 60.0).floor() as u32;
            let dur_secs = self.duration % 60.0;
            format!("{mins:02}:{secs:05.2} / {dur_mins:02}:{dur_secs:05.2}")
        }
    }

    // ── Transport & Seeking ────────────────────────────────────────────────

    /// Seek to an explicit timestamp in seconds (optionally snapping when requested).
    pub fn seek_to(&mut self, time_sec: f64, snap: bool) {
        let mut t = time_sec.clamp(0.0, self.duration.max(0.1));
        if snap && self.timeline_snap_enabled {
            t = self.snap_time(t);
        }
        self.current_time = t;
        self.anim.smooth_playhead = t;
        let _ = self.render_current_frame();
    }

    /// Step by `delta_frames` (e.g. `-1` or `+1`).
    pub fn step_frames(&mut self, delta_frames: i32) {
        self.is_playing = false;
        let dt = f64::from(delta_frames) / self.fps.max(1.0);
        let next = (self.current_time + dt).clamp(0.0, self.duration);
        self.current_time = next;
        self.anim.smooth_playhead = next;
        let _ = self.render_current_frame();
    }

    /// Advance the playback clock by `dt_real_seconds` when `self.is_playing` is true.
    pub fn tick_playback(&mut self, dt_real_seconds: f64) {
        if !self.is_playing {
            return;
        }
        let previous_frame = self.current_frame_index();
        let next = self.current_time + dt_real_seconds * self.playback_rate;
        if next >= self.duration {
            if self.loop_enabled {
                self.current_time = next % self.duration.max(0.1);
            } else {
                self.current_time = self.duration;
                self.is_playing = false;
            }
        } else {
            self.current_time = next.max(0.0);
        }
        self.anim.smooth_playhead = self.current_time;
        if self.current_frame_index() != previous_frame || !self.is_playing {
            let _ = self.render_current_frame();
        }
    }

    /// Snap a timestamp to the nearest keyframe, quarter-second tick, or frame boundary.
    pub fn snap_time(&self, raw_time: f64) -> f64 {
        let threshold = self.settings.snap_tolerance_sec.clamp(0.005, 0.25);
        // 1. Check existing keyframes across the scene
        if let Ok(scene) = self.engine.scene(self.scene_handle) {
            for anim in &scene.animations {
                for kf in &anim.track.keyframes {
                    if (kf.time - raw_time).abs() <= threshold {
                        return kf.time;
                    }
                }
            }
        }
        // 2. Check quarter-second ruler ticks
        let quarter = (raw_time * 4.0).round() / 4.0;
        if (quarter - raw_time).abs() <= threshold * 0.5 {
            return quarter;
        }
        // 3. Snap to exact frame grid
        let fps = self.fps.max(1.0);
        (raw_time * fps).round() / fps
    }

    // ── Node Lookup & Bidirectional AST Mapping ────────────────────────────

    /// Build the preorder mapping from `NodeId` (`#0`, `#1`, ...) to its child index path in `SceneDef`.
    pub fn node_paths(&self) -> Vec<(NodeId, Vec<usize>)> {
        let mut out = Vec::new();
        let mut next_id = 1u64;
        out.push((NodeId(0), Vec::new()));
        collect_node_paths(
            &self.scene_def.root,
            &mut Vec::new(),
            &mut next_id,
            &mut out,
        );
        out
    }

    /// Resolve a `NodeId` to its tree path in `self.scene_def`.
    pub fn path_for_node(&self, id: NodeId) -> Option<Vec<usize>> {
        self.node_paths()
            .into_iter()
            .find(|(nid, _)| *nid == id)
            .map(|(_, path)| path)
    }

    /// Check if any ancestor of `id` is in `self.collapsed_nodes`.
    pub fn is_node_hidden_by_collapse(&self, id: NodeId) -> bool {
        if self.collapsed_nodes.is_empty() || id == NodeId(0) {
            return false;
        }
        let all_paths = self.node_paths();
        let Some((_, target_path)) = all_paths.iter().find(|(nid, _)| *nid == id) else {
            return false;
        };
        for depth in 0..target_path.len() {
            let prefix = &target_path[..depth];
            if let Some((ancestor_id, _)) = all_paths.iter().find(|(_, p)| p.as_slice() == prefix) {
                if self.collapsed_nodes.contains(&ancestor_id.0) {
                    return true;
                }
            }
        }
        false
    }

    /// Toggle whether a container node's children are collapsed in the Layers & Timeline tree.
    pub fn toggle_node_collapsed(&mut self, id: NodeId) {
        if self.collapsed_nodes.contains(&id.0) {
            self.collapsed_nodes.remove(&id.0);
        } else {
            self.collapsed_nodes.insert(id.0);
        }
    }

    /// Borrow the authored `NodeDef` corresponding to `id`.
    pub fn node_def(&self, id: NodeId) -> Option<&NodeDef> {
        let path = self.path_for_node(id)?;
        node_at_path(&self.scene_def.root, &path)
    }

    /// Mutably borrow the authored `NodeDef` corresponding to `id`.
    pub fn node_def_mut(&mut self, id: NodeId) -> Option<&mut NodeDef> {
        let path = self.path_for_node(id)?;
        node_at_path_mut(&mut self.scene_def.root, &path)
    }

    /// Read the computed layout bounding box (`Rect`) for `id` at the current playhead time.
    pub fn node_abs_rect(&self, id: NodeId) -> Option<Rect> {
        self.engine
            .scene(self.scene_handle)
            .ok()
            .and_then(|s| s.abs_rect(id))
    }

    /// Read the runtime [`kine_core::Node`] for `id` at the current playhead time.
    pub fn runtime_node(&self, id: NodeId) -> Option<&kine_core::Node> {
        self.engine
            .scene(self.scene_handle)
            .ok()
            .and_then(|s| s.nodes.get(&id))
    }

    // ── Undo / Redo & Transactional Scene Mutations ────────────────────────

    /// Apply a mutation to `self.scene_def` with automatic undo checkpointing and in-process engine sync.
    /// If the mutated `SceneDef` fails validation, the previous `SceneDef` is restored atomically.
    pub fn commit_mutation<F>(
        &mut self,
        undo_label: impl Into<String>,
        mutate: F,
    ) -> Result<(), StudioError>
    where
        F: FnOnce(&mut SceneDef) -> Result<(), StudioError>,
    {
        let label = undo_label.into();
        let snapshot = UndoEntry {
            label: label.clone(),
            scene_def: self.scene_def.clone(),
            selected_node: self.selected_node,
        };

        let mut candidate = self.scene_def.clone();
        mutate(&mut candidate)?;

        // Validate that candidate compiles into a valid Scene before committing.
        let _validated = kine_script::scene_from_def(candidate.clone())?;

        self.undo_stack.push(snapshot);
        self.redo_stack.clear();
        self.scene_def = candidate;
        self.sync_scene_to_engine()?;
        self.notify_status(format!("Committed: {label}"));
        Ok(())
    }

    /// Mutate the specific `NodeDef` for `node_id` inside a checkpointed transaction.
    pub fn mutate_node<F>(
        &mut self,
        node_id: NodeId,
        undo_label: impl Into<String>,
        mutate: F,
    ) -> Result<(), StudioError>
    where
        F: FnOnce(&mut NodeDef) -> Result<(), StudioError>,
    {
        let path = self
            .path_for_node(node_id)
            .ok_or(StudioError::NodeNotFound(node_id))?;
        self.commit_mutation(undo_label, move |scene_def| {
            let target = node_at_path_mut(&mut scene_def.root, &path)
                .ok_or(StudioError::NodeNotFound(node_id))?;
            mutate(target)
        })
    }

    /// Undo the most recent edit and re-synchronize the in-process engine.
    pub fn undo(&mut self) -> Option<String> {
        let entry = self.undo_stack.pop()?;
        let current = UndoEntry {
            label: entry.label.clone(),
            scene_def: self.scene_def.clone(),
            selected_node: self.selected_node,
        };
        self.redo_stack.push(current);
        self.scene_def = entry.scene_def;
        self.selected_node = entry.selected_node;
        let _ = self.sync_scene_to_engine();
        self.notify_status(format!("Undo: {}", entry.label));
        Some(entry.label)
    }

    /// Redo the most recently undone edit and re-synchronize the in-process engine.
    pub fn redo(&mut self) -> Option<String> {
        let entry = self.redo_stack.pop()?;
        let current = UndoEntry {
            label: entry.label.clone(),
            scene_def: self.scene_def.clone(),
            selected_node: self.selected_node,
        };
        self.undo_stack.push(current);
        self.scene_def = entry.scene_def;
        self.selected_node = entry.selected_node;
        let _ = self.sync_scene_to_engine();
        self.notify_status(format!("Redo: {}", entry.label));
        Some(entry.label)
    }

    /// Apply changes from `self.source_buffer` (either JSON or TypeScript DSL) into the scene.
    pub fn apply_source_buffer(&mut self) -> Result<(), StudioError> {
        let src = self.source_buffer.clone();
        let trimmed = src.trim_start();
        let base = self.module_base();
        self.reload_modules(&base)?;
        let parsed: SceneDef = if trimmed.starts_with('{') {
            serde_json::from_str(&src)
                .map_err(|e| StudioError::Message(format!("JSON syntax error: {e}")))?
        } else {
            self.module_registry
                .eval_scene_ts(&src)
                .map_err(|e| StudioError::Message(format!("TypeScript error: {e}")))?
        };
        self.commit_mutation("Apply Code Editor changes", move |def| {
            *def = parsed;
            Ok(())
        })
    }

    /// Insert a preset item from `CATALOG_ITEMS` into the scene root (or audio bed).
    pub fn insert_catalog_item(&mut self, item_id: &str) -> Result<NodeId, StudioError> {
        if item_id == "audio-synth-chime" {
            self.commit_mutation("Add Synth Audio Track", |def| {
                let audio = def.audio.get_or_insert_with(|| AudioDef {
                    sample_rate: 48_000,
                    channels: 2,
                    normalize_peak_db: Some(-1.0),
                    tracks: Vec::new(),
                });
                audio.tracks.push(AudioTrackDef {
                    src: "synth:sine:440:2.0:0.3".to_string(),
                    gain_db: -4.0,
                    start: 0.0,
                    trim_start: 0.0,
                    trim_end: Some(2.0),
                    loop_track: false,
                    pan: 0.0,
                    fade_in: 0.1,
                    fade_out: 0.25,
                    fade_curve: Some("equalPower".to_string()),
                    highpass_hz: Some(80.0),
                    lowpass_hz: None,
                    eq: None,
                    compressor: None,
                    duck_db: 0.0,
                    duck_threshold_db: None,
                    muted: false,
                });
                Ok(())
            })?;
            return Ok(self.selected_node.unwrap_or(NodeId(0)));
        }

        let node_def = build_catalog_node(item_id, self.scene_def.width, self.scene_def.height)
            .ok_or_else(|| StudioError::Message(format!("unknown catalog item: {item_id}")))?;

        self.commit_mutation(format!("Insert catalog block '{item_id}'"), move |def| {
            let children = def.root.children.get_or_insert_with(Vec::new);
            children.push(node_def);
            Ok(())
        })?;

        let new_id = self
            .node_paths()
            .last()
            .map(|(id, _)| *id)
            .unwrap_or(NodeId(0));
        self.selected_node = Some(new_id);
        let _ = self.render_current_frame();
        Ok(new_id)
    }

    // ── Frame Capture & Render Queue Export ────────────────────────────────

    /// Capture the current playhead frame to disk (`PNG`).
    pub fn capture_current_frame(
        &mut self,
        format: FrameFormat,
        out_path: &Path,
    ) -> Result<PathBuf, StudioError> {
        let host = PlatformInfo::detect();
        let resolved = resolve_output(out_path, &host);
        let frame_idx = u64::from(self.current_frame_index());
        let bytes = self
            .engine
            .render_frame_format(self.scene_handle, frame_idx, format)?;
        write_output(&resolved, &bytes)
            .map_err(|e| StudioError::Message(format!("write {}: {e}", resolved.display())))?;
        self.notify_status(format!(
            "Captured frame {} ({} bytes) -> {}",
            frame_idx,
            bytes.len(),
            resolved.display()
        ));
        Ok(resolved)
    }

    /// Run a full composition export (`MP4`, `MKV`, `WebM`, `Y4M`, `GIF`, `KVID`, `PNG`)
    /// directly in-process and record the result in `self.render_queue`.
    pub fn run_export_job(&mut self) -> Result<RenderQueueJob, StudioError> {
        let host = PlatformInfo::detect();
        let out_path = resolve_output(Path::new(&self.export_output_path), &host);
        let comp_title = self
            .compositions
            .get(self.active_composition)
            .map(|c| c.title.clone())
            .unwrap_or_else(|| "Composition".to_string());
        let fps = self.fps.round().max(1.0) as u32;
        let frames = self.total_frames().max(1);
        let fmt = self.export_format.to_ascii_lowercase();

        let job_id = self.next_job_id;
        self.next_job_id += 1;

        if fmt == "png" {
            let bytes = self.engine.render_frame_format(
                self.scene_handle,
                u64::from(self.current_frame_index()),
                FrameFormat::Png,
            )?;
            write_output(&out_path, &bytes)
                .map_err(|e| StudioError::Message(format!("write {}: {e}", out_path.display())))?;
            let job = RenderQueueJob {
                id: job_id,
                composition_title: comp_title,
                format: fmt,
                output_path: out_path.clone(),
                frames: 1,
                fps,
                bytes_written: bytes.len(),
                status: RenderJobStatus::Completed,
            };
            self.render_queue.push(job.clone());
            self.notify_status(format!("Rendered still to {}", out_path.display()));
            return Ok(job);
        }

        let Some(container) = VideoContainer::from_extension(&fmt) else {
            return Err(StudioError::Message(format!(
                "unsupported export format: {fmt}"
            )));
        };
        let mut opts = VideoOptions::default();
        opts.frames = u64::from(frames);
        opts.fps_num = fps;
        opts.fps_den = 1;
        opts.container = container;
        opts.encoder = VideoEncoder::Av1Software;
        opts.quantizer = self.settings.export_quantizer as usize;
        opts.speed = self.settings.export_speed;

        match render_video(&mut self.engine, self.scene_handle, &opts) {
            Ok(report) => {
                let bytes_len = report.bytes.len();
                write_output(&out_path, &report.bytes).map_err(|e| {
                    StudioError::Message(format!("write {}: {e}", out_path.display()))
                })?;
                let job = RenderQueueJob {
                    id: job_id,
                    composition_title: comp_title,
                    format: fmt,
                    output_path: out_path.clone(),
                    frames: report.frames as u32,
                    fps,
                    bytes_written: bytes_len,
                    status: RenderJobStatus::Completed,
                };
                self.render_queue.push(job.clone());
                self.notify_status(format!(
                    "Rendered {} frames ({} B) -> {}",
                    report.frames,
                    bytes_len,
                    out_path.display()
                ));
                Ok(job)
            }
            Err(e) => {
                let job = RenderQueueJob {
                    id: job_id,
                    composition_title: comp_title,
                    format: fmt,
                    output_path: out_path,
                    frames,
                    fps,
                    bytes_written: 0,
                    status: RenderJobStatus::Failed(e.to_string()),
                };
                self.render_queue.push(job.clone());
                Err(StudioError::Engine(e))
            }
        }
    }

    /// Cycle through `RenderBackend::Auto` -> `RenderBackend::Cpu` -> `RenderBackend::Gpu`.
    pub fn cycle_render_backend(&mut self) {
        let next = match self.engine.backend() {
            RenderBackend::Auto => RenderBackend::Cpu,
            RenderBackend::Cpu => {
                if self.engine.gpu_available() {
                    RenderBackend::Gpu
                } else {
                    RenderBackend::Auto
                }
            }
            RenderBackend::Gpu => RenderBackend::Auto,
        };
        self.engine.set_backend(next);
        let _ = self.render_current_frame();
        self.notify_status(format!(
            "Render backend set to {}",
            next.name().to_uppercase()
        ));
    }
}

fn collect_node_paths(
    node: &NodeDef,
    current_path: &mut Vec<usize>,
    next_id: &mut u64,
    out: &mut Vec<(NodeId, Vec<usize>)>,
) {
    if let Some(children) = &node.children {
        for (idx, child) in children.iter().enumerate() {
            let child_id = NodeId(*next_id);
            *next_id += 1;
            current_path.push(idx);
            out.push((child_id, current_path.clone()));
            collect_node_paths(child, current_path, next_id, out);
            current_path.pop();
        }
    }
}

fn node_at_path<'a>(root: &'a NodeDef, path: &[usize]) -> Option<&'a NodeDef> {
    let mut cur = root;
    for &idx in path {
        cur = cur.children.as_ref()?.get(idx)?;
    }
    Some(cur)
}

pub(crate) fn node_at_path_mut<'a>(
    root: &'a mut NodeDef,
    path: &[usize],
) -> Option<&'a mut NodeDef> {
    let mut cur = root;
    for &idx in path {
        cur = cur.children.as_mut()?.get_mut(idx)?;
    }
    Some(cur)
}

fn blend_pixel(pixmap: &mut RasterPixmap, x: i32, y: i32, rgba: [u8; 4]) {
    let w = i32::from(pixmap.width());
    let h = i32::from(pixmap.height());
    if x < 0 || y < 0 || x >= w || y >= h {
        return;
    }
    let idx = (y as usize) * (w as usize) + (x as usize);
    let dst = &mut pixmap.data_mut()[idx];
    let sa = u32::from(rgba[3]);
    let inv = 255u32.saturating_sub(sa);
    let sr = (u32::from(rgba[0]) * sa + 127) / 255;
    let sg = (u32::from(rgba[1]) * sa + 127) / 255;
    let sb = (u32::from(rgba[2]) * sa + 127) / 255;
    dst.r = (sr + (u32::from(dst.r) * inv + 127) / 255).min(255) as u8;
    dst.g = (sg + (u32::from(dst.g) * inv + 127) / 255).min(255) as u8;
    dst.b = (sb + (u32::from(dst.b) * inv + 127) / 255).min(255) as u8;
    dst.a = (sa + (u32::from(dst.a) * inv + 127) / 255).min(255) as u8;
}

fn draw_rect_outline(
    pixmap: &mut RasterPixmap,
    rect: Rect,
    rgba: [u8; 4],
    thickness: i32,
    dashed: bool,
) {
    let x0 = rect.x.round() as i32;
    let y0 = rect.y.round() as i32;
    let x1 = (rect.x + rect.width).round() as i32;
    let y1 = (rect.y + rect.height).round() as i32;

    for t in 0..thickness.max(1) {
        for x in (x0 - t)..=(x1 + t) {
            if !dashed || ((x / 6) % 2 == 0) {
                blend_pixel(pixmap, x, y0 - t, rgba);
                blend_pixel(pixmap, x, y1 + t, rgba);
            }
        }
        for y in (y0 - t)..=(y1 + t) {
            if !dashed || ((y / 6) % 2 == 0) {
                blend_pixel(pixmap, x0 - t, y, rgba);
                blend_pixel(pixmap, x1 + t, y, rgba);
            }
        }
    }
}

fn draw_corner_handles(pixmap: &mut RasterPixmap, rect: Rect, rgba: [u8; 4]) {
    let x0 = rect.x.round() as i32;
    let y0 = rect.y.round() as i32;
    let x1 = (rect.x + rect.width).round() as i32;
    let y1 = (rect.y + rect.height).round() as i32;
    let xm = (rect.x + rect.width * 0.5).round() as i32;
    let ym = (rect.y + rect.height * 0.5).round() as i32;

    let handles = [
        (x0, y0),
        (x1, y0),
        (x0, y1),
        (x1, y1),
        (xm, y0),
        (xm, y1),
        (x0, ym),
        (x1, ym),
    ];
    for (cx, cy) in handles {
        for dy in -3i32..=3 {
            for dx in -3i32..=3 {
                let is_border = dx.abs() == 3 || dy.abs() == 3;
                let fill = if is_border {
                    rgba
                } else {
                    [255, 255, 255, 255]
                };
                blend_pixel(pixmap, cx + dx, cy + dy, fill);
            }
        }
    }
    // Center anchor reticle
    for d in -4..=4 {
        blend_pixel(pixmap, xm + d, ym, rgba);
        blend_pixel(pixmap, xm, ym + d, rgba);
    }
}
