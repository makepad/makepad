#!/bin/bash
# usage: run.sh <makepad binary>: a stale makepad (a provided crate built into it differs from the
# program's copy: "The JIT itself changed") never ends a run with 0 having run nothing. With nobody
# to take the update offer (no run window, a hidden one, a run that is no live session: a
# command-line program, a test's child) the update is applied before the program runs (makepad
# builds again and becomes the rebuilt binary), or the run says why it cannot and fails.
# The program is relax-provided-fixture's; its copy of the crate has one fn more than the one built
# into the binary.
#  1. the binary outside a cargo target dir (as devhub's boxes run theirs): it cannot rebuild
#     itself; for `run --no-live` (a test's child) and a hidden live `run` (`makepad test` binds no
#     provided crate: its crates are built from their source)
#  2. in a cargo target dir, the rebuild's cargo ($CARGO, this fixture's) fails
#  3. it builds nothing new (the binary stays as it is)
#  4. it writes a new binary that still differs from the program's copy: the run becomes that
#     binary once, which fails saying so (never a rebuild loop)
# Prints "stale_jit_boot: ok" or "stale_jit_boot: FAILED (...)"; exit 0 / 1.
set -u
H=$(cd "$(dirname "$0")" && pwd)
R=$(cd "$H/../../../../.." && pwd)
BIN=$1
T=$(mktemp -d "${TMPDIR:-/tmp}/stale_jit_boot.XXXXXX") || exit 1
: "${T:?}"
trap 'rm -rf "$T"' EXIT
unset MAKEPAD_RUN_STD MAKEPAD_RUN_LOADER
mkdir -p "$T/ws/fixture" "$T/home" "$T/tmp"
cp -R "$R/tools/run/fixture/Cargo.toml" "$R/tools/run/fixture/src" "$R/tools/run/fixture/tests" "$T/ws/fixture/"
cat > "$T/ws/Cargo.toml" <<EOF
[workspace]
members = ["fixture"]
resolver = "2"

[workspace.dependencies]
makepad-relax-api = { path = "$R/libs/relax_api" }
EOF
printf '\npub fn stale_jit_boot_marker() {}\n' >> "$T/ws/fixture/src/lib.rs"
fails=""
# check <case> <phrase> <binary> <makepad args..>: the run fails, naming the change (the copy's
# lib.rs and nothing else) and $2
check() {
    local case=$1 phrase=$2 exe=$3
    shift 3
    out=$(cd "$T/ws" && MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_HOME="$T/home" TMPDIR="$T/tmp" perl -e 'alarm 300; exec @ARGV' "$exe" "$@" 2>&1)
    rc=$?
    if [ $rc = 0 ] || [ $rc = 142 ] || ! echo "$out" | grep -qE "The JIT itself changed \(relax_provided_fixture: [^,)]*/ws/fixture/src/lib\.rs\)" || ! echo "$out" | grep -q "$phrase"; then
        fails="$fails $case:rc=$rc"
        echo "---- $case (rc $rc, 142: the alarm, a hang)"
        echo "$out" | tail -8
    else
        echo "$case: $(echo "$out" | grep 'did not run' | tail -1 | cut -c1-260)"
    fi
}
RUN="run --no-live relax-provided-fixture --bin relax-provided-fixture"

# 1. outside a cargo target dir
mkdir -p "$T/box"
cp "$BIN" "$T/box/makepad" || exit 1
check box-run "cannot rebuild itself" "$T/box/makepad" $RUN
MAKEPAD_HIDE_WINDOWS=1 check box-hidden-live "cannot rebuild itself" "$T/box/makepad" run relax-provided-fixture --bin relax-provided-fixture

# 2-4. in a cargo target dir, with this fixture's cargo
M="$T/mac/target/host/makepad"
mkdir -p "$T/mac/target/host/.fingerprint"
touch "$T/mac/target/CACHEDIR.TAG"
cp "$BIN" "$M" || exit 1
cat > "$T/cargo" <<EOF
#!/bin/bash
case "\$(cat "$T/cargo_mode")" in
    fail) echo "error[E0599]: stale_jit_boot's failing rebuild" >&2; exit 101 ;;
    new) cp "$M" "$M.new" && mv -f "$M.new" "$M" ;;
esac
exit 0
EOF
chmod +x "$T/cargo"
export CARGO="$T/cargo"
echo fail > "$T/cargo_mode"
check rebuild-fails "rebuilding makepad FAILED" "$M" $RUN
echo noop > "$T/cargo_mode"
check rebuild-noop "changed nothing" "$M" $RUN
echo new > "$T/cargo_mode"
check rebuild-still-stale "differs from the program's copy too" "$M" $RUN
echo "$out" | grep -q "the program runs on it" || fails="$fails rebuild-still-stale:no-exec"

if [ -z "$fails" ]; then
    echo "stale_jit_boot: ok"
else
    echo "stale_jit_boot: FAILED ($fails )"
    exit 1
fi
