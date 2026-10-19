// An agent-authored effect: procedural film grain over the scene.
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
    let n = (hash(uv * kine_frame.resolution + kine_frame.frame) - 0.5) * grain.amount;
    return vec4<f32>(base.rgb * grain.tint + n, base.a);
}
