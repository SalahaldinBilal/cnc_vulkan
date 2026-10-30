# Smoothness Inventory

Every visual has to move at the display rate without running faster than the original. The logic steps 30 times per second, and the client (input, UI, camera, rendering) runs every frame (see [frame-loop.md](frame-loop.md)). This file tracks each visual system and whether it's smooth yet.

Three kinds of problem show up:

- **Stepping:** state changes once per logic step, so it moves in 30 per second steps. Fix: blend between the last two steps while rendering, then put the real values back. Only for state the simulation owns.
- **Too fast:** a step per call that assumed 30 calls per second. Fix: scale the step by real frame time.
- **Frame counters:** a whole-number counter per call that can't be split. Fix: advance it once per 30 Hz tick while the code around it runs every frame, and blend what it moves where needed.

## Status

| What | Problem | Status |
|---|---|---|
| Camera input (scroll, rotate keys, zoom keys, wheel zoom easing, middle mouse) | Laggy and twitchy with the old matrix blend | Every frame, scaled by real time. Built, not tested. [camera.md](camera.md) |
| Camera scripted moves, unit follow, shake | Frame counters | On the tick, view change spread over the next tick. Built, not tested |
| Camera tracking of the selected unit | Looks at a 30 Hz position | Looks at the blended position. Built, not tested |
| Mouse, keyboard, UI hover and clicks, selection, drag box, placement preview | Only processed at 30 Hz | Every frame. Input frame counters held to 30 per second for key repeat. Built, not tested |
| Control bar update | Once per update | Every frame. Radar glow, scheme animations, animate managers and button flash on the tick. Built, not tested |
| Text entry caret | Blinks every 8 draws (`drawCnt`) | Counter advances on the tick, blinks every 0.53 s at any frame rate. Built, not tested. Before the fix it flickered at high frame rates |
| Model positions and facing | Stepping | Smooth, confirmed. Set_Transform and Set_Position replaced, see [frame-loop.md](frame-loop.md) |
| Model animation, texture animation, decal fades | Stepping | Smooth, confirmed. WW3D sync time advanced on redraws |
| Build and upgrade clock on buttons | Flashed | Fixed, confirmed |
| Turrets, wheels, recoil (bone controls) | Stepping | Smooth, confirmed. `bones.rs` |
| Health bars, icons, selection circles, shadows (drawable and object positions) | Stepping | Jumped ahead on logic step frames until the record moved into that frame's draw. Built, not tested. `things.rs` |
| Particles | Stepping | Built, not tested. `particles.rs` |
| Tank tread scroll | Stepping | Built, not tested. `treads.rs` |
| Tree sway | Too fast | Built, not tested. `trees.rs` |
| Water texture scroll | Too fast now that the update runs every frame | Steps scaled by real time, bump flipbook held to 30 per second. Built, not tested. `water.rs` |
| Infantry death animations | Reported stepping | Set_Position now recorded, not tested |
| World animations (rising icons) | Too fast (z rise added per draw) | Built, not tested. Rise scaled by real time |
| World animation fade | Stepping (alpha from logic frames left) | Built, not tested |
| Floating text | Stepping (frame count per update) | Built, not tested |
| UI window slides (control bar entering at match start, menu transitions) | Frame counters (animate window managers) | Shell and animate managers on the tick, winSetPosition blended with the tick factor. Built, not tested. `gui.rs` |
| Water bump flipbook | 30 frames per second flipbook | The animation's own frame rate, not a lock |
| Water grid waves (maps that use them) | Stepping (simulated per logic frame) | Built, not tested. Heights blended |
| Snow | Too slow (update adds the WW3D frame time once per update, which redraws shrink) | Built, not tested. Snow update moved into every draw |
| Cinematic text | Too short (countdown per draw) | Built, not tested. Only tick frames count down |
| Lasers, ropes and other segmented lines | Stepping (Set_Points and Set_Width once per drawable pass) | Built, not tested. `lines.rs` |
| Tracer fade | Stepping (Set_Opacity per drawable pass) | Built, not tested. `lines.rs` |
| Menu transition fades, scale-ups, flashes, control bar arrow | Frame counters (window manager update) | Window manager update on the tick, draws blended with the tick factor. Built, not tested. `transitions.rs` |
| Military subtitle during script freezes | Timers counted back per call | Only on ticks. Built, not tested |
| Transition text typing and number counting | One character or number per frame | Discrete by design |
| Letterbox, cursor animation, sky scroll, laser texture scroll | Already timed by real time or sync time | Fine |
| Radar view box | Built from the camera during the UI draw, while the camera blend is on | Fine |
| Anim2D icon animations | Frame flips per logic frame (the animation's own rate) | Fine |

## Drawables and objects (`things.rs`)

- Thing layout (constructor `0x00543c90`, vtable `0x0094eaac`): `+0x4` template, `+0x8` transform (Matrix3D), `+0x38` cached position, `+0x44` cached angle, `+0x48` to `+0x58` other caches, `+0x5c` cache flags (0 means nothing cached). The camera-follow code reads `+0x38` and `+0x44`.
- Drawable::draw (`0x00472fd0`) takes the object's transform (drawable `+0x88` is the object) or its own, applies the tilt matrix at drawable `+0xd4` and physics (`0x00470920`), and calls every draw module's vtable `+0x1c` (doDrawModule).
- The client walks its drawables with client vtable `+0x24` (first) and drawable `+0x90` (next), confirmed at `0x004a9f5c`.
- After each game update we record every drawable's and object's transform. Before rendering, ones that moved get the blended transform, position and angle with the cache flags cleared, and the whole `+0x8` to `+0x5c` block is put back after. Only pointers from the live list are touched.
- The game draws from inside its client update, so in the frame that ran the logic step the draw came before that update's record. The blend was skipped there and health bars and icons showed the new logic position, a full step ahead of the model, then jumped back on the next frame. A recording of circling stealth fighters at about 60 fps showed the bar going +17, -5, +17, -4 pixels while the jet moved evenly. Now the draw of that frame records first, and the record after the client update only takes the latest transforms, keeping the previous ones.

## Bone controls (`bones.rs`)

- RenderObjClass vtable bone slots (confirmed in the base vtable `0x00967e74`): `+0xb0` Get_Bone_Index(name), `+0xb4` Get_Bone_Transform(index) (returns a Matrix3D, translation at `+0xc`, `+0x1c`, `+0x2c`), `+0xbc` Capture_Bone, `+0xc0` Release_Bone, `+0xc4` Is_Bone_Captured, `+0xc8` Control_Bone(index, matrix, world space flag). The base Control_Bone `0x006202b0` is `ret 0xc` and the base Capture and Release `0x007c6300` are `ret 4`.
- The first time a render object's transform is recorded, its vtable is checked. If it's inside game.dat and overrides Control_Bone, our hook goes into its Control_Bone slot (and Release_Bone when that's overridden too). The log names each hooked vtable.
- The hook keeps the last two matrices per model and bone, holding a reference on the model. Bones controlled in the current update are blended before render and set back after. Release_Bone drops the entry.

## Particles (`particles.rs`)

- Manager: `TheParticleSystemManager` `0x00a39ccc`, W3D manager vtable `0x00969268` (constructor `0x00778150`, created by engine vtable `+0x6c`). Update `+0x10` (`0x004cea00`) runs only when TheGameLogic `+0x3c` differs from manager `+0x94`. Systems are in a std::list at manager `+0x80` (node: next, prev, system).
- Render: manager vtable `+0x20` (`0x00778590`) fills 512-entry buffers (positions, colors, sizes, orientations) for a point group (`+0xb0`) or streak (`+0xb4`).
- ParticleSystem vtable `0x00949794`, `+0x8` update `0x004cc8c0` (particle update inlined). System `+0x0c` shader type (1 additive), `+0x1bc` drift velocity, `+0x22c` wind motion, `+0x25c` first particle.
- Particle (0x180 bytes, vtable `0x00949768`), matches EA's released source exactly: `+0x8` velocity, `+0x14` position, `+0x20` emitter position, `+0x2c` velocity damping, `+0x30` angle, `+0x34` spin, `+0x38` spin damping, `+0x40` size, `+0x44` growth, `+0x48` growth damping, `+0x10c` color scale, `+0x114` faces emitter flag, `+0x118` next in system, `+0x128` system, `+0x12c` personality, `+0x130` acceleration, `+0x13c` last position, `+0x148` lifetime left, `+0x14c` created on client frame, `+0x150` alpha, `+0x154` alpha rate, `+0x15c` color, `+0x168` color rate.
- One step: velocity = (velocity + acceleration) * damping, position += velocity + drift, angle += spin then spin *= damping, size += growth then growth *= damping, alpha += rate (keyframes can snap it), color += rate + color scale, both clamped.
- We work the previous state out backward from those rates instead of storing it (particles are pooled, so addresses get reused). Particles created in the latest step are left alone. Nothing is blended if the manager didn't step (paused).

## Treads (`treads.rs`)

- W3DTankDraw (vtable `0x0096ad08`, constructor `0x007bf0c0`, doDrawModule `0x007bf6e0`, destructor `0x007bf200`): treads at `+0x8c`, 5 dwords each (render object, side, unknown, U offset `+0x98`, V offset), count at `+0xdc`. Dust particle systems at `+0x80`, `+0x84`.
- W3DTankTruckDraw (vtable `0x0096ba18`, constructor `0x007c4200`, doDrawModule `0x007c4be0`, destructor `0x007c4470`): treads at `+0x190`, U offset `+0x19c`, count `+0x1e0`.
- We hook both classes' doDrawModule (to see the step) and destructor (to forget the module) through their vtables, and blend the U offset across its 0 to 1 wrap.

## Trees (`trees.rs`)

- W3DTreeBuffer::drawTrees `0x00787d90`, called once from the terrain render at `0x0074ff01` with the tree buffer from terrain `+0x282c`. It returns early unless `+0xb4ae0` is set. Ten sway offsets at `+0xb68ac`, steps 10 floats after, factors 10 after that. Each draw adds the step unless the game or script time is paused. Breeze changes run updateSway `0x00785180`.
- We wrap the call and scale the steps by real time since the last draw over the update interval, putting the original steps back unless the game changed them during the draw.

## Water (`water.rs`)

- WaterRenderObjClass update `0x00758020`, reached by a tail jump at `0x007aabe0` from W3DTerrainVisual::update (`0x007aabd0`, `this` is the object `+4`). TheTerrainVisual `0x00a3a1a4`, water pointer at `+0x14`.
- Per call: `+0x274` river V origin += 0.002, `+0x294` and `+0x298` offsets += fixed steps (wrap inside -1 to 1), `+0x210` bump frame (0 to 31). On a new logic frame it also steps the water grid wave simulation.
- The update now runs every frame. The three `fadd [step]` operands (`0x00758042`, `0x00758054`, `0x00758066`) point at our steps, set each frame to the game's steps times real frame time. The bump frame is set back after the call and moved by the whole 30 Hz frames that passed.
- Wave grid (only on maps that enable it): `+0x22d` enabled, `+0x22c` still settling, `+0x224` cells of 12 bytes (height, velocity, flags), `(+0x26c + 3) * (+0x270 + 3)` cells. It steps inside the same update when the logic frame changes. We snapshot heights before and after the update and blend them while rendering.

## InGameUI per-draw work

- TheInGameUI is `0x00a39fb0` (W3DInGameUI, vtable `0x00969e30`, update `+0x10` jumps to `0x0050a730`, draw `+0x14` is `0x007a8d60`). TheMouse is `0x00a37a80` (draw `+0x14` draws the cursor after the UI). An earlier note had these two swapped.
- W3DDisplay::draw order: views (3D), then TheInGameUI draw, then TheMouse draw. The UI draw calls preDraw (vtable `+0x10c`, `0x0050a710`), which matches EA's source: handleBuildPlacements `0x0050a300`, handleRadiusCursor `0x0050a140`, drawFloatingText `0x00510a50`, updateAndDrawWorldAnimations `0x005112d0`.
- handleBuildPlacements reads the mouse status position at TheMouse `+0x49bc`, `+0x49c0` (`0x0050a453`). That only changes in the mouse update, which now runs every frame. The game window handle is `0x00a35a28`.
- Building preview: handleBuildPlacements (`0x0050a300`) moves the preview drawables (InGameUI `+0x174c` array, used while `+0x1740`, the thing being placed, is set; up to GlobalData `+0x7d0` of them for walls) with setPosition. Their models only follow in Drawable::draw, which the drawable pass calls once per logic step, and preDraw runs after the 3D scene. So even with the mouse read every frame, the preview lagged and our blends smoothed it. Now handleBuildPlacements and handleRadiusCursor (`0x0050a140`) run right after the camera update, before the scene; we call Drawable::draw (`0x00472fd0`, thiscall with the view) on each preview drawable; the preDraw calls at `0x0050a713` and `0x0050a71a` are skipped when that already happened; and the preview drawables and their models are left out of the blends.
- World animations: `z += zRisePerSecond * [0x0094ddb0]` (1/30) every draw at `0x0051133a`, unless paused. We repoint that multiply's operand (`0x0051133f`) at our own value, set to real seconds since the last call by a wrapper on the call at `0x0050a728`. The fade uses the same constant with logic frames left, unchanged.
- Floating text: `fild [esi+0x20]` (frame count) `fmul [ecx+0x1d0c]` (rise speed) at `0x00510b5d`. The count is bumped by `0x005108e0` in InGameUI::update when the logic frame changed, which is after the draw of the frame that ran the logic step. The stub subtracts the part of the logic step not yet elapsed, except during that draw.
- World animation fade: `alpha = framesLeft / 30` from logic frames left (`0x00511392`, only in the last 30 frames). The logic frame moves in the logic step, before the draw, so the stub always adds the part of the step not yet elapsed.
- Drag selection: TheInGameUI `+0x20` is the drag selecting flag and `+0x24` the rectangle (left, top, right, bottom), drawn by `0x007a8de0` through TheDisplay vtable `+0x84` (open rectangle, width 2, color 0x9933ff33). The rectangle comes from the selection translator, which now runs every frame. (Earlier redraws stretched it to the live cursor, which twitched because the update's own draws didn't.)

- WW3D frame time (sync minus previous sync, `0x00a71d9c` minus `0x00a71da0`) changes meaning with our animation clock: during redraws it covers one redraw, and during an update it only covers the last redraw. Readers of the previous sync: `0x0076da8a` (camera update, drawable pass check), `0x0079e60b` (snow update), and five WW3D emitter and FX functions in `0x0085` to `0x0088` that run while rendering (fine). Snow (TheSnowManager `0x00a39e28`, vtable `0x00969b84`, update `+0x10` `0x0079e600`, clock at `+0xc`, period `+0x14`) was the only update-time reader, so its vtable update is now a no-op and we call the real one in every draw after the sync.
- W3DDisplay::draw counts the cinematic text down once per draw (display `+0x24`, at `0x0073e3d3`). Only tick frames decrement.

Rule of thumb for the timing of a "remaining" correction: values bumped in the client update (before the update's draw) need it during that draw too. Values bumped in the logic step (after the draw) only need it on redraws.

## Line effects (`lines.rs`)

- SegmentedLineClass (a plain RenderObjClass subclass, reference count at `+4`): Set_Points(count, points) `0x00872600`, Set_Width `0x00872710` (stores `+0xb4`), texture tiling `0x008727b0`. Line3DClass keeps its RenderObjClass part at `+4` (Snapshot-style first base), Set_Opacity `0x00875e90`.
- W3DLaserDraw (vtable `0x0096aba0`, doDrawModule `0x007bea30`) reads the LaserUpdate client module for start, end and width scale, and sets points and width on each beam's line at `0x007bee92` and `0x007bee7e`. W3DTracerDraw (vtable `0x0096baac`, doDrawModule `0x007c5b90`) moves its Line3D with Set_Transform (covered by the model blend) and decays opacity with Set_Opacity at `0x007c5cb2`.
- Other Set_Points callers: `0x0078973a`, `0x00789f26`, `0x0078a11e` (in `0x007894b0`) and `0x007c2ca8` (in `0x007c2c30`). Other Set_Width callers: `0x00789428`, `0x007be7f1`, `0x00789513`, `0x0078a10d`, `0x007c2cdb`.
- We redirect all those call sites (each checked to be a `call` to the expected function) to wrappers that record values during game updates, then blend points, width and opacity before rendering and set the game's values back after. We hold a reference on each line.

## Window transitions (`transitions.rs`)

- Layouts from EA's header, checked against the flash update (`0x006efa40`, stores m_drawState at `+0x20`, m_win `+0xc`, m_isForward `+0x9`): Transition vtable is destructor, init `+4`, update `+8`, reverse `+0xc`, draw `+0x10`, skip `+0x14`. m_drawState is `+0x20` for flash, button flash, fade, scale up, the main menu scale styles and score scale up, `+0x24` for screen fade and full fade, `+0x30` for the control bar arrow.
- Style values from the name table at `0x0094e000`: FLASH 0, BUTTONFLASH 1, WINFADE 2, WINSCALEUP 3, MAINMENUSCALEUP 4, TYPETEXT 5, SCREENFADE 6, COUNTUP 7, FULLFADE 8, TEXTONFRAME 9, MAINMENUMEDIUMSCALEUP 10, MAINMENUSMALLSCALEDOWN 11, CONTROLBARARROW 12, SCORESCALEUP 13, REVERSESOUND 14. TransitionWindow: style `+0x8`, transition `+0x14`.
- GameWindowTransitionsHandler::draw `0x00515530` walks the draw group (`+0x14`) and secondary group (`+0x18`) and calls `transition->draw()` at `0x0051554f` and `0x00515575`. We replace both calls. When a transition's frame counter changed this update, we draw it at the previous and current frame with the display's drawOpenRect (`+0x84`), drawFillRect (`+0x88`) and drawImage (`+0x94`) captured (our wrappers sit in those vtable slots and pass through otherwise), then draw the blend. If the two captures don't line up, we draw the current frame as the game would.

## UI windows (`gui.rs`)

- GameWindow (W3DGameWindow, 0x2d8 bytes, vtable `0x0096be98`, deleting destructor `0x007cede0`): `+0x8` size (w, h), `+0x10` region (lo x, lo y, hi x, hi y), `+0x1f0` parent. Screen position (`0x004f90f0`) adds up the parents' region corners. winSetPosition `0x004f9040` sets the region from (x, y) and the size, kept ordered. winSetSize `0x004f91a0`.
- We replace winSetPosition with the same logic plus recording during game updates, and hook the destructor so freed windows are forgotten. Windows moved in the current update are shifted to the blended position before rendering and put back after. Children follow their parents automatically.
