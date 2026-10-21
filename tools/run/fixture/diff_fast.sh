#!/bin/sh
# diff_fast.sh: the Relax-vs-rustc differential net (fast tier, for the landing gate).
# Each program in tools/run/fixture/diff/*.rs (floats/trig, TLS + jump tables, struct copies, closures and
# iterators, async, panics/unwinding, integer overflow) is built once natively with `rustc -O --test` (cached by
# source hash + rustc version) and run under `makepad test` at O1 (MAKEPAD_RUN_O1=always: the baseline code and its quick register allocator, no O3), at the default
# O3=0 tier (O2 pipeline) and at O3
# (MAKEPAD_RUN_O3=1, call threshold 1, rounds with sleeps so the async O3 code is installed for later rounds).
# stdout lines `D ...`, the exit status and a checksum of them must equal the native run's.
# Prints `diff: N/N equal`, or per failing program a minimal diff, and exits 1.
# Usage (workspace root, after `cargo build --release -p makepad-run`): tools/run/fixture/diff_fast.sh
# Env: TARGET (default target/release), DIFF_CACHE (native binaries; default $TMPDIR/diff_fast_cache),
# DIFF_PROGRAMS ("floats tls": only those), DIFF_SLEEP_MS (O3 round gap, default 250), DIFF_JOBS (default 4).
# Each makepad run gets a fresh home/cache dir, hidden and muted, and deleted afterwards.
# Package programs (library-backed, e.g. box3d): every package dir in DIFF_PKG_DIRS (default
# tools/run/fixture/diffpkg, plus apps/commercial/relax_tests when the commercial checkout is there) is
# built once with cargo --release (its default bin is named like the package); `<bin> --list` names its
# programs, each runs natively and as `makepad run <package> -- <program>` at the same three tiers, and
# its D lines and exit status must match. They show as `<package>~<program>`. DIFF_PKG_DIRS=none skips them.
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
T=${TARGET:-$root/target/release}
case $T in /*) ;; *) T=$PWD/$T ;; esac
M=$T/makepad
[ -x "$M" ] || { echo "diff: no $M (cargo build --release -p makepad-run)"; exit 2; }
cache=${DIFF_CACHE:-${TMPDIR:-/tmp}/diff_fast_cache}
jobs=${DIFF_JOBS:-4}
sleepms=${DIFF_SLEEP_MS:-250}
work=$(mktemp -d "${TMPDIR:-/tmp}/diff_fast.XXXXXX") || exit 2
mkdir -p "$cache"
progs=${DIFF_PROGRAMS:-$(cd "$here/diff" && ls *.rs | sed 's/\.rs$//')}
known=${DIFF_KNOWN:-$(cd "$here/known" && ls *.rs | sed 's/\.rs$//')}
rv=$(rustc -V)
export M here root cache work sleepms rv
# one job: native build (cached) + native run, or a tier run. $1 = program, $2 = native|base|o3
job() {
    p=$1; k=$2; src=$here/diff/$p.rs; [ -f "$src" ] || src=$here/known/$p.rs; out=$work/$p.$k
    case $p in *~*) pkgjob "${p%%~*}" "${p#*~}" "$k"; return ;; esac
    case $k in
    native)
        h=$(printf '%s' "$rv" | cat - "$src" | cksum | tr -d " ")
        bin=$cache/$p.$h
        if [ ! -x "$bin" ]; then
            # a program with #![feature(..)] (nightly APIs such as core::intrinsics::simd) builds with
            # RUSTC_BOOTSTRAP=1 on the rustc side only
            boot=; grep -q '^#!\[feature' "$src" && boot=1
            env ${boot:+RUSTC_BOOTSTRAP=1} nice -n 10 rustc --edition 2021 -O --test "$src" -o "$bin.tmp" > "$out.build" 2>&1 || { echo "native build failed: $(head -5 "$out.build")" > "$out.raw"; echo 99 > "$out.rc"; return; }
            mv "$bin.tmp" "$bin"
        fi
        DIFF_SLEEP_MS=0 "$bin" --nocapture --test-threads=1 > "$out.raw" 2> "$out.err"
        echo $? > "$out.rc" ;;
    *)
        d=$work/$p.$k.home; mkdir -p "$d/home" "$d/tmp"
        case $k in
        o1) tier="MAKEPAD_RUN_O1=always MAKEPAD_RUN_O3=0"; ms=0 ;;
        base) tier="MAKEPAD_RUN_O3=0"; ms=0 ;;
        *) tier="MAKEPAD_RUN_O3=1"; ms=$sleepms ;;
        esac
        env $tier MAKEPAD_RUN_O3_CALLS=1 DIFF_SLEEP_MS=$ms \
            MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" \
            MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" \
            nice -n 10 "$M" test "$src" > "$out.raw" 2> "$out.err"
        echo $? > "$out.rc"
        rm -r "$d" ;;
    esac
}
# one package-program job: $1 package, $2 program, $3 native|o1|base|o3 (the native binary is built already)
pkgjob() {
    pkg=$1; prog=$2; k=$3; out=$work/$pkg~$prog.$k
    case $k in
    native)
        DIFF_SLEEP_MS=0 "$pkgtarget/release/$pkg" "$prog" > "$out.raw" 2> "$out.err"
        echo $? > "$out.rc" ;;
    *)
        d=$work/$pkg~$prog.$k.home; mkdir -p "$d/home" "$d/tmp"
        case $k in
        o1) tier="MAKEPAD_RUN_O1=always MAKEPAD_RUN_O3=0"; ms=0 ;;
        base) tier="MAKEPAD_RUN_O3=0"; ms=0 ;;
        *) tier="MAKEPAD_RUN_O3=1"; ms=$sleepms ;;
        esac
        (cd "$root" && env $tier MAKEPAD_RUN_O3_CALLS=1 DIFF_SLEEP_MS=$ms MAKEPAD_HAL_BACKEND=headless \
            MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 SANDBOX_MUTE=1 MUSIC_MUTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" \
            MAKEPAD_RUN_HOME="$d/runhome" MAKEPAD_RUN_CACHE_DIR="$d/cache" \
            nice -n 10 "$M" run --quiet "$pkg" -- "$prog") > "$out.raw" 2> "$out.err"
        echo $? > "$out.rc"
        rm -r "$d" ;;
    esac
}
pkgtarget=${DIFF_PKG_TARGET:-$root/target}
case $pkgtarget in /*) ;; *) pkgtarget=$PWD/$pkgtarget ;; esac
export pkgtarget
if [ "$1" = "--job" ]; then work=$2; job "$3" "$4"; exit 0; fi
# package programs: build each package (one cargo run per package, the workspace target dir), list its programs
pkgdirs=${DIFF_PKG_DIRS:-$here/diffpkg $root/apps/commercial/relax_tests}
[ "$pkgdirs" = none ] && pkgdirs=
pkgprogs=; pkgfail=
for dir in $pkgdirs; do
    [ -f "$dir/Cargo.toml" ] || continue
    pkg=$(sed -n 's/^name = "\(.*\)"$/\1/p' "$dir/Cargo.toml" | head -1)
    if ! (cd "$root" && CARGO_INCREMENTAL=0 CARGO_TARGET_DIR=$pkgtarget nice -n 10 cargo build --release -q -p "$pkg" --bin "$pkg" -j "${DIFF_CARGO_JOBS:-6}") > "$work/$pkg.build" 2>&1; then
        pkgfail="$pkgfail$pkg: native build failed: $(grep -m3 '^error' "$work/$pkg.build")
"; continue
    fi
    for prog in $("$pkgtarget/release/$pkg" --list); do pkgprogs="$pkgprogs $pkg~$prog"; done
done
for p in $progs $known $pkgprogs; do for k in native o1 base o3; do echo "$p $k"; done; done | xargs -P "$jobs" -n 2 "$0" --job "$work"
check() {
    p=$1; fail=
    sed -n 's/^test [A-Za-z0-9_:]* \.\.\. //; /^D /p' "$work/$p.native.raw" > "$work/$p.native.d"
    nrc=$(cat "$work/$p.native.rc" 2>/dev/null); [ -s "$work/$p.native.d" ] || fail="native run produced no D lines (rc $nrc): $(head -3 "$work/$p.native.raw" | cut -c1-200)"
    sum=$(cksum < "$work/$p.native.d")
    for k in o1 base o3; do
        [ -n "$fail" ] && break
        sed -n 's/^test [A-Za-z0-9_:]* \.\.\. //; /^D /p' "$work/$p.$k.raw" > "$work/$p.$k.d"
        rc=$(cat "$work/$p.$k.rc" 2>/dev/null)
        if ! cmp -s "$work/$p.native.d" "$work/$p.$k.d"; then
            fail="$p at tier $k: stdout differs from rustc (cksum native $sum, $k $(cksum < "$work/$p.$k.d"))
$(diff "$work/$p.native.d" "$work/$p.$k.d" | head -8 | cut -c1-240)"
        elif [ "$rc" != "$nrc" ] || { case $p in *~*) false ;; *) ! grep -aq '[1-9][0-9]* passed; 0 failed' "$work/$p.$k.raw" ;; esac; }; then
            fail="$p at tier $k: exit status $rc (native $nrc) or test failed: $(grep -a 'test result\|panicked' "$work/$p.$k.raw" | head -2 | cut -c1-200)"
        fi
    done
}
ok=0; n=0; bad=$pkgfail; kfail=0; kfixed=
[ -n "$pkgfail" ] && n=$(printf '%s' "$pkgfail" | grep -c .)
for p in $progs $pkgprogs; do
    n=$((n + 1)); check "$p"
    if [ -z "$fail" ]; then ok=$((ok + 1)); else bad="$bad$fail
"; fi
done
for p in $known; do
    check "$p"
    if [ -z "$fail" ]; then kfixed="$kfixed $p"; else kfail=$((kfail + 1)); fi
done
[ -n "$kfixed" ] && echo "diff: known-failing programs now pass, git mv them from known/ to diff/:$kfixed"
rm -r "$work"
if [ $ok = $n ]; then echo "diff: $ok/$n equal (known failing: $kfail)"; exit 0; fi
printf '%s' "$bad"
echo "diff: $ok/$n equal (known failing: $kfail)"
exit 1
