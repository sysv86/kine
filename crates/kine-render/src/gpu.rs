//! GPU path: Vello GPU scene rasterization plus agent-authored WGSL effect
//! passes.

use crate::video_io::VideoCache;
use crate::{
    build_affine, combine_transforms, image_source_pixels, png, rect_to_path,
    reversed_circle_path, sample_gradient_stops, scissor_for, stroke_rect_ring, to_shaper_align,
    RenderError,
};
use kine_core::{Gradient, NodeKind, ObjectFit, Rect, Scene, ShaderBlend, Transform2D};
use kine_shader::{ShaderModule, UniformPack};
use kine_text::TextShaper;
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::sync::Arc;
use vello_gpu::{
    Image, ImageSource, RectU16, RenderSize, RenderTargetConfig,
    Renderer as VelloRenderer, Resources as VelloResources, Scene as VelloScene, TargetInit,
    TextureBindings, TextureId,
};

pub const TARGET_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

pub const VERTEX_WGSL: &str = r#"
struct VertexOut {
    @builtin(position) position: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) index: u32) -> VertexOut {
    let x = f32((index << 1u) & 2u);
    let y = f32(index & 2u);
    var out: VertexOut;
    out.uv = vec2<f32>(x, y);
    out.position = vec4<f32>(x * 2.0 - 1.0, 1.0 - y * 2.0, 0.0, 1.0);
    return out;
}
"#;

pub const BLIT_WGSL: &str = r#"
@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    return textureSample(kine_input, kine_sampler, uv);
}
"#;

#[derive(Debug, Clone, serde::Serialize)]
pub struct GpuAdapterDetails {
    pub name: String,
    pub vendor: u32,
    pub device: u32,
    pub device_type: String,
    pub driver: String,
    pub driver_info: String,
    pub backend: String,
}

#[derive(Debug, thiserror::Error)]
pub enum GpuError {
    #[error("no usable GPU adapter: {0}")]
    NoAdapter(String),

    #[error("failed to create a GPU device: {0}")]
    Device(String),

    #[error("shader layer on node {node} cannot run: {reason}")]
    UnsupportedShader { node: u64, reason: String },

    #[error("GPU readback failed: {0}")]
    Readback(String),
}

/// Process-wide wgpu instance. Recreating instances per probe makes the Vulkan
/// loader unload ICDs while other threads enumerate extensions, which crashes
/// inside `libvulkan`.
fn shared_instance() -> &'static wgpu::Instance {
    static INSTANCE: std::sync::OnceLock<wgpu::Instance> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        // GL/EGL is not a supported backend here (Vulkan/Metal/DX12 are) and
        // its per-thread display setup fails when several threads start it at
        // once (`egl.rs: BadAccess`), so leave it out of the enumeration.
        desc.backends = wgpu::Backends::PRIMARY;
        wgpu::Instance::new(desc)
    })
}

/// Whether a GPU adapter can be reached. Probed once per process.
pub fn gpu_adapter_present() -> bool {
    static PRESENT: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *PRESENT.get_or_init(|| request_adapter_sync().is_ok())
}

pub fn probe_gpu_adapter() -> Option<GpuAdapterDetails> {
    let adapter = request_adapter_sync().ok()?;
    let info = adapter.get_info();
    Some(GpuAdapterDetails {
        name: info.name,
        vendor: info.vendor,
        device: info.device,
        device_type: format!("{:?}", info.device_type),
        driver: info.driver,
        driver_info: info.driver_info,
        backend: format!("{:?}", info.backend),
    })
}

pub fn request_adapter() -> Result<wgpu::Adapter, GpuError> {
    request_adapter_sync()
}

fn request_adapter_sync() -> Result<wgpu::Adapter, GpuError> {
    let instance = shared_instance();
    let primary = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        compatible_surface: None,
        ..Default::default()
    }));
    if let Ok(adapter) = primary {
        return Ok(adapter);
    }

    let allow_sw = std::env::var("KINE_ALLOW_SOFTWARE_GPU")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false);
    if allow_sw {
        if let Ok(fallback) =
            pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
                power_preference: wgpu::PowerPreference::None,
                force_fallback_adapter: true,
                compatible_surface: None,
                ..Default::default()
            }))
        {
            return Ok(fallback);
        }
    }

    primary.map_err(|e| GpuError::NoAdapter(e.to_string()))
}

struct EffectPass {
    node: u64,
    pipeline_key: (u64, ShaderBlend),
    frame_buffer: wgpu::Buffer,
    agent_uniforms: Option<wgpu::Buffer>,
    scissor: (u32, u32, u32, u32),
}

fn hash_shader_source(source: &str) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    source.hash(&mut hasher);
    hasher.finish()
}

fn hash_image_bytes(bytes: &[u8]) -> u64 {
    let mut hasher = std::collections::hash_map::DefaultHasher::new();
    bytes.hash(&mut hasher);
    hasher.finish()
}

/// One RGBA8 texture uploaded for an `image()`/`video()` node.
struct GpuImage {
    #[allow(dead_code)]
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u16,
    height: u16,
    /// Content key of the pixels in the texture: `None` for a static image,
    /// `Some(hash)` for the video frame that was uploaded.
    frame_key: Option<u64>,
}

/// Device hooks plus the caches needed to turn image/video nodes into GPU
/// textures, and the bindings the renderer resolves those textures through.
struct ImageDrawCtx<'a> {
    device: &'a wgpu::Device,
    queue: &'a wgpu::Queue,
    video_cache: &'a mut VideoCache,
    image_textures: &'a mut HashMap<String, GpuImage>,
    video_textures: &'a mut HashMap<u64, GpuImage>,
    bindings: &'a mut TextureBindings,
}

