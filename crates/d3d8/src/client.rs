//! Pieces of the client update that now run every frame. Steps that assumed 30 calls per second
//! are scaled by real frame time. Counters that work as clocks (key repeat, button flashes,
//! frame-counted animations) still advance once per 30 Hz tick, while the code around them runs
//! every frame.

use std::arch::naked_asm;
use std::sync::atomic::{AtomicU8, AtomicU32, AtomicUsize, Ordering};

use crate::{frame, game, objects, things};

struct Addresses {
    /// In GameClient::update: `mov reg, [ecx]; call [reg+0x10]` on TheKeyboard and on TheMouse.
    keyboard_update_call: usize,
    mouse_update_call: usize,
    /// In GameClient::update: `mov ecx, [global]; mov reg, [ecx]; call [reg+0x10]` for the window
    /// manager (destroy list and window transitions), the shell and InGameUI.
    window_manager_update_call: usize,
    shell_update_call: usize,
    in_game_ui_update_call: usize,
    /// In GameClient::update, camera tracking of the selected unit: `add eax, 0x38; push eax;
    /// call [edx+0x4c]`, which is TheTacticalView->lookAt(drawable position). EAX is the drawable.
    /// Generals has no camera tracking.
    tracking_look_at: Option<usize>,
    the_window_manager: usize,
    the_shell: usize,
    the_in_game_ui: usize,
    the_tactical_view: usize,
    /// Shell::update runs its screens only when 32.3 ms passed since it last did, measured with
    /// timeGetTime. This is the operand of that compare. We run it on ticks instead.
    shell_timer_operand: usize,
    /// In InGameUI::update: `fsub [eax+speed]` and `fadd [ecx+speed]` (camera rotate speed) while a
    /// rotate key is held, and `call [reg+zoomIn]` and `call [reg+zoomOut]` while a zoom key is held.
    rotate_left_step: usize,
    rotate_right_step: usize,
    zoom_in_call: usize,
    zoom_out_call: usize,
    gd_camera_rotate_speed: usize,
    view_get_height_vslot: usize,
    view_set_height_vslot: usize,
    /// In InGameUI::update: `call isTimeFrozenScript`. While frozen it counts the military subtitle's
    /// timers back once per call.
    subtitle_freeze_check: usize,
    script_frozen: usize,
    /// In ControlBar::update (EBP is the control bar): the radar attack glow countdown, the scheme
    /// animation update, the two animate window managers and the command button flash countdown.
    radar_glow: usize,
    radar_glow_continue: usize,
    radar_glow_skip: usize,
    radar_glow_stub: unsafe extern "C" fn(),
    scheme_update_call: usize,
    scheme_update: usize,
    animate_update_calls: [usize; 2],
    button_flash: usize,
    button_flash_continue: usize,
    button_flash_skip: usize,
    button_flash_stub: unsafe extern "C" fn(),
    /// In drawTextEntryText: the text caret shows while bit 3 of a static byte is set, and the byte
    /// goes up by one per draw: `mov al, [cnt]; mov cl, al; and cl, 8; inc al; test cl, cl;
    /// mov [cnt], al`, then `jz` past the caret.
    caret_blink: usize,
    caret_blink_stub: unsafe extern "C" fn(),
    /// InGameUI::preDraw, which runs after the 3D scene is drawn: `call handleBuildPlacements` and
    /// `call handleRadiusCursor`. We run both before the scene instead. Generals' preDraw does the
    /// radius cursor inline, so only the placements move there.
    pre_draw_placements_call: usize,
    pre_draw_radius_cursor_call: Option<usize>,
    handle_build_placements: usize,
    handle_radius_cursor: Option<usize>,
    /// InGameUI: the array of preview drawables (more than one for walls).
    ui_place_icons: usize,
    drawable_draw: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    keyboard_update_call: 0x004A_9E18,
    mouse_update_call: 0x004A_9E3D,
    window_manager_update_call: 0x004A_9EA7,
    shell_update_call: 0x004A_A007,
    in_game_ui_update_call: 0x004A_A012,
    tracking_look_at: Some(0x004A_9E71),
    the_window_manager: 0x00A3_9E14,
    the_shell: 0x00A3_A5BC,
    the_in_game_ui: 0x00A3_9FB0,
    the_tactical_view: 0x00A3_9598,
    shell_timer_operand: 0x005C_581D,
    rotate_left_step: 0x0050_AE5D,
    rotate_right_step: 0x0050_AE9B,
    zoom_in_call: 0x0050_AECD,
    zoom_out_call: 0x0050_AEEF,
    gd_camera_rotate_speed: 0x91C,
    view_get_height_vslot: 0xFC / 4,
    view_set_height_vslot: 0x100 / 4,
    subtitle_freeze_check: 0x0050_A938,
    script_frozen: 0x0043_74D0,
    radar_glow: 0x0045_E5D2,
    radar_glow_continue: 0x0045_E5D8,
    radar_glow_skip: 0x0045_E62B,
    radar_glow_stub: radar_glow_stub_zero_hour,
    scheme_update_call: 0x0045_E632,
    scheme_update: 0x005A_6AB0,
    animate_update_calls: [0x0045_E64A, 0x0045_E70A],
    button_flash: 0x0045_E822,
    button_flash_continue: 0x0045_E828,
    button_flash_skip: 0x0045_E8DD,
    button_flash_stub: button_flash_stub_zero_hour,
    caret_blink: 0x007C_A100,
    caret_blink_stub: caret_blink_stub_zero_hour,
    pre_draw_placements_call: 0x0050_A713,
    pre_draw_radius_cursor_call: Some(0x0050_A71A),
    handle_build_placements: 0x0050_A300,
    handle_radius_cursor: Some(0x0050_A140),
    ui_place_icons: 0x174C,
    drawable_draw: 0x0047_2FD0,
};

