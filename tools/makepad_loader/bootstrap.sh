#!/bin/sh
# Shared POSIX bootstrap. The download service supplies only pinned metadata.
# No Python, rustup, Homebrew, or prebuilt Builder executable is required.
set -eu
set +x
umask 077
builder_root=$(cd -P -- "$(dirname -- "$0")" && pwd)
export MAKEPAD_LOADER_ROOT="$builder_root"
builder_rust='@RUST_VERSION@'
builder_commit='@MAKEPAD_COMMIT@'
builder_release='@RELEASE_ID@'
builder_receipt='@MAKEPAD_RECEIPT@'
case "$(uname -s)/$(uname -m)" in
    Darwin/arm64|Darwin/aarch64) builder_triple=aarch64-apple-darwin ;;
    Darwin/x86_64) builder_triple=x86_64-apple-darwin ;;
    Linux/x86_64) builder_triple=x86_64-unknown-linux-gnu ;;
    Linux/aarch64|Linux/arm64) builder_triple=aarch64-unknown-linux-gnu ;;
    *) printf '%s\n' 'Builder supports macOS or glibc Linux on x86_64 and ARM64.'; exit 1 ;;
esac
builder_tools="$builder_root/toolchain/rust/$builder_rust-$builder_triple"
builder_prepare_host_tools() {
    case "$builder_triple" in
        *-apple-darwin)
            # rust-objcopy resolves LLVM beside rustlib's host tools. Keep the
            # link relative so moving this installation does not break it.
            builder_host_lib="$1/lib/rustlib/$builder_triple/lib"
            if [ -f "$1/lib/libLLVM.dylib" ] && [ ! -e "$builder_host_lib/libLLVM.dylib" ] && [ ! -L "$builder_host_lib/libLLVM.dylib" ]; then
                mkdir -p "$builder_host_lib"
                ln -s ../../../libLLVM.dylib "$builder_host_lib/libLLVM.dylib"
            fi
            "$1/lib/rustlib/$builder_triple/bin/rust-objcopy" --version >/dev/null
            ;;
    esac
}
if [ -x "$builder_root/makepad-builder" ] && [ "$(cat "$builder_root/.builder-version" 2>/dev/null || :)" = "$builder_commit $builder_rust $builder_triple" ]; then
    builder_prepare_host_tools "$builder_tools"
    exec "$builder_root/makepad-builder" tui
fi
builder_lock="$builder_root/.setup-lock"
if ! mkdir "$builder_lock" 2>/dev/null; then
    builder_owner=$(cat "$builder_lock/pid" 2>/dev/null || :)
    case "$builder_owner" in
        ''|0|*[!0-9]*) ;;
        *)
            # Window closure or a killed process bypasses Rust's Drop. Check
            # the owner, not the age of the directory, before reclaiming it.
            if ! kill -0 "$builder_owner" 2>/dev/null && ! ps -p "$builder_owner" -o pid= >/dev/null 2>&1; then
                if [ "$(cat "$builder_lock/pid" 2>/dev/null || :)" = "$builder_owner" ]; then
                    rm -f "$builder_lock/pid"
                    rmdir "$builder_lock" 2>/dev/null || :
                fi
            fi
            ;;
    esac
    if ! mkdir "$builder_lock" 2>/dev/null; then
        printf '%s\n' 'Another Builder setup is using this folder. Close it before retrying.'
        exit 1
    fi
