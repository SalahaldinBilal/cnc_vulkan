//! UI layout. The game positions every .wnd window by scaling its 800x600 rectangle separately in x
//! and y, which stretches the UI on wide screens. We replace that math with one uniform scale. Windows
//! that cover the whole layout still fill the screen. Everything else keeps its aspect ratio and is
//! anchored by which third of the layout it sits in, and children move with their anchored ancestor.

use std::arch::naked_asm;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use crate::config::{self, UiLayout};
use crate::controlbar;
use crate::game::{self, Build};
use crate::log;

/// Distance in layout pixels within which a rectangle counts as covering the whole layout.
const EDGE: i32 = 2;
const MAX_DEPTH: usize = 32;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Anchor {
    Start,
    Center,
    End,
}

/// The game's stack frame at the patched spot, starting at `esp + 0x28`.
#[repr(C)]
struct Frame {
    left: i32,
    top: i32,
    right: i32,
    bottom: i32,
    _unused: [i32; 6],
    create_w: i32,
    create_h: i32,
}

type Anchors = (Anchor, Anchor);
type Rect = [i32; 4];

/// Anchors of the windows currently being parsed, by nesting depth. `None` means stretched to fill.
static ANCHORS: Mutex<[Option<Anchors>; MAX_DEPTH]> = Mutex::new([None; MAX_DEPTH]);
/// Anchored windows parsed so far under the current parent at each depth, in layout coordinates.
static SIBLINGS: Mutex<Vec<Vec<(Rect, Anchors)>>> = Mutex::new(Vec::new());
/// Set while ControlBar.wnd is the layout being loaded.
static LOADING_CONTROL_BAR: AtomicBool = AtomicBool::new(false);
/// Set while a layout drawn over a full-screen movie is loaded. The movie stretches with the root
/// window, so every window over it stretches too.
static LOADING_MOVIE_LAYOUT: AtomicBool = AtomicBool::new(false);
const MOVIE_LAYOUTS: [&str; 2] = ["singleplayerloadscreen.wnd", "challengeloadscreen.wnd"];
/// Where the SCREENRECT parser continues after its scaling block: `test ebx, ebx`.
static SCREEN_RECT_RESUME: AtomicUsize = AtomicUsize::new(0);

/// Called with each .wnd path the game opens, lowercase.
pub fn set_loading_layout(path: &str) {
    LOADING_CONTROL_BAR.store(path.ends_with("\\controlbar.wnd"), Ordering::Relaxed);
    let file = path.rsplit(['\\', '/']).next().unwrap_or(path);
    LOADING_MOVIE_LAYOUT.store(MOVIE_LAYOUTS.contains(&file), Ordering::Relaxed);
    SIBLINGS.lock().unwrap().clear();
}

/// The anchor of an earlier sibling that holds the whole rectangle. Some layouts put a panel's
/// contents next to it instead of inside it (base Generals' score screen), and they have to move
/// with the panel.
fn container_anchor(depth: usize, rect: Rect) -> Option<Anchors> {
    let mut siblings = SIBLINGS.lock().unwrap();
    siblings.resize_with(MAX_DEPTH, Vec::new);
    siblings[depth + 1..].iter_mut().for_each(Vec::clear);
    siblings[depth]
        .iter()
        .rev()
        .find(|(r, _)| r[0] <= rect[0] && r[1] <= rect[1] && r[2] >= rect[2] && r[3] >= rect[3])
        .map(|&(_, a)| a)
}

fn add_sibling(depth: usize, rect: Rect, anchor: Option<Anchors>) {
    if let Some(a) = anchor {
        SIBLINGS.lock().unwrap()[depth].push((rect, a));
    }
}

pub fn install() {
    let Ok(cfg) = config::get() else { return };
    if cfg.ui.layout == UiLayout::Stretch {
        log!("ui layout 'stretch': original window layout kept");
        return;
    }
    let (Some(build), Some(&resume)) = (game::build(), game::pick(&0x006D_CB9D, Some(&0x006C_4E8E))) else { return };
    SCREEN_RECT_RESUME.store(resume, Ordering::Relaxed);
    unsafe { game::patch_jump(build.screen_rect_patch, screen_rect_stub as *const () as usize) };
    controlbar::install(build);
    log!("ui layout 'fit' installed, scale {}", cfg.ui.scale);
}

/// Uniform scale and top-left offset of a layout of size `layout_w` x `layout_h` placed on the screen.
pub fn canvas(screen: (i32, i32), layout_w: i32, layout_h: i32, anchor: (Anchor, Anchor)) -> (f64, i32, i32) {
    let fit = (screen.0 as f64 / layout_w as f64).min(screen.1 as f64 / layout_h as f64);
    let ui_scale = config::get().map_or(1.0, |c| c.ui.scale.clamp(0.25, 1.0)) as f64;
    let scale = fit * ui_scale;
    let ox = offset(anchor.0, screen.0, (layout_w as f64 * scale) as i32);
    let oy = offset(anchor.1, screen.1, (layout_h as f64 * scale) as i32);
    (scale, ox, oy)
}

