#!/bin/bash
# Regression test: `makepad check <package> <target flags>` reports the warnings `cargo check` does
# for the same flags -- cargo check's target selection (`--tests`: the lib's and bins' unit tests
# and the integration tests beside the plain lib; `--lib`, `--bins`: the plain targets), not
# `makepad test`'s. The package (check_targets/) has a lint in each target. Run from the
# workspace root; needs cargo.
root=$PWD
J="cargo run --profile host -q --manifest-path $root/Cargo.toml -p makepad-run --bin makepad --"
W=${CHECK_TARGETS_DIR:-$root/target/check_targets}
mkdir -p "$W/pkg"
cp -R tools/run/fixture/check_targets/. "$W/pkg/"
# path:line:col: warning: message (makepad's ` -- help` and ` [lint]` tails dropped), sorted
norm() {
  python3 -c '
import re, sys
d = sys.argv[1] + "/"
out = set()
for line in sys.stdin:
    line = line.rstrip("\n").replace(d, "")
    m = re.match(r"^((?:src|tests)/[\w/.]+\.rs:\d+:\d+): (warning|error)(?:\[\w+\])?: (.*?)(?: -- .*)?(?: \[[a-z_]+\])?$", line)
    if m: out.add("%s: %s: %s" % m.groups())
print("\n".join(sorted(out)))
' "$W/pkg"
}
fail=""
for sel in "" "--lib" "--bins" "--bin check-targets" "--tests" "--test it" "--lib --tests"; do
  want=$(cd "$W/pkg" && CARGO_TARGET_DIR="$W/cargo-target" cargo check -q --message-format=short $sel 2>&1 | norm)
  got=$(cd "$W/pkg" && $J check --no-serve check-targets $sel 2>&1 | norm)
  if [ "$want" != "$got" ] || [ -z "$want" ]; then
    echo "check_targets: \`makepad check $sel\` differs from cargo's"
    diff <(printf '%s\n' "$want") <(printf '%s\n' "$got") | head -12
    fail="$fail [$sel]"
  fi
done
if [ -n "$fail" ]; then echo "check_targets FAILED:$fail"; exit 1; fi
echo "check_targets: ok"
