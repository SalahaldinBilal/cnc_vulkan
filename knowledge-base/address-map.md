# Address Map

Zero Hour 1.04, EA app `game.dat`.

Addresses for the per-system smoothing (things, particles, treads, trees, water, InGameUI) are listed in [smoothness.md](smoothness.md) next to their explanations. The byte checks for all of them are in `crates/d3d8/src/game.rs`.

## Code

| Address | What |
|---|---|
| `0x00401000` | Window procedure |
| `0x00401590` | Command line tokenizer used by WinMain |
| `0x004016f0` | WinMain |
| `0x0040e0d0` | Message stream: propagate messages |
| `0x0040e7e0` | Engine init, registers subsystems by name |
| `0x0040ff30` | Base engine update (subsystems, then the logic step) |
| `0x0040ffa5` | Logic step tail jump |
| `0x0040ffc0` | Engine main loop (execute) |
| `0x0041000f` | Catch handler, "Uncaught Exception in GameEngine::update" |
| `0x004a4590` | GameLogic: is paused (`+0xa4`) |
| `0x00741c90` | Windows engine update (base update plus minimized handling) |
| `0x00741c94` | Its call to the base update (we redirect it) |
| `0x00741820` | Engine factory: create game client |
| `0x00774670` | W3DGameClient constructor |
| `0x00774690` | Client factory: create display |
| `0x004a9b00` | Game client update |
| `0x0073be40` | W3DDisplay constructor |
| `0x00421dc0` | Display update (copyright overlay timing) |
| `0x0073dde0` | W3DDisplay::draw |
| `0x0073cb50` | W3DDisplay fps measurement |
| `0x0073e07f` | Draw: updateViews and particle update (we replace it) |
| `0x0073e4a5` | Draw: frozen-time loop check (we replace it) |
| `0x004218a0` | Display updateViews |
| `0x0083d000` | WW3D::Sync (sets sync time) |
| `0x007a8be0` | In-game UI factory: create view |
| `0x0076b990` | W3DView constructor |
| `0x0076cd20` | W3DView init |
| `0x0076d030` | W3DView camera update (per draw) |
| `0x0076cb00` | W3DView setCameraTransform |
| `0x0076c060` | W3DView build camera matrix |
| `0x0076cf80` | W3DView scripted camera path step |
| `0x008553e0` | WW3D CameraClass constructor |
| `0x00855bc0` | CameraClass Set_Transform |
| `0x00850e80` | RenderObjClass Set_Transform (we replace it) |
| `0x00850ae0` | RenderObjClass constructor |
| `0x0076dac0` | Drawable pass callback, calls Drawable::draw |
| `0x0076da9b` | `push` of that callback in the camera update (we change it) |
| `0x00472fd0` | Drawable::draw |
| `0x006202b0` | RenderObjClass Control_Bone stub (`ret 0xc`) |
| `0x007c6300` | RenderObjClass Capture_Bone and Release_Bone stub (`ret 4`) |
| `0x0076a820` | Tire track setup, reads `TREADFX01` and `TREADFX02` |
| `0x00967e74` | RenderObjClass vtable (bone slots `+0xb0` to `+0xc8`) |
| `0x007cdb90` | `W3DGadgetPushButtonImageDraw` |
| `0x007cdc30`, `0x007ce1a0` | Push button draws that draw the build clock |
| `0x007cdec3`, `0x007ce5d8`, `0x007cd9d7` | Build clock request cleared after drawing |
| `0x0040ff49`, `0x0040ff54` | GameEngine::update: client update and message propagation calls (we turn them off on our ticks) |
| `0x0040ffa5` | Logic step tail jump (we replace it) |
| `0x0073df9f` | W3DDisplay::draw clock advance (we replace 27 bytes) |
| `0x0076d083`, `0x0076d711` | W3DView::update unit follow block start and end |
| `0x0076d731` | W3DView::update call to updateCameraMovements `0x0076cf80` |
| `0x0076d781`, `0x0076d7d3` | W3DView::update shake block start and end |
| `0x0076d9e4`, `0x0076da14` | W3DView::update zoom easing `fmul [GlobalData+0x7e4]` |
| `0x0076da84` | W3DView::update drawable pass sync check |
| `0x0076e980` | W3DView::setHeightAboveGround (entry `0x00968c58`, we wrap it). See [camera.md](camera.md) |
| `0x0044db3c`, `0x0044db6c` | View::zoomIn and zoomOut step reads `[0x00945ec8]`, we point them at our step |
| `0x0077b160` | W3DRadar::buildTerrainTexture, its calls `0x0077b155` and `0x0077bc41` go to our wrapper. See [radar.md](radar.md) |
| `0x0077b1fc` | buildTerrainTexture's Get_Surface_Level call, goes to our wrapper |
| `0x00496c50`, `0x00496cd0` | Radar::radarToWorld2D and radarToWorld, we change their cell limit immediates during our build |
| `0x0073e194` | W3DDisplay::draw's Begin_Render call, goes to ours (the radar map is drawn first) |
| `0x0079ce40` | HeightMapRenderObjClass::Render (vtable `0x00969978` slot 9) |
| `0x00839960`, `0x0083ca10`, `0x0083cfb0`, `0x0083cf70` | Set_Render_Target, Begin_Render, End_Render, the WW3D flush, all cdecl |
| `0x008553e0`, `0x00856830`, `0x00856d30`, `0x00858080` | CameraClass constructor, Apply, Set_Clip_Planes; RenderInfoClass constructor |
| `0x0079b710` | HeightMapRenderObjClass::oversizeTerrain (entry `0x00969b68`, we wrap it). See [camera.md](camera.md) |
| `0x0076714a` | renderOneObject's ghost object block, we replace it to give ghosts at a fog edge the fog pass. updateFixedLightEnvironments `0x00767500`. See [radar.md](radar.md) |
| `0x0076737b` | renderOneObject's shroud status compare, we replace it to give objects reaching into the fog the fog pass. W3DShroud::setShroudLevel `0x00769740`. See [radar.md](radar.md) |
| `0x0076cc1d`, `0x00968cd0` | calcCameraConstraints (`0x0076c810`): its call in setCameraTransform and the view vtable entry (thunk `0x0076bc60`), we wrap both to keep the camera's reach when zoomed out. See [camera.md](camera.md) |
| `0x0079d587`, `0x0079854c` | Calls of W3DRoadBuffer::drawRoads (`0x00795d00`, `ret 0x24`) in the terrain renders, we wrap them to clip roads to the terrain. See [camera.md](camera.md) |
| `0x0049fb70` | GameLogic::startNewGame (map object loop from `0x004a17c3`). TheGameLogic `0x00a39b0c`, `+0x51` loading a map, `+0x3c` frame |
| `0x00a488d0`, `0x00743880` | TheMapObjectListPtr, MapObject::getThingTemplate. See [radar.md](radar.md) |
| `0x004adc00`, `0x005442b0`, `0x00476700` | ThingFactory::newDrawable (TheThingFactory `0x00a39b24`), Thing::setOrientation, Drawable::setPosition |
| `0x004aa430`, `0x00476ca0` | GameClient::setTimeOfDay, Drawable::setTimeOfDay. TheGameClient `0x00a39b18`, vtable `+0x4c` destroyDrawable |
| `0x00767e30`, `0x00766d30`, `0x009687b4` | RTS3DScene::Customized_Render, renderOneObject, vtable. DoTrees `0x0074fc10` |
| `0x00a48b40`, `0x009682cc`, `0x00758a60` | TheWaterRenderObj, its RenderObjClass vtable (at `+4`) and Render. See [radar.md](radar.md) |
| `0x0076bc50`, `0x00750440` | CameraClass Get_User_Data (vtable `+0x150`) and Set_User_Data, field `+0x8c` (the scene, read by the terrain's Render). See [radar.md](radar.md) |
| `0x009692e8`, `0x009692ec` | W3DRadar clearShroud and setShroudLevel entries, we wrap them |
| `0x0077baa5` | W3DRadar::draw overlay redraw check, replaced with a call to us |
| `0x0077a4a0` | W3DRadar::renderObjectList, its 1/128 operands, 0x80 limits and DrawPixel calls we patch |
| `0x0077bc1a` | W3DRadar::draw's drawViewBox call, goes to ours |
| `0x0074e4e0` | Shroud shader setup, reads W3DShroud's texture at `+0x28` |
| `0x0096a268` | W3DTerrainVisual getTerrainColorAt entry (`0x007ab2c0`), we wrap it |
| `0x00747810` | WorldHeightMap::getTerrainColorAt, the layout we read |
| `0x007d5b20` | W3DDisplayString::setUseHotkey (entry `0x0096bf98`, we wrap it). See [text.md](text.md) |
| `0x007ca600` | Gadget text draw, sets up and draws a window's text every frame |
| `0x00747540` | WorldHeightMap::setDrawOrg. Its DrawEntireTerrain check `0x0074757e` we make always skip |
| `0x00743b70` | WorldHeightMap constructor. The GlobalData load before its DrawEntireTerrain read `0x0074414c` we point at our flag |
| `0x0076cba5` | setCameraTransform's GlobalData load before its DrawEntireTerrain read, we point at our flag |
| `0x007dbc6a` | D3DXCreateTexture (static D3DX), calls device vtable `+0x50` at `0x007dbcc1` |
| `0x00838870` | DX8Wrapper::_Create_DX8_Texture (width, height, format, mip levels, pool, render target) |
| `0x00413f30`, `0x004144e0` | Game memory pool allocation, and the zeroing allocate that calls it |
| `0x0076e6d0` | W3DView::scrollBy (vtable entry `0x00968bac`) |
| `0x0076ddb0` | W3DView::draw, called with `this` at the view's second base `+0x7c` (entry `0x00968b54`, we point it at our wrapper). See [scene.md](scene.md) |
| `0x0076dda0` | W3DView::drawView (view vtable `+0x148`): `add ecx, 0x7c; jmp [second vtable + 0x14]` |
| `0x0076dad0` | W3DView getAxisAlignedViewRegion, thiscall, one stack argument. Its call at `0x0076e4a7` starts the icon and text pass, we redirect it |
| `0x00767440` | RTS3DScene::Flush, its calls listed in [scene.md](scene.md). We redirect its DoTrees call `0x00767480` and SortingRendererClass::Flush call `0x007674e0` |
| `0x0074fc10` | DoTrees (terrain tree render, cdecl `rinfo`) |
| `0x00778570` | DoParticles (cdecl `rinfo`, calls TheParticleSystemManager vtable `+0x20`) |
| `0x00421870` | Display::drawViews (W3DDisplay vtable `+0x5c`): calls each view's `+0x148` |
| `0x0083ca10` | WW3D::Begin_Render (W3DDisplay::draw calls it with `(1, 1)`) |
| `0x0044db30` | View::zoomIn |
| `0x004a9e18`, `0x004a9e3d` | GameClient::update keyboard and mouse update calls |
| `0x004a9e71` | GameClient::update camera tracking lookAt |
| `0x004a9ea7`, `0x004aa007`, `0x004aa012` | GameClient::update window manager, shell, InGameUI update calls |
| `0x0040a620` | Keyboard::update (input frame `+0x160c`) |
| `0x00403490` | Mouse::update (input frame `+0x4a44`) |
| `0x005c57e0` | Shell::update, timer compare at `0x005c581b` |
| `0x0050a730` | InGameUI::update |
| `0x005108e0` | InGameUI floating text update (logic frame based) |
| `0x0050ae5d`, `0x0050ae9b` | InGameUI::update rotate key steps |
| `0x0050aecd`, `0x0050aeef` | InGameUI::update zoom key calls |
| `0x0050a938` | InGameUI::update military subtitle freeze check |
| `0x0045e5b0` | ControlBar::update |
| `0x0045e5d2`, `0x0045e632`, `0x0045e64a`, `0x0045e70a`, `0x0045e822` | ControlBar::update radar glow, scheme update call, animate managers, button flash |
| `0x007ca100` (19 bytes), `0x00a4a198` | drawTextEntryText (`0x007c9dc0`) caret blink: the static byte `drawCnt` goes up per draw and the caret shows while bit 3 is set. Reached from W3DGadgetTextEntryDraw `0x007c9bc0` and W3DGadgetTextEntryImageDraw `0x007ca170` (function lexicon entries at `0x00a1c6dc`, `0x00a1c6e8`) |
| `0x005a6ab0` | Control bar scheme manager update (steps scheme animations) |
| `0x00513570` | Constructor of `0x00a39fb8` (probably EVA) |
| `0x00410097` | Main loop fragment with the frame limiter |
| `0x00437550` | Probably "script time is fast", checked by the limiter |
| `0x004f4840` | Command line flag parser |
| `0x004f43c0` | `-win` handler |
| `0x004f4440` | `-xres` handler |
| `0x004f4480` | `-yres` handler |
| `0x004f90f0` | Gets a window's screen position (used for parent-relative rects) |
| `0x004616f0`, `0x00461710` | Control bar: saved background marker position (foreground, background) |
| `0x004f9040` | Window setPosition (thiscall, x, y) |
| `0x004f91a0` | Window setSize (thiscall, w, h) |
| `0x005a5350` | Control bar scheme setup, places scheme-driven windows |
| `0x005a6000`, `0x005a6110` | Control bar scheme draw foreground, background |
| `0x005a6a00` | Loads `ControlBarScheme.ini` |
| `0x005a6b40`, `0x005a6b70` | Scheme manager drawForeground, drawBackground (we replace these) |
| `0x005a6ba0`, `0x005a6e00` | Scheme manager: pick scheme for a side, compute multipliers |
| `0x007c8a10`, `0x007c8b60`, `0x007c89c0` | `W3DCommandBarBackgroundDraw`, `ForegroundDraw`, `TopDraw` |
| `0x006db5e0` | `.wnd` `STARTLAYOUTBLOCK` handling |
| `0x006dbbd0` | `.wnd` file loader |
| `0x006dc7d0` | `.wnd` window block parser |
| `0x006dcb1c` | SCREENRECT scaling block start (we patch a jump here) |
| `0x006dcb9d` | Right after the scaling block |
| `0x006dd470` | Creates a window from parsed `.wnd` data |
| `0x0073b9a0` | Queues a mouse event from a window message |
| `0x0073c09b`, `0x0073c128` | Display mode list aspect checks |
| `0x0076f7d0` | Camera: start a move to a position (likely "move camera to") |
| `0x00770050` | Camera: facing angle per path waypoint |
| `0x007702e0` | Camera: shift all path waypoints (inside an unmarked function) |
| `0x00770c10` | Camera: path setup at move start |
| `0x00771580` | Camera: finish or advance a move |
| `0x007ac1c0` | DirectInput 8 setup |
| `0x007f8b00` | Game's unhandled exception filter |
| `0x00835730` | D3D setup, `LoadLibraryA("D3D8.DLL")` |
| `0x0084f630` | Texture peek, crashed with a null texture during fullscreen alt-tab recovery |
| `0x0087b680` | GPU detection by driver DLL name |
| `0x00752280` | getHeightMapHeight |
| `0x00754cb0` | getStaticDiffuse(x, y), thiscall. renderExtraBlendTiles (`0x0079d9c0`) calls it at `0x0079dc70`, `0x0079dcdd`, `0x0079dd50`, `0x0079ddaa`, which we cache ([performance.md](performance.md)) |
| `0x0079b3f0` | HeightMapRenderObjClass::updateBlock, virtual (entry `0x00969b74`), which we count |

## Data

| Address | What |
|---|---|
| `0x009441c0` | INI entry `Windowed` |
| `0x00944220` | INI entry `UseFPSLimit` |
| `0x00944240` | INI entry `FramesPerSecondLimit` |
| `0x00945a20` | INI entry `ResolutionFontAdjustment` |
| `0x00968bb0`, `0x00968bd4` | Tactical view vtable entries for the camera functions |
| `0x00967ab0` | float 1.334, mode list upper aspect bound |
| `0x00967ab4` | float 1.332, mode list lower aspect bound |
| `0x009ee4c0` | Command line flag table |
| `0x00a15680` | `.wnd` keyword table |
| `0x00950d68` | `ControlBarScheme` INI field table |
| `0x0094d040` | `ImagePart` INI field table |
| `0x00a1c79c` | `.wnd` draw callback name table (around here) |
| `0x00a35a28` | Game window handle |
| `0x00a395f8` | `TheControlBar` pointer |
| `0x00a35a2c` | WinMain's own `-win` flag |
| `0x00944004` | Base engine vtable |
| `0x00967b14` | Windows engine vtable |
| `0x00969064` | W3DGameClient vtable (`+0x10` update, `+0x5c` frame, `+0x7c` create display) |
| `0x009679b8` | W3DDisplay vtable (`+0x10` update, `+0x14` draw, `+0x60` updateViews). We point the draw entry `0x009679cc` at our wrapper |
| `0x00968b58`, `0x00968b40` | W3DView vtables. The constructor `0x0076bcb0` writes the second one at `+0x7c`. Its `+0x10` is the camera update `0x0076d030`, `+0x14` is draw `0x0076ddb0` |
| `0x00973288` | WW3D CameraClass vtable (`+0x4c` Set_Transform) |
| `0x00a48b30` | Terrain render object |
| `0x00a39ccc` | Particle system manager |
| `0x00a1ad3c` | Client frame seen by the last draw |
| `0x009e5f14` | Client frame seen by the last client update (drawable update freeze check) |
| `0x00a1c558` | W3DView::update static follow factor |
| `0x00a3a5b4`, `0x00a3a5b8` | Shell::update static last run time and its init flag |
| `0x00945ec8` | 10.0, View::zoomIn and zoomOut height step |
| `0x00a39e14` | `TheWindowManager` |
| `0x00a3a5bc` | `TheShell` |
| `0x00a37a90` | `TheKeyboard` |
| `0x00a39fb0` | `TheInGameUI` |
| `0x00a404b8` | Render clock in ms, advanced 33 ms per unfrozen draw |
| `0x00a1c554` | Frame length added to the render clock |
| `0x00a71d9c`, `0x00a71da0` | WW3D current and previous sync time |
| `0x00a71cf0` | The D3D8 device |
| `0x00a37a8c` | `TheAudio` (probably) |
| `0x00a37a94` | Message stream |
| `0x00a381bc` | `TheGlobalData` pointer |
| `0x00a39580` | Script engine |
| `0x00a39598` | Tactical view |
| `0x00a3984c` | `TheNetwork` |
| `0x00a39b0c` | `TheGameLogic` |
| `0x00a39b18` | `TheGameClient` |
| `0x00a385e8` | `TheDisplay` pointer |
| `0x00a3fa84` | `.wnd` parent stack top, storage from `0x00a3fa88` |
| `0x00943338`, `0x00943430` | msvcrt `floor` and `ceil` import slots, which we point at our own ([performance.md](performance.md)) |

## Byte checks used by the DLL

The DLL only enables game patches when all of these match. Kept in sync with `crates/d3d8/src/game.rs`.

| Address | Bytes | Instruction or data |
|---|---|---|
| `0x004016f0` | `55 8B EC 6A FF 68 F5 8B 8F 00 64 A1 00 00 00 00` | WinMain prologue |
| `0x00410102` | `8B 15 BC 81 A3 00 8B D8 8A 42 14` | limiter reads UseFPSLimit |
| `0x009441c0` | `14 3C 9D 00 E0 B4 41 00 00 00 00 00 20 00 00 00` | INI entry `Windowed` |
| `0x004f4463` | `8B 15 BC 81 A3 00 89 42 24` | `-xres` stores to `+0x24` |
| `0x0073c09b` | `D8 15 B4 7A 96 00` | `fcom [1.332]` |
| `0x0073c0a8` | `D8 1D B0 7A 96 00` | `fcomp [1.334]` |
| `0x0073c128` | `D8 15 B4 7A 96 00` | `fcom [1.332]` |
| `0x0073c135` | `D8 1D B0 7A 96 00` | `fcomp [1.334]` |
| `0x00967ab0` | `83 C0 AA 3F FA 7E AA 3F` | the two aspect bounds |
| `0x006dca49` | `A1 84 FA A3 00 83 C4 08 3D 88 FA A3 00` | parent stack check |
| `0x006dcb1c` | `8B 0D E8 85 A3 00 8B 11 83 C4 0C FF 52 20` | SCREENRECT scaling start |
| `0x006dcb9b` | `DD D8 85 DB 89 44 24 34` | SCREENRECT scaling end |
| `0x005a6b40` | `8B C1 8B 08 85 C9 74 17` | scheme manager drawForeground start |
| `0x005a6b5a` | `E8 A1 F4 FF FF C2 08 00` | call scheme draw foreground, ret 8 |
| `0x005a6b70` | `8B C1 8B 08 85 C9 74 17` | scheme manager drawBackground start |
| `0x005a6b8a` | `E8 81 F5 FF FF C2 08 00` | call scheme draw background, ret 8 |
| `0x005a542d` | `FF 52 20 89 44 24 18` | scheme setup getWidth call |
| `0x005a5446` | `DB 46 04` | `fild [esi+4]` (scheme creation width) |
| `0x005a5451` | `FF 50 24 89 44 24 10` | scheme setup getHeight call |
| `0x005a5464` | `DB 46 08` | `fild [esi+8]` (scheme creation height) |
| `0x00741c94` | `E8 97 E2 CC FF` | call GameEngine::update |
| `0x009679cc` | `E0 DD 73 00` | W3DDisplay vtable draw slot |
| `0x0073e07f` | `8B 45 00 8B CD FF 50 60 8B 0D CC 9C A3 00 8B 11 FF 52 10` | updateViews, particle update |
| `0x0073e4a5` | `8A 44 24 13 84 C0 74 13 8B 0D 98 95 A3 00` | frozen-time loop check |
| `0x0076ccc9` | `8B 8E 84 00 00 00 8B 11 8D 44 24 18 50` | setCameraTransform reads the camera at `+0x84` |
| `0x009732d4` | `C0 5B 85 00` | CameraClass vtable Set_Transform entry |
| `0x00855bc0` | `8B 44 24 04 56 8B F1 50 E8 B3 B2 FF FF C6 86 CC 00 00 00 00` | CameraClass Set_Transform |
| `0x0076747f` | `57 E8 8B 87 FE FF` | RTS3DScene::Flush: push rinfo; call DoTrees |
| `0x007674d7` | `57 E8 93 10 01 00 83 C4 04 E8 7B B6 0D 00` | RTS3DScene::Flush: push rinfo; call DoParticles; add esp, 4; call SortingRendererClass::Flush (`0x00842b60`, also called from `0x0083cea5`, `0x0083cfb9`, `0x0083cf9e`, `0x007839c0`, `0x0083cf57`) |
| `0x00968b54` | `B0 DD 76 00` | W3DView second vtable draw entry |
| `0x0076ddb0` | `6A FF 68 6B 6D 93 00` | W3DView::draw start |
| `0x0076e49a` | `8D 84 24 80 00 00 00 8D 75 84 50 8B CE E8 24 F6 FF FF` | W3DView::draw calls getAxisAlignedViewRegion |
| `0x0076da9b` | `68 C0 DA 76 00` | push drawable pass callback |
| `0x0076dac0` | `8B 44 24 08 8B 4C 24 04 50 E8 02 55 D0 FF C3` | drawable pass callback |
| `0x00850e80` | `51 8B 44 24 08 8B 10 89 51 1C 8B 50 04 89 51 20` | RenderObjClass Set_Transform start |
| `0x0073dfa3` | `A1 B8 04 A4 00 8B 15 54 C5 A1 00 03 C2 A3 B8 04 A4 00` | draw advances the render clock |
| `0x0073dfb5` | `A1 B8 04 A4 00 50 E8 40 F0 0F 00` | draw calls WW3D::Sync with it |
| `0x00410097` | `8B 0D 98 95 A3 00 8B 11` | limiter reads tactical view |
| `0x004100a6` | `FF 92 C4 00 00 00 83 F8 01` | speed multiplier above 1 check |
| `0x004100b5` | `8B 0D 80 95 A3 00 E8 90 74 02 00` | script time fast check |
| `0x004100c8` | `A1 BC 81 A3 00 8A 88 24 09 00 00` | GlobalData `+0x924` check |
| `0x004100d7` | `8B 0D 0C 9B A3 00 83 B9 94 00 00 00 03` | game mode is replay check |

The checks for the client-every-frame patches (engine update calls, logic step, clock advance, W3DView::update blocks, scrollBy, client update calls, keyboard and mouse input frames, Shell timer, InGameUI steps, ControlBar blocks, water steps) and the terrain caches are listed with comments in `crates/d3d8/src/game.rs`, and their addresses are in the code table above.

The 9 `setPosition` call sites in the scheme setup are checked at runtime (opcode `E8` with target `0x004f9040`) before they're redirected.
