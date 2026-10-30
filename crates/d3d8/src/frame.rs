//! Game logic steps 30 times per second. The client (input, UI, camera, rendering) runs every
//! frame. See knowledge-base/frame-loop.md.

use std::arch::naked_asm;
use std::ffi::c_void;
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::time::{Duration, Instant};

use crate::config::{self, DisplayMode};
use crate::game::{self, Build};
use crate::{
    bones, camera, client, gui, lines, log, objects, overlays, particles, things, transitions, treads, trees, water,
};

struct Addresses {
    /// `call GameEngine::update` inside Win32GameEngine::update, followed by its window message pump.
    engine_update_call: usize,
    engine_update: usize,
    /// In GameEngine::update: `mov ecx, [TheGameClient]; mov edx, [ecx]; call [edx+0x10]`.
    engine_client_update: usize,
    /// In GameEngine::update: `mov ecx, [TheMessageStream]; call propagateMessages`.
    engine_propagate: usize,
    /// The logic step, a tail jump: `mov ecx, [TheGameLogic]; mov edx, [ecx]; jmp [edx+0x10]`.
    logic_step: usize,
    propagate_messages: usize,
    the_message_stream: usize,
    /// In W3DDisplay::draw: `this->updateViews()`, followed in Zero Hour by
    /// `TheParticleSystemManager->update()`, which this replaces too.
    view_update: usize,
    view_update_len: usize,
    /// Generals updates particles in GameClient::update instead: `mov ecx, [manager]; mov edx, [ecx];
    /// call [edx+0x10]`, 11 bytes.
    particle_update_call: Option<usize>,
    /// In W3DDisplay::draw: the check that repeats the draw while time is frozen and a camera move runs.
    draw_loop_tail: usize,
    draw_exit: usize,
    display_draw_entry: usize,
    display_draw: usize,
    /// In W3DDisplay::draw: `test bl, bl; jnz; render clock += frame length; mov eax, render clock`,
    /// up to the `push eax` that hands the clock to WW3D::Sync. BL is set when time is frozen.
    clock_advance: usize,
    /// Milliseconds, advanced by each unfrozen draw and handed to WW3D::Sync, which drives animation.
    render_clock: usize,
    /// Milliseconds the render clock moves per game update, 33.
    frame_length: usize,
    the_game_client: usize,
    the_game_logic: usize,
    the_network: usize,
    the_tactical_view: usize,
    the_script_engine: usize,
    script_time_is_fast: usize,
    script_frozen_debug: usize,
    script_frozen: usize,
    /// GlobalData's replay fast mode flag. Generals has no fast replays.
    replay_fast: Option<usize>,
    game_mode: usize,
    view_time_frozen_vslot: usize,
    view_time_multiplier_vslot: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    engine_update_call: 0x0074_1C94,
    engine_update: 0x0040_FF30,
    engine_client_update: 0x0040_FF49,
    engine_propagate: 0x0040_FF54,
    logic_step: 0x0040_FFA5,
    propagate_messages: 0x0040_E0D0,
    the_message_stream: 0x00A3_7A94,
    view_update: 0x0073_E07F,
    view_update_len: 19,
    particle_update_call: None,
    draw_loop_tail: 0x0073_E4A5,
    draw_exit: 0x0073_E4C0,
    display_draw_entry: 0x0096_79CC,
    display_draw: 0x0073_DDE0,
    clock_advance: 0x0073_DF9F,
    render_clock: 0x00A4_04B8,
    frame_length: 0x00A1_C554,
    the_game_client: 0x00A3_9B18,
    the_game_logic: 0x00A3_9B0C,
    the_network: 0x00A3_984C,
    the_tactical_view: 0x00A3_9598,
    the_script_engine: 0x00A3_9580,
    script_time_is_fast: 0x0043_7550,
    script_frozen_debug: 0x0043_7500,
    script_frozen: 0x0043_74D0,
    replay_fast: Some(0x924),
    game_mode: 0x94,
    view_time_frozen_vslot: 0xC0 / 4,
    view_time_multiplier_vslot: 0xC4 / 4,
};

