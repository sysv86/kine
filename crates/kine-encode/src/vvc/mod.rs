//! H.266 / VVC (ITU-T H.266 | ISO/IEC 23090-3) intra-only software encoder.
//!
//! This is a real, conformant VVC bitstream writer for a deliberately small
//! subset of the standard:
//!
//! * Main 10 profile signalling, 8-bit 4:2:0, BT.709 limited range;
//! * one IDR_N_LP picture per access unit, one slice, one tile;
//! * `CTU == min CU == 32x32`, quadtree partitioning disabled (no
//!   `split_cu_flag` appears at all);
//! * every coding unit is intra PLANAR (coded through the MPM path) with
//!   `intra_chroma_pred_mode = DM`, so no MPM neighbour derivation is needed;
//! * every residual block uses `transform_skip_flag = 1` and the normative
//!   `residual_ts_coding()` (Go-Rice with the transform-skip level remapping);
//! * no SAO / ALF / LMCS / deblocking / MTS / LFNST / MIP / ISP / BDPCM /
//!   palette / IBC / ACT / dependent quantisation / sign data hiding.
//!
//! The output is Annex-B with spec 2-byte NAL headers and emulation
//! prevention, and is decodable by VTM, VVdeC and FFmpeg's VVC decoder.
//!
//! The bitstream syntax is documented field-by-field in
//! `docs/vvc-bitstream.md`.

pub mod bits;
pub mod cabac;
pub mod hls;
pub mod predict;
pub mod slice;
pub mod transform;

use crate::{
    color::rgba_to_i420, AccessUnit, EncodeError, EncodeOutput, EncoderSettings, I420Frame,
};

pub use bits::{ebsp_to_rbsp, rbsp_to_ebsp};
pub use hls::{
    parse_sps_info as parse_vvc_sps_info, select_level_idc as select_vvc_level_idc, VvcSpsInfo,
};

use hls::{
    build_aud_rbsp, build_ph_rbsp, build_pps_rbsp, build_slice_header_bits, build_sps_rbsp,
    build_vps_rbsp, push_nal, VvcStreamConfig, NAL_AUD, NAL_IDR_N_LP, NAL_PH, NAL_PPS, NAL_SPS,
    NAL_VPS,
};

/// CTU size used by this encoder: 32x32 luma, 16x16 chroma per CU.
const CTU_LOG2: u8 = 5;

/// ITU-T H.266 / VVC Table 5 — NAL unit type codes (`nal_unit_type`, 5 bits).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VvcNalType {
    Trail,
    Stsa,
    Radl,
    Rasl,
    IdrWRadl,
    IdrNLp,
    Cra,
    Opi,
    Dci,
    Vps,
    Sps,
    Pps,
    PrefixAps,
    Ph,
    Aud,
    Eos,
    PrefixSei,
    Unknown(u8),
}

impl VvcNalType {
    pub fn from_u8(code: u8) -> Self {
        match code & 0x1F {
            0 => Self::Trail,
            1 => Self::Stsa,
            2 => Self::Radl,
            3 => Self::Rasl,
            7 => Self::IdrWRadl,
            8 => Self::IdrNLp,
            9 => Self::Cra,
            12 => Self::Opi,
            13 => Self::Dci,
            14 => Self::Vps,
            15 => Self::Sps,
            16 => Self::Pps,
            17 => Self::PrefixAps,
            19 => Self::Ph,
            20 => Self::Aud,
            21 => Self::Eos,
            23 => Self::PrefixSei,
            other => Self::Unknown(other),
        }
    }

    pub fn code(self) -> u8 {
        match self {
            Self::Trail => 0,
            Self::Stsa => 1,
            Self::Radl => 2,
            Self::Rasl => 3,
            Self::IdrWRadl => 7,
            Self::IdrNLp => 8,
            Self::Cra => 9,
            Self::Opi => 12,
            Self::Dci => 13,
            Self::Vps => 14,
            Self::Sps => 15,
            Self::Pps => 16,
            Self::PrefixAps => 17,
            Self::Ph => 19,
            Self::Aud => 20,
            Self::Eos => 21,
            Self::PrefixSei => 23,
            Self::Unknown(c) => c & 0x1F,
        }
    }

