#!/bin/bash
# usage: run.sh <makepad binary>: strict-mode rustc errors Relax must report (`//~ E....` on line 1 of
# each file, optionally `//~ help: <text>` on line 2 that the message must contain; `//~ none` =
# must compile; `//~ msg: <text>` = an error whose message contains <text>, for Relax's own errors
# without a rustc code). Prints one line per file, then "strict_errors: ok" or
# "strict_errors: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
# the binary as an absolute path (each check runs in its own temporary directory)
BIN=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
T=$(mktemp -d)
export MAKEPAD_RUN_CACHE=off
fail=0
for f in "$H"/*.rs; do
  want=$(head -1 "$f" | sed -n 's#^//~ ##p')
  n=$(basename "$f" .rs)
  printf '{"version":1,"roots":[0],"units":[{"pkg_id":"path+file://%s#%s@0.0.0","target":{"kind":["bin"],"name":"%s","src_path":"%s","edition":"2021"},"mode":"build","features":[],"dependencies":[]}]}' "$T" "$n" "$n" "$f" > "$T/$n.json"
  out=$(cd "$T" && MAKEPAD_RUN_STD=$H/../../std MAKEPAD_RUN_CACHE_DIR=$T/target-$n perl -e 'alarm shift; exec @ARGV' 60 "$BIN" check --units "$T/$n.json" "$n" 2>&1)
  if [ "${want#msg: }" != "$want" ]; then
    if echo "$out" | grep -qF -- "${want#msg: }"; then echo "$n: ok"; else echo "$n: FAILED (expected: ${want#msg: })"; fail=1; fi
  elif [ "$want" = none ]; then
    # (a compiler panic -- the background/inline borrow checker's included -- or a body the
    # borrow checker gave up on is never "compiles": makepad check still counts 0 errors then)
    if echo "$out" | grep -q "makepad check: .* 0 errors" && ! echo "$out" | grep -qE "panicked at|internal error in the borrow checker|closure capture not found"; then echo "$n: ok"; else echo "$n: FAILED (expected no errors, no compiler panic)"; fail=1; fi
  else
    help=$(sed -n '2s#^//~ help: ##p' "$f")
    if echo "$out" | grep -qE "$want:|\[$want\]" && { [ -z "$help" ] || echo "$out" | grep -qF -- "$help"; }; then echo "$n: ok"; else echo "$n: FAILED (expected $want${help:+ with: $help})"; fail=1; fi
  fi
done
if [ $fail = 0 ]; then echo "strict_errors: ok"; else echo "strict_errors: FAILED"; exit 1; fi
