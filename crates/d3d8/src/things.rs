//! Renders drawables and objects at positions blended between their last two game updates, so
//! everything drawn from a unit's game position (health bars, icons, selection circles, shadows)
//! follows the blended model. Only pointers found in the client's live drawable list are touched.

use std::collections::hash_map::Entry;
use std::sync::Mutex;

use rustc_hash::FxHashMap;

use crate::matrix::{self, Matrix};
use crate::{client, frame, game};

/// TheGameClient and the drawable's next pointer.
static ZERO_HOUR: (usize, usize) = (0x00A3_9B18, 0x90);
static GENERALS: (usize, usize) = (0x00A6_BE90, 0x94);
const FIRST_DRAWABLE_VSLOT: usize = 0x24 / 4;

mod offsets {
    pub const DRAWABLE_OBJECT: usize = 0x88;
    pub const TRANSFORM: usize = 0x08;
    pub const CACHED_POS: usize = 0x38;
    pub const CACHED_ANGLE: usize = 0x44;
    pub const CACHE_FLAGS: usize = 0x5C;
}

/// Transform plus every cached value after it, which is what we save and put back.
const STATE_WORDS: usize = (offsets::CACHE_FLAGS + 4 - offsets::TRANSFORM) / 4;

struct Track {
    previous: Matrix,
    current: Matrix,
    update: u64,
    saved: Option<[u32; STATE_WORDS]>,
    /// The record pass that last found it in the drawable list.
    pass: u64,
}

#[derive(Default)]
struct State {
    things: FxHashMap<usize, Track>,
    pass: u64,
    /// How many things moved in the latest update, as of the last record.
    moved: usize,
    /// How many things hold a blended state that `restore` has to put back.
    applied: usize,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Calls `f` with every drawable and object in the client's drawable list.
fn for_each_thing(mut f: impl FnMut(usize)) {
    let Some(&(the_game_client, drawable_next)) = game::pick(&ZERO_HOUR, Some(&GENERALS)) else { return };
    unsafe {
        let client = *(the_game_client as *const usize);
        if client == 0 {
            return;
        }
        let first: extern "thiscall" fn(usize) -> usize =
            std::mem::transmute(*(*(client as *const *const usize)).add(FIRST_DRAWABLE_VSLOT));
        let mut drawable = first(client);
        while drawable != 0 {
            f(drawable);
            let object = *((drawable + offsets::DRAWABLE_OBJECT) as *const usize);
            if object != 0 {
                f(object);
            }
            drawable = *((drawable + drawable_next) as *const usize);
        }
    }
}

fn transform(thing: usize) -> Matrix {
    unsafe { *((thing + offsets::TRANSFORM) as *const Matrix) }
}

/// Called after every game update, and before the draw inside it. A second call in the same update
/// only takes the latest transforms.
pub fn record() {
    let update = frame::update_count();
    let mut state = STATE.lock().unwrap();
    let s = state.get_or_insert_with(State::default);
    s.pass += 1;
    let pass = s.pass;
    let mut moved = 0;
    for_each_thing(|thing| {
        let m = transform(thing);
        let track = match s.things.entry(thing) {
            Entry::Occupied(e) => {
                let t = e.into_mut();
                if t.update != update {
                    t.previous = t.current;
                    t.update = update;
                }
                t.current = m;
                t.saved = None;
                t.pass = pass;
                t
            }
            Entry::Vacant(e) => e.insert(Track { previous: m, current: m, update, saved: None, pass }),
        };
        moved += (track.previous != track.current) as usize;
    });
    s.things.retain(|_, t| t.pass == pass);
    s.moved = moved;
    s.applied = 0;
}

/// Where a drawable or object is shown at blend factor `t`, if it moved in the latest update.
pub fn blended_position(thing: usize, t: f32) -> Option<[f32; 3]> {
    let state = STATE.lock().unwrap();
    let track = state.as_ref()?.things.get(&thing)?;
    if t >= 1.0 || track.update != frame::update_count() || transform(thing) != track.current {
        return None;
    }
    let (a, b) = (&track.previous, &track.current);
    Some([3, 7, 11].map(|i| a[i] + (b[i] - a[i]) * t))
}

/// Called right before the scene renders.
pub fn apply(t: f32) {
    let update = frame::update_count();
    let mut state = STATE.lock().unwrap();
    let Some(s) = state.as_mut() else { return };
    if t >= 1.0 || s.moved == 0 {
        return;
    }
    let place_icons = client::place_icons();
    for_each_thing(|thing| {
        if place_icons.contains(&thing) {
            return;
        }
        let Some(track) = s.things.get_mut(&thing) else { return };
        if track.update != update
            || track.saved.is_some()
            || track.previous == track.current
            || transform(thing) != track.current
        {
            return;
        }
        let m = matrix::blend(&track.previous, &track.current, t);
        unsafe {
            let state = (thing + offsets::TRANSFORM) as *mut [u32; STATE_WORDS];
            track.saved = Some(*state);
            *((thing + offsets::TRANSFORM) as *mut Matrix) = m;
            *((thing + offsets::CACHED_POS) as *mut [f32; 3]) = [m[3], m[7], m[11]];
            *((thing + offsets::CACHED_ANGLE) as *mut f32) = m[4].atan2(m[0]);
            *((thing + offsets::CACHE_FLAGS) as *mut u32) = 0;
        }
        s.applied += 1;
    });
}

/// Puts back every value the blend replaced.
pub fn restore() {
    let mut state = STATE.lock().unwrap();
    let Some(s) = state.as_mut() else { return };
    if s.applied == 0 {
        return;
    }
    s.applied = 0;
    for (&thing, track) in s.things.iter_mut() {
        if let Some(saved) = track.saved.take() {
            unsafe { *((thing + offsets::TRANSFORM) as *mut [u32; STATE_WORDS]) = saved };
        }
    }
}
