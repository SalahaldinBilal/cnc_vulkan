//! `Direct3DCreate8`: applies the settings that have to be in place before the game creates its
//! device, then hands the game our own IDirect3D8. It wraps the system one, which answers the
//! adapter, mode, format and caps queries, and its CreateDevice makes our Vulkan device.

use std::ffi::c_void;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU32, Ordering};

use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::System::LibraryLoader::GetProcAddress;
use windows_sys::Win32::UI::WindowsAndMessaging::{MB_ICONERROR, MessageBoxW};

use crate::config::Resolution;
use crate::d3d::{D3D_OK, D3DADAPTER_IDENTIFIER8, D3DDISPLAYMODE, D3DPRESENT_PARAMETERS, HRESULT};
use crate::{config, controlbar, frame, game, loader, log, pacing, vk, window};

pub const SLOT_GET_ADAPTER_IDENTIFIER: usize = 5;
pub const SLOT_GET_ADAPTER_MODE_COUNT: usize = 6;
pub const SLOT_ENUM_ADAPTER_MODES: usize = 7;
pub const SLOT_GET_ADAPTER_DISPLAY_MODE: usize = 8;
pub const SLOT_GET_DEVICE_CAPS: usize = 13;

static LOGGED_ADAPTERS: AtomicU32 = AtomicU32::new(0);

pub unsafe fn create(sdk_version: u32) -> *mut c_void {
    let cfg = match config::get() {
        Ok(c) => c,
        Err(e) => {
            log!("refusing to start: {e}");
            show_error(&format!("cnc-vulkan could not read its settings:\n\n{e}"));
            return std::ptr::null_mut();
        }
    };
    pacing::configure(&cfg.display);
    if matches!(cfg.display.mode, config::DisplayMode::Borderless | config::DisplayMode::Windowed) {
        game::set_windowed();
    }
    if cfg.engine.skip_intro {
        game::skip_intro();
    }
    match cfg.display.resolution {
        Resolution::Auto if crate::settings::has_resolution() => {}
        Resolution::Auto | Resolution::Native => {
            let (w, h) = window::monitor_size(cfg.display.monitor);
            game::set_resolution(w, h);
        }
        Resolution::Fixed(w, h) => game::set_resolution(w, h),
        Resolution::Game => {}
    }
    if let Some(build) = game::build() {
        if cfg.ui.layout == config::UiLayout::Fit {
            controlbar::install_display_hook(build);
        }
        if cfg.engine.split_logic {
            frame::disable_engine_limiter(build);
        }
    }

    let module = loader::system_library("d3d8.dll");
    let Some(real_create) = (unsafe { GetProcAddress(module, c"Direct3DCreate8".as_ptr() as *const u8) }) else {
        log!("system d3d8.dll could not be loaded");
        return std::ptr::null_mut();
    };
    let real_create: unsafe extern "system" fn(u32) -> *mut c_void = unsafe { std::mem::transmute(real_create) };
    let d3d = unsafe { real_create(sdk_version) };
    log!("Direct3DCreate8({sdk_version}) -> {d3d:?}");
    if d3d.is_null() {
        return d3d;
    }
    wrap(d3d)
}

pub fn show_error(text: &str) {
    let text: Vec<u16> = text.encode_utf16().chain([0]).collect();
    let title: Vec<u16> = "cnc-vulkan".encode_utf16().chain([0]).collect();
    unsafe { MessageBoxW(std::ptr::null_mut(), text.as_ptr(), title.as_ptr(), MB_ICONERROR) };
}

/// The function in a COM object's vtable at `slot`.
pub fn method(object: *mut c_void, slot: usize) -> usize {
    unsafe { *(*(object as *const *const usize)).add(slot) }
}

unsafe fn desktop_format(d3d: *mut c_void, adapter: u32) -> u32 {
    let f: unsafe extern "system" fn(*mut c_void, u32, *mut D3DDISPLAYMODE) -> HRESULT =
        unsafe { std::mem::transmute(method(d3d, SLOT_GET_ADAPTER_DISPLAY_MODE)) };
    let mut mode = D3DDISPLAYMODE::default();
    unsafe { f(d3d, adapter, &mut mode) };
    log!("desktop mode {}x{} {}Hz format {}", mode.Width, mode.Height, mode.RefreshRate, crate::d3d::format_name(mode.Format));
    mode.Format
}

fn bits(format: u32) -> u32 {
    match format {
        20..=22 => 32,
        23..=25 => 16,
        _ => 0,
    }
}

/// Highest refresh rate the adapter offers at this size and color depth, or 0 (driver default).
unsafe fn max_refresh(d3d: *mut c_void, adapter: u32, width: u32, height: u32, format: u32) -> u32 {
    let count: unsafe extern "system" fn(*mut c_void, u32) -> u32 =
        unsafe { std::mem::transmute(method(d3d, SLOT_GET_ADAPTER_MODE_COUNT)) };
    let enumerate: unsafe extern "system" fn(*mut c_void, u32, u32, *mut D3DDISPLAYMODE) -> HRESULT =
        unsafe { std::mem::transmute(method(d3d, SLOT_ENUM_ADAPTER_MODES)) };
    let mut best = 0;
    for i in 0..unsafe { count(d3d, adapter) } {
        let mut mode = D3DDISPLAYMODE::default();
        if unsafe { enumerate(d3d, adapter, i, &mut mode) } == D3D_OK
            && mode.Width == width
            && mode.Height == height
            && bits(mode.Format) == bits(format)
        {
            best = best.max(mode.RefreshRate);
        }
    }
    log!("highest refresh rate for {width}x{height} at {} bit: {best}Hz", bits(format));
    best
}

