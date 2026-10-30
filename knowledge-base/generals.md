# Generals (the base game)

Every patch runs on the base game's game.dat too, except the two features Generals doesn't have (tree sway and snow). The build check knows it as its own build (see [loading-and-hooking.md](loading-and-hooking.md)), and each module keeps a table of Generals addresses next to Zero Hour's.

## Ported

- Startup settings in `game.rs`: the windowed flag (GlobalData `+0x20`), the resolution (`+0x24`, `+0x28`), skipping the intro (`+0x819`, then the after intro flag `+0x81a`), and listing every aspect ratio. The display mode filter's two `fcomp qword` instructions (`0x007151dd`, `0x00715245`) are pointed at a double of ours, since the tolerance they read (`0x00992688`, 0.01) is shared by 16 other instructions.
- Sound in background ([audio.md](audio.md)).
- Discord status ([discord.md](discord.md)).
- The UI layout ([ui-layout.md](ui-layout.md)). The SCREENRECT scaling block in the .wnd parser is laid out exactly like Zero Hour's (same stack slots, corners in ESI, EDI, EBP, EAX), so the same stub works. It resumes at `0x006c4e8e`.
- The wide control bar. The scheme setup is `0x005b5690`, called from `0x005b715a`, `0x005b73d9`, `0x005b6ea8`, and it makes the same 9 setPosition calls. Its getWidth and getHeight calls return to `0x005b5770` and `0x005b5794`. The scheme draws are `0x005b6450` and `0x005b6560`, reached from `0x005b6f70` and `0x005b6fa0`. The faction bar images, their mapping INIs and every window name we touch are identical to Zero Hour's (Zero Hour loads the bar textures from the Generals archives), so the seams and band split carry over.
- Our options page ([options-menu.md](options-menu.md)). Generals' `OptionsMenu.wnd` (in `Window.big`) has every window the page copies, and the display rows sit where Zero Hour's do. The .wnd loader's openFile call is `0x006c404c`, the callback table entries are `0x00a2611c` and `0x00a25ba4`, and the gadget messages are the same numbers. Generals' Options.ini is under `Command and Conquer Generals Data` in Documents: GameData.ini sets that name (`UserDataLeafName`, GlobalData `+0x92c`), where Zero Hour reads it from the registry.
- The 30 Hz logic split and everything blended on top of it ([frame-loop.md](frame-loop.md), [smoothness.md](smoothness.md)): frame, objects, bones, things, particles, camera, client, treads, water, overlays, window slides, lines and transitions. GameEngine::update and W3DDisplay::draw are the same code with Generals' globals, so most hooks map one to one. What changed is listed under "Differences" below. Tree sway and snow don't exist in Generals and stay off.
- The 3D world's own render target ([scene.md](scene.md)): W3DView::draw `0x0074e100`, its getAxisAlignedViewRegion call `0x0074e816`, and RTS3DScene::Flush's DoTrees and SortingRendererClass::Flush calls `0x00746d10`, `0x00746d70` (the sorting flush right after DoParticles at `0x00746d68`).
- Zoom ([camera.md](camera.md)): setHeightAboveGround `0x0074ece0` (vtable `+0xf4`), zoomIn and zoomOut steps at `0x0044f6fc`, `0x0044f72c`, DrawEntireTerrain reads `0x0071d778` and `0x0074cf33`, setDrawOrg's check `0x007206ce`. oversizeTerrain is hooked at its one call `0x00728adf`.
- Text ([text.md](text.md)): setUseHotkey `0x007abb80` (vtable entry `0x009a907c`), the render calls `0x007ab7b6` and `0x007ab7be`, Render2DSentenceClass::Render `0x00843770`. The string layout is the same.
- The minimap ([radar.md](radar.md)): the sharper terrain image, the finer shroud, unit dots and camera box, the map drawn from above with the map file's objects and trees, and smooth fog of war in the world. W3DRadar's own fields are the same. radarToWorld only exists as the 3D version (`0x004958b0`), so only its two limits are raised.
- Window helpers: TheWindowManager `0x00a6c19c` (sendSystemMsg `+0x120` and winGetWindowFromId `+0x128` as in Zero Hour), TheNameKeyGenerator `0x00a6bbf4`, nameToKey `0x00477900`, winSetPosition `0x004f6ec0`, winSetSize `0x004f7050`, winHide `0x004f7110`, winGetScreenPosition `0x004f6f70`, UnicodeString::format `0x00420280` and its release `0x0041fcb0`.

