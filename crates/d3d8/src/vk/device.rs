//! Our IDirect3DDevice8, rendering with Vulkan.

use std::cell::Cell;
use std::collections::HashSet;
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicPtr, AtomicU32, Ordering};

use ash::vk;
use gpu_allocator::vulkan::Allocation;
use rustc_hash::FxHashMap;
use windows_sys::Win32::Foundation::HWND;

use super::buffer::{self, Buffer, BufferData};
use super::com::{self, D3DERR_INVALIDCALL, D3DERR_NOTFOUND, S_FALSE, Vtbl, vtbl};
use super::context::{self, Garbage};
use super::format::{self, Info};
use super::pipeline::{self, Blend, DUMMY_BINDING, MODULE_FF_PS, MODULE_FF_VS, PipelineKey, Pipelines};
use super::post::{self, Pass};
use super::render::{BINDING_DEPTH_COPY, BINDING_PS, BINDING_SAMPLERS, BINDING_TEXTURES, BINDING_VS, Cmd, FRAMES, GpuImage, ImageUse, Renderer, Section, Slice, VertexInput};
use super::shader::sm1::{self, Declaration};
use super::shader::{self, Stage, ff};
use super::state::{self, Light, Material, Matrix, State, VertexInfo, Viewport};
use super::swapchain::Swapchain;
use super::texture::{self, Backing, Rect, Surface, SurfaceData, Texture};
use super::{display, stats};
use crate::d3d::{D3D_OK, D3DDISPLAYMODE, D3DPRESENT_PARAMETERS, HRESULT};
use crate::{config, direct3d, frame, log, pacing, window};

struct VertexShader {
    decl: Declaration,
    decl_tokens: Vec<u32>,
    function: Vec<u32>,
    module: u32,
    layout: u32,
    info: VertexInfo,
}

struct PixelShader {
    function: Vec<u32>,
    module: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SamplerKey([u32; 11]);

/// Everything a descriptor set holds. Each frame in flight has its own uniform buffers, so its own sets.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
struct SetKey {
    buffers: [vk::Buffer; 2],
    views: [vk::ImageView; 8],
    samplers: [vk::Sampler; 8],
    depth_copy: vk::ImageView,
}

const SET_CACHE_LIMIT: usize = 8192;

/// The dynamic state a draw sets.
#[derive(Clone, Copy, PartialEq)]
struct DrawState {
    /// x, y, width, height, then the depth range as f32 bits.
    viewport: [u32; 6],
    cull: vk::CullModeFlags,
    topology: vk::PrimitiveTopology,
    depth: (bool, bool, vk::CompareOp),
    stencil_test: bool,
    stencil_ops: [vk::StencilOp; 3],
    stencil_compare: vk::CompareOp,
    /// Compare mask, write mask and reference.
    stencil_values: [u32; 3],
    blend: Blend,
    write_mask: vk::ColorComponentFlags,
    polygon: vk::PolygonMode,
}

const VERTEX_BINDINGS: usize = DUMMY_BINDING as usize + 1;

/// What the command buffer has bound, so a draw only records what changed since the last one.
#[derive(Default)]
struct Bound {
    pipeline: vk::Pipeline,
    state: Option<DrawState>,
    /// Vertex layout and the stride of each binding.
    vertex_input: Option<(u32, [u32; VERTEX_BINDINGS])>,
    /// Buffer, offset and stride of each binding.
    buffers: [Option<(vk::Buffer, u64, u32)>; VERTEX_BINDINGS],
    set: Option<(vk::DescriptorSet, [u32; 2])>,
    /// The fixed function vertex shader's object matrices last pushed.
    object: Option<[Matrix; 2]>,
}

/// Puts uniforms in the ring, unless they match the last ones written there, whose slice is reused.
fn uniforms(r: &mut Renderer, last: &mut Vec<u8>, slice: Option<Slice>, bytes: &[u8], size: u64) -> Slice {
    if let Some(s) = slice
        && state::bytes_equal(last, bytes)
    {
        return s;
    }
    let s = r.alloc(size, r.c.limits.min_uniform_buffer_offset_alignment);
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), s.ptr, bytes.len()) };
    last.clear();
    last.extend_from_slice(bytes);
    stats::add(&stats::UNIFORM_BYTES, size);
    s
}

/// Anisotropic filtering for 3D draws, from the options menu. 0 or 1 leaves the game's filtering.
static ANISOTROPY: AtomicU32 = AtomicU32::new(0);

pub fn set_anisotropy(level: u32) {
    ANISOTROPY.store(level.min(16), Ordering::Relaxed);
}

/// Size of the 3D world's target in percent of the back buffer, from the next frame on.
static RENDER_SCALE: AtomicU32 = AtomicU32::new(100);
/// Sharpening of the 3D world when its target is resolved, in percent.
static SHARPNESS: AtomicU32 = AtomicU32::new(0);

pub fn set_render_scale(percent: u32) {
    RENDER_SCALE.store(percent.clamp(MIN_RENDER_SCALE, MAX_RENDER_SCALE), Ordering::Relaxed);
}

pub fn set_sharpness(percent: u32) {
    SHARPNESS.store(percent.min(100), Ordering::Relaxed);
}

/// Samples per pixel for the 3D world, 1 for none.
static MSAA: AtomicU32 = AtomicU32::new(1);
static FXAA: AtomicBool = AtomicBool::new(false);

pub fn set_msaa(samples: u32) {
    MSAA.store(samples.clamp(1, 8), Ordering::Relaxed);
}

pub fn set_fxaa(on: bool) {
    FXAA.store(on, Ordering::Relaxed);
}

/// Ambient occlusion strength, 0 (off) to 3.
static AMBIENT_OCCLUSION: AtomicU32 = AtomicU32::new(0);
static SOFT_PARTICLES: AtomicBool = AtomicBool::new(false);

pub fn set_ambient_occlusion(level: u32) {
    AMBIENT_OCCLUSION.store(level.min(3), Ordering::Relaxed);
}

pub fn set_soft_particles(on: bool) {
    SOFT_PARTICLES.store(on, Ordering::Relaxed);
}

/// Radius and intensity of each ambient occlusion level, in world units (a terrain cell is 10).
const AO_LEVELS: [(f32, f32); 4] = [(0.0, 0.0), (8.0, 0.6), (12.0, 1.0), (16.0, 1.5)];
/// World units over which a soft particle fades in front of what's behind it.
const SOFT_FADE: f32 = 8.0;
/// How much larger a smoothed texture is drawn than its source, and the most it can be.
const SMOOTH_SCALE: u32 = 4;
const SMOOTH_MAX: u32 = 2048;

/// The highest sample count the GPU can draw color and depth with, up to `wanted`.
fn supported_samples(c: &context::Context, wanted: u32) -> vk::SampleCountFlags {
    let l = &c.limits;
    let allowed = l.framebuffer_color_sample_counts
        & l.framebuffer_depth_sample_counts
        & l.framebuffer_stencil_sample_counts
        & c.scene_sample_counts;
    [8, 4, 2]
        .into_iter()
        .filter(|&n| n <= wanted)
        .map(vk::SampleCountFlags::from_raw)
        .find(|&s| allowed.contains(s))
        .unwrap_or(vk::SampleCountFlags::TYPE_1)
}

pub const MIN_RENDER_SCALE: u32 = 25;
pub const MAX_RENDER_SCALE: u32 = 200;

/// The 3D world's own target (knowledge-base/scene.md).
struct SceneTarget {
    /// Single sampled: drawn into without multisampling, resolved into with it.
    color: Rc<GpuImage>,
    /// The multisampled color the world is drawn into, with multisampling on.
    ms_color: Option<Rc<GpuImage>>,
    depth: Option<Rc<GpuImage>>,
    /// With multisampling, the depth `depth` is resolved into once the sorted translucent pass starts.
    depth_single: Option<Rc<GpuImage>>,
    /// Whether the world has moved to `color` and `depth_single` this frame.
    single: bool,
    /// The world's depth as a float image, copied when its opaque geometry is done.
    depth_copy: Option<Rc<GpuImage>>,
    /// Ambient occlusion before its blur.
    occlusion: Option<Rc<GpuImage>>,
    /// Whether `depth_copy` holds this frame's depth.
    depth_ready: bool,
    /// Target size over back buffer size.
    scale: f32,
    /// Whether the world is being drawn into it now.
    active: bool,
}

#[repr(C)]
pub struct Device {
    vtbl: *const c_void,
    refs: Cell<u32>,
    pub r: Renderer,
    d3d: *mut c_void,
    system_d3d: *mut c_void,
    adapter: u32,
    device_type: u32,
    focus: HWND,
    behavior: u32,
    params: D3DPRESENT_PARAMETERS,
    swapchain: Swapchain,
    pipelines: Pipelines,
    samplers: FxHashMap<SamplerKey, vk::Sampler>,
    white: Rc<GpuImage>,
    dummy: (vk::Buffer, Option<Allocation>),
    backbuffer: *mut Surface,
    auto_depth: *mut Surface,
    rt: *mut Surface,
    ds: *mut Surface,
    s: State,
    textures: [*mut Texture; 8],
    streams: [(*mut Buffer, u32); 16],
    indices: *mut Buffer,
    base_vertex: u32,
    vs: u32,
    ps: u32,
    vertex_shaders: Vec<Option<VertexShader>>,
    pixel_shaders: Vec<Option<PixelShader>>,
    fvf_layouts: FxHashMap<u32, (u32, VertexInfo)>,
    vertex_inputs: FxHashMap<(u32, [u32; VERTEX_BINDINGS]), Arc<VertexInput>>,
    blocks: Vec<Option<(State, u32, u32)>>,
    recording_block: bool,
    gamma: [u16; 768],
    palette: u32,
    clip_status: [u32; 2],
    vs_dirty: bool,
    /// World or view changed since `object` was computed.
    object_dirty: bool,
    object: [Matrix; 2],
    /// `object[0]` times the projection, cleared with either.
    clip: Option<Matrix>,
    light_cache: state::LightCache,
    ps_dirty: bool,
    set_dirty: bool,
    /// Anisotropy the bound samplers were made with: the setting for 3D draws, 0 for the UI.
    anisotropy: u32,
    vs_slice: Option<Slice>,
    ps_slice: Option<Slice>,
    set: vk::DescriptorSet,
    set_buffers: (vk::Buffer, vk::Buffer),
    /// Texture stages the current set holds.
    set_stages: usize,
    /// Each stage's last sampler, to skip the lookup when its state didn't change.
    last_samplers: [(SamplerKey, vk::Sampler); 8],
    /// Descriptor sets written into each frame's pools, kept across frames.
    sets: [FxHashMap<SetKey, vk::DescriptorSet>; FRAMES],
    /// `context::images_destroyed` when each frame's sets were last checked.
    sets_checked: [u64; FRAMES],
    /// The last vertex and pixel uniforms written this frame.
    last_vs: Vec<u8>,
    last_ps: Vec<u8>,
    epoch: u64,
    bound: Bound,
    binds: u64,
    last_info: Option<(state::VertexInfo, bool)>,
    fullscreen: bool,
    warned: HashSet<String>,
    draws: u64,
    scene: Option<SceneTarget>,
    /// Whether the game was drawing the 3D world at the last draw, clear or present.
    scene_open: bool,
    resolve: Pass,
    /// Depth and stencil of the last clear of the automatic depth buffer, D3D convention.
    depth_clear: (f32, u32),
    /// Color of the last clear of the back buffer.
    color_clear: u32,
    /// The kind of world target pipelines were last built ahead for.
    prebuilt_target: (vk::SampleCountFlags, vk::Format),
    depth_copy: Pass,
    ao: Pass,
    ao_apply: Pass,
    /// Draws textures marked `smooth` larger (the fog of war).
    smooth_upscale: Pass,
    /// `scene::in_scene`, `opaque_done` and `in_sorted` as the game thread saw them when it queued
    /// the call being run.
    scene_flags: (bool, u32, bool),
    /// What the game thread answers Get* calls from. Only touched there.
    front: *mut Front,
    /// `scene_flags.1` as of the last check.
    opaque_seen: u32,
    sorted_seen: bool,
    /// Soft particle setup in the pixel uniforms, zero when off.
    soft: [f32; 4],
}

const FPU_PRESERVE: u32 = 0x2;

/// A viewport in the game's pixels, moved to a target `scale` times the size. Edges are rounded
/// so neighboring viewports still meet.
fn scaled_viewport(vp: &Viewport, scale: f32) -> Viewport {
    if scale == 1.0 {
        return *vp;
    }
    let edge = |v: u32| (v as f32 * scale).round() as u32;
    let (x, y) = (edge(vp.x), edge(vp.y));
    Viewport { x, y, width: edge(vp.x + vp.width) - x, height: edge(vp.y + vp.height) - y, ..*vp }
}

/// For a perspective projection, `a` and `b` in reversed depth = a + b / distance, where the
/// distance is clip w. WW3D's camera is right-handed (it looks down -z, `_34` is -1), D3D's
/// usual one left-handed (`_34` is 1).
fn reversed_depth_terms(proj: &Matrix) -> Option<(f32, f32)> {
    let m = &proj.0;
    let s = m[11];
    if (s.abs() - 1.0).abs() > 1e-4 || m[15].abs() > 1e-4 {
        return None;
    }
    let r = proj.reversed_depth();
    Some((r.0[10] / s, r.0[14]))
}

/// A D3DCMPFUNC for reversed depth: less and greater trade places.
fn reversed_compare(f: u32) -> u32 {
    match f {
        2 => 5,
        4 => 7,
        5 => 2,
        7 => 4,
        other => other,
    }
}

/// D3D sets the x87 unit to single precision, round to nearest, all exceptions masked, unless
/// the game asks it not to. The game's math depends on it, replays and multiplayer included.
fn setup_fpu() {
    #[cfg(target_arch = "x86")]
    unsafe {
        let mut cw: u16 = 0;
        std::arch::asm!("fnstcw [{0}]", in(reg) &mut cw, options(nostack));
        cw = (cw & !0x0F3F) | 0x003F;
        std::arch::asm!("fldcw [{0}]", in(reg) &cw, options(nostack));
    }
}

fn get_adapter_mode(system_d3d: *mut c_void, adapter: u32) -> D3DDISPLAYMODE {
    let f: unsafe extern "system" fn(*mut c_void, u32, *mut D3DDISPLAYMODE) -> HRESULT =
        unsafe { std::mem::transmute(crate::direct3d::method(system_d3d, crate::direct3d::SLOT_GET_ADAPTER_DISPLAY_MODE)) };
    let mut mode = D3DDISPLAYMODE::default();
    unsafe { f(system_d3d, adapter, &mut mode) };
    mode
}

fn adapter_ids(system_d3d: *mut c_void, adapter: u32) -> (u32, u32) {
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, *mut crate::d3d::D3DADAPTER_IDENTIFIER8) -> HRESULT =
        unsafe { std::mem::transmute(crate::direct3d::method(system_d3d, crate::direct3d::SLOT_GET_ADAPTER_IDENTIFIER)) };
    let mut id: crate::d3d::D3DADAPTER_IDENTIFIER8 = unsafe { std::mem::zeroed() };
    if unsafe { f(system_d3d, adapter, 0, &mut id) } == D3D_OK { (id.VendorId, id.DeviceId) } else { (0, 0) }
}

/// Creates our device in place of the system one. `d3d` is our IDirect3D8, `system_d3d` the real one.
#[allow(clippy::too_many_arguments)]
pub unsafe fn create(
    d3d: *mut c_void,
    system_d3d: *mut c_void,
    adapter: u32,
    device_type: u32,
    focus: HWND,
    behavior: u32,
    pp: *mut D3DPRESENT_PARAMETERS,
    out: *mut *mut c_void,
) -> HRESULT {
    if pp.is_null() || out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let requested = unsafe { *pp };
    log!("CreateDevice (vulkan) adapter {adapter} type {device_type} behavior {behavior:#x} requested {requested:?}");
    let (vendor, device_id) = adapter_ids(system_d3d, adapter);
    let c = match context::init(vendor, device_id) {
        Ok(c) => c,
        Err(e) => {
            log!("vulkan could not start: {e}");
            direct3d::show_error(&format!("cnc-vulkan could not start Vulkan:\n\n{e}"));
            return com::D3DERR_NOTAVAILABLE;
        }
    };
    if behavior & FPU_PRESERVE == 0 {
        setup_fpu();
    }
    let hwnd = if requested.hDeviceWindow.is_null() { focus } else { requested.hDeviceWindow };
    let fullscreen = unsafe { arrange_window(system_d3d, adapter, focus, &requested) };
    let vsync = config::get().map(|c| c.display.vsync).unwrap_or(true);
    let graphics = config::get().map_or(config::Graphics::default(), |c| c.graphics.clone());
    set_anisotropy(graphics.anisotropy);
    set_render_scale(graphics.render_scale);
    set_sharpness(graphics.sharpening);
    set_msaa(graphics.msaa);
    set_fxaa(graphics.fxaa);
    set_ambient_occlusion(graphics.ambient_occlusion);
    set_soft_particles(graphics.soft_particles);
    super::swapchain::set_shadow_brightness(graphics.shadow_brightness);
    stats::set_detail(config::get().is_ok_and(|c| c.debug.timers));
    let swapchain = match Swapchain::new(hwnd, vsync) {
        Ok(s) => s,
        Err(e) => {
            log!("swapchain could not be created: {e}");
            direct3d::show_error(&format!("cnc-vulkan could not create the Vulkan swapchain:\n\n{e}"));
            return com::D3DERR_NOTAVAILABLE;
        }
    };
    let ff_vs = match shader::compile(&ff::vertex(), Stage::Vertex) {
        Ok(s) => s,
        Err(e) => {
            log!("FATAL: fixed function vertex shader: {e}");
            return com::D3DERR_NOTAVAILABLE;
        }
    };
    let ff_ps = match shader::compile(&ff::fragment(), Stage::Fragment) {
        Ok(s) => s,
        Err(e) => {
            log!("FATAL: fixed function pixel shader: {e}");
            return com::D3DERR_NOTAVAILABLE;
        }
    };
    let mut r = Renderer::new(c);
    let white = GpuImage::new(&mut r, vk::Format::B8G8R8A8_UNORM, 1, 1, 1, ImageUse::Texture, false);
    r.write_image(true, &white, 0, 0, 0, 1, 1, &[255, 255, 255, 255]);
    let (dummy_buffer, dummy_alloc, dummy_ptr) = c.host_buffer("dummy vertex", 64, vk::BufferUsageFlags::VERTEX_BUFFER);
    unsafe { std::ptr::copy_nonoverlapping(pipeline::DUMMY_DATA.as_ptr(), dummy_ptr, pipeline::DUMMY_DATA.len()) };

    let mut pipelines = Pipelines::new(&ff_vs, &ff_ps, r.pipeline_layout);
    pipelines.prebuild(&[(MODULE_FF_VS, MODULE_FF_PS)]);
    let mut resolve = match Pass::new(c, "scene resolve", &format!("{}{}", post::COMMON, post::RESOLVE), None) {
        Ok(p) => p,
        Err(e) => {
            log!("FATAL: {e}");
            return com::D3DERR_NOTAVAILABLE;
        }
    };
    resolve.set_format(c, vk::Format::B8G8R8A8_UNORM);
    let pass = |name: &'static str, source: &str, blend: Option<(vk::BlendFactor, vk::BlendFactor)>, format: vk::Format| {
        Pass::new(c, name, source, blend).map(|mut p| {
            p.set_format(c, format);
            p
        })
    };
    let multiply = Some((vk::BlendFactor::ZERO, vk::BlendFactor::SRC_COLOR));
    let passes = (|| {
        Ok::<_, String>((
            pass("depth copy", post::DEPTH_COPY, None, vk::Format::R32_SFLOAT)?,
            pass("ambient occlusion", &post::ambient_occlusion(), None, vk::Format::R8_UNORM)?,
            pass("ambient occlusion apply", &post::ambient_occlusion_apply(), multiply, c.scene_format)?,
            pass("smooth upscale", &format!("{}{}", post::COMMON, post::SMOOTH_UPSCALE), None, vk::Format::B8G8R8A8_UNORM)?,
        ))
    })();
    let (depth_copy, ao, ao_apply, smooth_upscale) = match passes {
        Ok(p) => p,
        Err(e) => {
            log!("FATAL: {e}");
            return com::D3DERR_NOTAVAILABLE;
        }
    };

    let device = Box::into_raw(Box::new(Device {
        vtbl: &DEVICE_VTBL as *const _ as *const c_void,
        refs: Cell::new(1),
        r,
        d3d,
        system_d3d,
        adapter,
        device_type,
        focus,
        behavior,
        params: requested,
        swapchain,
        pipelines,
        samplers: FxHashMap::default(),
        white,
        dummy: (dummy_buffer, Some(dummy_alloc)),
        backbuffer: std::ptr::null_mut(),
        auto_depth: std::ptr::null_mut(),
        rt: std::ptr::null_mut(),
        ds: std::ptr::null_mut(),
        s: State::new(requested.EnableAutoDepthStencil != 0),
        textures: [std::ptr::null_mut(); 8],
        streams: [(std::ptr::null_mut(), 0); 16],
        indices: std::ptr::null_mut(),
        base_vertex: 0,
        vs: 0,
        ps: 0,
        vertex_shaders: Vec::new(),
        pixel_shaders: Vec::new(),
        fvf_layouts: FxHashMap::default(),
        vertex_inputs: FxHashMap::default(),
        blocks: Vec::new(),
        recording_block: false,
        gamma: std::array::from_fn(|i| ((i % 256) * 257) as u16),
        palette: 0,
        clip_status: [0; 2],
        vs_dirty: true,
        object_dirty: true,
        object: [Matrix::IDENTITY; 2],
        clip: None,
        light_cache: state::LightCache::default(),
        ps_dirty: true,
        set_dirty: true,
        anisotropy: 0,
        vs_slice: None,
        ps_slice: None,
        set: vk::DescriptorSet::null(),
        set_buffers: (vk::Buffer::null(), vk::Buffer::null()),
        set_stages: 0,
        last_samplers: [(SamplerKey([0; 11]), vk::Sampler::null()); 8],
        sets: Default::default(),
        sets_checked: [0; FRAMES],
        last_vs: Vec::new(),
        last_ps: Vec::new(),
        epoch: 0,
        bound: Bound::default(),
        binds: 0,
        last_info: None,
        fullscreen,
        warned: HashSet::new(),
        draws: 0,
        scene: None,
        scene_open: false,
        resolve,
        depth_clear: (1.0, 0),
        color_clear: 0,
        prebuilt_target: (vk::SampleCountFlags::TYPE_1, vk::Format::B8G8R8A8_UNORM),
        depth_copy,
        ao,
        ao_apply,
        smooth_upscale,
        scene_flags: (false, crate::scene::opaque_done(), false),
        front: std::ptr::null_mut(),
        opaque_seen: crate::scene::opaque_done(),
        sorted_seen: false,
        soft: [0.0; 4],
    }));
    unsafe {
        (*device).create_default_targets(&requested);
        (*device).front = Box::into_raw(Box::new(Front::of(&*device)));
        com::add_ref(d3d);
        *out = device as *mut c_void;
    }
    CURRENT.store(device, Ordering::Release);
    log!("vulkan device ready, back buffer {}x{} {}", requested.BackBufferWidth, requested.BackBufferHeight, crate::d3d::format_name(requested.BackBufferFormat));
    D3D_OK
}

