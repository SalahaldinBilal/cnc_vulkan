//! Control bar art and the windows the control bar scheme places, kept on the same canvas as the
//! .wnd layout: one uniform scale, centered horizontally, at the bottom of the screen.
//!
//! The scheme draws each image at `position * multiplier + slide offset` through two small manager
//! methods, which we replace. The scheme setup (`0x005a5350`) gets its multipliers from
//! `TheDisplay->getWidth()/getHeight()` and places windows with `setPosition`, so those two calls
//! return the canvas size and those `setPosition` calls get the canvas offset added.
//!
//! On screens wider than the canvas the bar is widened to the screen edges. The radar block moves
//! to the left edge, the portrait block to the right edge, and the art in between is lengthened
//! by repeating plain columns of the top rail and of the command panel. Windows move the same way:
//! one that crosses both seams of its band stretches, anything else moves with its center.

use std::arch::naked_asm;
use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicUsize, Ordering};

use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

use crate::game::{self, Build};
use crate::ui::{self, Anchor};
use crate::{gadgets, log};

/// Canvas y between the top rail band (above) and the command panel band (below).
const BAND_SPLIT_Y: f64 = 482.0;
/// Canvas x of the columns each band is lengthened at. Checked against all three factions' art.
const RAIL_SEAMS: (f64, f64) = (280.0, 515.0);
const PANEL_SEAMS: (f64, f64) = (240.0, 590.0);
/// Every faction's bar image starts 344 canvas pixels above its texture rows, so the band split
/// is this texture row.
const ART_SPLIT_ROW: f32 = BAND_SPLIT_Y as f32 - 344.0;
/// Width in texels of the strip that is stretched to fill each gap.
const STRIP: f32 = 4.0;
/// A bar image covers at least this many texels across.
const MIN_BAR_TEXELS: f32 = 790.0;

struct Addresses {
    draw_image: usize,
    display_draw_image_entry: usize,
    /// Calls of the scheme setup, in the scheme manager. ECX is the scheme.
    setup_calls: [usize; 3],
    scheme_setup: usize,
    /// The doubles 0.33 and 0.66 W3DCommandBarGridDraw places the selected unit grid lines with.
    grid_first_line: usize,
    grid_second_line: usize,
    draw_foreground_method: usize,
    draw_background_method: usize,
    scheme_draw_foreground: usize,
    scheme_draw_background: usize,
    set_position_calls: [usize; 9],
    /// Return addresses of the getWidth and getHeight calls in the scheme setup. ESI holds the scheme there.
    setup_get_width_return: usize,
    setup_get_height_return: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    draw_image: 0x0074_00C0,
    display_draw_image_entry: 0x0096_7A4C,
    setup_calls: [0x005A_6DCC, 0x005A_6F93, 0x005A_70F9],
    scheme_setup: 0x005A_5350,
    grid_first_line: 0x0096_BE30,
    grid_second_line: 0x0096_BE28,
    draw_foreground_method: 0x005A_6B40,
    draw_background_method: 0x005A_6B70,
    scheme_draw_foreground: 0x005A_6000,
    scheme_draw_background: 0x005A_6110,
    set_position_calls: [
        0x005A_558F, 0x005A_56FD, 0x005A_58A6, 0x005A_5A14, 0x005A_5AF6, 0x005A_5BD8, 0x005A_5D52, 0x005A_5E34, 0x005A_5F36,
    ],
    setup_get_width_return: 0x005A_5430,
    setup_get_height_return: 0x005A_5454,
};

static GENERALS: Addresses = Addresses {
    draw_image: 0x0071_9140,
    display_draw_image_entry: 0x009A_56DC,
    setup_calls: [0x005B_715A, 0x005B_73D9, 0x005B_6EA8],
    scheme_setup: 0x005B_5690,
    grid_first_line: 0x009A_8F28,
    grid_second_line: 0x009A_8F20,
    draw_foreground_method: 0x005B_6F70,
    draw_background_method: 0x005B_6FA0,
    scheme_draw_foreground: 0x005B_6450,
    scheme_draw_background: 0x005B_6560,
    set_position_calls: [
        0x005B_58CF, 0x005B_5A3D, 0x005B_5BE6, 0x005B_5D54, 0x005B_5E36, 0x005B_5F18, 0x005B_6092, 0x005B_6174, 0x005B_6276,
    ],
    setup_get_width_return: 0x005B_5770,
    setup_get_height_return: 0x005B_5794,
};

fn addresses() -> Option<&'static Addresses> {
    game::pick(&ZERO_HOUR, Some(&GENERALS))
}

