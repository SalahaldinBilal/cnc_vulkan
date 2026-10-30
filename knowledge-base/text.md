# Text

How the game renders text, and the per-frame rebuild we stopped. Code: `crates/d3d8/src/text.rs`.

## Display strings

- Text is drawn through W3DDisplayString objects (EA source `W3DDisplayString.cpp`), each holding a WW3D Render2DSentence. Building the sentence rasterizes the glyphs into a new texture (64x64 or 128x128 A4R4G4B4 for menu labels) through `_Create_DX8_Texture` `0x00838870` and D3DXCreateTexture `0x007dbc6a`.
- W3DDisplayString: memory pool "W3DDisplayString" (string `0x009f2afc`, `0x1d8` bytes per object), constructor `0x007d5190`, vtable `0x0096bf5c`. Fields: `+0x19c` use hotkey (byte), `+0x1a8` hotkey color (the constructor sets `0xffffffff`). Vtable `+0x14` is notifyTextChanged, which marks the text for a rebuild at the next draw.
- `+0x3c` setUseHotkey `0x007d5b20` (thiscall, bool, color, callee pops 8) stores both fields and the renderer's hotkey flag at `+0xc1`, then always calls notifyTextChanged. The setter after it (`0x007d5b50`) compares before notifying, setUseHotkey doesn't.
- Window text DisplayString is at GameWindow `+0x28` (`0x004f9870`).

## The rebuild every frame

- The gadget text draw `0x007ca600` (static texts and buttons) calls, every frame for every text it draws: setWordWrap, setWordWrapCentered (both only notify on a real change), setUseHotkey (always notifies), getSize, setClipRegion and draw. So every label and button rendered its glyphs into a new texture every frame.
- Measured with the options menu open: about 70 new textures per frame, 11 ms of the game's own time per frame (cpu per frame line), against 4 to 6 ms with it closed. Found with the texture creator chains in the log (`0x007dbcc4 < 0x00838a20 < ... < 0x007ca756`, see [vulkan-backend.md](vulkan-backend.md)).
- Fix: setUseHotkey's vtable entry `0x0096bf98` points at a wrapper that returns when all three values it stores are already set: `+0x19c`, `+0x1a8` and the sentence renderer's hotkey parsing at `+0xc1`. setText and the other setters still rebuild when something changes.
- The first version compared only `+0x19c` and `+0x1a8`, and building names in the control bar tooltips lost their hotkey letter ("S pply Center", " arracks"). draw (EA source) clears `+0x19c` when the text has no hotkey but leaves `+0xc1` on. With `+0xc1` on, the renderer cuts the `&` letter out of the line to be drawn separately, and with `+0x19c` off nothing draws it. In the game the next setUseHotkey(false) turns `+0xc1` off again, and that was the call we skipped.
- After that fix the letters showed for one frame and vanished. W3DDisplayString::draw `0x007d5440` keeps two Render2DSentenceClass objects, the text at `+0x14` and the hotkey letter at `+0xd4`. The text's Render (`0x008763e0`, called at `0x007d575f`) runs on every draw, but the hotkey letter's (`0x007d5757`) sits inside the block that rebuilds the quads, which only runs when the text, position or color changed. The game drew the letter every frame only because setUseHotkey rebuilt everything every frame. Now the inner call is removed and the outer one goes to a function of ours that renders the text and then, while `+0x19c` is set, the hotkey letter, on every draw.
- Other display strings that rebuild for a reason: the building construction percentage (`0x00475700`, when the percentage changes).
