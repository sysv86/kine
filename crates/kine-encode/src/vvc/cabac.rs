//! Bit-exact VVC CABAC engine (H.266 clause 9.3), ported from VTM 24.0.
//!
//! VVC probabilities are two sub-states (`m_state[0]` 10-bit, `m_state[1]`
//! 14-bit) updated with per-context window sizes, not the 64-state HEVC table.
//! The encoder's carry propagation (`writeOut`) is reproduced exactly: a `0xff`
//! lead byte is buffered because a later carry can turn it into `0x00`.

use super::bits::BitWriter;

/// Number of context models in VVC (`NumberOfContexts` in VTM).
pub const NUM_CONTEXTS: usize = 379;

/// `m_RenormTable_32`: LPS renormalisation bits indexed by `lps_range >> 3`.
const RENORM_LPS: [u8; 32] = [
    6, 5, 4, 4, 3, 3, 3, 3, 2, 2, 2, 2, 2, 2, 2, 2, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1,
];

const MASK_0: u32 = 0x7FE0; // keeps bits 15..5
const MASK_1: u32 = 0x7FFE; // keeps bits 15..1

/// One adaptive context model (`BinProbModel_Std`).
#[derive(Debug, Clone, Copy)]
pub struct CtxModel {
    state0: u16,
    state1: u16,
    rate0: u8,
    rate1: u8,
}

impl CtxModel {
    /// Initialise from `initId`, the slice QP and the context's log2 window size.
    pub fn init(qp: i32, init_id: u8, log2_window: u8) -> Self {
        let slope = i32::from(init_id >> 3) - 4;
        let offset = i32::from(init_id & 7) * 18 + 1;
        let inistate = ((slope * (qp - 16)) >> 1) + offset;
        let state = inistate.clamp(1, 127) as u16;
        let p1 = state << 8;
        let rate0 = 2 + ((log2_window >> 2) & 3);
        let rate1 = 3 + rate0 + (log2_window & 3);
        Self {
            state0: (u32::from(p1) & MASK_0) as u16,
            state1: (u32::from(p1) & MASK_1) as u16,
            rate0,
            rate1,
        }
    }

    #[inline]
    fn state(&self) -> u8 {
        ((self.state0 + self.state1) >> 8) as u8
    }

    #[inline]
    fn mps(&self) -> bool {
        self.state() >= 128
    }

    #[inline]
    fn get_lps(&self, range: u32) -> u32 {
        let mut q = self.state();
        if q & 0x80 != 0 {
            q ^= 0xFF;
        }
        (((u32::from(q) >> 2) * (range >> 5)) >> 1) + 4
    }

    #[inline]
    fn update(&mut self, bin: bool) {
        let s0 = u32::from(self.state0);
        let s1 = u32::from(self.state1);
        let r0 = u32::from(self.rate0);
        let r1 = u32::from(self.rate1);
        let mut n0 = s0 - ((s0 >> r0) & MASK_0);
        let mut n1 = s1 - ((s1 >> r1) & MASK_1);
        if bin {
            n0 += (0x7FFF >> r0) & MASK_0;
            n1 += (0x7FFF >> r1) & MASK_1;
        }
        self.state0 = (n0 & 0xFFFF) as u16;
        self.state1 = (n1 & 0xFFFF) as u16;
    }
}

/// All context models of one slice, initialised for an I slice at `qp`.
#[derive(Debug, Clone)]
pub struct CtxStore {
    pub models: [CtxModel; NUM_CONTEXTS],
}

impl CtxStore {
    pub fn new_i_slice(qp: i32) -> Self {
        let qp = qp.clamp(0, 63);
        let mut models = [CtxModel::init(qp, 35, 8); NUM_CONTEXTS];
        for (id, model) in models.iter_mut().enumerate() {
            let (init_id, ws) = i_slice_context_init(id);
            *model = CtxModel::init(qp, init_id, ws);
        }
        Self { models }
    }
}

