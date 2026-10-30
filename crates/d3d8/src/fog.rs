//! The fog of war in the world. W3DShroud keeps the fog as a texture with one texel per terrain
//! cell, which the terrain samples through a projection. We mark that texture so the device draws
//! a smoothed copy four times its size in its place. Objects the game counts as fully visible get
//! no fog at all, so where one reaches into the fog we draw it with the game's fog pass too. See
//! knowledge-base/radar.md.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::{game, log};
use crate::vk::texture;

/// TheTerrainRenderObject and HeightMapRenderObjClass's W3DShroud.
static ZERO_HOUR: (usize, usize) = (0x00A4_8B30, 0x2844);
static GENERALS: (usize, usize) = (0x00A7_2B58, 0x2868);
/// W3DShroud: the TextureClass the terrain samples.
const SHROUD_TEXTURE: usize = 0x28;

/// W3DShroud: cell counts, cell size, the source surface, and its data (R5G6B5, one per cell) and pitch.
mod shroud {
    pub const CELLS_X: usize = 0x00;
    pub const CELLS_Y: usize = 0x04;
    pub const CELL_WIDTH: usize = 0x10;
    pub const CELL_HEIGHT: usize = 0x14;
    pub const SOURCE: usize = 0x1C;
    pub const DATA: usize = 0x20;
    pub const PITCH: usize = 0x24;
    /// A fully visible cell. Any other value darkens the terrain.
    pub const CLEAR: u16 = 0xFFFF;
}

struct ObjectFog {
    /// renderOneObject's `cmp dword [esp+0x14], 1` on the object's shroud status before `jle`
    /// to the plain render, and where the render object is on the stack there.
    status_compare: usize,
    render_object: usize,
    /// renderOneObject's ghost object block (render with the fogged lights, then jump to `end`),
    /// with the scene in EBP, the render object in EDI and the RenderInfoClass at `[esp+rinfo]`.
    ghost_block: usize,
    ghost_block_len: usize,
    end: usize,
    rinfo: usize,
    /// RenderObjClass vtable: Get_Bounding_Sphere (the world space center and radius) and Render(rinfo).
    bounding_sphere_vslot: usize,
    render_vslot: usize,
    /// RenderInfoClass::Push_Material_Pass(pass) and Pop_Material_Pass, thiscall.
    push_material_pass: usize,
    pop_material_pass: usize,
    /// RTS3DScene: the default and fogged light environments, the custom pass mode and the shroud material pass.
    scene_default_lights: usize,
    scene_fogged_lights: usize,
    scene_pass_mode: usize,
    scene_shroud_pass: usize,
}

static ZERO_HOUR_OBJECTS: ObjectFog = ObjectFog {
    status_compare: 0x0076_737B,
    render_object: 0x29C,
    ghost_block: 0x0076_714A,
    ghost_block_len: 0x24,
    end: 0x0076_7410,
    rinfo: 0x298,
    bounding_sphere_vslot: 0xF0 / 4,
    render_vslot: 0x24 / 4,
    push_material_pass: 0x0085_80D0,
    pop_material_pass: 0x0085_8100,
    scene_default_lights: 0xF0,
    scene_fogged_lights: 0x314,
    scene_pass_mode: 0x58C,
    scene_shroud_pass: 0x538,
};

static GENERALS_OBJECTS: ObjectFog = ObjectFog {
    status_compare: 0x0074_6C0A,
    render_object: 0x248,
    ghost_block: 0x0074_6A00,
    ghost_block_len: 0x24,
    end: 0x0074_6C9F,
    rinfo: 0x244,
    bounding_sphere_vslot: 0xF4 / 4,
    render_vslot: 0x2C / 4,
    push_material_pass: 0x0082_34E0,
    pop_material_pass: 0x0082_3510,
    scene_default_lights: 0xF0,
    scene_fogged_lights: 0x2BC,
    scene_pass_mode: 0x4D8,
    scene_shroud_pass: 0x488,
};

/// RenderInfoClass: the light environment the render object is lit with.
const RINFO_LIGHT_ENVIRONMENT: usize = 0x1C;

