#!/bin/bash
# tools/run/host/build.sh: the `makepad` JIT binary (Relax + makepad run) built for speed on the machine
# that runs it: Cargo.toml [profile.host] (opt-level 3, thin LTO, codegen-units 16) + -Ctarget-cpu=native,
# optionally with PGO. The result is installed at target/host/makepad (never over target/release/makepad:
# release stays the distributable build and the one every gate, A/B and timing series uses).
#
#   build.sh plain           host profile + native cpu
#   build.sh gen  <dir>      instrumented build (-Cprofile-generate=<dir>), for train.sh
#   build.sh use  <profdata> PGO build from a merged profile
#   build.sh pgo             gen -> train.sh -> llvm-profdata merge -> use, in one go
#
# Env: HOST_OUT       installed binary (default <root>/target/host/makepad; gen: <root>/target/host/makepad-gen)
#      HOST_CPU       -Ctarget-cpu (default native; a distributable host build would name a cpu)
#      HOST_JOBS      cargo -j (default 4)
#      HOST_TARGET_DIR  cargo target dir (default <root>/target/hostbuild-<mode>: each flavour keeps its own,
#                     RUSTFLAGS differ between them)
#      HOST_PGO_DIR   pgo work dir (raw profiles, merged profile; default <root>/local/hostopt/<triple>)
#
# The flags go through --target <host triple>, so build scripts and proc macros are built plain (they are
# neither instrumented nor profiled). Release-class build, CARGO_INCREMENTAL=0, nice.
#
# When to retrain (build.sh pgo): a new rustc (the profile's function hashes follow the compiler), or large
# changes to Relax / makepad run. A stale profile only loses gains (unmatched functions are optimised as if
# unprofiled); it never changes behaviour.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
root=$(cd "$here/../../.." && pwd)
mode=${1:-plain}
triple=$(rustc -vV | sed -n 's/^host: //p')
cpu=${HOST_CPU:-native}
jobs=${HOST_JOBS:-4}
pgo_dir=${HOST_PGO_DIR:-$root/local/hostopt/$triple}
export CARGO_INCREMENTAL=0

profdata_tool() {
    local t
    t="$(rustc --print sysroot)/lib/rustlib/$triple/bin/llvm-profdata"
    [ -x "$t" ] || t=$(command -v llvm-profdata || true)
    [ -n "$t" ] && [ -x "$t" ] || { echo "build.sh: no llvm-profdata (rustup component add llvm-tools)" >&2; exit 2; }
    # the merged profile must be written by the LLVM major version rustc uses
    local want have
    want=$(rustc -vV | sed -n 's/^LLVM version: \([0-9]*\).*/\1/p')
    have=$("$t" --version | sed -n 's/.*LLVM version \([0-9]*\).*/\1/p' | head -1)
    [ "$want" = "$have" ] || { echo "build.sh: $t is LLVM $have, rustc uses LLVM $want" >&2; exit 2; }
    echo "$t"
}

# build <flavour> <extra rustflags> <out>
build() {
    local flavour=$1 extra=$2 out=$3
    local td=${HOST_TARGET_DIR:-$root/target/hostbuild-$flavour}
    local log=$td.log
    mkdir -p "$(dirname "$out")" "$td"
    echo "build.sh: $flavour -> $out (target dir $td, log $log)"
    local t0=$SECONDS
    if ! ( cd "$root" && RUSTFLAGS="-Ctarget-cpu=$cpu $extra" nice -n 15 cargo build --profile host \
        --target "$triple" -p makepad-run --bin makepad -j "$jobs" --target-dir "$td" ) > "$log" 2>&1; then
        grep -E '^(warning|error)' "$log" | head -20; echo "build.sh: $flavour build FAILED (log $log)" >&2; exit 3
    fi
    echo "build.sh: $flavour built in $((SECONDS - t0)) s"
    cp "$td/$triple/host/makepad" "$out.new"
    mv "$out.new" "$out"
}

case $mode in
plain)
    build plain "" "${HOST_OUT:-$root/target/host/makepad}"
    ;;
gen)
    dir=${2:?usage: build.sh gen <raw profile dir>}
    mkdir -p "$dir"
    build gen "-Cprofile-generate=$dir" "${HOST_OUT:-$root/target/host/makepad-gen}"
    ;;
use)
    prof=${2:?usage: build.sh use <merged .profdata>}
    prof=$(cd "$(dirname "$prof")" && pwd)/$(basename "$prof")
    build use "-Cprofile-use=$prof" "${HOST_OUT:-$root/target/host/makepad}"
    ;;
pgo)
    tool=$(profdata_tool)
    raw=$pgo_dir/raw
    rm -rf "$raw"
    mkdir -p "$raw"
    build gen "-Cprofile-generate=$raw" "$pgo_dir/makepad-gen"
    "$here/train.sh" "$pgo_dir/makepad-gen" "$raw" "$pgo_dir/train"
    "$tool" merge -o "$pgo_dir/makepad.profdata.new" "$raw"
    mv "$pgo_dir/makepad.profdata.new" "$pgo_dir/makepad.profdata"
    echo "build.sh: merged profile $pgo_dir/makepad.profdata"
    build use "-Cprofile-use=$pgo_dir/makepad.profdata" "${HOST_OUT:-$root/target/host/makepad}"
    ;;
*)
    echo "usage: build.sh plain | gen <dir> | use <profdata> | pgo" >&2; exit 2
    ;;
esac