impl ImageDrawCtx<'_> {
    fn upload(
        &self,
        label: &str,
        width: u16,
        height: u16,
        rgba: &[u8],
        frame_key: Option<u64>,
    ) -> GpuImage {
        let size = wgpu::Extent3d {
            width: u32::from(width),
            height: u32::from(height),
            depth_or_array_layers: 1,
        };
        let texture = self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some(label),
            size,
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        });
        // The decoders hand us straight alpha; the image pipeline composites
        // premultiplied pixels.
        let premultiplied: Vec<u8> = rgba
            .chunks_exact(4)
            .flat_map(|p| {
                let a = u32::from(p[3]);
                [
                    ((u32::from(p[0]) * a + 127) / 255) as u8,
                    ((u32::from(p[1]) * a + 127) / 255) as u8,
                    ((u32::from(p[2]) * a + 127) / 255) as u8,
                    p[3],
                ]
            })
            .collect();
        self.queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: 0,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            &premultiplied,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(u32::from(width) * 4),
                rows_per_image: Some(u32::from(height)),
            },
            size,
        );
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
        GpuImage {
            texture,
            view,
            width,
            height,
            frame_key,
        }
    }

    /// Uploaded texture for a static `image()` source (pattern or file).
    fn image_pixels(&mut self, src: &str) -> Option<(wgpu::TextureView, u16, u16)> {
        if let Some(cached) = self.image_textures.get(src) {
            return Some((cached.view.clone(), cached.width, cached.height));
        }
        let (w, h, rgba) = image_source_pixels(src)?;
        if w == 0 || h == 0 || w > u32::from(u16::MAX) || h > u32::from(u16::MAX) {
            return None;
        }
        let uploaded = self.upload("kine.image", w as u16, h as u16, &rgba, None);
        let out = (uploaded.view.clone(), uploaded.width, uploaded.height);
        self.image_textures.insert(src.to_string(), uploaded);
        Some(out)
    }

    /// Uploaded texture for the frame a `video()` node shows at `time`.
    #[allow(clippy::too_many_arguments)]
    fn video_pixels(
        &mut self,
        node: u64,
        src: &str,
        time: f32,
        start: f64,
        trim_start: f64,
        trim_end: Option<f64>,
        playback_rate: f64,
        loop_video: bool,
    ) -> Result<Option<(wgpu::TextureView, u16, u16)>, RenderError> {
        let clip = self
            .video_cache
            .get_or_load(src)
            .map_err(|reason| RenderError::VideoInput { node, reason })?;
        let Some(frame) = clip.sample_frame(
            f64::from(time),
            start,
            trim_start,
            trim_end,
            playback_rate,
            loop_video,
        ) else {
            return Ok(None);
        };
        let (width, height) = (clip.width as u16, clip.height as u16);
        if width == 0 || height == 0 {
            return Ok(None);
        }
        let frame_key = hash_image_bytes(frame);
        if let Some(cached) = self.video_textures.get(&node) {
            if cached.frame_key == Some(frame_key)
                && cached.width == width
                && cached.height == height
            {
                return Ok(Some((cached.view.clone(), width, height)));
            }
        }
        let uploaded = self.upload("kine.video", width, height, frame, Some(frame_key));
        let out = (uploaded.view.clone(), width, height);
        self.video_textures.insert(node, uploaded);
        Ok(Some(out))
    }
}

/// Draw one image/video texture into `rect`, honouring `object-fit` and corner
/// radius the same way the CPU blitter does.
#[allow(clippy::too_many_arguments)]
fn draw_image_gpu(
    target: &mut VelloScene,
    view: &wgpu::TextureView,
    node: u64,
    tex_w: u16,
    tex_h: u16,
    rect: Rect,
    fit: ObjectFit,
    corner_radius: f32,
    opacity: f32,
    affine: vello_gpu::kurbo::Affine,
    bindings: &mut TextureBindings,
) {
    use vello_gpu::kurbo::Affine;
    use vello_gpu::peniko::{ImageQuality, ImageSampler};

    if tex_w == 0 || tex_h == 0 || rect.width <= 0.0 || rect.height <= 0.0 {
        return;
    }
    let (iw, ih) = (f32::from(tex_w), f32::from(tex_h));
    let (scale_x, scale_y, off_x, off_y) = match fit {
        ObjectFit::Fill => (rect.width / iw, rect.height / ih, 0.0, 0.0),
        ObjectFit::Contain => {
            let s = (rect.width / iw).min(rect.height / ih);
            (s, s, (rect.width - iw * s) * 0.5, (rect.height - ih * s) * 0.5)
        }
        ObjectFit::Cover => {
            let s = (rect.width / iw).max(rect.height / ih);
            (s, s, (rect.width - iw * s) * 0.5, (rect.height - ih * s) * 0.5)
        }
    };
    if !scale_x.is_finite() || !scale_y.is_finite() || scale_x <= 0.0 || scale_y <= 0.0 {
        return;
    }
    // The placed image may be larger than the box (cover) or smaller (contain),
    // so only the part that lands inside the box is filled.
    let x0 = off_x.max(0.0);
    let y0 = off_y.max(0.0);
    let x1 = (off_x + iw * scale_x).min(rect.width);
    let y1 = (off_y + ih * scale_y).min(rect.height);
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let dest = Rect::new(rect.x + x0, rect.y + y0, x1 - x0, y1 - y0);

    let image = Image {
        image: ImageSource::external_texture(
            TextureId(node),
            RectU16::new(0, 0, tex_w, tex_h),
            true,
        ),
        sampler: ImageSampler {
            alpha: opacity,
            quality: ImageQuality::Medium,
            ..Default::default()
        },
    };
    // Image texels -> node-local coordinates, then through the node transform.
    let paint = affine
        * Affine::new([
            f64::from(scale_x),
            0.0,
            0.0,
            f64::from(scale_y),
            f64::from(off_x),
            f64::from(off_y),
        ]);

    target.push_clip_path(&(affine * rect_to_path(rect, corner_radius)));
    target.set_paint(image);
    target.set_paint_transform(paint);
    target.fill_path(&(affine * rect_to_path(dest, 0.0)));
    target.reset_paint_transform();
    target.pop_clip();

    bindings.insert(TextureId(node), view.clone());
}

