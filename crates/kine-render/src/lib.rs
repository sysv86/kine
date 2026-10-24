//! Frame renderer for Kine: unified CPU (`vello_cpu` + `kine_shader::vm`) and
//! GPU (`vello_gpu` + `wgpu`) rendering backends, video input decoding &
//! caching (`video_io`), and multi-format output (`PNG`, `QOI`, `BMP`, `PPM`,
//! `PAM`, `TGA`, `GIF89a`, `SVG`).

pub mod formats;
pub mod png;
pub mod video_io;

#[cfg(feature = "gpu")]
pub mod gpu;

pub use formats::{
    decode_any_image, encode_animated_gif, encode_pixmap, pixmap_to_bmp, pixmap_to_pam,
    pixmap_to_ppm, pixmap_to_qoi, pixmap_to_straight_rgba, pixmap_to_tga, scene_to_svg,
    FrameFormat,
};
pub use video_io::{
    decode_gif_clip, decode_image_sequence_dir, decode_kvid_clip, decode_video_bytes,
    decode_video_source, decode_y4m_clip, synthesize_procedural_video, write_image_sequence_dir,
    DecodedVideoClip, VideoCache,
};

/// Which rendering backend the engine should use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum RenderBackend {
    Auto,
    Gpu,
    Cpu,
}

impl Default for RenderBackend {
    fn default() -> Self {
        Self::Auto
    }
}

impl RenderBackend {
    pub fn parse(name: &str) -> Option<Self> {
        match name.to_ascii_lowercase().as_str() {
            "auto" => Some(Self::Auto),
            "gpu" | "wgpu" | "vulkan" | "metal" => Some(Self::Gpu),
            "cpu" | "software" => Some(Self::Cpu),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Auto => "auto",
            Self::Gpu => "gpu",
            Self::Cpu => "cpu",
        }
    }
}

#[cfg(feature = "gpu")]
pub use gpu::gpu_adapter_present;

#[cfg(not(feature = "gpu"))]
pub const fn gpu_adapter_present() -> bool {
    false
}

use kine_core::{
    BoxShadow, Color, Gradient, GradientStop, NodeId, NodeKind, ObjectFit, Rect, Scene,
    ShaderBlend, StrokeStyle, TextAlign, Transform2D,
};
use kine_shader::{CpuBlendMode, CpuShaderProgram};
use kine_text::{Align, TextShaper};
use std::sync::Arc;
use vello_cpu::color::{AlphaColor, PremulRgba8, Srgb};
use vello_cpu::kurbo::{
    Affine, BezPath, Circle as KurboCircle, Point as KurboPoint, Rect as KurboRect, RoundedRect,
    Shape,
};
use vello_cpu::peniko::{Blob, FontData};
use vello_cpu::{Glyph, Pixmap, RenderContext, Resources};

pub use vello_cpu::Pixmap as RasterPixmap;

#[derive(Debug, thiserror::Error)]
pub enum RenderError {
    #[error("scene layout not computed; call Scene::compute_layout first")]
    MissingLayout,

    #[error("node {0} not found in scene")]
    MissingNode(u64),

    #[error("font family not loaded into renderer: {0}")]
    FontNotLoaded(String),

    #[error("glyph rendering failed: {0}")]
    Glyph(String),

    #[error("shader error on node {node}: {reason}")]
    Shader { node: u64, reason: String },

    #[error("svg path error on node {node}: {reason}")]
    InvalidPath { node: u64, reason: String },

    #[error("video input error on node {node}: {reason}")]
    VideoInput { node: u64, reason: String },

