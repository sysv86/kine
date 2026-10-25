//! Dump a real H.266/VVC Annex-B stream plus the matching raw I420 source.
//!
//! Used by the external conformance check (`scripts/verify-vvc.sh`), which
//! decodes the stream with an independent decoder (VVdeC / FFmpeg) and compares
//! the result byte-for-byte with `source.yuv`.
//!
//! ```text
//! cargo run -p kine-encode --example vvc_dump -- /tmp/opencode/vvc/out 200 120 3 0
//! ```
//!
//! Arguments: `<output-prefix> [width] [height] [frames] [quantizer] [noise]`.

use kine_encode::{color::rgba_to_i420, decode_vvc_annexb_to_i420, EncoderSettings, VvcEncoder};
use std::fs;

fn scene(width: u32, height: u32, frame_index: u32, noise: bool) -> Vec<u8> {
    let mut rgba = Vec::with_capacity((width * height * 4) as usize);
    let bar_x = (frame_index * 7) % width;
    let mut seed = 0x1234_5678u32 ^ frame_index;
    for y in 0..height {
        for x in 0..width {
            let in_bar = x >= bar_x && x < bar_x + 11 && y >= height / 3 && y < 2 * height / 3;
            if noise {
                for _ in 0..3 {
                    seed = seed.wrapping_mul(1664525).wrapping_add(1013904223);
                    rgba.push((seed >> 24) as u8);
                }
                rgba.push(255);
            } else if in_bar {
                rgba.extend_from_slice(&[245, 92, 38, 255]);
            } else {
                let r = (x * 255 / width.max(1)) as u8;
                let g = (y * 255 / height.max(1)) as u8;
                let b = ((x + y) % 256) as u8;
                rgba.extend_from_slice(&[r, g, b, 255]);
            }
        }
    }
    rgba
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let prefix = args
        .get(1)
        .cloned()
        .unwrap_or_else(|| "/tmp/opencode/vvc/kine".to_string());
    let width: u32 = args.get(2).and_then(|s| s.parse().ok()).unwrap_or(200);
    let height: u32 = args.get(3).and_then(|s| s.parse().ok()).unwrap_or(120);
    let frames: u32 = args.get(4).and_then(|s| s.parse().ok()).unwrap_or(3);
    let quantizer: usize = args.get(5).and_then(|s| s.parse().ok()).unwrap_or(0);
    let noise = args.get(6).map(|s| s == "noise").unwrap_or(false);

    let mut settings = EncoderSettings::new(width, height, 30, 1);
    settings.quantizer = quantizer;
    let mut encoder = VvcEncoder::new(settings).expect("create VVC encoder");
    let (visible_w, visible_h) = encoder.visible_size();

    let mut stream = Vec::new();
    let mut source_yuv = Vec::new();
    for index in 0..frames {
        let rgba = scene(width, height, index, noise);
        let i420 = rgba_to_i420(&rgba, width, height);
        // The encoder rounds odd dimensions down to the nearest even sample;
        // keep the reference file in sync with the visible picture.
        for y in 0..visible_h as usize {
            let row = &i420.y[y * width as usize..y * width as usize + visible_w as usize];
            source_yuv.extend_from_slice(row);
        }
        let cw = visible_w.div_ceil(2) as usize;
        let ch = visible_h.div_ceil(2) as usize;
        for plane in [&i420.u, &i420.v] {
            let src_stride = width.div_ceil(2) as usize;
            for y in 0..ch {
                source_yuv.extend_from_slice(&plane[y * src_stride..y * src_stride + cw]);
            }
        }
        let out = encoder.encode_rgba(&rgba).expect("encode frame");
        for unit in out.units {
            stream.extend_from_slice(&unit.data);
        }
    }
    for unit in encoder.finish().expect("finish").units {
        stream.extend_from_slice(&unit.data);
    }

    // Decode with our own subset decoder too, so lossy streams can be compared
    // with an independent decoder's reconstruction (`recon.yuv`).
    let decoded = decode_vvc_annexb_to_i420(&[stream.as_slice()]).expect("decode own stream");
    let mut recon_yuv = Vec::new();
    for frame in &decoded {
        recon_yuv.extend_from_slice(&frame.y);
        recon_yuv.extend_from_slice(&frame.u);
        recon_yuv.extend_from_slice(&frame.v);
    }

    fs::write(format!("{prefix}.vvc"), &stream).expect("write bitstream");
    fs::write(format!("{prefix}.yuv"), &source_yuv).expect("write source yuv");
    fs::write(format!("{prefix}.recon.yuv"), &recon_yuv).expect("write reconstruction");
    println!(
        "wrote {prefix}.vvc ({} bytes), {prefix}.yuv ({} bytes), {prefix}.recon.yuv ({} bytes), {visible_w}x{visible_h}, {frames} frames",
        stream.len(),
        source_yuv.len(),
        recon_yuv.len()
    );
}