pub fn display_size(build: &Build) -> (i32, i32) {
    unsafe {
        let display = *(build.the_display as *const *mut usize);
        let vtable = *display as *const usize;
        let width: extern "thiscall" fn(*mut usize) -> i32 = std::mem::transmute(*vtable.add(8));
        let height: extern "thiscall" fn(*mut usize) -> i32 = std::mem::transmute(*vtable.add(9));
        (width(display), height(display))
    }
}

fn parent_depth(build: &Build) -> usize {
    let top = unsafe { *(build.window_parent_stack as *const usize) };
    (top.saturating_sub(build.window_parent_stack + 4) / 4).min(MAX_DEPTH - 1)
}

fn axis_anchor(lo: i32, hi: i32, size: i32) -> Anchor {
    let center = lo + hi;
    if center * 3 < size * 2 {
        Anchor::Start
    } else if center * 3 > size * 4 {
        Anchor::End
    } else {
        Anchor::Center
    }
}

fn offset(anchor: Anchor, screen: i32, canvas: i32) -> i32 {
    match anchor {
        Anchor::Start => 0,
        Anchor::Center => (screen - canvas) / 2,
        Anchor::End => screen - canvas,
    }
}

extern "C" fn transform(frame: &mut Frame) {
    let Some(build) = game::build() else { return };
    let (screen_w, screen_h) = display_size(build);
    let (cw, ch) = (frame.create_w, frame.create_h);
    if cw <= 0 || ch <= 0 {
        return;
    }
    let depth = parent_depth(build);

    let mut anchors = ANCHORS.lock().unwrap();
    let inherited = anchors[..depth].iter().flatten().next().copied();
    let fills = frame.left <= EDGE && frame.top <= EDGE && frame.right >= cw - EDGE && frame.bottom >= ch - EDGE;
    let rect = [frame.left, frame.top, frame.right, frame.bottom];
    let container = container_anchor(depth, rect);
    let anchor = match inherited {
        Some(a) => Some(a),
        None if fills || LOADING_MOVIE_LAYOUT.load(Ordering::Relaxed) => None,
        None => container.or(Some((axis_anchor(frame.left, frame.right, cw), axis_anchor(frame.top, frame.bottom, ch)))),
    };
    anchors[depth] = anchor;
    drop(anchors);
    if inherited.is_none() {
        add_sibling(depth, rect, anchor);
    }

    let map = |v: i32, s: f64, base: i32| base + (v as f64 * s) as i32;
    match anchor {
        None => {
            let (sx, sy) = (screen_w as f64 / cw as f64, screen_h as f64 / ch as f64);
            frame.left = map(frame.left, sx, 0);
            frame.right = map(frame.right, sx, 0);
            frame.top = map(frame.top, sy, 0);
            frame.bottom = map(frame.bottom, sy, 0);
        }
        Some(a) => {
            let (scale, ox, oy) = canvas((screen_w, screen_h), cw, ch, a);
            if LOADING_CONTROL_BAR.load(Ordering::Relaxed) && a == controlbar::ANCHOR {
                let half = controlbar::wide_half(screen_w, cw, scale);
                let edge = |v: i32| v as f64;
                let (left, right) =
                    controlbar::wide_span(edge(frame.left), edge(frame.right), edge(frame.top), edge(frame.bottom), half);
                frame.left = ox + (left * scale).round() as i32;
                frame.right = ox + (right * scale).round() as i32;
                frame.top = map(frame.top, scale, oy);
                frame.bottom = map(frame.bottom, scale, oy);
                return;
            }
            frame.left = map(frame.left, scale, ox);
            frame.right = map(frame.right, scale, ox);
            frame.top = map(frame.top, scale, oy);
            frame.bottom = map(frame.bottom, scale, oy);
        }
    }
}

/// Jumped to from the SCREENRECT parser in place of its scaling block. Pops the pending sscanf
/// arguments the original block popped, hands the rectangle to `transform`, and returns with the
/// scaled corners in ESI, EDI, EBP and EAX like the original code.
#[unsafe(naked)]
unsafe extern "C" fn screen_rect_stub() {
    naked_asm!(
        "add esp, 0xc",
        "lea eax, [esp + 0x28]",
        "push eax",
        "call {transform}",
        "add esp, 4",
        "mov esi, [esp + 0x28]",
        "mov edi, [esp + 0x2c]",
        "mov ebp, [esp + 0x30]",
        "mov eax, [esp + 0x34]",
        "push dword ptr [{resume}]",
        "ret",
        transform = sym transform,
        resume = sym SCREEN_RECT_RESUME,
    )
}
