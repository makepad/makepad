#!/bin/sh
# Makepad for macOS and Linux: gets Rust, compiles Makepad's loader (makepad run) from its source on
# this computer and starts it. Read it before you run it; it is this one file.
#
#   curl -fsSL https://makepad.nl/builder/unix | sh
#
# 1. Rust 1.98.0 (rustc, cargo, rust-std) from static.rust-lang.org, fetched as 25 MB pieces
#    over parallel connections, checked against the sizes and SHA-256 sums below.
# 2. At the same time, the loader's source: the release service's licence-free loader release,
#    a ZIP whose size and SHA-256 the service names (checked before unpacking); and, with no
#    arguments, the packs of the runner's git repositories (the service's latest `runner`
#    release), which makepad run takes instead of downloading them itself.
# 3. The loader is compiled (`cargo build --release --offline`: the ZIP holds every crate) the
#    moment Rust and the source are in place, and started: `makepad run` the runner, which
#    downloads the rest. With arguments, the loader runs with them instead.
#
# Beside this file it creates only two folders: rust/ (the toolchain) and cache/
# (everything else: downloads, the loader, its caches and logs). Piped into sh, the folder is
# ~/makepad-builder; $MAKEPAD_LOADER_ROOT overrides either. Run it again at any time: nothing is
# downloaded or compiled twice. The system needs a C linker (Apple's command line tools; on
# Linux cc) and, on Linux, OpenSSL 3's development files; it names the packages when missing.
set -u
umask 022

