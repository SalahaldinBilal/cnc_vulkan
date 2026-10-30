//! Fullscreen passes that draw one image into another with a fragment shader: the Present pass
//! (the back buffer into the swapchain, scaled to the window, through the gamma ramp) and the
//! scene resolve (the 3D world's own target into the back buffer, scaled and sharpened).

use ash::vk;

use super::context::Context;
use super::render::{FRAMES, Renderer};
use super::shader::{self, Stage};

const VERTEX: &str = "
struct Out {
    @builtin(position) pos: vec4f,
    @location(0) uv: vec2f,
}

@vertex
fn main(@builtin(vertex_index) i: u32) -> Out {
    let uv = vec2f(f32((i << 1u) & 2u), f32(i & 2u));
    var o: Out;
    o.pos = vec4f(uv * 2.0 - 1.0, 0.0, 1.0);
    o.uv = uv;
    return o;
}
";

/// Bindings 0 and 1 as a color image and its sampler, for passes that read one color image.
pub const COMMON: &str = "
@group(0) @binding(0) var source: texture_2d<f32>;
@group(0) @binding(1) var smp: sampler;
";

pub const PRESENT: &str = "
struct Post {
    // x: 1 when the source and the target have the same size, y: black level lift, z: 1 for an
    // sRGB target
    params: vec4f,
    // The calibration pattern: top left corner in target pixels, its unit (0 when it's off) and
    // its outline width
    pattern: vec4f,
    ramp: array<vec4f, 256>,
}

@group(0) @binding(2) var<uniform> post: Post;

// D3D looks up 8-bit output values. Values between them, from scaling, blend the two entries.
fn ramp(c: f32, channel: u32) -> f32 {
    let x = saturate(c) * 255.0;
    let i = u32(floor(x));
    let j = min(i + 1u, 255u);
    return mix(post.ramp[i][channel], post.ramp[j][channel], x - f32(i));
}

fn to_srgb(c: vec3f) -> vec3f {
    return select(1.055 * pow(c, vec3f(1.0 / 2.4)) - 0.055, c * 12.92, c <= vec3f(0.0031308));
}

fn to_linear(c: vec3f) -> vec3f {
    return select(pow((c + 0.055) / 1.055, vec3f(2.4)), c / 12.92, c <= vec3f(0.04045));
}

// Lifted in code values, which an sRGB target only holds after its own encoding.
fn lift(c: vec3f) -> vec3f {
    let l = post.params.y;
    if (post.params.z == 0.0) {
        return l + (1.0 - l) * c;
    }
    return to_linear(l + (1.0 - l) * to_srgb(c));
}

// The pattern's input level at a pixel, or -1 outside it. A black box 41 by 6 units with a gray
// outline, and a row of squares 4 units wide, one unit apart, from level 1 on the left to 16.
fn pattern(pos: vec2f) -> f32 {
    let k = post.pattern.z;
    let q = floor(pos) - post.pattern.xy;
    let size = vec2f(41.0, 6.0) * k;
    if (k == 0.0 || any(q < vec2f(0.0)) || any(q >= size)) {
        return -1.0;
    }
    let t = post.pattern.w;
    if (any(q < vec2f(t)) || any(q >= size - t)) {
        return 64.0 / 255.0;
    }
    let i = floor((q.x - k) / (5.0 * k));
    let in_square = q.y >= k && q.y < 5.0 * k && q.x >= k && q.x - k - i * 5.0 * k < 4.0 * k;
    if (in_square && i < 8.0) {
        var levels = array<f32, 8>(1.0, 2.0, 3.0, 4.0, 6.0, 8.0, 12.0, 16.0);
        return levels[u32(i)] / 255.0;
    }
    return 0.0;
}

@fragment
fn main(@builtin(position) pos: vec4f, @location(0) uv: vec2f) -> @location(0) vec4f {
    var c: vec3f;
    let level = pattern(pos.xy);
    if (level >= 0.0) {
        c = vec3f(level);
    } else if (post.params.x != 0.0) {
        c = textureLoad(source, vec2i(pos.xy), 0).rgb;
    } else {
        c = textureSampleLevel(source, smp, uv, 0.0).rgb;
    }
    return vec4f(lift(vec3f(ramp(c.r, 0u), ramp(c.g, 1u), ramp(c.b, 2u))), 1.0);
}
";

