//! Kine agent server: the in-process engine API + a newline-delimited
//! JSON-RPC loop over TCP.

pub mod engine;
pub mod rpc;
pub mod video;

pub use engine::{DiffReport, Engine, Finding, FindingSeverity, InspectReport, SceneHandle};
pub use kine_render::{
    decode_video_bytes, decode_video_source, write_image_sequence_dir, DecodedVideoClip,
    FrameFormat, RenderBackend,
};
pub use video::{
    mix_scene_audio, render_video, transcode_video_clip, VideoContainer, VideoEncoder,
    VideoOptions, VideoReport,
};

/// Benchmark helper converting premultiplied RGBA8 pixels to BT.709 limited-range I420.
pub fn rgba_to_i420_bench_shim(rgba: &[u8], width: u32, height: u32) -> usize {
    let f = kine_encode::rgba_to_i420(rgba, width, height);
    f.y.len() + f.u.len() + f.v.len()
}
