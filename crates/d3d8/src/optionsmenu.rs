//! Our settings page in the game's options menu. The page is built from copies of the menu's own
//! gadgets (taken from OptionsMenu.wnd in WindowZH.big at runtime) and placed next to the
//! Advanced Display Options page. A button beside the resolution list opens it. The values are
//! saved to Options.ini when the menu's Accept is pressed. See knowledge-base/options-menu.md.

use std::ffi::CString;
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

use crate::config::{DisplayMode, RadarTerrain, UiLayout};
use crate::gadgets::{self, UString, hide, send};
use crate::{config, game, log, pacing, paths, settings, ui, vk, window};

struct Addresses {
    /// `call FileSystem::openFile(path, access)` (thiscall on TheFileSystem) in the .wnd loader.
    layout_open_call: usize,
    file_system_open: usize,
    /// Function pointers in the callback name table: OptionsMenuInit and OptionsMenuSystem.
    init_entry: usize,
    options_menu_init: usize,
    system_entry: usize,
    options_menu_system: usize,
    layout_archive: &'static str,
    /// LocalFile::convertToRAMFile. Generals' LocalFile reads unbuffered, and the .wnd parser
    /// reads one byte per call, so our layout is loaded into memory first.
    convert_to_ram_file: Option<usize>,
    /// GadgetHorizontalSliderInput, which our sliders' input goes through for keys and hover.
    slider_input: usize,
    /// W3DGadgetHorizontalSliderImageDraw, which our sliders replace.
    slider_draw: usize,
}

const CONVERT_TO_RAM_FILE_VSLOT: usize = 0x3C / 4;
const DISPLAY_MODE_COUNT_VSLOT: usize = 0x3C / 4;
const DISPLAY_MODE_DESCRIPTION_VSLOT: usize = 0x40 / 4;

static ZERO_HOUR: Addresses = Addresses {
    layout_open_call: 0x006D_BCDC,
    file_system_open: 0x0044_EA30,
    init_entry: 0x009E_C42C,
    options_menu_init: 0x0046_5BC0,
    system_entry: 0x009E_BEA4,
    options_menu_system: 0x0046_85D0,
    layout_archive: "WindowZH.big",
    convert_to_ram_file: None,
    slider_input: 0x006D_3800,
    slider_draw: 0x007C_B220,
};

