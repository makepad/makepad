#!/bin/bash
# usage: run.sh <relax binary>: enum-level non_exhaustive permits integer casts;
# variant-level non_exhaustive forbids them only outside the defining crate.
# Native reference: cargo run --offline --release -p enum-cast-app; cargo test --offline --release.
# cargo check --offline -p enum-cast-app --features reject-variant must fail with E0606.
set -euo pipefail
H=$(cd "$(dirname "$0")" && pwd)
RELAX=$(realpath "$1")
export MAKEPAD_RUN_STD=${MAKEPAD_RUN_STD:-$H/../../std}
export MAKEPAD_RUN_CACHE=off
cd "$H"
"$RELAX" check enum-cast-app
test "$("$RELAX" run --quiet enum-cast-app)" = "non-exhaustive enum casts: ok"
"$RELAX" test enum-cast-app
set +e
ERROR=$("$RELAX" check enum-cast-app --features reject-variant 2>&1)
STATUS=$?
set -e
test "$STATUS" -ne 0
printf '%s\n' "$ERROR" | grep -F 'E0606: casting `VariantOnly` as `u32` is invalid'
echo "non_exhaustive_enum_cast: ok"
