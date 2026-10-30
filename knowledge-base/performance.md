# Performance

Where the main thread's time goes with the logic split. Measured with the sampling profiler (`profile = true`, see [tooling.md](tooling.md)) in a Generals skirmish at 100% render scale without MSAA: 100 to 143 fps with 900 to 2,100 draws per frame, 82 fps zoomed out with 3,360. The GPU waited 0.07 ms per frame, so the frame is bound by the main thread. Addresses on this page are Generals'. Zero Hour's aren't mapped yet.

## Reading the profile

- "Where the main thread was" is exact (the sampled EIP). Self time comes from it.
- Both "under" lists come from scanning the stack for return addresses, and stale values left in stack frames get counted. They overstate, sometimes by a lot: ControlBar's command availability check (`0x005b4870`, the function that names `BattlePlanUpdate`) showed 5 to 10% under its call, while the whole client update outside the draw measured 0.2 ms per frame.
- `ours+offset` resolves with the linker map of the same build (last entry at or below `0x10000000 + offset`).

## The split

- **About 35 to 40% was our DLL and the driver under it** (before the changes below). memcpy into GPU buffers at Unlock about 7% (4.8 to 5.5 MB per frame, copied once by the game into our shadow copy and again by us), the driver inside DrawIndexedPrimitive 5 to 7% (much of it writing descriptor sets, whose cache was cleared every frame), the Present path about 6%, `things.rs` hash maps 6 to 8% (`record` rebuilt a SipHash map of every drawable and object, twice on tick frames).
- **The rest is the game**, spread over many functions. None is above about 3% self. W3DDisplay::draw is 5.8 to 10.7 ms of each frame, the client update outside it about 0.2 ms, the engine update (logic) 0.3 to 0.6 ms per frame on average, all of it on tick frames.

## The scene in Generals

