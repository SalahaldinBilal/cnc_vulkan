//! The window's swapchain. The game draws into its own back buffer image, which is drawn into the
//! swapchain (`post.rs`), scaled to the window's client area. Present hands the frame to a
//! presenter thread, which acquires the image, records that copy, submits and presents, so the
//! game thread doesn't wait for the driver. The game thread stays at most one frame ahead.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};

use ash::vk;
use windows_sys::Win32::Foundation::{HWND, RECT};
use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::UI::WindowsAndMessaging::GetClientRect;

use super::context::{self, Context};
use super::post::{self, Pass};
use super::render::{self, FRAMES, GpuImage, Recorded, Renderer, Submit};
use super::stats;
use crate::log;

const COLOR_RANGE: vk::ImageSubresourceRange = vk::ImageSubresourceRange {
    aspect_mask: vk::ImageAspectFlags::COLOR,
    base_mip_level: 0,
    level_count: 1,
    base_array_layer: 0,
    layer_count: 1,
};

/// Black level lift of the whole picture in percent of full scale, from the options menu.
static SHADOW_BRIGHTNESS: AtomicU32 = AtomicU32::new(0);
/// The lift being tried in the options menu, in percent, used instead until `NO_PREVIEW`.
static PREVIEW: AtomicU32 = AtomicU32::new(NO_PREVIEW);
const NO_PREVIEW: u32 = u32::MAX;
/// Where the calibration pattern goes, in back buffer pixels: left, top, width, height.
static PATTERN: Mutex<Option<[f32; 4]>> = Mutex::new(None);
pub const MAX_SHADOW_BRIGHTNESS: u32 = 10;

pub fn set_shadow_brightness(percent: u32) {
    let percent = percent.min(MAX_SHADOW_BRIGHTNESS);
    if SHADOW_BRIGHTNESS.swap(percent, Ordering::Relaxed) != percent {
        log!("shadow brightness {percent}%");
    }
}

pub fn preview_shadow_brightness(percent: Option<u32>) {
    PREVIEW.store(percent.map_or(NO_PREVIEW, |p| p.min(MAX_SHADOW_BRIGHTNESS)), Ordering::Relaxed);
}

/// Draws the calibration pattern centered in `area`, or stops with `None`.
pub fn show_calibration(area: Option<[f32; 4]>) {
    *PATTERN.lock().unwrap() = area;
}

fn is_srgb(format: vk::Format) -> bool {
    matches!(format, vk::Format::B8G8R8A8_SRGB | vk::Format::R8G8B8A8_SRGB | vk::Format::A8B8G8R8_SRGB_PACK32)
}

fn client_size(hwnd: HWND) -> (u32, u32) {
    let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd, &mut r) };
    ((r.right - r.left).max(0) as u32, (r.bottom - r.top).max(0) as u32)
}

/// A finished frame for the presenter.
struct Job {
    /// Which frame in flight, for the pass's descriptor set and the command buffer.
    slot: usize,
    submit: Option<Submit>,
    backbuffer: (vk::ImageView, u32, u32),
    /// Room for the present pass's uniforms in that frame's ring: buffer, offset, mapped address.
    uniform: (vk::Buffer, u64, usize),
    gamma: [u16; 768],
    /// Black level lift in percent, and where the calibration pattern goes in the back buffer.
    shadow_brightness: u32,
    pattern: Option<[f32; 4]>,
}

#[derive(Default)]
struct Queue {
    job: Option<Job>,
    working: bool,
}

struct Shared {
    queue: Mutex<Queue>,
    changed: Condvar,
    chain: Mutex<Chain>,
    /// Set by Reset: the window may have changed size or mode.
    stale: AtomicBool,
}

static SHARED: Mutex<Option<Arc<Shared>>> = Mutex::new(None);

/// Waits until the presenter has submitted every frame handed to it.
pub fn drain() {
    let shared = SHARED.lock().unwrap().clone();
    if let Some(s) = shared {
        s.wait_idle();
    }
}

