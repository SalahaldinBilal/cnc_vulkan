//! Command recording. Everything the game does is recorded into one command buffer per frame,
//! in the order the game does it. Render passes start lazily and end whenever something has to
//! happen outside of one (a copy, an upload, a new render target). Images stay in the GENERAL
//! layout and a full memory barrier separates every change between rendering and transfers.

use std::rc::Rc;
use std::sync::Arc;

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::Allocation;

use super::context::{self, Context, Garbage};
use super::stats;
use crate::log;

pub const FRAMES: usize = 2;
const CHUNK: u64 = 16 << 20;
const SETS_PER_POOL: u32 = 2048;

pub const TEXTURE_SLOTS: u32 = 8;
pub const BINDING_VS: u32 = 0;
pub const BINDING_PS: u32 = 1;
pub const BINDING_TEXTURES: u32 = 2;
pub const BINDING_SAMPLERS: u32 = 10;
/// The 3D world's depth as of the end of its opaque geometry, for soft particles.
pub const BINDING_DEPTH_COPY: u32 = 18;

pub struct GpuImage {
    pub image: vk::Image,
    /// All levels, for sampling. Formats without alpha read alpha as 1.
    pub view: vk::ImageView,
    /// One view per level, for drawing into.
    pub attachment_views: Vec<vk::ImageView>,
    allocation: Option<Allocation>,
    pub width: u32,
    pub height: u32,
    pub id: u64,
    pub samples: vk::SampleCountFlags,
    pub format: vk::Format,
    /// The layout the image always stays in: GENERAL, or an attachment layout for multisampled
    /// targets, which are only ever drawn into, cleared and resolved by render passes.
    pub layout: vk::ImageLayout,
    /// Set for textures, whose image goes back to the pool when it's dropped.
    pool_key: Option<PoolKey>,
}

/// What a released texture image can be used for again: format, size, levels, alpha swizzle.
type PoolKey = (vk::Format, u32, u32, u32, bool);

struct Pooled {
    key: PoolKey,
    /// The submission after which nothing reads the image any more.
    free_after: u64,
    image: vk::Image,
    view: vk::ImageView,
    allocation: Allocation,
}

/// Released texture images kept for reuse. The game creates dozens of textures some frames and
/// releases them soon after, and making an image costs far more than taking one from here.
const POOL_LIMIT: usize = 512;

static NEXT_IMAGE_ID: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

thread_local! {
    static POOL: std::cell::RefCell<Vec<Pooled>> = const { std::cell::RefCell::new(Vec::new()) };
}

fn next_id() -> u64 {
    NEXT_IMAGE_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
}

#[derive(Clone, Copy)]
pub enum ImageUse {
    Texture,
    Target,
    Depth,
    /// A depth target that shaders can also read, through a depth-only `view`.
    SampledDepth,
}

impl GpuImage {
    pub fn new(
        r: &mut Renderer,
        format: vk::Format,
        width: u32,
        height: u32,
        levels: u32,
        kind: ImageUse,
        opaque: bool,
    ) -> Rc<GpuImage> {
        Self::multisampled(r, format, width, height, levels, kind, opaque, vk::SampleCountFlags::TYPE_1)
    }

