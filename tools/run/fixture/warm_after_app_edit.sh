#!/bin/sh
# An app-only edit ships warm (JITCOLD2): after an edit of the app's own crate, a restart compiles
# the edited fns and what depends on them, not the whole crate (and nothing below it).
#   session cold : a fresh run home: launch hidden + muted to the first present and JIT idle, quit;
#   session prime: the same launch again: the app's second start takes paths its first did not
#                  (Sandbox reads back the asset store its first start filled: ~570 instances the cold
#                  session never ran, ~400 of them first compiled in one frame -- the JIT spinner when the
#                  node is busy); it records them, so the steps below measure the cache, not the app's first rerun.
#                  Its compiles are not checked; its boot is: it restores the collected front the cold
#                  session recorded (a boot that parses every crate again starts seconds later);
#   session warm : the same launch again, nothing edited (the baseline: every pack accepted);
#   then one edit per step, each followed by the same launch on the same run home:
#     body   : a statement into one fn body of the app crate
#     item   : a new pub fn at the end of one app source file (an interface edit: a new item)
#     splash : a number in one script_mod! of the app crate (an item macro's text)
#     graph  : no edit: the program restored from the collected image the step before recorded (an edited
#              start keeps the unchanged crates' collected prefix and collects the rest again) is checked
#              against one rebuilt from the sources (RELAX_COLLECTED_VERIFY=1)
#   every edited file is restored at the end (also on failure or interrupt).
# A step passes when its session compiles at most MAX (default 400) fns first and its main thread's
# lazy work in the window after the first present (postframe.rs: the burst sits in the first frame
# after it) stays at most LOADS_MAX (default 1000) cache loads and FIRSTS_MAX (default 50) first
# compiles it ran or waited for; the prime session passes when its boot restored the front. The
# counts, not the UI's blocked ms: a busy box stretches the same work past the spinner's 100 ms (the
# item step at 171 and 182 ms with an owner busy in its cache write and publishes), while more lazy
# work is a regression on any box. On one node, 11-25 and 11-26, 36 sessions of the warm, item and
# graph steps: 4-542 cache loads (the install-ahead races main for the manifest's tail) and 0-21
# first compiles; the blocked ms and the spinner are still printed. Before the fix every non-body
# edit renamed the app crate's interface fingerprint, which keys every fn of the crate: the whole
# app recompiled ("cache cold", ~15k first compiles on Sandbox).
# Run from the workspace root after the loader is built. PACKAGE (default makepad-sandbox; a
# `makepad run` argument list with ',' for spaces), APPDIR (the app's source dir, default
# apps/commercial/sandbox/src), BODY / ITEM / SPLASH (file:line-pattern of the three edits), LOADS_MAX /
# FIRSTS_MAX (the bars above), WAIT
# (seconds, default 400), the JIT runs through `cargo run --profile host`, STEPS (default "body item splash";
# another step name <s> takes its edits from STEP_<s>: ';;'-separated <after|before|replace|append>|<file>|<exact line>|<new line>).
# OUT: where its run dir goes (default $TMPDIR); KEEP=1 keeps it; TRACE=1 writes each session's
# cache lookups (RELAX_CACHE_TRACE) there.
# Prints "warm_after_app_edit: OK ..." or "warm_after_app_edit: FAIL ..."; exit 1 on failure.
ROOT=$(pwd)
# the JIT through cargo: rebuilt when its sources changed, a ~0.2 s no-op otherwise (no binary path)
J="cargo run --profile host -q --manifest-path $ROOT/Cargo.toml -p makepad-run --bin makepad --"
B=$J
PACKAGE=${PACKAGE:-makepad-sandbox}
APPDIR=${APPDIR:-apps/commercial/sandbox/src}
WAIT=${WAIT:-400}
MAX=${MAX:-400}
LOADS_MAX=${LOADS_MAX:-1000}
FIRSTS_MAX=${FIRSTS_MAX:-50}
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
session() { # session <label>: one launch to idle and quit; prints its first-frame line, its `jit` session
    # line and its main thread's lazy work after the present
    label=$1
    trace=""; [ -n "${TRACE:-}" ] && trace="RELAX_CACHE_TRACE=$dir/trace.$label RELAX_DEPS_DUMP=$dir/deps.$label"
    [ "$label" = verify ] && trace="$trace RELAX_CACHE=verify"
    [ "$label" = graph ] && trace="$trace RELAX_COLLECTED_VERIFY=1"
    env MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 MAKEPAD_REMOTE=1 TMPDIR="$dir/t" \
        MAKEPAD_HOME="$dir/home" SANDBOX_HOME="$dir/home" RELAX_HOME="$dir/run" RELAX_CACHE_DIR="$dir/run/cache" \
        RELAX_TARGET_DIR="$dir/run/target" RELAX_CACHE_IFACE="$dir/iface.$label" $trace \
        RELAX_VERBOSE=1 $B run --report-to "$dir/report.$label" $(echo "$PACKAGE" | tr ',' ' ') > "$dir/log.$label" 2>&1 &
    pid=$!
    i=0
    while [ $i -lt $((WAIT * 5)) ] && kill -0 $pid 2>/dev/null && ! grep -qx present "$dir/report.$label" 2>/dev/null; do
        sleep 0.2; i=$((i + 1))
    done
    idle=0; i=0; last=""
    while [ $idle -lt 3 ] && [ $i -lt $WAIT ] && kill -0 $pid 2>/dev/null; do
        last=$(TMPDIR="$dir/t" $B inspect $pid jit ms=1000 2>&1)
        if echo "$last" | grep -q '^idle'; then idle=$((idle + 1)); else idle=0; fi
        i=$((i + 1))
    done
    kill -TERM $pid 2>/dev/null
    wait $pid
    grep -m1 ' first frame at ' "$dir/log.$label" | sed 's/^.* first frame at /first frame at /'
    echo "$last" | grep '^session:' || echo "no jit session line: $(echo "$last" | head -3 | tr '\n' ' ')"
    echo "$last" > "$dir/jit.$label"
    # the window's sums (its first line: the one when it passed, else the run summary's): first
    # compiles are the ones main ran (split, plain, memo) or waited for (inflight)
    awk '/^relax post-present [0-9]+ s: main thread stalled / {
        if (!match($0, /by how: [^)]*\)[^)]*\)[^)]*\)[^)]*\)[^)]*\)[^)]*\)[^)]*\)/)) exit
        k = split(substr($0, RSTART + 8, RLENGTH - 8), part, ", ")
        for (i = 1; i <= k; i++) { split(part[i], w, " "); v[w[1]] = w[2] }
        if (v["cache"] == "" || v["split"] == "" || v["plain"] == "" || v["memo"] == "" || v["inflight"] == "") exit
        split($0, a, "stalled "); split(a[2], b, " ")
        printf "lazy after the present: cache loads %d, first compiles %d (split %d, plain %d, memo %d, inflight %d), %d lazy compiles in all, %s ms\n", v["cache"], v["split"] + v["plain"] + v["memo"] + v["inflight"], v["split"], v["plain"], v["memo"], v["inflight"], b[4], b[1]
        found = 1; exit
    }
    END { if (!found) print "lazy after the present: no window line" }' "$dir/log.$label"
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
    loads=$(echo "$2" | sed -n 's/^lazy after the present: cache loads \([0-9]*\), .*/\1/p')
    firsts=$(echo "$2" | sed -n 's/^lazy after the present: cache loads [0-9]*, first compiles \([0-9]*\) .*/\1/p')
    out="$out
