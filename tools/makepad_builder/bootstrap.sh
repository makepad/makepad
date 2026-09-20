#!/bin/sh
# Shared POSIX bootstrap. The download service supplies only pinned metadata.
# No Python, rustup, Homebrew, or prebuilt Builder executable is required.
set -eu
set +x
umask 077

# Resolve the existing script directory physically. A downloaded helper can
# be invoked directly from $HOME or /, so reject those roots before touching
# its cache, lock, toolchain, source or target directories.
builder_root=$(cd -P -- "$(dirname -- "$0")" 2>/dev/null && pwd -P) || {
    printf '%s\n' 'Could not resolve the Builder installation folder.' >&2
    exit 1
}
builder_home=$(cd -P -- "$HOME" 2>/dev/null && pwd -P) || {
    printf '%s\n' 'Could not resolve the home folder.' >&2
    exit 1
}
case "$builder_root" in
    /|"$builder_home")
        printf '\n  Refusing to use %s as the Builder installation folder.\n  Re-run the downloaded installer and choose a dedicated subfolder such as %s.\n' "$builder_root" "$HOME/Makepad" >&2
        exit 1
        ;;
esac
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
# Linux shows the GPU driver notice before any probe, folder write, download
# or compile, once per invocation: the installer sets MAKEPAD_GPU_ACK=1 after
# asking, and the menu clears it so children never inherit a skip. macOS is
# unchanged. Closed input and Escape cancel; other input asks again.
if [ "$(uname -s)" = Linux ] && [ "${MAKEPAD_GPU_ACK:-}" != 1 ]; then
    printf '\n  Makepad Scope heavily uses the GPU to render its UI. If you have old or\n  broken video drivers, this can result in a system crash.\n'
    while :; do
        printf '  Do you want to continue? [Y/n] '
        IFS= read -r builder_answer || { printf '\n  Setup cancelled. Nothing was downloaded or installed.\n'; exit 1; }
        case "$builder_answer" in
            ''|y|Y|yes|Yes) break ;;
            n|N|no|No|"$(printf '\033')"*) printf '  Setup cancelled. Nothing was downloaded or installed.\n'; exit 1 ;;
            *) printf '  Please answer y or n.\n' ;;
        esac
    done
