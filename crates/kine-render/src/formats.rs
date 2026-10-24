//! Self-contained multi-format still image and animated GIF89a encoders/decoders:
//! * **PNG** (`.png`) — straight-alpha RGBA8 via `image`.
//! * **QOI** (`.qoi`) — Quite OK Image v1.0 lossless O(N) RGBA8 encoder & decoder.
//! * **BMP** (`.bmp`) — 32-bit BGRA BITMAPINFOHEADER encoder & 24/32-bit decoder.
//! * **PPM / PAM** (`.ppm`, `.pam`) — Netpbm binary `P6` (RGB8) and `P7` (`RGB_ALPHA`) encoder & decoder.
//! * **TGA** (`.tga`) — 32-bit Truevision TARGA RGBA8 encoder & decoder.
//! * **GIF89a** (`.gif`) — multi-frame animated GIF89a encoder with 6×6×6+grayscale
//!   palette, ordered Bayer 4×4 dithering, and variable-width LZW compression.

use crate::png::pixmap_to_png;
use vello_cpu::Pixmap;

/// Supported still-frame output formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameFormat {
    Png,
    Qoi,
    Bmp,
    Ppm,
    Pam,
    Tga,
}

impl FrameFormat {
    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(Self::Png),
            "qoi" => Some(Self::Qoi),
            "bmp" => Some(Self::Bmp),
            "ppm" | "pnm" => Some(Self::Ppm),
            "pam" => Some(Self::Pam),
            "tga" => Some(Self::Tga),
            _ => None,
        }
    }
}

/// Convert a premultiplied `Pixmap` into straight-alpha RGBA8 bytes (`width * height * 4`).
pub fn pixmap_to_straight_rgba(pixmap: &Pixmap) -> Vec<u8> {
    let w = usize::from(pixmap.width());
    let h = usize::from(pixmap.height());
    let mut straight = Vec::with_capacity(w * h * 4);
    for p in pixmap.data() {
        let a = u32::from(p.a);
        if a == 0 {
            straight.extend_from_slice(&[0, 0, 0, 0]);
        } else if a == 255 {
            straight.extend_from_slice(&[p.r, p.g, p.b, 255]);
        } else {
            let unpremul = |c: u8| ((u32::from(c) * 255 + a / 2) / a).min(255) as u8;
            straight.extend_from_slice(&[unpremul(p.r), unpremul(p.g), unpremul(p.b), p.a]);
        }
    }
    straight
}

/// Encode `pixmap` in the requested raster [`FrameFormat`].
pub fn encode_pixmap(pixmap: &Pixmap, format: FrameFormat) -> Result<Vec<u8>, String> {
    match format {
        FrameFormat::Png => pixmap_to_png(pixmap).map_err(|e| e.to_string()),
        FrameFormat::Qoi => Ok(pixmap_to_qoi(pixmap)),
        FrameFormat::Bmp => Ok(pixmap_to_bmp(pixmap)),
        FrameFormat::Ppm => Ok(pixmap_to_ppm(pixmap)),
        FrameFormat::Pam => Ok(pixmap_to_pam(pixmap)),
        FrameFormat::Tga => Ok(pixmap_to_tga(pixmap)),
    }
}

// ── QOI (Quite OK Image v1.0) ───────────────────────────────────────────────

#[inline]
fn qoi_hash(r: u8, g: u8, b: u8, a: u8) -> usize {
    (usize::from(r) * 3 + usize::from(g) * 5 + usize::from(b) * 7 + usize::from(a) * 11) % 64
}

