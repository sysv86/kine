//! The JNI surface the Android app links against.

use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::Path;
use std::sync::mpsc::{self, Sender};
use std::sync::{Mutex, OnceLock};

use jni::objects::{JByteArray, JClass, JString};
use jni::sys::{jbyteArray, jint, jlong, jstring};
use jni::JNIEnv;

use kine_server::engine::{Engine, SceneHandle};
use kine_server::video::{VideoContainer, VideoEncoder, VideoOptions};
use kine_server::RenderBackend;

const BUNDLED_FONT: &[u8] = include_bytes!("../../../assets/fonts/JetBrainsMono-Regular.ttf");

enum Request {
    Doctor(Sender<Result<String, String>>),
    LoadFont {
        family: String,
        data: Vec<u8>,
        reply: Sender<Result<(), String>>,
    },
    LoadScene {
        json: String,
        base_dir: String,
        reply: Sender<Result<u64, String>>,
    },
    FreeScene {
        handle: u64,
        reply: Sender<Result<(), String>>,
    },
    Inspect {
        handle: u64,
        reply: Sender<Result<String, String>>,
    },
    Timeline {
        handle: u64,
        reply: Sender<Result<String, String>>,
    },
    RenderFramePng {
        handle: u64,
        frame: u64,
        reply: Sender<Result<Vec<u8>, String>>,
    },
    RenderVideo {
        handle: u64,
        options_json: String,
        out_path: String,
        reply: Sender<Result<(), String>>,
    },
}

fn engine_thread() -> Result<&'static Sender<Request>, String> {
    static TX: OnceLock<Sender<Request>> = OnceLock::new();
    if let Some(tx) = TX.get() {
        return Ok(tx);
    }
    let (tx, rx) = mpsc::channel::<Request>();
    std::thread::Builder::new()
        .name("kine-engine".into())
        .spawn(move || {
            let mut engine = Engine::new();
            engine.load_font("JetBrains Mono", BUNDLED_FONT.to_vec());
            while let Ok(request) = rx.recv() {
                serve(&mut engine, request);
            }
        })
        .map_err(|e| format!("could not start the engine thread: {e}"))?;
    let _ = TX.set(tx);
    TX.get()
        .ok_or_else(|| "the engine thread did not start".to_string())
}

fn serve(engine: &mut Engine, request: Request) {
    match request {
        Request::Doctor(reply) => {
            let json = serde_json::to_string(&engine.doctor()).map_err(|e| e.to_string());
            let _ = reply.send(json);
        }
        Request::LoadFont {
            family,
            data,
            reply,
        } => {
            engine.load_font(&family, data);
            let _ = reply.send(Ok(()));
        }
        Request::LoadScene {
            json,
            base_dir,
            reply,
        } => {
            let result = engine
                .load_scene_json_with_base(&json, Path::new(&base_dir))
                .map(|handle| handle.0)
                .map_err(|e| e.to_string());
            let _ = reply.send(result);
        }
        Request::FreeScene { handle, reply } => {
            let freed = engine.free_scene(SceneHandle(handle));
            let _ = reply.send(if freed {
                Ok(())
            } else {
                Err(format!("no scene with handle {handle}"))
            });
        }
        Request::Inspect { handle, reply } => {
            let result = engine
                .inspect(SceneHandle(handle))
                .map_err(|e| e.to_string())
                .and_then(|report| serde_json::to_string(&report).map_err(|e| e.to_string()));
            let _ = reply.send(result);
        }
        Request::Timeline { handle, reply } => {
            let result = engine
                .timeline(SceneHandle(handle))
                .map(|value| value.to_string())
                .map_err(|e| e.to_string());
            let _ = reply.send(result);
        }
        Request::RenderFramePng {
            handle,
            frame,
            reply,
        } => {
            let result = engine
                .render_frame_png(SceneHandle(handle), frame)
                .map_err(|e| e.to_string());
            let _ = reply.send(result);
        }
        Request::RenderVideo {
            handle,
            options_json,
            out_path,
            reply,
        } => {
            let result = render_video_to_path(engine, handle, &options_json, &out_path);
            let _ = reply.send(result);
        }
    }
}

