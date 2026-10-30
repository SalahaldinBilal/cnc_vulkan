//! The game mutes all its audio when its window loses focus (AudioManager::loseFocus from the
//! window procedure's WM_ACTIVATE). With "Sound in background" on, that call is skipped. See
//! knowledge-base/audio.md.

use std::sync::atomic::{AtomicBool, Ordering};

use crate::game;

struct Addresses {
    /// In the window procedure's WM_ACTIVATE handling: `mov edx, [ecx]; call [edx+slot]`, the
    /// loseFocus call on TheAudio (in ECX), 5 bytes.
    lose_focus_call: usize,
    lose_focus_vslot: usize,
}

static ZERO_HOUR: Addresses = Addresses { lose_focus_call: 0x0040_11C3, lose_focus_vslot: 0x28 / 4 };
static GENERALS: Addresses = Addresses { lose_focus_call: 0x0040_114A, lose_focus_vslot: 0x30 / 4 };

static IN_BACKGROUND: AtomicBool = AtomicBool::new(true);

fn addresses() -> Option<&'static Addresses> {
    game::pick(&ZERO_HOUR, Some(&GENERALS))
}

pub fn set_in_background(on: bool) {
    IN_BACKGROUND.store(on, Ordering::Relaxed);
}

pub unsafe fn install() {
    let Some(a) = addresses() else { return };
    unsafe { game::replace_with_call(a.lose_focus_call, 5, lose_focus as *const () as usize) };
}

extern "thiscall" fn lose_focus(audio: usize) {
    if IN_BACKGROUND.load(Ordering::Relaxed) {
        return;
    }
    let Some(a) = addresses() else { return };
    unsafe {
        let original: extern "thiscall" fn(usize) = std::mem::transmute(*(*(audio as *const *const usize)).add(a.lose_focus_vslot));
        original(audio);
    }
}
