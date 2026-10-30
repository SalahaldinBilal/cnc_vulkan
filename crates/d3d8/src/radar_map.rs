//! The radar's terrain drawn from above by the game's own terrain renderer: roads, bridges, cliffs,
//! blends, water and lighting as they look in the world, and optionally the map file's objects
//! and trees, without the shroud. Done once the map has loaded, right before a frame starts, the
//! way the game draws its water reflection. See knowledge-base/radar.md.

use std::ffi::c_void;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::Instant;

use crate::vk::{com, format, texture};
use crate::{game, log, map_objects};

struct Addresses {
    /// W3DDisplay::draw: `call WW3D::Begin_Render`, cdecl (clear, clear depth, color, Zero Hour's
    /// alpha, callback).
    begin_render_call: usize,
    begin_render: usize,
    /// Zero Hour's Begin_Render takes an alpha before the callback.
    begin_render_alpha: bool,
    /// WW3D::End_Render(flip), cdecl.
    end_render: usize,
    /// DX8Wrapper::Set_Render_Target(surface, use the default depth buffer), cdecl. A null surface
    /// goes back to the back buffer. Generals' takes only the surface and keeps the depth buffer.
    set_render_target: usize,
    /// Flushes what a render left in WW3D's mesh and sorting renderers, cdecl (render info).
    ww3d_flush: usize,
    /// The game's operator new, cdecl (size).
    game_new: usize,
    /// CameraClass constructor, thiscall, no arguments.
    camera_new: usize,
    camera_size: usize,
    camera_apply: usize,
    camera_set_clip_planes: usize,
    /// CameraClass vtable: the frame update WW3D::Render calls first, and Set_Transform.
    camera_frame_update_vslot: usize,
    camera_set_transform_vslot: usize,
    camera_projection: usize,
    camera_viewport: usize,
    camera_view_plane: usize,
    camera_frustum_valid: usize,
    /// RenderInfoClass constructor, thiscall (camera).
    render_info_new: usize,
    the_terrain_render_object: usize,
    height_map_vtable: usize,
    the_water_render_object: usize,
    water_vtable: usize,
    /// RenderObjClass vtable slot Render(render info).
    render_vslot: usize,
    /// HeightMapRenderObjClass: its WorldHeightMap.
    terrain_map: usize,
    /// WorldHeightMap: the map's size.
    map_width: usize,
    map_height: usize,
    /// HeightMapRenderObjClass's W3DShroud. The water multiplies its texture in, which at map load
    /// is black everywhere not yet seen, so it's held at null while the map is drawn.
    terrain_shroud: usize,
    /// WorldHeightMap: the drawn window's size.
    draw_width: usize,
    draw_height: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    begin_render_call: 0x0073_E194,
    begin_render: 0x0083_CA10,
    begin_render_alpha: true,
    end_render: 0x0083_CFB0,
    set_render_target: 0x0083_9960,
    ww3d_flush: 0x0083_CF70,
    game_new: 0x0041_44A0,
    camera_new: 0x0085_53E0,
    camera_size: 0x38C,
    camera_apply: 0x0085_6830,
    camera_set_clip_planes: 0x0085_6D30,
    camera_frame_update_vslot: 0x2C / 4,
    camera_set_transform_vslot: 0x4C / 4,
    camera_projection: 0x94,
    camera_viewport: 0x98,
    camera_view_plane: 0xA8,
    camera_frustum_valid: 0xCC,
    render_info_new: 0x0085_8080,
    the_terrain_render_object: 0x00A4_8B30,
    height_map_vtable: 0x0096_9978,
    the_water_render_object: 0x00A4_8B40,
    water_vtable: 0x0096_82CC,
    render_vslot: 9,
    terrain_map: 0x27D4,
    map_width: 0xC,
    map_height: 0x10,
    terrain_shroud: 0x2844,
    draw_width: 0x1A_128C,
    draw_height: 0x1A_1290,
};