pub const RESOLVE: &str = "
struct Resolve {
    // source texel size in uv
    texel: vec4f,
    // x: sharpening, 0 to 1, y: 1 for FXAA, z: 1 to dither
    params: vec4f,
}

@group(0) @binding(2) var<uniform> resolve: Resolve;

fn tap(uv: vec2f) -> vec3f {
    return textureSampleLevel(source, smp, uv, 0.0).rgb;
}

fn luma(c: vec3f) -> f32 {
    return dot(c, vec3f(0.299, 0.587, 0.114));
}

// FXAA as Timothy Lottes first published it: blur along the edge direction found from the four
// diagonal neighbors, kept only when the result stays within the local luma range.
fn fxaa(uv: vec2f, m: vec3f) -> vec3f {
    let t = resolve.texel.xy;
    let nw = luma(tap(uv + vec2f(-t.x, -t.y)));
    let ne = luma(tap(uv + vec2f(t.x, -t.y)));
    let sw = luma(tap(uv + vec2f(-t.x, t.y)));
    let se = luma(tap(uv + vec2f(t.x, t.y)));
    let lm = luma(m);
    let lo = min(lm, min(min(nw, ne), min(sw, se)));
    let hi = max(lm, max(max(nw, ne), max(sw, se)));
    var dir = vec2f(-((nw + ne) - (sw + se)), (nw + sw) - (ne + se));
    let reduce = max((nw + ne + sw + se) * 0.25 * (1.0 / 8.0), 1.0 / 128.0);
    let scale = 1.0 / (min(abs(dir.x), abs(dir.y)) + reduce);
    dir = clamp(dir * scale, vec2f(-8.0), vec2f(8.0)) * t;
    let a = 0.5 * (tap(uv + dir * (1.0 / 3.0 - 0.5)) + tap(uv + dir * (2.0 / 3.0 - 0.5)));
    let b = a * 0.5 + 0.25 * (tap(uv - dir * 0.5) + tap(uv + dir * 0.5));
    let lb = luma(b);
    if (lb < lo || lb > hi) {
        return a;
    }
    return b;
}

// AMD's contrast adaptive sharpening (FidelityFX CAS) on the five taps one source texel apart:
// strong where local contrast is low, gentle on edges.
fn sharpen(uv: vec2f, c: vec3f, sharpness: f32) -> vec3f {
    let t = resolve.texel.xy;
    let n = tap(uv - vec2f(0.0, t.y));
    let s = tap(uv + vec2f(0.0, t.y));
    let w = tap(uv - vec2f(t.x, 0.0));
    let e = tap(uv + vec2f(t.x, 0.0));
    let lo = min(c, min(min(n, s), min(w, e)));
    let hi = max(c, max(max(n, s), max(w, e)));
    let amp = sqrt(saturate(min(lo, 1.0 - hi) / max(hi, vec3f(1.0 / 65536.0))));
    let weight = amp * (-1.0 / mix(8.0, 5.0, sharpness));
    return (c + (n + s + w + e) * weight) / (1.0 + 4.0 * weight);
}

fn hash(p: vec2u) -> f32 {
    var v = p.x * 1664525u + p.y * 1013904223u;
    v ^= v >> 16u;
    v *= 0x7feb352du;
    v ^= v >> 15u;
    v *= 0x846ca68bu;
    v ^= v >> 16u;
    return f32(v) / 4294967295.0;
}

// Filtered scaling, FXAA, sharpening, then triangular dithering when a wider source goes into
// the 8-bit back buffer, so the extra precision shows as fine noise instead of bands.
@fragment
fn main(@builtin(position) pos: vec4f, @location(0) uv: vec2f) -> @location(0) vec4f {
    var c = tap(uv);
    if (resolve.params.y != 0.0) {
        c = fxaa(uv, c);
    }
    if (resolve.params.x > 0.0) {
        c = sharpen(uv, c, resolve.params.x);
    }
    if (resolve.params.z != 0.0) {
        let p = vec2u(pos.xy);
        c += (hash(p) + hash(p + vec2u(7919u, 104729u)) - 1.0) / 255.0;
    }
    return vec4f(saturate(c), 1.0);
}
";

