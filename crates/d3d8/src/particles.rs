//! Renders particles between their last two updates. A particle step is known from the particle's
//! own rates (position += velocity + system drift, angle += spin, size += growth, alpha and color
//! += their rates), so the previous state is worked out backward instead of stored.

use std::sync::Mutex;

use crate::{frame, game};

const MANAGER_UPDATE_VSLOT: usize = 0x10 / 4;
const SHADER_ADDITIVE: u32 = 1;
const MANAGER_LAST_FRAME: usize = 0x94;
const MANAGER_SYSTEMS: usize = 0x80;
const CLIENT_FRAME: usize = 0x0C;
const VELOCITY: usize = 0x08;
const POSITION: usize = 0x14;

/// Where each build keeps the particle state. Generals' particles turn on three axes, of which
/// the one drawn is Z.
struct Layout {
    the_particle_system_manager: usize,
    the_game_client: usize,
    /// The system's shader type, when alpha only steps for some of them.
    system_shader: Option<usize>,
    system_drift: usize,
    system_particles: usize,
    angle: usize,
    spin: usize,
    spin_damping: usize,
    size: usize,
    growth: usize,
    growth_damping: usize,
    color_scale: usize,
    faces_emitter: usize,
    next: usize,
    created: usize,
    alpha: usize,
    alpha_rate: usize,
    color: usize,
    color_rate: usize,
}

static ZERO_HOUR: Layout = Layout {
    the_particle_system_manager: 0x00A3_9CCC,
    the_game_client: 0x00A3_9B18,
    system_shader: Some(0x0C),
    system_drift: 0x1BC,
    system_particles: 0x25C,
    angle: 0x30,
    spin: 0x34,
    spin_damping: 0x38,
    size: 0x40,
    growth: 0x44,
    growth_damping: 0x48,
    color_scale: 0x10C,
    faces_emitter: 0x114,
    next: 0x118,
    created: 0x14C,
    alpha: 0x150,
    alpha_rate: 0x154,
    color: 0x15C,
    color_rate: 0x168,
};

static GENERALS: Layout = Layout {
    the_particle_system_manager: 0x00A6_C054,
    the_game_client: 0x00A6_BE90,
    system_shader: None,
    system_drift: 0x1EC,
    system_particles: 0x28C,
    angle: 0x38,
    spin: 0x44,
    spin_damping: 0x48,
    size: 0x50,
    growth: 0x54,
    growth_damping: 0x58,
    color_scale: 0x11C,
    faces_emitter: 0x124,
    next: 0x128,
    created: 0x15C,
    alpha: 0x160,
    alpha_rate: 0x164,
    color: 0x16C,
    color_rate: 0x178,
};

fn layout() -> Option<&'static Layout> {
    game::pick(&ZERO_HOUR, Some(&GENERALS))
}

#[derive(Clone, Copy)]
struct Saved {
    position: [f32; 3],
    angle: f32,
    size: f32,
    alpha: f32,
    color: [f32; 3],
}

struct State {
    /// The game update and client frame of the latest particle step.
    stepped: Option<(u64, u32)>,
    saved: Vec<(usize, Saved)>,
}

static STATE: Mutex<State> = Mutex::new(State { stepped: None, saved: Vec::new() });

unsafe fn read<T: Copy>(addr: usize) -> T {
    unsafe { *(addr as *const T) }
}

unsafe fn write<T>(addr: usize, value: T) {
    unsafe { *(addr as *mut T) = value };
}

/// Runs the particle manager update and notes whether particles stepped.
pub extern "C" fn update_manager() {
    let Some(l) = layout() else { return };
    unsafe {
        let manager = read::<usize>(l.the_particle_system_manager);
        if manager == 0 {
            return;
        }
        let before = read::<u32>(manager + MANAGER_LAST_FRAME);
        let update: extern "thiscall" fn(usize) =
            std::mem::transmute(*(read::<*const usize>(manager)).add(MANAGER_UPDATE_VSLOT));
        update(manager);
        if read::<u32>(manager + MANAGER_LAST_FRAME) != before {
            let client = read::<usize>(l.the_game_client);
            let frame = if client == 0 { 0 } else { read::<u32>(client + CLIENT_FRAME) };
            STATE.lock().unwrap().stepped = Some((frame::update_count(), frame));
        }
    }
}

/// Calls `f` with every particle of every system.
unsafe fn for_each_particle(l: &Layout, mut f: impl FnMut(usize, usize)) {
    unsafe {
        let manager = read::<usize>(l.the_particle_system_manager);
        if manager == 0 {
            return;
        }
        let head = read::<usize>(manager + MANAGER_SYSTEMS);
        let mut node = read::<usize>(head);
        while node != head {
            let system = read::<usize>(node + 8);
            if system != 0 {
                let mut particle = read::<usize>(system + l.system_particles);
                while particle != 0 {
                    f(system, particle);
                    particle = read::<usize>(particle + l.next);
                }
            }
            node = read::<usize>(node);
        }
    }
}

fn undamped(rate: f32, damping: f32) -> f32 {
    if damping != 0.0 { rate / damping } else { 0.0 }
}

/// Called right before the scene renders.
pub fn apply(t: f32) {
    let Some(l) = layout() else { return };
    let mut state = STATE.lock().unwrap();
    let Some((update, created_frame)) = state.stepped else { return };
    if update != frame::update_count() || t >= 1.0 || !state.saved.is_empty() {
        return;
    }
    let back = 1.0 - t;
    let mut saved = Vec::new();
    unsafe {
        for_each_particle(l, |system, p| {
            if read::<u32>(p + l.created) == created_frame {
                return;
            }
            let position: [f32; 3] = read(p + POSITION);
            let velocity: [f32; 3] = read(p + VELOCITY);
            let drift: [f32; 3] = read(system + l.system_drift);
            let angle: f32 = read(p + l.angle);
            let size: f32 = read(p + l.size);
            let alpha: f32 = read(p + l.alpha);
            let color: [f32; 3] = read(p + l.color);
            saved.push((p, Saved { position, angle, size, alpha, color }));

            write(p + POSITION, [0, 1, 2].map(|i| position[i] - (velocity[i] + drift[i]) * back));
            if read::<u8>(p + l.faces_emitter) == 0 {
                let spin = undamped(read(p + l.spin), read(p + l.spin_damping));
                write(p + l.angle, angle - spin * back);
            }
            let growth = undamped(read(p + l.growth), read(p + l.growth_damping));
            write(p + l.size, (size - growth * back).max(0.0));
            if l.system_shader.is_none_or(|o| read::<u32>(system + o) != SHADER_ADDITIVE) {
                let rate: f32 = read(p + l.alpha_rate);
                write(p + l.alpha, (alpha - rate * back).clamp(0.0, 1.0));
            }
            let rate: [f32; 3] = read(p + l.color_rate);
            let scale: f32 = read(p + l.color_scale);
            write(p + l.color, [0, 1, 2].map(|i| (color[i] - (rate[i] + scale) * back).clamp(0.0, 1.0)));
        });
    }
    state.saved = saved;
}

/// Puts back every particle value the blend replaced.
pub fn restore() {
    let Some(l) = layout() else { return };
    let mut state = STATE.lock().unwrap();
    for (p, s) in state.saved.drain(..) {
        unsafe {
            write(p + POSITION, s.position);
            write(p + l.angle, s.angle);
            write(p + l.size, s.size);
            write(p + l.alpha, s.alpha);
            write(p + l.color, s.color);
        }
    }
}
