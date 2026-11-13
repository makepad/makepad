#!/bin/bash
# usage: run.sh <makepad binary>: compiles layout_diff.rs with rustc and runs it under Relax, in a plain
# run and in a live session (a live session lays out the program's own types as well); diffs each
set -u
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d) || exit 2
: "${T:?}"
trap 'rm -rf "$T"' EXIT
rustc --edition 2021 -O "$H/layout_diff.rs" -o "$T/real" 2>"$T/rustc.err" || { cat "$T/rustc.err"; exit 2; }
"$T/real" > "$T/real.out"
mkdir -p "$T/u"
cat > "$T/u/units.json" <<J
{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://$T#layout_diff@0.0.0","target":{"kind":["bin"],"name":"layout_diff","src_path":"$H/layout_diff.rs","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}
J
fail=0
for mode in plain live; do
  flags=(--quiet --units "$T/u/units.json")
  [ "$mode" = plain ] && flags+=(--no-live)
  t0=$SECONDS
  RELAX_STD=$H/../../std RELAX_CACHE_DIR=$T/tgt-$mode "$1" run "${flags[@]}" layout_diff > "$T/$mode.out" 2>"$T/$mode.err"
  if diff "$T/real.out" "$T/$mode.out" > "$T/$mode.diff"; then
    echo "layout_diff ($mode run): $(wc -l < "$T/real.out" | tr -d ' ') lines equal ($((SECONDS - t0)) s)"
  else
    echo "layout_diff ($mode run): differs from rustc"
    grep '^[<>]' "$T/$mode.diff" | head -80
    grep -E "error|panic" "$T/$mode.err" | head -5
    fail=1
  fi
done
exit $fail
