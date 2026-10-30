//! Frame rate cap, waited out right after each present, before the next frame reads input.

use std::sync::Mutex;
use std::time::{Duration, Instant};

use windows_sys::Win32::Foundation::CloseHandle;
use windows_sys::Win32::System::Threading::{
    CREATE_WAITABLE_TIMER_HIGH_RESOLUTION, CreateWaitableTimerExW, INFINITE, SetWaitableTimer, TIMER_ALL_ACCESS,
    WaitForSingleObject,
};

use crate::config::Display;
use crate::log;

/// The high resolution timer wakes up within about half a millisecond. The rest is spun out.
const SPIN_MARGIN: Duration = Duration::from_micros(700);

struct Limiter {
    period: Duration,
    next: Option<Instant>,
    /// High resolution waitable timer, or 0 on Windows builds older than 1803 where we only spin.
    timer: usize,
}

static LIMITER: Mutex<Option<Limiter>> = Mutex::new(None);

pub fn configure(display: &Display) {
    set_limit(display.fps_limit);
}

/// Sets the frame cap, 0 for none. With vsync on, a cap a little under the refresh rate keeps
/// a variable refresh rate display in its range.
pub fn set_limit(fps_limit: u32) {
    let mut limiter = LIMITER.lock().unwrap();
    if let Some(old) = limiter.take()
        && old.timer != 0
    {
        unsafe { CloseHandle(old.timer as _) };
    }
    if fps_limit == 0 {
        return;
    }
    let timer = unsafe {
        CreateWaitableTimerExW(
            std::ptr::null(),
            std::ptr::null(),
            CREATE_WAITABLE_TIMER_HIGH_RESOLUTION,
            TIMER_ALL_ACCESS,
        )
    } as usize;
    if timer == 0 {
        log!("high resolution timer unavailable, the fps limit will spin instead of sleeping");
    }
    log!("fps limit {fps_limit}");
    *limiter = Some(Limiter { period: Duration::from_secs(1) / fps_limit, next: None, timer });
}

pub fn wait() {
    let mut guard = LIMITER.lock().unwrap();
    let Some(limiter) = guard.as_mut() else { return };
    let now = Instant::now();
    let Some(next) = limiter.next else {
        limiter.next = Some(now + limiter.period);
        return;
    };
    if next <= now {
        let behind = now - next;
        limiter.next = Some(if behind > limiter.period { now + limiter.period } else { next + limiter.period });
        return;
    }
    let remaining = next - now;
    if limiter.timer != 0 && remaining > SPIN_MARGIN {
        let due = -(((remaining - SPIN_MARGIN).as_nanos() / 100) as i64);
        unsafe {
            SetWaitableTimer(limiter.timer as _, &due, 0, None, std::ptr::null(), 0);
            WaitForSingleObject(limiter.timer as _, INFINITE);
        }
    }
    while Instant::now() < next {
        std::hint::spin_loop();
    }
    limiter.next = Some(next + limiter.period);
}
