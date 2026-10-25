//! Transform-skip coefficient helpers: sub-block scan order, quantisation and
//! the normative inverse quantisation used by the decoder.
//!
//! All residual blocks in this encoder use `transform_skip_flag = 1`, so there
//! is no forward/inverse DCT anywhere.  The residual is quantised directly in
//! the sample domain and the decoder reconstructs `Clip3(pred + dequant(level))`.

/// Grouped diagonal scan over a square transform block (VTM `g_scanOrder`
/// `SCAN_GROUPED_4x4` with `DIAG`).  For every block size >= 4x4 the coefficient
/// groups are 4x4.
#[derive(Debug, Clone)]
pub struct CoeffScan {
    pub width: usize,
    pub height: usize,
    pub log2_cg_size: u32,
    pub width_in_groups: usize,
    pub height_in_groups: usize,
    pub max_num_coeff: usize,
    /// Raster index for every scan position.
    pub raster: Vec<usize>,
    /// `(x, y)` sample index for every scan position.
    pub xy: Vec<(usize, usize)>,
    /// `(x, y)` position of every coefficient group in the group grid.
    pub group_xy: Vec<(usize, usize)>,
}

impl CoeffScan {
    pub fn new(width: usize, height: usize) -> Self {
        assert!(width.is_power_of_two() && height.is_power_of_two());
        // 4:2:0 chroma of an 8x8 luma block is 4x4 at the smallest; this
        // encoder never produces smaller blocks.
        let log2_cg_w = if width >= 4 {
            2
        } else {
            width.trailing_zeros()
        };
        let log2_cg_h = if height >= 4 {
            2
        } else {
            height.trailing_zeros()
        };
        let cg_w = 1usize << log2_cg_w;
        let cg_h = 1usize << log2_cg_h;
        let capped_w = width.min(32);
        let capped_h = height.min(32);
        let width_in_groups = capped_w >> log2_cg_w;
        let height_in_groups = capped_h >> log2_cg_h;

        let group_order = diagonal_scan(width_in_groups, height_in_groups);
        let inner = diagonal_scan(cg_w, cg_h);
        let group_xy = group_order.clone();
        let mut raster = Vec::with_capacity(width * height);
        let mut xy = Vec::with_capacity(width * height);
        for (gx, gy) in group_order {
            for &(ix, iy) in &inner {
                let x = gx * cg_w + ix;
                let y = gy * cg_h + iy;
                raster.push(y * width + x);
                xy.push((x, y));
            }
        }
        debug_assert_eq!(raster.len(), width * height);
        Self {
            width,
            height,
            log2_cg_size: log2_cg_w + log2_cg_h,
            width_in_groups,
            height_in_groups,
            max_num_coeff: width * height,
            raster,
            xy,
            group_xy,
        }
    }

    /// Group index (`scan_pos >> log2_cg_size`) of every scan position.
    #[inline]
    pub fn cg_of(&self, scan_pos: usize) -> usize {
        scan_pos >> self.log2_cg_size
    }

    pub fn last_sub_set(&self) -> usize {
        (self.max_num_coeff - 1) >> self.log2_cg_size
    }

    /// Raster index of a coefficient group in the group grid
    /// (VTM `m_scanCG[sub_set].idx`).
    #[inline]
    pub fn cg_raster(&self, sub_set: usize) -> usize {
        let (x, y) = self.group_xy[sub_set];
        y * self.width_in_groups + x
    }
}

/// VTM `ScanGenerator` with `CoeffScanType::DIAG`, returning `(x, y)`.
fn diagonal_scan(w: usize, h: usize) -> Vec<(usize, usize)> {
    let mut out = Vec::with_capacity(w * h);
    if w == 0 || h == 0 {
        return out;
    }
    let (mut line, mut column) = (0usize, 0usize);
    for _ in 0..w * h {
        out.push((column, line));
        if column == w - 1 || line == 0 {
            line += column + 1;
            column = 0;
            if line >= h {
                column += line - (h - 1);
                line = h - 1;
            }
        } else {
            column += 1;
            line -= 1;
        }
    }
    out
}

