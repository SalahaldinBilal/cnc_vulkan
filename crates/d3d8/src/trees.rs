//! Tree sway advances by a fixed step every time trees are drawn, which assumed 30 draws per second.
//! We scale the step by the real time since the last draw, so sway keeps its speed at any frame rate.
//! Generals' tree buffer has no sway.

use std::sync::Mutex;
use std::time::Instant;

use crate::{frame, game};

/// `call W3DTreeBuffer::drawTrees` in the terrain render, with the tree buffer in ECX.
const DRAW_TREES_CALL: usize = 0x0074_FF01;
const DRAW_TREES: usize = 0x0078_7D90;
const SWAY_STEPS: usize = 0xB6_8D4;
const SWAY_TYPES: usize = 10;
/// Longest gap we make up for, so a hitch doesn't fling the trees.
const MAX_STEPS_PER_DRAW: f32 = 4.0;

static LAST_DRAW: Mutex<Option<Instant>> = Mutex::new(None);

fn steps_since_last_draw() -> f32 {
    let now = Instant::now();
    let mut last = LAST_DRAW.lock().unwrap();
    let steps = match *last {
        Some(t) => (now - t).as_secs_f32() / frame::update_interval().as_secs_f32(),
        None => 1.0,
    };
    *last = Some(now);
    steps.min(MAX_STEPS_PER_DRAW)
}

pub unsafe fn install() {
    if game::zero_hour() {
        unsafe { game::patch_call(DRAW_TREES_CALL, draw_trees as *const () as usize) };
    }
}

extern "thiscall" fn draw_trees(buffer: usize, camera: usize, lights: usize) {
    let original: extern "thiscall" fn(usize, usize, usize) = unsafe { std::mem::transmute(DRAW_TREES) };
    let steps = (buffer + SWAY_STEPS) as *mut [f32; SWAY_TYPES];
    let k = steps_since_last_draw();
    let normal = unsafe { *steps };
    let scaled = normal.map(|s| s * k);
    unsafe { *steps = scaled };
    original(buffer, camera, lights);
    let after = unsafe { &mut *steps };
    for i in 0..SWAY_TYPES {
        if after[i] == scaled[i] {
            after[i] = normal[i];
        }
    }
}
