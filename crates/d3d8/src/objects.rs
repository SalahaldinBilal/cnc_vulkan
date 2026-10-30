//! Renders models (units, buildings, projectiles) between their last two game updates. Every model
//! transform set during a game update, outside of rendering, is recorded. Models that moved in the
//! latest update are blended before each render and put back after it.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

use rustc_hash::FxHashMap;

use crate::matrix::{self, Matrix};
use crate::{bones, frame, game};

pub struct Addresses {
    /// RenderObjClass::Set_Transform, the non-virtual base every override calls. We replace it.
    pub set_transform: usize,
    /// RenderObjClass::Set_Position, same idea for translation only. We replace it.
    pub set_position: usize,
    /// The callback W3DView hands to the drawable iteration. It calls Drawable::draw.
    draw_drawable: usize,
    /// `push draw_drawable` in the W3DView camera update.
    pub draw_drawable_push: usize,
    camera_vtable: usize,
    set_transform_vslot: usize,
    /// RefCountClass::Delete_This. Generals' render objects have virtual Add_Ref and Release_Ref first.
    delete_vslot: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    set_transform: 0x0085_0E80,
    set_position: 0x0085_0F60,
    draw_drawable: 0x0076_DAC0,
    draw_drawable_push: 0x0076_DA9B,
    camera_vtable: 0x0097_3288,
    set_transform_vslot: 0x4C / 4,
    delete_vslot: 0,
};

static GENERALS: Addresses = Addresses {
    set_transform: 0x0081_B7E0,
    set_position: 0x0081_B8C0,
    draw_drawable: 0x0074_DE10,
    draw_drawable_push: 0x0074_DDEC,
    camera_vtable: 0x009B_0AB0,
    set_transform_vslot: 0x54 / 4,
    delete_vslot: 2,
};

pub fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

/// Models that haven't moved for this many updates are dropped.
pub const KEEP_UPDATES: u64 = 60;

mod offsets {
    pub const REF_COUNT: usize = 0x04;
    pub const BITS: usize = 0x18;
    pub const TRANSFORM: usize = 0x1C;
    pub const IS_IDENTITY: usize = 0x80;
}

const BOUNDING_VOLUMES_VALID: u32 = 0x2000;
const IDENTITY: [u32; 12] = [0x3F80_0000, 0, 0, 0, 0, 0x3F80_0000, 0, 0, 0, 0, 0x3F80_0000, 0];

static RECORDING: AtomicBool = AtomicBool::new(false);
/// On while client code moves models every frame (the building preview). Models it moves are
/// left out of the blend for the rest of the current update.
static LIVE_SCOPE: AtomicBool = AtomicBool::new(false);
static SET_CALLS: AtomicU32 = AtomicU32::new(0);
static RECORDED_CALLS: AtomicU32 = AtomicU32::new(0);
static BLENDS: AtomicU32 = AtomicU32::new(0);

/// Counts since the last call, for the log.
pub fn stats() -> String {
    let tracked = STATE.lock().unwrap().as_ref().map_or(0, |s| s.objects.len());
    format!(
        "model transforms set {}, {} recorded, {} tracked, {} blends",
        SET_CALLS.swap(0, Ordering::Relaxed),
        RECORDED_CALLS.swap(0, Ordering::Relaxed),
        tracked,
        BLENDS.swap(0, Ordering::Relaxed)
    )
}

struct Entry {
    /// Where the model was at the end of the update before the one that last moved it.
    previous: Matrix,
    /// The latest transform the game set.
    current: Matrix,
    /// The game update that last moved it.
    update: u64,
    /// The game update in which client code last moved it every frame.
    live: Option<u64>,
    replaced: Option<Matrix>,
}