rust_version=1.98.0
rust_date=2026-08-20
dist=https://static.rust-lang.org/dist/$rust_date
piece=25000000
service=${MAKEPAD_LOADER_SERVICE:-https://makepad.nl/api/loader}
service=${service%/}

# --- The screen ---------------------------------------------------------------------------
# The Builder's look: header, one row per step, a rule, a status line and a footer, in plain
# terminal colours (bold title, dim secondary text, orange for work in progress, green checks).
# Rows are rewritten in place on a terminal; anywhere else each change is one plain line.
if [ -t 1 ] && [ "${TERM:-dumb}" != dumb ]; then
    tty=1
    E=$(printf '\033')
    B="$E[0;1m" D="$E[0;2m" C="$E[0;38;2;255;92;57m" G="$E[0;32m" Y="$E[0;33m" R="$E[0;31m" N="$E[0m"
else
    tty= E= B= D= C= G= Y= R= N=
fi
rule='────────────────────────────────────────────────────────────────────────────'
framed=
logs=
jobs=
shown=

now() { date +%s; }
# stamp NAME appends "NAME HH:MM:SS.cc" to the timing log
stamp() {
    [ -n "$logs" ] && [ -d "$logs" ] || return 0
    t=$(date +%H:%M:%S.%N)
    case "$t" in *N) t=${t%.N} ;; *) t=$(printf '%s' "$t" | cut -c1-11) ;; esac
    printf '%s %s\n' "$1" "$t" >> "$logs/bootstrap.log"
}
# row N NAME TEXT [KEY]: TEXT after the name column on screen row N; off a terminal the line is
# printed when KEY (default TEXT) changed, so progress shows in steps, not every second
row() {
    [ -n "$framed" ] || return 0
    if [ -n "$tty" ]; then
        printf '%s[%s;1H%s[2K  %-20s%s' "$E" "$1" "$E" "$2" "$3"
    else
        eval "last=\${last_$1:-}"
        [ "$last" = "${4:-$3}" ] && return 0
        eval "last_$1=\${4:-\$3}"
        printf '  %-20s%s\n' "$2" "$3"
    fi
}
status() {
    [ -n "$framed" ] || return 0
    if [ -n "$tty" ]; then printf '%s[11;1H%s[2K  %s' "$E" "$E" "$1"
    elif [ "${last_status:-}" != "$1" ]; then last_status=$1; printf '  %s\n' "$1"; fi
}
footer() {
    [ -n "$framed" ] && [ -n "$tty" ] || return 0
    printf '%s[13;1H%s[2K  %s%s%s' "$E" "$E" "$D" "$1" "$N"
}
frame() {
    framed=1
    [ -z "$tty" ] || printf '%s[H%s[2J' "$E" "$E"
    printf '\n  %sMakepad%s › Getting started\n  %sShipped as source code, so your coding agent can customize everything.%s\n\n' "$B" "$N" "$D" "$N"
    if [ -n "$need_rust" ]; then row 5 "Rust $rust_version" "${D}○ downloads first$N"; else row 5 "Rust $rust_version" "$G✓$N ready"; fi
    if [ -n "$need_zip" ]; then row 6 "Loader source" "${D}○ downloads with Rust$N"; else row 6 "Loader source" "$G✓$N here"; fi
    row 7 "Makepad's loader" "${D}○ compiles once Rust is here$N"
    row 8 "Makepad" "$D○$N"
    [ -z "$tty" ] || printf '%s[10;1H%s[2K  %s%s%s' "$E" "$E" "$D" "$rule" "$N"
    status "Setting up this folder once; nothing outside it changes."
    footer "working · ctrl+c stops"
}
# below the frame, for whatever is printed after it
leave() { [ -z "$framed" ] || [ -z "$tty" ] || printf '%s[14;1H\n' "$E"; }
# bar DONE TOTAL sets BAR (24 cells)
bar() {
    filled=0
    [ "$2" -gt 0 ] && filled=$(($1 * 24 / $2))
    [ "$filled" -le 24 ] || filled=24
    done_bar= left_bar= i=1
    while [ $i -le 24 ]; do
        if [ $i -le "$filled" ]; then done_bar="$done_bar━"; else left_bar="$left_bar─"; fi
        i=$((i + 1))
    done
    BAR="$C$done_bar$D$left_bar$N"
}
# the background jobs this script started (by PID), stopped when it fails or is stopped
stop_jobs() {
    [ -n "$jobs" ] && [ -d "$jobs" ] || return 0
    for job in fetch build run unpack-rustc unpack-rust-std unpack-cargo; do
        [ "$job" = run ] && [ -n "$shown" ] && continue
        [ -f "$jobs/$job.pid" ] && [ ! -f "$jobs/$job.done" ] && kill "$(cat "$jobs/$job.pid")" 2>/dev/null
    done
    return 0
}
# fail MESSAGE: one red line, exit 1
fail() {
    stamp failed
    stop_jobs
    if [ -n "$framed" ] && [ -n "$tty" ]; then
        printf '%s[11;1H%s[2K  %s✗%s %s%s[13;1H%s[2K%s[14;1H\n' "$E" "$E" "$R" "$N" "$1" "$E" "$E" "$E"
    else
        printf '\n  %s✗%s %s\n\n' "$R" "$N" "$1"
    fi
    exit 1
}
trap 'stamp stopped; stop_jobs; leave; exit 130' INT TERM
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
nap() { sleep 0.5 2>/dev/null || sleep 1; }
sha256() { if command -v sha256sum >/dev/null 2>&1; then sha256sum "$1" | cut -d' ' -f1; else shasum -a 256 "$1" | cut -d' ' -f1; fi; }
size_of() { wc -c < "$1" | tr -d ' '; }
# a path inside a double-quoted curl config value
cfgq() { printf '%s' "$1" | sed 's/\\/\\\\/g; s/"/\\"/g'; }

# --- Where, and what this computer has ------------------------------------------------------
case "$0" in
    sh | -sh | */sh | bash | -bash | */bash | dash | */dash | zsh | */zsh) script_dir= ;;
    *) script_dir=; [ -f "$0" ] && script_dir=$(cd -P -- "$(dirname -- "$0")" && pwd -P) ;;
esac
root=${MAKEPAD_LOADER_ROOT:-${script_dir:-$HOME/makepad-builder}}
mkdir -p "$root" 2>/dev/null && root=$(cd -P -- "$root" && pwd -P) || fail "Cannot create the folder $root"
rust=$root/rust
cache=$root/cache
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
    http://*) [ "${MAKEPAD_ALLOW_HTTP:-}" = 1 ] && proto='=http,https' || fail "The release service must be an https URL ${D}· $service$N" ;;
    *) fail "The release service must be an https URL ${D}· $service$N" ;;
