//! Minimal H.266 SPS metadata parser used by the `vvcC` builder.
//!
//! This is container-level metadata parsing only: it reads the profile, level,
//! chroma format, bit depth and picture size needed for an ISO-BMFF
//! `VvcDecoderConfigurationRecord`. It does not decode video.

/// MSB-first bit writer used for the `vvcC` PTL block.
#[derive(Debug, Default)]
pub struct BitWriter {
    bytes: Vec<u8>,
    acc: u64,
    nbits: u32,
}

impl BitWriter {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn write_bit(&mut self, bit: bool) {
        self.acc = (self.acc << 1) | u64::from(bit);
        self.nbits += 1;
        if self.nbits == 8 {
            self.bytes.push(self.acc as u8);
            self.acc = 0;
            self.nbits = 0;
        }
    }

    pub fn write_bits(&mut self, value: u32, n: u32) {
        for i in (0..n).rev() {
            self.write_bit(((value >> i) & 1) != 0);
        }
    }

    pub fn bits_written(&self) -> usize {
        self.bytes.len() * 8 + self.nbits as usize
    }

    /// Finish the writer, padding with zero bits when necessary.
    pub fn into_bytes(mut self) -> Vec<u8> {
        if self.nbits != 0 {
            let pad = 8 - self.nbits;
            self.write_bits(0, pad);
        }
        self.bytes
    }
}

/// MSB-first bit reader.
#[derive(Debug, Clone)]
pub struct BitReader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> BitReader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Self { data, pos: 0 }
    }

    pub fn is_byte_aligned(&self) -> bool {
        self.pos & 7 == 0
    }

    pub fn read_bit(&mut self) -> Option<bool> {
        if self.pos >= self.data.len() * 8 {
            return None;
        }
        let byte = self.data[self.pos >> 3];
        let bit = (byte >> (7 - (self.pos & 7))) & 1;
        self.pos += 1;
        Some(bit != 0)
    }

    pub fn read_bits(&mut self, n: u32) -> Option<u32> {
        let mut value = 0u32;
        for _ in 0..n {
            value = (value << 1) | u32::from(self.read_bit()?);
        }
        Some(value)
    }

    pub fn read_ue(&mut self) -> Option<u32> {
        let mut leading = 0u32;
        while !self.read_bit()? {
            leading += 1;
            if leading > 31 {
                return None;
            }
        }
        if leading == 0 {
            return Some(0);
        }
        Some(((1u32 << leading) - 1) + self.read_bits(leading)?)
    }
}

/// Metadata the muxer needs from an SPS.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VvcSpsInfo {
    pub sps_id: u8,
    pub vps_id: u8,
    pub chroma_format_idc: u8,
    pub bit_depth_luma: u8,
    /// Visible (conformance-window-cropped) size.
    pub width: u32,
    pub height: u32,
    /// Coded size (`sps_pic_width_max_in_luma_samples`).
    pub coded_width: u32,
    pub coded_height: u32,
    pub ctu_log2: u8,
    pub profile_idc: u8,
    pub tier_flag: bool,
    pub level_idc: u8,
    pub frame_only_constraint: bool,
    pub multilayer_enabled: bool,
    /// `general_constraints_info()` syntax bits, starting with
    /// `gci_present_flag` and excluding its alignment zeros.
    pub gci_bits: Vec<bool>,
    pub num_sub_profiles: u8,
    pub sub_profile_idc: Vec<u32>,
}

