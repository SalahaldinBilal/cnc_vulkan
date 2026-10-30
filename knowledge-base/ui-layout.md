# UI Layout

## .wnd files

Menus, the control bar and most UI come from `.wnd` layout files. Every window has a `SCREENRECT` with an upper-left corner, a bottom-right corner and the creation resolution (800x600 for almost everything). Coordinates are absolute screen coordinates in that layout space, even for child windows.

## The parser

- `0x006dbbd0` loads a file (handles `WINDOW` blocks). `0x006db5e0` handles `STARTLAYOUTBLOCK`.
- `0x006dc7d0` parses one window block.
- Keyword table at `0x00a15680`: 8-byte entries, keyword pointer then handler. `NAME` `0x006d9840`, `STATUS` `0x006d98f0`, `STYLE` `0x006d99e0`, callbacks, `FONT` `0x006d9740`, `TEXT` `0x006da330`, `TOOLTIP` `0x006d9680`, the `*DRAWDATA` keywords all go to `0x006da860`, and more. `LAYOUTINIT` is at `0x00a157f8`.
- Tokenizer separators: `" =;\n\r\t"` at `0x00a15c3c` (pointer at `0x00a15678`) and `" ,:=\n\r\t"` at `0x00a15f44`.
- `SCREENRECT` is not in the keyword table and its name isn't stored as a string. The window parser reads it inline right after the window type.

## SCREENRECT scaling (original)

After six `sscanf("%d")` calls the values sit on the stack:

| Stack slot | Value |
|---|---|
| `esp+0x28` | left |
| `esp+0x2c` | top |
| `esp+0x30` | right |
| `esp+0x34` | bottom |
| `esp+0x50` | creation width |
| `esp+0x54` | creation height |

At `0x006dcb1c` (with 12 bytes of `sscanf` arguments still on the stack) the code calls `TheDisplay->getWidth()` (vtable `+0x20`) and `getHeight()` (`+0x24`), computes `xs = width / creationWidth` and `ys = height / creationHeight`, and converts each corner with `ftol(v * scale)`. Results end in ESI (left), EDI (top), EBP (right) and EAX (bottom), and are stored back to the same slots.

At `0x006dcb9d` the code tests EBX (the parent window). If there's a parent, `0x004f90f0` converts to parent-relative coordinates. Then width and height are computed and the window is created by `0x006dd470`.

The parent stack: top pointer at `0x00a3fa84`, storage starting at `0x00a3fa88`, 4-byte entries. Empty when the pointer equals the storage address.

`TheDisplay` pointer: `0x00a385e8`.

## Our replacement

A 5-byte jump at `0x006dcb1c` goes to `ui::screen_rect_stub`. The stub pops the 12 bytes, calls `transform` with a pointer to `esp+0x28`, loads the four results into ESI, EDI, EBP and EAX, and returns to `0x006dcb9d`.

`transform`:

- Scale = the largest uniform factor that fits (`min(width / 800, height / 600)`) times `ui.scale`.
- A window covering the whole layout (within 2 pixels of every edge) stretches to the screen like before. Backgrounds keep filling the screen.
- Any other window whose ancestors all stretched picks an anchor per axis by which third of the layout its center falls in (see the anchor rule section below).
- Children use the anchor of their outermost anchored ancestor, so groups stay together.
- Depth comes from the parent stack. Anchors are kept per depth while parsing.

Status: built and deployed, waiting for the first test.

## First test result

Main menu and Options are no longer stretched. Two problems showed up:

- The main menu buttons and logo sit centered instead of near the right edge of the frame, because they don't touch the layout's right edge within 2 pixels. Anchoring by which third of the layout a group's center falls in would fix it.
- The control bar's art still stretches across the full width while its buttons sit in the centered area, so the slots don't line up. The art comes from the control bar scheme, below.

## Control bar scheme

`Data\INI\ControlBarScheme.ini` (loaded by `0x005a6a00`) defines the control bar art per faction. INI field table at `0x00950d68`, 16-byte entries:

| Field | Parser | Offset |
|---|---|---|
| `ImagePart` | `0x005a63e0` | block |
| `AnimatingPart` | `0x005a6500` | block |
| `ScreenCreationRes` | `0x0041ce20` (2 ints) | `+0x04` |
| `Side` | `0x0041b660` | `+0x0c` |
| `QueueButtonImage`, `RightHUDImage` | `0x0041c0c0` | `+0x10`, `+0x14` |
| colors (`BuildUpClockColor` and others) | `0x0041c840` | `+0x18` to `+0x2c` |
| `OptionsButtonEnable`, `OptionsButtonHightlited` | `0x0041c0c0` | `+0x30`, `+0x34` |

`ImagePart` table at `0x0094d040`: `Position` (2 ints, `+0x04`), `Size` (2 ints, `+0x0c`), `ImageName` (`+0x14`). `LinePart` has `StartPosition`, `EndPosition`, `Color`.

