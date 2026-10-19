// Kine example — an agent-authored WGSL effect on top of a laid-out scene.
//
//   kine shader  --file examples/shaders/grain.wgsl   # validate the shader alone
//   kine inspect --scene examples/shaders/scene.ts    # checks the scene *and* the shader
//   kine render  --scene examples/shaders/scene.ts --out effect.png
//
// The shader node is a fullscreen pass scissored to its own box. On a host
// without a GPU it still validates, and `inspect` says so explicitly instead of
// silently drawing nothing.

const W = 960;
const H = 540;

// The effect: film grain + a warm tint, driven by engine-provided frame
// constants and the agent's own uniforms. Only a fragment entry point is
// required — the engine supplies the vertex stage and the bindings.
const grainEffect = `
struct Grain {
    amount: f32,
    tint: vec3<f32>,
};
@group(1) @binding(0) var<uniform> grain: Grain;

fn hash(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let base = textureSample(kine_input, kine_sampler, uv);
    let t = kine_frame.time;
    let n = (hash(uv * kine_frame.resolution + t) - 0.5) * grain.amount;
    return vec4<f32>(base.rgb * grain.tint + n, base.a);
}
`;

return scene(W, H,
  container(
    {
      size: { width: W, height: H },
      flexDirection: "column",
      justifyContent: "center",
      alignItems: "center",
      gap: 20,
    },
    [
      rect({
        fill: "#0d1224",
        position: "absolute",
        inset: { top: 0, left: 0, right: 0, bottom: 0 },
      }),

      rect({ fill: "#f2b880", cornerRadius: 3, size: { width: 72, height: 5 } }),

      text("WGSL in the loop", {
        fontFamily: "JetBrains Mono",
        fontSize: 64,
        color: "#f2f5ff",
        align: "center",
        size: { width: W, height: 80 },
      }),

      text("the agent writes the effect, the engine owns the pipeline", {
        fontFamily: "JetBrains Mono",
        fontSize: 22,
        color: "#8fa2c8",
        align: "center",
        size: { width: W, height: 40 },
      }),

      // The effect layer, clipped to a band across the bottom third.
      shader(grainEffect, {
        blend: "normal",
        uniforms: { amount: 0.08, tint: [1.02, 0.99, 0.95] },
        size: { width: W, height: 120 },
        position: "absolute",
        inset: { top: 400, left: 0 },
      }),
    ]
  )
);