static GENERALS: Addresses = Addresses {
    layout_open_call: 0x006C_404C,
    file_system_open: 0x0044_8940,
    init_entry: 0x00A2_611C,
    options_menu_init: 0x0046_5D50,
    system_entry: 0x00A2_5BA4,
    options_menu_system: 0x0046_8250,
    layout_archive: "Window.big",
    convert_to_ram_file: Some(0x007E_A350),
    slider_input: 0x006B_F880,
    slider_draw: 0x007A_1E80,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

const GGM_SET_LABEL: u32 = 0x4001;
const GBM_SELECTED: u32 = 0x4008;
const GSM_SLIDER_TRACK: u32 = 0x400B;
const GSM_SET_SLIDER: u32 = 0x400C;
const GCM_ADD_ENTRY: u32 = 0x4020;
const GCM_DEL_ALL: u32 = 0x4022;
const GCM_GET_SELECTION: u32 = 0x4029;
const GCM_SET_SELECTION: u32 = 0x402A;
const GWM_LEFT_DOWN: u32 = 5;
const GWM_LEFT_UP: u32 = 6;
const GWM_LEFT_DRAG: u32 = 8;
const WHITE: usize = 0xFFFF_FFFF;
const MSG_HANDLED: u32 = 1;

const LAYOUT_ENTRY: &str = "Window\\Menus\\OptionsMenu.wnd";
const PREFIX: &str = "OptionsMenu.wnd:";

const FPS_LIMITS: [u32; 9] = [0, 60, 75, 100, 120, 144, 165, 240, 360];
/// Interface sizes in percent.
const UI_SCALES: [u32; 7] = [50, 60, 70, 75, 80, 90, 100];
const MODES: [DisplayMode; 3] = [DisplayMode::Borderless, DisplayMode::Windowed, DisplayMode::Fullscreen];
const LAYOUTS: [UiLayout; 2] = [UiLayout::Fit, UiLayout::Stretch];
const RADAR_TERRAINS: [RadarTerrain; 3] = [RadarTerrain::Map, RadarTerrain::Textures, RadarTerrain::Game];
const ANISOTROPY_LEVELS: [u32; 5] = [16, 8, 4, 2, 0];
const RENDER_SCALES: [u32; 9] = [50, 67, 75, 85, 100, 125, 150, 175, 200];
const SHARPENING_LEVELS: [u32; 5] = [0, 25, 50, 75, 100];
const MSAA_LEVELS: [u32; 4] = [1, 2, 4, 8];
const ZOOM_OUT_LEVELS: [u32; 8] = [100, 125, 150, 175, 200, 250, 300, 400];
const ZOOM_SPEEDS: [u32; 7] = [50, 75, 100, 150, 200, 300, 400];
const AO_NAMES: [&str; 4] = ["Off", "Low", "Medium", "High"];
const GRAPHICS_TAB: usize = 1;
/// Our page, and the empty space below the Graphics tab's rows where the calibration pattern
/// goes, in the layout's 800x600.
const PAGE_RECT: [i32; 4] = [151, 68, 636, 560];
const PATTERN_AREA: [i32; 4] = [170, 380, 620, 472];

/// Rows of the page: window name suffix, label, and the tab it's on.
const ROWS: [(&str, &str, usize); 27] = [
    ("Mode", "Display mode *", 0),
    ("Monitor", "Monitor *", 0),
    ("VSync", "Vertical sync *", 0),
    ("FpsLimit", "Frame limit", 0),
    ("LockMouse", "Keep mouse in game window", 0),
    ("BackgroundAudio", "Play sound in background", 0),
    ("Discord", "Show what you play on Discord", 0),
    ("UiLayout", "Interface layout *", 0),
    ("UiScale", "Interface size *", 0),
    ("SkipIntro", "Skip intro movies *", 0),
    ("RenderScale", "3D resolution", 1),
    ("Sharpening", "Sharpening", 1),
    ("Anisotropy", "Texture filtering", 1),
    ("Msaa", "Multisampling", 1),
    ("Fxaa", "FXAA", 1),
    ("AmbientOcclusion", "Ambient occlusion", 1),
    ("SoftParticles", "Soft particles", 1),
    ("ShadowBrightness", "Shadow brightness", 1),
    ("ZoomOut", "Camera zoom out", 2),
    ("ZoomSpeed", "Camera zoom speed", 2),
    ("RadarSize", "Minimap resolution *", 2),
    ("RadarTerrain", "Minimap terrain *", 2),
    ("RadarObjects", "Objects and trees on minimap *", 2),
    ("RadarFog", "Minimap fog resolution", 2),
    ("RadarUnits", "Sharp minimap units and view", 2),
    ("SmoothFog", "Smooth fog of war edges", 2),
    ("FogOnObjects", "Shade objects in fog of war", 2),
];

/// Indices into `ROWS`.
mod row {
    pub const MODE: usize = 0;
    pub const MONITOR: usize = 1;
    pub const VSYNC: usize = 2;
    pub const FPS_LIMIT: usize = 3;
    pub const LOCK_MOUSE: usize = 4;
    pub const BACKGROUND_AUDIO: usize = 5;
    pub const DISCORD: usize = 6;
    pub const UI_LAYOUT: usize = 7;
    pub const UI_SCALE: usize = 8;
    pub const SKIP_INTRO: usize = 9;
    pub const RENDER_SCALE: usize = 10;
    pub const SHARPENING: usize = 11;
    pub const ANISOTROPY: usize = 12;
    pub const MSAA: usize = 13;
    pub const FXAA: usize = 14;
    pub const AMBIENT_OCCLUSION: usize = 15;
    pub const SOFT_PARTICLES: usize = 16;
    pub const SHADOW_BRIGHTNESS: usize = 17;
    pub const ZOOM_OUT: usize = 18;
    pub const ZOOM_SPEED: usize = 19;
    pub const RADAR_SIZE: usize = 20;
    pub const RADAR_TERRAIN: usize = 21;
    pub const RADAR_OBJECTS: usize = 22;
    pub const RADAR_FOG: usize = 23;
    pub const RADAR_UNITS: usize = 24;
    pub const SMOOTH_FOG: usize = 25;
    pub const FOG_ON_OBJECTS: usize = 26;
}

/// What each row is edited with.
#[derive(Clone, Copy, PartialEq)]
enum Control {
    Combo,
    /// A slider over a list of values. `off`: its lowest value gets a box of its own.
    Slider { off: bool },
    /// The game's checkbox, ticked for true.
    Check,
}

fn control(i: usize) -> Control {
    use row::*;
    match i {
        SHARPENING | AMBIENT_OCCLUSION | SHADOW_BRIGHTNESS => Control::Slider { off: true },
        UI_SCALE | RENDER_SCALE | ZOOM_OUT | ZOOM_SPEED | RADAR_SIZE | RADAR_FOG => Control::Slider { off: false },
        VSYNC | LOCK_MOUSE | BACKGROUND_AUDIO | DISCORD | SKIP_INTRO | FXAA | SOFT_PARTICLES | RADAR_OBJECTS | RADAR_UNITS
        | SMOOTH_FOG | FOG_ON_OBJECTS => Control::Check,
        _ => Control::Combo,
    }
}

/// The values a slider row steps through, lowest first.
fn slider_values(c: &Choices, i: usize) -> Vec<u32> {
    match i {
        row::SHARPENING => c.sharpening.clone(),
        row::AMBIENT_OCCLUSION => (0..AO_NAMES.len() as u32).collect(),
        row::SHADOW_BRIGHTNESS => (0..=vk::swapchain::MAX_SHADOW_BRIGHTNESS).collect(),
        row::UI_SCALE => c.scales.clone(),
        row::RENDER_SCALE => c.render_scales.clone(),
        row::ZOOM_OUT => c.zoom_out.clone(),
        row::ZOOM_SPEED => c.zoom_speed.clone(),
        row::RADAR_SIZE => c.radar_sizes.clone(),
        row::RADAR_FOG => c.radar_fog_sizes.clone(),
        _ => Vec::new(),
    }
}

/// A slider row's label with its value in brackets.
fn slider_label(i: usize, v: u32) -> String {
    let text = match i {
        row::SHARPENING | row::SHADOW_BRIGHTNESS if v == 0 => "Off".to_string(),
        row::AMBIENT_OCCLUSION => AO_NAMES[(v as usize).min(AO_NAMES.len() - 1)].to_string(),
        row::RENDER_SCALE if v == 100 => "100% (screen)".to_string(),
        row::ZOOM_OUT | row::ZOOM_SPEED if v == 100 => "Game default".to_string(),
        row::RADAR_SIZE | row::RADAR_FOG => v.to_string(),
        _ => format!("{v}%"),
    };
    format!("{} [{text}]", ROWS[i].1)
}

/// Title of each tab and the note under it.
const TABS: [(&str, &str); 3] = [
    ("Display and Interface", "* Takes effect after restarting the game."),
    ("Graphics", "Shadow brightness: lowest value that shows the leftmost square."),
    ("Map and Camera", "* Takes effect with the next map."),
];
/// The buttons on the game's page that open each tab: name, label and rectangle, down the right
/// of the display options next to Resolution, Detail and the Brightness label.
const OPEN_BUTTONS: [(&str, &str, [i32; 4]); 3] = [
    ("CncvButtonDisplay", "Display", [310, 126, 386, 154]),
    ("CncvButtonGraphics", "Graphics", [310, 180, 386, 208]),
    ("CncvButtonMap", "Map", [310, 214, 386, 240]),
];

#[derive(Clone, Copy, PartialEq)]
struct Values {
    mode: DisplayMode,
    monitor: u32,
    vsync: bool,
    fps_limit: u32,
    lock_mouse: bool,
    background_audio: bool,
    discord: bool,
    layout: UiLayout,
    scale: f32,
    skip_intro: bool,
    anisotropy: u32,
    render_scale: u32,
    sharpening: u32,
    msaa: u32,
    fxaa: bool,
    ambient_occlusion: u32,
    soft_particles: bool,
    zoom_out: u32,
    zoom_speed: u32,
    radar_size: u32,
    radar_terrain: RadarTerrain,
    radar_objects: bool,
    radar_fog_size: u32,
    radar_sharp_units: bool,
    smooth_fog: bool,
    fog_on_objects: bool,
    shadow_brightness: u32,
}

impl Values {
    const DEFAULT: Values = Values {
        mode: DisplayMode::Borderless,
        monitor: 0,
        vsync: true,
        fps_limit: 0,
        lock_mouse: false,
        background_audio: true,
        discord: true,
        layout: UiLayout::Fit,
        scale: 1.0,
        skip_intro: true,
        anisotropy: 16,
        render_scale: config::DEFAULT_RENDER_SCALE,
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
    };
}

/// The values each combo box and slider offers, the saved value included.
struct Choices {
    monitors: Vec<u32>,
    fps_limits: Vec<u32>,
    /// Interface sizes in percent.
    scales: Vec<u32>,
    anisotropy: Vec<u32>,
    render_scales: Vec<u32>,
    sharpening: Vec<u32>,
    msaa: Vec<u32>,
    zoom_out: Vec<u32>,
    zoom_speed: Vec<u32>,
    radar_sizes: Vec<u32>,
    radar_fog_sizes: Vec<u32>,
}

struct Page {
    opens: [usize; OPEN_BUTTONS.len()],
    page: usize,
    ok: usize,
    cancel: usize,
    accept: usize,
    back: usize,
    defaults: usize,
    /// The tab on show, `None` while the page is hidden.
    tab: Option<usize>,
    /// What the page showed when it was opened, which its Cancel goes back to.
    opened_with: Values,
    labels: [usize; ROWS.len()],
    /// Each row's combo box, or its slider.
    controls: [usize; ROWS.len()],
    note: usize,
    title: usize,
    choices: Choices,
}

static LAYOUT_PATH: OnceLock<Option<CString>> = OnceLock::new();
static PAGE: Mutex<Option<Page>> = Mutex::new(None);
/// Our sliders with an Off box, which their draw and input need to know.
static OFF_BOX_SLIDERS: Mutex<Vec<usize>> = Mutex::new(Vec::new());
/// What's saved, starting with what the game started with.
static SAVED: Mutex<Option<Values>> = Mutex::new(None);

pub fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    if game::call_target(a.layout_open_call) != Some(a.file_system_open) {
        log!("options menu layout call isn't what we expect, our settings page is off");
        return;
    }
    unsafe {
        game::patch_call(a.layout_open_call, open_layout as *const () as usize);
        game::write_u32(a.init_entry, options_init as *const () as u32);
        game::write_u32(a.system_entry, options_system as *const () as u32);
    }
}

