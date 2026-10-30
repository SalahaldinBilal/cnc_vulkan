//! The renderer thread. The device and everything behind it (state, recording, buffer and texture
//! data) is only touched here, in the order the game made its calls. See knowledge-base/vulkan-backend.md.

use std::cell::{Cell, RefCell};
use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Condvar, Mutex, OnceLock};
use std::thread::Thread;
use std::time::Instant;

use rustc_hash::FxHashMap;

use super::stats;

/// Commands stored inline: a header word (the call, then the entry's length in words) followed by
/// the closure's bytes, 8-byte aligned.
struct Batch(Vec<u64>);

/// Words a batch holds before it goes to the renderer thread, so it starts while the game queues more.
const BATCH_WORDS: usize = 4096;

unsafe fn call<F: FnOnce()>(at: *mut u64) {
    let f = unsafe { std::ptr::read(at as *mut F) };
    f()
}

/// A closure followed by its length-prefixed bytes.
unsafe fn call_with_bytes<F: FnOnce(&[u8])>(at: *mut u64) {
    let words = size_of::<F>().div_ceil(8);
    unsafe {
        let f = std::ptr::read(at as *mut F);
        let len = *at.add(words) as usize;
        f(std::slice::from_raw_parts(at.add(words + 1) as *const u8, len))
    }
}

impl Batch {
    fn push<F: FnOnce() + Send>(&mut self, f: F) {
        const { assert!(align_of::<F>() <= 8) };
        let words = 1 + size_of::<F>().div_ceil(8);
        let v = &mut self.0;
        v.reserve(words);
        let call: unsafe fn(*mut u64) = call::<F>;
        v.push(call as usize as u64 | (words as u64) << 32);
        unsafe {
            let at = v.as_mut_ptr().add(v.len());
            std::ptr::write(at as *mut F, f);
            v.set_len(v.len() + words - 1);
        }
    }

    fn push_with_bytes<F: FnOnce(&[u8]) + Send>(&mut self, f: F, bytes: &[u8]) {
        const { assert!(align_of::<F>() <= 8) };
        let f_words = size_of::<F>().div_ceil(8);
        let words = 1 + f_words + 1 + bytes.len().div_ceil(8);
        let v = &mut self.0;
        v.reserve(words);
        let call: unsafe fn(*mut u64) = call_with_bytes::<F>;
        v.push(call as usize as u64 | (words as u64) << 32);
        unsafe {
            let at = v.as_mut_ptr().add(v.len());
            std::ptr::write(at as *mut F, f);
            *at.add(f_words) = bytes.len() as u64;
            std::ptr::copy_nonoverlapping(bytes.as_ptr(), at.add(f_words + 1) as *mut u8, bytes.len());
            v.set_len(v.len() + words - 1);
        }
    }

    fn run(&mut self) {
        let v = &mut self.0;
        let mut i = 0;
        while i < v.len() {
            let header = v[i];
            let call: unsafe fn(*mut u64) = unsafe { std::mem::transmute(header as u32 as usize) };
            unsafe { call(v.as_mut_ptr().add(i + 1)) };
            i += (header >> 32) as usize;
        }
        v.clear();
    }
}

