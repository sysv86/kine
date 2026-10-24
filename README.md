# Kine — Video-as-Code Engine & Native Xilem Studio

Kine is a deterministic, self-contained **Video-as-Code** engine and **native Rust Xilem Studio** (`kine-studio`) targeting **Linux**, **Windows**, and **macOS**.

## Supported Platforms

| Platform | Architectures | Portable Default Pipeline | Optional GPU / Hardware Video (`--features gpu,hwencode`) |
| :--- | :--- | :--- | :--- |
| **Linux** | `x86_64`, `aarch64` | `vello_cpu` + CPU WGSL VM + `rav1e` AV1 + Xilem (Wayland/X11) | Vulkan (`wgpu` + `vello_gpu`) + Vulkan Video H.264 |
| **Windows** | `x86_64`, `aarch64` | `vello_cpu` + CPU WGSL VM + `rav1e` AV1 + Xilem (Win32/DX12) | DirectX 12 / Vulkan (`wgpu` + `vello_gpu`) + Vulkan Video H.264 |
| **macOS** | `aarch64` (Apple Silicon), `x86_64` | `vello_cpu` + CPU WGSL VM + `rav1e` AV1 + Xilem (AppKit/Metal) | Metal (`wgpu` + `vello_gpu`) + VideoToolbox H.264 |

The default build uses a 100% deterministic software rendering and encoding pipeline (`taffy` layout, `cosmic-text` shaping, `vello_cpu` sparse-strip rasterization, `naga` CPU WGSL shader VM, `rav1e` AV1 software encoder, `symphonia`/`rubato`/`flacenc` audio DSP, and `muxfin`/`mkv` container muxing) so every test and render produces identical results on headless Linux servers, Windows workstations, and macOS laptops.

## Kine Studio (`crates/kine-studio`)