static GENERALS: Addresses = Addresses {
    engine_update_call: 0x0071_AD16,
    engine_update: 0x0040_C760,
    engine_client_update: 0x0040_C779,
    engine_propagate: 0x0040_C784,
    logic_step: 0x0040_C7D5,
    propagate_messages: 0x0042_3F00,
    the_message_stream: 0x00A6_A9A0,
    view_update: 0x0071_71AC,
    view_update_len: 8,
    particle_update_call: Some(0x004A_A406),
    draw_loop_tail: 0x0071_7589,
    draw_exit: 0x0071_75A0,
    display_draw_entry: 0x009A_565C,
    display_draw: 0x0071_6F10,
    clock_advance: 0x0071_70CC,
    render_clock: 0x00A7_25C0,
    frame_length: 0x00A5_0E50,
    the_game_client: 0x00A6_BE90,
    the_game_logic: 0x00A6_BE84,
    the_network: 0x00A6_BBF8,
    the_tactical_view: 0x00A6_B95C,
    the_script_engine: 0x00A6_B940,
    script_time_is_fast: 0x0043_7F00,
    script_frozen_debug: 0x0043_7EB0,
    script_frozen: 0x0043_7E80,
    replay_fast: None,
    game_mode: 0x98,
    view_time_frozen_vslot: 0xB4 / 4,
    view_time_multiplier_vslot: 0xB8 / 4,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

/// TheGameLogic's address, for the logic step stub.
static THE_GAME_LOGIC: AtomicUsize = AtomicUsize::new(0);

const ENGINE_CALL_LEN: usize = 11;
const CLIENT_UPDATE_VSLOT: usize = 0x10 / 4;
const DISPLAY_UPDATE_VIEWS_VSLOT: usize = 0x60 / 4;
const CLOCK_ADVANCE_LEN: usize = 27;
const GAME_MODE_REPLAY: i32 = 3;
const VIEW_CAMERA_MOVE_FINISHED_VSLOT: usize = 0x60 / 4;
/// Updates owed after a slow frame are made up on the following frames, up to this many.
const MAX_BACKLOG_STEPS: u32 = 3;
/// Longest frame that per-frame steps make up for, in game updates.
const MAX_FRAME_SCALE: f32 = 3.0;

/// Set for the whole frame on frames that run the 30 Hz tick. Read by the game patches.
pub static TICK: AtomicU8 = AtomicU8::new(0);
/// Set by each draw when the draw moved the render clock, which happens once per logic step.
pub static UNFROZEN_DRAW: AtomicU8 = AtomicU8::new(0);
static IN_ENGINE_UPDATE: AtomicBool = AtomicBool::new(false);
static LOGIC_STEPPED: AtomicBool = AtomicBool::new(false);
static LOGIC_STEPS: AtomicU64 = AtomicU64::new(0);
static TICKS: AtomicU64 = AtomicU64::new(0);
/// Real time of the current frame in game updates, as f32 bits.
static FRAME_SCALE: AtomicU32 = AtomicU32::new(0);

/// Counts logic steps, including the one running right now.
pub fn update_count() -> u64 {
    LOGIC_STEPS.load(Ordering::Relaxed)
}

/// Counts 30 Hz ticks, including the current one.
pub fn tick_count() -> u64 {
    TICKS.load(Ordering::Relaxed)
}

pub fn is_tick() -> bool {
    TICK.load(Ordering::Relaxed) != 0
}

/// Real time since the last frame, in game updates. Per-call steps that assumed 30 calls per
/// second are multiplied by this.
pub fn frame_scale() -> f32 {
    f32::from_bits(FRAME_SCALE.load(Ordering::Relaxed))
}

struct Clock {
    last: Instant,
    pending: Duration,
    step: Duration,
    fast: bool,
    last_frame: Instant,
    /// When the last logic step was due.
    logic_at: Option<Instant>,
}

static CLOCK: Mutex<Option<Clock>> = Mutex::new(None);

struct Stats {
    since: Instant,
    frames: u32,
    ticks: u32,
    logic: u32,
    last_present: Instant,
    frame_ms: Vec<f32>,
    /// Frame times of the frames that ran the 30 Hz tick, and of the others.
    tick_ms: Vec<f32>,
    other_ms: Vec<f32>,
}

impl Stats {
    fn new(now: Instant) -> Stats {
        Stats {
            since: now,
            frames: 0,
            ticks: 0,
            logic: 0,
            last_present: now,
            frame_ms: Vec::new(),
            tick_ms: Vec::new(),
            other_ms: Vec::new(),
        }
    }
}

/// Median and 99th percentile of frame times, sorting them.
fn spread(ms: &mut [f32]) -> (f32, f32) {
    ms.sort_by(f32::total_cmp);
    let at = |q: f64| ms.get(((ms.len().max(1) - 1) as f64 * q) as usize).copied().unwrap_or(0.0);
    (at(0.5), at(0.99))
}

static STATS: Mutex<Option<Stats>> = Mutex::new(None);

/// Time spent in each part of the frame since the last report, in microseconds.
mod part {
    pub const ENGINE: usize = 0;
    pub const CLIENT: usize = 1;
    pub const MESSAGES: usize = 2;
    pub const DRAW: usize = 3;
}
static PART_US: [AtomicU64; 4] = [const { AtomicU64::new(0) }; 4];

fn timed<R>(part: usize, f: impl FnOnce() -> R) -> R {
    let start = Instant::now();
    let r = f();
    PART_US[part].fetch_add(start.elapsed().as_micros() as u64, Ordering::Relaxed);
    r
}

/// Counts a presented frame and logs render, tick and logic rates every 5 seconds.
pub fn on_present() {
    crate::watchdog::on_present();
    let now = Instant::now();
    let mut stats = STATS.lock().unwrap();
    let s = stats.get_or_insert_with(|| Stats::new(now));
    s.frames += 1;
    let ms = (now - s.last_present).as_secs_f32() * 1000.0;
    s.frame_ms.push(ms);
    // The present belongs to the frame that's ending, which ran the tick if the flag is up.
    if is_tick() {
        s.tick_ms.push(ms);
    } else {
        s.other_ms.push(ms);
    }
    s.last_present = now;
    let elapsed = now - s.since;
    if elapsed >= Duration::from_secs(5) {
        let secs = elapsed.as_secs_f64();
        let frames = s.frames.max(1) as f64;
        let engine_ms = PART_US[part::ENGINE].load(Ordering::Relaxed) as f64 / 1000.0;
        let [engine, client, messages, draw] = PART_US.each_ref().map(|p| p.swap(0, Ordering::Relaxed) as f64 / 1000.0 / frames);
        let (median, p99) = spread(&mut s.frame_ms);
        let worst = s.frame_ms.last().copied().unwrap_or(0.0);
        let (tick_median, tick_p99) = spread(&mut s.tick_ms);
        let (other_median, other_p99) = spread(&mut s.other_ms);
        log!(
            "render {:.0} fps (frame ms: median {:.1}, 99% {:.1}, worst {:.1}; tick frames median {:.1}, 99% {:.1}; other frames median {:.1}, 99% {:.1}; per frame: engine update {:.2}, client update {:.2}, messages {:.2}, draw {:.2}; engine update per tick {:.2}), ticks {:.1} per second, logic steps {:.1} per second, {}, {}",
            s.frames as f64 / secs,
            median,
            p99,
            worst,
            tick_median,
            tick_p99,
            other_median,
            other_p99,
            engine,
            client,
            messages,
            draw,
            engine_ms / s.ticks.max(1) as f64,
            s.ticks as f64 / secs,
            s.logic as f64 / secs,
            camera::stats(),
            objects::stats()
        );
        *s = Stats::new(now);
    }
}

fn count(field: fn(&mut Stats) -> &mut u32) {
    if let Some(s) = STATS.lock().unwrap().as_mut() {
        *field(s) += 1;
    }
}

pub fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    let o = objects::addresses();
    THE_GAME_LOGIC.store(a.the_game_logic, Ordering::SeqCst);
    unsafe {
        game::patch_call(a.engine_update_call, update_engine as *const () as usize);
        game::replace_with_call(a.engine_client_update, ENGINE_CALL_LEN, engine_client_update as *const () as usize);
        game::replace_with_call(a.engine_propagate, ENGINE_CALL_LEN, engine_propagate as *const () as usize);
        game::patch_jump(a.logic_step, logic_step_stub as *const () as usize);
        game::replace_with_call(a.view_update, a.view_update_len, view_update_stub as *const () as usize);
        if let Some(at) = a.particle_update_call {
            game::replace_with_call(at, ENGINE_CALL_LEN, particles::update_manager as *const () as usize);
        }
        game::replace_with_call(a.clock_advance, CLOCK_ADVANCE_LEN, clock_stub as *const () as usize);
        game::patch_jump(a.draw_loop_tail, a.draw_exit);
        game::write_u32(a.display_draw_entry, draw_display as *const () as u32);
        game::write_u32(o.draw_drawable_push + 1, objects::draw_drawable as *const () as u32);
        game::patch_jump(o.set_transform, objects::set_transform_base as *const () as usize);
        game::patch_jump(o.set_position, objects::set_position_base as *const () as usize);
        camera::install();
        client::install();
        treads::install();
        trees::install();
        water::install();
        overlays::install();
        gui::install();
        lines::install();
        transitions::install();
    }
    log!("game logic runs on its own clock, the client runs every frame");
}