/// Generals' render object vtables have Add_Ref and Release_Ref first, and its CameraClass lacks a
/// field before the projection type.
static GENERALS: Addresses = Addresses {
    begin_render_call: 0x0071_726E,
    begin_render: 0x007F_EB00,
    begin_render_alpha: false,
    end_render: 0x007F_F130,
    set_render_target: 0x007F_C180,
    ww3d_flush: 0x007F_F080,
    game_new: 0x0041_42E0,
    camera_new: 0x0082_1480,
    camera_size: 0x388,
    camera_apply: 0x0082_2D50,
    camera_set_clip_planes: 0x0082_30E0,
    camera_frame_update_vslot: 0x34 / 4,
    camera_set_transform_vslot: 0x54 / 4,
    camera_projection: 0x90,
    camera_viewport: 0x94,
    camera_view_plane: 0xA4,
    camera_frustum_valid: 0xC8,
    render_info_new: 0x0082_3490,
    the_terrain_render_object: 0x00A7_2B58,
    height_map_vtable: 0x009A_5CC0,
    the_water_render_object: 0x00A7_2B70,
    water_vtable: 0x009A_5EF0,
    render_vslot: 11,
    terrain_map: 0x27C4,
    map_width: 0x8,
    map_height: 0xC,
    terrain_shroud: 0x2868,
    draw_width: 0x1A_127C,
    draw_height: 0x1A_1280,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

/// W3DView's 3D camera.
const VIEW_CAMERA: usize = 0x84;
/// CameraClass: the scene that last rendered with the camera, which the terrain's Render reads.
const CAMERA_USER_DATA: usize = 0x8C;
const PROJECTION_ORTHO: u32 = 1;
/// Room for a RenderInfoClass of either build.
const RENDER_INFO_SIZE: usize = 0x200;
/// WaterRenderObjClass's RenderObjClass part.
const WATER_RENDER_OBJECT: usize = 4;


mod radar {
    pub const EXTENT_LO_X: usize = 0x1334;
    pub const EXTENT_LO_Y: usize = 0x1338;
    pub const EXTENT_LO_Z: usize = 0x133C;
    pub const EXTENT_HI_X: usize = 0x1340;
    pub const EXTENT_HI_Y: usize = 0x1344;
    pub const EXTENT_HI_Z: usize = 0x1348;
    pub const TERRAIN_TEXTURE: usize = 0x1358;
}

/// The radar whose terrain is to be drawn at the next frame, 0 for none.
static PENDING: AtomicUsize = AtomicUsize::new(0);
static SIZE: AtomicU32 = AtomicU32::new(1024);
static CAMERA: AtomicUsize = AtomicUsize::new(0);
static OBJECTS: AtomicBool = AtomicBool::new(true);

pub fn set_size(size: u32) {
    SIZE.store(size.clamp(128, 2048).next_power_of_two(), Ordering::Relaxed);
}

pub fn set_objects(on: bool) {
    OBJECTS.store(on, Ordering::Relaxed);
}

pub unsafe fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    if game::call_target(a.begin_render_call) != Some(a.begin_render) {
        log!("W3DDisplay::draw's Begin_Render call isn't what we expect, the radar map is off");
        return;
    }
    let hook = if a.begin_render_alpha { begin_render_zero_hour as *const () as usize } else { begin_render_generals as *const () as usize };
    unsafe { game::patch_call(a.begin_render_call, hook) };
}

fn read<T: Copy>(at: usize) -> T {
    unsafe { *(at as *const T) }
}

/// Whether the terrain renderer holds the whole map, which it only does when the map was loaded
/// with DrawEntireTerrain.
pub fn whole_map_loaded() -> bool {
    holds_whole_map(read::<usize>(addresses().the_terrain_render_object))
}

/// Whether this terrain render object's window covers its whole map.
pub fn holds_whole_map(terrain: usize) -> bool {
    let a = addresses();
    if terrain == 0 || read::<usize>(terrain) != a.height_map_vtable {
        return false;
    }
    let map = read::<usize>(terrain + a.terrain_map);
    map != 0 && read::<i32>(map + a.draw_width) >= read::<i32>(map + a.map_width) && read::<i32>(map + a.draw_height) >= read::<i32>(map + a.map_height)
}

/// Draws the radar's terrain at the start of the next frame.
pub fn queue(radar: usize) {
    PENDING.store(radar, Ordering::Relaxed);
}

/// Runs before the game's own Begin_Render.
fn before_begin_render() {
    let radar = PENDING.load(Ordering::Relaxed);
    // The radar is built early in a map load, before the trees and objects are in.
    if radar != 0
        && !map_objects::loading_map()
        && let Some(scene) = scene()
    {
        PENDING.store(0, Ordering::Relaxed);
        let start = Instant::now();
        match unsafe { draw_map(radar, scene) } {
            Ok((size, None)) => log!("radar map drawn from above at {size}x{size} in {:?}", start.elapsed()),
            Ok((size, Some((drawables, models)))) => {
                log!("radar map drawn from above at {size}x{size} with {drawables} map objects ({models} models) and trees in {:?}", start.elapsed())
            }
            Err(e) => log!("radar map not drawn: {e}"),
        }
    }
}

