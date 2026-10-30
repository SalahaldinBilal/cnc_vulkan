//! Window management. The game always gets a windowed device. In borderless mode its window covers
//! one monitor. When the window's client area and the back buffer differ in size, every coordinate
//! the game sees is mapped into back buffer space so the game never notices.

use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU8, AtomicUsize, Ordering};

use windows_sys::Win32::Foundation::{BOOL, FALSE, HWND, LPARAM, LRESULT, POINT, RECT, TRUE, WPARAM};
use windows_sys::Win32::Graphics::Gdi::{
    ClientToScreen, DEVMODEW, ENUM_CURRENT_SETTINGS, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, HDC,
    HMONITOR, MONITORINFOEXW,
};
use windows_sys::Win32::Devices::Display::{
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME, DISPLAYCONFIG_MODE_INFO,
    DISPLAYCONFIG_PATH_INFO, DISPLAYCONFIG_SOURCE_DEVICE_NAME, DISPLAYCONFIG_TARGET_DEVICE_NAME,
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QDC_ONLY_ACTIVE_PATHS, QueryDisplayConfig,
};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::UI::HiDpi::{
    DPI_AWARENESS_CONTEXT, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2, GetAwarenessFromDpiAwarenessContext,
    GetWindowDpiAwarenessContext, SetProcessDpiAwarenessContext, SetThreadDpiAwarenessContext,
};
use windows_sys::Win32::UI::WindowsAndMessaging::{
    AdjustWindowRect, CallWindowProcA, ClipCursor, GetClientRect, GetForegroundWindow, GetWindowLongA,
    SetWindowLongA, SetWindowPos,
};

use crate::config::{self, DisplayMode};
use crate::d3d::{
    D3DPRESENT_INTERVAL_IMMEDIATE, D3DPRESENT_INTERVAL_ONE, D3DPRESENT_PARAMETERS, D3DSWAPEFFECT_COPY_VSYNC,
    D3DSWAPEFFECT_DISCARD,
};
use crate::{iat, log};

const GWL_WNDPROC: i32 = -4;
const GWL_STYLE: i32 = -16;
const GWL_EXSTYLE: i32 = -20;
const WS_POPUP: u32 = 0x8000_0000;
const WS_VISIBLE: u32 = 0x1000_0000;
const WS_CHILD: u32 = 0x4000_0000;
const WS_CAPTION: u32 = 0x00C0_0000;
const WS_SYSMENU: u32 = 0x0008_0000;
const WS_MINIMIZEBOX: u32 = 0x0002_0000;
const WS_EX_TOPMOST: u32 = 0x0000_0008;
const SWP_NOSIZE: u32 = 0x1;
const SWP_NOMOVE: u32 = 0x2;
const SWP_FRAMECHANGED: u32 = 0x20;
const SWP_SHOWWINDOW: u32 = 0x40;
const HWND_TOPMOST: isize = -1;
const HWND_NOTOPMOST: isize = -2;
const SW_SHOWMINIMIZED: i32 = 2;
const SW_MINIMIZE: i32 = 6;
const SW_SHOWMINNOACTIVE: i32 = 7;
const SW_FORCEMINIMIZE: i32 = 11;
const MONITORINFOF_PRIMARY: u32 = 1;
const WM_ACTIVATEAPP: u32 = 0x1C;
const WM_MOUSEMOVE: u32 = 0x200;
const WM_LBUTTONDOWN: u32 = 0x201;
const WM_MBUTTONDBLCLK: u32 = 0x209;
const WM_XBUTTONDOWN: u32 = 0x20B;
const WM_XBUTTONDBLCLK: u32 = 0x20D;
const WM_RBUTTONDOWN: u32 = 0x204;
const WM_MBUTTONDOWN: u32 = 0x207;

macro_rules! original {
    ($slot:expr, fn($($t:ty),*) -> $r:ty) => {
        unsafe { std::mem::transmute::<usize, unsafe extern "system" fn($($t),*) -> $r>($slot.load(Ordering::SeqCst)) }
    };
}