/// A low resolution image (the fog of war, one texel per cell) drawn larger through a cubic
/// B-spline, so cell edges come out as smooth curves, alpha included. Four bilinear taps (Sigg
/// and Hadwiger).
pub const SMOOTH_UPSCALE: &str = "
struct Upscale {
    // xy: source texel size in uv, zw: source size
    texel: vec4f,
}

@group(0) @binding(2) var<uniform> upscale: Upscale;

@fragment
fn main(@location(0) uv: vec2f) -> @location(0) vec4f {
    let p = uv * upscale.texel.zw - 0.5;
    let i = floor(p);
    let f = p - i;
    let f2 = f * f;
    let f3 = f2 * f;
    let w0 = (1.0 - 3.0 * f + 3.0 * f2 - f3) / 6.0;
    let w1 = (4.0 - 6.0 * f2 + 3.0 * f3) / 6.0;
    let w2 = (1.0 + 3.0 * f + 3.0 * f2 - 3.0 * f3) / 6.0;
    let w3 = f3 / 6.0;
    let g0 = w0 + w1;
    let g1 = w2 + w3;
    let h0 = (i + 0.5 - 1.0 + w1 / g0) * upscale.texel.xy;
    let h1 = (i + 0.5 + 1.0 + w3 / g1) * upscale.texel.xy;
    let a = textureSampleLevel(source, smp, vec2f(h0.x, h0.y), 0.0);
    let b = textureSampleLevel(source, smp, vec2f(h1.x, h0.y), 0.0);
    let c = textureSampleLevel(source, smp, vec2f(h0.x, h1.y), 0.0);
    let d = textureSampleLevel(source, smp, vec2f(h1.x, h1.y), 0.0);
    let top = a * g0.x + b * g1.x;
    let bottom = c * g0.x + d * g1.x;
    return top * g0.y + bottom * g1.y;
}
";

/// The world's depth into a plain float image, from a single sampled depth buffer.
pub const DEPTH_COPY: &str = "
@group(0) @binding(0) var depth: texture_depth_2d;

@fragment
fn main(@builtin(position) pos: vec4f) -> @location(0) vec4f {
    return vec4f(textureLoad(depth, vec2i(pos.xy), 0), 0.0, 0.0, 1.0);
}
";

/// Declarations shared by the ambient occlusion passes.
const AO_COMMON: &str = "
struct Ao {
    // target size, then its inverse
    size: vec4f,
    // reversed depth = x + y / z, then the projection's x and y scale
    proj: vec4f,
    // radius in world units, intensity, bias in world units, pixels per world unit at depth 1
    params: vec4f,
}

@group(0) @binding(2) var<uniform> ao: Ao;

fn linear_depth(d: f32) -> f32 {
    return ao.proj.y / max(d - ao.proj.x, 1e-7);
}

fn clamp_pixel(p: vec2i) -> vec2i {
    return clamp(p, vec2i(0), vec2i(ao.size.xy) - 1);
}
";

