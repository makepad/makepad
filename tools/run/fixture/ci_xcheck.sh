#!/bin/bash
# ci_xcheck.sh <target> <bin|lib|std> <package>...: one CI cross-target shard (devhub ci, libs/devhub/src/orch/ci.rs):
# `cargo check --release --target <target>` of those packages, from the root of the tree under test. These are the
# targets the JIT does not build: the Apple-SDK targets (macOS x86_64, iOS, tvOS) on a Mac, Windows, web and Android
# from a Linux pool node. `lib` checks the libs only (web and Android build an app as a library); `std` is a tier-3
# target: nightly with -Z build-std=std. Compiler warnings fail the shard (devhub reads them from the short format).
# The CI box's flags, as tools/ci ran them: RUSTFLAGS empty (the workspace config's target-cpu=native is the host's
# CPU, which another target does not have) and MAKEPAD_BUILD " " (stable) or "lines" (nightly).
set -u
target=$1; mode=$2; shift 2
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 RUSTFLAGS= CARGO_TERM_COLOR=never
# a Linux pool node checks the cross targets with its own toolchain (tools/run/fixture/install_xtool.sh)
if [ -x "$HOME/devhub/xtool/cargo/bin/rustc" ] && [ "$(uname -s)" = Linux ]; then
  export RUSTUP_HOME=$HOME/devhub/xtool/rustup CARGO_HOME=$HOME/devhub/xtool/cargo PATH=$HOME/devhub/xtool/cargo/bin:$PATH
fi
pargs=()
for p in "$@"; do pargs+=(-p "$p"); done
case $mode in
  bin) MAKEPAD_BUILD=" " exec nice -n 10 cargo check --release --target "$target" "${pargs[@]}" --message-format short ;;
  lib) MAKEPAD_BUILD=" " exec nice -n 10 cargo check --release --target "$target" --lib "${pargs[@]}" --message-format short ;;
  std) MAKEPAD_BUILD=lines exec nice -n 10 cargo +nightly check --release --target "$target" -Z build-std=std "${pargs[@]}" --message-format short ;;
  *) echo "ci_xcheck.sh: mode is bin, lib or std, not $mode"; exit 2 ;;
esac
