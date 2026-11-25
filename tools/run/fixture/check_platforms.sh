#!/bin/bash
# check_platforms.sh [pkg...]: `cargo check --release` of a few small packages for every target we ship, in ONE cargo
# invocation (cargo checks the targets in parallel; no linking, no running). Run from the root of the tree under test
# on the Mac mini, which has every target's std (land.sh, blocking, next to the x64 gate:
# `makepad devhub lease build --node mini --lane landplat --rev <branch>,commercial=<branch> -- bash
# tools/run/fixture/check_platforms.sh`). Any lane before hand-in, from its worktree:
# `makepad devhub run <lane> build --node mini -- bash tools/run/fixture/check_platforms.sh`.
#   default packages: counter (the smallest app: platform + draw + widgets), calculator, clock (network + AI hub client),
#   xr-stream (the XR stream's serving side), score-live (the Score composer: audio synth + score player)
#   targets: CHECK_TARGETS, default every shipped one whose std is installed here (a missing one is reported, not failed)
#   a package whose `[package.metadata.makepad] platforms` leaves a target's platform out is not checked for it (as in CI)
# Errors and compiler warnings fail it.
# From a lane worktree use check_platforms_all.sh: it runs the Apple targets on the mini and the rest on the Linux pool, in parallel.
# Prints "PLATFORMS-OK <targets> (<s> s)" or "PLATFORMS-FAIL: <target: first error>..."; exit 0 / 1.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
# a Linux pool node checks the cross targets with its own toolchain (tools/run/fixture/install_xtool.sh), not the system rust
if [ -x "$HOME/devhub/xtool/cargo/bin/rustc" ] && [ "$(uname -s)" = Linux ]; then
  export RUSTUP_HOME=$HOME/devhub/xtool/rustup CARGO_HOME=$HOME/devhub/xtool/cargo PATH=$HOME/devhub/xtool/cargo/bin:$PATH
fi
pkgs=${*:-makepad-example-counter makepad-app-calculator makepad-app-clock makepad-xr-stream makepad-score-live}
want=${CHECK_TARGETS:-aarch64-apple-darwin x86_64-unknown-linux-gnu x86_64-pc-windows-msvc wasm32-unknown-unknown aarch64-linux-android aarch64-apple-ios}
log=$(mktemp -d "${TMPDIR:-/tmp}/checkplat.XXXXXX") || exit 2
trap 'rm -rf -- "$log"' EXIT
sysroot=$(rustc --print sysroot)
targets=""; args=""; missing=""
for t in $want; do
  if [ -d "$sysroot/lib/rustlib/$t/lib" ]; then targets="$targets $t"; args="$args --target $t"; else missing="$missing $t"; fi