fi
printf '%s' "$$" > "$builder_root/.setup-lock/pid"
builder_temp=
builder_cleanup() {
    [ -z "$builder_temp" ] || rm -rf "$builder_temp"
    rm -f "$builder_root/.setup-lock/pid"
    rmdir "$builder_root/.setup-lock" 2>/dev/null || :
}
trap builder_cleanup EXIT
trap 'exit 130' HUP INT TERM
printf '\n  1 / 3   Check system tools\n'
sh "$builder_root/check-tools.sh"
mkdir -p "$builder_root/cache" "$builder_root/toolchain/rust" "$builder_root/cargo-home" "$builder_root/target" "$builder_root/tmp"
builder_temp=$(mktemp -d "$builder_root/tmp/bootstrap.XXXXXX")
builder_hash() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{print $1}'; else shasum -a 256 "$1" | awk '{print $1}'; fi
}
builder_download() {
    builder_columns=${COLUMNS:-$(tput cols 2>/dev/null || printf 80)}
    case "$builder_columns" in ''|*[!0-9]*) builder_columns=80 ;; esac
    [ "$builder_columns" -ge 24 ] || builder_columns=24
    # No redirects, credentials, insecure TLS switches or downloaded shell code.
    # Reserve the indent inside curl's width, and preserve its failure status
    # across the POSIX pipeline (which otherwise reports only awk's status).
    rm -f "$builder_temp/curl.status"
    {
        builder_curl_status=0
        COLUMNS=$((builder_columns - 3)) curl --fail --show-error --progress-bar --proto '=https' --tlsv1.2 --output "$2" "$1" 2>&1 || builder_curl_status=$?
        printf '%s' "$builder_curl_status" > "$builder_temp/curl.status"
    } | awk 'BEGIN { RS="\r" } length { printf "\r   %s", $0; fflush() }' >&2
    return "$(cat "$builder_temp/curl.status")"
}
printf "\n  2 / 3   Makepad's local Rust\n"
if [ "$(cat "$builder_tools/.toolchain-version" 2>/dev/null || :)" != "$builder_rust $builder_triple" ] || [ ! -x "$builder_tools/bin/rustc" ] || [ ! -x "$builder_tools/bin/cargo" ]; then
    if [ -e "$builder_tools" ]; then
        printf '\n  Incomplete toolchain at %s. Move it aside before retrying.\n' "$builder_tools"
        exit 1
    fi
    printf "\n  Download Makepad's local version of Rust (%s).\n" "$builder_rust"
    printf '  It stays in this installation folder. Your existing Rust stays unchanged.\n'
    printf '  Source: static.rust-lang.org\n'
    printf '  Licenses: https://www.rust-lang.org/policies/licenses\n'
    printf '\n  Continue? [Y/n] '
    IFS= read -r builder_answer
    case "$builder_answer" in ''|y|Y|yes|Yes) ;; *) exit 0 ;; esac
    printf "\n  Downloading Makepad's local version of Rust...\n"
    builder_download "https://static.rust-lang.org/dist/channel-rust-$builder_rust.toml" "$builder_temp/rust.toml"
    mkdir "$builder_temp/ready"
    for builder_component in rustc rust-std cargo; do
        case "$builder_component" in
            rustc) builder_label='Rust compiler (1 / 3)' ;;
            rust-std) builder_label='Rust libraries (2 / 3)' ;;
            cargo) builder_label='Rust build tools (3 / 3)' ;;
        esac
        builder_header="[pkg.$builder_component.target.$builder_triple]"
        builder_field() {
            awk -v section="$builder_header" -v key="$1" '
                $0 == section {inside=1; next}
                /^\[/ {inside=0}
                inside && $1 == key && $2 == "=" {gsub(/"/, "", $3); print $3; exit}
            ' "$builder_temp/rust.toml"
        }
        builder_url=$(builder_field url)
        builder_expected=$(builder_field hash)
        case "$builder_url" in https://static.rust-lang.org/dist/*.tar.gz) ;; *) printf '%s\n' 'Unexpected Rust archive URL.'; exit 1 ;; esac
        case "$builder_expected" in ''|*[!0-9a-f]*) printf '%s\n' 'Invalid Rust checksum.'; exit 1 ;; esac
        [ "${#builder_expected}" = 64 ] || exit 1
        builder_archive="$builder_root/cache/$builder_expected.tar.gz"
        printf '\n  %s\n' "$builder_label"
        if [ ! -f "$builder_archive" ] || [ "$(builder_hash "$builder_archive")" != "$builder_expected" ]; then
            builder_download "$builder_url" "$builder_temp/component.tar.gz"
            if [ "$(builder_hash "$builder_temp/component.tar.gz")" != "$builder_expected" ]; then printf '%s\n' 'Rust checksum mismatch.'; exit 1; fi
            mv "$builder_temp/component.tar.gz" "$builder_archive"
        fi
        printf '  Setting up...\n'
        builder_unpack="$builder_temp/$builder_component"
        mkdir "$builder_unpack"
        tar -xzf "$builder_archive" -C "$builder_unpack"
        case "$builder_component" in rust-std) builder_inner="rust-std-$builder_triple" ;; *) builder_inner=$builder_component ;; esac
        # A verified official Rust component contains exactly one top directory.
        set -- "$builder_unpack"/*
        [ "$#" = 1 ] && [ -d "$1/$builder_inner" ] || { printf '%s\n' 'Unexpected Rust archive layout.'; exit 1; }
        cp -R "$1/$builder_inner/." "$builder_temp/ready/"
    done
    builder_prepare_host_tools "$builder_temp/ready"
    case "$("$builder_temp/ready/bin/rustc" --version)" in "rustc $builder_rust "*) ;; *) printf '%s\n' 'Private Rust failed its version check.'; exit 1 ;; esac
    "$builder_temp/ready/bin/cargo" --version >/dev/null
    printf '%s' "$builder_rust $builder_triple" > "$builder_temp/ready/.toolchain-version"
    mv "$builder_temp/ready" "$builder_tools"
fi
builder_prepare_host_tools "$builder_tools"
printf '  Local Rust is ready.\n'
printf '\n  3 / 3   Build the terminal menu\n'
builder_sources="$builder_root/sources/$builder_release"
builder_source="$builder_sources/makepad"
mkdir -p "$builder_sources/.builder-repositories" "$builder_root/available"
if [ ! -d "$builder_source" ]; then
    printf '  Fetch public Makepad source (one commit, depth 1)\n'
    git init -q "$builder_temp/makepad"
    git -C "$builder_temp/makepad" remote add origin https://github.com/makepad/makepad.git
    git -C "$builder_temp/makepad" -c fetch.fsckObjects=true fetch --depth=1 origin "$builder_commit"
    git -C "$builder_temp/makepad" -c advice.detachedHead=false checkout --detach FETCH_HEAD
    [ "$(git -C "$builder_temp/makepad" rev-parse HEAD)" = "$builder_commit" ] || exit 1
    git -C "$builder_temp/makepad" fsck --no-reflogs
    mv "$builder_temp/makepad" "$builder_source"
    # Git verified this pinned tree. Reuse it when Builder adds Scope next.
    printf '%s' "$builder_receipt" > "$builder_sources/.builder-repositories/makepad"
elif [ "$(cat "$builder_sources/.builder-repositories/makepad" 2>/dev/null || :)" != "$builder_receipt" ]; then
    printf '\n  Existing source has no matching Builder receipt: %s\n  It was left unchanged. Choose a different installation folder.\n' "$builder_source"
    exit 1
fi
printf '%s\n' @RELEASE_SHELL@ > "$builder_root/available/@MANIFEST_APP@.json"
export PATH="$builder_tools/bin:$PATH"
export CARGO_HOME="$builder_root/cargo-home"
export CARGO_TARGET_DIR="$builder_root/target"
export RUSTUP_HOME="$builder_root/rustup-home"
export RUSTC="$builder_tools/bin/rustc"
export RUSTDOC="$builder_tools/bin/rustdoc"
export TMPDIR="$builder_root/tmp"
unset RUSTUP_TOOLCHAIN RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_ENCODED_RUSTFLAGS RUSTFLAGS MAKEPAD_LOADER_EMAIL
export MAKEPAD_GGML_NO_CUDA=1
printf '  Compiling Makepad Builder; output stays in this installation.\n\n'
(
    cd "$builder_source"
    "$builder_tools/bin/cargo" build --release -p makepad-loader --no-default-features --bin makepad-builder-cli
)
cp "$builder_root/target/release/makepad-builder-cli" "$builder_root/makepad-builder.next"
chmod 700 "$builder_root/makepad-builder.next"
mv "$builder_root/makepad-builder.next" "$builder_root/makepad-builder"
printf '%s' "$builder_commit $builder_rust $builder_triple" > "$builder_root/.builder-version"
builder_cleanup
builder_temp=
trap - EXIT HUP INT TERM
printf '\n  Ready. Reopen with %s/run-builder.sh\n' "$builder_root"
exec "$builder_root/makepad-builder" tui
