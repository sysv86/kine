# Kine Studio UI & Design System Guideline (v2.0)

## 1. Core Architectural Philosophy
Kine Studio is a professional native **Video-as-Code Non-Linear Editor (NLE)** and **Shader/Scene Graph Studio** built in Rust on **Xilem `0.4.0`** and **Masonry `0.4.0`** (`vello` + `kurbo` + `peniko` + `parley`).

Every element in the studio follows four non-negotiable engineering rules:
1. **Zero Dead Controls (No Stubs)**: Every button, scrub cell, asset card, timeline lane, ruler tick, and settings toggle must perform an immediate, verifiable state mutation or engine synchronization.
2. **True Pixel-Coordinate Canvas Widgets Over Flexbox Hacks**: Time-domain NLE timelines, rulers, keyframe diamonds, and interactive stages must be painted with `vello::Scene` and `kurbo` geometry inside custom `masonry::core::Widget` implementations, never faked with proportional `FlexSpacer` percentages.
3. **Strict Cascading Hierarchy**: Scene hierarchies, property inspector groups, and multi-track NLE lanes use explicit depth-stepped indentation (`14px` per level), vertical tree guide lines (`1px` hairline at parent column axis), disclosure chevrons (`expand_more` / `chevron_right`), and 3-state property badges (`Default`, `Modified`, `Keyframed`).
4. **Deterministic 60Hz UI Motion**: Transitions (disclosure expansion, dock collapse, modal fade/scale, playhead glide, toast notifications) are driven by exponential decay / `ease_out_quint` (`1.0 - (1.0 - t).powi(5)`) and respected by the `reduced_motion` setting.

---

## 2. Surface Ladder & Color System (`StudioThemeMode` & `AccentPreset`)

Kine Studio supports three calibrated surface themes and five signal accent presets, resolved into precomputed `Copy` `StudioPalette` structs:

### 2.1 Surface Themes (`StudioThemeMode`)
| Token | Graphite Dark (Default) | Warm Paper (Light) | Carbon Obsidian (OLED) | Role |
| :--- | :--- | :--- | :--- | :--- |
| `bg_shell` | `#12110F` | `#ECEAE5` | `#08080A` | Outer window frame & `6px` inter-panel gutters |
| `bg_surface` | `#201E1B` | `#FDFBF8` | `#121216` | Primary floating panel card background |
| `bg_inset` | `#171614` | `#F4F2EE` | `#0C0C0F` | Recessed inputs, code wells, track gutters, stage frame |
| `bg_raised` | `#2B2825` | `#FFFFFF` | `#1C1C22` | Active tabs, hovered rows, popovers, header cards |
| `bg_elevated` | `#35312D` | `#F7F5F0` | `#26262E` | Modal dialogs, settings window, floating HUDs |
| `bg_stage` | `#0D0C0B` | `#181715` | `#050507` | Composition preview backdrop behind the frame |
| `border` | `rgba(255,255,255,0.08)` | `rgba(0,0,0,0.09)` | `rgba(255,255,255,0.07)` | `1px` structural section hairlines & tree guides |
| `border_strong` | `rgba(255,255,255,0.15)` | `rgba(0,0,0,0.16)` | `rgba(255,255,255,0.14)` | Panel card rim & focused control outline |

### 2.2 Signal Accents (`AccentPreset`)
- **Kine Mint (Default)**: `#3DE4A8` (Hover `#54F5B8`, Ink `#41E6AA`, Wash `rgba(61,228,168,0.15)`)
- **Cinema Amber**: `#F5A623` (Hover `#FFB840`, Ink `#F7B23B`, Wash `rgba(245,166,35,0.16)`)
- **Broadcast Cyan**: `#00E3FF` (Hover `#47ECFF`, Ink `#26E7FF`, Wash `rgba(0,227,255,0.15)`)
- **Studio Violet**: `#9B72F2` (Hover `#B18FF7`, Ink `#A882F5`, Wash `rgba(155,114,242,0.16)`)
- **Signal Crimson**: `#FF6B6B` (Hover `#FF8787`, Ink `#FF7A7A`, Wash `rgba(255,107,107,0.16)`)

### 2.3 Semantic Node & Track Colors
- **Selection Sky**: `#4BA3D2` (kept distinct from the primary accent so selected clips and keyframe diamonds never collide with active transport toggles)
- **Media / Video (`VID` / `IMG`)**: `#00E3FF`
- **Container / Layout (`CONT` / `RECT`)**: `#F5A623`
- **Shader / WGSL (`SHDR`)**: `#38BDF8`
- **Audio DSP (`AUD`)**: `#966FD6`
- **Danger / Error**: `#FF9C94`
- **Warning / Finding**: `#FFB900`

---

## 3. Typography & Numeric Discipline

- **UI Sans (`Inter Variable`)**:
  - `12.5px` (`FontWeight::SEMI_BOLD`): Modal titles, active composition title
  - `11.0px` (`FontWeight::MEDIUM`): Panel section headers, layer names, primary buttons
  - `10.0px` (`FontWeight::NORMAL`): Property labels, dock tabs, settings descriptions
  - `8.5px` (`FontWeight::SEMI_BOLD`): Node kind badges (`CONT`, `RECT`, `TEXT`, `SHDR`, `VID`, `AUD`)
