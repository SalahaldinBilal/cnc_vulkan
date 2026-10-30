//! A sharper radar terrain image. With the map drawn from above (`radar_map.rs`) the game's build
//! only queues that. Otherwise: W3DRadar::buildTerrainTexture draws the map into a 128x128
//! texture once per map. Right after, we run it again into a larger texture of ours, with the
//! radar's size and sampling step scaled and the 127 cell limit of radarToWorld raised for the
//! time, and the device draws that image wherever the game draws its own. Water, bridges and the
//! height shading stay the game's. Optionally the land colors come from the tile textures instead
//! of one color per tile. See knowledge-base/radar.md.

use std::cell::RefCell;
use std::ffi::c_void;
use std::hash::{Hash, Hasher};
use std::rc::Rc;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};
use std::time::Instant;

use crate::config::RadarTerrain;
use crate::vk::texture::TextureData;
use crate::vk::{com, texture};
use crate::{game, log};

thread_local! {
    /// The last image we drew and what it was drawn from. A map load runs the game's build two
    /// or three times in a row over the same terrain.
    static LAST: RefCell<Option<(u64, Rc<TextureData>)>> = const { RefCell::new(None) };
}

struct Addresses {
    build_terrain: usize,
    /// Calls to buildTerrainTexture from W3DRadar::newMap and refreshTerrain.
    build_calls: [usize; 2],
    /// TextureClass::Get_Surface_Level(level) on the terrain texture, in buildTerrainTexture.
    get_surface_call: usize,
    get_surface_level: usize,
    /// radarToWorld2D and radarToWorld keep radar cells below 128: `cmp reg, 0x80`, then
    /// `mov [cell], 0x7f`, for x and y in each. These are the immediates. Generals only has
    /// radarToWorld.
    cell_limits: &'static [usize],
    last_cells: &'static [usize],
    /// W3DTerrainVisual vtable +0x10, getTerrainColorAt(x, y, color), thiscall, callee pops 12.
    terrain_color_entry: usize,
    terrain_color_at: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    build_terrain: 0x0077_B160,
    build_calls: [0x0077_B155, 0x0077_BC41],
    get_surface_call: 0x0077_B1FC,
    get_surface_level: 0x0084_FE40,
    cell_limits: &[0x0049_6C7D, 0x0049_6C9B, 0x0049_6D01, 0x0049_6D1F],
    last_cells: &[0x0049_6C87, 0x0049_6CA5, 0x0049_6D0B, 0x0049_6D29],
    terrain_color_entry: 0x0096_A268,
    terrain_color_at: 0x007A_B2C0,
};

static GENERALS: Addresses = Addresses {
    build_terrain: 0x0075_A5F0,
    build_calls: [0x0075_A5E5, 0x0075_B0C1],
    get_surface_call: 0x0075_A635,
    get_surface_level: 0x0081_9E40,
    cell_limits: &[0x0049_58E1, 0x0049_58FF],
    last_cells: &[0x0049_58EB, 0x0049_5909],
    terrain_color_entry: 0x009A_7908,
    terrain_color_at: 0x0078_2080,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

fn texture_d3d() -> usize {
    game::build().map_or(0x38, |b| b.texture_d3d)
}

const GAME_SIZE: u32 = 128;

mod radar {
    pub const X_SAMPLE: usize = 0x20;
    pub const Y_SAMPLE: usize = 0x24;
    pub const TERRAIN_TEXTURE: usize = 0x1358;
    pub const TEXTURE_WIDTH: usize = 0x1374;
    pub const TEXTURE_HEIGHT: usize = 0x1378;
}

/// W3DTerrainVisual: its WorldHeightMap.
const VISUAL_HEIGHT_MAP: usize = 0x18;

/// WorldHeightMap: width, height, border, data size, tile indices, source tiles.
struct HeightMap {
    width: usize,
    height: usize,
    border: usize,
    data_size: usize,
    tiles: usize,
    source_tiles: usize,
}

static ZERO_HOUR_MAP: HeightMap = HeightMap { width: 0xC, height: 0x10, border: 0x14, data_size: 0x24, tiles: 0x44, source_tiles: 0x60 };
static GENERALS_MAP: HeightMap = HeightMap { width: 0x8, height: 0xC, border: 0x10, data_size: 0x20, tiles: 0x34, source_tiles: 0x50 };
const SOURCE_TILE_COUNT: i32 = 0x400;

/// Where a source tile keeps its BGRA pixels at each width. A tile covers 2x2 cells.
const TILE_MIPS: [(u32, usize); 7] = [(64, 0x8), (32, 0x4008), (16, 0x5008), (8, 0x5408), (4, 0x5508), (2, 0x5548), (1, 0x5558)];

static SIZE: AtomicU32 = AtomicU32::new(512);
/// A `RadarTerrain` as its index: drawn from above, from tile textures, or the game's colors.
static MODE: AtomicU8 = AtomicU8::new(0);
const MODE_MAP: u8 = 0;
const MODE_GAME: u8 = 2;
/// Our larger texture while the second build runs, 0 otherwise.
static BUILDING: AtomicUsize = AtomicUsize::new(0);
/// The tile mip the second build samples, as an index into TILE_MIPS.
static TILE_MIP: AtomicUsize = AtomicUsize::new(0);

pub const SIZES: [u32; 5] = [128, 256, 512, 1024, 2048];
/// The largest image the game's own build makes: about 1 s at 1024, four times that at 2048.
const MAX_BUILD_SIZE: u32 = 1024;

pub fn set_size(size: u32) {
    SIZE.store(size.clamp(GAME_SIZE, MAX_BUILD_SIZE).next_power_of_two(), Ordering::Relaxed);
    crate::radar_map::set_size(size);
}

pub fn set_terrain(mode: RadarTerrain) {
    MODE.store(mode as u8, Ordering::Relaxed);
    crate::zoom::set_radar_map(mode == RadarTerrain::Map);
}

pub unsafe fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    if a.build_calls.iter().any(|&at| game::call_target(at) != Some(a.build_terrain))
        || game::call_target(a.get_surface_call) != Some(a.get_surface_level)
    {
        log!("radar terrain calls aren't what we expect, the sharper radar is off");
        return;
    }
    unsafe {
        for at in a.build_calls {
            game::patch_call(at, build_terrain as *const () as usize);
        }
        game::patch_call(a.get_surface_call, surface_level as *const () as usize);
        game::write_u32(a.terrain_color_entry, terrain_color as *const () as u32);
    }
}

