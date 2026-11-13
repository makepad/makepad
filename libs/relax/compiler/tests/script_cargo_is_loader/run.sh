#!/bin/bash
# usage: run.sh <makepad binary>: a build script's CARGO is the loader, as its RUSTC is, and answers
# `$CARGO metadata`, also when the run was started with a cargo of its own (CARGO by an absolute
# path, as `cargo run` sets it; here one that fails). Prints "script_cargo_is_loader: ok" or
# "script_cargo_is_loader: FAILED" (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d) || exit 1
[ -n "$T" ] || exit 1
trap 'rm -rf "$T"' EXIT
printf '#!/bin/sh\necho "a cargo ran: $*" >&2\nexit 1\n' > "$T/cargo"
chmod +x "$T/cargo"
export RELAX_CACHE=off CARGO_NET_OFFLINE=true RELAX_CACHE_DIR="$T/cache" RELAX_TARGET_DIR="$T/target"
OUT=$(cd "$H" && CARGO="$T/cargo" RELAX_STD=$H/../../std "$1" run --quiet script-cargo-app 2>"$T/err")
echo "$OUT"
if [ "$OUT" = "CARGO is the loader, metadata ok" ]; then echo "script_cargo_is_loader: ok"; else tail -5 "$T/err"; echo "script_cargo_is_loader: FAILED"; exit 1; fi
