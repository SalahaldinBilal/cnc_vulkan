//! Exclusive fullscreen for the Vulkan backend: the monitor is switched to the game's resolution
//! while the game is in front, and back to the desktop mode when it isn't.

use std::sync::Mutex;

use windows_sys::Win32::Graphics::Gdi::{CDS_FULLSCREEN, ChangeDisplaySettingsExW, DEVMODEW, DISP_CHANGE_SUCCESSFUL, DM_BITSPERPEL, DM_DISPLAYFREQUENCY, DM_PELSHEIGHT, DM_PELSWIDTH};

use crate::{log, window};

struct Mode {
    device: Vec<u16>,
    width: u32,
    height: u32,
    refresh: u32,
    active: bool,
}

static MODE: Mutex<Option<Mode>> = Mutex::new(None);

fn apply(m: &Mode) -> bool {
    let mut dm: DEVMODEW = unsafe { std::mem::zeroed() };
    dm.dmSize = size_of::<DEVMODEW>() as u16;
    dm.dmFields = DM_PELSWIDTH | DM_PELSHEIGHT | DM_BITSPERPEL | if m.refresh != 0 { DM_DISPLAYFREQUENCY } else { 0 };
    dm.dmBitsPerPel = 32;
    dm.dmPelsWidth = m.width;
    dm.dmPelsHeight = m.height;
    dm.dmDisplayFrequency = m.refresh;
    let result = unsafe {
        ChangeDisplaySettingsExW(m.device.as_ptr(), &dm, std::ptr::null_mut(), CDS_FULLSCREEN, std::ptr::null())
    };
    if result != DISP_CHANGE_SUCCESSFUL {
        log!("display mode {}x{} {}Hz could not be set (error {result})", m.width, m.height, m.refresh);
        return false;
    }
    log!("display mode {}x{} {}Hz set", m.width, m.height, m.refresh);
    true
}

fn restore(m: &Mode) {
    unsafe { ChangeDisplaySettingsExW(m.device.as_ptr(), std::ptr::null(), std::ptr::null_mut(), 0, std::ptr::null()) };
    log!("desktop display mode restored");
}

/// Switches the configured monitor to the given mode.
pub fn enter(monitor: u32, width: u32, height: u32, refresh: u32) {
    let device: Vec<u16> = window::monitor_device(monitor).encode_utf16().chain([0]).collect();
    let mut m = Mode { device, width, height, refresh, active: false };
    m.active = apply(&m);
    *MODE.lock().unwrap() = Some(m);
}

/// Goes back to the desktop mode for good.
pub fn leave() {
    if let Some(m) = MODE.lock().unwrap().take()
        && m.active
    {
        restore(&m);
    }
}

/// The game window gained or lost focus.
pub fn on_activate(active: bool) {
    let mut guard = MODE.lock().unwrap();
    let Some(m) = guard.as_mut() else { return };
    if active && !m.active {
        m.active = apply(m);
    } else if !active && m.active {
        restore(m);
        m.active = false;
    }
}
