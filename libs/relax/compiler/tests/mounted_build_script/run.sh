#!/bin/bash
# usage: run.sh <makepad binary>: a package with a build script, run from a packed source archive
# (`pack-src`, then `run --src`): the workspace is a git store mounted on Relax's VFS, as a devhub
# overlay's is, so the run takes the mount's own std (a path on no disk), and the build script's
# child run inherits it with the mount table. The run must exit 0 and print the program's line,
# on the mount's std. Prints "mounted_build_script: ok" or "mounted_build_script: FAILED ..."
# (exit 1).
H=$(cd "$(dirname "$0")" && pwd)
T=$(mktemp -d) || exit 1
[ -n "$T" ] || exit 1
trap 'rm -rf "$T"' EXIT
W=$T/ws
mkdir -p "$W/libs/relax/compiler" || exit 1
cp -R "$H/Cargo.toml" "$H/build.rs" "$H/src" "$W/" && cp -R "$H/../../std" "$W/libs/relax/compiler/std" || exit 1
unset RELAX_STD RELAX_STD_AUTO RELAX_VFS RELAX_WORKSPACE RELAX_EDITS
export RELAX_HOME="$T/home" RELAX_CACHE_DIR="$T/cache" CARGO_NET_OFFLINE=true RUSTC_BOOTSTRAP=1
if ! (cd "$W" && "$1" pack-src "$T/src.store" mounted-build-script) > "$T/pack" 2>&1; then
  tail -8 "$T/pack"; echo "mounted_build_script: FAILED (pack-src)"; exit 1
fi
OUT=$(cd "$T" && "$1" run --src "$T/src.store" mounted-build-script 2>"$T/err"); rc=$?
bad=
[ $rc = 0 ] || bad="$bad exit $rc"
[ "$OUT" = "mounted_build_script result ran" ] || bad="$bad output [$OUT]"
# (the run fell back to a disk std: the mount's std was not exercised)
grep -q "using disk std" "$T/err" && bad="$bad disk std"
if [ -z "$bad" ]; then echo "mounted_build_script: ok"; else tail -8 "$T/err"; echo "mounted_build_script: FAILED$bad"; exit 1; fi
