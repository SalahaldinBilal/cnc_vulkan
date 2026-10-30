//! Renders line effects (lasers, ropes, tracers) between their last two game updates. Their draw
//! modules set line points, widths and opacity once per drawable pass, so we wrap those calls,
//! record the values and blend them while rendering. Lines are reference counted render objects, we
//! hold a reference while we keep their pointer.

use std::sync::Mutex;

use rustc_hash::{FxHashMap, FxHashSet};

use crate::{frame, game, log, objects};

struct Addresses {
    /// SegmentedLineClass::Set_Points(count, points) and Set_Width(width).
    set_points: usize,
    set_width: usize,
    /// Line3DClass::Set_Opacity(opacity).
    set_opacity: usize,
    point_calls: &'static [usize],
    width_calls: &'static [usize],
    opacity_calls: &'static [usize],
}

static ZERO_HOUR: Addresses = Addresses {
    set_points: 0x0087_2600,
    set_width: 0x0087_2710,
    set_opacity: 0x0087_5E90,
    point_calls: &[0x007B_EE92, 0x0078_973A, 0x0078_9F26, 0x0078_A11E, 0x007C_2CA8],
    width_calls: &[0x007B_EE7E, 0x0078_9428, 0x007B_E7F1, 0x0078_9513, 0x0078_A10D, 0x007C_2CDB],
    opacity_calls: &[0x007C_5CB2],
};

static GENERALS: Addresses = Addresses {
    set_points: 0x0083_B670,
    set_width: 0x0083_BA40,
    set_opacity: 0x0084_3170,
    point_calls: &[0x0079_5F22, 0x0076_6ABF, 0x0076_7279, 0x0079_9988],
    width_calls: &[0x0079_5F0E, 0x0076_6838, 0x0079_57F1, 0x0079_99BB],
    opacity_calls: &[0x0079_CD52],
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}
/// Line3DClass keeps its RenderObjClass part 4 bytes in.
const LINE3D_RENDER_OBJECT: usize = 4;

type Point = [f32; 3];

#[derive(Clone, Copy, PartialEq, Eq, Hash)]
enum Kind {
    Points,
    Width,
    Opacity,
}

#[derive(Clone)]
enum Values {
    Points(Vec<Point>),
    Scalar(f32),
}

struct Entry {
    previous: Values,
    current: Values,
    update: u64,
    blended: bool,
}

#[derive(Default)]
struct State {
    entries: FxHashMap<(usize, Kind), Entry>,
    /// Render object we hold a reference on, per line.
    refs: FxHashMap<usize, usize>,
    pruned: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

pub unsafe fn install() {
    let a = addresses();
    let sites = a
        .point_calls
        .iter()
        .map(|&at| (at, a.set_points, set_points as *const () as usize))
        .chain(a.width_calls.iter().map(|&at| (at, a.set_width, set_width as *const () as usize)))
        .chain(a.opacity_calls.iter().map(|&at| (at, a.set_opacity, set_opacity as *const () as usize)));
    let sites: Vec<_> = sites.collect();
    if let Some((at, _, _)) = sites.iter().find(|(at, target, _)| game::call_target(*at) != Some(*target)) {
        log!("line call at {at:#010x} isn't what we expect, line effects not blended");
        return;
    }
    for (at, _, hook) in sites {
        unsafe { game::patch_call(at, hook) };
    }
}

fn record(line: usize, render_object: usize, kind: Kind, value: Values) {
    if !objects::is_recording() {
        return;
    }
    let update = frame::update_count();
    let mut state = STATE.lock().unwrap();
    let s = state.get_or_insert_with(State::default);
    s.refs.entry(line).or_insert_with(|| {
        objects::add_ref(render_object);
        render_object
    });
    match s.entries.get_mut(&(line, kind)) {
        Some(e) if e.update == update => e.current = value,
        Some(e) => {
            e.previous = std::mem::replace(&mut e.current, value);
            e.update = update;
        }
        None => {
            s.entries.insert((line, kind), Entry { previous: value.clone(), current: value, update, blended: false });
        }
    }
}

fn call_points_raw(line: usize, count: u32, points: *const Point) {
    let f: extern "thiscall" fn(usize, u32, *const Point) = unsafe { std::mem::transmute(addresses().set_points) };
    f(line, count, points);
}

fn call_points(line: usize, points: &[Point]) {
    call_points_raw(line, points.len() as u32, points.as_ptr());
}

fn call_scalar(kind: Kind, line: usize, v: f32) {
    let addr = if kind == Kind::Width { addresses().set_width } else { addresses().set_opacity };
    let f: extern "thiscall" fn(usize, f32) = unsafe { std::mem::transmute(addr) };
    f(line, v);
}

extern "thiscall" fn set_points(line: usize, count: u32, points: *const Point) {
    call_points_raw(line, count, points);
    if !points.is_null() {
        let v = unsafe { std::slice::from_raw_parts(points, count as usize) }.to_vec();
        record(line, line, Kind::Points, Values::Points(v));
    }
}

extern "thiscall" fn set_width(line: usize, width: f32) {
    call_scalar(Kind::Width, line, width);
    record(line, line, Kind::Width, Values::Scalar(width));
}

extern "thiscall" fn set_opacity(line: usize, opacity: f32) {
    call_scalar(Kind::Opacity, line, opacity);
    record(line, line + LINE3D_RENDER_OBJECT, Kind::Opacity, Values::Scalar(opacity));
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

fn apply_value(line: usize, kind: Kind, v: &Values) {
    match v {
        Values::Points(p) => call_points(line, p),
        Values::Scalar(x) => call_scalar(kind, line, *x),
    }
}

/// Called right before the scene renders.
pub fn apply(t: f32) {
    let update = frame::update_count();
    let mut work = Vec::new();
    let mut dropped = Vec::new();
    {
        let mut state = STATE.lock().unwrap();
        let Some(s) = state.as_mut() else { return };
        if s.pruned != update {
            s.pruned = update;
            s.entries.retain(|_, e| e.update + objects::KEEP_UPDATES >= update);
            let live: FxHashSet<usize> = s.entries.keys().map(|(line, _)| *line).collect();
            s.refs.retain(|line, render_object| {
                let keep = live.contains(line);
                if !keep {
                    dropped.push(*render_object);
                }
                keep
            });
        }
        if t < 1.0 {
            for (&(line, kind), e) in s.entries.iter_mut() {
                if e.update != update || e.blended {
                    continue;
                }
                let blended = match (&e.previous, &e.current) {
                    (Values::Points(a), Values::Points(b)) if a.len() == b.len() && a != b => Values::Points(
                        a.iter().zip(b).map(|(p, q)| [0, 1, 2].map(|i| lerp(p[i], q[i], t))).collect(),
                    ),
                    (Values::Scalar(a), Values::Scalar(b)) if a != b => Values::Scalar(lerp(*a, *b, t)),
                    _ => continue,
                };
                e.blended = true;
                work.push((line, kind, blended));
            }
        }
    }
    for (line, kind, v) in work {
        apply_value(line, kind, &v);
    }
    for render_object in dropped {
        objects::release(render_object);
    }
}

/// Puts back the values the game set.
pub fn restore() {
    let mut work = Vec::new();
    {
        let mut state = STATE.lock().unwrap();
        let Some(s) = state.as_mut() else { return };
        for (&(line, kind), e) in s.entries.iter_mut() {
            if e.blended {
                e.blended = false;
                work.push((line, kind, e.current.clone()));
            }
        }
    }
    for (line, kind, v) in work {
        apply_value(line, kind, &v);
    }
}
