#!/bin/bash
# usage: run.sh <makepad binary>: four modules, each a call nested 2000 deep (cargo run takes it), parse on
# the load's parse workers (MAKEPAD_RUN_JOBS=2) and compile on the JIT's pool workers as on the owner thread:
# every compiler thread has the same stack (COMPILER_STACK), so the nesting fits whichever thread takes a
# module. Runs the program twice; prints "compiler_thread_stack: ok" or "compiler_thread_stack: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d)
N=2000
mkdir -p "$T/x/src"
printf '[package]\nname = "cts-app"\nversion = "0.1.0"\nedition = "2021"\npublish = false\n\n[workspace]\n' > "$T/x/Cargo.toml"
printf 'mod d0;\nmod d1;\nmod d2;\nmod d3;\nfn s(x: u64) -> u64 { x + 1 }\nfn main() { println!("deep {} {} {} {}", d0::f(), d1::f(), d2::f(), d3::f()); }\n' > "$T/x/src/main.rs"
for i in 0 1 2 3; do
  { printf 'pub fn f() -> u64 {\n'; for ((k = 0; k < N; k++)); do printf 'crate::s('; done; printf '%d' $i; for ((k = 0; k < N; k++)); do printf ')'; done; printf '\n}\n'; } > "$T/x/src/d$i.rs"
done
WANT="deep $N $((N + 1)) $((N + 2)) $((N + 3))"
ok=1
for r in 1 2; do
  OUT=$(cd "$T/x" && MAKEPAD_RUN_JOBS=2 MAKEPAD_RUN_CACHE=off MAKEPAD_RUN_STD=$H/../../std MAKEPAD_HOME=$T/home MAKEPAD_RUN_HOME=$T MAKEPAD_RUN_TARGET_DIR=$T/target "$1" run --quiet cts-app 2>>"$T/err")
  echo "run $r: $OUT"
  [ "$OUT" = "$WANT" ] || ok=0
done
[ $ok = 1 ] || grep -v '^\s*$' "$T/err" | tail -5
rm -rf "$T"
if [ $ok = 1 ]; then echo "compiler_thread_stack: ok"; else echo "compiler_thread_stack: FAILED"; exit 1; fi
