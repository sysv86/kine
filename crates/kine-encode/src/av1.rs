//! AV1 encoding via `rav1e` (pure Rust — no C codec is linked anywhere).

use crate::{color::rgba_to_i420, AccessUnit, EncodeError, EncodeOutput};
use rav1e::prelude::*;

/// Settings for the software encoder.
#[derive(Debug, Clone, Copy)]
pub struct EncoderSettings {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    /// rav1e quantizer: lower is better quality, larger files. 100 is a
    /// reasonable draft default; the CLI exposes it as `--quality`.
    pub quantizer: usize,
    /// rav1e speed preset 0–10 (10 is fastest, lowest quality).
    pub speed: u8,
    /// Keyframe cadence in frames.
    pub keyframe_interval: u32,
    /// Worker threads; `0` uses the number of available cores.
    pub threads: usize,
    /// Tile columns. `0` derives a value from the frame width so that a single
    /// frame can be encoded in parallel — rav1e's default of one tile leaves
    /// every core but one idle.
    pub tile_cols: usize,
}

impl EncoderSettings {
    pub fn new(width: u32, height: u32, fps_num: u32, fps_den: u32) -> Self {
        Self {
            width,
            height,
            fps_num,
            fps_den,
            quantizer: 100,
            speed: 9,
            keyframe_interval: 60,
            threads: 0,
            tile_cols: 0,
        }
    }

    /// Threads to actually use.
    pub fn resolved_threads(&self) -> usize {
        if self.threads != 0 {
            return self.threads;
        }
        std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
    }

    /// Tile columns to actually use.
    ///
    /// Roughly one tile per 320 columns, capped by the thread count: more tiles
    /// than workers only adds overhead, and rav1e refuses degenerate splits.
    pub fn resolved_tile_cols(&self) -> usize {
        let cols = if self.tile_cols != 0 {
            self.tile_cols.clamp(1, 64)
        } else {
            let by_width = (self.width as usize) / 320;
            by_width.clamp(1, 64).min(self.resolved_threads()).max(1)
        };
        // rav1e only accepts power-of-two tile column counts. Round the
        // heuristic (or an explicit request) down to the nearest valid one.
        1 << (usize::BITS - 1 - cols.leading_zeros())
    }
}

/// A streaming AV1 encoder.
pub struct Av1Encoder {
    ctx: Context<u8>,
    settings: EncoderSettings,
    frames_sent: u64,
    flushing: bool,
    drained: bool,
}

impl Av1Encoder {
    pub fn new(settings: EncoderSettings) -> Result<Self, EncodeError> {
        if settings.width == 0 || settings.height == 0 {
            return Err(EncodeError::InvalidConfig(
                "width and height must be non-zero".to_string(),
            ));
        }
        if settings.fps_num == 0 || settings.fps_den == 0 {
            return Err(EncodeError::InvalidConfig("fps must be non-zero".to_string()));
        }

        let mut enc = EncoderConfig::with_speed_preset(settings.speed);
        enc.width = settings.width as usize;
        enc.height = settings.height as usize;
        enc.chroma_sampling = ChromaSampling::Cs420;
        enc.chroma_sample_position = ChromaSamplePosition::Unknown;
        enc.bit_depth = 8;
        enc.pixel_range = PixelRange::Limited;
        // 8-bit SDR in BT.709 — stated explicitly rather than left to a default.
        enc.color_description = Some(ColorDescription {
            color_primaries: ColorPrimaries::BT709,
            transfer_characteristics: TransferCharacteristics::BT709,
            matrix_coefficients: MatrixCoefficients::BT709,
        });
        enc.time_base = Rational::new(u64::from(settings.fps_den), u64::from(settings.fps_num));
        enc.low_latency = false;
        enc.quantizer = settings.quantizer;
        enc.min_key_frame_interval = 0;
        enc.max_key_frame_interval = u64::from(settings.keyframe_interval.max(1));
        enc.enable_timing_info = true;
        // Without tiles rav1e encodes a frame on a single core.
        enc.tile_cols = settings.resolved_tile_cols();
        enc.tile_rows = 1;

        let config = Config::new()
            .with_encoder_config(enc)
            .with_threads(settings.resolved_threads());

        let ctx = config
            .new_context()
            .map_err(|e| EncodeError::InvalidConfig(e.to_string()))?;

        Ok(Self {
            ctx,
            settings,
            frames_sent: 0,
            flushing: false,
            drained: false,
        })
    }

    pub fn settings(&self) -> &EncoderSettings {
        &self.settings
    }

    pub fn frames_sent(&self) -> u64 {
        self.frames_sent
    }

