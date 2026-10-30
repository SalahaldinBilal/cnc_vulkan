//! Timers around the game's render systems, on with `timers = true` under `[debug]`. See knowledge-base/performance.md.

use std::arch::naked_asm;
use std::cell::RefCell;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

use crate::{game, log};

const SYSTEMS: [&str; 16] = [
    "models",
    "particles",
    "translucent",
    "ui",
    "bones",
    "shadow decals",
    "shadow volumes",
    "shadow volume updates",
    "static sort lists",
    "projected shadows",
    "other shadow work",
    "terrain",
    "water",
    "rest of the scene render and flush",
    "object list",
    "rest of the view draw",
];

enum Hook {
    /// A `call rel32` and the function it calls.
    Call(usize, usize),
    /// A vtable entry and the function it holds.
    Entry(usize, usize),
}

struct Site {
    system: usize,
    hook: Hook,
}

const fn call(system: usize, at: usize, target: usize) -> Site {
    Site { system, hook: Hook::Call(at, target) }
}

const fn entry(system: usize, at: usize, target: usize) -> Site {
    Site { system, hook: Hook::Entry(at, target) }
}

const MESH_FLUSH: usize = 0x0082_8C00;
const SORTING_FLUSH: usize = 0x0080_9900;
const WINDOW_DRAW: usize = 0x004F_9120;
const STATIC_SORT: usize = 0x007F_FA10;
const DECALS: usize = 0x0073_D990;
const DO_SHADOWS: usize = 0x0076_1860;

static GENERALS: [Site; SITES] = [
    call(0, 0x007F_EE9D, MESH_FLUSH),
    call(0, 0x007F_EF11, MESH_FLUSH),
    call(0, 0x0073_F170, MESH_FLUSH),
    call(0, 0x007F_EFDA, MESH_FLUSH),
    call(0, 0x007F_F049, MESH_FLUSH),
    call(0, 0x0074_6CF9, MESH_FLUSH),
    call(0, 0x0074_86F2, MESH_FLUSH),
    call(0, 0x0074_8789, MESH_FLUSH),
    call(0, 0x0074_89D0, MESH_FLUSH),
    call(0, 0x0074_8B8E, MESH_FLUSH),
    call(0, 0x0074_9152, MESH_FLUSH),
    call(0, 0x007F_FA83, MESH_FLUSH),
    call(0, 0x0074_8F2F, MESH_FLUSH),
    call(0, 0x0074_8FAC, MESH_FLUSH),
    call(0, 0x007F_F087, MESH_FLUSH),
    call(0, 0x007F_F0FB, MESH_FLUSH),
    call(1, 0x0074_6D68, 0x0075_7D20),
    call(2, 0x007F_EF2C, SORTING_FLUSH),
    call(2, 0x007F_F130, SORTING_FLUSH),
    call(2, 0x007F_F067, SORTING_FLUSH),
    call(2, 0x0074_6D70, SORTING_FLUSH),
    call(2, 0x007F_F11A, SORTING_FLUSH),
    call(3, 0x004F_920C, WINDOW_DRAW),
    call(3, 0x004F_922E, WINDOW_DRAW),
    call(3, 0x004F_9250, WINDOW_DRAW),
    call(4, 0x0086_107C, 0x0089_B5C0),
    entry(4, 0x009B_3D90, 0x0089_B5C0),
    call(5, 0x0073_EDD1, DECALS),
    call(5, 0x0073_F1D5, DECALS),
    call(6, 0x0076_18BC, 0x0077_DCF0),
    call(7, 0x0077_E0A3, 0x0077_B2E0),
    call(8, 0x0074_6D35, STATIC_SORT),
    call(8, 0x0074_915B, STATIC_SORT),
    call(9, 0x0076_1899, 0x0073_EC20),
    call(10, 0x0074_6CEC, DO_SHADOWS),
    call(10, 0x0074_6D2C, DO_SHADOWS),
    entry(11, 0x009A_5CEC, 0x0073_3170),
    entry(12, 0x009A_5F1C, 0x0073_7760),
    entry(13, 0x009A_6414, 0x0074_70F0),
    entry(14, 0x009A_6418, 0x0074_77F0),
    entry(15, 0x009A_676C, 0x0074_E100),
];

const SITES: usize = 41;

