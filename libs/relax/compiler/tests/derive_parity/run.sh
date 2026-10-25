#!/bin/bash
# usage: run.sh <makepad binary>: `makepad check` agrees with rustc on derive(Script) field parsing.
# - dp-reject: the derive rejects a fn-pointer field ("Unexpected field form"); rustc reports that
#   and E0599 `script_new` (2 errors); makepad check must report the same two and nothing more.
# - dp-paths: path field types (crate::a::B, ::std::.., self::); rustc accepts; check must be clean.
#   (10-25: work's makepad check passed it while rustc failed it -- the parser read path segments only
#   in relax's linked copy; relax-native-macros' build now refuses such cfgs, native_cfg.rs.)
# Prints "derive_parity: ok" or "derive_parity: FAILED".
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_STD=$H/../../std
ok=1
R=$(cd "$H" && "$1" check dp-reject 2>&1); rc=$?
E=$(echo "$R" | grep -E '^/.*:[0-9]+:[0-9]+: (E[0-9]{4}|error|internal)' | sed "s|^$H/||")
WANT="reject/src/main.rs:6:1: error: Unexpected field form
reject/src/main.rs:17:13: E0599: no function or associated item named \`script_new\` found for struct \`Rejected\` in the current scope"
if [ $rc -eq 0 ] || [ "$E" != "$WANT" ]; then echo "dp-reject: rc=$rc"; echo "$E"; ok=0; fi
P=$(cd "$H" && "$1" check dp-paths 2>&1); rc=$?
if [ $rc -ne 0 ] || ! echo "$P" | grep -q ", 0 errors$"; then echo "dp-paths: rc=$rc"; echo "$P" | tail -8; ok=0; fi
if [ $ok = 1 ]; then echo "derive_parity: ok"; else echo "derive_parity: FAILED"; exit 1; fi
