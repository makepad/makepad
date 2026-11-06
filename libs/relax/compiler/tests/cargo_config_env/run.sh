#!/bin/bash
# usage: run.sh <makepad binary>: cargo's `[env]` table of .cargo/config.toml (issue 1385) reaches
# env!() and build scripts as cargo hands it to rustc and build scripts: a `relative` value joined
# to the folder holding `.cargo`, `force` over the process environment, a plain value only where the
# process environment lacks it, a build script's rustc-env over [env]. Run with and without the
# variables in the process environment. Prints "cargo_config_env: ok" or "cargo_config_env: FAILED"
# (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
export MAKEPAD_RUN_CACHE=off
run() { (cd "$H" && env "$@" MAKEPAD_RUN_STD=$H/../../std "$MAKEPAD" run --quiet cce-app 2>>"$ERR"); }
MAKEPAD=$1
ERR=$(mktemp)
A=$(run -u CCE_FORCED -u CCE_PLAIN)
B=$(run CCE_FORCED="forced by process" CCE_PLAIN="plain from process")
echo "$A"; echo "$B"
if [ "$A" = "config env hello from data | forced by config | plain from config | script saw plain from config | override from script" ] &&
   [ "$B" = "config env hello from data | forced by config | plain from process | script saw plain from process | override from script" ]; then
  rm -f "$ERR"; echo "cargo_config_env: ok"
else
  tail -5 "$ERR"; rm -f "$ERR"; echo "cargo_config_env: FAILED"; exit 1
fi