pub struct GpuRenderer {
    device: wgpu::Device,
    queue: wgpu::Queue,
    vello: VelloRenderer,
    vello_resources: VelloResources,
    shaper: TextShaper,
    vertex_module: wgpu::ShaderModule,
    blit_pipeline: wgpu::RenderPipeline,
    blit_frame_buf: wgpu::Buffer,
    blit_agent_buf: wgpu::Buffer,
    frame_layout: wgpu::BindGroupLayout,
    agent_layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
    pipeline_cache: HashMap<(u64, ShaderBlend), wgpu::RenderPipeline>,
    /// Decoded video clips, mirrored from the CPU renderer so both backends
    /// resolve `video()` sources identically (including preloaded slices).
    video_cache: VideoCache,
    /// Uploaded `image()` textures, keyed by source string.
    image_textures: HashMap<String, GpuImage>,
    /// Uploaded `video()` textures, keyed by node id (a video's pixels change
    /// every frame, so it cannot be keyed by source like a static image).
    video_textures: HashMap<u64, GpuImage>,
    width: u16,
    height: u16,
}

impl GpuRenderer {
    pub fn new(width: u16, height: u16) -> Result<Self, GpuError> {
        let adapter = request_adapter_sync()?;
        let (device, queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("kine"),
                ..Default::default()
            }))
            .map_err(|e| GpuError::Device(e.to_string()))?;
        Self::from_device(device, queue, width, height)
    }

    pub fn from_device(
        device: wgpu::Device,
        queue: wgpu::Queue,
        width: u16,
        height: u16,
    ) -> Result<Self, GpuError> {
        let width = width.max(1);
        let height = height.max(1);
        let (vello, vello_resources) = VelloRenderer::new(
            &device,
            &RenderTargetConfig {
                format: TARGET_FORMAT,
                width,
                height,
            },
        );

        let vertex_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kine.effect.vertex"),
            source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
        });

        let frame_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kine.effect.frame"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: wgpu::BufferSize::new(u64::from(
                            kine_shader::uniforms::FRAME_BINDING_SIZE,
                        )),
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });

        let agent_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kine.effect.agent"),
            entries: &[wgpu::BindGroupLayoutEntry {
                binding: 0,
                visibility: wgpu::ShaderStages::FRAGMENT,
                ty: wgpu::BindingType::Buffer {
                    ty: wgpu::BufferBindingType::Uniform,
                    has_dynamic_offset: false,
                    min_binding_size: None,
                },
                count: None,
            }],
        });

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("kine.effect.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        let blit_pipeline = {
            let fragment = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("kine.blit.fragment"),
                source: wgpu::ShaderSource::Wgsl(kine_shader::wrap(BLIT_WGSL).into()),
            });
            let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("kine.blit.layout"),
                bind_group_layouts: &[Some(&frame_layout), Some(&agent_layout)],
                immediate_size: 0,
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some("kine.blit.pipeline"),
                layout: Some(&layout),
                vertex: wgpu::VertexState {
                    module: &vertex_module,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fragment,
                    entry_point: Some(kine_shader::ENTRY_POINT),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: TARGET_FORMAT,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let blit_frame_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kine.blit.frame"),
            size: u64::from(kine_shader::uniforms::FRAME_BINDING_SIZE),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let blit_agent_buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kine.blit.agent"),
            size: 16,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        Ok(Self {
            device,
            queue,
            vello,
            vello_resources,
            shaper: TextShaper::new(),
            vertex_module,
            blit_pipeline,
            blit_frame_buf,
            blit_agent_buf,
            frame_layout,
            agent_layout,
            sampler,
            pipeline_cache: HashMap::new(),
            video_cache: VideoCache::new(),
            image_textures: HashMap::new(),
            video_textures: HashMap::new(),
            width,
            height,
        })
    }

    pub fn load_font(&mut self, family: &str, data: Vec<u8>) {
        self.shaper.load_font(family, data);
    }

    pub fn sync_fonts_from(&mut self, source: &TextShaper) {
        self.shaper.sync_from(source);
    }

    /// Video clip cache used for `video()` nodes, mirroring the CPU renderer's.
    pub fn video_cache_mut(&mut self) -> &mut VideoCache {
        &mut self.video_cache
    }

    pub fn ensure_size(&mut self, width: u16, height: u16) {
        let width = width.max(1);
        let height = height.max(1);
        if self.width != width || self.height != height {
            let (vello, vello_resources) = VelloRenderer::new(
                &self.device,
                &RenderTargetConfig {
                    format: TARGET_FORMAT,
                    width,
                    height,
                },
            );
            self.vello = vello;
            self.vello_resources = vello_resources;
            self.width = width;
            self.height = height;
        }
    }

    pub fn size(&self) -> (u16, u16) {
        (self.width, self.height)
    }

    pub fn render(&mut self, scene: &Scene, time: f32) -> Result<vello_cpu::Pixmap, RenderError> {
        self.render_at(scene, time, (time * 30.0).round().max(0.0) as u32)
    }

    pub fn render_at(
        &mut self,
        scene: &Scene,
        time: f32,
        frame: u32,
    ) -> Result<vello_cpu::Pixmap, RenderError> {
        let texture = self.render_to_texture_at(scene, time, frame)?;
        read_texture(&self.device, &self.queue, &texture, self.width, self.height)
    }

    pub fn render_to_texture(
        &mut self,
        scene: &Scene,
        time: f32,
    ) -> Result<wgpu::Texture, RenderError> {
        self.render_to_texture_at(scene, time, (time * 30.0).round().max(0.0) as u32)
    }

    pub fn render_to_texture_at(
        &mut self,
        scene: &Scene,
        time: f32,
        frame: u32,
    ) -> Result<wgpu::Texture, RenderError> {
        self.ensure_size(scene.width as u16, scene.height as u16);
        let effects = self.prepare_effects(scene)?;

        let mut current = self.rasterize_scene(scene, time)?;
        if effects.is_empty() {
            return Ok(current);
        }
        let mut spare = self.create_target();

        for pass in &effects {
            let input_view = current.create_view(&wgpu::TextureViewDescriptor::default());
            let output_view = spare.create_view(&wgpu::TextureViewDescriptor::default());

            let mut encoder = self
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("kine.effects"),
                });
            self.encode_blit(&mut encoder, &input_view, &output_view);
            self.encode_effect(&mut encoder, pass, &input_view, &output_view, time, frame);
            self.queue.submit(Some(encoder.finish()));

            std::mem::swap(&mut current, &mut spare);
        }

        Ok(current)
    }

    pub fn device(&self) -> &wgpu::Device {
        &self.device
    }

    pub fn queue(&self) -> &wgpu::Queue {
        &self.queue
    }

    fn encode_blit(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
    ) {
        let group0 = self.frame_bind_group(&self.blit_frame_buf, input);
        let group1 = self.agent_bind_group(Some(&self.blit_agent_buf));

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("kine.blit"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rpass.set_pipeline(&self.blit_pipeline);
        rpass.set_bind_group(0, &group0, &[]);
        rpass.set_bind_group(1, &group1, &[]);
        rpass.draw(0..3, 0..1);
    }

    fn encode_effect(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        pass: &EffectPass,
        input: &wgpu::TextureView,
        output: &wgpu::TextureView,
        time: f32,
        frame_index: u32,
    ) {
        let frame = UniformPack::frame(
            (f32::from(self.width), f32::from(self.height)),
            time,
            frame_index,
        );
        self.queue
            .write_buffer(&pass.frame_buffer, 0, frame.as_bytes());

        let group0 = self.frame_bind_group(&pass.frame_buffer, input);
        let group1 = self.agent_bind_group(pass.agent_uniforms.as_ref());
        let pipeline = &self.pipeline_cache[&pass.pipeline_key];

        let mut rpass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("kine.effect"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                resolve_target: None,
                depth_slice: None,
                ops: wgpu::Operations {
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        rpass.set_pipeline(pipeline);
        rpass.set_bind_group(0, &group0, &[]);
        rpass.set_bind_group(1, &group1, &[]);
        rpass.set_scissor_rect(
            pass.scissor.0,
            pass.scissor.1,
            pass.scissor.2,
            pass.scissor.3,
        );
        rpass.draw(0..3, 0..1);
    }

    fn frame_bind_group(
        &self,
        frame: &wgpu::Buffer,
        input: &wgpu::TextureView,
    ) -> wgpu::BindGroup {
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kine.frame_bind_group"),
            layout: &self.frame_layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: frame.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(input),
                },
                wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        })
    }

    fn agent_bind_group(&self, uniforms: Option<&wgpu::Buffer>) -> wgpu::BindGroup {
        let buffer = uniforms.unwrap_or(&self.blit_agent_buf);
        self.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kine.agent_bind_group"),
            layout: &self.agent_layout,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: buffer.as_entire_binding(),
            }],
        })
    }

    fn rasterize_scene(&mut self, scene: &Scene, time: f32) -> Result<wgpu::Texture, RenderError> {
        let mut gpu_scene = VelloScene::new(self.width, self.height);
        // External textures (image/video pixels) are uploaded while the tree is
        // walked and handed to the renderer for this frame.
        let mut bindings = TextureBindings::new();
        {
            let mut images = ImageDrawCtx {
                device: &self.device,
                queue: &self.queue,
                video_cache: &mut self.video_cache,
                image_textures: &mut self.image_textures,
                video_textures: &mut self.video_textures,
                bindings: &mut bindings,
            };
            draw_node_gpu(
                &mut gpu_scene,
                &mut self.vello_resources,
                &mut self.shaper,
                scene,
                scene.root,
                1.0,
                Transform2D::IDENTITY,
                time,
                &mut images,
            )?;
        }

        let texture = self.create_target();
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("kine.scene"),
            });

        self.vello
            .render(
                &gpu_scene,
                &mut self.vello_resources,
                &self.device,
                &self.queue,
                &mut encoder,
                &RenderSize {
                    width: self.width,
                    height: self.height,
                },
                &view,
                None,
                &bindings,
                TargetInit::Clear(vello_gpu::ClearSettings::default()),
            )
            .map_err(|e| RenderError::Gpu(GpuError::Device(e.to_string())))?;
        self.queue.submit(Some(encoder.finish()));
        Ok(texture)
    }

    fn create_target(&self) -> wgpu::Texture {
        self.device.create_texture(&wgpu::TextureDescriptor {
            label: Some("kine.target"),
            size: wgpu::Extent3d {
                width: u32::from(self.width),
                height: u32::from(self.height),
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: TARGET_FORMAT,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | wgpu::TextureUsages::TEXTURE_BINDING
                | wgpu::TextureUsages::COPY_SRC,
            view_formats: &[],
        })
    }

    fn prepare_effects(&mut self, scene: &Scene) -> Result<Vec<EffectPass>, RenderError> {
        let mut passes = Vec::new();
        for (id, node) in scene.nodes.iter() {
            let NodeKind::Shader {
                source,
                uniforms,
                blend,
            } = &node.kind
            else {
                continue;
            };
            let rect = node.abs_pos.ok_or(RenderError::MissingLayout)?;
            let reflected = kine_shader::validate(source).map_err(|e| {
                RenderError::Gpu(GpuError::UnsupportedShader {
                    node: id.0,
                    reason: e.to_string(),
                })
            })?;
            let pass = self.build_pass(id.0, source, &reflected, uniforms, *blend, rect)?;
            passes.push(pass);
        }
        passes.sort_by_key(|p| p.node);
        Ok(passes)
    }

    fn build_pass(
        &mut self,
        node: u64,
        source: &str,
        reflected: &ShaderModule,
        uniforms: &serde_json::Map<String, serde_json::Value>,
        blend: ShaderBlend,
        rect: kine_core::Rect,
    ) -> Result<EffectPass, RenderError> {
        let unsupported = |reason: String| {
            RenderError::Gpu(GpuError::UnsupportedShader { node, reason })
        };

        if !reflected.textures.iter().all(|t| t.engine_provided) {
            return Err(unsupported(
                "declaring your own textures is not supported yet; sample `kine_input` instead"
                    .to_string(),
            ));
        }

        let agent_bindings: Vec<_> = reflected.agent_uniforms().collect();
        if agent_bindings.len() > 1 {
            return Err(unsupported(format!(
                "{} uniform buffers declared; v1 supports one (group 1, binding 0)",
                agent_bindings.len()
            )));
        }

        let pipeline_key = (hash_shader_source(source), blend);
        if !self.pipeline_cache.contains_key(&pipeline_key) {
            let fragment = self.device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some("kine.effect.fragment"),
                source: wgpu::ShaderSource::Wgsl(kine_shader::wrap(source).into()),
            });

            let layout = self
                .device
                .create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                    label: Some("kine.effect.layout"),
                    bind_group_layouts: &[Some(&self.frame_layout), Some(&self.agent_layout)],
                    immediate_size: 0,
                });

            let pipeline = self
                .device
                .create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                    label: Some("kine.effect.pipeline"),
                    layout: Some(&layout),
                    vertex: wgpu::VertexState {
                        module: &self.vertex_module,
                        entry_point: Some("vs_main"),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        buffers: &[],
                    },
                    fragment: Some(wgpu::FragmentState {
                        module: &fragment,
                        entry_point: Some(kine_shader::ENTRY_POINT),
                        compilation_options: wgpu::PipelineCompilationOptions::default(),
                        targets: &[Some(wgpu::ColorTargetState {
                            format: TARGET_FORMAT,
                            blend: Some(blend_state(blend)),
                            write_mask: wgpu::ColorWrites::ALL,
                        })],
                    }),
                    primitive: wgpu::PrimitiveState::default(),
                    depth_stencil: None,
                    multisample: wgpu::MultisampleState::default(),
                    multiview_mask: None,
                    cache: None,
                });
            self.pipeline_cache.insert(pipeline_key, pipeline);
        }

        let frame_buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("kine.effect.frame_uniforms"),
            size: u64::from(kine_shader::uniforms::FRAME_BINDING_SIZE),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });

        let agent_uniforms = match agent_bindings.first() {
            Some(binding) => {
                let bytes = UniformPack::pack(binding, uniforms)
                    .map_err(|e| unsupported(e.to_string()))?;
                let buffer = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("kine.effect.agent_uniforms"),
                    size: u64::from(binding.size.max(16)),
                    usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
                self.queue.write_buffer(&buffer, 0, bytes.as_bytes());
                Some(buffer)
            }
            None => None,
        };

        Ok(EffectPass {
            node,
            pipeline_key,
            frame_buffer,
            agent_uniforms,
            scissor: scissor_for(rect, self.width, self.height),
        })
    }
}

