#!/bin/bash
# ci_xcheck.sh <target> <bin|lib|std> <package>...: one CI cross-target shard (devhub ci, libs/devhub/src/orch/ci.rs):
# `cargo check --release --target <target>` of those packages, from the root of the tree under test. These are the
# targets the JIT does not build: the Apple-SDK targets (macOS x86_64, iOS, tvOS) on a Mac, Windows, web and Android
# from a Linux pool node. `lib` checks the libs only (web and Android build an app as a library); `std` is a tier-3
# target: nightly with -Z build-std=std. Compiler warnings fail the shard (devhub reads them from the short format).
# Exit 75: this box cannot check the target (no std for it); the shard goes to another box.
# The CI box's flags, as tools/ci ran them: RUSTFLAGS empty (the workspace config's target-cpu=native is the host's
# CPU, which another target does not have) and MAKEPAD_BUILD " " (stable) or "lines" (nightly).
set -u
target=$1; mode=$2; shift 2
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0 RUSTFLAGS= CARGO_TERM_COLOR=never
# a Linux pool node checks the cross targets with its own toolchain (tools/run/fixture/install_xtool.sh)
if [ -x "$HOME/devhub/xtool/cargo/bin/rustc" ] && [ "$(uname -s)" = Linux ]; then
  export RUSTUP_HOME=$HOME/devhub/xtool/rustup CARGO_HOME=$HOME/devhub/xtool/cargo PATH=$HOME/devhub/xtool/cargo/bin:$PATH
fi
# a box without the target's std cannot check it: exit 75 sends the shard to another box (devhub ci)
sysroot=$(rustc --print sysroot 2>/dev/null)
if [ "$mode" = std ]; then
  rustup toolchain list 2>/dev/null | grep -q '^nightly' || { echo "ci_xcheck: no nightly toolchain here for $target (build-std)"; exit 75; }
elif [ ! -d "$sysroot/lib/rustlib/$target/lib" ]; then
  echo "ci_xcheck: $target is not installed here (tools/run/fixture/install_xtool.sh)"; exit 75
fi
pargs=()
for p in "$@"; do pargs+=(-p "$p"); done
log=$(mktemp "${TMPDIR:-/tmp}/ci_xcheck.XXXXXX") || exit 2
trap 'rm -f -- "$log"' EXIT
case $mode in
  bin) MAKEPAD_BUILD=" " nice -n 10 cargo check --release --target "$target" "${pargs[@]}" --message-format short 2>&1 | tee "$log" ;;
  lib) MAKEPAD_BUILD=" " nice -n 10 cargo check --release --target "$target" --lib "${pargs[@]}" --message-format short 2>&1 | tee "$log" ;;
  std) MAKEPAD_BUILD=lines nice -n 10 cargo +nightly check --release --target "$target" -Z build-std=std "${pargs[@]}" --message-format short 2>&1 | tee "$log" ;;
  *) echo "ci_xcheck.sh: mode is bin, lib or std, not $mode"; exit 2 ;;
esac
rc=${PIPESTATUS[0]}
# std's own dependencies (build-std) are not in this box's offline registry: another box has them
if [ "$rc" != 0 ] && grep -q 'attempting to make an HTTP request, but --offline was specified' "$log"; then
  echo "ci_xcheck: a dependency of $target is not in this box's offline registry"; exit 75
fi
exit "$rc"
