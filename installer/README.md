# FNC Reborn Setup

`FNCReborn-Setup` installs and updates FNC Reborn on Windows. It builds the
game on the player's own PC from their own disc image or game folder. Neither
the program nor the release contains game code.

On Linux, Setup builds a Windows executable for WINE/Proton using a portable
cross toolchain and a native host recompiler. This is not a native Linux game
build. See [Linux development](../docs/LINUX_BUILDING.md) for validation.

What it does:

1. Downloads a portable build toolchain into the install folder
   (`build/toolchain`, on the same drive as the game): LLVM/clang 19.1.5 (only
   the parts the build uses, about 0.7 GB), CMake, Ninja, and Microsoft's C++
   headers and libraries through [xwin](https://github.com/Jake-Shadle/xwin).
   The player accepts Microsoft's license for those in the window. Every
   download is checked against a SHA-256 hash in `src/config.rs` and unpacked
   by Setup itself. Nothing is installed system-wide, and Visual Studio is not
   needed. Portable Git is downloaded when Git is missing, and the Visual C++
   runtime is installed when it is missing. Setup stops early when the drive
   has less than 12 GB free.
2. Downloads this repository (a shallow git clone) or updates it.
3. Extracts the disc image, or copies a folder with the game files, to
   `dist/game`.
4. Builds the ReXGlue SDK with `patches/rexglue-sdk.patch`, recompiles
   `default.xex`, builds the game and copies it to `dist` with the default
   settings. Each step is skipped when its inputs have not changed.
5. Adds the DISPLAY tab to the game's Settings screen in the player's own
   `fe.big` (`src/display_tab.rs`; the original is kept as `fe.big.orig`), and
   creates shortcuts.

## Command line

```
FNCReborn-Setup --cli --dir <folder> [--iso <file> | --game-dir <folder>]
                --accept-license [--update] [--force] [--no-shortcut]
FNCReborn-Setup --display-tab <fe.big>
FNCReborn-Setup --sdk-only <source folder> --accept-license
```

## Building

```
cd installer
cargo build --release
```

Set `FNC_BUILD_JOBS` to a positive integer to override Setup's automatic
parallel build limit (for both SDK and game builds). For example, on Linux:

```sh
FNC_BUILD_JOBS=2 ./installer/target/release/FNCReborn-Setup --sdk-only "$PWD" --accept-license
```

Unset, zero or invalid values keep the automatic CPU/RAM-based limit.
See [Linux development](../docs/LINUX_BUILDING.md) for local builds and
the Proton validation workflow.

CI builds the program and attaches `FNCReborn-Setup.exe` to every published
release (`.github/workflows/setup-app.yml`).
