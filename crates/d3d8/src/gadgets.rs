//! Talking to the game's own windows: finding them by name, sending them messages, moving, sizing
//! and hiding them, and making the game's UnicodeString for their text. See
//! knowledge-base/options-menu.md for the addresses.

use std::ffi::CString;

use crate::game;

struct Addresses {
    the_window_manager: usize,
    the_name_key_generator: usize,
    name_to_key: usize,
    win_hide: usize,
    win_set_position: usize,
    win_set_size: usize,
    win_screen_position: usize,
    /// UnicodeString::format (cdecl: this, format, ...) and the release of its buffer (thiscall).
    ustring_format: usize,
    ustring_release: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    the_window_manager: 0x00A3_9E14,
    the_name_key_generator: 0x00A3_9848,
    name_to_key: 0x0047_B3B0,
    win_hide: 0x004F_9270,
    win_set_position: 0x004F_9040,
    win_set_size: 0x004F_91A0,
    win_screen_position: 0x004F_90F0,
    ustring_format: 0x0042_0020,
    ustring_release: 0x0041_FA50,
};

static GENERALS: Addresses = Addresses {
    the_window_manager: 0x00A6_C19C,
    the_name_key_generator: 0x00A6_BBF4,
    name_to_key: 0x0047_7900,
    win_hide: 0x004F_7110,
    win_set_position: 0x004F_6EC0,
    win_set_size: 0x004F_7050,
    win_screen_position: 0x004F_6F70,
    ustring_format: 0x0042_0280,
    ustring_release: 0x0041_FCB0,
};

const SEND_SYSTEM_MSG_VSLOT: usize = 0x120 / 4;
const WINDOW_FROM_ID_VSLOT: usize = 0x128 / 4;

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}

/// winSetPosition, which the control bar's stub jumps to.
pub fn win_set_position() -> usize {
    addresses().win_set_position
}

mod offsets {
    pub const SIZE: usize = 0x08;
    pub const REGION: usize = 0x10;
    pub const USER_DATA: usize = 0x28;
    /// The window's own WinInstanceData, the one draw callbacks get.
    pub const INSTANCE_DATA: usize = 0x2C;
    /// The image of entry 0 of the disabled draw data, entry 1 of it, and entry 0 of the hilite one.
    pub const DISABLED_IMAGE_0: usize = 0xB0;
    pub const DISABLED_IMAGE_1: usize = 0xBC;
    pub const HILITE_IMAGE_0: usize = 0x11C;
    pub const INPUT_FUNC: usize = 0x1D8;
    pub const DRAW_FUNC: usize = 0x1E0;
    pub const CHILD: usize = 0x1F4;
    /// In the WinInstanceData a draw callback gets: the state, bit 2 while the mouse is over it.
    pub const STATE: usize = 0x08;
    /// In a slider's user data (SliderData): its range and position.
    pub const SLIDER_MIN: usize = 0x00;
    pub const SLIDER_MAX: usize = 0x04;
    pub const SLIDER_POSITION: usize = 0x0C;
}

const DRAW_IMAGE_VSLOT: usize = 0x140 / 4;
const DISPLAY_WIDTH_VSLOT: usize = 0x20 / 4;
/// A mapped image's width in pixels.
const IMAGE_WIDTH: usize = 0x24;
/// Pixels between a slider's boxes, as the game draws them.
pub const SLIDER_BOX_GAP: i32 = 2;

pub fn input_func(window: usize) -> *mut usize {
    (window + offsets::INPUT_FUNC) as *mut usize
}

pub fn draw_func(window: usize) -> *mut usize {
    (window + offsets::DRAW_FUNC) as *mut usize
}

pub fn first_child(window: usize) -> usize {
    unsafe { *((window + offsets::CHILD) as *const usize) }
}

const STATE_HILITED: u32 = 2;
const STATE_SELECTED: u32 = 4;

pub fn hilited(instance_data: usize) -> bool {
    let state = unsafe { *((instance_data + offsets::STATE) as *const u32) };
    state & STATE_HILITED != 0
}

fn state(window: usize) -> *mut u32 {
    (window + offsets::INSTANCE_DATA + offsets::STATE) as *mut u32
}

/// Whether a checkbox is ticked.
pub fn checked(window: usize) -> bool {
    window != 0 && unsafe { *state(window) } & STATE_SELECTED != 0
}

/// Ticks a checkbox like GadgetCheckBoxSetChecked, without the GBM_SELECTED it sends.
pub fn set_checked(window: usize, on: bool) {
    if window == 0 {
        return;
    }
    unsafe {
        let s = state(window);
        *s = if on { *s | STATE_SELECTED } else { *s & !STATE_SELECTED };
    }
}

/// Sets a slider's range in its slider data. Our draw and input read it from there.
pub fn set_slider_range(window: usize, min: i32, max: i32) {
    let data = unsafe { *((window + offsets::USER_DATA) as *const usize) };
    if data != 0 {
        unsafe {
            *((data + offsets::SLIDER_MIN) as *mut i32) = min;
            *((data + offsets::SLIDER_MAX) as *mut i32) = max;
        }
    }
}

