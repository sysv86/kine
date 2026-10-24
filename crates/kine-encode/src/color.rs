//! RGBA ↔ YUV 4:2:0 (I420 / NV12), BT.709 limited range.
//!
//! The rasterizer produces premultiplied RGBA; the encoder wants planar YUV,
//! and video input decoders convert planar/semi-planar YUV back to RGBA8.
//!
//! Forward conversions (`rgba_to_i420`, `rgba_to_nv12`) use 16-bit fixed-point
//! integer BT.709 arithmetic (`+32768 >> 16`) with multi-threaded row chunking
//! on frames ≥ 128×128 so zero per-pixel floating-point division or rounding
//! occurs in the hot path.

/// One planar 4:2:0 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I420Frame {
    pub width: u32,
    pub height: u32,
    /// Full-resolution luma plane (`width * height`).
    pub y: Vec<u8>,
    /// Half-resolution chroma planes (`ceil(width/2) * ceil(height/2)`).
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420Frame {
    /// Chroma plane dimensions (rounded up for odd sizes).
    pub fn chroma_size(&self) -> (u32, u32) {
        (self.width.div_ceil(2), self.height.div_ceil(2))
    }

    /// Re-pack as NV12: the same luma plane followed by interleaved chroma.
    pub fn to_nv12(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(self.y.len() + self.u.len() + self.v.len());
        out.extend_from_slice(&self.y);
        for (u, v) in self.u.iter().zip(self.v.iter()) {
            out.push(*u);
            out.push(*v);
        }
        out
    }

    /// Convert this BT.709 limited-range I420 frame into straight/opaque RGBA8 bytes.
    pub fn to_rgba(&self) -> Vec<u8> {
        i420_to_rgba(self)
    }
}

/// 16-bit fixed-point BT.709 limited-range 8-bit coefficients:
///
/// `Y'  = 16  + ((11966 * R + 40254 * G +  4063 * B + 32768) >> 16)`
/// `Cb' = 128 + ((-6596 * R - 22188 * G + 28784 * B + 32768) >> 16)`
/// `Cr' = 128 + ((28784 * R - 26149 * G -  2635 * B + 32768) >> 16)`
///
/// Note: `11966 + 40254 + 4063 = 56283` (`(56283 * 255 + 32768) >> 16 == 219`),
/// and chroma coefficients sum to 0 on neutral greys (`R == G == B` => `Cb = Cr = 128`).
#[inline(always)]
fn luma_i32(r: i32, g: i32, b: i32) -> u8 {
    let y = 16 + ((11_966 * r + 40_254 * g + 4_063 * b + 32_768) >> 16);
    y.clamp(0, 255) as u8
}

#[inline(always)]
fn chroma_b_i32(r: i32, g: i32, b: i32) -> u8 {
    let cb = 128 + ((-6_596 * r - 22_188 * g + 28_784 * b + 32_768) >> 16);
    cb.clamp(0, 255) as u8
}

#[inline(always)]
fn chroma_r_i32(r: i32, g: i32, b: i32) -> u8 {
    let cr = 128 + ((28_784 * r - 26_149 * g - 2_635 * b + 32_768) >> 16);
    cr.clamp(0, 255) as u8
}

#[inline(always)]
fn unpremul_rgb_i32(rgba: &[u8], i: usize) -> (i32, i32, i32) {
    let a = i32::from(rgba[i + 3]);
    if a == 255 {
        (
            i32::from(rgba[i]),
            i32::from(rgba[i + 1]),
            i32::from(rgba[i + 2]),
        )
    } else if a == 0 {
        (0, 0, 0)
    } else {
        let half = a >> 1;
        let r = ((i32::from(rgba[i]) * 255 + half) / a).min(255);
        let g = ((i32::from(rgba[i + 1]) * 255 + half) / a).min(255);
        let b = ((i32::from(rgba[i + 2]) * 255 + half) / a).min(255);
        (r, g, b)
    }
}

