//! Hardware H.264 encode through the **Vulkan Video** encode queue.
//!
//! Compiled only with the `hwencode` feature: it needs a GPU whose driver
//! implements Vulkan Video encoding (NVIDIA proprietary, RADV on VCN2/VCN3, or
//! Intel ANV) plus the matching extensions. Without one, the probe returns
//! `None` — a diagnosed condition, never a silent fallback to a software
//! encoder.

use crate::{Codec, MuxError, Muxer};
use gpu_video::parameters::{
    EncoderParametersH264, EncoderPreset, RateControl, Rational, VideoDeviceDescriptor,
    VideoParameters,
};
use gpu_video::{
    EncodedOutputChunk, InputFrame, RawFrameRef, VideoAdapterExt, VideoDevice, VideoDeviceExt,
};
use std::num::NonZeroU32;
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::time::Duration;

/// One coded access unit, in the shape the muxer wants.
#[derive(Debug, Clone)]
pub struct EncodedChunk {
    pub data: Vec<u8>,
    pub keyframe: bool,
    pub pts: Option<u64>,
}

/// Settings for the hardware encoder.
#[derive(Debug, Clone, Copy)]
pub struct HwEncodeConfig {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// Average bitrate in bits per second.
    pub bitrate: u64,
    /// IDR cadence in frames.
    pub keyframe_interval: u32,
    /// Speed/quality trade-off handed to the driver.
    pub preset: EncoderPreset,
}

impl HwEncodeConfig {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            width,
            height,
            fps_num,
            fps_den,
            // ~0.1 bits per pixel per frame is a sane default for H.264.
            bitrate: u64::from(width) * u64::from(height) * u64::from(fps_num) / 10,
            keyframe_interval: 60,
            preset: EncoderPreset::HighQuality,
        }
    }

    fn validate(&self) -> Result<(), MuxError> {
        if self.width == 0 || self.height == 0 {
            return Err(MuxError::HwEncodeUnavailable(
                "width and height must be non-zero".to_string(),
            ));
        }
        if self.fps_num == 0 || self.fps_den == 0 {
            return Err(MuxError::HwEncodeUnavailable(
                "frame rate must be non-zero".to_string(),
            ));
        }
        Ok(())
    }
}

/// Process-wide wgpu instance, kept alive for the whole run so the Vulkan
/// loader never unloads an ICD mid-flight. GL/EGL is excluded: it is not a
/// supported target and its per-thread display setup races.
fn shared_instance() -> &'static wgpu::Instance {
    static INSTANCE: std::sync::OnceLock<wgpu::Instance> = std::sync::OnceLock::new();
    INSTANCE.get_or_init(|| {
        let mut desc = wgpu::InstanceDescriptor::new_without_display_handle();
        desc.backends = wgpu::Backends::PRIMARY;
        wgpu::Instance::new(desc)
    })
}

/// Probe this machine for hardware H.264 encoding, without being handed an
/// adapter.
pub fn h264_encoder_available() -> Result<bool, MuxError> {
    match request_adapter() {
        Ok(adapter) => Ok(probe_adapter(&adapter).is_some()),
        Err(_) => Ok(false),
    }
}

/// What the driver says it can encode, if anything.
pub fn probe_adapter(adapter: &wgpu::Adapter) -> Option<gpu_video::capabilities::VideoAdapterInfo> {
    adapter.video_adapter_info()
}

/// Ask for a `wgpu` device that can also encode video, and register it.
pub fn request_video_capable_device(
    adapter: &wgpu::Adapter,
    features: wgpu::Features,
    limits: wgpu::Limits,
) -> Result<(wgpu::Device, wgpu::Queue), MuxError> {
    if probe_adapter(adapter).is_none() {
        return Err(MuxError::HwEncodeUnavailable(
            "the chosen adapter reports no video encode capability".to_string(),
        ));
    }
    adapter
        .request_device_with_video_support(&VideoDeviceDescriptor {
            wgpu_features: features,
            wgpu_experimental_features: wgpu::ExperimentalFeatures::disabled(),
            wgpu_limits: limits,
        })
        .map_err(|e| MuxError::HwEncodeUnavailable(format!("video-capable device: {e}")))
}

