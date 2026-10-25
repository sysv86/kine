# Kine — Video-as-Code Engine & Native Xilem Studio

Kine is a deterministic **Video-as-Code** engine, headless session/bridge library (`kine-session`), and **native Rust Xilem Studio** (`kine-studio`) targeting **Linux**, **Windows**, and **macOS**.

**Self-contained pipeline: zero system C-library dependencies (embedded QuickJS compiled via cc).**

## Supported Platforms

| Platform | Architectures | Portable Default Pipeline | Optional GPU / Hardware Video (`--features gpu,hwencode,asm`) |
| :--- | :--- | :--- | :--- |
| **Linux** | `x86_64`, `aarch64` | `vello_cpu` + CPU WGSL VM + `rav1e` AV1 / H.266 VVC | Vulkan (`wgpu` + `vello_gpu`) + Vulkan Video H.264 + NASM `asm` |
| **Windows** | `x86_64`, `aarch64` | `vello_cpu` + CPU WGSL VM + `rav1e` AV1 / H.266 VVC | DirectX 12 / Vulkan (`wgpu` + `vello_gpu`) + Vulkan Video H.264 + NASM `asm` |
| **macOS** | `aarch64` (Apple Silicon), `x86_64` | `vello_cpu` + CPU WGSL VM + `rav1e` AV1 / H.266 VVC | Metal (`wgpu` + `vello_gpu`) + VideoToolbox H.264 + GAS `asm` |

The default CLI build (`cargo build -p kine-cli`) compiles a headless, minimal dependency graph without Xilem, Winit, Wayland, or X11 system libraries (`taffy` layout, `cosmic-text` shaping, `vello_cpu` sparse-strip rasterization, `naga` CPU WGSL shader VM, `rav1e` AV1 and software H.266 / VVC encoders, `symphonia`/`rubato`/`flacenc` audio DSP, and `muxfin`/`mkv` container muxing).

## Kine Studio (`crates/kine-studio`) & Headless Session (`crates/kine-session`)

All headless Studio state (`StudioState`), NLE timeline track construction (`timeline.rs`), node/shader inspection (`inspector.rs`), preset catalog (`catalog.rs`), the 34 `studio_*` tools (`agent_tools.rs`), and the TCP bridge server (`bridge.rs`) live in `crates/kine-session` with zero GUI toolkit dependencies.

`crates/kine-studio` contains only the reactive **Xilem `0.4.0`** desktop view tree (`masonry`, `vello`, `peniko`, `parley`, and `winit`) layered over `kine-session`.

### Key Studio Capabilities

1. **Single-Process Engine Synchronization**:
   - `StudioState` owns `kine_server::Engine`, `SceneHandle`, and the authored `kine_script::SceneDef` AST **in the same OS process**.
   - Every edit in the Design Inspector, NLE Timeline, Code Editor, or External CLI Agent Bridge mutates `SceneDef` inside a checkpointed undo/redo transaction, rebuilds `kine_core::Scene` in-memory, evaluates animations & layout at the current playhead timestamp, and converts `vello_cpu::Pixmap` directly into `xilem::masonry::peniko::ImageData` (`AlphaPremultiplied` RGBA8) with zero IPC or process duplication.
2. **Hyperframes-Style 4-Zone Workspace**:
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
     - `Renders` Queue: In-process multi-format export (`MP4` AV1 or H.266/VVC, `MKV` AV1 or H.266/VVC, `WebM`, `GIF`, `PNG`) with frame count, byte size, and output path history.

## Benchmarks

The benchmark table below is populated **only** when the manual **Benchmark (`workflow_dispatch`)** GitHub Actions workflow (`.github/workflows/benchmark.yml`) is triggered (`gh workflow run benchmark.yml` or via the GitHub Actions UI). It does not run automatically on push or pull_request.