#[inline]
fn yuv_bt709_to_rgb(y_byte: u8, cb_byte: u8, cr_byte: u8) -> [u8; 4] {
    let y = (f32::from(y_byte) - 16.0) * (1.0 / 219.0);
    let cb = (f32::from(cb_byte) - 128.0) * (1.0 / 224.0);
    let cr = (f32::from(cr_byte) - 128.0) * (1.0 / 224.0);

    // Inverse BT.709:
    // R = Y + 1.5748 * Cr
    // B = Y + 1.8556 * Cb
    // G = Y - 0.1873243 * Cb - 0.4681243 * Cr
    let r = (y + 1.5748 * cr).clamp(0.0, 1.0);
    let g = (y - 0.1873243 * cb - 0.4681243 * cr).clamp(0.0, 1.0);
    let b = (y + 1.8556 * cb).clamp(0.0, 1.0);

    [
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
        255,
    ]
}

#[inline(always)]
fn sample_box_2x2_rgb(rgba: &[u8], w: usize, h: usize, cx: usize, cy: usize) -> (i32, i32, i32) {
    let mut sum_r = 0i32;
    let mut sum_g = 0i32;
    let mut sum_b = 0i32;
    let mut count = 0i32;
    for dy in 0..2 {
        let py = cy * 2 + dy;
        if py >= h {
            continue;
        }
        let row_base = py * w;
        for dx in 0..2 {
            let px = cx * 2 + dx;
            if px >= w {
                continue;
            }
            let (r, g, b) = unpremul_rgb_i32(rgba, (row_base + px) * 4);
            sum_r += r;
            sum_g += g;
            sum_b += b;
            count += 1;
        }
    }
    if count == 4 {
        ((sum_r + 2) >> 2, (sum_g + 2) >> 2, (sum_b + 2) >> 2)
    } else if count > 0 {
        let half = count >> 1;
        (
            (sum_r + half) / count,
            (sum_g + half) / count,
            (sum_b + half) / count,
        )
    } else {
        (0, 0, 0)
    }
}

/// Convert premultiplied RGBA8 pixels to a BT.709 limited-range I420 frame.
pub fn rgba_to_i420(rgba: &[u8], width: u32, height: u32) -> I420Frame {
    let (w, h) = (width as usize, height as usize);
    debug_assert_eq!(rgba.len(), w * h * 4);

    let mut y_plane = vec![0u8; w * h];
    let (cw, ch) = (width.div_ceil(2) as usize, height.div_ceil(2) as usize);
    let mut u_plane = vec![0u8; cw * ch];
    let mut v_plane = vec![0u8; cw * ch];

    if w * h >= 128 * 128 {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(ch.max(1));
        let y_rows_per_chunk = h.div_ceil(threads).max(1);
        let c_rows_per_chunk = ch.div_ceil(threads).max(1);

        std::thread::scope(|s| {
            for (chunk_idx, y_chunk) in y_plane.chunks_mut(y_rows_per_chunk * w).enumerate() {
                let start_row = chunk_idx * y_rows_per_chunk;
                s.spawn(move || {
                    let rows = y_chunk.len() / w;
                    for r in 0..rows {
                        let row = start_row + r;
                        let out_row = &mut y_chunk[r * w..(r + 1) * w];
                        let row_base = row * w;
                        for col in 0..w {
                            let (red, green, blue) = unpremul_rgb_i32(rgba, (row_base + col) * 4);
                            out_row[col] = luma_i32(red, green, blue);
                        }
                    }
                });
            }

            for (chunk_idx, (u_chunk, v_chunk)) in u_plane
                .chunks_mut(c_rows_per_chunk * cw)
                .zip(v_plane.chunks_mut(c_rows_per_chunk * cw))
                .enumerate()
            {
                let start_cy = chunk_idx * c_rows_per_chunk;
                s.spawn(move || {
                    let rows = u_chunk.len() / cw;
                    for r in 0..rows {
                        let cy = start_cy + r;
                        for cx in 0..cw {
                            let (red, green, blue) = sample_box_2x2_rgb(rgba, w, h, cx, cy);
                            u_chunk[r * cw + cx] = chroma_b_i32(red, green, blue);
                            v_chunk[r * cw + cx] = chroma_r_i32(red, green, blue);
                        }
                    }
                });
            }
        });
    } else {
        for row in 0..h {
            let row_base = row * w;
            for col in 0..w {
                let (r, g, b) = unpremul_rgb_i32(rgba, (row_base + col) * 4);
                y_plane[row_base + col] = luma_i32(r, g, b);
            }
        }

        for cy in 0..ch {
            for cx in 0..cw {
                let (r, g, b) = sample_box_2x2_rgb(rgba, w, h, cx, cy);
                u_plane[cy * cw + cx] = chroma_b_i32(r, g, b);
                v_plane[cy * cw + cx] = chroma_r_i32(r, g, b);
            }
        }
    }

    I420Frame {
        width,
        height,
        y: y_plane,
        u: u_plane,
        v: v_plane,
    }
}

