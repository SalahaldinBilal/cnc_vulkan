//! Terrain work the game redid every frame, kept until the terrain updates. See knowledge-base/performance.md.

use std::arch::naked_asm;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};

use rustc_hash::FxHashMap;

use crate::{game, log};

/// How the game reaches HeightMapRenderObjClass::updateBlock (thiscall, 6 arguments).
enum UpdateBlockHook {
    /// Zero Hour: virtual.
    Entry(usize),
    Calls(&'static [usize]),
}

struct Roads {
    /// `call loadRoadsInVertexAndIndexBuffers` in drawRoads, thiscall, no arguments.
    load_call: usize,
    load: usize,
}

struct RoadClip {
    /// `call W3DRoadBuffer::drawRoads` in the terrain renders, thiscall, nine arguments.
    calls: &'static [usize],
    draw: usize,
    /// TheTerrainRenderObject and its WorldHeightMap.
    terrain: usize,
    map: usize,
    /// WorldHeightMap width, height and border, in cells.
    map_width: usize,
    map_height: usize,
    map_border: usize,
}

struct Edging {
    /// `call clearAllEdging` in updateCenter.
    clear_call: usize,
    clear: usize,
    /// `call loadEdgingsInVertexAndIndexBuffers(map, minX, maxX, minY, maxY)` in drawEdging, thiscall.
    load_call: usize,
    load: usize,
}

struct Addresses {
    update_block: UpdateBlockHook,
    update_block_fn: usize,
    /// getStaticDiffuse(x, y) calls in renderExtraBlendTiles, thiscall.
    blend_tile_diffuse_calls: [usize; 4],
    get_static_diffuse: usize,
    roads: Option<Roads>,
    road_clip: RoadClip,
    edging: Option<Edging>,
}

static ZERO_HOUR: Addresses = Addresses {
    update_block: UpdateBlockHook::Entry(0x0096_9B74),
    update_block_fn: 0x0079_B3F0,
    blend_tile_diffuse_calls: [0x0079_DC70, 0x0079_DCDD, 0x0079_DD50, 0x0079_DDAA],
    get_static_diffuse: 0x0075_4CB0,
    roads: None,
    road_clip: RoadClip {
        calls: &[0x0079_D587, 0x0079_854C],
        draw: 0x0079_5D00,
        terrain: 0x00A4_8B30,
        map: 0x27D4,
        map_width: 0x0C,
        map_height: 0x10,
        map_border: 0x14,
    },
    edging: None,
};

static GENERALS: Addresses = Addresses {
    update_block: UpdateBlockHook::Calls(&[0x0073_0FF1, 0x0073_252B, 0x0073_2CC2, 0x0073_2CF9, 0x0073_2DEC, 0x0073_2E5B]),
    update_block_fn: 0x0072_CE00,
    blend_tile_diffuse_calls: [0x0073_4DB7, 0x0073_4E15, 0x0073_4E77, 0x0073_4EC2],
    get_static_diffuse: 0x0073_1A60,
    roads: Some(Roads { load_call: 0x0077_4EDC, load: 0x0076_E1D0 }),
    road_clip: RoadClip {
        calls: &[0x0073_3A1D],
        draw: 0x0077_4DD0,
        terrain: 0x00A7_2B58,
        map: 0x27C4,
        map_width: 0x08,
        map_height: 0x0C,
        map_border: 0x10,
    },
    edging: Some(Edging { clear_call: 0x0073_24E9, clear: 0x0077_5780, load_call: 0x0077_57BE, load: 0x0077_5070 }),
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

/// W3DRoadBuffer fields (Generals).
mod road {
    pub const TYPES: usize = 0x00;
    pub const INITIALIZED: usize = 0x0C;
    pub const BOUNDS: usize = 0x18;
    pub const CURRENT_TYPE: usize = 0x2C;
    pub const TYPE_SIZE: usize = 0x24;
    pub const TYPE_VB: usize = 0x04;
    pub const TYPE_IB: usize = 0x08;
}

/// Counts updateBlock calls: map loads, lighting changes, recentering and height changes.
static UPDATE_BLOCKS: AtomicU32 = AtomicU32::new(0);
static UPDATE_BLOCK: AtomicUsize = AtomicUsize::new(0);
static GET_STATIC_DIFFUSE: AtomicUsize = AtomicUsize::new(0);
static LOAD_ROADS: AtomicUsize = AtomicUsize::new(0);
static DRAW_ROADS: AtomicUsize = AtomicUsize::new(0);
static CLEAR_EDGING: AtomicUsize = AtomicUsize::new(0);
static LOAD_EDGING: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    let calls_ok = |sites: &[usize], target: usize| sites.iter().all(|&at| game::call_target(at) == Some(target));
    if calls_ok(a.road_clip.calls, a.road_clip.draw) {
        DRAW_ROADS.store(a.road_clip.draw, Ordering::Relaxed);
        for &at in a.road_clip.calls {
            unsafe { game::patch_call(at, draw_roads as *const () as usize) };
        }
    } else {
        log!("drawRoads calls aren't where expected, roads not clipped to the terrain");
    }
    let sites_ok = match a.update_block {
        UpdateBlockHook::Entry(_) => true,
        UpdateBlockHook::Calls(sites) => calls_ok(sites, a.update_block_fn),
    };
    if !sites_ok || !calls_ok(&a.blend_tile_diffuse_calls, a.get_static_diffuse) {
        log!("terrain calls aren't where expected, terrain caches off");
        return;
    }
    UPDATE_BLOCK.store(a.update_block_fn, Ordering::Relaxed);
    GET_STATIC_DIFFUSE.store(a.get_static_diffuse, Ordering::Relaxed);
    unsafe {
        match a.update_block {
            UpdateBlockHook::Entry(entry) => game::write_u32(entry, update_block_stub as *const () as u32),
            UpdateBlockHook::Calls(sites) => {
                for &at in sites {
                    game::patch_call(at, update_block_stub as *const () as usize);
                }
            }
        }
        for at in a.blend_tile_diffuse_calls {
            game::patch_call(at, static_diffuse as *const () as usize);
        }
    }
    let mut cached = vec!["blend tile lighting"];
    if let Some(r) = &a.roads
        && game::call_target(r.load_call) == Some(r.load)
    {
        LOAD_ROADS.store(r.load, Ordering::Relaxed);
        unsafe { game::patch_call(r.load_call, load_roads as *const () as usize) };
        cached.push("roads");
    }
    if let Some(e) = &a.edging
        && game::call_target(e.clear_call) == Some(e.clear)
        && game::call_target(e.load_call) == Some(e.load)
    {
        CLEAR_EDGING.store(e.clear, Ordering::Relaxed);
        LOAD_EDGING.store(e.load, Ordering::Relaxed);
        unsafe {
            game::patch_call(e.clear_call, skip_edging_clear as *const () as usize);
            game::patch_call(e.load_call, load_edging as *const () as usize);
        }
        cached.push("custom edging");
    }
    log!("terrain kept between updates: {}", cached.join(", "));
}

#[unsafe(naked)]
unsafe extern "C" fn update_block_stub() {
    naked_asm!(
        "lock inc dword ptr [{count}]",
        "jmp dword ptr [{target}]",
        count = sym UPDATE_BLOCKS,
        target = sym UPDATE_BLOCK,
    )
}

fn update_blocks() -> u32 {
    UPDATE_BLOCKS.load(Ordering::Relaxed)
}

struct DiffuseCache {
    terrain: usize,
    update_blocks: u32,
    colors: FxHashMap<(u32, u32), u32>,
}

static DIFFUSE: Mutex<Option<DiffuseCache>> = Mutex::new(None);

/// getStaticDiffuse reads only heights and static lights, which change with an updateBlock.
extern "thiscall" fn static_diffuse(terrain: usize, x: u32, y: u32) -> u32 {
    let mut guard = DIFFUSE.lock().unwrap();
    let blocks = update_blocks();
    let cache = guard.get_or_insert_with(|| DiffuseCache { terrain, update_blocks: blocks, colors: FxHashMap::default() });
    if cache.terrain != terrain || cache.update_blocks != blocks {
        cache.terrain = terrain;
        cache.update_blocks = blocks;
        cache.colors.clear();
    }
    *cache.colors.entry((x, y)).or_insert_with(|| {
        let original: extern "thiscall" fn(usize, u32, u32) -> u32 =
            unsafe { std::mem::transmute(GET_STATIC_DIFFUSE.load(Ordering::Relaxed)) };
        original(terrain, x, y)
    })
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct RoadKey {
    buffer: usize,
    road_type: usize,
    vertices: usize,
    indices: usize,
    initialized: u8,
    bounds: [u32; 4],
    update_blocks: u32,
}

static ROADS: Mutex<Vec<RoadKey>> = Mutex::new(Vec::new());

/// Skips the refill when the road type's buffers already hold this draw window's roads.
extern "thiscall" fn load_roads(buffer: usize) {
    let key = unsafe {
        let road_type = *((buffer + road::CURRENT_TYPE) as *const usize);
        let entry = *((buffer + road::TYPES) as *const usize) + road_type * road::TYPE_SIZE;
        RoadKey {
            buffer,
            road_type,
            vertices: *((entry + road::TYPE_VB) as *const usize),
            indices: *((entry + road::TYPE_IB) as *const usize),
            initialized: *((buffer + road::INITIALIZED) as *const u8),
            bounds: *((buffer + road::BOUNDS) as *const [u32; 4]),
            update_blocks: update_blocks(),
        }
    };
    let mut roads = ROADS.lock().unwrap();
    if roads.contains(&key) {
        return;
    }
    roads.retain(|k| !(k.buffer == key.buffer && k.road_type == key.road_type));
    roads.push(key);
    drop(roads);
    let load: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(LOAD_ROADS.load(Ordering::Relaxed)) };
    load(buffer);
}

/// World units per terrain cell (MAP_XY_FACTOR).
const CELL_SIZE: f32 = 10.0;

/// The terrain mesh's extent in cells from the playable area's corner (min x, max x, min y, max y).
fn heightmap_extent() -> Option<[i32; 4]> {
    let c = &addresses().road_clip;
    let read = |p: usize| unsafe { *(p as *const usize) };
    let terrain = read(c.terrain);
    if terrain == 0 {
        return None;
    }
    let map = read(terrain + c.map);
    if map == 0 {
        return None;
    }
    let (width, height, border) = (read(map + c.map_width) as i32, read(map + c.map_height) as i32, read(map + c.map_border) as i32);
    Some([-border, width - 1 - border, -border, height - 1 - border])
}

/// Only the terrain's shroud pass darkens roads, so road past the terrain's edge has to go.
extern "thiscall" fn draw_roads(
    buffer: usize,
    camera: usize,
    cloud: usize,
    noise: usize,
    no_textures: u32,
    min_x: i32,
    max_x: i32,
    min_y: i32,
    max_y: i32,
    lights: usize,
) {
    let draw: extern "thiscall" fn(usize, usize, usize, usize, u32, i32, i32, i32, i32, usize) =
        unsafe { std::mem::transmute(DRAW_ROADS.load(Ordering::Relaxed)) };
    let call = || draw(buffer, camera, cloud, noise, no_textures, min_x, max_x, min_y, max_y, lights);
    let Some([map_x0, map_x1, map_y0, map_y1]) = heightmap_extent() else { return call() };
    let x0 = min_x.max(map_x0) as f32 * CELL_SIZE;
    let x1 = max_x.min(map_x1) as f32 * CELL_SIZE;
    let y0 = min_y.max(map_y0) as f32 * CELL_SIZE;
    let y1 = max_y.min(map_y1) as f32 * CELL_SIZE;
    let planes = [[1.0, 0.0, 0.0, -x0], [-1.0, 0.0, 0.0, x1], [0.0, 1.0, 0.0, -y0], [0.0, -1.0, 0.0, y1]];
    crate::vk::device::with_clip_planes(&planes, call);
}

extern "thiscall" fn skip_edging_clear(_edging: usize) {}

static EDGING: Mutex<Option<(usize, usize, [i32; 4], u32)>> = Mutex::new(None);

/// Clears the edging only when its map, window or the terrain changed. Other clears (map resets) still count.
extern "thiscall" fn load_edging(edging: usize, map: usize, min_x: i32, max_x: i32, min_y: i32, max_y: i32) {
    let key = (edging, map, [min_x, max_x, min_y, max_y], update_blocks());
    if EDGING.lock().unwrap().replace(key) != Some(key) {
        let clear: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(CLEAR_EDGING.load(Ordering::Relaxed)) };
        clear(edging);
    }
    let load: extern "thiscall" fn(usize, usize, i32, i32, i32, i32) =
        unsafe { std::mem::transmute(LOAD_EDGING.load(Ordering::Relaxed)) };
    load(edging, map, min_x, max_x, min_y, max_y);
}
