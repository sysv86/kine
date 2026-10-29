# Kine — Video-as-Code Engine & Native Xilem Studio

Kine is a deterministic **Video-as-Code** engine, headless session/bridge library (`kine-session`), and **native Rust Xilem Studio** (`kine-studio`) targeting **Linux**, **Windows**, and **macOS**.

**Rust-native pipeline.** No C codec is linked into the Kine binaries. AV1 and H.264 encoding run in an external FFmpeg process; rendering, compositing, audio DSP, and muxing run in-process.

## Supported Platforms

| Platform | Architectures | Portable Pipeline | Optional GPU Rasterization (`--features gpu`) |
| :--- | :--- | :--- | :--- |
| **Linux** | `x86_64`, `aarch64` | `vello_cpu` + CPU WGSL VM + FFmpeg AV1 / H.264 | Vulkan (`wgpu` + `vello_gpu`) |
| **Windows** | `x86_64`, `aarch64` | `vello_cpu` + CPU WGSL VM + FFmpeg AV1 / H.264 | DirectX 12 / Vulkan (`wgpu` + `vello_gpu`) |
| **macOS** | `aarch64` (Apple Silicon), `x86_64` | `vello_cpu` + CPU WGSL VM + FFmpeg AV1 / H.264 | Metal (`wgpu` + `vello_gpu`) |

The default CLI build (`cargo build -p kine-cli`) compiles a headless, minimal dependency graph without Xilem, Winit, Wayland, or X11 system libraries (`taffy` layout, `cosmic-text` shaping, `vello_cpu` sparse-strip rasterization, `naga` CPU WGSL shader VM, external FFmpeg AV1/H.264 encoding, `symphonia`/`rubato`/`flacenc` audio DSP, and `muxfin`/`mkv` container muxing).

## Kine Studio (`crates/kine-studio`) & Headless Session (`crates/kine-session`)

All headless Studio state (`StudioState`), NLE timeline track construction (`timeline.rs`), node/shader inspection (`inspector.rs`), preset catalog (`catalog.rs`), the 34 `studio_*` tools (`agent_tools.rs`), and the TCP bridge server (`bridge.rs`) live in `crates/kine-session` with zero GUI toolkit dependencies.

`crates/kine-studio` contains only the reactive **Xilem `0.4.0`** desktop view tree (`masonry`, `vello`, `peniko`, `parley`, and `winit`) layered over `kine-session`.

### Key Studio Capabilities

1. **Single-Process Engine Synchronization**:
   - `StudioState` owns `kine_server::Engine`, `SceneHandle`, and the authored `kine_script::SceneDef` AST **in the same OS process**.
   - Every edit in the Design Inspector, NLE Timeline, Code Editor, or External CLI Agent Bridge mutates `SceneDef` inside a checkpointed undo/redo transaction, rebuilds `kine_core::Scene` in-memory, evaluates animations & layout at the current playhead timestamp, and converts `vello_cpu::Pixmap` directly into `xilem::masonry::peniko::ImageData` (`AlphaPremultiplied` RGBA8) with zero IPC or process duplication.