/// Convert premultiplied RGBA8 pixels directly to BT.709 limited-range NV12
/// (Y plane followed by interleaved Cb/Cr plane).
pub fn rgba_to_nv12(rgba: &[u8], width: u32, height: u32) -> Vec<u8> {
    let (w, h) = (width as usize, height as usize);
    debug_assert_eq!(rgba.len(), w * h * 4);

    let (cw, ch) = (width.div_ceil(2) as usize, height.div_ceil(2) as usize);
    let y_len = w * h;
    let uv_len = cw * ch * 2;
    let mut nv12 = vec![0u8; y_len + uv_len];
    let (y_plane, uv_plane) = nv12.split_at_mut(y_len);

    if w * h >= 128 * 128 {
        let threads = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1)
            .min(ch.max(1));
        let y_rows_per_chunk = h.div_ceil(threads).max(1);
        let c_rows_per_chunk = ch.div_ceil(threads).max(1);

        std::thread::scope(|s| {
            for (chunk_idx, y_chunk) in y_plane.chunks_mut(y_rows_per_chunk * w).enumerate() {
                let start_row = chunk_idx * y_rows_per_chunk;
                s.spawn(move || {
                    let rows = y_chunk.len() / w;
                    for r in 0..rows {
                        let row = start_row + r;
                        let out_row = &mut y_chunk[r * w..(r + 1) * w];
                        let row_base = row * w;
                        for col in 0..w {
                            let (red, green, blue) = unpremul_rgb_i32(rgba, (row_base + col) * 4);
                            out_row[col] = luma_i32(red, green, blue);
                        }
                    }
                });
            }

            let uv_row_stride = cw * 2;
            for (chunk_idx, uv_chunk) in uv_plane
                .chunks_mut(c_rows_per_chunk * uv_row_stride)
                .enumerate()
            {
                let start_cy = chunk_idx * c_rows_per_chunk;
                s.spawn(move || {
                    let rows = uv_chunk.len() / uv_row_stride;
                    for r in 0..rows {
                        let cy = start_cy + r;
                        let row_slice = &mut uv_chunk[r * uv_row_stride..(r + 1) * uv_row_stride];
                        for cx in 0..cw {
                            let (red, green, blue) = sample_box_2x2_rgb(rgba, w, h, cx, cy);
                            row_slice[cx * 2] = chroma_b_i32(red, green, blue);
                            row_slice[cx * 2 + 1] = chroma_r_i32(red, green, blue);
                        }
                    }
                });
            }
        });
    } else {
        for row in 0..h {
            let row_base = row * w;
            for col in 0..w {
                let (r, g, b) = unpremul_rgb_i32(rgba, (row_base + col) * 4);
                y_plane[row_base + col] = luma_i32(r, g, b);
            }
        }

        for cy in 0..ch {
            for cx in 0..cw {
                let (r, g, b) = sample_box_2x2_rgb(rgba, w, h, cx, cy);
                let idx = (cy * cw + cx) * 2;
                uv_plane[idx] = chroma_b_i32(r, g, b);
                uv_plane[idx + 1] = chroma_r_i32(r, g, b);
            }
        }
    }

    nv12
}

