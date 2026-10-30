//! How far and how fast the player can zoom the camera out. The view keeps the game's own maximum
//! height, which the default zoom and scripted cameras use, and only setHeightAboveGround (what
//! zoom input goes through) clamps to our higher one. While that is above the game's, or the radar
//! map is drawn from the terrain, the whole terrain is drawn, since the usual 129 cell window
//! around the camera would show its edge. See knowledge-base/camera.md.

use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, Ordering};

use crate::game;

/// Where the game calls oversizeTerrain: a vtable entry in Zero Hour, a direct call in Generals.
enum OversizeHook {
    Entry(usize),
    Call(usize),
}

struct Addresses {
    /// W3DView vtable entry setHeightAboveGround(height), thiscall, callee pops 4.
    set_height_entry: usize,
    set_height: usize,
    /// View::zoomIn and zoomOut: `fsub [step]` and `fadd [step]`, the step per wheel notch.
    zoom_step_reads: [usize; 2],
    /// The game's step, 10 world units.
    game_zoom_step: usize,
    /// `mov eax, [TheGlobalData]` before `cmp [eax+0x3f]` (DrawEntireTerrain): in the WorldHeightMap
    /// constructor, which sizes the terrain window when a map loads, and in setCameraTransform,
    /// which picks the far plane.
    terrain_flag_loads: [usize; 2],
    /// WorldHeightMap::setDrawOrg: `cmp byte [esi+0x3f], 0` resizes the window to the whole map. The
    /// terrain's buffers keep the size the map loaded with, so it may only happen at a map load.
    draw_org_check: usize,
    /// HeightMapRenderObjClass::oversizeTerrain(tiles), thiscall, callee pops 4. It sets the window
    /// to 129 cells plus 32 per tile, which mission scripts use around cinematics.
    oversize_hook: OversizeHook,
    oversize_terrain: usize,
    /// calcCameraConstraints, thiscall: its call in setCameraTransform and the view vtable entry
    /// (a jump thunk to it).
    constraints_call: usize,
    constraints_entry: usize,
    calc_constraints: usize,
    /// TheTerrainLogic, whose vtable +0x1c is getExtent(Region3D *).
    the_terrain_logic: usize,
    /// View fields: camera offset z, ground level, the limits (lo x, lo y, hi x, hi y), and Zero
    /// Hour's real zoom mode, which zooms by field of view.
    view_offset_z: usize,
    view_ground_level: usize,
    view_limits: usize,
    view_real_zoom: Option<usize>,
}

static ZERO_HOUR: Addresses = Addresses {
    set_height_entry: 0x0096_8C58,
    set_height: 0x0076_E980,
    zoom_step_reads: [0x0044_DB3C, 0x0044_DB6C],
    game_zoom_step: 0x0094_5EC8,
    terrain_flag_loads: [0x0074_414C, 0x0076_CBA5],
    draw_org_check: 0x0074_757E,
    oversize_hook: OversizeHook::Entry(0x0096_9B68),
    oversize_terrain: 0x0079_B710,
    constraints_call: 0x0076_CC1D,
    constraints_entry: 0x0096_8CD0,
    calc_constraints: 0x0076_C810,
    the_terrain_logic: 0x00A3_9594,
    view_offset_z: 0x478,
    view_ground_level: 0x494,
    view_limits: 0x498,
    view_real_zoom: Some(0x4AA),
};

