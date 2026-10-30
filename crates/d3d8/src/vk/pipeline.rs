//! Graphics pipelines, cached by the few pieces of D3D state Vulkan can't set dynamically:
//! shaders, vertex layout, primitive class, fill mode, blending and the depth attachment.

use std::collections::HashMap;

use ash::vk;
use rustc_hash::FxHashMap;

use super::context;
use super::shader::sm1::{self, Declaration};
use super::state::VertexInfo;
use crate::log;

/// Binding that feeds attributes the vertex data doesn't have, with a stride of 0.
pub const DUMMY_BINDING: u32 = 16;
/// Layout of the dummy binding: zeros at 0, opaque white at 16, transparent black at 20.
pub const DUMMY_DATA: [u8; 24] = [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 255, 255, 255, 255, 0, 0, 0, 0];

pub const LOC_POSITION: u32 = 0;
pub const LOC_NORMAL: u32 = 2;
pub const LOC_DIFFUSE: u32 = 4;
pub const LOC_SPECULAR: u32 = 5;
pub const LOC_TEX0: u32 = 6;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Attribute {
    pub location: u32,
    pub binding: u32,
    pub format: vk::Format,
    pub offset: u32,
}

#[derive(Clone, PartialEq, Eq, Hash, Debug, Default)]
pub struct VertexLayout {
    pub attributes: Vec<Attribute>,
    /// D3D streams the layout reads, plus the dummy binding when it is needed.
    pub bindings: Vec<u32>,
}

impl VertexLayout {
    fn finish(mut self) -> VertexLayout {
        let mut bindings: Vec<u32> = self.attributes.iter().map(|a| a.binding).collect();
        bindings.sort();
        bindings.dedup();
        self.bindings = bindings;
        self.attributes.sort_by_key(|a| a.location);
        self
    }
}

pub const FVF_XYZRHW: u32 = 0x004;
pub const FVF_NORMAL: u32 = 0x010;
pub const FVF_PSIZE: u32 = 0x020;
pub const FVF_DIFFUSE: u32 = 0x040;
pub const FVF_SPECULAR: u32 = 0x080;

fn float_format(n: u32) -> vk::Format {
    match n {
        1 => vk::Format::R32_SFLOAT,
        2 => vk::Format::R32G32_SFLOAT,
        3 => vk::Format::R32G32B32_SFLOAT,
        _ => vk::Format::R32G32B32A32_SFLOAT,
    }
}

/// The fixed function vertex layout for a flexible vertex format.
pub fn fvf_layout(fvf: u32) -> (VertexLayout, u32, VertexInfo) {
    let mut attrs = Vec::new();
    let mut offset = 0;
    let mut info = VertexInfo::default();
    let position = fvf & 0x00E;
    match position {
        0 => {}
        FVF_XYZRHW => {
            attrs.push(Attribute { location: LOC_POSITION, binding: 0, format: float_format(4), offset });
            offset += 16;
            info.rhw = true;
        }
        _ => {
            attrs.push(Attribute { location: LOC_POSITION, binding: 0, format: float_format(3), offset });
            // XYZB1 to XYZB5 carry 1 to 5 blend weights after the position.
            let weights = if position >= 0x006 { (position - 0x004) / 2 } else { 0 };
            offset += 12 + weights * 4;
        }
    }
    if fvf & FVF_NORMAL != 0 {
        attrs.push(Attribute { location: LOC_NORMAL, binding: 0, format: float_format(3), offset });
        offset += 12;
    } else {
        attrs.push(Attribute { location: LOC_NORMAL, binding: DUMMY_BINDING, format: float_format(3), offset: 0 });
    }
    if fvf & FVF_PSIZE != 0 {
        offset += 4;
    }
    if fvf & FVF_DIFFUSE != 0 {
        attrs.push(Attribute { location: LOC_DIFFUSE, binding: 0, format: vk::Format::R8G8B8A8_UNORM, offset });
        offset += 4;
        info.diffuse = true;
    } else {
        attrs.push(Attribute { location: LOC_DIFFUSE, binding: DUMMY_BINDING, format: vk::Format::R8G8B8A8_UNORM, offset: 16 });
    }
    if fvf & FVF_SPECULAR != 0 {
        attrs.push(Attribute { location: LOC_SPECULAR, binding: 0, format: vk::Format::R8G8B8A8_UNORM, offset });
        offset += 4;
        info.specular = true;
    } else {
        attrs.push(Attribute { location: LOC_SPECULAR, binding: DUMMY_BINDING, format: vk::Format::R8G8B8A8_UNORM, offset: 20 });
    }
    let count = ((fvf >> 8) & 0xF).min(8);
    for i in 0..8 {
        if i < count {
            let n = match (fvf >> (16 + i * 2)) & 3 {
                0 => 2,
                1 => 3,
                2 => 4,
                _ => 1,
            };
            attrs.push(Attribute { location: LOC_TEX0 + i, binding: 0, format: float_format(n), offset });
            offset += n * 4;
            info.tex_dims[i as usize] = n;
        } else {
            attrs.push(Attribute { location: LOC_TEX0 + i, binding: DUMMY_BINDING, format: float_format(4), offset: 0 });
            info.tex_dims[i as usize] = 2;
        }
    }
    if position == 0 {
        attrs.push(Attribute { location: LOC_POSITION, binding: DUMMY_BINDING, format: float_format(3), offset: 0 });
    }
    (VertexLayout { attributes: attrs, bindings: Vec::new() }.finish(), offset, info)
}

