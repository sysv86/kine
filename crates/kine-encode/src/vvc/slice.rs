//! Slice-data syntax and transform-skip residual coding for the intra-only
//! VVC subset this encoder emits.
//!
//! The picture is coded as one slice, one CU per CTU (`CU == CTU`, so no
//! `split_cu_flag` ever appears — `min CB == CTU`), every CU intra PLANAR via
//! the MPM path, and every residual block transform-skipped.  The bin order is
//! exactly VTM's `CABACWriter` for this configuration:
//!
//! ```text
//! intra_luma_mpm_flag = 1, intra_luma_not_planar_flag = 0   (PLANAR)
//! intra_chroma_pred_mode = 0                                (DM == PLANAR)
//! cbf_cb, cbf_cr, cbf_luma
//! per component with cbf: transform_skip_flag = 1, residual_ts_coding()
//! ```

use super::cabac::{ctx, BinDecoder, BinEncoder, CtxStore};
use super::predict::{self, IntraMode, Plane};
use super::transform::{self, CoeffScan};
use crate::I420Frame;

/// Coded planes at full coded resolution (before conformance-window cropping).
pub struct Planes {
    pub y: Plane,
    pub u: Plane,
    pub v: Plane,
}

impl Planes {
    pub fn from_frame(frame: &I420Frame) -> Self {
        let cw = frame.width as usize;
        let ch = frame.height as usize;
        let y = Plane::from_slice(cw, ch, &frame.y);
        let ccw = cw.div_ceil(2);
        let cch = ch.div_ceil(2);
        let u = Plane::from_slice(ccw, cch, &frame.u);
        let v = Plane::from_slice(ccw, cch, &frame.v);
        Self { y, u, v }
    }

    pub fn into_frame(self) -> I420Frame {
        I420Frame {
            width: self.y.width as u32,
            height: self.y.height as u32,
            y: self.y.data,
            u: self.u.data,
            v: self.v.data,
        }
    }
}

/// Encode the slice data for one IDR picture (CABAC substream only).
///
/// `src` must already be padded to the coded picture size; `qp` is `SliceQpY`.
pub fn encode_picture(src: &I420Frame, qp: u8, ctu_log2: u8) -> Vec<u8> {
    let mut enc = BinEncoder::new();
    let mut ctx = CtxStore::new_i_slice(i32::from(qp));
    let src_planes = Planes::from_frame(src);
    let mut recon = Planes::from_frame(src);
    let ctu = 1usize << ctu_log2;
    let width = src.width as usize;
    let height = src.height as usize;
    let luma_qp = transform::ts_qp(qp);
    let chroma_qp =
        transform::ts_qp(super::hls::derived_chroma_qp(i32::from(qp)).clamp(0, 63) as u8);

    debug_assert_eq!(width % ctu, 0);
    debug_assert_eq!(height % ctu, 0);

    let mut ctu_y = 0usize;
    while ctu_y < height {
        let mut ctu_x = 0usize;
        while ctu_x < width {
            let cw = ctu / 2;
            let cx = ctu_x / 2;
            let cy = ctu_y / 2;

            let (levels_y, cbf_y) = encode_component(
                &mut recon.y,
                &src_planes.y,
                ctu_x,
                ctu_y,
                ctu,
                ctu,
                true,
                luma_qp,
            );
            let (levels_cb, cbf_cb) = encode_component(
                &mut recon.u,
                &src_planes.u,
                cx,
                cy,
                cw,
                cw,
                false,
                chroma_qp,
            );
            let (levels_cr, cbf_cr) = encode_component(
                &mut recon.v,
                &src_planes.v,
                cx,
                cy,
                cw,
                cw,
                false,
                chroma_qp,
            );

            // ---- cu_pred_data: one PU, PLANAR, chroma DM ----
            enc.encode_bin(true, &mut ctx.models[ctx::INTRA_LUMA_MPM_FLAG]);
            enc.encode_bin(false, &mut ctx.models[ctx::INTRA_LUMA_PLANAR_FLAG + 1]);
            enc.encode_bin(false, &mut ctx.models[ctx::INTRA_CHROMA_PRED_MODE]);

            // ---- transform_unit: chroma CBFs first, then luma ----
            enc.encode_bin(cbf_cb, &mut ctx.models[ctx::QT_CBF_CB]);
            enc.encode_bin(
                cbf_cr,
                &mut ctx.models[ctx::QT_CBF_CR + usize::from(cbf_cb)],
            );
            enc.encode_bin(cbf_y, &mut ctx.models[ctx::QT_CBF_Y]);

            // ---- coefficients, luma first ----
            if cbf_y {
                enc.encode_bin(true, &mut ctx.models[ctx::TRANSFORM_SKIP_FLAG]);
                let scan = CoeffScan::new(ctu, ctu);
                encode_ts_block(&mut enc, &mut ctx, &scan, &levels_y);
            }
            if cbf_cb {
                enc.encode_bin(true, &mut ctx.models[ctx::TRANSFORM_SKIP_FLAG + 1]);
                let scan = CoeffScan::new(cw, cw);
                encode_ts_block(&mut enc, &mut ctx, &scan, &levels_cb);
            }
            if cbf_cr {
                enc.encode_bin(true, &mut ctx.models[ctx::TRANSFORM_SKIP_FLAG + 1]);
                let scan = CoeffScan::new(cw, cw);
                encode_ts_block(&mut enc, &mut ctx, &scan, &levels_cr);
            }

            ctu_x += ctu;
        }
        ctu_y += ctu;
    }

    enc.encode_bin_trm(true);
    enc.finish_and_align()
}

