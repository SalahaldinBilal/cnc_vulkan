//! The radar's other layers sharper: the shroud (fog of war) at a higher resolution, the unit
//! dots placed four times finer and redrawn every logic step, and the camera box from exact
//! positions. See knowledge-base/radar.md.

use std::cell::{Cell, RefCell};
use std::ffi::c_void;
use std::rc::Rc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::vk::com;
use crate::vk::texture::{self, TextureData};
use crate::{config, frame, game, log};

const GAME_SIZE: u32 = 128;
const D3DFMT_A8R8G8B8: u32 = 21;

mod radar {
    pub const AVERAGE_Z: usize = 0x18;
    pub const X_SAMPLE: usize = 0x20;
    pub const Y_SAMPLE: usize = 0x24;
    pub const WINDOW: usize = 0x1330;
    pub const EXTENT_LO_X: usize = 0x1334;
    pub const EXTENT_LO_Y: usize = 0x1338;
    pub const EXTENT_HI_X: usize = 0x1340;
    pub const EXTENT_HI_Y: usize = 0x1344;
    pub const OVERLAY_TEXTURE: usize = 0x1364;
    pub const SHROUD_TEXTURE: usize = 0x1370;
}

/// SurfaceClass: its Direct3D surface.
const SURFACE_D3D: usize = 0xC;

struct Addresses {
    /// W3DRadar vtable entries clearShroud and setShroudLevel(x, y, setting), thiscall.
    clear_shroud_entry: usize,
    clear_shroud: usize,
    set_shroud_entry: usize,
    set_shroud_level: usize,
    the_terrain_render_object: usize,
    /// HeightMapRenderObjClass: its W3DShroud, which has the cell size at +0x10 and +0x14.
    render_object_shroud: usize,
    /// In W3DRadar::draw: `TheGameClient->getFrame() % 6`, then a jump past the overlay redraw.
    overlay_refresh: usize,
    overlay_skip: usize,
    /// renderObjectList: `fmul [1/128]` operands, the size of a radar cell in the map.
    cell_scale_operands: [usize; 2],
    /// renderObjectList: `cmp reg, 0x80` immediates that keep dots inside the texture.
    dot_limits: [usize; 8],
    /// renderObjectList's four DrawPixel calls for a dot: (x, y), (x, y+1), (x+1, y+1), (x+1, y).
    dot_plots: [usize; 4],
    draw_pixel: usize,
    the_game_client: usize,
    /// W3DRadar::drawViewBox(x, y, width, height), thiscall, called once from W3DRadar::draw.
    view_box_call: usize,
    draw_view_box: usize,
    the_tactical_view: usize,
    the_display: usize,
    win_get_size: usize,
    win_get_screen_position: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    clear_shroud_entry: 0x0096_92E8,
    clear_shroud: 0x0077_B6F0,
    set_shroud_entry: 0x0096_92EC,
    set_shroud_level: 0x0077_B750,
    the_terrain_render_object: 0x00A4_8B30,
    render_object_shroud: 0x2844,
    overlay_refresh: 0x0077_BAA5,
    overlay_skip: 0x0077_BB0E,
    cell_scale_operands: [0x0077_A5C8, 0x0077_A5E7],
    dot_limits: [0x0077_A74D, 0x0077_A755, 0x0077_A774, 0x0077_A77C, 0x0077_A799, 0x0077_A7A1, 0x0077_A7C0, 0x0077_A7C8],
    dot_plots: [0x0077_A764, 0x0077_A78B, 0x0077_A7B0, 0x0077_A7D7],
    draw_pixel: 0x0085_2C40,
    the_game_client: 0x00A3_9B18,
    view_box_call: 0x0077_BC1A,
    draw_view_box: 0x0077_9550,
    the_tactical_view: 0x00A3_9598,
    the_display: 0x00A3_85E8,
    win_get_size: 0x004F_91E0,
    win_get_screen_position: 0x004F_90F0,
};