    /// A render or depth target with `samples` per pixel. Multisampled color can't be sampled,
    /// only resolved.
    #[allow(clippy::too_many_arguments)]
    pub fn multisampled(
        r: &mut Renderer,
        format: vk::Format,
        width: u32,
        height: u32,
        levels: u32,
        kind: ImageUse,
        opaque: bool,
        samples: vk::SampleCountFlags,
    ) -> Rc<GpuImage> {
        let c = context::get();
        let pool_key = matches!(kind, ImageUse::Texture).then_some((format, width, height, levels, opaque));
        if let Some(key) = pool_key {
            let completed = context::completed();
            let reused = POOL.with(|pool| {
                let mut pool = pool.borrow_mut();
                let i = pool.iter().position(|p| p.key == key && p.free_after <= completed)?;
                Some(pool.swap_remove(i))
            });
            if let Some(p) = reused {
                stats::add(&stats::TEXTURE_RECYCLED, 1);
                return Rc::new(GpuImage {
                    image: p.image,
                    view: p.view,
                    attachment_views: Vec::new(),
                    allocation: Some(p.allocation),
                    width,
                    height,
                    id: next_id(),
                    samples,
                    format,
                    layout: vk::ImageLayout::GENERAL,
                    pool_key,
                });
            }
        }
        let depth = matches!(kind, ImageUse::Depth | ImageUse::SampledDepth);
        let multisampled = samples != vk::SampleCountFlags::TYPE_1;
        let (usage, layout) = if multisampled {
            if depth {
                (vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT, vk::ImageLayout::DEPTH_STENCIL_ATTACHMENT_OPTIMAL)
            } else {
                (vk::ImageUsageFlags::COLOR_ATTACHMENT, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            }
        } else {
            let usage = vk::ImageUsageFlags::TRANSFER_SRC
                | vk::ImageUsageFlags::TRANSFER_DST
                | match kind {
                    ImageUse::Texture => vk::ImageUsageFlags::SAMPLED,
                    ImageUse::Target => vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::COLOR_ATTACHMENT,
                    ImageUse::Depth => vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                    ImageUse::SampledDepth => vk::ImageUsageFlags::SAMPLED | vk::ImageUsageFlags::DEPTH_STENCIL_ATTACHMENT,
                };
            (usage, vk::ImageLayout::GENERAL)
        };
        let image = unsafe {
            c.device.create_image(
                &vk::ImageCreateInfo::default()
                    .image_type(vk::ImageType::TYPE_2D)
                    .format(format)
                    .extent(vk::Extent3D { width, height, depth: 1 })
                    .mip_levels(levels)
                    .array_layers(1)
                    .samples(samples)
                    .tiling(vk::ImageTiling::OPTIMAL)
                    .usage(usage)
                    .initial_layout(vk::ImageLayout::UNDEFINED),
                None,
            )
        }
        .expect("vkCreateImage");
        let requirements = unsafe { c.device.get_image_memory_requirements(image) };
        let allocation = c.allocate("image", requirements, MemoryLocation::GpuOnly, false);
        unsafe { c.device.bind_image_memory(image, allocation.memory(), allocation.offset()) }.expect("vkBindImageMemory");
        let aspect = if depth { vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL } else { vk::ImageAspectFlags::COLOR };
        let view_for = |base: u32, count: u32, aspect: vk::ImageAspectFlags, swizzle: bool| unsafe {
            let a = if swizzle && opaque { vk::ComponentSwizzle::ONE } else { vk::ComponentSwizzle::IDENTITY };
            c.device
                .create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format)
                        .components(vk::ComponentMapping { r: vk::ComponentSwizzle::IDENTITY, g: vk::ComponentSwizzle::IDENTITY, b: vk::ComponentSwizzle::IDENTITY, a })
                        .subresource_range(vk::ImageSubresourceRange {
                            aspect_mask: aspect,
                            base_mip_level: base,
                            level_count: count,
                            base_array_layer: 0,
                            layer_count: 1,
                        }),
                    None,
                )
                .expect("vkCreateImageView")
        };
        let view = match kind {
            ImageUse::Depth => vk::ImageView::null(),
            ImageUse::SampledDepth => view_for(0, 1, vk::ImageAspectFlags::DEPTH, false),
            _ => view_for(0, levels, aspect, true),
        };
        let attachment_views = match kind {
            ImageUse::Texture => Vec::new(),
            _ => (0..levels).map(|l| view_for(l, 1, aspect, false)).collect(),
        };
        r.record_init(move |c, cmd| {
            let barrier = vk::ImageMemoryBarrier::default()
                .src_access_mask(vk::AccessFlags::empty())
                .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE)
                .old_layout(vk::ImageLayout::UNDEFINED)
                .new_layout(layout)
                .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
                .image(image)
                .subresource_range(vk::ImageSubresourceRange {
                    aspect_mask: aspect,
                    base_mip_level: 0,
                    level_count: levels,
                    base_array_layer: 0,
                    layer_count: 1,
                });
            unsafe {
                c.device.cmd_pipeline_barrier(
                    cmd,
                    vk::PipelineStageFlags::TOP_OF_PIPE,
                    vk::PipelineStageFlags::ALL_COMMANDS,
                    vk::DependencyFlags::empty(),
                    &[],
                    &[],
                    &[barrier],
                )
            };
        });
        Rc::new(GpuImage { image, view, attachment_views, allocation: Some(allocation), width, height, id: next_id(), samples, format, layout, pool_key })
    }

    pub fn level_extent(&self, level: u32) -> (u32, u32) {
        ((self.width >> level).max(1), (self.height >> level).max(1))
    }
}

impl Drop for GpuImage {
    fn drop(&mut self) {
        if let (Some(key), Some(allocation)) = (self.pool_key, self.allocation.take()) {
            let pooled = Pooled { key, free_after: context::recording(), image: self.image, view: self.view, allocation };
            let evicted = POOL.with(|pool| {
                let mut pool = pool.borrow_mut();
                pool.push(pooled);
                (pool.len() > POOL_LIMIT).then(|| pool.remove(0))
            });
            if let Some(old) = evicted {
                context::retire(Garbage::Image(old.image, vec![old.view], old.allocation));
            }
            return;
        }
        let mut views = self.attachment_views.clone();
        if self.view != vk::ImageView::null() {
            views.push(self.view);
        }
        if let Some(allocation) = self.allocation.take() {
            context::retire(Garbage::Image(self.image, views, allocation));
        }
    }
}

struct Chunk {
    buffer: vk::Buffer,
    _allocation: Allocation,
    ptr: *mut u8,
    size: u64,
}

#[derive(Clone, Copy)]
pub struct Slice {
    pub buffer: vk::Buffer,
    pub offset: u64,
    pub ptr: *mut u8,
}

