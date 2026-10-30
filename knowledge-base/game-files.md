# Game Files

## Install layout (EA app)

The EA app installs both games into one folder, called `Command and Conquer Generals Zero Hour` (under `EA Games` in Program Files by default). Inside it:

- `Command and Conquer Generals Zero Hour\` is Zero Hour.
- `Command and Conquer Generals\` is the base game.
- `Core\Activation.dll` and `Activation64.dll` are EA's activation code.
- `__Installer\` holds DirectX and VC++ redistributables.

The Steam release is said to ship the same `game.dat` as the EA app for both games, so the same build checks would match. That isn't confirmed on a Steam install yet. On Steam the games are separate apps, each with `game.dat` directly in its folder: `steamapps\common\Command & Conquer Generals - Zero Hour` and `steamapps\common\Command and Conquer Generals` (the paths Discord detects, see [discord.md](discord.md)).

## The executables

- `Generals.exe` in each folder is only a launcher. It calls `CreateProcess`/`ShellExecute` to start the real game.
- `game.dat` is the real game, a 32-bit PE. Our DLL must be built for `i686-pc-windows-msvc`.
- Zero Hour is version 1.04 (see `Launcher.txt`).
- The EA app build contains Steam code. WinMain shows "Failed to initialize Steam" if that init fails, and the binary references `steam_api`.

## Imports of Zero Hour `game.dat`

Statically imported: `binkw32`, `DINPUT8`, `KERNEL32`, `USER32`, `GDI32`, `ADVAPI32`, `SHELL32`, `ole32`, `OLEAUT32`, `mss32`, `WINMM`, `WSOCK32`, `IMM32`, `MSVCIRT`, `MSVCRT`, `DBGHELP`, `COMCTL32`, `AVIFIL32`, `WININET`, `SHLWAPI`.

**D3D8 is not a static import.** It's loaded later with `LoadLibraryA("D3D8.DLL")`. See [loading-and-hooking.md](loading-and-hooking.md).

The CRT is the dynamic `MSVCRT.dll`, so the exe has no `GetCommandLineA` import of its own.

Strings in the binary name old driver DLLs (`nv4_disp.dll`, `ati2dvag.dll` and others). GPU detection at `0x0087b680` checks them.

## Where the game writes

`Command and Conquer Generals Zero Hour Data\` in the user's Documents folder (the name can be changed through the registry, see [options-menu.md](options-menu.md)):

- `Options.ini`: user settings. `Resolution = W H` is written once a resolution is picked in the options menu. A fresh install has no resolution line and the game defaults to 800x600.
- `ReleaseCrashInfo.txt` and `ReleaseCrashInfoPrev.txt`: the game's own crash reports, with the exception, registers and the bytes at EIP. Useful together with our minidumps.
- `Save\`: saved games.

The game folder is under Program Files, so it isn't writable without admin rights. Our logs go to `%LOCALAPPDATA%\cnc-vulkan\<zh|generals>\` instead.
