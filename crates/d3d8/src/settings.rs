//! Our settings live in the game's own Options.ini, next to its settings, so the options menu can
//! show and change them. The game keeps every key it reads, so ours survive its own saves.

use std::path::PathBuf;
use std::sync::OnceLock;

use windows_sys::Win32::System::Registry::{HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, RRF_RT_REG_SZ, RegGetValueA};
use windows_sys::Win32::UI::Shell::{CSIDL_PERSONAL, SHGetFolderPathW};

use crate::config::{Config, DisplayMode, RadarTerrain, UiLayout};
use crate::log;

pub const DISPLAY_MODE: &str = "CncVulkanDisplayMode";
pub const MONITOR: &str = "CncVulkanMonitor";
pub const VSYNC: &str = "CncVulkanVSync";
pub const FPS_LIMIT: &str = "CncVulkanFpsLimit";
pub const UI_LAYOUT: &str = "CncVulkanUILayout";
pub const UI_SCALE: &str = "CncVulkanUIScale";
pub const LOCK_MOUSE: &str = "CncVulkanLockMouse";
pub const SKIP_INTRO: &str = "CncVulkanSkipIntro";
pub const BACKGROUND_AUDIO: &str = "CncVulkanBackgroundAudio";
pub const DISCORD: &str = "CncVulkanDiscord";
pub const ANISOTROPY: &str = "CncVulkanAnisotropy";
pub const RENDER_SCALE: &str = "CncVulkanRenderScale";
pub const SHARPENING: &str = "CncVulkanSharpening";
pub const MSAA: &str = "CncVulkanMsaa";
pub const FXAA: &str = "CncVulkanFxaa";
pub const AMBIENT_OCCLUSION: &str = "CncVulkanAmbientOcclusion";
pub const SOFT_PARTICLES: &str = "CncVulkanSoftParticles";
pub const ZOOM_OUT: &str = "CncVulkanZoomOut";
pub const ZOOM_SPEED: &str = "CncVulkanZoomSpeed";
pub const RADAR_SIZE: &str = "CncVulkanRadarSize";
pub const RADAR_TERRAIN: &str = "CncVulkanRadarTerrain";
pub const RADAR_OBJECTS: &str = "CncVulkanRadarObjects";
pub const RADAR_FOG_SIZE: &str = "CncVulkanRadarFogSize";
pub const RADAR_SHARP_UNITS: &str = "CncVulkanRadarSharpUnits";
pub const SMOOTH_FOG: &str = "CncVulkanSmoothFog";
pub const FOG_ON_OBJECTS: &str = "CncVulkanFogOnObjects";
pub const SHADOW_BRIGHTNESS: &str = "CncVulkanShadowBrightness";
const RESOLUTION: &str = "Resolution";

/// Where the game looks up its user data folder name, and the name it uses when that's unset.
const GAME_KEY: &[u8] = b"SOFTWARE\\Electronic Arts\\EA Games\\Command and Conquer Generals Zero Hour\0";
const LEAF_VALUE: &[u8] = b"UserDataLeafName\0";
const DEFAULT_LEAF: &str = "Command and Conquer Generals Zero Hour Data";
/// Base Generals sets its folder name in GameData.ini (`UserDataLeafName`) instead.
const GENERALS_LEAF: &str = "Command and Conquer Generals Data";

static PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn registry_leaf(root: HKEY) -> Option<String> {
    let mut buf = [0u8; 260];
    let mut len = buf.len() as u32;
    let status = unsafe {
        RegGetValueA(
            root,
            GAME_KEY.as_ptr(),
            LEAF_VALUE.as_ptr(),
            RRF_RT_REG_SZ,
            std::ptr::null_mut(),
            buf.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 {
        return None;
    }
    let text = String::from_utf8_lossy(&buf[..len as usize]).trim_end_matches('\0').to_string();
    (!text.is_empty()).then_some(text)
}

/// The game's Options.ini: My Documents, then the user data folder name the game would use.
pub fn options_ini() -> Option<&'static PathBuf> {
    PATH.get_or_init(|| {
        let mut buf = [0u16; 260];
        if unsafe { SHGetFolderPathW(std::ptr::null_mut(), CSIDL_PERSONAL as i32, std::ptr::null_mut(), 0, buf.as_mut_ptr()) }
            != 0
        {
            log!("could not find the Documents folder, Options.ini settings unavailable");
            return None;
        }
        let len = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
        let documents = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
        let leaf = if crate::paths::get().game_id == "generals" {
            GENERALS_LEAF.to_string()
        } else {
            registry_leaf(HKEY_LOCAL_MACHINE).or_else(|| registry_leaf(HKEY_CURRENT_USER)).unwrap_or_else(|| DEFAULT_LEAF.to_string())
        };
        Some(documents.join(leaf).join("Options.ini"))
    })
    .as_ref()
}

fn read_lines() -> Vec<String> {
    options_ini()
        .and_then(|p| std::fs::read(p).ok())
        .map(|bytes| String::from_utf8_lossy(&bytes).lines().map(str::to_string).collect())
        .unwrap_or_default()
}

fn split(line: &str) -> Option<(&str, &str)> {
    let (key, value) = line.split_once('=')?;
    Some((key.trim(), value.trim()))
}

fn value(lines: &[String], key: &str) -> Option<String> {
    lines.iter().filter_map(|l| split(l)).find(|(k, _)| k.eq_ignore_ascii_case(key)).map(|(_, v)| v.to_string())
}

pub fn display_mode_name(mode: DisplayMode) -> &'static str {
    match mode {
        DisplayMode::Borderless => "borderless",
        DisplayMode::Windowed => "windowed",
        DisplayMode::Fullscreen => "fullscreen",
    }
}