<!-- BENCHMARK_TABLE_START -->
| Stage / Workload | Resolution / Spec | Avg Latency | Throughput | Output Size | Host / Config |
         | :--- | :--- | ---: | ---: | ---: | :--- |
         | **Scene Parse + Static Inspect** | 1280×720 AST | `1.42 ms/op` | `703.7 ops/s` | — | `linux` (`x86_64`) |
         | **CPU Frame Rasterization (`vello_cpu`)** | 1280×720 RGBA8 | `2.59 ms/frame` | `385.6 fps` | `3,686,400 B` | `vello_cpu-sparse-strips` |
         | **BT.709 Fixed-Point RGBA → I420** | 1280×720 4:2:0 | `3.85 ms/frame` | `259.8 fps` | `1,382,400 B` | `16-bit int + MT` |
         | **H.266 / VVC Encode + MP4 (`vvc1`/`vvcC`)** | 1280×720 · 12f | `10.27 ms/frame` | `97.4 fps` | `2568606 B` | `software VVC` |
         | **AV1 Encode + MP4 (`av01`, speed 10)** | 1280×720 · 12f | `54.12 ms/frame` | `18.5 fps` | `11896 B` | `rav1e (asm=true)` |
         | **Headless Session Agent Tool (`studio_status`)** | `kine-session` | `0.02 ms/call` | `47010.7 calls/s` | — | `in-process` |
<!-- BENCHMARK_TABLE_END -->

To run the benchmark locally or update the table manually:

```bash
cargo run --release -p kine-cli -- bench --markdown
cargo run --release -p kine-cli -- bench --update-readme README.md
```

### Building & Launching

```bash
# Minimal headless CLI build (no Xilem/Winit/Wayland/X11 dependencies)
cargo build --release -p kine-cli

# Server build with NASM assembly acceleration for rav1e
cargo build --release -p kine-cli --features asm

# Render with software H.266 / VVC (ISO-BMFF vvc1/vvcC MP4 or Matroska V_MPEGH/ISO/VVC MKV)
cargo run --release -p kine-cli -- render --scene examples/hello/scene.ts --out out.mp4 --encoder h266
cargo run --release -p kine-cli -- render --scene examples/hello/scene.ts --out out.mkv --encoder vvc

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
- `crates/kine-encode` — Software AV1 (`rav1e`, with optional `asm` NASM/GAS feature), software H.266 / VVC (`vvc`, ITU-T H.266 Annex-B), `YUV4MPEG2` (`.y4m`), 16-bit fixed-point BT.709 `RGBA8`/`I420`/`NV12`, and indexed lossless `.kvid` encoders.
- `crates/kine-audio` — Waveform synthesis, file decoding (`symphonia`), resampling (`rubato`), biquad EQ, dynamic range compression, sidechain ducking, and `WAV`/`FLAC`/`AIFF` output.
- `crates/kine-mux` — ISO-BMFF `.mp4` (`av01`, `vvc1`/`vvcC`, `avc1`, `dfLa` FLAC) and Matroska `.mkv` / `.webm` (`V_AV1`, `V_MPEGH/ISO/VVC`, `V_MPEG4/ISO/AVC`, `A_FLAC`) muxers, plus optional hardware H.264 encoding (`gpu-video`).
- `crates/kine-script` — TypeScript stripping (`oxc`), embedded QuickJS (`rquickjs`, compiled via `cc`) evaluation, and JSON/TS scene authoring schema (`SceneDef`, `NodeDef`).
- `crates/kine-platform` — Host OS detection (`Linux`, `Windows`, `macOS`) and output path resolution (`KINE_OUTPUT_DIR`).
- `crates/kine-server` — In-process `Engine` API, static scene inspector, snapshot pixel-diff gate, and newline-delimited JSON-RPC TCP server.
- `crates/kine-session` — Headless `StudioState`, NLE timeline builder (`timeline.rs`), node/shader inspector (`inspector.rs`), preset catalog (`catalog.rs`), `StudioBridgeServer` (`bridge.rs`), `SceneFileWatcher`, and 34 `studio_*` agent tools (`agent_tools.rs`) with zero GUI dependencies.
- `crates/kine-studio` — Native Rust **Xilem** desktop view tree and window runner layered over `kine-session`.
- `crates/kine-cli` — Minimal headless `kine` CLI binary (`default = ["gpu"]`, no Xilem/Wayland/X11 dependencies by default; optional `studio-ui` and `asm` features).