/// Forward quantisation for a transform-skip residual sample
/// (VTM `Quant::quant`, transform-skip path, 8-bit, IRAP).
pub fn quantize_ts(residual: i32, qp: u8) -> i32 {
    let qp = u32::from(qp.max(4));
    let qp_per = qp / 6;
    let qp_rem = qp % 6;
    const QUANT_SCALES: [i32; 6] = [26214, 23302, 20560, 18396, 16384, 14564];
    let i_qbits = 14 + qp_per;
    let scale = QUANT_SCALES[qp_rem as usize];
    let i_add = 171i64 << (i_qbits - 9); // IRAP
    let sign = residual.signum();
    let level = ((i64::from(residual.abs()) * i64::from(scale) + i_add) >> i_qbits) as i32;
    (sign * level).clamp(-32768, 32767)
}

/// Normative inverse quantisation for a transform-skip level
/// (VTM `Quant::dequant`, scaling lists off).
///
/// `rightShift = 6 - QP/6` becomes negative above QP 41; the left shift is
/// intentional and the result is clipped to the 15-bit transform dynamic range.
pub fn dequantize_ts(level: i32, qp: u8) -> i32 {
    let qp = u32::from(qp.max(4));
    let qp_per = (qp / 6) as i32;
    let qp_rem = qp % 6;
    const INV_QUANT_SCALES: [i32; 6] = [40, 45, 51, 57, 64, 72];
    let scale = INV_QUANT_SCALES[qp_rem as usize];
    let right_shift = 6 - qp_per;
    let value = i64::from(level) * i64::from(scale);
    let scaled = if right_shift > 0 {
        (value + (1i64 << (right_shift - 1))) >> right_shift
    } else {
        value << (-right_shift)
    };
    (scaled as i32).clamp(-32768, 32767)
}

/// `QP_TS = max(QP, 4 + 6 * internalMinusInputBitDepth)` for 8-bit.
#[inline]
pub fn ts_qp(qp: u8) -> u8 {
    qp.max(4)
}

/// `sgn()` used by the transform-skip sign context.
#[inline]
pub fn sgn(value: i32) -> i32 {
    value.signum()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn diagonal_scan_is_anti_diagonal_with_x_increasing() {
        let scan = diagonal_scan(4, 4);
        assert_eq!(
            scan,
            vec![
                (0, 0),
                (0, 1),
                (1, 0),
                (0, 2),
                (1, 1),
                (2, 0),
                (0, 3),
                (1, 2),
                (2, 1),
                (3, 0),
                (1, 3),
                (2, 2),
                (3, 1),
                (2, 3),
                (3, 2),
                (3, 3),
            ]
        );
    }

    #[test]
    fn grouped_scan_covers_every_coefficient_once() {
        for size in [8usize, 16, 32] {
            let cs = CoeffScan::new(size, size);
            let mut seen = vec![false; size * size];
            for &idx in &cs.raster {
                assert!(!seen[idx]);
                seen[idx] = true;
            }
            assert!(seen.iter().all(|&v| v));
        }
    }

    #[test]
    fn qp_four_is_lossless_for_transform_skip() {
        for value in [-200i32, -1, 0, 1, 37, 200] {
            let level = quantize_ts(value, 4);
            assert_eq!(level, value);
            assert_eq!(dequantize_ts(level, 4), value);
        }
    }

    #[test]
    fn dequant_matches_reference_table() {
        // From the VTM derivation: QP 22 -> qpPer 3, qpRem 4, scale 64, shift 3.
        assert_eq!(dequantize_ts(1, 22), 8);
        assert_eq!(dequantize_ts(-1, 22), -8);
        // QP 63 -> qpPer 10, qpRem 3, scale 57, shift -4 and clipped.
        assert_eq!(dequantize_ts(1000, 63), 32767);
        assert_eq!(dequantize_ts(-1000, 63), -32768);
    }
}
