# Kine

A Rust engine for programmatic synthesis and processing of video, graphics and
audio. It takes a scene description in TypeScript or JSON, computes layout,
shapes text, rasterizes frames (CPU or GPU), executes WGSL shaders, processes
audio and packs the result into output containers.

## Workspace layout

The Cargo workspace is split into 12 crates:

* `crates/kine-core` — scene graph (`Scene`, `Node`, `NodeKind`), Flexbox layout through `taffy`, keyframes, easing curves (`Linear`, `EaseIn`, `EaseOut`, `EaseInOut`, `CubicBezier`, `Steps`) and an analytical spring solver (`Spring`).
* `crates/kine-text` — deterministic text shaping on `cosmic-text` (`harfrust`) with an isolated font database.
* `crates/kine-shader` — parsing, validation and reflection of WGSL fragment shaders via `naga`, `std140` uniform packing by reflected offsets, built-in presets (`presets.rs`) and a multithreaded software WGSL interpreter for the CPU path (`vm.rs`).
* `crates/kine-render` — scene rasterization through `vello_cpu` (CPU) and `vello_gpu` + `wgpu` (GPU), decoding and caching of input video (`video_io.rs`), export of raster and vector formats (`formats.rs`).
* `crates/kine-encode` — RGBA ↔ YUV BT.709 limited-range conversion (`I420`, `NV12`, `I444`), a software AV1 encoder (`rav1e`), `YUV4MPEG2` (`.y4m`) stream I/O, raw streams (`.rgba`, `.yuv`, `.nv12`) and the indexed `.kvid` container.
* `crates/kine-mux` — muxing video (`AV1`, `H.264`) and audio (`FLAC`) into `MP4` (`muxfin`), `Matroska` (`.mkv`) and `WebM` (`.webm`), plus hardware H.264 encoding through Vulkan Video (`hwencode.rs`).
* `crates/kine-audio` — decoding (`WAV`, `AIFF`, `FLAC`, `MP3`, `OGG`), procedural synthesis (`synth.rs`), polyphase resampling (`rubato`), a parametric biquad equalizer, a dynamics compressor, automatic sidechain ducking (`dsp.rs`), mixing (`mix.rs`) and writing `WAV`, `AIFF`, `FLAC`.
* `crates/kine-script` — TypeScript to JavaScript transpilation with `oxc`, execution in `QuickJS` (`rquickjs`), building a `Scene` from the declarative description and emitting the `kine.d.ts` declaration file.
* `crates/kine-platform` — run-environment detection (Linux, Android through `proot`, native Android) and file-write policy.
* `crates/kine-server` — high-level engine API (`Engine`), the video pipeline (`video.rs`), the static scene analyzer (`inspect`), per-pixel comparison (`snapshot_diff`) and a JSON-RPC 2.0 TCP server (`rpc.rs`).
* `crates/kine-cli` — the `kine` command-line interface.
* `crates/kine-android` — JNI bridge (`cdylib`) for calling the engine from Android applications.

## Build features

The CLI builds CPU-only by default. GPU rasterization, hardware encoding and the
GPU-to-encoder zero-copy path are feature-gated:

```sh
cargo build --release -p kine-cli                      # CPU rasterizer + software AV1
cargo build --release -p kine-cli --features gpu       # vello_gpu / wgpu rasterizer
cargo build --release -p kine-cli --features hwencode  # Vulkan Video H.264 encoder
cargo build --release -p kine-cli --features zero-copy # GPU texture handed to the encoder
                                                       # without a host round trip
```

GPU paths require a Vulkan driver. On hybrid laptops the encoder prefers the
discrete adapter; `gpu-video` is taken from the upstream `smelter` master rather
than the published 0.4.0 because that release still pins `wgpu` 29 while the
rasterizer is on `wgpu` 30.

## Video, image and audio input and output

### Video output formats
* `.mp4` — ISO-BMFF container (video `AV1` or hardware `H.264`, audio `FLAC`).
* `.mkv` — Matroska EBML v4 container (video `V_AV1` or `V_MPEG4/ISO/AVC`, audio `A_FLAC`).
* `.webm` — WebM EBML v4 container (video `V_AV1`, audio `A_FLAC`).
* `.y4m` — uncompressed `YUV4MPEG2` stream (`C420mpeg2`, BT.709 limited range).
* `.gif` — animated `GIF89a` with 4×4 Bayer dithering and LZW compression.
* `.kvid` — Kine indexed lossless container (RGBA8 frames compressed with QOI plus an offset table for `O(1)` random access that preserves the alpha channel).
* `.rgba`, `.yuv` (`.i420`), `.nv12` — raw headerless video streams.
* A frame directory (`--seq`, or a path ending in `/`) — writes a `frame_0000.<ext>` sequence in any single-frame format.
* `stdout` (`--out - --format <fmt>`) — streams container or raw bytes to standard output.

