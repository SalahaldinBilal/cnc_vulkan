# Display and Device

## What the game asks for

Seen in our log:

- `CreateDevice(adapter 0, HAL, behavior 0x80)`. `0x80` is `D3DCREATE_MIXED_VERTEXPROCESSING`. `D3DCREATE_FPU_PRESERVE` is not set.
- Fullscreen request: back buffer `A8R8G8B8`, count 2, `DISCARD`, depth `D24S8`, refresh 0 (driver default).
- With the `Windowed` flag set: back buffer count 1, `Windowed = 1`.
- The game enumerates all adapters. D3D8 lists one per monitor, so one GPU driving several monitors shows up several times.
- It calls `GetAdapterIdentifier` many times, and D3DX texture creation calls `GetDirect3D`, `GetDeviceCaps` and `GetDisplayMode` once per texture.

## Reset

- In fullscreen mode the game calls `Reset` right after `CreateDevice` with the same parameters, and twice more on every alt-tab.
- In windowed mode it only calls `Reset` when the resolution changes in the options menu.

## Windowed flag

`TheGlobalData + 0x20` (bool). The `-win` handler (`0x004f43c0`) sets it. We set it from our `Direct3DCreate8` hook, which runs inside the game's D3D init, before the game reads the flag to fill its present parameters.

## Resolution

- X at `TheGlobalData + 0x24`, Y at `+ 0x28`, both int. Written by `-xres` (`0x004f4440`), `-yres` (`0x004f4480`) and the `Options.ini` `Resolution` line.
- Writing them from our `Direct3DCreate8` hook works. The device and all UI come up at the new size.
- Default with no `Options.ini` line: 800x600.
- Picking a mode in the options menu calls `Reset` with the new size and saves it to `Options.ini`.

## Options menu mode list

The mode count and mode description functions (around `0x0073c09b` and `0x0073c128`) keep a mode only if:

- width is at least 800 (`cmp eax, 0x320`)
- bit depth is at least 24
- the aspect ratio is strictly between the float at `0x00967ab4` (1.332) and the float at `0x00967ab0` (1.334)

Those two floats are only used by these two functions. Setting them to 0 and 100 makes every aspect ratio show up. Confirmed in game: 16:10 and 16:9 modes are all listed.

## Refresh rate

In exclusive fullscreen the game passes refresh rate 0, which often means 60 Hz. Our fullscreen mode looks up the highest rate the adapter offers at the chosen size and depth with `EnumAdapterModes`.

## Present

Windowed D3D8 stretches the back buffer to the client area on `Present`. An 800x600 back buffer in a large window is stretched and blurry. With the resolution override it renders at native size and looks sharp.
