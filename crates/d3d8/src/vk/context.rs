//! The Vulkan instance, device and memory allocator, shared by everything in the backend. Objects the
//! game releases are destroyed only once the GPU has finished every submission that could use them.

use std::ffi::{CStr, c_char};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use ash::vk;
use gpu_allocator::MemoryLocation;
use gpu_allocator::vulkan::{Allocation, AllocationCreateDesc, AllocationScheme, Allocator, AllocatorCreateDesc};

use super::validation;
use crate::log;

pub struct Context {
    _entry: ash::Entry,
    _instance: ash::Instance,
    _validation: Option<validation::Validation>,
    pub surface_fn: ash::khr::surface::Instance,
    pub win32_surface_fn: ash::khr::win32_surface::Instance,
    pub physical: vk::PhysicalDevice,
    pub device: ash::Device,
    pub swapchain_fn: ash::khr::swapchain::Device,
    pub queue: vk::Queue,
    pub queue_family: u32,
    pub allocator: Mutex<Option<Allocator>>,
    pub limits: vk::PhysicalDeviceLimits,
    pub depth_format: vk::Format,
    pub anisotropy: bool,
    pub device_local_bytes: u64,
    /// Blending, write mask, fill mode and vertex layout as dynamic state, so pipelines only differ
    /// by shaders and attachments and can all be built up front.
    pub dynamic: Option<Dynamic>,
    /// Format of the 3D world's own target, and the sample counts it can be drawn with.
    pub scene_format: vk::Format,
    pub scene_sample_counts: vk::SampleCountFlags,
}

pub struct Dynamic {
    pub eds3: ash::ext::extended_dynamic_state3::Device,
    pub vertex_input: ash::ext::vertex_input_dynamic_state::Device,
}

static CONTEXT: OnceLock<Context> = OnceLock::new();

pub fn get() -> &'static Context {
    CONTEXT.get().expect("vulkan context used before it was created")
}

/// Creates the context once, on the GPU that matches the D3D adapter's vendor and device ids.
pub fn init(vendor: u32, device_id: u32) -> Result<&'static Context, String> {
    if let Some(c) = CONTEXT.get() {
        return Ok(c);
    }
    let context = unsafe { create(vendor, device_id)? };
    Ok(CONTEXT.get_or_init(|| context))
}

