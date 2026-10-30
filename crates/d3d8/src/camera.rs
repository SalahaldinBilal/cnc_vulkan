//! The tactical camera update (W3DView::update) runs every frame, and mouse and keyboard move the
//! camera directly. Unit follow, scripted moves and shake count frames, so they only step on the
//! 30 Hz tick. What they moved in a tick is spread over the following tick while rendering, and
//! the real view is put back after each draw.

use std::arch::naked_asm;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};

use crate::{frame, game};

struct Addresses {
    the_tactical_view: usize,
    w3d_view_vtable: usize,
    set_camera_transform: usize,
    camera_lock_drawable_vslot: usize,
    /// W3DView::scrollBy(delta) in the W3DView vtable. Only the look-at translator's frame tick calls it.
    scroll_by_entry: usize,
    scroll_by: usize,
    /// In W3DView::update, where ESI is the view + 0x7c: the start of the unit follow block, the
    /// instruction after its first 14 bytes, and the end of the block.
    follow_block: usize,
    follow_continue: usize,
    follow_skip: usize,
    follow_stub: unsafe extern "C" fn(),
    /// `call updateCameraMovements` (scripted rotate, zoom, pitch and waypoint path moves).
    scripted_moves_call: usize,
    update_camera_movements: usize,
    /// Shake: `fld [esi+0x2c]; fcomp [0.01]` starts it, both branches meet after it.
    shake_block: usize,
    shake_continue: usize,
    shake_skip: usize,
    shake_threshold: usize,
    shake_stub: unsafe extern "C" fn(),
    /// `fmul [reg+0x7e4]` (GlobalData camera adjust speed) in the zoom easing toward the wanted height.
    height_easing_rates: [usize; 2],
    /// `mov edx, [sync]; sub edx, [previous sync]; jz skip`: the drawable pass runs when sync time moved.
    drawable_pass_check: usize,
    drawable_pass_skip: usize,
    doing_rotate: usize,
    doing_pitch: usize,
    doing_zoom: usize,
    scripted_lock: usize,
    doing_waypoint_path: usize,
    scroll_amount: usize,
    /// The view values the camera transform is built from: position, angle, pitch, zoom, shake
    /// offset, Zero Hour's three effect rotations, the ground level and the effect pitch.
    fields: &'static [usize],
    /// Indices into `fields` that are angles.
    angles: &'static [usize],
}

static ZERO_HOUR: Addresses = Addresses {
    the_tactical_view: 0x00A3_9598,
    w3d_view_vtable: 0x0096_8B58,
    set_camera_transform: 0x0076_CB00,
    camera_lock_drawable_vslot: 0x160 / 4,
    scroll_by_entry: 0x0096_8BAC,
    scroll_by: 0x0076_E6D0,
    follow_block: 0x0076_D083,
    follow_continue: 0x0076_D091,
    follow_skip: 0x0076_D711,
    follow_stub: follow_stub_zero_hour,
    scripted_moves_call: 0x0076_D731,
    update_camera_movements: 0x0076_CF80,
    shake_block: 0x0076_D781,
    shake_continue: 0x0076_D78A,
    shake_skip: 0x0076_D7D3,
    shake_threshold: 0x0094_6318,
    shake_stub: shake_stub_zero_hour,
    height_easing_rates: [0x0076_D9E4, 0x0076_DA14],
    drawable_pass_check: 0x0076_DA84,
    drawable_pass_skip: 0x0076_DAA8,
    doing_rotate: 0xE8,
    doing_pitch: 0x110,
    doing_zoom: 0x134,
    scripted_lock: 0x135,
    doing_waypoint_path: 0x458,
    scroll_amount: 0x488,
    fields: &[0xC, 0x10, 0x14, 0x28, 0x2C, 0x40, 0x98, 0x9C, 0xAC, 0xB0, 0xB4, 0x494, 0x138],
    angles: &[3, 8, 9, 10],
};