#[derive(Clone, Copy)]
struct Managed {
    hwnd: usize,
    style: u32,
    ex_style: u32,
    /// Window rectangle in screen coordinates.
    rect: RECT,
    client_w: i32,
    client_h: i32,
    game_w: i32,
    game_h: i32,
}

impl Managed {
    fn scaled(&self) -> bool {
        self.client_w != self.game_w || self.client_h != self.game_h
    }

    fn to_game(self, p: &mut POINT) {
        p.x = scale(p.x, self.game_w, self.client_w);
        p.y = scale(p.y, self.game_h, self.client_h);
    }

    fn to_client(self, p: &mut POINT) {
        p.x = scale(p.x, self.client_w, self.game_w);
        p.y = scale(p.y, self.client_h, self.game_h);
    }
}

fn scale(v: i32, to: i32, from: i32) -> i32 {
    if from == 0 { v } else { (v as i64 * to as i64 / from as i64) as i32 }
}

static MANAGED: Mutex<Option<Managed>> = Mutex::new(None);
static GAME_WNDPROC: AtomicUsize = AtomicUsize::new(0);

static ORIG_CREATE_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_SET_WINDOW_POS: AtomicUsize = AtomicUsize::new(0);
static ORIG_SHOW_WINDOW: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_CLIENT_RECT: AtomicUsize = AtomicUsize::new(0);
static ORIG_SCREEN_TO_CLIENT: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLIENT_TO_SCREEN: AtomicUsize = AtomicUsize::new(0);
static ORIG_CLIP_CURSOR: AtomicUsize = AtomicUsize::new(0);

fn managed(hwnd: HWND) -> Option<Managed> {
    let m = (*MANAGED.lock().unwrap())?;
    (m.hwnd == hwnd as usize).then_some(m)
}

fn mode() -> Option<DisplayMode> {
    config::get().ok().map(|c| c.display.mode)
}

/// 0 until first read, then 1 (keep the mouse in the window) or 2 (let it out).
static CLIP: AtomicU8 = AtomicU8::new(0);

fn clip_enabled() -> bool {
    match CLIP.load(Ordering::Relaxed) {
        0 => config::get().is_ok_and(|c| c.display.clip_cursor),
        v => v == 1,
    }
}

/// Changes whether the mouse is kept in the game window, right away.
pub fn set_clip(on: bool) {
    CLIP.store(if on { 1 } else { 2 }, Ordering::Relaxed);
    let Some(m) = *MANAGED.lock().unwrap() else { return };
    if on && unsafe { GetForegroundWindow() } == m.hwnd as HWND {
        clip_to(&m);
    } else if !on {
        unsafe { ClipCursor(std::ptr::null()) };
    }
}

/// A window takes its DPI mode from the thread that creates it, so when the process default is
/// locked, the game's window is made per-monitor aware at the moment `CreateWindowExA` runs.
pub fn set_dpi_awareness() {
    if unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) } != 0 {
        log!("per-monitor DPI awareness enabled for the process");
    } else {
        log!("process DPI mode was already set (error {}), setting it per window instead", unsafe { GetLastError() });
    }
    if let Some(prev) =
        unsafe { iat::hook_main_module("USER32.dll", "CreateWindowExA", create_window_hook as *const () as usize) }
    {
        ORIG_CREATE_WINDOW.store(prev, Ordering::SeqCst);
    }
}

fn awareness_name(context: DPI_AWARENESS_CONTEXT) -> &'static str {
    match unsafe { GetAwarenessFromDpiAwarenessContext(context) } {
        0 => "unaware",
        1 => "system aware",
        2 => "per-monitor aware",
        _ => "invalid",
    }
}

/// Runs `f` with the calling thread set to per-monitor DPI awareness, so window geometry is in
/// physical pixels whatever the thread was set to before.
fn per_monitor<R>(f: impl FnOnce() -> R) -> R {
    let previous = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let result = f();
    if !previous.is_null() {
        unsafe { SetThreadDpiAwarenessContext(previous) };
    }
    result
}