Each scheme keeps 6 image layer lists at `+0x158` (`0x005a6920` walks them to preload textures through `TheDisplay` vtable `+0xc4`).

### Scheme manager

- `0x005a6ba0` and `0x005a6e00` pick the scheme for a side and compute the multipliers: float x at manager `+0x04` = display width / `ScreenCreationRes.x`, float y at `+0x08` = display height / `ScreenCreationRes.y` (code at `0x005a6d3d`). Same separate x and y stretch as the old `.wnd` math.
- The manager is reached from `TheControlBar` (`0x00a395f8`), called by `0x004607a0` and `0x00460430`.
- `0x005a5350` applies a scheme. It computes its own local multipliers (display size / creation res) and moves and resizes these `ControlBar.wnd` windows to the scheme's coordinates: `PopupCommunicator`, `ButtonIdleWorker`, `ExpBarForeground`, `ButtonOptions`, `ButtonPlaceBeacon`, `MoneyDisplay`, `PowerWindow`, `ButtonGeneral`, `ButtonLarge`, `WinUAttack`, and `GeneralsExpPoints.wnd:GenExpParent`. For each one: `x * multiplier`, minus the parent's screen position if it has a parent (`0x004f90f0`), then `setPosition` (`0x004f9040`) and `setSize` (`0x004f91a0`).

### How the art is drawn

`.wnd` files name draw callbacks by string. The table at about `0x00a1c79c` maps names to functions (12-byte entries):

| Callback | Function |
|---|---|
| `W3DCommandBarBackgroundDraw` | `0x007c8a10` |
| `W3DCommandBarForegroundDraw` | `0x007c8b60` |
| `W3DCommandBarTopDraw` | `0x007c89c0` |
| `W3DCommandBarGridDraw`, `W3DCommandBarHelpPopupDraw`, `W3DCommandBarGenExpDraw` | not looked up yet |

The background and foreground callbacks find the `ControlBar.wnd:BackgroundMarker` window, take its current screen position minus the marker position the control bar saved at setup (`0x00461710` for background, `0x004616f0` for foreground), and pass that slide offset to the scheme manager:

- `0x005a6b70` (background) and `0x005a6b40` (foreground): thiscall on the manager, two int arguments (offset x, y). They push offset y, offset x, multiplier y, multiplier x and call the current scheme's draw (`0x005a6110` background, `0x005a6000` foreground, thiscall, callee pops 16 bytes).
- So each image is drawn at `position * multiplier + slide offset`.

Searching for the layer offset `+0x158` led to unrelated camera code instead (see [camera.md](camera.md)).

### Our fix

- The two manager draw methods are replaced by stubs that pass the uniform scale as both multipliers and add the canvas offset (centered horizontally, bottom aligned) to the slide offset.
- The scheme setup's `getWidth` and `getHeight` calls (returning to `0x005a5430` and `0x005a5454`, with the scheme in ESI) get the canvas size from a `TheDisplay` vtable hook (slots 8 and 9) that checks the return address. Every other caller gets the real size.
- The 9 `setPosition` calls inside the scheme setup (`0x005a558f`, `0x005a56fd`, `0x005a58a6`, `0x005a5a14`, `0x005a5af6`, `0x005a5bd8`, `0x005a5d52`, `0x005a5e34`, `0x005a5f36`) go through a stub that adds the canvas offset.
- `ControlBarParent` in `ControlBar.wnd` is `(0, 416)` to `(799, 599)`, which the `.wnd` rule anchors center-bottom too, so windows and art share one canvas.

### Second test result

Main menu, options, the score screen, the general's promotion window and the control bar all line up. Fonts look right at high resolutions without changes.

Open issue: the unit info text still renders at the left, at the old stretched position, where its box no longer is. Probably drawn by one of the callbacks not handled yet (`W3DCommandBarHelpPopupDraw` or similar) or by code that places text with the old multipliers.

## Wide control bar