2. **Four-Zone Studio Workspace**:
   - **Header Bar**: Active composition switcher, live static inspection badge, `Backend` selector (`AUTO`/`CPU`/`GPU`), `Capture PNG`, `CLI :7701` live bridge toggle, Dock visibility toggle, and one-click `Render Export`.
   - **Left Dock**:
     - `Compositions`: Built-in starter compositions + automatic discovery of `.ts` and `.json` scenes in `examples/`.
     - `Assets`: Fonts (`JetBrains Mono`), procedural patterns (`pattern:checkerboard`), procedural video clips (`video:testbars`), synthesized DSP audio beds (`synth:...`), and WGSL shader assets.
     - `Code (TS/JSON)`: Live editable scene source buffer with instant `Apply to Engine` compilation.
     - `Catalog & FX`: One-click insertion of primitives (`Rect`, `Circle`, `Text`, `Path`, `Image`, `Video`, `Audio`) and WGSL post-processing shaders (`Film Grain`, `Vignette`, `Chromatic Aberration`, `CRT Scanlines`).
   - **Center Viewport & Transport**:
     - Live rasterized composition viewport with interactive selection bounding-box + corner handles overlay, action/title safe-area reticles, and zoom controls (`25%`–`400%`).
     - Transport bar: `|<`, `< -1f`, `Play / Pause`, `+1f >`, `>|`, clickable timecode (`MM:SS.ms` vs `Frame F / Total`), interactive seek scrubber with snapping, playback rate (`0.5x`–`2.0x`), `Loop`, and `Mute`.
   - **Bottom NLE Timeline & Nodes**:
     - Timeline toolbar: `Undo` / `Redo` with history depth, `Prev KF` / `+Keyframe (K)` / `Next KF`, `Split @ Playhead`, `Duplicate`, `Delete`, `Snap (N)`, `Auto-KF`, and `Ripple` edit mode.
     - Adaptive time ruler (`generate_ruler_ticks`) and multi-track lanes for hierarchical scene nodes, animated property sub-tracks with `◆` keyframe markers, and DSP audio tracks.
   - **Right Dock**:
     - `Design` Inspector: Computed bounding box (`X`, `Y`, `W`, `H`), canvas alignment buttons, position/size nudges, opacity slider, rotation & scale steppers, hex color & swatch picker, corner radius, typography controls, and animation track easing curves (`linear`, `easeOut`, `easeOutCubic`, `spring`).
     - `Layers` Tree: Hierarchical Z-ordered scene graph with visibility toggles (`👁`/`🚫`) and sibling Z-order reordering (`↑`/`↓`).
   - **Real-Time External CLI Agent Bridge (`crates/kine-session/src/bridge.rs`)**:
     - Kine Studio deliberately omits any built-in chat wrapper. External CLI agents run in a separate terminal or window and control Kine Studio in real time over a bidirectional TCP bridge (`127.0.0.1:7701`, auto-discovered via `.kine-studio-bridge.json`) or via live file hot-reloading (`SceneFileWatcher`). End-to-end pipeline and bridge throughput metrics are published in the [Benchmarks](#benchmarks) table below:
       - **State, Selection & Inspection**: `studio_status`, `studio_selection`, `studio_look`, `studio_tree`, `studio_timeline`, `studio_inspect`
       - **Node Hierarchy & Alignment**: `studio_add_node`, `studio_delete_node`, `studio_duplicate_node`, `studio_reorder_node`, `studio_toggle_visibility`, `studio_align_node`
       - **Content, Style, Transform, WGSL Shaders & Audio**: `studio_set_text`, `studio_set_style`, `studio_transform`, `studio_set_shader`, `studio_audio`
       - **Timeline, Keyframes & Transport**: `studio_seek`, `studio_frame`, `studio_transport`, `studio_resize_canvas`, `studio_add_animation`, `studio_update_animation`, `studio_add_keyframe`, `studio_delete_keyframe`, `studio_delete_animation`, `studio_shift_time`, `studio_split`
       - **History, Scene I/O, Export & Atomic Batching**: `studio_undo`, `studio_redo`, `studio_history`, `studio_load_scene`, `studio_reload`, `studio_apply_source`, `studio_save_scene`, `studio_switch_composition`, `studio_new_composition`, `studio_export`, `studio_batch`
     - `Renders` Queue: Multi-format export (`MP4`/`MKV` AV1 or H.264, `WebM` AV1, `GIF`, `PNG`) with frame count, byte size, and output path history.

## Benchmarks

The benchmark measures Kine's **own** engine paths on the portable CPU backend: JSON and TypeScript scene compilation, animation sampling + taffy layout, dense CPU rasterization, the `naga` WGSL CPU VM post chain, in-process PNG export, the audio DSP mixer, and in-process Studio agent dispatch. The external FFmpeg codec is deliberately excluded — codec throughput is FFmpeg's metric, not Kine's. Every row is headless and CPU-only; no external binary is in the measurement path.

The table below is populated **only** when the manual **Kine Engine Benchmark (`workflow_dispatch`)** GitHub Actions workflow (`.github/workflows/benchmark.yml`) is triggered (`gh workflow run benchmark.yml` or via the GitHub Actions UI). It does not run automatically on push or pull_request.

<!-- BENCHMARK_TABLE_START -->
| Kine Subsystem | Benchmark Workload | Median Latency | Throughput | Detail | Host / Engine |
| :--- | :--- | ---: | ---: | :--- | :--- |
| **JSON scene compile** | 256 nodes · 1920×1080 | `0.67 ms/op` | `1500.4 compiles/s` | serde → SceneDef → taffy | `linux` (`x86_64`) · `cpu` |
| **TypeScript scene compile** | 200 nodes · 28 KiB TS | `2.57 ms/op` | `389.4 compiles/s` | oxc → QuickJS → taffy | `linux` (`x86_64`) · `cpu` |
| **Animation + layout** | 256 nodes · 4 s sweep | `0.09 ms/frame` | `11071.4 layouts/s` | `apply_time` + `compute_layout` | `linux` (`x86_64`) · `cpu` |
| **CPU rasterization** | 256 nodes · 1920×1080 | `38.93 ms/frame` | `25.7 fps` | `2.07 Mpix/frame` · `vello_cpu` | `linux` (`x86_64`) · `cpu` |
| **WGSL CPU VM post chain** | 3 full-frame passes · 1920×1080 | `845.18 ms/frame` | `1.2 fps` | `2.07 Mpix/frame` · `naga` VM | `linux` (`x86_64`) · `cpu` |
| **PNG frame export** | 1920×1080 RGBA8 | `59.10 ms/frame` | `16.9 fps` | `949 KiB/frame` · in-process | `linux` (`x86_64`) · `cpu` |
| **Audio DSP mix** | 8 tracks · 12 s · 48 kHz stereo | `214.41 ms/mix` | `56.0× realtime` | synth + biquad EQ + compressor | `linux` (`x86_64`) · `cpu` |
| **Studio agent dispatch** | `studio_status` + tree + selection | `23.5 µs/call` | `42507.4 calls/s` | in-process `kine-session` | `linux` (`x86_64`) · `cpu` |
<!-- BENCHMARK_TABLE_END -->

Each row is a median over repeated runs on the same workload; `n/a` would only
appear if a subsystem failed to initialise. The benchmark needs no FFmpeg and no
GPU, so `ubuntu-latest` numbers are directly comparable across runs.

To run the benchmark locally or update the table manually:

```bash
cargo run --release -p kine-cli -- bench --markdown
cargo run --release -p kine-cli -- bench --update-readme README.md
```

### Building & Launching

```bash
# Minimal headless CLI build (no Xilem/Winit/Wayland/X11 dependencies)
cargo build --release -p kine-cli

# AV1 and H.264 are encoded by FFmpeg. Fetch the latest LGPL build once:
./scripts/install-ffmpeg.sh
# Then render AV1 (`av01`) or H.264 (`avc1`) into MP4 or Matroska:
cargo run --release -p kine-cli -- render --scene examples/hello/scene.ts --out out-av1.mp4 --encoder av1
cargo run --release -p kine-cli -- render --scene examples/hello/scene.ts --out out-h264.mp4 --encoder h264
cargo run --release -p kine-cli -- render --scene examples/hello/scene.ts --out out-h264.mkv --encoder h264
# Set KINE_FFMPEG=/path/to/ffmpeg to use an existing build. On macOS,
# `brew install ffmpeg` is enough (the script validates it).

# Launch desktop Kine Studio (standalone binary or CLI with studio-ui feature)
cargo run --release -p kine-studio -- --scene examples/hello/scene.ts
cargo run --release -p kine-cli --features studio-ui -- studio --scene examples/hello/scene.ts

# Run a headless verification check (zero X11/Wayland required)
cargo run --release -p kine-cli -- studio --headless-check

# Control a running Kine Studio window in real time from an external CLI agent
kine agent status
kine agent selection --prompt
kine agent tree
kine agent set-text --node 4 --text "LIVE FROM CLI AGENT"
kine agent transform --node 2 --dx 24 --dy -12 --scale 1.1
kine agent animate --node 3 --prop rotation --from 0 --to 360 --start 0 --end 2.0 --easing easeInOutCubic
kine agent watch
```

## Modules

A project can ship ready-made elements — subtitles, widgets, transitions,
animation blocks — and reuse them from any scene without copying the code.
Modules live in a `Modules/` directory next to the scene:

```text
project/
  scene.ts
  Modules/
    lower-third/
      module.json
      scene.ts
```

`module.json` describes the module; every field except the entry script is
optional:

```json
{
  "kind": "subtitle",
  "description": "Broadcast lower third with title and subtitle",
  "params": { "title": "UNTITLED", "subtitle": "" }
}
```

Use it like any other node; the module sees the same DSL globals as the scene:

```ts
container({ size: { width: W, height: H } }, [
  module("lower-third", { title: "MODULE OK", subtitle: "ready-made element" }),
]);
```

List what a project offers:

```sh
kine modules --dir ./project
```

## Workspace Architecture

- `crates/kine-core` — Scene graph (`Scene`, `Node`, `NodeKind`), `taffy` layout integration, and keyframe animation curves (`Linear`, `EaseIn`, `EaseOut`, `EaseInOut`, `CubicBezier`, `Steps`, analytical `Spring`).
- `crates/kine-text` — Self-contained font loading and text shaping (`cosmic-text`).
- `crates/kine-shader` — WGSL shader validation (`naga`), uniform reflection, and deterministic multi-threaded CPU WGSL fragment VM.
- `crates/kine-render` — Unified CPU (`vello_cpu`) and optional GPU (`vello_gpu` + `wgpu`) frame renderer, procedural/container video input cache, and multi-format still export (`PNG`, `QOI`, `BMP`, `PPM`, `PAM`, `TGA`, `GIF89a`).
- `crates/kine-encode` — AV1 and H.264 through an external FFmpeg process (`ffmpeg` module; no Rust codec implementations), `YUV4MPEG2` (`.y4m`), 16-bit fixed-point BT.709 `RGBA8`/`I420`/`NV12`, and indexed lossless `.kvid` encoders.
- `crates/kine-audio` — Waveform synthesis, file decoding (`symphonia`), resampling (`rubato`), biquad EQ, dynamic range compression, sidechain ducking, and `WAV`/`FLAC`/`AIFF` output.
- `crates/kine-mux` — ISO-BMFF `.mp4` (`av01`, `avc1`, `dfLa` FLAC) and Matroska `.mkv` / `.webm` (`V_AV1`, `V_MPEG4/ISO/AVC`, `A_FLAC`) muxers.
- `crates/kine-script` — TypeScript stripping (`oxc`), embedded QuickJS (`rquickjs`, compiled via `cc`) evaluation, and JSON/TS scene authoring schema (`SceneDef`, `NodeDef`).
- `crates/kine-platform` — Host OS detection (`Linux`, `Windows`, `macOS`) and output path resolution (`KINE_OUTPUT_DIR`).
- `crates/kine-server` — In-process `Engine` API, static scene inspector, snapshot pixel-diff gate, and newline-delimited JSON-RPC TCP server.
- `crates/kine-session` — Headless `StudioState`, NLE timeline builder (`timeline.rs`), node/shader inspector (`inspector.rs`), preset catalog (`catalog.rs`), `StudioBridgeServer` (`bridge.rs`), `SceneFileWatcher`, and 34 `studio_*` agent tools (`agent_tools.rs`) with zero GUI dependencies.
- `crates/kine-studio` — Native Rust **Xilem** desktop view tree and window runner layered over `kine-session`.
- `crates/kine-cli` — Minimal headless `kine` CLI binary (`default = ["gpu"]`, no Xilem/Wayland/X11 dependencies by default; optional `studio-ui` feature).