/// The images a slider is drawn with: its lit box, its unlit box and what's behind it on hover.
pub fn slider_images(window: usize) -> [usize; 3] {
    [offsets::DISABLED_IMAGE_0, offsets::DISABLED_IMAGE_1, offsets::HILITE_IMAGE_0].map(|o| unsafe { *((window + o) as *const usize) })
}

/// A slider's min, max and position.
pub fn slider_data(window: usize) -> Option<(i32, i32, i32)> {
    let data = unsafe { *((window + offsets::USER_DATA) as *const usize) };
    (data != 0).then(|| unsafe {
        (
            *((data + offsets::SLIDER_MIN) as *const i32),
            *((data + offsets::SLIDER_MAX) as *const i32),
            *((data + offsets::SLIDER_POSITION) as *const i32),
        )
    })
}

/// Where a slider's steps are drawn: a row of boxes, or a bar when boxes would be too narrow.
pub struct SliderLayout {
    pub left: i32,
    pub top: i32,
    pub width: i32,
    /// The game's box size, also the row's height.
    pub size: i32,
    pub steps: i32,
    pub boxes: bool,
    /// A box of its own in front of the steps for the lowest value. Only on a row of boxes, a
    /// bar shows the lowest value as empty.
    pub off: bool,
}

impl SliderLayout {
    /// The game's boxes from the left edge (it centers them), squeezed to fit, then a bar under half their width.
    pub fn new(window: usize, steps: i32, off: bool) -> Option<SliderLayout> {
        let (x, y) = screen_position(window);
        let (w, _) = size(window);
        let size = game_box_size(slider_images(window)[0])?;
        let cells = steps + off as i32;
        if steps <= 0 || w <= 0 {
            return None;
        }
        let natural = cells * (size + SLIDER_BOX_GAP) - SLIDER_BOX_GAP;
        let squeezed = (w - SLIDER_BOX_GAP * (cells - 1)) / cells;
        let (width, boxes) = if natural <= w { (natural, true) } else { (w, squeezed * 2 >= size) };
        let left = x;
        Some(SliderLayout { left, top: y, width, size, steps, boxes, off: off && boxes })
    }

    pub fn cells(&self) -> i32 {
        self.steps + self.off as i32
    }

    /// Cell `i`'s left and right edges on a row of boxes, the Off box first when there is one.
    pub fn cell_edges(&self, i: i32) -> (i32, i32) {
        let (span, cells) = (self.width + SLIDER_BOX_GAP, self.cells());
        (self.left + i * span / cells, self.left + (i + 1) * span / cells - SLIDER_BOX_GAP)
    }

    /// Step `i`'s box (from 0) on a row of boxes.
    pub fn step_edges(&self, i: i32) -> (i32, i32) {
        self.cell_edges(i + self.off as i32)
    }

    /// The step a press at screen x goes to: a pressed box lights (the Off box gives the lowest),
    /// a bar takes the nearest step.
    pub fn step_at(&self, x: i32) -> i32 {
        let step = if self.boxes {
            (0..self.steps).take_while(|&i| x >= self.step_edges(i).0).count() as i32
        } else {
            ((x - self.left) as f32 * self.steps as f32 / self.width as f32).round() as i32
        };
        step.clamp(0, self.steps)
    }
}

/// The game's slider box size: the box image's width times the screen width / 800.
fn game_box_size(image: usize) -> Option<i32> {
    let display = unsafe { *(game::build()?.the_display as *const usize) };
    if display == 0 || image == 0 {
        return None;
    }
    let screen_width: i32 = unsafe {
        let f: extern "thiscall" fn(usize) -> i32 = std::mem::transmute(*(*(display as *const *const usize)).add(DISPLAY_WIDTH_VSLOT));
        f(display)
    };
    let image_width = unsafe { *((image + IMAGE_WIDTH) as *const i32) };
    Some((image_width as f32 * screen_width as f32 / 800.0) as i32)
}

/// The slider position for a press at screen x.
pub fn slider_position_at(window: usize, x: i32, off: bool) -> Option<i32> {
    let (min, max, _) = slider_data(window)?;
    Some(min + SliderLayout::new(window, max - min, off)?.step_at(x))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_boxes_span_the_row() {
        let row = SliderLayout { left: 100, top: 0, width: 483, size: 40, steps: 10, boxes: true, off: false };
        assert_eq!(row.step_edges(0).0, 100);
        assert_eq!(row.step_edges(9).1, 583);
        for i in 1..10 {
            assert_eq!(row.step_edges(i).0 - row.step_edges(i - 1).1, SLIDER_BOX_GAP);
        }
        assert_eq!(row.step_at(99), 0);
        assert_eq!(row.step_at(100), 1);
        assert_eq!(row.step_at(583), 10);
        let with_off = SliderLayout { off: true, ..row };
        assert_eq!(with_off.cell_edges(0).0, 100);
        assert_eq!(with_off.step_edges(9).1, 583);
        assert_eq!(with_off.step_at(110), 0);
        assert_eq!(with_off.step_at(with_off.step_edges(0).0), 1);
        let bar = SliderLayout { boxes: false, steps: 100, ..row };
        assert_eq!(bar.step_at(100 + 483 / 2), 50);
    }
}