enum Job {
    Batch(Batch),
    Sync { f: *mut (dyn FnMut() + 'static), done: *const AtomicBool, waiter: Thread },
}

unsafe impl Send for Job {}

struct Queue {
    jobs: Mutex<VecDeque<Job>>,
    ready: Condvar,
    spare: Mutex<Vec<Batch>>,
}

static QUEUE: OnceLock<Queue> = OnceLock::new();

thread_local! {
    static ON_WORKER: Cell<bool> = const { Cell::new(false) };
    static PENDING: RefCell<Batch> = const { RefCell::new(Batch(Vec::new())) };
}

fn queue() -> &'static Queue {
    QUEUE.get_or_init(|| {
        std::thread::Builder::new().name("cnc-vulkan renderer".into()).spawn(work).expect("renderer thread");
        Queue { jobs: Mutex::new(VecDeque::new()), ready: Condvar::new(), spare: Mutex::new(Vec::new()) }
    })
}

fn work() {
    ON_WORKER.set(true);
    let q = queue();
    loop {
        let job = {
            let mut jobs = q.jobs.lock().unwrap();
            loop {
                if let Some(job) = jobs.pop_front() {
                    break job;
                }
                jobs = q.ready.wait(jobs).unwrap();
            }
        };
        let start = Instant::now();
        match job {
            Job::Batch(mut batch) => {
                batch.run();
                // One that grew with texture or buffer data at a load isn't kept for reuse.
                if batch.0.capacity() <= BATCH_WORDS * 4 {
                    q.spare.lock().unwrap().push(batch);
                }
            }
            Job::Sync { f, done, waiter } => {
                unsafe {
                    (*f)();
                    (*done).store(true, Ordering::Release);
                }
                waiter.unpark();
            }
        }
        stats::add(&stats::RENDERER_BUSY_US, start.elapsed().as_micros() as u64);
    }
}

fn send(job: Job) {
    let q = queue();
    q.jobs.lock().unwrap().push_back(job);
    q.ready.notify_one();
}

/// Hands what this thread queued so far to the renderer thread.
pub fn flush() {
    let full = PENDING.with_borrow_mut(|pending| {
        if pending.0.is_empty() {
            return None;
        }
        let fresh = queue().spare.lock().unwrap().pop().unwrap_or_else(|| Batch(Vec::with_capacity(BATCH_WORDS * 2)));
        Some(std::mem::replace(pending, fresh))
    });
    if let Some(batch) = full {
        send(Job::Batch(batch));
    }
}

pub fn on_worker() -> bool {
    ON_WORKER.get()
}

/// Runs `f` on the renderer thread after everything queued before it, without waiting. On the
/// renderer thread itself it just runs.
pub fn post(f: impl FnOnce() + Send + 'static) {
    if on_worker() {
        return f();
    }
    let full = PENDING.with_borrow_mut(|pending| {
        pending.push(f);
        pending.0.len() >= BATCH_WORDS
    });
    if full {
        flush();
    }
}

/// Like `post`, with a copy of `bytes` handed to `f`, stored in the batch itself.
pub fn post_bytes(bytes: &[u8], f: impl FnOnce(&[u8]) + Send + 'static) {
    if on_worker() {
        return f(bytes);
    }
    let full = PENDING.with_borrow_mut(|pending| {
        pending.push_with_bytes(f, bytes);
        pending.0.len() >= BATCH_WORDS
    });
    if full {
        flush();
    }
}

/// A pointer handed to the renderer thread, which is the only one that uses what it points to.
#[derive(Clone, Copy)]
pub struct Ptr<T>(pub *mut T);

unsafe impl<T> Send for Ptr<T> {}

/// Runs `f` on the renderer thread after everything queued before it, and waits for it. On the
/// renderer thread itself it just runs.
pub fn run<R>(f: impl FnOnce() -> R) -> R {
    if on_worker() {
        return f();
    }
    flush();
    let start = Instant::now();
    let mut f = Some(f);
    let mut result = None;
    let mut call = || result = Some((f.take().unwrap())());
    let call: &mut dyn FnMut() = &mut call;
    // The caller waits below until the job ran, so the borrow outlives it.
    let call: *mut (dyn FnMut() + 'static) = unsafe { std::mem::transmute(call) };
    let done = AtomicBool::new(false);
    send(Job::Sync { f: call, done: &done, waiter: std::thread::current() });
    while !done.load(Ordering::Acquire) {
        std::thread::park();
    }
    stats::add(&stats::SYNC_CALLS, 1);
    stats::add(&stats::SYNC_WAIT_US, start.elapsed().as_micros() as u64);
    result.unwrap()
}

/// Round trips by the method that made them, since the last report.
static SYNC_NAMES: Mutex<Option<FxHashMap<&'static str, u64>>> = Mutex::new(None);

/// `run` for a D3D method, counted by name, so the log shows which calls still wait for the renderer thread.
pub fn run_as<R>(name: &'static str, f: impl FnOnce() -> R) -> R {
    if !on_worker() {
        *SYNC_NAMES.lock().unwrap().get_or_insert_with(FxHashMap::default).entry(name).or_insert(0) += 1;
    }
    run(f)
}

/// The methods that waited most often since the last call, with their counts.
pub fn take_sync_names() -> Vec<(&'static str, u64)> {
    let mut names: Vec<_> = SYNC_NAMES.lock().unwrap().take().unwrap_or_default().into_iter().collect();
    names.sort_by(|a, b| b.1.cmp(&a.1));
    names
}

/// Presents queued and finished, to keep the game thread at most one frame ahead.
static PRESENTS_QUEUED: AtomicU64 = AtomicU64::new(0);
static PRESENTS_DONE: Mutex<u64> = Mutex::new(0);
static PRESENT_DONE: Condvar = Condvar::new();

/// Queues a Present and waits until the one before it has finished on the renderer thread.
pub fn present(f: impl FnOnce() + Send + 'static) {
    let n = PRESENTS_QUEUED.fetch_add(1, Ordering::Relaxed) + 1;
    post(move || {
        f();
        *PRESENTS_DONE.lock().unwrap() = n;
        PRESENT_DONE.notify_all();
    });
    flush();
    let start = Instant::now();
    let mut done = PRESENTS_DONE.lock().unwrap();
    while *done + 1 < n {
        done = PRESENT_DONE.wait(done).unwrap();
    }
    stats::add(&stats::PRESENT_WAIT_US, start.elapsed().as_micros() as u64);
}
