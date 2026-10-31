#!/bin/bash
# allapps_check.sh: `makepad check` of every app/example/wm package, from the tree root on a node (under a devhub
# lease: `makepad devhub lease long --rev work --lane allapps -- bash tools/run/fixture/allapps_check.sh`). Builds the
# loader, then checks every package; prints only packages with errors, then "allapps done".
T=${CARGO_TARGET_DIR:-$PWD/target}
# the JIT through cargo: rebuilt when its sources changed, a ~0.2 s no-op otherwise (no binary path)
J="cargo run --profile host -q --manifest-path $PWD/Cargo.toml -p makepad-run --bin makepad --"
B=$J
export MAKEPAD_RUN_STD=$PWD/libs/relax/compiler/std
CARGO_INCREMENTAL=0 cargo build --profile host -p makepad-run --bin makepad -j8 > ${TMPDIR:-/tmp}/allapps-build.log 2>&1 || { tail -20 ${TMPDIR:-/tmp}/allapps-build.log; echo "BUILD FAILED"; exit 3; }
for toml in apps/*/Cargo.toml apps/*/*/Cargo.toml examples/*/Cargo.toml wm/Cargo.toml wm/*/Cargo.toml; do
  [ -f "$toml" ] || continue
  case "$toml" in apps/commercial/*) continue;; esac
  name=$(grep -m1 '^name *=' "$toml" | sed 's/.*= *"\(.*\)".*/\1/')
  [ -n "$name" ] || continue
  [ "$name" = makepad-app-wm-all ] && continue  # not tested (user 2026-10-30)
  out=$($B check "$name" 2>&1 | tail -40)
  case "$out" in *"is not a member of the workspace"*) continue;; esac
  n=$(printf '%s\n' "$out" | grep -E ' errors$' | grep -oE '[0-9]+ errors$' | grep -oE '^[0-9]+')
  if [ -z "$n" ]; then echo "?? $name (no summary line)"; elif [ "$n" != "0" ]; then echo "ERR $n $name"; printf '%s\n' "$out" | grep -E 'E[0-9]{4}|error' | head -3 | sed 's/^/    /'; fi
done
# the smoke apps' and the WM's provided-crate boundary (`makepad check --boundary all`, provided_sweep.sh audits only)
RUNS=0 SWEEP_OUT=${TMPDIR:-/tmp}/allapps-sweep bash tools/run/fixture/provided_sweep.sh 2>&1 | grep -E "^audit |^provided_sweep|^violation" | head -40
echo "allapps done"
