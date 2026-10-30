//! Renders UI window transitions (fades, scale-ups, flashes, the control bar arrow) between their
//! frames. Every transition draws from an integer frame counter. When the counter moved this game
//! update, we run its draw at the previous and the current frame with the display's 2D calls
//! captured, then draw a blend of the two.

use std::arch::naked_asm;
use std::sync::Mutex;

use rustc_hash::FxHashMap;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::{frame, game, log};

/// `mov eax/edx, [ecx]; call [reg+0x10]` (transition->draw()) in the transitions handler draw, with
/// the TransitionWindow in EAX. 5 bytes each.
struct Addresses {
    draw_calls: [usize; 2],
    display_vtable: usize,
    draw_open_rect: usize,
    draw_fill_rect: usize,
    draw_image: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    draw_calls: [0x0051_554F, 0x0051_5575],
    display_vtable: 0x0096_79B8,
    draw_open_rect: 0x0073_EAF0,
    draw_fill_rect: 0x0073_ED20,
    draw_image: 0x0074_00C0,
};

static GENERALS: Addresses = Addresses {
    draw_calls: [0x0053_057F, 0x0053_05A5],
    display_vtable: 0x009A_5648,
    draw_open_rect: 0x0071_7B70,
    draw_fill_rect: 0x0071_7DA0,
    draw_image: 0x0071_9140,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

const DRAW_CALL_LEN: usize = 5;
const TRANSITION_DRAW_VSLOT: usize = 0x10 / 4;
const WINDOW_STYLE: usize = 0x08;
const DRAW_OPEN_RECT_SLOT: usize = 0x84;
const DRAW_FILL_RECT_SLOT: usize = 0x88;
const DRAW_IMAGE_SLOT: usize = 0x94;

/// Where each transition style keeps its frame counter (m_drawState). Styles that only type text,
/// count numbers or play sounds have nothing to blend.
fn draw_state_offset(style: i32) -> Option<usize> {
    match style {
        // flash, button flash, fade, scale up, main menu scale ups, score scale up
        0 | 1 | 2 | 3 | 4 | 10 | 11 | 13 => Some(0x20),
        // screen fade, full fade
        6 | 8 => Some(0x24),
        // control bar arrow
        12 => Some(0x30),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Call {
    OpenRect { x: i32, y: i32, w: i32, h: i32, width: f32, color: u32 },
    FillRect { x: i32, y: i32, w: i32, h: i32, color: u32 },
    Image { image: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, mode: i32 },
}

static CAPTURING: AtomicBool = AtomicBool::new(false);
static CAPTURED: Mutex<Vec<(usize, Call)>> = Mutex::new(Vec::new());

struct Track {
    previous: i32,
    current: i32,
    update: u64,
}

static TRACKS: Mutex<Option<FxHashMap<usize, Track>>> = Mutex::new(None);

pub unsafe fn install() {
    let a = addresses();
    unsafe {
        let slots = [
            (DRAW_OPEN_RECT_SLOT, draw_open_rect as *const () as u32),
            (DRAW_FILL_RECT_SLOT, draw_fill_rect as *const () as u32),
            (DRAW_IMAGE_SLOT, draw_image as *const () as u32),
        ];
        for (slot, hook) in slots {
            game::write_u32(a.display_vtable + slot, hook);
        }
        for at in a.draw_calls {
            game::replace_with_call(at, DRAW_CALL_LEN, draw_stub as *const () as usize);
        }
    }
    log!("window transitions blended between frames");
}

fn capture(display: usize, call: Call) -> bool {
    if !CAPTURING.load(Ordering::Relaxed) {
        return false;
    }
    CAPTURED.lock().unwrap().push((display, call));
    true
}

extern "thiscall" fn draw_open_rect(display: usize, x: i32, y: i32, w: i32, h: i32, width: f32, color: u32) {
    if !capture(display, Call::OpenRect { x, y, w, h, width, color }) {
        emit(display, &Call::OpenRect { x, y, w, h, width, color });
    }
}

extern "thiscall" fn draw_fill_rect(display: usize, x: i32, y: i32, w: i32, h: i32, color: u32) {
    if !capture(display, Call::FillRect { x, y, w, h, color }) {
        emit(display, &Call::FillRect { x, y, w, h, color });
    }
}

extern "thiscall" fn draw_image(display: usize, image: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: u32, mode: i32) {
    if crate::controlbar::draw_wide(display, image, x0, y0, x1, y1, color, mode) {
        return;
    }
    if !capture(display, Call::Image { image, x0, y0, x1, y1, color, mode }) {
        emit(display, &Call::Image { image, x0, y0, x1, y1, color, mode });
    }
}

fn emit(display: usize, call: &Call) {
    unsafe {
        match *call {
            Call::OpenRect { x, y, w, h, width, color } => {
                let f: extern "thiscall" fn(usize, i32, i32, i32, i32, f32, u32) = std::mem::transmute(addresses().draw_open_rect);
                f(display, x, y, w, h, width, color);
            }
            Call::FillRect { x, y, w, h, color } => {
                let f: extern "thiscall" fn(usize, i32, i32, i32, i32, u32) = std::mem::transmute(addresses().draw_fill_rect);
                f(display, x, y, w, h, color);
            }
            Call::Image { image, x0, y0, x1, y1, color, mode } => {
                let f: extern "thiscall" fn(usize, usize, i32, i32, i32, i32, u32, i32) = std::mem::transmute(addresses().draw_image);
                f(display, image, x0, y0, x1, y1, color, mode);
            }
        }
    }
}

fn lerp_i(a: i32, b: i32, t: f32) -> i32 {
    (a as f32 + (b - a) as f32 * t).round() as i32
}

fn lerp_color(a: u32, b: u32, t: f32) -> u32 {
    (0..4).fold(0, |c, i| {
        let shift = i * 8;
        let x = lerp_i(((a >> shift) & 0xFF) as i32, ((b >> shift) & 0xFF) as i32, t).clamp(0, 255) as u32;
        c | (x << shift)
    })
}

fn blend(a: &Call, b: &Call, t: f32) -> Option<Call> {
    Some(match (*a, *b) {
        (
            Call::OpenRect { x, y, w, h, width, color },
            Call::OpenRect { x: x2, y: y2, w: w2, h: h2, width: width2, color: color2 },
        ) => Call::OpenRect {
            x: lerp_i(x, x2, t),
            y: lerp_i(y, y2, t),
            w: lerp_i(w, w2, t),
            h: lerp_i(h, h2, t),
            width: width + (width2 - width) * t,
            color: lerp_color(color, color2, t),
        },
        (Call::FillRect { x, y, w, h, color }, Call::FillRect { x: x2, y: y2, w: w2, h: h2, color: color2 }) => {
            Call::FillRect {
                x: lerp_i(x, x2, t),
                y: lerp_i(y, y2, t),
                w: lerp_i(w, w2, t),
                h: lerp_i(h, h2, t),
                color: lerp_color(color, color2, t),
            }
        }
        (
            Call::Image { image, x0, y0, x1, y1, color, mode },
            Call::Image { image: image2, x0: a0, y0: b0, x1: a1, y1: b1, color: color2, mode: mode2 },
        ) if image == image2 && mode == mode2 => Call::Image {
            image,
            x0: lerp_i(x0, a0, t),
            y0: lerp_i(y0, b0, t),
            x1: lerp_i(x1, a1, t),
            y1: lerp_i(y1, b1, t),
            color: lerp_color(color, color2, t),
            mode,
        },
        _ => return None,
    })
}

/// Draws the transition at `state` with the display calls captured.
fn captured_draw(transition: usize, draw: extern "thiscall" fn(usize), offset: usize, state: i32) -> Vec<(usize, Call)> {
    unsafe { *((transition + offset) as *mut i32) = state };
    CAPTURING.store(true, Ordering::Relaxed);
    draw(transition);
    CAPTURING.store(false, Ordering::Relaxed);
    std::mem::take(&mut *CAPTURED.lock().unwrap())
}

extern "C" fn draw_transition(transition: usize, window: usize) {
    let draw: extern "thiscall" fn(usize) =
        unsafe { std::mem::transmute(*(*(transition as *const *const usize)).add(TRANSITION_DRAW_VSLOT)) };
    let style = unsafe { *((window + WINDOW_STYLE) as *const i32) };
    let Some(offset) = draw_state_offset(style) else {
        draw(transition);
        return;
    };
    let state = unsafe { *((transition + offset) as *const i32) };
    let update = frame::tick_count();
    let previous = {
        let mut tracks = TRACKS.lock().unwrap();
        let map = tracks.get_or_insert_with(FxHashMap::default);
        let track = map.entry(transition).or_insert(Track { previous: state, current: state, update });
        if track.current != state {
            track.previous = track.current;
            track.current = state;
            track.update = update;
        }
        (track.update == update && track.previous != state).then_some(track.previous)
    };
    let t = frame::tick_factor();
    let Some(previous) = previous.filter(|_| t < 1.0) else {
        draw(transition);
        return;
    };
    let from = captured_draw(transition, draw, offset, previous);
    let to = captured_draw(transition, draw, offset, state);
    let blended: Option<Vec<(usize, Call)>> = (from.len() == to.len())
        .then(|| from.iter().zip(&to).map(|((d, a), (_, b))| blend(a, b, t).map(|c| (*d, c))).collect())
        .flatten();
    for (display, call) in blended.as_ref().unwrap_or(&to) {
        emit(*display, call);
    }
}

/// Replaces `mov reg, [ecx]; call [reg+0x10]`. ECX is the transition, EAX its TransitionWindow.
#[unsafe(naked)]
unsafe extern "C" fn draw_stub() {
    naked_asm!(
        "push eax",
        "push ecx",
        "call {draw}",
        "add esp, 8",
        "ret",
        draw = sym draw_transition,
    )
}
