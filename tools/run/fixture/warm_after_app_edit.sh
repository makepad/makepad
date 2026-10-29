#!/bin/sh
# An app-only edit ships warm (JITCOLD2): after an edit of the app's own crate, a restart compiles
# the edited fns and what depends on them, not the whole crate (and nothing below it).
#   session cold : a fresh run home: launch hidden + muted to the first present and JIT idle, quit;
#   session warm : the same launch again, nothing edited (the baseline: every pack accepted);
#   then one edit per step, each followed by the same launch on the same run home:
#     body   : a statement into one fn body of the app crate
#     item   : a new pub fn at the end of one app source file (an interface edit: a new item)
#     splash : a number in one script_mod! of the app crate (an item macro's text)
#     graph  : no edit: the program restored from the collected image the step before recorded (an edited
#              start keeps the unchanged crates' collected prefix and collects the rest again) is checked
#              against one rebuilt from the sources (MAKEPAD_RUN_COLLECTED_VERIFY=1)
#   every edited file is restored at the end (also on failure or interrupt).
# A step passes when its session compiles at most MAX (default 400) fns first and the JIT spinner
# never shows. Before the fix every non-body edit renamed the app crate's interface fingerprint, which
# keys every fn of the crate: the whole app recompiled ("cache cold", ~15k first compiles on Sandbox).
# Run from the workspace root after the loader is built. PACKAGE (default makepad-sandbox; a
# `makepad run` argument list with ',' for spaces), APPDIR (the app's source dir, default
# apps/commercial/sandbox/src), BODY / ITEM / SPLASH (file:line-pattern of the three edits), WAIT
# (seconds, default 400), TARGET (default $CARGO_TARGET_DIR/release or target/release), STEPS (default "body item splash";
# another step name <s> takes its edits from STEP_<s>: ';;'-separated <after|before|replace|append>|<file>|<exact line>|<new line>).
# OUT: where its run dir goes (default $TMPDIR); KEEP=1 keeps it; TRACE=1 writes each session's
# cache lookups (MAKEPAD_RUN_CACHE_TRACE) there.
# Prints "warm_after_app_edit: OK ..." or "warm_after_app_edit: FAIL ..."; exit 1 on failure.
T=${TARGET:-${CARGO_TARGET_DIR:-target}/release}
B=$(cd "$(dirname "$T/makepad")" && pwd)/makepad
PACKAGE=${PACKAGE:-makepad-sandbox}
APPDIR=${APPDIR:-apps/commercial/sandbox/src}
WAIT=${WAIT:-400}
MAX=${MAX:-400}
STEPS=${STEPS:-body item splash}
# the edits: <file>|<a line of it, exact>|<the line to put after it>
[ -n "${BODY:-}" ] || BODY="$APPDIR/haptics.rs|    pub fn start_test(&mut self) {|        let _warm_after_app_edit = 1u32;"
[ -n "${ITEM:-}" ] || ITEM="$APPDIR/haptics.rs"
[ -n "${SPLASH:-}" ] || SPLASH="$APPDIR/voice_panel.rs|        spacing: 3|        spacing: 4"
mkdir -p "${OUT:-${TMPDIR:-/tmp}}"; dir=$(mktemp -d "${OUT:-${TMPDIR:-/tmp}}/warm_after_app_edit.XXXXXX") || exit 1
mkdir -p "$dir/home" "$dir/t" "$dir/orig"
saved=""
save() { # save <file>: keep its original once
    k=$(echo "$1" | tr '/' '_')
    [ -f "$dir/orig/$k" ] || { cp "$1" "$dir/orig/$k" || return 1; saved="$saved $1"; }
}
restore() {
    for f in $saved; do
        k=$(echo "$f" | tr '/' '_')
        cmp -s "$dir/orig/$k" "$f" || { cp "$dir/orig/$k" "$f.restore" && mv "$f.restore" "$f"; }
    done
}
trap 'restore; exit 1' INT TERM
session() { # session <label>: one launch to idle and quit; prints its first-frame line and its `jit` session line
    label=$1
    trace=""; [ -n "${TRACE:-}" ] && trace="MAKEPAD_RUN_CACHE_TRACE=$dir/trace.$label MAKEPAD_RUN_DEPS_DUMP=$dir/deps.$label"
    [ "$label" = verify ] && trace="$trace MAKEPAD_RUN_CACHE=verify"
    [ "$label" = graph ] && trace="$trace MAKEPAD_RUN_COLLECTED_VERIFY=1"
    env MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_REMOTE=1 TMPDIR="$dir/t" \
        MAKEPAD_HOME="$dir/home" SANDBOX_HOME="$dir/home" MAKEPAD_RUN_HOME="$dir/run" MAKEPAD_RUN_CACHE_DIR="$dir/run/cache" \
        MAKEPAD_RUN_TARGET_DIR="$dir/run/target" MAKEPAD_RUN_CACHE_IFACE="$dir/iface.$label" $trace \
        MAKEPAD_RUN_VERBOSE=1 "$B" run --report-to "$dir/report.$label" $(echo "$PACKAGE" | tr ',' ' ') > "$dir/log.$label" 2>&1 &
    pid=$!
    i=0
    while [ $i -lt $((WAIT * 5)) ] && kill -0 $pid 2>/dev/null && ! grep -qx present "$dir/report.$label" 2>/dev/null; do
        sleep 0.2; i=$((i + 1))
    done
    idle=0; i=0; last=""
    while [ $idle -lt 3 ] && [ $i -lt $WAIT ] && kill -0 $pid 2>/dev/null; do
        last=$(TMPDIR="$dir/t" "$B" inspect $pid jit ms=1000 2>&1)
        if echo "$last" | grep -q '^idle'; then idle=$((idle + 1)); else idle=0; fi
        i=$((i + 1))
    done
    kill -TERM $pid 2>/dev/null
    wait $pid
    grep -m1 ' first frame at ' "$dir/log.$label" | sed 's/^.* first frame/first frame/'
    echo "$last" | grep '^session:' || echo "no jit session line: $(echo "$last" | head -3 | tr '\n' ' ')"
    echo "$last" > "$dir/jit.$label"
    [ "$label" = verify ] && grep -m1 'cache: verify:' "$dir/log.$label"
    [ "$label" = graph ] && { grep -m1 -e 'collected: verified restored graph' -e 'collected: verification requires' -e 'panicked' "$dir/log.$label" || echo "graph: no verification line"; }
    # the crates whose interface fingerprint moved since the session before
    if [ -n "${prev:-}" ] && [ -f "$dir/iface.$prev" ] && [ -f "$dir/iface.$label" ]; then
        grep '^crate' "$dir/iface.$prev" | sort > "$dir/a"; grep '^crate' "$dir/iface.$label" | sort > "$dir/b"
        moved=$(comm -13 "$dir/a" "$dir/b" | cut -f2 | tr '\n' ' ')
        echo "interfaces moved: ${moved:-none}"
    fi
    prev=$label
}
edit_after() { # edit_after <file> <line> <new line>: put <new line> after the first exact <line>
    save "$1" || return 1
    grep -qxF "$2" "$1" || { echo "warm_after_app_edit: FAIL: $1 has no line '$2'"; return 1; }
    awk -v a="$2" -v b="$3" '{ print } !done && $0 == a { print b; done = 1 }' "$1" > "$1.probe" && mv "$1.probe" "$1"
}
edit_before() { # edit_before <file> <line> <new line>: put <new line> before the first exact <line>
    save "$1" || return 1
    grep -qxF "$2" "$1" || { echo "warm_after_app_edit: FAIL: $1 has no line '$2'"; return 1; }
    awk -v a="$2" -v b="$3" '!done && $0 == a { print b; done = 1 } { print }' "$1" > "$1.probe" && mv "$1.probe" "$1"
}
edit_replace() { # edit_replace <file> <line> <new line>: the first exact <line> becomes <new line>
    save "$1" || return 1
    grep -qxF "$2" "$1" || { echo "warm_after_app_edit: FAIL: $1 has no line '$2'"; return 1; }
    awk -v a="$2" -v b="$3" '!done && $0 == a { print b; done = 1; next } { print }' "$1" > "$1.probe" && mv "$1.probe" "$1"
}
fail=""; out=""
check() { # check <label> <session output>
    n=$(echo "$2" | sed -n 's/^session: compiled \([0-9]*\) first.*/\1/p')
    spin=$(echo "$2" | sed -n 's/.*spinner \([0-9]*\).*/\1/p')
    out="$out
[$1] $(echo "$2" | tr '\n' ' ')"
    if [ "$1" = graph ]; then
        echo "$2" | grep -q 'collected: verified restored graph against rebuilt graph' || fail="$fail graph"
        return
    fi
    if [ -z "$n" ] || [ "$n" -gt "$MAX" ] || [ "$spin" != 0 ]; then fail="$fail $1"; fi
}
prev=""
cold=$(session cold)
out="[cold] $(echo "$cold" | tr '\n' ' ')"
s=$(session warm); check warm "$s"
for step in $STEPS; do
    case $step in
    verify|graph) ;;  # no edit: verify compiles every fn it would link and compares (MAKEPAD_RUN_CACHE=verify); graph above
    body) f=${BODY%%|*}; r=${BODY#*|}; edit_after "$f" "${r%%|*}" "${r#*|}" || { restore; exit 1; };;
    item) save "$ITEM" || { restore; exit 1; }; printf '\npub fn warm_after_app_edit_probe() -> u32 { 7 }\n' >> "$ITEM";;
    splash) f=${SPLASH%%|*}; r=${SPLASH#*|}; edit_replace "$f" "${r%%|*}" "${r#*|}" || { restore; exit 1; };;
    *)  # STEP_<name>: edits ';;'-separated, each <after|before|replace|append>|<file>|<line of it, exact>|<new line>
        spec=$(printenv "STEP_$step") || { echo "warm_after_app_edit: FAIL: unknown step $step (no STEP_$step)"; restore; exit 1; }
        echo "$spec" | awk 'BEGIN { RS = ";;" } NF { sub(/\n$/, ""); print }' > "$dir/spec.$step"
        while IFS='|' read -r mode f a b; do
            case $mode in
            after) edit_after "$f" "$a" "$b" || { restore; exit 1; };;
            before) edit_before "$f" "$a" "$b" || { restore; exit 1; };;
            replace) edit_replace "$f" "$a" "$b" || { restore; exit 1; };;
            append) save "$f" || { restore; exit 1; }; printf '\n%s\n' "$a" >> "$f";;
            *) echo "warm_after_app_edit: FAIL: step $step: unknown edit '$mode'"; restore; exit 1;;
            esac
        done < "$dir/spec.$step";;
    esac
    s=$(session "$step"); check "$step" "$s"
done
restore
if [ -z "$fail" ]; then
    echo "warm_after_app_edit: OK$out" | tr '\n' ' '; echo
    [ "${KEEP:-0}" != 1 ] && rm -rf "$dir"
    exit 0
fi
echo "warm_after_app_edit: FAIL [$fail ] (MAX $MAX first compiles)$out" | tr '\n' ' '; echo " logs in $dir"
exit 1