/// The device the game draws with, for our patches that set state around the game's own calls.
static CURRENT: AtomicPtr<Device> = AtomicPtr::new(std::ptr::null_mut());

/// Runs `f` with fixed function draws clipped to world space `planes`, then restores the game's. Game thread.
pub fn with_clip_planes(planes: &[[f32; 4]], f: impl FnOnce()) {
    let this = CURRENT.load(Ordering::Acquire);
    if this.is_null() || planes.len() > shader::CLIP_PLANES {
        return f();
    }
    on_worker::with_clip_planes(this, planes, f);
}

/// What the game thread answers Get* calls from, kept up by the calls it queues for the renderer
/// thread (vk/worker.rs). Only touched on the game thread.
pub struct Front {
    s: State,
    vs: u32,
    ps: u32,
    textures: [*mut c_void; 8],
    streams: [(*mut c_void, u32); 16],
    indices: (*mut c_void, u32),
    rt: *mut Surface,
    ds: *mut Surface,
    clip_status: [u32; 2],
    palette: u32,
    /// Live vertex shaders with the constants their declarations set, and live pixel shaders.
    vertex_shaders: FxHashMap<u32, Vec<(u32, [f32; 4])>>,
    pixel_shaders: HashSet<u32>,
    /// The scene flags last sent to the renderer thread.
    scene: Option<(bool, u32, bool)>,
}

impl Front {
    /// The device's state as it is now. Runs on the renderer thread, or at creation.
    fn of(d: &Device) -> Front {
        let vertex_shaders = d
            .vertex_shaders
            .iter()
            .enumerate()
            .filter_map(|(i, v)| v.as_ref().map(|v| ((((i + 1) as u32) << 1) | 1, v.decl.constants.clone())))
            .collect();
        let pixel_shaders = d.pixel_shaders.iter().enumerate().filter(|(_, p)| p.is_some()).map(|(i, _)| i as u32 + 1).collect();
        Front {
            s: d.s.clone(),
            vs: d.vs,
            ps: d.ps,
            textures: d.textures.map(|t| t as *mut c_void),
            streams: d.streams.map(|(b, stride)| (b as *mut c_void, stride)),
            indices: (d.indices as *mut c_void, d.base_vertex),
            rt: d.rt,
            ds: d.ds,
            clip_status: d.clip_status,
            palette: d.palette,
            vertex_shaders,
            pixel_shaders,
            scene: None,
        }
    }
}

/// Lays out the game window for the configured display mode. Returns whether the display mode
/// was changed for exclusive fullscreen.
unsafe fn arrange_window(system_d3d: *mut c_void, adapter: u32, focus: HWND, requested: &D3DPRESENT_PARAMETERS) -> bool {
    let mut params = *requested;
    if let Some(plan) = unsafe { direct3d::prepare(system_d3d, adapter, focus, &mut params) } {
        window::apply(plan);
        display::leave();
        return false;
    }
    let hwnd = if requested.hDeviceWindow.is_null() { focus } else { requested.hDeviceWindow };
    let fullscreen = config::get().is_ok_and(|c| c.display.mode == config::DisplayMode::Fullscreen);
    if fullscreen && requested.Windowed == 0 {
        let monitor = config::get().map(|c| c.display.monitor).unwrap_or(0);
        display::enter(monitor, params.BackBufferWidth, params.BackBufferHeight, params.FullScreen_RefreshRateInHz);
        window::apply_fullscreen(hwnd, params.BackBufferWidth as i32, params.BackBufferHeight as i32, display::on_activate);
        return true;
    }
    false
}

impl Device {
    fn warn_once(&mut self, what: impl Into<String>) {
        let what = what.into();
        if self.warned.insert(what.clone()) {
            log!("vulkan: {what}");
        }
    }

    fn create_default_targets(&mut self, pp: &D3DPRESENT_PARAMETERS) {
        let this = self as *mut Device;
        let format = match pp.BackBufferFormat {
            0 => format::X8R8G8B8,
            f if format::renderable(f) => f,
            f => {
                self.warn_once(format!("back buffer format {} drawn as X8R8G8B8", crate::d3d::format_name(f)));
                format::X8R8G8B8
            }
        };
        let info = format::info(format).unwrap();
        let (w, h) = (pp.BackBufferWidth.max(1), pp.BackBufferHeight.max(1));
        let image = GpuImage::new(&mut self.r, vk::Format::B8G8R8A8_UNORM, w, h, 1, ImageUse::Target, !info.has_alpha);
        let back = SurfaceData::new(info, w, h, texture::USAGE_RENDERTARGET, texture::POOL_DEFAULT, Backing::Image(image.into()));
        self.backbuffer = texture::create_surface(this, std::ptr::null_mut(), back);
        self.auto_depth = std::ptr::null_mut();
        if pp.EnableAutoDepthStencil != 0 {
            let info = texture::depth_info(pp.AutoDepthStencilFormat);
            let image = GpuImage::new(&mut self.r, info.vk, w, h, 1, ImageUse::Depth, false);
            let depth = SurfaceData::new(info, w, h, texture::USAGE_DEPTHSTENCIL, texture::POOL_DEFAULT, Backing::Image(image.into()));
            self.auto_depth = texture::create_surface(this, std::ptr::null_mut(), depth);
        }
        unsafe {
            texture::surf_add_ref(self.backbuffer);
            self.rt = self.backbuffer;
            if !self.auto_depth.is_null() {
                texture::surf_add_ref(self.auto_depth);
            }
            self.ds = self.auto_depth;
        }
        self.s.viewport = Viewport { x: 0, y: 0, width: w, height: h, min_z: 0.0, max_z: 1.0 };
    }

    fn mark_all_dirty(&mut self) {
        self.vs_dirty = true;
        self.object_dirty = true;
        self.ps_dirty = true;
        self.set_dirty = true;
    }

    /// Follows the game into and out of drawing the 3D world. Runs before every draw, clear and
    /// present, so the world's target is filled and resolved in the game's order.
    fn sync_scene(&mut self) {
        let (drawing, opaque, sorted) = self.scene_flags;
        if opaque == self.opaque_seen && drawing == self.scene_open && sorted == self.sorted_seen {
            return;
        }
        let _timer = stats::Timer::new(&stats::SCENE_TICKS);
        if sorted != self.sorted_seen {
            self.sorted_seen = sorted;
            if sorted && self.scene_open {
                self.r.mark(Section::Resolve);
                self.single_sample();
            }
            self.r.mark(if sorted { Section::Sorted } else { Section::Translucent });
        }
        if opaque != self.opaque_seen {
            self.opaque_seen = opaque;
            if self.scene_open {
                self.r.mark(Section::DepthAndOcclusion);
                self.after_opaque();
                self.r.mark(Section::Translucent);
            }
        }
        if drawing == self.scene_open {
            return;
        }
        self.scene_open = drawing;
        if drawing {
            self.r.mark(Section::Clear);
            self.open_scene();
            self.r.mark(Section::Opaque);
        } else {
            self.r.mark(Section::Resolve);
            self.close_scene();
            self.r.mark(Section::Interface);
        }
    }

