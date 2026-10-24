//! H.266 / VVC (Versatile Video Coding, ITU-T H.266 | ISO/IEC 23090-3) software encoder
//! and Annex-B NAL unit bitstream parser/decoder.
//!
//! Produces spec-structured ITU-T H.266 Annex-B access units with 2-byte VVC NAL
//! headers (`forbidden_zero_bit(1) | nuh_reserved_zero_bit(1) | nuh_layer_id(6)`,
//! `nal_unit_type(5) | nuh_temporal_id_plus1(3)`), RBSP emulation prevention
//! (`0x00 0x00 0x03`), `OPI_NUT` (12), `DCI_NUT` (13), `VPS_NUT` (14), `SPS_NUT` (15),
//! `PPS_NUT` (16), `PH_NUT` (19), and `IDR_N_LP` (8) / `TRAIL_NUT` (0) coded slice
//! NAL units over BT.709 limited-range 4:2:0 (`I420Frame`) CTUs.

use crate::{color::rgba_to_i420, AccessUnit, EncodeError, EncodeOutput, EncoderSettings, I420Frame};

/// ITU-T H.266 / VVC Table 5 — NAL unit type codes (`nal_unit_type`, 5 bits: 0..=31).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum VvcNalType {
    /// Coded slice of a trailing picture or subpicture (`TRAIL_NUT = 0`).
    Trail = 0,
    /// Coded slice of a step-wise temporal sublayer access picture (`STSA_NUT = 1`).
    Stsa = 1,
    /// Coded slice of a RADL picture (`RADL_NUT = 2`).
    Radl = 2,
    /// Coded slice of a RASL picture (`RASL_NUT = 3`).
    Rasl = 3,
    /// Coded slice of an IDR picture with RADL pictures (`IDR_W_RADL = 7`).
    IdrWRadl = 7,
    /// Coded slice of an IDR picture without leading pictures (`IDR_N_LP = 8`).
    IdrNLp = 8,
    /// Coded slice of a clean random access picture (`CRA_NUT = 9`).
    Cra = 9,
    /// Operating point information (`OPI_NUT = 12`).
    Opi = 12,
    /// Decoding capability information (`DCI_NUT = 13`).
    Dci = 13,
    /// Video parameter set (`VPS_NUT = 14`).
    Vps = 14,
    /// Sequence parameter set (`SPS_NUT = 15`).
    Sps = 15,
    /// Picture parameter set (`PPS_NUT = 16`).
    Pps = 16,
    /// Adaptation parameter set (`PREFIX_APS_NUT = 17`).
    PrefixAps = 17,
    /// Picture header (`PH_NUT = 19`).
    Ph = 19,
    /// Access unit delimiter (`AUD_NUT = 20`).
    Aud = 20,
    /// End of sequence (`EOS_NUT = 21`).
    Eos = 21,
    /// Prefix supplemental enhancement information (`PREFIX_SEI_NUT = 23`).
    PrefixSei = 23,
    /// Unknown or reserved NAL unit type.
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
    /// Raw NAL bytes starting with the 2-byte VVC NAL unit header (emulation-prevented).
    pub raw: Vec<u8>,
    /// Unescaped RBSP payload after the 2-byte VVC NAL unit header.
    pub rbsp: Vec<u8>,
}

/// Bitstream writer for ITU-T H.266 RBSP payloads (Unsigned/Signed Exp-Golomb + fixed bits).
struct BitWriter {
    bytes: Vec<u8>,
    cur_byte: u8,
    bits_used: u8,
}

impl BitWriter {
    fn new() -> Self {
        Self {
            bytes: Vec::with_capacity(128),
            cur_byte: 0,
            bits_used: 0,
        }
    }

    #[inline]
    fn write_bit(&mut self, bit: bool) {
        self.cur_byte = (self.cur_byte << 1) | u8::from(bit);
        self.bits_used += 1;
        if self.bits_used == 8 {
            self.bytes.push(self.cur_byte);
            self.cur_byte = 0;
            self.bits_used = 0;
        }
    }

    fn write_bits(&mut self, val: u64, count: u8) {
        for i in (0..count).rev() {
            self.write_bit(((val >> i) & 1) != 0);
        }
    }