/// `(initId, log2WindowSize)` for the I-slice row of every context this codec
/// touches.  Unused contexts get the `CNU` sentinel and never affect output.
fn i_slice_context_init(ctx: usize) -> (u8, u8) {
    const CNU: u8 = 35;
    const DWS: u8 = 8;
    match ctx {
        // IntraLumaMpmFlag
        40 => (45, 6),
        // IntraLumaPlanarFlag
        41 => (13, 1),
        42 => (28, 5),
        // IntraChromaPredMode
        45 => (34, 5),
        // QtCbf[0] (luma)
        77 => (15, 5),
        78 => (12, 1),
        79 => (5, 8),
        80 => (7, 9),
        // QtCbf[1] (Cb)
        81 => (12, 5),
        82 => (21, 0),
        // QtCbf[2] (Cr)
        83 => (33, 2),
        84 => (28, 1),
        85 => (36, 0),
        // TransformSkipFlag (luma, chroma)
        310 => (25, 1),
        311 => (9, 1),
        // TsSigCoeffGroup
        357 => (18, 5),
        358 => (20, 8),
        359 => (38, 8),
        // TsSigFlag
        360 => (25, 13),
        361 => (28, 13),
        362 => (38, 8),
        // TsParFlag
        363 => (11, 6),
        // TsGtxFlag (index 0 is CNU and never used)
        364 => (CNU, DWS),
        365 => (10, 1),
        366 => (3, 1),
        367 => (3, 1),
        368 => (3, 1),
        // TsLrg1Flag
        369 => (11, 4),
        370 => (5, 2),
        371 => (5, 1),
        372 => (14, 6),
        // TsResidualSign
        373 => (12, 1),
        374 => (17, 4),
        375 => (46, 4),
        376 => (28, 5),
        377 => (25, 8),
        378 => (46, 8),
        _ => (CNU, DWS),
    }
}

/// Context id offsets for the syntax elements this encoder uses.
pub mod ctx {
    pub const INTRA_LUMA_MPM_FLAG: usize = 40;
    pub const INTRA_LUMA_PLANAR_FLAG: usize = 41;
    pub const INTRA_CHROMA_PRED_MODE: usize = 45;
    pub const QT_CBF_Y: usize = 77;
    pub const QT_CBF_CB: usize = 81;
    pub const QT_CBF_CR: usize = 83;
    pub const TRANSFORM_SKIP_FLAG: usize = 310;
    pub const TS_SIG_COEFF_GROUP: usize = 357;
    pub const TS_SIG_FLAG: usize = 360;
    pub const TS_PAR_FLAG: usize = 363;
    pub const TS_GTX_FLAG: usize = 364;
    pub const TS_LRG1_FLAG: usize = 369;
    pub const TS_RESIDUAL_SIGN: usize = 373;
}

/// VVC arithmetic encoder.
pub struct BinEncoder {
    low: u32,
    range: u32,
    buffered_byte: u8,
    num_buffered_bytes: u32,
    bits_left: i32,
    out: BitWriter,
}

impl Default for BinEncoder {
    fn default() -> Self {
        Self::new()
    }
}

impl BinEncoder {
    pub fn new() -> Self {
        Self {
            low: 0,
            range: 510,
            buffered_byte: 0xFF,
            num_buffered_bytes: 0,
            bits_left: 23,
            out: BitWriter::with_capacity(1024),
        }
    }

    /// Flush the arithmetic coder and append `byte_alignment()`.
    ///
    /// Must be called exactly once, after `encode_bin_trm(true)`.
    pub fn finish_and_align(mut self) -> Vec<u8> {
        self.finish();
        self.out.byte_alignment();
        self.out.into_bytes()
    }

    #[inline]
    pub fn encode_bin(&mut self, bin: bool, ctx: &mut CtxModel) {
        let lps_range = ctx.get_lps(self.range);
        let mps = ctx.mps();
        self.range -= lps_range;
        if bin != mps {
            let num_bits = RENORM_LPS[(lps_range >> 3) as usize];
            self.bits_left -= i32::from(num_bits);
            self.low = self.low.wrapping_add(self.range);
            self.low <<= num_bits;
            self.range = lps_range << num_bits;
            if self.bits_left < 12 {
                self.write_out();
            }
        } else if self.range < 256 {
            let num_bits = 1;
            self.bits_left -= num_bits;
            self.low <<= num_bits;
            self.range <<= num_bits;
            if self.bits_left < 12 {
                self.write_out();
            }
        }
        ctx.update(bin);
    }