fn set_cell_limit(size: u32) {
    let a = addresses();
    unsafe {
        for &at in a.cell_limits {
            game::write_u32(at, size);
        }
        for &at in a.last_cells {
            game::write_u32(at, size - 1);
        }
    }
}

/// The largest tile width that gives at most two tile pixels per radar texel `texel` world units wide.
fn tile_mip(texel: f32) -> usize {
    // A tile covers two cells of 10 units, so at width w it has w / 20 pixels per unit.
    let wanted = 40.0 / texel.max(0.001);
    TILE_MIPS.iter().position(|&(w, _)| w as f32 <= wanted).unwrap_or(TILE_MIPS.len() - 1)
}

extern "thiscall" fn build_terrain(radar: usize, terrain: usize) {
    let original: extern "thiscall" fn(usize, usize) = unsafe { std::mem::transmute(addresses().build_terrain) };
    original(radar, terrain);
    let mode = MODE.load(Ordering::Relaxed);
    if mode == MODE_MAP {
        if crate::radar_map::whole_map_loaded() {
            crate::radar_map::queue(radar);
            return;
        }
        log!("this map was loaded without the whole terrain, the radar uses tile textures until the next one");
    }
    let size = SIZE.load(Ordering::Relaxed);
    let textured = mode != MODE_GAME;
    if size <= GAME_SIZE && !textured {
        return;
    }
    let start = Instant::now();
    unsafe {
        let class = *((radar + radar::TERRAIN_TEXTURE) as *const usize);
        if class == 0 {
            return;
        }
        let Some(small) = texture::texture_from_ptr(*((class + texture_d3d()) as *const *mut c_void)) else {
            log!("the radar's terrain texture isn't ours, the sharper radar is off");
            return;
        };
        // Texture data belongs to the renderer thread.
        let device = (*small).device();
        let found = crate::vk::worker::run(|| {
            let data = &(*small).data;
            let mut hasher = rustc_hash::FxHasher::default();
            (size, textured).hash(&mut hasher);
            data.shadow.borrow().first().hash(&mut hasher);
            let key = hasher.finish();
            if let Some(previous) = LAST.with(|l| l.borrow().as_ref().filter(|(k, _)| *k == key).map(|(_, d)| d.clone())) {
                *data.replacement.borrow_mut() = Some(previous);
                return None;
            }
            Some((key, texture::create_texture(&mut (*device).r, device, data.info, size, size, 1, data.usage, data.pool)))
        });
        let Some((key, big)) = found else { return };

        let field = |o: usize| (radar + o) as *mut u32;
        let saved = [radar::TEXTURE_WIDTH, radar::TEXTURE_HEIGHT, radar::X_SAMPLE, radar::Y_SAMPLE].map(|o| *field(o));
        let scale = GAME_SIZE as f32 / size as f32;
        let x_sample = f32::from_bits(saved[2]) * scale;
        let y_sample = f32::from_bits(saved[3]) * scale;
        *field(radar::TEXTURE_WIDTH) = size;
        *field(radar::TEXTURE_HEIGHT) = size;
        *field(radar::X_SAMPLE) = x_sample.to_bits();
        *field(radar::Y_SAMPLE) = y_sample.to_bits();
        set_cell_limit(size);
        let mip = tile_mip(x_sample.max(y_sample));
        TILE_MIP.store(mip, Ordering::Relaxed);
        BUILDING.store(big as usize, Ordering::Relaxed);

        original(radar, terrain);

        BUILDING.store(0, Ordering::Relaxed);
        set_cell_limit(GAME_SIZE);
        for (o, v) in [radar::TEXTURE_WIDTH, radar::TEXTURE_HEIGHT, radar::X_SAMPLE, radar::Y_SAMPLE].into_iter().zip(saved) {
            *field(o) = v;
        }
        crate::vk::worker::run(|| {
            *(&(*small).data).replacement.borrow_mut() = Some((*big).data.clone());
            LAST.with(|l| *l.borrow_mut() = Some((key, (*big).data.clone())));
        });
        com::release(big as *mut c_void);
        let colors = if textured { format!("tile textures at {} pixels per tile", TILE_MIPS[mip].0) } else { "the game's colors".to_string() };
        log!("radar terrain drawn at {size}x{size} with {colors} in {:?}", start.elapsed());
    }
}