/// OBJECTSHROUD_CLEAR and OBJECTSHROUD_PARTIAL_CLEAR.
const STATUS_CLEAR: i32 = 1;
const STATUS_PARTIAL_CLEAR: i32 = 2;
/// Objects wider than this many cells get the fog pass without looking.
const MAX_CELLS: i32 = 128;

static SMOOTH: AtomicBool = AtomicBool::new(true);
static OBJECTS: AtomicBool = AtomicBool::new(true);
static RENDER_OBJECT_OFFSET: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn install() {
    let Some(o) = game::pick(&ZERO_HOUR_OBJECTS, Some(&GENERALS_OBJECTS)) else { return };
    if game::build().is_none() {
        return;
    }
    RENDER_OBJECT_OFFSET.store(o.render_object, Ordering::Relaxed);
    unsafe { game::replace_with_call(o.status_compare, 5, status_compare as *const () as usize) };
    // push dword [esp+rinfo]; push edi; push ebp; call render_ghost; add esp, 12; jmp end
    let mut code = vec![0x90u8; o.ghost_block_len];
    code[..3].copy_from_slice(&[0xFF, 0xB4, 0x24]);
    code[3..7].copy_from_slice(&(o.rinfo as u32).to_le_bytes());
    code[7] = 0x57;
    code[8] = 0x55;
    let call_at = o.ghost_block + 9;
    code[9] = 0xE8;
    code[10..14].copy_from_slice(&((render_ghost as *const () as usize).wrapping_sub(call_at + 5) as u32).to_le_bytes());
    code[14..17].copy_from_slice(&[0x83, 0xC4, 0x0C]);
    let jump_at = o.ghost_block + 17;
    code[17] = 0xE9;
    code[18..22].copy_from_slice(&(o.end.wrapping_sub(jump_at + 5) as u32).to_le_bytes());
    unsafe { game::write_code(o.ghost_block, &code) };
    log!("objects reaching into the fog get the fog pass");
}

/// Whether objects the game counts as visible are darkened where they reach into the fog.
pub fn set_objects(on: bool) {
    OBJECTS.store(on, Ordering::Relaxed);
}

/// Replaces `cmp dword [esp+0x14], 1` in renderOneObject: compares our status instead, for the
/// `jle` after it. EAX, ECX and EDX are free there.
#[unsafe(naked)]
unsafe extern "C" fn status_compare() {
    naked_asm!(
        "mov eax, dword ptr [esp + 0x18]",
        "mov ecx, dword ptr [{offset}]",
        "mov ecx, dword ptr [esp + ecx + 4]",
        "push ecx",
        "push eax",
        "call {status}",
        "add esp, 8",
        "cmp eax, 1",
        "ret",
        offset = sym RENDER_OBJECT_OFFSET,
        status = sym object_status,
    )
}

extern "C" fn object_status(status: i32, render_object: usize) -> i32 {
    if status > STATUS_CLEAR || render_object == 0 || !OBJECTS.load(Ordering::Relaxed) {
        return status;
    }
    match fog_under(render_object) {
        Some(Under::Clear) | None => status,
        Some(_) => STATUS_PARTIAL_CLEAR,
    }
}

/// A ghost at a fog edge gets the normal lights and the fog pass, which in the fog matches the fogged lights.
extern "C" fn render_ghost(scene: usize, render_object: usize, rinfo: usize) {
    let Some(o) = game::pick(&ZERO_HOUR_OBJECTS, Some(&GENERALS_OBJECTS)) else { return };
    let read = |p: usize| unsafe { *(p as *const usize) };
    let shroud_pass = read(scene + o.scene_shroud_pass);
    let fog_pass = OBJECTS.load(Ordering::Relaxed)
        && read(scene + o.scene_pass_mode) == 0
        && shroud_pass != 0
        && matches!(fog_under(render_object), Some(Under::Mixed));
    let lights = scene + if fog_pass { o.scene_default_lights } else { o.scene_fogged_lights };
    unsafe {
        let render: extern "thiscall" fn(usize, usize) = std::mem::transmute(*(*(render_object as *const *const usize)).add(o.render_vslot));
        let push: extern "thiscall" fn(usize, usize) = std::mem::transmute(o.push_material_pass);
        let pop: extern "thiscall" fn(usize) = std::mem::transmute(o.pop_material_pass);
        *((rinfo + RINFO_LIGHT_ENVIRONMENT) as *mut usize) = lights;
        if fog_pass {
            push(rinfo, shroud_pass);
        }
        render(render_object, rinfo);
        if fog_pass {
            pop(rinfo);
        }
        *((rinfo + RINFO_LIGHT_ENVIRONMENT) as *mut usize) = 0;
    }
}

