#!/bin/bash
# usage: run.sh [<makepad binary>] (default builds this checkout's host makepad)
# The makepad test dependency check (M51) and makepad run app check (cache.rs app_check_key)
# key a crate's verdict by its own text and its dependencies' interfaces.
# Cold, after a tdk-base body edit, and after an interface edit, test selects three dependencies
# and finds 0, 2, 1 stored clean. Run checks all app crates, then only tdk-base, then all but
# tdk-side. No edit touches tdk-side, beside tdk-base under tdk-mid: an edited start loads it ahead
# of tdk-base (the reordered load puts clean crates first), so a key that followed the load order
# would miss after the body edit.
# Never stale: in each case below the crates are clean as written, and one change that leaves the
# interface text as it was (a static a const reads, a position a const reads, an input a macro
# reads) or that changes an interface body (an impl Trait return losing Send, a const fn body, an
# inherent impl in a plain fn body) makes tdk-mid fail. The test run after it must report that
# error, not tdk-mid's stored verdict; so must `makepad run` in the cases marked `run`.
H=$(cd "$(dirname "$0")" && pwd)
if [ $# -ge 1 ]; then J=$1; else
  ROOT=$(cd "$H/../../../../.." && pwd)
  cargo build --profile host -q --manifest-path "$ROOT/Cargo.toml" -p makepad-run --bin makepad || exit 1
  J=${CARGO_TARGET_DIR:-$ROOT/target}/host/makepad
fi
W=$(mktemp -d)
trap 'rm -rf "$W"' EXIT
cp -R "$H/." "$W/"
unset RELAX_STD RELAX_CACHE RELAX_CACHE_DIR
export RELAX_VERBOSE=1
# "<selected> <stored clean>" of one run
counts() {
  local out
  out=$(cd "$W" && "$1" test test-dep-keys 2>&1)
  echo "$out" | grep -q '^test t::uses_mid \.\.\. ok' || { echo "$out" | tail -20 >&2; echo "x x"; return; }
  echo "$out" | sed -n 's/.*test dependency check: \([0-9]*\) crates selected, \([0-9]*\) stored clean.*/\1 \2/p' | tail -1
}
# "<checked> <all>" from the app check
run_counts() {
  local out
  out=$(cd "$W" && "$J" run test-dep-keys --bin tdk-run 2>&1)
  echo "$out" | grep -q '^tdk-run ' || { echo "$out" | tail -20 >&2; echo "x x"; return; }
  echo "$out" | sed -n 's/.*relax: app check: \([0-9][0-9]*\) of \([0-9][0-9]*\) app crates checked.*/\1 \2/p' | tail -1
}
cold=$(counts "$J")
run_cold=$(run_counts)
cp "$W/edits/body.rs" "$W/base/src/lib.rs"
body=$(counts "$J")
run_body=$(run_counts)
cp "$W/edits/iface.rs" "$W/base/src/lib.rs"
iface=$(counts "$J")
run_iface=$(run_counts)
echo "cold: $cold; body edit: $body; interface edit: $iface (selected, stored clean)"
echo "run: cold: $run_cold; body edit: $run_body; interface edit: $run_iface (checked, app crates)"
bad=""
if [ "$cold" != "3 0" ] || [ "$body" != "3 2" ] || [ "$iface" != "3 1" ]; then bad=" keys (want 3 0, 3 2, 3 1)"; fi
all=${run_cold#* }
if ! [[ $all =~ ^[0-9]+$ ]] || [ "$run_cold" != "$all $all" ] || [ "$run_body" != "1 $all" ] || [ "$run_iface" != "$((all - 1)) $all" ]; then
  bad="$bad run-keys (want all, 1, all but one)"
fi

# a run's outcome: ok, or mid (the dependency check reported tdk-mid's error)
outcome() {
  local out
  out=$(cd "$W" && "$J" test test-dep-keys 2>&1)
  if echo "$out" | grep -q 'could not compile `tdk-mid`'; then echo mid
  elif echo "$out" | grep -q '^test t::uses_mid \.\.\. ok'; then echo ok
  else echo "$out" | tail -20 >&2; echo x; fi
}
outcome_run() {
  local out status
  out=$(cd "$W" && "$J" run test-dep-keys --bin tdk-run 2>&1)
  status=$?
  if [ "$status" -eq 0 ] && echo "$out" | grep -q 'tdk-run'; then echo ok
  elif [ "$status" -ne 0 ] && echo "$out" | grep -Fq 'mid/src/lib.rs'; then echo mid
  else echo "$out" | tail -20 >&2; echo x; fi
}
# file $1 of the copy holds $2
put() { mkdir -p "$(dirname "$W/$1")"; printf '%s\n' "$2" > "$W/$1"; }
# case <name> <change> [run]: a clean run as the crates are written, then the change, then a run that
# reports tdk-mid's error. With `run`, `makepad run` too: the cases whose input reaches the two keys
# differently (the check extra, and the late inputs each check's store ledger holds); the others hash
# the same interface digest in both keys and run once.
case_() {
  local a ar=- b br=-
  a=$(outcome)
  [ "$3" = run ] && ar=$(outcome_run)
  eval "$2"
  b=$(outcome)
  [ "$3" = run ] && br=$(outcome_run)
  echo "$1: $a, then $b; run: $ar, then $br"
  if [ "$a" != ok ] || [ "$b" != mid ]; then bad="$bad $1"
  elif [ "$3" = run ] && { [ "$ar" != ok ] || [ "$br" != mid ]; }; then bad="$bad $1"; fi
}
MID='pub fn mid_value() -> u32 {
    tdk_base::base_value() + 1
}

// neither the test nor main calls it: both checks must catch its error
pub fn sized() -> [u8; 3] {
    [0u8; tdk_base::C]
}'
put mid/src/lib.rs "$MID"

# the statics' initializers a const reads: two statics trading them
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
static S: usize = 3;
pub static T: usize = 4;
pub const C: usize = S;'
case_ static "sed -i.x -e 's/S: usize = 3;/S: usize = 9;/' -e 's/T: usize = 4;/T: usize = 3;/' -e 's/S: usize = 9;/S: usize = 4;/' '$W/base/src/lib.rs'" run

# the line a const is written on (caller_location), after a body that gains a line
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
// C is the line it is written on
pub const C: usize = core::panic::Location::caller().line() as usize;'
case_ caller "sed -i.x 's/{ 1 }/{\\
    1 }/' '$W/base/src/lib.rs'"