done
[ -n "$missing" ] && echo "not installed here (skipped):$missing"
# targets asked for by name must be there: a node without them is a failed check, not a pass
if [ -n "${CHECK_TARGETS:-}" ] && [ -n "$missing" ]; then echo "PLATFORMS-FAIL: targets not installed on this node:$missing (tools/run/fixture/install_xtool.sh)"; exit 1; fi
# a package's `[package.metadata.makepad] platforms` (desktop, web, mobile; CI reads the same list,
# libs/devhub/src/orch/ci.rs declared_platforms): it is not checked for a target of a platform it does not list. No
# list, or one with a word that names no platform, checks it for every target.
platform_of() { case $1 in wasm32-*) echo web ;; *-android|*-androideabi|*-ios|*-ios-sim|*-tvos|*-tvos-sim) echo mobile ;; *) echo desktop ;; esac; }
declared() { # declared <package>: the platforms its manifest lists, nothing for all
  for f in $(git ls-files '*Cargo.toml' | xargs grep -l -x "name = \"$1\"" 2>/dev/null); do
    [ "$(sed -n 's/^name = "\(.*\)"$/\1/p' "$f" | head -1)" = "$1" ] || continue
    words=$(sed -n '/^\[package\.metadata\.makepad\]/,/^\[/s/^platforms *= *\[\(.*\)\].*/\1/p' "$f" | tr -d '"' | tr ',' ' ')
    for w in $words; do case $w in desktop|web|mobile) ;; *) return ;; esac; done
    echo $words
    return
  done
}
decl=(); for p in $pkgs; do decl+=("$(declared "$p")"); done
pkgs_for() { # pkgs_for <target>: the -p words of the packages built for its platform
  pl=$(platform_of "$1"); i=0; out=""
  for p in $pkgs; do
    d=${decl[$i]}; i=$((i + 1))
    if [ -z "$d" ] || [[ " $d " == *" $pl "* ]]; then out="$out -p $p"; else echo "$p: not checked for $1 (its platforms: $d)" >&2; fi
  done
  echo "$out"
}
# one cargo invocation per set of packages: the targets that check the same set share it
keys=(); tg=()
for t in $targets; do
  k=$(pkgs_for "$t")
  [ -n "$k" ] || continue
  found=""
  for i in "${!keys[@]}"; do [ "${keys[$i]}" = "$k" ] && { tg[$i]="${tg[$i]} --target $t"; found=1; break; }; done
  [ -n "$found" ] || { keys+=("$k"); tg+=("--target $t"); }
done
t0=$(date +%s)
: > "$log/check"
rc=0
# -j: the mini has 10 cores and is also the arm64 bench box; the lease budget holds it to one build job
for i in "${!keys[@]}"; do
  cargo check --release -j${CARGO_BUILD_JOBS:-8} ${tg[$i]} ${keys[$i]} --message-format short >> "$log/check" 2>&1 || rc=$?
done
secs=$(( $(date +%s) - t0 ))
fail=""
# compiler warnings fail the check too (a validated revision has none on any target it ships)
warn=$(grep -E '^[^ ]+:[0-9]+:[0-9]+: warning' "$log/check" | sort -u)
if [ $rc != 0 ] || [ -n "$warn" ]; then
  grep -E ': error(\[E[0-9]+\])?: |^error(\[E[0-9]+\])?: ' "$log/check" | grep -vE '^error: (could not compile|aborting|[0-9]+ targets? failed)' | sort -u | head -30
  [ -n "$warn" ] && printf '%s\n' "$warn" | head -30
  # cargo does not name the target of an error or a warning: re-check each one alone (the passing units are fresh
  # now, and cargo replays their warnings)
  for t in $targets; do
    k=$(pkgs_for "$t" 2>/dev/null)
    [ -n "$k" ] || continue
    cargo check --release -j${CARGO_BUILD_JOBS:-8} --target $t $k --message-format short > "$log/one" 2>&1; one=$?
    w=$(grep -cE '^[^ ]+:[0-9]+:[0-9]+: warning' "$log/one")
    [ $one = 0 ] && [ "$w" = 0 ] && continue
    if [ $one != 0 ]; then
      fail="$fail $t"
      echo "$t: $(grep -E ': error|^error' "$log/one" | grep -vE '^error: (could not compile|aborting)' | head -1 | cut -c1-200)"
    else
      fail="$fail $t($w warnings)"
      echo "$t: $(grep -E '^[^ ]+:[0-9]+:[0-9]+: warning' "$log/one" | head -1 | cut -c1-200)"
    fi
  done
  [ -z "$fail" ] && fail=" rc$rc($(grep -E '^error' "$log/check" | head -1 | cut -c1-160))"
fi
# whose verdict this is (a lane reading it from a shared file checks them)
who="[lease ${DEVHUB_LEASE:-none} lane ${MAKEPAD_TEST_LANE:-none} at $(git rev-parse --short=10 HEAD 2>/dev/null || echo unknown)]"
if [ -n "$fail" ]; then echo "PLATFORMS-FAIL:$fail ($secs s) $who"; exit 1; fi
echo "PLATFORMS-OK$targets [$pkgs] ($secs s) $who"
