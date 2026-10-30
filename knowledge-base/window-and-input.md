# Window and Input

## WinMain (`0x004016f0`)

1. Sets the current directory to the exe folder.
2. Tokenizes the command line itself with a strtok-like helper (`0x00401590`). Two flags are checked right here, `-win` among them (sets `0x00a35a2c`). The full flag table is parsed later (see [global-data.md](global-data.md)).
3. Loads the `Install_Final.bmp` splash.
4. Registers class `"Game Window"` with window procedure `0x00401000`.
5. Creates the window at 800x600, centered on the primary monitor with `GetSystemMetrics`.
   - Style without `-win`: `0x90080008` (`WS_POPUP | WS_VISIBLE | WS_SYSMENU`), then `SetWindowPos(HWND_TOPMOST)`.
   - Style with `-win`: `0x90C80000` (adds `WS_CAPTION`), no topmost.
6. Stores the window handle at `0x00a35a28` and the instance at `0x00a35a24`.
7. Initializes Steam, creates a single-instance mutex, then runs the engine.

## Window procedure (`0x00401000`)

- Messages first go to an object at `0x00a37aa8` (vtable `+0x58` decides whether it consumed the message).
- `WM_SETCURSOR`: calls the mouse object (`0x00a35a30`) to set the cursor.
- `WM_SIZE`, `WM_ACTIVATE`: on deactivate it calls `ClipCursor(NULL)` and notifies the input objects.
- `WM_ACTIVATEAPP`: tracks focus in `0x00a35a3d` and tells the engine (`0x00a37aa0` vtable `+0x3c`).
- `WM_SYSCOMMAND`: without `-win`, move, size, maximize, keyboard menu and screensaver are swallowed.
- `WM_KEYDOWN` with Escape: `PostQuitMessage` if nothing else handled it.
- `WM_MOUSEMOVE`: ignored if outside `GetClientRect`. Otherwise passed with the raw `lParam` to `0x0073b9a0`, which queues the mouse event.
- Button messages `0x201` to `0x209`: also go to `0x0073b9a0`.
- `WM_MOUSEWHEEL`: checked against `GetWindowRect` (screen coordinates).

Mouse position also comes from `GetCursorPos` in `0x007a19a0` and `0x007ad110`. Keyboard goes through DirectInput 8 (`0x007ac1c0`).

## Our borderless handling

- The device is always created windowed. The window is restyled to `WS_POPUP | WS_VISIBLE` covering the chosen monitor, with topmost removed.
- If the window's client area and the back buffer differ in size, mouse messages, `GetClientRect`, `ScreenToClient` and `ClientToScreen` are mapped into back buffer space so the game never notices.
- The cursor is clipped to the window while it has focus and released on deactivate.
- The game can't minimize its borderless window.

## DPI

- `SetProcessDpiAwarenessContext` fails with error 5 (access denied). The process DPI mode is already set before our DLL runs.
- A window gets its DPI mode from the thread that creates it, and something resets the main thread to DPI unaware before WinMain. Setting it in `DllMain` isn't enough.
- What works: our `CreateWindowExA` hook sets the thread to per-monitor aware right before the real call. The log shows the window comes out per-monitor aware.
- Symptom when it fails: at 125% Windows scaling the window measures 2048x1152 instead of 2560x1440, and DWM scales the picture up so it runs off screen.

## The alt-tab crash

If the game thinks it's fullscreen (`Windowed` flag off) but the device is really windowed, alt-tab makes the game run its fullscreen recovery: two `Reset` calls, then rebuild. That path crashes in `0x0084f630` (a texture's D3D object is null).

Fix: set the game's `Windowed` flag (GlobalData `+0x20`) before it creates its device. The game then asks for a windowed device itself and never calls `Reset` on alt-tab. See [display-and-device.md](display-and-device.md).

Earlier lesson: changing the window style while a device exists made `Reset` return `D3DERR_DEVICELOST`, and the D3D8 runtime's own window handler then crashed on a null pointer. Window changes now happen only before `CreateDevice` or after `Reset` returns, and only if something changed.