    #[error("text shaping failed: {0}")]
    Shape(#[from] kine_text::ShapeError),

    #[error("png encoding failed: {0}")]
    Image(#[from] image::ImageError),

    #[error("GPU backend requested, but this binary was built without `--features gpu`")]
    GpuNotCompiled,

    #[cfg(feature = "gpu")]
    #[error("gpu error: {0}")]
    Gpu(#[from] gpu::GpuError),
}

/// CPU frame renderer: scene graph → premultiplied RGBA pixmap, including
/// multi-threaded CPU evaluation of WGSL shader passes and cached video input nodes.
pub struct FrameRenderer {
    shaper: TextShaper,
    resources: Resources,
    video_cache: VideoCache,
}

impl Default for FrameRenderer {
    fn default() -> Self {
        Self::new()
    }
}

impl FrameRenderer {
    pub fn new() -> Self {
        Self {
            shaper: TextShaper::new(),
            resources: Resources::new(),
            video_cache: VideoCache::new(),
        }
    }

    pub fn load_font(&mut self, family: &str, data: Vec<u8>) {
        self.shaper.load_font(family, data);
    }

    pub fn shaper(&self) -> &TextShaper {
        &self.shaper
    }

    pub fn shaper_mut(&mut self) -> &mut TextShaper {
        &mut self.shaper
    }

    pub fn video_cache_mut(&mut self) -> &mut VideoCache {
        &mut self.video_cache
    }

    pub fn video_cache(&self) -> &VideoCache {
        &self.video_cache
    }

    pub fn render(&mut self, scene: &Scene) -> Result<Pixmap, RenderError> {
        self.render_at(scene, 0.0, 0)
    }

    pub fn render_at(
        &mut self,
        scene: &Scene,
        time: f32,
        frame: u32,
    ) -> Result<Pixmap, RenderError> {
        let w = scene.width as u16;
        let h = scene.height as u16;
        let mut target = Pixmap::new(w, h);
        let mut ctx = RenderContext::new(w, h);
        let mut has_pending_draw = false;

        self.render_node_cpu(
            &mut ctx,
            &mut target,
            &mut has_pending_draw,
            scene,
            scene.root,
            1.0,
            Transform2D::IDENTITY,
            time,
            frame,
        )?;

        if has_pending_draw {
            self.flush_context_into(&mut ctx, &mut target, w, h);
        }

        Ok(target)
    }

    fn flush_context_into(
        &mut self,
        ctx: &mut RenderContext,
        target: &mut Pixmap,
        w: u16,
        h: u16,
    ) {
        ctx.flush();
        let mut layer = Pixmap::new(w, h);
        ctx.render(&mut layer, &mut self.resources);
        composite_pixmap_over(target, &layer);
        *ctx = RenderContext::new(w, h);
    }

    #[allow(clippy::too_many_arguments)]
    fn render_node_cpu(
        &mut self,
        ctx: &mut RenderContext,
        target: &mut Pixmap,
        has_pending_draw: &mut bool,
        scene: &Scene,
        node_id: NodeId,
        parent_opacity: f32,
        parent_transform: Transform2D,
        time: f32,
        frame: u32,
    ) -> Result<(), RenderError> {
        let node = scene
            .nodes
            .get(&node_id)
            .ok_or(RenderError::MissingNode(node_id.0))?;
        let rect = node.abs_pos.ok_or(RenderError::MissingLayout)?;

        let effective_opacity = (parent_opacity * node.opacity).clamp(0.0, 1.0);
        if effective_opacity <= 0.0 {
            return Ok(());
        }
        let effective_transform = combine_transforms(parent_transform, node.transform);
        let affine = build_affine(rect, effective_transform);

        match &node.kind {
            NodeKind::Container => {}
            NodeKind::Shader {
                source,
                uniforms,
                blend,
            } => {
                let w = scene.width as u16;
                let h = scene.height as u16;
                if *has_pending_draw {
                    self.flush_context_into(ctx, target, w, h);
                    *has_pending_draw = false;
                }
                let program = CpuShaderProgram::compile(source, uniforms).map_err(|reason| {
                    RenderError::Shader {
                        node: node_id.0,
                        reason,
                    }
                })?;
                let scissor = scissor_for(rect, w, h);
                let cpu_blend = match blend {
                    ShaderBlend::Normal => CpuBlendMode::Normal,
                    ShaderBlend::Add => CpuBlendMode::Add,
                    ShaderBlend::Multiply => CpuBlendMode::Multiply,
                    ShaderBlend::Screen => CpuBlendMode::Screen,
                };
                program.execute_on_premul_rgba(
                    target.data_as_u8_slice_mut(),
                    u32::from(w),
                    u32::from(h),
                    scissor,
                    time,
                    frame,
                    cpu_blend,
                    effective_opacity,
                );
            }
            NodeKind::Rect {
                fill,
                corner_radius,
            } => {
                if let Some(sh) = &node.shadow {
                    draw_rect_shadow_cpu(ctx, rect, *corner_radius, sh, effective_opacity, affine);
                }
                let shape_path = rect_to_path(rect, *corner_radius);
                if let Some(grad) = &node.gradient {
                    fill_gradient_cpu(ctx, rect, *corner_radius, grad, effective_opacity, affine);
                } else {
                    ctx.set_paint(to_paint(fill.with_opacity(effective_opacity)));
                    if affine == Affine::IDENTITY && *corner_radius <= 0.0 {
                        ctx.fill_rect(&KurboRect::new(
                            f64::from(rect.x),
                            f64::from(rect.y),
                            f64::from(rect.x + rect.width),
                            f64::from(rect.y + rect.height),
                        ));
                    } else {
                        ctx.fill_path(&(affine * shape_path));
                    }
                }
                if let Some(stroke) = &node.stroke {
                    if let Some(ring) = stroke_rect_ring(rect, *corner_radius, stroke.width) {
                        ctx.set_paint(to_paint(stroke.color.with_opacity(effective_opacity)));
                        ctx.fill_path(&(affine * ring));
                    }
                }
                *has_pending_draw = true;
            }
            NodeKind::Circle { fill, radius } => {
                let (cx, cy) = rect.center();
                let r = radius
                    .unwrap_or_else(|| rect.width.min(rect.height) * 0.5)
                    .max(0.0);
                if let Some(sh) = &node.shadow {
                    let steps = 5usize;
                    for i in (1..=steps).rev() {
                        let expand = sh.blur.max(0.0) * (i as f32 / steps as f32);
                        let alpha = (effective_opacity / (steps as f32 + 1.0)).clamp(0.0, 1.0);
                        ctx.set_paint(to_paint(sh.color.with_opacity(alpha)));
                        let sc = KurboCircle::new(
                            (f64::from(cx + sh.offset_x), f64::from(cy + sh.offset_y)),
                            f64::from(r + expand),
                        );
                        ctx.fill_path(&(affine * sc.to_path(0.25)));
                    }
                }
                let circle = KurboCircle::new((f64::from(cx), f64::from(cy)), f64::from(r));
                ctx.set_paint(to_paint(fill.with_opacity(effective_opacity)));
                ctx.fill_path(&(affine * circle.to_path(0.25)));
                if let Some(stroke) = &node.stroke {
                    let sw = stroke.width.max(0.0);
                    if sw > 0.0 {
                        let outer =
                            KurboCircle::new((f64::from(cx), f64::from(cy)), f64::from(r + sw * 0.5));
                        let inner_r = (r - sw * 0.5).max(0.0);
                        let mut ring = outer.to_path(0.25);
                        if inner_r > 0.0 {
                            ring.extend(reversed_circle_path(cx, cy, inner_r));
                        }
                        ctx.set_paint(to_paint(stroke.color.with_opacity(effective_opacity)));
                        ctx.fill_path(&(affine * ring));
                    }
                }
                *has_pending_draw = true;
            }
            NodeKind::Path { d, fill } => {
                let mut path = BezPath::from_svg(d).map_err(|e| RenderError::InvalidPath {
                    node: node_id.0,
                    reason: e.to_string(),
                })?;
                let offset_affine =
                    Affine::translate((f64::from(rect.x), f64::from(rect.y))) * affine;
                path = offset_affine * path;
                if let Some(fill_color) = fill {
                    ctx.set_paint(to_paint(fill_color.with_opacity(effective_opacity)));
                    ctx.fill_path(&path);
                    *has_pending_draw = true;
                }
            }
            NodeKind::Image {
                src,
                fit,
                corner_radius,
            } => {
                let w = scene.width as u16;
                let h = scene.height as u16;
                if *has_pending_draw {
                    self.flush_context_into(ctx, target, w, h);
                    *has_pending_draw = false;
                }
                blit_image_node(
                    target.data_as_u8_slice_mut(),
                    u32::from(w),
                    u32::from(h),
                    rect,
                    src,
                    *fit,
                    *corner_radius,
                    effective_opacity,
                );
            }
            NodeKind::Video {
                src,
                fit,
                corner_radius,
                start,
                trim_start,
                trim_end,
                playback_rate,
                loop_video,
            } => {
                let w = scene.width as u16;
                let h = scene.height as u16;
                if *has_pending_draw {
                    self.flush_context_into(ctx, target, w, h);
                    *has_pending_draw = false;
                }
                let clip = self.video_cache.get_or_load(src).map_err(|reason| {
                    RenderError::VideoInput {
                        node: node_id.0,
                        reason,
                    }
                })?;
                if let Some(frame_rgba) = clip.sample_frame(
                    f64::from(time),
                    *start,
                    *trim_start,
                    *trim_end,
                    *playback_rate,
                    *loop_video,
                ) {
                    blit_rgba_buffer(
                        target.data_as_u8_slice_mut(),
                        u32::from(w),
                        u32::from(h),
                        rect,
                        clip.width,
                        clip.height,
                        frame_rgba,
                        *fit,
                        *corner_radius,
                        effective_opacity,
                    );
                }
            }
            NodeKind::Text {
                content,
                font_family,
                font_size,
                font_weight,
                color,
                align,
            } => {
                let scaled_size =
                    (*font_size * effective_transform.scale_y.abs().max(0.01)).max(1.0);
                let shaped = self.shaper.shape_with_weight(
                    content,
                    font_family,
                    scaled_size,
                    *font_weight,
                    Some(rect.width),
                    to_align(*align),
                )?;

                let font_bytes = self
                    .shaper
                    .font_blob(font_family)
                    .ok_or_else(|| RenderError::FontNotLoaded(font_family.clone()))?;
                let blob_data: Arc<dyn AsRef<[u8]> + Send + Sync> = font_bytes;
                let font = FontData::new(Blob::new(blob_data), 0);

                ctx.set_paint(to_paint(color.with_opacity(effective_opacity)));
                let run = ctx
                    .glyph_run(&mut self.resources, &font)
                    .font_size(scaled_size);
                let tx = effective_transform.translate_x;
                let ty = effective_transform.translate_y;
                let glyphs: Vec<Glyph> = shaped
                    .glyphs
                    .iter()
                    .map(|g| Glyph {
                        id: g.id,
                        x: rect.x + tx + g.x,
                        y: rect.y + ty + g.y,
                    })
                    .collect();
                run.fill_glyphs(glyphs.iter().copied())
                    .map_err(|e| RenderError::Glyph(e.to_string()))?;
                *has_pending_draw = true;
            }
        }

        let children: Vec<NodeId> = node.children.clone();
        for child in children {
            self.render_node_cpu(
                ctx,
                target,
                has_pending_draw,
                scene,
                child,
                effective_opacity,
                effective_transform,
                time,
                frame,
            )?;
        }
        Ok(())
    }
}

fn draw_rect_shadow_cpu(
    ctx: &mut RenderContext,
    rect: Rect,
    corner_radius: f32,
    sh: &BoxShadow,
    opacity: f32,
    affine: Affine,
) {
    let steps = 5usize;
    for i in (1..=steps).rev() {
        let expand = sh.blur.max(0.0) * (i as f32 / steps as f32);
        let s_rect = Rect::new(
            rect.x + sh.offset_x - expand,
            rect.y + sh.offset_y - expand,
            rect.width + expand * 2.0,
            rect.height + expand * 2.0,
        );
        let alpha = (opacity / (steps as f32 + 1.0)).clamp(0.0, 1.0);
        ctx.set_paint(to_paint(sh.color.with_opacity(alpha)));
        ctx.fill_path(&(affine * rect_to_path(s_rect, corner_radius + expand)));
    }
}

/// Pixels behind an `image()` node source: the built-in `pattern:checkerboard`
/// or a file in one of the supported raster formats. Shared by the CPU blitter
/// and the GPU rasterizer.
pub(crate) fn image_source_pixels(src: &str) -> Option<(u32, u32, Vec<u8>)> {
    if src.starts_with("pattern:checkerboard") {
        let mut buf = Vec::with_capacity(64 * 64 * 4);
        for y in 0..64u32 {
            for x in 0..64u32 {
                let dark = ((x / 8) + (y / 8)) % 2 == 0;
                let c = if dark { 40u8 } else { 200u8 };
                buf.extend_from_slice(&[c, c, c, 255]);
            }
        }
        return Some((64, 64, buf));
    }
    let bytes = std::fs::read(src).ok()?;
    decode_any_image(&bytes).ok()
}

#[allow(clippy::too_many_arguments)]
fn blit_image_node(
    target_rgba: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    rect: Rect,
    src: &str,
    fit: ObjectFit,
    corner_radius: f32,
    opacity: f32,
) {
    let Some((img_w, img_h, img_rgba)) = image_source_pixels(src) else {
        return;
    };

    blit_rgba_buffer(
        target_rgba,
        canvas_w,
        canvas_h,
        rect,
        img_w,
        img_h,
        &img_rgba,
        fit,
        corner_radius,
        opacity,
    );
}

#[allow(clippy::too_many_arguments)]
pub(crate) fn blit_rgba_buffer(
    target_rgba: &mut [u8],
    canvas_w: u32,
    canvas_h: u32,
    rect: Rect,
    img_w: u32,
    img_h: u32,
    img_rgba: &[u8],
    fit: ObjectFit,
    corner_radius: f32,
    opacity: f32,
) {
    if img_w == 0 || img_h == 0 || rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }

    let x0 = (rect.x.floor().max(0.0) as u32).min(canvas_w);
    let y0 = (rect.y.floor().max(0.0) as u32).min(canvas_h);
    let x1 = ((rect.x + rect.width).ceil().max(0.0) as u32).min(canvas_w);
    let y1 = ((rect.y + rect.height).ceil().max(0.0) as u32).min(canvas_h);
    let r = corner_radius.max(0.0);

    let (scale_x, scale_y, off_x, off_y) = match fit {
        ObjectFit::Fill => (1.0f32, 1.0f32, 0.0f32, 0.0f32),
        ObjectFit::Contain => {
            let box_ar = rect.width / rect.height;
            let img_ar = (img_w as f32) / (img_h as f32);
            if img_ar > box_ar {
                let s = box_ar / img_ar;
                (1.0, s, 0.0, (1.0 - s) * 0.5)
            } else {
                let s = img_ar / box_ar;
                (s, 1.0, (1.0 - s) * 0.5, 0.0)
            }
        }
        ObjectFit::Cover => {
            let box_ar = rect.width / rect.height;
            let img_ar = (img_w as f32) / (img_h as f32);
            if img_ar > box_ar {
                let s = box_ar / img_ar;
                (s, 1.0, (1.0 - s) * 0.5, 0.0)
            } else {
                let s = img_ar / box_ar;
                (1.0, s, 0.0, (1.0 - s) * 0.5)
            }
        }
    };

    for py in y0..y1 {
        for px in x0..x1 {
            let lx = (px as f32 + 0.5) - rect.x;
            let ly = (py as f32 + 0.5) - rect.y;
            if r > 0.0 {
                let dx = (lx - r).min(0.0) + (lx - (rect.width - r)).max(0.0);
                let dy = (ly - r).min(0.0) + (ly - (rect.height - r)).max(0.0);
                if dx * dx + dy * dy > r * r {
                    continue;
                }
            }
            let norm_x = lx / rect.width;
            let norm_y = ly / rect.height;
            let u = match fit {
                ObjectFit::Contain => (norm_x - off_x) / scale_x,
                ObjectFit::Cover => off_x + norm_x * scale_x,
                ObjectFit::Fill => norm_x,
            };
            let v = match fit {
                ObjectFit::Contain => (norm_y - off_y) / scale_y,
                ObjectFit::Cover => off_y + norm_y * scale_y,
                ObjectFit::Fill => norm_y,
            };
            if !(0.0..=1.0).contains(&u) || !(0.0..=1.0).contains(&v) {
                continue;
            }
            let sx = ((u * (img_w as f32)).floor() as u32).min(img_w - 1);
            let sy = ((v * (img_h as f32)).floor() as u32).min(img_h - 1);
            let s_idx = ((sy * img_w + sx) as usize) * 4;
            let sa = (f32::from(img_rgba[s_idx + 3]) / 255.0) * opacity;
            if sa <= 0.0 {
                continue;
            }
            let sr = (f32::from(img_rgba[s_idx]) * sa).round() as u32;
            let sg = (f32::from(img_rgba[s_idx + 1]) * sa).round() as u32;
            let sb = (f32::from(img_rgba[s_idx + 2]) * sa).round() as u32;
            let sa_u = (sa * 255.0).round() as u32;
            let inv = 255u32.saturating_sub(sa_u);
            let d_idx = ((py * canvas_w + px) as usize) * 4;
            target_rgba[d_idx] = (sr + (u32::from(target_rgba[d_idx]) * inv + 127) / 255).min(255) as u8;
            target_rgba[d_idx + 1] =
                (sg + (u32::from(target_rgba[d_idx + 1]) * inv + 127) / 255).min(255) as u8;
            target_rgba[d_idx + 2] =
                (sb + (u32::from(target_rgba[d_idx + 2]) * inv + 127) / 255).min(255) as u8;
            target_rgba[d_idx + 3] =
                (sa_u + (u32::from(target_rgba[d_idx + 3]) * inv + 127) / 255).min(255) as u8;
        }
    }
}

fn composite_pixmap_over(dst: &mut Pixmap, src: &Pixmap) {
    for (d, s) in dst.data_mut().iter_mut().zip(src.data().iter()) {
        if s.a == 0 {
            continue;
        }
        if s.a == 255 || d.a == 0 {
            *d = *s;
            continue;
        }
        let inv_sa = 255u32 - u32::from(s.a);
        let blend_ch = |sc: u8, dc: u8| -> u8 {
            (u32::from(sc) + (u32::from(dc) * inv_sa + 127) / 255).min(255) as u8
        };
        *d = PremulRgba8 {
            r: blend_ch(s.r, d.r),
            g: blend_ch(s.g, d.g),
            b: blend_ch(s.b, d.b),
            a: blend_ch(s.a, d.a),
        };
    }
}

pub(crate) fn combine_transforms(parent: Transform2D, child: Transform2D) -> Transform2D {
    Transform2D {
        translate_x: parent.translate_x + child.translate_x * parent.scale_x,
        translate_y: parent.translate_y + child.translate_y * parent.scale_y,
        scale_x: parent.scale_x * child.scale_x,
        scale_y: parent.scale_y * child.scale_y,
        rotation_deg: parent.rotation_deg + child.rotation_deg,
    }
}

pub(crate) fn build_affine(rect: Rect, tr: Transform2D) -> Affine {
    if tr.is_identity() {
        return Affine::IDENTITY;
    }
    let (cx, cy) = rect.center();
    let rad = f64::from(tr.rotation_deg).to_radians();
    Affine::translate((f64::from(tr.translate_x), f64::from(tr.translate_y)))
        * Affine::translate((f64::from(cx), f64::from(cy)))
        * Affine::rotate(rad)
        * Affine::scale_non_uniform(f64::from(tr.scale_x), f64::from(tr.scale_y))
        * Affine::translate((-f64::from(cx), -f64::from(cy)))
}

pub(crate) fn rect_to_path(rect: Rect, corner_radius: f32) -> BezPath {
    if corner_radius > 0.0 {
        let r = f64::from(corner_radius)
            .min(f64::from(rect.width) / 2.0)
            .min(f64::from(rect.height) / 2.0);
        RoundedRect::new(
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.x + rect.width),
            f64::from(rect.y + rect.height),
            r,
        )
        .to_path(0.25)
    } else {
        KurboRect::new(
            f64::from(rect.x),
            f64::from(rect.y),
            f64::from(rect.x + rect.width),
            f64::from(rect.y + rect.height),
        )
        .to_path(0.25)
    }
}

pub(crate) fn stroke_rect_ring(rect: Rect, corner_radius: f32, stroke_width: f32) -> Option<BezPath> {
    let sw = stroke_width.max(0.0);
    if sw <= 0.0 || rect.width <= 0.0 || rect.height <= 0.0 {
        return None;
    }
    let half = sw * 0.5;
    let outer_rect = Rect::new(
        rect.x - half,
        rect.y - half,
        rect.width + sw,
        rect.height + sw,
    );
    let mut path = rect_to_path(outer_rect, (corner_radius + half).max(0.0));
    let iw = rect.width - sw;
    let ih = rect.height - sw;
    if iw > 0.0 && ih > 0.0 {
        let ix0 = f64::from(rect.x + half);
        let iy0 = f64::from(rect.y + half);
        let ix1 = f64::from(rect.x + half + iw);
        let iy1 = f64::from(rect.y + half + ih);
        path.move_to(KurboPoint::new(ix0, iy0));
        path.line_to(KurboPoint::new(ix0, iy1));
        path.line_to(KurboPoint::new(ix1, iy1));
        path.line_to(KurboPoint::new(ix1, iy0));
        path.close_path();
    }
    Some(path)
}

pub(crate) fn reversed_circle_path(cx: f32, cy: f32, r: f32) -> BezPath {
    let mut path = BezPath::new();
    let steps = 32usize;
    for i in 0..=steps {
        let angle = -2.0 * std::f64::consts::PI * (i as f64) / (steps as f64);
        let px = f64::from(cx) + f64::from(r) * angle.cos();
        let py = f64::from(cy) + f64::from(r) * angle.sin();
        if i == 0 {
            path.move_to(KurboPoint::new(px, py));
        } else {
            path.line_to(KurboPoint::new(px, py));
        }
    }
    path.close_path();
    path
}

pub(crate) fn sample_gradient_stops(stops: &[GradientStop], t: f32) -> Color {
    if stops.is_empty() {
        return Color::BLACK;
    }
    if stops.len() == 1 || t <= stops[0].offset {
        return stops[0].color;
    }
    let last = &stops[stops.len() - 1];
    if t >= last.offset {
        return last.color;
    }
    for win in stops.windows(2) {
        let s0 = &win[0];
        let s1 = &win[1];
        if t >= s0.offset && t <= s1.offset {
            let span = (s1.offset - s0.offset).max(1e-6);
            let u = ((t - s0.offset) / span).clamp(0.0, 1.0);
            let lerp = |a: u8, b: u8| -> u8 {
                (f32::from(a) + (f32::from(b) - f32::from(a)) * u)
                    .round()
                    .clamp(0.0, 255.0) as u8
            };
            return Color::rgba(
                lerp(s0.color.r, s1.color.r),
                lerp(s0.color.g, s1.color.g),
                lerp(s0.color.b, s1.color.b),
                lerp(s0.color.a, s1.color.a),
            );
        }
    }
    last.color
}

fn fill_gradient_cpu(
    ctx: &mut RenderContext,
    rect: Rect,
    corner_radius: f32,
    grad: &Gradient,
    opacity: f32,
    affine: Affine,
) {
    match grad {
        Gradient::Linear { angle_deg, stops } => {
            let bands = 32usize;
            let horizontal =
                angle_deg.rem_euclid(180.0) >= 45.0 && angle_deg.rem_euclid(180.0) < 135.0;
            for i in 0..bands {
                let t0 = (i as f32) / (bands as f32);
                let t1 = ((i + 1) as f32) / (bands as f32);
                let c = sample_gradient_stops(stops, 0.5 * (t0 + t1)).with_opacity(opacity);
                ctx.set_paint(to_paint(c));
                let sub = if horizontal {
                    KurboRect::new(
                        f64::from(rect.x + rect.width * t0),
                        f64::from(rect.y),
                        f64::from(rect.x + rect.width * t1),
                        f64::from(rect.y + rect.height),
                    )
                } else {
                    KurboRect::new(
                        f64::from(rect.x),
                        f64::from(rect.y + rect.height * t0),
                        f64::from(rect.x + rect.width),
                        f64::from(rect.y + rect.height * t1),
                    )
                };
                if affine == Affine::IDENTITY && corner_radius <= 0.0 {
                    ctx.fill_rect(&sub);
                } else {
                    ctx.fill_path(&(affine * sub.to_path(0.25)));
                }
            }
        }
        Gradient::Radial { stops } => {
            let rings = 24usize;
            let (cx, cy) = rect.center();
            let max_r = rect.width.hypot(rect.height) * 0.5;
            for i in (0..rings).rev() {
                let t = ((i + 1) as f32) / (rings as f32);
                let c = sample_gradient_stops(stops, t).with_opacity(opacity);
                ctx.set_paint(to_paint(c));
                let circle =
                    KurboCircle::new((f64::from(cx), f64::from(cy)), f64::from(max_r * t));
                ctx.fill_path(&(affine * circle.to_path(0.25)));
            }
        }
    }
}

pub(crate) fn scissor_for(rect: Rect, width: u16, height: u16) -> (u32, u32, u32, u32) {
    let w = f32::from(width);
    let h = f32::from(height);
    let x0 = rect.x.clamp(0.0, w);
    let y0 = rect.y.clamp(0.0, h);
    let x1 = (rect.x + rect.width).clamp(0.0, w);
    let y1 = (rect.y + rect.height).clamp(0.0, h);
    let sw = (x1 - x0).max(0.0) as u32;
    let sh = (y1 - y0).max(0.0) as u32;
    (x0 as u32, y0 as u32, sw.max(1), sh.max(1))
}

fn to_paint(c: Color) -> AlphaColor<Srgb> {
    AlphaColor::<Srgb>::from_rgba8(c.r, c.g, c.b, c.a)
}

pub(crate) fn to_align(a: TextAlign) -> Align {
    match a {
        TextAlign::Left => Align::Left,
        TextAlign::Center => Align::Center,
        TextAlign::Right => Align::Right,
    }
}

#[cfg(feature = "gpu")]
pub(crate) use to_align as to_shaper_align;

#[allow(unused)]
fn _stroke_marker(_: &StrokeStyle) {}

#[cfg(test)]
mod tests {
    use super::*;
    use kine_core::{Color, NodeKind, Scene};