### Video input formats (the `video()` node and `kine transcode`)
* `.y4m` — `YUV4MPEG2` streams (`4:2:0`, `4:2:2`, `4:4:4`, `mono`).
* `.kvid` — indexed Kine lossless files with alpha support.
* `.gif` — animated `GIF87a` / `GIF89a` files (LZW decoder with variable code width, local palettes, transparency and frame-disposal handling).
* Image directories (`path/to/frames/` or `seq:path/to/frames`) — a name-sorted sequence of `.png`, `.qoi`, `.bmp`, `.ppm`, `.pam`, `.tga` files.
* Procedural sources `video:<kind>[:WxH[:fps[:seconds]]]`:
  * `video:smpte:160x90:30:2.0` — SMPTE test pattern with a moving marker.
  * `video:plasma:160x90:30:2.0` — animated colour field.
  * `video:grid:160x90:30:2.0` — moving coordinate grid.
* External compressed containers (`.mp4`, `.mkv`, `.webm`, `.mov`) — decoded through an `ffmpeg -> YUV4MPEG2` pipe when `ffmpeg` is available on the system.

### Single-frame and graphics formats
* Read and write: `.png` (RGBA8), `.qoi` (Quite OK Image v1.0), `.bmp` (BGRA32 / BGR24), `.ppm` (Netpbm `P6`), `.pam` (Netpbm `P7` `RGB_ALPHA`), `.tga` (Truevision TGA).
* Export only: `.svg` (standalone SVG 1.1 vector document built from the computed scene tree).

### Audio formats
* Input: `.wav` (16/24-bit PCM, 32-bit float), `.aiff` (16/24-bit big-endian PCM), `.flac`, `.mp3`, `.ogg` and procedural URIs `synth:<shape>:<hz>:<seconds>` (`sine`, `square`, `saw`, `triangle`, `white-noise`, `pink-noise`, `chord:...`, `sweep:...`).
* Output: `.wav` (`16`, `24`, `32f`), `.aiff` (`16`, `24`), `.flac` (`16`, `24`).

## Scene description (TypeScript DSL)

A scene script returns a `scene(width, height, root, options)` call:

```ts
const W = 1280;
const H = 720;

return scene(
  W,
  H,
  container({ size: { width: W, height: H }, justifyContent: "center", alignItems: "center" }, [
    rect({
      gradient: linearGradient(135, [
        { offset: 0.0, color: "#0b1020" },
        { offset: 1.0, color: "#1b2440" },
      ]),
      position: "absolute",
      inset: { top: 0, left: 0, right: 0, bottom: 0 },
    }),

    video("clip.kvid", {
      objectFit: "cover",
      cornerRadius: 12,
      start: 0.0,
      trimStart: 0.2,
      playbackRate: 1.0,
      loop: true,
      size: { width: 480, height: 270 },
    }),

    text("Kine", {
      fontFamily: "JetBrains Mono",
      fontSize: 64,
      fontWeight: 700,
      color: "#f0f4ff",
      align: "center",
      animate: {
        offsetY: { from: 40, to: 0, start: 0.0, end: 0.8, easing: spring(180, 22) },
      },
    }),

    presets.vignette({ intensity: 0.35, smoothness: 1.6 }, {
      position: "absolute",
      inset: { top: 0, left: 0, right: 0, bottom: 0 },
      size: { width: W, height: H },
    }),
  ]),
  {
    audio: {
      sampleRate: 48000,
      channels: 2,
      normalizePeakDb: -1.0,
      tracks: [
        { src: "voice.wav", gainDb: -2, start: 0.2, highpassHz: 80 },
        { src: "music.flac", gainDb: -10, loop: true, duckDb: -9, pan: -0.1 },
      ],
    },
  }
);
```

### Scene graph nodes
* `container(opts, children)` — grouping and Flexbox layout node.
* `rect(opts)` — rectangle with rounded corners (`cornerRadius`), fill (`fill` or `gradient`), stroke (`strokeColor`, `strokeWidth`) and shadow (`shadowColor`, `shadowBlur`, `shadowOffsetX`, `shadowOffsetY`).
* `circle(opts)` — circle or ellipse (`fill`, `radius`, `gradient`, `stroke*`, `shadow*`).
* `path(d, opts)` — vector outline in SVG path syntax (`M`, `L`, `H`, `V`, `C`, `Q`, `Z`).
* `image(src, opts)` — raster image (`objectFit`: `"cover" | "contain" | "fill"`, `cornerRadius`).
* `video(src, opts)` — input video stream (`start`, `trimStart`, `trimEnd`, `playbackRate`, `loop`, `objectFit`, `cornerRadius`).
* `text(content, opts)` — text block (`fontFamily`, `fontSize`, `fontWeight`, `color`, `align`).
* `shader(wgsl, opts)` — WGSL fragment shader layer (`blend`: `"normal" | "add" | "multiply" | "screen"`, `uniforms`).