static GENERALS: Addresses = Addresses {
    clear_shroud_entry: 0x009A_6EFC,
    clear_shroud: 0x0075_AB80,
    set_shroud_entry: 0x009A_6F00,
    set_shroud_level: 0x0075_ABD0,
    the_terrain_render_object: 0x00A7_2B58,
    render_object_shroud: 0x2868,
    overlay_refresh: 0x0075_AF25,
    overlay_skip: 0x0075_AF87,
    cell_scale_operands: [0x0075_9AA3, 0x0075_9ABF],
    dot_limits: [0x0075_9C13, 0x0075_9C1B, 0x0075_9C38, 0x0075_9C40, 0x0075_9C5B, 0x0075_9C63, 0x0075_9C80, 0x0075_9C88],
    dot_plots: [0x0075_9C28, 0x0075_9C4D, 0x0075_9C70, 0x0075_9C95],
    draw_pixel: 0x0081_FBC0,
    the_game_client: 0x00A6_BE90,
    view_box_call: 0x0075_B093,
    draw_view_box: 0x0075_8C10,
    the_tactical_view: 0x00A6_B95C,
    the_display: 0x00A6_A9A8,
    win_get_size: 0x004F_7090,
    win_get_screen_position: 0x004F_6F70,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

fn texture_d3d() -> usize {
    game::build().map_or(0x38, |b| b.texture_d3d)
}

const SHROUD_CELL_WIDTH: usize = 0x10;
const SHROUD_CELL_HEIGHT: usize = 0x14;
const OVERLAY_REFRESH_LEN: usize = 24;
const GET_FRAME_VSLOT: usize = 0x5C / 4;
/// The overlay's size with sharp dots: four texels per radar cell.
const DOT_TEXTURE_SIZE: u32 = 512;
/// View vtable +0x2c, getScreenCornerWorldPointsAtZ(four corners, z).
const CORNERS_VSLOT: usize = 0x2C / 4;
/// Display vtable +0x80 drawLine(x1, y1, x2, y2, width, color) and +0x7c the same with two colors.
const DRAW_LINE_VSLOT: usize = 0x80 / 4;
const DRAW_GRADIENT_LINE_VSLOT: usize = 0x7C / 4;
const BOX_BRIGHT: u32 = 0xFFE1_E100;
const BOX_DARK: u32 = 0xFF9E_9E00;

static FOG_SIZE: AtomicU32 = AtomicU32::new(512);
static SHARP_UNITS: AtomicBool = AtomicBool::new(true);
/// What renderObjectList multiplies the map size by to get a radar cell, as f32 bits.
static CELL_SCALE: AtomicU32 = AtomicU32::new((1.0f32 / GAME_SIZE as f32).to_bits());
/// Overlay texels per game radar cell.
static DOT_SCALE: AtomicU32 = AtomicU32::new(1);

struct Fog {
    small: usize,
    size: u32,
    big: Rc<TextureData>,
}

struct Overlay {
    class: usize,
    small: usize,
    big: usize,
}

thread_local! {
    static FOG: RefCell<Option<Fog>> = const { RefCell::new(None) };
    static OVERLAY: RefCell<Option<Overlay>> = const { RefCell::new(None) };
    static LAST_REFRESH: Cell<Option<u64>> = const { Cell::new(None) };
}

pub const FOG_SIZES: [u32; 4] = [128, 256, 512, 1024];

pub fn set_fog_size(size: u32) {
    FOG_SIZE.store(size.clamp(GAME_SIZE, 1024).next_power_of_two(), Ordering::Relaxed);
}

pub fn set_sharp_units(on: bool) {
    SHARP_UNITS.store(on, Ordering::Relaxed);
}

pub unsafe fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    if game::call_target(a.view_box_call) != Some(a.draw_view_box) || a.dot_plots.iter().any(|&at| game::call_target(at) != Some(a.draw_pixel)) {
        log!("radar overlay calls aren't what we expect, the sharper radar layers are off");
        return;
    }
    let plots: [usize; 4] = [plot_00 as *const () as usize, plot_01 as *const () as usize, plot_11 as *const () as usize, plot_10 as *const () as usize];
    unsafe {
        game::write_u32(a.clear_shroud_entry, clear_shroud as *const () as u32);
        game::write_u32(a.set_shroud_entry, set_shroud_level as *const () as u32);
        for (at, target) in a.dot_plots.into_iter().zip(plots) {
            game::patch_call(at, target);
        }
        for at in a.cell_scale_operands {
            game::write_u32(at, CELL_SCALE.as_ptr() as u32);
        }
        game::patch_call(a.view_box_call, draw_view_box as *const () as usize);
        // push ebp (the radar); call refresh_overlay; add esp, 4; test al, al; jz skip;
        // xor edx, edx (the surface level the code after pushes)
        let mut code = [0x90u8; OVERLAY_REFRESH_LEN];
        let call_at = a.overlay_refresh + 1;
        let jump_at = a.overlay_refresh + 11;
        code[0] = 0x55;
        code[1] = 0xE8;
        code[2..6].copy_from_slice(&((refresh_overlay as *const () as isize - (call_at as isize + 5)) as i32).to_le_bytes());
        code[6..9].copy_from_slice(&[0x83, 0xC4, 0x04]);
        code[9..11].copy_from_slice(&[0x84, 0xC0]);
        code[11..13].copy_from_slice(&[0x0F, 0x84]);
        code[13..17].copy_from_slice(&((a.overlay_skip as isize - (jump_at as isize + 6)) as i32).to_le_bytes());
        code[17..19].copy_from_slice(&[0x33, 0xD2]);
        game::write_code(a.overlay_refresh, &code);
    }
}