#[allow(clippy::too_many_arguments)]
unsafe extern "system" fn create_window_hook(
    ex_style: u32,
    class: *const u8,
    name: *const u8,
    style: u32,
    x: i32,
    y: i32,
    width: i32,
    height: i32,
    parent: HWND,
    menu: *mut c_void,
    instance: *mut c_void,
    param: *const c_void,
) -> HWND {
    let previous = unsafe { SetThreadDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    let f = original!(
        ORIG_CREATE_WINDOW,
        fn(u32, *const u8, *const u8, u32, i32, i32, i32, i32, HWND, *mut c_void, *mut c_void, *const c_void) -> HWND
    );
    let hwnd = unsafe { f(ex_style, class, name, style, x, y, width, height, parent, menu, instance, param) };
    let class_name = if class as usize > 0xFFFF {
        unsafe { std::ffi::CStr::from_ptr(class as *const i8) }.to_string_lossy().into_owned()
    } else {
        format!("atom {}", class as usize)
    };
    log!(
        "CreateWindowExA \"{class_name}\" -> {hwnd:?}, thread was {}, window is {}",
        awareness_name(previous),
        awareness_name(unsafe { GetWindowDpiAwarenessContext(hwnd) })
    );
    hwnd
}

pub fn install_hooks() {
    if !matches!(mode(), Some(DisplayMode::Borderless | DisplayMode::Windowed)) {
        log!("window hooks not installed for display mode {:?}", mode());
        return;
    }
    let hooks: [(&str, usize, &AtomicUsize); 6] = [
        ("SetWindowPos", set_window_pos_hook as *const () as usize, &ORIG_SET_WINDOW_POS),
        ("ShowWindow", show_window_hook as *const () as usize, &ORIG_SHOW_WINDOW),
        ("GetClientRect", get_client_rect_hook as *const () as usize, &ORIG_GET_CLIENT_RECT),
        ("ScreenToClient", screen_to_client_hook as *const () as usize, &ORIG_SCREEN_TO_CLIENT),
        ("ClientToScreen", client_to_screen_hook as *const () as usize, &ORIG_CLIENT_TO_SCREEN),
        ("ClipCursor", clip_cursor_hook as *const () as usize, &ORIG_CLIP_CURSOR),
    ];
    for (name, hook, original) in hooks {
        if let Some(prev) = unsafe { iat::hook_main_module("USER32.dll", name, hook) } {
            original.store(prev, Ordering::SeqCst);
        }
    }
}

/// The window layout to apply for a device. Applied before CreateDevice, and after Reset returns,
/// because changing the window while a device exists can put the device into the lost state.
pub struct WindowPlan {
    hwnd: HWND,
    game_w: i32,
    game_h: i32,
}

/// Rewrites the game's present parameters for the configured display mode. Borderless and windowed
/// get a windowed device and return the window layout to apply. `desktop_format` is the adapter's
/// current display format, which a windowed back buffer must match. `max_refresh` is the highest
/// refresh rate the adapter offers at the requested size, used for exclusive fullscreen.
pub fn prepare_device(
    focus: HWND,
    pp: &mut D3DPRESENT_PARAMETERS,
    desktop_format: u32,
    max_refresh: u32,
) -> Option<WindowPlan> {
    let cfg = config::get().ok()?;
    if cfg.display.mode == DisplayMode::Fullscreen {
        if pp.Windowed == FALSE {
            pp.FullScreen_RefreshRateInHz = max_refresh;
            pp.FullScreen_PresentationInterval =
                if cfg.display.vsync { D3DPRESENT_INTERVAL_ONE } else { D3DPRESENT_INTERVAL_IMMEDIATE };
        }
        return None;
    }
    let hwnd = if pp.hDeviceWindow.is_null() { focus } else { pp.hDeviceWindow };
    let style = unsafe { GetWindowLongA(hwnd, GWL_STYLE) } as u32;
    if style & WS_CHILD != 0 {
        log!("device window {hwnd:?} is a child window (editor?), leaving it alone");
        return None;
    }

    pp.Windowed = TRUE;
    pp.BackBufferFormat = desktop_format;
    pp.FullScreen_RefreshRateInHz = 0;
    pp.FullScreen_PresentationInterval = 0;
    pp.BackBufferCount = 1;
    pp.SwapEffect = if cfg.display.vsync { D3DSWAPEFFECT_COPY_VSYNC } else { D3DSWAPEFFECT_DISCARD };

    Some(WindowPlan { hwnd, game_w: pp.BackBufferWidth as i32, game_h: pp.BackBufferHeight as i32 })
}

pub fn apply(plan: WindowPlan) {
    per_monitor(|| apply_layout(plan));
}

static ON_ACTIVATE: Mutex<Option<fn(bool)>> = Mutex::new(None);

/// Covers the monitor with the window for the Vulkan backend's exclusive fullscreen, after the
/// display mode was switched. `on_activate` runs whenever the game gains or loses focus.
pub fn apply_fullscreen(hwnd: HWND, game_w: i32, game_h: i32, on_activate: fn(bool)) {
    *ON_ACTIVATE.lock().unwrap() = Some(on_activate);
    per_monitor(|| apply_layout(WindowPlan { hwnd, game_w, game_h }));
}

/// GDI device name of the configured monitor, like `\\.\DISPLAY1`.
pub fn monitor_device(index: u32) -> String {
    let list = monitors();
    list.get(index as usize).or(list.first()).map(|m| m.device.clone()).unwrap_or_default()
}

fn apply_layout(plan: WindowPlan) {
    let WindowPlan { hwnd, game_w, game_h } = plan;
    let Ok(cfg) = config::get() else { return };
    let monitor = monitor_rect(cfg.display.monitor);
    let (style, rect) = match cfg.display.mode {
        DisplayMode::Borderless | DisplayMode::Fullscreen => (WS_POPUP | WS_VISIBLE, monitor),
        _ => {
            let style = WS_CAPTION | WS_SYSMENU | WS_MINIMIZEBOX | WS_VISIBLE;
            let mut r = RECT { left: 0, top: 0, right: game_w, bottom: game_h };
            unsafe { AdjustWindowRect(&mut r, style, FALSE) };
            let (w, h) = (r.right - r.left, r.bottom - r.top);
            let left = monitor.left + ((monitor.right - monitor.left) - w).max(0) / 2;
            let top = monitor.top + ((monitor.bottom - monitor.top) - h).max(0) / 2;
            (style, RECT { left, top, right: left + w, bottom: top + h })
        }
    };
    let ex_style = unsafe { GetWindowLongA(hwnd, GWL_EXSTYLE) } as u32 & !WS_EX_TOPMOST;
    let unchanged = managed(hwnd).is_some_and(|m| m.style == style && m.ex_style == ex_style && same_rect(&m.rect, &rect));
    if !unchanged {
        unsafe {
            SetWindowLongA(hwnd, GWL_STYLE, style as i32);
            SetWindowLongA(hwnd, GWL_EXSTYLE, ex_style as i32);
            SetWindowPos(
                hwnd,
                HWND_NOTOPMOST as HWND,
                rect.left,
                rect.top,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_FRAMECHANGED | SWP_SHOWWINDOW,
            );
        }
    }
    let mut client = RECT { left: 0, top: 0, right: 0, bottom: 0 };
    unsafe { GetClientRect(hwnd, &mut client) };
    let m = Managed {
        hwnd: hwnd as usize,
        style,
        ex_style,
        rect,
        client_w: client.right,
        client_h: client.bottom,
        game_w,
        game_h,
    };
    log!(
        "window {hwnd:?}: {:?} at ({}, {}) client {}x{}, game renders {game_w}x{game_h}{}",
        cfg.display.mode,
        rect.left,
        rect.top,
        m.client_w,
        m.client_h,
        if m.scaled() { ", mouse coordinates are mapped" } else { "" }
    );
    *MANAGED.lock().unwrap() = Some(m);

    if GAME_WNDPROC.load(Ordering::SeqCst) == 0 {
        let previous = unsafe { SetWindowLongA(hwnd, GWL_WNDPROC, wndproc as *const () as usize as i32) };
        GAME_WNDPROC.store(previous as u32 as usize, Ordering::SeqCst);
    }
    if clip_enabled() && unsafe { GetForegroundWindow() } == hwnd {
        clip_to(&m);
    }
}

/// Physical pixel size of the configured monitor.
pub fn monitor_size(index: u32) -> (u32, u32) {
    let r = per_monitor(|| monitor_rect(index));
    ((r.right - r.left) as u32, (r.bottom - r.top) as u32)
}

fn same_rect(a: &RECT, b: &RECT) -> bool {
    (a.left, a.top, a.right, a.bottom) == (b.left, b.top, b.right, b.bottom)
}

struct Monitor {
    primary: bool,
    rect: RECT,
    /// GDI device name, like `\\.\DISPLAY1`.
    device: String,
}

/// Monitors, primary first, then in Windows' enumeration order.
fn monitors() -> Vec<Monitor> {
    unsafe extern "system" fn collect(monitor: HMONITOR, _: HDC, _: *mut RECT, list: LPARAM) -> BOOL {
        let list = unsafe { &mut *(list as *mut Vec<Monitor>) };
        let mut info: MONITORINFOEXW = unsafe { std::mem::zeroed() };
        info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;
        if unsafe { GetMonitorInfoW(monitor, &mut info.monitorInfo) } != 0 {
            list.push(Monitor {
                primary: info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
                rect: info.monitorInfo.rcMonitor,
                device: wide_to_string(&info.szDevice),
            });
        }
        TRUE
    }
    let mut list: Vec<Monitor> = Vec::new();
    unsafe { EnumDisplayMonitors(std::ptr::null_mut(), std::ptr::null(), Some(collect), &mut list as *mut _ as LPARAM) };
    list.sort_by_key(|m| !m.primary);
    list
}

fn wide_to_string(w: &[u16]) -> String {
    String::from_utf16_lossy(&w[..w.iter().position(|&c| c == 0).unwrap_or(w.len())])
}

/// Friendly monitor names (like "DELL U2720Q") by GDI device name, from the display configuration.
fn friendly_names() -> Vec<(String, String)> {
    let (mut path_count, mut mode_count) = (0u32, 0u32);
    if unsafe { GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count) } != 0 {
        return Vec::new();
    }
    let mut paths: Vec<DISPLAYCONFIG_PATH_INFO> = vec![unsafe { std::mem::zeroed() }; path_count as usize];
    let mut modes: Vec<DISPLAYCONFIG_MODE_INFO> = vec![unsafe { std::mem::zeroed() }; mode_count as usize];
    let status = unsafe {
        QueryDisplayConfig(
            QDC_ONLY_ACTIVE_PATHS,
            &mut path_count,
            paths.as_mut_ptr(),
            &mut mode_count,
            modes.as_mut_ptr(),
            std::ptr::null_mut(),
        )
    };
    if status != 0 {
        return Vec::new();
    }
    paths.truncate(path_count as usize);
    let mut names = Vec::new();
    for path in &paths {
        let mut source: DISPLAYCONFIG_SOURCE_DEVICE_NAME = unsafe { std::mem::zeroed() };
        source.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME;
        source.header.size = size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32;
        source.header.adapterId = path.sourceInfo.adapterId;
        source.header.id = path.sourceInfo.id;
        let mut target: DISPLAYCONFIG_TARGET_DEVICE_NAME = unsafe { std::mem::zeroed() };
        target.header.r#type = DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME;
        target.header.size = size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32;
        target.header.adapterId = path.targetInfo.adapterId;
        target.header.id = path.targetInfo.id;
        if unsafe { DisplayConfigGetDeviceInfo(&mut source.header) } == 0
            && unsafe { DisplayConfigGetDeviceInfo(&mut target.header) } == 0
        {
            let friendly = wide_to_string(&target.monitorFriendlyDeviceName);
            if !friendly.is_empty() {
                names.push((wide_to_string(&source.viewGdiDeviceName), friendly));
            }
        }
    }
    names
}