/// The fog cells under a render object's bounding sphere.
enum Under {
    Clear,
    /// All one level of fog.
    Uniform,
    /// Different levels, or reaching past the cells the fog covers.
    Mixed,
}

fn fog_under(render_object: usize) -> Option<Under> {
    let (&(the_terrain_render_object, render_object_shroud), o) =
        (game::pick(&ZERO_HOUR, Some(&GENERALS))?, game::pick(&ZERO_HOUR_OBJECTS, Some(&GENERALS_OBJECTS))?);
    unsafe {
        let terrain = *(the_terrain_render_object as *const usize);
        if terrain == 0 {
            return None;
        }
        let s = *((terrain + render_object_shroud) as *const usize);
        if s == 0 || *((s + shroud::SOURCE) as *const usize) == 0 {
            return None;
        }
        let data = *((s + shroud::DATA) as *const usize);
        let (cells_x, cells_y) = (*((s + shroud::CELLS_X) as *const i32), *((s + shroud::CELLS_Y) as *const i32));
        let (cell_w, cell_h) = (*((s + shroud::CELL_WIDTH) as *const f32), *((s + shroud::CELL_HEIGHT) as *const f32));
        let pitch = *((s + shroud::PITCH) as *const i32) as usize;
        if data == 0 || cell_w <= 0.0 || cell_h <= 0.0 {
            return None;
        }
        let get_sphere: extern "thiscall" fn(usize) -> *const [f32; 4] =
            std::mem::transmute(*(*(render_object as *const *const usize)).add(o.bounding_sphere_vslot));
        let [x, y, _, r] = *get_sphere(render_object);
        let x0 = ((x - r) / cell_w).floor() as i32;
        let x1 = ((x + r) / cell_w).floor() as i32;
        let y0 = ((y - r) / cell_h).floor() as i32;
        let y1 = ((y + r) / cell_h).floor() as i32;
        if x0 < 0 || y0 < 0 || x1 >= cells_x || y1 >= cells_y || x1 - x0 > MAX_CELLS || y1 - y0 > MAX_CELLS {
            return Some(Under::Mixed);
        }
        let first = *((data + y0 as usize * pitch) as *const u16).add(x0 as usize);
        let uniform = (y0..=y1).all(|cy| {
            let row = (data + cy as usize * pitch) as *const u16;
            (x0..=x1).all(|cx| *row.add(cx as usize) == first)
        });
        Some(match (uniform, first == shroud::CLEAR) {
            (true, true) => Under::Clear,
            (true, false) => Under::Uniform,
            (false, _) => Under::Mixed,
        })
    }
}

pub fn set_smooth(on: bool) {
    SMOOTH.store(on, Ordering::Relaxed);
}

/// Runs once a frame.
pub fn mark_world_fog() {
    let (Some(build), Some(&(the_terrain_render_object, render_object_shroud))) = (game::build(), game::pick(&ZERO_HOUR, Some(&GENERALS)))
    else {
        return;
    };
    let read = |p: usize| unsafe { *(p as *const usize) };
    let object = read(the_terrain_render_object);
    if object == 0 {
        return;
    }
    let shroud = read(object + render_object_shroud);
    if shroud == 0 {
        return;
    }
    let class = read(shroud + SHROUD_TEXTURE);
    if class == 0 {
        return;
    }
    let Some(t) = (unsafe { texture::texture_from_ptr(read(class + build.texture_d3d) as *mut _) }) else { return };
    let on = SMOOTH.load(Ordering::Relaxed);
    let t = crate::vk::worker::Ptr(t);
    crate::vk::worker::post(move || {
        let t = t;
        let data = unsafe { &(*t.0).data };
        if data.smooth.replace(on) && !on {
            data.smoothed.borrow_mut().take();
        }
    });
}
