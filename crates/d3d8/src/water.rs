//! WaterRenderObjClass::update runs every frame with the client. It scrolls three offsets by a
//! fixed step per call, which we scale by real frame time, and flips the bump texture once per
//! call, which we hold to 30 per second. The wave grid simulates once per logic step, so its
//! heights are blended between steps while rendering. We only write to the water object while
//! the terrain visual still points at it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::{frame, game};

struct Addresses {
    /// The terrain visual update's tail jump into WaterRenderObjClass::update, water in ECX.
    water_update_jump: usize,
    water_update: usize,
    the_terrain_visual: usize,
    /// Operands of the three `fadd [step]` in the update (river V origin, then the X and Y offsets).
    scroll_step_operands: [usize; 3],
    bump_frame: usize,
    grid_enabled: usize,
    grid_cells: usize,
    grid_columns: usize,
    grid_rows: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    water_update_jump: 0x007A_ABE0,
    water_update: 0x0075_8020,
    the_terrain_visual: 0x00A3_A1A4,
    scroll_step_operands: [0x0075_8042, 0x0075_8054, 0x0075_8066],
    bump_frame: 0x210,
    grid_enabled: 0x22D,
    grid_cells: 0x224,
    grid_columns: 0x26C,
    grid_rows: 0x270,
};

static GENERALS: Addresses = Addresses {
    water_update_jump: 0x0078_19F0,
    water_update: 0x0073_6D20,
    the_terrain_visual: 0x00A6_C4CC,
    scroll_step_operands: [0x0073_6D42, 0x0073_6D54, 0x0073_6D66],
    bump_frame: 0x20C,
    grid_enabled: 0x229,
    grid_cells: 0x220,
    grid_columns: 0x268,
    grid_rows: 0x26C,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

const TERRAIN_VISUAL_WATER: usize = 0x14;
const BUMP_FRAMES: i32 = 32;

mod grid {
    /// The stored counts leave out a 3 cell border.
    pub const BORDER: usize = 3;
    /// Height, velocity, flags.
    pub const CELL_SIZE: usize = 12;
}

/// The game's steps, then this frame's scaled ones as f32 bits, which the update reads.
static BASE_STEPS: Mutex<[f32; 3]> = Mutex::new([0.0; 3]);
static STEPS: [AtomicU32; 3] = [AtomicU32::new(0), AtomicU32::new(0), AtomicU32::new(0)];
/// Bump flips owed, carried between frames.
static BUMP_DEBT: Mutex<f32> = Mutex::new(0.0);

struct Grid {
    water: usize,
    cells: usize,
    before: Vec<f32>,
    after: Vec<f32>,
    update: u64,
    saved: Option<Vec<f32>>,
}

static GRID: Mutex<Option<Grid>> = Mutex::new(None);

pub unsafe fn install() {
    let a = addresses();
    let mut base = BASE_STEPS.lock().unwrap();
    for (i, at) in a.scroll_step_operands.iter().enumerate() {
        unsafe {
            base[i] = **(*at as *const *const f32);
            STEPS[i].store(base[i].to_bits(), Ordering::Relaxed);
            game::write_u32(*at, STEPS[i].as_ptr() as u32);
        }
    }
    unsafe { game::patch_jump(a.water_update_jump, water_update as *const () as usize) };
}

/// The wave grid's cell array and cell count, when the map uses one.
fn grid_cells(water: usize) -> Option<(usize, usize)> {
    let a = addresses();
    unsafe {
        if *((water + a.grid_enabled) as *const u8) == 0 {
            return None;
        }
        let cells = *((water + a.grid_cells) as *const usize);
        let columns = *((water + a.grid_columns) as *const i32);
        let rows = *((water + a.grid_rows) as *const i32);
        if cells == 0 || columns < 0 || rows < 0 {
            return None;
        }
        Some((cells, (columns as usize + grid::BORDER) * (rows as usize + grid::BORDER)))
    }
}

fn read_heights(cells: usize, count: usize) -> Vec<f32> {
    (0..count).map(|i| unsafe { *((cells + i * grid::CELL_SIZE) as *const f32) }).collect()
}

fn write_heights(cells: usize, heights: &[f32]) {
    for (i, h) in heights.iter().enumerate() {
        unsafe { *((cells + i * grid::CELL_SIZE) as *mut f32) = *h };
    }
}

fn current_water() -> usize {
    unsafe {
        let visual = *(addresses().the_terrain_visual as *const usize);
        if visual == 0 { 0 } else { *((visual + TERRAIN_VISUAL_WATER) as *const usize) }
    }
}

extern "thiscall" fn water_update(water: usize) {
    let original: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().water_update) };
    let scale = frame::frame_scale();
    for (step, base) in STEPS.iter().zip(*BASE_STEPS.lock().unwrap()) {
        step.store((base * scale).to_bits(), Ordering::Relaxed);
    }
    let flips = {
        let mut debt = BUMP_DEBT.lock().unwrap();
        *debt += scale;
        let whole = debt.floor();
        *debt -= whole;
        whole as i32
    };
    let bump = (water + addresses().bump_frame) as *mut i32;
    let bump_before = unsafe { *bump };
    let grid_before = grid_cells(water).map(|(cells, count)| (cells, count, read_heights(cells, count)));
    original(water);
    unsafe { *bump = (bump_before + flips).rem_euclid(BUMP_FRAMES) };
    let Some((cells, count, before)) = grid_before else { return };
    let after = read_heights(cells, count);
    if grid_cells(water) == Some((cells, count)) && before != after {
        *GRID.lock().unwrap() = Some(Grid { water, cells, before, after, update: frame::update_count(), saved: None });
    }
}

/// Called right before the scene renders.
pub fn apply(t: f32) {
    let update = frame::update_count();
    let mut state = GRID.lock().unwrap();
    let Some(g) = state.as_mut() else { return };
    if g.update != update || t >= 1.0 || g.saved.is_some() || current_water() != g.water {
        return;
    }
    if grid_cells(g.water).map(|(cells, _)| cells) != Some(g.cells) {
        return;
    }
    g.saved = Some(read_heights(g.cells, g.after.len()));
    let blended: Vec<f32> = g.before.iter().zip(&g.after).map(|(a, b)| a + (b - a) * t).collect();
    write_heights(g.cells, &blended);
}

/// Puts back the heights the blend replaced.
pub fn restore() {
    let mut state = GRID.lock().unwrap();
    let Some(g) = state.as_mut() else { return };
    let Some(saved) = g.saved.take() else { return };
    if current_water() == g.water {
        write_heights(g.cells, &saved);
    }
}