fn read<T: Copy>(at: usize) -> T {
    unsafe { *(at as *const T) }
}

/// The game's texture behind a TextureClass, if it's ours and A8R8G8B8.
fn our_texture(class: usize) -> Option<*mut texture::Texture> {
    if class == 0 {
        return None;
    }
    let t = unsafe { texture::texture_from_ptr(read::<usize>(class + texture_d3d()) as *mut c_void) }?;
    let data = unsafe { &(*t).data };
    (data.info.d3d == D3DFMT_A8R8G8B8).then_some(t)
}

/// A new texture of ours like `like`, `size` square.
fn new_texture(like: *mut texture::Texture, size: u32) -> *mut texture::Texture {
    crate::vk::worker::run(|| unsafe {
        let data = &(*like).data;
        let device = (*like).device();
        texture::create_texture(&mut (*device).r, device, data.info, size, size, 1, data.usage, data.pool)
    })
}

// The shroud. The game keeps writing its own 128x128 texture, and we write the same cells into
// ours, which is drawn in its place. The game's memory is read on the game thread, our texture is
// written on the renderer thread.

/// Our shroud texture and its size for the game's shroud texture of `radar`, if the setting wants one.
fn fog_target(radar: usize) -> Option<(usize, u32)> {
    let class = read::<usize>(radar + radar::SHROUD_TEXTURE);
    let small = our_texture(class)?;
    Some((small as usize, FOG_SIZE.load(Ordering::Relaxed)))
}

/// Runs `f` on our shroud texture, after making it match the setting. On the renderer thread.
fn with_fog((small, want): (usize, u32), f: impl FnOnce(&TextureData, u32)) {
    let small = small as *mut texture::Texture;
    FOG.with(|fog| {
        let mut fog = fog.borrow_mut();
        if fog.as_ref().is_some_and(|s| s.small != small as usize || s.size != want) {
            let old = fog.take().unwrap();
            if old.small == small as usize {
                let data = unsafe { &(*small).data };
                data.pinned.set(false);
                data.replacement.borrow_mut().take();
            }
        }
        if fog.is_none() && want > GAME_SIZE {
            let big = new_texture(small, want);
            // Released right away: our replacement keeps the data alive.
            let data = unsafe { (*big).data.clone() };
            unsafe { com::release(big as *mut c_void) };
            let source = unsafe { &(*small).data };
            copy_scaled(source, &data, want);
            source.pinned.set(true);
            *source.replacement.borrow_mut() = Some(data.clone());
            *fog = Some(Fog { small: small as usize, size: want, big: data });
        }
        if let Some(state) = fog.as_ref() {
            f(&state.big, state.size);
        }
    });
}

/// Fills our shroud from the game's, scaled up, for when it starts in the middle of a match.
fn copy_scaled(from: &TextureData, to: &TextureData, size: u32) {
    {
        let source_levels = from.shadow.borrow();
        let mut target_levels = to.shadow.borrow_mut();
        let (Some(source), Some(target)) = (source_levels.first(), target_levels.first_mut()) else { return };
        for y in 0..size {
            for x in 0..size {
                let s = (((y * GAME_SIZE / size) * GAME_SIZE + x * GAME_SIZE / size) * 4) as usize;
                let t = ((y * size + x) * 4) as usize;
                target[t..t + 4].copy_from_slice(&[0, 0, 0, source[s + 3]]);
            }
        }
    }
    to.mark_dirty(0);
}