/// Pick an adapter that can actually encode video. A plain `request_adapter`
/// returns the first adapter enumerated — on hybrid laptops the integrated GPU,
/// which usually has no Vulkan Video encode support.
pub fn request_adapter() -> Result<wgpu::Adapter, MuxError> {
    let instance = shared_instance();
    let preferred = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        force_fallback_adapter: false,
        ..Default::default()
    }));
    if let Ok(adapter) = preferred {
        if probe_adapter(&adapter).is_some() {
            return Ok(adapter);
        }
    }
    pollster::block_on(instance.enumerate_adapters(wgpu::Backends::PRIMARY))
        .into_iter()
        .find(|adapter| probe_adapter(adapter).is_some())
        .ok_or_else(|| {
            MuxError::HwEncodeUnavailable(
                "no adapter reports Vulkan Video encode support".to_string(),
            )
        })
}

/// A video-capable device with default limits — the entry point for the upload
/// path, which needs no renderer-specific features or limits.
pub fn video_capable_device() -> Result<(wgpu::Device, wgpu::Queue), MuxError> {
    let adapter = request_adapter()?;
    request_video_capable_device(
        &adapter,
        wgpu::Features::empty(),
        wgpu::Limits::downlevel_defaults(),
    )
}

/// Bind a video device to a `wgpu` device that was registered for video use.
fn video_device(device: &wgpu::Device) -> Result<VideoDevice, MuxError> {
    device.video().map_err(|e| {
        MuxError::HwEncodeUnavailable(format!(
            "this wgpu device is not registered for video use ({e}); create it with \
             request_video_capable_device"
        ))
    })
}

/// Encoder parameters shared by both encoder flavours.
fn encoder_parameters(
    config: &HwEncodeConfig,
    device: &VideoDevice,
) -> Result<EncoderParametersH264, MuxError> {
    let rate_control = RateControl::VariableBitrate {
        average_bitrate: config.bitrate,
        max_bitrate: config.bitrate + config.bitrate / 2,
        virtual_buffer_size: Duration::from_secs(1),
    };
    let mut output_parameters = device
        .encoder_output_parameters_h264_high_quality(rate_control)
        .map_err(|e| MuxError::HwEncodeUnavailable(format!("H.264 encode unsupported: {e}")))?;
    output_parameters.preset = config.preset;
    output_parameters.idr_period = NonZeroU32::new(config.keyframe_interval.max(1));

    Ok(EncoderParametersH264 {
        input_parameters: VideoParameters {
            width: NonZeroU32::new(config.width).ok_or_else(|| {
                MuxError::HwEncodeUnavailable("width is zero".to_string())
            })?,
            height: NonZeroU32::new(config.height).ok_or_else(|| {
                MuxError::HwEncodeUnavailable("height is zero".to_string())
            })?,
            target_framerate: Rational {
                numerator: config.fps_num,
                denominator: NonZeroU32::new(config.fps_den).ok_or_else(|| {
                    MuxError::HwEncodeUnavailable("frame-rate denominator is zero".to_string())
                })?,
            },
        },
        output_parameters,
        max_in_flight_submissions: None,
    })
}

/// Bookkeeping shared by both encoder flavours: frame count, IDR cadence, and
/// the channel the encoder's callback delivers chunks on.
struct ChunkSink {
    config: HwEncodeConfig,
    frames: u64,
    rx: Receiver<EncodedOutputChunk<Vec<u8>>>,
}

impl ChunkSink {
    fn new(
        config: HwEncodeConfig,
    ) -> (
        Self,
        impl FnMut(EncodedOutputChunk<Vec<u8>>) + Send + 'static,
    ) {
        let (tx, rx) = channel();
        let callback = move |chunk: EncodedOutputChunk<Vec<u8>>| {
            let _ = tx.send(chunk);
        };
        (
            Self {
                config,
                frames: 0,
                rx,
            },
            callback,
        )
    }

    /// Whether this frame should be forced to an IDR.
    fn force(&self, requested: bool) -> bool {
        requested || self.frames % u64::from(self.config.keyframe_interval.max(1)) == 0
    }

    /// Drain whatever the callback delivered since the last call.
    ///
    /// Every keyframe chunk is guaranteed to carry SPS/PPS in Annex-B form if
    /// `params` is available and not already present in the chunk payload,
    /// including chunks emitted during `flush()`.
    fn drain(&mut self, params: Option<Vec<u8>>, advance_frame_counter: bool) -> Vec<EncodedChunk> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(chunk) => {
                    let keyframe = chunk.is_keyframe;
                    let mut data = chunk.data;
                    if keyframe {
                        if let Some(p) = params.as_ref() {
                            if !contains_start_code(&data, p) {
                                let mut prefixed = p.clone();
                                prefixed.extend_from_slice(&data);
                                data = prefixed;
                            }
                        }
                    }
                    out.push(EncodedChunk {
                        data,
                        keyframe,
                        pts: chunk.pts,
                    });
                }
                Err(TryRecvError::Empty) | Err(TryRecvError::Disconnected) => break,
            }
        }
        if advance_frame_counter {
            self.frames += 1;
        }
        out
    }
}

