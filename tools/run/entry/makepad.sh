#!/bin/sh
# Makepad for macOS and Linux: gets Rust, compiles Makepad's loader (makepad run) from its source on
# this computer and starts the runner, with one compact JIT build progress line on a terminal
# (plain progress lines every ten seconds otherwise). Cargo's full output stays in the build log.
# Read it before you run it; it is this one file. Rust stays in its own folder; no system install.
#
#   curl -fsSL https://makepad.nl/runner/unix | sh
#
# 1. Use this folder's Rust if ready, else a suitable installed stable Rust (1.98.0 or newer).
#    Otherwise get Rust 1.98.0 (rustc, cargo, rust-std) from static.rust-lang.org, fetched as
#    25 MB pieces over parallel connections, checked against the sizes and SHA-256 sums below.
# 2. At the same time, the loader's source: a ZIP the service's latest licence-free `runner`
#    release names (size and SHA-256 checked before unpacking), cut from the same Makepad commit as
#    the runner, so the two always fit; and, with no arguments, the packs of the runner's git
#    repositories, which makepad run takes instead of downloading them itself.
# 3. The loader and its tiny app launcher are compiled for this computer's processor (`cargo build --profile host --offline`:
#    the ZIP holds every crate) the moment Rust and the source are in place, and started with no
#    arguments: it runs the runner, which downloads the rest. With arguments, the loader runs with
#    them instead. The loader ZIP's SHA-256 keys .version and, after a successful build,
#    .loader-sha256; .built also records Rust and the target triple. An unchanged ZIP reuses the
#    source; matching SHA and Rust stamps reuse the binary. A changed ZIP replaces only the files
#    that changed, so only their crates compile again. Under cache/loader, current selects src or
#    a commit folder; pending rolls back to previous if the new runner did not confirm its boot.
#
# In its folder it creates rust/ (the toolchain), cache/ (downloads, the loader, its caches and
# logs), apps/ (what `makepad` and `makepad app <id>` run: the runner's and each app's launch
# record), runner/ (the runner's own state: licence, catalog, launch logs) and target/ (what makepad
# run builds for the apps: build scripts, unit graphs, code cache). The folder is this
# file's own when it holds nothing else (or is an installation already), else ~/makepad-runner
# (piped into sh, or saved among other downloads); $MAKEPAD_LOADER_ROOT overrides. Run it again to
# update and start, reusing downloads and compiled crates. The system needs a C linker (Apple's command line tools; on
# Linux cc) and, on Linux, OpenSSL 3's development files; it names the packages when missing.
set -u
umask 022

rust_version=1.98.0
rust_date=2026-08-20
dist=https://static.rust-lang.org/dist/$rust_date
piece=25000000
service=${MAKEPAD_LOADER_SERVICE:-https://makepad.nl/api/loader}
service=${service%/}

# --- Progress and jobs --------------------------------------------------------------------
logs=
jobs=

say() { printf '  %s\n' "$1"; }
now() { date +%s; }
# stamp NAME appends "NAME HH:MM:SS.cc" to the timing log
stamp() {
    [ -n "$logs" ] && [ -d "$logs" ] || return 0
    t=$(date +%H:%M:%S.%N)
    case "$t" in *N) t=${t%.N} ;; *) t=$(printf '%s' "$t" | cut -c1-11) ;; esac
    printf '%s %s\n' "$1" "$t" >> "$logs/bootstrap.log"
}
# the background jobs this script started (by PID), stopped when it fails or is stopped
stop_jobs() {
    [ -n "$jobs" ] && [ -d "$jobs" ] || return 0
    for job in fetch build unpack-rustc unpack-rust-std unpack-cargo; do
        [ -f "$jobs/$job.pid" ] && [ ! -f "$jobs/$job.done" ] && kill "$(cat "$jobs/$job.pid")" 2>/dev/null
    done
    return 0
}
# fail MESSAGE [BUILD_LOG]: error and optional build-log excerpt on stderr, exit 1
fail() {
    stamp failed
    stop_jobs
    printf '  Error: %s\n' "$1" >&2
    if [ $# -gt 1 ]; then
        if grep -q '^error' "$2"; then
            grep -n -B1 -A8 '^error' "$2" | tail -20 >&2
        else
            tail -20 "$2" >&2
        fi
        printf '  Full log: %s\n' "$2" >&2
    fi
    exit 1
}
trap 'stamp stopped; stop_jobs; exit 130' INT TERM
# job NAME COMMAND...: runs COMMAND in the background; its PID in jobs/NAME.pid, its exit code
# in jobs/NAME.done when it ends
job() {
    name=$1
    shift
    rm -f "$jobs/$name.pid" "$jobs/$name.done"
    (
        "$@" &
        echo $! > "$jobs/$name.pid"
        wait $!
        echo $? > "$jobs/$name.code"
        mv -f "$jobs/$name.code" "$jobs/$name.done"
    ) &
    while [ ! -f "$jobs/$name.pid" ]; do sleep 0.1 2>/dev/null || sleep 1; done
}
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
size_of() { wc -c < "$1" | tr -d ' '; }
# a path inside a double-quoted curl config value
cfgq() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }
# One-line pointers and stamps are published only after the whole line is written.
atomic_line() { printf '%s\n' "$2" > "$1.part" && mv -f "$1.part" "$1"; }
loader_folder_ready() {
    case "$1" in
        src) ;;
        *[!0123456789abcdef]*) return 1 ;;
        *) [ "${#1}" -eq 40 ] || return 1 ;;
    esac
    [ -f "$loader_root/$1/target/host/makepad" ]
}

