//! Per-second counters of what the backend did, written to the log every 5 seconds.

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use rustc_hash::FxHashMap;

use crate::log;

pub static DRAWS: AtomicU64 = AtomicU64::new(0);
/// Every draw since the start, never reset, for the render system timers.
pub static DRAWS_TOTAL: AtomicU64 = AtomicU64::new(0);
/// Draws skipped because their vertex buffer lies outside the view.
pub static CULLED_DRAWS: AtomicU64 = AtomicU64::new(0);
pub static PASSES: AtomicU64 = AtomicU64::new(0);
pub static BUFFER_LOCKS: AtomicU64 = AtomicU64::new(0);
pub static BUFFER_RENAMES: AtomicU64 = AtomicU64::new(0);
pub static BUFFER_VERSIONS: AtomicU64 = AtomicU64::new(0);
pub static BUFFER_BYTES: AtomicU64 = AtomicU64::new(0);
/// Bytes of dynamic buffers locked for the game to write straight into GPU memory.
pub static BUFFER_DIRECT_BYTES: AtomicU64 = AtomicU64::new(0);
/// Dynamic buffers that stopped taking direct writes, each reading the GPU's copy once.
pub static BUFFER_SLOW_COPIES: AtomicU64 = AtomicU64::new(0);
pub static TEXTURE_UPLOADS: AtomicU64 = AtomicU64::new(0);
pub static TEXTURE_BYTES: AtomicU64 = AtomicU64::new(0);
/// Uploads of textures a draw already used, so changed by the game after drawing with them.
pub static TEXTURE_REUPLOADS: AtomicU64 = AtomicU64::new(0);
pub static TEXTURE_CREATES: AtomicU64 = AtomicU64::new(0);
/// Textures that got a released texture's image instead of a new one.
pub static TEXTURE_RECYCLED: AtomicU64 = AtomicU64::new(0);
/// Time spent creating textures, in timestamp counter ticks.
pub static CREATE_TICKS: AtomicU64 = AtomicU64::new(0);
pub static READBACKS: AtomicU64 = AtomicU64::new(0);
pub static PIPELINES: AtomicU64 = AtomicU64::new(0);
pub static PIPELINE_US: AtomicU64 = AtomicU64::new(0);
pub static UNIFORM_BYTES: AtomicU64 = AtomicU64::new(0);
/// Vertex and pixel uniform blocks built because their state changed, whether or not they matched the last one.
pub static VS_BLOCKS: AtomicU64 = AtomicU64::new(0);
pub static PS_BLOCKS: AtomicU64 = AtomicU64::new(0);
/// Microseconds spent turning recorded commands into command buffers, mostly on the presenter thread.
pub static RECORD_US: AtomicU64 = AtomicU64::new(0);
pub static DESCRIPTOR_WRITES: AtomicU64 = AtomicU64::new(0);
/// Microseconds the CPU spent waiting for the GPU or for a swapchain image.
pub static WAIT_US: AtomicU64 = AtomicU64::new(0);
/// Microseconds spent inside Present, waits included.
pub static PRESENT_US: AtomicU64 = AtomicU64::new(0);
/// Microseconds the renderer thread spent running the game's calls.
pub static RENDERER_BUSY_US: AtomicU64 = AtomicU64::new(0);
/// Calls the game thread waited for the renderer thread on, and how long in all.
pub static SYNC_CALLS: AtomicU64 = AtomicU64::new(0);
pub static SYNC_WAIT_US: AtomicU64 = AtomicU64::new(0);
/// Microseconds the game thread waited at Present for the renderer thread to finish the frame before.
pub static PRESENT_WAIT_US: AtomicU64 = AtomicU64::new(0);
/// Microseconds after Present in the frame cap.
pub static PACING_US: AtomicU64 = AtomicU64::new(0);
/// Ticks the CPU spent preparing and recording draws, texture uploads included.
pub static DRAW_TICKS: AtomicU64 = AtomicU64::new(0);
/// Ticks spent converting and recording texture uploads.
pub static UPLOAD_TICKS: AtomicU64 = AtomicU64::new(0);
/// Ticks spent in vertex and index buffer locks and unlocks.
pub static LOCK_TICKS: AtomicU64 = AtomicU64::new(0);
/// Ticks spent in the 3D world's extra passes (copies, occlusion, resolve).
pub static SCENE_TICKS: AtomicU64 = AtomicU64::new(0);

/// GPU nanoseconds per section of the frame, from timestamps (`[debug] timers`).
static GPU_NS: [AtomicU64; super::render::SECTION_NAMES.len()] = [const { AtomicU64::new(0) }; super::render::SECTION_NAMES.len()];
static GPU_TIMED: AtomicU64 = AtomicU64::new(0);