esac
command -v curl >/dev/null 2>&1 || fail "Makepad needs curl ${D}· install it, then run this again$N"
command -v tar >/dev/null 2>&1 || fail "Makepad needs tar ${D}· install it, then run this again$N"

# The system's linker (and on Linux OpenSSL's development files): named, never installed here.
if [ "$(uname -s)" = Darwin ]; then
    if ! xcode-select -p >/dev/null 2>&1 || ! /usr/bin/xcrun --find ld >/dev/null 2>&1; then
        xcode-select --install >/dev/null 2>&1 || :
        fail "Makepad needs Apple's command line tools ${D}· their installer opens now; run this again when it has finished$N"
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
        fail "Makepad needs $missing ${D}· install it with: $cmd$N"
    fi
fi
mkdir -p "$downloads" "$logs_dir" "$jobs" "$cache/loader" || fail "Cannot create $cache"
logs=$logs_dir
stamp start
export MAKEPAD_RUN_HOME="$root"

# --- The runner's packs, from the start (no arguments only) --------------------------------
# The release service's latest licence-free `runner` release names the runner's git
# repositories on the service's read-only git face; each commit's pack goes into
# $MAKEPAD_RUN_HOME/cache/incoming/<commit>.pack (renamed into place when complete), where makepad run
# takes it instead of downloading. A release fetched before is not downloaded again.
prefetch_pids=
runner_release=
repos=
# With a loader and a runner here already, the release service gets a few seconds: offline or
# unreachable, the folder's stores and loader start at once (makepad run runs the stored commits).
# The two questions to the service go out together.
meta="--retry 2 -m 60"
if [ -f "$cache/loader/current" ] && [ -f "$cache/runner/release" ]; then meta="--connect-timeout 3 -m 6"; fi
mkdir -p "$cache"
: > "$cache/loader-latest.json"
( curl -fsS --proto "$proto" $meta "$service/public/app/relax/latest" > "$cache/loader-latest.json.part" 2>>"$logs/download.log" \
    && mv -f "$cache/loader-latest.json.part" "$cache/loader-latest.json" ) &