impl Shared {
    fn wait_idle(&self) {
        let mut q = self.queue.lock().unwrap();
        while q.job.is_some() || q.working {
            q = self.changed.wait(q).unwrap();
        }
    }

    fn run(&self) {
        let c = context::get();
        loop {
            let job = {
                let mut q = self.queue.lock().unwrap();
                while q.job.is_none() {
                    q = self.changed.wait(q).unwrap();
                }
                q.working = true;
                q.job.take().unwrap()
            };
            {
                let mut chain = self.chain.lock().unwrap();
                if self.stale.swap(false, Ordering::Relaxed) {
                    chain.stale = true;
                }
                chain.present(c, job);
            }
            self.queue.lock().unwrap().working = false;
            self.changed.notify_all();
        }
    }
}

/// The game thread's side.
pub struct Swapchain {
    shared: Arc<Shared>,
}

impl Swapchain {
    pub fn new(hwnd: HWND, vsync: bool) -> Result<Swapchain, String> {
        let chain = Chain::new(hwnd, vsync)?;
        let shared = Arc::new(Shared { queue: Mutex::default(), changed: Condvar::new(), chain: Mutex::new(chain), stale: AtomicBool::new(false) });
        let worker = shared.clone();
        std::thread::Builder::new()
            .name("presenter".into())
            .spawn(move || worker.run())
            .map_err(|e| format!("presenter thread: {e}"))?;
        *SHARED.lock().unwrap() = Some(shared.clone());
        Ok(Swapchain { shared })
    }

    /// Hands the frame to the presenter and moves on to the next one.
    pub fn present(&mut self, r: &mut Renderer, backbuffer: &GpuImage, gamma: &[u16; 768]) {
        let uniform = r.alloc(post::PRESENT_UNIFORM_SIZE, r.c.limits.min_uniform_buffer_offset_alignment);
        let preview = PREVIEW.load(Ordering::Relaxed);
        let job = Job {
            slot: r.frame_index(),
            submit: Some(r.finish()),
            backbuffer: (backbuffer.view, backbuffer.width, backbuffer.height),
            uniform: (uniform.buffer, uniform.offset, uniform.ptr as usize),
            gamma: *gamma,
            shadow_brightness: if preview == NO_PREVIEW { SHADOW_BRIGHTNESS.load(Ordering::Relaxed) } else { preview },
            pattern: *PATTERN.lock().unwrap(),
        };
        {
            let mut q = self.shared.queue.lock().unwrap();
            while q.job.is_some() || q.working {
                q = self.shared.changed.wait(q).unwrap();
            }
            q.job = Some(job);
        }
        self.shared.changed.notify_all();
        r.next_frame();
    }

    pub fn set_window(&mut self, hwnd: HWND) {
        if hwnd != self.shared.chain.lock().unwrap().hwnd {
            log!("device window changed, the swapchain stays on the first one");
        }
        self.shared.stale.store(true, Ordering::Relaxed);
    }
}

/// The swapchain itself, used by the presenter thread.
struct Chain {
    hwnd: HWND,
    surface: vk::SurfaceKHR,
    swapchain: vk::SwapchainKHR,
    images: Vec<vk::Image>,
    views: Vec<vk::ImageView>,
    post: Pass,
    /// The present pass's command buffer per frame in flight, each in its own pool.
    pools: [vk::CommandPool; FRAMES],
    cmds: [vk::CommandBuffer; FRAMES],
    /// Signaled when the copy into each image is done, waited on by present.
    ready: Vec<vk::Semaphore>,
    extent: vk::Extent2D,
    format: vk::Format,
    vsync: bool,
    stale: bool,
}

// The window handle is only used for GetClientRect, which any thread may call.
unsafe impl Send for Chain {}