/// Convert a BT.709 limited-range [`I420Frame`] into straight/opaque RGBA8 bytes (`width * height * 4`).
pub fn i420_to_rgba(frame: &I420Frame) -> Vec<u8> {
    let w = frame.width as usize;
    let h = frame.height as usize;
    let cw = frame.width.div_ceil(2) as usize;
    let mut out = vec![0u8; w * h * 4];

    for row in 0..h {
        let cy = row / 2;
        for col in 0..w {
            let cx = col / 2;
            let y_val = frame.y[row * w + col];
            let c_idx = (cy * cw + cx).min(frame.u.len().saturating_sub(1));
            let u_val = frame.u.get(c_idx).copied().unwrap_or(128);
            let v_val = frame.v.get(c_idx).copied().unwrap_or(128);
            let rgba = yuv_bt709_to_rgb(y_byte_clamp(y_val), u_val, v_val);
            let dst = (row * w + col) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba);
        }
    }
    out
}

/// Convert BT.709 limited-range NV12 bytes into straight/opaque RGBA8 bytes (`width * height * 4`).
pub fn nv12_to_rgba(nv12: &[u8], width: u32, height: u32) -> Result<Vec<u8>, String> {
    let w = width as usize;
    let h = height as usize;
    let cw = width.div_ceil(2) as usize;
    let ch = height.div_ceil(2) as usize;
    let expected = w * h + cw * ch * 2;
    if nv12.len() < expected {
        return Err(format!(
            "NV12 buffer too short: got {} bytes, expected {expected} for {width}x{height}",
            nv12.len()
        ));
    }
    let (y_plane, uv_plane) = nv12.split_at(w * h);
    let mut out = vec![0u8; w * h * 4];
    for row in 0..h {
        let cy = row / 2;
        for col in 0..w {
            let cx = col / 2;
            let y_val = y_plane[row * w + col];
            let uv_idx = (cy * cw + cx) * 2;
            let u_val = uv_plane[uv_idx];
            let v_val = uv_plane[uv_idx + 1];
            let rgba = yuv_bt709_to_rgb(y_byte_clamp(y_val), u_val, v_val);
            let dst = (row * w + col) * 4;
            out[dst..dst + 4].copy_from_slice(&rgba);
        }
    }
    Ok(out)
}

/// Convert BT.709 limited-range planar 4:4:4 (`Y`, `U`, `V` of `width * height`) into RGBA8.
pub fn i444_to_rgba(y: &[u8], u: &[u8], v: &[u8], width: u32, height: u32) -> Vec<u8> {
    let total = (width as usize) * (height as usize);
    let mut out = vec![0u8; total * 4];
    for i in 0..total {
        let yv = y.get(i).copied().unwrap_or(16);
        let uv = u.get(i).copied().unwrap_or(128);
        let vv = v.get(i).copied().unwrap_or(128);
        let rgba = yuv_bt709_to_rgb(y_byte_clamp(yv), uv, vv);
        out[i * 4..i * 4 + 4].copy_from_slice(&rgba);
    }
    out
}