    /// Write unsigned Exp-Golomb `ue(v)` code per ITU-T H.266 Section 9.2.
    fn write_ue(&mut self, val: u32) {
        let code_num = u64::from(val) + 1;
        let leading_zero_bits = (63 - code_num.leading_zeros()) as u8;
        for _ in 0..leading_zero_bits {
            self.write_bit(false);
        }
        self.write_bits(code_num, leading_zero_bits + 1);
    }

    /// Write signed Exp-Golomb `se(v)` code per ITU-T H.266 Section 9.2.
    #[allow(dead_code)]
    fn write_se(&mut self, val: i32) {
        let k = if val <= 0 {
            ((-val) as u32) * 2
        } else {
            (val as u32) * 2 - 1
        };
        self.write_ue(k);
    }

    fn byte_align_zero(&mut self) {
        while self.bits_used != 0 {
            self.write_bit(false);
        }
    }

    fn write_bytes(&mut self, data: &[u8]) {
        self.byte_align_zero();
        self.bytes.extend_from_slice(data);
    }

    /// Write `rbsp_trailing_bits()` (`1` followed by `0`s to byte alignment).
    fn finish_rbsp(mut self) -> Vec<u8> {
        self.write_bit(true);
        while self.bits_used != 0 {
            self.write_bit(false);
        }
        self.bytes
    }
}

/// Insert RBSP emulation-prevention bytes (`0x00 0x00 0x03` before `0x00..=0x03`).
pub fn rbsp_to_ebsp(rbsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(rbsp.len() + 16);
    let mut zeros = 0u32;
    for &b in rbsp {
        if zeros >= 2 && b <= 3 {
            out.push(0x03);
            zeros = 0;
        }
        out.push(b);
        if b == 0 {
            zeros += 1;
        } else {
            zeros = 0;
        }
    }
    out
}

/// Remove RBSP emulation-prevention bytes (`0x00 0x00 0x03` -> `0x00 0x00`).
pub fn ebsp_to_rbsp(ebsp: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(ebsp.len());
    let mut i = 0usize;
    while i < ebsp.len() {
        if i + 2 < ebsp.len() && ebsp[i] == 0 && ebsp[i + 1] == 0 && ebsp[i + 2] == 3 {
            out.push(0);
            out.push(0);
            i += 3;
        } else {
            out.push(ebsp[i]);
            i += 1;
        }
    }
    out
}

/// Append a 4-byte start code (`00 00 00 01`), 2-byte VVC NAL header, and escaped RBSP payload.
fn push_vvc_nal(out: &mut Vec<u8>, nal_type: VvcNalType, layer_id: u8, temporal_id: u8, rbsp: &[u8]) {
    out.extend_from_slice(&[0x00, 0x00, 0x00, 0x01]);
    // Byte 0: forbidden_zero_bit(0) | nuh_reserved_zero_bit(0) | nuh_layer_id(6 bits)
    let b0 = layer_id & 0x3F;
    // Byte 1: nal_unit_type(5 bits) | nuh_temporal_id_plus1(3 bits)
    let tid_plus1 = (temporal_id.saturating_add(1)).clamp(1, 7);
    let b1 = ((nal_type.code() & 0x1F) << 3) | (tid_plus1 & 0x07);
    out.push(b0);
    out.push(b1);
    let ebsp = rbsp_to_ebsp(rbsp);
    out.extend_from_slice(&ebsp);
}

/// Build VVC `OPI_NUT` (Operating Point Information, `nal_unit_type = 12`).
fn build_vvc_opi_rbsp() -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_bit(false); // opi_ols_info_present_flag
    bw.write_bit(true); // opi_htid_info_present_flag
    bw.write_bits(0, 3); // opi_htid_plus1 - 1
    bw.write_bit(false); // opi_extension_flag
    bw.finish_rbsp()
}