/// The parts of preparing a draw, timed with `[debug] timers`.
pub const PREP_PARTS: [&str; 8] =
    ["culling", "shaders and textures", "target and pass", "pipeline", "dynamic state", "uniforms", "descriptor set and push constants", "vertex buffers"];
static PREP_TICKS: [AtomicU64; PREP_PARTS.len()] = [const { AtomicU64::new(0) }; PREP_PARTS.len()];
static DETAIL: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn set_detail(on: bool) {
    DETAIL.store(on, Ordering::Relaxed);
}

/// Splits a stretch of code into timed parts: each `part` call adds the time since the last one.
pub struct Laps(Option<u64>);

impl Laps {
    pub fn start() -> Laps {
        Laps(DETAIL.load(Ordering::Relaxed).then(ticks))
    }

    pub fn part(&mut self, index: usize) {
        if let Some(last) = self.0.as_mut() {
            let now = ticks();
            PREP_TICKS[index].fetch_add(now.wrapping_sub(*last), Ordering::Relaxed);
            *last = now;
        }
    }
}

fn report_prep(ms: impl Fn(&AtomicU64) -> f64) {
    if !DETAIL.load(Ordering::Relaxed) {
        return;
    }
    let parts: Vec<String> = PREP_PARTS.iter().zip(&PREP_TICKS).map(|(name, t)| format!("{name} {:.2}", ms(t))).collect();
    log!("our draw preparation per frame in ms: {}", parts.join(", "));
}

pub fn add(counter: &AtomicU64, n: u64) {
    counter.fetch_add(n, Ordering::Relaxed);
}

pub fn add_gpu(section: usize, ns: u64) {
    if let Some(counter) = GPU_NS.get(section) {
        counter.fetch_add(ns, Ordering::Relaxed);
        GPU_TIMED.store(1, Ordering::Relaxed);
    }
}

fn report_gpu(frames: u64) {
    if GPU_TIMED.swap(0, Ordering::Relaxed) == 0 {
        return;
    }
    let mut total = 0.0;
    let parts: Vec<String> = super::render::SECTION_NAMES
        .iter()
        .zip(&GPU_NS)
        .map(|(name, ns)| {
            let ms = ns.swap(0, Ordering::Relaxed) as f64 / 1e6 / frames as f64;
            total += ms;
            format!("{name} {ms:.2}")
        })
        .collect();
    log!("gpu per frame in ms (without the present pass): {}, {total:.2} in all", parts.join(", "));
}

/// The CPU's timestamp counter. Instant costs a QueryPerformanceCounter call and 64-bit
/// divisions on x86, too much for timers that run on every draw.
fn ticks() -> u64 {
    unsafe { std::arch::x86::_rdtsc() }
}

