#!/bin/sh
# Regression test (T1STUCK): a terminal program whose hot code tiers up to O3 -- relax-provided-
# fixture's relax-font-tier bin, a font crate's ScaledFont<F: Font> metrics (blanket impls, dyn)
# in a layout loop -- run under bare relax on a pseudo-terminal, so the boot bar draws:
# - the program ends on its own (every O3 snapshot finished);
# - no bar frame names a `tier1 locked:` take older than 2 s: the owner's item ends with its take
#   (it stayed on the last take's name for the rest of the run, 10 s and growing);
# - the last bar frame says `running`, not `compiling`, once nothing compiles (a program that
#   never presents a frame read as compiling forever).
# Linux (util-linux `script`). Run from the workspace root after
# `cargo build --release -p relax-runner --bin relax`. TARGET (default target/release).
# Prints one PASS/FAIL line per check and a last line `font_tier: N/M passed`.
T=${TARGET:-target/release}
dir=$(mktemp -d "${TMPDIR:-/tmp}/font_tier.XXXXXX") || exit 1
FONT_TIER_SECS=${SECS:-10} MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 TMPDIR="$dir" \
    MAKEPAD_RUN_HOME="$dir/home" MAKEPAD_RUN_CACHE_DIR="$dir/cache" \
    script -q -e -c "perl -e 'alarm 300; exec @ARGV' $T/relax run relax-provided-fixture --bin relax-font-tier" "$dir/tty.log" > /dev/null 2>&1
rc=$?
# the bar frames, one per line, escape codes off
tr '\r' '\n' < "$dir/tty.log" | sed 's/\x1b\[[0-9;]*[A-Za-z]//g' | grep -v '^$' > "$dir/frames"
pass=0; total=0
ok() { total=$((total + 1)); if [ "$2" = 0 ]; then pass=$((pass + 1)); echo "PASS $1"; else echo "FAIL $1"; fi; }
grep -q 'font_tier: done' "$dir/frames"; ok "the program ends on its own (rc $rc)" $?
# (a frame ends `<item> <age> s · <total> s`)
stale=$(grep 'tier1 locked:' "$dir/frames" | awk 'NF > 5 && $(NF - 3) == "s" && $(NF - 4) + 0 >= 2.0 { print; exit }')
[ -z "$stale" ]; ok "no tier1 take shown for 2 s or more" $?
[ -n "$stale" ] && echo "    $stale"
last=$(grep -E '(compiling|running) ' "$dir/frames" | tail -1)
echo "$last" | grep -q 'running '; ok "the last bar frame says running" $?
[ -n "$VERBOSE" ] && echo "    last: $last"
[ -n "$KEEP" ] && echo "kept $dir" || rm -rf "$dir"
echo "font_tier: $pass/$total passed"
[ $pass = $total ]