pub fn render_png(scene: &Scene, time: f32) -> Result<Vec<u8>, RenderError> {
    let mut renderer =
        GpuRenderer::new(scene.width as u16, scene.height as u16).map_err(RenderError::Gpu)?;
    let pixmap = renderer.render(scene, time)?;
    png::pixmap_to_png(&pixmap).map_err(RenderError::Image)
}

fn blend_state(blend: ShaderBlend) -> wgpu::BlendState {
    use wgpu::{BlendComponent, BlendFactor, BlendOperation, BlendState};

    const OVER: BlendComponent = BlendComponent {
        src_factor: BlendFactor::SrcAlpha,
        dst_factor: BlendFactor::OneMinusSrcAlpha,
        operation: BlendOperation::Add,
    };
    const PREMUL_OVER: BlendComponent = BlendComponent {
        src_factor: BlendFactor::One,
        dst_factor: BlendFactor::OneMinusSrcAlpha,
        operation: BlendOperation::Add,
    };

    match blend {
        ShaderBlend::Normal => BlendState {
            color: PREMUL_OVER,
            alpha: OVER,
        },
        ShaderBlend::Add => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::One,
                operation: BlendOperation::Add,
            },
            alpha: OVER,
        },
        ShaderBlend::Multiply => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::Dst,
                dst_factor: BlendFactor::Zero,
                operation: BlendOperation::Add,
            },
            alpha: OVER,
        },
        ShaderBlend::Screen => BlendState {
            color: BlendComponent {
                src_factor: BlendFactor::One,
                dst_factor: BlendFactor::OneMinusSrc,
                operation: BlendOperation::Add,
            },
            alpha: OVER,
        },
    }
}