- **Telemetry & Code Mono (`JetBrains Mono`)**:
  - `11.0px`: Scene source editor buffer (`TS` / `JSON`), timecode HUD (`00:00.50 / 00:03.00`)
  - `9.5px`: Interactive numeric scrub cells (`X`, `Y`, `W`, `H`, `Rot`, `Scale`, `Opacity`, `Radius`), render duration, frame counter
  - `8.5px`: Timeline ruler timestamps, keyframe diamond counts, asset URIs

---

## 4. Spatial Grid, Layout & Density Scale (`UiDensity`)

All measurements snap to a `4px` spatial grid with three user-selectable density modes in Settings:

| Metric | Compact | Comfortable (Default) | Spacious |
| :--- | :--- | :--- | :--- |
| Shell Gutter (`shell_gap`) | `4px` | `6px` | `8px` |
| Panel Corner Radius (`panel_radius`) | `8px` | `10px` | `12px` |
| Top Header Bar (`header_h`) | `36px` | `40px` | `44px` |
| Dock Tab Strip (`tab_h`) | `26px` | `30px` | `34px` |
| Inspector Row Height (`row_h`) | `26px` | `30px` | `34px` |
| Timeline Track Row (`track_h`) | `28px` | `32px` | `38px` |
| Timeline Sub-Track Row (`subtrack_h`) | `22px` | `24px` | `28px` |
| Left Dock Width (`left_dock_w`) | `232px` | `256px` | `280px` |
| Right Dock Width (`right_dock_w`) | `296px` | `324px` | `352px` |
| Cascade Indent Step (`cascade_indent`) | `12px` | `14px` | `18px` |

### 4.1 5-Zone Resizable Shell Architecture
1. **Top Command Header (`40px`)**: Brand mark, active composition breadcrumb switcher, static analyzer findings badge, Quick Action cluster (`New Comp`, `Capture PNG`, `Backend Cycle`, `Left Dock Toggle`, `Right Inspector Toggle`, `Theme Cycle`, `CLI Bridge (:7701)`, `Settings (⚙)`, `Export`).
2. **Left Navigator Dock**: Segmented pill strip (`Comps`, `Assets`, `Code`, `Catalog`) with quick filter/action header and scrollable body.
3. **Center Split Workspace**:
   - **Upper Preview Stage**: Viewport toolbar (composition resolution pill, selected node path, channel mode `RGB`/`R`/`G`/`B`/`A`, Grid, Safe Guides, Bounding Box, Topology Lens, Zoom `25%–400%` + `Fit`), interactive `PreviewCanvas` (click-to-select + drag-to-move node in canvas), and full Transport Bar (timecode mode toggle, transport buttons, scrub slider, volume slider, stereo peak meters).
   - **Lower NLE Timeline**: Timeline toolbar (Undo/Redo with badge counts, Snap, Ripple, Auto-KF, Prev/Add/Next Keyframe, Time Shift `-0.1s`/`+0.1s`, Split, Duplicate, Delete, Add Track menu, Zoom slider) + synchronized Track Header Gutter and custom Vello-rendered **`TimelineCanvas`** (pixel-exact ruler ticks, playhead line with top handle & soft glow, clip blocks with 15px label band, diamond keyframe markers, click/drag playhead scrubbing).
4. **Right Inspector Dock**: Segmented pill strip (`Design`, `Layers`, `Renders`) with cascading collapsible property cards (`Layout & Geometry`, `2D Transform`, `Appearance & Fill`, `Stroke & Drop Shadow`, `Typography`, `Keyframes & Motion`, `WGSL Shaders & Uniforms`, `Audio Bed DSP`).
5. **Modal Overlay Layer (`ZStack` / Workspace View)**:
   - **Settings Window (`SettingsTab`)**: 5-tab configuration dialog (`Appearance & UI`, `Editor & Viewport`, `Timeline & Playback`, `Engine & Export`, `Keyboard Shortcuts`) with instant live preview and JSON persistence (`studio-settings.json`).
   - **External CLI Agent Bridge (`127.0.0.1:7701`)**: No in-app chat wrapper. External CLI agents run in a separate terminal/window and mutate or inspect `StudioState` in real time over the TCP bridge (`kine agent ...`) or via watched disk files.

---

## 5. Cascading Elements & Three-State Inspector Grammar

Every property row in the `Design` Inspector communicates its authoring state visually:
1. **Default State**: Muted label (`t.muted`), recessed input background (`t.bg_inset`), subtle border (`t.border`).
2. **Modified (Static Override)**: Bright label (`t.text_soft`), subtle left indicator bar in `t.selection` (`#4BA3D2`).
3. **Keyframed (Animated)**: Mint diamond indicator (`◆` in `t.accent`), tinted border (`t.border_accent`), and direct click-to-focus on `active_anim_property`.

Hierarchical trees (in `Layers` and `Timeline` track headers) render:
- **Cascade Guide Lines**: Vertical hairlines indicating parent-child nesting depth.
- **Disclosure Chevrons**: Interactive expand/collapse toggle (`▾` / `▸`) per container node, updating `StudioState::collapsed_nodes` and filtering descendant rows cleanly.
- **Z-Order & State Controls**: Inline visibility (`visibility` / `visibility_off`), solo/isolation, and sibling reorder (`arrow_upward` / `arrow_downward`).