/// Targets the naked stubs jump to, set at install.
static SCHEME_DRAW_FOREGROUND: AtomicUsize = AtomicUsize::new(0);
static SCHEME_DRAW_BACKGROUND: AtomicUsize = AtomicUsize::new(0);
static SET_POSITION: AtomicUsize = AtomicUsize::new(0);
/// Windows the scheme setup moves. It only sets ExpBarForeground's image, and only sizes the
/// general's promotion popup, so those two keep their position from the layout.
const SCHEME_WINDOWS: [&str; 9] = [
    "ControlBar.wnd:PopupCommunicator",
    "ControlBar.wnd:ButtonIdleWorker",
    "ControlBar.wnd:ButtonOptions",
    "ControlBar.wnd:ButtonPlaceBeacon",
    "ControlBar.wnd:MoneyDisplay",
    "ControlBar.wnd:PowerWindow",
    "ControlBar.wnd:ButtonGeneral",
    "ControlBar.wnd:ButtonLarge",
    "ControlBar.wnd:WinUAttack",
];

/// W3DCommandBarGridDraw (`0x007c8430`) draws the selected unit grid on this window, with lines
/// at `trunc(pos + size * 0.33)` and `trunc(pos + size * 0.66)`.
const GRID_WINDOW: &str = "ControlBar.wnd:WinUnitSelected";
/// The pictures in that grid, with the first and last column and row each covers.
const GRID_CELLS: [(&str, (usize, usize), (usize, usize)); 6] = [
    ("ControlBar.wnd:CameoWindow", (0, 1), (0, 1)),
    ("ControlBar.wnd:UnitUpgrade1", (2, 2), (0, 0)),
    ("ControlBar.wnd:UnitUpgrade2", (2, 2), (1, 1)),
    ("ControlBar.wnd:UnitUpgrade3", (2, 2), (2, 2)),
    ("ControlBar.wnd:UnitUpgrade4", (1, 1), (2, 2)),
    ("ControlBar.wnd:UnitUpgrade5", (0, 0), (2, 2)),
];

mod image {
    pub const TEXTURE_SIZE: usize = 0x0C;
    pub const UV: usize = 0x14;
}

static DRAWING_SCHEME: AtomicBool = AtomicBool::new(false);

/// Half the width added on each side, in canvas units, for a layout `layout_w` wide at `scale`.
pub fn wide_half(screen_w: i32, layout_w: i32, scale: f64) -> f64 {
    ((screen_w as f64 / scale) - layout_w as f64).max(0.0) / 2.0
}

/// Where a control bar window's left and right edges go on the widened bar, in canvas units.
pub fn wide_span(left: f64, right: f64, top: f64, bottom: f64, half: f64) -> (f64, f64) {
    let (s1, s2) = if (top + bottom) / 2.0 < BAND_SPLIT_Y { RAIL_SEAMS } else { PANEL_SEAMS };
    if left < s1 && right >= s2 {
        return (left - half, right + half);
    }
    let center = (left + right) / 2.0;
    let shift = if center < s1 {
        -half
    } else if center >= s2 {
        half
    } else {
        0.0
    };
    (left + shift, right + shift)
}

/// Draws the scheme's bar image widened to the screen, in pieces. Returns false for anything else.
pub fn draw_wide(display: usize, img: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, mode: i32) -> bool {
    if !DRAWING_SCHEME.load(Ordering::Relaxed) || img == 0 {
        return false;
    }
    let (Some(build), Some(a)) = (game::build(), addresses()) else { return false };
    let (tex_w, tex_h) = unsafe { *((img + image::TEXTURE_SIZE) as *const (i32, i32)) };
    let uv = (img + image::UV) as *mut [f32; 4];
    let saved = unsafe { *uv };
    let [u0, v0, u1, v1] = saved;
    let (tw, th) = (tex_w as f32, tex_h as f32);
    let texels = (u1 - u0) * tw;
    let extra = original_display_size(build).0 - (x1 - x0);
    if tex_w <= 0 || tex_h <= 0 || texels < MIN_BAR_TEXELS || extra < 2 {
        return false;
    }
    let half = extra as f32 / 2.0;
    let sx = (x1 - x0) as f32 / texels;
    let sy = (y1 - y0) as f32 / ((v1 - v0) * th);
    let (top, bottom) = (v0 * th, v1 * th);
    let split = ART_SPLIT_ROW.clamp(top, bottom);
    let y_split = y0 as f32 + (split - top) * sy;
    let draw: extern "thiscall" fn(usize, usize, i32, i32, i32, i32, u32, i32) = unsafe { std::mem::transmute(a.draw_image) };
    let bands = [(top, split, y0 as f32, y_split, RAIL_SEAMS), (split, bottom, y_split, y1 as f32, PANEL_SEAMS)];
    for (t0, t1, sy0, sy1, (s1, s2)) in bands {
        if t1 <= t0 {
            continue;
        }
        let (s1, s2) = (s1 as f32, s2 as f32);
        let x = |texel: f32| x0 as f32 + texel * sx;
        let pieces = [
            (0.0, s1, x(0.0) - half, x(s1) - half),
            (s1 - STRIP, s1, x(s1) - half, x(s1)),
            (s1, s2, x(s1), x(s2)),
            (s2, s2 + STRIP, x(s2), x(s2) + half),
            (s2, texels, x(s2) + half, x(texels) + half),
        ];
        for (a, b, from, to) in pieces {
            unsafe { *uv = [u0 + a / tw, t0 / th, u0 + b / tw, t1 / th] };
            draw(display, img, from.round() as i32, sy0.round() as i32, to.round() as i32, sy1.round() as i32, color, mode);
        }
    }
    unsafe { *uv = saved };
    true
}

