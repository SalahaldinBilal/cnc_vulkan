# Frame Loop

## Main loop and limiter

The engine loop is around `0x00410097` (Ghidra splits it into fragments because of exception handling). The catch handler at `0x0041000f` reports "Uncaught Exception in GameEngine::update".

Each iteration:

1. Calls the engine's `update()` through the engine object's vtable `+0x10`. That runs the client update and one logic step together.
2. Applies the frame limiter if all of these hold:
   - an object at `0x00a39598`, vtable `+0xc4`, returns less than 2 (most likely the game speed multiplier)
   - the function at `0x00437550` returns false (most likely "script is running time fast")
   - GlobalData `+0x924` is false, or the object at `0x00a39b0c` has `+0x94 != 3`
3. The limiter spins on `Sleep(0)` and `timeGetTime` until `1000 / FramesPerSecondLimit - 1` ms have passed since the last frame, as long as `UseFPSLimit` (GlobalData `+0x14`) stays on.

Relevant instructions:

```
00410102  8B 15 BC 81 A3 00   mov edx, [TheGlobalData]
00410108  8B D8               mov ebx, eax
0041010A  8A 42 14            mov al, [edx+0x14]      ; UseFPSLimit
0041010D  84 C0               test al, al
...       FF 15 CC 31 94 00   call [Sleep]
```

## Consequence

One loop iteration is one rendered frame and one logic step. Turning off the limiter makes the game run faster, not smoother. A high frame rate needs logic on its own fixed 30 Hz clock, with rendering in between interpolated.

## Engine objects and update order

No RTTI in this build. The engine registers subsystems by name in `0x0040e7e0` (engine init), which is how the globals were found.

| Global | Object |
|---|---|
| `0x00a39b0c` | `TheGameLogic` (`+0x94` game mode, `+0xa4` paused flag read by `0x004a4590`) |
| `0x00a39b18` | `TheGameClient` |
| `0x00a3984c` | `TheNetwork` (null outside multiplayer) |
| `0x00a37a8c` | `TheAudio` (probably) |
| `0x00a39b00` | Radar, updated through a second vtable at `+4` (probably) |
| `0x00a39d68` | CD manager (probably) |
| `0x00a37a94` | Message stream, `0x0040e0d0` propagates messages |
| `0x00a39598` | Tactical view, vtable `+0xc4` returns the game speed multiplier |
| `0x00a39580` | Script engine, `0x00437550` returns "time is fast" |

Game modes at `TheGameLogic + 0x94` (3 is replay, used with GlobalData `+0x924` for replay fast-forward).

Engine vtables: base at `0x00944004`, Windows engine at `0x00967b14`. Slots: `+0x10` update (`0x00741c90`), `+0x18` init (`0x0040e7e0`), `+0x1c` execute (the main loop, `0x0040ffc0`). Factories: `+0x50` game client, `+0x6c` particle system manager.

GameLogic::update (EA source) sets the client frame to the logic frame early on, and increments the logic frame at the end. So during an update's own draw the client frame is the previous logic frame, and redraws after the logic step see the new one.

- `0x00741c90`, Windows engine update: calls the base update, then if the window is minimized it sleeps in 5 ms steps servicing window messages until restored.
- `0x0040ff30`, base update: radar, audio, game client, message stream, network (if any), CD manager, then the logic step. Single player: the logic step runs unless paused. Multiplayer: it runs when `TheNetwork` vtable `+0x20` (frame data ready) returns true.
- The logic step is a tail jump at `0x0040ffa5`: `mov ecx, [TheGameLogic]; mov edx, [ecx]; jmp [edx+0x10]`.

## Client update and draw

The game client is created by Win32GameEngine vtable `+0x50` (`0x00741820`), which builds a `0xb8` byte W3DGameClient (constructor `0x00774670`, vtable `0x00969064`). Its `+0x10` slot is a thunk to `0x004a9b00`, the client update. The display is created by client vtable `+0x7c` (`0x00774690`), a `0xd8` byte W3DDisplay (constructor `0x0073be40`, vtable `0x009679b8`).

Client update `0x004a9b00`, in order: message stream, intro movie and legal page states (GlobalData `+0x819`, `+0x81b`), several subsystem updates (shell, window manager, keyboard, mouse and others), then drawable updates when time isn't frozen, then `TheDisplay->update()` (`+0x10`, `0x00421dc0`) and `TheDisplay->draw()` (`+0x14`, `0x0073dde0`). Draw happens inside the client update, so one engine update is one rendered frame.