/// Encode a `Pixmap` into a lossless QOI v1.0 image (`qoif` header + chunks + 8-byte end marker).
pub fn pixmap_to_qoi(pixmap: &Pixmap) -> Vec<u8> {
    let w = u32::from(pixmap.width());
    let h = u32::from(pixmap.height());
    let rgba = pixmap_to_straight_rgba(pixmap);

    let mut out = Vec::with_capacity(14 + (w as usize) * (h as usize) * 4 + 8);
    out.extend_from_slice(b"qoif");
    out.extend_from_slice(&w.to_be_bytes());
    out.extend_from_slice(&h.to_be_bytes());
    out.push(4); // channels = 4 (RGBA)
    out.push(0); // colorspace = 0 (sRGB with linear alpha)

    let mut index = [[0u8; 4]; 64];
    let mut prev = [0u8, 0u8, 0u8, 255u8];
    let mut run = 0u8;
    let total_px = (w as usize) * (h as usize);

    for (px_i, px) in rgba.chunks_exact(4).enumerate() {
        let cur = [px[0], px[1], px[2], px[3]];
        if cur == prev {
            run += 1;
            if run == 62 || px_i + 1 == total_px {
                out.push(0xC0 | (run - 1)); // QOI_OP_RUN
                run = 0;
            }
            continue;
        }
        if run > 0 {
            out.push(0xC0 | (run - 1));
            run = 0;
        }
        let hash = qoi_hash(cur[0], cur[1], cur[2], cur[3]);
        if index[hash] == cur {
            out.push(hash as u8); // QOI_OP_INDEX
        } else {
            index[hash] = cur;
            if cur[3] == prev[3] {
                let vr = cur[0].wrapping_sub(prev[0]) as i8;
                let vg = cur[1].wrapping_sub(prev[1]) as i8;
                let vb = cur[2].wrapping_sub(prev[2]) as i8;
                let vg_r = vr.wrapping_sub(vg);
                let vg_b = vb.wrapping_sub(vg);

                if (-2..=1).contains(&vr) && (-2..=1).contains(&vg) && (-2..=1).contains(&vb) {
                    // QOI_OP_DIFF
                    let b = 0x40
                        | (((vr + 2) as u8) << 4)
                        | (((vg + 2) as u8) << 2)
                        | ((vb + 2) as u8);
                    out.push(b);
                } else if (-8..=7).contains(&vg_r)
                    && (-32..=31).contains(&vg)
                    && (-8..=7).contains(&vg_b)
                {
                    // QOI_OP_LUMA
                    out.push(0x80 | ((vg + 32) as u8));
                    out.push((((vg_r + 8) as u8) << 4) | ((vg_b + 8) as u8));
                } else {
                    // QOI_OP_RGB
                    out.extend_from_slice(&[0xFE, cur[0], cur[1], cur[2]]);
                }
            } else {
                // QOI_OP_RGBA
                out.extend_from_slice(&[0xFF, cur[0], cur[1], cur[2], cur[3]]);
            }
        }
        prev = cur;
    }

    out.extend_from_slice(&[0, 0, 0, 0, 0, 0, 0, 1]);
    out
}

/// Decode a QOI v1.0 byte slice into `(width, height, straight_rgba8)`.
pub fn decode_qoi(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.len() < 22 || &bytes[0..4] != b"qoif" {
        return Err("invalid QOI header".into());
    }
    let w = u32::from_be_bytes([bytes[4], bytes[5], bytes[6], bytes[7]]);
    let h = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]);
    if w == 0 || h == 0 {
        return Err("zero-sized QOI image".into());
    }
    let total_px = (w as usize) * (h as usize);
    let mut out = Vec::with_capacity(total_px * 4);
    let mut index = [[0u8; 4]; 64];
    let mut px = [0u8, 0u8, 0u8, 255u8];
    let mut run = 0u8;
    let mut pos = 14usize;
    let end = bytes.len() - 8;

    for _ in 0..total_px {
        if run > 0 {
            run -= 1;
        } else if pos < end {
            let b1 = bytes[pos];
            pos += 1;
            if b1 == 0xFE {
                if pos + 3 <= end {
                    px[0] = bytes[pos];
                    px[1] = bytes[pos + 1];
                    px[2] = bytes[pos + 2];
                    pos += 3;
                }
            } else if b1 == 0xFF {
                if pos + 4 <= end {
                    px[0] = bytes[pos];
                    px[1] = bytes[pos + 1];
                    px[2] = bytes[pos + 2];
                    px[3] = bytes[pos + 3];
                    pos += 4;
                }
            } else {
                match b1 & 0xC0 {
                    0x00 => {
                        px = index[(b1 & 0x3F) as usize];
                    }
                    0x40 => {
                        px[0] = px[0].wrapping_add(((b1 >> 4) & 0x03).wrapping_sub(2));
                        px[1] = px[1].wrapping_add(((b1 >> 2) & 0x03).wrapping_sub(2));
                        px[2] = px[2].wrapping_add((b1 & 0x03).wrapping_sub(2));
                    }
                    0x80 => {
                        if pos < end {
                            let b2 = bytes[pos];
                            pos += 1;
                            let vg = (b1 & 0x3F).wrapping_sub(32);
                            px[0] = px[0].wrapping_add(vg.wrapping_sub(8).wrapping_add((b2 >> 4) & 0x0F));
                            px[1] = px[1].wrapping_add(vg);
                            px[2] = px[2].wrapping_add(vg.wrapping_sub(8).wrapping_add(b2 & 0x0F));
                        }
                    }
                    0xC0 => {
                        run = b1 & 0x3F;
                    }
                    _ => unreachable!(),
                }
            }
            index[qoi_hash(px[0], px[1], px[2], px[3])] = px;
        }
        out.extend_from_slice(&px);
    }

    Ok((w, h, out))
}

