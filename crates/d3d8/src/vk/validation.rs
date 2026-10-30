//! The Khronos validation layer, turned on by `[debug] validation` in cnc-vulkan.toml or the
//! `CNC_VULKAN_VALIDATION` environment variable, with its messages written to our log.

use std::collections::HashMap;
use std::ffi::{CStr, c_void};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use ash::vk;

use crate::{config, log};

pub const LAYER: &CStr = c"VK_LAYER_KHRONOS_validation";

/// Each message id is logged this many times, then only counted.
const REPEATS_LOGGED: u32 = 3;

static ERRORS: AtomicU64 = AtomicU64::new(0);
static SEEN: Mutex<Option<HashMap<i32, u32>>> = Mutex::new(None);

pub struct Validation {
    _utils: ash::ext::debug_utils::Instance,
    _messenger: vk::DebugUtilsMessengerEXT,
}

/// Whether validation was asked for and the layer is installed.
pub fn requested(entry: &ash::Entry) -> bool {
    let wanted = std::env::var_os("CNC_VULKAN_VALIDATION").is_some() || config::get().is_ok_and(|c| c.debug.validation);
    if !wanted {
        return false;
    }
    let layers = unsafe { entry.enumerate_instance_layer_properties() }.unwrap_or_default();
    let found = layers.iter().any(|l| unsafe { CStr::from_ptr(l.layer_name.as_ptr()) } == LAYER);
    if found {
        log!("vulkan validation on");
    } else {
        log!("vulkan validation asked for, but the 32-bit VK_LAYER_KHRONOS_validation is not installed");
    }
    found
}

pub fn start(entry: &ash::Entry, instance: &ash::Instance) -> Result<Validation, String> {
    let utils = ash::ext::debug_utils::Instance::new(entry, instance);
    let info = vk::DebugUtilsMessengerCreateInfoEXT::default()
        .message_severity(vk::DebugUtilsMessageSeverityFlagsEXT::WARNING | vk::DebugUtilsMessageSeverityFlagsEXT::ERROR)
        .message_type(
            vk::DebugUtilsMessageTypeFlagsEXT::GENERAL
                | vk::DebugUtilsMessageTypeFlagsEXT::VALIDATION
                | vk::DebugUtilsMessageTypeFlagsEXT::PERFORMANCE,
        )
        .pfn_user_callback(Some(callback));
    let messenger = unsafe { utils.create_debug_utils_messenger(&info, None) }
        .map_err(|e| format!("vkCreateDebugUtilsMessengerEXT failed: {e}"))?;
    Ok(Validation { _utils: utils, _messenger: messenger })
}

#[cfg(test)]
pub fn errors() -> u64 {
    ERRORS.load(Ordering::Relaxed)
}

unsafe extern "system" fn callback(
    severity: vk::DebugUtilsMessageSeverityFlagsEXT,
    _types: vk::DebugUtilsMessageTypeFlagsEXT,
    data: *const vk::DebugUtilsMessengerCallbackDataEXT<'_>,
    _user: *mut c_void,
) -> vk::Bool32 {
    let error = severity.contains(vk::DebugUtilsMessageSeverityFlagsEXT::ERROR);
    if error {
        ERRORS.fetch_add(1, Ordering::Relaxed);
    }
    let data = unsafe { &*data };
    let count = {
        let mut seen = SEEN.lock().unwrap();
        let count = seen.get_or_insert_with(HashMap::new).entry(data.message_id_number).or_insert(0);
        *count += 1;
        *count
    };
    if count > REPEATS_LOGGED {
        return vk::FALSE;
    }
    let message = if data.p_message.is_null() {
        "".into()
    } else {
        unsafe { CStr::from_ptr(data.p_message) }.to_string_lossy()
    };
    let kind = if error { "error" } else { "warning" };
    let last = if count == REPEATS_LOGGED { " (repeats are no longer logged)" } else { "" };
    log!("vulkan validation {kind}{last}: {message}");
    vk::FALSE
}
