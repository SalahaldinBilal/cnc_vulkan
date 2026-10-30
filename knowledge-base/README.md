# Knowledge Base

Everything learned about C&C Generals and Zero Hour while building this project. Each file covers one area. Facts here were confirmed in the actual binary with Ghidra or by running the game, unless a line says otherwise.

## Ground rules

- All addresses are for **Zero Hour 1.04, the EA app `game.dat`** unless noted. Base Generals' are in [generals.md](generals.md). The image loads at `0x00400000` (no relocation).
- Source references: EA's release (github.com/electronicarts/CnC_Generals_Zero_Hour) matched the retail layout everywhere we've checked so far (for example Particle, including `m_lastPos`). TheSuperHackers' fork has drifted (it dropped `m_lastPos`). Use EA's first, but confirm everything in `game.dat` before relying on it.
- Before the DLL touches any address, it checks the bytes at a list of known spots (see [address-map.md](address-map.md)). If one doesn't match, every game patch is skipped and the log says so.

## Files

| File | What it covers |
|---|---|
| [game-files.md](game-files.md) | Install layout, the launcher and `game.dat`, imports, where the game writes its settings and crash reports |
| [loading-and-hooking.md](loading-and-hooking.md) | How our DLL gets in, load order, import hooks, the D3D8 vtable problem, build fingerprinting |
| [window-and-input.md](window-and-input.md) | WinMain, the game window, its window procedure, mouse input, DPI, the alt-tab crash |
| [display-and-device.md](display-and-device.md) | Device creation and Reset, the windowed flag, resolution, the options menu mode list, refresh rate |
| [global-data.md](global-data.md) | `TheGlobalData` fields, the INI field table format, the command line table |
| [frame-loop.md](frame-loop.md) | The main loop, the frame limiter, the client update, and how we split 30 Hz logic from the every-frame client |
| [ui-layout.md](ui-layout.md) | How `.wnd` layouts are parsed and scaled, our replacement, the control bar scheme and the wide control bar |
| [d3d8-usage.md](d3d8-usage.md) | Which parts of Direct3D 8 the game actually uses, from a trace |
| [vulkan-backend.md](vulkan-backend.md) | Our D3D8 device on Vulkan: design, how each D3D behavior is matched, what isn't implemented, tests |
| [scene.md](scene.md) | Where the 3D world ends and the interface starts in a frame, and the world's own render target (render scale, sharpening) |
| [camera.md](camera.md) | The tactical view, its WW3D camera, its per-call steps, and how our camera runs every frame |
| [generals.md](generals.md) | Porting the patches to the base game: how addresses are found, and what's been mapped |
| [discord.md](discord.md) | Discord status: why Discord loses the game, and what we send it |
| [audio.md](audio.md) | Sound muted when the window loses focus, and cutscenes ending on alt-tab |
| [text.md](text.md) | How the game renders text (display strings), and why menus rebuilt every label's texture every frame |
| [radar.md](radar.md) | How the minimap is made (terrain, fog of war, unit dots, camera box), our sharper one, and the fog of war in the world |
| [address-map.md](address-map.md) | Every address we rely on, in one table |
| [known-issues.md](known-issues.md) | Problems seen in game that are parked for later |
| [smoothness.md](smoothness.md) | Every visual system and whether it renders smoothly above 30 fps yet |
| [performance.md](performance.md) | Where the main thread's time goes, how to read the profiler, the game's render functions and what it rebuilds every frame |
| [options-menu.md](options-menu.md) | Our settings page in the options menu, the `.wnd` format, gadget messages, Options.ini |
| [tooling.md](tooling.md) | Ghidra setup, the query script, logs, traces, deploying builds |
