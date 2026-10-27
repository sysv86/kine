// ASCII / CRT / glow demo: a raymarched 3D scene (three gyroscope rings around a
// pulsing core over a wavy floor), quantized to ASCII glyphs, bloomed with three
// additive glow passes and finished through a curved CRT with scanlines,
// aperture grille, chromatic aberration, flicker and grain.
//
// Heavy on purpose: ~90 march steps per pixel + 3x24-tap bloom at 1080p.
// Render: kine render --scene examples/ascii_crt/scene.ts --out out.mp4 \
//                   --frames 180 --fps 30 --backend gpu --encoder h264

const W = 1920;
const H = 1080;

// ── 1. world: raymarched 3D, monochrome luminance ───────────────────────────
const WORLD = `
struct WorldParams {
    cam_speed: f32,
    march_steps: f32,
    glow_bias: f32,
    contrast: f32,
};
@group(1) @binding(0) var<uniform> wp: WorldParams;

fn rot_xz(p: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * p.x - s * p.z, p.y, s * p.x + c * p.z);
}

fn rot_xy(p: vec3<f32>, a: f32) -> vec3<f32> {
    let c = cos(a);
    let s = sin(a);
    return vec3<f32>(c * p.x - s * p.y, s * p.x + c * p.y, p.z);
}

fn sd_torus(p: vec3<f32>, big: f32, small: f32) -> f32 {
    let q = vec2<f32>(length(p.xz) - big, p.y);
    return length(q) - small;
}

fn sd_scene(p: vec3<f32>, t: f32) -> f32 {
    // wavy floor
    var d = p.y + 1.7 + 0.09 * sin(p.x * 1.7) * sin(p.z * 1.7);
    // three gyroscope rings, each spun around a different axis
    d = min(d, sd_torus(rot_xy(rot_xz(p, 0.2), t * 0.8), 1.55, 0.17));
    d = min(d, sd_torus(rot_xy(rot_xz(p, 1.25), t * 1.13 + 1.0), 1.15, 0.14));
    d = min(d, sd_torus(rot_xy(rot_xz(p, -1.15), -t * 0.92), 0.88, 0.10));
    // pulsing core
    d = min(d, length(p - vec3<f32>(0.0, 0.12 * sin(t * 1.7), 0.0)) - 0.42);
    return d;
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let t = kine_frame.time * wp.cam_speed;
    let res = kine_frame.resolution;
    let aspect = res.x / max(res.y, 1.0);

    let cam = vec3<f32>(cos(t * 0.5) * 3.0, 0.55 + 0.30 * sin(t * 0.31), sin(t * 0.5) * 3.0);
    let fwd = normalize(vec3<f32>(0.0, -0.08, 0.0) - cam);
    let right = normalize(cross(fwd, vec3<f32>(0.0, 1.0, 0.0)));
    let up = cross(right, fwd);
    let screen = uv * 2.0 - vec2<f32>(1.0, 1.0);
    let rd = normalize(fwd + right * screen.x * aspect * 0.62 + up * screen.y * 0.62);

    var dist = 0.05;
    var hit = false;
    let steps = i32(wp.march_steps);
    for (var i = 0; i < steps; i = i + 1) {
        let pos = cam + rd * dist;
        let d = sd_scene(pos, t);
        if (d < 0.0035) {
            hit = true;
            break;
        }
        dist = dist + max(d * 0.92, 0.0025);
        if (dist > 24.0) {
            break;
        }
    }

    var lum = 0.0;
    if (hit) {
        let pos = cam + rd * dist;
        let e = 0.0028;
        let nx = sd_scene(pos + vec3<f32>(e, 0.0, 0.0), t) - sd_scene(pos - vec3<f32>(e, 0.0, 0.0), t);
        let ny = sd_scene(pos + vec3<f32>(0.0, e, 0.0), t) - sd_scene(pos - vec3<f32>(0.0, e, 0.0), t);
        let nz = sd_scene(pos + vec3<f32>(0.0, 0.0, e), t) - sd_scene(pos - vec3<f32>(0.0, 0.0, e), t);
        let n = normalize(vec3<f32>(nx, ny, nz));
        let key = normalize(vec3<f32>(0.65, 1.0, 0.35));
        let diff = max(dot(n, key), 0.0);
        let rim = pow(1.0 - max(dot(n, -rd), 0.0), 2.5);
        let fog = clamp(1.0 - dist / 19.0, 0.0, 1.0);
        // banding around the rings + a grid on the floor give the ASCII ramp
        // something to bite into
        var pattern = 0.72 + 0.28 * sin(atan2(pos.z, pos.x) * 14.0);
        if (pos.y < -1.55) {
            let gx = abs(fract(pos.x * 0.7) - 0.5);
            let gz = abs(fract(pos.z * 0.7) - 0.5);
            pattern = select(0.30, 1.0, min(gx, gz) < 0.06);
        }
        lum = (0.08 + 0.92 * diff * pattern) * fog + rim * 0.8 * fog * pattern;
    } else {
        // starfield + hazy horizon
        let cell = floor(uv * res / 5.0);
        let h = fract(sin(dot(cell, vec2<f32>(12.9898, 78.233))) * 43758.5453);
        lum = select(0.0, 0.7, h > 0.9975);
        lum = lum + 0.05 * clamp(1.0 - abs(uv.y - 0.63) * 7.0, 0.0, 1.0);
    }

    lum = clamp(pow(lum, wp.contrast), 0.0, 1.0);
    let col = vec3<f32>(lum) + vec3<f32>(lum * wp.glow_bias, lum * wp.glow_bias * 1.15, lum * wp.glow_bias * 0.65);
    return vec4<f32>(col, 1.0);
}
`;

