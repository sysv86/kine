//! Intra prediction for the VVC subset this crate encodes.
//!
//! Only PLANAR is emitted (selected through the MPM path), but DC is
//! implemented as well because the reference-sample/PDPC machinery is shared
//! and it keeps the reconstruction path honest.  The arithmetic follows VTM
//! `IntraPrediction.cpp` exactly, including the `[1 2 1]` reference filter for
//! luma planar blocks larger than 4x8 and the mandatory PDPC step.

/// A single reconstructed plane.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plane {
    pub width: usize,
    pub height: usize,
    pub data: Vec<u8>,
}

impl Plane {
    pub fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            height,
            data: vec![0; width * height],
        }
    }

    pub fn from_slice(width: usize, height: usize, data: &[u8]) -> Self {
        assert_eq!(data.len(), width * height);
        Self {
            width,
            height,
            data: data.to_vec(),
        }
    }

    #[inline]
    pub fn get(&self, x: usize, y: usize) -> u8 {
        self.data[y * self.width + x]
    }

    #[inline]
    pub fn set(&mut self, x: usize, y: usize, value: u8) {
        let idx = y * self.width + x;
        self.data[idx] = value;
    }

    /// Copy a same-sized block from `src` at `(sx, sy)`.
    pub fn copy_block_from(&mut self, src: &Plane, sx: usize, sy: usize) {
        for y in 0..self.height {
            let s = (sy + y) * src.width + sx;
            let d = y * self.width;
            self.data[d..d + self.width].copy_from_slice(&src.data[s..s + self.width]);
        }
    }
}

/// Reference samples for one intra prediction block.
#[derive(Debug, Clone)]
pub struct RefSamples {
    /// `2 * w` samples above the block (`above[0]` is directly above-left).
    pub above: Vec<i32>,
    /// `2 * h` samples left of the block (`left[0]` is directly above-left).
    pub left: Vec<i32>,
    pub top_left: i32,
}

/// Collect reference samples for a `w x h` block at `(bx, by)`.
///
/// `unit` is the minimum prediction unit in the component's coordinates: 4 for
/// luma and 2 for 4:2:0 chroma.  Availability is resolved against the already
/// reconstructed plane (raster CTU order, CTU == CU for this encoder), then
/// padded exactly like VTM's `xFillReferenceSamples`.
pub fn gather_references(recon: &Plane, bx: usize, by: usize, w: usize, h: usize) -> RefSamples {
    let pw = recon.width;
    let ph = recon.height;
    let mut above = vec![0i32; 2 * w];
    let mut left = vec![0i32; 2 * h];

    // In this encoder a prediction block is always a full CTU (32x32 luma).
    // That fixes the availability rules exactly:
    //   * the row above belongs to the previous, fully decoded CTU row, so
    //     above + above-right are available (up to the picture edge);
    //   * the column to the left is a fully decoded CTU of the same CTU row,
    //     so only `h` left samples are decoded — the below-left extension lies
    //     in the *next* CTU row and must be padded.  Presuming it available
    //     would make the encoder read source samples the decoder does not have
    //     yet (this is the classic below-left availability bug).
    let above_len = if by == 0 { 0 } else { (pw - bx).min(2 * w) };
    let left_len = if bx == 0 { 0 } else { (ph - by).min(h) };

    if above_len > 0 {
        for (i, a) in above.iter_mut().take(above_len).enumerate() {
            *a = i32::from(recon.get(bx + i, by - 1));
        }
    }
    if left_len > 0 {
        for (i, l) in left.iter_mut().take(left_len).enumerate() {
            *l = i32::from(recon.get(bx - 1, by + i));
        }
    }

    // Top-left corner and the "only one side available" padding.
    let top_left = if above_len > 0 && left_len > 0 {
        i32::from(recon.get(bx - 1, by - 1))
    } else if above_len > 0 {
        above[0]
    } else if left_len > 0 {
        left[0]
    } else {
        128
    };

    if above_len == 0 {
        above.fill(if left_len > 0 { left[0] } else { 128 });
    } else {
        for i in above_len..2 * w {
            above[i] = above[above_len - 1];
        }
    }
    if left_len == 0 {
        left.fill(if above_len > 0 { above[0] } else { 128 });
    } else {
        for i in left_len..2 * h {
            left[i] = left[left_len - 1];
        }
    }

    RefSamples {
        above,
        left,
        top_left,
    }
}

impl RefSamples {
    /// `[1 2 1]` smoothing of both reference rows (VTM `xFilterReferenceSamples`).
    #[allow(clippy::needless_range_loop)] // three-neighbour window reads
    pub fn filtered(&self) -> RefSamples {
        let w2 = self.above.len();
        let h2 = self.left.len();
        let mut above = self.above.clone();
        let mut left = self.left.clone();

        let top_left = (2 * self.top_left + self.above[0] + self.left[0] + 2) >> 2;
        for j in 0..w2.saturating_sub(1) {
            let prev = if j == 0 {
                self.top_left
            } else {
                self.above[j - 1]
            };
            above[j] = (prev + 2 * self.above[j] + self.above[j + 1] + 2) >> 2;
        }
        if w2 > 0 {
            above[w2 - 1] = self.above[w2 - 1];
        }
        for j in 0..h2.saturating_sub(1) {
            let prev = if j == 0 {
                self.top_left
            } else {
                self.left[j - 1]
            };
            left[j] = (prev + 2 * self.left[j] + self.left[j + 1] + 2) >> 2;
        }
        if h2 > 0 {
            left[h2 - 1] = self.left[h2 - 1];
        }
        RefSamples {
            above,
            left,
            top_left,
        }
    }
}