    /// Encode one RGBA frame (premultiplied, `width * height * 4` bytes).
    pub fn encode_rgba(&mut self, rgba: &[u8]) -> Result<EncodeOutput, EncodeError> {
        let expected = (self.settings.width as usize) * (self.settings.height as usize) * 4;
        if rgba.len() != expected {
            return Err(EncodeError::FrameSize {
                actual: rgba.len(),
                expected,
                width: self.settings.width,
                height: self.settings.height,
            });
        }
        if self.flushing {
            return Err(EncodeError::Rejected(
                "encoder is already flushing; no more frames are accepted".to_string(),
            ));
        }

        let i420 = rgba_to_i420(rgba, self.settings.width, self.settings.height);
        let mut frame = self.ctx.new_frame();
        write_plane(&mut frame.planes[0], &i420.y);
        write_plane(&mut frame.planes[1], &i420.u);
        write_plane(&mut frame.planes[2], &i420.v);

        match self.ctx.send_frame(frame) {
            Ok(()) => {}
            // The encoder needs its output drained before accepting more.
            Err(EncoderStatus::EnoughData) => {
                let pending = self.drain()?;
                self.ctx
                    .send_frame(self.frame_from(&i420))
                    .map_err(|e| EncodeError::Rejected(format!("{e:?}")))?;
                self.frames_sent += 1;
                return Ok(pending);
            }
            Err(e) => return Err(EncodeError::Rejected(format!("{e:?}"))),
        }
        self.frames_sent += 1;
        self.drain()
    }

    /// Finish the stream and return the remaining access units.
    pub fn finish(&mut self) -> Result<EncodeOutput, EncodeError> {
        if !self.flushing {
            self.ctx.flush();
            self.flushing = true;
        }
        let out = self.drain_until_limit()?;
        self.drained = true;
        Ok(out)
    }

    fn frame_from(&self, i420: &crate::I420Frame) -> Frame<u8> {
        let mut frame = self.ctx.new_frame();
        write_plane(&mut frame.planes[0], &i420.y);
        write_plane(&mut frame.planes[1], &i420.u);
        write_plane(&mut frame.planes[2], &i420.v);
        frame
    }

    /// Pull every packet the encoder currently has.
    fn drain(&mut self) -> Result<EncodeOutput, EncodeError> {
        let mut out = EncodeOutput::default();
        loop {
            match self.ctx.receive_packet() {
                Ok(packet) => {
                    let keyframe = packet.frame_type == FrameType::KEY;
                    if out.units.is_empty() && keyframe {
                        out.starts_keyframe = true;
                    }
                    out.units.push(AccessUnit {
                        data: packet.data,
                        keyframe,
                    });
                }
                // `Encoded` means "a frame is ready but not emitted yet" — the
                // caller must come back for it. Breaking here silently drops
                // frames from the stream.
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::NeedMoreData) => break,
                Err(EncoderStatus::LimitReached) => break,
                Err(e) => return Err(EncodeError::Rejected(format!("{e:?}"))),
            }
        }
        Ok(out)
    }

    /// Drain after `flush`, where `LimitReached` means the stream is complete.
    fn drain_until_limit(&mut self) -> Result<EncodeOutput, EncodeError> {
        let mut out = EncodeOutput::default();
        loop {
            match self.ctx.receive_packet() {
                Ok(packet) => {
                    let keyframe = packet.frame_type == FrameType::KEY;
                    if out.units.is_empty() && keyframe {
                        out.starts_keyframe = true;
                    }
                    out.units.push(AccessUnit {
                        data: packet.data,
                        keyframe,
                    });
                }
                // As above: `Encoded` is "come back for the next packet", not
                // "stop" — breaking here is how frames go missing.
                Err(EncoderStatus::Encoded) => continue,
                Err(EncoderStatus::LimitReached) | Err(EncoderStatus::NeedMoreData) => break,
                Err(e) => return Err(EncodeError::Rejected(format!("{e:?}"))),
            }
        }
        Ok(out)
    }

    /// Whether the stream has been flushed to completion.
    pub fn is_drained(&self) -> bool {
        self.drained
    }
}