fi
# Linux only: tell the menu this invocation already asked.
[ "$(uname -s)" != Linux ] || export MAKEPAD_GPU_ACK=1
builder_tools="$builder_root/toolchain/rust/$builder_rust-$builder_triple"
# selected-rust records the compiler choice: absent = not asked yet, an
# absolute sysroot = use that installed Rust, "private" = declined the offer.
# Delete the file to be asked again. Builder reads the same file.
builder_choice=$(cat "$builder_root/selected-rust" 2>/dev/null || :)
builder_external_tools=
builder_rust_reason=
builder_private_ready() {
    [ "$(cat "$builder_tools/.toolchain-version" 2>/dev/null || :)" = "$builder_rust $builder_triple" ] && [ -x "$builder_tools/bin/rustc" ] && [ -x "$builder_tools/bin/cargo" ]
}
builder_rust_check() {
    # Read-only probe/validation; success yields the sysroot, failure a reason.
    builder_external_tools=
    builder_rust_reason=
    builder_result=$(sh "$builder_root/rust-tools.sh" "$@" 2>/dev/null) && builder_external_tools=$builder_result || builder_rust_reason=$builder_result
}
builder_record_rust() {
    printf '%s\n' "$1" > "$builder_root/selected-rust.new"
    mv "$builder_root/selected-rust.new" "$builder_root/selected-rust"
}
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
builder_built=no
if [ -x "$builder_root/makepad-builder" ] && [ "$(cat "$builder_root/.builder-version" 2>/dev/null || :)" = "$builder_commit $builder_rust $builder_triple" ]; then
    builder_built=yes
    # Fast path for reopening: only when the recorded choice still works and
    # there is nothing new to ask. Every other state runs the setup steps.
    case "$builder_choice" in
        private)
            if builder_private_ready; then
                builder_prepare_host_tools "$builder_tools"
                exec "$builder_root/makepad-builder" tui
            fi
            ;;
        /*)
            builder_rust_check --validate "$builder_rust" "$builder_triple" "$builder_choice"
            [ -z "$builder_external_tools" ] || exec "$builder_root/makepad-builder" tui
            ;;
        '')
            builder_rust_check --probe "$builder_rust" "$builder_triple"
            if [ -z "$builder_external_tools" ] && builder_private_ready; then
                builder_prepare_host_tools "$builder_tools"
                exec "$builder_root/makepad-builder" tui
            fi
            ;;
    esac
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
printf "\n  2 / 3   Rust compiler\n"
# Every change of the recorded choice is an explicit answer; declining keeps
# the previous record, so a stale selection is never replaced silently.
builder_stale=
case "$builder_choice" in
    private) ;;
    /*)
        builder_rust_check --validate "$builder_rust" "$builder_triple" "$builder_choice"
        if [ -z "$builder_external_tools" ]; then
            printf '  The selected Rust at %s cannot be used:\n  %s\n' "$builder_choice" "$builder_rust_reason"
            builder_stale=$builder_choice
            builder_choice=
        fi
        ;;
esac
if [ -z "$builder_choice" ]; then
    builder_rust_check --probe "$builder_rust" "$builder_triple"
    if [ -n "$builder_external_tools" ]; then
        printf '\n  Found %s\n  at %s\n' "$("$builder_external_tools/bin/rustc" --version)" "$builder_external_tools"
        printf '  Use this installed Rust? It is not modified; Cargo caches and build\n'
        printf '  output stay in this installation folder. Answering no downloads a\n'
        printf '  private Rust into this folder instead. [Y/n] '
        IFS= read -r builder_answer
        case "$builder_answer" in
            ''|y|Y|yes|Yes) builder_record_rust "$builder_external_tools" ;;
            *) builder_record_rust private; builder_external_tools= ;;
        esac
    else
        if [ -n "$builder_rust_reason" ]; then
            printf '  Installed Rust not used: %s\n' "$builder_rust_reason"
        else
            printf '  No installed Rust found (need %s or newer).\n' "$builder_rust"
        fi
        if [ -n "$builder_stale" ]; then
            printf '\n  Switch to a private Rust in this installation folder? Answering no keeps\n'
            printf '  the selected Rust; make it available again or delete selected-rust. [Y/n] '
            IFS= read -r builder_answer
            case "$builder_answer" in
                ''|y|Y|yes|Yes) builder_record_rust private ;;
                *) printf '  Kept the selected Rust at %s.\n' "$builder_stale"; exit 1 ;;
            esac
        fi
        # Otherwise nothing is recorded: the offer returns once a compatible
        # Rust is installed, and the private flow below continues.
    fi
fi
if [ -n "$builder_external_tools" ]; then
    builder_tools=$builder_external_tools
    printf '  Using installed Rust at %s\n' "$builder_tools"
else
if ! builder_private_ready; then
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
fi
printf '\n  3 / 3   Build the terminal menu\n'
builder_sources="$builder_root/sources/$builder_release"
builder_source="$builder_sources/makepad"
mkdir -p "$builder_sources/.builder-repositories" "$builder_root/available"
if [ ! -d "$builder_source" ]; then
    printf '  Fetch public Makepad source (one commit, depth 1)\n'
    git init -q "$builder_temp/makepad"
    git -C "$builder_temp/makepad" remote add origin https://github.com/makepad/makepad.git
    # Use our immutable verified pack; published branches can move or retire.
    builder_download "https://makepad.nl/api/loader/public/source/$builder_release/makepad.pack" "$builder_temp/makepad.pack"
    if [ "$(builder_hash "$builder_temp/makepad.pack")" != "${builder_receipt#* }" ]; then
        printf '%s\n' 'Makepad source checksum mismatch.'; exit 1
    fi
    git -C "$builder_temp/makepad" index-pack --stdin < "$builder_temp/makepad.pack"
    printf '%s\n' "$builder_commit" > "$builder_temp/makepad/.git/shallow"
    git -C "$builder_temp/makepad" -c advice.detachedHead=false checkout --detach "$builder_commit"
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
# The selected compiler's own binaries come first; rustup proxies later on
# PATH cannot install or switch anything with RUSTUP_AUTO_INSTALL=0 and a
# private empty RUSTUP_HOME. Caches, build output and temp files stay here.
export PATH="$builder_tools/bin:$PATH"
export CARGO_HOME="$builder_root/cargo-home"
export CARGO_TARGET_DIR="$builder_root/target"
export RUSTUP_HOME="$builder_root/rustup-home"
export RUSTUP_AUTO_INSTALL=0
export RUSTC="$builder_tools/bin/rustc"
if [ -x "$builder_tools/bin/rustdoc" ]; then export RUSTDOC="$builder_tools/bin/rustdoc"; else unset RUSTDOC; fi
export TMPDIR="$builder_root/tmp"
unset RUSTUP_TOOLCHAIN RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_ENCODED_RUSTFLAGS RUSTFLAGS MAKEPAD_LOADER_EMAIL
export MAKEPAD_GGML_NO_CUDA=1
if [ "$builder_built" = yes ]; then
    printf '  Builder is already compiled for this release.\n'
else
    printf '  Compiling Makepad Builder; output stays in this installation.\n\n'
    (
        cd "$builder_source"
        "$builder_tools/bin/cargo" build --release -p makepad-loader --no-default-features --bin makepad-builder-cli
    )
    cp "$builder_root/target/release/makepad-builder-cli" "$builder_root/makepad-builder.next"
    chmod 700 "$builder_root/makepad-builder.next"
    mv "$builder_root/makepad-builder.next" "$builder_root/makepad-builder"
    printf '%s' "$builder_commit $builder_rust $builder_triple" > "$builder_root/.builder-version"
fi
builder_cleanup
builder_temp=
trap - EXIT HUP INT TERM
printf '\n  Ready. Reopen with %s/run-builder.sh\n' "$builder_root"
exec "$builder_root/makepad-builder" tui