/// Scalable ambient obscurance (McGuire, Mara and Luebke 2012) from the depth copy, with
/// normals rebuilt from neighboring depths. Writes the light that reaches each pixel, 0 to 1.
/// Every pixel uses the same spiral, so a spot keeps its value as the picture moves over it.
pub fn ambient_occlusion() -> String {
    format!(
        "{AO_COMMON}
@group(0) @binding(0) var depth: texture_2d<f32>;

const SAMPLES: u32 = 16u;
const TURNS: f32 = 7.0;

// The view space position at a pixel. D3D's y points up, pixel rows go down.
fn position(p: vec2i) -> vec3f {{
    let q = clamp_pixel(p);
    let z = linear_depth(textureLoad(depth, q, 0).r);
    let ndc = vec2f((f32(q.x) + 0.5) * ao.size.z * 2.0 - 1.0, 1.0 - (f32(q.y) + 0.5) * ao.size.w * 2.0);
    return vec3f(ndc.x * z / ao.proj.z, ndc.y * z / ao.proj.w, z);
}}

@fragment
fn main(@builtin(position) pos: vec4f) -> @location(0) vec4f {{
    let p = vec2i(pos.xy);
    if (textureLoad(depth, p, 0).r <= 0.0) {{
        return vec4f(1.0);
    }}
    let c = position(p);
    // Of the two neighbors on each axis, the one nearer in depth, so edges don't bend normals.
    let r = position(p + vec2i(1, 0)) - c;
    let l = c - position(p - vec2i(1, 0));
    let d = position(p + vec2i(0, 1)) - c;
    let u = c - position(p - vec2i(0, 1));
    let dx = select(l, r, abs(r.z) < abs(l.z));
    let dy = select(u, d, abs(d.z) < abs(u.z));
    var n = normalize(cross(dx, dy));
    if (dot(n, c) > 0.0) {{
        n = -n;
    }}
    let radius = ao.params.x;
    let radius_px = radius * ao.params.w / c.z;
    if (radius_px < 1.0) {{
        return vec4f(1.0);
    }}
    let r2 = radius * radius;
    var sum = 0.0;
    for (var i = 0u; i < SAMPLES; i++) {{
        let a = (f32(i) + 0.5) / f32(SAMPLES);
        let angle = a * TURNS * 6.2831853;
        let offset = vec2f(cos(angle), sin(angle)) * a * radius_px;
        let v = position(p + vec2i(round(offset))) - c;
        let vv = dot(v, v);
        let f = max(r2 - vv, 0.0);
        // Times the radius, so the strength doesn't depend on the size of a world unit.
        sum += f * f * f * max((dot(v, n) - ao.params.z) / (vv + 0.01 * r2), 0.0) * radius;
    }}
    let light = max(0.0, 1.0 - sum * ao.params.y * 5.0 / (r2 * r2 * r2 * f32(SAMPLES)));
    return vec4f(light, 0.0, 0.0, 1.0);
}}
"
    )
}

/// Multiplies the occlusion into the world, blurred over 3x3 pixels at the same depth.
pub fn ambient_occlusion_apply() -> String {
    format!(
        "{AO_COMMON}
@group(0) @binding(0) var occlusion: texture_2d<f32>;
@group(0) @binding(3) var depth: texture_2d<f32>;

@fragment
fn main(@builtin(position) pos: vec4f) -> @location(0) vec4f {{
    let p = vec2i(pos.xy);
    let z = linear_depth(textureLoad(depth, p, 0).r);
    var sum = 0.0;
    var weight = 0.0;
    for (var y = -1; y <= 1; y++) {{
        for (var x = -1; x <= 1; x++) {{
            let q = clamp_pixel(p + vec2i(x, y));
            let w = max(0.0, 1.0 - abs(linear_depth(textureLoad(depth, q, 0).r) - z) / (0.05 * z));
            sum += textureLoad(occlusion, q, 0).r * w;
            weight += w;
        }}
    }}
    let light = sum / weight;
    // Most of the world isn't occluded. Multiplying it by 1 would still blend every sample.
    if (light > 0.998) {{
        discard;
    }}
    return vec4f(vec3f(light), 1.0);
}}
"
    )
}

/// Uniforms for the ambient occlusion passes. `a` and `b` give reversed depth = a + b / z,
/// `sx` and `sy` are the projection's scale on x and y.
#[allow(clippy::too_many_arguments)]
pub fn ao_uniform(width: u32, height: u32, a: f32, b: f32, sx: f32, sy: f32, radius: f32, intensity: f32) -> Vec<u8> {
    let (w, h) = (width as f32, height as f32);
    as_bytes(&[w, h, 1.0 / w, 1.0 / h, a, b, sx, sy, radius, intensity, radius * 0.05, 0.5 * h * sy])
}

/// How many times one pass may run per frame before its descriptor sets wrap around.
const SETS_PER_FRAME: usize = 8;

pub struct Pass {
    name: &'static str,
    vs: vk::ShaderModule,
    fs: vk::ShaderModule,
    _set_layout: vk::DescriptorSetLayout,
    pipeline_layout: vk::PipelineLayout,
    _pool: vk::DescriptorPool,
    sets: Vec<vk::DescriptorSet>,
    sampler: vk::Sampler,
    pipeline: vk::Pipeline,
    format: vk::Format,
    samples: vk::SampleCountFlags,
    /// Source and destination blend factors when the pass blends into what the target holds.
    blend: Option<(vk::BlendFactor, vk::BlendFactor)>,
    epoch: u64,
    used: usize,
    warned: bool,
}

