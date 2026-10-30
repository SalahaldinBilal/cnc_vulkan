//! Textures and surfaces. Textures the game can lock keep a copy of every level in the game's own
//! format, the way D3D8's managed pool does. Locks go to that copy and changed levels go to the
//! GPU right before the next draw that uses them. Render targets only live on the GPU and are
//! read back when the game locks them.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use ash::vk;

use super::com::{self, D3DERR_INVALIDCALL, PrivateData, Vtbl, vtbl};
use super::device::Device;
use super::format::{self, Info, Kind};
use super::render::{GpuImage, ImageUse, Renderer};
use super::stats;
use crate::d3d::{D3D_OK, HRESULT};
use crate::log;

pub const USAGE_RENDERTARGET: u32 = 1;
pub const USAGE_DEPTHSTENCIL: u32 = 2;
pub const POOL_DEFAULT: u32 = 0;
pub const POOL_MANAGED: u32 = 1;
pub const POOL_SYSTEMMEM: u32 = 2;
const RTYPE_SURFACE: u32 = 1;
const RTYPE_TEXTURE: u32 = 3;

pub struct TextureData {
    pub info: Info,
    pub width: u32,
    pub height: u32,
    pub levels: u32,
    pub usage: u32,
    pub pool: u32,
    pub gpu: Rc<GpuImage>,
    /// Every level in the game's format. Empty for render targets.
    pub shadow: RefCell<Vec<Vec<u8>>>,
    /// Levels changed since they were last sent to the GPU, one bit each.
    pub dirty: Cell<u32>,
    /// Whether any draw used the texture yet. Until then uploads can go ahead of everything.
    pub used: Cell<bool>,
    pub lod: Cell<u32>,
    /// A sharper image drawn in place of this texture, until the game writes the texture again
    /// unless `pinned`.
    pub replacement: RefCell<Option<Rc<TextureData>>>,
    /// The replacement is kept up to date alongside the texture, so writes don't drop it.
    pub pinned: Cell<bool>,
    /// Draw a smoothed, larger copy of this texture, made on the GPU whenever it changes.
    pub smooth: Cell<bool>,
    pub smoothed: RefCell<Option<Rc<GpuImage>>>,
    /// Written since the smoothed copy was last drawn. Apart from `dirty`, which any upload clears.
    pub changed_since_smooth: Cell<bool>,
    /// Written on the renderer thread other than through the game thread's locks (CopyRects,
    /// UpdateTexture, our radar), so the game thread's copy has to be fetched again before a lock.
    pub front_stale: AtomicBool,
}

impl TextureData {
    pub fn render_target(&self) -> bool {
        self.usage & USAGE_RENDERTARGET != 0
    }

    pub fn level_extent(&self, level: u32) -> (u32, u32) {
        format::level_extent(self.width, self.height, level)
    }

    /// Sends changed levels to the GPU, ordered after every draw recorded so far.
    pub fn upload(&self, r: &mut Renderer) {
        let dirty = self.dirty.replace(0);
        if dirty == 0 {
            return;
        }
        let _timer = stats::Timer::new(&stats::UPLOAD_TICKS);
        if self.used.get() {
            stats::add(&stats::TEXTURE_REUPLOADS, 1);
            note_reupload(self.width, self.height, self.info.d3d);
        }
        let init = !self.used.get();
        if !init {
            r.transfer();
        }
        let shadow = self.shadow.borrow();
        for level in 0..self.levels {
            if dirty & (1 << level) == 0 {
                continue;
            }
            let (w, h) = self.level_extent(level);
            let mut bytes = vec![0u8; self.info.gpu_size(w, h)];
            self.info.to_gpu(&shadow[level as usize], self.info.pitch(w) as usize, w, self.info.rows(h), &mut bytes);
            r.write_image(init, &self.gpu, level, 0, 0, w, h, &bytes);
            stats::add(&stats::TEXTURE_UPLOADS, 1);
            stats::add(&stats::TEXTURE_BYTES, bytes.len() as u64);
        }
    }

    pub fn mark_dirty(&self, level: u32) {
        self.mark_written(level);
        self.front_stale.store(true, Ordering::Relaxed);
    }

