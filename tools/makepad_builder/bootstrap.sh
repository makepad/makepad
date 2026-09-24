#!/bin/sh
# Makepad Builder bootstrap (macOS and Linux).
#
# Runs from the Builder folder and gets the Builder going:
#   1. check the system build tools        (check-tools.sh)
#   2. pick a Rust compiler                (yours, or a private copy here)
#   3. fetch the public Makepad source     (one pinned commit, verified)
#   4. compile the Builder and start it
#
# Safe to run again at any time: each step looks at what is already here.
# It never deletes anything. Downloads go to fixed names that the next run
# overwrites, and each finished step is moved into place with one rename.
# Everything stays inside this folder; nothing outside it is changed.
set -eu

# Pinned by the download service; the Builder updates these four lines
# (they must stay exactly in this form).
builder_rust='@RUST_VERSION@'
builder_commit='@MAKEPAD_COMMIT@'
builder_release='@RELEASE_ID@'
builder_receipt='@MAKEPAD_RECEIPT@'

say()  { printf '  %s\n' "$*"; }
step() { printf '\n  %s\n' "$*"; }
fail() { printf '\n  %s\n\n' "$*" >&2; exit 1; }

# --- Where we are -----------------------------------------------------------

root=$(cd -P -- "$(dirname -- "$0")" && pwd -P) || fail 'Could not find the Builder folder.'
home=$(cd -P -- "$HOME" && pwd -P) || fail 'Could not find your home folder.'
case "$root" in
    /|"$home") fail "The Builder needs a folder of its own, not $root. Download it again and choose a folder such as ~/makepad-commercial." ;;
esac

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64 | Darwin/aarch64) triple=aarch64-apple-darwin ;;
    Darwin/x86_64)                 triple=x86_64-apple-darwin ;;
    Linux/x86_64)                  triple=x86_64-unknown-linux-gnu ;;
    Linux/aarch64 | Linux/arm64)   triple=aarch64-unknown-linux-gnu ;;
    *) fail 'The Builder supports macOS, and glibc Linux on x86_64 and ARM64.' ;;
esac

export MAKEPAD_LOADER_ROOT="$root"
private_rust="$root/toolchain/rust/$builder_rust-$triple"
builder_stamp="$builder_commit $builder_rust $triple"

# --- Small helpers ----------------------------------------------------------

sha256() {
    if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | awk '{ print $1 }'
    else shasum -a 256 "$1" | awk '{ print $1 }'; fi
}

# download URL FILE: HTTPS only, no redirects, into FILE.part, then renamed
# to FILE. A failed or interrupted download leaves only FILE.part behind,
# which the next attempt overwrites.
download() {
    curl --fail --show-error --progress-bar --proto '=https' --tlsv1.2 \
        --output "$2.part" "$1" || fail "Download failed: $1"
    mv -f "$2.part" "$2"
}

# The recorded compiler choice: "private", the sysroot of your own Rust, or
# nothing yet. Delete selected-rust to be asked again.
recorded_rust() { cat "$root/selected-rust" 2>/dev/null || :; }
record_rust() {
    printf '%s\n' "$1" > "$root/selected-rust.new"
    mv -f "$root/selected-rust.new" "$root/selected-rust"
}

# rust_check --probe | --validate SYSROOT: prints a usable sysroot, or
# returns non-zero with the reason in $rust_reason. Read-only.
rust_check() {
    rust_reason=
    if rust_sysroot=$(sh "$root/rust-tools.sh" "$1" "$builder_rust" "$triple" ${2:+"$2"} 2>/dev/null); then
        return 0
    fi
    rust_reason=$rust_sysroot
    rust_sysroot=
    return 1
}

private_rust_ready() {
    [ "$(cat "$private_rust/.toolchain-version" 2>/dev/null || :)" = "$builder_rust $triple" ] &&
        [ -x "$private_rust/bin/rustc" ] && [ -x "$private_rust/bin/cargo" ]
}

# macOS: rust-objcopy (used to strip release builds) finds LLVM beside
# rustlib's host tools. A relative link keeps the folder movable.
prepare_host_tools() {
    case "$triple" in
        *-apple-darwin)
            host_lib="$1/lib/rustlib/$triple/lib"
            if [ -f "$1/lib/libLLVM.dylib" ] && [ ! -e "$host_lib/libLLVM.dylib" ]; then
                mkdir -p "$host_lib"
                ln -s ../../../libLLVM.dylib "$host_lib/libLLVM.dylib"
            fi ;;
    esac
}