fn render_video_to_path(
    engine: &mut Engine,
    handle: u64,
    options_json: &str,
    out_path: &str,
) -> Result<(), String> {
    let parsed: VideoOptionsJson = serde_json::from_str(options_json)
        .map_err(|e| format!("could not read the render options: {e}"))?;
    if let Some(backend) = parsed.parsed_backend()? {
        engine.set_backend(backend);
    }
    let mut options = parsed.into_options()?;
    let target = Path::new(out_path);
    if let Some(container) = VideoContainer::from_path(target) {
        options.container = container;
    }
    let report = kine_server::render_video(engine, SceneHandle(handle), &options)
        .map_err(|e| e.to_string())?;

    if let Some(parent) = target.parent() {
        if !parent.as_os_str().is_empty() {
            let _ = std::fs::create_dir_all(parent);
        }
    }

    let tmp = format!("{out_path}.part");
    if std::fs::write(&tmp, &report.bytes).is_ok() {
        if std::fs::rename(&tmp, out_path).is_ok() {
            return Ok(());
        }
        let _ = std::fs::remove_file(&tmp);
    }

    if let Err(e) = std::fs::write(out_path, &report.bytes) {
        let _ = std::fs::remove_file(out_path);
        return Err(format!("could not write the clip: {e}"));
    }
    Ok(())
}

fn ask<T>(build: impl FnOnce(Sender<Result<T, String>>) -> Request) -> Result<T, String> {
    let (tx, rx) = mpsc::channel();
    let sender = engine_thread()?;
    sender
        .send(build(tx))
        .map_err(|_| "the engine thread has stopped".to_string())?;
    rx.recv()
        .map_err(|_| "the engine thread did not answer".to_string())?
}

fn last_error() -> &'static Mutex<String> {
    static LAST_ERROR: Mutex<String> = Mutex::new(String::new());
    &LAST_ERROR
}

fn fail(message: impl Into<String>) -> jint {
    if let Ok(mut slot) = last_error().lock() {
        *slot = message.into();
    }
    -1
}

fn clear_error() {
    if let Ok(mut slot) = last_error().lock() {
        slot.clear();
    }
}

fn jstring_of(env: &mut JNIEnv, value: &str) -> jstring {
    match env.new_string(value) {
        Ok(s) => s.into_raw(),
        Err(e) => {
            fail(format!("could not allocate a Java string: {e}"));
            std::ptr::null_mut()
        }
    }
}

fn jbytes_of(env: &mut JNIEnv, value: &[u8]) -> jbyteArray {
    match env.byte_array_from_slice(value) {
        Ok(a) => a.into_raw(),
        Err(e) => {
            fail(format!("could not allocate a Java byte array: {e}"));
            std::ptr::null_mut()
        }
    }
}

fn rust_string(env: &mut JNIEnv, value: &JString) -> Option<String> {
    match env.get_string(value) {
        Ok(s) => Some(s.into()),
        Err(e) => {
            fail(format!("could not read a Java string: {e}"));
            None
        }
    }
}

fn guarded<T>(on_panic: T, body: impl FnOnce() -> T) -> T {
    match catch_unwind(AssertUnwindSafe(body)) {
        Ok(value) => value,
        Err(payload) => {
            let message = payload
                .downcast_ref::<&str>()
                .map(|s| (*s).to_string())
                .or_else(|| payload.downcast_ref::<String>().cloned())
                .unwrap_or_else(|| "panicked with a non-string payload".to_string());
            fail(format!("engine panicked: {message}"));
            on_panic
        }
    }
}

#[derive(Debug, Default, serde::Deserialize)]
struct VideoOptionsJson {
    encoder: Option<String>,
    container: Option<String>,
    backend: Option<String>,
    fps_num: Option<u32>,
    fps_den: Option<u32>,
    frames: Option<u64>,
    quantizer: Option<usize>,
    speed: Option<u8>,
    keyframe_interval: Option<u32>,
    bitrate: Option<u64>,
}