fn saved() -> Values {
    *SAVED.lock().unwrap().get_or_insert_with(|| match config::get() {
        Ok(c) => Values {
            mode: c.display.mode,
            monitor: c.display.monitor,
            vsync: c.display.vsync,
            fps_limit: c.display.fps_limit,
            lock_mouse: c.display.clip_cursor,
            background_audio: c.engine.background_audio,
            discord: c.engine.discord,
            layout: c.ui.layout,
            scale: c.ui.scale,
            skip_intro: c.engine.skip_intro,
            anisotropy: c.graphics.anisotropy,
            render_scale: c.graphics.render_scale,
            sharpening: c.graphics.sharpening,
            msaa: c.graphics.msaa,
            fxaa: c.graphics.fxaa,
            ambient_occlusion: c.graphics.ambient_occlusion,
            soft_particles: c.graphics.soft_particles,
            zoom_out: c.graphics.zoom_out,
            zoom_speed: c.graphics.zoom_speed,
            radar_size: c.graphics.radar_size,
            radar_terrain: c.graphics.radar_terrain,
            radar_objects: c.graphics.radar_objects,
            radar_fog_size: c.graphics.radar_fog_size,
            radar_sharp_units: c.graphics.radar_sharp_units,
            smooth_fog: c.graphics.smooth_fog,
            fog_on_objects: c.graphics.fog_on_objects,
            shadow_brightness: c.graphics.shadow_brightness,
        },
        Err(_) => Values::DEFAULT,
    })
}


fn be32(b: &[u8]) -> usize {
    u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize
}

/// Reads one file out of a .big archive.
fn read_from_archive(archive: &PathBuf, entry: &str) -> Option<Vec<u8>> {
    let mut file = File::open(archive).ok()?;
    let mut header = [0u8; 16];
    file.read_exact(&mut header).ok()?;
    if &header[..4] != b"BIGF" && &header[..4] != b"BIG4" {
        return None;
    }
    let count = be32(&header[8..]);
    let index_end = be32(&header[12..]);
    let mut index = vec![0u8; index_end.saturating_sub(16)];
    file.read_exact(&mut index).ok()?;
    let mut at = 0;
    for _ in 0..count {
        let offset = be32(&index[at..]);
        let size = be32(&index[at + 4..]);
        let name_end = at + 8 + index[at + 8..].iter().position(|&c| c == 0)?;
        let name = String::from_utf8_lossy(&index[at + 8..name_end]);
        if name.eq_ignore_ascii_case(entry) {
            let mut data = vec![0u8; size];
            file.seek(SeekFrom::Start(offset as u64)).ok()?;
            file.read_exact(&mut data).ok()?;
            return Some(data);
        }
        at = name_end + 1;
    }
    None
}

/// Byte ranges of the lines of `text`, each including its line break.
fn lines(text: &str) -> Vec<(usize, usize)> {
    let mut out = Vec::new();
    let mut start = 0;
    for line in text.split_inclusive('\n') {
        out.push((start, start + line.len()));
        start += line.len();
    }
    out
}

/// The window block (`WINDOW` to its matching `END`) that contains `needle`.
fn block_with(text: &str, needle: &str) -> Option<(usize, usize)> {
    let pos = text.find(needle)?;
    let lines = lines(text);
    let first = lines.iter().rposition(|&(s, e)| s < pos && text[s..e].trim() == "WINDOW")?;
    let mut depth = 0;
    for &(s, e) in &lines[first..] {
        match text[s..e].trim() {
            "WINDOW" => depth += 1,
            "END" => {
                depth -= 1;
                if depth == 0 {
                    return Some((lines[first].0, e));
                }
            }
            _ => {}
        }
    }
    None
}

fn block(text: &str, name: &str) -> Option<String> {
    let (s, e) = block_with(text, &format!("NAME = \"{PREFIX}{name}\";"))?;
    Some(text[s..e].to_string())
}

/// A copy of a gadget block with a new name and rectangle, and without text or tooltip.
fn gadget(template: &str, name: &str, rect: [i32; 4], keep_text: bool) -> String {
    let mut out = String::new();
    for line in template.split_inclusive('\n') {
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        if t.starts_with("NAME =") {
            out.push_str(&format!("{indent}NAME = \"{PREFIX}{name}\";\r\n"));
        } else if t.starts_with("SCREENRECT =") {
            out.push_str(&format!("{indent}SCREENRECT = UPPERLEFT: {} {},\r\n", rect[0], rect[1]));
        } else if t.starts_with("BOTTOMRIGHT:") {
            out.push_str(&format!("{indent}BOTTOMRIGHT: {} {},\r\n", rect[2], rect[3]));
        } else if t.starts_with("TOOLTIPTEXT =") || (!keep_text && t.starts_with("TEXT =")) {
        } else {
            out.push_str(line);
        }
    }
    out
}

/// A copy of the gamma slider with a new name and rectangle. Its range is set when the page opens,
/// from the row's list of values.
fn slider(template: &str, name: &str, rect: [i32; 4]) -> String {
    let mut out = String::new();
    for line in gadget(template, name, rect, false).split_inclusive('\n') {
        let t = line.trim_start();
        let indent = &line[..line.len() - t.len()];
        if t.starts_with("SLIDERDATA =") {
            out.push_str(&format!("{indent}SLIDERDATA = MINVALUE: 0,\r\n"));
        } else if t.starts_with("MAXVALUE:") {
            out.push_str(&format!("{indent}MAXVALUE: 10;\r\n"));
        } else {
            out.push_str(line);
        }
    }
    out
}