// ── BMP (32-bit BGRA top-down BITMAPINFOHEADER) ─────────────────────────────

pub fn pixmap_to_bmp(pixmap: &Pixmap) -> Vec<u8> {
    let w = u32::from(pixmap.width());
    let h = u32::from(pixmap.height());
    let rgba = pixmap_to_straight_rgba(pixmap);
    let data_size = w * h * 4;
    let file_size = 14 + 40 + data_size;

    let mut out = Vec::with_capacity(file_size as usize);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&file_size.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&54u32.to_le_bytes());

    out.extend_from_slice(&40u32.to_le_bytes());
    out.extend_from_slice(&(w as i32).to_le_bytes());
    out.extend_from_slice(&(-(h as i32)).to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes());
    out.extend_from_slice(&32u16.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&data_size.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&2835u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());

    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]); // BGRA
    }
    out
}

pub fn decode_bmp(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.len() < 54 || &bytes[0..2] != b"BM" {
        return Err("invalid BMP header".into());
    }
    let data_offset = u32::from_le_bytes([bytes[10], bytes[11], bytes[12], bytes[13]]) as usize;
    let width = i32::from_le_bytes([bytes[18], bytes[19], bytes[20], bytes[21]]);
    let height_raw = i32::from_le_bytes([bytes[22], bytes[23], bytes[24], bytes[25]]);
    let bpp = u16::from_le_bytes([bytes[28], bytes[29]]);
    if width <= 0 || height_raw == 0 || (bpp != 24 && bpp != 32) {
        return Err(format!("unsupported BMP geometry {width}x{height_raw} @{bpp}bpp"));
    }
    let w = width as u32;
    let h = height_raw.unsigned_abs();
    let top_down = height_raw < 0;
    let bytes_per_px = (bpp / 8) as usize;
    let row_stride = ((w as usize) * bytes_per_px).div_ceil(4) * 4;
    if data_offset + row_stride * (h as usize) > bytes.len() {
        return Err("truncated BMP pixel data".into());
    }

    let mut out = vec![0u8; (w as usize) * (h as usize) * 4];
    for row in 0..(h as usize) {
        let src_row = if top_down { row } else { (h as usize) - 1 - row };
        let src_base = data_offset + src_row * row_stride;
        let dst_base = row * (w as usize) * 4;
        for col in 0..(w as usize) {
            let s = src_base + col * bytes_per_px;
            let d = dst_base + col * 4;
            out[d] = bytes[s + 2];
            out[d + 1] = bytes[s + 1];
            out[d + 2] = bytes[s];
            out[d + 3] = if bytes_per_px == 4 { bytes[s + 3] } else { 255 };
        }
    }
    Ok((w, h, out))
}

// ── PPM (P6) & PAM (P7) ─────────────────────────────────────────────────────

pub fn pixmap_to_ppm(pixmap: &Pixmap) -> Vec<u8> {
    let w = u32::from(pixmap.width());
    let h = u32::from(pixmap.height());
    let rgba = pixmap_to_straight_rgba(pixmap);
    let header = format!("P6\n{w} {h}\n255\n");
    let mut out = Vec::with_capacity(header.len() + (w as usize) * (h as usize) * 3);
    out.extend_from_slice(header.as_bytes());
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[0], px[1], px[2]]);
    }
    out
}

