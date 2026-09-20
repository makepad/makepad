#!/bin/sh
# Read-only checks shared by the Unix bootstrap and Builder. Never run rustup
# installation or change the user's defaults. A rustup proxy is asked once
# (probe) which toolchain is the default; afterwards only that toolchain's own
# binaries run, so no shim can download or switch anything.
#   rust-tools.sh --probe MINIMUM TRIPLE
#   rust-tools.sh --validate MINIMUM TRIPLE SYSROOT
# Success prints the canonical sysroot (always starting with /). Exit 1 with
# no output: no installed Rust. Exit 2: installed but unusable; stdout holds
# the one-line reason. Only tools the Builder build path needs are required.
set -eu
mode=$1
minimum=$2
host=$3
reject() { printf '%s\n' "$1"; exit 2; }
case "$mode" in
    --probe)
        # Ask from / so no rust-toolchain file in the invoking directory can
        # select or auto-install a different toolchain through rustup.
        toolchain=$(cd / && unset RUSTUP_TOOLCHAIN && RUSTUP_AUTO_INSTALL=0 rustc --print sysroot 2>/dev/null) || exit 1
        [ -n "$toolchain" ] || exit 1
        ;;
    --validate) toolchain=$4 ;;
    *) exit 1 ;;
esac
case "$toolchain" in /*) ;; *) reject "The recorded Rust location is not an absolute path." ;; esac
[ -d "$toolchain" ] || reject "Rust is no longer installed at $toolchain."
toolchain=$(cd -P -- "$toolchain" && pwd) || reject "Rust is no longer installed at $toolchain."
[ -x "$toolchain/bin/rustc" ] || reject "No rustc in $toolchain/bin."
metadata=$("$toolchain/bin/rustc" --version --verbose 2>/dev/null) || reject "rustc in $toolchain/bin does not run."
version=$(printf '%s\n' "$metadata" | awk '$1 == "release:" { print $2 }')
actual_host=$(printf '%s\n' "$metadata" | awk '$1 == "host:" { print $2 }')
[ "$actual_host" = "$host" ] || reject "Installed Rust is built for ${actual_host:-an unknown host}; this Builder needs $host."
stable() {
    # A stable numeric release only: a prerelease of the minimum is older
    # than that release. Compare components, never strings.
    awk -v actual="$1" -v minimum="$minimum" 'BEGIN {
        if (actual !~ /^[0-9]+[.][0-9]+[.][0-9]+$/) exit 1
        split(actual, a, "."); split(minimum, b, ".")
        for (i = 1; i <= 3; i++) {
            if (a[i] + 0 > b[i] + 0) exit 0
            if (a[i] + 0 < b[i] + 0) exit 2
        }
        exit 0
    }'
}
stable "$version" || case $? in
    1) reject "Installed Rust $version is not a stable release; Builder needs $minimum or newer." ;;
    *) reject "Installed Rust $version is older than $minimum." ;;
esac
[ -x "$toolchain/bin/cargo" ] || reject "Installed Rust has no cargo beside rustc."
cargo_version=$("$toolchain/bin/cargo" --version 2>/dev/null | awk '$1 == "cargo" { print $2 }') || :
stable "${cargo_version:-none}" || reject "Installed cargo ${cargo_version:-is unusable and} is older than $minimum."
[ -d "$toolchain/lib/rustlib/$host/lib" ] || reject "Installed Rust has no standard library for $host."
case "$host" in
    *-apple-darwin)
        # Release builds strip through rustc's bundled rust-objcopy. Report a
        # copy that cannot load its LLVM library; never repair it here.
        "$toolchain/lib/rustlib/$host/bin/rust-objcopy" --version >/dev/null 2>&1 || reject "Installed Rust's rust-objcopy does not run; release builds could not be stripped."
        ;;
esac
printf '%s\n' "$toolchain"