/// The window name of row `i`'s combo box, slider or checkbox.
fn control_name(i: usize) -> String {
    let kind = match control(i) {
        Control::Combo => "CncvCombo",
        Control::Slider { .. } => "CncvSlider",
        Control::Check => "CncvCheck",
    };
    format!("{kind}{}", ROWS[i].0)
}

/// The position of row `i` within its tab.
fn row_in_tab(i: usize) -> usize {
    ROWS[..i].iter().filter(|r| r.2 == ROWS[i].2).count()
}

fn child(block: &str) -> String {
    format!("CHILD\r\n{block}")
}

/// The original layout with our page and its open button added.
fn build_layout(original: &str) -> Option<String> {
    let combo = block(original, "ComboBoxDetail")?;
    let slider_template = block(original, "SliderGamma")?;
    let check = block(original, "CheckAlternateMouse")?;
    let label = block(original, "DetailLabel")?;
    let button = block(original, "ButtonAdvanceAccept")?;
    let cancel = block(original, "ButtonAdvanceBack")?;
    let line = block(original, "AdvanceOptionsLine")?;
    let (ts, te) = block_with(original, "TEXT = \"GUI:AdvanceDisplayOptions\";")?;
    let title = &original[ts..te];
    let (ps, pe) = block_with(original, &format!("NAME = \"{PREFIX}WinAdvancedDisplayOptions\";"))?;
    let advanced = &original[ps..pe];
    let header_end = lines(advanced).into_iter().find(|&(s, e)| advanced[s..e].trim() == "CHILD")?.0;
    let header = gadget(&advanced[..header_end], "CncvPage", PAGE_RECT, false);

    let mut page = header;
    page += &child(&gadget(title, "CncvTitle", [162, 74, 576, 102], false));
    page += &child(&gadget(&line, "CncvLine", [151, 105, 636, 106], false));
    page += &child(&gadget(&label, "CncvNote", [170, 480, 620, 504], false));
    page += &child(&gadget(&button, "CncvButtonOk", [229, 521, 388, 553], true));
    page += &child(&gadget(&cancel, "CncvButtonCancel", [400, 521, 559, 553], true));
    // Windows defined later draw on top, so each row's drop-down list has to come after the rows below it.
    for (i, (name, _, _)) in ROWS.iter().enumerate().rev() {
        let y = 124 + row_in_tab(i) as i32 * 32;
        // A label wraps when its text is wider than its window, and only one line fits.
        page += &child(&gadget(&label, &format!("CncvLabel{name}"), [170, y, 414, y + 24], false));
        let rect = [420, y, 620, y + 24];
        page += &child(&match control(i) {
            Control::Combo => gadget(&combo, &control_name(i), rect, false),
            Control::Slider { .. } => slider(&slider_template, &control_name(i), rect),
            Control::Check => gadget(&check, &control_name(i), [420, y, 444, y + 24], false),
        });
    }
    page += "ENDALLCHILDREN\r\nEND\r\n";

    let opens: String = OPEN_BUTTONS.iter().map(|(name, _, rect)| child(&gadget(&button, name, *rect, false))).collect();
    let (_, resolution_end) = block_with(original, &format!("NAME = \"{PREFIX}ComboBoxResolution\";"))?;
    let mut out = String::with_capacity(original.len() + page.len() + opens.len() + 64);
    out.push_str(&original[..resolution_end]);
    out.push_str(&opens);
    out.push_str(&original[resolution_end..pe]);
    out.push_str(&child(&page));
    out.push_str(&original[pe..]);
    Some(out)
}

#[cfg(test)]
mod tests {
    /// Runs against a copy extracted with bigtool into re/out/big, when there is one.
    #[test]
    fn builds_from_the_real_layout() {
        check("big/WindowZH.big", "OptionsMenu.generated.wnd");
    }

    /// Generals' copy, extracted into re/out/big_gen.
    #[test]
    fn builds_from_the_generals_layout() {
        check("big_gen/Window.big", "OptionsMenu.generals.generated.wnd");
    }

    fn check(archive: &str, out: &str) {
        let root = format!("{}/../../re/out/{archive}/Window/Menus/OptionsMenu.wnd", env!("CARGO_MANIFEST_DIR"));
        let Ok(bytes) = std::fs::read(root) else { return };
        let original = String::from_utf8_lossy(&bytes);
        let built = super::build_layout(&original).expect("layout built");
        std::fs::write(format!("{}/../../re/out/{out}", env!("CARGO_MANIFEST_DIR")), &built).unwrap();
        let count = |t: &str, word: &str| t.lines().filter(|l| l.trim() == word).count();
        assert_eq!(count(&original, "WINDOW"), count(&original, "END"), "original");
        assert_eq!(count(&built, "WINDOW"), count(&built, "END"));
        let added = 1 + super::OPEN_BUTTONS.len() + 3 + 2 + 2 * super::ROWS.len();
        assert_eq!(count(&built, "WINDOW"), count(&original, "WINDOW") + added);
        assert_eq!(count(&built, "ENDALLCHILDREN"), count(&original, "ENDALLCHILDREN") + 1);
    }
}

fn layout_path() -> Option<&'static CString> {
    LAYOUT_PATH
        .get_or_init(|| {
            let archive = paths::get().exe_dir.join(addresses().layout_archive);
            let Some(data) = read_from_archive(&archive, LAYOUT_ENTRY) else {
                log!("{LAYOUT_ENTRY} not found in {}, our settings page is off", archive.display());
                return None;
            };
            let original = String::from_utf8_lossy(&data);
            let Some(layout) = build_layout(&original) else {
                log!("options menu layout doesn't have the windows we copy, our settings page is off");
                return None;
            };
            let path = paths::get().data_dir.as_ref()?.join("OptionsMenu.wnd");
            if let Err(e) = std::fs::write(&path, layout) {
                log!("could not write {}: {e}", path.display());
                return None;
            }
            log!("options menu layout with our settings page written to {}", path.display());
            CString::new(path.to_string_lossy().as_bytes()).ok()
        })
        .as_ref()
}

/// Replaces FileSystem::openFile in the .wnd loader. The options menu loads our copy.
extern "thiscall" fn open_layout(file_system: usize, path: *const i8, access: u32) -> usize {
    let original: extern "thiscall" fn(usize, *const i8, u32) -> usize =
        unsafe { std::mem::transmute(addresses().file_system_open) };
    let name = unsafe { std::ffi::CStr::from_ptr(path) }.to_string_lossy().to_ascii_lowercase().replace('/', "\\");
    ui::set_loading_layout(&name);
    if name.ends_with("\\optionsmenu.wnd")
        && let Some(ours) = layout_path()
    {
        let file = original(file_system, ours.as_ptr(), access);
        if file != 0 {
            return in_memory(file);
        }
        log!("the game could not open our options menu layout, using its own");
    }
    original(file_system, path, access)
}