/// Generals' W3DView has no shaker angles, and its fields from the waypoint path info on sit
/// 0xc0 later.
static GENERALS: Addresses = Addresses {
    the_tactical_view: 0x00A6_B95C,
    w3d_view_vtable: 0x009A_6770,
    set_camera_transform: 0x0074_CEB0,
    camera_lock_drawable_vslot: 0x154 / 4,
    scroll_by_entry: 0x009A_67C4,
    scroll_by: 0x0074_EA40,
    follow_block: 0x0074_D3F0,
    follow_continue: 0x0074_D3FE,
    follow_skip: 0x0074_DA81,
    follow_stub: follow_stub_generals,
    scripted_moves_call: 0x0074_DAA1,
    update_camera_movements: 0x0074_D2F0,
    shake_block: 0x0074_DAF1,
    shake_continue: 0x0074_DAFA,
    shake_skip: 0x0074_DB43,
    shake_threshold: 0x0098_A328,
    shake_stub: shake_stub_generals,
    height_easing_rates: [0x0074_DD42, 0x0074_DD72],
    drawable_pass_check: 0x0074_DDD5,
    drawable_pass_skip: 0x0074_DDFC,
    doing_rotate: 0xE8,
    doing_pitch: 0x108,
    doing_zoom: 0x124,
    scripted_lock: 0x125,
    doing_waypoint_path: 0x518,
    scroll_amount: 0x548,
    fields: &[0xC, 0x10, 0x14, 0x28, 0x2C, 0x40, 0x98, 0x9C, 0x554, 0x128],
    angles: &[3],
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

/// Where the naked stubs continue, set at install.
static FOLLOW_CONTINUE: AtomicUsize = AtomicUsize::new(0);
static FOLLOW_SKIP: AtomicUsize = AtomicUsize::new(0);
static SHAKE_CONTINUE: AtomicUsize = AtomicUsize::new(0);
static SHAKE_SKIP: AtomicUsize = AtomicUsize::new(0);
static SHAKE_THRESHOLD: AtomicUsize = AtomicUsize::new(0);

const DRAWABLE_PASS_CHECK_LEN: usize = 14;
const GD_CAMERA_ADJUST_SPEED: usize = 0x7E4;
const MAX_FIELDS: usize = 13;

type Params = [f32; MAX_FIELDS];

struct State {
    view: usize,
    before: Option<Params>,
    delta: Params,
    tick: u64,
    saved: Option<Params>,
}

static STATE: Mutex<Option<State>> = Mutex::new(None);
static SCROLL_SCALING: AtomicBool = AtomicBool::new(false);
/// Camera adjust speed for this frame, as f32 bits. The zoom easing reads it directly.
static HEIGHT_EASING_RATE: AtomicU32 = AtomicU32::new(0);
static BLENDS: AtomicU32 = AtomicU32::new(0);

/// Counts since the last call, for the log.
pub fn stats() -> String {
    format!("camera blends {}", BLENDS.swap(0, Ordering::Relaxed))
}

pub unsafe fn install() {
    let a = addresses();
    FOLLOW_CONTINUE.store(a.follow_continue, Ordering::SeqCst);
    FOLLOW_SKIP.store(a.follow_skip, Ordering::SeqCst);
    SHAKE_CONTINUE.store(a.shake_continue, Ordering::SeqCst);
    SHAKE_SKIP.store(a.shake_skip, Ordering::SeqCst);
    SHAKE_THRESHOLD.store(a.shake_threshold, Ordering::SeqCst);
    unsafe {
        game::patch_jump(a.follow_block, a.follow_stub as *const () as usize);
        game::patch_call(a.scripted_moves_call, scripted_moves as *const () as usize);
        game::patch_jump(a.shake_block, a.shake_stub as *const () as usize);
        let rate = HEIGHT_EASING_RATE.as_ptr() as u32;
        for at in a.height_easing_rates {
            let [b0, b1, b2, b3] = rate.to_le_bytes();
            game::write_code(at, &[0xD8, 0x0D, b0, b1, b2, b3]);
        }
        let flag = frame::UNFROZEN_DRAW.as_ptr() as u32;
        let [b0, b1, b2, b3] = flag.to_le_bytes();
        let skip = (a.drawable_pass_skip - (a.drawable_pass_check + 9)) as u8;
        let mut code = [0x90u8; DRAWABLE_PASS_CHECK_LEN];
        code[..9].copy_from_slice(&[0x80, 0x3D, b0, b1, b2, b3, 0x00, 0x74, skip]);
        game::write_code(a.drawable_pass_check, &code);
        game::write_u32(a.scroll_by_entry, scroll_by as *const () as u32);
    }
}

/// TheTacticalView, once it exists.
pub fn view() -> Option<usize> {
    let a = addresses();
    unsafe {
        let view = *(a.the_tactical_view as *const usize);
        (view != 0 && *(view as *const usize) == a.w3d_view_vtable).then_some(view)
    }
}

fn read(view: usize) -> Params {
    let mut p = [0.0; MAX_FIELDS];
    for (v, o) in p.iter_mut().zip(addresses().fields) {
        *v = unsafe { *((view + o) as *const f32) };
    }
    p
}

fn write(view: usize, p: &Params) {
    for (o, v) in addresses().fields.iter().zip(p) {
        unsafe { *((view + o) as *mut f32) = *v };
    }
}

fn set_camera_transform(view: usize) {
    let f: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().set_camera_transform) };
    f(view);
}