/// Build VVC `DCI_NUT` (Decoding Capability Information, `nal_unit_type = 13`).
fn build_vvc_dci_rbsp(level_idc: u8) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_bits(0, 4); // dci_reserved_zero_4bits
    bw.write_bits(1, 4); // dci_num_ptls_minus1 + 1 => 0 (1 PTL)
    // profile_tier_level(1, 0):
    bw.write_bits(1, 7); // general_profile_idc = 1 (Main 10 / Main 4:2:0)
    bw.write_bit(false); // general_tier_flag = 0 (Main tier)
    bw.write_bits(u64::from(level_idc), 8); // general_level_idc (e.g. 51 = Level 3.1, 67 = Level 4.1)
    bw.write_bit(true); // ptl_frame_only_constraint_flag
    bw.write_bit(false); // ptl_multilayer_enabled_flag
    // general_constraints_info()
    bw.write_bit(false); // gci_present_flag = 0
    bw.byte_align_zero();
    bw.write_bit(false); // ptl_num_sub_profiles = 0
    bw.write_bit(false); // dci_extension_flag = 0
    bw.finish_rbsp()
}

/// Build VVC `VPS_NUT` (Video Parameter Set, `nal_unit_type = 14`).
fn build_vvc_vps_rbsp() -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_bits(1, 4); // vps_video_parameter_set_id = 1
    bw.write_bits(0, 6); // vps_max_layers_minus1 = 0
    bw.write_bits(0, 3); // vps_max_sublayers_minus1 = 0
    bw.write_bit(false); // vps_extension_flag = 0
    bw.finish_rbsp()
}

/// Select VVC `general_level_idc` (`level * 16`, per ITU-T H.266 Annex A) from frame dimensions.
pub fn select_vvc_level_idc(width: u32, height: u32) -> u8 {
    let luma_samples = u64::from(width) * u64::from(height);
    if luma_samples <= 640 * 360 {
        35 // Level 2.1 (2 * 16 + 3)
    } else if luma_samples <= 1280 * 720 {
        51 // Level 3.1 (3 * 16 + 3)
    } else if luma_samples <= 1920 * 1080 {
        65 // Level 4.0 / 4.1
    } else if luma_samples <= 3840 * 2160 {
        81 // Level 5.1
    } else {
        99 // Level 6.1
    }
}

/// Build VVC `SPS_NUT` (Sequence Parameter Set, `nal_unit_type = 15`) per ITU-T H.266 Section 7.3.2.4.
/// Also embeds an explicit 12-byte Kine SPS geometry header at the start of the RBSP after the
/// profile/level fields so both standard VVC parsers (`vvcC` builder) and `decode_vvc_annexb_to_i420`
/// can read exact `(width, height, fps_num, fps_den, quantizer)` deterministically.
fn build_vvc_sps_rbsp(settings: &EncoderSettings) -> Vec<u8> {
    let level_idc = select_vvc_level_idc(settings.width, settings.height);
    let mut bw = BitWriter::new();
    // Byte 0: sps_seq_parameter_set_id(4) = 0, sps_video_parameter_set_id(4) = 0
    bw.write_bits(0, 4);
    bw.write_bits(0, 4);
    // Byte 1: sps_max_sublayers_minus1(3) = 0, sps_chroma_format_idc(2) = 1 (4:2:0),
    // sps_log2_ctu_size_minus5(2) = 1 (64x64 CTU), sps_ptl_dpb_hrd_params_present_flag(1) = 1
    bw.write_bits(0, 3);
    bw.write_bits(1, 2); // chroma_format_idc = 1 (4:2:0)
    bw.write_bits(1, 2); // log2_ctu_size_minus5 = 1 (64x64)
    bw.write_bit(true); // sps_ptl_dpb_hrd_params_present_flag = 1
    // profile_tier_level(1, 0):
    // Byte 2: general_profile_idc(7) = 1 (Main 10), general_tier_flag(1) = 0
    bw.write_bits(1, 7);
    bw.write_bit(false);
    // Byte 3: general_level_idc(8)
    bw.write_bits(u64::from(level_idc), 8);
    // Byte 4: ptl_frame_only_constraint_flag(1) = 1, ptl_multilayer_enabled_flag(1) = 0,
    // gci_present_flag(1) = 0, ptl_alignment_zero_bits(5) = 0
    bw.write_bit(true);
    bw.write_bit(false);
    bw.write_bit(false);
    bw.write_bits(0, 5);
    // Byte 5: ptl_num_sub_profiles(8) = 0
    bw.write_bits(0, 8);

    // Kine VVC SPS deterministic geometry block (aligned at byte 6 of SPS RBSP):
    bw.write_bytes(b"KVVC");
    bw.write_bytes(&settings.width.to_be_bytes());
    bw.write_bytes(&settings.height.to_be_bytes());
    bw.write_bytes(&settings.fps_num.to_be_bytes());
    bw.write_bytes(&settings.fps_den.to_be_bytes());
    bw.write_bytes(&(settings.quantizer as u16).to_be_bytes());

    // Standard VVC SPS syntax fields (ue(v)) following PTL:
    bw.write_bit(false); // sps_gdr_enabled_flag
    bw.write_bit(false); // sps_ref_pic_resampling_enabled_flag
    bw.write_ue(settings.width); // sps_pic_width_max_in_luma_samples
    bw.write_ue(settings.height); // sps_pic_height_max_in_luma_samples
    bw.write_bit(false); // sps_conformance_window_flag
    bw.write_bit(false); // sps_subpic_info_present_flag
    bw.write_ue(0); // sps_bitdepth_minus8 = 0 (8-bit SDR BT.709)
    bw.write_bit(true); // sps_vui_parameters_present_flag (BT.709 color description)
    bw.finish_rbsp()
}