unsafe fn create(vendor: u32, device_id: u32) -> Result<Context, String> {
    let entry = unsafe { ash::Entry::load() }.map_err(|e| format!("vulkan-1.dll could not be loaded: {e}"))?;
    let version = unsafe { entry.try_enumerate_instance_version() }.ok().flatten().unwrap_or(vk::API_VERSION_1_0);
    if version < vk::API_VERSION_1_3 {
        return Err(format!("the Vulkan loader only offers {}, 1.3 is needed", version_text(version)));
    }
    let app_name = c"cnc-vulkan";
    let app = vk::ApplicationInfo::default()
        .application_name(app_name)
        .engine_name(app_name)
        .api_version(vk::API_VERSION_1_3);
    let validate = validation::requested(&entry);
    let mut instance_extensions = vec![ash::khr::surface::NAME.as_ptr(), ash::khr::win32_surface::NAME.as_ptr()];
    let mut layers = Vec::new();
    if validate {
        instance_extensions.push(ash::ext::debug_utils::NAME.as_ptr());
        layers.push(validation::LAYER.as_ptr());
    }
    let instance = unsafe {
        entry.create_instance(
            &vk::InstanceCreateInfo::default()
                .application_info(&app)
                .enabled_extension_names(&instance_extensions)
                .enabled_layer_names(&layers),
            None,
        )
    }
    .map_err(|e| format!("vkCreateInstance failed: {e}"))?;
    let validation = if validate { Some(validation::start(&entry, &instance)?) } else { None };
    let surface_fn = ash::khr::surface::Instance::new(&entry, &instance);
    let win32_surface_fn = ash::khr::win32_surface::Instance::new(&entry, &instance);

    let physicals = unsafe { instance.enumerate_physical_devices() }.map_err(|e| format!("no Vulkan devices: {e}"))?;
    let mut candidates = Vec::new();
    for &p in &physicals {
        let props = unsafe { instance.get_physical_device_properties(p) };
        let name = unsafe { CStr::from_ptr(props.device_name.as_ptr()) }.to_string_lossy().into_owned();
        let family = unsafe { instance.get_physical_device_queue_family_properties(p) }
            .iter()
            .enumerate()
            .position(|(i, f)| {
                f.queue_flags.contains(vk::QueueFlags::GRAPHICS)
                    && unsafe { win32_surface_fn.get_physical_device_win32_presentation_support(p, i as u32) }
            });
        log!(
            "vulkan device \"{name}\" api {} vendor {:#06x} device {:#06x} type {:?}{}",
            version_text(props.api_version),
            props.vendor_id,
            props.device_id,
            props.device_type,
            if family.is_none() { ", no queue that can present" } else { "" }
        );
        if props.api_version < vk::API_VERSION_1_3 {
            continue;
        }
        if let Some(family) = family {
            let rank = if props.vendor_id == vendor && props.device_id == device_id {
                0
            } else if props.device_type == vk::PhysicalDeviceType::DISCRETE_GPU {
                1
            } else {
                2
            };
            candidates.push((rank, p, family as u32, name, props));
        }
    }
    candidates.sort_by_key(|c| c.0);
    let Some((_, physical, queue_family, name, props)) = candidates.into_iter().next() else {
        return Err("no GPU with Vulkan 1.3 that can present to a window".into());
    };

    let extensions = unsafe { instance.enumerate_device_extension_properties(physical) }.unwrap_or_default();
    let has = |n: &CStr| extensions.iter().any(|e| unsafe { CStr::from_ptr(e.extension_name.as_ptr()) } == n);
    let mut v13 = vk::PhysicalDeviceVulkan13Features::default();
    let mut eds3 = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default();
    let mut vertex_input = vk::PhysicalDeviceVertexInputDynamicStateFeaturesEXT::default();
    let mut features2 = vk::PhysicalDeviceFeatures2::default().push_next(&mut v13).push_next(&mut eds3).push_next(&mut vertex_input);
    unsafe { instance.get_physical_device_features2(physical, &mut features2) };
    let supported = features2.features;
    let dynamic = has(ash::ext::extended_dynamic_state3::NAME)
        && has(ash::ext::vertex_input_dynamic_state::NAME)
        && eds3.extended_dynamic_state3_color_blend_enable == vk::TRUE
        && eds3.extended_dynamic_state3_color_blend_equation == vk::TRUE
        && eds3.extended_dynamic_state3_color_write_mask == vk::TRUE
        && eds3.extended_dynamic_state3_polygon_mode == vk::TRUE
        && vertex_input.vertex_input_dynamic_state == vk::TRUE;
    let dynamic_rendering = v13.dynamic_rendering == vk::TRUE;
    if !dynamic_rendering {
        return Err(format!("\"{name}\" does not support dynamic rendering"));
    }
    if supported.texture_compression_bc != vk::TRUE {
        return Err(format!("\"{name}\" does not support BC (DXT) textures"));
    }
    if supported.shader_clip_distance != vk::TRUE {
        return Err(format!("\"{name}\" does not support clip distances"));
    }

    let features = vk::PhysicalDeviceFeatures::default()
        .robust_buffer_access(supported.robust_buffer_access == vk::TRUE)
        .texture_compression_bc(true)
        .shader_clip_distance(true)
        .sampler_anisotropy(supported.sampler_anisotropy == vk::TRUE)
        .fill_mode_non_solid(supported.fill_mode_non_solid == vk::TRUE);
    let mut enable13 = vk::PhysicalDeviceVulkan13Features::default().dynamic_rendering(true);
    let priorities = [1.0f32];
    let queues = [vk::DeviceQueueCreateInfo::default().queue_family_index(queue_family).queue_priorities(&priorities)];
    let mut device_extensions: Vec<*const c_char> = vec![ash::khr::swapchain::NAME.as_ptr()];
    let mut enable_eds3 = vk::PhysicalDeviceExtendedDynamicState3FeaturesEXT::default()
        .extended_dynamic_state3_color_blend_enable(true)
        .extended_dynamic_state3_color_blend_equation(true)
        .extended_dynamic_state3_color_write_mask(true)
        .extended_dynamic_state3_polygon_mode(true);
    let mut enable_vertex_input = vk::PhysicalDeviceVertexInputDynamicStateFeaturesEXT::default().vertex_input_dynamic_state(true);
    let mut info = vk::DeviceCreateInfo::default().queue_create_infos(&queues).enabled_features(&features).push_next(&mut enable13);
    if dynamic {
        device_extensions.push(ash::ext::extended_dynamic_state3::NAME.as_ptr());
        device_extensions.push(ash::ext::vertex_input_dynamic_state::NAME.as_ptr());
        info = info.push_next(&mut enable_eds3).push_next(&mut enable_vertex_input);
    }
    let info = info.enabled_extension_names(&device_extensions);
    let device = unsafe { instance.create_device(physical, &info, None) }.map_err(|e| format!("vkCreateDevice failed: {e}"))?;
    let queue = unsafe { device.get_device_queue(queue_family, 0) };
    let swapchain_fn = ash::khr::swapchain::Device::new(&instance, &device);
    let dynamic = dynamic.then(|| Dynamic {
        eds3: ash::ext::extended_dynamic_state3::Device::new(&instance, &device),
        vertex_input: ash::ext::vertex_input_dynamic_state::Device::new(&instance, &device),
    });

    let allocator = Allocator::new(&AllocatorCreateDesc {
        instance: instance.clone(),
        device: device.clone(),
        physical_device: physical,
        debug_settings: Default::default(),
        buffer_device_address: false,
        allocation_sizes: Default::default(),
    })
    .map_err(|e| format!("memory allocator: {e}"))?;

    // Float depth, for the precision reversed depth gives it.
    let depth_format = [vk::Format::D32_SFLOAT_S8_UINT, vk::Format::D24_UNORM_S8_UINT]
        .into_iter()
        .find(|&f| {
            let p = unsafe { instance.get_physical_device_format_properties(physical, f) };
            p.optimal_tiling_features.contains(vk::FormatFeatureFlags::DEPTH_STENCIL_ATTACHMENT)
        })
        .ok_or("no depth and stencil format")?;

    // 16 bits per channel for the 3D world's own target where the GPU can blend into it. UNORM
    // clamps blends like the game's 8-bit back buffer, so only precision changes.
    let wide = vk::Format::R16G16B16A16_UNORM;
    let needed = vk::FormatFeatureFlags::COLOR_ATTACHMENT
        | vk::FormatFeatureFlags::COLOR_ATTACHMENT_BLEND
        | vk::FormatFeatureFlags::SAMPLED_IMAGE
        | vk::FormatFeatureFlags::SAMPLED_IMAGE_FILTER_LINEAR
        | vk::FormatFeatureFlags::BLIT_DST
        | vk::FormatFeatureFlags::TRANSFER_SRC
        | vk::FormatFeatureFlags::TRANSFER_DST;
    let wide_ok = unsafe { instance.get_physical_device_format_properties(physical, wide) }.optimal_tiling_features.contains(needed);
    let scene_format = if wide_ok { wide } else { vk::Format::B8G8R8A8_UNORM };
    let scene_sample_counts = unsafe {
        instance.get_physical_device_image_format_properties(
            physical,
            scene_format,
            vk::ImageType::TYPE_2D,
            vk::ImageTiling::OPTIMAL,
            vk::ImageUsageFlags::COLOR_ATTACHMENT | vk::ImageUsageFlags::TRANSFER_SRC | vk::ImageUsageFlags::TRANSFER_DST,
            vk::ImageCreateFlags::empty(),
        )
    }
    .map_or(vk::SampleCountFlags::TYPE_1, |p| p.sample_counts);

    let memory = unsafe { instance.get_physical_device_memory_properties(physical) };
    let device_local_bytes = memory.memory_heaps[..memory.memory_heap_count as usize]
        .iter()
        .filter(|h| h.flags.contains(vk::MemoryHeapFlags::DEVICE_LOCAL))
        .map(|h| h.size)
        .max()
        .unwrap_or(0);

    log!(
        "vulkan on \"{name}\", api {}, driver {:#x}, depth {depth_format:?}, 3D world {scene_format:?}, {} MB device memory, {}",
        version_text(props.api_version),
        props.driver_version,
        device_local_bytes >> 20,
        if dynamic.is_some() { "blending and vertex layout set dynamically" } else { "no dynamic blending or vertex layout" },
    );
    Ok(Context {
        _entry: entry,
        _instance: instance,
        _validation: validation,
        surface_fn,
        win32_surface_fn,
        physical,
        device,
        swapchain_fn,
        queue,
        queue_family,
        allocator: Mutex::new(Some(allocator)),
        limits: props.limits,
        depth_format,
        anisotropy: supported.sampler_anisotropy == vk::TRUE,
        device_local_bytes,
        dynamic,
        scene_format,
        scene_sample_counts,
    })
}