/// Both the main loop and W3DDisplay::draw wait on this cap. Needs GlobalData, so it runs at D3D load.
pub fn disable_engine_limiter(build: &Build) {
    let gd = unsafe { *(build.global_data as *const usize) };
    if gd == 0 {
        log!("GlobalData not created yet, engine frame cap left on");
        return;
    }
    unsafe { *((gd + build.use_fps_limit) as *mut u8) = 0 };
    log!("engine frame cap turned off, game updates {} per second", update_rate(build));
}

fn update_rate(build: &Build) -> u32 {
    let gd = unsafe { *(build.global_data as *const usize) };
    let fps = if gd == 0 { 0 } else { unsafe { *((gd + game::global_data::FRAMES_PER_SECOND_LIMIT) as *const i32) } };
    if fps > 0 { fps as u32 } else { 30 }
}

unsafe fn vcall<R>(obj: usize, slot: usize) -> R {
    unsafe {
        let f: extern "thiscall" fn(usize) -> R = std::mem::transmute(*(*(obj as *const *const usize)).add(slot));
        f(obj)
    }
}

fn script_check(f: usize) -> bool {
    let scripts = unsafe { *(addresses().the_script_engine as *const usize) };
    if scripts == 0 {
        return false;
    }
    let check: extern "thiscall" fn(usize) -> u8 = unsafe { std::mem::transmute(f) };
    check(scripts) != 0
}