Everything else, starting with the 30 Hz logic split, still needs porting.

## The build

- The EA app installs it in a `Command and Conquer Generals` folder inside the Zero Hour install folder: `game.dat` (12 MB, about twice Zero Hour's 6.5 MB), `Generals.exe`, `WorldBuilder.exe`. Not large address aware, signed by EA like Zero Hour's.
- Imported into the Ghidra project as `/generals/game.dat` (`cnc/generals` in headless commands).
- The early code sits close to Zero Hour's: GameLogic::startNewGame is `0x0049fa20` (Zero Hour `0x0049fb70`), and the same strings are referenced from the same functions. Later code drifts further (the window layout parser `0x006dca49` in Zero Hour is `0x006c4d3a` here).

## Finding Zero Hour's addresses in Generals

`re/scripts/Port.java` (see [tooling.md](tooling.md)):

1. The list of Zero Hour byte checks, `<address> <length>` per line, taken from `game.rs`.
2. `sig`, run on Zero Hour: for each check, the instructions around it as a byte pattern with a mask. Strict patterns (4 widths, 16 to 128 bytes either side) mask every operand that refers to an address in the image: calls, jumps, globals, vtables, which all move between builds. Loose patterns (3 widths, 48 to 160 bytes) also mask every displacement and immediate, since struct offsets move too. Data checks (vtable entries, tables, constants) are listed apart.
3. `find`, run on Generals: each pattern, strict ones first, the first width with exactly one hit wins. It prints the Generals address and the bytes there, which also show Generals' own globals and offsets at the same spots.

A strict match is the same code. A loose match is the same code shape with different numbers, so the struct offsets need reading off the Generals bytes and checking.

### Function matching

Byte patterns only find code that barely changed. `re/scripts/Match.java` pairs whole functions between the builds first, then places each address inside its paired function (see [tooling.md](tooling.md)). Ghidra's Version Tracking was tried first and ran out of memory or ran for over an hour without output.

1. Ghidra leaves about 10,000 pieces of code in each build outside any function: code that only a table reaches (vtable methods, command line handlers, constructors). The script makes functions there first, in memory only. Without that step, a quarter of the checks sat in code with no function.
2. It pairs functions that are unique on both sides by masked bytes, then by instruction names, then by the set of strings they use, then by a string only one function uses.
3. Then it repeats until nothing changes: callees in the same order, a lone unmatched caller or callee, and the unmatched functions between two neighboring matches, since the linker keeps source file order in both builds.
4. Each address inside a matched function maps through an alignment of the two functions' instructions with every value masked. Where registers differ, a second alignment by instruction name places it ("aligned by name"), and those need checking by eye. Data addresses map through the code that refers to them, or by finding where Generals holds a pointer to the matched function (vtable entries).

Zero Hour has about 44,700 functions with that step and Generals about 48,700. 25,300 pair up, in under a minute.

## Results

Byte patterns alone, of 265 checks: 82 strict matches, 33 loose matches, 117 code checks with no unique match, 33 data checks.

Function matching places 221 of the 265 checks and 188 of the 209 address constants, and agrees with every strict byte pattern match. The rest were placed by hand in Ghidra (below). The per-check list with both builds' bytes is `re/out/port/match/zh_checks.mapped.txt`, and the bytes differ in most places, so every check is rebuilt from the Generals bytes when its module is ported.

Mistakes the matcher made, so its output still needs reading:

- Tiny functions with identical code (`xor eax, eax; ret`, `mov eax, [ecx+0x8c]; ret`) pair up arbitrarily. Vtable entries that point at them can't be checked through the function pairs.
- "Aligned by name" picked the wrong instruction twice in GameLogic::startNewGame and in drawTrees. It also picked the wrong one of two neighboring color reads in the particle render.
- Vtable entries placed as "vtable start + Zero Hour's offset" are wrong where the vtable layout changed (see below).
- GameClient::update's mouse update call was placed on the code after the keyboard block.
- Radar::radarToWorld2D's checks were placed in radarToWorld, which starts with the same code. Generals has no radarToWorld2D.

## Globals

| What | Zero Hour | Generals |
|---|---|---|
| TheGlobalData | `0x00a381bc` | `0x00a6a574` |
| TheDisplay | `0x00a385e8` | `0x00a6a9a8` |
| .wnd parser parent stack top | `0x00a3fa84` | `0x00a71bbc` |
| TheGameLogic | `0x00a39b0c` | `0x00a6be84` |
| TheGameClient | `0x00a39b18` | `0x00a6be90` |
| TheTacticalView | `0x00a39598` | `0x00a6b95c` |
| TheScriptEngine | `0x00a39580` | `0x00a6b940` |
| TheTerrainLogic | `0x00a39594` | `0x00a6b958` |
| TheThingFactory | `0x00a39b24` | `0x00a6be9c` |
| TheMouse | `0x00a37a80` | `0x00a69e34` |
| TheKeyboard | `0x00a37a90` | `0x00a69e44` |
| TheAudio | `0x00a37a8c` | `0x00a69e40` |
| TheTerrainRenderObject | `0x00a48b30` | `0x00a72b58` |
| TheWaterRenderObj | `0x00a48b40` | `0x00a72b70` |
| TheMapObjectListPtr | `0x00a488d0` | `0x00a72988` |
| TheDX8MeshRenderer | `0x00a762d8` | `0x00a7f808` |

## Functions and tables

| What | Zero Hour | Generals |
|---|---|---|
| WinMain | `0x004016f0` | `0x004017a0` |
| GameEngine::update | `0x0040ff30` | `0x0040c760` |
| GameLogic::startNewGame | `0x0049fb70` | `0x0049fa20` |
| `-xres` handler writing GlobalData `+0x24` | `0x004f4463` | `0x004f5b33` |
| .wnd parser, SCREENRECT scaling | `0x006dca49`, `0x006dcb1c` | `0x006c4d3a`, `0x006c4e0d` |
| RenderObjClass::Set_Transform, Set_Position | `0x00850e80`, `0x00850f60` | `0x0081b7e0`, `0x0081b8c0` |
| W3DDisplay::draw, W3DDisplay vtable | `0x0073dde0`, `0x009679b8` | `0x00716f10`, `0x009a5648` |
| W3DView::draw, update, setCameraTransform | `0x0076ddb0`, `0x0076d030`, `0x0076cb00` | `0x0074e100`, `0x0074d3a0`, `0x0074ceb0` |
| W3DView::scrollBy, setHeightAboveGround | `0x0076e6d0`, `0x0076e980` | `0x0074ea40`, `0x0074ece0` |
| CameraClass vtable, Set_Transform | `0x00973288`, `0x00855bc0` | `0x009b0ab0`, `0x00822030` |
| Keyboard and mouse vtable update thunks | `0x007ac550`, `0x0073b990` | `0x00782fe0`, `0x00714a90` |
| Control bar scheme drawForeground, drawBackground | `0x005a6b40`, `0x005a6b70` | `0x005b6f70`, `0x005b6fa0` (never disassembled by Ghidra) |
| InGameUI::preDraw, handleBuildPlacements | `0x0050a710`, `0x0050a300` | `0x005242d0`, `0x00523ec0` |
| OptionsMenuInit, OptionsMenuSystem | `0x00465bc0`, `0x004685d0` | `0x00465d50`, `0x00468250` |
| Callback name table entries for those two | `0x009ec428`, `0x009ebea0` | `0x00a26118`, `0x00a25ba0` |
| GameData INI fields Windowed, PlayIntro | `0x009441c0`, `0x009453d0` | `0x009881a0`, `0x009893c0` |
| HeightMapRenderObjClass constructor, vtable | `0x0079b670`, `0x00969978` | `0x0072dab0`, `0x009a5cc0` |
| HeightMapRenderObjClass Render, oversizeTerrain | `0x0079ce40`, `0x0079b710` | `0x00733170`, `0x0072fa80` |
| RTS3DScene vtable (stored at) | `0x009687b4` (`0x00765e44`) | `0x009a63c0` (`0x0074545d`) |
| W3DTreeBuffer::drawTrees | `0x00787d90` | `0x00766610` |
| Radar::radarToWorld (the 0x80 clamps, `+0x20` sample) | `0x00496cd0` | `0x004958b0` (clamps `0x004958e0`, `0x004958fd`, sample `0x00495913`) |
| WW3D::Begin_Render, End_Render, the flush helper | `0x0083ca10`, `0x0083cfb0`, `0x0083cf70` | `0x007feb00`, `0x007ff130`, `0x007ff080` |
| Shroud shader setup reading TheTerrainRenderObject's W3DShroud | `0x0074e4ec` | `0x00723d86` |
| HeightMapRenderObjClass updateBlock, updateCenter, getStaticDiffuse, renderExtraBlendTiles | `0x0079b3f0` (virtual), `0x0079c1e0`, `0x00754cb0`, `0x0079d9c0` | `0x0072ce00`, `0x007324a0`, `0x00731a60`, `0x00734ae0` |
| W3DRoadBuffer drawRoads, loadRoadsInVertexAndIndexBuffers; W3DCustomEdging drawEdging, its load, clearAllEdging | different code (refills only when changed); not checked | `0x00774dd0`, `0x0076e1d0`; `0x00775790`, `0x00775070`, `0x00775780` ([performance.md](performance.md)) |
| renderOneObject's ghost object block, updateFixedLightEnvironments ([radar.md](radar.md)) | `0x0076714a`, `0x00767500` | `0x00746a00`, `0x00746d90` |
| renderOneObject, its shroud status compare, W3DShroud::setShroudLevel ([radar.md](radar.md)) | `0x00766d30`, `0x0076737b`, `0x00769740` | `0x007465c0`, `0x00746c0a`, `0x00749c10` |
| calcCameraConstraints, its call in setCameraTransform and its view vtable entry ([camera.md](camera.md)) | `0x0076c810`, `0x0076cc1d`, `0x00968cd0` | `0x0074cbc0`, `0x0074cfbb`, `0x009a68dc` |
| drawRoads calls in the terrain renders, wrapped to clip roads to the terrain ([camera.md](camera.md)) | `0x0079d587`, `0x0079854c` | `0x00733a1d` |
| Water shroud passes (null checked) | `0x00759f37`, `0x0075b461`, `0x0075c960`, `0x0075cb1e`, `0x0075dfb9` | `0x00738a96`, `0x00739ef9`, `0x0073b2df`, `0x0073b3d1`, `0x0073c65e` |

## Differences that need code changes

- **Display mode list:** Generals keeps a mode when `|width / height - 4/3|` is below a double at `0x00992688` (the 4/3 float is at `0x009a573c`). Zero Hour has two float bounds instead.
- **Frame limiter:** the FPS limit flag is GlobalData `+0x13` (Zero Hour `+0x14`), read at `0x0040c912` with different instructions. There's no script engine "time fast" or replay fast mode check before it.
- **View vtable:** the first slots match, but from `+0x94` on every entry is 12 bytes earlier. Examples: the limiter's `+0xc4` is `+0xb8`, setHeightAboveGround `+0x100` is `+0xf4`, and zoom in and out `+0x104`, `+0x108` are `+0xf8`, `+0xfc`. Slots below `+0x94` (lookAt `+0x4c`, `+0x60`, scrollBy `+0x54`, `+0x2c`) are the same.
- **RenderObjClass vtables** (camera, water, height map): every entry is 8 bytes later up to about `+0x80`, and 4 bytes later from about `+0xd0`. Examples: Set_Transform `+0x4c` is `+0x54`, Render `+0x24` is `+0x2c`, Get_User_Data `+0x150` is `+0x154` (the camera's is `0x0072d730`, user data still at `+0x8c`), and WW3D::Render's camera call `+0x2c` is `+0x34`.
- **oversizeTerrain isn't virtual.** The only call is at `0x00728adf`, in a wrapper at `0x00728ad0` that passes TheTerrainRenderObject.
- **WW3D::Begin_Render takes 4 arguments**, not 5 (`add esp, 0x10` after the call at `0x00717269` in W3DDisplay::draw).
- **The flush helper** pushes ECX before EBX, so its argument is read from `[esp+0x10]` after the pushes. It's still one argument.
- **W3DDisplay::draw's loop tail** (`0x00717589`) keeps the "time frozen" flag in BL instead of `[esp+0x13]`.
- **W3DView::update's shake** clears with EDI (`0x0074db38`), and no call follows it.
- **GameClient::update** has no camera tracking block. The same `lookAt(drawable + 0x38)` call is at `0x0060e94a` and `0x0060eb53`. The keyboard update call is `0x004aa276` and the mouse's is `0x004aa29b`, with their registers swapped relative to Zero Hour.
- **No snow:** EA's Generals source has no W3DSnow.
- **No tree sway:** Generals' drawTrees is an older tree buffer with no sway offsets. The tree buffer is TheTerrainRenderObject `+0x2850`, and its shroud is `+0x2868` (Zero Hour `+0x282c`, `+0x2844`).
- **InGameUI::preDraw** calls handleBuildPlacements, then handles the radius cursor inline. It has no world animations call. handleBuildPlacements still reads the pending place type at `+0x1740` and TheMouse's position at `+0x49bc`, `+0x49c0` (`0x00524012`).
- **Map objects in startNewGame** (`0x004a12ab` on): the bridge flag is template `+0x23b`, and the KindOf bit tested at `+0x6c` is `0x1000000`. There's no clutter-to-prop switch. Trees are gated on GlobalData `+0x10` and the recorder (`0x00a6bc88`, `0x00489ea0`). newDrawable is `0x004ae3f0` with three arguments. setOrientation and setPosition are `0x005600f0` and `0x0055fee0`.
- **GameLogic `+0x51`** is `m_loadingScene` (set by setGameLoading, `C6 46 51 01` at `0x005c6f0b`). startNewGame doesn't set it. The reader at `0x005cac5f` matches Zero Hour's.
- **Particles:** first particle `+0x28c`, color `+0x16c`, alpha `+0x160`, next `+0x128` (Zero Hour `+0x25c`, `+0x15c`, `+0x150`, `+0x118`). Read at `0x00757e46`, `0x00757f6b`, `0x00757f86`, `0x00757fc6`.
- **Drawable:** the night flag is at `+0x11c`, draw modules at `+0xc4`, the dirty flag at `+0x149`, the model state call at vtable `+0x24` (Zero Hour `+0x2c`), and the lock at `0x00a6bbf0`.
- **Discord:** the game mode is TheGameLogic `+0x98` (Zero Hour `+0x94`), read at `0x00491c3d`. startNewGame reads the map name as `GlobalData + 8` with `add eax, 8` (`0x0049fa56`).
- **Sound:** WndProc's WM_ACTIVATE block is at `0x00401128`, and TheAudio's loseFocus is vtable `+0x30` (`0x0040114a`).
- **The flash transition** stores m_drawState from EDI (`0x006e00fb`).
- **Particle updates** happen in GameClient::update (`0x004aa406`), not after updateViews in W3DDisplay::draw. So the draw's view update is only `call [eax+0x60]` (8 bytes at `0x007171ac`).
- **Particles have three angles.** Each has its own spin, at `+0x30` to `+0x38` and `+0x3c` to `+0x44`, with one damping at `+0x48`. Z (`+0x38`) is the one drawn. Size is `+0x50`, its rate `+0x54` and damping `+0x58`. Faces emitter is `+0x124`, created frame `+0x15c`, alpha rate `+0x164`, color rate `+0x178`, color scale `+0x11c`. The system's drift is `+0x1ec`. Alpha steps for every particle (Zero Hour skips it when the system's shader slot is 0).
- **Render object vtables** start with virtual Add_Ref and Release_Ref, so Delete_This is slot 2. Release_Bone and Control_Bone are `+0xc4` and `+0xcc`, and RenderObjClass's empty versions are `0x0072d5b0` and `0x0072d5d0`.
- **W3DView** has no shaker angles (Zero Hour's `+0xac` to `+0xb4` are the rotate info here). From the waypoint path info on, its fields sit `0xc0` later: waypoint path flag `+0x518`, scroll amount `+0x548`, ground level `+0x554`. Doing pitch is `+0x108`, doing zoom `+0x124` and the scripted lock `+0x125`. The unit follow block keeps the view in EBX, and the camera lock getter is vtable `+0x154`.
- **GameClient::update** walks drawables with next at `+0x94` (Zero Hour `+0x90`).
- **ControlBar::update:** the radar glow flag is `+0x278` and the button flash flag `+0x1dc`. The flash block pushes EDI between its test and jump.
- **InGameUI:** camera rotate speed is GlobalData `+0x914`. preDraw handles the radius cursor inline, so there's no handleRadiusCursor. Place icons are `+0x1748`, and floating text rise speed is `+0x1c70`.
- **WaterRenderObjClass** fields sit 4 bytes earlier: scroll offsets `+0x270`, `+0x290`, `+0x294`, bump frame `+0x20c`, wave grid `+0x220` to `+0x26c`.
- **TextureClass** keeps its Direct3D texture at `+0x20` (Zero Hour `+0x38`), as TextureClass::Get_Surface_Level (`0x00819e40`) reads it.
- **WorldHeightMap:** width `+0x8`, height `+0xc`, border `+0x10`, data size `+0x20`, tile indices `+0x34`, source tiles `+0x50`, drawn window `+0x1a127c` and `+0x1a1280` (Zero Hour `+0xc`, `+0x10`, `+0x14`, `+0x24`, `+0x44`, `+0x60`, `+0x1a128c`, `+0x1a1290`). HeightMapRenderObjClass holds it at `+0x27c4`.
- **CameraClass** is `0x388` bytes, 4 less than Zero Hour's. Its fields from the projection type on sit 4 bytes earlier: projection `+0x90`, viewport `+0x94`, view plane `+0xa4`, near and far clip `+0xb8`, `+0xbc`, frustum valid `+0xc8`. The user data stays at `+0x8c`.
- **DX8Wrapper::Set_Render_Target** (`0x007fc180`) takes only the surface and keeps the current depth buffer.
- **Map file objects:** startNewGame makes Objects (newObject `0x004ae3f0`) for every entry except bridges and, without trees, shrubbery. So the radar map leaves bridges to the terrain there.
- **OptionsMenuInit's "%d x %d" format call** is at `0x00467368`. OptionsMenuSystem has no EBP frame, so its message argument isn't at `[ebp+0xc]`.
- **The .wnd loader doesn't make a RAM file** and LocalFile reads are unbuffered, so a layout loaded from disk costs one system call per byte. Our options layout is converted with LocalFile::convertToRAMFile (`0x007ea350`, vtable `+0x3c`). Details in [options-menu.md](options-menu.md).
- **Right-drag scrolling is off with the alternate mouse option** (GlobalData `+0x4c`, Options.ini `UseAlternateMouse`). LookAtTranslator's right button down case checks it at `0x0060c755`. Zero Hour scrolls either way. GlobalData `+0x4d` (`RightMouseAlwaysScrolls` in INI) isn't read there.
- **W3DShroud covers only the terrain's draw window**, where Zero Hour's always covers the whole map. init `0x007498a0` (thiscall: WorldHeightMap, cell width, cell height as floats, from GlobalData `+0x1a4`; called at `0x00730971`) sets the cell counts `+0`/`+4` from the whole heightmap (width `+0x8` minus twice the border `+0x10`, minus 1, times 10 / cell size) and the texture size `+0x2c`/`+0x30` from the draw window (`+0x1a127c`, `+0x1a1280`) plus 2, rounded by `0x007ffc50`. render `0x0074a190` (thiscall, called from W3DDisplay::draw at `0x007170c7`) copies the window's cells from the source surface (`+0x1c`, data `+0x20`, pitch `+0x24`) into the texture (`+0x28`) at (1, 1) with CopyRects (device `+0x70`), and stores the window's world origin at `+0x38`/`+0x3c`. Unlike EA's Generals source, retail clamps the start back to 0 after pulling it in from the far edge. The shader setup (`0x00723d86`) translates by cell size minus that origin and scales by 1 / (texture size × cell size). Cell size `+0x10`/`+0x14` (floats), clear flag `+0x41`, border level `+0x42`, fog data `+0x44`/`+0x48`. reset `0x00749a60` frees the source surface and fog data. With the whole terrain loaded the window is the whole heightmap, so the texture covers the map.
- **Drawable::draw** is `0x00470f70` (called by the drawable pass callback `0x0074de10`). It starts from the object's transform (drawable `+0x88`, Thing `+0x8`), multiplies in the tilt matrix at drawable `+0xd8` unless `+0x147` is set, then applies the physics transform `0x0046f1b0` and calls each draw module's `+0x1c`. The physics transform (skipped while the view's time is frozen or a script freezes time, and unless GlobalData `+0x714`) gets pitch, roll, yaw and a z offset from calcPhysicsXform `0x0046f3f0`, which picks by the current locomotor's appearance (`+0x5c` of its template): `0x004703e0` wheels, `0x0046fb70` treads, `0x0046f750` hover and wings, `0x0046f530` thrust. Hover and wings keep pitch and roll as damped springs in DrawableLocoInfo (drawable `+0xb8`), pushed by the physics acceleration and velocity, plus a pitch from the climb angle (atan of z velocity over ground speed) when `0x005cd270` allows. One step per call, and the pass calls it once per logic step, the same as the original.
- **Direct3DCreate8 is a static import** (slot `0x00987708`), not loaded with LoadLibraryA. See [loading-and-hooking.md](loading-and-hooking.md).