# rust_check --probe MIN: read-only. Prints nothing; sets rust_sysroot, or
# rust_reason. Only the toolchain's own binaries run, so no rustup proxy can
# download or switch anything.
rust_check() {
    rust_reason= rust_sysroot=
    rc_toolchain=$(cd / && unset RUSTUP_TOOLCHAIN && RUSTUP_AUTO_INSTALL=0 rustc --print sysroot 2>/dev/null) || { rust_reason='No installed Rust was found on PATH.'; return 1; }
    [ -n "$rc_toolchain" ] || { rust_reason='No installed Rust was found on PATH.'; return 1; }
    rc_min=$2
    case "$rc_toolchain" in /*) ;; *) rust_reason='The recorded Rust location is not an absolute path.'; return 1 ;; esac
    [ -d "$rc_toolchain" ] || { rust_reason="Rust is no longer installed at $rc_toolchain."; return 1; }
    rc_toolchain=$(cd -P -- "$rc_toolchain" && pwd)
    [ -x "$rc_toolchain/bin/rustc" ] || { rust_reason="No rustc in $rc_toolchain/bin."; return 1; }
    rc_meta=$("$rc_toolchain/bin/rustc" --version --verbose 2>/dev/null) || { rust_reason="rustc in $rc_toolchain/bin does not run."; return 1; }
    rc_version=$(printf '%s\n' "$rc_meta" | awk '$1 == "release:" { print $2 }')
    rc_host=$(printf '%s\n' "$rc_meta" | awk '$1 == "host:" { print $2 }')
    [ "$rc_host" = "$triple" ] || { rust_reason="Installed Rust is built for ${rc_host:-an unknown host}; Makepad needs $triple."; return 1; }
    rc_status=0
    version_at_least "$rc_version" "$rc_min" || rc_status=$?
    case "$rc_status" in
        0) ;;
        1) rust_reason="Installed Rust $rc_version is not a stable release; Makepad needs $rc_min or newer."; return 1 ;;
        *) rust_reason="Installed Rust $rc_version is older than $rc_min."; return 1 ;;
    esac
    [ -x "$rc_toolchain/bin/cargo" ] || { rust_reason='Installed Rust has no cargo beside rustc.'; return 1; }
    rc_cargo=$("$rc_toolchain/bin/cargo" --version 2>/dev/null | awk '$1 == "cargo" { print $2 }') || :
    version_at_least "${rc_cargo:-none}" "$rc_min" || { rust_reason="Installed cargo ${rc_cargo:-is unusable and} is older than $rc_min."; return 1; }
    [ -d "$rc_toolchain/lib/rustlib/$triple/lib" ] || { rust_reason="Installed Rust has no standard library for $triple."; return 1; }
    if [ "$(uname -s)" = Darwin ]; then
        # Release builds strip through rustc's bundled rust-objcopy; report a
        # copy that cannot load its LLVM library, never repair it.
        "$rc_toolchain/lib/rustlib/$triple/bin/rust-objcopy" --version >/dev/null 2>&1 || { rust_reason="Installed Rust's rust-objcopy does not run; release builds could not be stripped."; return 1; }
    fi
    rust_sysroot=$rc_toolchain
}
# version_at_least ACTUAL MINIMUM: 0 when newer or equal, 1 when not a
# stable x.y.z, 2 when older. Components compare as numbers.
version_at_least() {
    awk -v actual="$1" -v minimum="$2" 'BEGIN {
        if (actual !~ /^[0-9]+[.][0-9]+[.][0-9]+$/) exit 1
        split(actual, a, "."); split(minimum, b, ".")
        for (i = 1; i <= 3; i++) { if (a[i] + 0 > b[i] + 0) exit 0; if (a[i] + 0 < b[i] + 0) exit 2 }
        exit 0
    }'
}

# --- Where, and what this computer has ------------------------------------------------------
case "$0" in
    sh | -sh | */sh | bash | -bash | */bash | dash | */dash | zsh | */zsh) script_dir= ;;
    *) script_dir=; [ -f "$0" ] && script_dir=$(cd -P -- "$(dirname -- "$0")" && pwd -P) ;;
esac
# The folder: this file's own when it is an installation already, or holds nothing else (a folder
# made for it); else ~/makepad-runner (piped into sh, or a download among other files), where the
# file copies itself.
if [ -n "$script_dir" ] && [ -z "${MAKEPAD_LOADER_ROOT:-}" ] && [ ! -d "$script_dir/cache" ] \
    && [ -n "$(ls -A "$script_dir" | grep -v -x -e "$(basename -- "$0")" -e .DS_Store)" ]; then
    script_dir_other=1
