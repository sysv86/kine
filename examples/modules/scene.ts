const W = 640;
const H = 360;

return scene(
  W,
  H,
  container({ size: { width: W, height: H } }, [
    rect({
      fill: "#0d1220",
      position: "absolute",
      inset: { top: 0, left: 0, right: 0, bottom: 0 },
    }),
    text("PROJECT MODULE", {
      fontFamily: "JetBrains Mono",
      fontSize: 42,
      color: "#f5f3f1",
      position: "absolute",
      inset: { left: 40, top: 54 },
      size: { width: 560, height: 48 },
    }),
    module("lower-third", {
      title: "MODULE OK",
      subtitle: "ready-made element from Modules/",
      accent: "#3de4a8",
    }),
  ])
);
