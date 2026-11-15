#!/bin/bash
# usage: run.sh <makepad binary>: a build script's CARGO is the loader, as its RUSTC is, and answers
# `$CARGO metadata`, also when the run was started with a cargo of its own (CARGO by an absolute
# path, as `cargo run` sets it; here one that fails). The script's `$RUSTC` questions are answered
# by its own process, with the lines the loader started as RUSTC gives. Prints
# "script_cargo_is_loader: ok" or "script_cargo_is_loader: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d) || exit 1
[ -n "$T" ] || exit 1
trap 'rm -rf "$T"' EXIT
printf '#!/bin/sh\necho "a cargo ran: $*" >&2\nexit 1\n' > "$T/cargo"
chmod +x "$T/cargo"
export RELAX_CACHE=off CARGO_NET_OFFLINE=true RELAX_CACHE_DIR="$T/cache" RELAX_TARGET_DIR="$T/target" RELAX_STD=$H/../../std
OUT=$(cd "$H" && CARGO="$T/cargo" "$1" run --quiet script-cargo-app 2>"$T/err")
echo "$OUT"
# what the loader started as RUSTC answers
VERSION=$(RELAX_AS_RUSTC=rustc,cargo "$1" --version)
CFG=$(RELAX_AS_RUSTC=rustc,cargo "$1" --print cfg | paste -sd, -)
WANT=$(printf '%s\n%s\n%s\n%s' "CARGO is the loader, metadata ok" "RUSTC answered here, compile probe status 1, -vV agrees" "$VERSION" "$CFG")
if [ "$OUT" = "$WANT" ]; then echo "script_cargo_is_loader: ok"; else echo "-- wanted:"; echo "$WANT"; tail -5 "$T/err"; echo "script_cargo_is_loader: FAILED"; exit 1; fi