fi
if [ -n "$script_dir" ] && [ -z "${script_dir_other:-}" ]; then default_root=$script_dir; else default_root=$HOME/makepad-runner; fi
root=${MAKEPAD_LOADER_ROOT:-$default_root}
mkdir -p "$root" 2>/dev/null && root=$(cd -P -- "$root" && pwd -P) || fail "Cannot create the folder $root"
rust_own=$root/rust
rust=$rust_own
cache=$root/cache
loader_root=$cache/loader
downloads=$cache/downloads
logs_dir=$cache/logs
jobs=$cache/jobs

case "$(uname -s)/$(uname -m)" in
    Darwin/arm64 | Darwin/aarch64) triple=aarch64-apple-darwin ;;
    Darwin/x86_64) triple=x86_64-apple-darwin ;;
    Linux/x86_64) triple=x86_64-unknown-linux-gnu ;;
    Linux/aarch64 | Linux/arm64) triple=aarch64-unknown-linux-gnu ;;
    *) fail "Makepad supports macOS, and glibc Linux on x86_64 and ARM64." ;;
esac
# "<part> <bytes> <sha256>" for rustc, rust-std and cargo of this platform
case "$triple" in
    aarch64-apple-darwin) parts='rustc 83206125 7b39cbbb11995584741b5821511b8706ab8ecab3bc7e12b11fb1ab50cab9d079
rust-std 46977544 49dc82ff5b6fb033baa6e85635ed9cf07a27e5297a22314808ab4cf65a81335a
cargo 12993393 ae556d0783f4ab2a6fccd4674158df08cf36526b73de03b7fde0e10b2bfc3d2e' ;;
    x86_64-apple-darwin) parts='rustc 102807421 a0cf817df78615a10f4d0d29dbaafd657027e1a3665be728d0f19e341c8671bb
rust-std 47585228 57a2c40ea919c06c4cd9ff4fc8b26e309c00c112538ea522227251e8ccc1f931
cargo 13345290 7fdd094e131f98864fea06c6e0059fcecaba7b0010d07b90023f42215b6f33e8' ;;
    x86_64-unknown-linux-gnu) parts='rustc 134061173 18ed6559de1b8ea6b77474ea86992b9a507d3a3d134d9ee017d30cf3f406e3ee
rust-std 48777886 8aa6405356392ce50160d1b286e86091c5e14adae3061115699c84ed4394d546
cargo 15858529 18bf1598891b30dd5eb52a337d08a92b4456255ddbe4c1ab996ffb578077031c' ;;
    aarch64-unknown-linux-gnu) parts='rustc 102526949 b6fdb37bc08e320bd92381e13d7311837f04a98423b8a5b678a5eb5e87c978ca
rust-std 48592329 a2eece726a579de1f554a2c3bcc62410226b0be55736f57c2b5430c1bc71b98f
cargo 15833225 e271c3c5d50336259a166e68b60ba792a64278ab117686183be3e51a0958d34b' ;;
esac
part_names='rustc rust-std cargo'
part_field() { printf '%s\n' "$parts" | awk -v p="$1" -v f="$2" '$1 == p { print $f }'; }

# https only, except a release service on this machine (tests, a local service)
case "$service" in
    https://*) proto='=https' ;;
    http://127.0.0.1:* | http://localhost:*) proto='=http,https' ;;
    # MAKEPAD_ALLOW_HTTP=1 (tests only): a plain-http release service on another machine
    http://*) [ "${MAKEPAD_ALLOW_HTTP:-}" = 1 ] && proto='=http,https' || fail "The release service must be an https URL - $service" ;;
    *) fail "The release service must be an https URL - $service" ;;
esac
command -v curl >/dev/null 2>&1 || fail "Makepad needs curl - install it, then run this again"
command -v tar >/dev/null 2>&1 || fail "Makepad needs tar - install it, then run this again"

# The system's linker (and on Linux OpenSSL's development files): named, never installed here.
if [ "$(uname -s)" = Darwin ]; then
    if ! xcode-select -p >/dev/null 2>&1 || ! /usr/bin/xcrun --find ld >/dev/null 2>&1; then
        fail "Makepad needs Apple's command line tools - install them, then run this again"
    fi
