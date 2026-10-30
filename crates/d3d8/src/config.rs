use std::sync::OnceLock;

use serde::Deserialize;

use crate::{log, paths, settings};

pub const FILE_NAME: &str = "cnc-vulkan.toml";

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub display: Display,
    pub graphics: Graphics,
    pub ui: Ui,
    pub engine: Engine,
    pub debug: Debug,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Debug {
    /// Load the Khronos validation layer and log what it reports. Needs the layer installed.
    pub validation: bool,
    /// Sample the main thread about 1000 times a second and log where the time goes.
    pub profile: bool,
    /// Time the game's render systems and log ms per frame for each.
    pub timers: bool,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Graphics {
    /// Anisotropic filtering for the 3D world, 2 to 16. 0 keeps the game's own filtering.
    pub anisotropy: u32,
    /// Size the 3D world is drawn at, in percent of the screen, 25 to 200. The interface is
    /// always drawn at full size.
    pub render_scale: u32,
    /// Sharpening of the 3D world, in percent. 0 is off.
    pub sharpening: u32,
    /// Multisample anti-aliasing of the 3D world: 1 (off), 2, 4 or 8 samples per pixel.
    pub msaa: u32,
    /// FXAA on the 3D world.
    pub fxaa: bool,
    /// Ambient occlusion: 0 off, 1 low, 2 medium, 3 high.
    pub ambient_occlusion: u32,
    /// Particles fade out where they meet the ground and objects.
    pub soft_particles: bool,
    /// How high the camera can zoom out, in percent of the game's own limit, 100 to 400.
    pub zoom_out: u32,
    /// How far each zoom step goes, in percent of the game's, 25 to 400.
    pub zoom_speed: u32,
    /// Size the radar's terrain image is drawn at, 128 (the game's) to 2048.
    pub radar_size: u32,
    /// How the radar's terrain image is made.
    pub radar_terrain: RadarTerrain,
    /// With the map drawn from above, the map file's objects and trees drawn on it too.
    pub radar_objects: bool,
    /// Size the radar's fog of war is drawn at, 128 (the game's) to 1024.
    pub radar_fog_size: u32,
    /// Radar unit dots placed finer and redrawn every logic step, and the camera box exact.
    pub radar_sharp_units: bool,
    /// Fog of war in the world drawn with smooth edges.
    pub smooth_fog: bool,
    /// Objects the game counts as visible darkened where they reach into the fog, as the ground is.
    pub fog_on_objects: bool,
    /// Black raised to this percent of full scale, 0 to 10, so near black detail shows on
    /// monitors that crush it.
    pub shadow_brightness: u32,
}

pub const DEFAULT_RENDER_SCALE: u32 = 100;

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum RadarTerrain {
    /// The terrain drawn from above by the game's terrain renderer.
    Map,
    /// The game's build at a higher resolution with colors from the tile textures.
    Textures,
    /// The game's build with its colors, at the chosen resolution.
    Game,
}

impl Default for Graphics {
    fn default() -> Self {
        Self {
            anisotropy: 16,
            render_scale: DEFAULT_RENDER_SCALE,
            sharpening: 0,
            msaa: 1,
            fxaa: false,
            ambient_occlusion: 2,
            soft_particles: true,
            zoom_out: 100,
            zoom_speed: 100,
            radar_size: 1024,
            radar_terrain: RadarTerrain::Map,
            radar_objects: true,
            radar_fog_size: 512,
            radar_sharp_units: true,
            smooth_fog: true,
            fog_on_objects: true,
            shadow_brightness: 0,
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Engine {
    /// Run game logic on its own fixed clock so rendering can go faster than the logic rate.
    pub split_logic: bool,
    /// Go straight to the main menu instead of playing the intro movies.
    pub skip_intro: bool,
    /// Keep the game's sound playing while its window isn't focused.
    pub background_audio: bool,
    /// Show what's being played in Discord.
    pub discord: bool,
}

impl Default for Engine {
    fn default() -> Self {
        Self { split_logic: true, skip_intro: true, background_audio: true, discord: true }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Ui {
    pub layout: UiLayout,
    /// 1.0 is the largest UI that fits the screen without stretching. Lower values shrink it.
    pub scale: f32,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum UiLayout {
    /// Uniform scale, anchored to screen edges.
    Fit,
    /// The game's original layout, stretched to the screen.
    Stretch,
}

impl Default for Ui {
    fn default() -> Self {
        Self { layout: UiLayout::Fit, scale: 1.0 }
    }
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Display {
    pub mode: DisplayMode,
    /// "auto" (the options menu's once picked, else the monitor's), "native", "game", or "WIDTHxHEIGHT".
    pub resolution: Resolution,
    /// 0 is the primary monitor, 1 and up count the others in Windows' enumeration order.
    pub monitor: u32,
    pub vsync: bool,
    /// Frames per second cap. 0 means uncapped.
    pub fps_limit: u32,
    pub clip_cursor: bool,
}

#[derive(Debug, Deserialize, Clone, Copy, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum DisplayMode {
    Borderless,
    Windowed,
    /// Exclusive fullscreen, the game's own window handling.
    Fullscreen,
}

impl Default for Display {
    fn default() -> Self {
        Self {
            mode: DisplayMode::Borderless,
            resolution: Resolution::Auto,
            monitor: 0,
            vsync: true,
            fps_limit: 0,
            clip_cursor: false,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    /// The options menu's resolution once one was picked, the monitor's until then.
    Auto,
    Native,
    Game,
    Fixed(u32, u32),
}

impl<'de> Deserialize<'de> for Resolution {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let text = String::deserialize(d)?;
        match text.as_str() {
            "auto" => Ok(Self::Auto),
            "native" => Ok(Self::Native),
            "game" => Ok(Self::Game),
            other => other
                .split_once('x')
                .and_then(|(w, h)| Some((w.trim().parse().ok()?, h.trim().parse().ok()?)))
                .filter(|&(w, h)| w >= 640 && h >= 480)
                .map(|(w, h)| Self::Fixed(w, h))
                .ok_or_else(|| {
                    serde::de::Error::custom(format!(
                        "resolution must be \"auto\", \"native\", \"game\" or like \"3840x2160\" (at least 640x480), got \"{other}\""
                    ))
                }),
        }
    }
}

static CONFIG: OnceLock<Result<Config, String>> = OnceLock::new();

pub fn load() {
    CONFIG.get_or_init(|| {
        let path = paths::get().exe_dir.join(FILE_NAME);
        let mut config = match std::fs::read_to_string(&path) {
            Ok(text) => toml::from_str::<Config>(&text).map_err(|e| format!("{}: {e}", path.display()))?,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                log!("no {FILE_NAME} next to the game, using built-in settings");
                Config::default()
            }
            Err(e) => return Err(format!("{}: {e}", path.display())),
        };
        settings::apply(&mut config);
        log!("config: {config:?}");
        Ok(config)
    });
}

pub fn get() -> Result<&'static Config, &'static str> {
    match CONFIG.get() {
        Some(Ok(c)) => Ok(c),
        Some(Err(e)) => Err(e.as_str()),
        None => Err("config was never loaded"),
    }
}