# the same through a #[track_caller] const fn: the const that calls it names no position
put base/src/lib.rs '#[track_caller]
pub const fn line_here() -> usize { core::panic::Location::caller().line() as usize }
pub fn base_value() -> u32 { 1 }
pub const C: usize = line_here() - 1;'
case_ track_caller "sed -i.x 's/{ 1 }/{\\
    1 }/' '$W/base/src/lib.rs'"

# line!()
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
// C is the line it is written on
pub const C: usize = line!() as usize;'
case_ line "sed -i.x 's/{ 1 }/{\\
    1 }/' '$W/base/src/lib.rs'"

# a file a const includes
put base/src/data.txt abc
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
pub const C: usize = include_str!("data.txt").len() - 1;'
case_ include_str "put base/src/data.txt abcd" run

# an environment variable a const reads
export TDK_TEXT=abc
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
pub const C: usize = env!("TDK_TEXT").len();'
case_ env "export TDK_TEXT=abcd" run

# a file tdk-mid's own body includes (its type is the file's length)
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
pub const C: usize = 3;'
put mid/src/own.bin abc
put mid/src/lib.rs "$MID
pub fn own() -> [u8; 4] {
    *include_bytes!(\"own.bin\")
}"
case_ include_bytes "printf abc > '$W/mid/src/own.bin'" run

# the hidden return type of an impl Trait loses Send
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
pub const C: usize = 3;
pub fn hidden() -> impl Sized { 1u32 }'
put mid/src/lib.rs "$MID
fn need_send<T: Send>(_: T) {}
pub fn sends() { need_send(tdk_base::hidden()) }"
case_ rpit_send "sed -i.x '/^pub fn hidden()/s/{ 1u32 }$/{ std::rc::Rc::new(1u32) }/' '$W/base/src/lib.rs'"

# a const fn body sets an array length in tdk-mid
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
pub const C: usize = 3;
pub const fn n() -> usize { 3 }'
put mid/src/lib.rs "$MID
pub fn sized_n() -> [u8; 3] { [0u8; tdk_base::n()] }"
case_ const_fn_body "sed -i.x '/^pub const fn n()/s/{ 3 }$/{ 4 }/' '$W/base/src/lib.rs'"

# an inherent impl inside a function takes precedence over the trait method
put base/src/lib.rs 'pub fn base_value() -> u32 { 1 }
pub const C: usize = 3;
pub struct B;
pub trait Ext { fn go(&self) -> u32 { 1 } }
impl Ext for B {}'
put mid/src/lib.rs "$MID
pub fn goes() -> u32 { use tdk_base::Ext; tdk_base::B.go() }"
impl_edit="/^pub fn base_value()/s/{ 1 }$/{ impl B { pub fn go(\&self) -> \&'static str { \"\" } } 1 }/"
case_ impl_in_body 'sed -i.x "$impl_edit" "$W/base/src/lib.rs"'

if [ -z "$bad" ]; then echo "test_dep_keys: ok"
else echo "test_dep_keys: FAILED:$bad"; exit 1; fi
