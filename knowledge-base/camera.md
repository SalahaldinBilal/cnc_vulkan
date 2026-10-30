# Camera

Found by accident while looking for the control bar drawing code. Not studied in depth yet. Names in this file are educated guesses from the code's shape unless marked confirmed.

## The tactical view object

Both functions below are reached only through a vtable, with entries at `0x00968bb0` and `0x00968bd4`. That vtable most likely belongs to the 3D tactical view (the W3D view class). Fields used by these functions:

| Offset | Probably |
|---|---|
| `+0x0c`, `+0x10`, `+0x14` | current camera position (x, y, z), copied as the start of a move |
| `+0x13c` | number of waypoints in the current camera path |
| `+0x140` (`0x50` dwords) to `+0x15c` | start and target positions of the move |
| `+0x158` | camera path waypoints, 3 floats each |
| `+0x284` (`0xa1` dwords), `+0x288`, `+0x28c` | cleared when a move starts |
| `+0x2f8` | one angle per waypoint, written by the path setup |
| `+0x2f0` (`0xbc` dwords) | a float from vtable `+0xdc`, saved at move start |
| `+0x430` (`0x10c` dwords) | move duration, clamped to at least 1 |
| `+0x438`, `+0x43c`, `+0x440`, `+0x444` | move progress counters and flags |
| `+0xe8` | byte, skips the path angle setup when set |
| `+0x458`, `+0x459` | bytes set when a one-step move finishes immediately |

## Functions