fn version_text(v: u32) -> String {
    format!("{}.{}.{}", vk::api_version_major(v), vk::api_version_minor(v), vk::api_version_patch(v))
}

impl Context {
    pub fn allocate(&self, name: &str, requirements: vk::MemoryRequirements, location: MemoryLocation, linear: bool) -> Allocation {
        let mut guard = self.allocator.lock().unwrap();
        let allocator = guard.as_mut().unwrap();
        match allocator.allocate(&AllocationCreateDesc {
            name,
            requirements,
            location,
            linear,
            allocation_scheme: AllocationScheme::GpuAllocatorManaged,
        }) {
            Ok(a) => a,
            Err(e) => {
                log!("FATAL: out of GPU memory allocating {name} ({} bytes): {e}", requirements.size);
                std::process::abort();
            }
        }
    }

    pub fn free(&self, allocation: Allocation) {
        if let Some(a) = self.allocator.lock().unwrap().as_mut() {
            let _ = a.free(allocation);
        }
    }

    /// A buffer in memory the CPU writes and the GPU reads, mapped for its whole life. It's in
    /// video memory when the CPU can reach it, which is write-combined: fast to fill in order,
    /// slow to read and to write piecemeal.
    pub fn host_buffer(&self, name: &str, size: u64, usage: vk::BufferUsageFlags) -> (vk::Buffer, Allocation, *mut u8) {
        self.mapped_buffer(name, size, usage, MemoryLocation::CpuToGpu)
    }

