# FNC Reborn

An unofficial native Windows port of **Fight Night Champion** (Xbox 360, 2011).
The game's PowerPC code is statically recompiled to x86-64 with the
[ReXGlue SDK](https://github.com/rexglue/rexglue-sdk), so it runs as a normal
Windows program rather than under an emulator.

**This repository contains no game code or data.** You build the game yourself,
on your own PC, from your own disc.

> FNC Reborn is a fan project. It is not affiliated with, endorsed by or
> sponsored by Electronic Arts, EA Sports or Microsoft.

## Status

Playable. The menus, Fight Now, Legacy mode, Champion mode, Create a Fighter
and saving and loading all work, with sound. The game runs at 60 FPS by default
(30, 120, 144, 165, 180 and 240 are one setting away) and has its own **DISPLAY** tab in the
Settings screen for frame rate, VSync, resolution scale, anti-aliasing and
fullscreen. A camera behind your boxer is one button away (Back on the
controller). Older CPUs are covered: the build adapts to the CPU of the PC it is
built on.

### Known issues

- Fighters can occasionally disappear for a moment after skipping an in-fight
  cinematic.
- Some menu text flickers slightly.
- The short cutscene right after a fight shows nothing.
- Only the disc version this port was made with is supported. Setup warns if
  your `default.xex` is different.

## Easy install

Download **FNCReborn-Setup.exe** from the
[latest release](https://github.com/whompay/FNCReborn/releases/latest), run
it, choose your `.iso` or a folder with your game files (the one with
`default.xex` and `databigs` in it), and press **Install**.

Setup downloads everything it needs: clang, CMake, Ninja, Microsoft's C++
headers and libraries, and Git if you don't have it. That is about 2.5 GB, all
kept in the install folder, and no Visual Studio is needed. Windows only asks
for permission if the Visual C++ runtime is missing. Setup then builds the
game, adds the DISPLAY tab to the Settings screen and adds an FNC Reborn
shortcut. The first build takes 15–60 minutes, depending on your CPU. You need
about 25 GB of free disk space.

To update, run Setup again or use the **Update FNC Reborn** shortcut. Only
what changed is rebuilt, and your saves and settings are kept.

## Requirements (manual build with setup.bat)

- Your own Fight Night Champion (Xbox 360) disc, dumped to an `.iso` file.
- Windows 10 or 11 (64-bit) and a GPU with Direct3D 12 support.
- [Visual Studio 2022](https://visualstudio.microsoft.com/downloads/)
  (Community or Build Tools, both free) with the **Desktop development with
  C++** workload and these individual components:
  - C++ Clang Compiler for Windows
  - C++ CMake tools for Windows
- [Git for Windows](https://git-scm.com/download/win).
- Optional: [Python 3](https://www.python.org/downloads/), for the DISPLAY tab
  in the Settings screen. Without it the game works the same; the settings are
  then only in `dist\FNCReborn.toml`.
- About 25 GB of free disk space and 16 GB of RAM. The first build takes
  30–90 minutes depending on your CPU.

## Building manually

1. Download this repository to a short path, for example `C:\FNCReborn`.
2. Run `setup.bat`.
3. Select your Fight Night Champion `.iso` when asked.

When it finishes, the game is in the `dist` folder. If a step fails, fix the
cause and run `setup.bat` again; finished steps are skipped. See
[docs/BUILDING.md](docs/BUILDING.md) for what each step does and for
troubleshooting.

## Playing

Run `dist\FNCReborn.exe`. An Xbox controller works as on the console.

| Key / button | Effect |
|---|---|
| Back (View on newer controllers) or F2 | Switch between the TV camera and a camera behind your boxer |
| F11 | Fullscreen / window |
| F1 | Frame rate counter |

Saves are stored in `Documents\FNCReborn`.

### Settings

Most settings are in the game's Settings screen under **DISPLAY**. Everything
is also in `dist\FNCReborn.toml` (setup creates it once and never overwrites
it). The useful ones:

| Setting | Effect |
|---|---|
| `fnc_frame_rate` | Frame rate cap: `30` (original), `60` (default), `120`, `144`, `165`, `180` or `240` |
| `vsync` | `true` or `false` |
| `resolution_scale` | Internal resolution: `1` (720p) or `2` (1440p) |
| `fullscreen` | Start in fullscreen (`true`) or a window (`false`) |
| `texture_cache_memory_limit_soft` / `_hard` | Texture memory in MB. Lower to `1024` / `2048` on a GPU with 4 GB or less |
| `fnc_tp_back`, `fnc_tp_height`, `fnc_tp_side`, `fnc_tp_fov` | Position and zoom of the camera behind your boxer |
| `fnc_tp_smooth` | How smoothly that camera follows (`0` rigid to `0.95` very floaty) |
| `fnc_view_boxer` | Set to `1` if that camera follows your opponent instead of you |
| `fnc_view_first_person` | `true` turns that camera into a first-person view |

## How it works

The ReXGlue SDK translates every PowerPC function in the game's executable to
C++ and reimplements the Xbox 360 kernel, GPU (on Direct3D 12), audio and
input. This repository adds the Fight Night Champion-specific parts:

| Path | Contents |
|---|---|
| `config/fnc_manifest.toml` | Recompiler configuration: functions static analysis misses |
| `config/FNCReborn.toml` | Default settings |
| `project/src/fnc_hooks.cpp` | Frame rate unlock |
| `project/src/player_camera.cpp` | The camera behind your boxer |
| `project/src/fnc_settings_menu.cpp` | Values for the DISPLAY tab |
| `project/src/fnc_fps_overlay.cpp` | F1 frame rate counter, F11 fullscreen |
| `project/src/fnc_sampler.cpp` | Crash reports and an optional profiler |
| `patches/rexglue-sdk.patch` | Changes to the SDK that the game needs |
| `installer` | FNC Reborn Setup (the one-click installer) |
| `tools/xiso_extract` | Xbox disc image (XDVDFS) extractor |
| `tools/display_tab` | Adds the DISPLAY tab to your copy of the game's Settings screen |
| `scripts/setup.ps1` | The build script behind `setup.bat` |

[docs/HOW_IT_WORKS.md](docs/HOW_IT_WORKS.md) explains the individual fixes.

## Contributing

Bug reports and fixes are welcome. See [CONTRIBUTING.md](CONTRIBUTING.md).

## Legal

This project distributes only original source code, configuration files and a
patch to the BSD-licensed ReXGlue SDK. It does not include, and must not be
used to distribute, any part of Fight Night Champion: no disc images, game
files, recompiled code or built executables. Dump your own disc. Requests for
or links to game files will be removed.

The project's own code is released under the [Apache License 2.0](LICENSE).
If you share, re-upload or build on it, you must keep the [NOTICE](NOTICE) file
and credit it as **FNC Reborn by Whompay**. The SDK and the libraries it
downloads during the build are covered by their own licenses; see
[THIRD_PARTY_NOTICES.md](THIRD_PARTY_NOTICES.md). "Fight Night" and "Fight
Night Champion" are trademarks of their owner and are used here only to name
the game this project works with.

## Credits

- **FNC Reborn by Whompay**: the project itself.
- [ReXGlue SDK](https://github.com/rexglue/rexglue-sdk) by Tom Clay and
  contributors.
- [Xenia](https://xenia.jp) by Ben Vanik and contributors, which the SDK is
  derived from.
- illusion, for the Fight Night Champion "Unlock FPS" patch in
  [xenia-canary/game-patches](https://github.com/xenia-canary/game-patches),
  which pointed to the game's frame timer.
- EA Canada, for the game.