/// The same conditions under which the original limiter stepped aside and updated every frame.
fn fast_mode(build: &Build) -> bool {
    let a = addresses();
    unsafe {
        let view = *(a.the_tactical_view as *const usize);
        if view != 0 && vcall::<i32>(view, a.view_time_multiplier_vslot) > 1 {
            return true;
        }
        if script_check(a.script_time_is_fast) {
            return true;
        }
        let Some(replay_fast) = a.replay_fast else { return false };
        let gd = *(build.global_data as *const usize);
        let logic = *(a.the_game_logic as *const usize);
        gd != 0
            && logic != 0
            && *((gd + replay_fast) as *const u8) != 0
            && *((logic + a.game_mode) as *const i32) == GAME_MODE_REPLAY
    }
}

/// The original draw looped, with logic stopped, while a scripted camera move played in frozen
/// time. The draw no longer loops, so the logic waits here instead.
fn camera_holds_time() -> bool {
    let a = addresses();
    unsafe {
        let view = *(a.the_tactical_view as *const usize);
        if view == 0 || *(a.the_network as *const usize) != 0 {
            return false;
        }
        if vcall::<u8>(view, VIEW_CAMERA_MOVE_FINISHED_VSLOT) != 0 {
            return false;
        }
        vcall::<u8>(view, a.view_time_frozen_vslot) != 0
            || script_check(a.script_frozen_debug)
            || script_check(a.script_frozen)
    }
}

/// Starts a frame: measures its length and decides whether the 30 Hz tick runs.
fn begin_frame() -> bool {
    let Some(build) = game::build() else { return true };
    let now = Instant::now();
    let step = Duration::from_secs(1) / update_rate(build);
    let fast = fast_mode(build);
    let mut clock = CLOCK.lock().unwrap();
    let c = clock.get_or_insert(Clock {
        last: now,
        pending: Duration::ZERO,
        step,
        fast,
        last_frame: now,
        logic_at: None,
    });
    let scale = ((now - c.last_frame).as_secs_f32() / step.as_secs_f32()).min(MAX_FRAME_SCALE);
    FRAME_SCALE.store(scale.to_bits(), Ordering::Relaxed);
    c.last_frame = now;
    c.step = step;
    if fast || c.fast {
        *c = Clock { last: now, pending: Duration::ZERO, step, fast, last_frame: now, logic_at: None };
        return true;
    }
    c.pending = (c.pending + (now - c.last)).min(step * MAX_BACKLOG_STEPS);
    c.last = now;
    if c.pending < step {
        return false;
    }
    c.pending -= step;
    true
}