pub fn monitor_count() -> u32 {
    monitors().len() as u32
}

/// The monitor's current refresh rate in Hz, or 0 when Windows doesn't say.
pub fn monitor_refresh(index: u32) -> u32 {
    let Some(m) = monitors().into_iter().nth(index as usize) else { return 0 };
    let device: Vec<u16> = m.device.encode_utf16().chain([0]).collect();
    let mut dm: DEVMODEW = unsafe { std::mem::zeroed() };
    dm.dmSize = size_of::<DEVMODEW>() as u16;
    if unsafe { EnumDisplaySettingsW(device.as_ptr(), ENUM_CURRENT_SETTINGS, &mut dm) } == 0 || dm.dmDisplayFrequency <= 1 {
        return 0;
    }
    dm.dmDisplayFrequency
}

/// A display name per monitor, in the same order as the monitor setting counts them.
pub fn monitor_names() -> Vec<String> {
    let friendly = friendly_names();
    monitors()
        .iter()
        .enumerate()
        .map(|(i, m)| {
            let size = format!("{}x{}", m.rect.right - m.rect.left, m.rect.bottom - m.rect.top);
            match friendly.iter().find(|(device, _)| device.eq_ignore_ascii_case(&m.device)) {
                Some((_, name)) => format!("{name} ({size})"),
                None => format!("Monitor {} ({size})", i + 1),
            }
        })
        .collect()
}