/// Parse the leading fields of an SPS RBSP (no emulation-prevention bytes).
///
/// Supports the single-layer, progressive SPS emitted by FFmpeg's `libvvenc`
/// wrapper and by Kine's former encoder. Returns `None` for structures it
/// cannot follow (e.g. sub-pictures), so callers can fall back to the encoder's
/// known dimensions.
pub fn parse_sps_info(rbsp: &[u8]) -> Option<VvcSpsInfo> {
    let mut br = BitReader::new(rbsp);
    let sps_id = br.read_bits(4)? as u8;
    let vps_id = br.read_bits(4)? as u8;
    let max_sub_layers_minus1 = br.read_bits(3)? as usize;
    let chroma_format_idc = br.read_bits(2)? as u8;
    let log2_ctu_size_minus5 = br.read_bits(2)? as u8;
    if !br.read_bit()? {
        return None;
    }

    // profile_tier_level(1, max_sub_layers_minus1)
    let profile_idc = br.read_bits(7)? as u8;
    let tier_flag = br.read_bit()?;
    let level_idc = br.read_bits(8)? as u8;
    let frame_only_constraint = br.read_bit()?;
    let multilayer_enabled = br.read_bit()?;

    let mut gci_bits = Vec::with_capacity(96);
    let gci_present = br.read_bit()?;
    gci_bits.push(gci_present);
    if gci_present {
        for _ in 0..71 {
            gci_bits.push(br.read_bit()?);
        }
        let additional = br.read_bits(8)?;
        for i in (0..8).rev() {
            gci_bits.push((additional >> i) & 1 != 0);
        }
        if additional > 5 {
            for _ in 0..6 {
                gci_bits.push(br.read_bit()?);
            }
        }
        for _ in 0..additional.saturating_sub(if additional > 5 { 6 } else { 0 }) {
            gci_bits.push(br.read_bit()?);
        }
    }
    while !br.is_byte_aligned() {
        if br.read_bit()? {
            return None;
        }
    }

    // Sub-layer level flags, then their values.
    let mut sub_layer_level_present = vec![false; max_sub_layers_minus1];
    for i in (0..max_sub_layers_minus1).rev() {
        sub_layer_level_present[i] = br.read_bit()?;
    }
    while !br.is_byte_aligned() {
        if br.read_bit()? {
            return None;
        }
    }
    for present in sub_layer_level_present.into_iter().rev() {
        if present {
            let _ = br.read_bits(8)?;
        }
    }

    let num_sub_profiles = br.read_bits(8)? as u8;
    let mut sub_profile_idc = Vec::with_capacity(num_sub_profiles as usize);
    for _ in 0..num_sub_profiles {
        sub_profile_idc.push(br.read_bits(32)?);
    }

    let _gdr_enabled = br.read_bit()?;
    let _ref_pic_resampling = br.read_bit()?;
    let width = br.read_ue()?;
    let height = br.read_ue()?;
    let conformance_window = br.read_bit()?;
    let (mut crop_right, mut crop_bottom) = (0u32, 0u32);
    if conformance_window {
        let _left = br.read_ue()?;
        crop_right = br.read_ue()?;
        let _top = br.read_ue()?;
        crop_bottom = br.read_ue()?;
    }
    // Sub-pictures add a variable-length block; this muxer does not need them
    // and the encoder backends it supports do not emit them.
    if br.read_bit()? {
        return None;
    }
    let bit_depth_luma = br.read_ue()? as u8 + 8;

    let sub_w = if chroma_format_idc == 1 { 2 } else { 1 };
    Some(VvcSpsInfo {
        sps_id,
        vps_id,
        chroma_format_idc,
        bit_depth_luma,
        width: width.saturating_sub(crop_right * sub_w),
        height: height.saturating_sub(crop_bottom * sub_w),
        coded_width: width,
        coded_height: height,
        ctu_log2: log2_ctu_size_minus5 + 5,
        profile_idc,
        tier_flag,
        level_idc,
        frame_only_constraint,
        multilayer_enabled,
        gci_bits,
        num_sub_profiles,
        sub_profile_idc,
    })
}

/// Choose the smallest VVC level whose `MaxLumaPs` covers `width * height`.
pub fn select_level_idc(width: u32, height: u32) -> u8 {
    let samples = u64::from(width) * u64::from(height);
    const LEVELS: [(u8, u64); 13] = [
        (16, 36_864),
        (32, 122_880),
        (35, 245_760),
        (48, 552_960),
        (51, 983_040),
        (64, 2_228_224),
        (67, 2_228_224),
        (80, 8_912_896),
        (83, 8_912_896),
        (86, 8_912_896),
        (96, 35_651_584),
        (99, 35_651_584),
        (102, 35_651_584),
    ];
    for (idc, max) in LEVELS {
        if samples <= max {
            return idc;
        }
    }
    102
}