#[derive(Default)]
struct Ring {
    chunks: Vec<Chunk>,
    current: usize,
    offset: u64,
}

impl Ring {
    fn alloc(&mut self, c: &Context, size: u64, align: u64) -> Slice {
        loop {
            if let Some(chunk) = self.chunks.get(self.current) {
                let start = self.offset.next_multiple_of(align);
                if start + size <= chunk.size {
                    self.offset = start + size;
                    return Slice { buffer: chunk.buffer, offset: start, ptr: unsafe { chunk.ptr.add(start as usize) } };
                }
                self.current += 1;
                self.offset = 0;
                continue;
            }
            let chunk_size = CHUNK.max(size.next_multiple_of(CHUNK));
            let usage = vk::BufferUsageFlags::VERTEX_BUFFER
                | vk::BufferUsageFlags::INDEX_BUFFER
                | vk::BufferUsageFlags::UNIFORM_BUFFER
                | vk::BufferUsageFlags::TRANSFER_SRC;
            let (buffer, allocation, ptr) = c.host_buffer("ring", chunk_size, usage);
            self.chunks.push(Chunk { buffer, _allocation: allocation, ptr, size: chunk_size });
            self.current = self.chunks.len() - 1;
            self.offset = 0;
        }
    }

    fn reset(&mut self) {
        self.current = 0;
        self.offset = 0;
    }
}

/// A command for later. The game thread records these, and whoever submits the frame replays them
/// into its command buffer, so the driver's command calls run off the game thread.
pub enum Cmd {
    Pipeline(vk::Pipeline),
    Viewport(vk::Viewport, vk::Rect2D),
    /// Clockwise front faces and no depth bias, set once per command buffer.
    FixedState,
    Cull(vk::CullModeFlags),
    Topology(vk::PrimitiveTopology),
    Depth(bool, bool, vk::CompareOp),
    StencilTest(bool),
    StencilOp([vk::StencilOp; 3], vk::CompareOp),
    /// Compare mask, write mask and reference.
    StencilValues([u32; 3]),
    BlendEnable(bool),
    BlendEquation(vk::BlendFactor, vk::BlendFactor, vk::BlendOp),
    WriteMask(vk::ColorComponentFlags),
    Polygon(vk::PolygonMode),
    Set(vk::DescriptorSet, [u32; 2]),
    /// Index into the recording's push constants.
    Push(u32),
    Vertices(u32, vk::Buffer, u64),
    VerticesWithStride(u32, vk::Buffer, u64, u64),
    Indices(vk::Buffer, u64, vk::IndexType),
    Draw(u32, u32),
    DrawIndexed(u32, u32, i32),
    VertexInput(Arc<VertexInput>),
    Other(Box<dyn FnOnce(&Context, vk::CommandBuffer) + Send>),
}

/// A vertex layout for `vkCmdSetVertexInputEXT`: bindings (binding, stride) and attributes
/// (location, binding, format, offset).
pub struct VertexInput {
    pub bindings: Vec<(u32, u32)>,
    pub attributes: Vec<(u32, u32, vk::Format, u32)>,
}

const MAX_VERTEX_INPUTS: usize = 32;

pub const PUSH_SIZE: usize = crate::vk::state::OBJECT_MATRICES_SIZE as usize;

#[derive(Default)]
pub struct Recording {
    cmds: Vec<Cmd>,
    pushes: Vec<[u8; PUSH_SIZE]>,
}

impl Recording {
    fn is_empty(&self) -> bool {
        self.cmds.is_empty()
    }

    /// Hands over the commands and starts again with room for as many, so the lists don't regrow every frame.
    fn take(&mut self) -> Recording {
        let fresh = Recording { cmds: Vec::with_capacity(self.cmds.len()), pushes: Vec::with_capacity(self.pushes.len()) };
        std::mem::replace(self, fresh)
    }
}

