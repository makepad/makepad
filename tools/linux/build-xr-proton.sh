#!/usr/bin/env bash
# Cross-build our dependency-free PE helper with an already-installed Rust target.
# Nothing is downloaded or installed by this command.
set -euo pipefail
xr_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/../.." && pwd)
xr_target=x86_64-pc-windows-msvc
xr_sysroot=$(rustc --print sysroot)
xr_host=$(rustc -vV | sed -n 's/^host: //p')
xr_lld="$xr_sysroot/lib/rustlib/$xr_host/bin/rust-lld"
xr_libdir=$(rustc --target "$xr_target" --print target-libdir)
if [[ ! -d "$xr_libdir" || ! -x "$xr_lld" ]]; then
    printf 'This build needs the Rust %s target and rust-lld already installed.\n' "$xr_target" >&2
    exit 1
fi
cd "$xr_root"
# This is a cross-target build: the repository's host-native CPU flags cannot
# describe an x86 Windows executable when building on an ARM host.
CARGO_TARGET_X86_64_PC_WINDOWS_MSVC_LINKER="$xr_lld" \
RUSTFLAGS='-C target-cpu=x86-64 -C panic=abort' \
    cargo build --offline --release -p makepad-xr-proton --target "$xr_target"