/// Runs the scheme setup, then moves the windows it placed onto the widened bar.
extern "thiscall" fn setup_scheme(scheme: usize) {
    let Some(a) = addresses() else { return };
    let setup: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(a.scheme_setup) };
    setup(scheme);
    let Some(build) = game::build() else { return };
    let Some((cw, _, scale, ox, oy)) = scheme_layout(scheme) else { return };
    let half = wide_half(original_display_size(build).0, cw, scale);
    if half > 0.0 {
        widen_scheme_windows(half, scale, ox, oy);
    }
    fit_cameo_to_grid();
}

/// In the 800x600 layout each picture of the selected unit grid sits one pixel inside the grid
/// lines. Scaled and rounded one by one they drift off the lines by a few pixels, so they're
/// placed again from the lines the grid draw uses.
fn fit_cameo_to_grid() {
    let panel = gadgets::find(GRID_WINDOW);
    let Some(a) = addresses().filter(|_| panel != 0) else { return };
    let (first, second) = unsafe { (*(a.grid_first_line as *const f64), *(a.grid_second_line as *const f64)) };
    let (px, py) = gadgets::screen_position(panel);
    let (pw, ph) = gadgets::size(panel);
    let line = |pos: i32, size: i32, fraction: f64| (pos as f64 + size as f64 * fraction) as i32;
    let bounds = |pos: i32, size: i32| {
        let (l1, l2) = (line(pos, size, first), line(pos, size, second));
        ([pos + 1, l1 + 1, l2 + 1], [l1, l2, pos + size])
    };
    let (col_start, col_end) = bounds(px, pw);
    let (row_start, row_end) = bounds(py, ph);
    for (name, cols, rows) in GRID_CELLS {
        let cell = gadgets::find(name);
        if cell == 0 {
            continue;
        }
        let (x, y) = (col_start[cols.0], row_start[rows.0]);
        let (w, h) = (col_end[cols.1] - x, row_end[rows.1] - y);
        if w > 0 && h > 0 {
            gadgets::set_position(cell, x - px, y - py);
            gadgets::set_size(cell, w, h);
        }
    }
}

fn widen_scheme_windows(half: f64, scale: f64, ox: i32, oy: i32) {
    for name in SCHEME_WINDOWS {
        let window = gadgets::find(name);
        if window == 0 {
            continue;
        }
        let (sx, sy) = gadgets::screen_position(window);
        let (w, h) = gadgets::size(window);
        let canvas = |v: i32, o: i32| (v - o) as f64 / scale;
        let (left, right) = (canvas(sx, ox), canvas(sx + w, ox));
        let (new_left, new_right) = wide_span(left, right, canvas(sy, oy), canvas(sy + h, oy), half);
        let dx = ((new_left - left) * scale).round() as i32;
        let new_w = ((new_right - new_left) * scale).round() as i32;
        let (x, y) = gadgets::position(window);
        gadgets::set_position(window, x + dx, y);
        if new_w != w {
            gadgets::set_size(window, new_w, h);
        }
    }
}

/// W3DDisplay's drawImage entry. transitions.rs puts its own wrapper there later, which also
/// calls `draw_wide`.
extern "thiscall" fn draw_image(display: usize, img: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, mode: i32) {
    if !draw_wide(display, img, x0, y0, x1, y1, color, mode)
        && let Some(a) = addresses()
    {
        let draw: extern "thiscall" fn(usize, usize, i32, i32, i32, i32, u32, i32) = unsafe { std::mem::transmute(a.draw_image) };
        draw(display, img, x0, y0, x1, y1, color, mode);
    }
}