/// Copy a plane row by row.
///
/// rav1e's planes are not just stride-padded: the visible area starts at
/// `(xorigin, yorigin)` inside the allocation, and the encoder reads from there.
/// Writing from row 0 leaves the encoder reading untouched memory — which decodes
/// as a partially grey frame rather than failing loudly.
fn write_plane(plane: &mut Plane<u8>, src: &[u8]) {
    let stride = plane.cfg.stride;
    let width = plane.cfg.width;
    let height = plane.cfg.height;
    let base = plane.cfg.yorigin * stride + plane.cfg.xorigin;
    for row in 0..height {
        let dst = base + row * stride;
        let from = row * width;
        if from + width <= src.len() && dst + width <= plane.data.len() {
            plane.data[dst..dst + width].copy_from_slice(&src[from..from + width]);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(width: u32, height: u32, shade: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((width * height * 4) as usize);
        for i in 0..width * height {
            // A moving vertical bar so successive frames actually differ.
            let bright = if (i % width) < (shade as u32 % width) { 255 } else { 40 };
            v.extend_from_slice(&[bright, bright / 2, 255 - bright, 255]);
        }
        v
    }

    #[test]
    fn encodes_frames_into_av1_access_units() {
        let mut enc = Av1Encoder::new(EncoderSettings::new(64, 48, 30, 1)).expect("encoder");
        let mut units = Vec::new();
        let mut first_output_is_keyframe = None;
        for i in 0..6u32 {
            let out = enc
                .encode_rgba(&frame(64, 48, (i * 4) as u8))
                .expect("encode");
            // The encoder buffers, so the first frame may yield nothing; the
            // keyframe flag belongs to the first output that has units.
            if first_output_is_keyframe.is_none() && !out.is_empty() {
                first_output_is_keyframe = Some(out.starts_keyframe);
            }
            units.extend(out.units);
        }
        let tail = enc.finish().expect("finish");
        // With lookahead enabled a short stream is emitted entirely at flush,
        // so the keyframe flag may only be observable there.
        if first_output_is_keyframe.is_none() && !tail.is_empty() {
            first_output_is_keyframe = Some(tail.starts_keyframe);
        }
        units.extend(tail.units);

        assert!(!units.is_empty(), "encoder produced no access units");
        assert_eq!(
            first_output_is_keyframe,
            Some(true),
            "the first access unit must be a keyframe"
        );

        // Every access unit starts with an OBU header; the first one must carry
        // the sequence header, which the MP4 muxer requires to build `av01`.
        for unit in &units {
            assert!(!unit.data.is_empty(), "empty access unit");
            let obu_type = unit.data[0] >> 3;
            assert!(obu_type <= 15, "not an OBU header byte: {:#04x}", unit.data[0]);
        }
        let first = &units[0].data;
        // OBU_SEQUENCE_HEADER == 1; the first unit carries it, which is what the
        // MP4 muxer needs to build the `av01` sample entry.
        let has_sequence_header =
            (0..first.len().saturating_sub(1)).any(|i| (first[i] >> 3) == 1);
        assert!(
            has_sequence_header,
            "first access unit must contain a sequence header OBU"
        );
    }

    #[test]
    fn rejects_a_frame_of_the_wrong_size() {
        let mut enc = Av1Encoder::new(EncoderSettings::new(16, 16, 30, 1)).unwrap();
        let err = match enc.encode_rgba(&[0u8; 8]) {
            Err(e) => e,
            Ok(_) => panic!("a wrongly sized frame must be rejected"),
        };
        assert!(matches!(err, EncodeError::FrameSize { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_a_zero_sized_configuration() {
        let err = match Av1Encoder::new(EncoderSettings::new(0, 16, 30, 1)) {
            Err(e) => e,
            Ok(_) => panic!("a zero-sized config must be rejected"),
        };
        assert!(matches!(err, EncodeError::InvalidConfig(_)), "got {err:?}");
    }

    #[test]
    fn every_frame_becomes_exactly_one_access_unit() {
        // Regression: `EncoderStatus::Encoded` was treated as "stop draining",
        // which silently dropped packets — a 60-frame clip came out with 45
        // samples and would play wrong.
        const FRAMES: u32 = 20;
        let mut enc = Av1Encoder::new(EncoderSettings::new(64, 64, 24, 1)).unwrap();
        let mut units = 0usize;
        for i in 0..FRAMES {
            units += enc
                .encode_rgba(&frame(64, 64, (i * 3) as u8))
                .unwrap()
                .units
                .len();
        }
        units += enc.finish().unwrap().units.len();
        assert_eq!(
            units, FRAMES as usize,
            "one access unit per frame: got {units} for {FRAMES} frames"
        );
    }

    #[test]
    fn a_frame_may_produce_no_output_but_the_stream_still_completes() {
        let mut enc = Av1Encoder::new(EncoderSettings::new(32, 32, 24, 1)).unwrap();
        let mut total = 0usize;
        for i in 0..12u32 {
            total += enc
                .encode_rgba(&frame(32, 32, (i * 2) as u8))
                .unwrap()
                .units
                .len();
        }
        total += enc.finish().unwrap().units.len();
        assert!(total >= 1, "a 12-frame stream must produce output");
        assert!(enc.is_drained());
    }
}