/// Decode one IDR picture's CABAC substream back into planes.
pub fn decode_picture(
    cabac: &[u8],
    qp: u8,
    ctu_log2: u8,
    coded_width: u32,
    coded_height: u32,
) -> Result<I420Frame, String> {
    let mut dec = BinDecoder::new(cabac)?;
    let mut ctx = CtxStore::new_i_slice(i32::from(qp));
    let ctu = 1usize << ctu_log2;
    let width = coded_width as usize;
    let height = coded_height as usize;
    let cw = ctu / 2;
    let luma_qp = transform::ts_qp(qp);
    let chroma_qp =
        transform::ts_qp(super::hls::derived_chroma_qp(i32::from(qp)).clamp(0, 63) as u8);

    let mut recon = Planes {
        y: Plane::new(width, height),
        u: Plane::new(width.div_ceil(2), height.div_ceil(2)),
        v: Plane::new(width.div_ceil(2), height.div_ceil(2)),
    };

    let mut ctu_y = 0usize;
    while ctu_y < height {
        let mut ctu_x = 0usize;
        while ctu_x < width {
            let cx = ctu_x / 2;
            let cy = ctu_y / 2;

            // ---- cu_pred_data ----
            let mpm = dec.decode_bin(&mut ctx.models[ctx::INTRA_LUMA_MPM_FLAG]);
            if !mpm {
                return Err("unsupported VVC subset: non-MPM intra mode".into());
            }
            let not_planar = dec.decode_bin(&mut ctx.models[ctx::INTRA_LUMA_PLANAR_FLAG + 1]);
            if not_planar {
                return Err("unsupported VVC subset: non-planar intra mode".into());
            }
            let chroma_dm = !dec.decode_bin(&mut ctx.models[ctx::INTRA_CHROMA_PRED_MODE]);
            if !chroma_dm {
                return Err("unsupported VVC subset: non-DM chroma mode".into());
            }

            // ---- transform_unit CBFs ----
            let cbf_cb = dec.decode_bin(&mut ctx.models[ctx::QT_CBF_CB]);
            let cbf_cr = dec.decode_bin(&mut ctx.models[ctx::QT_CBF_CR + usize::from(cbf_cb)]);
            let cbf_y = dec.decode_bin(&mut ctx.models[ctx::QT_CBF_Y]);

            let mut levels_y = vec![0i32; ctu * ctu];
            if cbf_y {
                let ts = dec.decode_bin(&mut ctx.models[ctx::TRANSFORM_SKIP_FLAG]);
                if !ts {
                    return Err("unsupported VVC subset: DCT-2 residual".into());
                }
                let scan = CoeffScan::new(ctu, ctu);
                decode_ts_block(&mut dec, &mut ctx, &scan, &mut levels_y);
            }
            let mut levels_cb = vec![0i32; cw * cw];
            if cbf_cb {
                let ts = dec.decode_bin(&mut ctx.models[ctx::TRANSFORM_SKIP_FLAG + 1]);
                if !ts {
                    return Err("unsupported VVC subset: DCT-2 residual".into());
                }
                let scan = CoeffScan::new(cw, cw);
                decode_ts_block(&mut dec, &mut ctx, &scan, &mut levels_cb);
            }
            let mut levels_cr = vec![0i32; cw * cw];
            if cbf_cr {
                let ts = dec.decode_bin(&mut ctx.models[ctx::TRANSFORM_SKIP_FLAG + 1]);
                if !ts {
                    return Err("unsupported VVC subset: DCT-2 residual".into());
                }
                let scan = CoeffScan::new(cw, cw);
                decode_ts_block(&mut dec, &mut ctx, &scan, &mut levels_cr);
            }

            reconstruct_component(
                &mut recon.y,
                ctu_x,
                ctu_y,
                ctu,
                ctu,
                true,
                luma_qp,
                &levels_y,
            );
            reconstruct_component(&mut recon.u, cx, cy, cw, cw, false, chroma_qp, &levels_cb);
            reconstruct_component(&mut recon.v, cx, cy, cw, cw, false, chroma_qp, &levels_cr);

            ctu_x += ctu;
        }
        ctu_y += ctu;
    }

    if !dec.decode_bin_trm() {
        return Err("missing terminating bit at end of slice".into());
    }
    dec.finish()?;
    Ok(recon.into_frame())
}

