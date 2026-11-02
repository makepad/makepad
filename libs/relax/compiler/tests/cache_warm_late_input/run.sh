#!/bin/bash
# run.sh [makepad binary]: (default: the tree's, built here) a late macro input (the file of an include_str!/include_bytes!, the value of an env!) that
# changes between runs reaches every code that saw it, in its own crate and in the crates that depend on it: a const
# folded into a dependant, a fn body, a generic instantiated downstream, a macro_rules that wraps the include, a second
# level dependant. Each step changes one input on the same cache and runs again; the program prints what it sees, the
# line must equal what a fresh build of the same inputs prints (the steps' expected line). Before the fix a
# changed input left the dependants' stored code in place: the old text came back.
# Prints "cache_warm_late_input: ok" or "cache_warm_late_input: FAILED ...".
if [ -n "${1:-}" ]; then
  R=$(cd "$(dirname "$1")" && pwd)/$(basename "$1")
else
  cargo build --profile host -q -p makepad-run --bin makepad || exit 3
  R=${CARGO_TARGET_DIR:-$PWD/target}/host/makepad
fi
C=$(mktemp -d)
chmod 700 "$C"
P=$C/p
mkdir -p "$P/dep/src" "$P/mid/src" "$P/app/src" "$C/home/tmp"
cat > "$P/Cargo.toml" <<'T'
[workspace]
members = ["dep", "mid", "app"]
resolver = "2"
T
for k in dep mid app; do
  printf '[package]\nname = "late_%s"\nversion = "0.1.0"\nedition = "2021"\n\n[dependencies]\n' "$k" > "$P/$k/Cargo.toml"
done
echo 'late_dep = { path = "../dep" }' >> "$P/mid/Cargo.toml"
printf 'late_dep = { path = "../dep" }\nlate_mid = { path = "../mid" }\n' >> "$P/app/Cargo.toml"
cat > "$P/dep/src/lib.rs" <<'T'
pub const TEXT: &str = include_str!("../konst.txt");
pub fn in_body() -> &'static str { include_str!("../body.txt") }
#[inline]
pub fn generic<T>(_: T) -> &'static str { include_str!("../generic.txt") }
pub fn bytes_len() -> usize { include_bytes!("../bytes.bin").len() }
pub fn env_value() -> &'static str { env!("LATE_PROBE_V") }
pub fn opt_env_value() -> &'static str { match option_env!("LATE_PROBE_O") { Some(v) => v, None => "none" } }
macro_rules! wrapped { () => { include_str!("../macro.txt") } }
pub fn via_macro() -> &'static str { wrapped!() }
pub const LEN: usize = TEXT.len();
pub struct Sized_([u8; LEN]);
pub const fn const_len() -> usize { include_bytes!("../bytes.bin").len() }
T
cat > "$P/mid/src/lib.rs" <<'T'
pub const KONST: &str = late_dep::TEXT;
pub fn from_generic() -> &'static str { late_dep::generic(0u8) }
pub const BYTES: usize = late_dep::const_len();
T
cat > "$P/app/src/main.rs" <<'T'
fn main() {
    println!(
        "LINE konst={} mid={} body={} generic={} bytes={} env={} opt={} macro={} size={} cbytes={}",
        late_dep::TEXT, late_mid::KONST, late_dep::in_body(), late_mid::from_generic(),
        late_dep::bytes_len(), late_dep::env_value(), late_dep::opt_env_value(), late_dep::via_macro(),
        std::mem::size_of::<late_dep::Sized_>(), late_mid::BYTES
    );
}
T
printf k1 > "$P/dep/konst.txt"; printf b1 > "$P/dep/body.txt"; printf g1 > "$P/dep/generic.txt"
printf 123 > "$P/dep/bytes.bin"; printf m1 > "$P/dep/macro.txt"
V=v1; O=
bad=""
k=0
run() { # run <label> <expected line>
  k=$((k + 1))
  log=$C/run$k.log
  s=$EPOCHREALTIME
  (cd "$P" && env -u MAKEPAD_RUN_CACHE_TRACE MAKEPAD_RUN_HOME="$C/home" MAKEPAD_RUN_TARGET_DIR="$C/home/target" MAKEPAD_RUN_CACHE_DIR="$C/home/cache" \
    TMPDIR="$C/home/tmp" MAKEPAD_RUN_VERBOSE=1 LATE_PROBE_V="$V" ${O:+LATE_PROBE_O="$O"} "$R" run --verbose late_app > "$log" 2>&1)
  rc=$?
  e=$EPOCHREALTIME
  got=$(grep -a -m1 '^LINE ' "$log")
  printf 'step %-10s rc=%s %ss %s | %s\n' "$1" "$rc" "$(awk "BEGIN{printf \"%.2f\", $e - $s}")" "$got" "$(grep -a -m1 -oE 'compiled [0-9]+ fns' "$log")"
  if [ "$got" != "LINE $2" ]; then
    bad="$bad $1(want: $2)(got: $got);"
    grep -a -v '^LINE ' "$log" | grep -a -iE 'error|panic' | head -5 | cut -c1-200
  fi
}
line() { echo "konst=$1 mid=$1 body=$2 generic=$3 bytes=$4 env=$5 opt=$6 macro=$7 size=${#1} cbytes=$4"; }
run cold    "$(line k1 b1 g1 3 v1 none m1)"
run warm    "$(line k1 b1 g1 3 v1 none m1)"
printf k22 > "$P/dep/konst.txt";  run konst   "$(line k22 b1 g1 3 v1 none m1)"
printf b2 > "$P/dep/body.txt";    run body    "$(line k22 b2 g1 3 v1 none m1)"
printf g2 > "$P/dep/generic.txt"; run generic "$(line k22 b2 g2 3 v1 none m1)"
printf 12345 > "$P/dep/bytes.bin"; run bytes  "$(line k22 b2 g2 5 v1 none m1)"
printf m2 > "$P/dep/macro.txt";   run macro   "$(line k22 b2 g2 5 v1 none m2)"
V=v2;                             run env     "$(line k22 b2 g2 5 v2 none m2)"
O=o1;                             run opt_set "$(line k22 b2 g2 5 v2 o1 m2)"
O=;                               run opt_gone "$(line k22 b2 g2 5 v2 none m2)"
printf k1 > "$P/dep/konst.txt";   run konst_back "$(line k1 b2 g2 5 v2 none m2)"
run warm2   "$(line k1 b2 g2 5 v2 none m2)"
if [ -z "$bad" ]; then echo "cache_warm_late_input: ok"; else echo "cache_warm_late_input: FAILED$bad"; fi
# the temp dir this script made (mktemp -d), nothing else
python3 - "$C" <<'PY'
import os, shutil, sys
c = sys.argv[1]
if os.path.dirname(c) and os.path.isdir(c) and not os.path.islink(c) and os.path.basename(c).startswith("tmp."):
    shutil.rmtree(c)
PY
[ -z "$bad" ]
