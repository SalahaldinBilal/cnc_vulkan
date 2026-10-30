//! Marks where the 3D world is drawn each frame: from the start of W3DView::draw to the point
//! where it starts drawing icons, health bars and text over the world. In between, the renderer
//! can draw the world into a target of its own (render scale and the effects that need the world
//! without the interface). See knowledge-base/scene.md.

use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use crate::{game, log};

struct Addresses {
    /// W3DView's second vtable (the base at +0x7c) slot +0x14: W3DView::draw.
    view_draw_entry: usize,
    view_draw: usize,
    /// In W3DView::draw: `call getAxisAlignedViewRegion`, right before the icon and text pass.
    view_region_call: usize,
    view_region: usize,
    /// In RTS3DScene::Flush: `call DoTrees(rinfo)` (cdecl), the last opaque geometry, and
    /// `call SortingRendererClass::Flush` right after DoParticles, which draws the particles and
    /// every other sorted translucent polygon.
    do_trees_call: usize,
    do_trees: usize,
    sorted_call: usize,
    sorted: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    view_draw_entry: 0x0096_8B54,
    view_draw: 0x0076_DDB0,
    view_region_call: 0x0076_E4A7,
    view_region: 0x0076_DAD0,
    do_trees_call: 0x0076_7480,
    do_trees: 0x0074_FC10,
    sorted_call: 0x0076_74E0,
    sorted: 0x0084_2B60,
};

static GENERALS: Addresses = Addresses {
    view_draw_entry: 0x009A_676C,
    view_draw: 0x0074_E100,
    view_region_call: 0x0074_E816,
    view_region: 0x0074_DE20,
    do_trees_call: 0x0074_6D10,
    do_trees: 0x0072_87B0,
    sorted_call: 0x0074_6D70,
    sorted: 0x0080_9900,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

static IN_SCENE: AtomicBool = AtomicBool::new(false);
static IN_SORTED: AtomicBool = AtomicBool::new(false);
/// Counts the times opaque geometry finished drawing.
static OPAQUE_DONE: AtomicU32 = AtomicU32::new(0);

/// Whether the game is drawing the 3D world right now.
pub fn in_scene() -> bool {
    IN_SCENE.load(Ordering::Relaxed)
}

/// Whether the scene is drawing its sorted translucent polygons, particles among them, right now.
pub fn in_sorted() -> bool {
    IN_SORTED.load(Ordering::Relaxed)
}

/// Goes up each time a scene's opaque geometry (terrain, meshes, trees) has been drawn.
pub fn opaque_done() -> u32 {
    OPAQUE_DONE.load(Ordering::Relaxed)
}

#[cfg(test)]
pub fn set_in_scene(on: bool) {
    IN_SCENE.store(on, Ordering::Relaxed);
}

#[cfg(test)]
pub fn set_in_sorted(on: bool) {
    IN_SORTED.store(on, Ordering::Relaxed);
}

#[cfg(test)]
pub fn mark_opaque_done() {
    OPAQUE_DONE.fetch_add(1, Ordering::Relaxed);
}

pub fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    if game::call_target(a.view_region_call) != Some(a.view_region) {
        log!("W3DView::draw isn't what we expect, the 3D world gets no target of its own");
        return;
    }
    unsafe {
        game::write_u32(a.view_draw_entry, draw_view as *const () as u32);
        game::patch_call(a.view_region_call, view_region as *const () as usize);
    }
    log!("3D world drawing marked for the renderer");
    if game::call_target(a.do_trees_call) != Some(a.do_trees) || game::call_target(a.sorted_call) != Some(a.sorted) {
        log!("RTS3DScene::Flush isn't what we expect, no ambient occlusion or soft particles");
        return;
    }
    unsafe {
        game::patch_call(a.do_trees_call, do_trees as *const () as usize);
        game::patch_call(a.sorted_call, sorted as *const () as usize);
    }
}

extern "C" fn do_trees(rinfo: usize) {
    let original: extern "C" fn(usize) = unsafe { std::mem::transmute(addresses().do_trees) };
    original(rinfo);
    OPAQUE_DONE.fetch_add(1, Ordering::Relaxed);
}

extern "C" fn sorted() {
    let original: extern "C" fn() = unsafe { std::mem::transmute(addresses().sorted) };
    IN_SORTED.store(true, Ordering::Relaxed);
    original();
    IN_SORTED.store(false, Ordering::Relaxed);
}

/// W3DView::draw. `this` is the view's second base, as the vtable passes it.
extern "thiscall" fn draw_view(this: usize) {
    let draw: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().view_draw) };
    IN_SCENE.store(true, Ordering::Relaxed);
    draw(this);
    IN_SCENE.store(false, Ordering::Relaxed);
}

/// getAxisAlignedViewRegion, the first thing W3DView::draw does after the world is drawn.
extern "thiscall" fn view_region(view: usize, region: usize) {
    let original: extern "thiscall" fn(usize, usize) = unsafe { std::mem::transmute(addresses().view_region) };
    IN_SCENE.store(false, Ordering::Relaxed);
    original(view, region);
}
