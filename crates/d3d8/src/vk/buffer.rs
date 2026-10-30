//! Vertex and index buffers. Static buffers: the game locks a copy in normal memory, and Unlock
//! writes the locked range into the memory the GPU reads. The CPU never reads GPU memory, which is
//! write-combined and about 300 times slower to read than to write. An unlock that would change
//! data the GPU still has to read moves the buffer to a fresh GPU copy instead, so draws recorded
//! earlier keep seeing what they were recorded with. Dynamic buffers are written straight into the
//! GPU's copy, as on D3D8, until the game locks one in a way that needs a CPU copy.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use ash::vk;
use gpu_allocator::vulkan::Allocation;

use super::com::{self, D3DERR_INVALIDCALL, PrivateData, Vtbl, vtbl};
use super::context::{self, Garbage};
use super::stats;
use super::device::Device;
use crate::d3d::{D3D_OK, HRESULT};

static LOGGED_MEMORY: AtomicBool = AtomicBool::new(false);

pub const USAGE_DYNAMIC: u32 = 0x200;
pub const LOCK_READONLY: u32 = 0x10;
pub const LOCK_NOOVERWRITE: u32 = 0x1000;
pub const LOCK_DISCARD: u32 = 0x2000;

struct Version {
    buffer: vk::Buffer,
    allocation: Option<Allocation>,
    ptr: *mut u8,
    last_use: Cell<u64>,
}

impl Drop for Version {
    fn drop(&mut self) {
        if let Some(a) = self.allocation.take() {
            context::retire(Garbage::Buffer(self.buffer, a));
        }
    }
}

pub struct BufferData {
    pub size: u32,
    pub usage: u32,
    pub pool: u32,
    /// FVF for vertex buffers, index format for index buffers.
    pub format: u32,
    pub index: bool,
    /// The CPU's copy. Empty while the game writes straight into the GPU's copy.
    shadow: RefCell<Vec<u8>>,
    /// A dynamic buffer the game writes straight into the GPU's copy. One locked without DISCARD or
    /// NOOVERWRITE while the GPU reads it keeps a CPU copy from then on, like a static buffer.
    direct: Cell<bool>,
    versions: RefCell<Vec<Version>>,
    current: Cell<usize>,
    /// Ranges locked and not yet unlocked: offset, size, flags.
    locks: RefCell<Vec<(u32, u32, u32)>>,
    /// Box around every position in the buffer, for the vertex stride it was taken with. Cleared
    /// by any write.
    bounds: Cell<Option<(u32, Option<[f32; 6]>)>>,
}

impl BufferData {
    pub fn new(size: u32, usage: u32, pool: u32, format: u32, index: bool) -> BufferData {
        let direct = usage & USAGE_DYNAMIC != 0;
        let shadow = if direct { Vec::new() } else { vec![0; size as usize] };
        let b = BufferData {
            size,
            usage,
            pool,
            format,
            index,
            shadow: RefCell::new(shadow),
            direct: Cell::new(direct),
            versions: RefCell::new(Vec::new()),
            current: Cell::new(0),
            locks: RefCell::new(Vec::new()),
            bounds: Cell::new(None),
        };
        b.versions.borrow_mut().push(b.make_version());
        b
    }

    fn make_version(&self) -> Version {
        let usage = if self.index { vk::BufferUsageFlags::INDEX_BUFFER } else { vk::BufferUsageFlags::VERTEX_BUFFER };
        let c = context::get();
        // The game writes dynamic buffers itself, field by field, which write-combined video
        // memory makes slow (the sorting renderer's copy took 5% of the frame).
        let (buffer, allocation, ptr) = if self.dynamic() {
            c.system_buffer("d3d dynamic buffer", self.size as u64, usage)
        } else {
            c.host_buffer("d3d buffer", self.size as u64, usage)
        };
        if self.dynamic() && !LOGGED_MEMORY.swap(true, Ordering::Relaxed) {
            crate::log!("dynamic buffers in {:?} memory", allocation.memory_properties());
        }
        stats::add(&stats::BUFFER_VERSIONS, 1);
        Version { buffer, allocation: Some(allocation), ptr, last_use: Cell::new(0) }
    }

    /// The buffer draws should read right now. Marks it as used by the submission being recorded.
    pub fn bind(&self) -> vk::Buffer {
        let versions = self.versions.borrow();
        let v = &versions[self.current.get()];
        v.last_use.set(context::recording());
        v.buffer
    }

    fn dynamic(&self) -> bool {
        self.usage & USAGE_DYNAMIC != 0
    }

