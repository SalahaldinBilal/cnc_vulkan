mod audio;
mod bones;
mod camera;
mod client;
mod config;
mod controlbar;
mod crash;
mod crt;
mod d3d;
mod direct3d;
mod discord;
mod fog;
mod frame;
mod gadgets;
mod game;
mod gui;
mod iat;
mod lines;
mod loader;
mod logging;
mod matrix;
mod objects;
mod optionsmenu;
mod overlays;
mod particles;
mod pacing;
mod paths;
mod radar;
mod radar_layers;
mod radar_map;
mod render_timers;
mod map_objects;
mod scene;
mod settings;
mod terrain;
mod text;
mod things;
mod transitions;
mod treads;
mod trees;
mod ui;
mod vk;
mod watchdog;
mod water;
mod window;
mod zoom;

use std::ffi::c_void;

use windows_sys::Win32::Foundation::{BOOL, HINSTANCE, TRUE};
use windows_sys::Win32::System::LibraryLoader::DisableThreadLibraryCalls;
use windows_sys::Win32::System::SystemServices::DLL_PROCESS_ATTACH;

use crate::d3d::HRESULT;

#[unsafe(no_mangle)]
extern "system" fn DllMain(module: HINSTANCE, reason: u32, _reserved: *mut c_void) -> BOOL {
    if reason == DLL_PROCESS_ATTACH {
        unsafe { DisableThreadLibraryCalls(module) };
        attach(module);
    }
    TRUE
}

/// Runs while the game's static imports are being loaded, before its own code starts, so every
/// hook is in place before the game creates its window.
fn attach(module: HINSTANCE) {
    logging::init();
    let paths = paths::get();
    log!("cnc-vulkan {} loaded into {} (game: {})", env!("CARGO_PKG_VERSION"), paths.exe.display(), paths.game_id);
    config::load();
    game::build();
    game::unlock_aspect_ratios();
    ui::install();
    optionsmenu::install();
    if config::get().is_ok_and(|c| c.engine.split_logic) {
        frame::install();
    }
    scene::install();
    unsafe { zoom::install() };
    unsafe { terrain::install() };
    unsafe { fog::install() };
    unsafe { crt::install() };
    unsafe { text::install() };
    unsafe { radar::install() };
    unsafe { radar_layers::install() };
    unsafe { radar_map::install() };
    unsafe { audio::install() };
    unsafe { render_timers::install() };
    discord::install(paths.game_id);
    if let Ok(c) = config::get() {
        audio::set_in_background(c.engine.background_audio);
        discord::set_enabled(c.engine.discord);
        zoom::set_percent(c.graphics.zoom_out);
        zoom::set_speed(c.graphics.zoom_speed);
        radar::set_size(c.graphics.radar_size);
        radar::set_terrain(c.graphics.radar_terrain);
        radar_map::set_objects(c.graphics.radar_objects);
        radar_layers::set_fog_size(c.graphics.radar_fog_size);
        radar_layers::set_sharp_units(c.graphics.radar_sharp_units);
        fog::set_smooth(c.graphics.smooth_fog);
        fog::set_objects(c.graphics.fog_on_objects);
    }
    loader::install(module);
    window::set_dpi_awareness();
    crash::install();
    window::install_hooks();
}

#[unsafe(no_mangle)]
pub extern "system" fn Direct3DCreate8(sdk_version: u32) -> *mut c_void {
    unsafe { direct3d::create(sdk_version) }
}

#[unsafe(no_mangle)]
extern "system" fn DirectInput8Create(
    instance: *mut c_void,
    version: u32,
    riid: *const c_void,
    out: *mut *mut c_void,
    outer: *mut c_void,
) -> HRESULT {
    unsafe { loader::direct_input8_create(instance, version, riid, out, outer) }
}