pub fn radar_terrain_name(mode: RadarTerrain) -> &'static str {
    match mode {
        RadarTerrain::Map => "map",
        RadarTerrain::Textures => "textures",
        RadarTerrain::Game => "game",
    }
}

pub fn ui_layout_name(layout: UiLayout) -> &'static str {
    match layout {
        UiLayout::Fit => "fit",
        UiLayout::Stretch => "stretch",
    }
}

/// Replaces settings from cnc-vulkan.toml with the ones saved in Options.ini.
pub fn apply(config: &mut Config) {
    let lines = read_lines();
    let get = |key| value(&lines, key);
    if let Some(v) = get(DISPLAY_MODE) {
        match v.to_ascii_lowercase().as_str() {
            "borderless" => config.display.mode = DisplayMode::Borderless,
            "windowed" => config.display.mode = DisplayMode::Windowed,
            "fullscreen" => config.display.mode = DisplayMode::Fullscreen,
            _ => log!("Options.ini: unknown {DISPLAY_MODE} \"{v}\""),
        }
    }
    if let Some(v) = get(MONITOR).and_then(|v| v.parse().ok()) {
        config.display.monitor = v;
    }
    if let Some(v) = get(VSYNC) {
        config.display.vsync = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(FPS_LIMIT).and_then(|v| v.parse().ok()) {
        config.display.fps_limit = v;
    }
    if let Some(v) = get(UI_LAYOUT) {
        match v.to_ascii_lowercase().as_str() {
            "fit" => config.ui.layout = UiLayout::Fit,
            "stretch" => config.ui.layout = UiLayout::Stretch,
            _ => log!("Options.ini: unknown {UI_LAYOUT} \"{v}\""),
        }
    }
    if let Some(v) = get(UI_SCALE).and_then(|v| v.parse::<f32>().ok()) {
        config.ui.scale = v.clamp(0.25, 1.0);
    }
    if let Some(v) = get(LOCK_MOUSE) {
        config.display.clip_cursor = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(SKIP_INTRO) {
        config.engine.skip_intro = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(BACKGROUND_AUDIO) {
        config.engine.background_audio = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(DISCORD) {
        config.engine.discord = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(ANISOTROPY).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.anisotropy = v.min(16);
    }
    if let Some(v) = get(RENDER_SCALE).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.render_scale = v;
    }
    if let Some(v) = get(SHARPENING).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.sharpening = v.min(100);
    }
    if let Some(v) = get(MSAA).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.msaa = v.clamp(1, 8);
    }
    if let Some(v) = get(FXAA) {
        config.graphics.fxaa = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(AMBIENT_OCCLUSION).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.ambient_occlusion = v.min(3);
    }
    if let Some(v) = get(SOFT_PARTICLES) {
        config.graphics.soft_particles = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(ZOOM_OUT).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.zoom_out = v.clamp(crate::zoom::MIN_PERCENT, crate::zoom::MAX_PERCENT);
    }
    if let Some(v) = get(ZOOM_SPEED).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.zoom_speed = v.clamp(crate::zoom::MIN_SPEED, crate::zoom::MAX_SPEED);
    }
    if let Some(v) = get(RADAR_SIZE).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.radar_size = v.clamp(128, 2048);
    }
    if let Some(v) = get(RADAR_TERRAIN) {
        match v.to_ascii_lowercase().as_str() {
            "map" => config.graphics.radar_terrain = RadarTerrain::Map,
            "textures" => config.graphics.radar_terrain = RadarTerrain::Textures,
            "game" => config.graphics.radar_terrain = RadarTerrain::Game,
            _ => log!("Options.ini: unknown {RADAR_TERRAIN} \"{v}\""),
        }
    }
    if let Some(v) = get(RADAR_OBJECTS) {
        config.graphics.radar_objects = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(RADAR_FOG_SIZE).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.radar_fog_size = v.clamp(128, 1024);
    }
    if let Some(v) = get(RADAR_SHARP_UNITS) {
        config.graphics.radar_sharp_units = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(SMOOTH_FOG) {
        config.graphics.smooth_fog = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(FOG_ON_OBJECTS) {
        config.graphics.fog_on_objects = v.eq_ignore_ascii_case("yes");
    }
    if let Some(v) = get(SHADOW_BRIGHTNESS).and_then(|v| v.parse::<u32>().ok()) {
        config.graphics.shadow_brightness = v.min(crate::vk::swapchain::MAX_SHADOW_BRIGHTNESS);
    }
}

/// Whether the player picked a resolution in the options menu.
pub fn has_resolution() -> bool {
    value(&read_lines(), RESOLUTION).is_some()
}

/// Writes our keys, keeping every other line as it is.
pub fn save(values: &[(&str, String)]) {
    let Some(path) = options_ini() else { return };
    let crlf = std::fs::read(path).is_ok_and(|b| b.windows(2).any(|w| w == b"\r\n"));
    let mut lines = read_lines();
    for (key, v) in values {
        let line = format!("{key} = {v}");
        match lines.iter_mut().find(|l| split(l).is_some_and(|(k, _)| k.eq_ignore_ascii_case(key))) {
            Some(existing) => *existing = line,
            None => lines.push(line),
        }
    }
    let newline = if crlf { "\r\n" } else { "\n" };
    let mut text = lines.join(newline);
    text.push_str(newline);
    match std::fs::write(path, text) {
        Ok(()) => log!("saved our settings to {}", path.display()),
        Err(e) => log!("could not save our settings to {}: {e}", path.display()),
    }
}