fn monitor_rect(index: u32) -> RECT {
    let list: Vec<RECT> = monitors().into_iter().map(|m| m.rect).collect();
    match list.get(index as usize) {
        Some(r) => *r,
        None => {
            log!("monitor {index} does not exist ({} found), using the primary monitor", list.len());
            list.first().copied().unwrap_or(RECT { left: 0, top: 0, right: 1920, bottom: 1080 })
        }
    }
}

fn clip_to(m: &Managed) {
    let mut tl = POINT { x: 0, y: 0 };
    let mut br = POINT { x: m.client_w, y: m.client_h };
    unsafe {
        ClientToScreen(m.hwnd as HWND, &mut tl);
        ClientToScreen(m.hwnd as HWND, &mut br);
        ClipCursor(&RECT { left: tl.x, top: tl.y, right: br.x, bottom: br.y });
    }
}

unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, mut lparam: LPARAM) -> LRESULT {
    if let Some(m) = managed(hwnd) {
        let clip = clip_enabled();
        if msg == WM_ACTIVATEAPP
            && let Some(on_activate) = *ON_ACTIVATE.lock().unwrap()
        {
            on_activate(wparam != 0);
            if wparam == 0 {
                unsafe { windows_sys::Win32::UI::WindowsAndMessaging::ShowWindow(hwnd, SW_MINIMIZE) };
            } else {
                unsafe {
                    SetWindowPos(
                        hwnd,
                        HWND_NOTOPMOST as HWND,
                        m.rect.left,
                        m.rect.top,
                        m.rect.right - m.rect.left,
                        m.rect.bottom - m.rect.top,
                        SWP_SHOWWINDOW,
                    )
                };
            }
        }
        match msg {
            WM_ACTIVATEAPP if clip => {
                if wparam != 0 {
                    clip_to(&m);
                } else {
                    unsafe { ClipCursor(std::ptr::null()) };
                }
            }
            WM_LBUTTONDOWN | WM_RBUTTONDOWN | WM_MBUTTONDOWN if clip => clip_to(&m),
            _ => {}
        }
        let is_mouse = (WM_MOUSEMOVE..=WM_MBUTTONDBLCLK).contains(&msg) || (WM_XBUTTONDOWN..=WM_XBUTTONDBLCLK).contains(&msg);
        if is_mouse && m.scaled() {
            let mut p = POINT { x: (lparam & 0xFFFF) as i16 as i32, y: ((lparam >> 16) & 0xFFFF) as i16 as i32 };
            m.to_game(&mut p);
            lparam = ((p.y as u16 as u32) << 16 | p.x as u16 as u32) as i32 as LPARAM;
        }
    }
    let game: unsafe extern "system" fn(HWND, u32, WPARAM, LPARAM) -> LRESULT =
        unsafe { std::mem::transmute(GAME_WNDPROC.load(Ordering::SeqCst)) };
    unsafe { CallWindowProcA(Some(game), hwnd, msg, wparam, lparam) }
}