/// Records `rec` into `cmd`, from its beginning to its end.
fn replay(c: &Context, layout: vk::PipelineLayout, cmd: vk::CommandBuffer, rec: Recording) {
    let d = &c.device;
    let all = vk::StencilFaceFlags::FRONT_AND_BACK;
    unsafe {
        d.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))
            .expect("vkBeginCommandBuffer");
        for command in rec.cmds {
            match command {
                Cmd::Pipeline(p) => d.cmd_bind_pipeline(cmd, vk::PipelineBindPoint::GRAPHICS, p),
                Cmd::Viewport(viewport, scissor) => {
                    d.cmd_set_viewport(cmd, 0, &[viewport]);
                    d.cmd_set_scissor(cmd, 0, &[scissor]);
                }
                Cmd::FixedState => {
                    d.cmd_set_front_face(cmd, vk::FrontFace::CLOCKWISE);
                    d.cmd_set_depth_bias_enable(cmd, false);
                }
                Cmd::Cull(mode) => d.cmd_set_cull_mode(cmd, mode),
                Cmd::Topology(t) => d.cmd_set_primitive_topology(cmd, t),
                Cmd::Depth(test, write, op) => {
                    d.cmd_set_depth_test_enable(cmd, test);
                    d.cmd_set_depth_write_enable(cmd, write);
                    d.cmd_set_depth_compare_op(cmd, op);
                }
                Cmd::StencilTest(on) => d.cmd_set_stencil_test_enable(cmd, on),
                Cmd::StencilOp([fail, pass, depth_fail], compare) => d.cmd_set_stencil_op(cmd, all, fail, pass, depth_fail, compare),
                Cmd::StencilValues([compare_mask, write_mask, reference]) => {
                    d.cmd_set_stencil_compare_mask(cmd, all, compare_mask);
                    d.cmd_set_stencil_write_mask(cmd, all, write_mask);
                    d.cmd_set_stencil_reference(cmd, all, reference);
                }
                Cmd::BlendEnable(on) => dynamic(c).eds3.cmd_set_color_blend_enable(cmd, 0, &[on as vk::Bool32]),
                Cmd::BlendEquation(src, dst, op) => dynamic(c).eds3.cmd_set_color_blend_equation(
                    cmd,
                    0,
                    &[vk::ColorBlendEquationEXT {
                        src_color_blend_factor: src,
                        dst_color_blend_factor: dst,
                        color_blend_op: op,
                        src_alpha_blend_factor: src,
                        dst_alpha_blend_factor: dst,
                        alpha_blend_op: op,
                    }],
                ),
                Cmd::WriteMask(mask) => dynamic(c).eds3.cmd_set_color_write_mask(cmd, 0, &[mask]),
                Cmd::Polygon(mode) => dynamic(c).eds3.cmd_set_polygon_mode(cmd, mode),
                Cmd::Set(set, offsets) => d.cmd_bind_descriptor_sets(cmd, vk::PipelineBindPoint::GRAPHICS, layout, 0, &[set], &offsets),
                Cmd::Push(i) => d.cmd_push_constants(cmd, layout, vk::ShaderStageFlags::VERTEX, 0, &rec.pushes[i as usize]),
                Cmd::Vertices(binding, buffer, offset) => d.cmd_bind_vertex_buffers(cmd, binding, &[buffer], &[offset]),
                Cmd::VerticesWithStride(binding, buffer, offset, stride) => {
                    d.cmd_bind_vertex_buffers2(cmd, binding, &[buffer], &[offset], None, Some(&[stride]))
                }
                Cmd::Indices(buffer, offset, ty) => d.cmd_bind_index_buffer(cmd, buffer, offset, ty),
                Cmd::Draw(count, first) => d.cmd_draw(cmd, count, 1, first, 0),
                Cmd::DrawIndexed(count, first, base) => d.cmd_draw_indexed(cmd, count, 1, first, base, 0),
                Cmd::VertexInput(input) => {
                    let mut bindings = [vk::VertexInputBindingDescription2EXT::default(); MAX_VERTEX_INPUTS];
                    let mut attributes = [vk::VertexInputAttributeDescription2EXT::default(); MAX_VERTEX_INPUTS];
                    for (b, &(binding, stride)) in bindings.iter_mut().zip(&input.bindings) {
                        *b = b.binding(binding).stride(stride).input_rate(vk::VertexInputRate::VERTEX).divisor(1);
                    }
                    for (a, &(location, binding, format, offset)) in attributes.iter_mut().zip(&input.attributes) {
                        *a = a.location(location).binding(binding).format(format).offset(offset);
                    }
                    let (nb, na) = (input.bindings.len().min(MAX_VERTEX_INPUTS), input.attributes.len().min(MAX_VERTEX_INPUTS));
                    dynamic(c).vertex_input.cmd_set_vertex_input(cmd, &bindings[..nb], &attributes[..na]);
                }
                Cmd::Other(f) => f(c, cmd),
            }
        }
        d.end_command_buffer(cmd).expect("vkEndCommandBuffer");
    }
}

fn dynamic(c: &Context) -> &super::context::Dynamic {
    c.dynamic.as_ref().expect("dynamic state command without the extensions")
}

pub fn full_barrier(c: &Context, cmd: vk::CommandBuffer) {
    let barrier = vk::MemoryBarrier::default()
        .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
        .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
    unsafe {
        c.device.cmd_pipeline_barrier(
            cmd,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::PipelineStageFlags::ALL_COMMANDS,
            vk::DependencyFlags::empty(),
            &[barrier],
            &[],
            &[],
        )
    };
}

/// What the GPU works on after a timestamp mark.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Before,
    Clear,
    Opaque,
    DepthAndOcclusion,
    OcclusionApply,
    Translucent,
    Sorted,
    Resolve,
    Interface,
    End,
}

pub const SECTION_NAMES: [&str; 9] = [
    "before the world",
    "world clear",
    "world opaque",
    "depth copy and ambient occlusion",
    "ambient occlusion blur and apply",
    "shadows and other translucent",
    "sorted translucent and particles",
    "resolves, scaling and FXAA",
    "interface and the rest",
];