else
    missing=
    command -v cc >/dev/null 2>&1 || missing="a C linker (cc)"
    if ! { [ -e /usr/lib/libssl.so ] || [ -e /usr/lib64/libssl.so ] || ls /usr/lib/*-linux-gnu/libssl.so >/dev/null 2>&1; }; then
        missing="${missing:+$missing and }OpenSSL's development files (libssl.so)"
    fi
    if [ -n "$missing" ]; then
        if command -v apt-get >/dev/null 2>&1; then cmd='sudo apt-get install build-essential libssl-dev'
        elif command -v dnf >/dev/null 2>&1; then cmd='sudo dnf install gcc openssl-devel'
        elif command -v pacman >/dev/null 2>&1; then cmd='sudo pacman -S --needed base-devel openssl'
        elif command -v zypper >/dev/null 2>&1; then cmd='sudo zypper install gcc libopenssl-devel'
        else cmd='a C compiler/linker and the OpenSSL 3 development package'; fi
        fail "Makepad needs $missing - install it with: $cmd"
    fi
fi
mkdir -p "$downloads" "$logs_dir" "$jobs" "$loader_root" || fail "Cannot create $cache"
logs=$logs_dir
stamp start
export MAKEPAD_RUN_HOME="$root"

# Restore an unconfirmed switch before any decision about the loader already here.
if [ -e "$loader_root/pending" ]; then
    previous=$(cat "$loader_root/previous" 2>/dev/null)
    loader_folder_ready "$previous" || previous=src
    if [ "$previous" = src ]; then
        rm -f "$loader_root/current" || fail "Cannot remove $loader_root/current"
    else
        atomic_line "$loader_root/current" "$previous" || fail "Cannot write $loader_root/current"
    fi
    atomic_line "$loader_root/rolled-back" 'the new runner did not confirm its first start' \
        || fail "Cannot write $loader_root/rolled-back"
    rm -f "$loader_root/pending" || fail "Cannot remove $loader_root/pending"
    say "JIT: went back to $previous - the new runner did not confirm its first start"
fi
active=$(cat "$loader_root/current" 2>/dev/null)
loader_folder_ready "$active" || active=src
src=$loader_root/$active
makepad=$src/target/host/makepad

# --- The runner release, from the start ---------------------------------------------------------
# The release service's latest licence-free `runner` release names the runner's git repositories
# on the service's read-only git face and the loader's source cut from the same Makepad commit.
# With no arguments the folder moves to it: each commit's pack goes into
# $MAKEPAD_RUN_HOME/cache/incoming/<commit>.pack (renamed into place when complete), where makepad run
# takes it instead of downloading, and the loader is compiled for it. With arguments a loader
# compiled before stays as it is (the runner and the apps it recorded were made for it).
prefetch_pids=
runner_release=
repos=
release_json=
# With a loader and a runner here already, the release service gets a few seconds: offline or
# unreachable, the folder's stores and loader start at once (makepad run runs the stored commits).
meta="--retry 2 -m 60"
if [ -f "$src/.built" ] && [ -f "$cache/runner/release" ]; then meta="--connect-timeout 3 -m 6"; fi
mkdir -p "$cache/runner" "$cache/incoming"
if [ $# -eq 0 ] || [ ! -f "$src/.built" ]; then
    release_json=$(curl -fsS --proto "$proto" $meta "$service/public/app/runner/latest" 2>>"$logs/download.log") || release_json=
fi
runner_release=$(printf '%s' "$release_json" | sed -n 's/.*"release":"\([A-Za-z0-9_-]*\)".*/\1/p')
if [ $# -eq 0 ] && [ -n "$runner_release" ]; then
    # one "<name> <path> <commit>" line per repository of the release
    repos=$(printf '%s\n' "$release_json" | sed 's/"loader":{[^}]*}//' | tr '{}' '\n\n' | while IFS= read -r line; do
        name=$(printf '%s' "$line" | sed -n 's/.*"name":"\([A-Za-z0-9_-]*\)".*/\1/p')
        path=$(printf '%s' "$line" | sed -n 's/.*"path":"\([A-Za-z0-9_/-]*\)".*/\1/p')
        commit=$(printf '%s' "$line" | sed -n 's/.*"commit":"\([0-9a-f]\{40\}\)".*/\1/p')
        if [ -n "$name" ] && [ -n "$path" ]; then printf '%s %s %s\n' "$name" "$path" "$commit"; fi
    done)
    if [ "$(sed -n 1p "$cache/runner/release" 2>/dev/null)" != "$runner_release" ]; then
        for entry in $(printf '%s\n' "$repos" | tr ' ' ':'); do
            name=${entry%%:*}
            commit=${entry##*:}
            [ ${#commit} -eq 40 ] || continue
            pack=$cache/incoming/$commit.pack
            [ -f "$pack" ] && continue
            ( printf '0032want %s\n00000009done\n' "$commit" \
                | curl -fsS --proto "$proto" --retry 2 -m 1800 -H 'Content-Type: application/x-git-upload-pack-request' \
                    --data-binary @- -o "$pack.part" "$service/public/git/runner/$name.git/git-upload-pack" \
                && mv -f "$pack.part" "$pack" ) >/dev/null 2>>"$logs/download.log" &
            prefetch_pids="$prefetch_pids $!"
        done
    fi
fi

# --- What there is to do --------------------------------------------------------------------
need_rust=
rust_release=$rust_version
if [ "$(cat "$rust_own/.toolchain-version" 2>/dev/null)" = "$rust_version $triple" ] \
    && [ -x "$rust_own/bin/cargo" ] && [ -x "$rust_own/bin/rustc" ]; then
    say "Rust: ready"
elif rust_check --probe "$rust_version"; then
    rust=$rust_sysroot
    rust_release=$rc_version
    say "Rust $rust_release: using the one installed at $rust"
else
    need_rust=1
    [ "$rust_reason" = 'No installed Rust was found on PATH.' ] \
        || say "Rust: $rust_reason Downloading $rust_version into this folder"
fi
# The loader ZIP's SHA-256 names the source version, independent of the runner's commit.
# Offline, or with arguments, use the version recorded with the source already here.
archive=$(printf '%s' "$release_json" | sed -n 's/.*"loader":{"name":"\([A-Za-z0-9_.-]*\)","bytes":\([0-9]*\),"sha256":"\([0-9a-f]\{64\}\)"[^}]*}.*/\1 \2 \3/p')
loader_version=$(printf '%s' "$archive" | cut -s -d' ' -f3)
# Update the active source folder in place, replacing only changed files so cargo recompiles
# only their crates (target/ stays inside it: makepad finds its std two levels above itself).
loader_built_sha256=$(cat "$src/.loader-sha256" 2>/dev/null)
[ -n "$loader_version" ] || loader_version=$loader_built_sha256
[ -n "$loader_version" ] || loader_version=$(cat "$src/.version" 2>/dev/null)
[ -n "$loader_version" ] || fail "Could not reach the release service ($service) and no loader is here yet. Check the connection, then run this again."
built="$loader_version $rust_release $triple"
need_build=
[ -x "$makepad" ] && [ -x "$src/target/host/makepad-launcher" ] && [ "$loader_built_sha256" = "$loader_version" ] \
    && [ "$(awk 'NR == 1 { print $2, $3 }' "$src/.built" 2>/dev/null)" = "$rust_release $triple" ] || need_build=1
need_zip=
[ "$(cat "$src/.version" 2>/dev/null)" != "$loader_version" ] \
    && [ "$loader_built_sha256" != "$loader_version" ] && need_zip=1
if [ -n "$need_zip" ]; then
    [ -n "$archive" ] || fail "The release service did not answer; the loader's source is not here yet. Check the connection, then run this again."
    loader_name=$(printf '%s' "$archive" | cut -d' ' -f1)
    loader_bytes=$(printf '%s' "$archive" | cut -d' ' -f2)
    loader_sha256=$(printf '%s' "$archive" | cut -d' ' -f3)
    zip=$downloads/$loader_name
fi
[ -n "$need_zip" ] || say "Loader source: ready"

# --- 1 + 2. Rust's pieces and the loader's source, all at once ------------------------------
cfg=$downloads/fetch.curl
rm -f "$cfg"
rust_total=0
rust_base=0
# pieces PART SIZE adds the pieces of PART not yet here to the curl config
pieces() {
    idx=0 from=0
    while [ $from -lt "$2" ]; do
        to=$((from + piece - 1))
        [ $to -lt "$2" ] || to=$(($2 - 1))
        file=$downloads/$1.tar.gz.$idx
        have=0
        [ -f "$file" ] && have=$(size_of "$file")
        if [ "$have" != $((to - from + 1)) ]; then
            [ ! -f "$cfg" ] || echo next >> "$cfg"
            printf 'url = "%s"\noutput = "%s"\nrange = %s-%s\nproto = =https\nfail\nlocation\nretry = 3\n' \
                "$dist/$1-$rust_version-$triple.tar.gz" "$(cfgq "$file")" $from $to >> "$cfg"
        fi
        idx=$((idx + 1))
        from=$((to + 1))
    done
}
if [ -n "$need_rust" ]; then
    for part in $part_names; do
        size=$(part_field $part 2)
        rust_total=$((rust_total + size))
        if [ -f "$downloads/$part.tar.gz" ] && [ "$(size_of "$downloads/$part.tar.gz")" = "$size" ]; then
            rust_base=$((rust_base + size))
        else
            rm -f "$downloads/$part.tar.gz"
            pieces $part "$size"
        fi
    done
fi
if [ -n "$need_zip" ]; then
    rm -f "$zip.part"
    [ ! -f "$cfg" ] || echo next >> "$cfg"
    printf 'url = "%s"\noutput = "%s"\nproto = %s\nfail\nretry = 3\n' \
        "$service/public/app/runner/$runner_release/$loader_name" "$(cfgq "$zip.part")" "$proto" >> "$cfg"
fi
# the summed size of the Rust pieces here
pieces_sum() { ls -ln "$downloads" | awk '$9 ~ /\.tar\.gz\.[0-9]+$/ { s += $5 } END { print s + 0 }'; }
if [ -f "$cfg" ]; then
    [ -z "$need_rust" ] || say "Rust $rust_version: downloading (once, into this folder)"
    [ -z "$need_zip" ] || say "Loader source: downloading"
    job fetch curl -sS -Z --parallel-immediate --parallel-max 16 -K "$cfg" 2>"$logs/download.log"
    fetch_ticks=0
    while [ ! -f "$jobs/fetch.done" ]; do
        sleep 1
        fetch_ticks=$((fetch_ticks + 1))
        [ "$fetch_ticks" -ge 10 ] || continue
        fetch_ticks=0
        [ ! -f "$jobs/fetch.done" ] || break
        progress=
        if [ -n "$need_rust" ]; then
            rsum=$((rust_base + $(pieces_sum)))
            pct=$((rsum * 100 / rust_total))
            [ $pct -le 100 ] || pct=100
            progress="Rust: $pct% ($((rsum / 1048576)) / $((rust_total / 1048576)) MB)"
        fi
        if [ -n "$need_zip" ]; then
            zsum=0
            [ ! -f "$zip.part" ] || zsum=$(size_of "$zip.part")
            progress="${progress:+$progress. }Loader source: $((zsum / 1024)) / $((loader_bytes / 1024)) KB"
        fi
        say "$progress"
    done
    [ "$(cat "$jobs/fetch.done")" = 0 ] || fail "The download stopped. Run makepad.sh again to continue it - log: $logs/download.log"
    rm -f "$cfg"
    stamp downloaded
fi

# --- Rust: join the pieces, check, unpack the three parts side by side ----------------------
if [ -n "$need_rust" ]; then
    for part in $part_names; do
        archive_file=$downloads/$part.tar.gz
        [ -f "$archive_file" ] && continue
        idx=0
        : > "$archive_file.joined" || fail "Could not join the pieces of $part."
        while [ -f "$archive_file.$idx" ]; do
            cat "$archive_file.$idx" >> "$archive_file.joined" || fail "Could not join the pieces of $part."
            idx=$((idx + 1))
        done
        idx=0
        while [ -f "$archive_file.$idx" ]; do rm -f "$archive_file.$idx"; idx=$((idx + 1)); done
        mv -f "$archive_file.joined" "$archive_file"
    done
    say "Rust: checking"
    for part in $part_names; do
        if [ "$(sha256 "$downloads/$part.tar.gz")" != "$(part_field $part 3)" ]; then
            rm -f "$downloads/$part.tar.gz"
            fail "Checksum mismatch for $part.tar.gz - downloaded again on the next run"
        fi
    done
    stage=$cache/rust.unpack
    rm -rf "$stage"
    mkdir -p "$stage" || fail "Cannot create $stage"
    say "Rust: unpacking"
    for part in $part_names; do
        top=$part-$rust_version-$triple
        inner=$part
        [ $part != rust-std ] || inner=rust-std-$triple
        mkdir "$stage/$part" || fail "Cannot create $stage/$part"
        job unpack-$part tar -xzf "$downloads/$part.tar.gz" -C "$stage/$part" --strip-components=2 "$top/$inner" 2>"$logs/unpack-$part.log"
    done
    while :; do
        unpacked=0
        for part in $part_names; do [ ! -f "$jobs/unpack-$part.done" ] || unpacked=$((unpacked + 1)); done
        [ $unpacked -lt 3 ] || break
        sleep 1
    done
    ready=$stage/ready
    mkdir "$ready" || fail "Cannot create $ready"
    for part in $part_names; do
        [ "$(cat "$jobs/unpack-$part.done")" = 0 ] || fail "Unpacking $part failed - log: $logs/unpack-$part.log"
        rm -f "$stage/$part/manifest.in"
        # the parts share bin/, lib/ and share/: move each file into place
        ( cd "$stage/$part" && find . -type d -print | while IFS= read -r dir; do
            mkdir -p "$ready/$dir" || exit 1
        done && find . ! -type d -print | while IFS= read -r file; do
            mv -f "$file" "$ready/$file" || exit 1
        done ) \
            2>>"$logs/unpack-$part.log" || fail "Unpacking $part failed - log: $logs/unpack-$part.log"
    done
    printf '%s %s' "$rust_version" "$triple" > "$ready/.toolchain-version"
    rm -rf "$rust_own"
    mv "$ready" "$rust_own" || fail "Cannot move Rust into $rust_own"
    rm -rf "$stage"
    for part in $part_names; do rm -f "$downloads/$part.tar.gz"; done
    say "Rust: ready"
    stamp rust
fi

# --- The loader's source: check and unpack --------------------------------------------------
if [ -n "$need_zip" ]; then
    say "Loader source: checking"
    if [ "$(size_of "$zip.part" 2>/dev/null)" != "$loader_bytes" ]; then
        rm -f "$zip.part"
        fail "The loader's source has the wrong size - downloaded again on the next run"
    fi
    if [ "$(sha256 "$zip.part")" != "$loader_sha256" ]; then
        rm -f "$zip.part"
        fail "Checksum mismatch for $loader_name - downloaded again on the next run"
    fi
    unpack=$cache/loader/.unpack
    rm -rf "$unpack"
    mkdir -p "$unpack" || fail "Cannot create $unpack"
    say "Loader source: unpacking"
    if command -v unzip >/dev/null 2>&1; then unzip -q "$zip.part" -d "$unpack" >>"$logs/download.log" 2>&1
    elif command -v bsdtar >/dev/null 2>&1; then bsdtar -xf "$zip.part" -C "$unpack" >>"$logs/download.log" 2>&1
    elif command -v python3 >/dev/null 2>&1; then python3 -m zipfile -e "$zip.part" "$unpack" >>"$logs/download.log" 2>&1
    else fail "Makepad needs unzip - install it, then run this again"
    fi || fail "Unpacking the loader's source failed. Run this again - log: $logs/download.log"
    new=$unpack/$runner_release
    [ -f "$new/Cargo.toml" ] || fail "The loader's source has no $runner_release/Cargo.toml. Run this again later; the release service is being updated."
    if [ ! -f "$src/Cargo.toml" ]; then
        rm -rf "$src"
        mv "$new" "$src" || fail "Cannot move the loader's source into $src"
    else
        # an update: files that changed are replaced (with the time of now, so cargo sees them
        # as changed), files the new version no longer has are removed, the rest stay as they are
        say "Loader source: updating - $(printf '%s' "$loader_version" | cut -c1-10)"
        # one `diff -rq` names what differs (one process, not one per file)
        LC_ALL=C diff -rq -x target -x .version -x .built -x .loader-sha256 "$new" "$src" > "$unpack/changes" 2>&1
        [ $? -le 1 ] || fail "Cannot compare the loader's source with $src - $(head -1 "$unpack/changes")"
        while IFS= read -r line; do
            case "$line" in
                "Files $new/"*" differ")
                    rel=${line#"Files $new/"}
                    rel=${rel%%" and $src/"*}
                    mv -f "$new/$rel" "$src/$rel" && touch "$src/$rel" || exit 1 ;;
                "Only in $new"*)
                    dir=${line#"Only in $new"}
                    dir=${dir%%": "*}
                    rel=${dir#/}${dir:+/}${line#*": "}
                    rel=${rel#/}
                    mkdir -p "$(dirname "$src/$rel")" && mv -f "$new/$rel" "$src/$rel" || exit 1
                    find "$src/$rel" -exec touch {} + ;;
                "Only in $src"*)
                    dir=${line#"Only in $src"}
                    dir=${dir%%": "*}
                    rel=${dir#/}${dir:+/}${line#*": "}
                    rm -rf "$src/${rel#/}" ;;
            esac
        done < "$unpack/changes" || fail "Cannot update the loader's source in $src"
    fi
    atomic_line "$src/.version" "$loader_version" || fail "Cannot write $src/.version"
    rm -rf "$unpack"
    rm -f "$zip.part"
    say "Loader source: ready"
    stamp source
fi

# --- 3. Compile the loader --------------------------------------------------------------------
# The compile sees no Rust settings from elsewhere on this computer and writes only into
# cache/. Cargo's plain output stays in the log. A terminal refreshes one line every second;
# other output gets a plain line at most every ten seconds, plus the final progress and timing.
unset RUSTUP_TOOLCHAIN RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_BUILD_RUSTFLAGS CARGO_TARGET_DIR RUSTC_WRAPPER RUSTDOC
export PATH="$rust/bin:$PATH" CARGO_HOME="$cache/cargo-home" RUSTC="$rust/bin/rustc" CARGO_INCREMENTAL=0
build_loader() {
    cd "$src" && CARGO_TERM_COLOR=never CARGO_TERM_PROGRESS_WHEN=never RUSTFLAGS=-Ctarget-cpu=native \
        exec "$rust/bin/cargo" build --profile host --offline $locked -p makepad-run --bin makepad --bin makepad-launcher
}
build_progress() {
    progress=$(awk -v total="$crates" '
        $1 == "Compiling" && $3 ~ /^v[0-9]/ { compiled++; current = $2 }
        END { printf "Building the JIT · %d/%d crates · %s", compiled, total, (current == "" ? "waiting" : current) }
    ' "$build_log")
    if [ -n "$build_tty" ]; then printf '\r\033[K  %s' "$progress"; else say "$progress"; fi
}
if [ -n "$need_build" ]; then
    build_log=$logs/build.log
    rm -f "$build_log"
    # the crates to compile: the lock file's packages (the archive holds only the loader's crates)
    crates=$(grep -c '^name = ' "$src/Cargo.lock" 2>/dev/null)
    [ "${crates:-0}" -gt 0 ] || crates=1
    # the archive's lock file (path crates only): nothing is resolved or fetched on this computer
    locked=
    [ ! -f "$src/Cargo.lock" ] || locked=--locked
    # room for the build (about 2 GB of target/), said before it starts rather than as a cargo error
    free_kb=$(df -Pk "$cache" | awk 'NR == 2 { print $4 }')
    [ "${free_kb:-0}" -ge 3000000 ] || fail "Compiling Makepad's loader needs about 3 GB free on this disk. Free some space, then run this again."
    build_tty=
    [ -t 1 ] && build_tty=1
    build_t0=$(now)
    # for this computer's processor ([profile.host]: thin LTO, 16 codegen units, target-cpu=native)
    job build build_loader >"$build_log" 2>&1
    build_ticks=0
    build_progress
    while [ ! -f "$jobs/build.done" ]; do
        sleep 1
        build_ticks=$((build_ticks + 1))
        [ ! -f "$jobs/build.done" ] || break
        [ -n "$build_tty" ] || [ "$build_ticks" -ge 10 ] || continue
        build_ticks=0
        build_progress
    done
    build_progress
    [ -z "$build_tty" ] || printf '\n'
    if [ "$(cat "$jobs/build.done")" != 0 ]; then
        fail "the JIT did not compile." "$build_log"
    fi
    atomic_line "$src/.built" "$built" || fail "Cannot write $src/.built"
    atomic_line "$src/.loader-sha256" "$loader_version" || fail "Cannot write $src/.loader-sha256"
    say "Building the JIT · compiled in $(($(now) - build_t0)) s"
    stamp built
else
    say "Building the JIT · up to date"
fi

# This script stays in the folder for the next start.
if [ -n "$script_dir" ] && [ "$script_dir" != "$root" ]; then
    cp "$0" "$root/makepad.sh.next" 2>/dev/null && chmod 755 "$root/makepad.sh.next" && mv -f "$root/makepad.sh.next" "$root/makepad.sh" || :
fi

# --- 4. Start it --------------------------------------------------------------------------------
# makepad run runs cargo/rustc for build scripts and unit graphs (until it does that itself), so the
# toolchain stays on PATH for it and every app it starts. With no arguments `makepad` runs the
# runner: its launch record names the runner's repositories, the release service's latest
# licence-free `runner` release, which the service serves read-only over git
# ($service/public/git/runner/<repository>.git, one branch per release). The repository at
# `makepad` is the workspace, the others mount at their folders inside it. makepad run keeps its
# stores in $MAKEPAD_RUN_HOME/cache and, with --cached, starts the last one at once while it
# fetches the new release for the next start. Offline, the release recorded last.
export MAKEPAD_LOADER_ROOT="$root" MAKEPAD_LOADER_SERVICE="$service"
if [ $# -gt 0 ]; then
    stamp start-loader
    exec "$makepad" "$@"
fi
# the runner's packs downloaded alongside the build are complete before makepad run looks for them
[ -z "$prefetch_pids" ] || wait $prefetch_pids || :
if [ -n "$runner_release" ]; then
    [ -n "$repos" ] || fail "The runner release names no repositories."
    { printf '%s\n' "$runner_release"; printf '%s\n' "$repos"; } > "$cache/runner/release.next" && mv -f "$cache/runner/release.next" "$cache/runner/release"
else
    [ -f "$cache/runner/release" ] || fail "Could not reach the release service ($service) and the runner was never fetched."
    say "The release service did not answer; starting the runner fetched before."
    runner_release=$(sed -n 1p "$cache/runner/release")
    repos=$(sed -n '2,$p' "$cache/runner/release")
fi
git_base=$service/public/git/runner
# The runner's launch record ($MAKEPAD_RUN_HOME/apps/runner.launch, one `key<TAB>value` line each):
# what `makepad` with no arguments runs, from here and from anywhere else (makepad launch.rs).
launch_dir=$root/apps
mkdir -p "$launch_dir" || fail "Cannot create $launch_dir"
record=$launch_dir/runner.launch
printf 'title\tMakepad\narg\trun\narg\t--quiet\narg\t--cached\n' > "$record.part" || fail "Cannot write $record"
rm -f "$record.mounts"
root_repo=
for entry in $(printf '%s\n' "$repos" | tr ' ' ':'); do
    name=${entry%%:*}
    path=${entry#*:}
    path=${path%%:*}
    # the release's commit pinned: makepad run takes the pack downloaded above (or its store) as it is,
    # without asking the service again
    commit=${entry##*:}
    pin=
    [ ${#commit} -ne 40 ] || pin=@$commit
    case "$path" in
        makepad) root_repo="$git_base/$name.git#$runner_release$pin" ;;
        makepad/*) printf 'arg\t--mount\narg\t%s\n' "${path#makepad/}=$git_base/$name.git#$runner_release$pin" >> "$record.mounts" ;;
        *) rm -f "$record.mounts"; fail "The runner release has a repository outside its workspace: $path" ;;
    esac
done
[ -n "$root_repo" ] || fail "The runner release has no repository at makepad."
printf 'arg\t%s\n' "$root_repo" >> "$record.part"
[ ! -f "$record.mounts" ] || { cat "$record.mounts" >> "$record.part"; rm -f "$record.mounts"; }
printf 'arg\tmakepad-runner\n' >> "$record.part"
mv -f "$record.part" "$record" || fail "Cannot write $record"
stamp start-loader
# The loader stays in the foreground until the runner closes; its output is appended to the log.
say "Makepad: starting - log: $logs/makepad.log"
say "Makepad is open in its own window; closing it ends this one."
"$makepad" >>"$logs/makepad.log" 2>&1
code=$?
stamp "loader-exit-$code"
[ "$code" = 0 ] || fail "Makepad stopped with an error (exit $code). Run this again to start it; the log says why: $logs/makepad.log"
exit 0