extern "C" fn scheme_draw_done() {
    DRAWING_SCHEME.store(false, Ordering::Relaxed);
}

pub const ANCHOR: (Anchor, Anchor) = (Anchor::Center, Anchor::End);


static SCHEME_OX: AtomicI32 = AtomicI32::new(0);
static SCHEME_OY: AtomicI32 = AtomicI32::new(0);
static ORIG_GET_WIDTH: AtomicUsize = AtomicUsize::new(0);
static ORIG_GET_HEIGHT: AtomicUsize = AtomicUsize::new(0);

/// Replaces the two scheme draw methods. Safe to do at startup since it only touches code.
pub fn install(_build: &Build) {
    let Some(a) = addresses() else { return };
    SCHEME_DRAW_FOREGROUND.store(a.scheme_draw_foreground, Ordering::SeqCst);
    SCHEME_DRAW_BACKGROUND.store(a.scheme_draw_background, Ordering::SeqCst);
    SET_POSITION.store(gadgets::win_set_position(), Ordering::SeqCst);
    unsafe {
        game::patch_jump(a.draw_foreground_method, draw_foreground_stub as *const () as usize);
        game::patch_jump(a.draw_background_method, draw_background_stub as *const () as usize);
        game::write_u32(a.display_draw_image_entry, draw_image as *const () as u32);
    }
    log!("control bar art moved onto the ui canvas");
}

/// Hooks TheDisplay's size getters and the scheme setup's setPosition calls. Needs TheDisplay to
/// exist, so it runs once the game loads D3D.
pub fn install_display_hook(build: &Build) {
    let Some(a) = addresses() else { return };
    if ORIG_GET_WIDTH.load(Ordering::SeqCst) != 0 {
        return;
    }
    let display = unsafe { *(build.the_display as *const *mut usize) };
    if display.is_null() {
        log!("TheDisplay not created yet, control bar windows keep the original layout");
        return;
    }
    let calls_ok = a.set_position_calls.iter().all(|&at| call_target(at) == Some(gadgets::win_set_position()));
    if !calls_ok {
        log!("control bar setPosition calls don't match, control bar windows keep the original layout");
        return;
    }
    unsafe {
        let vtable = *display as *mut usize;
        ORIG_GET_WIDTH.store(*vtable.add(8), Ordering::SeqCst);
        ORIG_GET_HEIGHT.store(*vtable.add(9), Ordering::SeqCst);
        let mut old = 0;
        VirtualProtect(vtable.add(8) as *const c_void, 8, PAGE_READWRITE, &mut old);
        *vtable.add(8) = get_width_stub as *const () as usize;
        *vtable.add(9) = get_height_stub as *const () as usize;
        VirtualProtect(vtable.add(8) as *const c_void, 8, old, &mut old);
        for at in a.set_position_calls {
            game::patch_call(at, set_position_stub as *const () as usize);
        }
        if a.setup_calls.iter().all(|&at| call_target(at) == Some(a.scheme_setup)) {
            for at in a.setup_calls {
                game::patch_call(at, setup_scheme as *const () as usize);
            }
        } else {
            log!("control bar scheme setup calls don't match, its windows stay at the original width");
        }
    }
    log!("control bar windows moved onto the ui canvas");
}

fn call_target(at: usize) -> Option<usize> {
    let bytes = unsafe { std::slice::from_raw_parts(at as *const u8, 5) };
    (bytes[0] == 0xE8).then(|| {
        let rel = i32::from_le_bytes([bytes[1], bytes[2], bytes[3], bytes[4]]);
        (at as isize + 5 + rel as isize) as usize
    })
}

fn scheme_layout(scheme: usize) -> Option<(i32, i32, f64, i32, i32)> {
    let build = game::build()?;
    let (cw, ch) = unsafe { (*((scheme + 4) as *const i32), *((scheme + 8) as *const i32)) };
    if cw <= 0 || ch <= 0 {
        return None;
    }
    let (scale, ox, oy) = ui::canvas(original_display_size(build), cw, ch, ANCHOR);
    Some((cw, ch, scale, ox, oy))
}