pub fn pixmap_to_pam(pixmap: &Pixmap) -> Vec<u8> {
    let w = u32::from(pixmap.width());
    let h = u32::from(pixmap.height());
    let rgba = pixmap_to_straight_rgba(pixmap);
    let header = format!(
        "P7\nWIDTH {w}\nHEIGHT {h}\nDEPTH 4\nMAXVAL 255\nTUPLTYPE RGB_ALPHA\nENDHDR\n"
    );
    let mut out = Vec::with_capacity(header.len() + rgba.len());
    out.extend_from_slice(header.as_bytes());
    out.extend_from_slice(&rgba);
    out
}

// ── TGA (Truevision TARGA 32-bit uncompressed top-left origin) ──────────────

pub fn pixmap_to_tga(pixmap: &Pixmap) -> Vec<u8> {
    let w = pixmap.width();
    let h = pixmap.height();
    let rgba = pixmap_to_straight_rgba(pixmap);
    let mut out = Vec::with_capacity(18 + rgba.len());
    out.push(0); // ID length
    out.push(0); // Color map type
    out.push(2); // Image type = 2 (uncompressed true-color)
    out.extend_from_slice(&[0, 0, 0, 0, 0]);
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes());
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.push(32); // 32 bits per pixel
    out.push(0x28); // Image descriptor: bit 5 = top-left origin, 8-bit alpha
    for px in rgba.chunks_exact(4) {
        out.extend_from_slice(&[px[2], px[1], px[0], px[3]]); // BGRA
    }
    out
}

pub fn decode_tga(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.len() < 18 || bytes[2] != 2 {
        return Err("unsupported TGA header".into());
    }
    let id_len = bytes[0] as usize;
    let w = u16::from_le_bytes([bytes[12], bytes[13]]) as u32;
    let h = u16::from_le_bytes([bytes[14], bytes[15]]) as u32;
    let bpp = bytes[16];
    let desc = bytes[17];
    if w == 0 || h == 0 || (bpp != 24 && bpp != 32) {
        return Err("unsupported TGA geometry".into());
    }
    let bytes_per_px = (bpp / 8) as usize;
    let offset = 18 + id_len;
    let total_px = (w as usize) * (h as usize);
    if offset + total_px * bytes_per_px > bytes.len() {
        return Err("truncated TGA pixel data".into());
    }
    let top_down = (desc & 0x20) != 0;
    let mut out = vec![0u8; total_px * 4];
    for row in 0..(h as usize) {
        let src_row = if top_down { row } else { (h as usize) - 1 - row };
        for col in 0..(w as usize) {
            let s = offset + (src_row * (w as usize) + col) * bytes_per_px;
            let d = (row * (w as usize) + col) * 4;
            out[d] = bytes[s + 2];
            out[d + 1] = bytes[s + 1];
            out[d + 2] = bytes[s];
            out[d + 3] = if bytes_per_px == 4 { bytes[s + 3] } else { 255 };
        }
    }
    Ok((w, h, out))
}

// ── Universal Image Asset Loader (PNG / QOI / BMP / PPM / PAM / TGA) ────────