#[inline]
fn y_byte_clamp(y: u8) -> u8 {
    y.clamp(16, 235)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn solid(width: u32, height: u32, r: u8, g: u8, b: u8) -> Vec<u8> {
        let mut v = Vec::with_capacity((width * height * 4) as usize);
        for _ in 0..width * height {
            v.extend_from_slice(&[r, g, b, 255]);
        }
        v
    }

    #[test]
    fn black_and_white_land_on_limited_range_anchors() {
        let black = rgba_to_i420(&solid(4, 4, 0, 0, 0), 4, 4);
        assert_eq!(black.y[0], 16, "black luma must be 16 in limited range");

        let white = rgba_to_i420(&solid(4, 4, 255, 255, 255), 4, 4);
        assert_eq!(white.y[0], 235, "white luma must be 235 in limited range");
    }

    #[test]
    fn i420_and_nv12_round_trip_to_rgba() {
        let src = solid(4, 4, 200, 60, 40);
        let i420 = rgba_to_i420(&src, 4, 4);
        let rgba = i420_to_rgba(&i420);
        assert!((i32::from(rgba[0]) - 200).abs() <= 2);
        assert!((i32::from(rgba[1]) - 60).abs() <= 2);
        assert!((i32::from(rgba[2]) - 40).abs() <= 2);
        assert_eq!(rgba[3], 255);

        let nv12 = rgba_to_nv12(&src, 4, 4);
        let rgba_nv12 = nv12_to_rgba(&nv12, 4, 4).unwrap();
        assert_eq!(rgba, rgba_nv12);
    }

    #[test]
    fn neutral_grey_has_neutral_chroma() {
        let grey = rgba_to_i420(&solid(2, 2, 128, 128, 128), 2, 2);
        assert!(
            (i32::from(grey.u[0]) - 128).abs() <= 1,
            "Cb should be ~128, got {}",
            grey.u[0]
        );
        assert!(
            (i32::from(grey.v[0]) - 128).abs() <= 1,
            "Cr should be ~128, got {}",
            grey.v[0]
        );
    }

    #[test]
    fn primaries_shift_chroma_in_the_expected_direction() {
        let red = rgba_to_i420(&solid(2, 2, 255, 0, 0), 2, 2);
        assert!(red.v[0] > 230, "red should raise Cr, got {}", red.v[0]);
        assert!(red.u[0] < 110, "red should lower Cb, got {}", red.u[0]);

        let blue = rgba_to_i420(&solid(2, 2, 0, 0, 255), 2, 2);
        assert!(blue.u[0] > 230, "blue should raise Cb, got {}", blue.u[0]);
        assert!(blue.v[0] < 128, "blue should lower Cr below neutral, got {}", blue.v[0]);
        assert!(blue.v[0] > 110, "…but only slightly, got {}", blue.v[0]);
    }

    #[test]
    fn premultiplied_colour_is_unpremultiplied_before_conversion() {
        let mut half_transparent = Vec::new();
        half_transparent.extend_from_slice(&[128, 0, 0, 128]);
        half_transparent.extend_from_slice(&[128, 0, 0, 128]);
        half_transparent.extend_from_slice(&[128, 0, 0, 128]);
        half_transparent.extend_from_slice(&[128, 0, 0, 128]);
        let soft = rgba_to_i420(&half_transparent, 2, 2);
        let opaque = rgba_to_i420(&solid(2, 2, 255, 0, 0), 2, 2);

        assert_eq!(
            soft.v[0], opaque.v[0],
            "a half-transparent red must carry the same chroma as opaque red"
        );
        assert_eq!(soft.u[0], opaque.u[0]);
    }

    #[test]
    fn nv12_interleaves_chroma_after_luma() {
        let src = solid(4, 4, 200, 40, 40);
        let i420 = rgba_to_i420(&src, 4, 4);
        let nv12 = i420.to_nv12();
        let direct = rgba_to_nv12(&src, 4, 4);
        assert_eq!(nv12, direct, "direct rgba_to_nv12 must match i420.to_nv12()");
        assert_eq!(nv12.len(), 4 * 4 * 3 / 2, "NV12 is 1.5 bytes per pixel");
        assert_eq!(&nv12[..i420.y.len()], &i420.y[..], "luma comes first");
        assert_eq!(nv12[i420.y.len()], i420.u[0]);
        assert_eq!(nv12[i420.y.len() + 1], i420.v[0]);
        assert_eq!(nv12[i420.y.len() + 2], i420.u[1]);
    }

    #[test]
    fn odd_dimensions_round_chroma_up() {
        let frame = rgba_to_i420(&solid(3, 3, 10, 20, 30), 3, 3);
        assert_eq!(frame.chroma_size(), (2, 2));
        assert_eq!(frame.u.len(), 4);
        assert_eq!(frame.v.len(), 4);
        assert_eq!(frame.y.len(), 9);
    }

    #[test]
    fn parallel_path_matches_small_path() {
        let large = solid(256, 256, 180, 90, 40);
        let i420 = rgba_to_i420(&large, 256, 256);
        let nv12 = rgba_to_nv12(&large, 256, 256);
        let small = rgba_to_i420(&solid(4, 4, 180, 90, 40), 4, 4);
        assert_eq!(i420.y[0], small.y[0]);
        assert_eq!(i420.u[0], small.u[0]);
        assert_eq!(i420.v[0], small.v[0]);
        assert_eq!(nv12, i420.to_nv12());
    }
}
