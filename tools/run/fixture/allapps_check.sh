#!/bin/bash
# allapps_check.sh: `makepad check` of every app/example/wm package, from the tree root on a node (under a devhub
# lease: `makepad devhub lease long --rev work --lane allapps -- bash tools/run/fixture/allapps_check.sh`). Builds the
# loader, then checks every package; prints only packages with errors, then "allapps done".
T=${CARGO_TARGET_DIR:-$PWD/target}
B=$T/release/makepad
export MAKEPAD_RUN_STD=$PWD/libs/relax/compiler/std
CARGO_INCREMENTAL=0 cargo build --release -p makepad-run --bin makepad -j8 > ${TMPDIR:-/tmp}/allapps-build.log 2>&1 || { tail -20 ${TMPDIR:-/tmp}/allapps-build.log; echo "BUILD FAILED"; exit 3; }
for toml in apps/*/Cargo.toml apps/*/*/Cargo.toml examples/*/Cargo.toml wm/Cargo.toml wm/*/Cargo.toml; do
  [ -f "$toml" ] || continue
  case "$toml" in apps/commercial/*) continue;; esac
  name=$(grep -m1 '^name *=' "$toml" | sed 's/.*= *"\(.*\)".*/\1/')
  [ -n "$name" ] || continue
  out=$($B check "$name" 2>&1 | tail -40)
  case "$out" in *"is not a member of the workspace"*) continue;; esac
  n=$(printf '%s\n' "$out" | grep -E ' errors$' | grep -oE '[0-9]+ errors$' | grep -oE '^[0-9]+')
  if [ -z "$n" ]; then echo "?? $name (no summary line)"; elif [ "$n" != "0" ]; then echo "ERR $n $name"; printf '%s\n' "$out" | grep -E 'E[0-9]{4}|error' | head -3 | sed 's/^/    /'; fi
done
echo "allapps done"