/// Predict, quantise and reconstruct one component block.
/// Returns the quantised levels and whether the block is coded.
#[allow(clippy::too_many_arguments)]
fn encode_component(
    recon: &mut Plane,
    src: &Plane,
    bx: usize,
    by: usize,
    w: usize,
    h: usize,
    is_luma: bool,
    qp: u8,
) -> (Vec<i32>, bool) {
    let pred = predict::predict_block(recon, bx, by, w, h, is_luma, IntraMode::Planar);
    let mut levels = vec![0i32; w * h];
    let mut coded = false;
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let residual = i32::from(src.get(bx + x, by + y)) - pred[idx];
            let level = transform::quantize_ts(residual, qp);
            levels[idx] = level;
            coded |= level != 0;
            let value = pred[idx] + transform::dequantize_ts(level, qp);
            recon.set(bx + x, by + y, value.clamp(0, 255) as u8);
        }
    }
    (levels, coded)
}

#[allow(clippy::too_many_arguments)]
fn reconstruct_component(
    recon: &mut Plane,
    bx: usize,
    by: usize,
    w: usize,
    h: usize,
    is_luma: bool,
    qp: u8,
    levels: &[i32],
) {
    let pred = predict::predict_block(recon, bx, by, w, h, is_luma, IntraMode::Planar);
    for y in 0..h {
        for x in 0..w {
            let idx = y * w + x;
            let value = pred[idx] + transform::dequantize_ts(levels[idx], qp);
            recon.set(bx + x, by + y, value.clamp(0, 255) as u8);
        }
    }
}

// ---------------------------------------------------------------------------
// residual_ts_coding (H.266 7.3.11.11, transform-skip path)
// ---------------------------------------------------------------------------

#[inline]
fn neighbours(coeffs: &[i32], width: usize, x: usize, y: usize) -> (i32, i32) {
    let left = if x > 0 { coeffs[y * width + x - 1] } else { 0 };
    let above = if y > 0 {
        coeffs[(y - 1) * width + x]
    } else {
        0
    };
    (left, above)
}

#[inline]
fn num_nonzero_neighbours(left: i32, above: i32) -> usize {
    usize::from(left != 0) + usize::from(above != 0)
}

#[inline]
fn sign_context(left: i32, above: i32) -> usize {
    let right_sign = left.signum();
    let below_sign = above.signum();
    if (right_sign == 0 && below_sign == 0) || right_sign * below_sign < 0 {
        0
    } else if right_sign >= 0 && below_sign >= 0 {
        1
    } else {
        2
    }
}

