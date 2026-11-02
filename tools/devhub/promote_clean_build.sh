#!/bin/bash
# Builds one exact commit in a clean export, with nothing from the node's own checkout around it.
# Run by `makepad devhub promote` as a job: promote_clean_build.sh <commit>
set -u
sha=$1
dir=/tmp/makepad-promote-clean
mkdir -p "$dir"
git -C "$HOME/makepad" archive "$sha" | tar -x -C "$dir" || exit 2
cd "$dir" || exit 2
CARGO_INCREMENTAL=0 CARGO_TARGET_DIR="$dir/target" nice -n 5 cargo build --release --workspace 2> "$dir/err.log"
code=$?
grep -E '^(warning|error)' "$dir/err.log" | sort | uniq -c | sort -nr | head -20
tail -2 "$dir/err.log"
warnings=$(grep -c '^warning' "$dir/err.log")
echo "warnings: $warnings exit: $code"
cd /
rm -rf /tmp/makepad-promote-clean
[ "$code" = 0 ] && [ "$warnings" = 0 ]
