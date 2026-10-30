# Tooling

## Ghidra

- Ghidra 12.1.4, wherever it's installed (`<ghidra>` below). Needs JDK 21 or newer on PATH (JDK 25 works).
- Project: `re/ghidra/cnc`, with `/zh/game.dat` and `/generals/game.dat`. Ignored by git. The first analysis took about 5 minutes per binary.

Import a binary (only needed once):

```powershell
& "<ghidra>\support\analyzeHeadless.bat" "<repo>\re\ghidra" cnc/zh -import "<path to game.dat>" -overwrite
```

Run queries (read-only, nothing saved to the project):

```powershell
& "<ghidra>\support\analyzeHeadless.bat" "<repo>\re\ghidra" cnc/zh -process game.dat -noanalysis -readOnly `
  -scriptPath "<repo>\re\scripts" -postScript Query.java "<repo>\re\out\result.txt" "str:-xres" "fn:004016f0"
```

## Query.java

`re/scripts/Query.java` takes an output file and any number of queries:

| Query | Result |
|---|---|
| `str:<text>` | Every string containing the text (ASCII or UTF-16) and the functions that reference it |
| `imp:<name>` | Every call site of an imported function |
| `fn:<hex>` | Decompiled C for the function containing the address |
| `mkfn:<hex>` | Creates a function at the address if Ghidra missed it, then decompiles it |
| `callers:<hex>` | References to an address |
| `disasm:<hex>:<n>` | n instructions from the address, with raw bytes |
| `bytes:<hex>:<n>` | n raw bytes |
| `ptr:<hex>` | Every place the 4-byte value appears, with the 16 bytes that follow (finds table entries) |
| `find:<hex bytes>` | Every place a byte sequence appears, with references to it |
| `table:<hex>:<entries>:<stride>` | Dumps a table, showing the string behind any pointer to one |

Results go to `re/out/` (ignored by git).

## Port.java

`re/scripts/Port.java` finds code from one build in another, for porting addresses from Zero Hour to Generals ([generals.md](generals.md)). `sig <out> <list>` on the source program writes masked byte patterns around each listed address (`<hex address> <length>` per line). `find <out> <sigs>` on the target program searches them and prints the matching address with the bytes found there.

```powershell
& "<ghidra>\support\analyzeHeadless.bat" "<repo>\re\ghidra" cnc/zh -process game.dat -noanalysis -readOnly `
  -scriptPath "<repo>\re\scripts" -postScript Port.java sig "<repo>\re\out\port\zh_sigs.txt" "<repo>\re\out\port\zh_checks.txt"
& "<ghidra>\support\analyzeHeadless.bat" "<repo>\re\ghidra" cnc/generals -process game.dat -noanalysis -readOnly `
  -scriptPath "<repo>\re\scripts" -postScript Port.java find "<repo>\re\out\port\generals_found.txt" "<repo>\re\out\port\zh_sigs.txt"
```

## Match.java

`re/scripts/Match.java` pairs functions between Zero Hour and Generals and maps lists of Zero Hour addresses ([generals.md](generals.md)). Run it on Generals. It opens `/zh/game.dat` from the same project. Arguments: an output folder, optionally `seed:<Port find output>` to start from byte pattern matches, then address lists (`<hex address> [length] [label]` per line, other lines are copied as they are). The `seed:` prefix uses a colon because the headless launcher splits arguments at `=`.

```powershell
& "<ghidra>\support\analyzeHeadless.bat" "<repo>\re\ghidra" cnc/generals -process game.dat -noanalysis -readOnly `
  -scriptPath "<repo>\re\scripts" -postScript Match.java "<repo>\re\out\port\match" `
  "seed:<repo>\re\out\port\generals_found.txt" "<repo>\re\out\port\zh_checks.txt" "<repo>\re\out\port\zh_constants.txt"
