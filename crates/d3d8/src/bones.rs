//! Renders controlled bones (turrets, wheels, recoil) between their last two game updates. The
//! Control_Bone and Release_Bone overrides of every model class we see get hooked through its vtable.

use std::sync::Mutex;

use rustc_hash::FxHashMap;

use crate::matrix::{self, Matrix};
use crate::{frame, game, log, objects};

struct Addresses {
    control_bone_vslot: usize,
    release_bone_vslot: usize,
    /// RenderObjClass's own Control_Bone and Release_Bone, which do nothing.
    control_bone_stub: usize,
    release_bone_stub: usize,
}

static ZERO_HOUR: Addresses =
    Addresses { control_bone_vslot: 0xC8 / 4, release_bone_vslot: 0xC0 / 4, control_bone_stub: 0x0062_02B0, release_bone_stub: 0x007C_6300 };
static GENERALS: Addresses =
    Addresses { control_bone_vslot: 0xCC / 4, release_bone_vslot: 0xC4 / 4, control_bone_stub: 0x0072_D5D0, release_bone_stub: 0x0072_D5B0 };

type ControlBone = extern "thiscall" fn(usize, i32, *const Matrix, u32);
type ReleaseBone = extern "thiscall" fn(usize, i32);

#[derive(Clone, Copy)]
struct Originals {
    control: usize,
    release: usize,
}

struct Entry {
    previous: Matrix,
    current: Matrix,
    world_space: u32,
    update: u64,
    blended: bool,
}

#[derive(Default)]
struct State {
    /// Every vtable checked so far, with the originals of the ones we hooked.
    vtables: FxHashMap<usize, Option<Originals>>,
    bones: FxHashMap<(usize, i32), Entry>,
    pruned: u64,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);

/// Hooks the bone methods of `obj`'s class the first time we see that class.
pub fn discover(obj: usize) {
    let vtable = unsafe { *(obj as *const usize) };
    let mut state = STATE.lock().unwrap();
    let s = state.get_or_insert_with(State::default);
    if s.vtables.contains_key(&vtable) {
        return;
    }
    let originals = hook(vtable);
    s.vtables.insert(vtable, originals);
}

fn hook(vtable: usize) -> Option<Originals> {
    let a = game::pick(&ZERO_HOUR, Some(&GENERALS))?;
    let (start, end) = game::image_range();
    let in_image = |a: usize| a >= start && a < end;
    if !in_image(vtable) {
        return None;
    }
    let slots = vtable as *const usize;
    let control = unsafe { *slots.add(a.control_bone_vslot) };
    let release = unsafe { *slots.add(a.release_bone_vslot) };
    if control == a.control_bone_stub || !in_image(control) || !in_image(release) {
        return None;
    }
    unsafe {
        game::write_u32(vtable + a.control_bone_vslot * 4, control_bone as *const () as u32);
        if release != a.release_bone_stub {
            game::write_u32(vtable + a.release_bone_vslot * 4, release_bone as *const () as u32);
        }
    }
    log!("bone controls of render object class {vtable:#010x} hooked (Control_Bone {control:#010x})");
    Some(Originals { control, release })
}

fn originals(obj: usize) -> Option<Originals> {
    let vtable = unsafe { *(obj as *const usize) };
    STATE.lock().unwrap().as_ref()?.vtables.get(&vtable).copied().flatten()
}

extern "thiscall" fn control_bone(obj: usize, bone: i32, m: *const Matrix, world_space: u32) {
    let Some(o) = originals(obj) else { return };
    let original: ControlBone = unsafe { std::mem::transmute(o.control) };
    original(obj, bone, m, world_space);
    let m = unsafe { *m };
    let update = frame::update_count();
    let mut state = STATE.lock().unwrap();
    let Some(s) = state.as_mut() else { return };
    match s.bones.get_mut(&(obj, bone)) {
        Some(e) if e.update == update => {
            e.current = m;
            e.world_space = world_space;
        }
        Some(e) => {
            e.previous = e.current;
            e.current = m;
            e.world_space = world_space;
            e.update = update;
        }
        None => {
            objects::add_ref(obj);
            s.bones.insert((obj, bone), Entry { previous: m, current: m, world_space, update, blended: false });
        }
    }
}

extern "thiscall" fn release_bone(obj: usize, bone: i32) {
    let Some(o) = originals(obj) else { return };
    let original: ReleaseBone = unsafe { std::mem::transmute(o.release) };
    original(obj, bone);
    let removed = STATE.lock().unwrap().as_mut().and_then(|s| s.bones.remove(&(obj, bone)));
    if removed.is_some() {
        objects::release(obj);
    }
}

fn control(obj: usize, bone: i32, m: &Matrix, world_space: u32) {
    if let Some(o) = originals(obj) {
        let original: ControlBone = unsafe { std::mem::transmute(o.control) };
        original(obj, bone, m, world_space);
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
            s.bones.retain(|&(obj, _), e| {
                let keep = e.update + objects::KEEP_UPDATES >= update;
                if !keep {
                    dropped.push(obj);
                }
                keep
            });
        }
        if t < 1.0 {
            for (&(obj, bone), e) in s.bones.iter_mut() {
                if e.update == update && !e.blended && e.previous != e.current {
                    e.blended = true;
                    work.push((obj, bone, matrix::blend(&e.previous, &e.current, t), e.world_space));
                }
            }
        }
    }
    for (obj, bone, m, world_space) in work {
        control(obj, bone, &m, world_space);
    }
    for obj in dropped {
        objects::release(obj);
    }
}

/// Puts back the bone controls the game set.
pub fn restore() {
    let mut work = Vec::new();
    {
        let mut state = STATE.lock().unwrap();
        let Some(s) = state.as_mut() else { return };
        for (&(obj, bone), e) in s.bones.iter_mut() {
            if e.blended {
                e.blended = false;
                work.push((obj, bone, e.current, e.world_space));
            }
        }
    }
    for (obj, bone, m, world_space) in work {
        control(obj, bone, &m, world_space);
    }
}