/// Swaps an opened local file for a RAM file with its contents. The local file is freed.
fn in_memory(file: usize) -> usize {
    let Some(convert) = addresses().convert_to_ram_file else { return file };
    let slot = unsafe { *(*(file as *const *const usize)).add(CONVERT_TO_RAM_FILE_VSLOT) };
    if slot != convert {
        return file;
    }
    let f: extern "thiscall" fn(usize) -> usize = unsafe { std::mem::transmute(convert) };
    f(file)
}

fn find(name: &str) -> usize {
    gadgets::find(&format!("{PREFIX}{name}"))
}

fn set_label(window: usize, text: &str) {
    let s = UString::new(text);
    send(window, GGM_SET_LABEL, s.as_msg_data(), 0);
}

fn fill(combo: &[usize], row: usize, entries: &[String]) {
    send(combo[row], GCM_DEL_ALL, 0, 0);
    for e in entries {
        let s = UString::new(e);
        send(combo[row], GCM_ADD_ENTRY, s.as_msg_data(), WHITE);
    }
}

fn selection(combo: usize) -> usize {
    let mut pos: i32 = -1;
    send(combo, GCM_GET_SELECTION, 0, &mut pos as *mut i32 as usize);
    pos.max(0) as usize
}

fn select(combo: usize, pos: usize) {
    send(combo, GCM_SET_SELECTION, pos, 0);
}


fn choices(saved: &Values) -> Choices {
    let mut monitors: Vec<u32> = (0..window::monitor_count().max(1)).collect();
    if !monitors.contains(&saved.monitor) {
        monitors.push(saved.monitor);
    }
    let mut fps_limits = FPS_LIMITS.to_vec();
    // A few fps under the refresh rate keeps a variable refresh rate display in its range.
    let refresh = window::monitor_refresh(saved.monitor);
    for extra in [refresh.saturating_sub(3), saved.fps_limit] {
        if extra > 0 && !fps_limits.contains(&extra) {
            fps_limits.push(extra);
        }
    }
    fps_limits.sort();
    let with = |list: &[u32], value: u32| {
        let mut list = list.to_vec();
        if !list.contains(&value) {
            list.push(value);
            list.sort();
        }
        list
    };
    let scales = with(&UI_SCALES, percent(saved.scale));
    let anisotropy = with(&ANISOTROPY_LEVELS, saved.anisotropy);
    let render_scales = with(&RENDER_SCALES, saved.render_scale);
    let sharpening = with(&SHARPENING_LEVELS, saved.sharpening);
    let msaa = with(&MSAA_LEVELS, saved.msaa);
    let zoom_out = with(&ZOOM_OUT_LEVELS, saved.zoom_out);
    let zoom_speed = with(&ZOOM_SPEEDS, saved.zoom_speed);
    let radar_sizes = with(&crate::radar::SIZES, saved.radar_size);
    let radar_fog_sizes = with(&crate::radar_layers::FOG_SIZES, saved.radar_fog_size);
    Choices { monitors, fps_limits, scales, anisotropy, render_scales, sharpening, msaa, zoom_out, zoom_speed, radar_sizes, radar_fog_sizes }
}

/// Shows the page with the rows of one tab, or hides it with `None`.
fn show_tab(p: &mut Page, tab: Option<usize>) {
    if p.tab.is_none() && tab.is_some() {
        p.opened_with = read_values(p);
    }
    p.tab = tab;
    hide(p.page, tab.is_none());
    if let Some(tab) = tab {
        for (i, r) in ROWS.iter().enumerate() {
            hide(p.labels[i], r.2 != tab);
            hide(p.controls[i], r.2 != tab);
        }
        set_label(p.title, TABS[tab].0);
        set_label(p.note, TABS[tab].1);
    }
    update_calibration(p);
}

/// The calibration pattern shows in the empty space under the Graphics tab's rows while it's open.
fn update_calibration(p: &Page) {
    let area = || {
        let (x, y) = gadgets::screen_position(p.page);
        let (w, h) = gadgets::size(p.page);
        let sx = w as f32 / (PAGE_RECT[2] - PAGE_RECT[0]) as f32;
        let sy = h as f32 / (PAGE_RECT[3] - PAGE_RECT[1]) as f32;
        [
            x as f32 + (PATTERN_AREA[0] - PAGE_RECT[0]) as f32 * sx,
            y as f32 + (PATTERN_AREA[1] - PAGE_RECT[1]) as f32 * sy,
            (PATTERN_AREA[2] - PATTERN_AREA[0]) as f32 * sx,
            (PATTERN_AREA[3] - PATTERN_AREA[1]) as f32 * sy,
        ]
    };
    vk::swapchain::show_calibration((p.tab == Some(GRAPHICS_TAB)).then(area));
}

fn percent(scale: f32) -> u32 {
    (scale * 100.0).round() as u32
}

/// A slider row's value, as its position picks it from the row's list.
fn slider_value(p: &Page, i: usize) -> Option<u32> {
    let values = slider_values(&p.choices, i);
    let position = gadgets::slider_position(p.controls[i])?;
    values.get((position.max(0) as usize).min(values.len().saturating_sub(1))).copied()
}

/// Moves a slider row to `v`, or the nearest value its list has, and shows it in the label.
fn show_slider(p: &Page, i: usize, v: u32) {
    let values = slider_values(&p.choices, i);
    let Some(position) = (0..values.len()).min_by_key(|&k| values[k].abs_diff(v)) else { return };
    send(p.controls[i], GSM_SET_SLIDER, position, 0);
    set_label(p.labels[i], &slider_label(i, values[position]));
}

/// A slider moved: its label follows, and Shadow brightness previews on the whole picture until
/// the menu closes, whatever tab is open.
fn slider_changed(p: &Page, i: usize, v: u32) {
    set_label(p.labels[i], &slider_label(i, v));
    if i == row::SHADOW_BRIGHTNESS {
        vk::swapchain::preview_shadow_brightness(Some(v));
    }
}