    #[inline]
    pub fn encode_bin_ep(&mut self, bin: bool) {
        self.low <<= 1;
        if bin {
            self.low = self.low.wrapping_add(self.range);
        }
        self.bits_left -= 1;
        if self.bits_left < 12 {
            self.write_out();
        }
    }

    /// Equiprobable bins, MSB of `bins` first (H.266 `encode_bins_ep`).
    pub fn encode_bins_ep(&mut self, bins: u32, num_bins: u32) {
        if num_bins == 0 {
            return;
        }
        if self.range == 256 {
            self.encode_aligned_bins_ep(bins, num_bins);
            return;
        }
        let mut rem = num_bins;
        let mut bins = bins;
        while rem > 8 {
            rem -= 8;
            let pattern = (bins >> rem) & 0xFF;
            self.low <<= 8;
            self.low = self.low.wrapping_add(self.range.wrapping_mul(pattern));
            bins -= pattern << rem;
            self.bits_left -= 8;
            if self.bits_left < 12 {
                self.write_out();
            }
        }
        self.low <<= rem;
        self.low = self.low.wrapping_add(self.range.wrapping_mul(bins));
        self.bits_left -= rem as i32;
        if self.bits_left < 12 {
            self.write_out();
        }
    }

    fn encode_aligned_bins_ep(&mut self, bins: u32, num_bins: u32) {
        let mut rem = num_bins;
        while rem > 0 {
            let n = rem.min(8);
            let mask = (1u32 << n) - 1;
            let new_bins = (bins >> (rem - n)) & mask;
            self.low = (self.low << n) + (new_bins << 8);
            rem -= n;
            self.bits_left -= n as i32;
            if self.bits_left < 12 {
                self.write_out();
            }
        }
    }

    pub fn encode_bin_trm(&mut self, bin: bool) {
        self.range -= 2;
        if bin {
            self.low = self.low.wrapping_add(self.range);
            self.low <<= 7;
            self.range = 2 << 7;
            self.bits_left -= 7;
        } else if self.range >= 256 {
            return;
        } else {
            self.low <<= 1;
            self.range <<= 1;
            self.bits_left -= 1;
        }
        if self.bits_left < 12 {
            self.write_out();
        }
    }

    /// `encode_rem_abs_ep` (H.266 9.3.4.2.3): truncated Rice plus Escape.
    pub fn encode_rem_abs_ep(&mut self, bins: u32, k: u32, cutoff: u32, max_log2_tr: u32) {
        let threshold = cutoff << k;
        if bins < threshold {
            let bit_mask = (1u32 << k) - 1;
            let length = (bins >> k) + 1;
            self.encode_bins_ep((1u32 << length) - 2, length);
            self.encode_bins_ep(bins & bit_mask, k);
        } else {
            let max_prefix_length = 32 - cutoff - max_log2_tr;
            let code_value = (bins >> k) - cutoff;
            let (prefix_length, suffix_length);
            if code_value >= (1u32 << max_prefix_length) - 1 {
                prefix_length = max_prefix_length;
                suffix_length = max_log2_tr;
            } else {
                let mut pl = 0u32;
                while code_value > ((2u32 << pl) - 2) {
                    pl += 1;
                }
                prefix_length = pl;
                suffix_length = pl + k + 1;
            }
            let total_prefix_length = prefix_length + cutoff;
            let bit_mask = (1u32 << k) - 1;
            let prefix = (1u32 << total_prefix_length) - 1;
            let suffix = ((code_value - ((1u32 << prefix_length) - 1)) << k) | (bins & bit_mask);
            self.encode_bins_ep(prefix, total_prefix_length);
            self.encode_bins_ep(suffix, suffix_length);
        }
    }