    const FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

    fn renderer() -> FrameRenderer {
        let mut r = FrameRenderer::new();
        r.load_font("JetBrains Mono", FONT.to_vec());
        r
    }

    fn sample_scene() -> Scene {
        let mut scene = Scene::new(320, 200);
        let bg = scene.add_node(
            scene.root,
            NodeKind::Rect {
                fill: Color::rgb(20, 24, 40),
                corner_radius: 0.0,
            },
        );
        scene.with_layout(bg, |l| {
            l.position = taffy::Position::Absolute;
            l.inset = taffy::Rect {
                top: taffy::LengthPercentageAuto::length(0.0),
                left: taffy::LengthPercentageAuto::length(0.0),
                right: taffy::LengthPercentageAuto::length(0.0),
                bottom: taffy::LengthPercentageAuto::length(0.0),
            };
        });
        let card = scene.add_node(
            scene.root,
            NodeKind::Rect {
                fill: Color::rgb(240, 240, 245),
                corner_radius: 12.0,
            },
        );
        scene.with_layout(card, |l| {
            l.size = taffy::Size {
                width: taffy::Dimension::length(200.0),
                height: taffy::Dimension::length(80.0),
            };
            l.margin = taffy::Rect {
                left: taffy::LengthPercentageAuto::length(60.0),
                top: taffy::LengthPercentageAuto::length(60.0),
                right: taffy::LengthPercentageAuto::length(0.0),
                bottom: taffy::LengthPercentageAuto::length(0.0),
            };
        });
        let label = scene.add_node(
            card,
            NodeKind::Text {
                content: "kine".into(),
                font_family: "JetBrains Mono".into(),
                font_size: 24.0,
                font_weight: 400,
                color: Color::rgb(20, 24, 40),
                align: TextAlign::Center,
            },
        );
        scene.with_layout(label, |l| {
            l.size = taffy::Size {
                width: taffy::Dimension::length(200.0),
                height: taffy::Dimension::length(40.0),
            };
            l.inset = taffy::Rect {
                top: taffy::LengthPercentageAuto::length(20.0),
                left: taffy::LengthPercentageAuto::auto(),
                right: taffy::LengthPercentageAuto::auto(),
                bottom: taffy::LengthPercentageAuto::auto(),
            };
        });
        scene.compute_layout().unwrap();
        scene
    }

