//! Renders tank tread scrolling between game updates. The tank draw modules move each tread's
//! texture offset once per drawable pass, we hook their doDrawModule to see the step and their
//! destructor to forget modules before they're freed.

use std::sync::Mutex;

use rustc_hash::FxHashMap;

use crate::{frame, game, log};

const DRAW_VSLOT: usize = 0x1C / 4;
const DESTROY_VSLOT: usize = 0;
const TREAD_STRIDE: usize = 0x14;
const MAX_TREADS: usize = 4;

struct Class {
    name: &'static str,
    vtable: usize,
    draw: usize,
    destroy: usize,
    /// Offset of the first tread's texture U offset.
    offset: usize,
    count: usize,
}

static ZERO_HOUR: [Class; 2] = [
    Class { name: "W3DTankDraw", vtable: 0x0096_AD08, draw: 0x007B_F6E0, destroy: 0x007B_F200, offset: 0x98, count: 0xDC },
    Class { name: "W3DTankTruckDraw", vtable: 0x0096_BA18, draw: 0x007C_4BE0, destroy: 0x007C_4470, offset: 0x19C, count: 0x1E0 },
];
static GENERALS: [Class; 2] = [
    Class { name: "W3DTankDraw", vtable: 0x009A_8340, draw: 0x0079_6860, destroy: 0x0079_6240, offset: 0x98, count: 0xDC },
    Class { name: "W3DTankTruckDraw", vtable: 0x009A_8DB8, draw: 0x0079_BB80, destroy: 0x0079_B1B0, offset: 0x19C, count: 0x1E0 },
];

fn classes() -> &'static [Class; 2] {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

struct Entry {
    class: usize,
    before: [f32; MAX_TREADS],
    after: [f32; MAX_TREADS],
    count: usize,
    update: u64,
    saved: Option<[f32; MAX_TREADS]>,
}

static STATE: Mutex<Option<FxHashMap<usize, Entry>>> = Mutex::new(None);

pub fn install() {
    let hooks: [(usize, usize); 2] = [
        (draw_hook::<0> as *const () as usize, destroy_hook::<0> as *const () as usize),
        (draw_hook::<1> as *const () as usize, destroy_hook::<1> as *const () as usize),
    ];
    for (c, (draw, destroy)) in classes().iter().zip(hooks) {
        unsafe {
            game::write_u32(c.vtable + DRAW_VSLOT * 4, draw as u32);
            game::write_u32(c.vtable + DESTROY_VSLOT * 4, destroy as u32);
        }
        log!("tread scrolling of {} blended", c.name);
    }
}

fn offsets(module: usize, class: usize) -> (usize, usize) {
    let c = &classes()[class];
    let count = unsafe { *((module + c.count) as *const i32) }.clamp(0, MAX_TREADS as i32) as usize;
    (module + c.offset, count)
}

fn read_offsets(base: usize, count: usize) -> [f32; MAX_TREADS] {
    let mut v = [0.0; MAX_TREADS];
    for (i, slot) in v.iter_mut().enumerate().take(count) {
        *slot = unsafe { *((base + i * TREAD_STRIDE) as *const f32) };
    }
    v
}

extern "thiscall" fn draw_hook<const CLASS: usize>(module: usize, transform: usize) {
    let original: extern "thiscall" fn(usize, usize) = unsafe { std::mem::transmute(classes()[CLASS].draw) };
    let (base, count) = offsets(module, CLASS);
    let before = read_offsets(base, count);
    original(module, transform);
    let (base, count) = offsets(module, CLASS);
    let after = read_offsets(base, count);
    let mut state = STATE.lock().unwrap();
    state.get_or_insert_with(FxHashMap::default).insert(
        module,
        Entry { class: CLASS, before, after, count, update: frame::update_count(), saved: None },
    );
}

extern "thiscall" fn destroy_hook<const CLASS: usize>(module: usize, flags: u32) -> usize {
    if let Some(map) = STATE.lock().unwrap().as_mut() {
        map.remove(&module);
    }
    let original: extern "thiscall" fn(usize, u32) -> usize = unsafe { std::mem::transmute(classes()[CLASS].destroy) };
    original(module, flags)
}

/// Called right before the scene renders.
pub fn apply(t: f32) {
    if t >= 1.0 {
        return;
    }
    let update = frame::update_count();
    let mut state = STATE.lock().unwrap();
    let Some(map) = state.as_mut() else { return };
    for (&module, e) in map.iter_mut() {
        if e.update != update || e.saved.is_some() || e.before == e.after {
            continue;
        }
        let (base, _) = offsets(module, e.class);
        e.saved = Some(read_offsets(base, e.count));
        for i in 0..e.count {
            let mut step = e.after[i] - e.before[i];
            step -= step.round();
            let u = e.before[i] + step * t;
            unsafe { *((base + i * TREAD_STRIDE) as *mut f32) = u - u.floor() };
        }
    }
}

/// Puts back the offsets the blend replaced.
pub fn restore() {
    let mut state = STATE.lock().unwrap();
    let Some(map) = state.as_mut() else { return };
    for (&module, e) in map.iter_mut() {
        if let Some(saved) = e.saved.take() {
            let (base, _) = offsets(module, e.class);
            for (i, u) in saved.iter().enumerate().take(e.count) {
                unsafe { *((base + i * TREAD_STRIDE) as *mut f32) = *u };
            }
        }
    }
}