`kine-studio` is a full-featured visual composition editor, NLE timeline, node/shader inspector, and real-time CLI agent bridge studio inspired by **Hyperframes Studio**, written in pure Rust on **Xilem `0.4.0`** (Linebender's reactive UI architecture built on `masonry`, `vello`, `peniko`, `parley`, and `winit`).

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
   - **Real-Time External CLI Agent Bridge (`crates/kine-studio/src/bridge.rs`)**:
     - Kine Studio deliberately omits any built-in LLM chat wrapper. External CLI agents (Claude Code / **Claude Opus 5.5**, Codex / **GPT-6 Astra**, Gemini CLI / **Gemini Argon 4**, OpenCode) run in a separate terminal or window and control Kine Studio in real time over a bidirectional TCP bridge (`127.0.0.1:7701`, auto-discovered via `.kine-studio-bridge.json`) or via live file hot-reloading (`SceneFileWatcher`):
       - **State, Selection & Inspection**: `studio_status`, `studio_selection`, `studio_look`, `studio_tree`, `studio_timeline`, `studio_inspect`
       - **Node Hierarchy & Alignment**: `studio_add_node`, `studio_delete_node`, `studio_duplicate_node`, `studio_reorder_node`, `studio_toggle_visibility`, `studio_align_node`
       - **Content, Style, Transform, WGSL Shaders & Audio**: `studio_set_text`, `studio_set_style`, `studio_transform`, `studio_set_shader`, `studio_audio`
       - **Timeline, Keyframes & Transport**: `studio_seek`, `studio_frame`, `studio_transport`, `studio_resize_canvas`, `studio_add_animation`, `studio_update_animation`, `studio_add_keyframe`, `studio_delete_keyframe`, `studio_delete_animation`, `studio_shift_time`, `studio_split`
       - **History, Scene I/O, Export & Atomic Batching**: `studio_undo`, `studio_redo`, `studio_history`, `studio_load_scene`, `studio_reload`, `studio_apply_source`, `studio_save_scene`, `studio_switch_composition`, `studio_new_composition`, `studio_export`, `studio_batch`
     - `Renders` Queue: In-process multi-format export (`MP4`, `MKV`, `WebM`, `GIF`, `PNG`) with frame count, byte size, and output path history.

### Launching Kine Studio

Kine Studio is **GPU-first**: the default features enable the `gpu` pipeline and the
`auto` backend selects Vulkan/Metal/DX12 when an adapter is present, falling back to the
deterministic CPU path otherwise. Always use the optimized profile for interactive
preview — a debug build spends most of its frame budget in unoptimized scene evaluation:

```bash
# Recommended: release build + GPU (also what scripts/studio.sh does)
cargo run --release -p kine-studio -- --scene examples/hello/scene.ts
./scripts/studio.sh --scene examples/motion/scene.ts

# Launch via the kine CLI
cargo run --release -p kine-cli -- studio
cargo run --release -p kine-cli -- studio --scene examples/hello/scene.ts

# Start with transport already playing (useful for capture/demo)
cargo run --release -p kine-studio -- --scene examples/hello/scene.ts --play

# Run a headless verification check (for CI or automated agents)
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

The status bar shows the active backend and last rasterization time (`GPU · 5.4ms`).
CPU is a fallback, not the default. Builds without `gpu` still work and print `CPU`.

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

`kind` is free-form (`subtitle`, `widget`, `transition`, `animation`, ...).
`params` are the module's default props: they are merged under the call-site
props, so `module("lower-third")` gets `UNTITLED` while
`module("lower-third", { title: "Hello" })` overrides it. `scene.ts` must
evaluate to a factory that takes the props and returns one scene node:

```ts
return (props) => container(
  { position: "absolute", inset: { left: 40, bottom: 40 }, size: { width: 420, height: 72 } },
  [
    rect({ fill: props.accent || "#f5b642", size: { width: 8, height: 72 } }),
    text(props.title || "UNTITLED", {
      fontFamily: "JetBrains Mono",
      fontSize: 34,
      color: "#ffffff",
      position: "absolute",
      inset: { left: 24, top: 8 },
      size: { width: 380, height: 44 },
    }),
  ]
);
```

Use it like any other node; the module sees the same DSL globals as the scene:

```ts
container({ size: { width: W, height: H } }, [
  module("lower-third", { title: "MODULE OK", subtitle: "ready-made element" }),
]);
```

Rules:

* the module name is its directory name (`Modules/lower-third` → `module("lower-third")`);
* `module.json` is optional, `scene.ts` is not;
* unknown or broken modules fail scene load with an error listing what is available;
* modules are evaluated per scene load, before the scene body, so a scene can use any module without imports.

List what a project offers:

```sh
kine modules --dir ./project
```

## Workspace Architecture

- `crates/kine-core` — Scene graph (`Scene`, `Node`, `NodeKind`), `taffy` layout integration, and keyframe animation curves (`Linear`, `EaseIn`, `EaseOut`, `EaseInOut`, `CubicBezier`, `Steps`, analytical `Spring`).
- `crates/kine-text` — Pure-Rust font loading and text shaping (`cosmic-text`).
- `crates/kine-shader` — WGSL shader validation (`naga`), uniform reflection, and deterministic multi-threaded CPU WGSL fragment VM.
- `crates/kine-render` — Unified CPU (`vello_cpu`) and optional GPU (`vello_gpu` + `wgpu`) frame renderer, procedural/container video input cache, and multi-format still export (`PNG`, `QOI`, `BMP`, `PPM`, `PAM`, `TGA`, `GIF89a`).
- `crates/kine-encode` — Software AV1 (`rav1e`), `YUV4MPEG2` (`.y4m`), raw `RGBA8`/`I420`/`NV12`, and indexed lossless `.kvid` encoders.
- `crates/kine-audio` — Waveform synthesis, file decoding (`symphonia`), resampling (`rubato`), biquad EQ, dynamic range compression, sidechain ducking, and `WAV`/`FLAC`/`AIFF` output.
- `crates/kine-mux` — ISO-BMFF `.mp4` (`muxfin`) and Matroska `.mkv` / `.webm` muxers, plus optional hardware H.264 encoding (`gpu-video`).
- `crates/kine-script` — TypeScript stripping (`oxc`), embedded QuickJS (`rquickjs`) evaluation, and JSON/TS scene authoring schema (`SceneDef`, `NodeDef`).
- `crates/kine-platform` — Host OS detection (`Linux`, `Windows`, `macOS`) and output path resolution (`KINE_OUTPUT_DIR`).
- `crates/kine-server` — In-process `Engine` API, static scene inspector, snapshot pixel-diff gate, and newline-delimited JSON-RPC TCP server.
- `crates/kine-studio` — Native Rust **Xilem** GUI studio (`StudioState`, NLE timeline, node/shader inspector, live `StudioBridgeServer`, `SceneFileWatcher`, and 34 `studio_*` agent bridge tools) living in-process with `kine-server::Engine`.
- `crates/kine-cli` — Non-interactive `kine` command-line binary, `kine studio` launcher, and `kine agent` real-time CLI bridge controller.
