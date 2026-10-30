//! The D3D8 fixed function pipeline as two shaders. Transform, lighting, texture coordinate
//! generation and the texture stage cascade all read their settings from the uniforms, so one
//! pipeline covers every fixed function state.

use super::{FRAGMENT_COMMON, VERTEX_OUT};

pub const F_LIGHTING: u32 = 1;
pub const F_RHW: u32 = 2;
pub const F_NORMALIZE: u32 = 4;
pub const F_LOCAL_VIEWER: u32 = 8;
pub const F_SPECULAR: u32 = 16;
pub const F_COLOR_VERTEX: u32 = 32;
pub const F_HAS_DIFFUSE: u32 = 64;
pub const F_HAS_SPECULAR: u32 = 128;

pub fn vertex() -> String {
    let mut s = String::from(VERTEX_OUT);
    s.push_str(
        "
struct Light {
    diffuse: vec4f,
    specular: vec4f,
    ambient: vec4f,
    position: vec4f,
    direction: vec4f,
    // range, attenuation 0, 1, 2
    atten: vec4f,
    // cos(theta / 2), cos(phi / 2), falloff, type
    spot: vec4f,
}

// Changes with nearly every draw, so it's pushed instead of written to the uniform ring.
struct Object {
    wv: mat4x4f,
    nrm: mat4x4f,
}

var<immediate> object: Object;

struct Vs {
    proj: mat4x4f,
    tex: array<mat4x4f, 8>,
    mat_diffuse: vec4f,
    mat_ambient: vec4f,
    mat_specular: vec4f,
    mat_emissive: vec4f,
    ambient: vec4f,
    // power
    misc: vec4f,
    lights: array<Light, 8>,
    // coordinate set, generation, transform flags, components in the set
    tci: array<vec4u, 8>,
    // flags, light count, material sources, enabled clip planes
    flags: vec4u,
    viewport: vec4f,
    depth: vec4f,
    // user clip planes in view space
    clip: array<vec4f, 6>,
}

@group(0) @binding(0) var<uniform> vs: Vs;

fn safe_normalize(v: vec3f) -> vec3f {
    let l = length(v);
    return select(v / l, v, l == 0.0);
}

fn source(s: u32, material: vec4f, d0: vec4f, d1: vec4f) -> vec4f {
    let flags = vs.flags.x;
    if ((flags & 32u) == 0u) {
        return material;
    }
    if (s == 1u && (flags & 64u) != 0u) {
        return d0;
    }
    if (s == 2u && (flags & 128u) != 0u) {
        return d1;
    }
    return material;
}

fn expand(t: vec4f, n: u32) -> vec4f {
    switch n {
        case 1u: { return vec4f(t.x, 1.0, 0.0, 0.0); }
        case 2u: { return vec4f(t.xy, 1.0, 0.0); }
        case 3u: { return vec4f(t.xyz, 1.0); }
        default: { return t; }
    }
}

fn texcoord(stage: u32, sets: array<vec4f, 8>, pv: vec3f, n: vec3f) -> vec4f {
    var all = sets;
    let tci = vs.tci[stage];
    var c: vec4f;
    switch tci.y {
        case 1u: { c = vec4f(n, 1.0); }
        case 2u: { c = vec4f(pv, 1.0); }
        case 3u: {
            var e = vec3f(0.0, 0.0, 1.0);
            if ((vs.flags.x & 8u) != 0u) {
                e = safe_normalize(pv);
            }
            let nn = safe_normalize(n);
            c = vec4f(e - 2.0 * dot(nn, e) * nn, 1.0);
        }
        default: { c = expand(all[min(tci.x, 7u)], tci.w); }
    }
    if ((tci.z & 255u) != 0u) {
        c = vs.tex[stage] * c;
    }
    return c;
}

@vertex
fn main(
    @location(0) in_pos: vec4f,
    @location(2) in_normal: vec3f,
    @location(4) in_d0: vec4f,
    @location(5) in_d1: vec4f,
    @location(6) i0: vec4f,
    @location(7) i1: vec4f,
    @location(8) i2: vec4f,
    @location(9) i3: vec4f,
    @location(10) i4: vec4f,
    @location(11) i5: vec4f,
    @location(12) i6: vec4f,
    @location(13) i7: vec4f,
) -> Out {
    var o: Out;
    let flags = vs.flags.x;
    let d0 = in_d0.zyxw;
    let d1 = in_d1.zyxw;
    var pv: vec3f;
    if ((flags & 2u) != 0u) {
        let w = select(1.0 / in_pos.w, 1.0, in_pos.w == 0.0);
        let nx = (in_pos.x + 0.5 - vs.viewport.x) / vs.viewport.z * 2.0 - 1.0;
        let ny = (in_pos.y + 0.5 - vs.viewport.y) / vs.viewport.w * 2.0 - 1.0;
        let range = vs.depth.y - vs.depth.x;
        let nz = select((in_pos.z - vs.depth.x) / range, 0.0, range == 0.0);
        o.pos = vec4f(nx * w, ny * w, (1.0 - nz + vs.depth.z) * w, w);
        pv = in_pos.xyz;
    } else {
        let p = object.wv * vec4f(in_pos.xyz, 1.0);
        pv = p.xyz;
        // The projection arrives with depth already reversed.
        o.pos = to_vulkan(vs.proj * p, vs.viewport);
        o.pos.z += vs.depth.z * o.pos.w;
    }
    for (var i = 0u; i < 6u; i++) {
        o.clip[i] = select(1.0, dot(vs.clip[i], vec4f(pv, 1.0)), ((vs.flags.w >> i) & 1u) != 0u);
    }
    var n =(object.nrm * vec4f(in_normal, 0.0)).xyz;
    if ((flags & 4u) != 0u) {
        n = safe_normalize(n);
    }
    var c0 = select(vec4f(1.0), d0, (flags & 64u) != 0u);
    var c1 = select(vec4f(0.0), d1, (flags & 128u) != 0u);
    if ((flags & 1u) != 0u && (flags & 2u) == 0u) {
        let sources = vs.flags.z;
        let md = source(sources & 3u, vs.mat_diffuse, d0, d1);
        let ma = source((sources >> 2u) & 3u, vs.mat_ambient, d0, d1);
        let ms = source((sources >> 4u) & 3u, vs.mat_specular, d0, d1);
        let me = source((sources >> 6u) & 3u, vs.mat_emissive, d0, d1);
        var diffuse = me.rgb + ma.rgb * vs.ambient.rgb;
        var spec = vec3f(0.0);
        var eye = vec3f(0.0, 0.0, -1.0);
        if ((flags & 8u) != 0u) {
            eye = -safe_normalize(pv);
        }
        for (var i = 0u; i < vs.flags.y; i++) {
            let l = vs.lights[i];
            var dir: vec3f;
            var att = 1.0;
            let kind = u32(l.spot.w);
            if (kind == 3u) {
                dir = -l.direction.xyz;
            } else {
                let to = l.position.xyz - pv;
                let dist = length(to);
                if (dist > l.atten.x) {
                    continue;
                }
                dir = select(to / dist, vec3f(0.0), dist == 0.0);
                att = 1.0 / (l.atten.y + l.atten.z * dist + l.atten.w * dist * dist);
                if (kind == 2u) {
                    let rho = dot(-dir, l.direction.xyz);
                    if (rho <= l.spot.y) {
                        att = 0.0;
                    } else if (rho <= l.spot.x) {
                        att *= pow((rho - l.spot.y) / (l.spot.x - l.spot.y), l.spot.z);
                    }
                }
            }
            let ndotl = dot(n, dir);
            diffuse += att * (ma.rgb * l.ambient.rgb + md.rgb * l.diffuse.rgb * max(ndotl, 0.0));
            if ((flags & 16u) != 0u && ndotl > 0.0) {
                let h = safe_normalize(dir + eye);
                spec += att * ms.rgb * l.specular.rgb * pow(max(dot(n, h), 0.0), vs.misc.x);
            }
        }
        c0 = vec4f(diffuse, md.a);
        c1 = vec4f(spec, ms.a);
    }
    c0 = saturate(c0);
    c1 = saturate(c1);
    o.d0 = c0;
    o.d1 = c1;
    o.d0f = c0;
    o.d1f = c1;
    let sets = array<vec4f, 8>(i0, i1, i2, i3, i4, i5, i6, i7);
    o.t0 = texcoord(0u, sets, pv, n);
    o.t1 = texcoord(1u, sets, pv, n);
    o.t2 = texcoord(2u, sets, pv, n);
    o.t3 = texcoord(3u, sets, pv, n);
    o.t4 = texcoord(4u, sets, pv, n);
    o.t5 = texcoord(5u, sets, pv, n);
    o.t6 = texcoord(6u, sets, pv, n);
    o.t7 = texcoord(7u, sets, pv, n);
    return o;
}
",
    );
    s
}

