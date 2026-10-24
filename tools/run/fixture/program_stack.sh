#!/bin/sh
# program_stack.sh: JIT program threads run on program stacks (large lazily committed reservations
# with a guard; relax_base::stack::on_program_stack). stack/deep.rs recurses ~600 MiB deep, keeps a
# 256 MiB stack buffer and recurses on a spawned thread; stack/overflow.rs must end with a stack
# overflow report (MAKEPAD_RUN_STACK_GB=1), never hang or corrupt. Prints PROGRAM-STACK-OK or the
# failing case. Usage (workspace root, after cargo build --release -p makepad-run): TARGET=... program_stack.sh
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
M=${TARGET:-$root/target/release}/makepad
export MAKEPAD_RUN_STD=${MAKEPAD_RUN_STD:-$root/libs/relax/compiler/std}
work=$(mktemp -d "${TMPDIR:-/tmp}/program_stack.XXXXXX") || exit 2
run() {
    d=$work/$1; mkdir -p "$d/home" "$d/tmp"
    shift
    env MAKEPAD_HIDE_WINDOWS=1 MAKEPAD_MUTE=1 MAKEPAD_HOME="$d/home" TMPDIR="$d/tmp" MAKEPAD_RUN_HOME="$d/runhome" \
        MAKEPAD_RUN_CACHE_DIR="$d/cache" "$@"
}
fail=0
for tier in "MAKEPAD_RUN_O1=always MAKEPAD_RUN_O3=0" "MAKEPAD_RUN_O3=1 MAKEPAD_RUN_O3_CALLS=1"; do
    run deep env $tier "$M" test "$here/stack/deep.rs" > "$work/deep.out" 2> "$work/deep.err"
    rc=$?
    for want in "D deep 500000" "D buffer 8355840" "D thread 250000"; do
        grep -q "^$want\$" "$work/deep.out" || { echo "program_stack: deep ($tier) rc $rc: missing '$want'"; tail -5 "$work/deep.err"; fail=1; }
    done
done
run overflow env MAKEPAD_RUN_STACK_GB=1 "$M" test "$here/stack/overflow.rs" > "$work/ovf.out" 2> "$work/ovf.err"
rc=$?
if [ $rc = 0 ] || ! grep -q "stack overflow" "$work/ovf.err" "$work/ovf.out"; then
    echo "program_stack: overflow rc $rc without a stack overflow report"; tail -5 "$work/ovf.err"; fail=1
fi
rm -r "$work"
[ $fail = 0 ] && echo PROGRAM-STACK-OK
exit $fail