fn read_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture: &wgpu::Texture,
    width: u16,
    height: u16,
) -> Result<vello_cpu::Pixmap, RenderError> {
    let unpadded = u32::from(width) * 4;
    let align = wgpu::COPY_BYTES_PER_ROW_ALIGNMENT;
    let padded = unpadded.div_ceil(align) * align;

    let buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("kine.readback"),
        size: u64::from(padded) * u64::from(height),
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("kine.readback"),
    });
    encoder.copy_texture_to_buffer(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        wgpu::TexelCopyBufferInfo {
            buffer: &buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(padded),
                rows_per_image: Some(u32::from(height)),
            },
        },
        wgpu::Extent3d {
            width: u32::from(width),
            height: u32::from(height),
            depth_or_array_layers: 1,
        },
    );
    queue.submit(Some(encoder.finish()));

    let slice = buffer.slice(..);
    let (tx, rx) = std::sync::mpsc::channel();
    slice.map_async(wgpu::MapMode::Read, move |result| {
        let _ = tx.send(result);
    });
    device
        .poll(wgpu::PollType::wait_indefinitely())
        .map_err(|e| RenderError::Gpu(GpuError::Readback(e.to_string())))?;
    rx.recv()
        .map_err(|e| RenderError::Gpu(GpuError::Readback(e.to_string())))?
        .map_err(|e| RenderError::Gpu(GpuError::Readback(e.to_string())))?;

    let mut pixmap = vello_cpu::Pixmap::new(width, height);
    {
        let data = slice
            .get_mapped_range()
            .map_err(|e| RenderError::Gpu(GpuError::Readback(e.to_string())))?;
        let pixels = pixmap.data_mut();
        for y in 0..usize::from(height) {
            let row = &data[y * padded as usize..y * padded as usize + unpadded as usize];
            for x in 0..usize::from(width) {
                let o = x * 4;
                pixels[y * usize::from(width) + x] = vello_cpu::color::PremulRgba8 {
                    r: row[o],
                    g: row[o + 1],
                    b: row[o + 2],
                    a: row[o + 3],
                };
            }
        }
    }
    buffer.unmap();
    Ok(pixmap)
}