W3DDisplay::draw `0x0073dde0`:

- Returns early if the window is minimized. Measures fps with QueryPerformanceCounter (`0x0073cb50`, average stored at display `+0x8c`).
- Decides "time frozen": tactical view frozen with its camera move unfinished, script or debug freeze, game paused, or **the client frame (client vtable `+0x5c`) is the same as at the last draw** (kept in `0x00a1ad3c`).
- When not frozen, adds the frame length (`0x00a1c554`, 33 ms) to the render clock `0x00a404b8` and passes it to WW3D::Sync (`0x0083d000`, stores current and previous sync time in `0x00a71d9c` and `0x00a71da0`). Model animation, water and similar effects run off that clock.
- Has its own copy of the frame cap: spins until 29 ms have passed while GlobalData `+0x14` is set.
- If the device's TestCooperativeLevel is OK, calls `this->updateViews()` (vtable `+0x60`, `0x004218a0`, which calls every view's `+0x14c`, the camera update) and `TheParticleSystemManager->update()` (`0x00a39ccc`, vtable `+0x10`). Both advance per call, which is why they ran too fast when the client update ran every frame.
- Renders the views, UI, mouse and other overlays, then presents.
- Loops back to the top while time is frozen and the tactical view says its camera move isn't finished (view `+0x60`). This is how scripted camera moves play while the game is frozen.

## Our split: logic at 30 Hz, client every frame