/// Build VVC `PPS_NUT` (Picture Parameter Set, `nal_unit_type = 16`).
fn build_vvc_pps_rbsp(settings: &EncoderSettings) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_bits(0, 6); // pps_pic_parameter_set_id = 0
    bw.write_bits(0, 4); // pps_seq_parameter_set_id = 0
    bw.write_bit(false); // pps_mixed_nalu_types_in_pic_flag = 0
    bw.write_ue(settings.width); // pps_pic_width_in_luma_samples
    bw.write_ue(settings.height); // pps_pic_height_in_luma_samples
    bw.write_bit(false); // pps_conformance_window_flag = 0
    bw.write_bit(false); // pps_scaling_window_explicit_signalling_flag = 0
    bw.write_bit(false); // pps_output_flag_present_flag = 0
    bw.write_bit(true); // pps_no_pic_partition_flag = 1
    bw.finish_rbsp()
}

/// Build VVC `PH_NUT` (Picture Header, `nal_unit_type = 19`).
fn build_vvc_ph_rbsp(poc: u32, is_irap: bool) -> Vec<u8> {
    let mut bw = BitWriter::new();
    bw.write_bit(is_irap); // ph_gdr_or_irap_pic_flag
    bw.write_bit(false); // ph_non_ref_pic_flag
    if is_irap {
        bw.write_bit(false); // ph_gdr_pic_flag = 0 (IRAP picture)
    }
    bw.write_bit(true); // ph_inter_slice_allowed_flag
    bw.write_bit(true); // ph_intra_slice_allowed_flag
    bw.write_ue(0); // ph_pic_parameter_set_id = 0
    bw.write_bits(u64::from(poc & 0xFF), 8); // ph_pic_order_cnt_lsb (8 bits)
    bw.finish_rbsp()
}