/// Time between game updates.
pub fn update_interval() -> Duration {
    let rate = game::build().map_or(30, update_rate);
    Duration::from_secs(1) / rate
}

/// How far the current moment is from the last logic step toward the next one, 0 to 1.
pub fn blend_factor() -> f32 {
    let clock = CLOCK.lock().unwrap();
    match clock.as_ref() {
        Some(c) if !c.fast => {
            c.logic_at.map_or(1.0, |at| (at.elapsed().as_secs_f32() / c.step.as_secs_f32()).min(1.0))
        }
        _ => 1.0,
    }
}

/// Same, from the last 30 Hz tick. Differs from `blend_factor` while the logic is paused.
pub fn tick_factor() -> f32 {
    let clock = CLOCK.lock().unwrap();
    match clock.as_ref() {
        Some(c) if !c.fast => ((c.pending + c.last.elapsed()).as_secs_f32() / c.step.as_secs_f32()).min(1.0),
        _ => 1.0,
    }
}

/// The game resets GlobalData to its INI values between matches, which turns the frame cap back
/// on and clears the windowed flag. Both are set again every frame.
fn keep_global_overrides() {
    let Some(build) = game::build() else { return };
    let gd = unsafe { *(build.global_data as *const usize) };
    if gd == 0 {
        return;
    }
    unsafe { *((gd + build.use_fps_limit) as *mut u8) = 0 };
    if config::get().is_ok_and(|c| c.display.mode != DisplayMode::Fullscreen) {
        unsafe { *((gd + game::global_data::WINDOWED) as *mut u8) = 1 };
    }
}

/// Replaces the call to GameEngine::update. The tick runs radar, audio, network and the logic
/// step. The client update and message translation run every frame, after them.
extern "thiscall" fn update_engine(engine: *mut c_void) {
    keep_global_overrides();
    let tick = begin_frame();
    LOGIC_STEPPED.store(false, Ordering::Relaxed);
    if tick {
        TICK.store(1, Ordering::SeqCst);
        TICKS.fetch_add(1, Ordering::Relaxed);
        count(|s| &mut s.ticks);
        let update: extern "thiscall" fn(*mut c_void) = unsafe { std::mem::transmute(addresses().engine_update) };
        IN_ENGINE_UPDATE.store(true, Ordering::Relaxed);
        timed(part::ENGINE, || update(engine));
        IN_ENGINE_UPDATE.store(false, Ordering::Relaxed);
    }
    let stepped = LOGIC_STEPPED.load(Ordering::Relaxed);
    let objects_recording = objects::set_recording(stepped);
    let gui_recording = gui::set_recording(tick);
    timed(part::CLIENT, client_update);
    gui::set_recording(gui_recording);
    objects::set_recording(objects_recording);
    if stepped {
        things::record();
    }
    camera::set_scroll_scaling(true);
    timed(part::MESSAGES, propagate_messages);
    camera::set_scroll_scaling(false);
    TICK.store(0, Ordering::SeqCst);
}

fn client_update() {
    unsafe {
        let client = *(addresses().the_game_client as *const usize);
        if client != 0 {
            vcall::<()>(client, CLIENT_UPDATE_VSLOT);
        }
    }
}

fn propagate_messages() {
    let a = addresses();
    let stream = unsafe { *(a.the_message_stream as *const usize) };
    if stream != 0 {
        let propagate: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(a.propagate_messages) };
        propagate(stream);
    }
}

/// GameEngine::update's own client update. Skipped when we run the engine update, since we run
/// the client after it.
extern "C" fn engine_client_update() {
    if !IN_ENGINE_UPDATE.load(Ordering::Relaxed) {
        client_update();
    }
}

extern "C" fn engine_propagate() {
    if !IN_ENGINE_UPDATE.load(Ordering::Relaxed) {
        propagate_messages();
    }
}