    fn open_scene(&mut self) {
        let c = self.r.c;
        let scale = RENDER_SCALE.load(Ordering::Relaxed) as f32 / 100.0;
        let samples = supported_samples(c, MSAA.load(Ordering::Relaxed));
        let multisampled = samples != vk::SampleCountFlags::TYPE_1;
        let needs_depth = AMBIENT_OCCLUSION.load(Ordering::Relaxed) > 0 || SOFT_PARTICLES.load(Ordering::Relaxed);
        if scale == 1.0 && SHARPNESS.load(Ordering::Relaxed) == 0 && !multisampled && !FXAA.load(Ordering::Relaxed) && !needs_depth {
            self.scene = None;
            return;
        }
        let Some(back) = Self::surface(self.backbuffer) else { return };
        let Some((back_image, _)) = back.gpu() else { return };
        let opaque = !back.info.has_alpha;
        let (w, h) = (((back_image.width as f32 * scale).round() as u32).max(1), ((back_image.height as f32 * scale).round() as u32).max(1));
        let depth_vk = Self::surface(self.auto_depth).map(|d| d.info.vk);
        let reuse = self.scene.as_ref().is_some_and(|s| {
            (s.color.width, s.color.height) == (w, h)
                && s.depth.is_some() == depth_vk.is_some()
                && s.ms_color.as_ref().map_or(vk::SampleCountFlags::TYPE_1, |m| m.samples) == samples
        });
        let format = c.scene_format;
        if !reuse {
            let color = GpuImage::new(&mut self.r, format, w, h, 1, ImageUse::Target, opaque);
            let ms_color = multisampled.then(|| GpuImage::multisampled(&mut self.r, format, w, h, 1, ImageUse::Target, opaque, samples));
            let depth_use = if multisampled { ImageUse::Depth } else { ImageUse::SampledDepth };
            let depth = depth_vk.map(|f| GpuImage::multisampled(&mut self.r, f, w, h, 1, depth_use, false, samples));
            let depth_single = depth_vk.filter(|_| multisampled).map(|f| GpuImage::new(&mut self.r, f, w, h, 1, ImageUse::SampledDepth, false));
            let depth_copy = depth.is_some().then(|| GpuImage::new(&mut self.r, vk::Format::R32_SFLOAT, w, h, 1, ImageUse::Target, false));
            let occlusion = depth.is_some().then(|| GpuImage::new(&mut self.r, vk::Format::R8_UNORM, w, h, 1, ImageUse::Target, false));
            log!("3D world target {w}x{h} ({:.0}% of the back buffer), {format:?}, {} samples", scale * 100.0, samples.as_raw());
            self.scene = Some(SceneTarget {
                color,
                ms_color,
                depth,
                depth_single,
                single: false,
                depth_copy,
                occlusion,
                depth_ready: false,
                scale,
                active: false,
            });
        }
        if self.prebuilt_target != (samples, format) {
            self.pipelines.prebuild_target(samples, format);
            if multisampled {
                self.pipelines.prebuild_target(vk::SampleCountFlags::TYPE_1, format);
            }
            self.prebuilt_target = (samples, format);
        }
        let Some(scene) = self.scene.as_mut() else { return };
        scene.scale = w as f32 / back_image.width as f32;
        scene.active = true;
        scene.single = false;
        scene.depth_ready = false;
        let k = self.color_clear;
        let clear_color = [((k >> 16) & 0xFF) as f32 / 255.0, ((k >> 8) & 0xFF) as f32 / 255.0, (k & 0xFF) as f32 / 255.0, (k >> 24) as f32 / 255.0];
        let (z, stencil) = self.depth_clear;
        if let Some(ms) = scene.ms_color.clone() {
            // Nothing can be copied into a multisampled image. The back buffer only holds the
            // frame's clear at this point, so that clear is repeated, by a render pass so the
            // images stay in their attachment layouts.
            let depth = scene.depth.as_ref().map(|d| (d.attachment_views[0], d.layout));
            let (view, layout) = (ms.attachment_views[0], ms.layout);
            let area = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: ms.width, height: ms.height } };
            self.r.transfer();
            self.r.record(move |c, cmd| {
                let color_attachment = [vk::RenderingAttachmentInfo::default()
                    .image_view(view)
                    .image_layout(layout)
                    .load_op(vk::AttachmentLoadOp::CLEAR)
                    .store_op(vk::AttachmentStoreOp::STORE)
                    .clear_value(vk::ClearValue { color: vk::ClearColorValue { float32: clear_color } })];
                let depth_attachment = depth.map(|(view, layout)| {
                    vk::RenderingAttachmentInfo::default()
                        .image_view(view)
                        .image_layout(layout)
                        .load_op(vk::AttachmentLoadOp::CLEAR)
                        .store_op(vk::AttachmentStoreOp::STORE)
                        .clear_value(vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0 - z, stencil } })
                });
                let mut info = vk::RenderingInfo::default().render_area(area).layer_count(1).color_attachments(&color_attachment);
                if let Some(d) = depth_attachment.as_ref() {
                    info = info.depth_attachment(d).stencil_attachment(d);
                }
                unsafe {
                    c.device.cmd_begin_rendering(cmd, &info);
                    c.device.cmd_end_rendering(cmd);
                }
            });
            return;
        }
        let (color, depth) = (scene.color.image, scene.depth.as_ref().map(|d| d.image));
        self.r.transfer();
        let layers = vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 };
        let blit = vk::ImageBlit {
            src_subresource: layers,
            src_offsets: [vk::Offset3D::default(), vk::Offset3D { x: back_image.width as i32, y: back_image.height as i32, z: 1 }],
            dst_subresource: layers,
            dst_offsets: [vk::Offset3D::default(), vk::Offset3D { x: w as i32, y: h as i32, z: 1 }],
        };
        let filter = if (w, h) == (back_image.width, back_image.height) { vk::Filter::NEAREST } else { vk::Filter::LINEAR };
        let back = back_image.image;
        self.r.record(move |c, cmd| unsafe {
            c.device.cmd_blit_image(cmd, back, vk::ImageLayout::GENERAL, color, vk::ImageLayout::GENERAL, &[blit], filter);
            if let Some(depth) = depth {
                let value = vk::ClearDepthStencilValue { depth: 1.0 - z, stencil };
                let range = vk::ImageSubresourceRange {
                    aspect_mask: vk::ImageAspectFlags::DEPTH | vk::ImageAspectFlags::STENCIL,
                    base_mip_level: 0,
                    level_count: 1,
                    base_array_layer: 0,
                    layer_count: 1,
                };
                c.device.cmd_clear_depth_stencil_image(cmd, depth, vk::ImageLayout::GENERAL, &value, &[range]);
            }
        });
    }

    /// Runs when the world's opaque geometry is done, before shadows, water, translucent objects
    /// and particles: copies the world's depth for soft particles and the occlusion pass, and
    /// darkens the world by its ambient occlusion.
    fn after_opaque(&mut self) {
        if self.rt != self.backbuffer {
            return;
        }
        let Some(scene) = self.scene.as_mut().filter(|s| s.active) else { return };
        let (Some(depth), Some(copy), Some(occlusion)) = (scene.depth.clone(), scene.depth_copy.clone(), scene.occlusion.clone()) else {
            return;
        };
        scene.depth_ready = true;
        let color = scene.ms_color.clone().filter(|_| !scene.single).unwrap_or_else(|| scene.color.clone());
        // A multisampled depth is resolved (sample 0) into the single sampled one first, so it's
        // never sampled and stays in its attachment layout.
        let single = scene.single;
        let source = match scene.depth_single.clone() {
            Some(depth_single) => {
                if !single {
                    self.resolve_pass(None, Some((&depth, &depth_single)), true);
                }
                depth_single
            }
            None => depth,
        };
        let c = self.r.c;
        let extent = vk::Extent2D { width: source.width, height: source.height };
        self.r.transfer();
        self.depth_copy.draw(&mut self.r, &[source.view], copy.attachment_views[0], vk::ImageLayout::GENERAL, extent, &[0u8; 16]);
        let (radius, intensity) = AO_LEVELS[AMBIENT_OCCLUSION.load(Ordering::Relaxed) as usize];
        if radius == 0.0 {
            return;
        }
        let Some((a, b)) = reversed_depth_terms(&self.s.proj) else {
            self.warn_once("ambient occlusion skipped, the world's projection isn't a perspective one");
            return;
        };
        let m = &self.s.proj.0;
        let uniform = post::ao_uniform(extent.width, extent.height, a, b, m[0], m[5], radius, intensity);
        self.r.transfer();
        self.ao.draw(&mut self.r, &[copy.view], occlusion.attachment_views[0], vk::ImageLayout::GENERAL, extent, &uniform);
        self.r.mark(Section::OcclusionApply);
        self.r.transfer();
        self.ao_apply.set_target(c, color.format, color.samples);
        self.ao_apply.draw(&mut self.r, &[occlusion.view, copy.view], color.attachment_views[0], color.layout, extent, &uniform);
    }

    /// Redraws a texture's smoothed copy, `SMOOTH_SCALE` times its size up to `SMOOTH_MAX`, from
    /// the image draws would otherwise use.
    fn smooth(&mut self, data: &texture::TextureData) {
        let (source, sw, sh) = match data.replacement.borrow().as_ref() {
            Some(r) => (r.gpu.clone(), r.width, r.height),
            None => (data.gpu.clone(), data.width, data.height),
        };
        let (w, h) = ((sw * SMOOTH_SCALE).min(SMOOTH_MAX), (sh * SMOOTH_SCALE).min(SMOOTH_MAX));
        let reuse = data.smoothed.borrow().as_ref().is_some_and(|s| (s.width, s.height) == (w, h));
        if !reuse {
            let image = GpuImage::new(&mut self.r, vk::Format::B8G8R8A8_UNORM, w, h, 1, ImageUse::Target, false);
            *data.smoothed.borrow_mut() = Some(image);
        }
        let Some(target) = data.smoothed.borrow().clone() else { return };
        let uniform = post::smooth_uniform(sw, sh);
        self.r.transfer();
        let extent = vk::Extent2D { width: w, height: h };
        self.smooth_upscale.draw(&mut self.r, &[source.view], target.attachment_views[0], vk::ImageLayout::GENERAL, extent, &uniform);
    }

    /// Soft particle setup for a draw: zero unless it's a fixed function particle draw into the
    /// world's target after its depth was copied.
    fn soft_setup(&self, fixed_ps: bool, rhw: bool, blend: &Blend) -> [f32; 4] {
        let redirected = self.rt == self.backbuffer && self.scene.as_ref().is_some_and(|s| s.active && s.depth_ready);
        let effect = blend.enable && self.s.rs[state::RS_ZWRITEENABLE] == 0;
        if !SOFT_PARTICLES.load(Ordering::Relaxed) || !self.scene_flags.2 || !effect || !fixed_ps || rhw || !redirected {
            return [0.0; 4];
        }
        let Some((a, b)) = reversed_depth_terms(&self.s.proj) else { return [0.0; 4] };
        let additive = blend.src == vk::BlendFactor::ONE && blend.dst == vk::BlendFactor::ONE;
        [a, b, 1.0 / SOFT_FADE, if additive { 2.0 } else { 1.0 }]
    }

    /// Resolves the world's multisampled color and depth into single sampled images and draws the
    /// rest of the world into those, so particles and other sorted translucent polygons blend once
    /// per pixel instead of once per sample. Shadows and everything opaque keep multisampling.
    fn single_sample(&mut self) {
        if self.rt != self.backbuffer {
            return;
        }
        let Some(scene) = self.scene.as_mut().filter(|s| s.active && !s.single) else { return };
        let Some(ms) = scene.ms_color.clone() else { return };
        scene.single = true;
        let color = scene.color.clone();
        let depth = scene.depth.clone().zip(scene.depth_single.clone());
        self.resolve_pass(Some((&ms, &color)), depth.as_ref().map(|(d, s)| (&**d, &**s)), false);
    }

    /// Resolves multisampled images into single sampled ones with an empty render pass, so the
    /// multisampled ones never leave their attachment layouts: color averaged, depth and stencil
    /// from sample 0. `keep` stores the multisampled contents for more drawing.
    fn resolve_pass(&mut self, color: Option<(&GpuImage, &GpuImage)>, depth: Option<(&GpuImage, &GpuImage)>, keep: bool) {
        let Some((w, h)) = color.or(depth).map(|(from, _)| (from.width, from.height)) else { return };
        let area = vk::Rect2D { offset: vk::Offset2D { x: 0, y: 0 }, extent: vk::Extent2D { width: w, height: h } };
        let views = |(from, to): (&GpuImage, &GpuImage)| (from.attachment_views[0], from.layout, to.attachment_views[0], to.layout);
        let (color, depth) = (color.map(views), depth.map(views));
        let store = if keep { vk::AttachmentStoreOp::STORE } else { vk::AttachmentStoreOp::DONT_CARE };
        self.r.transfer();
        self.r.record(move |c, cmd| {
            let attachment = |(view, layout, resolve_view, resolve_layout), mode| {
                vk::RenderingAttachmentInfo::default()
                    .image_view(view)
                    .image_layout(layout)
                    .load_op(vk::AttachmentLoadOp::LOAD)
                    .store_op(store)
                    .resolve_mode(mode)
                    .resolve_image_view(resolve_view)
                    .resolve_image_layout(resolve_layout)
            };
            let color_attachments: Vec<_> = color.into_iter().map(|v| attachment(v, vk::ResolveModeFlags::AVERAGE)).collect();
            let depth_attachment = depth.map(|v| attachment(v, vk::ResolveModeFlags::SAMPLE_ZERO));
            let mut info = vk::RenderingInfo::default().render_area(area).layer_count(1).color_attachments(&color_attachments);
            if let Some(d) = depth_attachment.as_ref() {
                info = info.depth_attachment(d).stencil_attachment(d);
            }
            unsafe {
                c.device.cmd_begin_rendering(cmd, &info);
                c.device.cmd_end_rendering(cmd);
            }
        });
    }

    /// Draws the 3D world's target into the back buffer, scaled and sharpened.
    fn close_scene(&mut self) {
        let Some(scene) = self.scene.as_mut().filter(|s| s.active) else { return };
        scene.active = false;
        let (color, ms_color) = (scene.color.clone(), scene.ms_color.clone().filter(|_| !scene.single));
        let Some((back, _)) = Self::surface(self.backbuffer).and_then(|b| b.gpu()) else { return };
        if let Some(ms) = ms_color {
            self.resolve_pass(Some((&ms, &color)), None, false);
        }
        let sharpness = SHARPNESS.load(Ordering::Relaxed) as f32 / 100.0;
        let dither = color.format != vk::Format::B8G8R8A8_UNORM;
        let uniform = post::resolve_uniform(color.width, color.height, sharpness, FXAA.load(Ordering::Relaxed), dither);
        self.r.transfer();
        let extent = vk::Extent2D { width: back.width, height: back.height };
        self.resolve.draw(&mut self.r, &[color.view], back.attachment_views[0], vk::ImageLayout::GENERAL, extent, &uniform);
    }

    /// Where draws and clears go: the color target and level, the depth target, and the scale from
    /// the game's pixel coordinates. The back buffer is swapped for the 3D world's target while
    /// the world is drawn.
    fn targets(&self) -> Option<(Rc<GpuImage>, u32, Option<Rc<GpuImage>>, f32)> {
        let rt = Self::surface(self.rt)?;
        let (color, level) = rt.gpu()?;
        let depth = Self::surface(self.ds).and_then(|d| d.gpu()).map(|(image, _)| image);
        if self.rt == self.backbuffer
            && (self.ds.is_null() || self.ds == self.auto_depth)
            && let Some(scene) = self.scene.as_ref().filter(|s| s.active)
        {
            let depth = match (self.ds.is_null(), scene.single) {
                (true, _) => None,
                (false, true) => scene.depth_single.clone(),
                (false, false) => scene.depth.clone(),
            };
            let color = scene.ms_color.clone().filter(|_| !scene.single).unwrap_or_else(|| scene.color.clone());
            return Some((color, 0, depth, scene.scale));
        }
        // D3D8 lets a depth buffer be smaller than the target and clips to it, Vulkan doesn't.
        let (w, h) = color.level_extent(level);
        let depth = depth.filter(|d| d.width >= w && d.height >= h);
        Some((color, level, depth, 1.0))
    }

    /// Copies an image into another of the same size, upside down.
    pub fn copy_flipped(&mut self, from: &GpuImage, to: &GpuImage) {
        self.r.transfer();
        let layers = vk::ImageSubresourceLayers { aspect_mask: vk::ImageAspectFlags::COLOR, mip_level: 0, base_array_layer: 0, layer_count: 1 };
        let (w, h) = (from.width.min(to.width) as i32, from.height.min(to.height) as i32);
        let blit = vk::ImageBlit {
            src_subresource: layers,
            src_offsets: [vk::Offset3D::default(), vk::Offset3D { x: w, y: h, z: 1 }],
            dst_subresource: layers,
            dst_offsets: [vk::Offset3D { x: 0, y: h, z: 0 }, vk::Offset3D { x: w, y: 0, z: 1 }],
        };
        let (source, target) = (from.image, to.image);
        self.r.record(move |c, cmd| unsafe {
            c.device.cmd_blit_image(cmd, source, vk::ImageLayout::GENERAL, target, vk::ImageLayout::GENERAL, &[blit], vk::Filter::NEAREST)
        });
    }

    fn texture_data(&self, stage: usize) -> Option<&Rc<texture::TextureData>> {
        let t = self.textures[stage];
        if t.is_null() { None } else { Some(unsafe { &(*t).data }) }
    }

    fn surface<'a>(p: *mut Surface) -> Option<&'a SurfaceData> {
        if p.is_null() { None } else { Some(unsafe { &(*p).data }) }
    }

    fn sampler(&mut self, stage: usize, lod: u32) -> vk::Sampler {
        let t = &self.s.tss[stage];
        let key = SamplerKey([
            t[state::TSS_MAGFILTER],
            t[state::TSS_MINFILTER],
            t[state::TSS_MIPFILTER],
            t[state::TSS_ADDRESSU],
            t[state::TSS_ADDRESSV],
            t[state::TSS_ADDRESSW],
            t[state::TSS_BORDERCOLOR],
            t[state::TSS_MIPMAPLODBIAS],
            t[state::TSS_MAXMIPLEVEL].max(lod),
            t[state::TSS_MAXANISOTROPY],
            self.anisotropy,
        ]);
        let (last_key, last) = self.last_samplers[stage];
        if last != vk::Sampler::null() && state::same(&last_key, &key) {
            return last;
        }
        if let Some(&s) = self.samplers.get(&key) {
            self.last_samplers[stage] = (key, s);
            return s;
        }
        let k = key.0;
        let filter = |f: u32| if f >= 2 { vk::Filter::LINEAR } else { vk::Filter::NEAREST };
        let address = |a: u32| match a {
            2 => vk::SamplerAddressMode::MIRRORED_REPEAT,
            3 | 5 => vk::SamplerAddressMode::CLAMP_TO_EDGE,
            4 => vk::SamplerAddressMode::CLAMP_TO_BORDER,
            _ => vk::SamplerAddressMode::REPEAT,
        };
        let border = if k[6] >> 24 < 128 {
            vk::BorderColor::FLOAT_TRANSPARENT_BLACK
        } else if k[6] & 0xFF_FFFF == 0 {
            vk::BorderColor::FLOAT_OPAQUE_BLACK
        } else {
            vk::BorderColor::FLOAT_OPAQUE_WHITE
        };
        let c = self.r.c;
        // The setting turns filtered, mipmapped sampling into trilinear anisotropic.
        let forced = k[10] > 1 && k[1] >= 2 && k[2] != 0 && c.anisotropy;
        let anisotropic = forced || ((k[0] == 3 || k[1] == 3) && c.anisotropy);
        let max_anisotropy = if forced { k[10].max(k[9]) } else { k[9].max(1) };
        let min_lod = k[8] as f32;
        let info = vk::SamplerCreateInfo::default()
            .mag_filter(filter(k[0]))
            .min_filter(filter(k[1]))
            .mipmap_mode(if forced || k[2] == 2 { vk::SamplerMipmapMode::LINEAR } else { vk::SamplerMipmapMode::NEAREST })
            .address_mode_u(address(k[3]))
            .address_mode_v(address(k[4]))
            .address_mode_w(address(k[5]))
            .border_color(border)
            .mip_lod_bias(f32::from_bits(k[7]))
            .anisotropy_enable(anisotropic)
            .max_anisotropy((max_anisotropy as f32).min(c.limits.max_sampler_anisotropy))
            .min_lod(min_lod)
            .max_lod(if k[2] == 0 { min_lod } else { vk::LOD_CLAMP_NONE });
        let sampler = unsafe { c.device.create_sampler(&info, None) }.expect("vkCreateSampler");
        self.samplers.insert(key, sampler);
        self.last_samplers[stage] = (key, sampler);
        sampler
    }

    fn vertex_shader(&self, handle: u32) -> Option<&VertexShader> {
        let index = (handle >> 1).checked_sub(1)? as usize;
        self.vertex_shaders.get(index)?.as_ref()
    }

    fn pixel_shader(&self, handle: u32) -> Option<&PixelShader> {
        let index = handle.checked_sub(1)? as usize;
        self.pixel_shaders.get(index)?.as_ref()
    }

    /// Sets everything up for a draw. `up` overrides stream 0 with data in the upload ring.
    fn object_matrices(&mut self) -> [Matrix; 2] {
        if self.object_dirty {
            self.object = state::object_matrices(&self.s);
            self.object_dirty = false;
            self.clip = None;
        }
        self.object
    }

    /// World, view and projection together, for culling.
    fn clip_matrix(&mut self) -> Matrix {
        let world_view = self.object_matrices()[0];
        *self.clip.get_or_insert_with(|| world_view.mul(&self.s.proj))
    }

    /// A fixed function draw from a vertex buffer that lies wholly outside the view, so it can't
    /// draw a pixel. With the whole terrain loaded (zoomed out past the game's limit) the terrain
    /// draws every tile of the map in every pass.
    fn outside_view(&mut self) -> bool {
        const POSITION_MASK: u32 = 0x00E;
        const XYZ: u32 = 0x002;
        if self.vs & 1 != 0 || self.vs & POSITION_MASK != XYZ {
            return false;
        }
        let (b, stride) = self.streams[0];
        let Some(data) = (unsafe { buffer::from_ptr(b as *mut c_void) }) else { return false };
        let Some(bounds) = data.bounds(stride) else { return false };
        let clip = self.clip_matrix();
        state::box_outside_clip(&clip, &bounds)
    }

    fn prepare_draw(&mut self, prim: u32, up: Option<(Slice, u32)>) -> bool {
        self.sync_scene();
        let mut laps = stats::Laps::start();
        let culled = up.is_none() && self.outside_view();
        laps.part(0);
        if culled {
            stats::add(&stats::CULLED_DRAWS, 1);
            return false;
        }
        let _timer = stats::Timer::new(&stats::DRAW_TICKS);
        let Some(topology) = pipeline::topology(prim) else {
            self.warn_once(format!("primitive type {prim} is not supported"));
            return false;
        };
        let (vs_module, layout_id, info, fixed_vs) = if self.vs & 1 == 0 {
            let fvf = self.vs;
            let entry = match self.fvf_layouts.get(&fvf) {
                Some(&e) => e,
                None => {
                    let (layout, _, info) = pipeline::fvf_layout(fvf);
                    let id = self.pipelines.layout_id(&layout);
                    self.fvf_layouts.insert(fvf, (id, info));
                    (id, info)
                }
            };
            (MODULE_FF_VS, entry.0, entry.1, true)
        } else {
            match self.vertex_shader(self.vs) {
                Some(v) => (v.module, v.layout, v.info, v.module == MODULE_FF_VS),
                None => {
                    self.warn_once(format!("draw with unknown vertex shader {:#x}", self.vs));
                    return false;
                }
            }
        };
        let fixed_ps = self.ps == 0;
        let ps_module = if fixed_ps {
            MODULE_FF_PS
        } else {
            match self.pixel_shader(self.ps) {
                Some(p) => p.module,
                None => {
                    self.warn_once(format!("draw with unknown pixel shader {:#x}", self.ps));
                    return false;
                }
            }
        };
        if self.last_info != Some((info, fixed_vs)) {
            self.last_info = Some((info, fixed_vs));
            self.vs_dirty = true;
        }
        let anisotropy = if info.rhw { 0 } else { ANISOTROPY.load(Ordering::Relaxed) };
        if anisotropy != self.anisotropy {
            self.anisotropy = anisotropy;
            self.set_dirty = true;
        }

        for stage in 0..8 {
            let t = self.textures[stage];
            if !t.is_null() {
                let data = unsafe { &(*t).data };
                data.upload(&mut self.r);
                data.used.set(true);
                if let Some(replacement) = data.replacement.borrow().as_ref() {
                    replacement.upload(&mut self.r);
                    replacement.used.set(true);
                }
                if data.smooth.get() {
                    let replaced = data.replacement.borrow().as_ref().is_some_and(|r| r.changed_since_smooth.replace(false));
                    if data.changed_since_smooth.replace(false) | replaced || data.smoothed.borrow().is_none() {
                        self.smooth(data);
                    }
                }
            }
        }
        laps.part(1);
        let Some(rt) = Self::surface(self.rt) else { return false };
        let target_alpha = rt.info.has_alpha;
        let Some((color, level, depth, scale)) = self.targets() else { return false };
        self.r.begin_pass(&color, level, depth.as_deref());
        if self.epoch != self.r.epoch {
            self.epoch = self.r.epoch;
            self.mark_all_dirty();
            self.check_sets();
            self.vs_slice = None;
            self.ps_slice = None;
        }
        if self.binds != self.r.binds {
            self.binds = self.r.binds;
            self.bound = Bound::default();
        }
        laps.part(2);

        let rs = &self.s.rs;
        let (src, dst) = match rs[state::RS_SRCBLEND] {
            12 => (vk::BlendFactor::SRC_ALPHA, vk::BlendFactor::ONE_MINUS_SRC_ALPHA),
            13 => (vk::BlendFactor::ONE_MINUS_SRC_ALPHA, vk::BlendFactor::SRC_ALPHA),
            s => (pipeline::blend_factor(s, target_alpha), pipeline::blend_factor(rs[state::RS_DESTBLEND], target_alpha)),
        };
        let key = PipelineKey {
            vs: vs_module,
            ps: ps_module,
            layout: layout_id,
            topology: pipeline::topology_class(topology),
            polygon: match rs[state::RS_FILLMODE] {
                1 => vk::PolygonMode::POINT,
                2 => vk::PolygonMode::LINE,
                _ => vk::PolygonMode::FILL,
            },
            blend: Blend { enable: rs[state::RS_ALPHABLENDENABLE] != 0, src, dst, op: pipeline::blend_op(rs[state::RS_BLENDOP]) },
            write_mask: vk::ColorComponentFlags::from_raw(rs[state::RS_COLORWRITEENABLE] & 0xF),
            depth: depth.is_some(),
            samples: color.samples,
            format: color.format,
        };
        let (rw, rh) = color.level_extent(level);
        let vp = scaled_viewport(&self.s.viewport, scale);
        let x = vp.x.min(rw);
        let y = vp.y.min(rh);
        let w = vp.width.min(rw - x).max(1);
        let h = vp.height.min(rh - y).max(1);
        let has_depth = depth.is_some();
        let stencil_op = |i: usize| vk::StencilOp::from_raw(rs[i].clamp(1, 8) as i32 - 1);
        let compare = |v: u32| vk::CompareOp::from_raw(v.clamp(1, 8) as i32 - 1);
        let draw_state = DrawState {
            // Depth is reversed (knowledge-base/vulkan-backend.md), so the D3D range flips too.
            viewport: [x, y, w, h, (1.0 - vp.max_z).to_bits(), (1.0 - vp.min_z).to_bits()],
            cull: match rs[state::RS_CULLMODE] {
                2 => vk::CullModeFlags::FRONT,
                3 => vk::CullModeFlags::BACK,
                _ => vk::CullModeFlags::NONE,
            },
            topology,
            depth: (
                has_depth && rs[state::RS_ZENABLE] != 0,
                has_depth && rs[state::RS_ZWRITEENABLE] != 0,
                compare(reversed_compare(rs[state::RS_ZFUNC])),
            ),
            stencil_test: has_depth && rs[state::RS_STENCILENABLE] != 0,
            stencil_ops: [stencil_op(state::RS_STENCILFAIL), stencil_op(state::RS_STENCILPASS), stencil_op(state::RS_STENCILZFAIL)],
            stencil_compare: compare(rs[state::RS_STENCILFUNC]),
            stencil_values: [rs[state::RS_STENCILMASK] & 0xFF, rs[state::RS_STENCILWRITEMASK] & 0xFF, rs[state::RS_STENCILREF] & 0xFF],
            blend: key.blend,
            write_mask: key.write_mask,
            polygon: key.polygon,
        };
        let soft = self.soft_setup(fixed_ps, info.rhw, &key.blend);
        if soft != self.soft {
            self.soft = soft;
            self.ps_dirty = true;
            self.set_dirty = true;
        }
        let pipeline = self.pipelines.get(&key);
        if pipeline != self.bound.pipeline {
            self.r.push(Cmd::Pipeline(pipeline));
            self.bound.pipeline = pipeline;
        }
        laps.part(3);

        let s = draw_state;
        let old = self.bound.state.replace(s);
        let first = old.is_none();
        let old = old.unwrap_or(s);
        let r = &mut self.r;
        if first || old.viewport != s.viewport {
            let [x, y, w, h, min_depth, max_depth] = s.viewport;
            let (min_depth, max_depth) = (f32::from_bits(min_depth), f32::from_bits(max_depth));
            r.push(Cmd::Viewport(
                vk::Viewport { x: x as f32, y: y as f32, width: w as f32, height: h as f32, min_depth, max_depth },
                vk::Rect2D { offset: vk::Offset2D { x: x as i32, y: y as i32 }, extent: vk::Extent2D { width: w, height: h } },
            ));
        }
        if first {
            // ZBIAS is applied in the vertex shader.
            r.push(Cmd::FixedState);
        }
        if first || old.cull != s.cull {
            r.push(Cmd::Cull(s.cull));
        }
        if first || old.topology != s.topology {
            r.push(Cmd::Topology(s.topology));
        }
        if first || old.depth != s.depth {
            r.push(Cmd::Depth(s.depth.0, s.depth.1, s.depth.2));
        }
        if first || old.stencil_test != s.stencil_test {
            r.push(Cmd::StencilTest(s.stencil_test));
        }
        if first || old.stencil_ops != s.stencil_ops || old.stencil_compare != s.stencil_compare {
            r.push(Cmd::StencilOp(s.stencil_ops, s.stencil_compare));
        }
        if first || old.stencil_values != s.stencil_values {
            r.push(Cmd::StencilValues(s.stencil_values));
        }
        if r.c.dynamic.is_some() {
            let b = s.blend;
            if first || old.blend.enable != b.enable {
                r.push(Cmd::BlendEnable(b.enable));
            }
            if first || old.blend != b {
                r.push(Cmd::BlendEquation(b.src, b.dst, b.op));
            }
            if first || old.write_mask != s.write_mask {
                r.push(Cmd::WriteMask(s.write_mask));
            }
            if first || old.polygon != s.polygon {
                r.push(Cmd::Polygon(s.polygon));
            }
        }
        laps.part(4);

        if self.vs_dirty || self.vs_slice.is_none() {
            stats::add(&stats::VS_BLOCKS, 1);
            let slice = if fixed_vs {
                let block = state::vs_block(&self.s, &info, &mut self.light_cache);
                uniforms(&mut self.r, &mut self.last_vs, self.vs_slice, state::as_bytes(&block), state::VS_BLOCK_SIZE)
            } else {
                if self.s.rs[state::RS_CLIPPLANEENABLE] != 0 {
                    self.warn_once("user clip planes are not applied to the game's vertex shaders");
                }
                let block = state::vs_shader_block(&self.s);
                uniforms(&mut self.r, &mut self.last_vs, self.vs_slice, state::as_bytes(&block), state::VS_BLOCK_SIZE)
            };
            self.vs_slice = Some(slice);
            self.vs_dirty = false;
        }
        if self.ps_dirty || self.ps_slice.is_none() {
            stats::add(&stats::PS_BLOCKS, 1);
            let bound = (0..8).filter(|&i| !self.textures[i].is_null()).fold(0u32, |m, i| m | 1 << i);
            let block = state::ps_block(&self.s, bound, fixed_ps, self.soft);
            let slice = uniforms(&mut self.r, &mut self.last_ps, self.ps_slice, state::as_bytes(&block), state::PS_BLOCK_SIZE);
            self.ps_slice = Some(slice);
            self.ps_dirty = false;
        }
        laps.part(5);
        let (vs_slice, ps_slice) = (self.vs_slice.unwrap(), self.ps_slice.unwrap());
        let stages = if fixed_ps { state::active_stages(&self.s) } else { 8 };
        if self.set_dirty || self.set_buffers != (vs_slice.buffer, ps_slice.buffer) || self.set_stages != stages {
            self.set = self.descriptor_set(vs_slice.buffer, ps_slice.buffer, stages);
            self.set_buffers = (vs_slice.buffer, ps_slice.buffer);
            self.set_stages = stages;
            self.set_dirty = false;
        }
        let offsets = [vs_slice.offset as u32, ps_slice.offset as u32];
        if self.bound.set != Some((self.set, offsets)) {
            self.r.push(Cmd::Set(self.set, offsets));
            self.bound.set = Some((self.set, offsets));
        }
        if fixed_vs {
            let object = self.object_matrices();
            if self.bound.object != Some(object) {
                self.r.push_constants(state::as_bytes(&object));
                self.bound.object = Some(object);
            }
        }
        laps.part(6);

        let mut bound = [(0u32, vk::Buffer::null(), 0u64, 0u32); VERTEX_BINDINGS];
        let mut count = 0;
        let mut strides = [0u32; VERTEX_BINDINGS];
        let mut missing = None;
        for &binding in &self.pipelines.layout(layout_id).bindings {
            let (buffer, offset, stride) = if binding == DUMMY_BINDING {
                (self.dummy.0, 0, 0)
            } else if let (0, Some((slice, stride))) = (binding, up) {
                (slice.buffer, slice.offset, stride)
            } else {
                let (b, stride) = self.streams[binding as usize];
                match unsafe { buffer::from_ptr(b as *mut c_void) } {
                    Some(data) => (data.bind(), 0, stride),
                    None => {
                        missing = Some(binding);
                        break;
                    }
                }
            };
            bound[count] = (binding, buffer, offset, stride);
            strides[binding as usize] = stride;
            count += 1;
        }
        if let Some(binding) = missing {
            self.warn_once(format!("draw without vertex stream {binding}"));
            return false;
        }
        let bound = &bound[..count];
        if self.r.c.dynamic.is_some() {
            if self.bound.vertex_input != Some((layout_id, strides)) {
                let pipelines = &self.pipelines;
                let input = self.vertex_inputs.entry((layout_id, strides)).or_insert_with(|| {
                    Arc::new(VertexInput {
                        bindings: bound.iter().map(|&(binding, _, _, stride)| (binding, stride)).collect(),
                        attributes: pipelines.layout(layout_id).attributes.iter().map(|a| (a.location, a.binding, a.format, a.offset)).collect(),
                    })
                });
                self.r.push(Cmd::VertexInput(input.clone()));
                self.bound.vertex_input = Some((layout_id, strides));
            }
            for &(binding, buffer, offset, stride) in bound {
                let slot = &mut self.bound.buffers[binding as usize];
                if *slot != Some((buffer, offset, stride)) {
                    self.r.push(Cmd::Vertices(binding, buffer, offset));
                    *slot = Some((buffer, offset, stride));
                }
            }
        } else {
            for &(binding, buffer, offset, stride) in bound {
                let slot = &mut self.bound.buffers[binding as usize];
                if *slot != Some((buffer, offset, stride)) {
                    self.r.push(Cmd::VerticesWithStride(binding, buffer, offset, stride as u64));
                    *slot = Some((buffer, offset, stride));
                }
            }
        }
        self.draws += 1;
        stats::add(&stats::DRAWS, 1);
        laps.part(7);
        true
    }

    /// Runs when a command buffer starts, so nothing in flight uses the frame's sets.
    fn check_sets(&mut self) {
        let frame = self.r.frame_index();
        let destroyed = context::images_destroyed();
        if self.sets_checked[frame] != destroyed || self.sets[frame].len() > SET_CACHE_LIMIT {
            self.sets[frame].clear();
            self.r.reset_descriptor_pools();
            self.sets_checked[frame] = destroyed;
        }
    }

    /// A descriptor set with the bound textures of the first `stages` stages and these uniform
    /// buffers. The other stages get the white image and stage 0's sampler, so textures left in
    /// stages a draw never samples don't make sets of their own.
    fn descriptor_set(&mut self, vs_buffer: vk::Buffer, ps_buffer: vk::Buffer, stages: usize) -> vk::DescriptorSet {
        let mut key = SetKey {
            buffers: [vs_buffer, ps_buffer],
            views: [self.white.view; 8],
            samplers: [vk::Sampler::null(); 8],
            depth_copy: match self.scene.as_ref().and_then(|s| s.depth_copy.as_ref()) {
                Some(copy) if self.soft[3] != 0.0 => copy.view,
                _ => self.white.view,
            },
        };
        for stage in 0..stages.max(1) {
            let (view, lod) = match self.texture_data(stage) {
                Some(t) => (t.view(), t.lod.get()),
                None => (self.white.view, 0),
            };
            key.views[stage] = view;
            key.samplers[stage] = self.sampler(stage, lod);
        }
        for stage in stages.max(1)..8 {
            key.samplers[stage] = key.samplers[0];
        }
        let frame = self.r.frame_index();
        if let Some(&set) = self.sets[frame].get(&key) {
            return set;
        }
        let set = self.r.descriptor_set();
        let vs_info = [vk::DescriptorBufferInfo { buffer: vs_buffer, offset: 0, range: state::VS_BLOCK_SIZE }];
        let ps_info = [vk::DescriptorBufferInfo { buffer: ps_buffer, offset: 0, range: state::PS_BLOCK_SIZE }];
        let images = key.views.map(|image_view| [vk::DescriptorImageInfo { sampler: vk::Sampler::null(), image_view, image_layout: vk::ImageLayout::GENERAL }]);
        let samplers = key.samplers.map(|sampler| [vk::DescriptorImageInfo { sampler, image_view: vk::ImageView::null(), image_layout: vk::ImageLayout::UNDEFINED }]);
        let mut writes = vec![
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(BINDING_VS)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                .buffer_info(&vs_info),
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(BINDING_PS)
                .descriptor_type(vk::DescriptorType::UNIFORM_BUFFER_DYNAMIC)
                .buffer_info(&ps_info),
        ];
        for stage in 0..8 {
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(BINDING_TEXTURES + stage as u32)
                    .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                    .image_info(&images[stage]),
            );
            writes.push(
                vk::WriteDescriptorSet::default()
                    .dst_set(set)
                    .dst_binding(BINDING_SAMPLERS + stage as u32)
                    .descriptor_type(vk::DescriptorType::SAMPLER)
                    .image_info(&samplers[stage]),
            );
        }
        let depth_info = [vk::DescriptorImageInfo { sampler: vk::Sampler::null(), image_view: key.depth_copy, image_layout: vk::ImageLayout::GENERAL }];
        writes.push(
            vk::WriteDescriptorSet::default()
                .dst_set(set)
                .dst_binding(BINDING_DEPTH_COPY)
                .descriptor_type(vk::DescriptorType::SAMPLED_IMAGE)
                .image_info(&depth_info),
        );
        unsafe { self.r.c.device.update_descriptor_sets(&writes, &[]) };
        self.sets[frame].insert(key, set);
        stats::add(&stats::DESCRIPTOR_WRITES, 1);
        set
    }

    fn release_bindings(&mut self) {
        unsafe {
            for t in &mut self.textures {
                if !t.is_null() {
                    com::release(*t as *mut c_void);
                    *t = std::ptr::null_mut();
                }
            }
            for (b, _) in &mut self.streams {
                if !b.is_null() {
                    com::release(*b as *mut c_void);
                    *b = std::ptr::null_mut();
                }
            }
            if !self.indices.is_null() {
                com::release(self.indices as *mut c_void);
                self.indices = std::ptr::null_mut();
            }
            if !self.rt.is_null() {
                texture::surf_release(self.rt);
                self.rt = std::ptr::null_mut();
            }
            if !self.ds.is_null() {
                texture::surf_release(self.ds);
                self.ds = std::ptr::null_mut();
            }
        }
    }
}