| What | Address |
|---|---|
| WW3D::Render (calls the scene's Render, vtable `+0x54`) | `0x007fecc0` |
| RTS3DScene::Render | `0x007470f0` |
| RTS3DScene::Customized_Render (vtable `+0x58`), terrain Render call | `0x007477f0`, `0x007478ee` |
| renderOneObject | `0x007465c0` |
| RTS3DScene::Flush | `0x00746cd0` |
| DoShadows (`rinfo`, stencil pass) | `0x00761860` |
| Projected shadows render, decal geometry on terrain (floor and ceil per vertex) | `0x0073ec20`, `0x0073d990` |
| Stencil shadow volumes render, volume update (terrain height lookups, asin) | `0x0077dcf0`, `0x0077b2e0` and `0x00779e60` |
| TheDX8MeshRenderer.Flush | `0x00828c00` |
| flushOccludedObjectsIntoStencil (only with stencil) | `0x00748000` |
| WW3D::Render_And_Clear_Static_Sort_Lists | `0x007ffa10` |
| flushTranslucentObjects | `0x007490e0` |
| DoParticles, particle buffer render (from `0x0075824f`) | `0x00757d20`, `0x00836750` |
| SortingRendererClass::Flush, its pool flush, sort | `0x00809900`, `0x00808c70`, `0x0080a680` |
| DX8Wrapper::Draw (DrawIndexedPrimitive at `0x007fafe2`), Draw_Triangles, Apply_Render_State | `0x007faf10`, `0x007fb090`, `0x007fb0d0` |
| Texture apply (SetTexture at `0x007f7e85`) | `0x007f7e40` |
| Dynamic VB and IB access release (the game's Unlock of the shared dynamic buffers) | `0x0081e200`, `0x00820dd0` |
| Display::drawViews' drawView call (view `+0x13c`), updateViews' update call (`+0x140`) | `0x00424562`, `0x0042458c` |
| Window draw recursion (each window's draw callback, then its children) | `0x004f9120` |
| Message pump GetMessageA call | `0x0071adfd` |
| Game allocator, Enter and LeaveCriticalSection on every block | `0x00413910`, `0x00413a40`, `0x00413150`, `0x00414b70` |
| TheDX8MeshRenderer.Flush call inside the projected shadow render (the opaque models are drawn under it) | `0x0073f170` |
| Texture category render's call into the polygon renderer `0x00827890` | `0x00825e6f` |
| Bone and hierarchy transform update (called at `0x0086107c`, also in the vtable at `0x009b3d90`) | `0x0089b5c0` |
| RTS3DScene vtable Render (`+0x54`) and Customized_Render (`+0x58`); Render calls Flush at `0x00747225`, `0x007472cb`, `0x00747413`, `0x007474fc`, `0x007475e1`, `0x0074774a` | `0x009a6414`, `0x009a6418` |

Every caller of these, as timed by `render_timers.rs` (all `call rel32`):

- TheDX8MeshRenderer.Flush `0x00828c00`: `0x007fee9d`, `0x007fef11`, `0x007fefda`, `0x007ff049` (WW3D), `0x0073f170` (projected shadows), `0x00746cf9` (RTS3DScene::Flush), `0x007486f2`, `0x00748789`, `0x007489d0`, `0x00748b8e` (flushOccludedObjectsIntoStencil), `0x00749152` (flushTranslucentObjects), `0x007ffa83` (static sort lists), `0x00748f2f`, `0x00748fac` (`0x00748c60`), `0x007ff087`, `0x007ff0fb` (`0x007ff080`).
- SortingRendererClass::Flush `0x00809900`: `0x007fef2c`, `0x007ff130`, `0x007ff067`, `0x00746d70`, `0x007ff11a`.
- DoParticles `0x00757d20`: only `0x00746d68`. scene.rs hooks the sorting flush call right after it (`0x00746d70`), which the timers chain through. The particle buffer render `0x00836750` is called at `0x0083990f`, `0x00895fe6` and `0x0075824f`.
- The window draw recursion `0x004f9120`: `0x004f920c`, `0x004f922e`, `0x004f9250` from `0x004f91f0`, and itself at `0x004f91ac`.
- Decal geometry `0x0073d990`: `0x0073edd1`, `0x0073f1d5`. Stencil volume render `0x0077dcf0`: `0x007618bc`. Volume update `0x0077b2e0`: `0x0077e0a3`. Static sort lists `0x007ffa10`: `0x00746d35`, `0x0074915b`. Projected shadow render `0x0073ec20`: `0x00761899`. DoShadows `0x00761860`: `0x00746cec`, `0x00746d2c`.
- Water: `0x00738c70` at `0x0073780e` and `0x0073796b`, the wave mesh `0x00739810` at `0x0073782d`, all inside the water's Render `0x00737760`.

## The terrain, HeightMapRenderObjClass

Render (EA source `HeightMap.cpp`) draws the tiles from buffers it doesn't rebuild, then calls the sub-renderers below on every call.

- **Roads.** drawRoads `0x00774dd0` (called at `0x00733a1d`) calls loadRoadsInVertexAndIndexBuffers `0x0076e1d0` for every road type, every frame. That locks the type's vertex and index buffers, scans every road segment once per joint type (8), culls it against the draw window and copies its precomputed vertices (loadRoadSegment `0x0076e420`). Nothing gates it: the draw window it culls against only changes when the terrain recenters, and never when the whole terrain is loaded. The buffers are static (flags 0 locks), so each frame's lock of a buffer the GPU still reads makes us rename it and copy the whole buffer. The scan loop is 2 to 3% self.
- **Extra blend tiles.** renderExtraBlendTiles `0x00734ae0` (called at `0x00733810`, when `m_use3WayTerrainBlends`) fills a dynamic vertex and index buffer every frame, calling getStaticDiffuse `0x00731a60` for each vertex, which computes a normal and lights it (doTheLight `0x00728cb0`, square root `0x0044ab70`).
- **Custom edging.** drawEdging `0x00775790` (called at `0x007338d9`, edging object at terrain `+0x285c`) calls its load `0x00775070` at `0x007757be` (thiscall: map, minX, maxX, minY, maxY). The load returns unless the changed flag (`+0x14`) is set, but clearAllEdging `0x00775780` (zeroes the counts at `+0x8`, `+0xc`, sets `+0x14`) is called at the top of updateCenter (`0x007324e9`), right after its null checks, and updateCenter runs every frame. So the edging was rebuilt every frame: 0.6 to 2.6% self. The same spot in updateCenter also sets the tree buffer's full update flag (`+0x1b59d` of the tree buffer at `+0x2850`) every frame. Not measured.
- **A terrain sub-renderer refilled every frame with a plain lock.** `0x0074baa0`, called from the terrain's Render at `0x00733bcd` after the null check on its `+0x10` and a backface-inverted check (`0x00810f40`), reads the GlobalData terrain light colors (`+0x648`, `+0x66c` and on), then locks a dynamic vertex buffer through the write lock helper `0x0081cbf0` (at `0x0074bb80`), without DISCARD or NOOVERWRITE, and draws. Which renderer it is (bibs, bridges, scorches) isn't identified. Its lock is what made direct writes into dynamic buffers read GPU memory every frame at first ([vulkan-backend.md](vulkan-backend.md)), and the refill is another candidate for a cache. The other caller of clearAllEdging is `0x0072e2e0` (`0x0072e310`).
- **Recentering.** W3DView::update (`0x0074d3db`) calls updateCenter `0x007324a0`, which rewrites terrain blocks through updateBlock `0x0072ce00` and updateVB `0x00729650` as the camera crosses cells. 1 to 2% under it while scrolling. updateBlock is called at `0x00730ff1` (full update) and from updateCenter at `0x0073252b`, `0x00732cc2`, `0x00732cf9`, `0x00732dec`, `0x00732e5b`. Map loads, lighting changes (`m_needFullUpdate`, which also relights the roads right after) and height changes go through it.
- **getHeightMapHeight** `0x0072f020` calls `floor` four times per call. About 1.5% with its floor calls, mostly from the stencil shadow volume update.

Zero Hour (EA source `GeneralsMD`): drawRoads only refills when `m_updateBuffers` is set and the visible roads changed, so its roads were already fine. renderExtraBlendTiles (`0x0079d9c0`) still fills its buffers and lights every vertex every frame. updateBlock is virtual there. The custom edging draw is under `TEST_CUSTOM_EDGING` in Zero Hour's source, and whether the retail build draws it isn't checked.

## Water

- The wave mesh `0x00739810` (wave grid flag `+0x229`) is rebuilt every frame with a DISCARD lock, about 1.2% self.
- Water areas are drawn by `0x00738c70`.

## Rebuilt every frame

Built for 30 calls per second and now made at the display rate: roads, extra blend tiles (lit per vertex), the water wave mesh, shadow decals, shadow volumes, particle quads, the sorting renderer's buffers, and the whole interface (the window draw recursion). Each one is small. Together they and WW3D's per-mesh work make the game's share. Some only change when their inputs change (roads, blend tiles, edging), and those are now cached (below). Others follow blended positions every frame (decals, particles).

## Not done yet

- **GPU cost with the 3D world's extras on.** Zero Hour at 150% render scale (a 4K world target), 8x MSAA on R16G16B16A16, ambient occlusion and FXAA waited 3 to 4.4 ms per frame for the GPU in the main menu, so it was GPU-bound there. Suspects, unconfirmed: images stay in the GENERAL layout with a full barrier between rendering and transfers, which may cost AMD its color and depth compression on the multisampled images, and the world's own passes (depth copy, occlusion, resolve) each add one. Needs a GPU capture (Radeon GPU Profiler) before changing anything.
- **Tick-frame spikes.** The logic step runs on whichever frame the 30 Hz tick lands on, so that frame also carries its 1.3 to 2.6 ms (more in big battles), and the 99th percentile frame time sat near twice the median. Moving the logic off the render frame means threading the game logic, which it was never built for, so it was set aside. Making the rest of the frame cheaper is what shrinks the spike.
- **Zero Hour** hasn't been profiled with the current build, and whether its retail build draws the custom edging isn't checked. Measured without the profiler at 150% render scale, 8x MSAA on R16G16B16A16, FXAA, ambient occlusion and soft particles: the CPU side was 1.2 to 3 ms a frame and the rest was waiting for the GPU (3.5 to 10 ms, mostly Anti-Lag 2's sleep after Present). 100 to 178 fps in the menu and light scenes, 28 fps in the particle-heavy cutscene with 32 ms of GPU wait per frame at 883 draws. So at those settings Zero Hour is GPU-bound, and the GPU item above is what limits it.

## What we changed

- **Terrain caches** (`crates/d3d8/src/terrain.rs`). A stub counts updateBlock calls (Generals: the six calls redirected; Zero Hour: the vtable entry) and jumps on with the arguments untouched. Everything below is rebuilt when that count changes.
  - Blend tile lighting, both games: renderExtraBlendTiles' four getStaticDiffuse calls go through a cache keyed by (x, y). getStaticDiffuse only reads heights, the GlobalData terrain light directions and the scene's static lights (`createLightsIterator` walks `LightList`, never the dynamic lights), so it changes only with a terrain update. The buffers are still filled every frame, only the lighting is kept.
  - Roads, Generals: the loadRoads call in drawRoads is skipped when the road type's buffers were last filled for the same road buffer, type, vertex and index buffer, initialized flag, draw window (`+0x18` to `+0x24`) and updateBlock count. Fields: types `+0` (0x24 bytes each, vertex buffer `+4`, index buffer `+8`), initialized `+0xc`, current type `+0x2c`. Skipping also avoids the whole-buffer copies those flags-0 locks caused.
  - Custom edging, Generals: updateCenter's clearAllEdging call does nothing. The load call goes through ours, which calls clearAllEdging when the edging's map, window or the updateBlock count changed, then loads. Clears from elsewhere (map resets) still count, since the load checks the flag itself.
- **floor and ceil** (`crates/d3d8/src/crt.rs`). Both games import them from msvcrt (Zero Hour slots `0x00943338`, `0x00943430`, Generals `0x009874cc`, `0x009874b4`), and the logic uses them too, so results must stay identical. Ours truncate through an i32 below 2^31 and fall back to msvcrt's otherwise (and for infinities and NaN), with the sign of a zero result taken from the input, as msvcrt does (`ceil(-0.5)` is `-0.0`). At startup they're compared bit for bit with msvcrt's on edge cases and only kept if every result matches. A unit test does the same over a range.
- **things.rs** keeps one FxHash map and updates it in place (it rebuilt a SipHash map twice per tick), and skips the per-frame walk when nothing moved. The other per-frame maps (objects, bones, lines, gui, transitions, treads) use FxHash too.
- **A renderer thread runs the device** ([vulkan-backend.md](vulkan-backend.md)): the game thread queues its D3D calls and our draw preparation (about 0.45 µs per draw, 1.2 to 1.5 ms in busy Generals frames) runs beside it. In Zero Hour at 150%, 8x MSAA (GPU-bound) the game thread's own work was 1 to 2 ms a frame, calls that wait for the renderer thread 0.01 to 0.9 ms, and the rest of the frame waiting at Present behind the GPU; frame rates unchanged, as expected there.
- **Sorted translucent polygons and particles draw single sampled** when multisampling is on ([scene.md](scene.md)), for the GPU cost the Zero Hour cutscene showed. The same hook makes soft particles apply where particles are actually drawn.
- **Per-draw bookkeeping**: no memcmp calls for state and uniform comparisons, recording lists that keep their capacity, and vertex layouts as cached commands instead of allocating closures ([vulkan-backend.md](vulkan-backend.md)). Not measured yet.
- **Buffers, descriptor sets, the presenter thread and deferred recording** in the Vulkan device: see [vulkan-backend.md](vulkan-backend.md). Deferred recording was chosen over a full worker thread because the driver's command calls were the largest part of our share (about 13%), and it leaves the D3D state handling where it was.
- **Log**: the render line splits frame times into tick frames and the rest, and gives the engine update per tick. The cpu line counts the frame cap apart from the game.

Seen in game afterwards (Generals, same settings, profiler on, before dynamic buffers moved to system memory): main menu 220 to 264 fps (was about 200), a match 158 to 164 fps at about 1,650 draws and 129 to 141 at 2,050 to 2,300 (was 116 to 143 at 900 to 1,400), 89 to 105 at 2,750 to 3,250 (was 82 at 3,360). Preparing draws took about 0.6 µs per draw, buffer locks 0.1 to 0.2 ms, Present 0.02 ms, no descriptor sets written once a scene was seen, and the game itself 5 to 9 ms. Tick frames ran about 2.5 ms longer than the others (median 10 to 12 ms against 8 to 9).

With dynamic buffers in system memory as well: main menu 231 to 294 fps, a match 188 to 255 fps at 1,100 to 2,000 draws, 147 to 168 at 2,500 to 3,000, and 147 at 3,366 draws (6.8 ms a frame, against 12.2 ms at 3,360 in the first measurement). The game itself was 3 to 6 ms of the frame. The largest single item left is the driver recording draws (`amdvlk32` under DrawIndexedPrimitive, about 9%, plus about 4% under the stencil shadow volume draws `0x0077a65d` and `0x0077a9ce`), which is what moving recording to a worker thread would take off the main thread. Tick frames still run about 2.5 ms longer than the others.

With deferred recording (vsync on, 300 Hz display): main menu and light scenes sit at the 300 fps vsync limit. A match ran 200 to 290 fps at 1,200 to 2,000 draws and 137 to 169 at 2,450 to 2,850. `amdvlk32` dropped from about 13% of the main thread's samples to under 0.5%. Replaying the lists on the presenter took 0.3 µs per draw, and preparing draws on the game thread about 0.5 µs per draw (1.3 to 1.6 ms in a busy match). The game itself and everything else was 4 to 5.5 ms of a 6 to 6.5 ms frame in a busy match. The profile is now flat: the largest self items of ours are memcpy and memcmp (about 2 to 3% each, called from draw preparation) and `prepare_draw` itself. Tick frames still run 2 to 3 ms longer (median 8 to 8.8 ms against 5.5 to 6.4 in a busy match). So the next gains are in the game's own per-frame and per-tick work, not in the backend.

In that busy match the logic averaged 0.15 to 0.4 ms per frame (1 to 2 ms per tick) and drawing 4.4 to 6.4 ms, of which ours was 1.2 to 1.7 ms. Under the drawing, by the stack scan (overstated, nested): opaque models 17 to 21%, particles 12 to 14%, stencil shadow volumes about 7%, bone transforms 6 to 7%, the sorting renderer 6 to 7%, shadow decals 5 to 6%, UI 4.5 to 6%, terrain 4.5 to 5%, static sort lists 3 to 4%, water about 3%, the message pump 2 to 3%. Of ours, besides `draw_indexed_primitive` itself, `set_transform`, `set_indices`, `set_light` and `set_texture` stood out, along with vectors growing every frame and a heap allocation per boxed recorded command.

Timed with `render_timers.rs` (Generals skirmish, same settings, 204 to 263 fps, `draw` 3.4 to 4.6 ms a frame), ms per frame, each without the timed systems under it, our draw preparation included:

| System | ms |
|---|---|
| Models (TheDX8MeshRenderer.Flush) | 0.5 to 1.0 |
| Translucent (SortingRendererClass::Flush) | 0.3 to 0.9 |
| Stencil shadow volume render | 0.17 to 0.45 |
| UI (window draw) | 0.38 to 0.64, steady |
| Terrain Render | 0.32 to 0.37, steady |
| DoParticles | 0.13 to 0.32 |
| Volume updates (300 to 350 calls) | 0.07 to 0.10 |
| Shadow decal geometry | 0.03 to 0.09 |
| Bones (60 to 300 calls) | 0.02 to 0.07 |
| Water, static sort lists, projected shadows, DoShadows | under 0.05 together |
| Untimed rest of the draw | about 1.2 |

Our draw preparation was 0.8 to 1.55 ms of that, almost all of it under models, translucent and UI, so it's most of the models and translucent time. The stack scan had overstated decals and bones about tenfold. The GPU took 1.1 to 2.2 ms a frame (world opaque 0.3 to 0.65, depth copy and ambient occlusion 0.33 to 0.55, translucent 0.2 to 0.97), so Generals at 100% without MSAA is CPU-bound.

The GPU's particles section stayed at 0.00: no draw happens while DoParticles runs in Generals. Its particles are queued into the sorting renderer and drawn by the SortingRendererClass::Flush call right after it (`0x00746d70`), which means the soft particle setup (keyed on DoParticles) never applies in Generals. Zero Hour is the same: its particles section stayed at 0.00 to 0.04 ms through the particle-heavy cutscene.

A full 8-player Generals skirmish (30 minutes, same settings, with the per-draw bookkeeping changes), averaged by draws per frame:

| Draws | fps | Frame median (ticks / others) | Draw | Ours preparing | Logic per tick | GPU | Vertex / pixel blocks built |
|---|---|---|---|---|---|---|---|
| under 1,000 | 294 | 3.3 (3.8 / 3.3) | 3.0 | 0.50 | 1.75 | 0.84 | 354 / 94 |
| 1,000 to 1,500 | 292 | 3.3 (4.7 / 3.3) | 3.0 | 0.76 | 1.12 | 0.94 | 428 / 150 |
| 1,500 to 2,000 | 268 | 3.5 (5.3 / 3.4) | 3.3 | 0.95 | 1.24 | 1.16 | 482 / 197 |
| 2,000 to 2,500 | 242 | 3.9 (5.7 / 3.8) | 3.7 | 1.15 | 1.01 | 1.50 | 563 / 220 |
| 2,500 to 3,000 | 206 | 4.7 (6.3 / 4.6) | 4.4 | 1.40 | 0.92 | 1.94 | 756 / 325 |
| 3,000 to 3,600 | 144 to 178 | 5.9 to 6.8 | 5.5 to 6.4 | 1.8 to 2.2 | 0.9 | 3.0 to 3.8 | 1,170 / 580 to 800 |

Preparing a draw stayed at about 0.5 µs, so the bookkeeping changes didn't move it measurably. The vertex block was rebuilt for a third to a half of all draws: set_vertex_shader marked it dirty on every call, and WW3D sets the FVF for every mesh, though an FVF only feeds the block through its vertex info, which prepare_draw compares itself (fixed after this run, along with set_viewport marking it dirty when unchanged). The 3,000+ draw stretches were early in the match (90 to 155 s) with the UI at 1.6 to 1.8 ms and translucent at 1.5 to 1.7 ms of CPU. At 2,000 to 2,500 draws the timed systems were models 0.68, UI 0.49, shadow volumes 0.48, translucent 0.44, terrain 0.26, volume updates 0.19, particles 0.17, scene render and flush 0.15, object list 0.12, water 0.10, rest of the view draw 0.10, decals 0.08, bones 0.06, leaving about 0.4 ms of the draw untimed.

With those two fixed, a short look at the end of the same match: vertex blocks built fell by about a quarter (392 at 1,500 to 2,000 draws, 422 at 2,000 to 2,500). The breakdown of our preparation (`our draw preparation per frame`, timers on) at about 1,800 draws, ms per frame: descriptor set and push constants 0.27 to 0.38, uniforms 0.16 to 0.22, vertex buffers 0.15 to 0.23, culling 0.12 to 0.17, shaders and textures 0.13 to 0.18, dynamic state 0.10 to 0.15, pipeline 0.06 to 0.09, target and pass 0.05 to 0.07. The descriptor set part built a key over all 8 texture stages, each with its own sampler hash lookup, on nearly every draw. Now only the stages a fixed function draw samples (up to the first disabled one) go into the key, the rest get the white image and stage 0's sampler, and each stage remembers its last sampler key. Spot cones (two `cos` per light) are only computed for spot lights. Afterwards the descriptor set part fell to 0.17 to 0.22 ms at the same draw counts, and preparing draws to 0.89 ms at 1,500 to 2,000 draws (was 0.95 to 1.0). Then: culling reuses world times view times projection until one of them changes (`clip_matrix`), and the vertex block takes its lights from `state::LightCache`, keyed on the view and the enabled lights, instead of transforming them for every rebuild. The vertex buffer part is a vtable check and a reference count per binding, nothing to cut there. Measured afterwards, culling (0.09 to 0.16 ms) and uniforms (0.12 to 0.20 ms) didn't move: what's left of each part is a few dozen nanoseconds per draw, most likely reaching into the game's COM objects, our `Rc` buffer and texture data and the state arrays, which no longer sit in cache between draws, plus the timing itself (eight timestamp reads per draw with timers on). Preparing draws costs 0.45 to 0.5 µs per draw now. Cutting it much further would take fewer draws or moving draw preparation to another thread.

The UI's 1.6 to 1.9 ms with 3,000+ draws per frame wasn't in a match: in another 8-player skirmish it showed in the 30 seconds before the map loaded, the skirmish setup screen over the shell map, with 1,000 to 1,090 UI draws per frame (150 to 225 fps there). In the match the UI took 0.4 to 0.5 ms. Per draw, averaged over the match (time includes the game's own work for that system): terrain 2.3 µs, UI 2.0, models 1.2, translucent 1.1, shadow volumes 0.7. This match ran 285 fps at 1,000 to 1,500 draws, 244 at 1,500 to 2,000, 216 at 2,000 to 2,500 and 181 at 2,500 to 3,000, with 1.9 to 2.5 ms of logic per tick (tick frames 2.1 to 2.7 ms longer than the others).

GPU timestamps in Zero Hour at 150% render scale (3840x2160), 8x MSAA on R16G16B16A16, FXAA, ambient occlusion: the CPU side was 1 to 3 ms and the GPU 5 to 20 ms a frame, so it's GPU-bound throughout.

| Section | ms per frame |
|---|---|
| World opaque (includes clearing the multisampled color and depth) | 2.4 to 4.0, even at 130 to 700 draws |
| Depth copy and ambient occlusion (the apply pass blends into the 8x target) | 0.9 to 2.2 |
| World translucent (particles included) | 0.2 to 2 normally, 5 to 14 in the particle-heavy cutscene |
| Resolve, scaling and FXAA | 0.6 to 0.7 |
| Interface and the rest | under 0.15 |

After sorted translucent polygons and particles moved to single sampling ([scene.md](scene.md)), the same cutscene at the same settings: the worst 5 seconds went from 50 fps (19.6 ms of GPU work) to 86 fps (11.5 ms), with the sorted section at 1.5 to 3.7 ms in the heaviest stretches against 7 to 14 ms of translucent before. The resolve to single sampling added about 0.3 ms (resolves 0.9 to 1.0 ms, was 0.7). What's left is mostly per pixel and fixed: world opaque 2.4 to 3.9 ms at 125 to 1,400 draws, depth copy and ambient occlusion 0.8 to 2.2 ms, resolves about 1 ms. The opaque section included the clear of the multisampled target and the occlusion section its blended apply into the 8x target, so both got their own GPU marks (`world clear`, `ambient occlusion blur and apply`).

With the finer marks (same cutscene and settings), ms per frame: world clear 0.31 to 0.34 (so not the problem), world opaque 2.2 to 3.4 at 130 to 900 draws, depth copy and occlusion 1.1 to 1.4, occlusion blur and apply 0.4 to 0.8, shadows and other translucent 0.3 to 2.2, sorted up to 3.1, resolves about 1.0. Half resolution occlusion brought depth copy and occlusion to 0.4 to 0.7 ms (from 1.1 to 1.4), with the apply at 0.35 to 0.85, and quiet stretches of the cutscene went to 145 to 186 fps with 5 to 7 ms of GPU work. It's back at full resolution with 16 samples because half resolution made it shift as the picture moved ([scene.md](scene.md)), so those 0.7 ms are spent again, more with the extra samples, not yet measured. The opaque section's cost is per pixel at 3840x2160 with 8x; whether AMD keeps its compression with every image in GENERAL is still the open question there.

The cutscene's drop to 50 to 80 fps (before that change) was the translucent section alone (7.2 and 14.2 ms in the worst stretches): particle overdraw blended at 8 samples per pixel into a 64-bit target at 4K. Every image stays in the GENERAL layout with full barriers between passes, and render passes load and store every time. Whether AMD keeps its color compression (DCC, FMASK) under that isn't known. Image usage has no STORAGE flag, which would have ruled it out on its own.