/// H.264 encoder that takes frames uploaded from the host as NV12.
pub struct HwH264Encoder {
    video: VideoDevice,
    encoder: gpu_video::BytesEncoderH264,
    sink: ChunkSink,
}

impl HwH264Encoder {
    /// `device` must come from [`request_video_capable_device`].
    pub fn new(device: &wgpu::Device, config: HwEncodeConfig) -> Result<Self, MuxError> {
        config.validate()?;
        let video = video_device(device)?;
        let (sink, callback) = ChunkSink::new(config);
        let encoder = video
            .create_bytes_encoder_h264(encoder_parameters(&config, &video)?, callback)
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("encoder creation: {e}")))?;
        Ok(Self {
            video,
            encoder,
            sink,
        })
    }

    pub fn config(&self) -> &HwEncodeConfig {
        &self.sink.config
    }

    pub fn frames(&self) -> u64 {
        self.sink.frames
    }

    /// The video device this encoder belongs to.
    pub fn video(&self) -> &VideoDevice {
        &self.video
    }

    /// Encode one NV12 frame supplied from the host.
    pub fn encode_nv12(
        &mut self,
        nv12: &[u8],
        pts: Option<u64>,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedChunk>, MuxError> {
        let expected =
            (self.sink.config.width as usize) * (self.sink.config.height as usize) * 3 / 2;
        if nv12.len() != expected {
            return Err(MuxError::HwEncodeUnavailable(format!(
                "NV12 frame has {} bytes, expected {expected} for {}x{}",
                nv12.len(),
                self.sink.config.width,
                self.sink.config.height
            )));
        }
        let force = self.sink.force(force_keyframe);
        let frame = InputFrame {
            data: RawFrameRef {
                frame: nv12,
                width: self.sink.config.width,
                height: self.sink.config.height,
            },
            pts,
        };
        self.encoder
            .encode(&frame, force)
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("encode: {e}")))?;
        let params = self.stream_params().ok();
        Ok(self.sink.drain(params, true))
    }

    /// Flush the encoder; the remaining chunks are delivered here with SPS/PPS
    /// attached to any flushed keyframe.
    pub fn flush(&mut self) -> Result<Vec<EncodedChunk>, MuxError> {
        self.encoder
            .flush()
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("flush: {e}")))?;
        let params = self.stream_params().ok();
        Ok(self.sink.drain(params, false))
    }

    /// SPS and PPS as Annex-B, for out-of-band use.
    pub fn stream_params(&self) -> Result<Vec<u8>, MuxError> {
        let sps = self
            .encoder
            .sps()
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("SPS: {e}")))?;
        let pps = self
            .encoder
            .pps()
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("PPS: {e}")))?;
        let mut out = sps;
        out.extend_from_slice(&pps);
        Ok(out)
    }
}

/// H.264 encoder that consumes textures living in video memory — the zero-copy
/// path.
pub struct HwH264TextureEncoder {
    video: VideoDevice,
    encoder: gpu_video::WgpuTexturesEncoderH264,
    sink: ChunkSink,
    pending: Option<gpu_video::EncodeTexture>,
}