extern "thiscall" fn clear_shroud(radar: usize) {
    let original: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().clear_shroud) };
    original(radar);
    let Some(target) = fog_target(radar) else { return };
    crate::vk::worker::post(move || {
        with_fog(target, |big, _| {
            if let Some(level) = big.shadow.borrow_mut().first_mut() {
                level.fill(0);
            }
            big.mark_dirty(0);
        })
    });
}

extern "thiscall" fn set_shroud_level(radar: usize, x: i32, y: i32, setting: i32) {
    let original: extern "thiscall" fn(usize, i32, i32, i32) = unsafe { std::mem::transmute(addresses().set_shroud_level) };
    original(radar, x, y, setting);
    let Some(target) = fog_target(radar) else { return };
    let Some(cell) = cell_texels(radar, target.1, x, y, setting) else { return };
    crate::vk::worker::post(move || with_fog(target, |big, size| write_cell(big, size, cell)));
}

/// Our texels for one shroud cell: those whose centers lie in it, or the one under its center
/// when the cell is smaller than a texel.
fn cell_span(cell: i32, cell_size: f32, texel: f32, size: u32) -> (i32, i32) {
    let a = cell as f32 * cell_size / texel;
    let b = (cell + 1) as f32 * cell_size / texel;
    let (mut first, mut last) = (a.round() as i32, b.round() as i32 - 1);
    if last < first {
        first = ((a + b) * 0.5).floor() as i32;
        last = first;
    }
    (first.max(0), last.min(size as i32 - 1))
}

/// Our texels for a shroud cell at `size` (first and last x, first and last y) and their alpha,
/// from the game's memory.
fn cell_texels(radar: usize, size: u32, x: i32, y: i32, setting: i32) -> Option<([i32; 4], u8)> {
    let a = addresses();
    let object = read::<usize>(a.the_terrain_render_object);
    if object == 0 {
        return None;
    }
    let shroud = read::<usize>(object + a.render_object_shroud);
    if shroud == 0 {
        return None;
    }
    let scale = GAME_SIZE as f32 / size as f32;
    let (x_texel, y_texel) = (read::<f32>(radar + radar::X_SAMPLE) * scale, read::<f32>(radar + radar::Y_SAMPLE) * scale);
    if x_texel <= 0.0 || y_texel <= 0.0 {
        return None;
    }
    let (x0, x1) = cell_span(x, read::<f32>(shroud + SHROUD_CELL_WIDTH), x_texel, size);
    let (y0, y1) = cell_span(y, read::<f32>(shroud + SHROUD_CELL_HEIGHT), y_texel, size);
    // Shrouded is black, fogged half black, the same as the game's.
    let alpha = match setting {
        2 => 255,
        1 => 127,
        _ => 0,
    };
    Some(([x0, x1, y0, y1], alpha))
}

fn write_cell(big: &TextureData, size: u32, ([x0, x1, y0, y1], alpha): ([i32; 4], u8)) {
    {
        let mut shadow = big.shadow.borrow_mut();
        let Some(level) = shadow.first_mut() else { return };
        for ty in y0..=y1 {
            for tx in x0..=x1 {
                level[((ty as u32 * size + tx as u32) * 4 + 3) as usize] = alpha;
            }
        }
    }
    big.mark_dirty(0);
}

// The unit dots. With sharp dots on, the overlay's TextureClass draws into a 512x512 texture of
// ours, renderObjectList places dots in its finer cells, and each of a dot's four pixels becomes a
// 4x4 block, so dots keep their size.