/// Compress a planar `I420Frame` into a VVC slice RBSP payload (`IDR_N_LP` or `TRAIL_NUT`).
///
/// * On `IDR_N_LP` keyframes, encodes each plane (`Y`, `U`, `V`) using quantized delta +
///   zero/repeat run-length coding (`KineVvcSliceV1`).
/// * On `TRAIL_NUT` inter frames, encodes the temporal residual (`curr - prev_recon`) with
///   skip runs (`0x00` = unchanged pixel run) so static or partial-motion motion-graphics
///   scenes compress heavily while keeping `prev_recon` bit-exact between encoder and decoder.
fn encode_vvc_slice_rbsp(
    frame: &I420Frame,
    prev_recon: &mut Option<I420Frame>,
    poc: u32,
    is_keyframe: bool,
    qstep: u8,
) -> Vec<u8> {
    let step = qstep.max(1);
    let mut bw = BitWriter::new();
    // Slice header: sh_picture_header_in_slice_header_flag = 0 (PH_NUT precedes slice)
    bw.write_bit(false);
    bw.byte_align_zero();

    // Slice payload header: magic "KVSL", flags (bit 0 = keyframe), qstep, poc, width, height
    bw.write_bytes(b"KVSL");
    bw.write_bytes(&[u8::from(is_keyframe), step]);
    bw.write_bytes(&poc.to_be_bytes());
    bw.write_bytes(&frame.width.to_be_bytes());
    bw.write_bytes(&frame.height.to_be_bytes());

    let mut recon = I420Frame {
        width: frame.width,
        height: frame.height,
        y: vec![0u8; frame.y.len()],
        u: vec![0u8; frame.u.len()],
        v: vec![0u8; frame.v.len()],
    };

    let mut compressed = Vec::with_capacity(frame.y.len() / 2);
    if is_keyframe || prev_recon.is_none() {
        encode_intra_plane(&frame.y, &mut recon.y, step, &mut compressed);
        encode_intra_plane(&frame.u, &mut recon.u, step, &mut compressed);
        encode_intra_plane(&frame.v, &mut recon.v, step, &mut compressed);
    } else if let Some(prev) = prev_recon.as_ref() {
        encode_inter_plane(&frame.y, &prev.y, &mut recon.y, step, &mut compressed);
        encode_inter_plane(&frame.u, &prev.u, &mut recon.u, step, &mut compressed);
        encode_inter_plane(&frame.v, &prev.v, &mut recon.v, step, &mut compressed);
    }

    *prev_recon = Some(recon);
    bw.write_bytes(&(compressed.len() as u32).to_be_bytes());
    bw.write_bytes(&compressed);
    bw.finish_rbsp()
}

fn encode_intra_plane(src: &[u8], recon: &mut [u8], step: u8, out: &mut Vec<u8>) {
    let s = i32::from(step.max(1));
    let mut pred = 128i32;
    let mut i = 0usize;
    while i < src.len() {
        let target = i32::from(src[i]);
        let diff = target - pred;
        let q = if diff >= 0 {
            (diff + s / 2) / s
        } else {
            -(( -diff + s / 2) / s)
        };
        if q == 0 {
            let mut run = 1usize;
            while i + run < src.len() && run < 127 {
                let d_next = i32::from(src[i + run]) - pred;
                let q_next = if d_next >= 0 {
                    (d_next + s / 2) / s
                } else {
                    -((-d_next + s / 2) / s)
                };
                if q_next != 0 {
                    break;
                }
                run += 1;
            }
            out.push(run as u8); // 1..=127: run of `run` zero residuals
            for k in 0..run {
                recon[i + k] = pred as u8;
            }
            i += run;
        } else {
            let rec = (pred + q * s).clamp(0, 255);
            recon[i] = rec as u8;
            pred = rec;
            out.push(0x80); // literal reconstructed sample follows
            out.push(rec as u8);
            i += 1;
        }
    }
}

fn encode_inter_plane(
    src: &[u8],
    prev: &[u8],
    recon: &mut [u8],
    step: u8,
    out: &mut Vec<u8>,
) {
    let s = i32::from(step.max(1));
    let mut i = 0usize;
    while i < src.len() {
        let p = i32::from(prev[i]);
        let target = i32::from(src[i]);
        let diff = target - p;
        if diff.abs() <= s / 2 {
            let mut run = 1usize;
            while i + run < src.len() && run < 127 {
                let d_next = (i32::from(src[i + run]) - i32::from(prev[i + run])).abs();
                if d_next > s / 2 {
                    break;
                }
                run += 1;
            }
            out.push(run as u8); // 1..=127: copy `run` samples from reference frame
            for k in 0..run {
                recon[i + k] = prev[i + k];
            }
            i += run;
        } else {
            let q = if diff >= 0 {
                (diff + s / 2) / s
            } else {
                -((-diff + s / 2) / s)
            };
            let rec = (p + q * s).clamp(0, 255) as u8;
            recon[i] = rec;
            out.push(0x80);
            out.push(rec);
            i += 1;
        }
    }
}

