//! Per-draw InGameUI overlays. World animations rise by a fixed step every draw, which assumed 30
//! draws per second, so we scale it by real time. Floating text rises by whole update counts, so we
//! draw it the unfinished part of an update lower.

use std::arch::naked_asm;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Instant;

use crate::game;

struct Addresses {
    /// `call updateAndDrawWorldAnimations` in InGameUI::preDraw.
    world_animations_call: usize,
    world_animations: usize,
    /// Operand of `fmul [1/30]` in `z += zRisePerSecond / 30`.
    world_animation_rise_operand: usize,
    /// `fild [esi+0x20]; fmul [ecx+speed]`: floating text frame count times rise speed, 9 bytes.
    floating_text_rise: usize,
    floating_text_rise_stub: unsafe extern "C" fn(),
    /// `fild qword [esp+0x24]` (logic frames left) `fmul [1/30]`: world animation fade, 10 bytes.
    world_animation_fade: usize,
    /// `mov eax, [ebp+0x24]; dec eax; lea ecx, [esp+0x14]; mov [ebp+0x24], eax`: the cinematic
    /// text countdown in W3DDisplay::draw, 11 bytes.
    cinematic_text_countdown: usize,
    /// TheSnowManager, its vtable's update slot and the update. Generals has no snow.
    snow: Option<(usize, usize, usize)>,
}

static ZERO_HOUR: Addresses = Addresses {
    world_animations_call: 0x0050_A728,
    world_animations: 0x0051_12D0,
    world_animation_rise_operand: 0x0051_133F,
    floating_text_rise: 0x0051_0B5D,
    floating_text_rise_stub: floating_text_rise_zero_hour,
    world_animation_fade: 0x0051_1392,
    cinematic_text_countdown: 0x0073_E3D3,
    snow: Some((0x00A3_9E28, 0x0096_9B94, 0x0079_E600)),
};

static GENERALS: Addresses = Addresses {
    world_animations_call: 0x0052_4367,
    world_animations: 0x0052_A470,
    world_animation_rise_operand: 0x0052_A4DB,
    floating_text_rise: 0x0052_9BB7,
    floating_text_rise_stub: floating_text_rise_generals,
    world_animation_fade: 0x0052_A52E,
    cinematic_text_countdown: 0x0071_74AE,
    snow: None,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

const FLOATING_TEXT_RISE_LEN: usize = 9;
const WORLD_ANIMATION_FADE_LEN: usize = 10;
const CINEMATIC_TEXT_COUNTDOWN_LEN: usize = 11;
static ONE_THIRTIETH: f32 = 1.0 / 30.0;
/// Longest gap a single draw makes up for.
const MAX_RISE_SECONDS: f32 = 0.1;

/// Seconds since the last world animation draw, as f32 bits. The rise multiply reads it directly.
pub static RISE_SECONDS: AtomicU32 = AtomicU32::new(0);
/// How much of the current update hasn't happened yet, as f32 bits, 0 when paused.
static UPDATE_REMAINING: AtomicU32 = AtomicU32::new(0);
/// Same, for values the logic step changes after an update's own draw, so 0 during that draw.
static LOGIC_REMAINING: AtomicU32 = AtomicU32::new(0);
static LAST_DRAW: Mutex<Option<Instant>> = Mutex::new(None);

pub unsafe fn install() {
    let a = addresses();
    unsafe {
        game::patch_call(a.world_animations_call, draw_world_animations as *const () as usize);
        game::write_u32(a.world_animation_rise_operand, RISE_SECONDS.as_ptr() as u32);
        game::replace_with_call(a.floating_text_rise, FLOATING_TEXT_RISE_LEN, a.floating_text_rise_stub as *const () as usize);
        game::replace_with_call(a.world_animation_fade, WORLD_ANIMATION_FADE_LEN, world_animation_fade as *const () as usize);
        if let Some((_, entry, _)) = a.snow {
            game::write_u32(entry, skip_snow_update as *const () as u32);
        }
        game::replace_with_call(
            a.cinematic_text_countdown,
            CINEMATIC_TEXT_COUNTDOWN_LEN,
            cinematic_text_countdown as *const () as usize,
        );
    }
}

/// The client update calls this once per game update, when the WW3D frame time only covers the
/// last redraw. Snow moves in `update_snow` instead.
extern "thiscall" fn skip_snow_update(_snow: usize) {}

/// Advances the snow clock by the frame time of this draw. Called from every draw after its sync.
pub fn update_snow() {
    let Some((manager, _, update)) = addresses().snow else { return };
    let snow = unsafe { *(manager as *const usize) };
    if snow != 0 {
        let update: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(update) };
        update(snow);
    }
}

/// Called before each render with the logic blend factor and whether the logic stepped this
/// frame. The floating text count moves in InGameUI::update, after this frame's draw, so the
/// draw right after a step still shows the old count in full. Logic frames left move in the step
/// itself, before the draw.
pub fn set_progress(t: f32, stepped: bool) {
    let remaining = 1.0 - t;
    UPDATE_REMAINING.store((if stepped { 0.0 } else { remaining }).to_bits(), Ordering::Relaxed);
    LOGIC_REMAINING.store(remaining.to_bits(), Ordering::Relaxed);
}

extern "thiscall" fn draw_world_animations(ui: usize) {
    let now = Instant::now();
    let seconds = {
        let mut last = LAST_DRAW.lock().unwrap();
        let s = last.map_or(0.0, |l| (now - l).as_secs_f32().min(MAX_RISE_SECONDS));
        *last = Some(now);
        s
    };
    RISE_SECONDS.store(seconds.to_bits(), Ordering::Relaxed);
    let original: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().world_animations) };
    original(ui);
}

/// Leaves (frame count - remaining) * speed on the FPU stack, as the replaced pair did without the subtraction.
macro_rules! floating_text_rise {
    ($name:ident, $speed:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "fild dword ptr [esi + 0x20]",
                "fsub dword ptr [{remaining}]",
                concat!("fmul dword ptr [ecx + ", $speed, "]"),
                "ret",
                remaining = sym UPDATE_REMAINING,
            )
        }
    };
}

floating_text_rise!(floating_text_rise_zero_hour, "0x1d0c");
floating_text_rise!(floating_text_rise_generals, "0x1c70");

/// The cinematic text shows for a number of draws, counted down every draw. Only tick frames count.
#[unsafe(naked)]
unsafe extern "C" fn cinematic_text_countdown() {
    naked_asm!(
        "mov eax, dword ptr [ebp + 0x24]",
        "cmp byte ptr [{tick}], 0",
        "je 2f",
        "dec eax",
        "2:",
        "lea ecx, [esp + 0x18]",
        "mov dword ptr [ebp + 0x24], eax",
        "ret",
        tick = sym crate::frame::TICK,
    )
}

/// Leaves (frames left + remaining) / 30 on the FPU stack. Called, so the frames left are at esp+0x28.
#[unsafe(naked)]
unsafe extern "C" fn world_animation_fade() {
    naked_asm!(
        "fild qword ptr [esp + 0x28]",
        "fadd dword ptr [{remaining}]",
        "fmul dword ptr [{one_thirtieth}]",
        "ret",
        remaining = sym LOGIC_REMAINING,
        one_thirtieth = sym ONE_THIRTIETH,
    )
}
