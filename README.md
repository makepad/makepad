# Makepad

## Socials

- Discord: https://discord.gg/adqBRq7Ece
- Rik Arends: https://twitter.com/rikarends
- Eddy Bruel: -
- Sebastian Michailidis: https://bsky.app/profile/okpokpokp.bsky.social

Makepad is an AI-accelerated application development environment for Rust. It combines a high-performance UI runtime, a live-editable design language, and a fast iteration loop so you can build native and web apps with a tight feedback cycle.

This repository contains the core engine, widgets, tools, and examples.

## What Makepad Is

- A cross-platform UI runtime for native and web targets.
- A Rust-first framework with a scriptable UI DSL.
- A studio app for running, inspecting, and iterating on examples and projects.
- An AI-accelerated workflow: structure and tooling aimed at making code generation, refactoring, and iteration faster and safer.

## Features

- Streaming Splash: fast, animated, streaming UI example.
- Script Engine: live-editable UI DSL and runtime script integration.
- 3D Rendering: glTF example with GPU rendering.
- Maps: built-in map rendering with downloadable tiles.
- Voice Analysis: built-in voice support with Whisper model downloads.
- GPU-accelerated 2D and 3D rendering.
- AI automation inside Studio to control and inspect UI.

## Prerequisites

- Rust toolchain (stable works for native).
- For non-standard targets (iOS, tvOS, Android, wasm), install the Makepad build tool:

```bash
cargo install --path=./tools/cargo_makepad
```

Then install target toolchains as needed:

```bash
cargo makepad wasm install-toolchain
cargo makepad apple ios install-toolchain
cargo makepad apple tvos install-toolchain
cargo makepad android --abi=all install-toolchain
```

## Linux Dependencies

Linux build/runtime dependencies are listed in `./tools/linux_deps.sh`:
Use the apt-get command below, or run the script on Ubuntu/WSL2:

```bash
sudo apt-get update && sudo apt-get install -y --no-install-recommends build-essential pkg-config clang ca-certificates libssl-dev libx11-dev libxcursor-dev libxkbcommon-dev libxrandr-dev libxi-dev libxinerama-dev libasound2-dev libpulse-dev libwayland-dev wayland-protocols libegl1-mesa-dev libgl1-mesa-dev libgles2-mesa-dev libglx-dev libdrm-dev libgbm-dev libgl1-mesa-dri mesa-vulkan-drivers mesa-utils mesa-utils-extra x11-apps
```

## Build And Run Makepad Studio

Makepad Studio is the main entry point for exploring examples and iterating on UI.

```bash
cargo run -p makepad-studio --release
```

If you want a local install (note: may lag the repo):

```bash
cargo install makepad-studio
```

## Examples

Run a few representative apps directly from the repo:

```bash
# Splash (simple animated demo)
cargo run -p makepad-example-splash --release

# 3D rendering (glTF)
cargo run -p makepad-example-gltf --release

# Maps
cargo run -p makepad-example-map --release
```

## Maps And Voice Assets

For built-in maps and voice support, download the assets first:

```bash
./download_map.sh
./download_voice.sh
```

## Run A WASM App

1. Install toolchain:

```bash
cargo makepad wasm install-toolchain
```

2. Run an example:

```bash
cargo makepad wasm run -p makepad-example-splash --release
```

For smaller shipped wasm output, use the shipping-size optimization pass. It keeps the post-link size reduction behavior and pairs well with the existing `small` profile:

```bash
cargo makepad wasm build -p makepad-example-splash --profile=small --strip
```

To split the wasm payloads, add `--split`. Bare `--split` uses an automatic cold-first policy: it first moves defer-safe cold functions into a secondary wasm so startup can begin on the primary first, and if there are no useful cold candidates it falls back to the normal function split. To override the function-splitting threshold directly:

```bash
cargo makepad wasm build -p makepad-example-splash --release --strip --split=200
```

For maximum size reduction, combine `--wasm-opt` (Binaryen IR optimization) and `--brotli` (compression). Install Binaryen for `--wasm-opt` (e.g. `brew install binaryen` or `apt install binaryen`):

```bash
cargo makepad wasm build -p makepad-example-splash --release --wasm-opt --strip --split --brotli
```

Notes:

- `--strip` strips custom sections (names, producers, etc.) for smaller binaries.
- `--strip-custom-sections` preserves the old behavior when you only want to remove custom sections.
- `--wasm-opt` runs Binaryen `wasm-opt -Os` for IR-level optimization (optional; requires [Binaryen](https://github.com/WebAssembly/binaryen)).
- `--brotli` compresses `.wasm` and assets with Brotli for delivery.
- `--profile=small` uses smaller fonts and pairs well with `--strip`.
- `--no-threads` trims the web thread bridge and thread exports when threading is disabled.
- The wasm linker packs relocations before the post-link size and split passes.
- `--split` emits a primary wasm plus secondary payloads (`.secondary.wasm`, `.data.bin`) and implies function splitting.
- Bare `--split` uses an automatic cold-first split policy.
- Auto mode defers the secondary when it finds defer-safe cold functions, otherwise falls back to the normal startup-path split.
- `--split=200` switches to an explicit function-body threshold (bytes).

3. Open:

```text
http://127.0.0.1:8010
```

## Run An Android App

Plug in a device with developer mode enabled, then:

1. Install toolchain:

```bash
cargo run -p cargo-makepad --release -- android --target=all toolchain-install
```

2. Run an example:

```bash
cargo run -p cargo-makepad --release -- android run -p makepad-example-ironfish
```

## Notes

- Studio uses `cargo-makepad` internally for non-standard targets.