impl HwH264TextureEncoder {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        config: HwEncodeConfig,
    ) -> Result<Self, MuxError> {
        config.validate()?;
        let video = video_device(device)?;
        let (sink, callback) = ChunkSink::new(config);
        let encoder = video
            .create_wgpu_textures_encoder_h264(
                queue,
                encoder_parameters(&config, &video)?,
                callback,
            )
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("encoder creation: {e}")))?;
        Ok(Self {
            video,
            encoder,
            sink,
            pending: None,
        })
    }

    pub fn config(&self) -> &HwEncodeConfig {
        &self.sink.config
    }

    pub fn frames(&self) -> u64 {
        self.sink.frames
    }

    pub fn video(&self) -> &VideoDevice {
        &self.video
    }

    /// Obtain this frame's input texture — NV12, in video memory.
    pub fn begin_frame(&mut self) -> Result<&wgpu::Texture, MuxError> {
        let input = self
            .encoder
            .input_texture()
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("input texture: {e}")))?;
        self.pending = Some(input);
        self.pending
            .as_ref()
            .and_then(|p| p.texture().nv12_texture())
            .ok_or_else(|| {
                MuxError::HwEncodeUnavailable(
                    "the encoder's input texture is not a single NV12 texture".to_string(),
                )
            })
    }

    /// Submit the frame obtained from [`Self::begin_frame`].
    pub fn submit_frame(
        &mut self,
        pts: Option<u64>,
        force_keyframe: bool,
    ) -> Result<Vec<EncodedChunk>, MuxError> {
        let Some(input) = self.pending.take() else {
            return Err(MuxError::HwEncodeUnavailable(
                "submit_frame called without begin_frame".to_string(),
            ));
        };
        let force = self.sink.force(force_keyframe);
        self.encoder
            .encode(InputFrame { data: input, pts }, force)
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("encode: {e}")))?;
        let params = self.stream_params().ok();
        Ok(self.sink.drain(params, true))
    }

    /// Flush the encoder; the remaining chunks are delivered here.
    pub fn flush(&mut self) -> Result<Vec<EncodedChunk>, MuxError> {
        self.encoder
            .flush()
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("flush: {e}")))?;
        let params = self.stream_params().ok();
        Ok(self.sink.drain(params, false))
    }

    /// SPS and PPS as Annex-B, for out-of-band use.
    pub fn stream_params(&self) -> Result<Vec<u8>, MuxError> {
        let sps = self
            .encoder
            .sps()
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("SPS: {e}")))?;
        let pps = self
            .encoder
            .pps()
            .map_err(|e| MuxError::HwEncodeUnavailable(format!("PPS: {e}")))?;
        let mut out = sps;
        out.extend_from_slice(&pps);
        Ok(out)
    }
}

/// Mux a sequence of already-encoded chunks into an MP4.
pub fn mux_chunks<W: std::io::Write>(
    chunks: &[EncodedChunk],
    sink: W,
    config: &HwEncodeConfig,
) -> Result<W, MuxError> {
    let mut muxer = Muxer::new(
        sink,
        Codec::H264,
        config.width,
        config.height,
        config.fps_num,
        config.fps_den,
    );
    for (index, chunk) in chunks.iter().enumerate() {
        muxer.push_h264_annexb(index as u64, &chunk.data, chunk.keyframe)?;
    }
    muxer.finalize()
}

/// Whether `data` already contains `params` (so the prefix would duplicate it).
fn contains_start_code(data: &[u8], params: &[u8]) -> bool {
    if params.is_empty() || params.len() > data.len() {
        return false;
    }
    data.windows(params.len()).any(|w| w == params)
}

#[cfg(test)]
mod tests {
    use super::*;


    #[test]
    fn default_bitrate_scales_with_pixels_and_rate() {
        let config = HwEncodeConfig::new(1920, 1080, 30, 1);
        assert_eq!(config.bitrate, 1920 * 1080 * 30 / 10);
        assert_eq!(config.keyframe_interval, 60);
        assert!(matches!(config.preset, EncoderPreset::HighQuality));
    }

    #[test]
    fn zero_sized_configuration_is_rejected_before_touching_vulkan() {
        let err = HwEncodeConfig::new(0, 240, 30, 1).validate();
        assert!(matches!(err, Err(MuxError::HwEncodeUnavailable(_))));
    }

    #[test]
    fn start_code_detection_is_conservative() {
        let params = vec![0, 0, 0, 1, 0x67];
        assert!(contains_start_code(&[0, 0, 0, 1, 0x67, 0xAA], &params));
        assert!(!contains_start_code(&[0, 0, 0, 1, 0x41], &params));
        assert!(!contains_start_code(&[1, 2], &params));
        assert!(!contains_start_code(&params, &[]));
    }

    #[test]
    fn requesting_a_video_device_without_support_is_a_typed_error() {
        let instance = shared_instance();
        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            force_fallback_adapter: false,
            ..Default::default()
        }));
        let Ok(adapter) = adapter else {
            return;
        };
        match request_video_capable_device(
            &adapter,
            wgpu::Features::empty(),
            wgpu::Limits::downlevel_defaults(),
        ) {
            Ok((device, _queue)) => {
                let config = HwEncodeConfig::new(320, 240, 30, 1);
                let _ = HwH264Encoder::new(&device, config);
            }
            Err(e) => assert!(
                matches!(e, MuxError::HwEncodeUnavailable(_)),
                "unexpected error shape: {e:?}"
            ),
        }
    }
}
