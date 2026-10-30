//! We ship as dinput8.dll, which the game imports statically, so our code runs before WinMain.
//! Zero Hour loads D3D8 later with LoadLibraryA, and those calls get our own module back. Base
//! Generals imports Direct3DCreate8 statically, so that import is pointed at ours.

use std::ffi::{CStr, c_void};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::HMODULE;
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_PIN, GetModuleHandleExW, GetProcAddress,
    LoadLibraryW,
};
use windows_sys::Win32::System::SystemInformation::GetSystemDirectoryW;

use crate::d3d::HRESULT;
use crate::{iat, log};

const E_FAIL: HRESULT = 0x8000_4005_u32 as i32;

static OUR_MODULE: AtomicUsize = AtomicUsize::new(0);
static ORIG_LOAD_LIBRARY: AtomicUsize = AtomicUsize::new(0);
static REDIRECT_LOGGED: AtomicBool = AtomicBool::new(false);

/// Pins our module so a FreeLibrary from the game can't unload it, then hooks the game's LoadLibraryA.
pub fn install(module: HMODULE) {
    let mut pinned: HMODULE = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_PIN;
    unsafe { GetModuleHandleExW(flags, install as *const () as *const u16, &mut pinned) };
    OUR_MODULE.store(module as usize, Ordering::SeqCst);
    if let Some(prev) =
        unsafe { iat::hook_main_module("KERNEL32.dll", "LoadLibraryA", load_library_hook as *const () as usize) }
    {
        ORIG_LOAD_LIBRARY.store(prev, Ordering::SeqCst);
    }
    unsafe { iat::hook_main_module_if_imported("d3d8.dll", "Direct3DCreate8", crate::Direct3DCreate8 as *const () as usize) };
}

unsafe extern "system" fn load_library_hook(name: *const u8) -> HMODULE {
    if !name.is_null() {
        let requested = unsafe { CStr::from_ptr(name as *const i8) }.to_string_lossy();
        let file = requested.rsplit(['\\', '/']).next().unwrap_or_default();
        if file.eq_ignore_ascii_case("d3d8.dll") || file.eq_ignore_ascii_case("d3d8") {
            if !REDIRECT_LOGGED.swap(true, Ordering::Relaxed) {
                log!("LoadLibraryA(\"{requested}\") answered with our module");
            }
            return OUR_MODULE.load(Ordering::SeqCst) as HMODULE;
        }
    }
    let f: unsafe extern "system" fn(*const u8) -> HMODULE =
        unsafe { std::mem::transmute(ORIG_LOAD_LIBRARY.load(Ordering::SeqCst)) };
    unsafe { f(name) }
}

/// Loads a DLL from the Windows system directory, never from the game folder.
pub fn system_library(name: &str) -> HMODULE {
    let mut dir = [0u16; 260];
    let len = unsafe { GetSystemDirectoryW(dir.as_mut_ptr(), dir.len() as u32) } as usize;
    let path: Vec<u16> = dir[..len].iter().copied().chain(format!("\\{name}\0").encode_utf16()).collect();
    unsafe { LoadLibraryW(path.as_ptr()) }
}

type DirectInput8CreateFn =
    unsafe extern "system" fn(*mut c_void, u32, *const c_void, *mut *mut c_void, *mut c_void) -> HRESULT;

static SYSTEM_DINPUT8: OnceLock<Option<DirectInput8CreateFn>> = OnceLock::new();

pub unsafe fn direct_input8_create(
    instance: *mut c_void,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> HRESULT {
    let real = SYSTEM_DINPUT8.get_or_init(|| {
        let module = system_library("dinput8.dll");
        let f = unsafe { GetProcAddress(module, c"DirectInput8Create".as_ptr() as *const u8) };
        if f.is_none() {
            log!("system dinput8.dll could not be loaded");
        }
        f.map(|f| unsafe { std::mem::transmute::<_, DirectInput8CreateFn>(f) })
    });
    match real {
        Some(f) => unsafe { f(instance, version, riid, out, outer) },
        None => E_FAIL,
    }
}