    fn busy(&self) -> bool {
        self.versions.borrow()[self.current.get()].last_use.get() > context::completed()
    }

    fn lock(&self, offset: u32, size: u32, flags: u32) -> *mut u8 {
        let _timer = stats::Timer::new(&stats::LOCK_TICKS);
        let size = if size == 0 { self.size - offset } else { size };
        self.locks.borrow_mut().push((offset, size, flags));
        stats::add(&stats::BUFFER_LOCKS, 1);
        if self.direct.get() && flags & (LOCK_NOOVERWRITE | LOCK_READONLY) == 0 && self.busy() {
            if flags & LOCK_DISCARD != 0 {
                self.rename();
            } else {
                self.stop_direct_writes();
            }
        }
        if self.direct.get() {
            stats::add(&stats::BUFFER_DIRECT_BYTES, size as u64);
            return unsafe { self.versions.borrow()[self.current.get()].ptr.add(offset as usize) };
        }
        unsafe { self.shadow.borrow_mut().as_mut_ptr().add(offset as usize) }
    }

    fn unlock(&self) -> bool {
        let _timer = stats::Timer::new(&stats::LOCK_TICKS);
        let Some((offset, size, flags)) = self.locks.borrow_mut().pop() else { return false };
        if flags & LOCK_READONLY != 0 || self.direct.get() {
            return true;
        }
        self.bounds.set(None);
        let (start, len) = if !self.busy() || flags & LOCK_NOOVERWRITE != 0 {
            (offset, size)
        } else if self.dynamic() && flags & LOCK_DISCARD != 0 {
            self.rename();
            (offset, size)
        } else {
            self.rename();
            (0, self.size)
        };
        let shadow = self.shadow.borrow();
        let versions = self.versions.borrow();
        let dst = versions[self.current.get()].ptr;
        unsafe { std::ptr::copy_nonoverlapping(shadow.as_ptr().add(start as usize), dst.add(start as usize), len as usize) };
        stats::add(&stats::BUFFER_BYTES, len as u64);
        true
    }

    /// What the buffer holds now, for the game thread's copy. On the renderer thread.
    fn contents(&self) -> Vec<u8> {
        if !self.direct.get() {
            return self.shadow.borrow().clone();
        }
        let versions = self.versions.borrow();
        unsafe { std::slice::from_raw_parts(versions[self.current.get()].ptr, self.size as usize) }.to_vec()
    }

    /// Box around the positions (three floats at the start of each vertex) of a vertex buffer the
    /// game rarely writes. Dynamic buffers are rewritten every frame and their draws use parts of
    /// them, so they get none.
    pub fn bounds(&self, stride: u32) -> Option<[f32; 6]> {
        if self.index || self.usage & USAGE_DYNAMIC != 0 || stride < 12 {
            return None;
        }
        if let Some((s, b)) = self.bounds.get()
            && s == stride
        {
            return b;
        }
        let b = self.scan_bounds(stride);
        self.bounds.set(Some((stride, b)));
        b
    }

    fn scan_bounds(&self, stride: u32) -> Option<[f32; 6]> {
        let shadow = self.shadow.borrow();
        let count = shadow.len() / stride as usize;
        if count == 0 {
            return None;
        }
        let mut b = [f32::MAX, f32::MAX, f32::MAX, f32::MIN, f32::MIN, f32::MIN];
        for i in 0..count {
            let p = unsafe { (shadow.as_ptr().add(i * stride as usize) as *const [f32; 3]).read_unaligned() };
            if !p.iter().all(|v| v.is_finite()) {
                return None;
            }
            for k in 0..3 {
                b[k] = b[k].min(p[k]);
                b[k + 3] = b[k + 3].max(p[k]);
            }
        }
        Some(b)
    }

    /// Reads the GPU's write-combined copy once, which is slow, so the log counts it. Reading it
    /// on every such lock instead cost 30 ms a frame in Generals.
    fn stop_direct_writes(&self) {
        let current = self.versions.borrow()[self.current.get()].ptr;
        let mut shadow = vec![0u8; self.size as usize];
        unsafe { std::ptr::copy_nonoverlapping(current, shadow.as_mut_ptr(), shadow.len()) };
        *self.shadow.borrow_mut() = shadow;
        self.direct.set(false);
        stats::add(&stats::BUFFER_SLOW_COPIES, 1);
    }