### WGSL fragment shader contract
The engine injects the vertex stage and a fixed binding prelude:
* `@group(0) @binding(0) var<uniform> kine_frame: KineFrame` — fields `resolution: vec2<f32>`, `time: f32`, `frame: f32`.
* `@group(0) @binding(1) var kine_input: texture_2d<f32>` — the frame rendered so far.
* `@group(0) @binding(2) var kine_sampler: sampler` — linear sampler.
* `@group(1) @binding(0) var<uniform> <name>: <Type>` — the user parameter buffer, filled from the node's `uniforms` field.

Shader layers run in scene order; `add`, `multiply` and `screen` blend the layer
over the accumulated frame, which is what the multi-pass glow chains in
`examples/ascii_crt/scene.ts` rely on.

## Command-line interface (`kine`)

```sh
# Build
cargo build --release -p kine-cli

# Render video into the various containers and streams
kine render --scene examples/scored/scene.ts --out out.mp4 --frames 60 --fps 30
kine render --scene examples/scored/scene.ts --out out.mkv --frames 60 --fps 30
kine render --scene examples/scored/scene.ts --out out.webm --frames 60 --fps 30
kine render --scene examples/scored/scene.ts --out out.y4m --frames 60 --fps 30
kine render --scene examples/scored/scene.ts --out out.gif --frames 30 --fps 15
kine render --scene examples/scored/scene.ts --out out.kvid --frames 60 --fps 30

# GPU rasterizer into the hardware H.264 encoder, no host readback
kine render --scene examples/ascii_crt/scene.ts --out ascii.mp4 --frames 300 --fps 30 \
            --backend gpu --encoder h264-zero-copy

# Write a frame sequence into a directory
kine render --scene examples/motion/scene.ts --out frames_dir/ --format qoi --frames 30 --fps 30

# Stream Y4M to standard output
kine render --scene examples/motion/scene.ts --out - --format y4m --frames 30 --fps 30 > stream.y4m

# Render a single frame (PNG, QOI, BMP, PPM, PAM, TGA, SVG)
kine render --scene examples/hello/scene.ts --out frame.png --frame 0
kine render --scene examples/hello/scene.ts --out frame.svg --frame 0

# Inspect input video and transcode between formats
kine video-info --in out.kvid
kine transcode  --in out.kvid --out transcoded.mp4 --fps 30

# Synthesize test signals and mix a scene's audio bed
kine tone      --out tone.wav --wave sine --hz 440 --seconds 2 --bits 24
kine audio-mix --scene examples/scored/scene.ts --out mix.flac --seconds 3

# Emit TypeScript declarations (.d.ts)
kine types --out kine.d.ts

# Static checks on scene, shaders and audio, plus comparison against a golden file
kine shader   --file examples/shaders/grain.wgsl
kine inspect  --scene examples/scored/scene.ts
kine check    --scene examples/scored/scene.ts
kine snapshot --scene examples/hello/scene.ts --against golden.png --frame 0

# Contact sheet (strip) and frame overlay (onion)
kine strip --scene examples/motion/scene.ts --out strip.png --frames 6 --fps 30
kine onion --scene examples/motion/scene.ts --out onion.png --frames 6 --fps 30

# Environment diagnostics and the JSON-RPC server
kine doctor
kine host
kine serve --listen 127.0.0.1:7700
```

## Examples

* `examples/hello` — minimal scene, single frame.
* `examples/motion` — keyframed animation and video output.
* `examples/scored` — video input, graphics layers and a mixed audio bed.
* `examples/shaders` — a custom WGSL effect layer on top of the scene.
* `examples/ascii_crt` — six-pass GPU demo: raymarched 3D, ASCII quantization, three additive glow passes and a curved CRT finish.

## JSON-RPC 2.0 methods (`kine serve`)

The server accepts newline-delimited (`\n`) JSON-RPC 2.0 messages:

* `ping` — connection check.
* `doctor` — renderer, fonts, formats and platform details.
* `types` — contents of `kine.d.ts`.
* `set_backend` — pick the rasterization backend (`"auto"`, `"gpu"`, `"cpu"`).
* `load_scene` — load a scene from JSON or TypeScript, returns a `scene` handle.
* `free_scene` — drop a scene from memory.
* `render_frame` — render one frame by `frame` number or `time` (returns base64).
* `render_video` — render a clip into the chosen container (`mp4`, `mkv`, `webm`, `y4m`, `gif`, `kvid`, `rgba`, `i420`, `nv12`).
* `video_info` — metadata of an input video source (`width`, `height`, `fps_num`, `fps_den`, `frames`, `duration_seconds`).
* `render_strip` / `render_onion` — contact sheet or frame overlay.
* `audio_metrics` — peak (`peak_db`), `rms_db` and `lufs_approx` of the scene audio bed.
* `inspect` — static check of the scene tree, fonts, shaders and video inputs without rasterizing.
* `snapshot` — per-pixel comparison against a golden PNG.
* `timeline` — summary of nodes, animations, video inputs and audio tracks in the scene.

## Testing

```sh
cargo test --workspace                     # default features
cargo test --workspace --all-features      # adds the GPU and hardware-encoder paths
```

## License

MIT. The bundled font is JetBrains Mono (SIL Open Font License 1.1).