fn show_values(p: &Page, v: &Values) {
    let c = &p.choices;
    let check = |i: usize, on: bool| gadgets::set_checked(p.controls[i], on);
    select(p.controls[row::MODE], MODES.iter().position(|m| *m == v.mode).unwrap_or(0));
    select(p.controls[row::MONITOR], c.monitors.iter().position(|m| *m == v.monitor).unwrap_or(0));
    check(row::VSYNC, v.vsync);
    select(p.controls[row::FPS_LIMIT], c.fps_limits.iter().position(|f| *f == v.fps_limit).unwrap_or(0));
    check(row::LOCK_MOUSE, v.lock_mouse);
    check(row::BACKGROUND_AUDIO, v.background_audio);
    check(row::DISCORD, v.discord);
    select(p.controls[row::UI_LAYOUT], LAYOUTS.iter().position(|l| *l == v.layout).unwrap_or(0));
    show_slider(p, row::UI_SCALE, percent(v.scale));
    check(row::SKIP_INTRO, v.skip_intro);
    select(p.controls[row::ANISOTROPY], c.anisotropy.iter().position(|a| *a == v.anisotropy).unwrap_or(0));
    show_slider(p, row::RENDER_SCALE, v.render_scale);
    show_slider(p, row::SHARPENING, v.sharpening);
    select(p.controls[row::MSAA], c.msaa.iter().position(|s| *s == v.msaa).unwrap_or(0));
    check(row::FXAA, v.fxaa);
    show_slider(p, row::AMBIENT_OCCLUSION, v.ambient_occlusion);
    check(row::SOFT_PARTICLES, v.soft_particles);
    show_slider(p, row::SHADOW_BRIGHTNESS, v.shadow_brightness);
    show_slider(p, row::ZOOM_OUT, v.zoom_out);
    show_slider(p, row::ZOOM_SPEED, v.zoom_speed);
    show_slider(p, row::RADAR_SIZE, v.radar_size);
    select(p.controls[row::RADAR_TERRAIN], RADAR_TERRAINS.iter().position(|m| *m == v.radar_terrain).unwrap_or(0));
    check(row::RADAR_OBJECTS, v.radar_objects);
    show_slider(p, row::RADAR_FOG, v.radar_fog_size);
    check(row::RADAR_UNITS, v.radar_sharp_units);
    check(row::SMOOTH_FOG, v.smooth_fog);
    check(row::FOG_ON_OBJECTS, v.fog_on_objects);
}

fn read_values(p: &Page) -> Values {
    let c = &p.choices;
    let pick = |row: usize, len: usize| selection(p.controls[row]).min(len.saturating_sub(1));
    let checked = |i: usize| gadgets::checked(p.controls[i]);
    let saved = saved();
    // A slider without slider data can't be read, which leaves the saved value.
    let slider = |i: usize, saved: u32| slider_value(p, i).unwrap_or(saved);
    Values {
        mode: MODES[pick(row::MODE, MODES.len())],
        monitor: c.monitors[pick(row::MONITOR, c.monitors.len())],
        vsync: checked(row::VSYNC),
        fps_limit: c.fps_limits[pick(row::FPS_LIMIT, c.fps_limits.len())],
        lock_mouse: checked(row::LOCK_MOUSE),
        background_audio: checked(row::BACKGROUND_AUDIO),
        discord: checked(row::DISCORD),
        layout: LAYOUTS[pick(row::UI_LAYOUT, LAYOUTS.len())],
        scale: slider(row::UI_SCALE, percent(saved.scale)) as f32 / 100.0,
        skip_intro: checked(row::SKIP_INTRO),
        anisotropy: c.anisotropy[pick(row::ANISOTROPY, c.anisotropy.len())],
        render_scale: slider(row::RENDER_SCALE, saved.render_scale),
        sharpening: slider(row::SHARPENING, saved.sharpening),
        msaa: c.msaa[pick(row::MSAA, c.msaa.len())],
        fxaa: checked(row::FXAA),
        ambient_occlusion: slider(row::AMBIENT_OCCLUSION, saved.ambient_occlusion),
        soft_particles: checked(row::SOFT_PARTICLES),
        shadow_brightness: slider(row::SHADOW_BRIGHTNESS, saved.shadow_brightness),
        zoom_out: slider(row::ZOOM_OUT, saved.zoom_out),
        zoom_speed: slider(row::ZOOM_SPEED, saved.zoom_speed),
        radar_size: slider(row::RADAR_SIZE, saved.radar_size),
        radar_terrain: RADAR_TERRAINS[pick(row::RADAR_TERRAIN, RADAR_TERRAINS.len())],
        radar_objects: checked(row::RADAR_OBJECTS),
        radar_fog_size: slider(row::RADAR_FOG, saved.radar_fog_size),
        radar_sharp_units: checked(row::RADAR_UNITS),
        smooth_fog: checked(row::SMOOTH_FOG),
        fog_on_objects: checked(row::FOG_ON_OBJECTS),
    }
}

fn setup_page() -> Option<Page> {
    let page = find("CncvPage");
    if page == 0 {
        return None;
    }
    let saved = saved();
    let mut p = Page {
        opens: OPEN_BUTTONS.map(|(name, _, _)| find(name)),
        page,
        ok: find("CncvButtonOk"),
        cancel: find("CncvButtonCancel"),
        accept: find("ButtonAccept"),
        back: find("ButtonBack"),
        defaults: find("ButtonDefaults"),
        tab: None,
        opened_with: saved,
        labels: ROWS.map(|(name, _, _)| find(&format!("CncvLabel{name}"))),
        controls: std::array::from_fn(|i| find(&control_name(i))),
        note: find("CncvNote"),
        title: find("CncvTitle"),
        choices: choices(&saved),
    };
    for (window, (_, label, _)) in p.opens.iter().zip(OPEN_BUTTONS) {
        set_label(*window, label);
    }
    for (i, (_, text, _)) in ROWS.iter().enumerate() {
        set_label(p.labels[i], text);
    }
    for i in 0..ROWS.len() {
        if let Control::Slider { off } = control(i) {
            install_slider(p.controls[i], off);
            gadgets::set_slider_range(p.controls[i], 0, slider_values(&p.choices, i).len() as i32 - 1);
        }
    }
    show_tab(&mut p, None);
    let c = &p.choices;
    fill(&p.controls, row::MODE, &["Borderless window".into(), "Window".into(), "Fullscreen".into()]);
    let names = window::monitor_names();
    let monitor = |m: &u32| names.get(*m as usize).cloned().unwrap_or_else(|| format!("Monitor {}", m + 1));
    fill(&p.controls, row::MONITOR, &c.monitors.iter().map(monitor).collect::<Vec<_>>());
    let fps = |f: &u32| if *f == 0 { "No limit".to_string() } else { format!("{f} fps") };
    fill(&p.controls, row::FPS_LIMIT, &c.fps_limits.iter().map(fps).collect::<Vec<_>>());
    fill(&p.controls, row::UI_LAYOUT, &["Fit".into(), "Stretch".into()]);
    let anisotropy = |a: &u32| if *a <= 1 { "Game default".to_string() } else { format!("{a}x anisotropic") };
    fill(&p.controls, row::ANISOTROPY, &c.anisotropy.iter().map(anisotropy).collect::<Vec<_>>());
    let msaa = |s: &u32| if *s <= 1 { "Off".to_string() } else { format!("{s}x") };
    fill(&p.controls, row::MSAA, &c.msaa.iter().map(msaa).collect::<Vec<_>>());
    fill(&p.controls, row::RADAR_TERRAIN, &["Map from above".into(), "Terrain textures".into(), "Game default".into()]);
    show_values(&p, &saved);
    Some(p)
}