    #[test]
    fn renders_scene_to_pixmap() {
        let mut r = renderer();
        let scene = sample_scene();
        let pixmap = r.render(&scene).unwrap();
        assert_eq!(pixmap.width(), 320);
        assert_eq!(pixmap.height(), 200);
        let p0 = pixmap.data()[0];
        assert!(p0.r < 60 && p0.b > 30, "unexpected bg pixel {p0:?}");
        let card = pixmap.data()[100 * 320 + 100];
        assert!(card.r > 200, "unexpected card pixel {card:?}");
    }

    #[test]
    fn renders_video_node_over_time() {
        let mut scene = Scene::new(32, 24);
        let vid = scene.add_node(
            scene.root,
            NodeKind::Video {
                src: "video:plasma:32x24:15:1.0".into(),
                fit: ObjectFit::Cover,
                corner_radius: 0.0,
                start: 0.0,
                trim_start: 0.0,
                trim_end: None,
                playback_rate: 1.0,
                loop_video: true,
            },
        );
        scene.with_layout(vid, |l| {
            l.position = taffy::Position::Absolute;
            l.size = taffy::Size {
                width: taffy::Dimension::length(32.0),
                height: taffy::Dimension::length(24.0),
            };
        });
        scene.compute_layout().unwrap();

        let mut r = renderer();
        let f0 = r.render_at(&scene, 0.0, 0).unwrap();
        let f5 = r.render_at(&scene, 0.4, 6).unwrap();
        assert_ne!(
            f0.data_as_u8_slice(),
            f5.data_as_u8_slice(),
            "video node must advance frames over time"
        );
    }

