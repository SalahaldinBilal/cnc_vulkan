# Options Menu

Our settings page inside the game's options menu, and what it relies on. Code: `crates/d3d8/src/optionsmenu.rs` and `settings.rs`.

## The layout

- `Window\Menus\OptionsMenu.wnd` is only in `WindowZH.big` (checked with `bigtool`). Layout callbacks: `LAYOUTINIT = OptionsMenuInit`, `LAYOUTUPDATE = OptionsMenuUpdate`, `LAYOUTSHUTDOWN = OptionsMenuShutdown`. The root and `OptionsMenuParent` use `SYSTEMCALLBACK = "OptionsMenuSystem"`, `INPUTCALLBACK = "OptionsMenuInput"`.
- Block syntax: `WINDOW` ... `END`. A window with children lists them as `CHILD` followed by a `WINDOW` block each, then `ENDALLCHILDREN`, then its own `END`.
- The main page is full. Sub-pages are plain `USER` windows with `STATUS = ENABLED+HIDDEN` and `SYSTEMCALLBACK = "PassMessagesToParentSystem"`, so their gadgets' messages reach OptionsMenuSystem. `WinAdvancedDisplayOptions` (151,68 to 636,560, a child of `OptionsMenuParentOld`) is one, shown when the Detail combo picks Custom.
- Useful gadget templates: `ComboBoxDetail` (combo box), `DetailLabel` (static text), `ButtonAdvanceAccept` and `ButtonAdvanceBack` (buttons), `AdvanceOptionsLine` (separator), the static text with `TEXT = "GUI:AdvanceDisplayOptions"` (page title).
- Draw order follows definition order: windows defined later draw on top. A combo box's drop-down list belongs to the combo, so a combo must come after every sibling its list can cover. The original menu does this (ComboBoxResolution is defined after ComboBoxDetail, which sits below it). Our first page listed rows top to bottom, and each list opened behind the next row.
- A `TEXT` label goes through the game's string table. We leave `TEXT` out of our gadgets and set the text at runtime instead.

## Loading our copy