const MARKS: usize = 64;

struct Frame {
    /// Timestamps for `[debug] timers`, null when off.
    queries: vk::QueryPool,
    marks: Vec<Section>,
    pool: vk::CommandPool,
    main: vk::CommandBuffer,
    init: vk::CommandBuffer,
    fence: vk::Fence,
    /// Submission id the fence will signal for, if one is pending.
    pending: Option<u64>,
    acquire: vk::Semaphore,
    ring: Ring,
    descriptor_pools: Vec<vk::DescriptorPool>,
    descriptor_pool: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PassKey {
    pub color: (u64, u32),
    pub depth: Option<u64>,
}

pub struct Renderer {
    pub c: &'static Context,
    frames: Vec<Frame>,
    frame: usize,
    /// What the frame's command buffer and the init buffer that runs before it will hold.
    main: Recording,
    init: Recording,
    pub pass: Option<PassKey>,
    barrier_needed: bool,
    pub set_layout: vk::DescriptorSetLayout,
    pub pipeline_layout: vk::PipelineLayout,
    /// Bumped whenever the command buffer changes, so cached bindings are set again.
    pub epoch: u64,
    /// Bumped with the epoch and whenever a fullscreen pass binds its own pipeline, which leaves
    /// the device's pipeline, descriptor set and dynamic state undefined.
    pub binds: u64,
    section: Section,
}

impl Renderer {
    pub fn new(c: &'static Context) -> Renderer {
        let d = &c.device;
        let mut bindings = vec![
            vk::DescriptorSetLayoutBinding::default()
                .binding(BINDING_VS)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::VERTEX),
            vk::DescriptorSetLayoutBinding::default()
                .binding(BINDING_PS)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        ];
        for i in 0..TEXTURE_SLOTS {
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(BINDING_TEXTURES + i)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            );
            bindings.push(
                vk::DescriptorSetLayoutBinding::default()
                    .binding(BINDING_SAMPLERS + i)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .descriptor_count(1)
                    .stage_flags(vk::ShaderStageFlags::FRAGMENT),
            );
        }
        bindings.push(
            vk::DescriptorSetLayoutBinding::default()
                .binding(BINDING_DEPTH_COPY)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .descriptor_count(1)
                .stage_flags(vk::ShaderStageFlags::FRAGMENT),
        );
        let set_layout = unsafe {
            d.create_descriptor_set_layout(&vk::DescriptorSetLayoutCreateInfo::default().bindings(&bindings), None)
        }
        .expect("vkCreateDescriptorSetLayout");
        let layouts = [set_layout];
        let push = [vk::PushConstantRange::default()
            .stage_flags(vk::ShaderStageFlags::VERTEX)
            .offset(0)
            .size(crate::vk::state::OBJECT_MATRICES_SIZE)];
        let pipeline_layout = unsafe {
            d.create_pipeline_layout(&vk::PipelineLayoutCreateInfo::default().set_layouts(&layouts).push_constant_ranges(&push), None)
        }
        .expect("vkCreatePipelineLayout");