/// Rewrites the game's present parameters for the configured display mode and returns the window
/// layout to apply. `d3d` is the system IDirect3D8.
pub unsafe fn prepare(d3d: *mut c_void, adapter: u32, focus: HWND, params: &mut D3DPRESENT_PARAMETERS) -> Option<window::WindowPlan> {
    let desktop = unsafe { desktop_format(d3d, adapter) };
    let refresh = unsafe { max_refresh(d3d, adapter, params.BackBufferWidth, params.BackBufferHeight, params.BackBufferFormat) };
    window::prepare_device(focus, params, desktop, refresh)
}

#[repr(C)]
pub struct Object {
    vtable: *const usize,
    inner: *mut c_void,
}

fn inner(this: *mut Object) -> *mut c_void {
    unsafe { (*this).inner }
}

/// Wraps the system IDirect3D8 in ours.
pub fn wrap(real: *mut c_void) -> *mut c_void {
    static TABLE: OnceLock<[usize; 16]> = OnceLock::new();
    let vtable = TABLE.get_or_init(|| {
        [
            query_interface as *const () as usize,
            add_ref as *const () as usize,
            release as *const () as usize,
            forward1::<3> as *const () as usize,
            forward0::<4> as *const () as usize,
            get_adapter_identifier as *const () as usize,
            forward1::<6> as *const () as usize,
            forward3::<7> as *const () as usize,
            forward2::<8> as *const () as usize,
            forward5::<9> as *const () as usize,
            forward6::<10> as *const () as usize,
            forward5::<11> as *const () as usize,
            forward5::<12> as *const () as usize,
            forward3::<13> as *const () as usize,
            forward1::<14> as *const () as usize,
            create_device as *const () as usize,
        ]
    });
    let ours = Box::into_raw(Box::new(Object { vtable: vtable.as_ptr(), inner: real }));
    log!("IDirect3D8 {real:?} wrapped as {ours:?}");
    ours as *mut c_void
}

/// IDirect3D8 methods only take 32-bit arguments, so one forwarder per argument count covers them.
macro_rules! forwarder {
    ($name:ident $(, $arg:ident)*) => {
        unsafe extern "system" fn $name<const S: usize>(this: *mut Object $(, $arg: u32)*) -> u32 {
            let inner = inner(this);
            let f: unsafe extern "system" fn(*mut c_void $(, forwarder!(@u32 $arg))*) -> u32 =
                unsafe { std::mem::transmute(method(inner, S)) };
            unsafe { f(inner $(, $arg)*) }
        }
    };
    (@u32 $arg:ident) => { u32 };
}

forwarder!(forward0);
forwarder!(forward1, a);
forwarder!(forward2, a, b);
forwarder!(forward3, a, b, c);
forwarder!(forward5, a, b, c, d, e);
forwarder!(forward6, a, b, c, d, e, f);

unsafe extern "system" fn query_interface(this: *mut Object, iid: *const c_void, out: *mut *mut c_void) -> HRESULT {
    let inner = inner(this);
    let f: unsafe extern "system" fn(*mut c_void, *const c_void, *mut *mut c_void) -> HRESULT =
        unsafe { std::mem::transmute(method(inner, 0)) };
    let hr = unsafe { f(inner, iid, out) };
    if hr == D3D_OK && unsafe { *out } == inner {
        unsafe { *out = this as *mut c_void };
    }
    hr
}

/// Our object shares the wrapped object's reference count, so it lives exactly as long as it does.
unsafe extern "system" fn add_ref(this: *mut Object) -> u32 {
    let inner = inner(this);
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(method(inner, 1)) };
    unsafe { f(inner) }
}

unsafe extern "system" fn release(this: *mut Object) -> u32 {
    let inner = inner(this);
    let f: unsafe extern "system" fn(*mut c_void) -> u32 = unsafe { std::mem::transmute(method(inner, 2)) };
    let refs = unsafe { f(inner) };
    if refs == 0 {
        drop(unsafe { Box::from_raw(this) });
    }
    refs
}

unsafe extern "system" fn get_adapter_identifier(this: *mut Object, adapter: u32, flags: u32, id: *mut D3DADAPTER_IDENTIFIER8) -> HRESULT {
    let inner = inner(this);
    let f: unsafe extern "system" fn(*mut c_void, u32, u32, *mut D3DADAPTER_IDENTIFIER8) -> HRESULT =
        unsafe { std::mem::transmute(method(inner, SLOT_GET_ADAPTER_IDENTIFIER)) };
    let hr = unsafe { f(inner, adapter, flags, id) };
    let first_time = adapter < 32 && LOGGED_ADAPTERS.fetch_or(1 << adapter, Ordering::Relaxed) & (1 << adapter) == 0;
    if hr == D3D_OK && first_time {
        let id = unsafe { &*id };
        let text = |b: &[u8]| String::from_utf8_lossy(b.split(|&c| c == 0).next().unwrap_or_default()).into_owned();
        log!(
            "adapter {adapter}: \"{}\" driver {} version {:#x} vendor {:#06x} device {:#06x}",
            text(&id.Description),
            text(&id.Driver),
            id.DriverVersion,
            id.VendorId,
            id.DeviceId
        );
    }
    hr
}

unsafe extern "system" fn create_device(
    this: *mut Object,
    adapter: u32,
    device_type: u32,
    focus: HWND,
    behavior: u32,
    pp: *mut D3DPRESENT_PARAMETERS,
    out: *mut *mut c_void,
) -> HRESULT {
    unsafe { vk::device::create(this as *mut c_void, inner(this), adapter, device_type, focus, behavior, pp, out) }
}
