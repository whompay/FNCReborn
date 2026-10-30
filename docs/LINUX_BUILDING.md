# Building the Windows port from Linux

The Linux installer cross-compiles the Windows game and SDK and builds a native
Linux host recompiler. This is a WINE/Proton path, not a native Linux game port.
The original SDK commit and game executable requirements remain unchanged.

From the source root, build Setup with Rust 1.88 or newer:

```sh
cargo build --locked --release -j 2 --manifest-path installer/Cargo.toml
```

The portable LLVM/CMake/Ninja/xwin toolchain, Windows SDK/runtime and native
recompiler can be prepared without disc data:

```sh
FNC_BUILD_JOBS=2 ./installer/target/release/FNCReborn-Setup \
  --sdk-only "$PWD" --accept-license
```

`--accept-license` accepts Microsoft's terms for the downloaded headers and
libraries; review them first. To build from your own disc image:

```sh
FNC_BUILD_JOBS=2 ./installer/target/release/FNCReborn-Setup \
  --cli --dir "$PWD" --iso /path/to/your/game.iso \
  --accept-license --no-download --no-integration
```

Keep `--no-download` when testing local source changes. After extraction,
rebuilds can omit `--iso`. `--no-integration` avoids creating menu entries or
remembering the install folder. Existing TOML settings are preserved.

`FNC_BUILD_JOBS` overrides the automatic CPU/RAM-based parallel build limit
for the SDK, host recompiler and game. It accepts a positive integer; unset,
zero, invalid or overflowing values retain automatic selection.

The SDK patch allows explicit SDL cache options to override Linux backend
defaults. This lets Setup's console-only host build disable desktop/audio
backends. Normal native runtime configurations still default to enabling them.
Setup locates the host recompiler in the SDK source tree's `out` directory,
where the pinned SDK writes it, with a fallback to the CMake build tree.

Validation: locked release Setup build, SDK-only build, host recompiler help
and Windows game cross-build completed on Linux x86-64. The Windows game was
played under Proton. This does not validate a native Linux game build or
execution on a Windows host. Share source and diagnostic observations only,
following CONTRIBUTING.md; keep game files and generated code local.