    pub fn is_irap(self) -> bool {
        matches!(self, Self::IdrWRadl | Self::IdrNLp | Self::Cra)
    }
}

/// One parsed VVC NAL unit from an Annex-B byte stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VvcNalUnit {
    pub nal_type: VvcNalType,
    pub layer_id: u8,
    pub temporal_id: u8,
    /// Raw NAL bytes starting with the 2-byte VVC NAL unit header.
    pub raw: Vec<u8>,
    /// Unescaped RBSP payload after the 2-byte VVC NAL unit header.
    pub rbsp: Vec<u8>,
}

/// Parse an ITU-T H.266 / VVC Annex-B byte stream into individual NAL units.
pub fn parse_vvc_annexb_nals(data: &[u8]) -> Vec<VvcNalUnit> {
    let mut slices: Vec<&[u8]> = Vec::new();
    let mut i = 0usize;
    let mut nal_start: Option<usize> = None;
    while i < data.len() {
        let sc_len = if i + 4 <= data.len() && data[i..i + 4] == [0, 0, 0, 1] {
            4
        } else if i + 3 <= data.len() && data[i..i + 3] == [0, 0, 1] {
            3
        } else {
            0
        };
        if sc_len > 0 {
            if let Some(start) = nal_start {
                if start < i {
                    slices.push(&data[start..i]);
                }
            }
            i += sc_len;
            nal_start = Some(i);
        } else {
            i += 1;
        }
    }
    if let Some(start) = nal_start {
        if start < data.len() {
            slices.push(&data[start..]);
        }
    }

    let mut out = Vec::with_capacity(slices.len());
    for raw in slices {
        if raw.len() < 2 {
            continue;
        }
        let layer_id = raw[0] & 0x3F;
        let nal_type = VvcNalType::from_u8((raw[1] >> 3) & 0x1F);
        let temporal_id = (raw[1] & 0x07).saturating_sub(1);
        let rbsp = ebsp_to_rbsp(&raw[2..]);
        out.push(VvcNalUnit {
            nal_type,
            layer_id,
            temporal_id,
            raw: raw.to_vec(),
            rbsp,
        });
    }
    out
}

/// Map the generic `quantizer` setting (`0..=255`) onto an H.266 QP (`4..=63`).
///
/// QP 4 is the lossless transform-skip point for 8-bit (`dequant(level) ==
/// level`), so a quantizer of 0 produces a bit-exact stream.
fn quantizer_to_qp(quantizer: usize) -> u8 {
    let quantizer = quantizer.min(255) as u32;
    let qp = 4 + (quantizer * 59) / 255;
    qp.clamp(4, 63) as u8
}

/// A streaming H.266 / VVC intra-only software encoder.
///
/// Every access unit is an `IDR_N_LP` keyframe: H.266 inter prediction is not
/// implemented, so `keyframe_interval` is accepted for API compatibility but
/// ignored.
pub struct VvcEncoder {
    settings: EncoderSettings,
    frames_sent: u64,
    flushing: bool,
    drained: bool,
    cfg: VvcStreamConfig,
    visible_width: u32,
    visible_height: u32,
    aud_rbsp: Vec<u8>,
    vps_rbsp: Vec<u8>,
    sps_rbsp: Vec<u8>,
    pps_rbsp: Vec<u8>,
}