impl Pass {
    /// A pass with `fragment` as its whole fragment shader. Bindings: 0 an image, 1 a linear
    /// sampler, 2 the uniforms, 3 a second image.
    pub fn new(c: &Context, name: &'static str, fragment: &str, blend: Option<(vk::BlendFactor, vk::BlendFactor)>) -> Result<Pass, String> {
        let d = &c.device;
        let module = |source: &str, stage: Stage| -> Result<vk::ShaderModule, String> {
            let spirv = shader::compile(source, stage).map_err(|e| format!("{name} shader: {e}"))?;
            unsafe { d.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(&spirv), None) }
                .map_err(|e| format!("vkCreateShaderModule: {e}"))
        };
        let vs = module(VERTEX, Stage::Vertex)?;
        let fs = module(fragment, Stage::Fragment)?;
        let bindings = [
            vk::DescriptorSetLayoutBinding::default()
                .binding(0)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(1)
                .descriptor_type(vk::DescriptorType::SAMPLER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(2)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            vk::DescriptorSetLayoutBinding::default()
                .binding(3)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        let set_layout = unsafe { d.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None) }
            .expect("vkCreateDescriptorSetLayout");
        let set_layouts = [set_layout];
        let pipeline_layout = unsafe { d.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&set_layouts), None) }
            .expect("vkCreatePipelineLayout");
        let count = (FRAMES * SETS_PER_FRAME) as u32;
        let sizes = [
            vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: count * 2 },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLER, descriptor_count: count },
            vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_BUFFER, descriptor_count: count },
        ];
        let pool = unsafe { d.create_descriptor_pool(&vk::DescriptorPoolCreateInfo::default().max_sets(count).pool_sizes(&sizes), None) }
            .expect("vkCreateDescriptorPool");
        let layouts = vec![set_layout; count as usize];
        let sets = unsafe { d.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts)) }
            .expect("vkAllocateDescriptorSets");
        let sampler = unsafe {
            d.create_sampler(
                &vk::SamplerCreateInfo::default()
                    .mag_filter(vk::Filter::LINEAR)
                    .min_filter(vk::Filter::LINEAR)
                    .mipmap_mode(vk::SamplerMipmapMode::NEAREST)
                    .address_mode_u(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_v(vk::SamplerAddressMode::CLAMP_TO_EDGE)
                    .address_mode_w(vk::SamplerAddressMode::CLAMP_TO_EDGE),
                None,
            )
        }
        .expect("vkCreateSampler");
        Ok(Pass {
            name,
            vs,
            fs,
            _set_layout: set_layout,
            pipeline_layout,
            _pool: pool,
            sets,
            sampler,
            pipeline: vk::Pipeline::null(),
            format: vk::Format::UNDEFINED,
            samples: vk::SampleCountFlags::TYPE_1,
            blend,
            epoch: 0,
            used: 0,
            warned: false,
        })
    }

    /// Builds the pipeline for the target's format, if it changed.
    pub fn set_format(&mut self, c: &Context, format: vk::Format) {
        self.set_target(c, format, vk::SampleCountFlags::TYPE_1);
    }

    /// Builds the pipeline for the target's format and sample count, if they changed.
    pub fn set_target(&mut self, c: &Context, format: vk::Format, samples: vk::SampleCountFlags) {
        if format == self.format && samples == self.samples && self.pipeline != vk::Pipeline::null() {
            return;
        }
        let stages = [
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(self.vs).name(c"main"),
            vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(self.fs).name(c"main"),
        ];
        let vertex_input = vk::PipelineVertexInputStateCreateInfo::default();
        let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(vk::PrimitiveTopology::TRIANGLE_LIST);
        let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
        let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(vk::PolygonMode::FILL).line_width(1.0);
        let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(samples);
        let (src, dst) = self.blend.unwrap_or((vk::BlendFactor::ONE, vk::BlendFactor::ZERO));
        let attachment = [vk::PipelineColorBlendAttachmentState::default()
            .blend_enable(self.blend.is_some())
            .src_color_blend_factor(src)
            .dst_color_blend_factor(dst)
            .color_blend_op(vk::BlendOp::ADD)
            .src_alpha_blend_factor(vk::BlendFactor::ZERO)
            .dst_alpha_blend_factor(vk::BlendFactor::ONE)
            .alpha_blend_op(vk::BlendOp::ADD)
            .color_write_mask(vk::ColorComponentFlags::RGBA)];
        let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
        let dynamic = [vk::DynamicState::VIEWPORT, vk::DynamicState::SCISSOR];
        let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic);
        let formats = [format];
        let mut rendering = vk::PipelineRenderingCreateInfo::default().color_attachment_formats(&formats);
        let info = vk::GraphicsPipelineCreateInfo::default()
            .stages(&stages)
            .vertex_input_state(&vertex_input)
            .input_assembly_state(&input_assembly)
            .viewport_state(&viewport)
            .rasterization_state(&raster)
            .multisample_state(&multisample)
            .color_blend_state(&blend)
            .dynamic_state(&dynamic_state)
            .layout(self.pipeline_layout)
            .push_next(&mut rendering);
        let pipeline = match unsafe { c.device.create_graphics_pipelines(vk::PipelineCache::null(), &[info], None) } {
            Ok(p) => p[0],
            Err((_, e)) => {
                crate::log!("FATAL: the {} pipeline could not be built: {e}", self.name);
                std::process::abort();
            }
        };
        if self.pipeline != vk::Pipeline::null() {
            // Commands recorded for later may still name it.
            super::context::retire(super::context::Garbage::Pipeline(self.pipeline));
        }
        self.pipeline = pipeline;
        self.format = format;
        self.samples = samples;
    }

    /// Records the pass into `target`, covering `extent`. `sources` are bindings 0 and 3 (the
    /// first stands in for the second when there's one). The renderer must be outside a render
    /// pass, past a barrier that makes the sources readable (`Renderer::transfer`).
    pub fn draw(&mut self, r: &mut Renderer, sources: &[vk::ImageView], target: vk::ImageView, target_layout: vk::ImageLayout, extent: vk::Extent2D, uniform: &[u8]) {
        let c = r.c;
        if self.epoch != r.epoch {
            self.epoch = r.epoch;
            self.used = 0;
        }
        if self.used == SETS_PER_FRAME && !self.warned {
            crate::log!("the {} pass ran more than {SETS_PER_FRAME} times in one frame", self.name);
            self.warned = true;
        }
        let set = self.sets[r.frame_index() * SETS_PER_FRAME + self.used % SETS_PER_FRAME];
        self.used += 1;
        let size = uniform.len() as u64;
        let uniform = r.upload(uniform, c.limits.min_uniform_buffer_offset_alignment);
        r.binds += 1;
        self.write_set(c, set, sources, (uniform.buffer, uniform.offset, size));
        let draw = self.commands(set, target, target_layout, extent);
        r.record(move |c, cmd| draw.record(c, cmd));
    }

    /// The first descriptor set of frame `frame`, for a pass that runs once a frame without the renderer.
    pub fn frame_set(&self, frame: usize) -> vk::DescriptorSet {
        self.sets[frame * SETS_PER_FRAME]
    }

    /// Points `set` at the sources and the uniforms (buffer, offset, size). Only while no command
    /// buffer that could still run holds the set.
    pub fn write_set(&self, c: &Context, set: vk::DescriptorSet, sources: &[vk::ImageView], (buffer, offset, size): (vk::Buffer, u64, u64)) {
        let image_info = [vk::DescriptorImageInfo::default().image_view(sources[0]).image_layout(vk::ImageLayout::GENERAL)];
        let second_info = [vk::DescriptorImageInfo::default().image_view(*sources.get(1).unwrap_or(&sources[0])).image_layout(vk::ImageLayout::GENERAL)];
        let sampler_info = [vk::DescriptorImageInfo::default().sampler(self.sampler)];
        let buffer_info = [vk::DescriptorBufferInfo::default().buffer(buffer).offset(offset).range(size)];
        let writes = [
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(0).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&image_info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(1).descriptor_type(vk::DescriptorType::SAMPLER).image_info(&sampler_info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(2).descriptor_type(vk::DescriptorType::UNIFORM_BUFFER).buffer_info(&buffer_info),
            vk::WriteDescriptorSet::default().dst_set(set).dst_binding(3).descriptor_type(vk::DescriptorType::SAMPLED_IMAGE).image_info(&second_info),
        ];
        unsafe { c.device.update_descriptor_sets(&writes, &[]) };
    }

    /// The pass's commands with `set`, ready to record now or later.
    pub fn commands(&self, set: vk::DescriptorSet, target: vk::ImageView, target_layout: vk::ImageLayout, extent: vk::Extent2D) -> PassDraw {
        PassDraw { pipeline: self.pipeline, layout: self.pipeline_layout, set, target, target_layout, extent, load: self.blend.is_some() }
    }
}