static TARGETS: [AtomicUsize; SITES] = [const { AtomicUsize::new(0) }; SITES];
static SITE_SYSTEM: [AtomicUsize; SITES] = [const { AtomicUsize::new(0) }; SITES];
static TICKS: [AtomicU64; SYSTEMS.len()] = [const { AtomicU64::new(0) }; SYSTEMS.len()];
static CALLS: [AtomicU64; SYSTEMS.len()] = [const { AtomicU64::new(0) }; SYSTEMS.len()];
static DRAWS: [AtomicU64; SYSTEMS.len()] = [const { AtomicU64::new(0) }; SYSTEMS.len()];

fn draws() -> u64 {
    crate::vk::stats::DRAWS_TOTAL.load(Ordering::Relaxed)
}

pub unsafe fn install() {
    if !crate::config::get().is_ok_and(|c| c.debug.timers) {
        return;
    }
    if game::build().is_none() {
        return;
    }
    if game::zero_hour() {
        log!("render system timers aren't mapped for Zero Hour yet");
        return;
    }
    let sites = &GENERALS;
    let (image_start, image_end) = game::image_range();
    let ours = |target: usize| target < image_start || target >= image_end;
    let current = |hook: &Hook| match *hook {
        Hook::Call(at, _) => game::call_target(at),
        Hook::Entry(at, _) => Some(unsafe { *(at as *const usize) }),
    };
    for site in sites {
        let (Hook::Call(at, expected) | Hook::Entry(at, expected)) = site.hook;
        if !current(&site.hook).is_some_and(|t| t == expected || ours(t)) {
            log!("render system timer site {at:#010x} isn't what we expect, timers off");
            return;
        }
    }
    for (k, site) in sites.iter().enumerate() {
        TARGETS[k].store(current(&site.hook).unwrap(), Ordering::Relaxed);
        SITE_SYSTEM[k].store(site.system, Ordering::Relaxed);
        let stub = STUBS[k] as *const () as usize;
        unsafe {
            match site.hook {
                Hook::Call(at, _) => game::patch_call(at, stub),
                Hook::Entry(at, _) => game::write_u32(at, stub as u32),
            }
        }
    }
    log!("render system timers on, {} call sites", sites.len());
}

struct Frame {
    slot: usize,
    ret: usize,
    system: usize,
    start: u64,
    children: u64,
    start_draws: u64,
    children_draws: u64,
}

thread_local! {
    static STACK: RefCell<Vec<Frame>> = const { RefCell::new(Vec::new()) };
}

fn ticks() -> u64 {
    unsafe { std::arch::x86::_rdtsc() }
}

/// `slot` holds the call's return address, which the stub then points at `after`.
extern "C" fn enter(slot: *const usize, site: usize) {
    let system = SITE_SYSTEM[site].load(Ordering::Relaxed);
    let ret = unsafe { *slot };
    let start_draws = draws();
    STACK.with_borrow_mut(|stack| stack.push(Frame { slot: slot as usize, ret, system, start: ticks(), children: 0, start_draws, children_draws: 0 }));
}

/// Returns where the timed call came from. Frames left by an unwind (deeper, so lower on the stack) are dropped.
extern "C" fn leave(esp: usize) -> usize {
    let now = ticks();
    STACK.with_borrow_mut(|stack| {
        let mut frame = stack.pop().expect("timed call returned without an entry");
        while stack.last().is_some_and(|f| f.slot < esp) {
            frame = stack.pop().unwrap();
        }
        let elapsed = now.wrapping_sub(frame.start);
        let drawn = draws().wrapping_sub(frame.start_draws);
        TICKS[frame.system].fetch_add(elapsed.saturating_sub(frame.children), Ordering::Relaxed);
        CALLS[frame.system].fetch_add(1, Ordering::Relaxed);
        DRAWS[frame.system].fetch_add(drawn.saturating_sub(frame.children_draws), Ordering::Relaxed);
        if let Some(parent) = stack.last_mut() {
            parent.children += elapsed;
            parent.children_draws += drawn;
        }
        frame.ret
    })
}

#[unsafe(naked)]
unsafe extern "C" fn after() {
    naked_asm!(
        "sub esp, 4",
        "push eax",
        "push edx",
        "lea eax, [esp + 12]",
        "push eax",
        "call {leave}",
        "add esp, 4",
        "mov dword ptr [esp + 8], eax",
        "pop edx",
        "pop eax",
        "ret",
        leave = sym leave,
    )
}

