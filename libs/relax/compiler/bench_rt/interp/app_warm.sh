#!/bin/bash
# app_warm.sh <makepad binary> <package> <script> <mode: jit|nocodegen> <cache dir> <out prefix>
# Three hidden runs (cold, warm, warm) of an app with its own code cache: first-frame times.
set -u
M=$1; P=$2; S=$3; MODE=$4; C=$5; OUT=$6
export MAKEPAD_HOME=${MAKEPAD_HOME:-/private/tmp/claude-501/-Users-admin-makepad/19c9664e-e6df-4018-800b-33e980182889/scratchpad/mphome}
export MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_HAL_BACKEND=offscreen SANDBOX_MUTE=1 MAKEPAD_RUN_CACHE_DIR=$C MAKEPAD_HAL_SCRIPT=$S MAKEPAD_RUN_INTERP_STATS=1
if [ "$MODE" = nocodegen ]; then export MAKEPAD_RUN_NO_CODEGEN=1; fi
cd /Users/admin/wt.noindex/ios-interp
for r in cold warm1 warm2; do
  nice -n 5 "$M" run --quiet "$P" > "$OUT.$r.txt" 2>&1
  echo "$r rc=$? $(grep -E 'first frame' "$OUT.$r.txt") $(grep -cE '"summary":true,"ok":true' "$OUT.$r.txt") ok"
done
