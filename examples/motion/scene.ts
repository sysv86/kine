// Kine example — a real animated clip, rendered to MP4 on GPU or CPU.
//
//   kine render --scene examples/motion/scene.ts --out motion.mp4 --frames 90 --fps 30
//   kine render --scene examples/motion/scene.ts --out poster.png --frame 20
//
// `animate` drives layout, transform and opacity properties over time; the
// engine evaluates them before every frame pass. Easing supports `linear`,
// `easeIn`, `easeOut`, `easeInOut`, `spring`, `spring(stiffness, damping)`,
// `steps(N)` and `cubic-bezier(x1, y1, x2, y2)`.

const W = 640;
const H = 360;

return scene(W, H,
  container(
    {
      size: { width: W, height: H },
      flexDirection: "column",
      justifyContent: "center",
      alignItems: "center",
      gap: 18,
    },
    [
      // Background with subtle vertical linear gradient.
      rect({
        fill: "#0a0f1e",
        gradient: linearGradient(0, [
          { offset: 0, color: "#0a0f1e" },
          { offset: 1, color: "#111a33" },
        ]),
        position: "absolute",
        inset: { top: 0, left: 0, right: 0, bottom: 0 },
      }),

      // A progress rule that grows across the frame, then holds.
      rect({
        fill: "#4fd1c5",
        cornerRadius: 3,
        position: "absolute",
        inset: { top: 300, left: 40 },
        size: { width: 0, height: 6 },
        animate: {
          width: { from: 0, to: 560, start: 0.0, end: 2.0, easing: "easeInOut" },
        },
      }),

      // The card slides up, scales in, and settles with an analytical spring.
      rect({
        fill: "#16203a",
        strokeColor: "#2d3f66",
        strokeWidth: 2,
        cornerRadius: 16,
        position: "absolute",
        inset: { top: 150, left: 120 },
        size: { width: 400, height: 120 },
        animate: {
          offsetY: { from: 120, to: 0, start: 0.0, end: 1.0, easing: "spring" },
          opacity: { from: 0.0, to: 1.0, start: 0.0, end: 0.45, easing: "easeOut" },
        },
      }),

      // Title fades and slides into place.
      text("Kine", {
        fontFamily: "JetBrains Mono",
        fontSize: 72,
        fontWeight: 700,
        color: "#eef2ff",
        align: "center",
        size: { width: 400, height: 90 },
        position: "absolute",
        inset: { top: 90, left: 120 },
        animate: {
          offsetY: { from: -40, to: 0, start: 0.2, end: 1.1, easing: "easeOut" },
          opacity: { from: 0.0, to: 1.0, start: 0.2, end: 0.8, easing: "easeOut" },
        },
      }),

      // Subtitle, arriving last with cubic-bezier easing.
      text("frames out, in one process", {
        fontFamily: "JetBrains Mono",
        fontSize: 20,
        color: "#8fa2c8",
        align: "center",
        size: { width: 400, height: 32 },
        position: "absolute",
        inset: { top: 200, left: 120 },
        animate: {
          offsetX: { from: -160, to: 0, start: 0.8, end: 1.8, easing: "cubic-bezier(0.22, 1.0, 0.36, 1.0)" },
          opacity: { from: 0.0, to: 1.0, start: 0.8, end: 1.4, easing: "easeOut" },
        },
      }),
    ]
  )
);