- `0x0076f7d0`: starts a camera move to a target position. Arguments look like (target position, duration, shutter, orient flag, ease in, ease out). It splits the ease values by the duration (`0x007fbbd0(easeIn / duration, easeOut / duration)`), sets up the path with `0x00770c10`, and if the duration is 1 it finishes right away through `0x00771580(1)`. This matches a "move camera to" script action.
- `0x00770050`: walks the path waypoints at `+0x158` and computes a facing angle for each one with `acos`, wrapped into a fixed range. Stores the angles at `+0x2f8`.
- `0x007702e0` (inside a function Ghidra didn't mark): shifts every waypoint in the path by an offset, 3 floats per waypoint, `+0x13c` entries.
- `0x00770c10`: path setup called when a move starts.
- `0x00771580`: completes or advances a move.

## W3DView (confirmed)

- `TheTacticalView` (`0x00a39598`) is created by the in-game UI's vtable `+0x1a0` (`0x007a8be0`), which allocates `0x4b4` bytes and runs the W3DView constructor `0x0076b990`.
- Vtables: main `0x00968b58`, second one at object `+0x7c` is `0x00968b40`. Main `+0x14c` (`0x0076d020`) jumps to the second vtable's `+0x10`, `0x0076d030`, the per-draw camera update. Display updateViews (`0x004218a0`) calls `+0x14c` on every view.
- Init `0x0076cd20` sets the start position (`+0xc`, `+0x10`, `+0x14`) and creates two WW3D cameras (`0x38c` bytes each, constructor `0x008553e0`): `+0x84` is the 3D camera, `+0x88` the 2D one.
- `0x0076cb00` is setCameraTransform: sets the clip planes (near 10, far 1200 or 12000), builds the matrix with `0x0076c060`, calls `camera->Set_Transform` (`+0x84`, vtable `+0x4c`), then tells the terrain (`0x00a48b30`, vtable `+0x1e0`) the new camera. It has 13 callers: the camera update, init, and setters for angle, pitch, zoom, position and so on.
- The camera update `0x0076d030` handles follow-unit, scripted path moves (`0x0076cf80`), zoom limits against terrain height and shake, then calls setCameraTransform only if something changed. At the end, if the WW3D sync time moved, it updates drawables in view through the client's vtable `+0x44`.

## WW3D camera (confirmed)

- CameraClass vtable `0x00973288`. `+0x4c` Set_Transform (`0x00855bc0`) copies the matrix through RenderObjClass Set_Transform (`0x00850e80`) and clears the frustum-valid byte `+0xcc`.
- RenderObjClass keeps its transform at `+0x1c`, a Matrix3D of 3 rows by 4 floats (rotation columns then translation), with an is-identity byte at `+0x80`. Set_Transform clears bit `0x2000` of `+0x18`.

## View fields (confirmed in buildCameraTransform `0x0076c060` and the update)

View layout matches EA's View class: `+0xc` position, `+0x28` angle, `+0x2c` pitch, `+0x40` zoom, `+0x44` height above ground, `+0x54` current height above ground, `+0x58` terrain height under the camera. W3DView adds: `+0x98`, `+0x9c` shake offset, `+0xa0`, `+0xa4` shake direction, `+0xa8` shake intensity, `+0xac`, `+0xb0`, `+0xb4` effect rotations (FXPitch and two more), `+0xe8` doing rotate, `+0x110` doing pitch, `+0x134` doing zoom, `+0x135` scripted lock, `+0x458` doing waypoint path, `+0x470` camera offset, `+0x488` scroll amount, `+0x494` ground level, `+0x498` to `+0x4a4` position constraint, `+0x4a8` constraint valid.

Vtable slots: `+0x4c` lookAt, `+0x54` scrollBy (`0x0076e6d0`), `+0x60` isCameraMovementFinished, `+0xc0` isTimeFrozen, `+0xc4` time multiplier, `+0xd8` setAngle, `+0xdc` getAngle, `+0xfc` getHeightAboveGround, `+0x100` setHeightAboveGround, `+0x104` zoomIn, `+0x108` zoomOut, `+0x150` getCameraLock, `+0x160` getCameraLockDrawable.

scrollBy, setAngle, setPitch, setZoom, setHeightAboveGround and lookAt all call setCameraTransform themselves (EA source), so input moves the camera at once. setHeightAboveGround only sets the wanted height: the zoom follows in W3DView::update.

## W3DView::update per-call steps

`0x0076d030`, ESI is the view + 0x7c. Matches EA's source.

- Unit follow block, `0x0076d083` to `0x0076d711`: a static follow factor `0x00a1c558` ramps by 0.05 per call, the position moves toward the unit by that factor per call, tether and angle follow use fixed factors per call.
- updateCameraMovements `0x0076cf80`, called at `0x0076d731`: rotateCameraOneFrame, pitchCameraOneFrame, zoomCameraOneFrame (integer frame counters) and moveAlongWaypointPath (milliseconds from `0x00a1c554`, plus a shutter counted per call).
- Shake, `0x0076d781` to `0x0076d7d3`: intensity times 0.75 and the direction flips every call.
- Zoom easing toward the wanted height: `zoom += (wanted - zoom) * GlobalData[0x7e4]` per call, at `0x0076d9e4` (while scrolling, only out of bounds) and `0x0076da14` (at rest).
- setCameraTransform when something changed, then the drawable pass when sync time moved (`0x0076da84`).

## Our camera handling

`crates/d3d8/src/camera.rs`. W3DView::update runs every frame.

- Mouse and keyboard move the camera directly every frame. scrollBy is hooked through its vtable entry `0x00968bac`: while the message translators run, the delta is multiplied by real frame time in game updates, since the look-at translator scrolls by a fixed amount per frame tick message, and the scroll amount at `+0x488` keeps the unscaled value for the cutoff check. The keyboard rotate and zoom steps are scaled in InGameUI::update.
- The zoom easing reads our rate `1 - (1 - k)^frames` instead of `k`.
- Unit follow, updateCameraMovements and shake only run on the 30 Hz tick. Between ticks updateCameraMovements reports whether a scripted move runs, without stepping. On a tick we snapshot the view fields the camera is built from (position, angle, pitch, zoom, shake offset, effect rotations, ground level) before and after the update. Before each render the view is set back by the part of that change not yet due (tick blend factor), setCameraTransform runs, and after the draw the real fields go back with another setCameraTransform. Skipped while a camera lock drawable is set, since that mode sets the camera matrix directly.
- The blended fields have to cover everything buildCameraTransform reads that the tick changes. It reads (Generals `0x0074c860`, Zero Hour `0x0076c060`): position x and y, angle, pitch and zoom (through the view vtable), shake offset, ground level, Zero Hour's three effect rotations, the effect pitch (FXPitch, Generals `+0x128`, Zero Hour `+0x138`), the camera offset (Generals `+0x530` to `+0x538`, Zero Hour `+0x470` to `+0x478`) and the position limits with their valid flag (Generals `+0x558` to `+0x568`, Zero Hour `+0x498` to `+0x4a8`). The scripted pitch step (Generals `0x00751720`, Zero Hour `0x007714e0`) writes the effect pitch once per tick. It was missing from both field lists, so scripted cameras (the main menu map, mission cameras) jumped on every tick frame while everything else moved smoothly, and hard shadow edges showed it most. The waypoint path step (Generals `0x007517a0`) writes position, angle and ground level, recomputes the camera offset from constants, and only grows the position limits, so a blended position stays inside them.- The drawable pass check reads our "unfrozen draw" flag, see [frame-loop.md](frame-loop.md).

An earlier version blended the camera matrix between game updates. It added up to 33 ms of delay to every input and made rotation twitch, because a matrix blend doesn't follow an orbit.

## Camera height limits (confirmed)

- View fields `+0x38` maximum height above ground, `+0x3c` minimum (EA's `m_maxHeightAboveGround`, `m_minHeightAboveGround`). setHeightAboveGround clamps the wanted height to them while `m_zoomLimited` is set, which it always is.
- View vtable `+0xfc` getHeightAboveGround `0x0044d920`, `+0x100` setHeightAboveGround `0x0076e980` (W3DView), `+0x104` zoomIn `0x0044db30`, `+0x108` zoomOut `0x0044db60`.
- View::init `0x0044daa0` (called from W3DView::init `0x0076cd20`) copies GlobalData `+0x178` (`MaxCameraHeight`, 310 in GameData.ini) to `+0x38` at `0x0044dae1` (`mov edx, [edx+0x178]`) and `+0x17c` (`MinCameraHeight`, 120) to `+0x3c`.
- W3DView::setDefaultView `0x0076e950` (vtable entry `0x00968c24`, args pitch, angle, maxHeight, callee pops 12): `+0x50` = pitch, `+0x38` = maxHeight × GlobalData `+0x178` (`fmul` at `0x0076e961`), raised to `+0x3c` if lower. Maps call it through scripts.
- No other code reads GlobalData `+0x178` (searched every FPU and `mov` read of `+0x178`).
- setCameraTransform `0x0076cb00`: near plane 10, far plane 1200, or 12000 when GlobalData `+0x3f` (`DrawEntireTerrain`) is set, the pitch is under a threshold or the zoom over one.
- The terrain is drawn in a window of 129 by 129 cells around the camera (`WorldHeightMap::NORMAL_DRAW_WIDTH`, EA source), 1290 world units. The WorldHeightMap constructor `0x00743b70` sets the window (`+0x1a128c`, `+0x1a1290`) to 129, to 65 when GlobalData `+0x3d` (`StretchTerrain`) is set, and to the whole map (width and height at `+0xc`, `+0x10`) when DrawEntireTerrain is (read at `0x0074414c`: `mov eax, [TheGlobalData]; cmp eax, ebx; jz; cmp [eax+0x3f], bl`). WorldHeightMap::setDrawOrg `0x00747540`, called as the camera moves, applies the same two flags again (`0x0074757e`).
- setCameraTransform reads DrawEntireTerrain at `0x0076cba5` (`mov eax, [TheGlobalData]; test; jz; mov cl, [eax+0x3f]`).

## Zooming out further

`crates/d3d8/src/zoom.rs`, the "Camera zoom out" setting (100% to 400% of the game's limit) and "Camera zoom speed".

- W3DView::setHeightAboveGround `0x0076e980` (vtable entry `0x00968c58`, thiscall, one float, callee pops 4) stores the wanted height at `+0x44`, clamps it to `+0x3c` and `+0x38` while `+0x48` (zoom limited) is set, cancels scripted camera moves and calls setCameraTransform. Zoom input reaches it only through the vtable (View::zoomIn `0x0044db30`, zoomOut `0x0044db60`, our held-key zoom in `client.rs`).
- Our wrapper raises `+0x38` by the factor only while the original runs. The first version multiplied MaxCameraHeight where View::init and setDefaultView read it, so `+0x38` itself grew. The default zoom (`setZoomToDefault`, which goes to the maximum height at the start of a map) and scripted cameras that work relative to it then started far out too: the main menu map's camera began at the new maximum.
- The zoom step: View::zoomIn and zoomOut move by `[0x00945ec8]` (10.0) per call, once per wheel notch, and `client.rs` uses the same step for a held zoom key, scaled by frame time. Both now read our step instead (the address in `fsub`/`fadd` at `0x0044db3c` and `0x0044db6c` points at it): the game's step times the zoom out factor times the speed setting. With the factor in it, the whole range from the lowest to the highest zoom takes about as many notches as the game's did. How fast the camera then glides to the wanted height is CameraAdjustSpeed (`camera.rs`), unchanged.
- Above 100% the whole terrain is drawn. Both `mov eax, [TheGlobalData]` before a DrawEntireTerrain read (constructor and setCameraTransform) became `mov eax, <our flag - 0x3f>`, so they read our flag and GlobalData resets don't matter. The far plane goes to 12000 at once, reversed depth keeps the precision.
- setDrawOrg's DrawEntireTerrain check became `cmp esi, esi` (always equal, so skipped). The first version set the GlobalData flag mid-map: setDrawOrg then resized the window to the whole map on the next camera move while the terrain's buffers kept the size the map loaded with. The main menu map showed only water where the terrain was, and spent 31 ms per frame rebuilding terrain (8 MB to buffers per frame). Now the window only changes size when a map loads, so a new setting reaches the terrain with the next map, and the camera height at once.
- HeightMapRenderObjClass::oversizeTerrain `0x0079b710` (vtable entry `0x00969b68`, thiscall, tiles) is the other writer of the window: it sets it to 129 cells plus 32 per tile (1 to 4, otherwise none), capped at the map, recenters it and rebuilds the terrain's buffers (vtable `+0x1d8`). Mission scripts call it, so on a campaign map loaded with the whole terrain the window went back to 129 cells: at 400% the terrain was cut off around the camera with trees and buildings standing in black, and the radar map fell back to tile textures. Our wrapper skips the call while the terrain holds the whole map, which already covers any oversized window. Without the whole terrain it runs as before.
- In a skirmish at 200%: 2,000 to 3,000 draws and 116 to 134 fps, the game's own time about 6 ms per frame.
- The game never culled terrain: the window around the camera was all it had loaded, and HeightMapRenderObjClass::Render draws every vertex buffer tile of the window in every pass. With the whole terrain loaded that is every tile of the map, on or off screen, at any zoom. A moving window big enough for 400% isn't an option (resizing breaks the buffers, moving a big window rebuilds big strips). Instead the device skips draws that can't reach the screen, see [vulkan-backend.md](vulkan-backend.md) (draw culling).

## Camera constraints (confirmed)

- calcCameraConstraints: Zero Hour `0x0076c810` (also reached through the thunk `0x0076bc60`), Generals `0x0074cbc0` (and a caller at `0x0074c4c0`). Same code in both, matching EA's W3DView. It takes TheTerrainLogic's extent (Zero Hour `0x00a39594`, Generals `0x00a6b958`, vtable `+0x1c`, getExtent), which is the active boundary: 0 to the boundary's cells × 10, not the heightmap border. It picks the ground plane at the view's ground level (Zero Hour `+0x494`, Generals `+0x554`) under the screen center (`[0x00945dfc]` = 0.5 of the height) and under the point 95% down the screen (`[0x009497b0]` = 0.95), and takes the margin as `sqrt(groundLevel² + dx² + dy²)` (EA's Coord3D length with z left at the ground level). With GlobalData `+0x71c` (debug AI) set, the margin is `[0x00967cc8]` = -1000 instead (Zero Hour, Generals `[0x009a5948]`), so the camera may go 1000 units past every edge.
- The limits are the extent shrunk by the margin on every side, stored at Zero Hour `+0x498` (lo x), `+0x49c` (lo y), `+0x4a0` (hi x), `+0x4a4` (hi y), valid flag `+0x4a8` (Generals `+0x558` to `+0x564`, `+0x568`). setCameraTransform (Zero Hour `0x0076cb00`, Generals `0x0074ceb0`) computes them when the flag is clear, then clamps the view position (`+0xc`, `+0x10`) as `min(hi, max(lo, p))` and writes it back. buildCameraTransform clamps its own copy (position plus camera offset) the same way. setHeightAboveGround, setZoom, setZoomToDefault, a new map and a boundary change clear the flag, scrolling doesn't.
- The margin is the ground distance from the screen center to near the bottom edge, so it grows with the camera height. Zooming out past the game's limit shrinks the area the camera may move in on every side by the same factor. When the margin passes half the extent, lo > hi and the clamp pins the position to hi, the far corner rather than the middle.
- Off the heightmap, ground height comes from getHeightMapHeight, which falls back to getClipHeight at the unclamped cell (EA source, not checked in game.dat).
- The camera's height: W3DView::update (Zero Hour `0x0076d8e4`, Generals `0x0074dc42`, `this` is the view + `0x7c` there) stores the terrain height under the camera at `+0x58` and the current height above it at `+0x54` as zoom (`+0x40`) × camera offset z (Zero Hour `+0x478`, Generals `+0x538`) minus `+0x58`, then eases the zoom toward (wanted height `+0x44` + `+0x58`) / offset z. The eye sits at zoom × offset z, so its height over the ground level plane is that minus the ground level, and the ground distances the margin is made of scale with it.
- Zero Hour's "real zoom" camera mode (`+0x4aa`, set by the view vtable entries at `0x00968ce4` and cleared by `0x00968ce8`, script actions) zooms by field of view instead (`+0x6c`), and buildCameraTransform doesn't scale the offset by the zoom then. Generals has no such mode.

### Ours: the same reach at any zoom

`zoom.rs` wraps calcCameraConstraints at its call in setCameraTransform (Zero Hour `0x0076cc1d`, Generals `0x0074cfbb`) and its vtable entry (Zero Hour `0x00968cd0`, Generals `0x009a68dc`, both pointing at the thunk). After the game's code, when the camera is higher above the terrain than the view's maximum (`+0x38`, the game's own limit, which our zoom only raises during setHeightAboveGround), it takes the margin back out of the limits (lo x minus the extent from getExtent), removes the ground level part, scales the rest by (maximum + terrain - ground level) / (eye height - ground level), which is what it would be with the camera at the maximum, and writes the limits again. At or below the game's maximum nothing changes, and neither in the real zoom mode or with the debug AI margin (negative). So zoomed out past the game's limit, the camera can center on the same points as at the limit, and the extra view past the edges is black.

Constraints are only recalculated when the height or zoom is set, not while the zoom eases, which the game does the same way. A recalculation at the old height followed by easing outward keeps the margin from the old height, which is never more than at the maximum.

## Past the terrain's edge

Zoomed out, the camera sees past the edge of the heightmap, where nothing is drawn (black). Between the playable area and that edge, the heightmap's border is darkened by the shroud texture's border texel.

- **Roads stuck out at full brightness** there (seen in Generals' first China mission). HeightMapRenderObjClass::Render draws the roads (drawRoads) without depth writes, then the shroud pass (`renderTerrainPass` with the shroud material, a multiplicative blend and an equal depth test) redraws the terrain tiles over them, so a road is only darkened where terrain lies under it. drawRoads skips a segment only when both ends lie past the same edge of its bounds, so a segment crossing the terrain's edge runs on past it. The game's own camera never saw that far.
- drawRoads: Zero Hour `0x00795d00`, Generals `0x00774dd0`. Thiscall on the road buffer (Zero Hour terrain `+0x283c`, Generals `+0x2860`), nine arguments: camera, cloud texture, noise texture, disable textures (a byte pushed as a dword), min x, max x, min y, max y, the dynamic lights iterator, `ret 0x24`. The bounds are cells from the playable area's corner: the terrain's draw origin to origin plus its vertex count minus 1, minus the border (WorldHeightMap `+0x1a1284`, `+0x1a1288` and render object `+0x9c`, `+0xa0` in Zero Hour, `+0x1a1274`, `+0x1a1278`, `+0x27c8`, `+0x27cc` in Generals). drawRoads scales them by 10 (`[0x00945ec8]` in Zero Hour).
- Calls: Zero Hour `0x0079d587` (HeightMapRenderObjClass::Render) and `0x0079854c` (another terrain render with the same layout, most likely FlatHeightMapRenderObjClass, whose bounds are the union of its visible 16-cell tiles, EA source), Generals `0x00733a1d`.
- The terrain's vertices sit at `(cell - border) * 10`, with the cell running over the draw window (Generals' tile fill `0x00729650`: `(x - [map + 0x10]) * [0x00989ee8]`, which is 10.0).
- Ours (`terrain.rs`): each call goes to a wrapper that sets four user clip planes around the bounds, limited to the heightmap (`-border` to `width - 1 - border` cells, from TheTerrainRenderObject's WorldHeightMap), and calls drawRoads. The road ends exactly at the terrain's edge. User clip planes are ours to implement, see [vulkan-backend.md](vulkan-backend.md).

## Not found yet

- Field of view and the projection setup are separate from this code.