    /// Moves to a GPU copy no submission still reads.
    fn rename(&self) {
        let current = self.current.get();
        let completed = context::completed();
        let free = self.versions.borrow().iter().enumerate().position(|(i, v)| i != current && v.last_use.get() <= completed);
        let next = match free {
            Some(i) => i,
            None => {
                let v = self.make_version();
                let mut versions = self.versions.borrow_mut();
                versions.push(v);
                versions.len() - 1
            }
        };
        self.current.set(next);
        stats::add(&stats::BUFFER_RENAMES, 1);
    }
}

/// The game thread's side of a buffer: its own copy of the contents, which Lock hands out, and the
/// ranges locked. Unlock queues what was written for the renderer thread (vk/worker.rs).
struct Front {
    /// Made at the first lock and dropped after the first unlock: most buffers are written once at
    /// load, and a copy of each would run the 32-bit game out of memory. Kept from the second lock on.
    bytes: Option<Vec<u8>>,
    /// Whether `bytes` (or the zeros a new buffer starts with) holds everything the renderer thread has.
    current: bool,
    locks: Vec<(u32, u32, u32)>,
    unlocks: u32,
}

#[repr(C)]
pub struct Buffer {
    vtbl: *const c_void,
    refs: AtomicU32,
    device: *mut Device,
    pub data: Rc<BufferData>,
    private: RefCell<PrivateData>,
    priority: Cell<u32>,
    front: std::cell::UnsafeCell<Front>,
}

pub fn create(device: *mut Device, data: BufferData) -> *mut Buffer {
    let vtbl = if data.index { &INDEX_VTBL as *const _ as *const c_void } else { &VERTEX_VTBL as *const _ as *const c_void };
    let front = Front { bytes: None, current: true, locks: Vec::new(), unlocks: 0 };
    Box::into_raw(Box::new(Buffer {
        vtbl,
        refs: AtomicU32::new(1),
        device,
        data: Rc::new(data),
        private: RefCell::new(PrivateData::default()),
        priority: Cell::new(0),
        front: front.into(),
    }))
}

/// Whether a pointer is one of our buffers and if so, whether an index buffer. Reads only the
/// vtable, so any thread may ask.
pub unsafe fn kind(p: *mut c_void) -> Option<bool> {
    if p.is_null() {
        return None;
    }
    let v = unsafe { *(p as *const *const c_void) };
    if v == &VERTEX_VTBL as *const _ as *const c_void {
        Some(false)
    } else if v == &INDEX_VTBL as *const _ as *const c_void {
        Some(true)
    } else {
        None
    }
}

/// The buffer data behind a pointer the game handed us, if it is one of ours.
pub unsafe fn from_ptr(p: *mut c_void) -> Option<Rc<BufferData>> {
    if p.is_null() {
        return None;
    }
    let v = unsafe { *(p as *const *const c_void) };
    if v == &VERTEX_VTBL as *const _ as *const c_void || v == &INDEX_VTBL as *const _ as *const c_void {
        Some(unsafe { (*(p as *mut Buffer)).data.clone() })
    } else {
        None
    }
}

unsafe extern "system" fn query_interface(this: *mut Buffer, _iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    unsafe {
        add_ref(this);
        *out = this as *mut c_void;
    }
    D3D_OK
}

/// Any thread. The last release frees the buffer on the renderer thread, after everything queued before it.
pub unsafe extern "system" fn add_ref(this: *mut Buffer) -> u32 {
    unsafe { &*this }.refs.fetch_add(1, Ordering::Relaxed) + 1
}

pub unsafe extern "system" fn release(this: *mut Buffer) -> u32 {
    let n = unsafe { &*this }.refs.fetch_sub(1, Ordering::AcqRel) - 1;
    if n == 0 {
        let this = super::worker::Ptr(this);
        super::worker::post(move || drop(unsafe { Box::from_raw({ this }.0) }));
    }
    n
}

unsafe extern "system" fn get_device(this: *mut Buffer, out: *mut *mut c_void) -> HRESULT {
    unsafe {
        let device = (*this).device as *mut c_void;
        com::add_ref(device);
        *out = device;
    }
    D3D_OK
}

unsafe extern "system" fn set_private_data(this: *mut Buffer, guid: *const c_void, data: *const c_void, size: u32, flags: u32) -> HRESULT {
    unsafe { (*this).private.borrow_mut().set(guid, data, size, flags) }
}

unsafe extern "system" fn get_private_data(this: *mut Buffer, guid: *const c_void, data: *mut c_void, size: *mut u32) -> HRESULT {
    unsafe { (*this).private.borrow().get(guid, data, size) }
}

unsafe extern "system" fn free_private_data(this: *mut Buffer, guid: *const c_void) -> HRESULT {
    unsafe { (*this).private.borrow_mut().free(guid) }
}