macro_rules! stubs {
    ($($name:ident $k:literal),* $(,)?) => {
        $(
            #[unsafe(naked)]
            unsafe extern "C" fn $name() {
                naked_asm!(
                    "push eax",
                    "push ecx",
                    "push edx",
                    "lea eax, [esp + 12]",
                    "push {k}",
                    "push eax",
                    "call {enter}",
                    "add esp, 8",
                    "lea eax, [{after}]",
                    "mov dword ptr [esp + 12], eax",
                    "pop edx",
                    "pop ecx",
                    "pop eax",
                    "jmp dword ptr [{targets} + {offset}]",
                    k = const $k,
                    offset = const $k * 4,
                    enter = sym enter,
                    after = sym after,
                    targets = sym TARGETS,
                )
            }
        )*
        static STUBS: [unsafe extern "C" fn(); SITES] = [$($name),*];
    };
}

stubs!(
    s0 0, s1 1, s2 2, s3 3, s4 4, s5 5, s6 6, s7 7, s8 8, s9 9, s10 10, s11 11, s12 12, s13 13, s14 14, s15 15, s16 16, s17 17,
    s18 18, s19 19, s20 20, s21 21, s22 22, s23 23, s24 24, s25 25, s26 26, s27 27, s28 28, s29 29, s30 30, s31 31, s32 32,
    s33 33, s34 34, s35 35, s36 36, s37 37, s38 38, s39 39, s40 40,
);

#[cfg(test)]
mod tests {
    use super::*;

    extern "C" fn product(a: u32, b: u32) -> u32 {
        a * b
    }

    extern "stdcall" fn sum(a: u32, b: u32) -> u32 {
        a + b
    }

    extern "thiscall" fn difference(this: u32, a: u32) -> u64 {
        this as u64 - a as u64 + (1 << 40)
    }

    extern "C" fn half(x: f32) -> f32 {
        x * 0.5
    }

    extern "C" fn outer(x: u32) -> u32 {
        let inner: extern "stdcall" fn(u32, u32) -> u32 = unsafe { std::mem::transmute(STUBS[1]) };
        let mut spin = 0u32;
        for i in 0..100_000 {
            spin = std::hint::black_box(spin.wrapping_add(i));
        }
        inner(x, spin & 0)
    }

    #[test]
    fn stubs_pass_calls_through() {
        let targets = [product as usize, sum as usize, difference as usize, half as usize, outer as usize];
        for (k, &t) in targets.iter().enumerate() {
            TARGETS[k].store(t, Ordering::Relaxed);
            SITE_SYSTEM[k].store(k, Ordering::Relaxed);
        }
        unsafe {
            let f0: extern "C" fn(u32, u32) -> u32 = std::mem::transmute(STUBS[0]);
            let f1: extern "stdcall" fn(u32, u32) -> u32 = std::mem::transmute(STUBS[1]);
            let f2: extern "thiscall" fn(u32, u32) -> u64 = std::mem::transmute(STUBS[2]);
            let f3: extern "C" fn(f32) -> f32 = std::mem::transmute(STUBS[3]);
            let f4: extern "C" fn(u32) -> u32 = std::mem::transmute(STUBS[4]);
            for _ in 0..3 {
                assert_eq!(f0(6, 7), 42);
                assert_eq!(f1(6, 7), 13);
                assert_eq!(f2(10, 3), 7 + (1 << 40));
                assert_eq!(f3(3.0), 1.5);
                assert_eq!(f4(5), 5);
            }
        }
        STACK.with_borrow(|s| assert!(s.is_empty()));
        assert_eq!(CALLS[1].load(Ordering::Relaxed), 6);
        assert_eq!(CALLS[4].load(Ordering::Relaxed), 3);
        assert!(TICKS[4].load(Ordering::Relaxed) > TICKS[1].load(Ordering::Relaxed));
        for k in 0..targets.len() {
            TARGETS[k].store(0, Ordering::Relaxed);
        }
    }
}

/// Logs each system's time per frame since the last report.
pub fn report(frames: u64, ticks_per_ms: f64) {
    if TARGETS[0].load(Ordering::Relaxed) == 0 {
        return;
    }
    let mut total = 0.0;
    let parts: Vec<String> = SYSTEMS
        .iter()
        .enumerate()
        .map(|(i, name)| {
            let ms = TICKS[i].swap(0, Ordering::Relaxed) as f64 / ticks_per_ms / frames as f64;
            let calls = CALLS[i].swap(0, Ordering::Relaxed) / frames;
            let drawn = DRAWS[i].swap(0, Ordering::Relaxed) / frames;
            total += ms;
            format!("{name} {ms:.2} ({calls} calls, {drawn} draws)")
        })
        .collect();
    log!("render systems per frame in ms, each without the timed systems under it, our draw preparation included: {}, {total:.2} in all", parts.join(", "));
}