- The call to GameEngine::update at `0x00741c94` inside Win32GameEngine::update goes to our function. Window messages are still pumped every frame, since that happens after this call.
- A tick is due when `1 / FramesPerSecondLimit` seconds (30 per second) have built up. Time owed after a slow frame is made up on the following frames, capped at 3. On a tick we call GameEngine::update with its client update (`0x0040ff49`, 11 bytes) and message propagation (`0x0040ff54`, 11 bytes) turned off, so it runs radar, audio, network, CD manager and the logic step. Then, on every frame, we call the client update (TheGameClient vtable `+0x10`) and propagateMessages (`0x0040e0d0`, `this` = `0x00a37a94`).
- So the logic step now runs before the client in a tick frame. GameLogic::update sets the client frame, so the client update and draw right after it see a new frame: drawable updates, the render clock step and the drawable pass all happen in the tick frame. Every other frame sees the same client frame and is frozen, the same way the original spun its client in network games. No save and restore of `0x00a1ad3c` is needed any more.
- The logic step tail jump (`0x0040ffa5`, 11 bytes) goes to our stub. It counts logic steps, notes when the step was due (for the blend factor), records model transforms during the step, and skips the step while the tactical view has an unfinished camera move in frozen time (view vtable `+0x60` false and `+0xc0` true, or script debug freeze `0x00437500`, or script freeze `0x004374d0`), outside multiplayer. That replaces the draw's frozen-time loop (`0x0073e4a5`, now always jumps to the exit `0x0073e4c0`), which stopped the whole engine inside the draw until the move finished.
- Commands from the translators collect in TheCommandList between ticks and reach the logic (or the network) at the next tick, so the simulation is untouched.
- Two blend factors: time since the last logic step was due, for state the simulation owns (models, things, bones, particles, treads, lines, wave grid, render clock), and time since the last tick, for client animations that step on the tick (window slides, transitions, the camera's tick-driven moves). They differ while the game is paused, when ticks continue and the logic doesn't.
- Exceptions copied from the original limiter: when the speed multiplier is above 1, scripted time is fast, or a replay plays fast-forward, a tick runs every frame and both blend factors are 1.
- GlobalData `+0x14` (`UseFPSLimit`) is cleared when the game loads D3D, which turns off both copies of the cap. Frame rate then follows vsync or our `fps_limit`.
- The game resets GlobalData to its INI values between matches. That turned the cap back on: in the second skirmish of a session the log showed render fps fall from about 290 to 31 while loading it. It would also clear the windowed flag (`+0x20`) we set for borderless. Both are now written again at the start of every frame.
- The log prints render fps, ticks per second and logic steps per second every 5 seconds.

The first attempt gated only the logic step at `0x0040ffa5` and let the client update run every frame without fixing its per-call steps. Camera, UI animations, particles, tree sway and water all sped up. The current version fixes each of those steps, see [smoothness.md](smoothness.md).

## Client update in the binary

`0x004a9b00`, matches EA's GameClient::update. In order: append MSG_FRAME_TICK (type 1) through the message stream vtable `+0x28`, intro movie and legal page states, TheSnowManager `0x00a39e28` `+0x10`, Anim2D collection `0x00a39fd0` `+0x10`, TheKeyboard `0x00a37a90` update `+0x10` and createStreamMessages `+0x1c`, `0x00a39fb8` `+0x10` (constructor `0x00513570`, vtable `0x0094df18`, update `0x005137e0`, checks events against the logic frame, probably EVA), TheMouse update `+0x10` and createStreamMessages `+0x20`, camera tracking of the selected unit (`0x004a9e4d`, TheInGameUI `+0x1d24`, lookAt through view vtable `+0x4c`), TheWindowManager `0x00a39e14` `+0x10`, video player `0x00a39fd4` `+0x10`, the time freeze check, drawable updates only when the client frame changed (static last frame `0x009e5f14`), TheTerrainVisual update, TheDisplay update and draw, display string manager `0x00a385dc` `+0x10`, TheShell `0x00a3a5bc` `+0x10`, TheInGameUI `+0x10`.

- Keyboard: Keyboard::update `0x0040a620` is `inc [ecx+0x160c]` (input frame) then updateKeys. Key repeat counts input frames. Vtable `0x0096a2e4` (constructor `0x007ac440`), update entry `0x0096a2f4`.
- Mouse: W3D mouse vtable `0x0096a318` (constructor `0x007ac650`), update entry `0x0096a328` jumps to Mouse::update `0x00403490`, `inc [esi+0x4a44]` (input frame). Button events stamp it.
- Window manager update: processDestroyList and TheTransitionHandler->update (EA source). Window input arrives through the WindowTranslator in the message stream, not here.
- Shell::update `0x005c57e0` (vtable `0x009517c4`, constructor `0x005c53a0`): runs screen update callbacks, the animate window manager and the scheme manager only when `timeGetTime() - [0x00a3a5b4]` reaches the float at `0x009517dc` (32.33 ms). Compare at `0x005c581b`.
- InGameUI::update `0x0050a730`: video streams, message fades (logic frame based), military subtitle (counts its timers back once per call while script time is frozen, check at `0x0050a938`), floating text update `0x005108e0` (bumps each text's frame count only when the logic frame changed), money and power displays, ControlBar::update (TheControlBar `0x00a395f8` vtable `+0x10`), layout update callbacks, then the rotate keys (`+0x1d21`, `+0x1d22`: `setAngle(getAngle() -/+ GlobalData[0x91c])` at `0x0050ae5d` and `0x0050ae9b`) and zoom keys (`+0x1d23`, `+0x1d25`: view `+0x104` zoomIn at `0x0050aecd`, `+0x108` zoomOut at `0x0050aeef`).
- View::zoomIn `0x0044db30` is `setHeightAboveGround(getHeightAboveGround() - [0x00945ec8])` (10.0) through view vtable `+0xfc` and `+0x100`. The zoom itself only follows in W3DView::update, which eases toward that height.
- ControlBar::update `0x0045e5b0` (EBP is the control bar): getStarImage `0x0045ebd0` (logic frame based), radar attack glow countdown (`+0x320` on, `+0x324` frames left, toggles every 15, `0x0045e5d2` to `0x0045e62b`), scheme manager update `0x005a6ab0` (steps scheme animations per call, called at `0x0045e632`), video manager `+0x8`, animate window manager `+0xc` (`0x0045e64a`) and the general shortcut one `+0x10` (`0x0045e70a`), special power shortcut update `0x00462320`, observer info (logic frame % 15), command button flash (`+0x284`, decrements when the client frame % 10 is 0, `0x0045e822` to `0x0045e8dd`), then context updates (state based).

Interpolation:

- Camera: see [camera.md](camera.md). The draw stub at `0x0073e07f` puts the real values back, runs updateViews and the particle update every frame, then applies the blends. The W3DDisplay vtable draw entry (`0x009679cc`) points to a wrapper that puts the real values back after the draw.
- The logic blend factor is the time since the last logic step was due divided by the step length, capped at 1. In fast mode it's always 1.
- Models (units, buildings, projectiles): `crates/d3d8/src/objects.rs`, details below.

## One-shot draw requests

Some UI state is requested on every control bar update and cleared by the draw that uses it.

- Build and upgrade clock on control bar buttons: the push button data (window user data, `0x004f9870`) starts with a byte that asks for a clock (1 normal through display vtable `+0x8c`, 2 inverse through `+0x90`), with percent and color at `+4` and `+8`. The image draw `0x007cdc30` (from the `W3DGadgetPushButtonImageDraw` callback `0x007cdb90`) and two other push button draws (`0x007ce1a0` and code around `0x007cd970`) draw it, then run `mov byte ptr [esi], 0; call 0x00445fe0`, at `0x007cdec3`, `0x007ce5d8` and `0x007cd9d7`. With redraws between updates the clock flashed, and we skipped the clear on redraws. Since the control bar update now runs every frame, the request is renewed every frame and that patch is gone.

## How models get positioned

- At the end of the W3DView camera update (`0x0076d030`), if the WW3D sync time moved since the last draw (so not on our redraws and not while paused), it walks the drawables in view through the client's vtable `+0x44`, with the callback `0x0076dac0` pushed at `0x0076da9b`. The callback calls Drawable::draw (`0x00472fd0`) with the view.
- Drawable::draw hands its matrix to its draw modules, which end up in RenderObjClass::Set_Transform. The base version is `0x00850e80`: it copies 12 floats to `+0x1c`, sets the is-identity byte `+0x80` (exact bit compare against identity), and clears bit `0x2000` of `+0x18` (bounding volumes valid). Overrides (camera, animated models) call it and then do their own invalidation. The virtual slot is vtable `+0x4c`.
- Render objects are reference counted: count at `+4` (the RenderObjClass constructor `0x00850ae0` sets it to 1), and when it drops to 0 the vtable slot 0 (Delete_This) frees it.

Measured in a skirmish: about 10,000 to 25,000 Set_Transform calls per second, and almost none of them inside the drawable pass. So unit positions are pushed to their models outside the pass, during the game update (most likely when logic moves an object). The drawable pass only set transforms while a map was loading.

## Our model blend

- `0x00850e80` jumps to our copy of Set_Transform, which does the same three writes and records the matrix per render object. It records during the logic step and the client update right after it (the only client update that sees a new frame), but not inside the display draw (the draw wrapper turns it off), except in the drawable pass callback, which we replace with a wrapper that turns it back on. Cameras (vtable `0x00973288`) are skipped, they have their own blend.
- Each recorded render object keeps the transform from before the update that last moved it, and the latest one. We hold a reference on it, so an object destroyed by logic stays valid until we drop it.
- Before each render, objects moved in the current game update are set to the blend through their virtual Set_Transform. After the draw, each gets back what it held before. During an update's own draw this is close to the old position, which is right because the blend factor is near 0 there. Objects not moved for 60 updates are dropped and their reference released.
## Animation clock

- The render clock `0x00a404b8` is integer milliseconds. An unfrozen draw adds the frame length (`0x00a1c554`, 33) at `0x0073dfa3`, then calls WW3D::Sync with it at `0x0073dfb5`. WW3D keeps current and previous sync time at `0x00a71d9c` and `0x00a71da0`. Over 100 places read the sync time: model animation, texture animation, decal fades and more.
- The camera update runs the drawable pass only when the current sync time differs from the previous one (`0x0076da84`).
- Frozen draws still call WW3D::Sync with the clock (`0x0073df9f`: `test bl, bl; jnz` skips only the add). We replace `0x0073df9f` to `0x0073dfba` (27 bytes) with a stub: an unfrozen draw adds the frame length to the clock, and every draw hands Sync the clock plus the elapsed part of the next step, at most step minus 1 so the next step still moves it. It also sets our "unfrozen draw" flag, which replaces the sync check before the drawable pass (`0x0076da84`), since Sync time now moves every frame. Animation runs at the display rate and still moves 33 ms per logic step in total.

## Things that are not the limiter

`0x0082ca70`, `0x0082c9a0` and `0x0082cc30` call `timeGetTime` and `Sleep(0)`, but they wait for a thread to exit and kill it after a timeout. `0x0082ccf0` is a plain `Sleep` wrapper.

## Timing calls

- `timeGetTime` is used in 60+ functions.
- `QueryPerformanceCounter` is used in `0x0047c290`, `0x0073cc40`, `0x0073db70` and `0x0073cb50`.
