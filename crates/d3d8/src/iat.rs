use std::ffi::{CStr, c_void};

use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
use windows_sys::Win32::System::Memory::{PAGE_READWRITE, VirtualProtect};

use crate::log;

const IMPORT_DIRECTORY_OFFSET: usize = 0x80;
const ORDINAL_FLAG: u32 = 0x8000_0000;

#[repr(C)]
struct ImportDescriptor {
    original_first_thunk: u32,
    time_date_stamp: u32,
    forwarder_chain: u32,
    name: u32,
    first_thunk: u32,
}

/// Points the main executable's import of `dll!function` at `hook` and returns the previous target.
pub unsafe fn hook_main_module(dll: &str, function: &str, hook: usize) -> Option<usize> {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let result = unsafe { find_slot(base, dll, function) }.map(|slot| unsafe { swap(slot, hook) });
    match result {
        Some(_) => log!("hooked import {dll}!{function}"),
        None => log!("import {dll}!{function} not found, hook not applied"),
    }
    result
}

/// Same, for an import only some builds have. Quiet when there's none.
pub unsafe fn hook_main_module_if_imported(dll: &str, function: &str, hook: usize) -> Option<usize> {
    let base = unsafe { GetModuleHandleW(std::ptr::null()) } as usize;
    let previous = unsafe { find_slot(base, dll, function) }.map(|slot| unsafe { swap(slot, hook) });
    if previous.is_some() {
        log!("hooked import {dll}!{function}");
    }
    previous
}

unsafe fn find_slot(base: usize, dll: &str, function: &str) -> Option<*mut usize> {
    unsafe {
        let nt = base + *((base + 0x3C) as *const u32) as usize;
        let dir_rva = *((nt + IMPORT_DIRECTORY_OFFSET) as *const u32) as usize;
        if dir_rva == 0 {
            return None;
        }
        let mut desc = (base + dir_rva) as *const ImportDescriptor;
        while (*desc).name != 0 {
            let name = CStr::from_ptr((base + (*desc).name as usize) as *const i8);
            if name.to_string_lossy().eq_ignore_ascii_case(dll) && (*desc).original_first_thunk != 0 {
                let names = (base + (*desc).original_first_thunk as usize) as *const u32;
                let slots = (base + (*desc).first_thunk as usize) as *mut usize;
                let mut i = 0;
                while *names.add(i) != 0 {
                    let entry = *names.add(i);
                    if entry & ORDINAL_FLAG == 0 {
                        let import_name = CStr::from_ptr((base + entry as usize + 2) as *const i8);
                        if import_name.to_bytes() == function.as_bytes() {
                            return Some(slots.add(i));
                        }
                    }
                    i += 1;
                }
            }
            desc = desc.add(1);
        }
        None
    }
}

unsafe fn swap(slot: *mut usize, value: usize) -> usize {
    unsafe {
        let mut old = 0;
        VirtualProtect(slot as *const c_void, size_of::<usize>(), PAGE_READWRITE, &mut old);
        let previous = slot.replace(value);
        VirtualProtect(slot as *const c_void, size_of::<usize>(), old, &mut old);
        previous
    }
}