fn element_format(ty: u32) -> vk::Format {
    match ty {
        sm1::VSDT_FLOAT1 => vk::Format::R32_SFLOAT,
        sm1::VSDT_FLOAT2 => vk::Format::R32G32_SFLOAT,
        sm1::VSDT_FLOAT3 => vk::Format::R32G32B32_SFLOAT,
        sm1::VSDT_D3DCOLOR => vk::Format::R8G8B8A8_UNORM,
        sm1::VSDT_UBYTE4 => vk::Format::R8G8B8A8_USCALED,
        sm1::VSDT_SHORT2 => vk::Format::R16G16_SSCALED,
        sm1::VSDT_SHORT4 => vk::Format::R16G16B16A16_SSCALED,
        _ => vk::Format::R32G32B32A32_SFLOAT,
    }
}

/// The vertex layout of a vertex shader declaration.
pub fn declaration_layout(decl: &Declaration) -> VertexLayout {
    let mut attrs: Vec<Attribute> = Vec::new();
    for e in &decl.elements {
        if attrs.iter().any(|a| a.location == e.reg) {
            continue;
        }
        attrs.push(Attribute { location: e.reg, binding: e.stream, format: element_format(e.ty), offset: e.offset });
    }
    VertexLayout { attributes: attrs, bindings: Vec::new() }.finish()
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Blend {
    pub enable: bool,
    pub src: vk::BlendFactor,
    pub dst: vk::BlendFactor,
    pub op: vk::BlendOp,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct PipelineKey {
    pub vs: u32,
    pub ps: u32,
    pub layout: u32,
    pub topology: vk::PrimitiveTopology,
    pub polygon: vk::PolygonMode,
    pub blend: Blend,
    pub write_mask: vk::ColorComponentFlags,
    pub depth: bool,
    pub samples: vk::SampleCountFlags,
    pub format: vk::Format,
}

pub struct Pipelines {
    cache: FxHashMap<PipelineKey, vk::Pipeline>,
    /// The last key looked up, since most draws use the same pipeline as the one before.
    last: Option<(PipelineKey, vk::Pipeline)>,
    layouts: Vec<VertexLayout>,
    layout_ids: HashMap<VertexLayout, u32>,
    pub modules: Vec<vk::ShaderModule>,
    pipeline_cache: vk::PipelineCache,
    pipeline_layout: vk::PipelineLayout,
}

pub const MODULE_FF_VS: u32 = 0;
pub const MODULE_FF_PS: u32 = 1;

fn cache_file() -> Option<std::path::PathBuf> {
    Some(crate::paths::get().data_dir.as_ref()?.join("vulkan-pipelines.bin"))
}

impl Pipelines {
    pub fn new(ff_vs: &[u32], ff_ps: &[u32], pipeline_layout: vk::PipelineLayout) -> Pipelines {
        let c = context::get();
        let saved = cache_file().and_then(|f| std::fs::read(f).ok()).unwrap_or_default();
        let create = |data: &[u8]| unsafe {
            c.device.create_pipeline_cache(&vk::PipelineCacheCreateInfo::default().initial_data(data), None)
        };
        let pipeline_cache = match create(&saved) {
            Ok(cache) => {
                log!("pipeline cache: {} KB from disk", saved.len() / 1024);
                cache
            }
            Err(e) => {
                log!("saved pipeline cache rejected ({e}), starting empty");
                create(&[]).expect("vkCreatePipelineCache")
            }
        };
        let mut p = Pipelines {
            cache: FxHashMap::default(),
            last: None,
            layouts: Vec::new(),
            layout_ids: HashMap::new(),
            modules: Vec::new(),
            pipeline_cache,
            pipeline_layout,
        };
        p.add_module(ff_vs);
        p.add_module(ff_ps);
        p
    }

    pub fn add_module(&mut self, spirv: &[u32]) -> u32 {
        let module = unsafe {
            context::get().device.create_shader_module(&vk::ShaderModuleCreateInfo::default().code(spirv), None)
        }
        .expect("vkCreateShaderModule");
        self.modules.push(module);
        self.modules.len() as u32 - 1
    }

    pub fn layout_id(&mut self, layout: &VertexLayout) -> u32 {
        if let Some(&id) = self.layout_ids.get(layout) {
            return id;
        }
        let id = self.layouts.len() as u32;
        self.layouts.push(layout.clone());
        self.layout_ids.insert(layout.clone(), id);
        id
    }

    pub fn layout(&self, id: u32) -> &VertexLayout {
        &self.layouts[id as usize]
    }

    /// The key a pipeline is stored under. With dynamic blending and vertex layout, only shaders,
    /// primitive class and the depth attachment set pipelines apart.
    fn normalize(key: &PipelineKey) -> PipelineKey {
        if context::get().dynamic.is_none() {
            return *key;
        }
        PipelineKey {
            layout: 0,
            polygon: vk::PolygonMode::FILL,
            blend: Blend { enable: false, src: vk::BlendFactor::ONE, dst: vk::BlendFactor::ZERO, op: vk::BlendOp::ADD },
            write_mask: vk::ColorComponentFlags::RGBA,
            ..*key
        }
    }

    pub fn get(&mut self, key: &PipelineKey) -> vk::Pipeline {
        if let Some((last, p)) = self.last
            && last == *key
        {
            return p;
        }
        let p = self.lookup(key);
        self.last = Some((*key, p));
        p
    }

    fn lookup(&mut self, key: &PipelineKey) -> vk::Pipeline {
        let key = Self::normalize(key);
        if let Some(&p) = self.cache.get(&key) {
            return p;
        }
        let start = std::time::Instant::now();
        let p = build(&self.modules, &self.layouts, self.pipeline_cache, self.pipeline_layout, &key);
        super::stats::add(&super::stats::PIPELINE_US, start.elapsed().as_micros() as u64);
        log!("vulkan pipeline built during play in {:?} (shaders {} and {})", start.elapsed(), key.vs, key.ps);
        self.cache.insert(key, p);
        super::stats::add(&super::stats::PIPELINES, 1);
        self.save();
        p
    }

    /// Builds the pipelines for these shader pairs ahead of their first draw, all at once.
    /// Without dynamic blending and vertex layout the combinations aren't known, so nothing is built.
    pub fn prebuild(&mut self, pairs: &[(u32, u32)]) {
        if context::get().dynamic.is_none() {
            return;
        }
        let mut keys = Vec::new();
        for &(vs, ps) in pairs {
            for depth in [true, false] {
                let key = Self::normalize(&PipelineKey {
                    vs,
                    ps,
                    layout: 0,
                    topology: vk::PrimitiveTopology::TRIANGLE_LIST,
                    polygon: vk::PolygonMode::FILL,
                    blend: Blend { enable: false, src: vk::BlendFactor::ONE, dst: vk::BlendFactor::ZERO, op: vk::BlendOp::ADD },
                    write_mask: vk::ColorComponentFlags::RGBA,
                    depth,
                    samples: vk::SampleCountFlags::TYPE_1,
                    format: vk::Format::B8G8R8A8_UNORM,
                });
                if !self.cache.contains_key(&key) {
                    keys.push(key);
                }
            }
        }
        self.build_all(keys);
    }

    /// Builds every pipeline built so far for the back buffer again for a target with `samples`
    /// per pixel and `format`, all at once, so a new kind of target doesn't compile pipelines one
    /// draw at a time.
    pub fn prebuild_target(&mut self, samples: vk::SampleCountFlags, format: vk::Format) {
        let keys: Vec<PipelineKey> = self
            .cache
            .keys()
            .filter(|k| k.samples == vk::SampleCountFlags::TYPE_1 && k.format == vk::Format::B8G8R8A8_UNORM)
            .map(|k| PipelineKey { samples, format, ..*k })
            .filter(|k| !self.cache.contains_key(k))
            .collect();
        self.build_all(keys);
    }

    fn build_all(&mut self, keys: Vec<PipelineKey>) {
        if keys.is_empty() {
            return;
        }
        let start = std::time::Instant::now();
        let count = keys.len();
        let (modules, layouts, cache, layout) = (&self.modules, &self.layouts, self.pipeline_cache, self.pipeline_layout);
        let built: Vec<vk::Pipeline> = std::thread::scope(|s| {
            let handles: Vec<_> = keys.iter().map(|k| s.spawn(move || build(modules, layouts, cache, layout, k))).collect();
            handles.into_iter().map(|h| h.join().unwrap()).collect()
        });
        for (k, p) in keys.into_iter().zip(built) {
            self.cache.insert(k, p);
        }
        log!("{count} vulkan pipelines built ahead in {:?}, {} in total", start.elapsed(), self.cache.len());
        self.save();
    }

    /// Writes the driver's compiled pipelines to disk so the next start skips compiling them.
    fn save(&self) {
        let Some(path) = cache_file() else { return };
        let data = unsafe { context::get().device.get_pipeline_cache_data(self.pipeline_cache) };
        if let Ok(data) = data
            && let Err(e) = std::fs::write(&path, data)
        {
            log!("pipeline cache could not be saved: {e}");
        }
    }
}

fn build(
    modules: &[vk::ShaderModule],
    layouts: &[VertexLayout],
    cache: vk::PipelineCache,
    pipeline_layout: vk::PipelineLayout,
    key: &PipelineKey,
) -> vk::Pipeline {
    let c = context::get();
    let dynamic_input = c.dynamic.is_some();
    let stages = [
        vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::VERTEX).module(modules[key.vs as usize]).name(c"main"),
        vk::PipelineShaderStageCreateInfo::default().stage(vk::ShaderStageFlags::FRAGMENT).module(modules[key.ps as usize]).name(c"main"),
    ];
    let (bindings, attributes): (Vec<vk::VertexInputBindingDescription>, Vec<vk::VertexInputAttributeDescription>) = if dynamic_input {
        (Vec::new(), Vec::new())
    } else {
        let layout = &layouts[key.layout as usize];
        (
            layout.bindings.iter().map(|&b| vk::VertexInputBindingDescription { binding: b, stride: 0, input_rate: vk::VertexInputRate::VERTEX }).collect(),
            layout
                .attributes
                .iter()
                .map(|a| vk::VertexInputAttributeDescription { location: a.location, binding: a.binding, format: a.format, offset: a.offset })
                .collect(),
        )
    };
    let vertex_input = vk::PipelineVertexInputStateCreateInfo::default().vertex_binding_descriptions(&bindings).vertex_attribute_descriptions(&attributes);
    let input_assembly = vk::PipelineInputAssemblyStateCreateInfo::default().topology(key.topology);
    let viewport = vk::PipelineViewportStateCreateInfo::default().viewport_count(1).scissor_count(1);
    let raster = vk::PipelineRasterizationStateCreateInfo::default().polygon_mode(key.polygon).line_width(1.0);
    let multisample = vk::PipelineMultisampleStateCreateInfo::default().rasterization_samples(key.samples);
    let depth_stencil = vk::PipelineDepthStencilStateCreateInfo::default();
    let attachment = [vk::PipelineColorBlendAttachmentState::default()
        .blend_enable(key.blend.enable)
        .src_color_blend_factor(key.blend.src)
        .dst_color_blend_factor(key.blend.dst)
        .color_blend_op(key.blend.op)
        .src_alpha_blend_factor(key.blend.src)
        .dst_alpha_blend_factor(key.blend.dst)
        .alpha_blend_op(key.blend.op)
        .color_write_mask(key.write_mask)];
    let blend = vk::PipelineColorBlendStateCreateInfo::default().attachments(&attachment);
    let mut dynamic = vec![
        vk::DynamicState::VIEWPORT,
        vk::DynamicState::SCISSOR,
        vk::DynamicState::DEPTH_BIAS,
        vk::DynamicState::STENCIL_COMPARE_MASK,
        vk::DynamicState::STENCIL_WRITE_MASK,
        vk::DynamicState::STENCIL_REFERENCE,
        vk::DynamicState::CULL_MODE,
        vk::DynamicState::FRONT_FACE,
        vk::DynamicState::PRIMITIVE_TOPOLOGY,
        vk::DynamicState::DEPTH_TEST_ENABLE,
        vk::DynamicState::DEPTH_WRITE_ENABLE,
        vk::DynamicState::DEPTH_COMPARE_OP,
        vk::DynamicState::STENCIL_TEST_ENABLE,
        vk::DynamicState::STENCIL_OP,
        vk::DynamicState::DEPTH_BIAS_ENABLE,
    ];
    if dynamic_input {
        dynamic.extend([
            vk::DynamicState::VERTEX_INPUT_EXT,
            vk::DynamicState::COLOR_BLEND_ENABLE_EXT,
            vk::DynamicState::COLOR_BLEND_EQUATION_EXT,
            vk::DynamicState::COLOR_WRITE_MASK_EXT,
            vk::DynamicState::POLYGON_MODE_EXT,
        ]);
    } else {
        dynamic.push(vk::DynamicState::VERTEX_INPUT_BINDING_STRIDE);
    }
    let dynamic_state = vk::PipelineDynamicStateCreateInfo::default().dynamic_states(&dynamic);
    let color_formats = [key.format];
    let depth_format = if key.depth { c.depth_format } else { vk::Format::UNDEFINED };
    let mut rendering = vk::PipelineRenderingCreateInfo::default()
        .color_attachment_formats(&color_formats)
        .depth_attachment_format(depth_format)
        .stencil_attachment_format(depth_format);
    let info = vk::GraphicsPipelineCreateInfo::default()
        .stages(&stages)
        .vertex_input_state(&vertex_input)
        .input_assembly_state(&input_assembly)
        .viewport_state(&viewport)
        .rasterization_state(&raster)
        .multisample_state(&multisample)
        .depth_stencil_state(&depth_stencil)
        .color_blend_state(&blend)
        .dynamic_state(&dynamic_state)
        .layout(pipeline_layout)
        .push_next(&mut rendering);
    match unsafe { c.device.create_graphics_pipelines(cache, &[info], None) } {
        Ok(p) => p[0],
        Err((_, e)) => {
            log!("FATAL: vkCreateGraphicsPipelines failed for {key:?}: {e}");
            std::process::abort();
        }
    }
}

pub fn topology(d3d: u32) -> Option<vk::PrimitiveTopology> {
    Some(match d3d {
        1 => vk::PrimitiveTopology::POINT_LIST,
        2 => vk::PrimitiveTopology::LINE_LIST,
        3 => vk::PrimitiveTopology::LINE_STRIP,
        4 => vk::PrimitiveTopology::TRIANGLE_LIST,
        5 => vk::PrimitiveTopology::TRIANGLE_STRIP,
        6 => vk::PrimitiveTopology::TRIANGLE_FAN,
        _ => return None,
    })
}

/// The topology pipelines are built with. Dynamic topology may differ within the same class.
pub fn topology_class(t: vk::PrimitiveTopology) -> vk::PrimitiveTopology {
    match t {
        vk::PrimitiveTopology::POINT_LIST => vk::PrimitiveTopology::POINT_LIST,
        vk::PrimitiveTopology::LINE_LIST | vk::PrimitiveTopology::LINE_STRIP => vk::PrimitiveTopology::LINE_LIST,
        _ => vk::PrimitiveTopology::TRIANGLE_LIST,
    }
}

pub fn vertex_count(d3d: u32, primitives: u32) -> u32 {
    match d3d {
        1 => primitives,
        2 => primitives * 2,
        3 => primitives + 1,
        4 => primitives * 3,
        _ => primitives + 2,
    }
}

pub fn blend_factor(d3d: u32, target_alpha: bool) -> vk::BlendFactor {
    use vk::BlendFactor as B;
    match d3d {
        1 => B::ZERO,
        2 => B::ONE,
        3 => B::SRC_COLOR,
        4 => B::ONE_MINUS_SRC_COLOR,
        5 => B::SRC_ALPHA,
        6 => B::ONE_MINUS_SRC_ALPHA,
        7 => if target_alpha { B::DST_ALPHA } else { B::ONE },
        8 => if target_alpha { B::ONE_MINUS_DST_ALPHA } else { B::ZERO },
        9 => B::DST_COLOR,
        10 => B::ONE_MINUS_DST_COLOR,
        11 => B::SRC_ALPHA_SATURATE,
        _ => B::ONE,
    }
}

pub fn blend_op(d3d: u32) -> vk::BlendOp {
    match d3d {
        2 => vk::BlendOp::SUBTRACT,
        3 => vk::BlendOp::REVERSE_SUBTRACT,
        4 => vk::BlendOp::MIN,
        5 => vk::BlendOp::MAX,
        _ => vk::BlendOp::ADD,
    }
}
