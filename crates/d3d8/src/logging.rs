use std::fs::File;
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

use windows_sys::Win32::System::Threading::GetCurrentThreadId;

use crate::paths;

struct Logger {
    file: Mutex<File>,
    start: Instant,
}

static LOGGER: OnceLock<Option<Logger>> = OnceLock::new();

pub fn init() {
    LOGGER.get_or_init(|| {
        let dir = paths::get().data_dir.as_ref()?;
        let path = dir.join("cnc-vulkan.log");
        let _ = std::fs::rename(&path, dir.join("cnc-vulkan.previous.log"));
        let file = File::create(path).ok()?;
        Some(Logger { file: Mutex::new(file), start: Instant::now() })
    });
}

pub fn write(args: std::fmt::Arguments) {
    #[cfg(test)]
    eprintln!("{args}");
    let Some(Some(logger)) = LOGGER.get() else { return };
    let secs = logger.start.elapsed().as_secs_f64();
    let tid = unsafe { GetCurrentThreadId() };
    let line = format!("[{secs:10.3}] [{tid:5}] {args}\n");
    if let Ok(mut f) = logger.file.lock() {
        let _ = f.write_all(line.as_bytes());
        let _ = f.flush();
    }
}

/// For use while the process is going down, where another thread may have died holding the lock.
pub fn try_write(args: std::fmt::Arguments) {
    let Some(Some(logger)) = LOGGER.get() else { return };
    if let Ok(mut f) = logger.file.try_lock() {
        let _ = writeln!(f, "[{:10.3}] {args}", logger.start.elapsed().as_secs_f64());
    }
}

#[macro_export]
macro_rules! log {
    ($($t:tt)*) => { $crate::logging::write(format_args!($($t)*)) };
}