/// Adds the time until it's dropped to a counter of timestamp counter ticks.
pub struct Timer(&'static AtomicU64, u64);

impl Timer {
    pub fn new(counter: &'static AtomicU64) -> Timer {
        Timer(counter, ticks())
    }
}

impl Drop for Timer {
    fn drop(&mut self) {
        add(self.0, ticks().wrapping_sub(self.1));
    }
}

const CHAIN: usize = 6;

/// The game's call chains that created textures since the last report, each with a count and the
/// size and format of its last texture.
static CREATORS: Mutex<Option<FxHashMap<[usize; CHAIN], (u64, u32, u32, u32)>>> = Mutex::new(None);

pub fn note_texture_creator(width: u32, height: u32, format: u32) {
    let mut chain = [0; CHAIN];
    crate::game::stack_callers(&mut chain);
    let mut creators = CREATORS.lock().unwrap();
    let entry = creators.get_or_insert_with(FxHashMap::default).entry(chain).or_insert((0, 0, 0, 0));
    *entry = (entry.0 + 1, width, height, format);
}

/// Logs where the game created most of its textures when it created many.
fn report_creators(total: u64) {
    let Some(creators) = CREATORS.lock().unwrap().take() else { return };
    if total < 100 {
        return;
    }
    let mut list: Vec<_> = creators.into_iter().collect();
    list.sort_by(|a, b| b.1.0.cmp(&a.1.0));
    for (chain, (count, w, h, format)) in list.into_iter().take(4) {
        let calls: Vec<String> = chain.iter().filter(|&&a| a != 0).map(|a| format!("{a:#010x}")).collect();
        log!("{count} textures created from {} (last {w}x{h} {})", calls.join(" < "), crate::d3d::format_name(format));
    }
}

/// When the current report period started, its frame count, and the timestamp counter then.
static SINCE: Mutex<Option<(Instant, u64, u64)>> = Mutex::new(None);

/// Called once per presented frame.
pub fn frame() {
    let now = Instant::now();
    let mut since = SINCE.lock().unwrap();
    let (start, frames, start_ticks) = since.get_or_insert((now, 0, ticks()));
    *frames += 1;
    let elapsed = now - *start;
    if elapsed < Duration::from_secs(5) {
        return;
    }
    let f = (*frames).max(1);
    let take = |c: &AtomicU64| c.swap(0, Ordering::Relaxed);
    let elapsed_ms = elapsed.as_secs_f64() * 1000.0;
    let ticks_per_ms = (ticks().wrapping_sub(*start_ticks) as f64 / elapsed_ms).max(1.0);
    let ms = |c: &AtomicU64| take(c) as f64 / ticks_per_ms / f as f64;
    let frame_ms = elapsed_ms / f as f64;
    let (draw, upload, lock, scene, create) =
        (ms(&DRAW_TICKS), ms(&UPLOAD_TICKS), ms(&LOCK_TICKS), ms(&SCENE_TICKS), ms(&CREATE_TICKS));
    let present = PRESENT_US.load(Ordering::Relaxed) as f64 / f as f64 / 1000.0;
    let pacing = take(&PACING_US) as f64 / f as f64 / 1000.0;
    let (creates, recycled) = (take(&TEXTURE_CREATES), take(&TEXTURE_RECYCLED));
    report_creators(creates);
    let per_frame = |c: &AtomicU64| take(c) as f64 / f as f64 / 1000.0;
    let (sync_calls, sync_wait, present_wait, busy) = (take(&SYNC_CALLS), per_frame(&SYNC_WAIT_US), per_frame(&PRESENT_WAIT_US), per_frame(&RENDERER_BUSY_US));
    log!(
        "game thread per frame: {frame_ms:.2} ms, of which {sync_wait:.2} waiting on {:.1} calls to the renderer thread, {present_wait:.2} waiting at Present for the frame before, {pacing:.2} frame cap, {:.2} the game and everything else",
        sync_calls as f64 / f as f64,
        (frame_ms - sync_wait - present_wait - pacing).max(0.0)
    );
    log!(
        "renderer thread per frame: busy {busy:.2} ms, of which {draw:.2} preparing draws ({upload:.2} of it texture uploads), {lock:.2} buffer writes, {create:.2} creating {} textures ({} on recycled images), {scene:.2} 3D world passes, {present:.2} handing the frame to the presenter",
        creates / f,
        recycled / f,
    );
    let names = super::worker::take_sync_names();
    if !names.is_empty() {
        let top: Vec<String> = names.iter().take(8).map(|(name, n)| format!("{name} {:.1}", *n as f64 / f as f64)).collect();
        log!("calls that waited for the renderer thread, per frame: {}", top.join(", "));
    }
    log!(
        "vulkan per frame: {} draws ({} more culled), {} passes, {} buffer locks, {} renames, {} new buffer copies, {} KB copied to buffers, {} KB locked for direct writes, {} buffers switched to a CPU copy, {} texture uploads ({} KB, {} of textures already drawn with), {} KB uniforms ({} vertex and {} pixel blocks built), {} descriptor sets written, {} readbacks, {} new pipelines ({} ms), {:.2} ms waiting, {:.2} ms in present, {:.2} ms recording command buffers off the game thread",
        take(&DRAWS) / f,
        take(&CULLED_DRAWS) / f,
        take(&PASSES) / f,
        take(&BUFFER_LOCKS) / f,
        take(&BUFFER_RENAMES) / f,
        take(&BUFFER_VERSIONS),
        take(&BUFFER_BYTES) / f / 1024,
        take(&BUFFER_DIRECT_BYTES) / f / 1024,
        take(&BUFFER_SLOW_COPIES),
        take(&TEXTURE_UPLOADS) / f,
        take(&TEXTURE_BYTES) / f / 1024,
        take(&TEXTURE_REUPLOADS) / f,
        take(&UNIFORM_BYTES) / f / 1024,
        take(&VS_BLOCKS) / f,
        take(&PS_BLOCKS) / f,
        take(&DESCRIPTOR_WRITES) / f,
        take(&READBACKS),
        take(&PIPELINES),
        take(&PIPELINE_US) / 1000,
        take(&WAIT_US) as f64 / f as f64 / 1000.0,
        take(&PRESENT_US) as f64 / f as f64 / 1000.0,
        take(&RECORD_US) as f64 / f as f64 / 1000.0,
    );
    report_prep(&ms);
    crate::render_timers::report(f, ticks_per_ms);
    report_gpu(f);
    *since = Some((now, 0, ticks()));
}
