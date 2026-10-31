#!/bin/bash
# usage: run.sh <makepad binary>: boots the two crates with `makepad run --units`; prints the exit code
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
sed "s#@R@#$H#g" $H/units.json.in > $T/units.json
MAKEPAD_RUN_STD=$H/../../std $1 run --units $T/units.json app 2>&1 | grep -v '^makepad-run: [a-z +()0-9]*  *[0-9.]* ms'
echo "exit ${PIPESTATUS[0]}"
