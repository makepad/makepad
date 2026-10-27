#!/bin/bash
# snap.sh <dir> : build the scoreboard binaries of the tree in <dir>/src (on the bench box; untimed).
#   <dir>/jit/release/makepad                      the JIT (`makepad run`), plain release (the campaign baseline)
#   <dir>/ncpu/release/<bin>                       native rustc, -C target-cpu=native (the workspace's host default)
#   <dir>/nship/release/<bin>                      native rustc, shipping (RUSTFLAGS="": the target's default CPU)
# Every native package is its own cargo invocation (feature unification: relax-commercial-tests turns on box3d
# double precision for anything built with it). SB_PKGS overrides the native package list ("pkg[:bin] ...").
# Offline (the box's cargo registry), CARGO_INCREMENTAL=0, nice. Prints SNAP-OK or SNAP-FAIL <what>.
set -u
D=$(cd "$1" && pwd); S=$D/src
cd "$S" || exit 2
export CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true MAKEPAD_GGML_NO_CUDA=1
J=${SB_JOBS:-12}
PKGS=${SB_PKGS:-"relax-rwbench relax-commercial-tests:relax-rwbench-commercial makepad-sandbox makepad-stage makepad-story makepad-scope:scope makepad-app-calculator makepad-app-code makepad-app-amp makepad-example-box3d"}
fail=""
b() { # <target> <rustflags-mode> <cargo args...>
  t=$1; m=$2; shift 2
  if [ "$m" = ship ]; then
    RUSTFLAGS="" CARGO_TARGET_DIR=$D/$t nice -n 10 cargo build --release --offline -j "$J" "$@" > "$D/build.$t.$(echo "$*" | tr ' /' '__').log" 2>&1
  else
    env -u RUSTFLAGS CARGO_TARGET_DIR=$D/$t nice -n 10 cargo build --release --offline -j "$J" "$@" > "$D/build.$t.$(echo "$*" | tr ' /' '__').log" 2>&1
  fi || fail="$fail $t:$*"
}
b jit cpu -p makepad-run --bin makepad
for spec in $PKGS; do
  pkg=${spec%%:*}; bin=${spec#*:}
  a=(-p "$pkg"); [ "$bin" != "$spec" ] && a+=(--bin "$bin")
  b ncpu cpu "${a[@]}"
  b nship ship "${a[@]}"
done
# SB_BINS=1 (the mini: its disk guard evicts target dirs): copy every built binary to <dir>/bins/<variant>/
if [ "${SB_BINS:-0}" = 1 ]; then
  mkdir -p "$D/bins/jit" "$D/bins/ncpu" "$D/bins/nship"
  cp "$D/jit/release/makepad" "$D/bins/jit/" 2>/dev/null
  for spec in $PKGS; do
    pkg=${spec%%:*}; bin=${spec#*:}; [ "$bin" = "$spec" ] && bin=$pkg
    for v in ncpu nship; do cp "$D/$v/release/$bin" "$D/bins/$v/" 2>/dev/null; done
  done
  ls "$D/bins"/*
fi
grep -h "^warning" "$D"/build.*.log | sort | uniq -c | sort -rn | head -5
if [ -n "$fail" ]; then echo "SNAP-FAIL$fail"; exit 1; fi
echo SNAP-OK