```

It appends a line to `progress.log` after every step, rewrites `functions.txt` (`<zh entry> <generals entry> <method> <zh instructions> <generals instructions>`) after every step, and writes `<list>.mapped.txt` per list with the method, the Zero Hour bytes and the Generals bytes, or why an address couldn't be placed. The whole run takes under a minute, with `GHIDRA_HEADLESS_MAXMEM` set to `4G`.

Tips:

- Ghidra misses functions that are only reached through tables (command line handlers, window procedures). Use `mkfn:`.
- Decompiled output of big functions loses `sscanf` arguments and stack slots. Use `disasm:` for exact offsets.
- A string with no references is usually reached through a table. Search for a pointer to it with `ptr:`.

## bigtool

`crates/bigtool` lists and extracts files from `.big` archives (header `BIGF`, big-endian count and index size, then offset, size and a NUL-terminated name per entry).

```powershell
cargo run -q -p bigtool --release -- "<game folder or .big>" "<name filter>" "<repo>\re\out\big"
```

Without the output folder it only lists matches. Extracted files go under `re/out/big/<archive>/...`, which git ignores. `crates/d3d8` has a test (`builds_from_the_real_layout`) that runs our options layout generation against the extracted `OptionsMenu.wnd` and writes the result to `re/out/OptionsMenu.generated.wnd`.

## Our DLL

- Build: `cargo build --release` (the default target is `i686-pc-windows-msvc`, static CRT). Output: `target\i686-pc-windows-msvc\release\dinput8.dll`.
- Version: `version` under `[workspace.package]` in the root `Cargo.toml`. The first log line prints it (`cnc-vulkan <version> loaded into ...`), and `crates/d3d8/build.rs` embeds it as the DLL's Windows version resource (file properties, crash dumps) through `embed-resource`, which needs the Windows SDK's `rc.exe`. Raise it for every release.
- Changes go under `## [Unreleased]` in `CHANGELOG.md` as they land, written for players.
- Release: run `npm run version:bump <version>` (`scripts/bump-version.ts`, Node 22.18 or newer runs TypeScript directly), which sets the version in `Cargo.toml` and `Cargo.lock` and turns the Unreleased section into a dated one for that version. Commit, then push a `v<version>` tag. `.github/workflows/release.yml` checks that the tag, the version and a changelog section all match, builds on a Windows runner (no tests, the runner has no GPU), and makes a GitHub release with that changelog section as the notes. It attaches `cnc-vulkan-<version>.zip` (`dinput8.dll` and `cnc-vulkan.toml`) and `cnc-vulkan-<version>-pdb.zip`. Crash dumps can only be read against the PDB of the exact build that crashed.
- Deploy: copy `dinput8.dll` and `dist\cnc-vulkan.toml` next to `game.dat`, which is in the inner `Command and Conquer Generals Zero Hour` folder of the install (see [game-files.md](game-files.md)). Not the outer folder of the same name, which the game never reads. Under Program Files the inner folder needs write access for the user account. The first log line shows the folder game.dat ran from. The copy fails while the game is running.
- After every deploy, check the log says `recognized Zero Hour 1.04` (or `recognized Generals` in the base game, whose log is under `generals`). If a byte check is wrong, the log names its address and every game patch is off, so the game silently runs as the original (30 fps, no windowed fix, alt-tab crash). That happened once with a check placed one byte early.
- Log: `%LOCALAPPDATA%\cnc-vulkan\<zh|generals>\cnc-vulkan.log`. The previous run's log is kept as `cnc-vulkan.previous.log`.
- Crash dumps: `%LOCALAPPDATA%\cnc-vulkan\<game>\crashes\`. The game catches exceptions in its main loop and shows its own "Technical Difficulties" box, which writes no dump. For those, the log has an `access violation at ...` line (the first 8 of a run) with the address, registers and the return address on top of the stack.
- Vulkan stats: a `vulkan per frame:` line every 5 seconds in the log ([vulkan-backend.md](vulkan-backend.md)).
- Timers: `timers = true` under `[debug]` adds two lines every 5 seconds. `render systems per frame` times the game's render systems on the CPU (Generals only so far), including RTS3DScene::Render and Customized_Render (the object list) through their vtable entries and W3DView::draw, so little of the draw is left untimed, with each system's calls and draws per frame (draws counted the same way as time, without the timed systems under it): each call site in `render_timers.rs` goes through a stub that swaps the return address, so it works whatever the calling convention, and each system counts its time without the timed systems under it, so they add up. Our draw preparation under them is included. `gpu per frame` comes from GPU timestamps between the frame's sections ([vulkan-backend.md](vulkan-backend.md)). Turn the profiler off while timing, since its samples suspend the main thread. A unit test calls through the stubs with cdecl, stdcall, thiscall, a float return and a nested timed call.
- The `render ... fps` line every 5 seconds also has the frame time median, 99th percentile and worst (tells stutter from a low frame rate), the same for the frames that ran the 30 Hz tick and for the others (the logic step lands on tick frames), the average time per frame in the engine update (logic ticks, radar, audio, network), the client update, message translation (input), and W3DDisplay::draw (the game's drawing, including our Vulkan recording and present), and the engine update per tick. The game's draw runs inside its client update (GameClient::update calls TheDisplay->draw), so the client update number includes the draw.
- Stalls: when no frame has been presented for 500 ms, a background thread (`watchdog.rs`) suspends the main thread for a moment and logs `main thread stalled <ms>`, with the module and offset it's in and the game's return addresses on its stack, nearest first. It repeats every 500 ms while the stall lasts. Loading screens show up too. Look the addresses up with `fn:` queries.
- Profiling: `profile = true` under `[debug]` in `cnc-vulkan.toml` makes the same thread sample the main thread about every millisecond. Every 5 seconds it logs two `profile` lines: where the main thread was (game code by 16-byte block, other modules with the game call site that led there), and the share of samples under each game return address on the stack (inclusive time per call site). Samples inside our DLL show as `ours+<offset>` (16-byte blocks), and a third line gives the share under each return address into our code, counting only our frames above the nearest game frame. The build writes a linker map (`/MAP` in `.cargo/config.toml`) to `target\i686-pc-windows-msvc\release\deps\dinput8.map`: an offset is `Rva+Base - 0x10000000`, so the function is the last entry at or below `0x10000000 + offset`. The map only fits the DLL from the same build. dbghelp in the game process didn't load our PDB (every lookup fell back to the nearest export, DllMain), so names aren't resolved in the game. Look the game addresses up with `fn:` queries. Only the "where the main thread was" line is exact: both "under" lines scan the stack, count stale return addresses too, and overstate (see [performance.md](performance.md)). A decoded call is `[reg+offset]` on the device: IDirect3DDevice8 slot = offset / 4 (`0x11c` DrawIndexedPrimitive, `0x3c` Present). Off by default, since every sample briefly suspends the main thread.
- Pipeline cache: `%LOCALAPPDATA%\cnc-vulkan\<game>\vulkan-pipelines.bin`. Deleting it only makes the next start compile pipelines again.
