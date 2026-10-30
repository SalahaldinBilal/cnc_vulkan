# Loading and Hooking

## How the game loads D3D8

The D3D setup function at `0x00835730` calls `LoadLibraryA("D3D8.DLL")` and then looks up `Direct3DCreate8`. Two more functions (`0x007deae3`, `0x007defa6`) load `"d3d8.dll"` the same way. That code is statically linked D3DX helper code. All of them go through the game's `LoadLibraryA` import.

This happens about 3 seconds after start, after WinMain has created the window.

## Why we ship as dinput8.dll

A `d3d8.dll` in the game folder does get picked up (D3D8 isn't a KnownDLL), but it loads too late:

- The window already exists, so the DPI fix at window creation can't run.
- Any fix that has to run before WinMain is impossible.

`DINPUT8.dll` is a static import, so a `dinput8.dll` next to `game.dat` loads during process start, before the exe's entry point runs. Our DLL:

1. Exports `DirectInput8Create` and forwards it to the system `dinput8.dll` (loaded by full path from the system directory).
2. Hooks the game's `LoadLibraryA` import. Any request for `d3d8.dll` or `d3d8` gets our own module back. The game then calls `GetProcAddress(ourModule, "Direct3DCreate8")`, which we export. Base Generals imports `d3d8.dll!Direct3DCreate8` statically instead (import slot `0x00987708`). Windows binds that before our code runs, so that import slot is pointed at our Direct3DCreate8 too. Without it, Generals runs on the system D3D8, and nothing from device creation applies (resolution, windowed flag, our renderer).
3. Pins itself with `GetModuleHandleExW(PIN)` so a `FreeLibrary` from the game can't unload it.

Our `DllMain` runs on the main thread, before the exe's entry point and before WinMain.

## Import (IAT) hooks

We patch the main module's import address table, matching on `OriginalFirstThunk` names, so it works even if an AppCompat shim already replaced the IAT entry. The import directory is at NT header + `0x80` for PE32.

Imports we hook: `KERNEL32!LoadLibraryA`, `KERNEL32!SetUnhandledExceptionFilter`, `USER32!CreateWindowExA`, `SetWindowPos`, `ShowWindow`, `GetClientRect`, `ScreenToClient`, `ClientToScreen`, `ClipCursor`.

The game doesn't import `SetWindowLongA`.

## Crash filter

The game registers its own unhandled exception filter at `0x007f8b00` (it writes `ReleaseCrashInfo.txt`). We install ours first and hook `SetUnhandledExceptionFilter`, so the game's filter becomes the one we chain to. Ours writes a minidump to `%LOCALAPPDATA%\cnc-vulkan\<game>\crashes\`, then calls the game's.

## The D3D8 vtable problem

Why we stopped patching the system D3D8 and wrote our own device. With the system D3D8 on Windows 11, the `IDirect3DDevice8` vtable is in heap memory, not in `d3d8.dll`'s read-only data. The runtime rewrites some of its slots itself, probably when vertex processing mode changes (the game toggles `D3DRS_SOFTWAREVERTEXPROCESSING`). Hooks we patched into `SetStreamSource`, `SetIndices` and `SetVertexShader` never fired over 12,100 frames. Slots it left alone (`Reset`, `Present`, `TestCooperativeLevel`, draw calls) patched fine, as did the vtables of textures, surfaces and buffers, which live in `d3d8.dll`'s image. That patched path was removed once the Vulkan backend worked.

## Our own IDirect3D8

`Direct3DCreate8` (`direct3d.rs`) applies the settings that must be in place before the device exists, creates the system IDirect3D8 and returns our own wrapped around it. Its `CreateDevice` makes the Vulkan device ([vulkan-backend.md](vulkan-backend.md)).

- Our IDirect3D8 holds the system one. Every other method reads the system object's vtable at call time and forwards to it.
- `AddRef` and `Release` forward too, so our object shares the system object's count and is freed when it reaches 0. `QueryInterface` hands back our object when the system one returns itself.
- EA's `dx8wrapper.cpp` never calls `GetDevice`, `GetDirect3D` or `QueryInterface` and only compares the device pointer against NULL, so nothing in WW3D sees through our objects.

## Build fingerprinting

Fixed addresses are only used after a byte check passes. `game.rs` lists address and expected bytes pairs per known build, and each check is bounded by `SizeOfImage` from the PE header. The first build whose checks all pass is the one in use. The checked spots are in [address-map.md](address-map.md) for Zero Hour and [generals.md](generals.md) for Generals.

Two builds are known: Zero Hour 1.04 and Generals. Each `Build` says which game it is. Generals' entry only checks what's been ported, so each module decides whether it runs:

- A ported module keeps its addresses in a struct per build and gets the running build's with `game::pick(&ZERO_HOUR, Some(&GENERALS))`. It skips everything when that's `None`.
- A module not ported yet only installs when `game::zero_hour()` is true. Its hooks never run in Generals, so the Zero Hour constants inside them stay safe.
- A few settings that differ by build sit on `Build` itself: GlobalData's frame cap flag, the after intro flag, and how the display mode list filters aspect ratios.