impl VideoOptionsJson {
    fn parsed_backend(&self) -> Result<Option<RenderBackend>, String> {
        let Some(name) = self.backend.as_deref() else {
            return Ok(None);
        };
        RenderBackend::parse(name)
            .map(Some)
            .ok_or_else(|| format!("unknown backend {name:?} (expected auto, gpu or cpu)"))
    }

    fn into_options(self) -> Result<VideoOptions, String> {
        let mut opts = VideoOptions::default();
        if let Some(name) = self.encoder.as_deref() {
            opts.encoder = match name {
                "av1" | "software" => VideoEncoder::Av1Software,
                #[cfg(feature = "hwencode")]
                "h264-hw" | "hardware" | "h264" => VideoEncoder::H264Hardware,
                #[cfg(feature = "zero-copy")]
                "h264-zero-copy" | "zero-copy" => VideoEncoder::H264ZeroCopy,
                other => return Err(format!("unknown encoder {other:?}")),
            };
        }
        if let Some(c_name) = self.container.as_deref() {
            opts.container = VideoContainer::from_extension(c_name)
                .ok_or_else(|| format!("unknown container {c_name:?}"))?;
        }
        if let Some(v) = self.fps_num {
            opts.fps_num = v;
        }
        if let Some(v) = self.fps_den {
            opts.fps_den = v;
        }
        if let Some(v) = self.frames {
            opts.frames = v;
        }
        if let Some(v) = self.quantizer {
            opts.quantizer = v;
        }
        if let Some(v) = self.speed {
            opts.speed = v;
        }
        if let Some(v) = self.keyframe_interval {
            opts.keyframe_interval = v;
        }
        if let Some(v) = self.bitrate {
            opts.bitrate = v;
        }
        if opts.fps_den == 0 || opts.fps_num == 0 {
            return Err("the frame rate must have a non-zero numerator and denominator".into());
        }
        Ok(opts)
    }
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeVersion<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    guarded(std::ptr::null_mut(), || {
        jstring_of(&mut env, env!("CARGO_PKG_VERSION"))
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeDoctor<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    guarded(std::ptr::null_mut(), || match ask(Request::Doctor) {
        Ok(json) => jstring_of(&mut env, &json),
        Err(e) => {
            fail(e);
            std::ptr::null_mut()
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeLoadFont<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    family: JString<'local>,
    data: JByteArray<'local>,
) -> jint {
    guarded(-1, || {
        let Some(family) = rust_string(&mut env, &family) else {
            return -1;
        };
        let bytes = match env.convert_byte_array(&data) {
            Ok(bytes) => bytes,
            Err(e) => return fail(format!("could not read the font bytes: {e}")),
        };
        match ask(|reply| Request::LoadFont {
            family,
            data: bytes,
            reply,
        }) {
            Ok(()) => {
                clear_error();
                0
            }
            Err(e) => fail(e),
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeLoadScene<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    json: JString<'local>,
    base_dir: JString<'local>,
) -> jlong {
    guarded(0, || {
        let Some(json) = rust_string(&mut env, &json) else {
            return 0;
        };
        let Some(base_dir) = rust_string(&mut env, &base_dir) else {
            return 0;
        };
        match ask(|reply| Request::LoadScene {
            json,
            base_dir,
            reply,
        }) {
            Ok(handle) => {
                clear_error();
                handle as jlong
            }
            Err(e) => {
                fail(e);
                0
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeFreeScene<'local>(
    _env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jint {
    guarded(-1, || {
        match ask(|reply| Request::FreeScene {
            handle: handle as u64,
            reply,
        }) {
            Ok(()) => {
                clear_error();
                0
            }
            Err(e) => fail(e),
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeInspect<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jstring {
    guarded(std::ptr::null_mut(), || {
        match ask(|reply| Request::Inspect {
            handle: handle as u64,
            reply,
        }) {
            Ok(json) => jstring_of(&mut env, &json),
            Err(e) => {
                fail(e);
                std::ptr::null_mut()
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeTimeline<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
) -> jstring {
    guarded(std::ptr::null_mut(), || {
        match ask(|reply| Request::Timeline {
            handle: handle as u64,
            reply,
        }) {
            Ok(json) => jstring_of(&mut env, &json),
            Err(e) => {
                fail(e);
                std::ptr::null_mut()
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeRenderFramePng<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    frame: jlong,
) -> jbyteArray {
    guarded(std::ptr::null_mut(), || {
        match ask(|reply| Request::RenderFramePng {
            handle: handle as u64,
            frame: frame.max(0) as u64,
            reply,
        }) {
            Ok(png) => {
                clear_error();
                jbytes_of(&mut env, &png)
            }
            Err(e) => {
                fail(e);
                std::ptr::null_mut()
            }
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeRenderVideoToFile<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
    handle: jlong,
    options_json: JString<'local>,
    out_path: JString<'local>,
) -> jint {
    guarded(-1, || {
        let Some(options_json) = rust_string(&mut env, &options_json) else {
            return -1;
        };
        let Some(out_path) = rust_string(&mut env, &out_path) else {
            return -1;
        };
        match ask(|reply| Request::RenderVideo {
            handle: handle as u64,
            options_json,
            out_path,
            reply,
        }) {
            Ok(()) => {
                clear_error();
                0
            }
            Err(e) => fail(e),
        }
    })
}

#[no_mangle]
pub extern "system" fn Java_com_kerneldroid_kine_data_KineBridge_nativeLastError<'local>(
    mut env: JNIEnv<'local>,
    _class: JClass<'local>,
) -> jstring {
    let message = match last_error().lock() {
        Ok(slot) => slot.clone(),
        Err(_) => "the error slot was poisoned by an earlier panic".to_string(),
    };
    jstring_of(&mut env, &message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_options_mirror_defaults_to_the_engine_defaults() {
        let from_json = VideoOptionsJson::default().into_options().unwrap();
        let engine = VideoOptions::default();
        assert_eq!(from_json.frames, engine.frames);
        assert_eq!(from_json.fps_num, engine.fps_num);
        assert_eq!(from_json.fps_den, engine.fps_den);
        assert_eq!(from_json.quantizer, engine.quantizer);
        assert_eq!(from_json.speed, engine.speed);
        assert_eq!(from_json.keyframe_interval, engine.keyframe_interval);
    }

    #[test]
    fn an_unknown_encoder_is_refused_rather_than_ignored() {
        let json = VideoOptionsJson {
            encoder: Some("hevc".into()),
            ..Default::default()
        };
        assert!(json.into_options().is_err());
    }

    #[test]
    fn a_zero_frame_rate_is_refused() {
        let json = VideoOptionsJson {
            fps_num: Some(0),
            ..Default::default()
        };
        assert!(json.into_options().is_err());
    }

    #[test]
    fn the_last_error_slot_records_and_clears() {
        clear_error();
        assert_eq!(last_error().lock().unwrap().as_str(), "");
        fail("something went wrong");
        assert_eq!(last_error().lock().unwrap().as_str(), "something went wrong");
        clear_error();
        assert_eq!(last_error().lock().unwrap().as_str(), "");
    }

    #[test]
    fn a_panic_is_turned_into_a_sentinel_and_a_message() {
        clear_error();
        let value = guarded(-1, || panic!("deliberate"));
        assert_eq!(value, -1);
        let recorded = last_error().lock().unwrap().clone();
        assert!(
            recorded.contains("deliberate"),
            "the panic message must survive: {recorded}"
        );
    }

    #[test]
    fn the_engine_thread_answers_a_round_trip() {
        let json = ask(Request::Doctor).expect("doctor must answer");
        assert!(json.contains("version"), "unexpected doctor payload: {json}");
        assert!(
            json.contains("JetBrains Mono"),
            "bundled font must be preloaded on Android worker thread: {json}"
        );
    }

    #[test]
    fn a_missing_scene_is_an_error_rather_than_a_silent_no_op() {
        let result = ask(|reply| Request::FreeScene {
            handle: 9_999,
            reply,
        });
        assert!(result.is_err(), "freeing a handle that never existed must fail");
    }
}