- Each faction's bar is one ImagePart covering the 800 wide layout, layer 4: `InGameUIAmericaBase`, `InGameUIChinaBase`, `InGameUIGLABase`. The definitions in `MappedImages\HandCreated` are the ones in use: `SACommandBar.tga`, `SNCommandBar.tga`, `SUCommandBar.tga` (1024x256, rows 64, 71 and 55 down to 255). The `TextureSize_512` definitions name textures that aren't in the archives. Every bar's texture rows sit 344 above its canvas rows.
- The art has three blocks: radar housing on the left (to about x 185), the top rail with the money housing plus the command panel in the middle, and the portrait panel on the right (from about x 610). Plain columns that can be repeated, checked in all three textures: top rail at x 280 and 515, command panel at x 240 and 590. The rail band ends and the panel's top border starts at canvas y 482 (texture row 138).
- `PowerWindow` (261,473 to 544,480) is the green bar on the rail. It's a progress bar drawn relative to its own width.
- W3DDisplay::drawImage `0x007400c0` (thiscall: image, x0, y0, x1, y1, color, mode) reads the image's UV rectangle at image `+0x14` (four floats) and its texture size at `+0x0c`. The scheme's background draw `0x005a6110` walks the layer lists from scheme `+0x16c` back three slots and draws each part through display vtable `+0x94`.
- Our version: the bar fills the screen width. While our scheme draw stub runs, our drawImage wrapper (the display vtable entry `0x00967a4c`, also inside transitions.rs' wrapper) replaces the bar image with five pieces per band: the left block moved to the screen's left edge, a 4 texel strip stretched over the gap, the middle, another stretched strip, and the right block at the right edge. It changes the image's UV rectangle per piece and puts it back after.
- Windows use the same mapping (`controlbar::wide_span`), chosen by band from the window's vertical center: a window crossing both seams of its band stretches (ControlBarParent, the panel containers, `PowerWindow`), anything else moves with its center (radar and small buttons left, portrait, queue and general button right, command grid and money in place). ControlBar.wnd windows get it while the file parses (ui.rs knows the file from the `.wnd` open hook). The 10 bar windows the scheme setup places get it right after the setup (`0x005a5350`, wrapped at its three calls `0x005a6dcc`, `0x005a6f93`, `0x005a70f9`): screen rectangle to canvas, mapped, then setPosition and setSize. The setup also places `GeneralsExpPoints.wnd:GenExpParent`, the general's promotion popup. A first version remapped it too, which stretched its frame while its text and icons stayed at the old width on the left. It's left alone now.
- First game test of the wide bar (China): bar, seams, green bar and buttons looked right.
- ExpBarForeground (the general's exp frame) is not moved by the scheme setup, only given its image. Remapping it after the setup moved it twice, off the panel.
- Selected unit grid: `WinUnitSelected` draws it with W3DCommandBarGridDraw `0x007c8430`, lines at `trunc(pos + size * 0.33)` and `trunc(pos + size * 0.66)` (doubles at `0x0096be30` and `0x0096be28`), through display vtable `+0x80` (lines). In the 800x600 layout the cameo (`CameoWindow`, two by two cells) and `UnitUpgrade1` to `5` each sit one pixel inside those lines. Scaled and rounded one window at a time they drift a few pixels off the lines, so after each scheme setup we place them again from the lines: columns `[pos+1, l1)`, `[l1+1, l2)`, `[l2+1, pos+size)`, same for rows.

## Reading .wnd files

The `.wnd` files are plain text inside `WindowZH.big`. A text search on the archive (for example `NAME = "MainMenu.wnd:` with a few lines of context) shows each window's `SCREENRECT` without extracting anything.

Main menu layout: `MainMenuParent` and `MainMenuRuler` cover `(0, 0)` to `(800, 600)`. The button panel `MapBorder` is `(532, 108)` to `(756, 360)` with buttons at x 540 to 748.

### Anchor rule, second version

Groups are now anchored by which third of the layout their center falls in (left third to the left edge, right third to the right edge, middle centered), per axis. The first version (touching an edge within 2 pixels) centered the main menu panel because it doesn't touch the right edge.

### Contained siblings

Base Generals' `ScoreScreen.wnd` puts the column headers, header icons (`Cameo...`), chat and divider line next to the backdrop panel (an unnamed window at `37,29` to `767,546`) as direct children of the full-screen `ParentScoreScreen`. Zero Hour's has them inside `MainBackdrop`. Anchored by thirds one at a time, the left-third headers slid left and the right-third ones right, off their columns. So a window with no anchored ancestor that lies entirely inside an earlier anchored sibling (layout coordinates, same parent) takes that sibling's anchor. The bottom buttons are outside the backdrop and keep their own anchors in both games. Both games' `ControlBar.wnd` have a single top-level window, so the rule doesn't touch the control bar.

### Layouts over a movie

The campaign briefing (`SinglePlayerLoadScreen.wnd`, both games) and Zero Hour's `ChallengeLoadScreen.wnd` play their movie through the root window's video buffer (`m_loadScreen->winGetInstanceData()->setVideoBuffer`, EA `LoadScreen.cpp`). The root covers the whole layout, so the movie stretches to the screen. The cameo pictures, their frame and the location inset are part of the movie, while the labels (`StaticTextCameoText0` to `3`), `WindowHead` and the objectives are windows on top. Anchored by thirds, they left the pictures they belong to: "Fuel Air Bomb" (`StaticTextCameoText1`, 544 to 642) sat right-anchored at x 1944 on a 2558 wide screen, under the picture's right edge instead of below it at 1741. Every window in these two layouts keeps the game's own stretched mapping. The game shows and hides the labels at fixed movie frames and never moves them (the cameo slide code is commented out).

## Not covered yet

- Fonts. `.wnd` fonts aren't scaled by the layout. Zero Hour scales font size with resolution using `ResolutionFontAdjustment`.
- The control bar's images come from `ControlBarScheme` INI entries with `ScreenCreationRes` (string at `0x00a0464c`), not only from `.wnd`.
- UI placed by code instead of `.wnd` (tooltips, in-game messages, the radar and others) hasn't been checked.