impl VvcEncoder {
    pub fn new(settings: EncoderSettings) -> Result<Self, EncodeError> {
        if settings.width == 0 || settings.height == 0 {
            return Err(EncodeError::InvalidConfig(
                "width and height must be non-zero".to_string(),
            ));
        }
        if settings.width < 2 || settings.height < 2 {
            return Err(EncodeError::InvalidConfig(
                "H.266 4:2:0 needs at least 2x2 luma samples".to_string(),
            ));
        }
        if settings.fps_num == 0 || settings.fps_den == 0 {
            return Err(EncodeError::InvalidConfig(
                "fps must be non-zero".to_string(),
            ));
        }

        // The conformance window can only crop in units of two luma samples
        // for 4:2:0, so odd source dimensions are rounded down by one pixel.
        let visible_width = settings.width & !1;
        let visible_height = settings.height & !1;
        let ctu_size = 1u32 << CTU_LOG2;
        let coded_width = visible_width.div_ceil(ctu_size) * ctu_size;
        let coded_height = visible_height.div_ceil(ctu_size) * ctu_size;
        let qp = quantizer_to_qp(settings.quantizer);
        let level_idc = hls::select_level_idc(coded_width, coded_height);
        let cfg = VvcStreamConfig {
            width: coded_width,
            height: coded_height,
            crop_right: coded_width - visible_width,
            crop_bottom: coded_height - visible_height,
            qp,
            level_idc,
            ctu_log2: CTU_LOG2,
            min_cb_log2: CTU_LOG2,
            ts_log2: CTU_LOG2,
        };
        Ok(Self {
            settings,
            frames_sent: 0,
            flushing: false,
            drained: false,
            visible_width,
            visible_height,
            cfg: cfg.clone(),
            aud_rbsp: build_aud_rbsp(),
            vps_rbsp: build_vps_rbsp(&cfg),
            sps_rbsp: build_sps_rbsp(&cfg),
            pps_rbsp: build_pps_rbsp(&cfg),
        })
    }

    pub fn settings(&self) -> &EncoderSettings {
        &self.settings
    }

    pub fn frames_sent(&self) -> u64 {
        self.frames_sent
    }

    pub fn is_drained(&self) -> bool {
        self.drained
    }

    /// Coded (padded) picture size.
    pub fn coded_size(&self) -> (u32, u32) {
        (self.cfg.width, self.cfg.height)
    }

    /// Visible (conformance-window-cropped) picture size.
    pub fn visible_size(&self) -> (u32, u32) {
        (self.visible_width, self.visible_height)
    }

    /// Encode one premultiplied RGBA8 frame into an Annex-B access unit.
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

        let source = rgba_to_i420(rgba, self.settings.width, self.settings.height);
        let padded = pad_i420(&source, self.cfg.width, self.cfg.height);
        let cabac = slice::encode_picture(&padded, self.cfg.qp, self.cfg.ctu_log2);

        let mut au = Vec::with_capacity(cabac.len() + 256);
        push_nal(&mut au, NAL_AUD, 0, 1, &self.aud_rbsp);
        // In-band parameter sets on every IDR so each access unit is
        // self-contained for Annex-B consumers; the ISO-BMFF muxer also
        // carries them out-of-band in `vvcC`.
        push_nal(&mut au, NAL_VPS, 0, 1, &self.vps_rbsp);
        push_nal(&mut au, NAL_SPS, 0, 1, &self.sps_rbsp);
        push_nal(&mut au, NAL_PPS, 0, 1, &self.pps_rbsp);
        // Each picture is an IDR, so its POC LSB restarts at 0.
        push_nal(&mut au, NAL_PH, 0, 1, &build_ph_rbsp(0));

        let mut slice_rbsp = build_slice_header_bits(0);
        slice_rbsp.extend_from_slice(&cabac);
        push_nal(&mut au, NAL_IDR_N_LP, 0, 1, &slice_rbsp);

        self.frames_sent += 1;
        Ok(EncodeOutput {
            units: vec![AccessUnit {
                data: au,
                keyframe: true,
            }],
            starts_keyframe: true,
        })
    }

    /// Finish the stream.  All frames are emitted immediately, so no delayed
    /// pictures are produced.
    pub fn finish(&mut self) -> Result<EncodeOutput, EncodeError> {
        self.flushing = true;
        self.drained = true;
        Ok(EncodeOutput::default())
    }
}