// ── 2. ascii: 5x6 glyph ramp, procedurally drawn ────────────────────────────
const ASCII = `
struct AsciiParams {
    cell_px: f32,
    contrast: f32,
    tint_r: f32,
    tint_g: f32,
    tint_b: f32,
};
@group(1) @binding(0) var<uniform> ap: AsciiParams;

// ramp " .:-=+*#%@" rasterized into 5x6 bitmaps, one glyph per u32
var<private> GLYPHS: array<u32, 10> = array<u32, 10>(
    0x00000000u, 0x0000018cu, 0x00c6018cu, 0x00007c00u, 0x01f07c00u,
    0x00427c84u, 0x01577dd5u, 0x15f52beau, 0x33a22e60u, 0x1d1bd60eu
);

fn glyph_bit(idx: i32, gx: i32, gy: i32) -> f32 {
    let bits = GLYPHS[clamp(u32(idx), 0u, 9u)];
    let shift = u32((5 - gy) * 5 + (4 - gx));
    return f32((bits >> shift) & 1u);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let res = kine_frame.resolution;
    let cell = max(ap.cell_px, 4.0);
    let cells = floor(res / cell);
    let c = floor(uv * cells);
    let center = (c + vec2<f32>(0.5, 0.5)) / cells;

    let src = textureSampleLevel(kine_input, kine_sampler, center, 0.0).rgb;
    var luma = dot(src, vec3<f32>(0.2126, 0.7152, 0.0722));
    luma = clamp(pow(luma, ap.contrast), 0.0, 1.0);

    let idx = i32(floor(luma * 9.999));
    let f = fract(uv * cells);
    let on = glyph_bit(idx, i32(floor(f.x * 5.0)), i32(floor(f.y * 6.0)));

    let tint = vec3<f32>(ap.tint_r, ap.tint_g, ap.tint_b);
    let level = 0.30 + 0.70 * luma;
    let phosphor = clamp(tint * on * level * 1.7, vec3<f32>(0.0), vec3<f32>(1.0));
    return vec4<f32>(phosphor, 1.0);
}
`;

// ── 3. glow: additive golden-angle spiral bloom ─────────────────────────────
const GLOW = `
struct GlowParams {
    radius: f32,
    gain: f32,
    threshold: f32,
    taps: f32,
};
@group(1) @binding(0) var<uniform> gp: GlowParams;

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let res = kine_frame.resolution;
    var acc = vec3<f32>(0.0);
    let n = i32(gp.taps);
    for (var i = 0; i < n; i = i + 1) {
        let fi = f32(i);
        let ang = fi * 2.399963;
        let rad = sqrt(fi / f32(n)) * gp.radius;
        let off = vec2<f32>(cos(ang), sin(ang)) * rad / res;
        let s = textureSampleLevel(kine_input, kine_sampler, clamp(uv + off, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).rgb;
        let lum = dot(s, vec3<f32>(0.2126, 0.7152, 0.0722));
        acc = acc + s * max(lum - gp.threshold, 0.0);
    }
    return vec4<f32>(acc * (gp.gain / f32(n)), 1.0);
}
`;