extern "C" fn begin_render_zero_hour(clear: u32, clear_depth: u32, color: *const [f32; 3], alpha: u32, callback: usize) -> u32 {
    before_begin_render();
    let original: extern "C" fn(u32, u32, *const [f32; 3], u32, usize) -> u32 = unsafe { std::mem::transmute(addresses().begin_render) };
    original(clear, clear_depth, color, alpha, callback)
}

extern "C" fn begin_render_generals(clear: u32, clear_depth: u32, color: *const [f32; 3], callback: usize) -> u32 {
    before_begin_render();
    let original: extern "C" fn(u32, u32, *const [f32; 3], usize) -> u32 = unsafe { std::mem::transmute(addresses().begin_render) };
    original(clear, clear_depth, color, callback)
}

/// The game's Begin_Render with a clear to black and no callback.
fn begin_black() -> u32 {
    let a = addresses();
    let black = [0f32; 3];
    unsafe {
        if a.begin_render_alpha {
            let begin: extern "C" fn(u32, u32, *const [f32; 3], u32, usize) -> u32 = std::mem::transmute(a.begin_render);
            begin(1, 1, &black, 0, 0)
        } else {
            let begin: extern "C" fn(u32, u32, *const [f32; 3], usize) -> u32 = std::mem::transmute(a.begin_render);
            begin(1, 1, &black, 0)
        }
    }
}

/// The world's scene, once it has rendered through the tactical view's camera.
fn scene() -> Option<usize> {
    let view = crate::camera::view()?;
    let camera = read::<usize>(view + VIEW_CAMERA);
    if camera == 0 {
        return None;
    }
    let scene = read::<usize>(camera + CAMERA_USER_DATA);
    (scene != 0).then_some(scene)
}

unsafe fn render_object(object: usize, render_info: *mut u8) {
    unsafe {
        let render: extern "thiscall" fn(usize, *mut u8) = std::mem::transmute(*read::<*const usize>(object).add(addresses().render_vslot));
        render(object, render_info);
    }
}

/// The water areas, without the sky box the water's Render draws around the camera.
unsafe fn draw_water(render_info: *mut u8) {
    let a = addresses();
    let water = read::<usize>(a.the_water_render_object);
    let gd = game::build().map_or(0, |b| read::<usize>(b.global_data));
    if water == 0 || gd == 0 || read::<usize>(water + WATER_RENDER_OBJECT) != a.water_vtable {
        return;
    }
    let sky_box = (gd + game::global_data::SKY_BOX_SCALE) as *mut f32;
    unsafe {
        let scale = *sky_box;
        *sky_box = 0.0;
        render_object(water + WATER_RENDER_OBJECT, render_info);
        *sky_box = scale;
    }
}

/// Our camera, made once with the game's allocator and constructor.
unsafe fn camera() -> usize {
    let existing = CAMERA.load(Ordering::Relaxed);
    if existing != 0 {
        return existing;
    }
    let a = addresses();
    unsafe {
        let new: extern "C" fn(usize) -> usize = std::mem::transmute(a.game_new);
        let memory = new(a.camera_size);
        let construct: extern "thiscall" fn(usize) -> usize = std::mem::transmute(a.camera_new);
        let camera = construct(memory);
        CAMERA.store(camera, Ordering::Relaxed);
        camera
    }
}

