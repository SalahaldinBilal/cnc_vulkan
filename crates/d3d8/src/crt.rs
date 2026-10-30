//! Faster `floor` and `ceil` for the game's msvcrt imports. Results must match msvcrt's bit for bit,
//! since the game logic calls them too, so they're compared on hard cases before being used.

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::{game, iat, log};

static FLOOR: AtomicUsize = AtomicUsize::new(0);
static CEIL: AtomicUsize = AtomicUsize::new(0);

/// Beyond this every double is a whole number, and below it a truncation fits an i32.
const LIMIT: f64 = 2_147_483_648.0;

fn original(slot: &AtomicUsize) -> extern "C" fn(f64) -> f64 {
    unsafe { std::mem::transmute(slot.load(Ordering::Relaxed)) }
}

extern "C" fn fast_floor(x: f64) -> f64 {
    if !(x.abs() < LIMIT) {
        return original(&FLOOR)(x);
    }
    let t = (x as i32) as f64;
    let r = if t > x { t - 1.0 } else { t };
    if r == 0.0 { 0.0f64.copysign(x) } else { r }
}

extern "C" fn fast_ceil(x: f64) -> f64 {
    if !(x.abs() < LIMIT) {
        return original(&CEIL)(x);
    }
    let t = (x as i32) as f64;
    let r = if t < x { t + 1.0 } else { t };
    if r == 0.0 { 0.0f64.copysign(x) } else { r }
}

const CASES: &[f64] = &[
    0.0, -0.0, 0.5, -0.5, 1.0, -1.0, 1.5, -1.5, 2.5, -2.5, 0.999_999_9, -0.999_999_9, 1e-300, -1e-300,
    123_456.789, -123_456.789, 2_147_483_647.5, -2_147_483_647.5, -2_147_483_648.0, 2_147_483_648.0,
    -2_147_483_648.5, 4.5e15 + 0.5, -4.5e15 - 0.5, 1e300, -1e300, f64::INFINITY, f64::NEG_INFINITY,
    f64::MIN_POSITIVE, -f64::MIN_POSITIVE, 3.999_999_76, -3.999_999_76, 8_388_607.5, -8_388_607.5,
];

fn same(a: f64, b: f64) -> bool {
    a.to_bits() == b.to_bits() || (a.is_nan() && b.is_nan())
}

fn matches(ours: extern "C" fn(f64) -> f64, theirs: extern "C" fn(f64) -> f64) -> bool {
    CASES.iter().chain(&[f64::NAN]).all(|&x| same(ours(x), theirs(x)))
}

#[cfg(test)]
mod tests {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryA};

    use super::*;

    #[test]
    fn floor_and_ceil_match_msvcrt() {
        let msvcrt = unsafe { LoadLibraryA(c"msvcrt.dll".as_ptr() as *const u8) };
        assert!(!msvcrt.is_null());
        for (name, slot, ours) in [(c"floor", &FLOOR, fast_floor as extern "C" fn(f64) -> f64), (c"ceil", &CEIL, fast_ceil)] {
            let theirs = unsafe { GetProcAddress(msvcrt, name.as_ptr() as *const u8) }.unwrap();
            slot.store(theirs as usize, Ordering::Relaxed);
            assert!(matches(ours, original(slot)), "{name:?}");
            let mut x = -3.0;
            while x < 3.0 {
                assert!(same(ours(x), original(slot)(x)), "{name:?}({x})");
                x += 0.0078125 / 3.0;
            }
        }
    }
}

pub unsafe fn install() {
    if game::build().is_none() {
        return;
    }
    for (name, slot, ours) in [("floor", &FLOOR, fast_floor as extern "C" fn(f64) -> f64), ("ceil", &CEIL, fast_ceil)] {
        let Some(previous) = (unsafe { iat::hook_main_module("msvcrt.dll", name, ours as usize) }) else { continue };
        slot.store(previous, Ordering::Relaxed);
        if !matches(ours, original(slot)) {
            unsafe { iat::hook_main_module("msvcrt.dll", name, previous) };
            log!("our {name} differs from msvcrt's, the game keeps msvcrt's");
        }
    }
}