fn wrap_angle(a: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    a - ((a + PI) / TAU).floor() * TAU
}

/// Scroll input moves the camera by a fixed amount per frame tick message, which now comes every
/// frame. On while the message translators run.
pub fn set_scroll_scaling(on: bool) {
    SCROLL_SCALING.store(on, Ordering::Relaxed);
}

extern "thiscall" fn scroll_by(view: usize, delta: *const [f32; 2]) {
    let a = addresses();
    let original: extern "thiscall" fn(usize, *const [f32; 2]) = unsafe { std::mem::transmute(a.scroll_by) };
    if delta.is_null() || !SCROLL_SCALING.load(Ordering::Relaxed) {
        original(view, delta);
        return;
    }
    let d = unsafe { *delta };
    let s = frame::frame_scale();
    original(view, &[d[0] * s, d[1] * s]);
    if d != [0.0, 0.0] {
        // Other code compares the scroll amount with a cutoff, so it keeps the per-tick size.
        unsafe { *((view + a.scroll_amount) as *mut [f32; 2]) = d };
    }
}

/// Called right before W3DView::update.
pub fn before_view_update() {
    let gd = game::build().map_or(0, |b| unsafe { *(b.global_data as *const usize) });
    if gd != 0 {
        let k = unsafe { *((gd + GD_CAMERA_ADJUST_SPEED) as *const f32) }.clamp(0.0, 1.0);
        let rate = 1.0 - (1.0 - k).powf(frame::frame_scale());
        HEIGHT_EASING_RATE.store(rate.to_bits(), Ordering::Relaxed);
    }
    if !frame::is_tick() {
        return;
    }
    let Some(view) = view() else { return };
    let mut state = STATE.lock().unwrap();
    let s = state.get_or_insert(State { view, before: None, delta: [0.0; MAX_FIELDS], tick: 0, saved: None });
    if s.view != view {
        *s = State { view, before: None, delta: [0.0; MAX_FIELDS], tick: 0, saved: None };
    }
    s.before = Some(read(view));
}

/// Called right after W3DView::update.
pub fn after_view_update() {
    let mut state = STATE.lock().unwrap();
    let Some(s) = state.as_mut() else { return };
    let Some(before) = s.before.take() else { return };
    if view() != Some(s.view) {
        return;
    }
    let a = addresses();
    let after = read(s.view);
    let mut delta = [0.0; MAX_FIELDS];
    for i in 0..a.fields.len() {
        delta[i] = after[i] - before[i];
        if a.angles.contains(&i) {
            delta[i] = wrap_angle(delta[i]);
        }
    }
    s.delta = delta;
    s.tick = frame::tick_count();
}