    /// A level written from the game thread's copy, which already holds what was written.
    fn mark_written(&self, level: u32) {
        self.dirty.set(self.dirty.get() | (1 << level));
        self.changed_since_smooth.set(true);
        if !self.pinned.get() {
            self.replacement.borrow_mut().take();
        }
    }

    /// The image draws sample: the smoothed copy, else the replacement, else the texture's own.
    pub fn view(&self) -> vk::ImageView {
        if self.smooth.get()
            && let Some(s) = self.smoothed.borrow().as_ref()
        {
            return s.view;
        }
        self.replacement.borrow().as_ref().map_or(self.gpu.view, |r| r.gpu.view)
    }
}

/// Logs each kind of texture the game changes after drawing with it, the first 20 kinds.
fn note_reupload(width: u32, height: u32, format: u32) {
    thread_local! {
        static SEEN: RefCell<Vec<(u32, u32, u32)>> = const { RefCell::new(Vec::new()) };
    }
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if seen.len() < 20 && !seen.contains(&(width, height, format)) {
            seen.push((width, height, format));
            log!("texture {width}x{height} {} changed after a draw used it", crate::d3d::format_name(format));
        }
    });
}

pub enum Backing {
    Level { texture: Rc<TextureData>, level: u32 },
    /// An image surface, only ever in memory.
    Memory(RefCell<Vec<u8>>),
    /// A render target, back buffer or depth buffer.
    Image(RefCell<Rc<GpuImage>>),
}

struct Lock {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
    flags: u32,
    /// The game's format copy of a GPU-only surface, written back on unlock unless read only.
    scratch: Option<Vec<u8>>,
}

pub struct SurfaceData {
    pub info: Info,
    pub width: u32,
    pub height: u32,
    pub usage: u32,
    pub pool: u32,
    pub multisample: u32,
    pub backing: Backing,
    lock: RefCell<Option<Lock>>,
}

impl SurfaceData {
    pub fn new(info: Info, width: u32, height: u32, usage: u32, pool: u32, backing: Backing) -> SurfaceData {
        SurfaceData { info, width, height, usage, pool, multisample: 0, backing, lock: RefCell::new(None) }
    }

    /// The GPU image and level behind the surface, if it has one.
    pub fn gpu(&self) -> Option<(Rc<GpuImage>, u32)> {
        match &self.backing {
            Backing::Level { texture, level } => Some((texture.gpu.clone(), *level)),
            Backing::Image(image) => Some((image.borrow().clone(), 0)),
            Backing::Memory(_) => None,
        }
    }

    pub fn pitch(&self) -> u32 {
        self.info.pitch(self.width)
    }

    /// Runs `f` on the surface's bytes in the game's format. GPU-only surfaces are read back first.
    pub fn with_bytes<R>(&self, r: &mut Renderer, f: impl FnOnce(&[u8]) -> R) -> R {
        match &self.backing {
            Backing::Memory(bytes) => f(&bytes.borrow()),
            Backing::Level { texture, level } if !texture.render_target() => f(&texture.shadow.borrow()[*level as usize]),
            _ => f(&self.read_back(r)),
        }
    }

    fn read_back(&self, r: &mut Renderer) -> Vec<u8> {
        let (image, level) = self.gpu().unwrap();
        let bgra = r.read_image(&image, level);
        let mut out = vec![0u8; self.info.level_size(self.width, self.height)];
        self.info.from_bgra(&bgra, self.width, self.height, &mut out, self.pitch() as usize);
        out
    }

    /// Writes a rectangle given in the game's format (rows `src_pitch` apart) at (x, y).
    #[allow(clippy::too_many_arguments)]
    pub fn write_rect(&self, r: &mut Renderer, src: &[u8], src_pitch: usize, x: u32, y: u32, w: u32, h: u32) {
        let info = self.info;
        match &self.backing {
            Backing::Memory(bytes) => copy_rect(info, src, src_pitch, &mut bytes.borrow_mut(), self.pitch() as usize, x, y, w, h, self.width),
            Backing::Level { texture, level } if !texture.render_target() => {
                let mut shadow = texture.shadow.borrow_mut();
                copy_rect(info, src, src_pitch, &mut shadow[*level as usize], self.pitch() as usize, x, y, w, h, self.width);
                texture.mark_dirty(*level);
            }
            _ => {
                let (image, level) = self.gpu().unwrap();
                let mut bgra = vec![0u8; (w * h * 4) as usize];
                info.to_gpu(src, src_pitch, w, h, &mut bgra);
                r.transfer();
                r.write_image(false, &image, level, x, y, w, h, &bgra);
            }
        }
    }