/// Replicate the right/bottom edges until the picture reaches the coded size.
fn pad_i420(frame: &I420Frame, width: u32, height: u32) -> I420Frame {
    let pad_plane = |src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32| -> Vec<u8> {
        let mut out = vec![0u8; (dw * dh) as usize];
        for y in 0..dh {
            let sy = y.min(sh - 1);
            for x in 0..dw {
                let sx = x.min(sw - 1);
                out[(y * dw + x) as usize] = src[(sy * sw + sx) as usize];
            }
        }
        out
    };
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let scw = frame.width.div_ceil(2);
    let sch = frame.height.div_ceil(2);
    I420Frame {
        width,
        height,
        y: pad_plane(&frame.y, frame.width, frame.height, width, height),
        u: pad_plane(&frame.u, scw, sch, cw, ch),
        v: pad_plane(&frame.v, scw, sch, cw, ch),
    }
}

fn parse_our_pps_init_qp(rbsp: &[u8]) -> Option<i32> {
    let mut br = bits::BitReader::new(rbsp);
    br.read_bits(6).ok()?; // pps_pic_parameter_set_id
    br.read_bits(4).ok()?; // pps_seq_parameter_set_id
    br.read_bit().ok()?; // pps_mixed_nalu_types_in_pic_flag
    br.read_ue().ok()?; // pps_pic_width_in_luma_samples
    br.read_ue().ok()?; // pps_pic_height_in_luma_samples
    if br.read_bit().ok()? {
        // pps_conformance_window_flag
        for _ in 0..4 {
            br.read_ue().ok()?;
        }
    }
    br.read_bit().ok()?; // pps_scaling_window_explicit_signalling_flag
    br.read_bit().ok()?; // pps_output_flag_present_flag
    br.read_bit().ok()?; // pps_no_pic_partition_flag
    if br.read_bit().ok()? {
        return None; // sub-picture id mapping is not emitted
    }
    br.read_bit().ok()?; // pps_cabac_init_present_flag
    br.read_ue().ok()?; // pps_num_ref_idx_default_active_minus1[0]
    br.read_ue().ok()?; // pps_num_ref_idx_default_active_minus1[1]
    br.read_bit().ok()?; // pps_rpl1_idx_present_flag
    br.read_bit().ok()?; // pps_weighted_pred_flag
    br.read_bit().ok()?; // pps_weighted_bipred_flag
    br.read_bit().ok()?; // pps_ref_wraparound_enabled_flag
    br.read_se().ok()
}

/// Parse the slice header of this encoder's slices and return
/// `(sh_qp_delta, offset_of_slice_data_in_bytes)`.
fn parse_our_slice_header(rbsp: &[u8]) -> Option<(i32, usize)> {
    let mut br = bits::BitReader::new(rbsp);
    if br.read_bit().ok()? {
        return None; // picture header inside slice header is not emitted
    }
    br.read_bit().ok()?; // sh_no_output_of_prior_pics_flag
    let qp_delta = br.read_se().ok()?;
    if br.read_bit().ok()? {
        return None; // `residual_ts_coding` disabled: not emitted by this encoder
    }
    br.byte_align();
    let offset = br.bit_pos() / 8;
    Some((qp_delta, offset))
}