        let timers = crate::config::get().is_ok_and(|c| c.debug.timers) && c.limits.timestamp_compute_and_graphics == vk::TRUE;
        let frames = (0..FRAMES)
            .map(|_| unsafe {
                let queries = if timers {
                    let info = vk::QueryPoolCreateInfo::default().query_type(vk::QueryType::TIMESTAMP).query_count(MARKS as u32);
                    d.create_query_pool(&info, None).expect("vkCreateQueryPool")
                } else {
                    vk::QueryPool::null()
                };
                let pool = d
                    .create_command_pool(
                        &vk::CommandPoolCreateInfo::default()
                            .queue_family_index(c.queue_family)
                            .flags(vk::CommandPoolCreateFlags::TRANSIENT),
                        None,
                    )
                    .expect("vkCreateCommandPool");
                let buffers = d
                    .allocate_command_buffers(
                        &vk::CommandBufferAllocateInfo::default()
                            .command_pool(pool)
                            .level(vk::CommandBufferLevel::PRIMARY)
                            .command_buffer_count(2),
                    )
                    .expect("vkAllocateCommandBuffers");
                Frame {
                    queries,
                    marks: Vec::new(),
                    pool,
                    main: buffers[0],
                    init: buffers[1],
                    fence: d.create_fence(&vk::FenceCreateInfo::default(), None).expect("vkCreateFence"),
                    pending: None,
                    acquire: d.create_semaphore(&vk::SemaphoreCreateInfo::default(), None).expect("vkCreateSemaphore"),
                    ring: Ring::default(),
                    descriptor_pools: Vec::new(),
                    descriptor_pool: 0,
                }
            })
            .collect();
        let mut r = Renderer {
            c,
            frames,
            frame: 0,
            main: Recording::default(),
            init: Recording::default(),
            pass: None,
            barrier_needed: false,
            set_layout,
            pipeline_layout,
            epoch: 0,
            binds: 0,
            section: Section::Before,
        };
        r.begin_main();
        r
    }

    fn f(&mut self) -> &mut Frame {
        &mut self.frames[self.frame]
    }

    /// Which of the frames in flight is being recorded.
    pub fn frame_index(&self) -> usize {
        self.frame
    }

    pub fn push(&mut self, cmd: Cmd) {
        self.main.cmds.push(cmd);
    }

    pub fn push_constants(&mut self, data: &[u8]) {
        let mut bytes = [0u8; PUSH_SIZE];
        bytes.copy_from_slice(data);
        self.main.pushes.push(bytes);
        let index = self.main.pushes.len() as u32 - 1;
        self.main.cmds.push(Cmd::Push(index));
    }

    /// Records a command that isn't one of the common ones. It runs later, maybe on another thread,
    /// so it may only capture plain handles and owned data.
    pub fn record(&mut self, f: impl FnOnce(&Context, vk::CommandBuffer) + Send + 'static) {
        self.main.cmds.push(Cmd::Other(Box::new(f)));
    }

    /// Records into the buffer that runs before everything recorded for this frame. Used for work
    /// that nothing earlier depends on, like preparing new images.
    pub fn record_init(&mut self, f: impl FnOnce(&Context, vk::CommandBuffer) + Send + 'static) {
        self.init.cmds.push(Cmd::Other(Box::new(f)));
    }

    fn begin_main(&mut self) {
        self.epoch += 1;
        self.binds += 1;
        self.barrier_needed = true;
        self.pass = None;
        let queries = self.f().queries;
        if queries != vk::QueryPool::null() {
            self.f().marks.clear();
            self.record(move |c, cmd| unsafe { c.device.cmd_reset_query_pool(cmd, queries, 0, MARKS as u32) });
            self.mark(self.section);
        }
    }

    /// Writes a GPU timestamp after everything recorded so far, starting `section`.
    pub fn mark(&mut self, section: Section) {
        let frame = &mut self.frames[self.frame];
        let index = frame.marks.len();
        if frame.queries == vk::QueryPool::null() || index >= MARKS - (section != Section::End) as usize {
            return;
        }
        frame.marks.push(section);
        if section != Section::End {
            self.section = section;
        }
        let queries = frame.queries;
        self.record(move |c, cmd| unsafe { c.device.cmd_write_timestamp(cmd, vk::PipelineStageFlags::BOTTOM_OF_PIPE, queries, index as u32) });
    }

    /// Adds the time between a finished submission's marks to their sections.
    fn collect_marks(&mut self, index: usize) {
        let period = self.c.limits.timestamp_period as f64;
        let frame = &mut self.frames[index];
        if frame.marks.len() < 2 {
            frame.marks.clear();
            return;
        }
        let mut stamps = vec![0u64; frame.marks.len()];
        if unsafe { self.c.device.get_query_pool_results(frame.queries, 0, &mut stamps, vk::QueryResultFlags::TYPE_64) }.is_ok() {
            for (i, pair) in stamps.windows(2).enumerate() {
                let ns = pair[1].saturating_sub(pair[0]) as f64 * period;
                stats::add_gpu(frame.marks[i] as usize, ns as u64);
            }
        }
        frame.marks.clear();
    }

    pub fn alloc(&mut self, size: u64, align: u64) -> Slice {
        let c = self.c;
        self.f().ring.alloc(c, size, align.max(16))
    }

    pub fn upload(&mut self, data: &[u8], align: u64) -> Slice {
        let slice = self.alloc(data.len() as u64, align);
        unsafe { std::ptr::copy_nonoverlapping(data.as_ptr(), slice.ptr, data.len()) };
        slice
    }

    pub fn end_pass(&mut self) {
        if self.pass.take().is_some() {
            self.record(|c, cmd| unsafe { c.device.cmd_end_rendering(cmd) });
            self.barrier_needed = true;
        }
    }

    /// Ends any render pass, ready for transfer commands.
    pub fn transfer(&mut self) {
        self.end_pass();
        if self.barrier_needed {
            self.record(full_barrier);
        }
        self.barrier_needed = true;
    }

    pub fn begin_pass(&mut self, color: &GpuImage, level: u32, depth: Option<&GpuImage>) {
        let key = PassKey { color: (color.id, level), depth: depth.map(|d| d.id) };
        if self.pass == Some(key) {
            return;
        }
        self.end_pass();
        if self.barrier_needed {
            self.record(full_barrier);
            self.barrier_needed = false;
        }
        let (width, height) = color.level_extent(level);
        let color_view = (color.attachment_views[level as usize], color.layout);
        let depth_view = depth.map(|d| (d.attachment_views[0], d.layout));
        self.record(move |c, cmd| {
            let attachment = |(view, layout)| {
                vk::RenderingAttachmentInfo::default()
                    .image_view(view)
                    .image_layout(layout)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(vk::AttachmentStoreOp::STORE)
            };
            let color_attachment = [attachment(color_view)];
            let depth_attachment = depth_view.map(attachment);
            let mut info = vk::RenderingInfo::default()
                .render_area(vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width, height } })
                .layer_count(1)
                .color_attachments(&color_attachment);
            if let Some(d) = depth_attachment.as_ref() {
                info = info.depth_attachment(d).stencil_attachment(d);
            }
            unsafe { c.device.cmd_begin_rendering(cmd, &info) };
        });
        self.pass = Some(key);
        stats::add(&stats::PASSES, 1);
    }

    pub fn descriptor_set(&mut self) -> vk::DescriptorSet {
        let d = &self.c.device;
        let layouts = [self.set_layout];
        loop {
            let frame = &mut self.frames[self.frame];
            if frame.descriptor_pool == frame.descriptor_pools.len() {
                let sizes = [
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC, descriptor_count: SETS_PER_POOL * 2 },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLED_IMAGE, descriptor_count: SETS_PER_POOL * (TEXTURE_SLOTS + 1) },
                    vk::DescriptorPoolSize { ty: vk::DescriptorType::SAMPLER, descriptor_count: SETS_PER_POOL * TEXTURE_SLOTS },
                ];
                let pool = unsafe {
                    d.create_descriptor_pool(
                        &vk::DescriptorPoolCreateInfo::default().max_sets(SETS_PER_POOL).pool_sizes(&sizes),
                        None,
                    )
                }
                .expect("vkCreateDescriptorPool");
                frame.descriptor_pools.push(pool);
            }
            let pool = frame.descriptor_pools[frame.descriptor_pool];
            match unsafe {
                d.allocate_descriptor_sets(&vk::DescriptorSetAllocateInfo::default().descriptor_pool(pool).set_layouts(&layouts))
            } {
                Ok(sets) => return sets[0],
                Err(vk::Result::ERROR_OUT_OF_POOL_MEMORY | vk::Result::ERROR_FRAGMENTED_POOL) => frame.descriptor_pool += 1,
                Err(e) => panic!("vkAllocateDescriptorSets: {e}"),
            }
        }
    }

    /// Hands over what was recorded for the frame and counts it as submitted. Whoever takes it must
    /// record (`Submit::record`) and submit it before anything waits for its fence.
    pub fn finish(&mut self) -> Submit {
        self.end_pass();
        self.mark(Section::End);
        let frame = &self.frames[self.frame];
        let init = if self.init.is_empty() {
            None
        } else {
            self.init.cmds.push(Cmd::Other(Box::new(full_barrier)));
            Some((frame.init, self.init.take()))
        };
        let submit = Submit {
            init,
            main: (frame.main, self.main.take()),
            layout: self.pipeline_layout,
            fence: frame.fence,
            acquire: frame.acquire,
        };
        let id = context::submitted();
        self.frames[self.frame].pending = Some(id);
        submit
    }

    /// Records and submits what was recorded, here and now.
    pub fn submit(&mut self) {
        let recorded = self.finish().record(self.c);
        queue_submit(self.c, recorded.buffers(), None, None, recorded.fence);
    }

    fn wait_frame(&mut self, index: usize) {
        let d = &self.c.device;
        let frame = &mut self.frames[index];
        if let Some(id) = frame.pending.take() {
            let start = std::time::Instant::now();
            if let Err(e) = unsafe { d.wait_for_fences(&[frame.fence], true, u64::MAX) } {
                log!("FATAL: waiting for the GPU failed: {e}");
                std::process::abort();
            }
            stats::add(&stats::WAIT_US, start.elapsed().as_micros() as u64);
            unsafe { d.reset_fences(&[frame.fence]) }.expect("vkResetFences");
            context::set_completed(id);
            self.collect_marks(index);
        }
    }

    fn reset_frame(&mut self) {
        let d = &self.c.device;
        let frame = &mut self.frames[self.frame];
        unsafe { d.reset_command_pool(frame.pool, vk::CommandPoolResetFlags::empty()) }.expect("vkResetCommandPool");
        frame.ring.reset();
    }

    /// Only safe at the start of a frame, once its last submission finished.
    pub fn reset_descriptor_pools(&mut self) {
        let d = &self.c.device;
        let frame = &mut self.frames[self.frame];
        for &pool in &frame.descriptor_pools {
            unsafe { d.reset_descriptor_pool(pool, vk::DescriptorPoolResetFlags::empty()) }.expect("vkResetDescriptorPool");
        }
        frame.descriptor_pool = 0;
    }

    /// Submits everything, waits for the GPU to finish all of it and starts recording again.
    pub fn flush_and_wait(&mut self) {
        super::swapchain::drain();
        self.submit();
        for i in 0..FRAMES {
            self.wait_frame(i);
        }
        self.reset_frame();
        self.begin_main();
    }

    /// Moves to the next frame after a present, waiting for the GPU if it is a full frame behind.
    pub fn next_frame(&mut self) {
        self.frame = (self.frame + 1) % FRAMES;
        self.wait_frame(self.frame);
        self.reset_frame();
        self.section = Section::Before;
        self.begin_main();
    }

    /// Copies a level of an image into memory as tightly packed rows. Waits for the GPU.
    pub fn read_image(&mut self, image: &GpuImage, level: u32) -> Vec<u8> {
        stats::add(&stats::READBACKS, 1);
        let (w, h) = image.level_extent(level);
        let size = w as u64 * h as u64 * 4;
        let (buffer, allocation, ptr) = self.c.readback_buffer(size);
        self.transfer();
        let source = image.image;
        self.record(move |c, cmd| {
            let region = vk::BufferImageCopy::default()
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: level,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_extent(vk::Extent3D { width: w, height: h, depth: 1 });
            unsafe { c.device.cmd_copy_image_to_buffer(cmd, source, vk::ImageLayout::GENERAL, buffer, &[region]) };
        });
        self.flush_and_wait();
        let mut out = vec![0u8; size as usize];
        unsafe { std::ptr::copy_nonoverlapping(ptr, out.as_mut_ptr(), out.len()) };
        context::retire(Garbage::Buffer(buffer, allocation));
        out
    }

    /// Records a copy of tightly packed rows into a rectangle of an image level, into the init
    /// buffer or after what was recorded so far (which needs `transfer` first).
    #[allow(clippy::too_many_arguments)]
    pub fn write_image(&mut self, init: bool, image: &GpuImage, level: u32, x: u32, y: u32, w: u32, h: u32, data: &[u8]) {
        let slice = self.upload(data, 16);
        let (buffer, offset, target) = (slice.buffer, slice.offset, image.image);
        let copy = move |c: &Context, cmd| {
            let region = vk::BufferImageCopy::default()
                .buffer_offset(offset)
                .image_subresource(vk::ImageSubresourceLayers {
                    aspect_mask: vk::ImageAspectFlags::COLOR,
                    mip_level: level,
                    base_array_layer: 0,
                    layer_count: 1,
                })
                .image_offset(vk::Offset3D { x: x as i32, y: y as i32, z: 0 })
                .image_extent(vk::Extent3D { width: w, height: h, depth: 1 });
            unsafe { c.device.cmd_copy_buffer_to_image(cmd, buffer, target, vk::ImageLayout::GENERAL, &[region]) };
        };
        if init { self.record_init(copy) } else { self.record(copy) }
    }

    pub fn wait_idle(&mut self) {
        super::swapchain::drain();
        for i in 0..FRAMES {
            self.wait_frame(i);
        }
    }
}