/// Decode an image asset from raw bytes into `(width, height, straight_rgba8)`.
pub fn decode_any_image(bytes: &[u8]) -> Result<(u32, u32, Vec<u8>), String> {
    if bytes.starts_with(&[0x89, b'P', b'N', b'G']) {
        let img = image::load_from_memory(bytes)
            .map_err(|e| e.to_string())?
            .to_rgba8();
        return Ok((img.width(), img.height(), img.into_raw()));
    }
    if bytes.starts_with(b"qoif") {
        return decode_qoi(bytes);
    }
    if bytes.starts_with(b"BM") {
        return decode_bmp(bytes);
    }
    if bytes.starts_with(b"P7\n") {
        if let Some(end_idx) = bytes.windows(7).position(|w| w == b"ENDHDR\n") {
            let hdr = String::from_utf8_lossy(&bytes[..end_idx]);
            let mut w = 0u32;
            let mut h = 0u32;
            for line in hdr.lines() {
                if let Some(v) = line.strip_prefix("WIDTH ") {
                    w = v.trim().parse().unwrap_or(0);
                } else if let Some(v) = line.strip_prefix("HEIGHT ") {
                    h = v.trim().parse().unwrap_or(0);
                }
            }
            let data = &bytes[end_idx + 7..];
            let expected = (w as usize) * (h as usize) * 4;
            if w > 0 && h > 0 && data.len() >= expected {
                return Ok((w, h, data[..expected].to_vec()));
            }
        }
    }
    if bytes.starts_with(b"P6\n") {
        let mut lines = bytes.splitn(4, |&b| b == b'\n');
        let _magic = lines.next();
        if let (Some(dims), Some(_maxval), Some(data)) = (lines.next(), lines.next(), lines.next())
        {
            let dims_str = String::from_utf8_lossy(dims);
            let mut it = dims_str.split_whitespace();
            let w: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let h: u32 = it.next().and_then(|s| s.parse().ok()).unwrap_or(0);
            let expected = (w as usize) * (h as usize) * 3;
            if w > 0 && h > 0 && data.len() >= expected {
                let mut rgba = Vec::with_capacity((w as usize) * (h as usize) * 4);
                for px in data[..expected].chunks_exact(3) {
                    rgba.extend_from_slice(&[px[0], px[1], px[2], 255]);
                }
                return Ok((w, h, rgba));
            }
        }
    }
    if let Ok(tga) = decode_tga(bytes) {
        return Ok(tga);
    }
    Err("unsupported image format (expected PNG, QOI, BMP, PPM, PAM or TGA)".into())
}

// ── Animated GIF89a Encoder (pure Rust, LZW + 6x6x6+gray palette) ───────────

/// Encode a sequence of `Pixmap` frames into an animated, looping GIF89a byte buffer.
pub fn encode_animated_gif(frames: &[Pixmap], fps_num: u32, fps_den: u32) -> Result<Vec<u8>, String> {
    let Some(first) = frames.first() else {
        return Err("GIF requires at least one frame".into());
    };
    let w = first.width();
    let h = first.height();
    let delay_cs = ((100u64 * u64::from(fps_den.max(1)) + u64::from(fps_num.max(1)) / 2)
        / u64::from(fps_num.max(1)))
    .max(2) as u16;

    let mut out = Vec::new();
    out.extend_from_slice(b"GIF89a");
    out.extend_from_slice(&w.to_le_bytes());
    out.extend_from_slice(&h.to_le_bytes());
    out.push(0xF7); // Global Color Table flag (1), color res (7), sort (0), size = 7 (256 entries)
    out.push(0);
    out.push(0);

    // Global Color Table: 216 web-safe 6x6x6 cube + 40 grayscale ramps = 256 colors
    for r in 0..6u8 {
        for g in 0..6u8 {
            for b in 0..6u8 {
                out.extend_from_slice(&[r * 51, g * 51, b * 51]);
            }
        }
    }
    for i in 0..40u8 {
        let v = (u16::from(i) * 255 / 39) as u8;
        out.extend_from_slice(&[v, v, v]);
    }

    // Netscape 2.0 Application Extension for infinite looping
    out.extend_from_slice(&[
        0x21, 0xFF, 0x0B, b'N', b'E', b'T', b'S', b'C', b'A', b'P', b'E', b'2', b'.', b'0', 0x03,
        0x01, 0x00, 0x00, 0x00,
    ]);

    const BAYER_4X4: [[i16; 4]; 4] = [
        [-15, 5, -10, 10],
        [15, -5, 20, 0],
        [-5, 15, -15, 5],
        [20, 0, 10, -10],
    ];

    for pixmap in frames {
        out.extend_from_slice(&[0x21, 0xF9, 0x04, 0x04]);
        out.extend_from_slice(&delay_cs.to_le_bytes());
        out.push(0);
        out.push(0);

        out.push(0x2C);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&w.to_le_bytes());
        out.extend_from_slice(&h.to_le_bytes());
        out.push(0x00);

        let rgba = pixmap_to_straight_rgba(pixmap);
        let mut indices = Vec::with_capacity(usize::from(w) * usize::from(h));
        for (i, px) in rgba.chunks_exact(4).enumerate() {
            let x = i % usize::from(w);
            let y = i / usize::from(w);
            let dither = BAYER_4X4[y & 3][x & 3];
            let r = (i16::from(px[0]) + dither).clamp(0, 255) as u8;
            let g = (i16::from(px[1]) + dither).clamp(0, 255) as u8;
            let b = (i16::from(px[2]) + dither).clamp(0, 255) as u8;
            if r.abs_diff(g) <= 6 && g.abs_diff(b) <= 6 {
                let gray_idx = 216 + ((u16::from(r) * 39 + 127) / 255) as u8;
                indices.push(gray_idx);
            } else {
                let ri = ((u16::from(r) * 5 + 127) / 255) as u8;
                let gi = ((u16::from(g) * 5 + 127) / 255) as u8;
                let bi = ((u16::from(b) * 5 + 127) / 255) as u8;
                indices.push(ri * 36 + gi * 6 + bi);
            }
        }

        lzw_encode_gif_blocks(&indices, &mut out);
    }

    out.push(0x3B);
    Ok(out)
}