static GENERALS: Addresses = Addresses {
    keyboard_update_call: 0x004A_A280,
    mouse_update_call: 0x004A_A2A5,
    window_manager_update_call: 0x004A_A2D6,
    shell_update_call: 0x004A_A440,
    in_game_ui_update_call: 0x004A_A44B,
    tracking_look_at: None,
    the_window_manager: 0x00A6_C19C,
    the_shell: 0x00A6_C870,
    the_in_game_ui: 0x00A6_C330,
    the_tactical_view: 0x00A6_B95C,
    shell_timer_operand: 0x005C_6CED,
    rotate_left_step: 0x0052_4A36,
    rotate_right_step: 0x0052_4A74,
    zoom_in_call: 0x0052_4AA6,
    zoom_out_call: 0x0052_4AC8,
    gd_camera_rotate_speed: 0x914,
    view_get_height_vslot: 0xF0 / 4,
    view_set_height_vslot: 0xF4 / 4,
    subtitle_freeze_check: 0x0052_4568,
    script_frozen: 0x0043_7E80,
    radar_glow: 0x0045_F591,
    radar_glow_continue: 0x0045_F597,
    radar_glow_skip: 0x0045_F5EA,
    radar_glow_stub: radar_glow_stub_generals,
    scheme_update_call: 0x0045_F5F1,
    scheme_update: 0x005B_6EE0,
    animate_update_calls: [0x0045_F609, 0x0045_F69A],
    button_flash: 0x0045_F7AF,
    button_flash_continue: 0x0045_F7B5,
    button_flash_skip: 0x0045_F86B,
    button_flash_stub: button_flash_stub_generals,
    caret_blink: 0x007A_0B60,
    caret_blink_stub: caret_blink_stub_generals,
    pre_draw_placements_call: 0x0052_42D6,
    pre_draw_radius_cursor_call: None,
    handle_build_placements: 0x0052_3EC0,
    handle_radius_cursor: None,
    ui_place_icons: 0x1748,
    drawable_draw: 0x0047_0F70,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

const VIRTUAL_CALL_LEN: usize = 5;
/// The input frame each update increments. Key repeat and click timing count it.
const KEYBOARD_INPUT_FRAME: usize = 0x160C;
const MOUSE_INPUT_FRAME: usize = 0x4A44;
const GLOBAL_CALL_LEN: usize = 11;
const TRACKING_LOOK_AT_LEN: usize = 7;
const VIEW_LOOK_AT_VSLOT: usize = 0x4C / 4;
const THING_POSITION: usize = 0x38;
const UPDATE_VSLOT: usize = 0x10 / 4;
const CARET_BLINK_LEN: usize = 19;
/// InGameUI: the thing being placed.
const UI_PENDING_PLACE: usize = 0x1740;
/// GlobalData: the most preview drawables a line of walls can have.
const GD_MAX_LINE_BUILD_OBJECTS: usize = 0x7D0;
const RAN_PLACEMENTS: u8 = 1;
const RAN_RADIUS_CURSOR: u8 = 2;

static ZERO: f32 = 0.0;
/// Which preDraw steps already ran before the scene this frame.
static RAN_EARLY: AtomicU8 = AtomicU8::new(0);
/// Camera rotate speed times this frame's length, as f32 bits. InGameUI::update reads it directly.
static ROTATE_STEP: AtomicU32 = AtomicU32::new(0);
/// Where the naked stubs continue and the caret counter, set at install.
static RADAR_GLOW_CONTINUE: AtomicUsize = AtomicUsize::new(0);
static RADAR_GLOW_SKIP: AtomicUsize = AtomicUsize::new(0);
static BUTTON_FLASH_CONTINUE: AtomicUsize = AtomicUsize::new(0);
static BUTTON_FLASH_SKIP: AtomicUsize = AtomicUsize::new(0);

pub unsafe fn install() {
    let a = addresses();
    RADAR_GLOW_CONTINUE.store(a.radar_glow_continue, Ordering::SeqCst);
    RADAR_GLOW_SKIP.store(a.radar_glow_skip, Ordering::SeqCst);
    BUTTON_FLASH_CONTINUE.store(a.button_flash_continue, Ordering::SeqCst);
    BUTTON_FLASH_SKIP.store(a.button_flash_skip, Ordering::SeqCst);
    unsafe {
        game::replace_with_call(a.keyboard_update_call, VIRTUAL_CALL_LEN, keyboard_update as *const () as usize);
        game::replace_with_call(a.mouse_update_call, VIRTUAL_CALL_LEN, mouse_update as *const () as usize);
        if let Some(at) = a.tracking_look_at {
            game::replace_with_call(at, TRACKING_LOOK_AT_LEN, tracking_look_at_stub as *const () as usize);
        }
        game::replace_with_call(a.window_manager_update_call, GLOBAL_CALL_LEN, window_manager_update as *const () as usize);
        game::replace_with_call(a.shell_update_call, GLOBAL_CALL_LEN, shell_update as *const () as usize);
        game::replace_with_call(a.in_game_ui_update_call, GLOBAL_CALL_LEN, in_game_ui_update as *const () as usize);
        game::write_u32(a.shell_timer_operand, &ZERO as *const f32 as u32);
        let [b0, b1, b2, b3] = (ROTATE_STEP.as_ptr() as u32).to_le_bytes();
        game::write_code(a.rotate_left_step, &[0xD8, 0x25, b0, b1, b2, b3]);
        game::write_code(a.rotate_right_step, &[0xD8, 0x05, b0, b1, b2, b3]);
        game::replace_with_call(a.zoom_in_call, 6, zoom_in as *const () as usize);
        game::replace_with_call(a.zoom_out_call, 6, zoom_out as *const () as usize);
        game::patch_call(a.subtitle_freeze_check, subtitle_frozen as *const () as usize);
        game::patch_jump(a.radar_glow, a.radar_glow_stub as *const () as usize);
        game::patch_call(a.scheme_update_call, scheme_update as *const () as usize);
        for at in a.animate_update_calls {
            game::replace_with_call(at, VIRTUAL_CALL_LEN, animate_update as *const () as usize);
        }
        game::patch_jump(a.button_flash, a.button_flash_stub as *const () as usize);
        game::replace_with_call(a.caret_blink, CARET_BLINK_LEN, a.caret_blink_stub as *const () as usize);
        game::patch_call(a.pre_draw_placements_call, pre_draw_placements as *const () as usize);
        if let Some(at) = a.pre_draw_radius_cursor_call {
            game::patch_call(at, pre_draw_radius_cursor as *const () as usize);
        }
    }
}

/// The building preview drawables while a building is being placed.
pub fn place_icons() -> Vec<usize> {
    let a = addresses();
    unsafe {
        let ui = *(a.the_in_game_ui as *const usize);
        let gd = game::build().map_or(0, |b| *(b.global_data as *const usize));
        if ui == 0 || gd == 0 || *((ui + UI_PENDING_PLACE) as *const usize) == 0 {
            return Vec::new();
        }
        let icons = *((ui + a.ui_place_icons) as *const *const usize);
        if icons.is_null() {
            return Vec::new();
        }
        let count = (*((gd + GD_MAX_LINE_BUILD_OBJECTS) as *const i32)).max(1) as usize;
        (0..count).map(|i| *icons.add(i)).filter(|&d| d != 0).collect()
    }
}

/// Runs before the 3D scene, after the camera update: places the building preview at this
/// frame's cursor and hands it to its models right away, since the drawable pass only runs once
/// per logic step. Also moves the radius cursor.
pub fn update_placement() {
    let a = addresses();
    let ui = unsafe { *(a.the_in_game_ui as *const usize) };
    let view = unsafe { *(a.the_tactical_view as *const usize) };
    if ui == 0 || view == 0 {
        return;
    }
    let placements: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(a.handle_build_placements) };
    placements(ui);
    let mut ran = RAN_PLACEMENTS;
    if let Some(radius) = a.handle_radius_cursor {
        let radius: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(radius) };
        radius(ui);
        ran |= RAN_RADIUS_CURSOR;
    }
    RAN_EARLY.store(ran, Ordering::Relaxed);
    let draw: extern "thiscall" fn(usize, usize) = unsafe { std::mem::transmute(a.drawable_draw) };
    let was = objects::set_live_scope(true);
    for icon in place_icons() {
        draw(icon, view);
    }
    objects::set_live_scope(was);
}