impl Chain {
    fn new(hwnd: HWND, vsync: bool) -> Result<Chain, String> {
        let c = context::get();
        let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
        let surface = unsafe {
            c.win32_surface_fn.create_win32_surface(
                &vk::Win32SurfaceCreateInfoKHR::default().hinstance(instance as isize).hwnd(hwnd as isize),
                None,
            )
        }
        .map_err(|e| format!("vkCreateWin32SurfaceKHR: {e}"))?;
        let supported = unsafe { c.surface_fn.get_physical_device_surface_support(c.physical, c.queue_family, surface) }
            .map_err(|e| e.to_string())?;
        if !supported {
            return Err("the GPU's queue can't present to the game window".into());
        }
        let mut pools = [vk::CommandPool::null(); FRAMES];
        let mut cmds = [vk::CommandBuffer::null(); FRAMES];
        for i in 0..FRAMES {
            pools[i] = unsafe {
                c.device.create_command_pool(
                    &vk::CommandPoolCreateInfo::default().queue_family_index(c.queue_family).flags(vk::CommandPoolCreateFlags::TRANSIENT),
                    None,
                )
            }
            .map_err(|e| format!("vkCreateCommandPool: {e}"))?;
            cmds[i] = unsafe {
                c.device.allocate_command_buffers(
                    &vk::CommandBufferAllocateInfo::default().command_pool(pools[i]).level(vk::CommandBufferLevel::PRIMARY).command_buffer_count(1),
                )
            }
            .map_err(|e| format!("vkAllocateCommandBuffers: {e}"))?[0];
        }
        let mut s = Chain {
            hwnd,
            surface,
            swapchain: vk::SwapchainKHR::null(),
            images: Vec::new(),
            views: Vec::new(),
            post: Pass::new(c, "present", &format!("{}{}", post::COMMON, post::PRESENT), None)?,
            pools,
            cmds,
            ready: Vec::new(),
            extent: vk::Extent2D::default(),
            format: vk::Format::B8G8R8A8_UNORM,
            vsync,
            stale: true,
        };
        s.recreate(c)?;
        Ok(s)
    }