/// Decode a sequence of Annex-B access units produced by [`VvcEncoder`] back
/// into `I420Frame`s.
///
/// This is a decoder for the exact subset the encoder emits (intra PLANAR,
/// transform skip, one CU per 32x32 CTU).  It is used for self-checks and for
/// tools that need to inspect Kine's VVC output; it is **not** a general H.266
/// decoder.
pub fn decode_vvc_annexb_to_i420(access_units: &[&[u8]]) -> Result<Vec<I420Frame>, String> {
    let mut frames = Vec::with_capacity(access_units.len());
    let mut sps_info: Option<VvcSpsInfo> = None;
    let mut pps_init_qp: Option<i32> = None;

    for au in access_units {
        let nals = parse_vvc_annexb_nals(au);
        for nal in &nals {
            match nal.nal_type {
                VvcNalType::Sps => {
                    sps_info = hls::parse_sps_info(&nal.rbsp);
                }
                VvcNalType::Pps => {
                    pps_init_qp = parse_our_pps_init_qp(&nal.rbsp);
                }
                VvcNalType::IdrNLp | VvcNalType::IdrWRadl | VvcNalType::Cra => {
                    let info = sps_info.as_ref().ok_or("missing SPS before slice")?;
                    let pps_qp = pps_init_qp.ok_or("missing PPS before slice")?;
                    let (qp_delta, data_off) =
                        parse_our_slice_header(&nal.rbsp).ok_or("unsupported slice header")?;
                    let slice_qp = (26 + pps_qp + qp_delta).clamp(0, 63) as u8;
                    let cabac = nal
                        .rbsp
                        .get(data_off..)
                        .ok_or("slice data offset out of range")?;
                    let frame = slice::decode_picture(
                        cabac,
                        slice_qp,
                        info.ctu_log2,
                        info.coded_width,
                        info.coded_height,
                    )?;
                    frames.push(crop_frame(&frame, info.width, info.height));
                }
                _ => {}
            }
        }
    }
    Ok(frames)
}

