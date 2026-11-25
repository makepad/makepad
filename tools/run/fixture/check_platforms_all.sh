#!/bin/bash
# check_platforms_all.sh [pkg...]: the platform check of a lane, from its worktree on the Mac, in parallel legs:
#   apple : aarch64-apple-darwin and aarch64-apple-ios, on any free Apple-silicon Mac (one lease each, kind test);
#           MAKEPAD_MAC_NODE=<node> pins both to that Mac, only for a check that needs its files
#   cross : x86_64-unknown-linux-gnu, x86_64-pc-windows-msvc, wasm32-unknown-unknown, aarch64-linux-android on a Linux
#           pool node (kind build; the node's xtool toolchain, tools/run/fixture/install_xtool.sh)
# Prints one "PLATFORMS-OK <targets> (<s> s)" when every leg passed, else every leg's PLATFORMS-FAIL lines; exit 0 / 1.
#   Each leg is its own lease AND its own lane dir (<lane>-<leg>): two ships into one dir would race on the same files; DEVHUB_LANE / DEVHUB_AGENT as usual.
set -u
lane=${DEVHUB_LANE:-$(basename "$PWD")}
M=${MAKEPAD_BIN:-cargo run --profile host -q --manifest-path $HOME/makepad/Cargo.toml -p makepad-run --bin makepad --}
log=$(mktemp -d "${TMPDIR:-/tmp}/platall.XXXXXX") || exit 2
trap 'rm -rf -- "$log"' EXIT
t0=$(date +%s)
leg() { # leg <name> <kind> <node args...> -- <CHECK_TARGETS>
  name=$1; kind=$2; shift 2
  targets=${!#}; set -- "${@:1:$#-1}"
  $M devhub run "$lane-$name" "$kind" "$@" -- env CHECK_TARGETS="$targets" bash tools/run/fixture/check_platforms.sh ${PKGS[@]+"${PKGS[@]}"} > "$log/$name" 2>&1
  echo $? > "$log/$name.rc"
}
PKGS=("$@")
if [ -n "${MAKEPAD_MAC_NODE:-}" ]; then mac=(--node "$MAKEPAD_MAC_NODE"); else mac=(--needs os=macos,arch=aarch64); fi
leg apple_darwin test "${mac[@]}" "aarch64-apple-darwin" &
leg apple_ios test "${mac[@]}" "aarch64-apple-ios" &
leg cross build --needs arch=x64,os=linux "x86_64-unknown-linux-gnu x86_64-pc-windows-msvc wasm32-unknown-unknown aarch64-linux-android" &
wait
fail=0; ok=""
for n in apple_darwin apple_ios cross; do
  if [ "$(cat "$log/$n.rc")" = 0 ]; then ok="$ok $(grep -h '^PLATFORMS-OK' "$log/$n" | sed 's/^PLATFORMS-OK \([^[]*\)\[.*/\1/')"; else
    fail=1; echo "== $n"; grep -E "^PLATFORMS-FAIL|not installed|^[a-z0-9_-]+: |error|devhub" "$log/$n" | head -12
  fi
done
secs=$(( $(date +%s) - t0 ))
if [ $fail = 1 ]; then echo "PLATFORMS-FAIL ($secs s)"; exit 1; fi
echo "PLATFORMS-OK$ok ($secs s)"
