// Kine example project — write video as TypeScript.
//
//   kine render examples/hello/scene.ts --out frame.png
//   kine inspect examples/hello/scene.ts
//
// The script is a function body: it must `return scene(...)`.
// Layout is declarative (Flexbox/Grid via Taffy) — no hard-coded pixel soup,
// the engine owns measurement, wrapping and rasterization.

interface Palette {
  bg: string;
  card: string;
  ink: string;
  accent: string;
}

const palette: Palette = {
  bg: "#0b1020",
  card: "#151c33",
  ink: "#e8ecf8",
  accent: "#5ee0c0",
};

const W = 1280;
const H = 720;

// A title card: an out-of-flow background, a centred column, and a footer.
return scene(W, H,
  container(
    {
      size: { width: W, height: H },
      flexDirection: "column",
      justifyContent: "center",
      alignItems: "center",
      gap: 24,
    },
    [
      // Full-bleed background (absolute, so it never affects flex flow).
      rect({
        fill: palette.bg,
        position: "absolute",
        inset: { top: 0, left: 0, right: 0, bottom: 0 },
      }),

      // Accent rule.
      rect({ fill: palette.accent, cornerRadius: 3, size: { width: 96, height: 6 } }),

      // Title.
      text("Kine", {
        fontFamily: "JetBrains Mono",
        fontSize: 96,
        color: palette.ink,
        align: "center",
        size: { width: W, height: 120 },
      }),

      // Subtitle.
      text("video as code — TypeScript in, frames out", {
        fontFamily: "JetBrains Mono",
        fontSize: 28,
        color: palette.accent,
        align: "center",
        size: { width: W, height: 48 },
      }),
    ]
  )
);
