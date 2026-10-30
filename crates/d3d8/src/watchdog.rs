//! A background thread that watches the main thread. It logs where the main thread is when a frame
//! stalls, and with `debug.profile` it samples it about 1000 times a second and logs where the
//! time goes every 5 seconds.

use std::collections::HashMap;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};

use windows_sys::Win32::System::Diagnostics::Debug::{CONTEXT, GetThreadContext};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentThreadId, OpenThread, ResumeThread, SuspendThread, THREAD_GET_CONTEXT, THREAD_SUSPEND_RESUME,
};

use crate::{config, game, log};

const STALL: Duration = Duration::from_millis(500);
const CONTEXT_CONTROL_X86: u32 = 0x0001_0001;
const PROFILE_REPORT: Duration = Duration::from_secs(5);
const PROFILE_LINES: usize = 80;

struct MainThread {
    handle: usize,
    stack_top: usize,
}

static MAIN: OnceLock<MainThread> = OnceLock::new();
static EPOCH: OnceLock<Instant> = OnceLock::new();
/// Milliseconds since `EPOCH` at the last present.
static LAST_PRESENT: AtomicU64 = AtomicU64::new(0);

fn now_ms() -> u64 {
    EPOCH.get_or_init(Instant::now).elapsed().as_millis() as u64
}

/// Called on the main thread at every present. Starts the watchdog on the first one.
pub fn on_present() {
    if game::build().is_none() {
        return;
    }
    LAST_PRESENT.store(now_ms(), Ordering::Relaxed);
    MAIN.get_or_init(|| {
        let top: usize;
        unsafe { std::arch::asm!("mov {}, fs:[4]", out(reg) top, options(nostack, readonly, preserves_flags)) };
        let handle = unsafe { OpenThread(THREAD_SUSPEND_RESUME | THREAD_GET_CONTEXT, 0, GetCurrentThreadId()) } as usize;
        if handle != 0 {
            std::thread::spawn(watch);
        }
        MainThread { handle, stack_top: top }
    });
}

struct Sample {
    eip: usize,
    callers: [usize; 12],
    n: usize,
    /// Return addresses into our own code above the nearest game frame, nearest first.
    ours: [usize; 8],
    ours_n: usize,
}

/// Start and end of our DLL's image.
fn our_range() -> (usize, usize) {
    static RANGE: OnceLock<(usize, usize)> = OnceLock::new();
    *RANGE.get_or_init(|| {
        let mut module = std::ptr::null_mut();
        let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
        unsafe { GetModuleHandleExW(flags, our_range as *const () as *const u16, &mut module) };
        let base = module as usize;
        (base, base + game::image_size(base))
    })
}

/// Our return addresses from `esp` up to the first one into the game.
unsafe fn our_callers(esp: usize, top: usize, out: &mut [usize]) -> usize {
    let (game_start, game_end) = game::code_range();
    let (start, end) = our_range();
    let mut n = 0;
    let mut at = esp;
    while n < out.len() && at + 4 <= top {
        let v = unsafe { *(at as *const usize) };
        if v >= game_start + 8 && v < game_end && game::call_before(v).is_some() {
            break;
        }
        if v >= start + 0x1000 && v < end && game::call_before(v).is_some() {
            out[n] = v;
            n += 1;
        }
        at += 4;
    }
    n
}

fn capture() -> Option<Sample> {
    let main = MAIN.get()?;
    let handle = main.handle as _;
    if unsafe { SuspendThread(handle) } == u32::MAX {
        return None;
    }
    let mut context: CONTEXT = unsafe { std::mem::zeroed() };
    context.ContextFlags = CONTEXT_CONTROL_X86;
    let ok = unsafe { GetThreadContext(handle, &mut context) } != 0;
    let mut s = Sample { eip: context.Eip as usize, callers: [0; 12], n: 0, ours: [0; 8], ours_n: 0 };
    if ok {
        let esp = context.Esp as usize;
        s.n = unsafe { game::stack_callers_between(esp, main.stack_top, &mut s.callers) };
        s.ours_n = unsafe { our_callers(esp, main.stack_top, &mut s.ours) };
    }
    unsafe { ResumeThread(handle) };
    ok.then_some(s)
}