/// A finished frame: its command buffers with what to record into them, its fence, and the
/// semaphore its swapchain image is acquired with.
pub struct Submit {
    init: Option<(vk::CommandBuffer, Recording)>,
    main: (vk::CommandBuffer, Recording),
    layout: vk::PipelineLayout,
    pub fence: vk::Fence,
    pub acquire: vk::Semaphore,
}

impl Submit {
    pub fn record(self, c: &Context) -> Recorded {
        let start = std::time::Instant::now();
        let mut recorded = Recorded { buffers: [vk::CommandBuffer::null(); 2], count: 0, fence: self.fence, acquire: self.acquire };
        for (cmd, rec) in self.init.into_iter().chain(std::iter::once(self.main)) {
            replay(c, self.layout, cmd, rec);
            recorded.buffers[recorded.count] = cmd;
            recorded.count += 1;
        }
        stats::add(&stats::RECORD_US, start.elapsed().as_micros() as u64);
        recorded
    }
}

/// A frame's ended command buffers, the init buffer first when there is one.
pub struct Recorded {
    buffers: [vk::CommandBuffer; 2],
    count: usize,
    pub fence: vk::Fence,
    pub acquire: vk::Semaphore,
}

impl Recorded {
    pub fn buffers(&self) -> &[vk::CommandBuffer] {
        &self.buffers[..self.count]
    }
}

pub fn queue_submit(c: &Context, buffers: &[vk::CommandBuffer], wait: Option<vk::Semaphore>, signal: Option<vk::Semaphore>, fence: vk::Fence) {
    let waits: Vec<vk::Semaphore> = wait.into_iter().collect();
    let stages = vec![vk::PipelineStageFlags::ALL_COMMANDS; waits.len()];
    let signals: Vec<vk::Semaphore> = signal.into_iter().collect();
    let info = vk::SubmitInfo::default()
        .wait_semaphores(&waits)
        .wait_dst_stage_mask(&stages)
        .command_buffers(buffers)
        .signal_semaphores(&signals);
    let _queue = context::QUEUE.lock().unwrap();
    if let Err(e) = unsafe { c.device.queue_submit(c.queue, &[info], fence) } {
        log!("FATAL: vkQueueSubmit failed: {e}");
        std::process::abort();
    }
}
