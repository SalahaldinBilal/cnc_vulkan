# cnc-vulkan

A mod for Command & Conquer Generals and Zero Hour that replaces the game's Direct3D 8 renderer with Vulkan and fixes the things that make the game hard to play on a modern PC.

It's a single `dinput8.dll` that goes next to the game. No game files are changed, and removing the DLL brings the original game back.

## Features

- **High frame rates.** The game draws as fast as your PC and monitor allow, while the game itself still runs at its original 30 updates per second. Units, the camera, particles, water and the interface move smoothly in between, and nothing runs faster than it should.
- **Any resolution.** Every resolution your monitor supports shows up in the options menu, including ultrawide and 4K.
- **An interface that scales.** Menus and the control bar keep their shape at any resolution and aspect ratio, with a wide control bar and an adjustable interface size.
- **Borderless, windowed and exclusive fullscreen**, on the monitor of your choice, without the alt-tab crash, DPI scaling problems or a stretched picture.
- **Better graphics.** Render scale (25% to 200%), MSAA, FXAA, sharpening, ambient occlusion, soft particles, anisotropic filtering, smooth fog of war edges and a sharper minimap.
- **Quality of life.** Camera zoom out and zoom speed, a frame limit, vsync, keeping the mouse in the window, sound while the game is in the background, skipping the intro movies, and Discord status.

## Requirements

- Windows 10 or 11.
- A GPU and driver with Vulkan 1.3.
- Command & Conquer Generals Zero Hour 1.04, or Command & Conquer Generals as the EA app installs it. Tested with the EA app versions of both.

## Installing

1. Download `cnc-vulkan-<version>.zip` from the latest release on the [Releases](../../releases) page and unzip it.
2. Find the folder that holds the game's `game.dat` and copy `dinput8.dll` and `cnc-vulkan.toml` next to it.
   - EA app: both games are installed into `Command and Conquer Generals Zero Hour`, under `EA Games` in Program Files by default. Inside it there's a second folder with the same name, and that inner folder holds Zero Hour's `game.dat`. The outer folder is never read.
   - Steam: Zero Hour is in `steamapps\common\Command & Conquer Generals - Zero Hour`, with `game.dat` directly inside. To find it, right-click the game in your library, then Manage, then Browse local files.
3. For base Generals, also copy both files next to its own `game.dat`. In the EA app install that's the `Command and Conquer Generals` folder. On Steam, Generals is a separate game with its own folder, `steamapps\common\Command and Conquer Generals`.
4. Start the game as usual.

Program Files needs admin rights to write to, so Windows may ask for permission when you copy the files. The copy fails while the game is running.

To uninstall, delete `dinput8.dll` and `cnc-vulkan.toml` from the game folders.

## Settings

Open **Options** in the game. Three buttons next to the display options open the mod's settings page:

- **Display**, next to Resolution: display mode, monitor, vsync, frame limit, interface layout and size, mouse, sound, intro and Discord.
- **Graphics**, next to Detail: render scale, multisampling, FXAA, sharpening, ambient occlusion, texture filtering, soft particles and shadow brightness.
- **Map**, next to Brightness: camera zoom out and speed, minimap resolution, minimap terrain, and the fog of war options.

They're saved in the game's own `Options.ini` with the rest of its settings. The frame limit, the mouse setting and the Graphics and Map tabs apply right away. Display mode, monitor, vsync and the interface settings apply after a restart.

`cnc-vulkan.toml` only holds a few things that aren't in the menu, mostly for debugging. Each option is explained in the file.

## Reporting a problem

Each run writes a log:

- Zero Hour: `%LOCALAPPDATA%\cnc-vulkan\zh\cnc-vulkan.log`
- Generals: `%LOCALAPPDATA%\cnc-vulkan\generals\cnc-vulkan.log`

The previous run's log is kept next to it as `cnc-vulkan.previous.log`. If the game crashed, there's a dump in the `crashes` folder in the same place.

When you open an issue, include:

- The mod version. It's on the first line of the log (`cnc-vulkan 0.1.0 loaded into ...`), and in the DLL's file properties under Details.
- Which game, and your GPU and driver version.
- The log, and the crash dump if there is one.
- What you were doing when it happened.
- A screenshot or video showing the problem, when you can.

Near the top, the log should say `recognized Zero Hour 1.04` or `recognized Generals`. If it says `unrecognized game.dat build` instead, your copy of the game isn't supported yet and the mod has turned itself off.

## Building from source

You need Rust with the `i686-pc-windows-msvc` target, and Visual Studio's C++ build tools with a Windows SDK.

```
rustup target add i686-pc-windows-msvc
cargo build --release
```

The DLL is `target\i686-pc-windows-msvc\release\dinput8.dll`. The game is 32-bit, so the DLL is too. The repo's cargo config already picks that target.

`cargo test -p d3d8 --release` runs the tests. The GPU test opens a small window for a few seconds.

The [knowledge base](knowledge-base/README.md) documents everything found about the game's internals while building this, and why each patch exists. Start there before changing anything.