extern "thiscall" fn pre_draw_placements(ui: usize) {
    if RAN_EARLY.fetch_and(!RAN_PLACEMENTS, Ordering::Relaxed) & RAN_PLACEMENTS == 0 {
        let original: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().handle_build_placements) };
        original(ui);
    }
}

extern "thiscall" fn pre_draw_radius_cursor(ui: usize) {
    if RAN_EARLY.fetch_and(!RAN_RADIUS_CURSOR, Ordering::Relaxed) & RAN_RADIUS_CURSOR == 0
        && let Some(radius) = addresses().handle_radius_cursor
    {
        let original: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(radius) };
        original(ui);
    }
}

unsafe fn vcall(obj: usize, slot: usize) {
    unsafe {
        let f: extern "thiscall" fn(usize) = std::mem::transmute(*(*(obj as *const *const usize)).add(slot));
        f(obj)
    }
}

fn update_global(global: usize) {
    let obj = unsafe { *(global as *const usize) };
    if obj != 0 {
        unsafe { vcall(obj, UPDATE_VSLOT) };
    }
}

/// Runs the update with the input frame held between ticks, so it counts 30 per second.
fn input_update(device: usize, input_frame: usize) {
    if !frame::is_tick() {
        let counter = (device + input_frame) as *mut u32;
        unsafe { *counter = (*counter).wrapping_sub(1) };
    }
    unsafe { vcall(device, UPDATE_VSLOT) };
}