/// Returns whether the logic step may run, and if so marks it as running.
extern "C" fn before_logic() -> u8 {
    if camera_holds_time() {
        return 0;
    }
    LOGIC_STEPS.fetch_add(1, Ordering::Relaxed);
    LOGIC_STEPPED.store(true, Ordering::Relaxed);
    if let Some(c) = CLOCK.lock().unwrap().as_mut() {
        c.logic_at = Some(Instant::now() - c.pending);
    }
    objects::set_recording(true);
    1
}

extern "C" fn after_logic() {
    objects::set_recording(false);
    count(|s| &mut s.logic);
}

/// Replaces the logic step tail jump, so it returns to GameEngine::update's caller.
#[unsafe(naked)]
unsafe extern "C" fn logic_step_stub() {
    naked_asm!(
        "call {before}",
        "test al, al",
        "jz 2f",
        "mov ecx, dword ptr [{logic}]",
        "mov ecx, dword ptr [ecx]",
        "mov edx, dword ptr [ecx]",
        "call dword ptr [edx + 0x10]",
        "call {after}",
        "2:",
        "ret",
        before = sym before_logic,
        after = sym after_logic,
        logic = sym THE_GAME_LOGIC,
    )
}

/// The render clock moves 33 ms on the draw after each logic step. WW3D::Sync gets it plus the
/// part of the next step that has passed, kept below a full step so the next step still moves it.
extern "C" fn clock_for_sync(frozen: u32) -> u32 {
    let a = addresses();
    unsafe {
        let clock = a.render_clock as *mut u32;
        let step = *(a.frame_length as *const u32);
        if frozen == 0 {
            *clock = (*clock).wrapping_add(step);
        }
        UNFROZEN_DRAW.store((frozen == 0) as u8, Ordering::Relaxed);
        let progress = if step == 0 { 0 } else { ((step as f32 * blend_factor()) as u32).min(step - 1) };
        (*clock).wrapping_add(progress)
    }
}

/// Replaces the clock advance in W3DDisplay::draw. Leaves the time for WW3D::Sync in EAX.
#[unsafe(naked)]
unsafe extern "C" fn clock_stub() {
    naked_asm!(
        "movzx eax, bl",
        "push eax",
        "call {clock}",
        "add esp, 4",
        "ret",
        clock = sym clock_for_sync,
    )
}

extern "C" fn update_views(display: usize) {
    camera::before_view_update();
    unsafe { vcall::<()>(display, DISPLAY_UPDATE_VIEWS_VSLOT) };
    if addresses().particle_update_call.is_none() {
        particles::update_manager();
    }
    camera::after_view_update();
    client::update_placement();
}

/// Runs `this->updateViews(); TheParticleSystemManager->update();` and blends this frame.
/// EBP is the display.
#[unsafe(naked)]
unsafe extern "C" fn view_update_stub() {
    naked_asm!(
        "call {after_render}",
        "push ebp",
        "call {update_views}",
        "add esp, 4",
        "call {before_render}",
        "ret",
        after_render = sym after_render,
        update_views = sym update_views,
        before_render = sym before_render,
    )
}

extern "C" fn before_render() {
    let t = blend_factor();
    let tick_t = tick_factor();
    overlays::update_snow();
    camera::apply(tick_t);
    objects::apply(t);
    bones::apply(t);
    // The draw runs inside the client update, before that update's own record.
    if LOGIC_STEPPED.load(Ordering::Relaxed) {
        things::record();
    }
    things::apply(t);
    particles::apply(t);
    treads::apply(t);
    water::apply(t);
    gui::apply(tick_t);
    lines::apply(t);
    overlays::set_progress(t, LOGIC_STEPPED.load(Ordering::Relaxed));
}

extern "C" fn after_render() {
    lines::restore();
    gui::restore();
    water::restore();
    treads::restore();
    particles::restore();
    things::restore();
    bones::restore();
    objects::restore();
    camera::restore();
}

extern "thiscall" fn draw_display(display: *mut c_void) {
    let draw: extern "thiscall" fn(*mut c_void) = unsafe { std::mem::transmute(addresses().display_draw) };
    let recording = objects::set_recording(false);
    let gui_recording = gui::set_recording(false);
    timed(part::DRAW, || draw(display));
    after_render();
    gui::set_recording(gui_recording);
    objects::set_recording(recording);
}