    #[test]
    fn rendering_is_deterministic() {
        let scene = sample_scene();
        let mut r1 = renderer();
        let mut r2 = renderer();
        let a = r1.render(&scene).unwrap();
        let b = r2.render(&scene).unwrap();
        assert_eq!(a.data_as_u8_slice(), b.data_as_u8_slice());
    }

    #[test]
    fn cpu_renderer_executes_wgsl_shader_layers() {
        let mut scene = Scene::new(32, 32);
        let bg = scene.add_node(
            scene.root,
            NodeKind::Rect {
                fill: Color::rgb(20, 40, 80),
                corner_radius: 0.0,
            },
        );
        scene.with_layout(bg, |l| {
            l.position = taffy::Position::Absolute;
            l.size = taffy::Size {
                width: taffy::Dimension::length(32.0),
                height: taffy::Dimension::length(32.0),
            };
        });
        let fx = scene.add_node(
            scene.root,
            NodeKind::Shader {
                source: "@fragment fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> { return vec4<f32>(1.0, 0.0, 0.0, 1.0); }".into(),
                uniforms: serde_json::Map::new(),
                blend: ShaderBlend::Normal,
            },
        );
        scene.with_layout(fx, |l| {
            l.position = taffy::Position::Absolute;
            l.inset = taffy::Rect {
                top: taffy::LengthPercentageAuto::length(16.0),
                left: taffy::LengthPercentageAuto::length(0.0),
                right: taffy::LengthPercentageAuto::auto(),
                bottom: taffy::LengthPercentageAuto::auto(),
            };
            l.size = taffy::Size {
                width: taffy::Dimension::length(32.0),
                height: taffy::Dimension::length(16.0),
            };
        });
        scene.compute_layout().unwrap();

        let mut r = renderer();
        let pixmap = r.render_at(&scene, 0.5, 15).unwrap();
        let top_px = pixmap.data()[4 * 32 + 4];
        assert_eq!(top_px.r, 20);
        let bot_px = pixmap.data()[24 * 32 + 4];
        assert_eq!(bot_px.r, 255);
        assert_eq!(bot_px.g, 0);
    }