fn dev<'a>(this: *mut Device) -> &'a mut Device {
    unsafe { &mut *this }
}

unsafe extern "system" fn query_interface(this: *mut Device, _iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    unsafe {
        add_ref(this);
        *out = this as *mut c_void;
    }
    D3D_OK
}

unsafe extern "system" fn add_ref(this: *mut Device) -> u32 {
    let d = dev(this);
    d.refs.set(d.refs.get() + 1);
    d.refs.get()
}

unsafe extern "system" fn release(this: *mut Device) -> u32 {
    let d = dev(this);
    let n = d.refs.get() - 1;
    d.refs.set(n);
    if n == 0 {
        log!("vulkan device released after {} draws", d.draws);
        d.r.wait_idle();
        d.release_bindings();
        unsafe {
            if !d.backbuffer.is_null() {
                texture::surf_release(d.backbuffer);
            }
            if !d.auto_depth.is_null() {
                texture::surf_release(d.auto_depth);
            }
        }
        // The device's Vulkan objects stay until the process ends, the game only releases it on exit.
    }
    n
}

unsafe extern "system" fn test_cooperative_level(_this: *mut Device) -> HRESULT {
    D3D_OK
}

unsafe extern "system" fn get_available_texture_mem(this: *mut Device) -> u32 {
    dev(this).r.c.device_local_bytes.min(0x7FF0_0000) as u32
}

unsafe extern "system" fn resource_manager_discard_bytes(_this: *mut Device, _bytes: u32) -> HRESULT {
    D3D_OK
}

unsafe extern "system" fn get_direct3d(this: *mut Device, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let d = dev(this);
    unsafe {
        com::add_ref(d.d3d);
        *out = d.d3d;
    }
    D3D_OK
}

unsafe extern "system" fn get_device_caps(this: *mut Device, caps: *mut c_void) -> HRESULT {
    let d = dev(this);
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, *mut c_void) -> HRESULT =
        unsafe { std::mem::transmute(crate::direct3d::method(d.system_d3d, crate::direct3d::SLOT_GET_DEVICE_CAPS)) };
    unsafe { f(d.system_d3d, d.adapter, d.device_type, caps) }
}

unsafe extern "system" fn get_display_mode(this: *mut Device, out: *mut D3DDISPLAYMODE) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let d = dev(this);
    let mut mode = get_adapter_mode(d.system_d3d, d.adapter);
    if d.fullscreen {
        mode.Width = d.params.BackBufferWidth;
        mode.Height = d.params.BackBufferHeight;
    }
    unsafe { *out = mode };
    D3D_OK
}

#[repr(C)]
struct CreationParameters {
    adapter: u32,
    device_type: u32,
    focus: HWND,
    behavior: u32,
}

unsafe extern "system" fn get_creation_parameters(this: *mut Device, out: *mut CreationParameters) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let d = dev(this);
    unsafe { *out = CreationParameters { adapter: d.adapter, device_type: d.device_type, focus: d.focus, behavior: d.behavior } };
    D3D_OK
}

unsafe extern "system" fn set_cursor_properties(_this: *mut Device, _x: u32, _y: u32, _surface: *mut c_void) -> HRESULT {
    D3D_OK
}

unsafe extern "system" fn set_cursor_position(_this: *mut Device, _x: i32, _y: i32, _flags: u32) {}

unsafe extern "system" fn show_cursor(_this: *mut Device, _show: i32) -> i32 {
    0
}

unsafe extern "system" fn not_available2(this: *mut Device, _a: u32, _b: u32) -> HRESULT {
    dev(this).warn_once("CreateAdditionalSwapChain is not supported");
    D3DERR_INVALIDCALL
}

/// Reset's first half, on the renderer thread: everything recorded so far finishes.
fn reset_begin(this: *mut Device, requested: &D3DPRESENT_PARAMETERS) {
    let d = dev(this);
    log!("Reset (vulkan) requested {requested:?}");
    d.r.end_pass();
    d.r.flush_and_wait();
}

/// Reset's second half, on the renderer thread, after the window was arranged on the game thread.
fn reset_finish(this: *mut Device, requested: D3DPRESENT_PARAMETERS, fullscreen: bool) -> HRESULT {
    let d = dev(this);
    d.fullscreen = fullscreen;
    d.release_bindings();
    unsafe {
        if !d.backbuffer.is_null() {
            texture::surf_release(d.backbuffer);
        }
        if !d.auto_depth.is_null() {
            texture::surf_release(d.auto_depth);
        }
    }
    d.params = requested;
    d.s = State::new(requested.EnableAutoDepthStencil != 0);
    d.vs = 0;
    d.ps = 0;
    d.scene = None;
    d.scene_open = false;
    d.create_default_targets(&requested);
    d.swapchain.set_window(if requested.hDeviceWindow.is_null() { d.focus } else { requested.hDeviceWindow });
    d.mark_all_dirty();
    D3D_OK
}

/// Present's work on the renderer thread: the frame goes to the presenter.
fn present_frame(this: *mut Device) -> HRESULT {
    let d = dev(this);
    d.sync_scene();
    let Some(back) = Device::surface(d.backbuffer) else { return D3DERR_INVALIDCALL };
    let Some((image, _)) = back.gpu() else { return D3DERR_INVALIDCALL };
    let start = std::time::Instant::now();
    d.swapchain.present(&mut d.r, &image, &d.gamma);
    stats::add(&stats::PRESENT_US, start.elapsed().as_micros() as u64);
    D3D_OK
}

/// The present pass from a render target texture of the back buffer's size into the back buffer,
/// with a straight gamma ramp, so the GPU test can read what it makes.
#[cfg(test)]
pub fn present_pass_for_test(this: *mut Device, source: *mut c_void, lift: f32, pattern: Option<[f32; 4]>) {
    crate::vk::worker::run(|| {
        let d = dev(this);
        let c = d.r.c;
        let source = unsafe { &(*texture::texture_from_ptr(source).expect("a texture")).data }.gpu.clone();
        let (back, _) = Device::surface(d.backbuffer).and_then(|b| b.gpu()).expect("a back buffer");
        let mut pass = Pass::new(c, "present test", &format!("{}{}", post::COMMON, post::PRESENT), None).expect("present pass");
        pass.set_format(c, back.format);
        let gamma: [u16; 768] = std::array::from_fn(|i| (i % 256) as u16 * 257);
        let uniform = post::present_uniform(true, &gamma, lift, false, pattern);
        d.r.transfer();
        let extent = vk::Extent2D { width: back.width, height: back.height };
        pass.draw(&mut d.r, &[source.view], back.attachment_views[0], vk::ImageLayout::GENERAL, extent, &uniform);
    });
}

unsafe extern "system" fn get_back_buffer(this: *mut Device, _index: u32, _ty: u32, out: *mut *mut Surface) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let d = dev(this);
    unsafe {
        texture::surf_add_ref(d.backbuffer);
        *out = d.backbuffer;
    }
    D3D_OK
}