    fn recreate(&mut self, c: &Context) -> Result<(), String> {
        let caps = unsafe { c.surface_fn.get_physical_device_surface_capabilities(c.physical, self.surface) }
            .map_err(|e| e.to_string())?;
        let (w, h) = client_size(self.hwnd);
        let extent = if caps.current_extent.width != u32::MAX {
            caps.current_extent
        } else {
            vk::Extent2D {
                width: w.clamp(caps.min_image_extent.width, caps.max_image_extent.width),
                height: h.clamp(caps.min_image_extent.height, caps.max_image_extent.height),
            }
        };
        if extent.width == 0 || extent.height == 0 {
            return Ok(());
        }
        let formats = unsafe { c.surface_fn.get_physical_device_surface_formats(c.physical, self.surface) }
            .map_err(|e| e.to_string())?;
        let format = formats
            .iter()
            .find(|f| f.format == vk::Format::B8G8R8A8_UNORM)
            .or_else(|| formats.iter().find(|f| f.format == vk::Format::R8G8B8A8_UNORM))
            .or(formats.first())
            .copied()
            .ok_or("the window offers no surface formats")?;
        let modes = unsafe { c.surface_fn.get_physical_device_surface_present_modes(c.physical, self.surface) }
            .map_err(|e| e.to_string())?;
        let mode = if self.vsync {
            vk::PresentModeKHR::FIFO
        } else {
            [vk::PresentModeKHR::IMMEDIATE, vk::PresentModeKHR::MAILBOX]
                .into_iter()
                .find(|m| modes.contains(m))
                .unwrap_or(vk::PresentModeKHR::FIFO)
        };
        let mut count = caps.min_image_count.max(2) + 1;
        if caps.max_image_count != 0 {
            count = count.min(caps.max_image_count);
        }
        let old = self.swapchain;
        let info = vk::SwapchainCreateInfoKHR::default()
            .surface(self.surface)
            .min_image_count(count)
            .image_format(format.format)
            .image_color_space(format.color_space)
            .image_extent(extent)
            .image_array_layers(1)
            .image_usage(vk::ImageUsageFlags::COLOR_ATTACHMENT)
            .image_sharing_mode(vk::SharingMode::EXCLUSIVE)
            .pre_transform(vk::SurfaceTransformFlagsKHR::IDENTITY)
            .composite_alpha(vk::CompositeAlphaFlagsKHR::OPAQUE)
            .present_mode(mode)
            .clipped(true)
            .old_swapchain(old);
        let swapchain = unsafe { c.swapchain_fn.create_swapchain(&info, None) }.map_err(|e| format!("vkCreateSwapchainKHR: {e}"))?;
        unsafe {
            for &v in &self.views {
                c.device.destroy_image_view(v, None);
            }
            if old != vk::SwapchainKHR::null() {
                c.swapchain_fn.destroy_swapchain(old, None);
            }
            for &s in &self.ready {
                c.device.destroy_semaphore(s, None);
            }
        }
        self.swapchain = swapchain;
        self.images = unsafe { c.swapchain_fn.get_swapchain_images(swapchain) }.map_err(|e| e.to_string())?;
        self.views = self
            .images
            .iter()
            .map(|&image| unsafe {
                c.device.create_image_view(
                    &vk::ImageViewCreateInfo::default()
                        .image(image)
                        .view_type(vk::ImageViewType::TYPE_2D)
                        .format(format.format)
                        .subresource_range(COLOR_RANGE),
                    None,
                )
            }
            .expect("vkCreateImageView"))
            .collect();
        self.post.set_format(c, format.format);
        self.ready = self
            .images
            .iter()
            .map(|_| unsafe { c.device.create_semaphore(&vk::SemaphoreCreateInfo::default(), None) }.expect("vkCreateSemaphore"))
            .collect();
        self.extent = extent;
        self.format = format.format;
        self.stale = false;
        log!("swapchain {}x{} {:?} {:?}, {} images", extent.width, extent.height, format.format, mode, self.images.len());
        Ok(())
    }

    /// Submits the frame without showing it, so its fence still signals.
    fn submit_only(c: &Context, frame: &Recorded) {
        render::queue_submit(c, frame.buffers(), None, None, frame.fence);
    }

    /// Records the frame, draws the back buffer to the window through the gamma ramp and presents it.
    fn present(&mut self, c: &Context, mut job: Job) {
        let recorded = job.submit.take().expect("a frame submitted twice").record(c);
        let (w, h) = client_size(self.hwnd);
        if w == 0 || h == 0 {
            Self::submit_only(c, &recorded);
            return;
        }
        if self.stale || w != self.extent.width || h != self.extent.height {
            {
                let _queue = context::QUEUE.lock().unwrap();
                unsafe { c.device.device_wait_idle() }.expect("vkDeviceWaitIdle");
            }
            if let Err(e) = self.recreate(c) {
                log!("swapchain could not be recreated: {e}");
            }
        }
        let acquire = recorded.acquire;
        let start = std::time::Instant::now();
        let acquired = unsafe { c.swapchain_fn.acquire_next_image(self.swapchain, u64::MAX, acquire, vk::Fence::null()) };
        stats::add(&stats::WAIT_US, start.elapsed().as_micros() as u64);
        let index = match acquired {
            Ok((i, suboptimal)) => {
                self.stale |= suboptimal;
                i
            }
            Err(e) => {
                if e != vk::Result::ERROR_OUT_OF_DATE_KHR {
                    log!("vkAcquireNextImageKHR: {e}");
                }
                self.stale = true;
                Self::submit_only(c, &recorded);
                return;
            }
        };
        let cmd = self.record(c, &job, index as usize);
        let ready = self.ready[index as usize];
        let mut buffers = recorded.buffers().to_vec();
        buffers.push(cmd);
        render::queue_submit(c, &buffers, Some(acquire), Some(ready), recorded.fence);
        let waits = [ready];
        let swapchains = [self.swapchain];
        let indices = [index];
        let info = vk::PresentInfoKHR::default().wait_semaphores(&waits).swapchains(&swapchains).image_indices(&indices);
        let presented = {
            let _queue = context::QUEUE.lock().unwrap();
            unsafe { c.swapchain_fn.queue_present(c.queue, &info) }
        };
        match presented {
            Ok(suboptimal) => self.stale |= suboptimal,
            Err(vk::Result::ERROR_OUT_OF_DATE_KHR) => self.stale = true,
            Err(e) => log!("vkQueuePresentKHR: {e}"),
        }
    }