/// One run of a pass, with everything its commands need.
pub struct PassDraw {
    pipeline: vk::Pipeline,
    layout: vk::PipelineLayout,
    set: vk::DescriptorSet,
    target: vk::ImageView,
    target_layout: vk::ImageLayout,
    extent: vk::Extent2D,
    /// Keeps what the target holds, for a pass that blends into it.
    load: bool,
}

impl PassDraw {
    pub fn record(&self, c: &Context, cmd: vk::CommandBuffer) {
        let d = &c.device;
        let load = if self.load { vk::AttachmentLoadOp::LOAD } else { vk::AttachmentLoadOp::DONT_CARE };
        let color = [vk::RenderingAttachmentInfo::default()
            .image_view(self.target)
            .image_layout(self.target_layout)
            .load_op(load)
            .store_op(vk::AttachmentStoreOp::STORE)];
        let area = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: self.extent };
        let viewport = vk::Viewport { x: 0.0, y: 0.0, width: self.extent.width as f32, height: self.extent.height as f32, min_depth: 0.0, max_depth: 1.0 };
        unsafe {
            d.cmd_begin_rendering(cmd, &vk::RenderingInfo::default().render_area(area).layer_count(1).color_attachments(&color));
            d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, self.pipeline);
            d.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, self.layout, 0, &[self.set], &[]);
            d.cmd_set_viewport(cmd, 0, &[viewport]);
            d.cmd_set_scissor(cmd, 0, &[area]);
            d.cmd_draw(cmd, 3, 1, 0, 0);
            d.cmd_end_rendering(cmd);
        }
    }
}