    fn write_out(&mut self) {
        let lead_byte = self.low >> (24 - self.bits_left);
        self.bits_left += 8;
        let keep = if self.bits_left >= 32 {
            0
        } else {
            0xFFFF_FFFFu32 >> self.bits_left
        };
        self.low &= keep;
        if lead_byte == 0xFF {
            self.num_buffered_bytes += 1;
        } else if self.num_buffered_bytes > 0 {
            let carry = lead_byte >> 8;
            let byte = u32::from(self.buffered_byte) + carry;
            self.buffered_byte = (lead_byte & 0xFF) as u8;
            self.out.write_bits(byte, 8);
            let fill = (0xFF + carry) & 0xFF;
            while self.num_buffered_bytes > 1 {
                self.out.write_bits(fill, 8);
                self.num_buffered_bytes -= 1;
            }
        } else {
            self.num_buffered_bytes = 1;
            self.buffered_byte = lead_byte as u8;
        }
    }

    fn finish(&mut self) {
        if self.bits_left < 32 && (self.low >> (32 - self.bits_left)) != 0 {
            self.out.write_bits(u32::from(self.buffered_byte) + 1, 8);
            while self.num_buffered_bytes > 1 {
                self.out.write_bits(0x00, 8);
                self.num_buffered_bytes -= 1;
            }
            self.low -= 1u32 << (32 - self.bits_left);
        } else {
            if self.num_buffered_bytes > 0 {
                self.out.write_bits(u32::from(self.buffered_byte), 8);
            }
            while self.num_buffered_bytes > 1 {
                self.out.write_bits(0xFF, 8);
                self.num_buffered_bytes -= 1;
            }
        }
        let n = (24 - self.bits_left) as u32;
        self.out.write_bits(self.low >> 8, n);
    }
}

/// VVC arithmetic decoder (subset used for round-trip tests and the
/// intra-only subset decoder).
pub struct BinDecoder<'a> {
    data: &'a [u8],
    pos: usize,
    range: u32,
    value: u32,
    bits_needed: i32,
    last_byte: u8,
}

impl<'a> BinDecoder<'a> {
    pub fn new(data: &'a [u8]) -> Result<Self, &'static str> {
        if data.len() < 2 {
            return Err("CABAC substream is shorter than 2 bytes");
        }
        Ok(Self {
            data,
            pos: 2,
            range: 510,
            value: (u32::from(data[0]) << 8) + u32::from(data[1]),
            bits_needed: -8,
            last_byte: data[1],
        })
    }

    #[inline]
    fn read_byte(&mut self) -> u32 {
        if self.pos < self.data.len() {
            let b = self.data[self.pos];
            self.pos += 1;
            self.last_byte = b;
            u32::from(b)
        } else {
            0
        }
    }

    #[inline]
    pub fn decode_bin(&mut self, ctx: &mut CtxModel) -> bool {
        let mps = ctx.mps();
        let lps_range = ctx.get_lps(self.range);
        self.range -= lps_range;
        let scaled_range = self.range << 7;
        let bin;
        if self.value < scaled_range {
            bin = mps;
            if self.range < 256 {
                self.range <<= 1;
                self.value <<= 1;
                self.bits_needed += 1;
                if self.bits_needed >= 0 {
                    self.value += self.read_byte() << (self.bits_needed as u32);
                    self.bits_needed -= 8;
                }
            }
        } else {
            bin = !mps;
            let num_bits = i32::from(RENORM_LPS[(lps_range >> 3) as usize]);
            self.value -= scaled_range;
            self.value <<= num_bits as u32;
            self.range = lps_range << num_bits;
            self.bits_needed += num_bits;
            if self.bits_needed >= 0 {
                self.value += self.read_byte() << (self.bits_needed as u32);
                self.bits_needed -= 8;
            }
        }
        ctx.update(bin);
        bin
    }

    #[inline]
    pub fn decode_bin_ep(&mut self) -> bool {
        self.value <<= 1;
        self.bits_needed += 1;
        if self.bits_needed >= 0 {
            self.value += self.read_byte();
            self.bits_needed = -8;
        }
        let scaled_range = self.range << 7;
        if self.value >= scaled_range {
            self.value -= scaled_range;
            true
        } else {
            false
        }
    }

    pub fn decode_bins_ep(&mut self, num_bins: u32) -> u32 {
        let mut bins = 0u32;
        for _ in 0..num_bins {
            bins = (bins << 1) | u32::from(self.decode_bin_ep());
        }
        bins
    }

    pub fn decode_bin_trm(&mut self) -> bool {
        self.range -= 2;
        let scaled_range = self.range << 7;
        if self.value >= scaled_range {
            true
        } else {
            if self.range < 256 {
                self.range <<= 1;
                self.value <<= 1;
                self.bits_needed += 1;
                if self.bits_needed == 0 {
                    self.value += self.read_byte();
                    self.bits_needed = -8;
                }
            }
            false
        }
    }

    /// Verify the stop/alignment pattern required by VTM's `BinDecoder::finish`.
    pub fn finish(&self) -> Result<(), &'static str> {
        let shift = 8 + self.bits_needed;
        if ((u32::from(self.last_byte) << shift) & 0xFF) != 0x80 {
            return Err("no proper stop/alignment pattern at end of CABAC stream");
        }
        Ok(())
    }