    /// The same in cached system memory, which the GPU reads over the bus: for data the game
    /// writes itself, piece by piece, and the GPU reads once.
    pub fn system_buffer(&self, name: &str, size: u64, usage: vk::BufferUsageFlags) -> (vk::Buffer, Allocation, *mut u8) {
        self.mapped_buffer(name, size, usage, MemoryLocation::GpuToCpu)
    }

    fn mapped_buffer(&self, name: &str, size: u64, usage: vk::BufferUsageFlags, location: MemoryLocation) -> (vk::Buffer, Allocation, *mut u8) {
        let buffer = unsafe {
            self.device.create_buffer(&vk::BufferCreateInfo::default().size(size.max(4)).usage(usage), None)
        }
        .expect("vkCreateBuffer");
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let allocation = self.allocate(name, requirements, location, true);
        if !allocation.memory_properties().contains(vk::MemoryPropertyFlags::HOST_COHERENT) {
            log!("FATAL: {name} got memory that isn't host coherent");
            std::process::abort();
        }
        unsafe { self.device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
            .expect("vkBindBufferMemory");
        let ptr = allocation.mapped_ptr().expect("host buffer is not mapped").as_ptr() as *mut u8;
        (buffer, allocation, ptr)
    }

    /// A buffer the GPU writes and the CPU reads back.
    pub fn readback_buffer(&self, size: u64) -> (vk::Buffer, Allocation, *const u8) {
        let buffer = unsafe {
            self.device.create_buffer(
                &vk::BufferCreateInfo::default().size(size.max(4)).usage(vk::BufferUsageFlags::TRANSFER_DST),
                None,
            )
        }
        .expect("vkCreateBuffer");
        let requirements = unsafe { self.device.get_buffer_memory_requirements(buffer) };
        let allocation = self.allocate("readback", requirements, MemoryLocation::GpuToCpu, true);
        unsafe { self.device.bind_buffer_memory(buffer, allocation.memory(), allocation.offset()) }
            .expect("vkBindBufferMemory");
        let ptr = allocation.mapped_ptr().expect("readback buffer is not mapped").as_ptr() as *const u8;
        (buffer, allocation, ptr)
    }
}

pub enum Garbage {
    Buffer(vk::Buffer, Allocation),
    Image(vk::Image, Vec<vk::ImageView>, Allocation),
    Pipeline(vk::Pipeline),
}

/// Id of the submission being recorded. Everything recorded so far finishes when it does.
static RECORDING: AtomicU64 = AtomicU64::new(1);
/// Highest submission id the GPU is known to have finished.
static COMPLETED: AtomicU64 = AtomicU64::new(0);
static GARBAGE: Mutex<Vec<(u64, Garbage)>> = Mutex::new(Vec::new());
/// Held around every use of the queue: the main thread and the presenter thread both submit.
pub static QUEUE: Mutex<()> = Mutex::new(());
/// Kept descriptor sets name image views, so they're dropped when this changes.
static IMAGES_DESTROYED: AtomicU64 = AtomicU64::new(0);

pub fn images_destroyed() -> u64 {
    IMAGES_DESTROYED.load(Ordering::Relaxed)
}

pub fn recording() -> u64 {
    RECORDING.load(Ordering::Relaxed)
}

pub fn completed() -> u64 {
    COMPLETED.load(Ordering::Relaxed)
}

/// Called after a submission is handed to the queue. Returns its id.
pub fn submitted() -> u64 {
    RECORDING.fetch_add(1, Ordering::Relaxed)
}

/// Destroys the object once the GPU is done with everything recorded up to now.
pub fn retire(item: Garbage) {
    GARBAGE.lock().unwrap().push((recording(), item));
}

/// Records that submissions up to `id` finished and destroys what they were holding on to.
pub fn set_completed(id: u64) {
    if id <= COMPLETED.load(Ordering::Relaxed) {
        return;
    }
    COMPLETED.store(id, Ordering::Relaxed);
    let done: Vec<Garbage> = {
        let mut garbage = GARBAGE.lock().unwrap();
        let (done, keep): (Vec<_>, Vec<_>) = garbage.drain(..).partition(|(at, _)| *at <= id);
        *garbage = keep;
        done.into_iter().map(|(_, g)| g).collect()
    };
    let c = get();
    for item in done {
        unsafe {
            match item {
                Garbage::Buffer(buffer, allocation) => {
                    c.device.destroy_buffer(buffer, None);
                    c.free(allocation);
                }
                Garbage::Image(image, views, allocation) => {
                    for view in views {
                        c.device.destroy_image_view(view, None);
                    }
                    c.device.destroy_image(image, None);
                    c.free(allocation);
                    IMAGES_DESTROYED.fetch_add(1, Ordering::Relaxed);
                }
                Garbage::Pipeline(pipeline) => c.device.destroy_pipeline(pipeline, None),
            }
        }
    }
}
