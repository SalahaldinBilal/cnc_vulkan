# Discord

Showing what's being played in Discord. Code: `crates/d3d8/src/discord.rs`.

## Why Discord doesn't see the game

- Discord recognizes games by executable path from its list of detectable games (`https://discord.com/api/v9/applications/detectable`). It knows the launcher (`generals.exe`, "Command & Conquer™: Generals and Zero Hour", 457301510515195924), which asks which game to start, and then game.dat only as `command & conquer generals - zero hour/game.dat` ("Command & Conquer: Generals - Zero Hour", 1215962340441198682) and `command and conquer generals/game.dat` ("Command & Conquer: Generals", 1216521049978699857). Those are the Steam folder names. The EA app installs Zero Hour in `Command and Conquer Generals Zero Hour`, which doesn't match, so the status ends when the launcher exits.

## What we do

- The Discord client listens on `\\.\pipe\discord-ipc-0` (up to 9). Frames are an opcode (u32), a length (u32) and JSON. A handshake (opcode 0, `{"v":1,"client_id":...}`) is answered with READY (opcode 1). Then `SET_ACTIVITY` frames (opcode 1) with our pid and the activity, each answered. An activity of null clears it. The status goes away by itself when the game exits and the pipe closes.
- We use Discord's own application for the game (the IDs above, Zero Hour or Generals by the game folder), so it says "Playing Command & Conquer: Generals - Zero Hour" with the game's icon. That application isn't ours, so it can't carry pictures of our own.
- The game is read on its own thread once every 2 seconds, from Present, into plain strings. A thread of ours connects (retrying every 10 seconds while Discord isn't running), and sends a change at most every 5 seconds (Discord allows 5 updates in 20 seconds).
- The elapsed time restarts when the first line changes.
- "Discord status" on the first tab of our page, on by default (`CncVulkanDiscord`). Turning it off clears the status.

## What it reads

| What | Where |
|---|---|
| Game mode | TheGameLogic (`0x00a39b0c`) `+0x94`: 0 campaign, 1 LAN, 2 skirmish, 3 replay, 4 the shell map (main menu), 5 online, 6 none. Compared in over a dozen places, for example `0x00492489` |
| Loading a map | TheGameLogic `+0x51` |
| Map | GlobalData `+0x8`, an AsciiString with the map's path (startNewGame passes it to loadMapINI and loadMap, `0x0049fbaa`). The label is the file name without `.map`. The path's case varies (in game it was `maps\md_gla03\md_gla03.map`, and the first version's case-sensitive `MD_` check then showed "Campaign: md_gla03"). Campaign maps are `MD_GLA03` and so on (`USA01` without `MD_` in Generals), shown as "GLA campaign, mission 3", and the cutscene maps around a mission (`_CINE`, `_INTRO`, `_END`) with "(cutscene)". Generals Challenge maps show the opponent from Campaign.ini's GeneralName (`GC_ChemGeneral` is "GUI:BioNameEntry_Pos1", Dr. Thrax). Mission titles exist in the string table for only 4 of the 15 Zero Hour missions (`MAP:MD_USA01Title` and so on), so they aren't used |
| Faction | the local player (ThePlayerList `0x00a395a0` `+0xc`) `+0x28`, its side, an AsciiString (Player::getSide `0x00452e00` copies it). Sides like `AmericaAirForceGeneral` are shown as "USA Air Force General" |
| AsciiString | a pointer to a 16-bit reference count and a 16-bit size, the characters after them at `+4` |

The two lines: "Skirmish on Alpine Assault" / "Playing as GLA Toxin General", "LAN game on ...", "Online game on ...", "Watching a replay on ...", "Loading ...", "In the menus". Base Generals reads the same things from its own addresses: TheGameLogic `0x00a6be84` with the game mode at `+0x98` and the loading flag (its m_loadingScene) at `+0x51`, and ThePlayerList `0x00a6b968`.

## Not done

- Opponents and player counts.
- The map's display name from the map cache, for maps whose file name differs from it.