/// While our build runs, the terrain texture hands out our larger texture's surface.
extern "thiscall" fn surface_level(class: usize, level: u32) -> usize {
    let original: extern "thiscall" fn(usize, u32) -> usize = unsafe { std::mem::transmute(addresses().get_surface_level) };
    let big = BUILDING.load(Ordering::Relaxed);
    if big == 0 {
        return original(class, level);
    }
    let slot = (class + texture_d3d()) as *mut usize;
    unsafe {
        let saved = *slot;
        *slot = big;
        let surface = original(class, level);
        *slot = saved;
        surface
    }
}

extern "thiscall" fn terrain_color(visual: usize, x: f32, y: f32, color: *mut [f32; 3]) {
    if BUILDING.load(Ordering::Relaxed) != 0
        && MODE.load(Ordering::Relaxed) != MODE_GAME
        && !color.is_null()
        && let Some(c) = unsafe { tile_color(visual, x, y) }
    {
        unsafe { *color = c };
        return;
    }
    let original: extern "thiscall" fn(usize, f32, f32, *mut [f32; 3]) = unsafe { std::mem::transmute(addresses().terrain_color_at) };
    original(visual, x, y, color);
}

/// The base tile's pixel at a world position, the way WorldHeightMap::getTerrainColorAt finds the
/// tile but from a finer mip than its 1x1.
unsafe fn tile_color(visual: usize, x: f32, y: f32) -> Option<[f32; 3]> {
    let h = game::pick(&ZERO_HOUR_MAP, Some(&GENERALS_MAP))?;
    unsafe {
        let map = *((visual + VISUAL_HEIGHT_MAP) as *const usize);
        if map == 0 {
            return None;
        }
        let int = |o: usize| *((map + o) as *const i32);
        let (width, height, border) = (int(h.width), int(h.height), int(h.border));
        let (fx, fy) = (x * 0.1, y * 0.1);
        let (u, v) = (fx - fx.floor(), fy - fy.floor());
        let column = (fx.floor() as i32 + border).clamp(0, width - 1);
        let row = (fy.floor() as i32 + border).clamp(0, height - 1);
        let ndx = row * width + column;
        let tiles = *((map + h.tiles) as *const *const i16);
        if ndx < 0 || ndx >= int(h.data_size) || tiles.is_null() {
            return None;
        }
        let tile_ndx = *tiles.add(ndx as usize) as i32;
        let source = tile_ndx >> 2;
        if !(0..SOURCE_TILE_COUNT).contains(&source) {
            return None;
        }
        let tile = *((map + h.source_tiles + source as usize * 4) as *const usize);
        if tile == 0 {
            return None;
        }
        let (w, offset) = TILE_MIPS[TILE_MIP.load(Ordering::Relaxed)];
        // The low two bits pick the cell's quarter of the tile. Rows run with world y.
        let (px, py) = if w == 1 {
            (0, 0)
        } else {
            let half = w / 2;
            let at = |quarter: i32, f: f32| quarter as u32 * half + ((f * half as f32) as u32).min(half - 1);
            (at(tile_ndx & 1, u), at((tile_ndx >> 1) & 1, v))
        };
        let p = (tile + offset + ((py * w + px) * 4) as usize) as *const u8;
        Some([*p.add(2) as f32 / 255.0, *p.add(1) as f32 / 255.0, *p as f32 / 255.0])
    }
}
