# CLAUDE.md

A Rust DLL that loads into Command & Conquer Generals Zero Hour 1.04 (the EA app's `game.dat`) as `dinput8.dll`, and is being ported to base Generals (`knowledge-base/generals.md`). It hands the game our own Direct3D 8 device, which draws with Vulkan, and patches the game in memory. Goals: very high resolutions, a UI that scales and keeps its shape, borderless and exclusive fullscreen without issues, stability, and high frame rates with the game logic kept at its original 30 updates per second. Vulkan is the only renderer. The system D3D8 is only asked about adapters, display modes and caps.

## Read first

`knowledge-base/README.md`, then the page for whatever you're touching. The knowledge base holds every address, struct layout and behavior we've found in `game.dat`, and why each patch exists. Keep it current: add every discovery and every changed patch to the right page, in the same change.

## Layout

- `crates/d3d8`: the DLL (`dinput8.dll`, a cdylib for `i686-pc-windows-msvc`). Each module covers one area: `frame.rs` (logic/client split, render clock), `client.rs` (client update pieces), `camera.rs`, `objects.rs`, `things.rs`, `bones.rs`, `particles.rs`, `treads.rs`, `lines.rs`, `water.rs`, `trees.rs`, `terrain.rs` (terrain work kept between terrain updates), `overlays.rs`, `gui.rs`, `transitions.rs` (blends and per-frame fixes), `ui.rs` and `controlbar.rs` (layout scaling, wide control bar), `optionsmenu.rs`, `settings.rs` and `gadgets.rs` (our options page, Options.ini, game window helpers), `window.rs` (window modes, DPI, cursor), `direct3d.rs` (`Direct3DCreate8`, startup settings, our IDirect3D8 over the system one), `vk/` (the Vulkan backend: `device.rs`, `texture.rs`, `buffer.rs`, `render.rs` for command recording, `pipeline.rs`, `swapchain.rs`, `shader/` for the fixed function shaders and the D3D8 shader translator), `game.rs` (build detection, byte checks, patch helpers), `crt.rs` (faster floor and ceil for the game, identical results), `watchdog.rs` (logs where the main thread is during stalls, and the optional sampling profiler), `render_timers.rs` (optional timers around the game's render systems), `config.rs` (`cnc-vulkan.toml`).
- `crates/bigtool`: lists and extracts files from the game's `.big` archives, converts TGA to PNG, crops and zooms images.
- `dist/cnc-vulkan.toml`: the config shipped next to the DLL. Most settings live in the game's options menu and Options.ini instead.
- `re/scripts/Query.java`: the Ghidra query script. `re/ghidra` (the project) and `re/out` (query results, extracted game files) are ignored by git.

## Build, deploy, check

- `cargo build --release`, output `target\i686-pc-windows-msvc\release\dinput8.dll`.
- Copy it (and `dist\cnc-vulkan.toml` when it changes) into the inner `Command and Conquer Generals Zero Hour` folder, next to `game.dat`. The outer folder of the same name is never read. For base Generals, also copy them into the `Command and Conquer Generals` folder inside the outer one. The copy fails while the game runs.
- `cargo test -p d3d8 --release` runs the options layout test against `OptionsMenu.wnd` extracted into `re/out/big` (it's skipped when the file isn't there), the shader translation tests, and the Vulkan GPU test, which opens a small window for a few seconds.
- After a deploy, the log (`%LOCALAPPDATA%\cnc-vulkan\zh\cnc-vulkan.log`) must say `recognized Zero Hour 1.04` (base Generals: `generals\cnc-vulkan.log`, `recognized Generals`). If any byte check fails it names the address and every patch is off, so the game quietly runs unmodified. Every 5 seconds the log prints render fps, ticks and logic steps per second, and blend counters. Use it to diagnose before guessing.
- Only the user can run the game. Say what to test and what the log should show.

## Reverse engineering

- Ghidra headless queries against the project in `re/ghidra` (`cnc/zh`), with `-noanalysis -readOnly`. Commands and the query list are in `knowledge-base/tooling.md`.
- EA's source release (github.com/electronicarts/CnC_Generals_Zero_Hour, files under `GeneralsMD/Code/...`) matches the retail build closely and is the best map. Fetch raw files and ask for exact code, since the fetch tool summarizes. TheSuperHackers' fork has drifted from retail, use it only for hints.
- Confirm everything in `game.dat` before code depends on it: addresses, struct offsets, calling conventions (check who pops the stack and whether ECX is set before the call), message numbers.
- Use `bigtool` to read the game's own data (`.wnd` layouts, INI files, textures) instead of guessing.

## Rules for patches

- Every address we patch or rely on gets a byte check in `game.rs`, made from bytes actually read in Ghidra. Never write a check from memory or from a guess: a wrong check turns off every patch.
- Check call sites at runtime too (`game::call_target`) before redirecting them.
- Logic stays at 30 Hz and deterministic. The client runs every frame: anything with a fixed step per call gets scaled by real frame time (`frame::frame_scale`). A whole-number counter that works as a clock advances once per 30 Hz tick (`frame::is_tick`), while the code around it still runs every frame. Never hold input or UI updates to the tick.
- Blend between logic steps only for state the simulation owns (models, things, particles and so on), with `frame::blend_factor`. Client animations that step on the tick blend with `frame::tick_factor`.
- Every visual must move smoothly at the display rate without running faster than the original, whatever system drives it.
- Fix root causes. Don't paper over a bug with fallback values.

## Settings

Our settings are extra keys in the game's Options.ini (`CncVulkan...`), edited on our page in the options menu ("More" next to the resolution list). The game keeps unknown keys when it saves. `cnc-vulkan.toml` only holds what isn't in the menu.
