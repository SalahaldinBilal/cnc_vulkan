# Audio

What the game does with its sound when the window loses focus. Code: `crates/d3d8/src/audio.rs`.

## Losing focus (EA source `WinMain.cpp`, `GameAudio.cpp`)

- The window procedure's WM_ACTIVATE: when the low word of wParam is WA_INACTIVE it calls `ClipCursor(NULL)` and `TheAudio->loseFocus()`. On activation it sets the mouse limits and calls `regainFocus()`.
- AudioManager::loseFocus saves the four system volumes (music, sound, 3D sound, speech) in a new array and sets them all to 0. Nothing is paused. regainFocus writes them back and frees the array, and returns at once when there's no array.
- In game.dat (`0x004011a1`): `and eax, 0xffff` on wParam, `jnz` past the inactive branch, `push 0; call [ClipCursor]`, `mov ecx, [0x00a37a8c]` (TheAudio), `test; jz`, `mov edx, [ecx]; call [edx+0x28]` (loseFocus, at `0x004011c3`). Base Generals has the same block at `0x00401128`, with TheAudio at `0x00a69e40` and loseFocus at vtable `+0x30` (the call at `0x0040114a`).
- Ours ("Sound in background" on the first tab of our page, keep playing by default, `CncVulkanBackgroundAudio`): the 5 bytes of that call go to a function of ours, which makes the call only when the setting is off. regainFocus then finds nothing saved and does nothing. Applies at once.

## Cutscenes when the window isn't focused (not changed)

- The campaign's mission movie plays in SinglePlayerLoadScreen::init's own loop, which ends it as soon as `TheGameEngine->isActive()` is false (EA's comment: "Changing for MissionDisk, just skip to end"). The engine's active flag follows WM_ACTIVATEAPP. ChallengeLoadScreen fast-forwards through the movie instead. So alt-tabbing during a briefing ends it.
