//! Renders UI windows between their last two game updates. Window slides (the control bar coming
//! in, menu transitions) move windows with winSetPosition once per update, so we replace it to
//! record moves and blend the position while rendering. The W3DGameWindow destructor is hooked so
//! freed windows are forgotten.

use std::sync::Mutex;

use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{frame, game};

/// W3DGameWindow's vtable and its deleting destructor (slot 0).
static ZERO_HOUR: (usize, usize) = (0x0096_BE98, 0x007C_EDE0);
static GENERALS: (usize, usize) = (0x009A_8F90, 0x007A_60F0);

fn window_class() -> &'static (usize, usize) {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

mod offsets {
    pub const SIZE: usize = 0x08;
    pub const REGION: usize = 0x10;
}

static RECORDING: AtomicBool = AtomicBool::new(false);

struct Entry {
    previous: [i32; 2],
    current: [i32; 2],
    update: u64,
    saved: Option<[i32; 4]>,
}

static STATE: Mutex<Option<FxHashMap<usize, Entry>>> = Mutex::new(None);

pub unsafe fn install() {
    unsafe {
        game::patch_jump(crate::gadgets::win_set_position(), set_position as *const () as usize);
        game::write_u32(window_class().0, destroy_window as *const () as u32);
    }
}

pub fn set_recording(on: bool) -> bool {
    RECORDING.swap(on, Ordering::Relaxed)
}

fn region(window: usize) -> *mut [i32; 4] {
    (window + offsets::REGION) as *mut [i32; 4]
}

/// Same as the game's: the region starts at (x, y) and spans the window size, kept ordered.
extern "thiscall" fn set_position(window: usize, x: i32, y: i32) -> i32 {
    let [w, h]: [i32; 2] = unsafe { *((window + offsets::SIZE) as *const [i32; 2]) };
    let (x2, y2) = (x.wrapping_add(w), y.wrapping_add(h));
    unsafe { *region(window) = [x.min(x2), y.min(y2), x.max(x2), y.max(y2)] };
    if RECORDING.load(Ordering::Relaxed) {
        record(window, [x, y]);
    }
    0
}

fn record(window: usize, position: [i32; 2]) {
    let update = frame::tick_count();
    let mut state = STATE.lock().unwrap();
    let map = state.get_or_insert_with(FxHashMap::default);
    match map.get_mut(&window) {
        Some(e) if e.update == update => e.current = position,
        Some(e) => {
            e.previous = e.current;
            e.current = position;
            e.update = update;
        }
        None => {
            map.insert(window, Entry { previous: position, current: position, update, saved: None });
        }
    }
}

extern "thiscall" fn destroy_window(window: usize, flags: u32) -> usize {
    if let Some(map) = STATE.lock().unwrap().as_mut() {
        map.remove(&window);
    }
    let original: extern "thiscall" fn(usize, u32) -> usize = unsafe { std::mem::transmute(window_class().1) };
    original(window, flags)
}

/// Called right before the scene renders.
pub fn apply(t: f32) {
    if t >= 1.0 {
        return;
    }
    let update = frame::tick_count();
    let mut state = STATE.lock().unwrap();
    let Some(map) = state.as_mut() else { return };
    for (&window, e) in map.iter_mut() {
        if e.update != update || e.saved.is_some() || e.previous == e.current {
            continue;
        }
        let r = unsafe { *region(window) };
        if [r[0], r[1]] != e.current && [r[2], r[3]] != e.current {
            continue;
        }
        let dx = ((e.previous[0] - e.current[0]) as f32 * (1.0 - t)).round() as i32;
        let dy = ((e.previous[1] - e.current[1]) as f32 * (1.0 - t)).round() as i32;
        e.saved = Some(r);
        unsafe { *region(window) = [r[0] + dx, r[1] + dy, r[2] + dx, r[3] + dy] };
    }
}

/// Puts back the regions the blend replaced.
pub fn restore() {
    let mut state = STATE.lock().unwrap();
    let Some(map) = state.as_mut() else { return };
    for (&window, e) in map.iter_mut() {
        if let Some(r) = e.saved.take() {
            unsafe { *region(window) = r };
        }
    }
}