#[repr(C)]
struct RasterStatus {
    in_vblank: i32,
    scan_line: u32,
}

unsafe extern "system" fn get_raster_status(_this: *mut Device, out: *mut RasterStatus) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    unsafe { *out = RasterStatus { in_vblank: 0, scan_line: 0 } };
    D3D_OK
}

unsafe extern "system" fn set_gamma_ramp(this: *mut Device, _flags: u32, ramp: *const [u16; 768]) {
    if !ramp.is_null() {
        let ramp = unsafe { *ramp };
        log!("gamma ramp set, red 0, 64, 128, 192, 255 map to {} {} {} {} {}", ramp[0] >> 8, ramp[64] >> 8, ramp[128] >> 8, ramp[192] >> 8, ramp[255] >> 8);
        dev(this).gamma = ramp;
    }
}

unsafe extern "system" fn get_gamma_ramp(this: *mut Device, ramp: *mut [u16; 768]) {
    if !ramp.is_null() {
        unsafe { *ramp = dev(this).gamma };
    }
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_texture(
    this: *mut Device,
    width: u32,
    height: u32,
    levels: u32,
    usage: u32,
    format: u32,
    pool: u32,
    out: *mut *mut Texture,
) -> HRESULT {
    let d = dev(this);
    if out.is_null() || width == 0 || height == 0 {
        return D3DERR_INVALIDCALL;
    }
    let _timer = stats::Timer::new(&stats::CREATE_TICKS);
    stats::add(&stats::TEXTURE_CREATES, 1);
    stats::note_texture_creator(width, height, format);
    let Some(info) = format::info(format).filter(|i| i.kind != format::Kind::Depth) else {
        texture::log_unsupported("CreateTexture", format);
        return D3DERR_INVALIDCALL;
    };
    if usage & texture::USAGE_RENDERTARGET != 0 && !format::renderable(format) {
        texture::log_unsupported("CreateTexture render target", format);
        return D3DERR_INVALIDCALL;
    }
    let info = if usage & texture::USAGE_RENDERTARGET != 0 { Info { vk: vk::Format::B8G8R8A8_UNORM, ..info } } else { info };
    if usage & texture::USAGE_RENDERTARGET != 0 {
        d.warn_once(format!("render target texture {width}x{height} {}", crate::d3d::format_name(format)));
    }
    unsafe { *out = texture::create_texture(&mut d.r, this, info, width, height, levels, usage, pool) };
    D3D_OK
}

unsafe extern "system" fn not_available8(this: *mut Device, _a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32, _h: u32) -> HRESULT {
    dev(this).warn_once("volume textures are not supported");
    D3DERR_INVALIDCALL
}

unsafe extern "system" fn not_available6(this: *mut Device, _a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32) -> HRESULT {
    dev(this).warn_once("cube textures are not supported");
    D3DERR_INVALIDCALL
}

unsafe extern "system" fn create_vertex_buffer(this: *mut Device, length: u32, usage: u32, fvf: u32, pool: u32, out: *mut *mut Buffer) -> HRESULT {
    if out.is_null() || length == 0 {
        return D3DERR_INVALIDCALL;
    }
    unsafe { *out = buffer::create(this, BufferData::new(length, usage, pool, fvf, false)) };
    D3D_OK
}

unsafe extern "system" fn create_index_buffer(this: *mut Device, length: u32, usage: u32, format: u32, pool: u32, out: *mut *mut Buffer) -> HRESULT {
    if out.is_null() || length == 0 || !matches!(format, format::INDEX16 | format::INDEX32) {
        return D3DERR_INVALIDCALL;
    }
    unsafe { *out = buffer::create(this, BufferData::new(length, usage, pool, format, true)) };
    D3D_OK
}

unsafe extern "system" fn create_render_target(
    this: *mut Device,
    width: u32,
    height: u32,
    format: u32,
    _multisample: u32,
    _lockable: i32,
    out: *mut *mut Surface,
) -> HRESULT {
    let d = dev(this);
    if out.is_null() || !format::renderable(format) {
        return D3DERR_INVALIDCALL;
    }
    let info = format::info(format).unwrap();
    let image = GpuImage::new(&mut d.r, vk::Format::B8G8R8A8_UNORM, width.max(1), height.max(1), 1, ImageUse::Target, !info.has_alpha);
    let s = SurfaceData::new(info, width, height, texture::USAGE_RENDERTARGET, texture::POOL_DEFAULT, Backing::Image(image.into()));
    unsafe { *out = texture::create_surface(this, std::ptr::null_mut(), s) };
    D3D_OK
}

unsafe extern "system" fn create_depth_stencil_surface(this: *mut Device, width: u32, height: u32, format: u32, _multisample: u32, out: *mut *mut Surface) -> HRESULT {
    let d = dev(this);
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let info = texture::depth_info(format);
    d.warn_once(format!("depth stencil surface {width}x{height}"));
    let image = GpuImage::new(&mut d.r, info.vk, width.max(1), height.max(1), 1, ImageUse::Depth, false);
    let s = SurfaceData::new(info, width, height, texture::USAGE_DEPTHSTENCIL, texture::POOL_DEFAULT, Backing::Image(image.into()));
    unsafe { *out = texture::create_surface(this, std::ptr::null_mut(), s) };
    D3D_OK
}

unsafe extern "system" fn create_image_surface(this: *mut Device, width: u32, height: u32, format: u32, out: *mut *mut Surface) -> HRESULT {
    if out.is_null() || width == 0 || height == 0 {
        return D3DERR_INVALIDCALL;
    }
    let Some(info) = format::info(format).filter(|i| i.kind != format::Kind::Depth) else {
        texture::log_unsupported("CreateImageSurface", format);
        return D3DERR_INVALIDCALL;
    };
    let bytes = vec![0u8; info.level_size(width, height)];
    let s = SurfaceData::new(info, width, height, 0, texture::POOL_SYSTEMMEM, Backing::Memory(bytes.into()));
    unsafe { *out = texture::create_surface(this, std::ptr::null_mut(), s) };
    D3D_OK
}

#[repr(C)]
#[derive(Clone, Copy)]
struct Point {
    x: i32,
    y: i32,
}

unsafe extern "system" fn copy_rects(this: *mut Device, src: *mut c_void, rects: *const Rect, count: u32, dst: *mut c_void, points: *const Point) -> HRESULT {
    let d = dev(this);
    if src == d.backbuffer as *mut c_void && d.scene.as_ref().is_some_and(|s| s.active) {
        d.warn_once("the back buffer was copied while the 3D world is drawn into its own target");
    }
    let (Some(src), Some(dst)) = (unsafe { texture::surface_from_ptr(src) }, unsafe { texture::surface_from_ptr(dst) }) else {
        return D3DERR_INVALIDCALL;
    };
    if src == dst {
        return D3DERR_INVALIDCALL;
    }
    let (s, t) = unsafe { (&(*src).data, &(*dst).data) };
    if s.info.d3d != t.info.d3d {
        d.warn_once(format!("CopyRects between {} and {}", crate::d3d::format_name(s.info.d3d), crate::d3d::format_name(t.info.d3d)));
        return D3DERR_INVALIDCALL;
    }
    let list: Vec<Rect> = if rects.is_null() || count == 0 {
        vec![Rect { left: 0, top: 0, right: s.width as i32, bottom: s.height as i32 }]
    } else {
        unsafe { std::slice::from_raw_parts(rects, count as usize) }.to_vec()
    };
    let bytes = s.with_bytes(&mut d.r, |b| b.to_vec());
    let pitch = s.pitch() as usize;
    for (i, rc) in list.iter().enumerate() {
        let (px, py) = if points.is_null() { (rc.left, rc.top) } else { unsafe { let p = *points.add(i); (p.x, p.y) } };
        if rc.left < 0 || rc.top < 0 || px < 0 || py < 0 || rc.right <= rc.left || rc.bottom <= rc.top {
            continue;
        }
        let w = ((rc.right - rc.left) as u32).min(s.width.saturating_sub(rc.left as u32)).min(t.width.saturating_sub(px as u32));
        let h = ((rc.bottom - rc.top) as u32).min(s.height.saturating_sub(rc.top as u32)).min(t.height.saturating_sub(py as u32));
        if w == 0 || h == 0 {
            continue;
        }
        let offset = s.info.offset(s.width, rc.left as u32, rc.top as u32);
        t.write_rect(&mut d.r, &bytes[offset..], pitch, px as u32, py as u32, w, h);
    }
    D3D_OK
}

unsafe extern "system" fn update_texture(this: *mut Device, src: *mut c_void, dst: *mut c_void) -> HRESULT {
    let d = dev(this);
    let (Some(src), Some(dst)) = (unsafe { texture::texture_from_ptr(src) }, unsafe { texture::texture_from_ptr(dst) }) else {
        return D3DERR_INVALIDCALL;
    };
    let (s, t) = unsafe { (&(*src).data, &(*dst).data) };
    if s.info.d3d != t.info.d3d || s.levels < t.levels {
        return D3DERR_INVALIDCALL;
    }
    let skip = s.levels - t.levels;
    if t.render_target() {
        d.warn_once("UpdateTexture into a render target");
        return D3DERR_INVALIDCALL;
    }
    let from = s.shadow.borrow();
    let mut to = t.shadow.borrow_mut();
    for level in 0..t.levels {
        let a = &from[(level + skip) as usize];
        let b = &mut to[level as usize];
        if a.len() == b.len() {
            b.copy_from_slice(a);
            t.mark_dirty(level);
        }
    }
    D3D_OK
}

unsafe extern "system" fn get_front_buffer(this: *mut Device, dst: *mut c_void) -> HRESULT {
    let d = dev(this);
    let Some(dst) = (unsafe { texture::surface_from_ptr(dst) }) else { return D3DERR_INVALIDCALL };
    let t = unsafe { &(*dst).data };
    let Some(back) = Device::surface(d.backbuffer) else { return D3DERR_INVALIDCALL };
    let Some((image, _)) = back.gpu() else { return D3DERR_INVALIDCALL };
    let bgra = d.r.read_image(&image, 0);
    let w = image.width.min(t.width);
    let h = image.height.min(t.height);
    let mut converted = vec![0u8; t.info.level_size(w, h)];
    let mut tight = Vec::with_capacity((w * h * 4) as usize);
    for y in 0..h as usize {
        let row = y * image.width as usize * 4;
        tight.extend_from_slice(&bgra[row..row + w as usize * 4]);
    }
    let pitch = t.info.pitch(w) as usize;
    t.info.from_bgra(&tight, w, h, &mut converted, pitch);
    t.write_rect(&mut d.r, &converted, pitch, 0, 0, w, h);
    D3D_OK
}

unsafe extern "system" fn set_render_target(this: *mut Device, rt: *mut c_void, ds: *mut c_void) -> HRESULT {
    let d = dev(this);
    unsafe {
        if !rt.is_null() {
            let Some(s) = texture::surface_from_ptr(rt) else { return D3DERR_INVALIDCALL };
            let data = &(*s).data;
            if data.gpu().is_none() || data.info.kind == format::Kind::Depth || data.usage & texture::USAGE_RENDERTARGET == 0 {
                return D3DERR_INVALIDCALL;
            }
            texture::surf_add_ref(s);
            if !d.rt.is_null() {
                texture::surf_release(d.rt);
            }
            d.rt = s;
        }
        let new_ds = if ds.is_null() {
            std::ptr::null_mut()
        } else {
            match texture::surface_from_ptr(ds) {
                Some(s) if (&*s).data.info.kind == format::Kind::Depth => s,
                _ => return D3DERR_INVALIDCALL,
            }
        };
        if !new_ds.is_null() {
            texture::surf_add_ref(new_ds);
        }
        if !d.ds.is_null() {
            texture::surf_release(d.ds);
        }
        d.ds = new_ds;
    }
    let rt = Device::surface(d.rt).unwrap();
    let (w, h) = (rt.width, rt.height);
    d.s.viewport = Viewport { x: 0, y: 0, width: w, height: h, min_z: 0.0, max_z: 1.0 };
    d.vs_dirty = true;
    if d.rt != d.backbuffer {
        let depth = match Device::surface(d.ds) {
            None => "no depth".to_string(),
            Some(_) if d.ds == d.auto_depth => "the automatic depth buffer".to_string(),
            Some(s) => format!("a {}x{} depth surface", s.width, s.height),
        };
        d.warn_once(format!("drawing into a {w}x{h} render target with {depth}"));
    }
    D3D_OK
}

unsafe extern "system" fn clear(this: *mut Device, count: u32, rects: *const Rect, flags: u32, color: u32, z: f32, stencil: u32) -> HRESULT {
    let d = dev(this);
    d.sync_scene();
    if flags & 2 != 0 && d.ds == d.auto_depth {
        d.depth_clear = (z.clamp(0.0, 1.0), stencil & 0xFF);
    }
    if flags & 1 != 0 && d.rt == d.backbuffer {
        d.color_clear = color;
    }
    let Some((image, level, depth, scale)) = d.targets() else { return D3DERR_INVALIDCALL };
    for stage in 0..8 {
        if let Some(t) = d.texture_data(stage).cloned() {
            t.upload(&mut d.r);
        }
    }
    d.r.begin_pass(&image, level, depth.as_deref());
    let mut attachments = Vec::new();
    if flags & 1 != 0 {
        let c = [((color >> 16) & 0xFF) as f32 / 255.0, ((color >> 8) & 0xFF) as f32 / 255.0, (color & 0xFF) as f32 / 255.0, (color >> 24) as f32 / 255.0];
        attachments.push(vk::ClearAttachment {
            aspect_mask: vk::ImageAspectFlags::COLOR,
            color_attachment: 0,
            clear_value: vk::ClearValue { color: vk::ClearColorValue { float32: c } },
        });
    }
    let mut aspect = vk::ImageAspectFlags::empty();
    if depth.is_some() {
        if flags & 2 != 0 {
            aspect |= vk::ImageAspectFlags::DEPTH;
        }
        if flags & 4 != 0 {
            aspect |= vk::ImageAspectFlags::STENCIL;
        }
    }
    if !aspect.is_empty() {
        attachments.push(vk::ClearAttachment {
            aspect_mask: aspect,
            color_attachment: 0,
            clear_value: vk::ClearValue { depth_stencil: vk::ClearDepthStencilValue { depth: 1.0 - z.clamp(0.0, 1.0), stencil: stencil & 0xFF } },
        });
    }
    if attachments.is_empty() {
        return D3D_OK;
    }
    let (rw, rh) = image.level_extent(level);
    let vp = scaled_viewport(&d.s.viewport, scale);
    let (vx0, vy0) = (vp.x.min(rw) as i32, vp.y.min(rh) as i32);
    let (vx1, vy1) = ((vp.x + vp.width).min(rw) as i32, (vp.y + vp.height).min(rh) as i32);
    let edge = |v: i32| (v as f32 * scale).round() as i32;
    let list: Vec<Rect> = if rects.is_null() || count == 0 {
        vec![Rect { left: vx0, top: vy0, right: vx1, bottom: vy1 }]
    } else {
        unsafe { std::slice::from_raw_parts(rects, count as usize) }
            .iter()
            .map(|r| Rect { left: edge(r.left), top: edge(r.top), right: edge(r.right), bottom: edge(r.bottom) })
            .collect()
    };
    let clear_rects: Vec<vk::ClearRect> = list
        .iter()
        .filter_map(|rc| {
            let (x0, y0, x1, y1) = (rc.left.max(vx0), rc.top.max(vy0), rc.right.min(vx1), rc.bottom.min(vy1));
            (x1 > x0 && y1 > y0).then(|| vk::ClearRect {
                rect: vk::Rect2D { offset: vk::Offset2D { x: x0, y: y0 }, extent: vk::Extent2D { width: (x1 - x0) as u32, height: (y1 - y0) as u32 } },
                base_array_layer: 0,
                layer_count: 1,
            })
        })
        .collect();
    if !clear_rects.is_empty() {
        d.r.record(move |c, cmd| unsafe { c.device.cmd_clear_attachments(cmd, &attachments, &clear_rects) });
    }
    D3D_OK
}

fn transform_slot(s: &mut State, state: u32) -> Option<&mut Matrix> {
    match state {
        state::TS_VIEW => Some(&mut s.view),
        state::TS_PROJECTION => Some(&mut s.proj),
        state::TS_WORLD => Some(&mut s.world),
        t if (state::TS_TEXTURE0..state::TS_TEXTURE0 + 8).contains(&t) => Some(&mut s.tex[(t - state::TS_TEXTURE0) as usize]),
        _ => None,
    }
}

/// Stores a transform. World and view go to the push constants, and everything but world is
/// also in the vertex uniforms (lights are moved into view space there).
fn store_transform(d: &mut Device, state: u32, m: Matrix) {
    let Some(slot) = transform_slot(&mut d.s, state) else { return };
    if state::same(slot, &m) {
        return;
    }
    *slot = m;
    if state == state::TS_WORLD || state == state::TS_VIEW {
        d.object_dirty = true;
    }
    if state == state::TS_PROJECTION {
        d.clip = None;
    }
    if state != state::TS_WORLD {
        d.vs_dirty = true;
    }
}

unsafe extern "system" fn set_viewport(this: *mut Device, vp: *const Viewport) -> HRESULT {
    let d = dev(this);
    if vp.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let vp = unsafe { *vp };
    if !state::same(&d.s.viewport, &vp) {
        d.s.viewport = vp;
        d.vs_dirty = true;
    }
    D3D_OK
}

unsafe extern "system" fn set_material(this: *mut Device, m: *const Material) -> HRESULT {
    let d = dev(this);
    if m.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let m = unsafe { *m };
    if !state::same(&d.s.material, &m) {
        d.s.material = m;
        d.vs_dirty = true;
    }
    D3D_OK
}

fn default_light() -> Light {
    Light { ty: 3, diffuse: [1.0, 1.0, 1.0, 0.0], direction: [0.0, 0.0, 1.0], ..Default::default() }
}

unsafe extern "system" fn set_light(this: *mut Device, index: u32, light: *const Light) -> HRESULT {
    let d = dev(this);
    if light.is_null() || index > 4096 {
        return D3DERR_INVALIDCALL;
    }
    let i = index as usize;
    if d.s.lights.len() <= i {
        d.s.lights.resize(i + 1, default_light());
    }
    let light = unsafe { *light };
    if !state::same(&d.s.lights[i], &light) {
        d.s.lights[i] = light;
        d.vs_dirty |= d.s.enabled_lights.contains(&index);
    }
    D3D_OK
}

unsafe extern "system" fn light_enable(this: *mut Device, index: u32, enable: i32) -> HRESULT {
    let d = dev(this);
    if index > 4096 {
        return D3DERR_INVALIDCALL;
    }
    if d.s.lights.len() <= index as usize {
        d.s.lights.resize(index as usize + 1, default_light());
    }
    let present = d.s.enabled_lights.contains(&index);
    if enable != 0 && !present {
        d.s.enabled_lights.push(index);
        d.vs_dirty = true;
    } else if enable == 0 && present {
        d.s.enabled_lights.retain(|&i| i != index);
        d.vs_dirty = true;
    }
    D3D_OK
}

unsafe extern "system" fn set_render_state(this: *mut Device, state: u32, value: u32) -> HRESULT {
    let d = dev(this);
    if state >= 256 {
        return D3DERR_INVALIDCALL;
    }
    match state as usize {
        state::RS_FOGENABLE if value != 0 => d.warn_once("fog is not supported"),
        state::RS_CLIPPLANEENABLE if value >> shader::CLIP_PLANES != 0 => d.warn_once("user clip planes past the sixth are not supported"),
        151 if value != 0 => d.warn_once("vertex blending is not supported"),
        156 if value != 0 => d.warn_once("point sprites are not supported"),
        _ => {}
    }
    let changed = d.s.rs[state as usize] != value;
    d.s.rs[state as usize] = value;
    if changed {
        // Only these feed the uniform blocks (state::vs_block, viewport_vec, ps_block). The
        // rest is read at each draw.
        match state as usize {
            state::RS_LIGHTING
            | state::RS_NORMALIZENORMALS
            | state::RS_LOCALVIEWER
            | state::RS_COLORVERTEX
            | state::RS_DIFFUSEMATERIALSOURCE
            | state::RS_AMBIENTMATERIALSOURCE
            | state::RS_SPECULARMATERIALSOURCE
            | state::RS_EMISSIVEMATERIALSOURCE
            | state::RS_AMBIENT
            | state::RS_ZBIAS
            | state::RS_CLIPPLANEENABLE => d.vs_dirty = true,
            state::RS_SPECULARENABLE => {
                d.vs_dirty = true;
                d.ps_dirty = true;
            }
            state::RS_ALPHATESTENABLE | state::RS_ALPHAFUNC | state::RS_ALPHAREF | state::RS_TEXTUREFACTOR | state::RS_SHADEMODE => {
                d.ps_dirty = true
            }
            _ => {}
        }
    }
    D3D_OK
}

unsafe extern "system" fn begin_state_block(this: *mut Device) -> HRESULT {
    dev(this).recording_block = true;
    D3D_OK
}

unsafe extern "system" fn end_state_block(this: *mut Device, out: *mut u32) -> HRESULT {
    let d = dev(this);
    if out.is_null() || !d.recording_block {
        return D3DERR_INVALIDCALL;
    }
    d.recording_block = false;
    d.blocks.push(Some((d.s.clone(), d.vs, d.ps)));
    unsafe { *out = d.blocks.len() as u32 };
    D3D_OK
}

unsafe extern "system" fn apply_state_block(this: *mut Device, token: u32) -> HRESULT {
    let d = dev(this);
    let Some(Some((s, vs, ps))) = token.checked_sub(1).and_then(|i| d.blocks.get(i as usize)).cloned() else {
        return D3DERR_INVALIDCALL;
    };
    d.s = s;
    d.vs = vs;
    d.ps = ps;
    d.mark_all_dirty();
    D3D_OK
}

unsafe extern "system" fn capture_state_block(this: *mut Device, token: u32) -> HRESULT {
    let d = dev(this);
    let snapshot = (d.s.clone(), d.vs, d.ps);
    match token.checked_sub(1).and_then(|i| d.blocks.get_mut(i as usize)) {
        Some(slot @ Some(_)) => {
            *slot = Some(snapshot);
            D3D_OK
        }
        _ => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn delete_state_block(this: *mut Device, token: u32) -> HRESULT {
    let d = dev(this);
    match token.checked_sub(1).and_then(|i| d.blocks.get_mut(i as usize)) {
        Some(slot) => {
            *slot = None;
            D3D_OK
        }
        None => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn create_state_block(this: *mut Device, _ty: u32, out: *mut u32) -> HRESULT {
    let d = dev(this);
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    d.blocks.push(Some((d.s.clone(), d.vs, d.ps)));
    unsafe { *out = d.blocks.len() as u32 };
    D3D_OK
}

unsafe extern "system" fn set_texture(this: *mut Device, stage: u32, texture: *mut c_void) -> HRESULT {
    let d = dev(this);
    if stage >= 8 {
        return D3DERR_INVALIDCALL;
    }
    let new = if texture.is_null() {
        std::ptr::null_mut()
    } else {
        match unsafe { texture::texture_from_ptr(texture) } {
            Some(t) => t,
            None => {
                d.warn_once("SetTexture with a texture that is not ours");
                return D3DERR_INVALIDCALL;
            }
        }
    };
    let old = d.textures[stage as usize];
    if old == new {
        return D3D_OK;
    }
    unsafe {
        if !new.is_null() {
            com::add_ref(new as *mut c_void);
        }
        if !old.is_null() {
            com::release(old as *mut c_void);
        }
    }
    d.textures[stage as usize] = new;
    // The pixel block only reads which stages have a texture.
    d.ps_dirty |= old.is_null() != new.is_null();
    d.set_dirty = true;
    D3D_OK
}

unsafe extern "system" fn set_texture_stage_state(this: *mut Device, stage: u32, ty: u32, value: u32) -> HRESULT {
    let d = dev(this);
    if stage >= 8 || ty >= 32 {
        return D3DERR_INVALIDCALL;
    }
    let slot = &mut d.s.tss[stage as usize][ty as usize];
    if *slot == value {
        return D3D_OK;
    }
    *slot = value;
    match ty as usize {
        state::TSS_TEXCOORDINDEX | state::TSS_TEXTURETRANSFORMFLAGS => {
            d.vs_dirty = true;
            d.ps_dirty = true;
        }
        state::TSS_ADDRESSU..=state::TSS_MAXANISOTROPY | state::TSS_ADDRESSW => d.set_dirty = true,
        _ => d.ps_dirty = true,
    }
    D3D_OK
}

unsafe extern "system" fn validate_device(_this: *mut Device, passes: *mut u32) -> HRESULT {
    if !passes.is_null() {
        unsafe { *passes = 1 };
    }
    D3D_OK
}

unsafe extern "system" fn get_info(_this: *mut Device, _id: u32, _data: *mut c_void, _size: u32) -> HRESULT {
    S_FALSE
}

unsafe extern "system" fn set_palette_entries(_this: *mut Device, _index: u32, _entries: *const c_void) -> HRESULT {
    D3D_OK
}

unsafe extern "system" fn get_palette_entries(_this: *mut Device, _index: u32, _entries: *mut c_void) -> HRESULT {
    D3DERR_INVALIDCALL
}

unsafe extern "system" fn draw_primitive(this: *mut Device, prim: u32, start: u32, count: u32) -> HRESULT {
    let d = dev(this);
    if count == 0 {
        return D3D_OK;
    }
    if d.prepare_draw(prim, None) {
        d.r.push(Cmd::Draw(pipeline::vertex_count(prim, count), start));
    }
    D3D_OK
}

fn bind_indices(d: &mut Device) -> bool {
    let Some(data) = (unsafe { buffer::from_ptr(d.indices as *mut c_void) }) else {
        d.warn_once("indexed draw without an index buffer");
        return false;
    };
    let ty = if data.format == format::INDEX32 { vk::IndexType::UINT32 } else { vk::IndexType::UINT16 };
    d.r.push(Cmd::Indices(data.bind(), 0, ty));
    true
}

unsafe extern "system" fn draw_indexed_primitive(this: *mut Device, prim: u32, _min_index: u32, _vertices: u32, start: u32, count: u32) -> HRESULT {
    let d = dev(this);
    if count == 0 {
        return D3D_OK;
    }
    if d.prepare_draw(prim, None) && bind_indices(d) {
        let indices = pipeline::vertex_count(prim, count);
        d.r.push(Cmd::DrawIndexed(indices, start, d.base_vertex as i32));
    }
    D3D_OK
}

fn clear_stream0_and_indices(d: &mut Device) {
    unsafe {
        let (b, _) = d.streams[0];
        if !b.is_null() {
            com::release(b as *mut c_void);
        }
        d.streams[0] = (std::ptr::null_mut(), 0);
        if !d.indices.is_null() {
            com::release(d.indices as *mut c_void);
            d.indices = std::ptr::null_mut();
        }
    }
}

unsafe extern "system" fn draw_primitive_up(this: *mut Device, prim: u32, count: u32, data: *const u8, stride: u32) -> HRESULT {
    let d = dev(this);
    if data.is_null() || stride == 0 {
        return D3DERR_INVALIDCALL;
    }
    if count == 0 {
        return D3D_OK;
    }
    let vertices = pipeline::vertex_count(prim, count);
    let bytes = unsafe { std::slice::from_raw_parts(data, (vertices * stride) as usize) };
    let slice = d.r.upload(bytes, 16);
    if d.prepare_draw(prim, Some((slice, stride))) {
        d.r.push(Cmd::Draw(vertices, 0));
    }
    clear_stream0_and_indices(d);
    D3D_OK
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn draw_indexed_primitive_up(
    this: *mut Device,
    prim: u32,
    min_vertex: u32,
    vertices: u32,
    count: u32,
    index_data: *const u8,
    index_format: u32,
    vertex_data: *const u8,
    stride: u32,
) -> HRESULT {
    let d = dev(this);
    if index_data.is_null() || vertex_data.is_null() || stride == 0 {
        return D3DERR_INVALIDCALL;
    }
    if count == 0 {
        return D3D_OK;
    }
    let index_count = pipeline::vertex_count(prim, count);
    let index_size = if index_format == format::INDEX32 { 4 } else { 2 };
    let vertex_bytes = unsafe { std::slice::from_raw_parts(vertex_data, ((min_vertex + vertices) * stride) as usize) };
    let index_bytes = unsafe { std::slice::from_raw_parts(index_data, (index_count * index_size) as usize) };
    let vslice = d.r.upload(vertex_bytes, 16);
    let islice = d.r.upload(index_bytes, 16);
    if d.prepare_draw(prim, Some((vslice, stride))) {
        let ty = if index_size == 4 { vk::IndexType::UINT32 } else { vk::IndexType::UINT16 };
        d.r.push(Cmd::Indices(islice.buffer, islice.offset, ty));
        d.r.push(Cmd::DrawIndexed(index_count, 0, 0));
    }
    clear_stream0_and_indices(d);
    D3D_OK
}

unsafe extern "system" fn process_vertices(this: *mut Device, _a: u32, _b: u32, _c: u32, _d: u32, _e: u32) -> HRESULT {
    dev(this).warn_once("ProcessVertices is not supported");
    D3DERR_INVALIDCALL
}

/// Reads a vertex shader declaration from the game's memory.
unsafe fn read_declaration(p: *const u32) -> Vec<u32> {
    let mut tokens = Vec::new();
    let mut i = 0;
    loop {
        let t = unsafe { *p.add(i) };
        tokens.push(t);
        i += 1;
        if t == 0xFFFF_FFFF || i > 4096 {
            return tokens;
        }
        match t >> 29 {
            4 => {
                for _ in 0..((t >> 25) & 0xF) * 4 {
                    tokens.push(unsafe { *p.add(i) });
                    i += 1;
                }
            }
            5 => {
                for _ in 0..(t >> 24) & 0x1F {
                    tokens.push(unsafe { *p.add(i) });
                    i += 1;
                }
            }
            _ => {}
        }
    }
}

/// Reads a shader function from the game's memory, up to its end token.
unsafe fn read_function(p: *const u32) -> Vec<u32> {
    let mut tokens = vec![unsafe { *p }];
    let mut i = 1;
    loop {
        let t = unsafe { *p.add(i) };
        tokens.push(t);
        i += 1;
        if t == 0x0000_FFFF || i > 65536 {
            return tokens;
        }
        if t & 0xFFFF == 0xFFFE {
            for _ in 0..(t >> 16) & 0x7FFF {
                tokens.push(unsafe { *p.add(i) });
                i += 1;
            }
        }
    }
}

/// D3D8's fixed function input registers mapped to the fixed function shader's locations.
fn fixed_function_layout(decl: &Declaration) -> (pipeline::VertexLayout, VertexInfo) {
    let mut info = VertexInfo { tex_dims: [2; 8], ..Default::default() };
    let mut renamed = Declaration::default();
    for e in &decl.elements {
        let reg = match e.reg {
            0 => pipeline::LOC_POSITION,
            3 => pipeline::LOC_NORMAL,
            5 => {
                info.diffuse = true;
                pipeline::LOC_DIFFUSE
            }
            6 => {
                info.specular = true;
                pipeline::LOC_SPECULAR
            }
            r @ 7..=14 => {
                info.tex_dims[(r - 7) as usize] = match e.ty {
                    sm1::VSDT_FLOAT1 => 1,
                    sm1::VSDT_FLOAT3 => 3,
                    sm1::VSDT_FLOAT4 => 4,
                    _ => 2,
                };
                pipeline::LOC_TEX0 + r - 7
            }
            _ => continue,
        };
        renamed.elements.push(sm1::Element { reg, ..*e });
    }
    let mut layout = pipeline::declaration_layout(&renamed);
    let fill = pipeline::fvf_layout(0).0;
    for a in fill.attributes {
        if !layout.attributes.iter().any(|b| b.location == a.location) {
            layout.attributes.push(a);
        }
    }
    layout.attributes.sort_by_key(|a| a.location);
    let mut bindings: Vec<u32> = layout.attributes.iter().map(|a| a.binding).collect();
    bindings.sort();
    bindings.dedup();
    layout.bindings = bindings;
    (layout, info)
}

unsafe extern "system" fn create_vertex_shader(this: *mut Device, decl: *const u32, function: *const u32, handle: *mut u32, _usage: u32) -> HRESULT {
    let d = dev(this);
    if decl.is_null() || handle.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let decl_tokens = unsafe { read_declaration(decl) };
    let parsed = match sm1::parse_declaration(&decl_tokens) {
        Ok((p, _)) => p,
        Err(e) => {
            log!("vertex shader declaration rejected: {e}");
            return D3DERR_INVALIDCALL;
        }
    };
    let (function_tokens, module, layout, info) = if function.is_null() {
        let (layout, info) = fixed_function_layout(&parsed);
        (Vec::new(), MODULE_FF_VS, d.pipelines.layout_id(&layout), info)
    } else {
        let tokens = unsafe { read_function(function) };
        let source = match sm1::translate_vertex(&tokens, &parsed) {
            Ok(s) => s,
            Err(e) => {
                log!("vertex shader could not be translated: {e}");
                return D3DERR_INVALIDCALL;
            }
        };
        let spirv = match shader::compile(&source, Stage::Vertex) {
            Ok(s) => s,
            Err(e) => {
                log!("vertex shader could not be compiled: {e}\n{source}");
                return D3DERR_INVALIDCALL;
            }
        };
        let layout = pipeline::declaration_layout(&parsed);
        let module = d.pipelines.add_module(&spirv);
        let mut pairs = vec![(module, MODULE_FF_PS)];
        pairs.extend(d.pixel_shaders.iter().flatten().map(|p| (module, p.module)));
        d.pipelines.prebuild(&pairs);
        (tokens, module, d.pipelines.layout_id(&layout), VertexInfo::default())
    };
    d.vertex_shaders.push(Some(VertexShader { decl: parsed, decl_tokens, function: function_tokens, module, layout, info }));
    let h = ((d.vertex_shaders.len() as u32) << 1) | 1;
    log!("vertex shader {h:#x} created");
    unsafe { *handle = h };
    D3D_OK
}

unsafe extern "system" fn set_vertex_shader(this: *mut Device, handle: u32) -> HRESULT {
    let d = dev(this);
    // An FVF only feeds the uniforms through its vertex info, which prepare_draw compares itself.
    // WW3D sets the FVF for every mesh, and marking every one dirty rebuilt the vertex block each time.
    if handle & 1 != 0 {
        let Some(index) = (handle as usize >> 1).checked_sub(1).filter(|&i| d.vertex_shaders.get(i).is_some_and(|v| v.is_some())) else {
            return D3DERR_INVALIDCALL;
        };
        let constants = &d.vertex_shaders[index].as_ref().unwrap().decl.constants;
        if !constants.is_empty() {
            for &(reg, value) in constants {
                if let Some(c) = d.s.vs_const.get_mut(reg as usize) {
                    *c = value;
                }
            }
            d.vs_dirty = true;
        }
    }
    d.vs = handle;
    D3D_OK
}

unsafe extern "system" fn delete_vertex_shader(this: *mut Device, handle: u32) -> HRESULT {
    let d = dev(this);
    let Some(index) = (handle >> 1).checked_sub(1) else { return D3DERR_INVALIDCALL };
    match d.vertex_shaders.get_mut(index as usize) {
        Some(slot @ Some(_)) => {
            *slot = None;
            if d.vs == handle {
                d.vs = 0;
            }
            D3D_OK
        }
        _ => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn set_vertex_shader_constant(this: *mut Device, reg: u32, data: *const [f32; 4], count: u32) -> HRESULT {
    let d = dev(this);
    if data.is_null() || reg + count > 96 {
        return D3DERR_INVALIDCALL;
    }
    for i in 0..count {
        d.s.vs_const[(reg + i) as usize] = unsafe { *data.add(i as usize) };
    }
    d.vs_dirty = true;
    D3D_OK
}

unsafe fn copy_tokens(tokens: &[u32], data: *mut c_void, size: *mut u32) -> HRESULT {
    if size.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let bytes = (tokens.len() * 4) as u32;
    let available = unsafe { *size };
    unsafe { *size = bytes };
    if data.is_null() {
        return D3D_OK;
    }
    if available < bytes {
        return com::D3DERR_MOREDATA;
    }
    unsafe { std::ptr::copy_nonoverlapping(tokens.as_ptr() as *const u8, data as *mut u8, bytes as usize) };
    D3D_OK
}

unsafe extern "system" fn get_vertex_shader_declaration(this: *mut Device, handle: u32, data: *mut c_void, size: *mut u32) -> HRESULT {
    match dev(this).vertex_shader(handle) {
        Some(v) => unsafe { copy_tokens(&v.decl_tokens, data, size) },
        None => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn get_vertex_shader_function(this: *mut Device, handle: u32, data: *mut c_void, size: *mut u32) -> HRESULT {
    match dev(this).vertex_shader(handle) {
        Some(v) => unsafe { copy_tokens(&v.function, data, size) },
        None => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn set_stream_source(this: *mut Device, stream: u32, vb: *mut c_void, stride: u32) -> HRESULT {
    let d = dev(this);
    if stream >= 16 {
        return D3DERR_INVALIDCALL;
    }
    let new = if vb.is_null() {
        std::ptr::null_mut()
    } else if unsafe { buffer::from_ptr(vb) }.is_some_and(|b| !b.index) {
        vb as *mut Buffer
    } else {
        return D3DERR_INVALIDCALL;
    };
    let (old, _) = d.streams[stream as usize];
    unsafe {
        if !new.is_null() {
            com::add_ref(new as *mut c_void);
        }
        if !old.is_null() {
            com::release(old as *mut c_void);
        }
    }
    d.streams[stream as usize] = (new, stride);
    D3D_OK
}

unsafe extern "system" fn set_indices(this: *mut Device, ib: *mut c_void, base_vertex: u32) -> HRESULT {
    let d = dev(this);
    let new = if ib.is_null() {
        std::ptr::null_mut()
    } else if unsafe { buffer::from_ptr(ib) }.is_some_and(|b| b.index) {
        ib as *mut Buffer
    } else {
        return D3DERR_INVALIDCALL;
    };
    unsafe {
        if !new.is_null() {
            com::add_ref(new as *mut c_void);
        }
        if !d.indices.is_null() {
            com::release(d.indices as *mut c_void);
        }
    }
    d.indices = new;
    d.base_vertex = base_vertex;
    D3D_OK
}

unsafe extern "system" fn create_pixel_shader(this: *mut Device, function: *const u32, handle: *mut u32) -> HRESULT {
    let d = dev(this);
    if function.is_null() || handle.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let tokens = unsafe { read_function(function) };
    let source = match sm1::translate_pixel(&tokens) {
        Ok((s, _)) => s,
        Err(e) => {
            log!("pixel shader could not be translated: {e}");
            return D3DERR_INVALIDCALL;
        }
    };
    let spirv = match shader::compile(&source, Stage::Fragment) {
        Ok(s) => s,
        Err(e) => {
            log!("pixel shader could not be compiled: {e}\n{source}");
            return D3DERR_INVALIDCALL;
        }
    };
    let module = d.pipelines.add_module(&spirv);
    let mut pairs = vec![(MODULE_FF_VS, module)];
    pairs.extend(d.vertex_shaders.iter().flatten().filter(|v| v.module != MODULE_FF_VS).map(|v| (v.module, module)));
    d.pipelines.prebuild(&pairs);
    d.pixel_shaders.push(Some(PixelShader { function: tokens, module }));
    unsafe { *handle = d.pixel_shaders.len() as u32 };
    D3D_OK
}

unsafe extern "system" fn set_pixel_shader(this: *mut Device, handle: u32) -> HRESULT {
    let d = dev(this);
    if handle != 0 && d.pixel_shader(handle).is_none() {
        return D3DERR_INVALIDCALL;
    }
    d.ps = handle;
    d.ps_dirty = true;
    D3D_OK
}

unsafe extern "system" fn delete_pixel_shader(this: *mut Device, handle: u32) -> HRESULT {
    let d = dev(this);
    match handle.checked_sub(1).and_then(|i| d.pixel_shaders.get_mut(i as usize)) {
        Some(slot @ Some(_)) => {
            *slot = None;
            if d.ps == handle {
                d.ps = 0;
            }
            D3D_OK
        }
        _ => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn set_pixel_shader_constant(this: *mut Device, reg: u32, data: *const [f32; 4], count: u32) -> HRESULT {
    let d = dev(this);
    if data.is_null() || reg + count > 8 {
        return D3DERR_INVALIDCALL;
    }
    for i in 0..count {
        d.s.ps_const[(reg + i) as usize] = unsafe { *data.add(i as usize) };
    }
    d.ps_dirty = true;
    D3D_OK
}

unsafe extern "system" fn get_pixel_shader_function(this: *mut Device, handle: u32, data: *mut c_void, size: *mut u32) -> HRESULT {
    match dev(this).pixel_shader(handle) {
        Some(p) => unsafe { copy_tokens(&p.function, data, size) },
        None => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn patch3(this: *mut Device, _a: u32, _b: u32, _c: u32) -> HRESULT {
    dev(this).warn_once("patches are not supported");
    D3DERR_INVALIDCALL
}

unsafe extern "system" fn delete_patch(_this: *mut Device, _handle: u32) -> HRESULT {
    D3DERR_INVALIDCALL
}

impl Drop for Device {
    fn drop(&mut self) {
        if let Some(a) = self.dummy.1.take() {
            context::retire(Garbage::Buffer(self.dummy.0, a));
        }
    }
}

static DEVICE_VTBL: Vtbl<97> = vtbl![
    on_worker::query_interface,
    on_worker::add_ref,
    on_worker::release,
    test_cooperative_level,
    get_available_texture_mem,
    resource_manager_discard_bytes,
    get_direct3d,
    get_device_caps,
    get_display_mode,
    get_creation_parameters,
    set_cursor_properties,
    set_cursor_position,
    show_cursor,
    on_worker::not_available2,
    on_worker::reset,
    on_worker::present,
    on_worker::get_back_buffer,
    get_raster_status,
    on_worker::set_gamma_ramp,
    on_worker::get_gamma_ramp,
    on_worker::create_texture,
    on_worker::not_available8,
    on_worker::not_available6,
    on_worker::create_vertex_buffer,
    on_worker::create_index_buffer,
    on_worker::create_render_target,
    on_worker::create_depth_stencil_surface,
    on_worker::create_image_surface,
    on_worker::copy_rects,
    on_worker::update_texture,
    on_worker::get_front_buffer,
    on_worker::set_render_target,
    on_worker::get_render_target,
    on_worker::get_depth_stencil_surface,
    on_worker::begin_scene,
    on_worker::end_scene,
    on_worker::clear,
    on_worker::set_transform,
    on_worker::get_transform,
    on_worker::multiply_transform,
    on_worker::set_viewport,
    on_worker::get_viewport,
    on_worker::set_material,
    on_worker::get_material,
    on_worker::set_light,
    on_worker::get_light,
    on_worker::light_enable,
    on_worker::get_light_enable,
    on_worker::set_clip_plane,
    on_worker::get_clip_plane,
    on_worker::set_render_state,
    on_worker::get_render_state,
    on_worker::begin_state_block,
    on_worker::end_state_block,
    on_worker::apply_state_block,
    on_worker::capture_state_block,
    on_worker::delete_state_block,
    on_worker::create_state_block,
    on_worker::set_clip_status,
    on_worker::get_clip_status,
    on_worker::get_texture,
    on_worker::set_texture,
    on_worker::get_texture_stage_state,
    on_worker::set_texture_stage_state,
    validate_device,
    get_info,
    set_palette_entries,
    get_palette_entries,
    on_worker::set_current_texture_palette,
    on_worker::get_current_texture_palette,
    on_worker::draw_primitive,
    on_worker::draw_indexed_primitive,
    on_worker::draw_primitive_up,
    on_worker::draw_indexed_primitive_up,
    on_worker::process_vertices,
    on_worker::create_vertex_shader,
    on_worker::set_vertex_shader,
    on_worker::get_vertex_shader,
    on_worker::delete_vertex_shader,
    on_worker::set_vertex_shader_constant,
    on_worker::get_vertex_shader_constant,
    on_worker::get_vertex_shader_declaration,
    on_worker::get_vertex_shader_function,
    on_worker::set_stream_source,
    on_worker::get_stream_source,
    on_worker::set_indices,
    on_worker::get_indices,
    on_worker::create_pixel_shader,
    on_worker::set_pixel_shader,
    on_worker::get_pixel_shader,
    on_worker::delete_pixel_shader,
    on_worker::set_pixel_shader_constant,
    on_worker::get_pixel_shader_constant,
    on_worker::get_pixel_shader_function,
    on_worker::patch3,
    on_worker::patch3,
    on_worker::delete_patch,
];

/// The vtable's methods: each runs on the renderer thread while the caller waits (vk/worker.rs).
mod on_worker {
    use super::*;
    use crate::vk::worker;

    /// The window is arranged on the game thread: window calls from the renderer thread would wait
    /// for the game thread's message loop while it waits for them.
    pub unsafe extern "system" fn reset(this: *mut Device, pp: *mut D3DPRESENT_PARAMETERS) -> HRESULT {
        if pp.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let requested = unsafe { *pp };
        let (system_d3d, adapter, focus) = worker::run(|| {
            super::reset_begin(this, &requested);
            let d = dev(this);
            (d.system_d3d, d.adapter, d.focus)
        });
        let fullscreen = unsafe { arrange_window(system_d3d, adapter, focus, &requested) };
        let result = worker::run(|| super::reset_finish(this, requested, fullscreen));
        refresh_front(this);
        result
    }

    /// The game thread's parts of Present (fog, Discord, stats, the frame clock, pacing) stay on it.
    pub unsafe extern "system" fn present(this: *mut Device, _src: *const c_void, _dst: *const c_void, _window: HWND, _dirty: *const c_void) -> HRESULT {
        crate::fog::mark_world_fog();
        crate::discord::on_frame();
        send_scene(this);
        // Queued: the game thread goes on with the next frame while this one is prepared, at most
        // one frame ahead.
        let device = worker::Ptr(this);
        worker::present(move || {
            let device = device;
            super::present_frame(device.0);
        });
        stats::frame();
        frame::on_present();
        // Waiting after the present, not before, lets the next frame read newer input.
        let start = std::time::Instant::now();
        pacing::wait();
        stats::add(&stats::PACING_US, start.elapsed().as_micros() as u64);
        D3D_OK
    }

    pub unsafe extern "system" fn query_interface(this: *mut Device, _iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
        crate::vk::worker::run_as("device::query_interface", || unsafe { super::query_interface(this, _iid, out) })
    }

    pub unsafe extern "system" fn add_ref(this: *mut Device) -> u32 {
        crate::vk::worker::run_as("device::add_ref", || unsafe { super::add_ref(this) })
    }

    /// The display mode is restored and our IDirect3D8 released on the game thread: a mode change
    /// sends messages to its window.
    pub unsafe extern "system" fn release(this: *mut Device) -> u32 {
        let d3d = worker::run(|| dev(this).d3d);
        let n = crate::vk::worker::run_as("device::release", || unsafe { super::release(this) });
        if n == 0 {
            let _ = CURRENT.compare_exchange(this, std::ptr::null_mut(), Ordering::AcqRel, Ordering::Relaxed);
            display::leave();
            unsafe { com::release(d3d) };
        }
        n
    }


    pub unsafe extern "system" fn not_available2(this: *mut Device, _a: u32, _b: u32) -> HRESULT {
        crate::vk::worker::run_as("device::not_available2", || unsafe { super::not_available2(this, _a, _b) })
    }

    pub unsafe extern "system" fn get_back_buffer(this: *mut Device, _index: u32, _ty: u32, out: *mut *mut Surface) -> HRESULT {
        crate::vk::worker::run_as("device::get_back_buffer", || unsafe { super::get_back_buffer(this, _index, _ty, out) })
    }

    pub unsafe extern "system" fn set_gamma_ramp(this: *mut Device, _flags: u32, ramp: *const [u16; 768]) {
        crate::vk::worker::run_as("device::set_gamma_ramp", || unsafe { super::set_gamma_ramp(this, _flags, ramp) })
    }

    pub unsafe extern "system" fn get_gamma_ramp(this: *mut Device, ramp: *mut [u16; 768]) {
        crate::vk::worker::run_as("device::get_gamma_ramp", || unsafe { super::get_gamma_ramp(this, ramp) })
    }

    pub unsafe extern "system" fn create_texture(this: *mut Device, width: u32, height: u32, levels: u32, usage: u32, format: u32, pool: u32, out: *mut *mut Texture) -> HRESULT {
        crate::vk::worker::run_as("device::create_texture", || unsafe { super::create_texture(this, width, height, levels, usage, format, pool, out) })
    }

    pub unsafe extern "system" fn not_available8(this: *mut Device, _a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32, _g: u32, _h: u32) -> HRESULT {
        crate::vk::worker::run_as("device::not_available8", || unsafe { super::not_available8(this, _a, _b, _c, _d, _e, _f, _g, _h) })
    }

    pub unsafe extern "system" fn not_available6(this: *mut Device, _a: u32, _b: u32, _c: u32, _d: u32, _e: u32, _f: u32) -> HRESULT {
        crate::vk::worker::run_as("device::not_available6", || unsafe { super::not_available6(this, _a, _b, _c, _d, _e, _f) })
    }

    pub unsafe extern "system" fn create_vertex_buffer(this: *mut Device, length: u32, usage: u32, fvf: u32, pool: u32, out: *mut *mut Buffer) -> HRESULT {
        crate::vk::worker::run_as("device::create_vertex_buffer", || unsafe { super::create_vertex_buffer(this, length, usage, fvf, pool, out) })
    }

    pub unsafe extern "system" fn create_index_buffer(this: *mut Device, length: u32, usage: u32, format: u32, pool: u32, out: *mut *mut Buffer) -> HRESULT {
        crate::vk::worker::run_as("device::create_index_buffer", || unsafe { super::create_index_buffer(this, length, usage, format, pool, out) })
    }

    pub unsafe extern "system" fn create_render_target(this: *mut Device, width: u32, height: u32, format: u32, _multisample: u32, _lockable: i32, out: *mut *mut Surface) -> HRESULT {
        crate::vk::worker::run_as("device::create_render_target", || unsafe { super::create_render_target(this, width, height, format, _multisample, _lockable, out) })
    }

    pub unsafe extern "system" fn create_depth_stencil_surface(this: *mut Device, width: u32, height: u32, format: u32, _multisample: u32, out: *mut *mut Surface) -> HRESULT {
        crate::vk::worker::run_as("device::create_depth_stencil_surface", || unsafe { super::create_depth_stencil_surface(this, width, height, format, _multisample, out) })
    }

    pub unsafe extern "system" fn create_image_surface(this: *mut Device, width: u32, height: u32, format: u32, out: *mut *mut Surface) -> HRESULT {
        crate::vk::worker::run_as("device::create_image_surface", || unsafe { super::create_image_surface(this, width, height, format, out) })
    }


    pub unsafe extern "system" fn update_texture(this: *mut Device, src: *mut c_void, dst: *mut c_void) -> HRESULT {
        crate::vk::worker::run_as("device::update_texture", || unsafe { super::update_texture(this, src, dst) })
    }

    pub unsafe extern "system" fn get_front_buffer(this: *mut Device, dst: *mut c_void) -> HRESULT {
        crate::vk::worker::run_as("device::get_front_buffer", || unsafe { super::get_front_buffer(this, dst) })
    }

    pub unsafe extern "system" fn begin_state_block(this: *mut Device) -> HRESULT {
        crate::vk::worker::run_as("device::begin_state_block", || unsafe { super::begin_state_block(this) })
    }

    pub unsafe extern "system" fn end_state_block(this: *mut Device, out: *mut u32) -> HRESULT {
        crate::vk::worker::run_as("device::end_state_block", || unsafe { super::end_state_block(this, out) })
    }

    pub unsafe extern "system" fn capture_state_block(this: *mut Device, token: u32) -> HRESULT {
        crate::vk::worker::run_as("device::capture_state_block", || unsafe { super::capture_state_block(this, token) })
    }

    pub unsafe extern "system" fn delete_state_block(this: *mut Device, token: u32) -> HRESULT {
        crate::vk::worker::run_as("device::delete_state_block", || unsafe { super::delete_state_block(this, token) })
    }

    pub unsafe extern "system" fn create_state_block(this: *mut Device, _ty: u32, out: *mut u32) -> HRESULT {
        crate::vk::worker::run_as("device::create_state_block", || unsafe { super::create_state_block(this, _ty, out) })
    }

    pub unsafe extern "system" fn process_vertices(this: *mut Device, _a: u32, _b: u32, _c: u32, _d: u32, _e: u32) -> HRESULT {
        crate::vk::worker::run_as("device::process_vertices", || unsafe { super::process_vertices(this, _a, _b, _c, _d, _e) })
    }

    pub unsafe extern "system" fn get_vertex_shader_declaration(this: *mut Device, handle: u32, data: *mut c_void, size: *mut u32) -> HRESULT {
        crate::vk::worker::run_as("device::get_vertex_shader_declaration", || unsafe { super::get_vertex_shader_declaration(this, handle, data, size) })
    }

    pub unsafe extern "system" fn get_vertex_shader_function(this: *mut Device, handle: u32, data: *mut c_void, size: *mut u32) -> HRESULT {
        crate::vk::worker::run_as("device::get_vertex_shader_function", || unsafe { super::get_vertex_shader_function(this, handle, data, size) })
    }

    pub unsafe extern "system" fn get_pixel_shader_function(this: *mut Device, handle: u32, data: *mut c_void, size: *mut u32) -> HRESULT {
        crate::vk::worker::run_as("device::get_pixel_shader_function", || unsafe { super::get_pixel_shader_function(this, handle, data, size) })
    }

    pub unsafe extern "system" fn patch3(this: *mut Device, _a: u32, _b: u32, _c: u32) -> HRESULT {
        crate::vk::worker::run_as("device::patch3", || unsafe { super::patch3(this, _a, _b, _c) })
    }

    pub unsafe extern "system" fn delete_patch(_this: *mut Device, _handle: u32) -> HRESULT {
        crate::vk::worker::run_as("device::delete_patch", || unsafe { super::delete_patch(_this, _handle) })
    }
    // Calls the game makes many times a frame: validated and mirrored in `Front` here, then queued
    // for the renderer thread without waiting.

    fn front<'a>(this: *mut Device) -> &'a mut Front {
        unsafe { &mut *(*this).front }
    }

    /// Runs `f` with the device on the renderer thread, after everything queued before it.
    fn queue(this: *mut Device, f: impl FnOnce(&mut Device) + Send + 'static) {
        let device = worker::Ptr(this);
        worker::post(move || {
            let device = device;
            f(dev(device.0))
        });
    }

    /// An object a queued call names, kept alive until the call ran.
    #[derive(Clone, Copy)]
    struct Held(*mut c_void);

    unsafe impl Send for Held {}

    impl Held {
        fn new(p: *mut c_void) -> Held {
            if !p.is_null() {
                unsafe { com::add_ref(p) };
            }
            Held(p)
        }

        fn release(self) {
            if !self.0.is_null() {
                unsafe { com::release(self.0) };
            }
        }
    }

    /// Sends the scene flags along when they changed, so the renderer thread sees them as they were
    /// when the game made the call.
    fn send_scene(this: *mut Device) {
        let flags = (crate::scene::in_scene(), crate::scene::opaque_done(), crate::scene::in_sorted());
        let f = front(this);
        if f.scene != Some(flags) {
            f.scene = Some(flags);
            queue(this, move |d| d.scene_flags = flags);
        }
    }

    /// After a call that replaced much of the state on the renderer thread.
    fn refresh_front(this: *mut Device) {
        let snapshot = worker::run(|| Front::of(dev(this)));
        unsafe { *(*this).front = snapshot };
    }

    pub unsafe extern "system" fn begin_scene(_this: *mut Device) -> HRESULT {
        D3D_OK
    }

    pub unsafe extern "system" fn end_scene(_this: *mut Device) -> HRESULT {
        D3D_OK
    }

    pub unsafe extern "system" fn set_render_state(this: *mut Device, state: u32, value: u32) -> HRESULT {
        if state >= 256 {
            return D3DERR_INVALIDCALL;
        }
        front(this).s.rs[state as usize] = value;
        queue(this, move |d| unsafe {
            super::set_render_state(d, state, value);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_render_state(this: *mut Device, state: u32, out: *mut u32) -> HRESULT {
        if out.is_null() || state >= 256 {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *out = front(this).s.rs[state as usize] };
        D3D_OK
    }

    pub unsafe extern "system" fn set_texture_stage_state(this: *mut Device, stage: u32, ty: u32, value: u32) -> HRESULT {
        if stage >= 8 || ty >= 32 {
            return D3DERR_INVALIDCALL;
        }
        front(this).s.tss[stage as usize][ty as usize] = value;
        queue(this, move |d| unsafe {
            super::set_texture_stage_state(d, stage, ty, value);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_texture_stage_state(this: *mut Device, stage: u32, ty: u32, out: *mut u32) -> HRESULT {
        if out.is_null() || stage >= 8 || ty >= 32 {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *out = front(this).s.tss[stage as usize][ty as usize] };
        D3D_OK
    }

    pub unsafe extern "system" fn set_transform(this: *mut Device, state: u32, m: *const Matrix) -> HRESULT {
        if m.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let m = unsafe { *m };
        if let Some(slot) = transform_slot(&mut front(this).s, state) {
            *slot = m;
        }
        queue(this, move |d| store_transform(d, state, m));
        D3D_OK
    }

    pub unsafe extern "system" fn get_transform(this: *mut Device, state: u32, m: *mut Matrix) -> HRESULT {
        if m.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *m = transform_slot(&mut front(this).s, state).map(|s| *s).unwrap_or(Matrix::IDENTITY) };
        D3D_OK
    }

    pub unsafe extern "system" fn multiply_transform(this: *mut Device, state: u32, m: *const Matrix) -> HRESULT {
        if m.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let m = unsafe { *m };
        if let Some(slot) = transform_slot(&mut front(this).s, state) {
            let product = m.mul(slot);
            *slot = product;
            queue(this, move |d| store_transform(d, state, product));
        }
        D3D_OK
    }

    pub unsafe extern "system" fn set_viewport(this: *mut Device, vp: *const Viewport) -> HRESULT {
        if vp.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let vp = unsafe { *vp };
        front(this).s.viewport = vp;
        queue(this, move |d| unsafe {
            super::set_viewport(d, &vp);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_viewport(this: *mut Device, vp: *mut Viewport) -> HRESULT {
        if vp.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *vp = front(this).s.viewport };
        D3D_OK
    }

    pub unsafe extern "system" fn set_material(this: *mut Device, m: *const Material) -> HRESULT {
        if m.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let m = unsafe { *m };
        front(this).s.material = m;
        queue(this, move |d| unsafe {
            super::set_material(d, &m);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_material(this: *mut Device, m: *mut Material) -> HRESULT {
        if m.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *m = front(this).s.material };
        D3D_OK
    }

    pub unsafe extern "system" fn set_light(this: *mut Device, index: u32, light: *const Light) -> HRESULT {
        if light.is_null() || index > 4096 {
            return D3DERR_INVALIDCALL;
        }
        let light = unsafe { *light };
        let lights = &mut front(this).s.lights;
        if lights.len() <= index as usize {
            lights.resize(index as usize + 1, default_light());
        }
        lights[index as usize] = light;
        queue(this, move |d| unsafe {
            super::set_light(d, index, &light);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_light(this: *mut Device, index: u32, light: *mut Light) -> HRESULT {
        match front(this).s.lights.get(index as usize) {
            Some(l) if !light.is_null() => {
                unsafe { *light = *l };
                D3D_OK
            }
            _ => D3DERR_INVALIDCALL,
        }
    }

    pub unsafe extern "system" fn light_enable(this: *mut Device, index: u32, enable: i32) -> HRESULT {
        if index > 4096 {
            return D3DERR_INVALIDCALL;
        }
        let s = &mut front(this).s;
        if s.lights.len() <= index as usize {
            s.lights.resize(index as usize + 1, default_light());
        }
        let present = s.enabled_lights.contains(&index);
        if enable != 0 && !present {
            s.enabled_lights.push(index);
        } else if enable == 0 && present {
            s.enabled_lights.retain(|&i| i != index);
        }
        queue(this, move |d| unsafe {
            super::light_enable(d, index, enable);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_light_enable(this: *mut Device, index: u32, out: *mut i32) -> HRESULT {
        if out.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *out = front(this).s.enabled_lights.contains(&index) as i32 };
        D3D_OK
    }

    pub unsafe extern "system" fn set_clip_plane(this: *mut Device, index: u32, plane: *const [f32; 4]) -> HRESULT {
        if plane.is_null() || index >= 32 {
            return D3DERR_INVALIDCALL;
        }
        let plane = unsafe { *plane };
        front(this).s.clip_planes[index as usize] = plane;
        queue(this, move |d| {
            let slot = &mut d.s.clip_planes[index as usize];
            if *slot != plane {
                *slot = plane;
                d.vs_dirty = true;
            }
        });
        D3D_OK
    }

    pub fn with_clip_planes(this: *mut Device, planes: &[[f32; 4]], f: impl FnOnce()) {
        let enabled = front(this).s.rs[state::RS_CLIPPLANEENABLE];
        let saved = front(this).s.clip_planes;
        let mask = (1u32 << planes.len()) - 1;
        unsafe {
            for (i, p) in planes.iter().enumerate() {
                set_clip_plane(this, i as u32, p);
            }
            set_render_state(this, state::RS_CLIPPLANEENABLE as u32, mask);
        }
        f();
        unsafe {
            set_render_state(this, state::RS_CLIPPLANEENABLE as u32, enabled);
            for (i, p) in saved.iter().enumerate().take(planes.len()) {
                set_clip_plane(this, i as u32, p);
            }
        }
    }

    pub unsafe extern "system" fn get_clip_plane(this: *mut Device, index: u32, plane: *mut [f32; 4]) -> HRESULT {
        if plane.is_null() || index >= 32 {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *plane = front(this).s.clip_planes[index as usize] };
        D3D_OK
    }

    pub unsafe extern "system" fn set_clip_status(this: *mut Device, status: *const [u32; 2]) -> HRESULT {
        if status.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let status = unsafe { *status };
        front(this).clip_status = status;
        queue(this, move |d| d.clip_status = status);
        D3D_OK
    }

    pub unsafe extern "system" fn get_clip_status(this: *mut Device, status: *mut [u32; 2]) -> HRESULT {
        if status.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *status = front(this).clip_status };
        D3D_OK
    }

    pub unsafe extern "system" fn set_current_texture_palette(this: *mut Device, index: u32) -> HRESULT {
        front(this).palette = index;
        queue(this, move |d| d.palette = index);
        D3D_OK
    }

    pub unsafe extern "system" fn get_current_texture_palette(this: *mut Device, out: *mut u32) -> HRESULT {
        if out.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *out = front(this).palette };
        D3D_OK
    }

    pub unsafe extern "system" fn apply_state_block(this: *mut Device, token: u32) -> HRESULT {
        let result = worker::run(|| unsafe { super::apply_state_block(this, token) });
        if result == D3D_OK {
            refresh_front(this);
        }
        result
    }

    pub unsafe extern "system" fn set_texture(this: *mut Device, stage: u32, texture: *mut c_void) -> HRESULT {
        if stage >= 8 || (!texture.is_null() && unsafe { texture::texture_from_ptr(texture) }.is_none()) {
            return D3DERR_INVALIDCALL;
        }
        let f = front(this);
        if f.textures[stage as usize] == texture {
            return D3D_OK;
        }
        f.textures[stage as usize] = texture;
        let held = Held::new(texture);
        queue(this, move |d| unsafe {
            super::set_texture(d, stage, held.0);
            held.release();
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_texture(this: *mut Device, stage: u32, out: *mut *mut c_void) -> HRESULT {
        if out.is_null() || stage >= 8 {
            return D3DERR_INVALIDCALL;
        }
        let t = front(this).textures[stage as usize];
        unsafe {
            if !t.is_null() {
                com::add_ref(t);
            }
            *out = t;
        }
        D3D_OK
    }

    pub unsafe extern "system" fn set_stream_source(this: *mut Device, stream: u32, vb: *mut c_void, stride: u32) -> HRESULT {
        if stream >= 16 || (!vb.is_null() && unsafe { buffer::kind(vb) } != Some(false)) {
            return D3DERR_INVALIDCALL;
        }
        front(this).streams[stream as usize] = (vb, stride);
        let held = Held::new(vb);
        queue(this, move |d| unsafe {
            super::set_stream_source(d, stream, held.0, stride);
            held.release();
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_stream_source(this: *mut Device, stream: u32, out: *mut *mut c_void, stride: *mut u32) -> HRESULT {
        if stream >= 16 || out.is_null() || stride.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let (b, s) = front(this).streams[stream as usize];
        unsafe {
            if !b.is_null() {
                com::add_ref(b);
            }
            *out = b;
            *stride = s;
        }
        D3D_OK
    }

    pub unsafe extern "system" fn set_indices(this: *mut Device, ib: *mut c_void, base_vertex: u32) -> HRESULT {
        if !ib.is_null() && unsafe { buffer::kind(ib) } != Some(true) {
            return D3DERR_INVALIDCALL;
        }
        front(this).indices = (ib, base_vertex);
        let held = Held::new(ib);
        queue(this, move |d| unsafe {
            super::set_indices(d, held.0, base_vertex);
            held.release();
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_indices(this: *mut Device, out: *mut *mut c_void, base_vertex: *mut u32) -> HRESULT {
        if out.is_null() || base_vertex.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let (ib, base) = front(this).indices;
        unsafe {
            if !ib.is_null() {
                com::add_ref(ib);
            }
            *out = ib;
            *base_vertex = base;
        }
        D3D_OK
    }

    pub unsafe extern "system" fn create_vertex_shader(this: *mut Device, decl: *const u32, function: *const u32, handle: *mut u32, usage: u32) -> HRESULT {
        let result = worker::run(|| unsafe { super::create_vertex_shader(this, decl, function, handle, usage) });
        if result == D3D_OK {
            let h = unsafe { *handle };
            let constants = worker::run(|| dev(this).vertex_shader(h).map(|v| v.decl.constants.clone()).unwrap_or_default());
            front(this).vertex_shaders.insert(h, constants);
        }
        result
    }

    pub unsafe extern "system" fn delete_vertex_shader(this: *mut Device, handle: u32) -> HRESULT {
        let result = worker::run(|| unsafe { super::delete_vertex_shader(this, handle) });
        if result == D3D_OK {
            let f = front(this);
            f.vertex_shaders.remove(&handle);
            if f.vs == handle {
                f.vs = 0;
            }
        }
        result
    }

    pub unsafe extern "system" fn set_vertex_shader(this: *mut Device, handle: u32) -> HRESULT {
        let f = front(this);
        if handle & 1 != 0 {
            let Some(constants) = f.vertex_shaders.get(&handle) else { return D3DERR_INVALIDCALL };
            for &(reg, value) in constants {
                if let Some(c) = f.s.vs_const.get_mut(reg as usize) {
                    *c = value;
                }
            }
        }
        f.vs = handle;
        queue(this, move |d| unsafe {
            super::set_vertex_shader(d, handle);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_vertex_shader(this: *mut Device, out: *mut u32) -> HRESULT {
        if out.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *out = front(this).vs };
        D3D_OK
    }

    pub unsafe extern "system" fn set_vertex_shader_constant(this: *mut Device, reg: u32, data: *const [f32; 4], count: u32) -> HRESULT {
        if data.is_null() || reg + count > 96 {
            return D3DERR_INVALIDCALL;
        }
        let values = unsafe { std::slice::from_raw_parts(data, count as usize) }.to_vec();
        front(this).s.vs_const[reg as usize..(reg + count) as usize].copy_from_slice(&values);
        queue(this, move |d| unsafe {
            super::set_vertex_shader_constant(d, reg, values.as_ptr(), count);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_vertex_shader_constant(this: *mut Device, reg: u32, data: *mut [f32; 4], count: u32) -> HRESULT {
        if data.is_null() || reg + count > 96 {
            return D3DERR_INVALIDCALL;
        }
        let f = front(this);
        for i in 0..count {
            unsafe { *data.add(i as usize) = f.s.vs_const[(reg + i) as usize] };
        }
        D3D_OK
    }

    pub unsafe extern "system" fn create_pixel_shader(this: *mut Device, function: *const u32, handle: *mut u32) -> HRESULT {
        let result = worker::run(|| unsafe { super::create_pixel_shader(this, function, handle) });
        if result == D3D_OK {
            front(this).pixel_shaders.insert(unsafe { *handle });
        }
        result
    }

    pub unsafe extern "system" fn delete_pixel_shader(this: *mut Device, handle: u32) -> HRESULT {
        let result = worker::run(|| unsafe { super::delete_pixel_shader(this, handle) });
        if result == D3D_OK {
            let f = front(this);
            f.pixel_shaders.remove(&handle);
            if f.ps == handle {
                f.ps = 0;
            }
        }
        result
    }

    pub unsafe extern "system" fn set_pixel_shader(this: *mut Device, handle: u32) -> HRESULT {
        let f = front(this);
        if handle != 0 && !f.pixel_shaders.contains(&handle) {
            return D3DERR_INVALIDCALL;
        }
        f.ps = handle;
        queue(this, move |d| unsafe {
            super::set_pixel_shader(d, handle);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_pixel_shader(this: *mut Device, out: *mut u32) -> HRESULT {
        if out.is_null() {
            return D3DERR_INVALIDCALL;
        }
        unsafe { *out = front(this).ps };
        D3D_OK
    }

    pub unsafe extern "system" fn set_pixel_shader_constant(this: *mut Device, reg: u32, data: *const [f32; 4], count: u32) -> HRESULT {
        if data.is_null() || reg + count > 8 {
            return D3DERR_INVALIDCALL;
        }
        let values = unsafe { std::slice::from_raw_parts(data, count as usize) }.to_vec();
        front(this).s.ps_const[reg as usize..(reg + count) as usize].copy_from_slice(&values);
        queue(this, move |d| unsafe {
            super::set_pixel_shader_constant(d, reg, values.as_ptr(), count);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_pixel_shader_constant(this: *mut Device, reg: u32, data: *mut [f32; 4], count: u32) -> HRESULT {
        if data.is_null() || reg + count > 8 {
            return D3DERR_INVALIDCALL;
        }
        let f = front(this);
        for i in 0..count {
            unsafe { *data.add(i as usize) = f.s.ps_const[(reg + i) as usize] };
        }
        D3D_OK
    }

    pub unsafe extern "system" fn set_render_target(this: *mut Device, rt: *mut c_void, ds: *mut c_void) -> HRESULT {
        // The checks only read what a surface never changes after it was made.
        let f = front(this);
        let new_rt = if rt.is_null() {
            f.rt
        } else {
            let Some(s) = (unsafe { texture::surface_from_ptr(rt) }) else { return D3DERR_INVALIDCALL };
            let data = unsafe { &(*s).data };
            if matches!(data.backing, Backing::Memory(_)) || data.info.kind == format::Kind::Depth || data.usage & texture::USAGE_RENDERTARGET == 0 {
                return D3DERR_INVALIDCALL;
            }
            s
        };
        let new_ds = if ds.is_null() {
            std::ptr::null_mut()
        } else {
            match unsafe { texture::surface_from_ptr(ds) } {
                Some(s) if unsafe { &(*s).data }.info.kind == format::Kind::Depth => s,
                _ => return D3DERR_INVALIDCALL,
            }
        };
        f.rt = new_rt;
        f.ds = new_ds;
        let target = unsafe { &(*new_rt).data };
        f.s.viewport = Viewport { x: 0, y: 0, width: target.width, height: target.height, min_z: 0.0, max_z: 1.0 };
        let (rt, ds) = (Held::new(rt), Held::new(ds));
        queue(this, move |d| unsafe {
            super::set_render_target(d, rt.0, ds.0);
            rt.release();
            ds.release();
        });
        D3D_OK
    }

    pub unsafe extern "system" fn get_render_target(this: *mut Device, out: *mut *mut Surface) -> HRESULT {
        if out.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let rt = front(this).rt;
        unsafe {
            texture::surf_add_ref(rt);
            *out = rt;
        }
        D3D_OK
    }

    pub unsafe extern "system" fn get_depth_stencil_surface(this: *mut Device, out: *mut *mut Surface) -> HRESULT {
        if out.is_null() {
            return D3DERR_INVALIDCALL;
        }
        let ds = front(this).ds;
        unsafe {
            *out = ds;
            if ds.is_null() {
                return D3DERR_NOTFOUND;
            }
            texture::surf_add_ref(ds);
        }
        D3D_OK
    }

    pub unsafe extern "system" fn clear(this: *mut Device, count: u32, rects: *const Rect, flags: u32, color: u32, z: f32, stencil: u32) -> HRESULT {
        send_scene(this);
        let rects = if rects.is_null() || count == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(rects, count as usize) }.to_vec() };
        queue(this, move |d| unsafe {
            let ptr = if rects.is_empty() { std::ptr::null() } else { rects.as_ptr() };
            super::clear(d, rects.len() as u32, ptr, flags, color, z, stencil);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn draw_primitive(this: *mut Device, prim: u32, start: u32, count: u32) -> HRESULT {
        send_scene(this);
        stats::add(&stats::DRAWS_TOTAL, 1);
        queue(this, move |d| unsafe {
            super::draw_primitive(d, prim, start, count);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn draw_indexed_primitive(this: *mut Device, prim: u32, min_index: u32, vertices: u32, start: u32, count: u32) -> HRESULT {
        send_scene(this);
        stats::add(&stats::DRAWS_TOTAL, 1);
        queue(this, move |d| unsafe {
            super::draw_indexed_primitive(d, prim, min_index, vertices, start, count);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn draw_primitive_up(this: *mut Device, prim: u32, count: u32, data: *const u8, stride: u32) -> HRESULT {
        if data.is_null() || stride == 0 {
            return D3DERR_INVALIDCALL;
        }
        send_scene(this);
        stats::add(&stats::DRAWS_TOTAL, 1);
        // DrawPrimitiveUP leaves stream 0 and the indices unset.
        let f = front(this);
        f.streams[0] = (std::ptr::null_mut(), 0);
        f.indices.0 = std::ptr::null_mut();
        let bytes = unsafe { std::slice::from_raw_parts(data, (pipeline::vertex_count(prim, count) * stride) as usize) }.to_vec();
        queue(this, move |d| unsafe {
            super::draw_primitive_up(d, prim, count, bytes.as_ptr(), stride);
        });
        D3D_OK
    }

    pub unsafe extern "system" fn draw_indexed_primitive_up(this: *mut Device, prim: u32, min_vertex: u32, vertices: u32, count: u32, index_data: *const u8, index_format: u32, vertex_data: *const u8, stride: u32) -> HRESULT {
        if index_data.is_null() || vertex_data.is_null() || stride == 0 {
            return D3DERR_INVALIDCALL;
        }
        send_scene(this);
        stats::add(&stats::DRAWS_TOTAL, 1);
        let f = front(this);
        f.streams[0] = (std::ptr::null_mut(), 0);
        f.indices.0 = std::ptr::null_mut();
        let index_size = if index_format == format::INDEX32 { 4 } else { 2 };
        let index_bytes = unsafe { std::slice::from_raw_parts(index_data, (pipeline::vertex_count(prim, count) * index_size) as usize) }.to_vec();
        let vertex_bytes = unsafe { std::slice::from_raw_parts(vertex_data, ((min_vertex + vertices) * stride) as usize) }.to_vec();
        queue(this, move |d| unsafe {
            super::draw_indexed_primitive_up(d, prim, min_vertex, vertices, count, index_bytes.as_ptr(), index_format, vertex_bytes.as_ptr(), stride);
        });
        D3D_OK
    }

    /// Queued: the checks only read what a surface never changes after it was made.
    pub unsafe extern "system" fn copy_rects(this: *mut Device, src: *mut c_void, rects: *const Rect, count: u32, dst: *mut c_void, points: *const Point) -> HRESULT {
        let (Some(s), Some(t)) = (unsafe { texture::surface_from_ptr(src) }, unsafe { texture::surface_from_ptr(dst) }) else {
            return D3DERR_INVALIDCALL;
        };
        if s == t || unsafe { &(*s).data }.info.d3d != unsafe { &(*t).data }.info.d3d {
            return D3DERR_INVALIDCALL;
        }
        let rects = if rects.is_null() || count == 0 { Vec::new() } else { unsafe { std::slice::from_raw_parts(rects, count as usize) }.to_vec() };
        let points = if points.is_null() || rects.is_empty() { Vec::new() } else { unsafe { std::slice::from_raw_parts(points, rects.len()) }.to_vec() };
        let (src, dst) = (Held::new(src), Held::new(dst));
        queue(this, move |d| unsafe {
            let rect_ptr = if rects.is_empty() { std::ptr::null() } else { rects.as_ptr() };
            let point_ptr = if points.is_empty() { std::ptr::null() } else { points.as_ptr() };
            super::copy_rects(d, src.0, rect_ptr, rects.len() as u32, dst.0, point_ptr);
            src.release();
            dst.release();
        });
        D3D_OK
    }
}