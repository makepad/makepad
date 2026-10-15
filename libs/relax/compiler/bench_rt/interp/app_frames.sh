#!/bin/bash
# app_frames.sh <makepad binary> <package> <script> <mode: jit|interp|nocodegen> <out file>
# Hidden, muted, offscreen run of an app under makepad run with a HAL automation script; logs each
# frame's Cx build time (MAKEPAD_HAL_FRAME_LOG, research patch in metal/frame.rs).
set -u
M=$1; P=$2; S=$3; MODE=$4; OUT=$5
export MAKEPAD_HOME=${MAKEPAD_HOME:-/private/tmp/claude-501/-Users-admin-makepad/19c9664e-e6df-4018-800b-33e980182889/scratchpad/mphome}
export MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_HAL_BACKEND=offscreen SANDBOX_MUTE=1 MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_O3=0 MAKEPAD_RUN_O1=0 MAKEPAD_HAL_SCRIPT=$S MAKEPAD_HAL_FRAME_LOG=1 MAKEPAD_RUN_INTERP_STATS=1
if [ "$MODE" = interp ]; then export MAKEPAD_RUN_INTERP=all; fi
# nocodegen: no machine code at all (precompiled glue, every function interpreted, cache off)
if [ "$MODE" = nocodegen ]; then export MAKEPAD_RUN_NO_CODEGEN=1; fi
cd /Users/admin/wt.noindex/ios-interp && /usr/bin/time -l nice -n 5 "$M" run --quiet "$P" > "$OUT" 2>&1
echo "rc=$?"
