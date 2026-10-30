# GlobalData, INI Fields and Command Line

## TheGlobalData

The pointer is at `0x00a381bc`. It's null until the engine creates GlobalData, which happens before D3D is loaded.

| Offset | Type | Meaning | Source |
|---|---|---|---|
| `0x14` | bool | `UseFPSLimit` | INI field table |
| `0x18` | int | `FramesPerSecondLimit` | INI field table |
| `0x20` | bool | `Windowed` | INI field table, `-win` handler |
| `0x24` | int | X resolution | `-xres` handler |
| `0x28` | int | Y resolution | `-yres` handler |
| `0x3d` | bool | `StretchTerrain` | WorldHeightMap::setDrawOrg |
| `0x3f` | bool | `DrawEntireTerrain` | INI field table (entry `0x00944300`) |
| `0x178` | float | `MaxCameraHeight` | INI field table (entry `0x00944810`) |
| `0x17c` | float | `MinCameraHeight` | INI field table (entry `0x00944820`) |
| `0x7e4` | float | `CameraAdjustSpeed` | W3DView::update zoom easing |
| `0x7e8` | bool | `EnforceMaxCameraHeight` | INI field table (entry `0x009452b0`) |
| `0x924` | byte | checked by the frame limiter, meaning unknown | frame limiter |

The INI table also names `XResolution` and `YResolution` (strings at `0x009d3c08` and `0x009d3bfc`). Their table entries haven't been read yet.

## INI field table format

GameData INI fields are stored as 16-byte entries: name pointer, parse function, user data, offset into the struct. Examples:

| Entry at | Name | Parser | Offset |
|---|---|---|---|
| `0x009441c0` | `Windowed` | `0x0041b4e0` (bool) | `0x20` |
| `0x00944220` | `UseFPSLimit` | `0x0041b4e0` (bool) | `0x14` |
| `0x00944240` | `FramesPerSecondLimit` | `0x0041b1f0` | `0x18` |

`ResolutionFontAdjustment` is in a different table (entry at `0x00945a20`, parser `0x0041b2d0`, offset `0xe8`), probably the language settings struct.

To find a field: search the name string, then search for a pointer to that string (Query.java `ptr:`), then read the entry.

## Command line table

`0x009ee4c0`: 8-byte entries, flag name pointer then handler. The parser at `0x004f4840` walks it. Handlers return how many arguments they used.

| Flag | Handler |
|---|---|
| `-noshellmap` | `0x004f4520` |
| `-win` | `0x004f43c0` |
| `-xres` | `0x004f4440` |
| `-yres` | `0x004f4480` (by position in the table, matches its code) |

More entries follow. The table ends at about `0x009ee51f`.
