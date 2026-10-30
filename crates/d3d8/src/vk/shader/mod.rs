//! Shaders for the Vulkan backend, written as WGSL and compiled to SPIR-V with naga. The fixed
//! function stages are one shader each, driven by the uniforms. The game's own D3D8 shaders are
//! translated when it creates them.

pub mod ff;
pub mod sm1;

use naga::back::spv;
use naga::valid::{Capabilities, ValidationFlags, Validator};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Stage {
    Vertex,
    Fragment,
}

pub fn compile(source: &str, stage: Stage) -> Result<Vec<u32>, String> {
    let module = naga::front::wgsl::parse_str(source).map_err(|e| e.emit_to_string(source))?;
    let info = Validator::new(ValidationFlags::all(), Capabilities::IMMEDIATES | Capabilities::CLIP_DISTANCES)
        .validate(&module)
        .map_err(|e| e.emit_to_string(source))?;
    let options = spv::Options { lang_version: (1, 3), flags: spv::WriterFlags::empty(), ..Default::default() };
    let pipeline = spv::PipelineOptions {
        shader_stage: match stage {
            Stage::Vertex => naga::ShaderStage::Vertex,
            Stage::Fragment => naga::ShaderStage::Fragment,
        },
        entry_point: "main".into(),
    };
    spv::write_vec(&module, &info, &options, Some(&pipeline)).map_err(|e| e.to_string())
}

pub const CLIP_PLANES: usize = 6;

/// Declarations shared by every vertex shader.
pub const VERTEX_OUT: &str = "enable clip_distances;

struct Out {
    @builtin(position) pos: vec4f,
    @builtin(clip_distances) clip: array<f32, 6>,
    @location(0) d0: vec4f,
    @location(1) d1: vec4f,
    @location(2) @interpolate(flat) d0f: vec4f,
    @location(3) @interpolate(flat) d1f: vec4f,
    @location(4) t0: vec4f,
    @location(5) t1: vec4f,
    @location(6) t2: vec4f,
    @location(7) t3: vec4f,
    @location(8) t4: vec4f,
    @location(9) t5: vec4f,
    @location(10) t6: vec4f,
    @location(11) t7: vec4f,
}

// D3D clip space to Vulkan: y points down, and pixel centers sit half a pixel further in.
fn to_vulkan(p: vec4f, viewport: vec4f) -> vec4f {
    return vec4f(p.x + p.w / viewport.z, -p.y + p.w / viewport.w, p.z, p.w);
}
";

/// Declarations shared by every fragment shader: inputs, uniforms, textures and the alpha test.
pub const FRAGMENT_COMMON: &str = "
struct In {
    @location(0) d0: vec4f,
    @location(1) d1: vec4f,
    @location(2) @interpolate(flat) d0f: vec4f,
    @location(3) @interpolate(flat) d1f: vec4f,
    @location(4) t0: vec4f,
    @location(5) t1: vec4f,
    @location(6) t2: vec4f,
    @location(7) t3: vec4f,
    @location(8) t4: vec4f,
    @location(9) t5: vec4f,
    @location(10) t6: vec4f,
    @location(11) t7: vec4f,
}

struct Stage {
    color: vec4u,
    alpha: vec4u,
    misc: vec4u,
    bump: vec4f,
    bumpl: vec4f,
}

struct Ps {
    stages: array<Stage, 8>,
    tfactor: vec4f,
    flags: vec4u,
    c: array<vec4f, 8>,
    // stages to run
    count: vec4u,
    // soft particles: depth = x + y / z, 1 / fade distance, mode (0 off, 1 alpha, 2 color)
    soft: vec4f,
}

@group(0) @binding(1) var<uniform> ps: Ps;
@group(0) @binding(2) var tex0: texture_2d<f32>;
@group(0) @binding(3) var tex1: texture_2d<f32>;
@group(0) @binding(4) var tex2: texture_2d<f32>;
@group(0) @binding(5) var tex3: texture_2d<f32>;
@group(0) @binding(6) var tex4: texture_2d<f32>;
@group(0) @binding(7) var tex5: texture_2d<f32>;
@group(0) @binding(8) var tex6: texture_2d<f32>;
@group(0) @binding(9) var tex7: texture_2d<f32>;
@group(0) @binding(10) var smp0: sampler;
@group(0) @binding(11) var smp1: sampler;
@group(0) @binding(12) var smp2: sampler;
@group(0) @binding(13) var smp3: sampler;
@group(0) @binding(14) var smp4: sampler;
@group(0) @binding(15) var smp5: sampler;
@group(0) @binding(16) var smp6: sampler;
@group(0) @binding(17) var smp7: sampler;

// Divides by the last coordinate when the stage asks for a projected lookup.
fn project(t: vec4f, ttff: u32) -> vec4f {
    if ((ttff & 256u) == 0u) {
        return t;
    }
    let k = clamp(ttff & 255u, 2u, 4u) - 1u;
    return t / t[k];
}

fn passes_alpha_test(color: vec4f) -> bool {
    let func = ps.flags.x;
    if (func == 0u) {
        return true;
    }
    let a = u32(round(saturate(color.a) * 255.0));
    let r = ps.flags.y;
    switch func {
        case 1u: { return false; }
        case 2u: { return a < r; }
        case 3u: { return a == r; }
        case 4u: { return a <= r; }
        case 5u: { return a > r; }
        case 6u: { return a != r; }
        case 7u: { return a >= r; }
        default: { return true; }
    }
}
";