fn original_display_size(build: &Build) -> (i32, i32) {
    let (w, h) = (ORIG_GET_WIDTH.load(Ordering::SeqCst), ORIG_GET_HEIGHT.load(Ordering::SeqCst));
    if w == 0 || h == 0 {
        return ui::display_size(build);
    }
    unsafe {
        let display = *(build.the_display as *const *mut usize);
        let w: extern "thiscall" fn(*mut usize) -> i32 = std::mem::transmute(w);
        let h: extern "thiscall" fn(*mut usize) -> i32 = std::mem::transmute(h);
        (w(display), h(display))
    }
}

/// `args` is what the scheme draw method receives: multiplier x and y (floats), offset x and y.
extern "C" fn fix_draw_args(scheme: usize, args: &mut [u32; 4]) {
    DRAWING_SCHEME.store(true, Ordering::Relaxed);
    let Some((_, _, scale, ox, oy)) = scheme_layout(scheme) else { return };
    args[0] = (scale as f32).to_bits();
    args[1] = (scale as f32).to_bits();
    args[2] = (args[2] as i32 + ox) as u32;
    args[3] = (args[3] as i32 + oy) as u32;
}

extern "C" fn get_width(this: *mut usize, ret: usize, esi: usize) -> i32 {
    if addresses().is_some_and(|a| ret == a.setup_get_width_return)
        && let Some((cw, _, scale, ox, oy)) = scheme_layout(esi)
    {
        SCHEME_OX.store(ox, Ordering::SeqCst);
        SCHEME_OY.store(oy, Ordering::SeqCst);
        return (cw as f64 * scale) as i32;
    }
    let f: extern "thiscall" fn(*mut usize) -> i32 = unsafe { std::mem::transmute(ORIG_GET_WIDTH.load(Ordering::SeqCst)) };
    f(this)
}

extern "C" fn get_height(this: *mut usize, ret: usize, esi: usize) -> i32 {
    if addresses().is_some_and(|a| ret == a.setup_get_height_return)
        && let Some((_, ch, scale, _, _)) = scheme_layout(esi)
    {
        return (ch as f64 * scale) as i32;
    }
    let f: extern "thiscall" fn(*mut usize) -> i32 = unsafe { std::mem::transmute(ORIG_GET_HEIGHT.load(Ordering::SeqCst)) };
    f(this)
}

/// thiscall, no arguments. Passes `this`, the return address and the caller's ESI to Rust.
#[unsafe(naked)]
unsafe extern "C" fn get_width_stub() {
    naked_asm!(
        "push esi",
        "push dword ptr [esp + 4]",
        "push ecx",
        "call {f}",
        "add esp, 12",
        "ret",
        f = sym get_width,
    )
}

#[unsafe(naked)]
unsafe extern "C" fn get_height_stub() {
    naked_asm!(
        "push esi",
        "push dword ptr [esp + 4]",
        "push ecx",
        "call {f}",
        "add esp, 12",
        "ret",
        f = sym get_height,
    )
}

/// Replaces a scheme manager draw method (thiscall, manager in ECX, offset x and y on the stack).
/// Builds the four arguments the scheme's draw takes, lets Rust fix them, then calls the scheme.
macro_rules! draw_stub {
    ($name:ident, $target:expr) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "mov eax, ecx",
                "mov ecx, [eax]",
                "test ecx, ecx",
                "jz 2f",
                "sub esp, 16",
                "mov edx, [eax + 4]",
                "mov [esp], edx",
                "mov edx, [eax + 8]",
                "mov [esp + 4], edx",
                "mov edx, [esp + 20]",
                "mov [esp + 8], edx",
                "mov edx, [esp + 24]",
                "mov [esp + 12], edx",
                "push ecx",
                "lea edx, [esp + 4]",
                "push edx",
                "push ecx",
                "call {fix}",
                "add esp, 8",
                "pop ecx",
                "call dword ptr [{target}]",
                "call {done}",
                "2:",
                "ret 8",
                fix = sym fix_draw_args,
                done = sym scheme_draw_done,
                target = sym $target,
            )
        }
    };
}

draw_stub!(draw_foreground_stub, SCHEME_DRAW_FOREGROUND);
draw_stub!(draw_background_stub, SCHEME_DRAW_BACKGROUND);

/// Stands in for setPosition (thiscall, x and y on the stack) at the scheme setup's call sites.
#[unsafe(naked)]
unsafe extern "C" fn set_position_stub() {
    naked_asm!(
        "mov eax, dword ptr [{ox}]",
        "add [esp + 4], eax",
        "mov eax, dword ptr [{oy}]",
        "add [esp + 8], eax",
        "push dword ptr [{target}]",
        "ret",
        ox = sym SCHEME_OX,
        oy = sym SCHEME_OY,
        target = sym SET_POSITION,
    )
}
