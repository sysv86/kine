// Ready-made lower third. The scene calls `module("lower-third", props)`.
return (props) => container(
  {
    position: "absolute",
    inset: { left: 40, bottom: 40 },
    size: { width: 420, height: 72 },
  },
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
    text(props.subtitle || "", {
      fontFamily: "JetBrains Mono",
      fontSize: 18,
      color: "#aab6c8",
      position: "absolute",
      inset: { left: 24, top: 48 },
      size: { width: 380, height: 24 },
    }),
  ]
);