// ── 4. crt: curvature, scanlines, grille, aberration, flicker, grain ───────
const CRT = `
struct CrtParams {
    curvature: f32,
    scan: f32,
    grille: f32,
    aberration: f32,
    vignette: f32,
    noise: f32,
    flicker: f32,
};
@group(1) @binding(0) var<uniform> cp: CrtParams;

fn curve(uv: vec2<f32>, k: f32) -> vec2<f32> {
    let c = uv * 2.0 - vec2<f32>(1.0, 1.0);
    let r2 = dot(c, c);
    return (c * (1.0 + k * r2)) * 0.5 + vec2<f32>(0.5, 0.5);
}

fn hash21(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

@fragment
fn main(@location(0) uv: vec2<f32>) -> @location(0) vec4<f32> {
    let res = kine_frame.resolution;
    let t = kine_frame.time;
    let p = curve(uv, cp.curvature);
    if (p.x < 0.0 || p.x > 1.0 || p.y < 0.0 || p.y > 1.0) {
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    let dir = normalize(p - vec2<f32>(0.5, 0.5) + vec2<f32>(0.0001, 0.0));
    let ab = cp.aberration * 0.0035;
    let r = textureSampleLevel(kine_input, kine_sampler, clamp(p + dir * ab, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).r;
    let g = textureSampleLevel(kine_input, kine_sampler, p, 0.0).g;
    let b = textureSampleLevel(kine_input, kine_sampler, clamp(p - dir * ab, vec2<f32>(0.0), vec2<f32>(1.0)), 0.0).b;
    var col = vec3<f32>(r, g, b);

    // aperture grille: RGB triples across the beam
    let ph = fract(p.x * res.x / 3.0);
    var mask = vec3<f32>(1.0);
    if (ph < 0.34) {
        mask = vec3<f32>(1.0, 0.32, 0.36);
    } else if (ph < 0.67) {
        mask = vec3<f32>(0.30, 1.0, 0.34);
    } else {
        mask = vec3<f32>(0.32, 0.36, 1.0);
    }
    col = col * mix(vec3<f32>(1.0), mask, clamp(cp.grille, 0.0, 1.0));

    // scanlines
    let sl = 1.0 - cp.scan * pow(abs(sin(p.y * res.y * 3.14159265)), 0.75);
    col = col * sl;

    // vignette + edge falloff
    let d = length((p - vec2<f32>(0.5, 0.5)) * vec2<f32>(1.0, 0.92)) * 1.42;
    col = col * clamp(1.0 - cp.vignette * pow(d, 2.1), 0.0, 1.0);

    // flicker + grain
    let flick = 1.0 - cp.flicker * (0.5 + 0.5 * sin(t * 63.0)) * 0.30;
    let grain = (hash21(p * res + vec2<f32>(t * 331.0, t * 197.0)) - 0.5) * cp.noise;
    col = clamp(col * flick + vec3<f32>(grain), vec3<f32>(0.0), vec3<f32>(1.0));

    return vec4<f32>(col, 1.0);
}
`;

const full = {
  position: "absolute",
  inset: { top: 0, left: 0, right: 0, bottom: 0 },
  size: { width: W, height: H },
};

return scene(W, H,
  container({ size: { width: W, height: H } }, [
    shader(WORLD, { ...full, uniforms: { cam_speed: 1.0, march_steps: 90.0, glow_bias: 0.12, contrast: 1.5 } }),
    shader(ASCII, { ...full, uniforms: { cell_px: 12.0, contrast: 1.30, tint_r: 0.45, tint_g: 1.0, tint_b: 0.50 } }),
    shader(GLOW, { ...full, blend: "add", uniforms: { radius: 4.0, gain: 0.55, threshold: 0.26, taps: 24.0 } }),
    shader(GLOW, { ...full, blend: "add", uniforms: { radius: 12.0, gain: 0.40, threshold: 0.22, taps: 24.0 } }),
    shader(GLOW, { ...full, blend: "add", uniforms: { radius: 30.0, gain: 0.28, threshold: 0.18, taps: 24.0 } }),
    shader(CRT, { ...full, uniforms: { curvature: 0.14, scan: 0.32, grille: 0.28, aberration: 0.55, vignette: 0.50, noise: 0.06, flicker: 1.0 } }),
  ])
);