fn resolution_fields() -> Option<(*mut u32, *mut u32)> {
    let gd = unsafe { *(game::build()?.global_data as *const usize) };
    (gd != 0).then(|| ((gd + game::global_data::X_RESOLUTION) as *mut u32, (gd + game::global_data::Y_RESOLUTION) as *mut u32))
}

fn display_mode_index(width: u32, height: u32) -> Option<usize> {
    let display = unsafe { *(game::build()?.the_display as *const usize) };
    if display == 0 {
        return None;
    }
    unsafe {
        let vtable = *(display as *const *const usize);
        let count: extern "thiscall" fn(usize) -> i32 = std::mem::transmute(*vtable.add(DISPLAY_MODE_COUNT_VSLOT));
        let describe: extern "thiscall" fn(usize, i32, *mut u32, *mut u32, *mut u32) =
            std::mem::transmute(*vtable.add(DISPLAY_MODE_DESCRIPTION_VSLOT));
        (0..count(display)).position(|i| {
            let (mut w, mut h, mut bits) = (0, 0, 0);
            describe(display, i, &mut w, &mut h, &mut bits);
            w == width && h == height
        })
    }
}

/// OptionsMenuInit selects the resolution saved in Options.ini (800x600 when there's none) and
/// writes it into GlobalData. We render at our own resolution, so it's put back and selected.
fn show_real_resolution(width: u32, height: u32) {
    if let Some((x, y)) = resolution_fields() {
        unsafe { (*x, *y) = (width, height) };
    }
    match display_mode_index(width, height) {
        Some(i) => select(find("ComboBoxResolution"), i),
        None => log!("{width}x{height} isn't in the display mode list, the options menu can't show it"),
    }
}

extern "C" fn options_init(layout: usize, user_data: usize) {
    let original: extern "C" fn(usize, usize) = unsafe { std::mem::transmute(addresses().options_menu_init) };
    let resolution = resolution_fields().map(|(x, y)| unsafe { (*x, *y) });
    original(layout, user_data);
    if let Some((width, height)) = resolution {
        show_real_resolution(width, height);
    }
    // The last menu's windows are gone, and new ones can take their addresses.
    *PAGE.lock().unwrap() = None;
    OFF_BOX_SLIDERS.lock().unwrap().clear();
    vk::swapchain::show_calibration(None);
    vk::swapchain::preview_shadow_brightness(None);
    let page = setup_page();
    if page.is_none() {
        log!("options menu opened without our settings page");
    }
    *PAGE.lock().unwrap() = page;
}

fn save(v: &Values) {
    settings::save(&[
        (settings::DISPLAY_MODE, settings::display_mode_name(v.mode).to_string()),
        (settings::MONITOR, v.monitor.to_string()),
        (settings::VSYNC, if v.vsync { "yes" } else { "no" }.to_string()),
        (settings::FPS_LIMIT, v.fps_limit.to_string()),
        (settings::UI_LAYOUT, settings::ui_layout_name(v.layout).to_string()),
        (settings::UI_SCALE, format!("{}", v.scale)),
        (settings::LOCK_MOUSE, if v.lock_mouse { "yes" } else { "no" }.to_string()),
        (settings::SKIP_INTRO, if v.skip_intro { "yes" } else { "no" }.to_string()),
        (settings::BACKGROUND_AUDIO, if v.background_audio { "yes" } else { "no" }.to_string()),
        (settings::DISCORD, if v.discord { "yes" } else { "no" }.to_string()),
        (settings::ANISOTROPY, v.anisotropy.to_string()),
        (settings::RENDER_SCALE, v.render_scale.to_string()),
        (settings::SHARPENING, v.sharpening.to_string()),
        (settings::MSAA, v.msaa.to_string()),
        (settings::FXAA, if v.fxaa { "yes" } else { "no" }.to_string()),
        (settings::AMBIENT_OCCLUSION, v.ambient_occlusion.to_string()),
        (settings::SOFT_PARTICLES, if v.soft_particles { "yes" } else { "no" }.to_string()),
        (settings::ZOOM_OUT, v.zoom_out.to_string()),
        (settings::ZOOM_SPEED, v.zoom_speed.to_string()),
        (settings::RADAR_SIZE, v.radar_size.to_string()),
        (settings::RADAR_TERRAIN, settings::radar_terrain_name(v.radar_terrain).to_string()),
        (settings::RADAR_OBJECTS, if v.radar_objects { "yes" } else { "no" }.to_string()),
        (settings::RADAR_FOG_SIZE, v.radar_fog_size.to_string()),
        (settings::RADAR_SHARP_UNITS, if v.radar_sharp_units { "yes" } else { "no" }.to_string()),
        (settings::SMOOTH_FOG, if v.smooth_fog { "yes" } else { "no" }.to_string()),
        (settings::FOG_ON_OBJECTS, if v.fog_on_objects { "yes" } else { "no" }.to_string()),
        (settings::SHADOW_BRIGHTNESS, v.shadow_brightness.to_string()),
    ]);
    vk::swapchain::set_shadow_brightness(v.shadow_brightness);
    crate::radar::set_size(v.radar_size);
    crate::radar::set_terrain(v.radar_terrain);
    crate::radar_map::set_objects(v.radar_objects);
    crate::radar_layers::set_fog_size(v.radar_fog_size);
    crate::radar_layers::set_sharp_units(v.radar_sharp_units);
    crate::fog::set_smooth(v.smooth_fog);
    crate::fog::set_objects(v.fog_on_objects);
    crate::zoom::set_percent(v.zoom_out);
    crate::zoom::set_speed(v.zoom_speed);
    vk::device::set_msaa(v.msaa);
    vk::device::set_fxaa(v.fxaa);
    vk::device::set_ambient_occlusion(v.ambient_occlusion);
    vk::device::set_soft_particles(v.soft_particles);
    vk::device::set_anisotropy(v.anisotropy);
    vk::device::set_render_scale(v.render_scale);
    vk::device::set_sharpness(v.sharpening);
    pacing::set_limit(v.fps_limit);
    window::set_clip(v.lock_mouse);
    crate::audio::set_in_background(v.background_audio);
    crate::discord::set_enabled(v.discord);
    *SAVED.lock().unwrap() = Some(*v);
}