fn watch() {
    let profiling = config::get().is_ok_and(|c| c.debug.profile);
    let mut profile = Profile::new();
    if profiling {
        log!("profiling the main thread");
    }
    let mut sampled_at = 0;
    loop {
        std::thread::sleep(Duration::from_millis(if profiling { 1 } else { 100 }));
        if profiling {
            if let Some(s) = capture() {
                profile.add(&s);
            }
            if profile.since.elapsed() >= PROFILE_REPORT {
                profile.report();
                profile = Profile::new();
            }
        }
        let stalled = now_ms().saturating_sub(LAST_PRESENT.load(Ordering::Relaxed));
        if stalled >= STALL.as_millis() as u64 && stalled >= sampled_at + STALL.as_millis() as u64 {
            sampled_at = stalled;
            if let Some(s) = capture() {
                let chain: Vec<String> = s.callers[..s.n].iter().map(|a| format!("{a:#010x}")).collect();
                log!("main thread stalled {stalled} ms, at {} called from {}", module_of(s.eip), chain.join(" < "));
            }
        } else if stalled < sampled_at {
            sampled_at = 0;
        }
    }
}

/// The module holding `address` and the offset into it.
fn module_and_base(address: usize) -> Option<(String, usize)> {
    let mut module = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, address as *const u16, &mut module) } == 0 {
        return None;
    }
    let mut name = [0u16; 260];
    let len = unsafe { GetModuleFileNameW(module, name.as_mut_ptr(), name.len() as u32) } as usize;
    let path = String::from_utf16_lossy(&name[..len]);
    Some((path.rsplit('\\').next().unwrap_or_default().to_string(), module as usize))
}

fn module_of(address: usize) -> String {
    match module_and_base(address) {
        Some((file, base)) => format!("{file}+{:#x}", address - base),
        None => "?".into(),
    }
}

/// Sample counts since the last report.
struct Profile {
    since: Instant,
    samples: u32,
    /// Where the main thread was: game code by 16-byte block, anything else by module.
    at: HashMap<String, u32>,
    /// Game return addresses on the stack, counted once per sample: time spent in or under each call.
    under: HashMap<usize, u32>,
    /// Our functions on the stack above the nearest game frame, counted once per sample.
    ours_under: HashMap<String, u32>,
    modules: HashMap<usize, String>,
}

/// Offset into our DLL, looked up in the linker's map file (`dinput8.map` next to the build).
fn ours(address: usize) -> String {
    format!("ours+{:#x}", address - our_range().0)
}

impl Profile {
    fn new() -> Profile {
        Profile {
            since: Instant::now(),
            samples: 0,
            at: HashMap::new(),
            under: HashMap::new(),
            ours_under: HashMap::new(),
            modules: HashMap::new(),
        }
    }

    fn module_name(&mut self, address: usize) -> String {
        let Some((file, base)) = module_and_base(address) else { return "?".into() };
        self.modules.entry(base).or_insert(file).clone()
    }

    fn add(&mut self, s: &Sample) {
        self.samples += 1;
        let (start, end) = game::code_range();
        let (our_start, our_end) = our_range();
        let in_ours = (our_start..our_end).contains(&s.eip);
        let mut counted = [0usize; 8];
        for (i, &a) in s.ours[..s.ours_n].iter().enumerate() {
            if !counted[..i].contains(&a) {
                *self.ours_under.entry(ours(a)).or_default() += 1;
            }
            counted[i] = a;
        }
        let at = if (start..end).contains(&s.eip) {
            format!("game {:#010x}", s.eip & !0xF)
        } else if in_ours {
            ours(s.eip & !0xF)
        } else {
            let module = self.module_name(s.eip);
            match s.callers[..s.n].first() {
                Some(c) => format!("{module} from {c:#010x}"),
                None => module,
            }
        };
        *self.at.entry(at).or_default() += 1;
        let mut seen = [0usize; 12];
        for (i, &c) in s.callers[..s.n].iter().enumerate() {
            if !seen[..i].contains(&c) {
                *self.under.entry(c).or_default() += 1;
            }
            seen[i] = c;
        }
    }

    fn report(&self) {
        let total = self.samples.max(1) as f64;
        let top = |map: Vec<(String, u32)>| {
            let mut list = map;
            list.sort_by(|a, b| b.1.cmp(&a.1));
            list.into_iter()
                .take(PROFILE_LINES)
                .filter(|(_, v)| *v * 1000 >= self.samples)
                .map(|(k, v)| format!("{:.1}% {k}", v as f64 * 100.0 / total))
                .collect::<Vec<_>>()
                .join(", ")
        };
        log!("profile, {} samples, where the main thread was: {}", self.samples, top(self.at.iter().map(|(k, v)| (k.clone(), *v)).collect()));
        log!(
            "profile, time in or under each game call site: {}",
            top(self.under.iter().map(|(k, v)| (format!("{k:#010x}"), *v)).collect())
        );
        log!("profile, time under each return address into our code: {}", top(self.ours_under.iter().map(|(k, v)| (k.clone(), *v)).collect()));
    }
}
