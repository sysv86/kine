//! Engine: scene registry + dual-backend frame rendering (GPU `vello_gpu` /
//! CPU `vello_cpu` + `kine_shader::vm`) + multi-format export (`PNG`, `QOI`,
//! `BMP`, `PPM`, `PAM`, `TGA`, `SVG`) + static inspection + contact-sheet
//! (`strip` / `onion`) + pixel-diff gate + diagnostics.

use base64::engine::general_purpose::STANDARD as B64;
use base64::Engine as _;
use kine_core::{NodeId, NodeKind, Scene};
use kine_render::{
    encode_pixmap, png, scene_to_svg, FrameFormat, FrameRenderer, RasterPixmap, RenderBackend,
    RenderError,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Engine diagnostics payload (`doctor`).
#[derive(Debug, Serialize)]
pub struct DoctorReport {
    pub version: &'static str,
    pub renderer: &'static str,
    pub backend_preference: &'static str,
    pub gpu_available: bool,
    pub cpu_shader_vm: bool,
    pub formats: Vec<&'static str>,
    pub fonts: Vec<String>,
    pub platform: &'static str,
    /// Host environment policy: Linux/macOS detection, graphics backend, output directory, notes.
    pub host: kine_platform::PlatformInfo,
    /// Whether a hardware H.264 encoder is compiled in *and* usable here.
    pub hw_encode_available: bool,
    /// Whether the software AV1 encoder is available (it always is).
    pub sw_encode_available: bool,
}

/// One static finding from `inspect`, produced without rasterizing.
#[derive(Debug, Serialize, Deserialize)]
pub struct Finding {
    pub severity: FindingSeverity,
    pub code: String,
    pub node: u64,
    pub message: String,
}

#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
pub enum FindingSeverity {
    Error,
    Warning,
    /// Not a defect: a statement about what this host can do.
    Info,
}

#[derive(Debug, Serialize)]
pub struct InspectReport {
    pub findings: Vec<Finding>,
}

impl InspectReport {
    pub fn has_errors(&self) -> bool {
        self.findings
            .iter()
            .any(|f| f.severity == FindingSeverity::Error)
    }
}

/// Pixel-diff report between an approved and a just-rendered frame.
#[derive(Debug, Serialize)]
pub struct DiffReport {
    pub width_mismatch: bool,
    pub total_pixels: u64,
    pub differing_pixels: u64,
    /// Differing pixels as a fraction of the frame (0.0–1.0).
    pub diff_ratio: f64,
    pub max_channel_delta: u8,
    /// True when `diff_ratio <= threshold` (gate passes).
    pub passed: bool,
    pub threshold: f64,
}

#[derive(Debug, thiserror::Error)]
pub enum EngineError {
    #[error("render error: {0}")]
    Render(#[from] kine_render::RenderError),

    #[error("scene {0} not loaded")]
    UnknownScene(u64),

    #[error("layout error: {0}")]
    Layout(#[from] kine_core::LayoutError),

    #[error("shader error: {0}")]
    Shader(String),

    #[error("scene definition error: {0}")]
    Definition(String),

    #[error("video error: {0}")]
    Video(String),

    #[error("format error: {0}")]
    Format(String),

    #[error("json error: {0}")]
    Json(#[from] serde_json::Error),

    #[error("image error: {0}")]
    Image(#[from] image::ImageError),
}

/// Registry handle for a loaded scene.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct SceneHandle(pub u64);

pub struct Engine {
    renderer: FrameRenderer,
    #[cfg(feature = "gpu")]
    gpu_renderer: Option<kine_render::gpu::GpuRenderer>,
    backend: RenderBackend,
    scenes: HashMap<SceneHandle, Scene>,
    /// Audio bed and the directory its source paths are relative to.
    audio: HashMap<SceneHandle, (kine_script::AudioDef, std::path::PathBuf)>,
    next_scene: u64,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    pub fn new() -> Self {
        Self {
            renderer: FrameRenderer::new(),
            #[cfg(feature = "gpu")]
            gpu_renderer: None,
            backend: RenderBackend::Auto,
            scenes: HashMap::new(),
            audio: HashMap::new(),
            next_scene: 1,
        }
    }

    pub fn set_backend(&mut self, backend: RenderBackend) {
        self.backend = backend;
    }

    pub fn backend(&self) -> RenderBackend {
        self.backend
    }

    pub fn shaper(&self) -> &kine_text::TextShaper {
        self.renderer.shaper()
    }

    pub fn preload_video_clip(&mut self, key: impl Into<String>, clip: kine_render::DecodedVideoClip) {
        let key = key.into();
        self.renderer
            .video_cache_mut()
            .insert(key.clone(), clip.clone());
        #[cfg(feature = "gpu")]
        if let Some(gpu) = self.gpu_renderer.as_mut() {
            gpu.video_cache_mut().insert(key, clip);
        }
    }

    pub fn load_font(&mut self, family: &str, data: Vec<u8>) {
        self.renderer.load_font(family, data);
        #[cfg(feature = "gpu")]
        if let Some(gpu) = self.gpu_renderer.as_mut() {
            gpu.sync_fonts_from(self.renderer.shaper());
        }
    }

    pub fn load_scene_json(&mut self, json: &str) -> Result<SceneHandle, EngineError> {
        let base = std::env::current_dir().unwrap_or_else(|_| std::path::PathBuf::from("."));
        self.load_scene_json_with_base(json, &base)
    }

    pub fn load_scene_json_with_base(
        &mut self,
        source: &str,
        base_dir: &std::path::Path,
    ) -> Result<SceneHandle, EngineError> {
        let trimmed = source.trim_start();
        let def: kine_script::SceneDef = if trimmed.starts_with('{') {
            serde_json::from_str(source)?
        } else {
            kine_script::eval_scene_def_ts(source)
                .map_err(|e| EngineError::Definition(e.to_string()))?
        };
        let audio = def.audio.clone();
        let mut scene = kine_script::scene_from_def(def)
            .map_err(|e| EngineError::Definition(e.to_string()))?;
        for node in scene.nodes.values_mut() {
            match &mut node.kind {
                NodeKind::Image { src, .. } if !src.starts_with("pattern:") => {
                    let path = std::path::Path::new(src.as_str());
                    if path.is_relative() {
                        *src = base_dir.join(path).to_string_lossy().into_owned();
                    }
                }
                NodeKind::Video { src, .. } if !src.starts_with("video:") => {
                    if let Some(seq_rel) = src.strip_prefix("seq:") {
                        let path = std::path::Path::new(seq_rel);
                        if path.is_relative() {
                            *src = format!("seq:{}", base_dir.join(path).to_string_lossy());
                        }
                    } else {
                        let path = std::path::Path::new(src.as_str());
                        if path.is_relative() {
                            *src = base_dir.join(path).to_string_lossy().into_owned();
                        }
                    }
                }
                _ => {}
            }
        }
        let handle = self.insert_scene(scene);
        if let Some(audio) = audio {
            self.audio.insert(handle, (audio, base_dir.to_path_buf()));
        }
        Ok(handle)
    }

    pub fn audio_def(
        &self,
        handle: SceneHandle,
    ) -> Option<&(kine_script::AudioDef, std::path::PathBuf)> {
        self.audio.get(&handle)
    }

    pub fn insert_scene(&mut self, scene: Scene) -> SceneHandle {
        let handle = SceneHandle(self.next_scene);
        self.next_scene += 1;
        self.scenes.insert(handle, scene);
        handle
    }

    pub fn free_scene(&mut self, handle: SceneHandle) -> bool {
        self.audio.remove(&handle);
        self.scenes.remove(&handle).is_some()
    }

    pub fn scene(&self, handle: SceneHandle) -> Result<&Scene, EngineError> {
        self.scenes
            .get(&handle)
            .ok_or(EngineError::UnknownScene(handle.0))
    }

    pub fn scene_mut(&mut self, handle: SceneHandle) -> Result<&mut Scene, EngineError> {
        self.scenes
            .get_mut(&handle)
            .ok_or(EngineError::UnknownScene(handle.0))
    }

    /// Render one frame to PNG bytes.
    pub fn render_frame_png(
        &mut self,
        handle: SceneHandle,
        frame: u64,
    ) -> Result<Vec<u8>, EngineError> {
        let pixmap =
            self.render_frame_at_with_index(handle, frame as f64 / 30.0, frame as u32)?;
        Ok(png::pixmap_to_png(&pixmap)?)
    }

    /// Render one frame in the requested [`FrameFormat`] (`PNG`, `QOI`, `BMP`,
    /// `PPM`, `PAM`, `TGA`, or vector `SVG`).
    pub fn render_frame_format(
        &mut self,
        handle: SceneHandle,
        frame: u64,
        format: FrameFormat,
    ) -> Result<Vec<u8>, EngineError> {
        let time = frame as f64 / 30.0;
        if format == FrameFormat::Svg {
            let scene = self.scene_at(handle, time)?;
            let svg = scene_to_svg(scene).map_err(EngineError::Format)?;
            return Ok(svg.into_bytes());
        }
        let pixmap = self.render_frame_at_with_index(handle, time, frame as u32)?;
        encode_pixmap(&pixmap, format).map_err(EngineError::Format)
    }

    /// Render one frame at an explicit time in seconds (frame index inferred at 30 fps).
    pub fn render_frame_at(
        &mut self,
        handle: SceneHandle,
        time: f64,
    ) -> Result<RasterPixmap, EngineError> {
        let frame_idx = (time * 30.0).round().max(0.0) as u32;
        self.render_frame_at_with_index(handle, time, frame_idx)
    }

    /// Render one frame at an explicit `time` (seconds) and `frame_index`.
    pub fn render_frame_at_with_index(
        &mut self,
        handle: SceneHandle,
        time: f64,
        frame_index: u32,
    ) -> Result<RasterPixmap, EngineError> {
        let scene = self
            .scenes
            .get_mut(&handle)
            .ok_or(EngineError::UnknownScene(handle.0))?;
        scene.apply_time(time);
        scene.compute_layout()?;

        let use_gpu = match self.backend {
            RenderBackend::Cpu => false,
            RenderBackend::Gpu => true,
            RenderBackend::Auto => self.gpu_available(),
        };

        if use_gpu {
            #[cfg(feature = "gpu")]
            {
                let scene_ref = &self.scenes[&handle];
                let w = scene_ref.width as u16;
                let h = scene_ref.height as u16;
                if self.gpu_renderer.is_none() {
                    let mut gpu = kine_render::gpu::GpuRenderer::new(w, h)
                        .map_err(RenderError::Gpu)?;
                    gpu.sync_fonts_from(self.renderer.shaper());
                    // Clips decoded (or preloaded) before the GPU renderer
                    // existed must stay visible to it.
                    for (src, clip) in self.renderer.video_cache().shared_clips() {
                        gpu.video_cache_mut().insert_shared(src, clip);
                    }
                    self.gpu_renderer = Some(gpu);
                }
                let gpu = self.gpu_renderer.as_mut().unwrap();
                gpu.sync_fonts_from(self.renderer.shaper());
                return Ok(gpu.render_at(scene_ref, time as f32, frame_index)?);
            }
            #[cfg(not(feature = "gpu"))]
            {
                return Err(EngineError::Render(RenderError::GpuNotCompiled));
            }
        }

        let scene_ref = &self.scenes[&handle];
        Ok(self.renderer.render_at(scene_ref, time as f32, frame_index)?)
    }

    /// Render a horizontal contact sheet (`strip`) of `count` frames.
    pub fn render_strip_png(
        &mut self,
        handle: SceneHandle,
        count: u32,
        fps: f64,
    ) -> Result<Vec<u8>, EngineError> {
        let count = count.clamp(1, 32);
        let fps = fps.max(1.0);
        let (w, h) = {
            let s = self.scene(handle)?;
            (s.width as u16, s.height as u16)
        };
        let total_w = w.saturating_mul(count as u16).max(w);
        let mut sheet = RasterPixmap::new(total_w, h);

        for i in 0..count {
            let time = f64::from(i) / fps;
            let frame_px = self.render_frame_at_with_index(handle, time, i)?;
            let x_off = (i as usize) * (w as usize);
            for y in 0..usize::from(h) {
                let src_row = &frame_px.data()[y * usize::from(w)..(y + 1) * usize::from(w)];
                let dst_row = &mut sheet.data_mut()
                    [y * usize::from(total_w) + x_off..y * usize::from(total_w) + x_off + usize::from(w)];
                dst_row.copy_from_slice(src_row);
            }
        }
        Ok(png::pixmap_to_png(&sheet)?)
    }

    /// Render an onion-skin composite (`onion`) of `count` frames.
    pub fn render_onion_png(
        &mut self,
        handle: SceneHandle,
        count: u32,
        fps: f64,
    ) -> Result<Vec<u8>, EngineError> {
        let count = count.clamp(1, 32);
        let fps = fps.max(1.0);
        let (w, h) = {
            let s = self.scene(handle)?;
            (s.width as u16, s.height as u16)
        };
        let mut accum = RasterPixmap::new(w, h);

        for i in 0..count {
            let time = f64::from(i) / fps;
            let frame_px = self.render_frame_at_with_index(handle, time, i)?;
            let weight = if i + 1 == count {
                1.0f32
            } else {
                0.25 + 0.5 * ((i + 1) as f32 / (count as f32))
            };
            for (dst, src) in accum.data_mut().iter_mut().zip(frame_px.data().iter()) {
                let sr = (f32::from(src.r) * weight) as u32;
                let sg = (f32::from(src.g) * weight) as u32;
                let sb = (f32::from(src.b) * weight) as u32;
                let sa = (f32::from(src.a) * weight) as u32;
                let inv = 255u32.saturating_sub(sa);
                dst.r = (sr + (u32::from(dst.r) * inv + 127) / 255).min(255) as u8;
                dst.g = (sg + (u32::from(dst.g) * inv + 127) / 255).min(255) as u8;
                dst.b = (sb + (u32::from(dst.b) * inv + 127) / 255).min(255) as u8;
                dst.a = (sa + (u32::from(dst.a) * inv + 127) / 255).min(255) as u8;
            }
        }
        Ok(png::pixmap_to_png(&accum)?)
    }

    pub fn scene_at(&mut self, handle: SceneHandle, time: f64) -> Result<&Scene, EngineError> {
        let scene = self
            .scenes
            .get_mut(&handle)
            .ok_or(EngineError::UnknownScene(handle.0))?;
        scene.apply_time(time);
        scene.compute_layout()?;
        Ok(&self.scenes[&handle])
    }

    pub fn is_animated(&self, handle: SceneHandle) -> Result<bool, EngineError> {
        Ok(self.scene(handle)?.is_animated())
    }

    pub fn render_frame_png_b64(
        &mut self,
        handle: SceneHandle,
        frame: u64,
    ) -> Result<String, EngineError> {
        Ok(B64.encode(self.render_frame_png(handle, frame)?))
    }

    /// Static inspection: missing fonts, zero-size nodes, text overflow,
    /// out-of-canvas geometry, and WGSL shader validation.
    pub fn inspect(&mut self, handle: SceneHandle) -> Result<InspectReport, EngineError> {
        let gpu_available = self.gpu_available();

        let scene = self.scene_mut(handle)?;
        scene.compute_layout()?;

        let mut findings = Vec::new();
        let w = scene.width as f32;
        let h = scene.height as f32;
        let nodes: Vec<(NodeId, NodeKind, kine_core::Rect)> = scene
            .nodes
            .iter()
            .filter_map(|(id, n)| n.abs_pos.map(|r| (*id, n.kind.clone(), r)))
            .collect();

        for (id, kind, rect) in nodes {
            if rect.width <= 0.0 || rect.height <= 0.0 {
                findings.push(Finding {
                    severity: FindingSeverity::Error,
                    code: "zero-size".into(),
                    node: id.0,
                    message: format!("node has zero size: {rect:?}"),
                });
            }
            if rect.x + rect.width <= 0.0
                || rect.y + rect.height <= 0.0
                || rect.x >= w
                || rect.y >= h
            {
                findings.push(Finding {
                    severity: FindingSeverity::Warning,
                    code: "out-of-canvas".into(),
                    node: id.0,
                    message: format!("node lies outside the canvas: {rect:?}"),
                });
            }
            if let NodeKind::Shader {
                source, uniforms, ..
            } = &kind
            {
                match kine_shader::validate(source) {
                    Ok(module) => {
                        for binding in module.agent_uniforms() {
                            if let Err(e) = kine_shader::UniformPack::pack(binding, uniforms) {
                                findings.push(Finding {
                                    severity: FindingSeverity::Error,
                                    code: "shader-uniforms".into(),
                                    node: id.0,
                                    message: format!("uniform buffer `{}`: {e}", binding.name),
                                });
                            }
                        }
                        if !gpu_available {
                            findings.push(Finding {
                                severity: FindingSeverity::Info,
                                code: "shader-needs-gpu".into(),
                                node: id.0,
                                message: format!(
                                    "shader validated ({} binding(s)); execution needs a GPU, which this host does not expose (CPU WGSL VM active)",
                                    module.uniforms.len() + module.textures.len()
                                ),
                            });
                        }
                    }
                    Err(e) => {
                        let where_ = e
                            .line()
                            .map(|l| format!("line {l}: "))
                            .unwrap_or_default();
                        findings.push(Finding {
                            severity: FindingSeverity::Error,
                            code: "shader-error".into(),
                            node: id.0,
                            message: format!("{where_}{e}"),
                        });
                        findings.push(Finding {
                            severity: FindingSeverity::Info,
                            code: "shader-diagnostic".into(),
                            node: id.0,
                            message: e.diagnostic(),
                        });
                    }
                }
            }

            if let NodeKind::Video { src, .. } = &kind {
                if let Err(e) = self.renderer.video_cache_mut().get_or_load(src) {
                    findings.push(Finding {
                        severity: FindingSeverity::Error,
                        code: "missing-video".into(),
                        node: id.0,
                        message: e,
                    });
                }
            }

            if let NodeKind::Text {
                content,
                font_family,
                font_size,
                ..
            } = &kind
            {
                if self.renderer.shaper().font_blob(font_family).is_none() {
                    findings.push(Finding {
                        severity: FindingSeverity::Error,
                        code: "missing-font".into(),
                        node: id.0,
                        message: format!("font family not loaded: {font_family}"),
                    });
                    continue;
                }
                let shaped = self.renderer.shaper_mut().shape(
                    content,
                    font_family,
                    *font_size,
                    None,
                    kine_text::Align::Left,
                );
                if let Ok(shaped) = shaped {
                    if shaped.width > rect.width + 1.0 {
                        findings.push(Finding {
                            severity: FindingSeverity::Warning,
                            code: "text-overflow".into(),
                            node: id.0,
                            message: format!(
                                "text width {:.1}px exceeds box width {:.1}px (no wrap width)",
                                shaped.width, rect.width
                            ),
                        });
                    }
                }
            }
        }

        Ok(InspectReport { findings })
    }

    pub fn snapshot_diff(
        &mut self,
        handle: SceneHandle,
        frame: u64,
        approved_png: &[u8],
        threshold: f64,
    ) -> Result<DiffReport, EngineError> {
        let time = frame as f64 / 30.0;
        let rendered = self.render_frame_at_with_index(handle, time, frame as u32)?;
        let approved = image::load_from_memory(approved_png)?.to_rgba8();

        let (rw, rh) = (u32::from(rendered.width()), u32::from(rendered.height()));
        if rw != approved.width() || rh != approved.height() {
            return Ok(DiffReport {
                width_mismatch: true,
                total_pixels: 0,
                differing_pixels: 0,
                diff_ratio: 1.0,
                max_channel_delta: u8::MAX,
                passed: false,
                threshold,
            });
        }

        let mut differing = 0u64;
        let mut max_delta = 0u8;
        let premul = |c: u8, a: u32| ((u32::from(c) * a + 127) / 255).min(255) as u8;
        for (i, p) in rendered.data().iter().enumerate() {
            let x = (i as u32) % rw;
            let y = (i as u32) / rw;
            let a = approved.get_pixel(x, y).0;
            let alpha = u32::from(a[3]);
            let deltas = [
                premul(a[0], alpha).abs_diff(p.r),
                premul(a[1], alpha).abs_diff(p.g),
                premul(a[2], alpha).abs_diff(p.b),
                a[3].abs_diff(p.a),
            ];
            let local_max = *deltas.iter().max().unwrap();
            if local_max > 2 {
                differing += 1;
            }
            max_delta = max_delta.max(local_max);
        }
        let total = (rw * rh) as u64;
        let diff_ratio = differing as f64 / total as f64;

        Ok(DiffReport {
            width_mismatch: false,
            total_pixels: total,
            differing_pixels: differing,
            diff_ratio,
            max_channel_delta: max_delta,
            passed: diff_ratio <= threshold,
            threshold,
        })
    }

    pub fn timeline(&self, handle: SceneHandle) -> Result<serde_json::Value, EngineError> {
        let scene = self.scene(handle)?;
        let node_count = scene.nodes.len();
        let text_count = scene
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::Text { .. }))
            .count();
        let shader_count = scene
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::Shader { .. }))
            .count();
        let video_count = scene
            .nodes
            .values()
            .filter(|n| matches!(n.kind, NodeKind::Video { .. }))
            .count();
        let animations: Vec<serde_json::Value> = scene
            .animations
            .iter()
            .map(|a| {
                let (start, end) = a.track.time_range();
                serde_json::json!({
                    "node": a.node.0,
                    "property": a.property.name(),
                    "base": a.base,
                    "start": start,
                    "end": end,
                    "keyframes": a.track.keyframes.len(),
                })
            })
            .collect();
        let audio_tracks = self
            .audio
            .get(&handle)
            .map(|(a, _)| a.tracks.len())
            .unwrap_or(0);

        Ok(serde_json::json!({
            "scene": handle.0,
            "width": scene.width,
            "height": scene.height,
            "nodes": node_count,
            "texts": text_count,
            "shaders": shader_count,
            "videos": video_count,
            "animated": scene.is_animated(),
            "duration_seconds": scene.duration_seconds(),
            "animations": animations,
            "audio_tracks": audio_tracks,
        }))
    }

    pub fn gpu_available(&self) -> bool {
        cfg!(feature = "gpu") && kine_render::gpu_adapter_present()
    }

    pub fn shader_modules(
        &self,
        handle: SceneHandle,
    ) -> Result<Vec<(kine_core::NodeId, kine_shader::ShaderModule)>, EngineError> {
        let scene = self.scene(handle)?;
        let mut out = Vec::new();
        for (id, node) in scene.nodes.iter() {
            if let NodeKind::Shader { source, .. } = &node.kind {
                let module = kine_shader::validate(source)
                    .map_err(|e| EngineError::Shader(e.to_string()))?;
                out.push((*id, module));
            }
        }
        out.sort_by_key(|(id, _)| id.0);
        Ok(out)
    }

    pub fn doctor(&self) -> DoctorReport {
        let host = kine_platform::PlatformInfo::detect();
        let gpu_avail = self.gpu_available();
        let renderer = if self.backend == RenderBackend::Gpu && gpu_avail {
            "vello_gpu-wgpu"
        } else {
            "vello_cpu-sparse-strips"
        };
        DoctorReport {
            version: env!("CARGO_PKG_VERSION"),
            renderer,
            backend_preference: self.backend.name(),
            gpu_available: gpu_avail,
            cpu_shader_vm: true,
            formats: vec![
                "mp4", "mkv", "webm", "y4m", "gif", "kvid", "rgba", "i420", "nv12", "png", "qoi",
                "bmp", "ppm", "pam", "tga", "svg", "wav", "flac", "aiff",
            ],
            fonts: self
                .renderer
                .shaper()
                .families()
                .iter()
                .map(|s| s.to_string())
                .collect(),
            platform: std::env::consts::OS,
            host,
            hw_encode_available: self.hw_encode_available(),
            sw_encode_available: true,
        }
    }

    pub fn hw_encode_available(&self) -> bool {
        #[cfg(feature = "hwencode")]
        {
            matches!(kine_mux::hwencode::h264_encoder_available(), Ok(true))
        }
        #[cfg(not(feature = "hwencode"))]
        {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

    fn engine_with_scene() -> (Engine, SceneHandle) {
        let mut engine = Engine::new();
        engine.load_font("JetBrains Mono", FONT.to_vec());
        let handle = engine
            .load_scene_json(&sample_scene_json())
            .expect("authoring scene");
        (engine, handle)
    }

    fn sample_scene_json() -> String {
        serde_json::json!({
            "width": 320,
            "height": 200,
            "root": {
                "kind": "container",
                "size": { "width": 320, "height": 200 },
                "children": [
                    {
                        "kind": "rect",
                        "fill": "#141828",
                        "position": "absolute",
                        "inset": { "top": 0, "left": 0, "right": 0, "bottom": 0 }
                    },
                    {
                        "kind": "text",
                        "content": "agent loop",
                        "fontFamily": "JetBrains Mono",
                        "fontSize": 20,
                        "color": "#f0f0f5",
                        "align": "center",
                        "size": { "width": 320, "height": 40 },
                        "position": "absolute",
                        "inset": { "top": 80, "left": 0 }
                    }
                ]
            }
        })
        .to_string()
    }

    #[test]
    fn renders_png_qoi_bmp_svg_via_engine() {
        let (mut engine, handle) = engine_with_scene();
        let png_bytes = engine.render_frame_png(handle, 0).unwrap();
        assert_eq!(&png_bytes[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);

        let qoi_bytes = engine
            .render_frame_format(handle, 0, FrameFormat::Qoi)
            .unwrap();
        assert_eq!(&qoi_bytes[..4], b"qoif");

        let bmp_bytes = engine
            .render_frame_format(handle, 0, FrameFormat::Bmp)
            .unwrap();
        assert_eq!(&bmp_bytes[..2], b"BM");

        let svg_bytes = engine
            .render_frame_format(handle, 0, FrameFormat::Svg)
            .unwrap();
        let svg_str = String::from_utf8(svg_bytes).unwrap();
        assert!(svg_str.contains("<svg") && svg_str.contains("agent loop"));
    }

    #[test]
    fn renders_strip_and_onion_contact_sheets() {
        let (mut engine, handle) = engine_with_scene();
        let strip = engine.render_strip_png(handle, 3, 30.0).unwrap();
        let decoded = image::load_from_memory(&strip).unwrap();
        assert_eq!(decoded.width(), 320 * 3);
        assert_eq!(decoded.height(), 200);

        let onion = engine.render_onion_png(handle, 3, 30.0).unwrap();
        let decoded_onion = image::load_from_memory(&onion).unwrap();
        assert_eq!(decoded_onion.width(), 320);
        assert_eq!(decoded_onion.height(), 200);
    }

    #[test]
    fn inspect_reports_clean_scene() {
        let (mut engine, handle) = engine_with_scene();
        let report = engine.inspect(handle).unwrap();
        assert!(!report.has_errors(), "unexpected errors: {:?}", report.findings);
    }

    #[test]
    fn inspect_flags_missing_font() {
        let mut engine = Engine::new();
        let handle = engine.load_scene_json(&sample_scene_json()).unwrap();
        let report = engine.inspect(handle).unwrap();
        assert!(report.has_errors());
        assert!(report.findings.iter().any(|f| f.code == "missing-font"));
    }

    #[test]
    fn snapshot_diff_passes_against_itself() {
        let (mut engine, handle) = engine_with_scene();
        let golden = engine.render_frame_png(handle, 0).unwrap();
        let report = engine
            .snapshot_diff(handle, 0, &golden, 0.001)
            .unwrap();
        assert!(!report.width_mismatch);
        assert_eq!(report.differing_pixels, 0, "self-diff should be empty");
        assert!(report.passed);
    }

    fn scene_with_shader(wgsl: &str, uniforms: &str) -> kine_script::SceneDef {
        let json = format!(
            r##"{{
                "width": 64, "height": 64,
                "root": {{
                    "kind": "container",
                    "children": [
                        {{ "kind": "rect", "fill": "#101020", "size": {{ "width": 64, "height": 64 }},
                           "position": "absolute", "inset": {{ "top": 0, "left": 0 }} }},
                        {{ "kind": "shader", "shader": {}, "uniforms": {}, "position": "absolute",
                           "inset": {{ "top": 0, "left": 0 }}, "size": {{ "width": 64, "height": 32 }} }}
                    ]
                }}
            }}"##,
            serde_json::to_string(wgsl).unwrap(),
            uniforms
        );
        serde_json::from_str(&json).expect("scene definition")
    }

    fn insert(engine: &mut Engine, def: kine_script::SceneDef) -> SceneHandle {
        engine.insert_scene(kine_script::scene_from_def(def).expect("scene"))
    }

    #[test]
    fn inspect_reports_shader_errors_with_the_agent_line() {
        let mut engine = Engine::new();
        let wgsl = "struct P { a: f32 };
@group(1) @binding(0) var<uniform> p: P;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(p.b);
}
";
        let handle = insert(&mut engine, scene_with_shader(wgsl, "{}"));

        let report = engine.inspect(handle).unwrap();
        let error = report
            .findings
            .iter()
            .find(|f| f.code == "shader-error")
            .expect("shader error reported");
        assert_eq!(error.severity, FindingSeverity::Error);
        assert!(error.message.contains("line 6"), "got: {}", error.message);

        let diagnostic = report
            .findings
            .iter()
            .find(|f| f.code == "shader-diagnostic")
            .expect("diagnostic excerpt");
        assert!(diagnostic.message.contains("p.b"), "got: {}", diagnostic.message);
    }

    #[test]
    fn inspect_rejects_a_uniform_name_the_shader_does_not_declare() {
        let mut engine = Engine::new();
        let wgsl = "struct P { speed: f32, tint: vec4<f32> };
@group(1) @binding(0) var<uniform> p: P;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return p.tint * p.speed;
}
";
        let handle = insert(
            &mut engine,
            scene_with_shader(wgsl, r#"{"speed": 1.0, "tnt": [1,0,0,1]}"#),
        );

        let report = engine.inspect(handle).unwrap();
        let finding = report
            .findings
            .iter()
            .find(|f| f.code == "shader-uniforms")
            .expect("uniform typo reported");
        assert!(finding.message.contains("tnt"), "got: {}", finding.message);
        assert!(finding.message.contains("tint"), "got: {}", finding.message);
    }

    #[test]
    fn a_valid_shader_on_a_host_without_a_gpu_says_so_instead_of_approximating() {
        let mut engine = Engine::new();
        let wgsl = "@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(kine_input, kine_sampler, uv);
}
";
        let handle = insert(&mut engine, scene_with_shader(wgsl, "{}"));

        let report = engine.inspect(handle).unwrap();
        assert!(!report.has_errors(), "expected no errors: {:?}", report.findings);
        if !kine_render::gpu_adapter_present() {
            assert!(
                report.findings.iter().any(|f| f.code == "shader-needs-gpu"),
                "a host without a GPU must say so: {:?}",
                report.findings
            );
        }
    }

    #[test]
    fn shader_modules_can_be_reflected_for_pipeline_building() {
        let mut engine = Engine::new();
        let wgsl = "struct P { amount: f32 };
@group(1) @binding(0) var<uniform> p: P;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return vec4<f32>(p.amount);
}
";
        let handle = insert(&mut engine, scene_with_shader(wgsl, r#"{"amount": 0.5}"#));

        let modules = engine.shader_modules(handle).unwrap();
        assert_eq!(modules.len(), 1);
        let (_, module) = &modules[0];
        assert_eq!(module.agent_uniforms().count(), 1);
        assert_eq!(module.agent_uniforms().next().unwrap().members[0].offset, 0);
    }

    #[test]
    fn doctor_reports_cpu_renderer() {
        let (engine, _) = engine_with_scene();
        let doc = engine.doctor();
        assert_eq!(doc.renderer, "vello_cpu-sparse-strips");
        assert!(doc.fonts.contains(&"JetBrains Mono".to_string()));
    }

    #[test]
    fn doctor_reports_host_and_storage_policy() {
        let (engine, _) = engine_with_scene();
        let doc = engine.doctor();
        assert!(!doc.host.output_dir.as_os_str().is_empty());
        assert!(
            doc.host.kind.is_linux()
                || doc.host.kind.is_windows()
                || doc.host.kind.is_macos()
                || matches!(doc.host.kind, kine_platform::PlatformKind::Other)
        );
    }
}
