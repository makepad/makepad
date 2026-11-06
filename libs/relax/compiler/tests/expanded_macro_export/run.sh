#!/bin/bash
# usage: run.sh <makepad binary>: macros only macro expansions define: a dependency's `#[macro_export]` ones
# by path, by `use`, by glob and through `#[macro_use]`, the crate's own local ones in later modules, and the
# crate root calling one a later `#[macro_use]` module's expansion exports (the load reaches that module after
# the root: the crate's outline has it). Uncached, also with each defining module deferred; then cached: cold,
# warm, warm after an edit of the root, and after an edit of the defining module's expansion (the root sees
# the new definition). Prints "expanded_macro_export: ok" or "expanded_macro_export: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
WANT="expanded macros 42 7 3 42 16 9 | 4 20"
ok=1
run() { # run <label> <expected> [env..]
  local label=$1 want=$2; shift 2
  OUT=$(cd "$T/x" && env "$@" MAKEPAD_RUN_STD=$H/../../std MAKEPAD_HOME=$T/home MAKEPAD_RUN_HOME=$T MAKEPAD_RUN_CACHE_DIR=$T/cache MAKEPAD_RUN_TARGET_DIR=$T/target "$R" run --quiet xmx-app 2>>"$T/err")
  echo "$label: $OUT"
  [ "$OUT" = "$want" ] || ok=0
}
R=$1
mkdir -p "$T/x" && cp -R "$H/Cargo.toml" "$H/app" "$H/lib" "$T/x/"
for defer in "" app_exports.rs app_defs.rs; do
  run "uncached${defer:+, $defer deferred}" "$WANT" MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_LAZY_DEFER=$defer
done
run "cold" "$WANT"
run "warm" "$WANT"
echo "// edited" >> "$T/x/app/src/main.rs"
run "warm, root edited" "$WANT"
sed -i.bak 's/app_file, 4/app_file, 5/' "$T/x/app/src/app_exports.rs"
run "warm, definition edited" "${WANT/| 4/| 5}"
[ $ok = 1 ] || tail -5 "$T/err"
rm -rf "$T"
if [ $ok = 1 ]; then echo "expanded_macro_export: ok"; else echo "expanded_macro_export: FAILED"; exit 1; fi