latest_pid=$!
if [ $# -eq 0 ]; then
    mkdir -p "$cache/runner" "$cache/incoming"
    release_json=$(curl -fsS --proto "$proto" $meta "$service/public/app/runner/latest" 2>>"$logs/download.log") || release_json=
    runner_release=$(printf '%s' "$release_json" | sed -n 's/.*"release":"\([A-Za-z0-9_-]*\)".*/\1/p')
    if [ -n "$runner_release" ]; then
        # one "<name> <path> <commit>" line per repository of the release
        repos=$(printf '%s\n' "$release_json" | tr '{}' '\n\n' | while IFS= read -r line; do
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
fi

# --- What there is to do --------------------------------------------------------------------
need_rust=
[ "$(cat "$rust/.toolchain-version" 2>/dev/null)" = "$rust_version $triple" ] || need_rust=1
[ -x "$rust/bin/cargo" ] && [ -x "$rust/bin/rustc" ] || need_rust=1
# The latest loader release; offline, the one compiled before.
wait "$latest_pid" 2>/dev/null
latest=$(cat "$cache/loader-latest.json" 2>/dev/null) || latest=
loader_version=$(printf '%s' "$latest" | sed -n 's/.*"release":"\([A-Za-z0-9_-]*\)".*/\1/p')
archive=$(printf '%s' "$latest" | sed -n 's/.*"archive":{"name":"\([A-Za-z0-9_.-]*\)","bytes":\([0-9]*\),"sha256":"\([0-9a-f]*\)"}.*/\1 \2 \3/p')
[ -n "$archive" ] || loader_version=
[ -n "$loader_version" ] || loader_version=$(cat "$cache/loader/current" 2>/dev/null)
[ -n "$loader_version" ] || fail "Could not reach the release service ${D}($service)$N and no loader is here yet."
src=$cache/loader/$loader_version
makepad=$src/target/release/makepad
need_build=
[ -x "$makepad" ] && [ -f "$src/.built" ] || need_build=1
need_zip=
[ -n "$need_build" ] && [ ! -f "$src/Cargo.toml" ] && need_zip=1
if [ -n "$need_zip" ]; then
    [ -n "$archive" ] || fail "The release service did not answer; the loader's source is not here yet."
    loader_name=$(printf '%s' "$archive" | cut -d' ' -f1)
    loader_bytes=$(printf '%s' "$archive" | cut -d' ' -f2)
    loader_sha256=$(printf '%s' "$archive" | cut -d' ' -f3)
    zip=$downloads/$loader_name
fi
[ -z "$need_rust" ] && [ -z "$need_build" ] || frame

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
        "$service/public/app/relax/$loader_version/$loader_name" "$(cfgq "$zip.part")" "$proto" >> "$cfg"
fi
# the summed size of the Rust pieces here
pieces_sum() { ls -ln "$downloads" | awk '$9 ~ /\.tar\.gz\.[0-9]+$/ { s += $5 } END { print s + 0 }'; }
if [ -f "$cfg" ]; then
    status "Downloading Rust and Makepad's loader; once, into this folder."
    job fetch curl -sS -Z --parallel-immediate --parallel-max 16 -K "$cfg" 2>"$logs/download.log"
    prev_sum=0 prev_t=$(now) speed='0.0 MB/s'
    while :; do
        rsum=$((rust_base + $(pieces_sum)))
        zsum=0
        [ -z "$need_zip" ] || [ ! -f "$zip.part" ] || zsum=$(size_of "$zip.part")
        sum=$((rsum + zsum))
        t=$(now)
        if [ $((t - prev_t)) -ge 1 ]; then
            speed=$(awk -v b=$((sum - prev_sum)) -v s=$((t - prev_t)) 'BEGIN { printf "%.1f MB/s", b / s / 1048576 }')
            prev_sum=$sum prev_t=$t
        fi
        if [ -n "$need_rust" ]; then
            pct=$((rsum * 100 / rust_total))
            [ $pct -le 100 ] || pct=100
            bar $rsum $rust_total
            row 5 "Rust $rust_version" "$C●$N $BAR $(printf '%3d' $pct)% $D· $((rsum / 1048576)) / $((rust_total / 1048576)) MB · $speed$N" \
                "downloading · $((pct / 10 * 10))%"
        fi
        if [ -n "$need_zip" ]; then
            if [ "$zsum" -ge "$loader_bytes" ]; then
                row 6 "Loader source" "$G✓$N downloaded"
            else
                bar $zsum $loader_bytes
                row 6 "Loader source" "$C●$N $BAR $D· $((zsum / 1024)) / $((loader_bytes / 1024)) KB$N" "downloading"
            fi
        fi
        [ -f "$jobs/fetch.done" ] && break
        nap
    done
    [ "$(cat "$jobs/fetch.done")" = 0 ] || fail "The download stopped. ${D}Run makepad.sh again to continue it · log: $logs/download.log$N"
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
    row 5 "Rust $rust_version" "$C●$N checking $D· SHA-256 of each part$N"
    for part in $part_names; do
        if [ "$(sha256 "$downloads/$part.tar.gz")" != "$(part_field $part 3)" ]; then
            rm -f "$downloads/$part.tar.gz"
            fail "Checksum mismatch for $part.tar.gz ${D}· downloaded again on the next run$N"
        fi
    done
    stage=$cache/rust.unpack
    rm -rf "$stage"
    mkdir -p "$stage" || fail "Cannot create $stage"
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
        row 5 "Rust $rust_version" "$C●$N unpacking $D· $unpacked of 3$N" "unpacking"
        nap
    done
    ready=$stage/ready
    mkdir "$ready" || fail "Cannot create $ready"
    for part in $part_names; do
        [ "$(cat "$jobs/unpack-$part.done")" = 0 ] || fail "Unpacking $part failed. ${D}· log: $logs/unpack-$part.log$N"
        rm -f "$stage/$part/manifest.in"
        # the parts share bin/, lib/ and share/: move each file into place
        ( cd "$stage/$part" && find . -type d -exec sh -c 'for d; do mkdir -p "$0/$d"; done' "$ready" {} + \
            && find . ! -type d -exec sh -c 'for f; do mv -f "$f" "$0/$f"; done' "$ready" {} + ) \
            2>>"$logs/unpack-$part.log" || fail "Unpacking $part failed. ${D}· log: $logs/unpack-$part.log$N"
    done
    printf '%s %s' "$rust_version" "$triple" > "$ready/.toolchain-version"
    rm -rf "$rust"
    mv "$ready" "$rust" || fail "Cannot move Rust into $rust"
    rm -rf "$stage"
    for part in $part_names; do rm -f "$downloads/$part.tar.gz"; done
    row 5 "Rust $rust_version" "$G✓$N ready $D· in rust/$N"
    stamp rust
fi

# --- The loader's source: check and unpack --------------------------------------------------
if [ -n "$need_zip" ]; then
    if [ "$(size_of "$zip.part" 2>/dev/null)" != "$loader_bytes" ]; then
        rm -f "$zip.part"
        fail "The loader's source has the wrong size ${D}· downloaded again on the next run$N"
    fi
    if [ "$(sha256 "$zip.part")" != "$loader_sha256" ]; then
        rm -f "$zip.part"
        fail "Checksum mismatch for $loader_name ${D}· downloaded again on the next run$N"
    fi
    unpack=$cache/loader/.unpack
    rm -rf "$unpack"
    mkdir -p "$unpack" || fail "Cannot create $unpack"
    if command -v unzip >/dev/null 2>&1; then unzip -q "$zip.part" -d "$unpack"
    elif command -v bsdtar >/dev/null 2>&1; then bsdtar -xf "$zip.part" -C "$unpack"
    elif command -v python3 >/dev/null 2>&1; then python3 -m zipfile -e "$zip.part" "$unpack"
    else fail "Makepad needs unzip ${D}· install it, then run this again$N"
    fi >>"$logs/download.log" 2>&1 || fail "Unpacking the loader's source failed. ${D}· log: $logs/download.log$N"
    rm -rf "$src"
    mv "$unpack/$loader_version" "$src" || fail "Unpacking the loader's source failed. ${D}· log: $logs/download.log$N"
    rm -rf "$unpack"
    rm -f "$zip.part"
    stamp source
fi

# --- 3. Compile the loader --------------------------------------------------------------------
# The compile sees no Rust settings from elsewhere on this computer and writes only into
# cache/. Cargo's own output goes to the log; the screen shows one progress line.
unset RUSTUP_TOOLCHAIN RUSTFLAGS CARGO_ENCODED_RUSTFLAGS CARGO_BUILD_TARGET CARGO_BUILD_RUSTFLAGS CARGO_TARGET_DIR RUSTC_WRAPPER RUSTDOC
export PATH="$rust/bin:$PATH" CARGO_HOME="$cache/cargo-home" RUSTC="$rust/bin/rustc" CARGO_INCREMENTAL=0
if [ -n "$need_build" ]; then
    build_log=$logs/build.log
    rm -f "$build_log"
    # the crates to compile: one Cargo.toml each, besides the workspace's own
    crates=$(($(find "$src" -name target -prune -o -name Cargo.toml -print | wc -l) - 1))
    [ $crates -gt 0 ] || crates=1
    # the archive's lock file (path crates only): nothing is resolved or fetched on this computer
    locked=
    [ ! -f "$src/Cargo.lock" ] || locked=--locked
    status "Compiling Makepad's loader; once per version."
    build_t0=$(now)
    job build sh -c 'cd "$1" && exec "$2" build --release --offline $3 -p makepad-run --bin makepad' \
        build "$src" "$rust/bin/cargo" "$locked" >"$build_log" 2>&1
    while [ ! -f "$jobs/build.done" ]; do
        compiled=$(grep -c '^ *Compiling ' "$build_log" 2>/dev/null)
        compiled=${compiled:-0}
        [ "$compiled" -le $crates ] || compiled=$crates
        bar "$compiled" $crates
        row 7 "Makepad's loader" "$C●$N $BAR $D· $compiled of $crates crates started · $(($(now) - build_t0)) s$N" "compiling"
        nap
    done
    [ "$(cat "$jobs/build.done")" = 0 ] || fail "Makepad's loader did not compile. ${D}· log: $build_log$N"
    : > "$src/.built"
    # the loader compiled before this one is no longer needed
    old=$(cat "$cache/loader/current" 2>/dev/null)
    [ -z "$old" ] || [ "$old" = "$loader_version" ] || [ ! -d "$cache/loader/$old" ] || rm -rf "$cache/loader/$old"
    printf '%s\n' "$loader_version" > "$cache/loader/current.next" && mv -f "$cache/loader/current.next" "$cache/loader/current"
    row 7 "Makepad's loader" "$G✓$N compiled $D· $(($(now) - build_t0)) s$N"
    stamp built
fi

# This script stays in the folder for the next start.
if [ -n "$script_dir" ] && [ "$script_dir" != "$root" ]; then
    cp "$0" "$root/makepad.sh.next" 2>/dev/null && chmod 755 "$root/makepad.sh.next" && mv -f "$root/makepad.sh.next" "$root/makepad.sh" || :
fi

# --- 4. Start it --------------------------------------------------------------------------------
# makepad run runs cargo/rustc for build scripts and unit graphs (until it does that itself), so the
# toolchain stays on PATH for it and every app it starts. makepad run knows only git URLs: with no
# arguments this script names the runner's: the release service's latest licence-free `runner`
# release, whose repositories the service serves read-only over git
# ($service/public/git/runner/<repository>.git, one branch per release). The repository at
# `makepad` is the workspace, the others mount at their folders inside it. makepad run keeps its
# snapshots in $MAKEPAD_RUN_HOME/cache and, with --cached, starts the last one at once while it
# fetches the new release for the next start. Offline, the release recorded last.
export MAKEPAD_LOADER_ROOT="$root" MAKEPAD_LOADER_SERVICE="$service"
if [ $# -gt 0 ]; then
    stamp start-loader
    leave
    exec "$makepad" "$@"
fi
if [ -n "$framed" ]; then
    row 8 "Makepad" "$C●$N starting"
    status "Makepad opens in its own window; closing it ends this one."
else
    printf '  %sMakepad%s %s· starting · log: %s%s\n' "$B" "$N" "$D" "$logs/makepad.log" "$N"
fi
# the runner's packs downloaded alongside the build are complete before makepad run looks for them
[ -z "$prefetch_pids" ] || wait $prefetch_pids || :
if [ -n "$runner_release" ]; then
    [ -n "$repos" ] || fail "The runner release names no repositories."
    { printf '%s\n' "$runner_release"; printf '%s\n' "$repos"; } > "$cache/runner/release.next" && mv -f "$cache/runner/release.next" "$cache/runner/release"
else
    [ -f "$cache/runner/release" ] || fail "Could not reach the release service ${D}($service)$N and the runner was never fetched."
    status "The release service did not answer; starting the runner fetched before."
    runner_release=$(sed -n 1p "$cache/runner/release")
    repos=$(sed -n '2,$p' "$cache/runner/release")
fi
git_base=$service/public/git/runner
set -- run --quiet --cached
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
        makepad/*) set -- "$@" --mount "${path#makepad/}=$git_base/$name.git#$runner_release$pin" ;;
        *) fail "The runner release has a repository outside its workspace: $path" ;;
    esac
done
[ -n "$root_repo" ] || fail "The runner release has no repository at makepad."
stamp start-loader
# The loader runs as a job; the runner reports its first frame (MAKEPAD_RUN_REPORT_TO), and the row
# turns to running then. Its own output goes to the log.
report=$logs/runner.report
: > "$report"
export MAKEPAD_RUN_REPORT_TO="$report"
job run "$makepad" "$@" "$root_repo" makepad-runner >"$logs/makepad.log" 2>&1
while [ ! -f "$jobs/run.done" ]; do
    if [ -z "$shown" ] && grep -q '^present' "$report" 2>/dev/null; then
        shown=1
        stamp runner-present
        row 8 "Makepad" "$G✓$N running"
        status "Makepad is open in its own window; closing it ends this one."
        footer "ctrl+c leaves Makepad running"
    fi
    nap
done
code=$(cat "$jobs/run.done")
stamp "loader-exit-$code"
[ "$code" = 0 ] || fail "Makepad stopped with an error. ${D}· log: $logs/makepad.log$N"
leave
exit 0