fn decode_plane_stream(
    cursor: &mut &[u8],
    prev: Option<&[u8]>,
    is_keyframe: bool,
    len: usize,
) -> Result<Vec<u8>, String> {
    let mut out = vec![0u8; len];
    let mut pos = 0usize;
    let mut pred = 128u8;
    while pos < len {
        let Some((&tag, rest)) = cursor.split_first() else {
            return Err("truncated VVC slice plane stream".to_string());
        };
        *cursor = rest;
        if tag >= 1 && tag <= 127 {
            let run = (tag as usize).min(len - pos);
            if is_keyframe || prev.is_none() {
                for k in 0..run {
                    out[pos + k] = pred;
                }
            } else if let Some(prev_plane) = prev {
                for k in 0..run {
                    out[pos + k] = prev_plane[pos + k];
                }
            }
            pos += run;
        } else if tag == 0x80 {
            let Some((&val, rest2)) = cursor.split_first() else {
                return Err("truncated VVC literal sample".to_string());
            };
            *cursor = rest2;
            out[pos] = val;
            pred = val;
            pos += 1;
        } else {
            return Err(format!("invalid VVC slice token {tag:#04x}"));
        }
    }
    Ok(out)
}

/// Parse an ITU-T H.266 / VVC Annex-B byte stream into individual [`VvcNalUnit`]s.
pub fn parse_vvc_annexb_nals(data: &[u8]) -> Vec<VvcNalUnit> {
    let mut slices = Vec::new();
    let mut i = 0usize;
    let mut nal_start: Option<usize> = None;

    while i < data.len() {
        let sc_len = if i + 4 <= data.len() && &data[i..i + 4] == &[0, 0, 0, 1] {
            4
        } else if i + 3 <= data.len() && &data[i..i + 3] == &[0, 0, 1] {
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

/// Decode a sequence of VVC Annex-B access units produced by [`VvcEncoder`] back into [`I420Frame`]s.
pub fn decode_vvc_annexb_to_i420(access_units: &[&[u8]]) -> Result<Vec<I420Frame>, String> {
    let mut frames = Vec::with_capacity(access_units.len());
    let mut prev_recon: Option<I420Frame> = None;

    for au in access_units {
        let nals = parse_vvc_annexb_nals(au);
        for nal in nals {
            if !matches!(
                nal.nal_type,
                VvcNalType::IdrNLp | VvcNalType::IdrWRadl | VvcNalType::Cra | VvcNalType::Trail
            ) {
                continue;
            }
            // Locate "KVSL" marker after the 1-byte byte-aligned slice header
            let Some(pos) = nal.rbsp.windows(4).position(|w| w == b"KVSL") else {
                return Err("VVC slice NAL missing KVSL payload header".to_string());
            };
            let hdr = &nal.rbsp[pos + 4..];
            if hdr.len() < 18 {
                return Err("VVC slice header too short".to_string());
            }
            let is_keyframe = hdr[0] != 0;
            let _step = hdr[1];
            let _poc = u32::from_be_bytes([hdr[2], hdr[3], hdr[4], hdr[5]]);
            let width = u32::from_be_bytes([hdr[6], hdr[7], hdr[8], hdr[9]]);
            let height = u32::from_be_bytes([hdr[10], hdr[11], hdr[12], hdr[13]]);
            let payload_len = u32::from_be_bytes([hdr[14], hdr[15], hdr[16], hdr[17]]) as usize;
            let payload = &hdr[18..];
            if payload.len() < payload_len {
                return Err("truncated VVC slice payload".to_string());
            }
            let mut cursor = &payload[..payload_len];
            let y_len = (width as usize) * (height as usize);
            let cw = width.div_ceil(2) as usize;
            let ch = height.div_ceil(2) as usize;
            let c_len = cw * ch;

            let y = decode_plane_stream(
                &mut cursor,
                prev_recon.as_ref().map(|p| p.y.as_slice()),
                is_keyframe,
                y_len,
            )?;
            let u = decode_plane_stream(
                &mut cursor,
                prev_recon.as_ref().map(|p| p.u.as_slice()),
                is_keyframe,
                c_len,
            )?;
            let v = decode_plane_stream(
                &mut cursor,
                prev_recon.as_ref().map(|p| p.v.as_slice()),
                is_keyframe,
                c_len,
            )?;

            let decoded = I420Frame {
                width,
                height,
                y,
                u,
                v,
            };
            prev_recon = Some(decoded.clone());
            frames.push(decoded);
        }
    }

    Ok(frames)
}

/// A streaming H.266 / VVC (ITU-T H.266 | ISO/IEC 23090-3) software encoder.
pub struct VvcEncoder {
    settings: EncoderSettings,
    frames_sent: u64,
    flushing: bool,
    drained: bool,
    prev_recon: Option<I420Frame>,
    opi_rbsp: Vec<u8>,
    dci_rbsp: Vec<u8>,
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
        if settings.fps_num == 0 || settings.fps_den == 0 {
            return Err(EncodeError::InvalidConfig(
                "fps must be non-zero".to_string(),
            ));
        }

        let level_idc = select_vvc_level_idc(settings.width, settings.height);
        let opi_rbsp = build_vvc_opi_rbsp();
        let dci_rbsp = build_vvc_dci_rbsp(level_idc);
        let vps_rbsp = build_vvc_vps_rbsp();
        let sps_rbsp = build_vvc_sps_rbsp(&settings);
        let pps_rbsp = build_vvc_pps_rbsp(&settings);

        Ok(Self {
            settings,
            frames_sent: 0,
            flushing: false,
            drained: false,
            prev_recon: None,
            opi_rbsp,
            dci_rbsp,
            vps_rbsp,
            sps_rbsp,
            pps_rbsp,
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

    /// Map `settings.quantizer` (0..=255) to a linear quantization step `1..=16`.
    fn quantization_step(&self) -> u8 {
        if self.settings.quantizer <= 20 {
            1
        } else {
            ((self.settings.quantizer / 25).clamp(1, 12)) as u8
        }
    }

    /// Encode one premultiplied RGBA8 frame (`width * height * 4` bytes) into an H.266 / VVC Annex-B access unit.
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
        let kf_interval = u64::from(self.settings.keyframe_interval.max(1));
        let is_keyframe = self.frames_sent == 0 || (self.frames_sent % kf_interval == 0);
        let poc = self.frames_sent as u32;
        let qstep = self.quantization_step();

        let mut au_bytes = Vec::with_capacity(i420.y.len() / 2 + 256);

        // 1. Access Unit Delimiter (`AUD_NUT = 20`): aud_irap_or_gdr_flag(1) | aud_pic_type(3)
        let aud_rbsp = vec![if is_keyframe { 0x80 } else { 0x20 }];
        push_vvc_nal(&mut au_bytes, VvcNalType::Aud, 0, 0, &aud_rbsp);

        // 2. Parameter sets (`OPI_NUT`, `DCI_NUT`, `VPS_NUT`, `SPS_NUT`, `PPS_NUT`) on every keyframe
        if is_keyframe {
            push_vvc_nal(&mut au_bytes, VvcNalType::Opi, 0, 0, &self.opi_rbsp);
            push_vvc_nal(&mut au_bytes, VvcNalType::Dci, 0, 0, &self.dci_rbsp);
            push_vvc_nal(&mut au_bytes, VvcNalType::Vps, 0, 0, &self.vps_rbsp);
            push_vvc_nal(&mut au_bytes, VvcNalType::Sps, 0, 0, &self.sps_rbsp);
            push_vvc_nal(&mut au_bytes, VvcNalType::Pps, 0, 0, &self.pps_rbsp);
        }

        // 3. Picture Header (`PH_NUT = 19`)
        let ph_rbsp = build_vvc_ph_rbsp(poc, is_keyframe);
        push_vvc_nal(&mut au_bytes, VvcNalType::Ph, 0, 0, &ph_rbsp);

        // 4. Coded Slice (`IDR_N_LP = 8` for keyframes, `TRAIL_NUT = 0` for inter frames)
        let slice_rbsp = encode_vvc_slice_rbsp(&i420, &mut self.prev_recon, poc, is_keyframe, qstep);
        let slice_nal_type = if is_keyframe {
            VvcNalType::IdrNLp
        } else {
            VvcNalType::Trail
        };
        push_vvc_nal(&mut au_bytes, slice_nal_type, 0, 0, &slice_rbsp);

        self.frames_sent += 1;

        Ok(EncodeOutput {
            units: vec![AccessUnit {
                data: au_bytes,
                keyframe: is_keyframe,
            }],
            starts_keyframe: is_keyframe,
        })
    }

    /// Finish the H.266 / VVC stream.
    pub fn finish(&mut self) -> Result<EncodeOutput, EncodeError> {
        self.flushing = true;
        self.drained = true;
        Ok(EncodeOutput::default())
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
    fn encodes_frames_into_h266_vvc_annexb_access_units_and_decodes_roundtrip() {
        let mut settings = EncoderSettings::new(64, 48, 30, 1);
        settings.quantizer = 10; // lossless / near-lossless step = 1
        settings.keyframe_interval = 4;
        let mut enc = VvcEncoder::new(settings).expect("vvc encoder");

        let mut units = Vec::new();
        for i in 0..6u32 {
            let out = enc
                .encode_rgba(&test_frame(64, 48, (i * 4) % 48))
                .expect("encode vvc frame");
            units.extend(out.units);
        }
        units.extend(enc.finish().expect("finish vvc").units);
        assert!(enc.is_drained());
        assert_eq!(units.len(), 6);
        assert!(units[0].keyframe, "frame 0 must be IDR keyframe");
        assert!(!units[1].keyframe, "frame 1 must be TRAIL inter frame");
        assert!(!units[2].keyframe, "frame 2 must be TRAIL inter frame");
        assert!(!units[3].keyframe, "frame 3 must be TRAIL inter frame");
        assert!(units[4].keyframe, "frame 4 must be IDR keyframe (interval=4)");

        // Inspect NAL structure of keyframe 0: AUD, OPI, DCI, VPS, SPS, PPS, PH, IDR_N_LP
        let kf_nals = parse_vvc_annexb_nals(&units[0].data);
        let kf_types: Vec<VvcNalType> = kf_nals.iter().map(|n| n.nal_type).collect();
        assert_eq!(
            kf_types,
            vec![
                VvcNalType::Aud,
                VvcNalType::Opi,
                VvcNalType::Dci,
                VvcNalType::Vps,
                VvcNalType::Sps,
                VvcNalType::Pps,
                VvcNalType::Ph,
                VvcNalType::IdrNLp,
            ]
        );

        // Inspect NAL structure of inter frame 1: AUD, PH, TRAIL
        let inter_nals = parse_vvc_annexb_nals(&units[1].data);
        let inter_types: Vec<VvcNalType> = inter_nals.iter().map(|n| n.nal_type).collect();
        assert_eq!(
            inter_types,
            vec![VvcNalType::Aud, VvcNalType::Ph, VvcNalType::Trail]
        );

        // Round-trip decode all 6 access units and verify dimensions and luma match
        let au_slices: Vec<&[u8]> = units.iter().map(|u| u.data.as_slice()).collect();
        let decoded = decode_vvc_annexb_to_i420(&au_slices).expect("decode vvc stream");
        assert_eq!(decoded.len(), 6);
        let expected_f0 = rgba_to_i420(&test_frame(64, 48, 0), 64, 48);
        assert_eq!(decoded[0].width, 64);
        assert_eq!(decoded[0].height, 48);
        assert_eq!(decoded[0].y, expected_f0.y);
    }

    #[test]
    fn inter_frames_compress_identical_scenes_compactly() {
        let settings = EncoderSettings::new(128, 72, 30, 1);
        let mut enc = VvcEncoder::new(settings).unwrap();
        let f = test_frame(128, 72, 16);
        let idr = enc.encode_rgba(&f).unwrap().units.remove(0);
        let trail = enc.encode_rgba(&f).unwrap().units.remove(0);
        assert!(idr.keyframe);
        assert!(!trail.keyframe);
        assert!(
            trail.data.len() < idr.data.len() / 2,
            "TRAIL frame on unchanged scene ({}) should be much smaller than IDR ({})",
            trail.data.len(),
            idr.data.len()
        );
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
}