/// `deriveModCoeff`: map a true magnitude to the coded "modified" domain.
#[inline]
fn derive_mod_coeff(left: i32, above: i32, abs_coeff: i32, identity: bool) -> i32 {
    if abs_coeff == 0 {
        return 0;
    }
    if identity {
        return abs_coeff;
    }
    let pred = left.abs().max(above.abs());
    if abs_coeff == pred {
        1
    } else if abs_coeff < pred {
        abs_coeff + 1
    } else {
        abs_coeff
    }
}

/// `decDeriveModCoeff`: inverse of [`derive_mod_coeff`].
#[inline]
fn dec_derive_mod_coeff(left: i32, above: i32, abs_coeff: i32) -> i32 {
    if abs_coeff == 0 {
        return 0;
    }
    let pred = left.abs().max(above.abs());
    if abs_coeff == 1 && pred > 0 {
        pred
    } else {
        abs_coeff - i32::from(abs_coeff <= pred)
    }
}

fn encode_ts_block(enc: &mut BinEncoder, ctx: &mut CtxStore, scan: &CoeffScan, coeffs: &[i32]) {
    let max_ctx_bins = (scan.max_num_coeff * 7) >> 2;
    let mut remaining = max_ctx_bins as i32;
    let last = scan.last_sub_set();
    debug_assert!(
        coeffs.iter().any(|&c| c != 0),
        "residual_ts_coding requires a coded (non-zero) block"
    );
    let mut group_flags = vec![false; scan.width_in_groups * scan.height_in_groups];
    let group_size = 1usize << scan.log2_cg_size;
    // `only1stSigGroup()` is evaluated at the last sub-block: it is true when
    // no earlier sub-block was coded.
    let mut any_coded_group = false;

    for sub_set in 0..=last {
        let min_sub = sub_set * group_size;
        let max_sub = min_sub + group_size - 1;
        let group_has = (min_sub..=max_sub).any(|sp| coeffs[scan.raster[sp]] != 0);
        let cg_raster = scan.cg_raster(sub_set);
        let cg_x = cg_raster % scan.width_in_groups;
        let cg_y = cg_raster / scan.width_in_groups;
        let sig_left = cg_x > 0 && group_flags[cg_raster - 1];
        let sig_above = cg_y > 0 && group_flags[cg_raster - scan.width_in_groups];
        group_flags[cg_raster] = group_has;

        let only_first = !any_coded_group;
        if sub_set != last || !only_first {
            let group_ctx =
                ctx::TS_SIG_COEFF_GROUP + usize::from(sig_left) + usize::from(sig_above);
            enc.encode_bin(group_has, &mut ctx.models[group_ctx]);
            if !group_has {
                continue;
            }
        }

        // ---- pass 1: significance, sign, gt1, parity ----
        // VTM's `inferSigPos` is `cctx.maxSubPos()`, i.e. the *last* scan
        // position of the sub-block, despite the local variable's name.
        let infer_sig_pos = max_sub;
        let mut num_nonzero = 0usize;
        let mut last_p1: i32 = -1;
        let mut sp = min_sub;
        while sp <= max_sub && remaining >= 4 {
            let blk = scan.raster[sp];
            let coeff = coeffs[blk];
            let sig = coeff != 0;
            if num_nonzero != 0 || sp != infer_sig_pos {
                let (x, y) = scan.xy[sp];
                let (left, above) = neighbours(coeffs, scan.width, x, y);
                let n = num_nonzero_neighbours(left, above);
                enc.encode_bin(sig, &mut ctx.models[ctx::TS_SIG_FLAG + n]);
                remaining -= 1;
            }
            if sig {
                let (x, y) = scan.xy[sp];
                let (left, above) = neighbours(coeffs, scan.width, x, y);
                let sign_ctx = sign_context(left, above);
                enc.encode_bin(coeff < 0, &mut ctx.models[ctx::TS_RESIDUAL_SIGN + sign_ctx]);
                remaining -= 1;
                num_nonzero += 1;

                let mod_abs = derive_mod_coeff(left, above, coeff.abs(), false);
                let rem_abs = mod_abs - 1;
                let gt1 = rem_abs != 0;
                let n = num_nonzero_neighbours(left, above);
                enc.encode_bin(gt1, &mut ctx.models[ctx::TS_LRG1_FLAG + n]);
                remaining -= 1;
                if gt1 {
                    enc.encode_bin((rem_abs - 1) & 1 != 0, &mut ctx.models[ctx::TS_PAR_FLAG]);
                    remaining -= 1;
                }
            }
            last_p1 = sp as i32;
            sp += 1;
        }

        // ---- pass 2: gt2..gt5 ----
        let mut last_p2: i32 = -1;
        let mut sp = min_sub;
        while sp <= max_sub && remaining >= 4 {
            let (x, y) = scan.xy[sp];
            let (left, above) = neighbours(coeffs, scan.width, x, y);
            let abs_level = derive_mod_coeff(left, above, coeffs[scan.raster[sp]].abs(), false);
            for i in 0..4 {
                let cutoff = 2 + 2 * i;
                if abs_level >= cutoff {
                    let gt = abs_level >= cutoff + 2;
                    enc.encode_bin(
                        gt,
                        &mut ctx.models[ctx::TS_GTX_FLAG + ((cutoff as usize) >> 1)],
                    );
                    remaining -= 1;
                }
            }
            last_p2 = sp as i32;
            sp += 1;
        }

        // ---- pass 3: Go-Rice remainder / escape ----
        for sp in min_sub..=max_sub {
            let cutoff = if sp as i32 <= last_p2 {
                10
            } else if sp as i32 <= last_p1 {
                2
            } else {
                0
            };
            let (x, y) = scan.xy[sp];
            let (left, above) = neighbours(coeffs, scan.width, x, y);
            let coeff = coeffs[scan.raster[sp]];
            let abs_level = derive_mod_coeff(left, above, coeff.abs(), cutoff == 0);
            if abs_level >= cutoff {
                let rem = if (sp as i32) <= last_p1 {
                    (abs_level - cutoff) >> 1
                } else {
                    abs_level
                };
                enc.encode_rem_abs_ep(rem as u32, 1, 5, 15);
                if abs_level != 0 && (sp as i32) > last_p1 {
                    enc.encode_bin_ep(coeff < 0);
                }
            }
        }

        if group_has {
            any_coded_group = true;
        }
    }
}