/// Makes the overlay texture match the setting. Returns whether it changed.
fn sync_overlay(radar: usize) -> bool {
    let class = read::<usize>(radar + radar::OVERLAY_TEXTURE);
    if class == 0 {
        return false;
    }
    OVERLAY.with(|overlay| {
        let mut overlay = overlay.borrow_mut();
        if overlay.as_ref().is_some_and(|s| s.class != class) {
            *overlay = None;
        }
        let slot = (class + texture_d3d()) as *mut usize;
        let want_sharp = SHARP_UNITS.load(Ordering::Relaxed);
        if want_sharp == overlay.is_some() {
            return false;
        }
        if let Some(state) = overlay.take() {
            unsafe {
                *slot = state.small;
                com::release(state.big as *mut c_void);
            }
            set_dot_size(GAME_SIZE);
            return true;
        }
        let Some(small) = our_texture(class) else {
            SHARP_UNITS.store(false, Ordering::Relaxed);
            log!("the radar overlay isn't an A8R8G8B8 texture of ours, sharp dots are off");
            return false;
        };
        let big = new_texture(small, DOT_TEXTURE_SIZE);
        unsafe { *slot = big as usize };
        *overlay = Some(Overlay { class, small: small as usize, big: big as usize });
        set_dot_size(DOT_TEXTURE_SIZE);
        true
    })
}

fn set_dot_size(size: u32) {
    CELL_SCALE.store((1.0 / size as f32).to_bits(), Ordering::Relaxed);
    DOT_SCALE.store(size / GAME_SIZE, Ordering::Relaxed);
    for at in addresses().dot_limits {
        unsafe { game::write_u32(at, size) };
    }
}

/// Decides whether W3DRadar::draw redraws the dots: every 6 client frames as the game did, or
/// with sharp dots every logic step, when positions change.
extern "C" fn refresh_overlay(radar: usize) -> u8 {
    let changed = sync_overlay(radar);
    let client = read::<usize>(addresses().the_game_client);
    if client == 0 {
        return 0;
    }
    let get_frame: extern "thiscall" fn(usize) -> u32 = unsafe { std::mem::transmute(*read::<*const usize>(client).add(GET_FRAME_VSLOT)) };
    let frame = get_frame(client);
    if !SHARP_UNITS.load(Ordering::Relaxed) {
        return (frame % 6 == 0) as u8;
    }
    let step = if config::get().is_ok_and(|c| c.engine.split_logic) { frame::tick_count() } else { frame as u64 };
    let due = changed || LAST_REFRESH.with(|l| l.replace(Some(step))) != Some(step);
    due as u8
}

fn plot(surface: usize, x: i32, y: i32, color: u32, dx: i32, dy: i32) {
    let k = DOT_SCALE.load(Ordering::Relaxed) as i32;
    let draw_pixel: extern "thiscall" fn(usize, i32, i32, u32) = unsafe { std::mem::transmute(addresses().draw_pixel) };
    let ours = unsafe { texture::surface_from_ptr(read::<usize>(surface + SURFACE_D3D) as *mut c_void) };
    let Some(s) = ours.filter(|_| k > 1) else {
        draw_pixel(surface, x, y, color);
        return;
    };
    let data = unsafe { &(*s).data };
    let (left, top) = (x - dx + dx * k, y - dy + dy * k);
    let (x0, y0) = (left.max(0), top.max(0));
    let (x1, y1) = ((left + k).min(data.width as i32), (top + k).min(data.height as i32));
    if x1 <= x0 || y1 <= y0 {
        return;
    }
    let (w, h) = ((x1 - x0) as u32, (y1 - y0) as u32);
    let block: Vec<u8> = (0..w * h).flat_map(|_| color.to_le_bytes()).collect();
    let s = crate::vk::worker::Ptr(s);
    crate::vk::worker::post(move || unsafe {
        let s = s;
        (*s.0).data.write_rect(&mut (*(*s.0).device()).r, &block, (w * 4) as usize, x0 as u32, y0 as u32, w, h)
    });
}

extern "thiscall" fn plot_00(surface: usize, x: i32, y: i32, color: u32) {
    plot(surface, x, y, color, 0, 0);
}

extern "thiscall" fn plot_01(surface: usize, x: i32, y: i32, color: u32) {
    plot(surface, x, y, color, 0, 1);
}

extern "thiscall" fn plot_11(surface: usize, x: i32, y: i32, color: u32) {
    plot(surface, x, y, color, 1, 1);
}

extern "thiscall" fn plot_10(surface: usize, x: i32, y: i32, color: u32) {
    plot(surface, x, y, color, 1, 0);
}

// The camera box. The game turns its corners into whole radar cells and caches them until the
// zoom or angle changes, so the box moves in steps of a cell. We place it from the corners every
// frame.