    #[test]
    fn png_has_valid_signature() {
        let mut r = renderer();
        let scene = sample_scene();
        let pixmap = r.render(&scene).unwrap();
        let png = png::pixmap_to_png(&pixmap).unwrap();
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]);
    }

    fn assert_roundtrip_lossless(pixmap: &Pixmap, label: &str) -> usize {
        let w = u32::from(pixmap.width());
        let png_bytes = png::pixmap_to_png(pixmap).unwrap();
        let decoded = image::load_from_memory(&png_bytes).unwrap().to_rgba8();
        let mut partial = 0usize;
        for (i, p) in pixmap.data().iter().enumerate() {
            let d = decoded.get_pixel((i as u32) % w, (i as u32) / w).0;
            let a = u32::from(p.a);
            if p.a > 0 && p.a < 255 {
                partial += 1;
            }
            let premul = |c: u8| ((u32::from(c) * a + 127) / 255).min(255) as u8;
            let deltas = [
                premul(d[0]).abs_diff(p.r),
                premul(d[1]).abs_diff(p.g),
                premul(d[2]).abs_diff(p.b),
                d[3].abs_diff(p.a),
            ];
            let worst = *deltas.iter().max().unwrap();
            assert!(
                worst <= 1,
                "{label}: pixel {i} changed by {worst} (premul {:?} -> decoded {:?})",
                (p.r, p.g, p.b, p.a),
                d
            );
        }
        partial
    }

    #[test]
    fn png_roundtrip_preserves_partial_alpha() {
        let mut pixmap = Pixmap::new(4, 1);
        {
            let px = pixmap.data_mut();
            px[0] = PremulRgba8 { r: 0, g: 0, b: 0, a: 0 };
            px[1] = PremulRgba8 { r: 1, g: 0, b: 1, a: 1 };
            px[2] = PremulRgba8 { r: 10, g: 20, b: 30, a: 128 };
            px[3] = PremulRgba8 { r: 200, g: 30, b: 30, a: 255 };
        }
        let partial = assert_roundtrip_lossless(&pixmap, "synthetic");
        assert_eq!(partial, 2, "the test must exercise partial-alpha pixels");
    }

    #[test]
    fn png_roundtrip_is_lossless_for_rendered_text() {
        let mut scene = Scene::new(96, 48);
        let label = scene.add_node(
            scene.root,
            NodeKind::Text {
                content: "Ag".into(),
                font_family: "JetBrains Mono".into(),
                font_size: 32.0,
                font_weight: 400,
                color: kine_core::Color::WHITE,
                align: TextAlign::Left,
            },
        );
        scene.with_layout(label, |l| {
            l.size = taffy::Size {
                width: taffy::Dimension::length(96.0),
                height: taffy::Dimension::length(48.0),
            };
        });
        scene.compute_layout().unwrap();

        let mut r = renderer();
        let pixmap = r.render(&scene).unwrap();
        let partial = assert_roundtrip_lossless(&pixmap, "text");
        assert!(
            partial > 0,
            "rendered text should produce antialiased pixels"
        );
    }
}