/// Called right before the scene renders: the view goes back by the part of the last tick's
/// movement that hasn't happened yet.
pub fn apply(t: f32) {
    let Some(view) = view() else { return };
    let mut state = STATE.lock().unwrap();
    let Some(s) = state.as_mut().filter(|s| s.view == view) else { return };
    if t >= 1.0 || s.saved.is_some() || s.tick != frame::tick_count() || s.delta.iter().all(|d| *d == 0.0) {
        return;
    }
    let a = addresses();
    let lock: usize = unsafe {
        let f: extern "thiscall" fn(usize) -> usize =
            std::mem::transmute(*(*(view as *const *const usize)).add(a.camera_lock_drawable_vslot));
        f(view)
    };
    if lock != 0 {
        return;
    }
    let live = read(view);
    let mut p = live;
    for i in 0..a.fields.len() {
        p[i] -= s.delta[i] * (1.0 - t);
    }
    s.saved = Some(live);
    write(view, &p);
    set_camera_transform(view);
    BLENDS.fetch_add(1, Ordering::Relaxed);
}

/// Puts back the real view.
pub fn restore() {
    let mut state = STATE.lock().unwrap();
    let Some(s) = state.as_mut() else { return };
    let Some(saved) = s.saved.take() else { return };
    if view() == Some(s.view) {
        write(s.view, &saved);
        set_camera_transform(s.view);
    }
}

/// Scripted moves step once per tick. Between ticks this reports whether one is running, which
/// is what the rest of W3DView::update uses the result for.
extern "thiscall" fn scripted_moves(view: usize) -> u8 {
    let a = addresses();
    if frame::is_tick() {
        let original: extern "thiscall" fn(usize) -> u8 = unsafe { std::mem::transmute(a.update_camera_movements) };
        return original(view);
    }
    let flag = |o: usize| unsafe { *((view + o) as *const u8) } != 0;
    (flag(a.doing_zoom) || flag(a.doing_pitch) || flag(a.doing_rotate) || flag(a.doing_waypoint_path) || flag(a.scripted_lock))
        as u8
}

/// Replaces the first 14 bytes of the unit follow block and skips the block between ticks.
#[unsafe(naked)]
unsafe extern "C" fn follow_stub_zero_hour() {
    naked_asm!(
        "mov edx, dword ptr [esi - 0x7c]",
        "lea edi, [esi - 0x7c]",
        "push ebx",
        "push ebp",
        "mov ecx, edi",
        "mov dword ptr [esp + 0x44], edi",
        "cmp byte ptr [{tick}], 0",
        "je 2f",
        "push dword ptr [{cont}]",
        "ret",
        "2:",
        "push dword ptr [{skip}]",
        "ret",
        tick = sym frame::TICK,
        cont = sym FOLLOW_CONTINUE,
        skip = sym FOLLOW_SKIP,
    )
}

/// Generals keeps the view in EBX here.
#[unsafe(naked)]
unsafe extern "C" fn follow_stub_generals() {
    naked_asm!(
        "mov edx, dword ptr [esi - 0x7c]",
        "push ebx",
        "lea ebx, [esi - 0x7c]",
        "push ebp",
        "mov ecx, ebx",
        "mov dword ptr [esp + 0x48], ebx",
        "cmp byte ptr [{tick}], 0",
        "je 2f",
        "push dword ptr [{cont}]",
        "ret",
        "2:",
        "push dword ptr [{skip}]",
        "ret",
        tick = sym frame::TICK,
        cont = sym FOLLOW_CONTINUE,
        skip = sym FOLLOW_SKIP,
    )
}

/// The shake decays and flips sign once per call, so it only runs on ticks. Both of its branches
/// end with EBX cleared (EDI in Generals).
macro_rules! shake_stub {
    ($name:ident, $clear:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "cmp byte ptr [{tick}], 0",
                "je 2f",
                "fld dword ptr [esi + 0x2c]",
                "mov eax, dword ptr [{threshold}]",
                "fcomp dword ptr [eax]",
                "push dword ptr [{cont}]",
                "ret",
                "2:",
                $clear,
                "push dword ptr [{skip}]",
                "ret",
                tick = sym frame::TICK,
                threshold = sym SHAKE_THRESHOLD,
                cont = sym SHAKE_CONTINUE,
                skip = sym SHAKE_SKIP,
            )
        }
    };
}

shake_stub!(shake_stub_zero_hour, "xor ebx, ebx");
shake_stub!(shake_stub_generals, "xor edi, edi");
