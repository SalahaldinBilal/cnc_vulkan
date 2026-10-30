//! W3DDisplayString::setUseHotkey always marks the text as changed, and the gadget text draw calls
//! it every frame, so every button and label rendered its glyphs into a new texture every frame.
//! We skip it when nothing changes. The hotkey letter then needs rendering on every draw like the
//! rest of the text, which the game only did on draws that rebuilt it. See knowledge-base/text.md.

use crate::{game, log};

struct Addresses {
    /// W3DDisplayString vtable entry +0x3c, setUseHotkey(bool, color), thiscall, callee pops 8.
    set_use_hotkey_entry: usize,
    set_use_hotkey: usize,
    /// Render2DSentenceClass::Render, thiscall.
    sentence_render: usize,
    /// In W3DDisplayString::draw: the hotkey letter's Render, only reached when the text's quads were
    /// rebuilt, and the text's Render, reached on every draw.
    hotkey_render_call: usize,
    text_render_call: usize,
}

static ZERO_HOUR: Addresses = Addresses {
    set_use_hotkey_entry: 0x0096_BF98,
    set_use_hotkey: 0x007D_5B20,
    sentence_render: 0x0087_63E0,
    hotkey_render_call: 0x007D_5757,
    text_render_call: 0x007D_575F,
};

static GENERALS: Addresses = Addresses {
    set_use_hotkey_entry: 0x009A_907C,
    set_use_hotkey: 0x007A_BB80,
    sentence_render: 0x0084_3770,
    hotkey_render_call: 0x007A_B7B6,
    text_render_call: 0x007A_B7BE,
};

fn addresses() -> &'static Addresses {
    game::pick(&ZERO_HOUR, Some(&GENERALS)).unwrap_or(&ZERO_HOUR)
}
const USE_HOTKEY: usize = 0x19C;
const HOTKEY_COLOR: usize = 0x1A8;
/// The sentence renderer's own hotkey parsing. draw clears the string's flag when the text has no
/// hotkey but leaves this one on, and with it on the hotkey letter is cut out of the text.
const RENDERER_PARSES_HOTKEY: usize = 0xC1;
/// The string's two Render2DSentenceClass objects: the text, and the hotkey letter.
const TEXT_RENDERER: usize = 0x14;
const HOTKEY_RENDERER: usize = 0xD4;


pub unsafe fn install() {
    if game::build().is_none() {
        return;
    }
    let a = addresses();
    if game::call_target(a.hotkey_render_call) != Some(a.sentence_render) || game::call_target(a.text_render_call) != Some(a.sentence_render) {
        log!("W3DDisplayString::draw's render calls aren't what we expect, display strings rebuild every frame as before");
        return;
    }
    unsafe {
        game::write_code(a.hotkey_render_call, &[0x90; 5]);
        game::patch_call(a.text_render_call, render_text as *const () as usize);
        game::write_u32(a.set_use_hotkey_entry, set_use_hotkey as *const () as u32);
    }
    log!("display strings only rebuild when their hotkey setting changes");
}

extern "thiscall" fn set_use_hotkey(string: usize, use_hotkey: u32, color: u32) {
    let unchanged = unsafe {
        *((string + USE_HOTKEY) as *const u8) == use_hotkey as u8
            && *((string + RENDERER_PARSES_HOTKEY) as *const u8) == use_hotkey as u8
            && *((string + HOTKEY_COLOR) as *const u32) == color
    };
    if unchanged {
        return;
    }
    let original: extern "thiscall" fn(usize, u32, u32) = unsafe { std::mem::transmute(addresses().set_use_hotkey) };
    original(string, use_hotkey, color);
}

/// The text's Render, then the hotkey letter's whenever the string shows one.
extern "thiscall" fn render_text(renderer: usize) {
    let render: extern "thiscall" fn(usize) = unsafe { std::mem::transmute(addresses().sentence_render) };
    render(renderer);
    let string = renderer - TEXT_RENDERER;
    if unsafe { *((string + USE_HOTKEY) as *const u8) } != 0 {
        render(string + HOTKEY_RENDERER);
    }
}
