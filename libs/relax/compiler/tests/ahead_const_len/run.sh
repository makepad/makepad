#!/bin/bash
# usage: run.sh <makepad binary>: n01-land fix 1. Checks the bin root three times (`check
# --root-only`: padlib's padded types are reached only through callees lowered ahead on helper
# threads), then runs it. Every check must exit 0 with 0 errors and no failed constant
# evaluation; the run must print the lanes rustc prints. Before the fix, a NeedsUnit unwind of
# lowering ahead was cached as a constant evaluation error: E0080 and a field table static that
# failed to type-check (in makepad, then read through a null address: a segfault). Prints
# "ahead_const_len: ok" or "ahead_const_len: FAILED ...".
H=$(cd "$(dirname "$0")" && pwd)
R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
T=$(mktemp -d)
export MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_STD="$H/../../std" MAKEPAD_RUN_CACHE_DIR="$T/cache"
bad=()
for k in 1 2 3; do
  (cd "$H" && ulimit -c 0 && "$R" check --root-only --no-serve ahead-const-len) > "$T/check$k.log" 2>&1
  rc=$?
  [ "$rc" -eq 0 ] || bad+=("check $k exit $rc")
  grep -q 'makepad check: .* 0 errors' "$T/check$k.log" || bad+=("check $k: $(grep 'makepad check: .* errors' "$T/check$k.log")")
  grep -Eq 'E0080|\{error\}|segfault|could not evaluate' "$T/check$k.log" && bad+=("check $k: $(grep -Em1 'E0080|\{error\}|segfault|could not evaluate' "$T/check$k.log" | cut -c1-200)")
done
OUT=$(cd "$H" && ulimit -c 0 && "$R" run --quiet ahead-const-len 2>&1 | grep -v '^relax-\|^makepad \(run\|check\|live\|fix\):')
[ "$OUT" = "ahead_const_len lanes 360" ] || bad+=("run: $OUT")
if [ ${#bad[@]} -eq 0 ]; then
  echo "ahead_const_len: ok"
else
  printf 'ahead_const_len: FAILED %s (logs: %s)\n' "$(IFS='; '; echo "${bad[*]}")" "$T"
  exit 1
fi
