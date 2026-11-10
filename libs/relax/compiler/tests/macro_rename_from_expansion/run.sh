#!/bin/bash
# usage: run.sh <makepad binary>: a macro an expansion defines under an internal `#[macro_export]`
# name and renames beside it (`pub use internal as name;`), called under the public name in files
# the load reads before the expansion's (icu_locale_core: `subtag!` in extensions/*/value.rs,
# `pub mod extensions;` being declared before `#[macro_use] pub mod subtags;`). Neither the macro
# nor the import is in the raw scan of the crate's files; its outline has both. In a dependency and
# in the crate being run; uncached, then cached: cold, warm, and warm after an edit of a calling
# file. Prints "macro_rename_from_expansion: ok" or "macro_rename_from_expansion: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
WANT="renamed macros 34 true true | 6 30"
ok=1
run() { # run <label> <expected> [env..]
  local label=$1 want=$2; shift 2
  OUT=$(cd "$T/x" && env "$@" MAKEPAD_RUN_STD=$H/../../std MAKEPAD_HOME=$T/home MAKEPAD_RUN_HOME=$T MAKEPAD_RUN_CACHE_DIR=$T/cache MAKEPAD_RUN_TARGET_DIR=$T/target "$R" run --quiet mrx-app 2>>"$T/err")
  echo "$label: $OUT"
  [ "$OUT" = "$want" ] || ok=0
}
R=$1
mkdir -p "$T/x" && cp -R "$H/Cargo.toml" "$H/app" "$H/lib" "$T/x/"
run "uncached" "$WANT" MAKEPAD_RUN_CACHE=off
run "cold" "$WANT"
run "warm" "$WANT"
sed -i.bak 's/"yes"/"yep"/' "$T/x/lib/src/extensions/unicode.rs" "$T/x/app/src/main.rs"
sed -i.bak 's/triple!(2)/triple!(4)/' "$T/x/app/src/early.rs"
run "warm, callers edited" "${WANT/| 6/| 12}"
[ $ok = 1 ] || tail -5 "$T/err"
rm -rf "$T"
if [ $ok = 1 ]; then echo "macro_rename_from_expansion: ok"; else echo "macro_rename_from_expansion: FAILED"; exit 1; fi