    pub fn decode_rem_abs_ep(&mut self, go_rice_param: u32, cutoff: u32, max_log2_tr: u32) -> u32 {
        let max_prefix = 32 - max_log2_tr;
        let mut prefix = 0u32;
        let mut code_word;
        loop {
            prefix += 1;
            code_word = u32::from(self.decode_bin_ep());
            if !(code_word != 0 && prefix < max_prefix) {
                break;
            }
        }
        prefix -= 1 - code_word;

        let mut length = go_rice_param;
        let offset;
        if prefix < cutoff {
            offset = prefix << go_rice_param;
        } else {
            offset = ((1u32 << (prefix - cutoff)) + cutoff - 1) << go_rice_param;
            length += if prefix == max_prefix {
                max_log2_tr - go_rice_param
            } else {
                prefix - cutoff
            };
        }
        offset + self.decode_bins_ep(length)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn model() -> CtxModel {
        CtxModel::init(26, 25, 13)
    }

    #[test]
    fn round_trips_context_coded_and_bypass_bins() {
        let bins: Vec<bool> = (0..500).map(|i| ((i * 37 + i / 7) % 11) < 5).collect();
        let mut enc = BinEncoder::new();
        let mut enc_ctx = model();
        for &b in &bins {
            enc.encode_bin(b, &mut enc_ctx);
        }
        for &b in &bins[..100] {
            enc.encode_bin_ep(b);
        }
        enc.encode_bin_trm(true);
        let bytes = enc.finish_and_align();

        let mut dec = BinDecoder::new(&bytes).unwrap();
        let mut dec_ctx = model();
        for (i, &b) in bins.iter().enumerate() {
            assert_eq!(dec.decode_bin(&mut dec_ctx), b, "ctx bin {i}");
        }
        for &b in &bins[..100] {
            assert_eq!(dec.decode_bin_ep(), b);
        }
        assert!(dec.decode_bin_trm());
        dec.finish().unwrap();
    }

    #[test]
    fn round_trips_remainders() {
        for &value in &[0u32, 1, 7, 9, 10, 42, 1000, 40967] {
            let mut enc = BinEncoder::new();
            enc.encode_rem_abs_ep(value, 1, 5, 15);
            enc.encode_bin_trm(true);
            let bytes = enc.finish_and_align();
            let mut dec = BinDecoder::new(&bytes).unwrap();
            assert_eq!(dec.decode_rem_abs_ep(1, 5, 15), value);
            assert!(dec.decode_bin_trm());
        }
    }

    #[test]
    fn initialises_every_i_slice_context() {
        let store = CtxStore::new_i_slice(26);
        // Just make sure the table does not panic and known entries differ.
        assert_eq!(store.models.len(), NUM_CONTEXTS);
    }
}
