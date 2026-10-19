// Kine example scene: video input, graphics layers, animation, post-effects and audio mixing.

const W = 640;
const H = 360;

const audio = {
  sampleRate: 48000,
  channels: 2,
  normalizePeakDb: -1.0,
  tracks: [
    {
      src: "synth:sine:330:1.2",
      gainDb: -3,
      start: 0.35,
      fadeIn: 0.15,
      fadeOut: 0.25,
      fadeCurve: "sCurve" as const,
      pan: 0.0,
      highpassHz: 90,
      eq: [{ kind: "peaking" as const, freqHz: 2400, q: 1.2, gainDb: 2.0 }],
      compressor: {
        thresholdDb: -16,
        ratio: 2.5,
        attackMs: 8,
        releaseMs: 120,
        makeupDb: 1.5,
      },
    },
    {
      src: "synth:chord:110,164.81,220:1.0",
      gainDb: -9,
      loop: true,
      duckDb: -9.0,
      duckThresholdDb: -32.0,
      fadeIn: 0.2,
      fadeOut: 0.4,
      fadeCurve: "equalPower" as const,
      pan: -0.15,
      lowpassHz: 3200,
    },
  ],
};

return scene(
  W,
  H,
  container(
    {
      size: { width: W, height: H },
      flexDirection: "column",
      justifyContent: "center",
      alignItems: "center",
    },
    [
      rect({
        gradient: linearGradient(135, [
          { offset: 0.0, color: "#0b1020" },
          { offset: 1.0, color: "#1a1438" },
        ]),
        position: "absolute",
        inset: { top: 0, left: 0, right: 0, bottom: 0 },
      }),

      video("video:plasma:160x90:30:2.0", {
        objectFit: "cover",
        cornerRadius: 8,
        loop: true,
        opacity: 0.35,
        position: "absolute",
        inset: { top: 24, left: 456 },
        size: { width: 160, height: 90 },
      }),

      rect({
        fill: "#7c5cff",
        cornerRadius: 4,
        shadowColor: "#7c5cff66",
        shadowBlur: 14,
        shadowOffsetY: 6,
        position: "absolute",
        inset: { top: 250, left: 60 },
        size: { width: 0, height: 10 },
        animate: {
          width: { from: 0, to: 520, start: 0.0, end: 2.0, easing: "easeInOut" },
        },
      }),

      text("scored", {
        fontFamily: "JetBrains Mono",
        fontSize: 56,
        color: "#f4f6ff",
        align: "center",
        size: { width: W, height: 80 },
        position: "absolute",
        inset: { top: 116, left: 0 },
        animate: {
          offsetY: { from: 50, to: 0, start: 0.0, end: 0.9, easing: "spring" },
        },
      }),

      text("video + audio + shaders", {
        fontFamily: "JetBrains Mono",
        fontSize: 18,
        color: "#8fa2c8",
        align: "center",
        size: { width: W, height: 32 },
        position: "absolute",
        inset: { top: 198, left: 0 },
        animate: {
          offsetX: { from: -120, to: 0, start: 0.3, end: 1.3, easing: "easeOut" },
        },
      }),

      presets.vignette(
        { intensity: 0.32, smoothness: 1.6 },
        {
          position: "absolute",
          inset: { top: 0, left: 0, right: 0, bottom: 0 },
          size: { width: W, height: H },
        }
      ),
    ]
  ),
  { audio }
);
