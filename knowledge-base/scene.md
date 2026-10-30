# The 3D World and the Interface

Where in a frame the game draws the 3D world and where it draws the interface over it, and how the renderer gives the world a target of its own. Code: `crates/d3d8/src/scene.rs` (the game side) and the scene functions in `vk/device.rs`.

## Order of a frame

W3DDisplay::draw `0x0073dde0` (EA source `W3DDisplay.cpp`, matches retail):

1. Before `WW3D::Begin_Render` (`0x0083ca10`, called with `(1, 1)`): shroud, water reflection and shadow textures, each into its own render target.
2. Begin_Render clears the back buffer and the depth buffer.
3. `drawViews` (display vtable `+0x5c`, `0x00421870`) calls each view's vtable `+0x148`, W3DView::drawView `0x0076dda0`, which moves `this` to the second base at `+0x7c` and jumps to W3DView::draw `0x0076ddb0` through that base's vtable `0x00968b40` slot `+0x14`.
4. `TheInGameUI->DRAW()` (`0x00a39fb0`, vtable `+0x14`), the mouse, letterbox, cinematic text, debug displays.
5. End_Render, Present.

W3DView::draw (EA source `W3DView.cpp`):

1. View filter pre-render (`W3DShaderManager::filterPreRender`, may render the scene into a texture), the 3D scene (`m_3DScene->doRender`), filter post-render (draws that texture to the back buffer), an optional extra pass, debug lines in debug builds.
2. `getAxisAlignedViewRegion` (`0x0076dad0`, called at `0x0076e4a7`), then `TheGameClient->iterateDrawablesInRegion(region, drawablePostDraw)` (client vtable `+0x44`, callback `0x0076e510`): each drawable's `drawIconUI`, so health bars, veterancy and status icons. Then `flushTextBearingDrawables` (`0x004aa490`) and the 2D scene (`0x00769160`).

So the world is everything W3DView::draw draws before step 2. Every path through the function reaches that call.

## Inside the world: RTS3DScene::Flush

RTS3DScene::Flush `0x00767440` (thiscall, one argument `rinfo`, EA source `W3DScene.cpp`), in order:

| Call at | Function | What |
|---|---|---|
| `0x0076745c` | `0x00781620(rinfo, 0)` | DoShadows, decal shadows (skipped when `this+0x58c` or `this+0x18` is set, a custom pass) |
| `0x00767469` | `0x0085c040` | TheDX8MeshRenderer.Flush, opaque meshes |
| `0x0076747a` | `0x00768350` | occluded objects into stencil, when `0x00837360` says so |
| `0x00767480` | `0x0074fc10(rinfo)` | DoTrees, the last opaque geometry (it calls drawTrees at `0x0074ff01`) |
| `0x0076749c` | `0x00781620(rinfo, 1)` | DoShadows, stencil shadows |
| `0x007674a5` | `0x0083d400(rinfo)` | WW3D::Render_And_Clear_Static_Sort_Lists, water |
| `0x007674c1` | `0x00768c70(rinfo)` | flushTranslucentObjects (not in custom passes) |
| `0x007674d8` | `0x00778570(rinfo)` | DoParticles: TheParticleSystemManager vtable `+0x20` (not in custom passes) |
| `0x007674e0` | `0x00842b60` | SortingRendererClass::Flush, sorted translucent polygons |
| `0x007674ea` | `0x0085bc00` | Clear_Pending_Delete_Lists |

It runs six times from `0x00767a40` (the custom pass modes), and also for the water reflection and inside view filters.

We redirect two calls (cdecl, caller pops):

- DoTrees: after it returns, `scene::opaque_done` goes up. The device acts on it at its next draw or clear, while the world is drawn into its own target and the back buffer is the render target (so not for the reflection or a filter): `after_opaque`.
- SortingRendererClass::Flush (static, no arguments): `scene::in_sorted` is set during it. DoParticles draws nothing itself: it queues every particle into the sorting renderer, which this flush draws along with the other sorted translucent polygons (GPU timestamps showed no draw at all during DoParticles, in both games). Generals: the call at `0x00746d70` to `0x00809900`, right after DoParticles at `0x00746d68`. An earlier version hooked DoParticles instead, so soft particles never applied.

## Depth copy, ambient occlusion and soft particles