/// Draws a mapped image over a screen rectangle, through TheWindowManager like the gadgets do.
pub fn draw_image(image: usize, x0: i32, y0: i32, x1: i32, y1: i32) {
    draw_image_tinted(image, x0, y0, x1, y1, 0xFFFF_FFFF);
}

/// `draw_image` with an ARGB color the image is drawn in.
pub fn draw_image_tinted(image: usize, x0: i32, y0: i32, x1: i32, y1: i32, color: u32) {
    let m = manager();
    if m == 0 || image == 0 || x1 <= x0 || y1 <= y0 {
        return;
    }
    unsafe {
        let f: extern "thiscall" fn(usize, usize, i32, i32, i32, i32, u32) =
            std::mem::transmute(*(*(m as *const *const usize)).add(DRAW_IMAGE_VSLOT));
        f(m, image, x0, y0, x1, y1, color);
    }
}

/// A UnicodeString made by the game's own allocator. Released when dropped.
pub struct UString(pub usize);

impl UString {
    pub fn new(text: &str) -> UString {
        let mut s = UString(0);
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        let fmt: Vec<u16> = "%s".encode_utf16().chain([0]).collect();
        let format: unsafe extern "C" fn(*mut usize, *const u16, ...) = unsafe { std::mem::transmute(addresses().ustring_format) };
        unsafe { format(&mut s.0, fmt.as_ptr(), wide.as_ptr()) };
        s
    }

    pub fn as_msg_data(&self) -> usize {
        &self.0 as *const usize as usize
    }
}

impl Drop for UString {
    fn drop(&mut self) {
        let release: extern "thiscall" fn(*mut usize) = unsafe { std::mem::transmute(addresses().ustring_release) };
        release(&mut self.0);
    }
}

fn manager() -> usize {
    unsafe { *(addresses().the_window_manager as *const usize) }
}

pub fn send(window: usize, msg: u32, data1: usize, data2: usize) -> u32 {
    let m = manager();
    if m == 0 || window == 0 {
        return 0;
    }
    unsafe {
        let f: extern "thiscall" fn(usize, usize, u32, usize, usize) -> u32 =
            std::mem::transmute(*(*(m as *const *const usize)).add(SEND_SYSTEM_MSG_VSLOT));
        f(m, window, msg, data1, data2)
    }
}

/// A window by its full name, like `ControlBar.wnd:PowerWindow`, or 0.
pub fn find(full_name: &str) -> usize {
    let m = manager();
    let keys = unsafe { *(addresses().the_name_key_generator as *const usize) };
    let Ok(name) = CString::new(full_name) else { return 0 };
    if m == 0 || keys == 0 {
        return 0;
    }
    unsafe {
        let name_to_key: extern "thiscall" fn(usize, *const i8) -> u32 = std::mem::transmute(addresses().name_to_key);
        let id = name_to_key(keys, name.as_ptr());
        let from_id: extern "thiscall" fn(usize, usize, u32) -> usize =
            std::mem::transmute(*(*(m as *const *const usize)).add(WINDOW_FROM_ID_VSLOT));
        from_id(m, 0, id)
    }
}

pub fn hide(window: usize, hidden: bool) {
    if window != 0 {
        let f: extern "thiscall" fn(usize, u32) -> i32 = unsafe { std::mem::transmute(addresses().win_hide) };
        f(window, hidden as u32);
    }
}

/// Top-left corner on the screen, adding up the parents.
pub fn screen_position(window: usize) -> (i32, i32) {
    let (mut x, mut y) = (0, 0);
    let f: extern "thiscall" fn(usize, *mut i32, *mut i32) -> i32 = unsafe { std::mem::transmute(addresses().win_screen_position) };
    f(window, &mut x, &mut y);
    (x, y)
}

pub fn size(window: usize) -> (i32, i32) {
    unsafe { *((window + offsets::SIZE) as *const (i32, i32)) }
}

/// Top-left corner relative to the parent.
pub fn position(window: usize) -> (i32, i32) {
    unsafe { *((window + offsets::REGION) as *const (i32, i32)) }
}

/// A slider's position, as GadgetSliderGetPosition reads it.
pub fn slider_position(window: usize) -> Option<i32> {
    if window == 0 {
        return None;
    }
    let data = unsafe { *((window + offsets::USER_DATA) as *const usize) };
    (data != 0).then(|| unsafe { *((data + offsets::SLIDER_POSITION) as *const i32) })
}

pub fn set_position(window: usize, x: i32, y: i32) {
    let f: extern "thiscall" fn(usize, i32, i32) -> i32 = unsafe { std::mem::transmute(addresses().win_set_position) };
    f(window, x, y);
}

pub fn set_size(window: usize, w: i32, h: i32) {
    let f: extern "thiscall" fn(usize, i32, i32) -> i32 = unsafe { std::mem::transmute(addresses().win_set_size) };
    f(window, w, h);
}