fn to_gpu_paint(c: kine_core::Color) -> vello_gpu::color::AlphaColor<vello_gpu::color::Srgb> {
    vello_gpu::color::AlphaColor::<vello_gpu::color::Srgb>::from_rgba8(c.r, c.g, c.b, c.a)
}

fn fill_gradient_gpu(
    target: &mut VelloScene,
    rect: Rect,
    corner_radius: f32,
    grad: &Gradient,
    opacity: f32,
    affine: vello_gpu::kurbo::Affine,
) {
    use vello_gpu::kurbo::{Circle as GpuCircle, Rect as GpuRect, Shape};
    match grad {
        Gradient::Linear { angle_deg, stops } => {
            let bands = 32usize;
            let horizontal =
                angle_deg.rem_euclid(180.0) >= 45.0 && angle_deg.rem_euclid(180.0) < 135.0;
            for i in 0..bands {
                let t0 = (i as f32) / (bands as f32);
                let t1 = ((i + 1) as f32) / (bands as f32);
                let c = sample_gradient_stops(stops, 0.5 * (t0 + t1)).with_opacity(opacity);
                target.set_paint(to_gpu_paint(c));
                let sub = if horizontal {
                    GpuRect::new(
                        f64::from(rect.x + rect.width * t0),
                        f64::from(rect.y),
                        f64::from(rect.x + rect.width * t1),
                        f64::from(rect.y + rect.height),
                    )
                } else {
                    GpuRect::new(
                        f64::from(rect.x),
                        f64::from(rect.y + rect.height * t0),
                        f64::from(rect.x + rect.width),
                        f64::from(rect.y + rect.height * t1),
                    )
                };
                if affine == vello_gpu::kurbo::Affine::IDENTITY && corner_radius <= 0.0 {
                    target.fill_rect(&sub);
                } else {
                    target.fill_path(&(affine * sub.to_path(0.25)));
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
                target.set_paint(to_gpu_paint(c));
                let circle = GpuCircle::new((f64::from(cx), f64::from(cy)), f64::from(max_r * t));
                target.fill_path(&(affine * circle.to_path(0.25)));
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_node_gpu(
    target: &mut VelloScene,
    resources: &mut VelloResources,
    shaper: &mut TextShaper,
    scene: &Scene,
    node_id: kine_core::NodeId,
    parent_opacity: f32,
    parent_transform: Transform2D,
    time: f32,
    images: &mut ImageDrawCtx<'_>,
) -> Result<(), RenderError> {
    use glifo::Glyph;
    use vello_gpu::kurbo::{BezPath as GpuBezPath, Circle as GpuCircle, Shape};
    use vello_gpu::peniko::{Blob, FontData};

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
        NodeKind::Container | NodeKind::Shader { .. } => {}
        NodeKind::Image {
            src,
            fit,
            corner_radius,
        } => {
            if let Some((view, tex_w, tex_h)) = images.image_pixels(src) {
                draw_image_gpu(
                    target,
                    &view,
                    node_id.0,
                    tex_w,
                    tex_h,
                    rect,
                    *fit,
                    *corner_radius,
                    effective_opacity,
                    affine,
                    images.bindings,
                );
            }
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
            if let Some((view, tex_w, tex_h)) = images.video_pixels(
                node_id.0,
                src,
                time,
                *start,
                *trim_start,
                *trim_end,
                *playback_rate,
                *loop_video,
            )? {
                draw_image_gpu(
                    target,
                    &view,
                    node_id.0,
                    tex_w,
                    tex_h,
                    rect,
                    *fit,
                    *corner_radius,
                    effective_opacity,
                    affine,
                    images.bindings,
                );
            }
        }
        NodeKind::Rect {
            fill,
            corner_radius,
        } => {
            if let Some(sh) = &node.shadow {
                let steps = 5usize;
                for i in (1..=steps).rev() {
                    let expand = sh.blur.max(0.0) * (i as f32 / steps as f32);
                    let s_rect = Rect::new(
                        rect.x + sh.offset_x - expand,
                        rect.y + sh.offset_y - expand,
                        rect.width + expand * 2.0,
                        rect.height + expand * 2.0,
                    );
                    let alpha = (effective_opacity / (steps as f32 + 1.0)).clamp(0.0, 1.0);
                    target.set_paint(to_gpu_paint(sh.color.with_opacity(alpha)));
                    target.fill_path(&(affine * rect_to_path(s_rect, *corner_radius + expand)));
                }
            }
            let shape_path = rect_to_path(rect, *corner_radius);
            if let Some(grad) = &node.gradient {
                fill_gradient_gpu(target, rect, *corner_radius, grad, effective_opacity, affine);
            } else {
                target.set_paint(to_gpu_paint(fill.with_opacity(effective_opacity)));
                if affine == vello_gpu::kurbo::Affine::IDENTITY && *corner_radius <= 0.0 {
                    target.fill_rect(&vello_gpu::kurbo::Rect::new(
                        f64::from(rect.x),
                        f64::from(rect.y),
                        f64::from(rect.x + rect.width),
                        f64::from(rect.y + rect.height),
                    ));
                } else {
                    target.fill_path(&(affine * shape_path));
                }
            }
            if let Some(stroke) = &node.stroke {
                if let Some(ring) = stroke_rect_ring(rect, *corner_radius, stroke.width) {
                    target.set_paint(to_gpu_paint(stroke.color.with_opacity(effective_opacity)));
                    target.fill_path(&(affine * ring));
                }
            }
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
                    target.set_paint(to_gpu_paint(sh.color.with_opacity(alpha)));
                    let sc = GpuCircle::new(
                        (f64::from(cx + sh.offset_x), f64::from(cy + sh.offset_y)),
                        f64::from(r + expand),
                    );
                    target.fill_path(&(affine * sc.to_path(0.25)));
                }
            }
            let circle = GpuCircle::new((f64::from(cx), f64::from(cy)), f64::from(r));
            target.set_paint(to_gpu_paint(fill.with_opacity(effective_opacity)));
            target.fill_path(&(affine * circle.to_path(0.25)));
            if let Some(stroke) = &node.stroke {
                let sw = stroke.width.max(0.0);
                if sw > 0.0 {
                    let outer =
                        GpuCircle::new((f64::from(cx), f64::from(cy)), f64::from(r + sw * 0.5));
                    let inner_r = (r - sw * 0.5).max(0.0);
                    let mut ring = outer.to_path(0.25);
                    if inner_r > 0.0 {
                        ring.extend(reversed_circle_path(cx, cy, inner_r));
                    }
                    target.set_paint(to_gpu_paint(stroke.color.with_opacity(effective_opacity)));
                    target.fill_path(&(affine * ring));
                }
            }
        }
        NodeKind::Path { d, fill } => {
            let mut path = GpuBezPath::from_svg(d).map_err(|e| RenderError::InvalidPath {
                node: node_id.0,
                reason: e.to_string(),
            })?;
            let offset_affine =
                vello_gpu::kurbo::Affine::translate((f64::from(rect.x), f64::from(rect.y)))
                    * affine;
            path = offset_affine * path;
            if let Some(fill_color) = fill {
                target.set_paint(to_gpu_paint(fill_color.with_opacity(effective_opacity)));
                target.fill_path(&path);
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
            let shaped = shaper.shape_with_weight(
                content,
                font_family,
                scaled_size,
                *font_weight,
                Some(rect.width),
                to_shaper_align(*align),
            )?;
            let bytes = shaper
                .font_blob(font_family)
                .ok_or_else(|| RenderError::FontNotLoaded(font_family.clone()))?;
            let blob_data: Arc<dyn AsRef<[u8]> + Send + Sync> = bytes;
            let font = FontData::new(Blob::new(blob_data), 0);

            target.set_paint(to_gpu_paint(color.with_opacity(effective_opacity)));
            let run = target
                .glyph_run(resources, &font)
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
        }
    }

    let children = node.children.clone();
    for child in children {
        draw_node_gpu(
            target,
            resources,
            shaper,
            scene,
            child,
            effective_opacity,
            effective_transform,
            time,
            images,
        )?;
    }
    Ok(())
}

pub const NV12_Y_WGSL: &str = r#"
@group(0) @binding(0) var kine_input: texture_2d<f32>;
@group(0) @binding(1) var kine_sampler: sampler;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let premul = textureSample(kine_input, kine_sampler, uv);
    let a = max(premul.a, 0.00001);
    let rgb = clamp(premul.rgb / a, vec3<f32>(0.0), vec3<f32>(1.0));
    let y = 0.2126 * rgb.r + 0.7152 * rgb.g + 0.0722 * rgb.b;
    return vec4<f32>((16.0 + 219.0 * y) / 255.0, 0.0, 0.0, 1.0);
}
"#;

pub const NV12_UV_WGSL: &str = r#"
@group(0) @binding(0) var kine_input: texture_2d<f32>;
@group(0) @binding(1) var kine_sampler: sampler;

fn rgb_at(uv: vec2<f32>) -> vec3<f32> {
    let premul = textureSample(kine_input, kine_sampler, uv);
    let a = max(premul.a, 0.00001);
    return clamp(premul.rgb / a, vec3<f32>(0.0), vec3<f32>(1.0));
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let texel = vec2<f32>(textureDimensions(kine_input));
    let step = 1.0 / texel;
    let rgb = (rgb_at(uv)
        + rgb_at(uv + vec2<f32>(step.x, 0.0))
        + rgb_at(uv + vec2<f32>(0.0, step.y))
        + rgb_at(uv + step)) * 0.25;
    let y = 0.2126 * rgb.r + 0.7152 * rgb.g + 0.0722 * rgb.b;
    let cb = 128.0 + 224.0 * (rgb.b - y) / 1.8556;
    let cr = 128.0 + 224.0 * (rgb.r - y) / 1.5748;
    return vec4<f32>(cb / 255.0, cr / 255.0, 0.0, 1.0);
}
"#;

pub struct Nv12Converter {
    pipeline_y: wgpu::RenderPipeline,
    pipeline_uv: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    sampler: wgpu::Sampler,
}

impl Nv12Converter {
    pub fn new(device: &wgpu::Device) -> Result<Self, GpuError> {
        let vertex = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("kine.nv12.vertex"),
            source: wgpu::ShaderSource::Wgsl(VERTEX_WGSL.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("kine.nv12.layout"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("kine.nv12.pipeline_layout"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });

        let build = |label: &str, source: &str, format: wgpu::TextureFormat| {
            let fragment = device.create_shader_module(wgpu::ShaderModuleDescriptor {
                label: Some(label),
                source: wgpu::ShaderSource::Wgsl(source.into()),
            });
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                vertex: wgpu::VertexState {
                    module: &vertex,
                    entry_point: Some("vs_main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    buffers: &[],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &fragment,
                    entry_point: Some("main"),
                    compilation_options: wgpu::PipelineCompilationOptions::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format,
                        blend: Some(wgpu::BlendState::REPLACE),
                        write_mask: wgpu::ColorWrites::ALL,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                depth_stencil: None,
                multisample: wgpu::MultisampleState::default(),
                multiview_mask: None,
                cache: None,
            })
        };

        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("kine.nv12.sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            ..Default::default()
        });

        Ok(Self {
            pipeline_y: build("kine.nv12.y", NV12_Y_WGSL, wgpu::TextureFormat::R8Unorm),
            pipeline_uv: build("kine.nv12.uv", NV12_UV_WGSL, wgpu::TextureFormat::Rg8Unorm),
            layout,
            sampler,
        })
    }

    pub fn convert(
        &self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        source: &wgpu::TextureView,
        target: &wgpu::Texture,
    ) {
        let plane_y = target.create_view(&wgpu::TextureViewDescriptor {
            label: Some("kine.nv12.plane0"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            format: Some(wgpu::TextureFormat::R8Unorm),
            aspect: wgpu::TextureAspect::Plane0,
            ..Default::default()
        });
        let plane_uv = target.create_view(&wgpu::TextureViewDescriptor {
            label: Some("kine.nv12.plane1"),
            dimension: Some(wgpu::TextureViewDimension::D2),
            format: Some(wgpu::TextureFormat::Rg8Unorm),
            aspect: wgpu::TextureAspect::Plane1,
            ..Default::default()
        });

        let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("kine.nv12.bind_group"),
            layout: &self.layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: wgpu::BindingResource::TextureView(source),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::Sampler(&self.sampler),
                },
            ],
        });

        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("kine.nv12"),
        });
        for (pipeline, view, label) in [
            (&self.pipeline_y, &plane_y, "kine.nv12.y"),
            (&self.pipeline_uv, &plane_uv, "kine.nv12.uv"),
        ] {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some(label),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    resolve_target: None,
                    depth_slice: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
                multiview_mask: None,
            });
            pass.set_pipeline(pipeline);
            pass.set_bind_group(0, &bind_group, &[]);
            pass.draw(0..3, 0..1);
        }
        queue.submit(Some(encoder.finish()));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_engine_supplied_shaders_are_valid_wgsl() {
        kine_shader::validate(BLIT_WGSL).expect("blit fragment must be valid");

        let module = kine_shader::reflect_source(VERTEX_WGSL).expect("vertex must be valid");
        let entry = module
            .entry_points
            .iter()
            .find(|e| e.name == "vs_main")
            .expect("vs_main entry point");
        assert_eq!(entry.stage, kine_shader::ShaderStage::Vertex);
    }

    #[test]
    fn agent_and_engine_shaders_agree_on_the_vertex_interface() {
        assert!(VERTEX_WGSL.contains("@location(0) uv: vec2<f32>"));
        assert!(BLIT_WGSL.contains("@location(0) uv: vec2<f32>"));
        assert!(kine_shader::PRELUDE.contains("@binding(0) var<uniform> kine_frame"));
        assert!(kine_shader::PRELUDE.contains("@binding(1) var kine_input"));
        assert!(kine_shader::PRELUDE.contains("@binding(2) var kine_sampler"));
    }

    #[test]
    fn the_nv12_conversion_shaders_are_valid_wgsl() {
        // The NV12 modules declare their own bindings (group 0 texture/sampler),
        // so they are validated as-is: the agent prelude has a different layout.
        kine_shader::validate_full(NV12_Y_WGSL).expect("luma shader must be valid");
        kine_shader::validate_full(NV12_UV_WGSL).expect("chroma shader must be valid");
    }

    #[test]
    fn the_gpu_conversion_uses_the_same_bt709_constants_as_the_cpu_path() {
        for shader in [NV12_Y_WGSL, NV12_UV_WGSL] {
            assert!(shader.contains("0.2126"), "Kr missing");
            assert!(shader.contains("0.7152"), "Kg missing");
            assert!(shader.contains("0.0722"), "Kb missing");
        }
        assert!(
            NV12_Y_WGSL.contains("16.0 + 219.0"),
            "limited-range luma missing"
        );
        assert!(
            NV12_UV_WGSL.contains("128.0 + 224.0"),
            "limited-range chroma missing"
        );
    }

    #[test]
    fn scissor_clamps_to_the_frame() {
        let rect = kine_core::Rect::new(-10.0, -5.0, 100.0, 60.0);
        assert_eq!(scissor_for(rect, 64, 64), (0, 0, 64, 55));

        let inside = kine_core::Rect::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!(scissor_for(inside, 64, 64), (10, 20, 30, 40));

        let outside = kine_core::Rect::new(100.0, 100.0, 10.0, 10.0);
        let (_, _, w, h) = scissor_for(outside, 64, 64);
        assert!(w >= 1 && h >= 1);
    }

    #[test]
    fn blend_states_are_premultiplied_correct() {
        let normal = blend_state(ShaderBlend::Normal);
        assert_eq!(normal.color.src_factor, wgpu::BlendFactor::One);
        assert_eq!(normal.color.dst_factor, wgpu::BlendFactor::OneMinusSrcAlpha);

        let add = blend_state(ShaderBlend::Add);
        assert_eq!(add.color.src_factor, wgpu::BlendFactor::One);
        assert_eq!(add.color.dst_factor, wgpu::BlendFactor::One);

        let multiply = blend_state(ShaderBlend::Multiply);
        assert_eq!(multiply.color.src_factor, wgpu::BlendFactor::Dst);
        assert_eq!(multiply.color.dst_factor, wgpu::BlendFactor::Zero);
    }
}