static GENERALS: Addresses = Addresses {
    set_height_entry: 0x009A_6864,
    set_height: 0x0074_ECE0,
    zoom_step_reads: [0x0044_F6FC, 0x0044_F72C],
    game_zoom_step: 0x0098_9EE8,
    terrain_flag_loads: [0x0071_D778, 0x0074_CF33],
    draw_org_check: 0x0072_06CE,
    oversize_hook: OversizeHook::Call(0x0072_8ADF),
    oversize_terrain: 0x0072_FA80,
    constraints_call: 0x0074_CFBB,
    constraints_entry: 0x009A_68DC,
    calc_constraints: 0x0074_CBC0,
    the_terrain_logic: 0x00A6_B958,
    view_offset_z: 0x538,
    view_ground_level: 0x554,
    view_limits: 0x558,
    view_real_zoom: None,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

const GD_DRAW_ENTIRE_TERRAIN: u32 = 0x3F;
/// View field: the highest the camera may be above the ground.
const VIEW_MAX_HEIGHT: usize = 0x38;
const VIEW_ZOOM: usize = 0x40;
/// View field: the terrain height under the camera, which the zoom is measured from.
const VIEW_TERRAIN_HEIGHT: usize = 0x58;

/// How much higher the player may zoom out than the game allows, as f32 bits.
static FACTOR: AtomicU32 = AtomicU32::new(0x3F80_0000);
/// The zoom step as f32 bits. View::zoomIn and zoomOut read it in place of the game's.
static STEP: AtomicU32 = AtomicU32::new(0x4120_0000);
/// Zoom speed in percent of the game's.
static SPEED: AtomicU32 = AtomicU32::new(100);
/// DrawEntireTerrain as the game reads it.
static WHOLE_TERRAIN: AtomicU8 = AtomicU8::new(0);
static RADAR_MAP: AtomicBool = AtomicBool::new(false);

pub const MIN_PERCENT: u32 = 100;
pub const MAX_PERCENT: u32 = 400;
pub const MIN_SPEED: u32 = 25;
pub const MAX_SPEED: u32 = 400;

pub unsafe fn install() {
    if game::build().is_none() {
        return;
    }
    let z = addresses();
    let [a, b, c, d] = (WHOLE_TERRAIN.as_ptr() as u32 - GD_DRAW_ENTIRE_TERRAIN).to_le_bytes();
    let [s0, s1, s2, s3] = (STEP.as_ptr() as u32).to_le_bytes();
    unsafe {
        game::write_u32(z.set_height_entry, set_height_above_ground as *const () as u32);
        for at in z.zoom_step_reads {
            game::write_code(at + 2, &[s0, s1, s2, s3]);
        }
        for at in z.terrain_flag_loads {
            game::write_code(at, &[0xB8, a, b, c, d]);
        }
        // cmp esi, esi: always equal, so the window keeps its size.
        game::write_code(z.draw_org_check, &[0x3B, 0xF6, 0x90, 0x90]);
        match z.oversize_hook {
            OversizeHook::Entry(entry) => game::write_u32(entry, oversize_terrain as *const () as u32),
            OversizeHook::Call(at) => game::patch_call(at, oversize_terrain as *const () as usize),
        }
        if game::call_target(z.constraints_call) == Some(z.calc_constraints) {
            game::patch_call(z.constraints_call, calc_camera_constraints as *const () as usize);
            game::write_u32(z.constraints_entry, calc_camera_constraints as *const () as u32);
        } else {
            crate::log!("calcCameraConstraints call isn't where expected, camera limits as the game's");
        }
    }
    update_step();
}

/// Sets the highest the player can zoom out, in percent of the game's own limit. Applies at once,
/// the whole terrain from the next map on.
pub fn set_percent(percent: u32) {
    let factor = percent.clamp(MIN_PERCENT, MAX_PERCENT) as f32 / 100.0;
    FACTOR.store(factor.to_bits(), Ordering::Relaxed);
    update_whole_terrain();
    update_step();
}

/// The radar map is drawn from the terrain renderer, so it needs the whole map loaded too.
pub fn set_radar_map(on: bool) {
    RADAR_MAP.store(on, Ordering::Relaxed);
    update_whole_terrain();
}

fn update_whole_terrain() {
    let zoomed = f32::from_bits(FACTOR.load(Ordering::Relaxed)) > 1.0;
    WHOLE_TERRAIN.store((zoomed || RADAR_MAP.load(Ordering::Relaxed)) as u8, Ordering::Relaxed);
}

/// Sets how far each zoom step goes, in percent of the game's.
pub fn set_speed(percent: u32) {
    SPEED.store(percent.clamp(MIN_SPEED, MAX_SPEED), Ordering::Relaxed);
    update_step();
}

/// The height one wheel notch, or a held zoom key over one 30 Hz frame, moves the camera. It
/// grows with the zoom out limit, so the whole range takes as many steps as the game's did.
pub fn step() -> f32 {
    f32::from_bits(STEP.load(Ordering::Relaxed))
}

fn update_step() {
    let game_step = if game::build().is_some() { unsafe { *(addresses().game_zoom_step as *const f32) } } else { 10.0 };
    let factor = f32::from_bits(FACTOR.load(Ordering::Relaxed));
    let speed = SPEED.load(Ordering::Relaxed) as f32 / 100.0;
    STEP.store((game_step * factor * speed).to_bits(), Ordering::Relaxed);
}

/// A terrain that holds the whole map already covers any oversized window, and the game's
/// oversizeTerrain would shrink it back to 129 cells around the camera.
extern "thiscall" fn oversize_terrain(terrain: usize, tiles: i32) {
    if crate::radar_map::holds_whole_map(terrain) {
        return;
    }
    let original: extern "thiscall" fn(usize, i32) = unsafe { std::mem::transmute(addresses().oversize_terrain) };
    original(terrain, tiles);
}

/// The game keeps the camera a margin inside the map that grows with the camera's height, so above
/// the game's own maximum the margin is scaled back to what it is there.
extern "thiscall" fn calc_camera_constraints(view: usize) {
    let a = addresses();
    let original: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(a.calc_constraints) };
    original(view);
    let field = |offset: usize| unsafe { *((view + offset) as *const f32) };
    if a.view_real_zoom.is_some_and(|offset| unsafe { *((view + offset) as *const u8) } != 0) {
        return;
    }
    let (terrain, ground, limit) = (field(VIEW_TERRAIN_HEIGHT), field(a.view_ground_level), field(VIEW_MAX_HEIGHT));
    let eye = field(VIEW_ZOOM) * field(a.view_offset_z);
    if !(eye - terrain > limit) {
        return;
    }
    let logic = unsafe { *(a.the_terrain_logic as *const usize) };
    if logic == 0 {
        return;
    }
    let mut extent = [0f32; 6];
    unsafe {
        let get_extent: extern "thiscall" fn(usize, *mut [f32; 6]) = std::mem::transmute(*(*(logic as *const *const usize)).add(0x1C / 4));
        get_extent(logic, &mut extent);
    }
    let limits = (view + a.view_limits) as *mut [f32; 4];
    let margin = unsafe { (*limits)[0] } - extent[0];
    // The game's margin is the length of (dx, dy, ground level), where only dx and dy scale with the height.
    let reach_squared = margin * margin - ground * ground;
    let (above_ground, at_limit) = (eye - ground, limit + terrain - ground);
    if margin <= 0.0 || reach_squared <= 0.0 || above_ground <= 0.0 || at_limit <= 0.0 {
        return;
    }
    let reach = reach_squared.sqrt() * at_limit / above_ground;
    let margin = (reach * reach + ground * ground).sqrt();
    unsafe { *limits = [extent[0] + margin, extent[1] + margin, extent[3] - margin, extent[4] - margin] };
}

/// The game's setHeightAboveGround, clamping to our maximum instead of the view's.
extern "thiscall" fn set_height_above_ground(view: usize, height: f32) {
    let original: extern "thiscall" fn(usize, f32) = unsafe { std::mem::transmute(addresses().set_height) };
    let max = (view + VIEW_MAX_HEIGHT) as *mut f32;
    unsafe {
        let limit = *max;
        *max = limit * f32::from_bits(FACTOR.load(Ordering::Relaxed));
        original(view, height);
        *max = limit;
    }
}