fn as_bytes(data: &[f32]) -> Vec<u8> {
    data.iter().flat_map(|f| f.to_le_bytes()).collect()
}

pub const PRESENT_UNIFORM_SIZE: u64 = (8 + 256 * 4) * 4;

/// The calibration pattern as large as fits centered in `area` (left, top, width, height in
/// target pixels): top left corner, unit and outline width, for `present_uniform`.
pub fn calibration_pattern([x, y, w, h]: [f32; 4]) -> [f32; 4] {
    let unit = (w / 41.0).min(h / 6.0).floor().max(1.0);
    let left = x + (w - 41.0 * unit) / 2.0;
    let top = y + (h - 6.0 * unit) / 2.0;
    [left.round().max(0.0), top.round().max(0.0), unit, (unit / 5.0).round().max(1.0)]
}

/// Uniforms for `PRESENT`. `lift` raises black to that fraction of full scale, `pattern` comes
/// from `calibration_pattern`.
pub fn present_uniform(same_size: bool, gamma: &[u16; 768], lift: f32, srgb: bool, pattern: Option<[f32; 4]>) -> Vec<u8> {
    let mut data = vec![0f32; PRESENT_UNIFORM_SIZE as usize / 4];
    data[0] = if same_size { 1.0 } else { 0.0 };
    data[1] = lift;
    data[2] = if srgb { 1.0 } else { 0.0 };
    data[4..8].copy_from_slice(&pattern.unwrap_or([0.0; 4]));
    for i in 0..256 {
        for channel in 0..3 {
            data[8 + i * 4 + channel] = gamma[channel * 256 + i] as f32 / 65535.0;
        }
    }
    as_bytes(&data)
}

/// Uniforms for `SMOOTH_UPSCALE`.
pub fn smooth_uniform(source_width: u32, source_height: u32) -> Vec<u8> {
    let (w, h) = (source_width as f32, source_height as f32);
    as_bytes(&[1.0 / w, 1.0 / h, w, h])
}

/// Uniforms for `RESOLVE`.
pub fn resolve_uniform(source_width: u32, source_height: u32, sharpness: f32, fxaa: bool, dither: bool) -> Vec<u8> {
    let flag = |on: bool| if on { 1.0 } else { 0.0 };
    as_bytes(&[1.0 / source_width as f32, 1.0 / source_height as f32, 0.0, 0.0, sharpness, flag(fxaa), flag(dither), 0.0])
}