pub fn fragment() -> String {
    let mut s = String::from(FRAGMENT_COMMON);
    s.push_str(
        "
fn arg(sel: u32, d: vec4f, cur: vec4f, t: vec4f, s: vec4f, tmp: vec4f) -> vec4f {
    var v: vec4f;
    switch sel & 15u {
        case 0u: { v = d; }
        case 1u: { v = cur; }
        case 2u: { v = t; }
        case 3u: { v = ps.tfactor; }
        case 4u: { v = s; }
        case 5u: { v = tmp; }
        default: { v = vec4f(1.0); }
    }
    if ((sel & 16u) != 0u) {
        v = 1.0 - v;
    }
    if ((sel & 32u) != 0u) {
        v = vec4f(v.a);
    }
    return v;
}

fn sample(stage: u32, uv: vec2f) -> vec4f {
    switch stage {
        case 0u: { return textureSample(tex0, smp0, uv); }
        case 1u: { return textureSample(tex1, smp1, uv); }
        case 2u: { return textureSample(tex2, smp2, uv); }
        case 3u: { return textureSample(tex3, smp3, uv); }
        case 4u: { return textureSample(tex4, smp4, uv); }
        case 5u: { return textureSample(tex5, smp5, uv); }
        case 6u: { return textureSample(tex6, smp6, uv); }
        default: { return textureSample(tex7, smp7, uv); }
    }
}

fn op(o: u32, a0: vec4f, a1: vec4f, a2: vec4f, d: vec4f, cur: vec4f, t: vec4f) -> vec4f {
    switch o {
        case 2u: { return a1; }
        case 3u: { return a2; }
        case 4u: { return a1 * a2; }
        case 5u: { return a1 * a2 * 2.0; }
        case 6u: { return a1 * a2 * 4.0; }
        case 7u: { return a1 + a2; }
        case 8u: { return a1 + a2 - 0.5; }
        case 9u: { return (a1 + a2 - 0.5) * 2.0; }
        case 10u: { return a1 - a2; }
        case 11u: { return a1 + a2 - a1 * a2; }
        case 12u: { return mix(a2, a1, d.a); }
        case 13u: { return mix(a2, a1, t.a); }
        case 14u: { return mix(a2, a1, ps.tfactor.a); }
        case 15u: { return a1 + a2 * (1.0 - t.a); }
        case 16u: { return mix(a2, a1, cur.a); }
        case 17u: { return a1; }
        case 18u: { return vec4f(a1.rgb + a1.a * a2.rgb, a1.a); }
        case 19u: { return vec4f(a1.rgb * a2.rgb + a1.a, a1.a * a2.a); }
        case 20u: { return vec4f((1.0 - a1.a) * a2.rgb + a1.rgb, a1.a); }
        case 21u: { return vec4f((1.0 - a1.rgb) * a2.rgb + a1.a, a1.a); }
        case 24u: { return vec4f(saturate(dot(a1.rgb - 0.5, a2.rgb - 0.5) * 4.0)); }
        case 25u: { return a0 + a1 * a2; }
        case 26u: { return mix(a2, a1, a0); }
        default: { return cur; }
    }
}

@group(0) @binding(18) var depth_copy: texture_2d<f32>;

// Linear view depth from reversed depth, with ps.soft.x and .y the projection's a and b in
// depth = a + b / z.
fn view_depth(depth: f32) -> f32 {
    return ps.soft.y / max(depth - ps.soft.x, 1e-7);
}

@fragment
fn main(input: In, @builtin(position) frag: vec4f) -> @location(0) vec4f {
    let z = view_depth(frag.z);
    let z_slope = fwidth(z);
    var d = input.d0;
    var s = input.d1;
    if (ps.flags.z != 0u) {
        d = input.d0f;
        s = input.d1f;
    }
    var cur = d;
    var tmp = vec4f(0.0);
    var bump = vec2f(0.0);
    var lum = 1.0;
    let coords = array<vec4f, 8>(input.t0, input.t1, input.t2, input.t3, input.t4, input.t5, input.t6, input.t7);
    // One loop body for all stages, bounded by a uniform so the compiler can't unroll it 8 times.
    for (var i = 0u; i < ps.count.x; i++) {
        let st = ps.stages[i];
        var tc = project(coords[i], st.misc.z);
        tc = vec4f(tc.xy + bump, tc.zw);
        var t = vec4f(1.0);
        if (st.misc.y != 0u) {
            t = sample(i, tc.xy);
        }
        t = vec4f(t.rgb * lum, t.a);
        bump = vec2f(0.0);
        lum = 1.0;
        var res: vec4f;
        if (st.color.x == 22u || st.color.x == 23u) {
            bump = vec2f(st.bump.x * t.x + st.bump.z * t.y, st.bump.y * t.x + st.bump.w * t.y);
            if (st.color.x == 23u) {
                lum = saturate(t.z * st.bumpl.x + st.bumpl.y);
            }
            res = cur;
        } else {
            res = op(st.color.x, arg(st.color.w, d, cur, t, s, tmp), arg(st.color.y, d, cur, t, s, tmp), arg(st.color.z, d, cur, t, s, tmp), d, cur, t);
        }
        var alpha = cur.a;
        if (st.color.x == 24u) {
            alpha = res.a;
        } else if (st.alpha.x != 1u) {
            alpha = op(st.alpha.x, arg(st.alpha.w, d, cur, t, s, tmp), arg(st.alpha.y, d, cur, t, s, tmp), arg(st.alpha.z, d, cur, t, s, tmp), d, cur, t).a;
        }
        let result = saturate(vec4f(res.rgb, alpha));
        if (st.misc.x == 5u) {
            tmp = result;
        } else {
            cur = result;
        }
    }
    var color = cur;
    if (ps.flags.w != 0u) {
        color = vec4f(saturate(color.rgb + s.rgb), color.a);
    }
    if (!passes_alpha_test(color)) {
        discard;
    }
    // Soft particles fade out as they near the opaque world behind them. Additive ones fade
    // their color, the rest their alpha. Only camera facing polygons, whose depth barely changes
    // across the screen: lines, markers and decals lying on the ground share the sorted pass.
    if (ps.soft.w != 0.0) {
        let behind = view_depth(textureLoad(depth_copy, vec2i(frag.xy), 0).r);
        let facing = saturate(1.0 - z_slope * ps.soft.z * 200.0);
        let fade = mix(1.0, saturate((behind - z) * ps.soft.z), facing);
        if (ps.soft.w > 1.5) {
            color = color * fade;
        } else {
            color.a = color.a * fade;
        }
    }
    return color;
}
",
    );
    s
}