unsafe extern "system" fn set_priority(this: *mut Buffer, priority: u32) -> u32 {
    unsafe { (*this).priority.replace(priority) }
}

unsafe extern "system" fn get_priority(this: *mut Buffer) -> u32 {
    unsafe { (*this).priority.get() }
}

unsafe extern "system" fn pre_load(_this: *mut Buffer) {}

unsafe extern "system" fn get_type(this: *mut Buffer) -> u32 {
    if unsafe { &*this }.data.index { 7 } else { 6 }
}

/// On the game thread: a pointer into its own copy.
unsafe extern "system" fn lock(this: *mut Buffer, offset: u32, size: u32, out: *mut *mut u8, flags: u32) -> HRESULT {
    let b = unsafe { &*this };
    let total = b.data.size;
    if out.is_null() || offset > total || (size != 0 && offset + size > total) {
        return D3DERR_INVALIDCALL;
    }
    let size = if size == 0 { total - offset } else { size };
    let front = unsafe { &mut *b.front.get() };
    // DISCARD and NOOVERWRITE locks don't read what's there, other locks may.
    let reads = flags & (LOCK_DISCARD | LOCK_NOOVERWRITE) == 0;
    // Not while another lock is open: the game still holds a pointer into the copy.
    if reads && !front.current && front.locks.is_empty() {
        let buffer = super::worker::Ptr(this);
        front.bytes = Some(super::worker::run(move || {
            let buffer = buffer;
            unsafe { &(*buffer.0).data }.contents()
        }));
        front.current = true;
    }
    let bytes = front.bytes.get_or_insert_with(|| vec![0; total as usize]);
    front.locks.push((offset, size, flags));
    unsafe { *out = bytes.as_mut_ptr().add(offset as usize) };
    D3D_OK
}

/// On the game thread: what was written goes to the renderer thread, which locks, copies and
/// unlocks its own side the way the game did, in order with the draws.
unsafe extern "system" fn unlock(this: *mut Buffer) -> HRESULT {
    let front = unsafe { &mut *(*this).front.get() };
    let Some((offset, size, flags)) = front.locks.pop() else { return D3DERR_INVALIDCALL };
    if flags & LOCK_READONLY == 0
        && let Some(bytes) = front.bytes.as_ref()
    {
        let buffer = super::worker::Ptr(this);
        super::worker::post_bytes(&bytes[offset as usize..(offset + size) as usize], move |bytes| {
            let buffer = buffer;
            let data = unsafe { &(*buffer.0).data };
            let p = data.lock(offset, size, flags);
            unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), p, bytes.len()) };
            data.unlock();
        });
    }
    front.unlocks += 1;
    if front.unlocks == 1 && front.locks.is_empty() {
        front.bytes = None;
        front.current = false;
    }
    D3D_OK
}

#[repr(C)]
struct VertexBufferDesc {
    format: u32,
    ty: u32,
    usage: u32,
    pool: u32,
    size: u32,
    fvf: u32,
}

#[repr(C)]
struct IndexBufferDesc {
    format: u32,
    ty: u32,
    usage: u32,
    pool: u32,
    size: u32,
}

unsafe extern "system" fn get_vertex_desc(this: *mut Buffer, out: *mut VertexBufferDesc) -> HRESULT {
    let b = unsafe { &(*this).data };
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    // D3DFMT_VERTEXDATA
    unsafe { *out = VertexBufferDesc { format: 100, ty: 6, usage: b.usage, pool: b.pool, size: b.size, fvf: b.format } };
    D3D_OK
}

unsafe extern "system" fn get_index_desc(this: *mut Buffer, out: *mut IndexBufferDesc) -> HRESULT {
    let b = unsafe { &(*this).data };
    if out.is_null() {
        return D3DERR_INVALIDCALL;
    }
    unsafe { *out = IndexBufferDesc { format: b.format, ty: 7, usage: b.usage, pool: b.pool, size: b.size } };
    D3D_OK
}

static VERTEX_VTBL: Vtbl<14> = vtbl![
    query_interface,
    add_ref,
    release,
    get_device,
    set_private_data,
    get_private_data,
    free_private_data,
    set_priority,
    get_priority,
    pre_load,
    get_type,
    lock,
    unlock,
    get_vertex_desc,
];

static INDEX_VTBL: Vtbl<14> = vtbl![
    query_interface,
    add_ref,
    release,
    get_device,
    set_private_data,
    get_private_data,
    free_private_data,
    set_priority,
    get_priority,
    pre_load,
    get_type,
    lock,
    unlock,
    get_index_desc,
];