fn decode_ts_block(
    dec: &mut BinDecoder<'_>,
    ctx: &mut CtxStore,
    scan: &CoeffScan,
    coeffs: &mut [i32],
) {
    coeffs.fill(0);
    let max_ctx_bins = (scan.max_num_coeff * 7) >> 2;
    let mut remaining = max_ctx_bins as i32;
    let last = scan.last_sub_set();
    let mut group_flags = vec![false; scan.width_in_groups * scan.height_in_groups];
    let group_size = 1usize << scan.log2_cg_size;

    for sub_set in 0..=last {
        let min_sub = sub_set * group_size;
        let max_sub = min_sub + group_size - 1;
        let cg_raster = scan.cg_raster(sub_set);
        let cg_x = cg_raster % scan.width_in_groups;
        let cg_y = cg_raster / scan.width_in_groups;
        let sig_left = cg_x > 0 && group_flags[cg_raster - 1];
        let sig_above = cg_y > 0 && group_flags[cg_raster - scan.width_in_groups];

        let mut sig_group = sub_set == last && group_flags.iter().all(|&f| !f);
        if !sig_group {
            let group_ctx =
                ctx::TS_SIG_COEFF_GROUP + usize::from(sig_left) + usize::from(sig_above);
            sig_group = dec.decode_bin(&mut ctx.models[group_ctx]);
        }
        if !sig_group {
            group_flags[cg_raster] = false;
            continue;
        }
        group_flags[cg_raster] = true;

        // ---- pass 1 ----
        // VTM's `inferSigPos` is `cctx.maxSubPos()`, i.e. the *last* scan
        // position of the sub-block, despite the local variable's name.
        let infer_sig_pos = max_sub;
        let mut num_nonzero = 0usize;
        let mut last_p1: i32 = -1;
        let mut sign_pattern: u32 = 0;
        let mut sig_blk_pos: Vec<usize> = Vec::new();
        let mut sp = min_sub;
        while sp <= max_sub && remaining >= 4 {
            let blk = scan.raster[sp];
            let (x, y) = scan.xy[sp];
            let mut sig = num_nonzero == 0 && sp == infer_sig_pos;
            if !sig {
                let (left, above) = neighbours(coeffs, scan.width, x, y);
                let n = num_nonzero_neighbours(left, above);
                sig = dec.decode_bin(&mut ctx.models[ctx::TS_SIG_FLAG + n]);
                remaining -= 1;
            }
            if sig {
                let (left, above) = neighbours(coeffs, scan.width, x, y);
                let sign_ctx = sign_context(left, above);
                let sign = dec.decode_bin(&mut ctx.models[ctx::TS_RESIDUAL_SIGN + sign_ctx]);
                remaining -= 1;
                sign_pattern |= u32::from(sign) << num_nonzero;
                sig_blk_pos.push(blk);
                num_nonzero += 1;

                let (left, above) = neighbours(coeffs, scan.width, x, y);
                let n = num_nonzero_neighbours(left, above);
                let gt1 = dec.decode_bin(&mut ctx.models[ctx::TS_LRG1_FLAG + n]);
                remaining -= 1;
                let par = if gt1 {
                    remaining -= 1;
                    dec.decode_bin(&mut ctx.models[ctx::TS_PAR_FLAG])
                } else {
                    false
                };
                coeffs[blk] = (1 + i32::from(gt1) + i32::from(par)) * if sign { -1 } else { 1 };
            }
            last_p1 = sp as i32;
            sp += 1;
        }

        // ---- pass 2 ----
        let mut last_p2: i32 = -1;
        let mut sp = min_sub;
        while sp <= max_sub && remaining >= 4 {
            let blk = scan.raster[sp];
            let mut abs_level = coeffs[blk].abs();
            for i in 0..4 {
                let cutoff = 2 + 2 * i;
                if abs_level >= cutoff {
                    let gt = dec
                        .decode_bin(&mut ctx.models[ctx::TS_GTX_FLAG + ((cutoff as usize) >> 1)]);
                    abs_level += i32::from(gt) << 1;
                    remaining -= 1;
                }
            }
            coeffs[blk] = abs_level;
            last_p2 = sp as i32;
            sp += 1;
        }

        // ---- pass 3 ----
        for sp in min_sub..=max_sub {
            let blk = scan.raster[sp];
            let (x, y) = scan.xy[sp];
            let cutoff = if sp as i32 <= last_p2 {
                10
            } else if sp as i32 <= last_p1 {
                2
            } else {
                0
            };
            let mut abs_level = coeffs[blk].abs();
            if abs_level >= cutoff {
                let rem = dec.decode_rem_abs_ep(1, 5, 15);
                abs_level += if (sp as i32) <= last_p1 {
                    (rem as i32) << 1
                } else {
                    rem as i32
                };
                if abs_level != 0 && (sp as i32) > last_p1 {
                    let sign = dec.decode_bin_ep();
                    sign_pattern |= u32::from(sign) << num_nonzero;
                    sig_blk_pos.push(blk);
                    num_nonzero += 1;
                }
            }
            if cutoff != 0 && abs_level > 0 {
                let (left, above) = neighbours(coeffs, scan.width, x, y);
                abs_level = dec_derive_mod_coeff(left, above, abs_level);
            }
            coeffs[blk] = abs_level;
        }

        // ---- apply signs ----
        for (k, &blk) in sig_blk_pos.iter().enumerate() {
            let abs = coeffs[blk];
            coeffs[blk] = if (sign_pattern >> k) & 1 != 0 {
                -abs
            } else {
                abs
            };
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vvc::cabac::BinEncoder;

    fn lcg(seed: &mut u32) -> i32 {
        *seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
        ((*seed >> 8) % 61) as i32 - 30
    }

    #[test]
    fn ts_block_inferred_significance_positions_round_trip() {
        // Exercise both inferences: `sb_coded_flag` of the last group and the
        // significance flag of the last scan position inside a coded group.
        let cases: [&[(usize, usize)]; 4] = [
            &[(31, 31)],
            &[(0, 0)],
            &[(3, 3)],
            &[(30, 31), (31, 30), (31, 31)],
        ];
        for size in [16usize, 32] {
            for case in cases {
                let mut coeffs = vec![0i32; size * size];
                for &(x, y) in case {
                    let x = x.min(size - 1);
                    let y = y.min(size - 1);
                    coeffs[y * size + x] = if (x + y) % 2 == 0 { 7 } else { -3 };
                }
                let scan = CoeffScan::new(size, size);
                let mut enc = BinEncoder::new();
                let mut ctx = CtxStore::new_i_slice(26);
                encode_ts_block(&mut enc, &mut ctx, &scan, &coeffs);
                enc.encode_bin_trm(true);
                let bytes = enc.finish_and_align();
                let mut dec = BinDecoder::new(&bytes).unwrap();
                let mut dctx = CtxStore::new_i_slice(26);
                let mut out = vec![0i32; size * size];
                decode_ts_block(&mut dec, &mut dctx, &scan, &mut out);
                assert!(dec.decode_bin_trm());
                assert_eq!(
                    out, coeffs,
                    "inferred TS round-trip size={size} case={case:?}"
                );
            }
        }
    }

    #[test]
    fn ts_block_round_trips_coefficients() {
        for &size in &[16usize, 32] {
            for qp in [4u8, 26, 40] {
                let mut seed = 0x1234_5678u32 ^ (size as u32);
                let mut coeffs = vec![0i32; size * size];
                for c in coeffs.iter_mut() {
                    let v = lcg(&mut seed);
                    *c = if v.abs() < 6 { 0 } else { v };
                }
                // A dense block exercises the context-bin budget as well.
                let scan = CoeffScan::new(size, size);
                let mut enc = BinEncoder::new();
                let mut ctx = CtxStore::new_i_slice(qp as i32);
                encode_ts_block(&mut enc, &mut ctx, &scan, &coeffs);
                enc.encode_bin_trm(true);
                let bytes = enc.finish_and_align();

                let mut dec = BinDecoder::new(&bytes).unwrap();
                let mut dctx = CtxStore::new_i_slice(qp as i32);
                let mut out = vec![0i32; size * size];
                decode_ts_block(&mut dec, &mut dctx, &scan, &mut out);
                assert!(dec.decode_bin_trm());
                assert_eq!(out, coeffs, "TS round-trip size={size} qp={qp}");
            }
        }
    }
}

#[cfg(test)]
mod picture_tests {
    use super::*;

    fn gradient(width: u32, height: u32) -> I420Frame {
        let cw = width.div_ceil(2) as usize;
        let ch = height.div_ceil(2) as usize;
        let mut y = vec![0u8; (width * height) as usize];
        for j in 0..height as usize {
            for i in 0..width as usize {
                y[j * width as usize + i] = ((i * 3 + j * 5) % 251) as u8;
            }
        }
        let u = vec![90u8; cw * ch];
        let v = vec![160u8; cw * ch];
        I420Frame {
            width,
            height,
            y,
            u,
            v,
        }
    }

    #[test]
    fn picture_round_trip_is_exact_at_qp4() {
        let (w, h) = (64u32, 64u32);
        let frame = gradient(w, h);
        let cabac = encode_picture(&frame, 4, 5);
        let dec = decode_picture(&cabac, 4, 5, w, h).expect("decode");
        let diffs: Vec<usize> = dec
            .y
            .iter()
            .zip(frame.y.iter())
            .enumerate()
            .filter(|(_, (a, b))| a != b)
            .map(|(i, _)| i)
            .collect();
        if !diffs.is_empty() {
            let i = diffs[0];
            panic!(
                "{} luma mismatches, first at ({}, {}): decoded {} expected {}; next: {:?}",
                diffs.len(),
                i % w as usize,
                i / w as usize,
                dec.y[i],
                frame.y[i],
                diffs
                    .iter()
                    .take(8)
                    .map(|&j| (j % w as usize, j / w as usize))
                    .collect::<Vec<_>>()
            );
        }
        assert_eq!(dec.u, frame.u);
        assert_eq!(dec.v, frame.v);
    }
}