unsafe extern "system" fn set_window_pos_hook(
    hwnd: HWND,
    mut after: HWND,
    mut x: i32,
    mut y: i32,
    mut cx: i32,
    mut cy: i32,
    mut flags: u32,
) -> BOOL {
    if let Some(m) = managed(hwnd) {
        x = m.rect.left;
        y = m.rect.top;
        cx = m.rect.right - m.rect.left;
        cy = m.rect.bottom - m.rect.top;
        flags &= !(SWP_NOSIZE | SWP_NOMOVE);
        if after as isize == HWND_TOPMOST {
            after = HWND_NOTOPMOST as HWND;
        }
    }
    let f = original!(ORIG_SET_WINDOW_POS, fn(HWND, HWND, i32, i32, i32, i32, u32) -> BOOL);
    unsafe { f(hwnd, after, x, y, cx, cy, flags) }
}

unsafe extern "system" fn show_window_hook(hwnd: HWND, cmd: i32) -> BOOL {
    let minimize = matches!(cmd, SW_SHOWMINIMIZED | SW_MINIMIZE | SW_SHOWMINNOACTIVE | SW_FORCEMINIMIZE);
    if minimize && mode() == Some(DisplayMode::Borderless) && managed(hwnd).is_some() {
        log!("blocked the game from minimizing its borderless window");
        return TRUE;
    }
    let f = original!(ORIG_SHOW_WINDOW, fn(HWND, i32) -> BOOL);
    unsafe { f(hwnd, cmd) }
}