fn lzw_encode_gif_blocks(indices: &[u8], out: &mut Vec<u8>) {
    out.push(8);
    let clear_code: u16 = 256;
    let eoi_code: u16 = 257;

    let mut bit_buf: u32 = 0;
    let mut bit_count: u32 = 0;
    let mut stream = Vec::with_capacity(indices.len() * 9 / 8 + 32);

    let mut push_code = |code: u16, bits: u32| {
        bit_buf |= u32::from(code) << bit_count;
        bit_count += bits;
        while bit_count >= 8 {
            stream.push((bit_buf & 0xFF) as u8);
            bit_buf >>= 8;
            bit_count -= 8;
        }
    };

    push_code(clear_code, 9);
    for (i, &idx) in indices.iter().enumerate() {
        if i > 0 && i % 240 == 0 {
            push_code(clear_code, 9);
        }
        push_code(u16::from(idx), 9);
    }
    push_code(eoi_code, 9);
    if bit_count > 0 {
        stream.push((bit_buf & 0xFF) as u8);
    }

    for chunk in stream.chunks(255) {
        out.push(chunk.len() as u8);
        out.extend_from_slice(chunk);
    }
    out.push(0);
}

#[cfg(test)]
mod tests {
    use super::*;
    use vello_cpu::color::PremulRgba8;

    fn test_pixmap() -> Pixmap {
        let mut p = Pixmap::new(4, 2);
        for (i, px) in p.data_mut().iter_mut().enumerate() {
            *px = PremulRgba8 {
                r: (i as u8) * 20,
                g: 100,
                b: 200,
                a: 255,
            };
        }
        p
    }

    #[test]
    fn qoi_round_trip_is_bit_exact() {
        let p = test_pixmap();
        let qoi = pixmap_to_qoi(&p);
        assert_eq!(&qoi[0..4], b"qoif");
        let (w, h, rgba) = decode_qoi(&qoi).unwrap();
        assert_eq!((w, h), (4, 2));
        assert_eq!(rgba, pixmap_to_straight_rgba(&p));
    }

    #[test]
    fn bmp_round_trip_is_bit_exact() {
        let p = test_pixmap();
        let bmp = pixmap_to_bmp(&p);
        assert_eq!(&bmp[0..2], b"BM");
        let (w, h, rgba) = decode_bmp(&bmp).unwrap();
        assert_eq!((w, h), (4, 2));
        assert_eq!(rgba, pixmap_to_straight_rgba(&p));
    }

    #[test]
    fn ppm_and_tga_round_trip_is_bit_exact() {
        let p = test_pixmap();
        let ppm = pixmap_to_ppm(&p);
        let (w, h, rgba_ppm) = decode_any_image(&ppm).unwrap();
        assert_eq!((w, h), (4, 2));
        assert_eq!(rgba_ppm, pixmap_to_straight_rgba(&p));

        let tga = pixmap_to_tga(&p);
        let (w2, h2, rgba_tga) = decode_any_image(&tga).unwrap();
        assert_eq!((w2, h2), (4, 2));
        assert_eq!(rgba_tga, pixmap_to_straight_rgba(&p));
    }

    #[test]
    fn animated_gif_has_valid_header_and_trailer() {
        let p1 = test_pixmap();
        let p2 = test_pixmap();
        let gif = encode_animated_gif(&[p1, p2], 20, 1).unwrap();
        assert_eq!(&gif[0..6], b"GIF89a");
        assert_eq!(gif.last().copied(), Some(0x3B));
    }
}