[$1] $(echo "$2" | tr '\n' ' ')"
    if [ "$1" = graph ]; then
        echo "$2" | grep -q 'collected: verified restored graph against rebuilt graph' || fail="$fail graph"
        return
    fi
    if [ -z "$n" ] || [ "$n" -gt "$MAX" ] || [ -z "$loads" ] || [ "$loads" -gt "$LOADS_MAX" ] || [ -z "$firsts" ] || [ "$firsts" -gt "$FIRSTS_MAX" ]; then
        fail="$fail $1"
    fi
}
prev=""
cold=$(session cold)
prime=$(session prime)
# the boot restored the collected front: its restore line comes before the load's (a later one is
# the settle rebuild's)
if awk '/relax: load\+parse/ { exit } /relax collected: restored [0-9]+ crates/ { found = 1 } END { exit !found }' "$dir/log.prime"; then
    front="front restored"
else
    front="front NOT restored at boot"; fail="$fail prime"
fi
out="[cold] $(echo "$cold" | tr '\n' ' ')"
out="$out
[prime, $front; compiles not checked] $(echo "$prime" | tr '\n' ' ')"
s=$(session warm); check warm "$s"
for step in $STEPS; do
    case $step in
    verify|graph) ;;  # no edit: verify compiles every fn it would link and compares (RELAX_CACHE=verify); graph above
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
echo "warm_after_app_edit: FAIL [$fail ] (MAX $MAX first compiles; after the present LOADS_MAX $LOADS_MAX cache loads, FIRSTS_MAX $FIRSTS_MAX first compiles)$out" | tr '\n' ' '; echo " logs in $dir"
exit 1
