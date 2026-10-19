//! Pixmap → PNG encoding (straight-alpha RGBA8).

use image::codecs::png::PngEncoder;
use image::ColorType;
use image::ImageEncoder;
use vello_cpu::Pixmap;

/// Encode a premultiplied pixmap as a PNG file image (straight alpha).
pub fn pixmap_to_png(pixmap: &Pixmap) -> Result<Vec<u8>, image::ImageError> {
    let w = u32::from(pixmap.width());
    let h = u32::from(pixmap.height());

    let mut straight = Vec::with_capacity(w as usize * h as usize * 4);
    for p in pixmap.data() {
        let a = u32::from(p.a);
        if a == 0 {
            straight.extend_from_slice(&[0, 0, 0, 0]);
        } else {
            let unpremul = |c: u8| ((u32::from(c) * 255 + a / 2) / a).min(255) as u8;
            straight.extend_from_slice(&[unpremul(p.r), unpremul(p.g), unpremul(p.b), p.a]);
        }
    }

    let mut out = Vec::new();
    let encoder = PngEncoder::new(&mut out);
    encoder.write_image(&straight, w, h, ColorType::Rgba8.into())?;
    Ok(out)
}
