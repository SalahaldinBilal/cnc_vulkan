use std::ffi::OsString;
use std::os::windows::ffi::OsStringExt;
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use windows_sys::Win32::Foundation::{EXCEPTION_ACCESS_VIOLATION, HMODULE};
use windows_sys::Win32::System::Diagnostics::Debug::{
    AddVectoredExceptionHandler, EXCEPTION_POINTERS, LPTOP_LEVEL_EXCEPTION_FILTER, MINIDUMP_EXCEPTION_INFORMATION, MiniDumpWithDataSegs,
    MiniDumpWithIndirectlyReferencedMemory, MiniDumpWithThreadInfo, MiniDumpWriteDump, SetUnhandledExceptionFilter,
};
use windows_sys::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT, GetModuleFileNameW,
    GetModuleHandleExW,
};
use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetCurrentProcessId, GetCurrentThreadId};

use crate::{iat, log, logging, paths};

const EXCEPTION_CONTINUE_SEARCH: i32 = 0;

/// The filter the game registered, which runs after ours so its own crash report still gets written.
static GAME_FILTER: AtomicUsize = AtomicUsize::new(0);
static HANDLING: AtomicBool = AtomicBool::new(false);
static LOGGED_FAULTS: AtomicU32 = AtomicU32::new(0);

pub fn install() {
    unsafe {
        AddVectoredExceptionHandler(1, Some(on_exception));
        SetUnhandledExceptionFilter(Some(on_crash));
        iat::hook_main_module("KERNEL32.dll", "SetUnhandledExceptionFilter", set_filter_hook as *const () as usize);
    }
    std::panic::set_hook(Box::new(|info| {
        log!("panic: {info}");
    }));
}

unsafe extern "system" fn set_filter_hook(filter: LPTOP_LEVEL_EXCEPTION_FILTER) -> LPTOP_LEVEL_EXCEPTION_FILTER {
    let new = filter.map_or(0, |f| f as usize);
    let previous = GAME_FILTER.swap(new, Ordering::SeqCst);
    log!("game registered crash filter {new:#x}, kept ours in front of it");
    unsafe { std::mem::transmute::<usize, LPTOP_LEVEL_EXCEPTION_FILTER>(previous) }
}

/// The game catches exceptions in its main loop and shows its own error box, which never reaches
/// the crash filter, so access violations are logged when they happen.
unsafe extern "system" fn on_exception(info: *mut EXCEPTION_POINTERS) -> i32 {
    let record = unsafe { &*(*info).ExceptionRecord };
    if record.ExceptionCode == EXCEPTION_ACCESS_VIOLATION && LOGGED_FAULTS.fetch_add(1, Ordering::Relaxed) < 8 {
        let address = record.ExceptionAddress as usize;
        let context = unsafe { &*(*info).ContextRecord };
        logging::try_write(format_args!(
            "access violation at {address:#010x} ({}) {} {:#010x}, eax {:#x} ecx {:#x} edx {:#x} esi {:#x} edi {:#x}, return {:#010x}",
            module_offset(address),
            if record.ExceptionInformation[0] == 0 { "reading" } else { "writing" },
            record.ExceptionInformation[1],
            context.Eax,
            context.Ecx,
            context.Edx,
            context.Esi,
            context.Edi,
            unsafe { *(context.Esp as *const u32) }
        ));
    }
    EXCEPTION_CONTINUE_SEARCH
}

unsafe extern "system" fn on_crash(info: *const EXCEPTION_POINTERS) -> i32 {
    if !HANDLING.swap(true, Ordering::SeqCst) {
        unsafe { report(info) };
    }
    let game = GAME_FILTER.load(Ordering::SeqCst);
    if game != 0 {
        let f: unsafe extern "system" fn(*const EXCEPTION_POINTERS) -> i32 = unsafe { std::mem::transmute(game) };
        return unsafe { f(info) };
    }
    EXCEPTION_CONTINUE_SEARCH
}

unsafe fn report(info: *const EXCEPTION_POINTERS) {
    let record = unsafe { &*(*info).ExceptionRecord };
    let address = record.ExceptionAddress as usize;
    logging::try_write(format_args!(
        "CRASH: exception {:#010x} at {address:#010x} ({})",
        record.ExceptionCode as u32,
        module_offset(address)
    ));

    let Some(dir) = paths::data_subdir("crashes") else { return };
    let stamp = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    let path = dir.join(format!("crash-{stamp}.dmp"));
    let Ok(file) = std::fs::File::create(&path) else { return };
    let exception = MINIDUMP_EXCEPTION_INFORMATION {
        ThreadId: unsafe { GetCurrentThreadId() },
        ExceptionPointers: info as *mut _,
        ClientPointers: 0,
    };
    let kind = MiniDumpWithDataSegs | MiniDumpWithIndirectlyReferencedMemory | MiniDumpWithThreadInfo;
    let ok = unsafe {
        MiniDumpWriteDump(
            GetCurrentProcess(),
            GetCurrentProcessId(),
            file.as_raw_handle() as _,
            kind,
            &exception,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
    logging::try_write(format_args!(
        "minidump {} {}",
        if ok != 0 { "written to" } else { "FAILED for" },
        path.display()
    ));
}

fn module_offset(address: usize) -> String {
    let mut module: HMODULE = std::ptr::null_mut();
    let flags = GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT;
    if unsafe { GetModuleHandleExW(flags, address as *const u16, &mut module) } == 0 {
        return "unknown module".into();
    }
    let mut buf = [0u16; 512];
    let len = unsafe { GetModuleFileNameW(module, buf.as_mut_ptr(), buf.len() as u32) } as usize;
    let path = OsString::from_wide(&buf[..len]);
    let name = std::path::Path::new(&path).file_name().map(|n| n.to_string_lossy().into_owned());
    format!("{}+{:#x}", name.unwrap_or_default(), address - module as usize)
}