extern "thiscall" fn keyboard_update(keyboard: usize) {
    input_update(keyboard, KEYBOARD_INPUT_FRAME);
}

extern "thiscall" fn mouse_update(mouse: usize) {
    input_update(mouse, MOUSE_INPUT_FRAME);
}

/// The unit only moves once per logic step, so the camera looks at where it is drawn.
extern "C" fn tracking_look_at(view: usize, drawable: usize) {
    let position = things::blended_position(drawable, frame::blend_factor())
        .unwrap_or_else(|| unsafe { *((drawable + THING_POSITION) as *const [f32; 3]) });
    unsafe {
        let look_at: extern "thiscall" fn(usize, *const [f32; 3]) =
            std::mem::transmute(*(*(view as *const *const usize)).add(VIEW_LOOK_AT_VSLOT));
        look_at(view, &position);
    }
}

/// ECX is the view, EAX the drawable.
#[unsafe(naked)]
unsafe extern "C" fn tracking_look_at_stub() {
    naked_asm!(
        "push eax",
        "push ecx",
        "call {look_at}",
        "add esp, 8",
        "ret",
        look_at = sym tracking_look_at,
    )
}

/// Window input reaches windows through the message translators every frame. This update only
/// destroys queued windows and steps the window transitions.
extern "C" fn window_manager_update() {
    if frame::is_tick() {
        update_global(addresses().the_window_manager);
    }
}

/// The shell's screen updates and window animations step by fixed amounts per call.
extern "C" fn shell_update() {
    if frame::is_tick() {
        update_global(addresses().the_shell);
    }
}

extern "C" fn in_game_ui_update() {
    let a = addresses();
    let gd = game::build().map_or(0, |b| unsafe { *(b.global_data as *const usize) });
    if gd != 0 {
        let speed = unsafe { *((gd + a.gd_camera_rotate_speed) as *const f32) };
        ROTATE_STEP.store((speed * frame::frame_scale()).to_bits(), Ordering::Relaxed);
    }
    update_global(a.the_in_game_ui);
}