fn slider_moved(slider: usize, position: i32) {
    // Moving a slider from our own handlers could get here too, with the page locked.
    let Ok(guard) = PAGE.try_lock() else { return };
    let Some(p) = guard.as_ref() else { return };
    let Some(i) = (0..ROWS.len()).find(|&i| p.controls[i] == slider && matches!(control(i), Control::Slider { .. })) else { return };
    let values = slider_values(&p.choices, i);
    if let Some(&v) = values.get((position.max(0) as usize).min(values.len().saturating_sub(1))) {
        slider_changed(p, i, v);
    }
}

/// Jumps to the mouse on a press and follows it until release, since drags go to the window that took the press.
extern "C" fn slider_input(window: usize, msg: u32, data1: usize, data2: usize) -> u32 {
    match msg {
        GWM_LEFT_DOWN | GWM_LEFT_DRAG => {
            let x = (data1 & 0xFFFF) as u16 as i16 as i32;
            if let Some(position) = gadgets::slider_position_at(window, x, has_off_box(window))
                && gadgets::slider_position(window) != Some(position)
            {
                send(window, GSM_SET_SLIDER, position as usize, 0);
                slider_moved(window, position);
            }
            MSG_HANDLED
        }
        GWM_LEFT_UP => MSG_HANDLED,
        _ => {
            let original: extern "C" fn(usize, u32, usize, usize) -> u32 = unsafe { std::mem::transmute(addresses().slider_input) };
            original(window, msg, data1, data2)
        }
    }
}

/// The Off box's middle, drawn over a lit box so only a thin outline of it shows.
const OFF_BOX_TINT: u32 = 0xFF30_3030;

/// One box per step lit up to the position, or a bar once boxes would be too narrow.
extern "C" fn slider_draw(window: usize, instance_data: usize) {
    let Some((min, max, position)) = gadgets::slider_data(window) else { return };
    let Some(row) = gadgets::SliderLayout::new(window, max - min, has_off_box(window)) else { return };
    let [lit, unlit, frame] = gadgets::slider_images(window);
    let done = (position - min).clamp(0, row.steps);
    let gap = gadgets::SLIDER_BOX_GAP;
    let (top, bottom) = (row.top, row.top + row.size);
    let hovered = gadgets::hilited(instance_data);
    if hovered {
        // Tiled one box apart from half a box left of the row, a third of a box lower, as the game does.
        let cell = if row.boxes { (row.width + gap) / row.cells() } else { row.size + gap };
        let mut left = row.left - cell / 2;
        while left < row.left + row.width + cell / 2 {
            gadgets::draw_image(frame, left, top + row.size / 3, left + cell, top + row.size / 3 + row.size + gap);
            left += cell;
        }
    }
    if row.boxes {
        if row.off {
            let (left, right) = row.cell_edges(0);
            let edge = (row.size / 14).max(1);
            gadgets::draw_image(lit, left, top, right, bottom);
            gadgets::draw_image_tinted(unlit, left + edge, top + edge, right - edge, bottom - edge, OFF_BOX_TINT);
        }
        for i in 0..row.steps {
            let (left, right) = row.step_edges(i);
            gadgets::draw_image(if i < done { lit } else { unlit }, left, top, right, bottom);
        }
    } else {
        let split = row.left + row.width * done / row.steps;
        gadgets::draw_image(lit, row.left, top, split, bottom);
        gadgets::draw_image(unlit, split, top, row.left + row.width, bottom);
    }
    // The hidden thumb's hover image (an arrow) at the position, where the game draws the thumb.
    let thumb = gadgets::first_child(window);
    if hovered && thumb != 0 {
        let arrow = gadgets::slider_images(thumb)[2];
        let (w, h) = gadgets::size(thumb);
        let center_of = |(left, right): (i32, i32)| (left + right) / 2;
        let center = match (row.boxes, done) {
            (false, _) => row.left + row.width * done / row.steps,
            (true, 0) if row.off => center_of(row.cell_edges(0)),
            (true, 0) => row.left + w / 2,
            (true, _) => center_of(row.step_edges(done - 1)),
        };
        let y = top + gadgets::position(thumb).1;
        gadgets::draw_image(arrow, center - w / 2, y, center - w / 2 + w, y + h);
    }
}

fn has_off_box(slider: usize) -> bool {
    OFF_BOX_SLIDERS.lock().unwrap().contains(&slider)
}

/// Our input and draw. The thumb is hidden, it would take presses where our draw shows nothing.
fn install_slider(slider: usize, off: bool) {
    let (input, draw) = (gadgets::input_func(slider), gadgets::draw_func(slider));
    if slider == 0 || unsafe { *input } != addresses().slider_input || unsafe { *draw } != addresses().slider_draw {
        log!("a slider on our settings page has callbacks we don't know, it keeps the game's");
        return;
    }
    unsafe {
        *input = slider_input as *const () as usize;
        *draw = slider_draw as *const () as usize;
    }
    hide(gadgets::first_child(slider), true);
    if off {
        OFF_BOX_SLIDERS.lock().unwrap().push(slider);
    }
}

extern "C" fn options_system(window: usize, msg: u32, data1: usize, data2: usize) -> u32 {
    let original: extern "C" fn(usize, u32, usize, usize) -> u32 = unsafe { std::mem::transmute(addresses().options_menu_system) };
    if msg == GSM_SLIDER_TRACK {
        slider_moved(data1, data2 as i32);
        return original(window, msg, data1, data2);
    }
    if msg != GBM_SELECTED {
        return original(window, msg, data1, data2);
    }
    let mut accepted = None;
    {
        let mut guard = PAGE.lock().unwrap();
        if let Some(p) = guard.as_mut() {
            if let Some(tab) = p.opens.iter().position(|&w| w == data1) {
                show_tab(p, Some(tab));
                return MSG_HANDLED;
            }
            if data1 == p.ok {
                show_tab(p, None);
                return MSG_HANDLED;
            }
            if data1 == p.cancel {
                let opened_with = p.opened_with;
                show_values(p, &opened_with);
                show_tab(p, None);
                vk::swapchain::preview_shadow_brightness(Some(opened_with.shadow_brightness));
                return MSG_HANDLED;
            }
            if data1 == p.defaults {
                show_values(p, &Values::DEFAULT);
                slider_changed(p, row::SHADOW_BRIGHTNESS, Values::DEFAULT.shadow_brightness);
            }
            if data1 == p.accept {
                accepted = Some(read_values(p));
            }
            // Accept and Back (Escape too) close the whole menu.
            if data1 == p.accept || data1 == p.back {
                show_tab(p, None);
                vk::swapchain::preview_shadow_brightness(None);
            }
        }
    }
    let result = original(window, msg, data1, data2);
    if let Some(v) = accepted {
        save(&v);
    }
    result
}