/// The size drawn at, and with objects, how many drawables were made and how many models drawn.
unsafe fn draw_map(radar: usize, scene: usize) -> Result<(u32, Option<(usize, usize)>), &'static str> {
    if !whole_map_loaded() {
        return Err("the map wasn't loaded with the whole terrain");
    }
    let a = addresses();
    let texture_d3d = game::build().map_or(0x38, |b| b.texture_d3d);
    let terrain = read::<usize>(a.the_terrain_render_object);
    let class = read::<usize>(radar + radar::TERRAIN_TEXTURE);
    if class == 0 {
        return Err("the radar has no terrain texture");
    }
    let small = unsafe { texture::texture_from_ptr(read::<usize>(class + texture_d3d) as *mut c_void) }.ok_or("the radar's terrain texture isn't ours")?;
    let device = unsafe { (*small).device() };
    let size = SIZE.load(Ordering::Relaxed);
    let info = format::info(format::A8R8G8B8).ok_or("no A8R8G8B8")?;
    let target = crate::vk::worker::run(|| unsafe {
        texture::create_texture(&mut (*device).r, device, info, size, size, 1, texture::USAGE_RENDERTARGET, texture::POOL_DEFAULT)
    });
    let surface = unsafe { (*target).level(0) }.ok_or("the render target has no surface")?;

    let (lo_x, lo_y, lo_z) = (read::<f32>(radar + radar::EXTENT_LO_X), read::<f32>(radar + radar::EXTENT_LO_Y), read::<f32>(radar + radar::EXTENT_LO_Z));
    let (hi_x, hi_y, hi_z) = (read::<f32>(radar + radar::EXTENT_HI_X), read::<f32>(radar + radar::EXTENT_HI_Y), read::<f32>(radar + radar::EXTENT_HI_Z));
    let (half_w, half_h) = ((hi_x - lo_x) * 0.5, (hi_y - lo_y) * 0.5);
    if half_w <= 0.0 || half_h <= 0.0 {
        unsafe { com::release(target as *mut c_void) };
        return Err("the radar has no map extent");
    }
    let height = hi_z.max(lo_z) + 1000.0;
    let objects = OBJECTS.load(Ordering::Relaxed);
    let drawables = if objects { unsafe { map_objects::create() } } else { Vec::new() };
    let mut models = 0;
    unsafe {
        let set_target: extern "C" fn(usize, u32) = std::mem::transmute(a.set_render_target);
        let end: extern "C" fn(u32) -> u32 = std::mem::transmute(a.end_render);
        let flush: extern "C" fn(*mut u8) = std::mem::transmute(a.ww3d_flush);

        set_target(surface as usize, 1);
        if begin_black() != 0 {
            set_target(0, 0);
            map_objects::destroy(&drawables);
            com::release(target as *mut c_void);
            return Err("Begin_Render failed");
        }

        // Straight down from above the middle of the map, everything in view at once.
        let camera = camera();
        let vtable = read::<*const usize>(camera);
        let transform: [[f32; 4]; 3] = [[1.0, 0.0, 0.0, (lo_x + hi_x) * 0.5], [0.0, 1.0, 0.0, (lo_y + hi_y) * 0.5], [0.0, 0.0, 1.0, height]];
        let set_transform: extern "thiscall" fn(usize, *const [[f32; 4]; 3]) = std::mem::transmute(*vtable.add(a.camera_set_transform_vslot));
        set_transform(camera, &transform);
        *((camera + CAMERA_USER_DATA) as *mut usize) = scene;
        *((camera + a.camera_projection) as *mut u32) = PROJECTION_ORTHO;
        *((camera + a.camera_viewport) as *mut [f32; 4]) = [0.0, 0.0, 1.0, 1.0];
        *((camera + a.camera_view_plane) as *mut [f32; 4]) = [-half_w, -half_h, half_w, half_h];
        let clip: extern "thiscall" fn(usize, f32, f32) = std::mem::transmute(a.camera_set_clip_planes);
        clip(camera, 1.0, height - lo_z.min(0.0) + 1000.0);
        *((camera + a.camera_frustum_valid) as *mut u8) = 0;
        let frame_update: extern "thiscall" fn(usize) = std::mem::transmute(*vtable.add(a.camera_frame_update_vslot));
        frame_update(camera);
        let apply: extern "thiscall" fn(usize) = std::mem::transmute(a.camera_apply);
        apply(camera);

        let mut render_info = [0u8; RENDER_INFO_SIZE];
        let new_render_info: extern "thiscall" fn(*mut u8, usize) = std::mem::transmute(a.render_info_new);
        new_render_info(render_info.as_mut_ptr(), camera);
        let render_info = render_info.as_mut_ptr();
        let shroud = (terrain + a.terrain_shroud) as *mut usize;
        let kept_shroud = *shroud;
        *shroud = 0;
        render_object(terrain, render_info);
        if objects {
            map_objects::draw_trees(render_info);
            models = map_objects::render(scene, render_info, &drawables);
        }
        // Opaque models first, so the water goes over what stands in it.
        flush(render_info);
        draw_water(render_info);
        flush(render_info);
        *shroud = kept_shroud;

        end(0);
        set_target(0, 0);
        map_objects::destroy(&drawables);

        // The terrain passes leave alpha mostly 0 and the radar draws with it, so the copy is shown opaque.
        let opaque = format::info(format::X8R8G8B8).ok_or("no X8R8G8B8")?;
        crate::vk::worker::run(|| {
            let shown = texture::create_texture(&mut (*device).r, device, opaque, size, size, 1, 0, texture::POOL_MANAGED);
            let rendered = (*target).data.clone();
            let copy = (*shown).data.clone();
            (*device).copy_flipped(&rendered.gpu, &copy.gpu);
            com::release(shown as *mut c_void);
            *(&(*small).data).replacement.borrow_mut() = Some(copy);
        });
        com::release(target as *mut c_void);
    }
    Ok((size, objects.then_some((drawables.len(), models))))
}