unsafe extern "system" fn get_client_rect_hook(hwnd: HWND, rect: *mut RECT) -> BOOL {
    if let Some(m) = managed(hwnd).filter(Managed::scaled) {
        if rect.is_null() {
            return FALSE;
        }
        unsafe { *rect = RECT { left: 0, top: 0, right: m.game_w, bottom: m.game_h } };
        return TRUE;
    }
    let f = original!(ORIG_GET_CLIENT_RECT, fn(HWND, *mut RECT) -> BOOL);
    unsafe { f(hwnd, rect) }
}

unsafe extern "system" fn screen_to_client_hook(hwnd: HWND, p: *mut POINT) -> BOOL {
    let f = original!(ORIG_SCREEN_TO_CLIENT, fn(HWND, *mut POINT) -> BOOL);
    let ok = unsafe { f(hwnd, p) };
    if ok != 0
        && !p.is_null()
        && let Some(m) = managed(hwnd).filter(Managed::scaled)
    {
        m.to_game(unsafe { &mut *p });
    }
    ok
}

unsafe extern "system" fn client_to_screen_hook(hwnd: HWND, p: *mut POINT) -> BOOL {
    if !p.is_null()
        && let Some(m) = managed(hwnd).filter(Managed::scaled)
    {
        m.to_client(unsafe { &mut *p });
    }
    let f = original!(ORIG_CLIENT_TO_SCREEN, fn(HWND, *mut POINT) -> BOOL);
    unsafe { f(hwnd, p) }
}

unsafe extern "system" fn clip_cursor_hook(rect: *const RECT) -> BOOL {
    let managed = *MANAGED.lock().unwrap();
    if let (Some(m), false) = (managed, rect.is_null()) {
        if clip_enabled() {
            clip_to(&m);
        }
        return TRUE;
    }
    let f = original!(ORIG_CLIP_CURSOR, fn(*const RECT) -> BOOL);
    unsafe { f(rect) }
}