start_builder() {
    prepare_host_tools "$1"
    exec "$root/makepad-builder" tui
}

# --- Fast path: everything is already here ----------------------------------

if [ -x "$root/makepad-builder" ] && [ "$(cat "$root/.builder-version" 2>/dev/null || :)" = "$builder_stamp" ]; then
    choice=$(recorded_rust)
    case "$choice" in
        private) private_rust_ready && start_builder "$private_rust" ;;
        /*) rust_check --validate "$choice" && start_builder "$rust_sysroot" ;;
    esac
fi

# --- 1. System build tools --------------------------------------------------

step '1/4  System build tools'
sh "$root/check-tools.sh"
mkdir -p "$root/cache" "$root/toolchain/rust" "$root/cargo-home" "$root/target" "$root/tmp"

# --- 2. Rust ----------------------------------------------------------------

step '2/4  Rust'
choice=$(recorded_rust)
case "$choice" in
    /*)
        if ! rust_check --validate "$choice"; then
            say "Your Rust at $choice can no longer be used: $rust_reason"
            say 'Using a private Rust in this folder instead.'
            choice=
        fi ;;
esac
if [ -z "$choice" ] && rust_check --probe; then
    say "Found $("$rust_sysroot/bin/rustc" --version)."
    say 'Use it, or a private Rust in this folder that keeps your system clean?'
    printf '  Type system, or press Return for private: '
    IFS= read -r answer || answer=
    case "$answer" in
        s | system) record_rust "$rust_sysroot" ; choice=$rust_sysroot ;;
        *)          record_rust private ;;
    esac
fi
[ -n "$choice" ] || record_rust private

if [ "$(recorded_rust)" = private ]; then
    if ! private_rust_ready; then
        say "Installing a private Rust $builder_rust into this folder (about 70 MB from static.rust-lang.org)."
        say 'Rust licenses: https://www.rust-lang.org/policies/licenses'
        printf '  Press Return to continue, or Ctrl+C to stop. '
        IFS= read -r answer || fail 'Stopped. Nothing was installed.'

        # The three components are unpacked into a staging folder, checked,
        # then renamed into place. A stopped run leaves the staging folder,
        # which the next run unpacks over.
        manifest="$root/cache/channel-rust-$builder_rust.toml"
        download "https://static.rust-lang.org/dist/channel-rust-$builder_rust.toml" "$manifest"
        staging="$root/toolchain/rust/.staging-$builder_rust-$triple"
        mkdir -p "$staging"
        for component in rustc rust-std cargo; do
            section="[pkg.$component.target.$triple]"
            field() {
                awk -v section="$section" -v key="$1" '
                    $0 == section { inside = 1; next }
                    /^\[/         { inside = 0 }
                    inside && $1 == key && $2 == "=" { gsub(/"/, "", $3); print $3; exit }
                ' "$manifest"
            }
            url=$(field url)
            hash=$(field hash)
            case "$url" in https://static.rust-lang.org/dist/*.tar.gz) ;; *) fail "Unexpected Rust download address for $component." ;; esac
            case "$hash" in *[!0-9a-f]* | '') fail "Invalid Rust checksum for $component." ;; esac
            [ "${#hash}" = 64 ] || fail "Invalid Rust checksum for $component."

            archive="$root/cache/$hash.tar.gz"
            if [ ! -f "$archive" ] || [ "$(sha256 "$archive")" != "$hash" ]; then
                say "Downloading $component"
                download "$url" "$archive"
                [ "$(sha256 "$archive")" = "$hash" ] || fail "The $component download does not match its checksum."
            fi
            # An official archive holds <name>/<component>/{bin,lib,...};
            # unpack only that inner folder.
            top=$(basename "$url" .tar.gz)
            case "$component" in rust-std) inner=rust-std-$triple ;; *) inner=$component ;; esac
            say "Unpacking $component"
            tar -xzf "$archive" -C "$staging" --strip-components=2 "$top/$inner"
        done
        prepare_host_tools "$staging"
        case "$("$staging/bin/rustc" --version)" in
            "rustc $builder_rust "*) ;;
            *) fail 'The private Rust did not pass its version check.' ;;
        esac
        "$staging/bin/cargo" --version >/dev/null
        printf '%s' "$builder_rust $triple" > "$staging/.toolchain-version"
        [ ! -e "$private_rust" ] || fail "An unfinished toolchain is in the way: $private_rust. Move it aside and run the Builder again."
        mv "$staging" "$private_rust"
    fi
    rust_sysroot=$private_rust
    say "Private Rust $builder_rust is ready."
else
    rust_sysroot=$(recorded_rust)
    say "Using your Rust at $rust_sysroot."
fi
prepare_host_tools "$rust_sysroot"

# --- 3. Makepad source ------------------------------------------------------

step '3/4  Makepad source'
sources="$root/sources/$builder_release"
source_dir="$sources/makepad"
receipt_file="$sources/.builder-repositories/makepad"
mkdir -p "$sources/.builder-repositories" "$root/available"
if [ ! -d "$source_dir" ]; then
    # One commit from our verified pack, checked out in a staging folder and
    # renamed into place once git has verified it.
    pack="$root/cache/makepad-$builder_release.pack"
    say 'Downloading the public Makepad source'
    download "https://makepad.nl/api/loader/public/source/$builder_release/makepad.pack" "$pack"
    [ "$(sha256 "$pack")" = "${builder_receipt#* }" ] || fail 'The Makepad source does not match its checksum.'
    staging="$sources/.staging-makepad"
    git init -q "$staging"
    git -C "$staging" config remote.origin.url https://github.com/makepad/makepad.git
    git -C "$staging" index-pack --stdin < "$pack" >/dev/null
    printf '%s\n' "$builder_commit" > "$staging/.git/shallow"
    git -C "$staging" -c advice.detachedHead=false checkout -q -f --detach "$builder_commit"
    [ "$(git -C "$staging" rev-parse HEAD)" = "$builder_commit" ] || fail 'The Makepad source is not the expected commit.'
    git -C "$staging" fsck --no-reflogs --no-progress >/dev/null
    mv "$staging" "$source_dir"
    printf '%s' "$builder_receipt" > "$receipt_file"
elif [ "$(cat "$receipt_file" 2>/dev/null || :)" != "$builder_receipt" ]; then
    fail "$source_dir was not made by this Builder and is left as it is. Choose another folder."
fi
printf '%s\n' @RELEASE_SHELL@ > "$root/available/@MANIFEST_APP@.json"
say 'Source is ready.'

# --- 4. Compile the Builder -------------------------------------------------

step '4/4  Builder'
# The chosen compiler comes first on PATH; caches, build output and temp
# files stay in this folder. No rustup proxy can install or switch anything.
export PATH="$rust_sysroot/bin:$PATH"
export CARGO_HOME="$root/cargo-home"
export CARGO_TARGET_DIR="$root/target"
export RUSTUP_HOME="$root/rustup-home"
export RUSTUP_AUTO_INSTALL=0
export RUSTC="$rust_sysroot/bin/rustc"
export TMPDIR="$root/tmp"
export MAKEPAD_GGML_NO_CUDA=1
unset RUSTUP_TOOLCHAIN RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_ENCODED_RUSTFLAGS RUSTFLAGS RUSTDOC
if [ -x "$rust_sysroot/bin/rustdoc" ]; then export RUSTDOC="$rust_sysroot/bin/rustdoc"; fi

if [ "$(cat "$root/.builder-version" 2>/dev/null || :)" = "$builder_stamp" ] && [ -x "$root/makepad-builder" ]; then
    say 'Already compiled for this release.'
else
    # One counting line instead of cargo's crate list; the full output is in
    # build.log.
    say 'Compiling (about a minute; the full log is in build.log)'
    (
        cd "$source_dir"
        status=0
        cargo build --release -p makepad-loader --no-default-features --bin makepad-builder-cli \
            > "$root/build.log" 2>&1 || status=$?
        exit "$status"
    ) &
    compile=$!
    while kill -0 "$compile" 2>/dev/null; do
        crates=$(grep -c '^ *Compiling ' "$root/build.log" 2>/dev/null) || :
        printf '\r  %s crates compiled' "${crates:-0}"
        sleep 1
    done
    printf '\n'
    wait "$compile" || fail "Compiling failed; see $root/build.log"
    cp "$root/target/release/makepad-builder-cli" "$root/makepad-builder.next"
    chmod 700 "$root/makepad-builder.next"
    mv -f "$root/makepad-builder.next" "$root/makepad-builder"
    printf '%s' "$builder_stamp" > "$root/.builder-version"
fi

say "Ready. Start it again any time with $root/run-builder.sh"
exec "$root/makepad-builder" tui