fn zoom(view: usize, direction: f32) {
    let a = addresses();
    unsafe {
        let vtable = *(view as *const *const usize);
        let get: extern "thiscall" fn(usize) -> f32 = std::mem::transmute(*vtable.add(a.view_get_height_vslot));
        let set: extern "thiscall" fn(usize, f32) = std::mem::transmute(*vtable.add(a.view_set_height_vslot));
        set(view, get(view) + direction * crate::zoom::step() * frame::frame_scale());
    }
}

/// Replaces the zoomIn call for a held zoom key. ECX is the view.
extern "thiscall" fn zoom_in(view: usize) {
    zoom(view, -1.0);
}

extern "thiscall" fn zoom_out(view: usize) {
    zoom(view, 1.0);
}

extern "thiscall" fn subtitle_frozen(scripts: usize) -> u8 {
    let original: extern "thiscall" fn(usize) -> u8 = unsafe { std::mem::transmute(addresses().script_frozen) };
    (frame::is_tick() && original(scripts) != 0) as u8
}

extern "thiscall" fn scheme_update(manager: usize) {
    if frame::is_tick() {
        let original: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().scheme_update) };
        original(manager);
    }
}

extern "thiscall" fn animate_update(manager: usize) {
    if frame::is_tick() {
        unsafe { vcall(manager, UPDATE_VSLOT) };
    }
}

/// The glow countdown steps per call, so it only runs on ticks.
macro_rules! radar_glow_stub {
    ($name:ident, $flag:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                "cmp byte ptr [{tick}], 0",
                "je 2f",
                concat!("mov al, byte ptr [ebp + ", $flag, "]"),
                "push dword ptr [{cont}]",
                "ret",
                "2:",
                "push dword ptr [{skip}]",
                "ret",
                tick = sym frame::TICK,
                cont = sym RADAR_GLOW_CONTINUE,
                skip = sym RADAR_GLOW_SKIP,
            )
        }
    };
}

radar_glow_stub!(radar_glow_stub_zero_hour, "0x320");
radar_glow_stub!(radar_glow_stub_generals, "0x278");

/// The caret counter advances on ticks only, so the caret blinks every 0.53 s as at 30 fps.
/// Leaves the flags of `test cl, cl` for the `jz` that follows, as the original did.
macro_rules! caret_blink_stub {
    ($name:ident, $counter:literal) => {
        #[unsafe(naked)]
        unsafe extern "C" fn $name() {
            naked_asm!(
                concat!("mov al, byte ptr [", $counter, "]"),
                "mov cl, al",
                "and cl, 8",
                "add al, byte ptr [{tick}]",
                concat!("mov byte ptr [", $counter, "], al"),
                "test cl, cl",
                "ret",
                tick = sym frame::TICK,
            )
        }
    };
}

caret_blink_stub!(caret_blink_stub_zero_hour, "0x00A4A198");
caret_blink_stub!(caret_blink_stub_generals, "0x00A740E8");

/// The flash counts down on client frames that are a multiple of 10, so it may only run once per
/// client frame.
#[unsafe(naked)]
unsafe extern "C" fn button_flash_stub_zero_hour() {
    naked_asm!(
        "cmp byte ptr [{tick}], 0",
        "je 2f",
        "mov al, byte ptr [ebp + 0x284]",
        "push dword ptr [{cont}]",
        "ret",
        "2:",
        "push dword ptr [{skip}]",
        "ret",
        tick = sym frame::TICK,
        cont = sym BUTTON_FLASH_CONTINUE,
        skip = sym BUTTON_FLASH_SKIP,
    )
}

/// Generals pushes EDI between the flag test and its jump, so the skip pushes it too.
#[unsafe(naked)]
unsafe extern "C" fn button_flash_stub_generals() {
    naked_asm!(
        "cmp byte ptr [{tick}], 0",
        "je 2f",
        "mov al, byte ptr [ebp + 0x1dc]",
        "push dword ptr [{cont}]",
        "ret",
        "2:",
        "push edi",
        "push dword ptr [{skip}]",
        "ret",
        tick = sym frame::TICK,
        cont = sym BUTTON_FLASH_CONTINUE,
        skip = sym BUTTON_FLASH_SKIP,
    )
}