fn crop_frame(frame: &I420Frame, width: u32, height: u32) -> I420Frame {
    if frame.width == width && frame.height == height {
        return frame.clone();
    }
    let cw = width.div_ceil(2);
    let ch = height.div_ceil(2);
    let crop_plane = |src: &[u8], sw: u32, sh: u32, dw: u32, dh: u32| -> Vec<u8> {
        let mut out = vec![0u8; (dw * dh) as usize];
        for y in 0..dh.min(sh) {
            for x in 0..dw.min(sw) {
                out[(y * dw + x) as usize] = src[(y * sw + x) as usize];
            }
        }
        out
    };
    let scw = frame.width.div_ceil(2);
    let sch = frame.height.div_ceil(2);
    I420Frame {
        width,
        height,
        y: crop_plane(&frame.y, frame.width, frame.height, width, height),
        u: crop_plane(&frame.u, scw, sch, cw, ch),
        v: crop_plane(&frame.v, scw, sch, cw, ch),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_frame(width: u32, height: u32, bar_x: u32) -> Vec<u8> {
        let mut rgba = Vec::with_capacity((width * height * 4) as usize);
        for y in 0..height {
            for x in 0..width {
                if x >= bar_x && x < bar_x + 8 {
                    rgba.extend_from_slice(&[240, 90, 40, 255]);
                } else {
                    let g = ((y * 4) % 200) as u8;
                    rgba.extend_from_slice(&[20, g, 80, 255]);
                }
            }
        }
        rgba
    }

    #[test]
    fn emits_a_conformant_slice_structure_and_round_trips_losslessly() {
        let mut settings = EncoderSettings::new(96, 64, 30, 1);
        settings.quantizer = 0; // QP 4: transform-skip lossless
        let mut enc = VvcEncoder::new(settings).expect("vvc encoder");
        assert_eq!(enc.coded_size(), (96, 64)); // already a multiple of 32
        assert_eq!(enc.visible_size(), (96, 64));

        let source = test_frame(96, 64, 24);
        let out = enc.encode_rgba(&source).expect("encode frame");
        assert_eq!(out.units.len(), 1);
        assert!(out.units[0].keyframe);
        let au = &out.units[0].data;

        // Real VVC syntax, not the old private `KVSL` payload.
        assert!(!au.windows(4).any(|w| w == b"KVSL"));
        assert!(!au.windows(4).any(|w| w == b"KVVC"));

        let nals = parse_vvc_annexb_nals(au);
        let types: Vec<VvcNalType> = nals.iter().map(|n| n.nal_type).collect();
        assert_eq!(
            types,
            vec![
                VvcNalType::Aud,
                VvcNalType::Vps,
                VvcNalType::Sps,
                VvcNalType::Pps,
                VvcNalType::Ph,
                VvcNalType::IdrNLp,
            ]
        );
        for nal in &nals {
            assert_eq!(nal.layer_id, 0);
            assert_eq!(nal.temporal_id, 0);
        }
        assert_eq!(nals[0].rbsp, vec![0x88]);
        let sps = hls::parse_sps_info(&nals[2].rbsp).expect("parse SPS");
        assert_eq!((sps.width, sps.height), (96, 64));
        assert_eq!((sps.coded_width, sps.coded_height), (96, 64));
        assert_eq!(sps.ctu_log2, 5);

        let decoded = decode_vvc_annexb_to_i420(&[au.as_slice()]).expect("decode");
        assert_eq!(decoded.len(), 1);
        assert_eq!((decoded[0].width, decoded[0].height), (96, 64));
        let expected = rgba_to_i420(&source, 96, 64);
        assert_eq!(decoded[0].y, expected.y);
        assert_eq!(decoded[0].u, expected.u);
        assert_eq!(decoded[0].v, expected.v);

        assert!(enc.finish().expect("finish").units.is_empty());
        assert!(enc.is_drained());
    }

    #[test]
    fn conformance_window_handles_non_multiple_of_ctu_sizes() {
        let mut settings = EncoderSettings::new(100, 60, 30, 1);
        settings.quantizer = 0; // QP 4: lossless, so the crop must be exact
        let mut enc = VvcEncoder::new(settings).expect("vvc encoder");
        assert_eq!(enc.coded_size(), (128, 64));
        assert_eq!(enc.visible_size(), (100, 60));
        let out = enc.encode_rgba(&test_frame(100, 60, 10)).unwrap();
        let decoded = decode_vvc_annexb_to_i420(&[out.units[0].data.as_slice()]).unwrap();
        assert_eq!((decoded[0].width, decoded[0].height), (100, 60));
        let expected = rgba_to_i420(&test_frame(100, 60, 10), 100, 60);
        assert_eq!(decoded[0].y.len(), expected.y.len());
        assert_eq!(decoded[0].y, expected.y);
        assert_eq!(decoded[0].u, expected.u);
        assert_eq!(decoded[0].v, expected.v);
    }

    #[test]
    fn every_access_unit_is_a_keyframe_and_decodes() {
        let settings = EncoderSettings::new(64, 64, 30, 1);
        let mut enc = VvcEncoder::new(settings).unwrap();
        let mut units = Vec::new();
        for i in 0..3u32 {
            let out = enc.encode_rgba(&test_frame(64, 64, i * 8)).unwrap();
            assert!(out.starts_keyframe);
            units.extend(out.units);
        }
        assert_eq!(units.len(), 3);
        assert!(units.iter().all(|u| u.keyframe));
        let slices: Vec<&[u8]> = units.iter().map(|u| u.data.as_slice()).collect();
        let decoded = decode_vvc_annexb_to_i420(&slices).unwrap();
        assert_eq!(decoded.len(), 3);
    }

    #[test]
    fn rejects_invalid_config_and_wrong_frame_size() {
        assert!(matches!(
            VvcEncoder::new(EncoderSettings::new(0, 48, 30, 1)),
            Err(EncodeError::InvalidConfig(_))
        ));
        let mut enc = VvcEncoder::new(EncoderSettings::new(32, 32, 30, 1)).unwrap();
        assert!(matches!(
            enc.encode_rgba(&[0u8; 12]),
            Err(EncodeError::FrameSize { .. })
        ));
    }

    #[test]
    fn quantizer_maps_into_the_legal_qp_range() {
        assert_eq!(quantizer_to_qp(0), 4);
        assert_eq!(quantizer_to_qp(255), 63);
        for q in 0..=255usize {
            assert!((4..=63).contains(&quantizer_to_qp(q)));
        }
    }
}