    unsafe fn lock(&self, r: &mut Renderer, out: *mut LockedRect, rect: *const Rect, flags: u32) -> HRESULT {
        if out.is_null() || self.info.kind == Kind::Depth || self.lock.borrow().is_some() {
            return D3DERR_INVALIDCALL;
        }
        let (x, y, w, h) = if rect.is_null() {
            (0, 0, self.width, self.height)
        } else {
            let rc = unsafe { &*rect };
            if rc.left < 0 || rc.top < 0 || rc.right as u32 > self.width || rc.bottom as u32 > self.height || rc.left >= rc.right || rc.top >= rc.bottom {
                return D3DERR_INVALIDCALL;
            }
            (rc.left as u32, rc.top as u32, (rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32)
        };
        let offset = self.info.offset(self.width, x, y);
        let mut scratch = None;
        let base = match &self.backing {
            Backing::Memory(bytes) => bytes.borrow_mut().as_mut_ptr(),
            Backing::Level { texture, level } if !texture.render_target() => texture.shadow.borrow_mut()[*level as usize].as_mut_ptr(),
            _ => {
                let mut bytes = self.read_back(r);
                let p = bytes.as_mut_ptr();
                scratch = Some(bytes);
                p
            }
        };
        unsafe {
            *out = LockedRect { pitch: self.pitch() as i32, bits: base.add(offset) as *mut c_void };
        }
        *self.lock.borrow_mut() = Some(Lock { x, y, w, h, flags, scratch });
        D3D_OK
    }

    fn unlock(&self, r: &mut Renderer) -> HRESULT {
        let Some(lock) = self.lock.borrow_mut().take() else { return D3DERR_INVALIDCALL };
        if lock.flags & super::buffer::LOCK_READONLY != 0 {
            return D3D_OK;
        }
        match (&self.backing, lock.scratch) {
            (Backing::Level { texture, level }, None) => texture.mark_dirty(*level),
            (_, Some(scratch)) => {
                let offset = self.info.offset(self.width, lock.x, lock.y);
                self.write_rect(r, &scratch[offset..], self.pitch() as usize, lock.x, lock.y, lock.w, lock.h);
            }
            _ => {}
        }
        D3D_OK
    }
}

/// Copies a rectangle between two buffers in the same format.
#[allow(clippy::too_many_arguments)]
pub fn copy_rect(info: Info, src: &[u8], src_pitch: usize, dst: &mut [u8], dst_pitch: usize, x: u32, y: u32, w: u32, h: u32, dst_width: u32) {
    let rows = info.rows(h) as usize;
    let row_bytes = match info.kind {
        Kind::Block(b) => (w.div_ceil(4) * b) as usize,
        _ => (w * info.bytes) as usize,
    };
    let start = info.offset(dst_width, x, y);
    for row in 0..rows {
        let s = &src[row * src_pitch..row * src_pitch + row_bytes];
        let d = start + row * dst_pitch;
        if d + row_bytes <= dst.len() {
            dst[d..d + row_bytes].copy_from_slice(s);
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

#[repr(C)]
pub struct LockedRect {
    pub pitch: i32,
    pub bits: *mut c_void,
}

#[repr(C)]
pub struct SurfaceDesc {
    pub format: u32,
    pub ty: u32,
    pub usage: u32,
    pub pool: u32,
    pub size: u32,
    pub multisample: u32,
    pub width: u32,
    pub height: u32,
}

/// The game thread's side of a lockable texture: its own copy of every level, which LockRect hands
/// out, and each level's lock (x, y, width, height, flags). UnlockRect queues the written rectangle
/// for the renderer thread (vk/worker.rs).
struct Front {
    /// Each level's copy, made at its first lock and dropped after its first unlock: most textures
    /// are written once at load, and a copy of each would run the 32-bit game out of memory.
    levels: Vec<Option<Vec<u8>>>,
    /// Whether the level's copy (or the zeros a new texture starts with) holds what the renderer thread has.
    current: Vec<bool>,
    locks: Vec<Option<(u32, u32, u32, u32, u32)>>,
    unlocks: Vec<u32>,
}

#[repr(C)]
pub struct Texture {
    vtbl: *const c_void,
    refs: AtomicU32,
    device: *mut Device,
    pub data: Rc<TextureData>,
    levels: Vec<*mut Surface>,
    private: RefCell<PrivateData>,
    priority: Cell<u32>,
    /// None for render targets, which are only on the GPU.
    front: std::cell::UnsafeCell<Option<Front>>,
}

impl Texture {
    pub fn device(&self) -> *mut Device {
        self.device
    }

    pub fn level(&self, level: usize) -> Option<*mut Surface> {
        self.levels.get(level).copied()
    }
}

impl Surface {
    pub fn device(&self) -> *mut Device {
        self.device
    }
}

#[repr(C)]
pub struct Surface {
    vtbl: *const c_void,
    refs: AtomicU32,
    device: *mut Device,
    container: *mut Texture,
    pub data: Rc<SurfaceData>,
    private: RefCell<PrivateData>,
}

pub fn create_texture(r: &mut Renderer, device: *mut Device, info: Info, width: u32, height: u32, levels: u32, usage: u32, pool: u32) -> *mut Texture {
    let levels = if levels == 0 { format::full_chain(width, height) } else { levels.min(format::full_chain(width, height)) };
    let rt = usage & USAGE_RENDERTARGET != 0;
    let gpu = GpuImage::new(r, info.vk, width, height, levels, if rt { ImageUse::Target } else { ImageUse::Texture }, !info.has_alpha);
    let shadow = if rt {
        Vec::new()
    } else {
        (0..levels)
            .map(|l| {
                let (w, h) = format::level_extent(width, height, l);
                vec![0u8; info.level_size(w, h)]
            })
            .collect()
    };
    let data = Rc::new(TextureData {
        info,
        width,
        height,
        levels,
        usage,
        pool,
        gpu,
        shadow: RefCell::new(shadow),
        dirty: Cell::new(0),
        used: Cell::new(false),
        lod: Cell::new(0),
        replacement: RefCell::new(None),
        pinned: Cell::new(false),
        smooth: Cell::new(false),
        smoothed: RefCell::new(None),
        changed_since_smooth: Cell::new(false),
        front_stale: AtomicBool::new(false),
    });
    let n = levels as usize;
    let front = (!rt).then(|| Front { levels: vec![None; n], current: vec![true; n], locks: vec![None; n], unlocks: vec![0; n] });
    let texture = Box::into_raw(Box::new(Texture {
        vtbl: &TEXTURE_VTBL as *const _ as *const c_void,
        refs: AtomicU32::new(1),
        device,
        data: data.clone(),
        levels: Vec::new(),
        private: RefCell::new(PrivateData::default()),
        priority: Cell::new(0),
        front: front.into(),
    }));
    let surfaces = (0..levels)
        .map(|level| {
            let (w, h) = format::level_extent(width, height, level);
            let s = SurfaceData::new(info, w, h, usage, pool, Backing::Level { texture: data.clone(), level });
            create_surface(device, texture, s)
        })
        .collect();
    unsafe { (*texture).levels = surfaces };
    texture
}

/// LockRect on a lockable texture's level, answered on the game thread from its own copy. None for
/// render targets.
unsafe fn front_lock(t: *mut Texture, level: u32, out: *mut LockedRect, rect: *const Rect, flags: u32) -> Option<HRESULT> {
    let texture = unsafe { &*t };
    let front = unsafe { &mut *texture.front.get() }.as_mut()?;
    let data = &texture.data;
    if out.is_null() || level >= data.levels || front.locks[level as usize].is_some() {
        return Some(D3DERR_INVALIDCALL);
    }
    let (lw, lh) = data.level_extent(level);
    let (x, y, w, h) = if rect.is_null() {
        (0, 0, lw, lh)
    } else {
        let rc = unsafe { &*rect };
        if rc.left < 0 || rc.top < 0 || rc.right as u32 > lw || rc.bottom as u32 > lh || rc.left >= rc.right || rc.top >= rc.bottom {
            return Some(D3DERR_INVALIDCALL);
        }
        (rc.left as u32, rc.top as u32, (rc.right - rc.left) as u32, (rc.bottom - rc.top) as u32)
    };
    if data.front_stale.swap(false, Ordering::Relaxed) {
        front.current.fill(false);
    }
    let l = level as usize;
    if !front.current[l] {
        let texture = super::worker::Ptr(t);
        front.levels[l] = Some(super::worker::run(move || {
            let texture = texture;
            unsafe { &(*texture.0).data }.shadow.borrow()[l].clone()
        }));
        front.current[l] = true;
    }
    let info = data.info;
    let bytes = front.levels[l].get_or_insert_with(|| vec![0; info.level_size(lw, lh)]);
    let offset = info.offset(lw, x, y);
    unsafe { *out = LockedRect { pitch: info.pitch(lw) as i32, bits: bytes.as_mut_ptr().add(offset) as *mut c_void } };
    front.locks[l] = Some((x, y, w, h, flags));
    Some(D3D_OK)
}

/// UnlockRect on a lockable texture's level: the written rectangle goes to the renderer thread,
/// which writes it into its copy and uploads the level before the next draw that uses it.
unsafe fn front_unlock(t: *mut Texture, level: u32) -> Option<HRESULT> {
    let texture = unsafe { &*t };
    let front = unsafe { &mut *texture.front.get() }.as_mut()?;
    let Some((x, y, w, h, flags)) = front.locks.get_mut(level as usize).and_then(|l| l.take()) else {
        return Some(D3DERR_INVALIDCALL);
    };
    let l = level as usize;
    if flags & super::buffer::LOCK_READONLY == 0
        && let Some(bytes) = front.levels[l].as_ref()
    {
        let info = texture.data.info;
        let (lw, _) = texture.data.level_extent(level);
        let pitch = info.pitch(lw) as usize;
        let start = info.offset(lw, x, y);
        let end = (start + info.rows(h) as usize * pitch).min(bytes.len());
        let texture = super::worker::Ptr(t);
        super::worker::post_bytes(&bytes[start..end], move |rows| {
            let texture = texture;
            let data = unsafe { &(*texture.0).data };
            copy_rect(info, rows, pitch, &mut data.shadow.borrow_mut()[l], pitch, x, y, w, h, lw);
            data.mark_written(level);
        });
    }
    front.unlocks[l] += 1;
    if front.unlocks[l] == 1 {
        front.levels[l] = None;
        front.current[l] = false;
    }
    Some(D3D_OK)
}

/// The texture and level behind a texture's level surface.
fn level_of(s: *mut Surface) -> Option<(*mut Texture, u32)> {
    let s = unsafe { &*s };
    match &s.data.backing {
        Backing::Level { level, .. } if !s.container.is_null() => Some((s.container, *level)),
        _ => None,
    }
}

unsafe extern "system" fn tex_lock_rect_front(this: *mut Texture, level: u32, out: *mut LockedRect, rect: *const Rect, flags: u32) -> HRESULT {
    match unsafe { front_lock(this, level, out, rect, flags) } {
        Some(result) => result,
        None => unsafe { on_worker::tex_lock_rect(this, level, out, rect, flags) },
    }
}

unsafe extern "system" fn tex_unlock_rect_front(this: *mut Texture, level: u32) -> HRESULT {
    match unsafe { front_unlock(this, level) } {
        Some(result) => result,
        None => unsafe { on_worker::tex_unlock_rect(this, level) },
    }
}

unsafe extern "system" fn surf_lock_rect_front(this: *mut Surface, out: *mut LockedRect, rect: *const Rect, flags: u32) -> HRESULT {
    if let Some((t, level)) = level_of(this)
        && let Some(result) = unsafe { front_lock(t, level, out, rect, flags) }
    {
        return result;
    }
    unsafe { on_worker::surf_lock_rect(this, out, rect, flags) }
}

unsafe extern "system" fn surf_unlock_rect_front(this: *mut Surface) -> HRESULT {
    if let Some((t, level)) = level_of(this)
        && let Some(result) = unsafe { front_unlock(t, level) }
    {
        return result;
    }
    unsafe { on_worker::surf_unlock_rect(this) }
}

pub fn create_surface(device: *mut Device, container: *mut Texture, data: SurfaceData) -> *mut Surface {
    Box::into_raw(Box::new(Surface {
        vtbl: &SURFACE_VTBL as *const _ as *const c_void,
        refs: AtomicU32::new(1),
        device,
        container,
        data: Rc::new(data),
        private: RefCell::new(PrivateData::default()),
    }))
}

pub unsafe fn texture_from_ptr(p: *mut c_void) -> Option<*mut Texture> {
    if !p.is_null() && unsafe { *(p as *const *const c_void) } == &TEXTURE_VTBL as *const _ as *const c_void {
        Some(p as *mut Texture)
    } else {
        None
    }
}

pub unsafe fn surface_from_ptr(p: *mut c_void) -> Option<*mut Surface> {
    if !p.is_null() && unsafe { *(p as *const *const c_void) } == &SURFACE_VTBL as *const _ as *const c_void {
        Some(p as *mut Surface)
    } else {
        None
    }
}

fn renderer<'a>(device: *mut Device) -> &'a mut Renderer {
    unsafe { &mut (*device).r }
}

unsafe extern "system" fn tex_query_interface(this: *mut Texture, _iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    unsafe {
        tex_add_ref(this);
        *out = this as *mut c_void;
    }
    D3D_OK
}

/// Any thread. The last release frees the texture on the renderer thread, after everything queued before it.
unsafe extern "system" fn tex_add_ref(this: *mut Texture) -> u32 {
    unsafe { &*this }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

unsafe extern "system" fn tex_release(this: *mut Texture) -> u32 {
    let n = unsafe { &*this }.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        let this = super::worker::Ptr(this);
        super::worker::post(move || {
            let t = unsafe { Box::from_raw({ this }.0) };
            for &s in &t.levels {
                drop(unsafe { Box::from_raw(s) });
            }
        });
    }
    n
}

unsafe extern "system" fn tex_get_device(this: *mut Texture, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let device = (*this).device as *mut c_void;
        com::add_ref(device);
        *out = device;
    }
    D3D_OK
}

unsafe extern "system" fn tex_set_private_data(this: *mut Texture, guid: *const c_void, data: *const c_void, size: u32, flags: u32) -> HRESULT {
    unsafe { (*this).private.borrow_mut().set(guid, data, size, flags) }
}

unsafe extern "system" fn tex_get_private_data(this: *mut Texture, guid: *const c_void, data: *mut c_void, size: *mut u32) -> HRESULT {
    unsafe { (*this).private.borrow().get(guid, data, size) }
}

unsafe extern "system" fn tex_free_private_data(this: *mut Texture, guid: *const c_void) -> HRESULT {
    unsafe { (*this).private.borrow_mut().free(guid) }
}

unsafe extern "system" fn tex_set_priority(this: *mut Texture, priority: u32) -> u32 {
    unsafe { (*this).priority.replace(priority) }
}

unsafe extern "system" fn tex_get_priority(this: *mut Texture) -> u32 {
    unsafe { (*this).priority.get() }
}

unsafe extern "system" fn tex_pre_load(_this: *mut Texture) {}

unsafe extern "system" fn tex_get_type(_this: *mut Texture) -> u32 {
    RTYPE_TEXTURE
}

unsafe extern "system" fn tex_set_lod(this: *mut Texture, lod: u32) -> u32 {
    let t = unsafe { &(*this).data };
    if t.pool != 1 {
        return 0;
    }
    t.lod.replace(lod.min(t.levels - 1))
}

unsafe extern "system" fn tex_get_lod(this: *mut Texture) -> u32 {
    unsafe { &*this }.data.lod.get()
}

unsafe extern "system" fn tex_get_level_count(this: *mut Texture) -> u32 {
    unsafe { &*this }.data.levels
}

unsafe extern "system" fn tex_get_level_desc(this: *mut Texture, level: u32, out: *mut SurfaceDesc) -> HRESULT {
    let t = unsafe { &*this };
    match t.levels.get(level as usize) {
        Some(&s) if !out.is_null() => unsafe { surf_get_desc(s, out) },
        _ => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn tex_get_surface_level(this: *mut Texture, level: u32, out: *mut *mut Surface) -> HRESULT {
    let t = unsafe { &*this };
    match t.levels.get(level as usize) {
        Some(&s) if !out.is_null() => {
            unsafe {
                tex_add_ref(this);
                *out = s;
            }
            D3D_OK
        }
        _ => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn tex_lock_rect(this: *mut Texture, level: u32, out: *mut LockedRect, rect: *const Rect, flags: u32) -> HRESULT {
    let t = unsafe { &*this };
    match t.levels.get(level as usize) {
        Some(&s) => unsafe { (*s).data.lock(renderer(t.device), out, rect, flags) },
        None => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn tex_unlock_rect(this: *mut Texture, level: u32) -> HRESULT {
    let t = unsafe { &*this };
    match t.levels.get(level as usize) {
        Some(&s) => unsafe { (*s).data.unlock(renderer(t.device)) },
        None => D3DERR_INVALIDCALL,
    }
}

unsafe extern "system" fn tex_add_dirty_rect(this: *mut Texture, _rect: *const Rect) -> HRESULT {
    let t = unsafe { &(*this).data };
    if !t.render_target() {
        t.dirty.set(u32::MAX >> (32 - t.levels));
        t.changed_since_smooth.set(true);
    }
    D3D_OK
}

static TEXTURE_VTBL: Vtbl<19> = vtbl![
    tex_query_interface,
    tex_add_ref,
    tex_release,
    tex_get_device,
    tex_set_private_data,
    tex_get_private_data,
    tex_free_private_data,
    tex_set_priority,
    tex_get_priority,
    tex_pre_load,
    tex_get_type,
    on_worker::tex_set_lod,
    on_worker::tex_get_lod,
    tex_get_level_count,
    tex_get_level_desc,
    tex_get_surface_level,
    tex_lock_rect_front,
    tex_unlock_rect_front,
    on_worker::tex_add_dirty_rect,
];

unsafe extern "system" fn surf_query_interface(this: *mut Surface, _iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    unsafe {
        surf_add_ref(this);
        *out = this as *mut c_void;
    }
    D3D_OK
}

/// A surface that belongs to a texture shares the texture's reference count.
pub unsafe extern "system" fn surf_add_ref(this: *mut Surface) -> u32 {
    let s = unsafe { &*this };
    if !s.container.is_null() {
        return unsafe { tex_add_ref(s.container) };
    }
    s.refs.fetch_add(1, Ordering::Relaxed) + 1
}

pub unsafe extern "system" fn surf_release(this: *mut Surface) -> u32 {
    let s = unsafe { &*this };
    if !s.container.is_null() {
        return unsafe { tex_release(s.container) };
    }
    let n = s.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        let this = super::worker::Ptr(this);
        super::worker::post(move || drop(unsafe { Box::from_raw({ this }.0) }));
    }
    n
}

unsafe extern "system" fn surf_get_device(this: *mut Surface, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let device = (*this).device as *mut c_void;
        com::add_ref(device);
        *out = device;
    }
    D3D_OK
}

unsafe extern "system" fn surf_set_private_data(this: *mut Surface, guid: *const c_void, data: *const c_void, size: u32, flags: u32) -> HRESULT {
    unsafe { (*this).private.borrow_mut().set(guid, data, size, flags) }
}

unsafe extern "system" fn surf_get_private_data(this: *mut Surface, guid: *const c_void, data: *mut c_void, size: *mut u32) -> HRESULT {
    unsafe { (*this).private.borrow().get(guid, data, size) }
}

unsafe extern "system" fn surf_free_private_data(this: *mut Surface, guid: *const c_void) -> HRESULT {
    unsafe { (*this).private.borrow_mut().free(guid) }
}

unsafe extern "system" fn surf_get_container(this: *mut Surface, _iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    let s = unsafe { &*this };
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    let container = if s.container.is_null() { s.device as *mut c_void } else { s.container as *mut c_void };
    unsafe {
        com::add_ref(container);
        *out = container;
    }
    D3D_OK
}

pub unsafe extern "system" fn surf_get_desc(this: *mut Surface, out: *mut SurfaceDesc) -> HRESULT {
    let s = unsafe { &(*this).data };
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    unsafe {
        *out = SurfaceDesc {
            format: s.info.d3d,
            ty: RTYPE_SURFACE,
            usage: s.usage,
            pool: s.pool,
            size: s.info.level_size(s.width, s.height) as u32,
            multisample: s.multisample,
            width: s.width,
            height: s.height,
        }
    };
    D3D_OK
}

unsafe extern "system" fn surf_lock_rect(this: *mut Surface, out: *mut LockedRect, rect: *const Rect, flags: u32) -> HRESULT {
    let s = unsafe { &*this };
    unsafe { s.data.lock(renderer(s.device), out, rect, flags) }
}

unsafe extern "system" fn surf_unlock_rect(this: *mut Surface) -> HRESULT {
    let s = unsafe { &*this };
    s.data.unlock(renderer(s.device))
}

static SURFACE_VTBL: Vtbl<11> = vtbl![
    surf_query_interface,
    surf_add_ref,
    surf_release,
    surf_get_device,
    surf_set_private_data,
    surf_get_private_data,
    surf_free_private_data,
    surf_get_container,
    surf_get_desc,
    surf_lock_rect_front,
    surf_unlock_rect_front,
];

pub fn depth_info(d3d: u32) -> Info {
    let mut info = format::info(d3d).unwrap_or(format::info(format::D24S8).unwrap());
    info.vk = super::context::get().depth_format;
    info
}

pub fn log_unsupported(what: &str, format: u32) {
    log!("{what}: format {} is not supported", crate::d3d::format_name(format));
}

/// The vtable's methods: each runs on the renderer thread while the caller waits (vk/worker.rs).
mod on_worker {
    use super::*;

    pub unsafe extern "system" fn tex_set_lod(this: *mut Texture, lod: u32) -> u32 {
        crate::vk::worker::run_as("texture::tex_set_lod", || unsafe { super::tex_set_lod(this, lod) })
    }

    pub unsafe extern "system" fn tex_get_lod(this: *mut Texture) -> u32 {
        crate::vk::worker::run_as("texture::tex_get_lod", || unsafe { super::tex_get_lod(this) })
    }

    pub unsafe extern "system" fn tex_lock_rect(this: *mut Texture, level: u32, out: *mut LockedRect, rect: *const Rect, flags: u32) -> HRESULT {
        crate::vk::worker::run_as("texture::tex_lock_rect", || unsafe { super::tex_lock_rect(this, level, out, rect, flags) })
    }

    pub unsafe extern "system" fn tex_unlock_rect(this: *mut Texture, level: u32) -> HRESULT {
        crate::vk::worker::run_as("texture::tex_unlock_rect", || unsafe { super::tex_unlock_rect(this, level) })
    }

    pub unsafe extern "system" fn tex_add_dirty_rect(this: *mut Texture, _rect: *const Rect) -> HRESULT {
        crate::vk::worker::run_as("texture::tex_add_dirty_rect", || unsafe { super::tex_add_dirty_rect(this, _rect) })
    }

    pub unsafe extern "system" fn surf_lock_rect(this: *mut Surface, out: *mut LockedRect, rect: *const Rect, flags: u32) -> HRESULT {
        crate::vk::worker::run_as("texture::surf_lock_rect", || unsafe { super::surf_lock_rect(this, out, rect, flags) })
    }

    pub unsafe extern "system" fn surf_unlock_rect(this: *mut Surface) -> HRESULT {
        crate::vk::worker::run_as("texture::surf_unlock_rect", || unsafe { super::surf_unlock_rect(this) })
    }
}
