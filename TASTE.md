# Taste — Kine Studio

## Direction
A warm-graphite native NLE for a video-as-code engine: OpenCode Desktop's floating-panel shell and calm neutral chrome, filled with HyperFrames Studio's dense editor grammar and mint signal colour. It must read as a tool built for hours of editing, not a demo of widgets.

## Type
- Primary: **Inter** variable (bundled), 13px UI base, 11px labels/meta, 10px micro, 9px chips. Weights 400 / 500 / 600; no faux-bold.
- Mono: **JetBrains Mono** (already bundled) for timecode, frame numbers, ids, property values, render stats. Tabular numerals, right-aligned values in the inspector.
- No text below 9px. No letter-spacing except uppercase micro chips.

## Color
- Window frame: `#12110f` (deep, warm black).
- Panel card / surface: `#201e1b`; inset field: `#1a1917`; raised popover: `#2f2c29`.
- Text: primary `#f5f3f1`, secondary `#b8b6b4`, muted `#979593`, disabled `#6f6d6a`.
- Borders: white 8% hairline, white 14% strong; no solid grey lines.
- Accent / signal: mint `#3de4a8`; ink on mint `#001c11`; accent text `#41e6aa`.
- Selection: blue `#4ba3d2` (must stay distinct from mint). Playhead: `#f5f3f1`.
- Status: danger `#ff9c94`, warning `#ffb900`, media cyan `#00e3ff`, container amber `#f5a623`, audio violet `#60438d`.

## Shape & material
- Panel cards radius 10px, 1px strong border; 6px gutters of frame colour between cards.
- Controls radius 6px, chips/badges 4px, timeline clips 12px with a 15px label band, keyframe diamonds 10px.
- Depth comes from borders + one-step surface lifts, not blurry shadows. No glassmorphism, no glow except a soft 6px playhead glow.
- Dark is the primary theme. Paper/light must be a real warm off-white variant, not an inverted afterthought.

## Spacing & density
- 4px grid. Header 40px, dock tab strip 30–36px, controls 24/28/32px, inspector rows 30px, section headers 40px, timeline ruler 24px, track rows 48px, track header 232px.
- Tight but not cramped: 8/12/16px panel padding, 4/6px intra-group gaps.
- Minimum pointer target 24×24 even when the painted control is smaller.

## Motion
- 120–160ms, ease-out-quint (`cubic-bezier(.22,1,.36,1)`); press scale 0.98; no bounce in chrome.
- Popovers may enter at 0.97 scale / 0.8 opacity; never from zero.
- All motion optional/reduced-motion safe; static state colours must carry the meaning on their own.

## References
- **OpenCode Desktop** (`anomalyco/opencode`, MIT, dev@20a6cc7): 8px inset frame, floating 10px panels with 0.5px rim, 36px titlebar, 28px tab pills, 4px spacing, 13px/440–530 type, neutral-only chrome, custom 4px scrollbar.
- **HyperFrames Studio** (`heygen-com/hyperframes`, Apache-2.0, 8811f0bf): Graphite tokens, mint accent `#3de4a8`, 40px header, 6px gutters, 30px tabs, 232/48/28/24 timeline constants, three-state inspector values, clip label band, focus/state recipes.
- **Material Symbols Outlined** (Apache-2.0): icon ligatures at 14–16px, one weight, currentColor.
- **Inter** (OFL) and **JetBrains Mono** (OFL) bundled.

## Explicitly avoid
- Emoji or arbitrary Unicode pictograms as icons.
- The current "Qt5 fake": oversized 14–16px chrome text, every action as a wide pill button, thick mint borders on every card, stacked identical rounded rectangles.
- Neon gradients, purple SaaS gradients, glass blur, huge drop shadows, glowing outlines.
- Icon + text on every toolbar button; prefer icon-only with labels only where the action is ambiguous or destructive.
- Fake window chrome, oversized empty headers, centered modal for routine work.
- Web-only tricks that Xilem cannot reproduce (backdrop-filter, CSS masks); use borders, clipping and explicit layout instead.