    /// Records the copy of the back buffer into swapchain image `index`.
    fn record(&mut self, c: &Context, job: &Job, index: usize) -> vk::CommandBuffer {
        let d = &c.device;
        let cmd = self.cmds[job.slot];
        unsafe {
            d.reset_command_pool(self.pools[job.slot], vk::CommandPoolResetFlags::empty()).expect("vkResetCommandPool");
            d.begin_command_buffer(cmd, &vk::CommandBufferBeginInfo::default().flags(vk::CommandBufferUsageFlags::ONE_TIME_SUBMIT))
                .expect("vkBeginCommandBuffer");
        }
        let written = vk::MemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::MEMORY_WRITE)
            .dst_access_mask(vk::AccessFlags::MEMORY_READ | vk::AccessFlags::MEMORY_WRITE);
        let image = self.images[index];
        let to_attachment = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::empty())
            .dst_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .old_layout(vk::ImageLayout::UNDEFINED)
            .new_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(COLOR_RANGE);
        let to_present = vk::ImageMemoryBarrier::default()
            .src_access_mask(vk::AccessFlags::COLOR_ATTACHMENT_WRITE)
            .dst_access_mask(vk::AccessFlags::empty())
            .old_layout(vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL)
            .new_layout(vk::ImageLayout::PRESENT_SRC_KHR)
            .src_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .dst_queue_family_index(vk::QUEUE_FAMILY_IGNORED)
            .image(image)
            .subresource_range(COLOR_RANGE);
        let (view, width, height) = job.backbuffer;
        let same_size = width == self.extent.width && height == self.extent.height;
        let (sx, sy) = (self.extent.width as f32 / width as f32, self.extent.height as f32 / height as f32);
        let pattern = job.pattern.map(|[x, y, w, h]| post::calibration_pattern([x * sx, y * sy, w * sx, h * sy]));
        let uniform = post::present_uniform(same_size, &job.gamma, job.shadow_brightness as f32 / 100.0, is_srgb(self.format), pattern);
        let (buffer, offset, ptr) = job.uniform;
        unsafe { std::ptr::copy_nonoverlapping(uniform.as_ptr(), ptr as *mut u8, uniform.len()) };
        unsafe {
            d.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::PipelineStageFlags::ALL_COMMANDS,
                vk::DependencyFlags::empty(),
                &[written],
                &[],
                &[to_attachment],
            );
        }
        let target = self.views[index];
        let set = self.post.frame_set(job.slot);
        self.post.write_set(c, set, &[view], (buffer, offset, uniform.len() as u64));
        self.post.commands(set, target, vk::ImageLayout::COLOR_ATTACHMENT_OPTIMAL, self.extent).record(c, cmd);
        unsafe {
            d.cmd_pipeline_barrier(
                cmd,
                vk::PipelineStageFlags::COLOR_ATTACHMENT_OUTPUT,
                vk::PipelineStageFlags::BOTTOM_OF_PIPE,
                vk::DependencyFlags::empty(),
                &[],
                &[],
                &[to_present],
            );
            d.end_command_buffer(cmd).expect("vkEndCommandBuffer");
        }
        cmd
    }

}