/// Clips a line to a rectangle (Liang and Barsky).
fn clip(a: [f32; 2], b: [f32; 2], min: [f32; 2], max: [f32; 2]) -> Option<([f32; 2], [f32; 2])> {
    let d = [b[0] - a[0], b[1] - a[1]];
    let (mut t0, mut t1) = (0.0f32, 1.0f32);
    for axis in 0..2 {
        for (p, q) in [(-d[axis], a[axis] - min[axis]), (d[axis], max[axis] - a[axis])] {
            if p == 0.0 {
                if q < 0.0 {
                    return None;
                }
            } else {
                let t = q / p;
                if p < 0.0 {
                    t0 = t0.max(t);
                } else {
                    t1 = t1.min(t);
                }
            }
        }
    }
    (t0 <= t1).then(|| ([a[0] + d[0] * t0, a[1] + d[1] * t0], [a[0] + d[0] * t1, a[1] + d[1] * t1]))
}

extern "thiscall" fn draw_view_box(radar: usize, x: i32, y: i32, width: i32, height: i32) {
    let a = addresses();
    let original: extern "thiscall" fn(usize, i32, i32, i32, i32) = unsafe { std::mem::transmute(a.draw_view_box) };
    let window = read::<usize>(radar + radar::WINDOW);
    let view = read::<usize>(a.the_tactical_view);
    let display = read::<usize>(a.the_display);
    if !SHARP_UNITS.load(Ordering::Relaxed) || window == 0 || view == 0 || display == 0 {
        original(radar, x, y, width, height);
        return;
    }
    let (mut ww, mut wh, mut wx, mut wy) = (0i32, 0i32, 0i32, 0i32);
    unsafe {
        let size: extern "thiscall" fn(usize, *mut i32, *mut i32) -> i32 = std::mem::transmute(a.win_get_size);
        let position: extern "thiscall" fn(usize, *mut i32, *mut i32) -> i32 = std::mem::transmute(a.win_get_screen_position);
        size(window, &mut ww, &mut wh);
        position(window, &mut wx, &mut wy);
    }
    let map_w = read::<f32>(radar + radar::EXTENT_HI_X) - read::<f32>(radar + radar::EXTENT_LO_X);
    let map_h = read::<f32>(radar + radar::EXTENT_HI_Y) - read::<f32>(radar + radar::EXTENT_LO_Y);
    if map_w <= 0.0 || map_h <= 0.0 {
        return;
    }
    let mut corners = [[0f32; 3]; 4];
    unsafe {
        let f: extern "thiscall" fn(usize, *mut [f32; 3], *mut [f32; 3], *mut [f32; 3], *mut [f32; 3], f32) =
            std::mem::transmute(*read::<*const usize>(view).add(CORNERS_VSLOT));
        let [a, b, c, d] = &mut corners;
        f(view, a, b, c, d, read::<f32>(radar + radar::AVERAGE_Z));
    }
    // The radar shows the map with y up, as the game's radarToPixel does.
    let pixel = |c: [f32; 3]| [wx as f32 + c[0] / map_w * ww as f32, wy as f32 + (1.0 - c[1] / map_h) * wh as f32];
    let min = [wx as f32, wy as f32];
    let max = [(wx + ww) as f32, (wy + wh) as f32];
    let colors = [(BOX_BRIGHT, BOX_BRIGHT), (BOX_BRIGHT, BOX_DARK), (BOX_DARK, BOX_DARK), (BOX_DARK, BOX_BRIGHT)];
    for (i, (from, to)) in colors.into_iter().enumerate() {
        let Some((a, b)) = clip(pixel(corners[i]), pixel(corners[(i + 1) % 4]), min, max) else { continue };
        let [x1, y1, x2, y2] = [a[0], a[1], b[0], b[1]].map(|v| v.round() as i32);
        unsafe {
            let vtable = read::<*const usize>(display);
            if from == to {
                let line: extern "thiscall" fn(usize, i32, i32, i32, i32, f32, u32) = std::mem::transmute(*vtable.add(DRAW_LINE_VSLOT));
                line(display, x1, y1, x2, y2, 1.0, from);
            } else {
                let line: extern "thiscall" fn(usize, i32, i32, i32, i32, f32, u32, u32) = std::mem::transmute(*vtable.add(DRAW_GRADIENT_LINE_VSLOT));
                line(display, x1, y1, x2, y2, 1.0, from, to);
            }
        }
    }
}