- The `.wnd` loader `0x006dbbd0` builds `Window\<name>` and opens it with FileSystem::openFile `0x0044ea30` at `0x006dbcdc`. openFile is thiscall on TheFileSystem (`0x00a3959c`), (path, access), callee pops: local file system (`0x00a39d84`, vtable `+0x18`) first, then the archives (`0x00a39d80`, vtable `+0x24`). The file is then turned into a RAM file (file vtable `+0x3c`) and read line by line.
- The shell asks for `Menus/OptionsMenu.wnd` (forward slash, string at `0x00a05138`). The loader only adds `Window\` when the name has no backslash, so the path it opens is `Window\Menus/OptionsMenu.wnd`. A first version matched only backslashes and never fired.
- Generals' loader (`0x006c4b00` and down) skips the RAM file step and reads the opened file one byte per `read` call (vtable `+0x10`). Archive files are already in memory, but a local file is a LocalFile, whose read (`0x007e9f40`) goes straight to the CRT's unbuffered `_read`. Our 1.2 MB layout took about 2.6 seconds per menu open. In Generals we call LocalFile::convertToRAMFile (`0x007ea350`, thiscall, no stack args, vtable `+0x3c` of both local file vtables `0x009afae0` and `0x009a8fa4`) on the file we open. It returns the RAM file and frees the local one.
- We redirect that one call. When the path ends in `OptionsMenu.wnd` (either slash), it opens our copy at `%LOCALAPPDATA%\cnc-vulkan\zh\OptionsMenu.wnd` (an absolute path works with the local file system). The copy is made on first use each run from `WindowZH.big`: the original text plus our page as the next sibling of `WinAdvancedDisplayOptions`, and the three tab buttons after `ComboBoxResolution` (inside `VideoParent`).
- `_open` (MSVCRT import) is only used by a patch access test (`0x007117e0`), not by the file system.

## Callbacks

- Callback name table entries are 12 bytes: name pointer, function, 0. `OptionsMenuInit` `0x00465bc0` (entry `0x009ec428`), `OptionsMenuSystem` `0x004685d0` (entry `0x009ebea0`), `OptionsMenuInput` `0x004684c0` (entry `0x009ec1d0`), `OptionsMenuShutdown` and `OptionsMenuUpdate` both `0x004ef5c0`. We point the init and system entries at wrappers.
- Init is cdecl (layout, user data). System is cdecl (window, msg, data1, data2), returns 1 when handled. For button and combo messages, data1 is the gadget window.
- OptionsMenuInit keeps window pointers in globals, for example WinAdvancedDisplayOptions at `0x00a396d4` (its key at `0x00a396d0`) and ComboBoxDetail's at `0x00a3964c`.

## The resolution list

- OptionsMenuInit picks the selected resolution from Options.ini's `Resolution` key, 800x600 when there's none. It fills the list from TheDisplay's mode count (vtable `+0x3c`) and descriptions (`+0x40`, thiscall: index, &x, &y, &bits, callee pops), selects the matching entry, and then writes that resolution into GlobalData `+0x24` and `+0x28` (Zero Hour `0x004676d8`, Generals `0x004673da`). When the saved mode isn't in the list, the fallback sets 600 into the width and keeps the old height (a bug in the game).
- Accept compares the selected mode with GlobalData and calls setDisplayMode only when they differ.
- We usually render at a resolution that isn't in Options.ini (native when there's no `Resolution` key). So the list showed 800 x 600, and GlobalData said 800x600 while we drew at native. Our init wrapper saves GlobalData's resolution before the game's init, writes it back afterwards and selects its entry in the list. Accept then only changes the mode when the player picks a different one.

## Game helpers

| What | Where |
|---|---|
| TheWindowManager | `0x00a39e14`, vtable `+0x120` winSendSystemMsg (thiscall: window, msg, data1, data2), `+0x128` winGetWindowFromId (parent, id) |
| NameKeyGenerator::nameToKey(const char*) | `0x0047b3b0`, thiscall on TheNameKeyGenerator `0x00a39848` |
| GameWindow::winHide(bool) | `0x004f9270`, thiscall. winIsHidden `0x004f92b0` |
| UnicodeString::format | `0x00420020`, cdecl (this, format, ...) with `_vsnwprintf` |
| UnicodeString release | `0x0041fa50`, thiscall on the string |
| GadgetComboBoxAddEntry | `0x005c5040` (takes the string by value) |
| GadgetComboBoxReset, SetSelectedPos, GetSelectedPos | `0x005c50d0`, `0x005c52e0`, `0x005c52b0` |
| GadgetCheckBoxSetChecked, set text | `0x005c3b30`, `0x005c3ac0` |
| GadgetStaticTextSetText | `0x005a3380` |

Gadget messages match EA's `Gadget.h` (confirmed: `0x4001`, `0x4008`, `0x400b`, `0x400c`, `0x400e`, `0x4020`, `0x4022`, `0x4023`, `0x4029`, `0x402a` in use): GGM_SET_LABEL `0x4001` (data1 = UnicodeString pointer), GBM_SELECTED `0x4008`, GSM_SLIDER_TRACK `0x400b`, GSM_SET_SLIDER `0x400c`, GSM_SLIDER_DONE `0x400e`, GCM_ADD_ENTRY `0x4020` (data1 = string pointer, data2 = color, -1 is white), GCM_DEL_ALL `0x4022`, GCM_SELECTED `0x4023`, GCM_GET_SELECTION `0x4029` (data2 = int pointer), GCM_SET_SELECTION `0x402a` (data1 = position).

OptionsMenuSystem itself only handles `0x4008` and `0x4023` (returns 0 for the rest), so slider messages reaching it pass through untouched.

### Sliders

The same in both builds (Zero Hour addresses, Generals in brackets):

- A `HORZSLIDER` window's user data (GameWindow `+0x28`, winGetUserData `0x004f9870` [`0x004f77b0`]) is its SliderData: min `+0x00`, max `+0x04`, a float `+0x08` (pixels per step, used to place the thumb), position `+0x0c`. The position is in min..max.
- GadgetSliderGetPosition `0x0046bed0` [`0x0046bcb0`] (cdecl, the window): winGetUserData, then `+0x0c`, or -1 without user data. The options menu's save reads the gamma slider with it (`0x0046bda4`).
- GSM_SET_SLIDER `0x400c` sets the position (data1 = position). OptionsMenuInit uses it for the volume, scroll speed and gamma sliders.
- Every way the position changes (arrow keys, clicking the track, dragging the thumb) stores it at `+0x0c`, then sends GSM_SLIDER_TRACK `0x400b` to the owner (GameWindow `+0x40`, winGetOwner `0x004f9960`) through winSendSystemMsg: data1 = the slider window, data2 = the new position. Nine send sites in the gadget code around `0x006d32a2` to `0x006d3f86` [`0x006bf322` to `0x006c0006`], in functions only reached through tables. GSM_SLIDER_DONE `0x400e` goes the same way with the same data, once (`0x006d37d5` [`0x006bf855`]).
- In a layout, `SLIDERDATA = MINVALUE: <n>, MAXVALUE: <n>;` sets the range. The gamma slider (`SliderGamma`, 164,244 to 372,268) is 0 to 100.
- Our page's sliders are copies of `SliderGamma`. Their owner is the page, whose `PassMessagesToParentSystem` passes the track messages up to OptionsMenuSystem, where our wrapper sees them, the same way combo messages arrive.
- The pixels per step come from GSM_SET_MIN_MAX in EA's source: (width - thumb width) / (max - min). So the thumb's width is width - step × (max - min), and the thumb sits at (position - min) × step from the slider's left edge (`0x006d3aed` [`0x006bfb6d`]).

- Our draw and input read the range from the slider data, so a page sets each slider's range there when it opens (`gadgets::set_slider_range`, 0 to the number of values - 1) instead of in the layout. The game's own pixels per step (`+0x08`) then no longer matches, which only its hidden thumb and its arrow keys use.

#### Input

- GadgetHorizontalSliderInput `0x006d3800` [`0x006bf880`], cdecl (window, msg, data1, data2), returns 1 when handled. It's installed at `0x004fc685` [`0x004fa5e5`] through winSetInputFunc `0x004f9a20` [`0x004f79f0`] (thiscall, the function, callee pops), which stores it at GameWindow `+0x1d8` (null gets the manager's default from its vtable `+0x7c`). The callback name table also lists it (`0x009ec18c` [`0x00a25e84`]).
- It switches on msg - 5 through an index table and a jump table (`0x006d3bdc` [`0x006bfc5c`]). The message numbers match EA's GameWindow.h: GWM_LEFT_DOWN 5 only returns handled, GWM_LEFT_UP 6 steps the thumb a tenth of the width toward the click (mouse x is data1's low 16 bits), GWM_LEFT_DRAG 8 only forwards GGM_LEFT_DRAG `0x4000` to the owner, GWM_MOUSE_ENTERING 0x11 and LEAVING 0x12 send GBM_MOUSE_ENTERING `0x4006` and LEAVING `0x4007`, GWM_CHAR 0x15 moves 2 steps on the arrow keys. Everything else returns 0.
- The thumb is a child window the window manager drags itself (WIN_STATUS_DRAGABLE in EA's source). The slider's system function then reads the position back from where the thumb is. So with the game's input, a press on the track only steps toward the click, and dragging only works from the thumb.
- EA's window manager (winProcessMouseEvent) makes the window whose input accepts GWM_LEFT_DOWN the grab window, sends it every GWM_LEFT_DRAG until the button is released wherever the mouse goes, and packs the mouse as x | y << 16.
- Our sliders get `optionsmenu::slider_input` in that slot after a check that it still holds the game's function. A press or a drag sets the position under the mouse (`gadgets::slider_position_at`, then GSM_SET_SLIDER), the release is swallowed, and everything else (keys, hover) goes to the game's function.

#### Drawing

- W3DGadgetHorizontalSliderImageDraw `0x007cb220` [`0x007a1e80`], cdecl (window, instance data), named in the draw function table (`0x00a1c688` [`0x00a50f78`]). winSetDrawFunc `0x004f9a50` [`0x004f7a20`] stores draw callbacks at GameWindow `+0x1e0` (null gets the manager's default, vtable `+0x74`).
- It draws no thumb (the thumb is a child window, GameWindow `+0x1f4` through winGetChild `0x004f9930` [`0x004f78e0`], whose `SLIDERTHUMB...DRAWDATA` images are `NoImage`). It draws square boxes with the window's draw data images: the lit box is disabled entry 0 (window `+0xb0`, `hilightedbox`), the unlit one disabled entry 1 (`+0xbc`, `dehilightedbox`), and on hover (instance data `+8` bit 2) hilite entry 0 (`+0x11c`, `linebox`) behind them, a third of a box lower and half a box out on each side.
- The box size is the lit image's width (image `+0x24`) × TheDisplay's width (`0x00a385e8`, vtable `+0x20`) / 800, with 2 pixels between boxes. It fits as many boxes as the window's width holds and lights those starting left of (position - min) / (max - min) of the width. So the number of boxes has nothing to do with the range and changes with the resolution: our 0 to 10 slider showed 13 boxes. Every image goes through TheWindowManager vtable `+0x140` (thiscall: image, x0, y0, x1, y1, color, -1 for none).
- Our sliders also get `optionsmenu::slider_draw` there, and their thumb is hidden, since it would still take presses where nothing is drawn. It draws one box per step, lit up to the position, the way the game draws its own (`gadgets::SliderLayout`): square boxes of the game's size, 2 pixels apart, the row from the slider's left edge (the game centers it). With too many steps for the width the boxes are squeezed to fit, and once they'd be under half their width it's a bar instead (the lit image up to the position, the unlit one after it), so a 0 to 100 slider doesn't turn into slivers. On hover it tiles the hover image one box apart from half a box left of the row, a third of a box lower, as the game does, and draws the thumb's hover image (an arrow, thumb `+0x11c`, at the thumb's size and height) on the last lit box, where the game shows its thumb. A first version stretched the hover image across the whole row as one smear and filled the slider's full height. The mouse follows the same layout: a press lights the box under it, or goes to the nearest step on a bar.
- A slider whose lowest value means off (the fourth field in `SLIDERS`) gets an Off box in front of its row of boxes: a lit box with the unlit image inset over it, tinted almost black, so only a thin outline shows. It never lights, the arrow sits on it at the lowest value, and pressing it gives that value. With the arrow on the last lit box, Off otherwise had nowhere to go and the arrow sat over the first box, reading as one step. A bar has no Off box, since its arrow marks where the fill ends and sits at the empty left end.
- W3DDisplay::drawImage `0x007400c0` hands its color to Render2D's Add_Quad (`0x0083eb60`) as the quad's color, so the color argument of TheWindowManager's draw image tints the image (`gadgets::draw_image_tinted`).

### Checkboxes

- The game's options menu uses checkboxes for its own on/off settings (`CheckAlternateMouse`, `CheckDoubleClickAttackMove`, `CheckSendDelay`), drawn with the `Active-Unchecked` and `Active-Checked` images and their text beside the box.
- The ticked state is bit 4 (WIN_STATE_SELECTED) of the window's instance data state. The instance data is at GameWindow `+0x2c` (winGetInstanceData `0x004f9860` [`0x004f77a0`]: `lea eax, [ecx + 0x2c]`), its state at `+0x08`, so the bit is at window `+0x34`. The hover bit 2 (WIN_STATE_HILITED) is in the same word.
- GadgetCheckBoxSetChecked `0x005c3b30` [`0x005c4fd0`] (cdecl: window, bool) sets or clears the bit, then sends GBM_SELECTED (`0x4008`, data1 the checkbox) to the owner. We set the bit directly (`gadgets::set_checked`), because that message would reach our OptionsMenuSystem wrapper while it holds the page's lock (Cancel, Defaults).
- Our checkboxes are copies of `CheckAlternateMouse` without its text, a 24 by 24 box at the left of the control column (420 to 444). A click toggles it through the game's own input. The value is read on Accept (`gadgets::checked`).

## Options.ini

- `Documents\<leaf>\Options.ini`, where the leaf is the registry value `UserDataLeafName` under `SOFTWARE\Electronic Arts\EA Games\Command and Conquer Generals Zero Hour` (HKLM, then HKCU, `0x004956f0`), or `Command and Conquer Generals Zero Hour Data`. The options code builds an OptionPreferences from `Options.ini` (`0x00462bd0`).
- The game keeps every key it reads and writes them all back sorted, so our keys survive its saves: `CncVulkanDisplayMode` (borderless, windowed, fullscreen), `CncVulkanMonitor`, `CncVulkanVSync` (yes, no), `CncVulkanFpsLimit`, `CncVulkanLockMouse` (yes, no, default no), `CncVulkanBackgroundAudio` (yes, no, default yes), `CncVulkanDiscord` (yes, no, default yes), `CncVulkanUILayout` (fit, stretch), `CncVulkanUIScale`, `CncVulkanSkipIntro` (yes, no, default yes), `CncVulkanAnisotropy` (0, 2, 4, 8, 16, default 16), `CncVulkanRenderScale` (percent, 25 to 200, default 100), `CncVulkanSharpening` (percent, default 0), `CncVulkanMsaa` (1, 2, 4, 8, default 1), `CncVulkanFxaa` (yes, no, default no), `CncVulkanAmbientOcclusion` (0 to 3, default 2), `CncVulkanSoftParticles` (yes, no, default yes), `CncVulkanZoomOut` (percent of the game's maximum camera height, 100 to 400, default 100), `CncVulkanZoomSpeed` (percent of the game's zoom step, 25 to 400, default 100), `CncVulkanRadarSize` (128 to 2048, default 1024), `CncVulkanRadarTerrain` (map, textures, game, default map), `CncVulkanRadarObjects` (yes, no, default yes), `CncVulkanRadarFogSize` (128 to 1024, default 512), `CncVulkanRadarSharpUnits` (yes, no, default yes), `CncVulkanSmoothFog` (yes, no, default yes), `CncVulkanFogOnObjects` (yes, no, default yes), `CncVulkanShadowBrightness` (percent, 0 to 10, default 0).
- Skipping the intro: GlobalData `+0x819` is the INI field `PlayIntro` (field table entry `0x009453d0`). The client update (`0x004a9b2b`) plays the EA logo movie while it's set, then clears it and sets `+0x81b` (after intro). With `+0x81b` set, the next client update runs the "intro done" path, which shows the legal page on some machines and opens the main menu. Its second movie only plays when the logo movie just played. We clear `+0x819` and set `+0x81b` when the game loads D3D, before the first client update. There's no `-nologo` command line flag in this build.
- Keeping the mouse in the window applies at once (`window::set_clip`). Before this setting the cursor was always clipped in borderless and windowed.
- On the menu's Accept we read our combos first, let the game save and close the menu, then write our keys. Writing before would be undone, since the game writes back the keys it loaded when the menu opened.
- Options.ini overrides `cnc-vulkan.toml` at startup. Resolution is the game's own `Resolution = W H`. Until it exists we use the monitor's.
- Only the frame limit, the mouse setting and everything on the Graphics tab (camera zoom out included) apply at once (render scale and sharpening from the next frame). The frame limit works with vsync on too, and its list includes the chosen monitor's refresh rate minus 3 (`window::monitor_refresh`), the cap for a variable refresh rate display. Display mode, monitor and vsync are used when the device is created, and the interface settings when layouts are parsed, so they take effect after a restart.

## Monitor names

The monitor list shows each monitor's friendly name with its size. Names come from the display configuration (QueryDisplayConfig, then DisplayConfigGetDeviceInfo for the source's GDI name and the target's friendly name), matched to monitors through `MONITORINFOEXW.szDevice`. Monitors without a name show as "Monitor N".

## Status

The page has three tabs, "Display and Interface", "Graphics" and "Map and Camera". Each one has its own button on the game's page, down the right of the display options (`VideoParent`, 151,69 to 387,271): "Display" next to Resolution (310,126 to 386,154, where a single "More" button was), "Graphics" next to Detail (310,180 to 386,208) and "Map" next to the Brightness label (310,214 to 386,240, above the slider at 244). They sit right of the combo boxes (to 304), so the drop-down lists don't run under them. Each opens the page with its tab's rows shown and its title and note. A button that cycled through the tabs from inside the page was found confusing and removed, and OK and Cancel are centered on the page (229 to 388 and 400 to 559). Rows sit 32 pixels apart from y 124 within their tab, so a tab holds 11 rows above the note at 480 (40 apart fit only 9, and the first tab grew to 11 with "Sound in background" and "Discord status"). Each row in `ROWS` names its tab, and row indices are named in `optionsmenu::row`.

Each row is edited with one of three controls (`optionsmenu::control`):

- **Sliders** (window `CncvSlider<row>`): rows that are an amount. Shadow brightness, sharpening and ambient occlusion have an Off box. Interface size, 3D resolution, camera zoom out and speed, minimap resolution and minimap fog resolution don't. Each steps through a list of values (`slider_values`, the saved value added when it isn't one of them), and the label shows the value in brackets (`slider_label`).
- **Checkboxes** (`CncvCheck<row>`): the on/off rows. Their labels say what ticked means, so no value is shown: "Play sound in background", "Show what you play on Discord", "Objects and trees on minimap", "Sharp minimap units and view", "Smooth fog of war edges", "Shade objects in fog of war", and the ones that already read that way (vertical sync, keep mouse in game window, skip intro movies, FXAA, soft particles). Ticked is always the feature on or our change, unticked the game's own behaviour.
- Labels run from 170 to 414 and the controls from 420 to 620 (a checkbox is 420 to 444). A static text label wraps when its text is wider than its window, and the second line is cut off, since only one fits. The labels' font measured about 7 layout units per character in game, so a label holds about 34. Two first names at 35 and 36 wrapped with labels ending at 406.
- Slider rows start at the control column's left edge like the combo boxes and checkboxes. The game centers its row of boxes, which put a short one (minimap fog resolution, 4 values) in the middle of the column.
- **Combo boxes** (`CncvCombo<row>`): choices that are categories or named tiers. Display mode, monitor, frame limit (refresh-rate values plus "No limit"), interface layout, texture filtering, multisampling and minimap terrain.

The Graphics tab's last row is "Shadow brightness", the black level lift in the present pass ([vulkan-backend.md](vulkan-backend.md)). Games call their SDR version of this "Brightness" (taken here by the game's gamma slider) and the HDR one "Black point". With no HDR here, "Shadow brightness" says what it does. It's our first slider (a copy of `SliderGamma`, 0 to 10 percent, `optionsmenu::SLIDERS` lists the slider rows and their ranges, window names `CncvSlider<row>` instead of `CncvCombo<row>`, our input callback, see Sliders). The label shows the value in brackets ("Shadow brightness [4%]", "[Off]"), updated from GSM_SLIDER_TRACK (data2) and from our input. Accept reads the slider's position from its slider data (`gadgets::slider_position`). A dropdown made trying values slow, and the slider previews every step while it's dragged.

- The value applies to the whole picture from the first move until the menu closes, whatever tab is open, and then goes back to the saved one unless Accept saved it. OK on our page keeps the changes pending until the menu's Accept. Cancel on our page goes back to what the page showed when it was opened (`Page::opened_with`, taken when it goes from hidden to a tab), so it only undoes that visit. It used to go back to the last Accept, which threw away changes an earlier visit had confirmed with OK, on every row. Previewing only on the Graphics tab felt like the change was undone on every tab switch, since the menu itself got brighter and darker.
- The calibration pattern shows while the page is open on the Graphics tab (`Page::tab`, `update_calibration`), in the empty space under the rows (170,380 to 620,472 of the layout's 800x600, scaled from the page window's screen position and size). Opening a tab, OK, Cancel, Accept and Back (Escape sends Back through OptionsMenuInput) all go through `show_tab`, and a new menu's init starts with the pattern hidden and no preview.
- The tab's note says how to set it against the pattern, kept short enough for one line of the 450 pixel wide note at Arial 14.
- Moving the slider from our own code could send the track message while our handler holds the page's lock, so the handler only tries the lock. Init also drops the old menu's page before building the new one, since the new windows can get the old addresses.

The page, its drop-downs and the monitor names work in game. The mouse setting and the Graphics tab are not tested yet. The "Low latency" row was removed with the vendor latency extensions ([vulkan-backend.md](vulkan-backend.md)), so the first tab has 10 rows.