#[derive(Default)]
struct State {
    objects: FxHashMap<usize, Entry>,
    pruned: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub fn is_recording() -> bool {
    RECORDING.load(Ordering::Relaxed)
}

/// Turns recording on or off and returns the previous setting.
pub fn set_recording(on: bool) -> bool {
    RECORDING.swap(on, Ordering::Relaxed)
}

pub fn set_live_scope(on: bool) -> bool {
    LIVE_SCOPE.swap(on, Ordering::Relaxed)
}

pub fn transform(obj: usize) -> Matrix {
    unsafe { *((obj + offsets::TRANSFORM) as *const Matrix) }
}

/// Calls the object's own Set_Transform, so cameras and animated models refresh what depends on it.
pub fn set_transform(obj: usize, m: &Matrix) {
    unsafe {
        let vtable = *(obj as *const *const usize);
        let f: extern "thiscall" fn(usize, *const Matrix) =
            std::mem::transmute(*vtable.add(addresses().set_transform_vslot));
        f(obj, m);
    }
}

pub fn add_ref(obj: usize) {
    unsafe { *((obj + offsets::REF_COUNT) as *mut i32) += 1 };
}

pub fn release(obj: usize) {
    unsafe {
        let refs = (obj + offsets::REF_COUNT) as *mut i32;
        *refs -= 1;
        if *refs == 0 {
            let vtable = *(obj as *const *const usize);
            let delete_this: extern "thiscall" fn(usize) = std::mem::transmute(*vtable.add(addresses().delete_vslot));
            delete_this(obj);
        }
    }
}

/// Same as the game's RenderObjClass::Set_Transform, plus recording.
pub extern "thiscall" fn set_transform_base(obj: usize, m: *const Matrix) {
    let m = unsafe { *m };
    write_transform(obj, &m);
}

/// Same as the game's RenderObjClass::Set_Position (only the translation changes), plus recording.
pub extern "thiscall" fn set_position_base(obj: usize, position: *const [f32; 3]) {
    let mut m = transform(obj);
    let [x, y, z] = unsafe { *position };
    m[3] = x;
    m[7] = y;
    m[11] = z;
    write_transform(obj, &m);
}

fn write_transform(obj: usize, m: &Matrix) {
    unsafe {
        *((obj + offsets::TRANSFORM) as *mut Matrix) = *m;
        let bits: [u32; 12] = std::mem::transmute(*m);
        *((obj + offsets::IS_IDENTITY) as *mut u8) = (bits == IDENTITY) as u8;
        *((obj + offsets::BITS) as *mut u32) &= !BOUNDING_VOLUMES_VALID;
    }
    SET_CALLS.fetch_add(1, Ordering::Relaxed);
    if LIVE_SCOPE.load(Ordering::Relaxed) {
        if let Some(e) = STATE.lock().unwrap().as_mut().and_then(|s| s.objects.get_mut(&obj)) {
            e.live = Some(frame::update_count());
        }
    } else if RECORDING.load(Ordering::Relaxed) && unsafe { *(obj as *const usize) } != addresses().camera_vtable {
        RECORDED_CALLS.fetch_add(1, Ordering::Relaxed);
        record(obj, *m);
    }
}

fn record(obj: usize, m: Matrix) {
    let update = frame::update_count();
    let mut state = STATE.lock().unwrap();
    let s = state.get_or_insert_with(State::default);
    match s.objects.get_mut(&obj) {
        Some(e) if e.update == update => e.current = m,
        Some(e) => {
            e.previous = e.current;
            e.current = m;
            e.update = update;
        }
        None => {
            add_ref(obj);
            s.objects.insert(obj, Entry { previous: m, current: m, update, live: None, replaced: None });
            drop(state);
            bones::discover(obj);
        }
    }
}

/// Replaces the drawable pass callback, which runs inside the draw, so models it moves are recorded too.
pub extern "C" fn draw_drawable(drawable: usize, view: usize) -> usize {
    let original: extern "C" fn(usize, usize) -> usize = unsafe { std::mem::transmute(addresses().draw_drawable) };
    let was = set_recording(true);
    let result = original(drawable, view);
    set_recording(was);
    result
}

/// Called right before the scene renders.
pub fn apply(t: f32) {
    let update = frame::update_count();
    let dropped = {
        let mut state = STATE.lock().unwrap();
        let Some(s) = state.as_mut() else { return };
        let mut dropped = Vec::new();
        if s.pruned != update {
            s.pruned = update;
            s.objects.retain(|&obj, e| {
                let keep = e.update + KEEP_UPDATES >= update;
                if !keep {
                    dropped.push(obj);
                }
                keep
            });
        }
        if t < 1.0 {
            for (&obj, e) in s.objects.iter_mut() {
                // Moved again since we recorded it, so something updates it every draw: leave it live.
                if e.update == update
                    && e.live != Some(update)
                    && e.replaced.is_none()
                    && e.previous != e.current
                    && transform(obj) == e.current
                {
                    e.replaced = Some(transform(obj));
                    set_transform(obj, &matrix::blend(&e.previous, &e.current, t));
                    BLENDS.fetch_add(1, Ordering::Relaxed);
                }
            }
        }
        dropped
    };
    for obj in dropped {
        release(obj);
    }
}

/// Puts back what each model held before the blend.
pub fn restore() {
    let mut state = STATE.lock().unwrap();
    let Some(s) = state.as_mut() else { return };
    for (&obj, e) in s.objects.iter_mut() {
        if let Some(m) = e.replaced.take() {
            set_transform(obj, &m);
        }
    }
}
