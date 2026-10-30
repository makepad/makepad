#!/bin/bash
# install_xtool.sh: the cross-check toolchain of a Linux pool node, ~/devhub/xtool (its own RUSTUP_HOME and CARGO_HOME:
# the node's system rust and every lane's cargo stay as they are). rustc from the official channel plus the rust-std
# of the targets check_platforms.sh `cross` and the CI cross shards (ci_xcheck.sh) check without linking: windows msvc and gnu, wasm32, android (aarch64, x86_64, i686). Idempotent; run on
# the node (`makepad devhub run <lane> build --node <n> -- bash tools/run/fixture/install_xtool.sh`).
set -eu
X=$HOME/devhub/xtool
export RUSTUP_HOME=$X/rustup CARGO_HOME=$X/cargo
mkdir -p "$X"
if [ ! -x "$CARGO_HOME/bin/rustup" ]; then
  curl -sSf --max-time 120 -o "$X/rustup-init" https://static.rust-lang.org/rustup/dist/x86_64-unknown-linux-gnu/rustup-init
  chmod +x "$X/rustup-init"
  "$X/rustup-init" -y --no-modify-path --profile minimal --default-toolchain stable >/dev/null 2>&1
  rm -f "$X/rustup-init"
fi
"$CARGO_HOME/bin/rustup" target add x86_64-pc-windows-msvc x86_64-pc-windows-gnu wasm32-unknown-unknown aarch64-linux-android x86_64-linux-android i686-linux-android 2>&1 | tail -3
"$CARGO_HOME/bin/rustc" -V
echo XTOOL-OK
