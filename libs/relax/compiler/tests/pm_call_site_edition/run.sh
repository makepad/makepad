#!/bin/bash
# usage: run.sh <makepad binary>: an edition 2021 proc-macro crate used by an edition 2024 app. The tokens a
# macro makes (`Span::call_site()`: parsed strings, new groups) keep 2021's block-tail and `if let` drop
# orders, `Box<[T]>::into_iter` and a `macro_rules!` it writes; tokens carried from the input keep 2024's,
# as rustc gives them. Runs cold, then warm from the images the cold run wrote; prints
# "pm_call_site_edition: ok" or "pm_call_site_edition: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
WANT="block local,temp / temp,local; braces local,temp; if-let else,scrutinee / scrutinee,else; into_iter &u8 / u8; rules local,temp; 4"
ok=1
for run in cold warm; do
  OUT=$(cd "$H" && RELAX_STD=$H/../../std MAKEPAD_HOME=$T/home RELAX_HOME=$T RELAX_CACHE_DIR=$T/cache RELAX_TARGET_DIR=$T/target "$1" run --quiet pmce-app 2>>"$T/err")
  echo "$run: $OUT"
  [ "$OUT" = "$WANT" ] || ok=0
done
[ $ok = 1 ] || tail -5 "$T/err"
rm -rf "$T"
if [ $ok = 1 ]; then echo "pm_call_site_edition: ok"; else echo "pm_call_site_edition: FAILED"; exit 1; fi