- The world's depth image is made sampleable (`ImageUse::SampledDepth`, a depth-only view). At `after_opaque` a pass copies it into an `R32_SFLOAT` image the size of the world's target (`post::DEPTH_COPY`). A multisampled depth is first resolved into the single sampled depth (sample 0) by an empty render pass, and copied from there. That copy is what everything else reads, so nothing samples a depth buffer that's attached for drawing.
- The world's projection is right-handed: WW3D's camera looks down -z, so `_34` is -1 and clip w is minus view z. `reversed_depth_terms` takes the distance as clip w either way (`d = a + b / w`, with `a` divided by the sign of `_34`). The first version only accepted `_34 = 1` and turned both effects off in game ("the world's projection isn't a perspective one"). The GPU test now draws the soft particle case with a right-handed camera and the occlusion case with a left-handed one.
- Ambient occlusion (`post::ambient_occlusion`): scalable ambient obscurance (McGuire, Mara and Luebke 2012), at the world's full resolution. View space positions come from the copy with the world's projection (reversed depth `d = a + b / z`, x and y scale from `_11` and `_22`), normals from the nearer neighbor on each axis, 16 samples on a spiral of 7 turns, radius in world units projected to pixels, each sample offset rounded to the nearest pixel. Each sample's term is multiplied by the radius: the paper's term has units of one over length and was tuned for meters, so at our scale (radius 8 to 16, a terrain cell is 10) it came out about 12 times too weak. The second pass (`ambient_occlusion_apply`) blurs it over 3x3 pixels, skipping pixels more than 5% away in depth, discards pixels whose light is above 0.998 (most of the world, which would otherwise still be blended into every sample), and multiplies it into the world's color by blending (`ZERO, SRC_COLOR`), into the multisampled image when there is one. Running at `after_opaque` puts it under stencil shadows, water, translucent objects and particles, not over them.
- What made the occlusion shift or jitter while the camera or a unit moved (it went away with the occlusion off, and showed in the main menu's slow camera too). The GPU test slides a wall down by about a quarter pixel per frame and measures where the darkening sits against the wall's edge, as a spread over 16 frames (wall at depth 100, then 30): half resolution with truncated offsets and a 4x4 rotation 2.25 and 0.57 pixels, full resolution 1.22 and 0.31, full resolution with rounded offsets 0.75 and 0.42, the same without rotation 0.5 to 0.9 and 0.3 to 0.46 depending on the sample count. At full resolution the far wall's offsets repeat every 3 to 4 frames, which is the rasterized edge stepping a whole pixel, so the occlusion moves with the picture and the test accepts anything under a pixel. Half resolution: each texel stood for a 2x2 block, so the darkening moved in 2 pixel steps fixed to the screen, and interpolating the blurred texels in the apply pass didn't change that. Truncated offsets: a tap's pixel jumped by one as the subpixel position crossed a whole number, always toward zero. A per pixel rotation fixed to the screen (first random, later a 4x4 Bayer pattern averaged by a 5x5 blur) gives a world spot a different sample pattern every time the picture moves under it, which the blur can't fully cancel at edges, and a 2x2 pattern measured worse (0.89). So every pixel uses the same spiral. Half resolution existed for speed: at 3840x2160 with 8x, full resolution took 1.1 to 1.4 ms with the depth copy against 0.4 to 0.7.
- Soft particles: a fixed function draw while `in_sorted` is set, blended and without depth writes, into the world's target after the copy, gets the copy at binding 18 and `ps.soft` = `(a, b, 1 / 8 units, mode)`. The fragment shader fades the result by how far the world behind is: `saturate((z_behind - z_fragment) / 8)`, on alpha, or on color too for additive (`ONE, ONE`) blending. Only camera facing polygons fade: the fade is scaled down by the fragment's view depth slope (`fwidth`), gone at 0.04 units per pixel. The sorted pass also draws rally point lines, the building preview's front marker and decals lying on the ground, and with the fade on every polygon those vanished (the ground right behind them is closer than the fade distance). Billboard particles have a constant view depth, so they keep fading. The GPU test draws a slanted polygon a unit in front of a wall and checks it stays fully visible. Other draws bind a white image there and leave `ps.soft` zero.
- Settings: "Ambient occlusion" Off, Low (radius 8, intensity 0.6), Medium (12, 1.0, the default), High (16, 1.5), and "Soft particles" (default on). Either one makes the world get its own target.
- The GPU test checks a floor meeting a wall comes out darker at the wall (138 of 255 next to it), and that a particle 2 units in front of a wall is a quarter visible while one 30 units in front is fully visible.

## The game's other render targets (EA source)

- Water reflection: `WaterRenderObjClass::ReAcquireResources` makes a `SEA_REFLECTION_SIZE` (256) square target. `renderMirror` draws the scene into it with `Set_Render_Target_With_Z`, clears only depth, and goes back to the back buffer. Which depth buffer it gets isn't confirmed.
- Projected shadows: `W3DProjectedShadowManager` makes a 512x512 `A8R8G8B8` target (`DEFAULT_RENDER_TARGET_WIDTH`, `_HEIGHT`). `W3DProjectedShadow::updateTexture` renders a shadow into it and copies it (`SurfaceClass::Copy`, so CopyRects) into a plain 512x512 texture kept per shadow.
- View filters: `W3DShaderManager::init` makes one target the size of the back buffer. `startRenderToTexture` pairs it with the depth buffer that was set, `endRenderToTexture` goes back. Inside the world, so with the world's own target the filter draws at 100% and its result is drawn into the world's target.
- The device logs each render target size it's asked for, each depth surface size, and each pairing of a render target with a depth buffer, once (`drawing into a 256x256 render target with ...`), to settle the open questions before these targets are made larger.

## The hooks

- W3DView::draw's vtable entry `0x00968b54` points at `scene::draw_view`, which raises the "in scene" flag and calls the original.
- The call at `0x0076e4a7` goes to `scene::view_region`, which lowers the flag and calls getAxisAlignedViewRegion.

## The world's target

When the 3D resolution (render scale) isn't 100% or sharpening is on, the device draws the world into its own color and depth images (`SceneTarget`):

- The device checks the flag before each draw, clear and Present (`sync_scene`), so the switch happens in the game's draw order.
- Opening: the back buffer (just cleared by Begin_Render) is copied into the world's color image, scaled, and the world's depth is cleared to the last clear of the automatic depth buffer.
- While open, draws and clears aimed at the back buffer with the automatic depth buffer (or none) go to the world's images instead. Viewports, scissors and clear rectangles are scaled by the target's size. Pretransformed vertices need nothing: the shader turns them into clip space with the game's viewport, which doesn't change. Render targets the game sets itself (filters, reflections) aren't touched.
- Closing: a fullscreen pass (`post::RESOLVE`) draws the world's image into the back buffer, filtered, with optional contrast adaptive sharpening (AMD FidelityFX CAS on five taps one source texel apart). Health bars, icons, text and the whole interface then draw at full resolution on top.
- The world's images are `R16G16B16A16_UNORM` where the GPU can blend into it (`Context::scene_format`), `B8G8R8A8` otherwise. UNORM, not float: blends clamp to 0..1 after every step exactly like the game's 8-bit back buffer, so an additive blend followed by a multiply comes out as it always did, only with finer steps. Stacked smoke, fog of war and dark gradients band less. The resolve pass adds triangular dithering of one 8-bit step when it writes the wider image into the back buffer.
- Pipelines carry the target's format and sample count in their key. The first time the world uses a kind of target, every pipeline built so far for the back buffer is built again for it in one go (`Pipelines::prebuild_target`), about half a second once, then from the pipeline cache on disk.
- Multisampling: the world draws into a multisampled color and depth image instead, with as many samples as the GPU allows up to the setting. Nothing can be copied into a multisampled image, so opening clears it with the back buffer's last clear color (all the back buffer holds at that point), with a render pass whose loads clear. Closing resolves it into the single sampled image (an empty render pass with a resolve attachment, unless the sorted pass already did) before the resolve pass. The multisampled images stay in attachment layouts throughout ([vulkan-backend.md](vulkan-backend.md)).
- Sorted translucent polygons and particles are drawn single sampled. When the sorting renderer's flush starts (`in_sorted`), `single_sample` ends the pass and runs one empty dynamic rendering pass over the multisampled color and depth, resolving color (average) into the single sampled color image and depth and stencil (sample 0) into a single sampled depth image (`depth_single`), with the multisampled contents not stored. The rest of the world, until closing, draws into those two, depth and stencil tested as before. Closing then skips its resolve. Particle overdraw blended at every sample was what made Zero Hour's particle-heavy cutscene GPU-bound (5 to 14 ms of translucent work at 3840x2160 with 8x). Opaque geometry, stencil shadows, water and flushTranslucentObjects still draw multisampled, since they come before the flush. Edges of sorted translucent polygons and particles lose multisampling, which soft particles and their own alpha mostly hide. The GPU test checks a sorted polygon behind earlier geometry stays hidden, one over the clear blends, and an edge drawn before the switch stays smooth.
- FXAA runs in the resolve pass on the world alone (Lottes' original FXAA: edge direction from the four diagonal neighbors, kept only within the local luma range), before sharpening. Text and interface never go through it.
- The automatic depth buffer keeps its Begin_Render clear, the world's depth isn't copied back. Nothing drawn after the world was seen to depth test against it.
- If the game copies the back buffer while the world is open, the log says so once.
- The GPU test draws a world at 50% and an interface quad over it and checks the quad's edges stay exact, checks that sharpening leaves flat areas alone, and draws a diagonal edge with 4x multisampling and with FXAA and checks it comes out blended.