#[inline]
fn log2_exact(value: usize) -> u32 {
    debug_assert!(value.is_power_of_two());
    value.trailing_zeros()
}

/// Luma planar uses `[1 2 1]`-filtered references when `w * h > 32`; chroma
/// never filters.  DC never filters either.
pub fn uses_reference_filter(is_luma: bool, mode: IntraMode, w: usize, h: usize) -> bool {
    match mode {
        IntraMode::Planar => is_luma && w * h > 32,
        IntraMode::Dc => false,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IntraMode {
    Planar,
    Dc,
}

/// PLANAR prediction (VTM `xPredIntraPlanar`).
pub fn predict_planar(refs: &RefSamples, w: usize, h: usize) -> Vec<i32> {
    let log2w = log2_exact(w);
    let log2h = log2_exact(h);
    let offset = 1i32 << (log2w + log2h);
    let final_shift = 1 + log2w + log2h;

    let mut top: Vec<i32> = refs.above[..=w].to_vec();
    let mut left: Vec<i32> = refs.left[..=h].to_vec();
    let top_right = top[w];
    let bottom_left = left[h];

    let bottom_row: Vec<i32> = (0..w).map(|k| bottom_left - top[k]).collect();
    let right_column: Vec<i32> = (0..h).map(|k| top_right - left[k]).collect();
    for value in top.iter_mut() {
        *value <<= log2h;
    }
    for value in left.iter_mut() {
        *value <<= log2w;
    }

    let mut pred = vec![0i32; w * h];
    for y in 0..h {
        let mut hor = left[y];
        for x in 0..w {
            hor += right_column[y];
            top[x] += bottom_row[x];
            let vert = top[x];
            pred[y * w + x] = ((hor << log2h) + (vert << log2w) + offset) >> final_shift;
        }
    }
    pred
}

/// DC prediction (VTM `xGetPredValDc`).
pub fn predict_dc(refs: &RefSamples, w: usize, h: usize) -> Vec<i32> {
    let denom = if w == h { w << 1 } else { w.max(h) };
    let div_shift = log2_exact(denom);
    let div_offset = (denom >> 1) as i32;
    let mut sum = 0i32;
    if w >= h {
        sum += refs.above[..w].iter().sum::<i32>();
    }
    if w <= h {
        sum += refs.left[..h].iter().sum::<i32>();
    }
    let dc = (sum + div_offset) >> div_shift;
    vec![dc; w * h]
}

/// Position-dependent prediction combination for planar/DC
/// (VTM `IntraPrediction.cpp` PDPC step).
pub fn apply_pdpc(pred: &mut [i32], refs: &RefSamples, w: usize, h: usize) {
    let log2w = log2_exact(w);
    let log2h = log2_exact(h);
    let scale = ((log2w + log2h).saturating_sub(2)) >> 2;
    for y in 0..h {
        let wt = 32i32 >> ((y as u32 * 2) >> scale).min(31);
        let left = refs.left[y];
        for x in 0..w {
            let wl = 32i32 >> ((x as u32 * 2) >> scale).min(31);
            let top = refs.above[x];
            let val = pred[y * w + x];
            pred[y * w + x] = val + ((wl * (left - val) + wt * (top - val) + 32) >> 6);
        }
    }
}

/// Produce the prediction block for one component.
pub fn predict_block(
    recon: &Plane,
    bx: usize,
    by: usize,
    w: usize,
    h: usize,
    is_luma: bool,
    mode: IntraMode,
) -> Vec<i32> {
    let refs = gather_references(recon, bx, by, w, h);
    let used_refs = if uses_reference_filter(is_luma, mode, w, h) {
        refs.filtered()
    } else {
        refs
    };
    let mut pred = match mode {
        IntraMode::Planar => predict_planar(&used_refs, w, h),
        IntraMode::Dc => predict_dc(&used_refs, w, h),
    };
    apply_pdpc(&mut pred, &used_refs, w, h);
    pred
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn planar_of_flat_reconstruction_is_flat() {
        let plane = Plane::from_slice(64, 64, &[77u8; 64 * 64]);
        let pred = predict_block(&plane, 32, 32, 32, 32, true, IntraMode::Planar);
        assert!(
            pred.iter().all(|&p| p == 77),
            "planar must reproduce constant"
        );
    }

    #[test]
    fn dc_of_flat_reconstruction_is_flat() {
        let plane = Plane::from_slice(64, 64, &[42u8; 64 * 64]);
        let pred = predict_block(&plane, 32, 32, 32, 32, true, IntraMode::Dc);
        assert!(pred.iter().all(|&p| p == 42));
    }

    #[test]
    fn top_left_corner_block_uses_neutral_references() {
        let plane = Plane::new(32, 32);
        let pred = predict_block(&plane, 0, 0, 32, 32, true, IntraMode::Planar);
        assert!(pred.iter().all(|&p| p == 128));
    }

    #[test]
    fn reference_padding_repeats_last_available_sample() {
        let plane = Plane::from_slice(32, 32, &[9u8; 32 * 32]);
        let refs = gather_references(&plane, 0, 16, 32, 32);
        assert!(refs.above.iter().all(|&v| v == 9));
        // No left column at x == 0: padded from above.
        assert!(refs.left.iter().all(|&v| v == 9));
    }
}
