#!/bin/sh
# First-run test of makepad.sh in a fresh folder, in a pty, against a local release service
# serving tiny fixture loaders (a three-crate workspace whose `makepad` handles recovery and
# reports which loader started). Runs: first, unchanged (no rebuild), changed loader (rebuild),
# broken loader (error with log excerpt and path). Usage: sh test.sh BASE_DIR [clean|host]   (BASE_DIR: a new, empty folder)
#   clean: PATH holds no Rust, so the script downloads its own into its folder
#   host:  PATH is left alone, so a suitable installed Rust is used
# Never run it against an existing installation or on a machine someone works on.
set -u
base=${1:?usage: test.sh BASE_DIR [clean|host]}
mode=${2:-clean}
here=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
[ -z "$(ls -A "$base" 2>/dev/null)" ] || { echo "$base is not empty"; exit 2; }
mkdir -p "$base/www/public/app/runner/fx1" "$base/home" || exit 2
cd "$base" || exit 2
failures=0
check() { # check NAME FILE PATTERN
    if grep -q "$3" "$2"; then echo "PASS $1"; else echo "FAIL $1 (no '$3' in $2)"; failures=$((failures + 1)); fi
}

# publish TEXT [broken]: a new loader ZIP (named by its SHA-256 in the release) with main printing TEXT
publish() {
    rm -rf "$base/zipfix" "$base/www/public/app/runner/fx1/loader.zip"
    switch=$base/zipfix/fx1/libs/loader_switch
    mkdir -p "$base/zipfix/fx1/dep-a" "$switch/src" || exit 2
    cp "$here/../../../libs/loader_switch/Cargo.toml" "$switch/Cargo.toml" &&
        cp "$here/../../../libs/loader_switch/src/lib.rs" "$switch/src/lib.rs" || exit 2
    cat > "$base/zipfix/fx1/Cargo.toml" <<'TOML'
[package]
name = "makepad-run"
version = "0.1.0"
edition = "2021"
[dependencies]
dep-a = { path = "dep-a" }
makepad-loader-switch = { path = "libs/loader_switch" }
[[bin]]
name = "makepad"
path = "main.rs"
[[bin]]
name = "makepad-launcher"
path = "launcher.rs"
[profile.host]
inherits = "release"
TOML
    cat > "$base/zipfix/fx1/Cargo.lock" <<'LOCK'
version = 4

[[package]]
name = "dep-a"
version = "0.1.0"

[[package]]
name = "makepad-loader-switch"
version = "0.1.0"

[[package]]
name = "makepad-run"
version = "0.1.0"
dependencies = [
 "dep-a",
 "makepad-loader-switch",
]
LOCK
    printf '[package]\nname = "dep-a"\nversion = "0.1.0"\nedition = "2021"\n' > "$base/zipfix/fx1/dep-a/Cargo.toml"
    printf 'pub fn text() -> &'"'"'static str { "%s" }\n' "$1" > "$base/zipfix/fx1/dep-a/lib.rs"
    printf '\n[lib]\npath = "lib.rs"\n' >> "$base/zipfix/fx1/dep-a/Cargo.toml"
    printf 'fn main() {}\n' > "$base/zipfix/fx1/launcher.rs"
    if [ "$1" = six ]; then
        # Keep the shell-error build running until the date stub supplies its bad clock value.
        cat > "$base/zipfix/fx1/build.rs" <<RUST || exit 2
use std::path::Path;
use std::time::{Duration, Instant};

fn main() {
    let marker = Path::new(r#"$base/stub2/once"#);
    let start = Instant::now();
    while !marker.is_file() {
        if start.elapsed() >= Duration::from_secs(60) {
            panic!("timed out waiting for the date stub to create {}", marker.display());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}
RUST
    fi
    # two paths that are a file in the first release and a folder in the next, and the other way round
    if [ "$1" = one ]; then first=flip second=flop; else first=flop second=flip; fi
    printf 'a file\n' > "$base/zipfix/fx1/$first"
    mkdir "$base/zipfix/fx1/$second" || exit 2
    printf 'inside\n' > "$base/zipfix/fx1/$second/inner"
    if [ "${2:-}" = broken ]; then
        printf 'fn main() { this is not rust }\n' > "$base/zipfix/fx1/main.rs"
    else
        cat > "$base/zipfix/fx1/main.rs" <<'RUST'
fn main() {
    let home = std::path::PathBuf::from(std::env::var_os("RELAX_HOME").unwrap());
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|cmd| cmd == "recover") {
        std::process::exit(makepad_loader_switch::recover_command(&home.join("cache/loader"), &args[1..]));
    }
    let home = home.canonicalize().unwrap();
    let exe = std::env::current_exe().unwrap().canonicalize().unwrap();
    let target = exe.parent().unwrap().parent().unwrap();
    let loaders = home.join("cache/loader");
    let folder = if target.parent() == Some(home.join("target/loader").as_path()) {
        loaders.join(target.file_name().unwrap())
    } else {
        assert!(target.ends_with("target") && target.parent().unwrap().parent() == Some(loaders.as_path()));
        target.parent().unwrap().to_path_buf()
    };
    let record = folder.join("runner.launch");
    println!("fixture loader: {} launch record present: {} args: {:?}", dep_a::text(), record.is_file(), args);
    println!("fixture loader folder: {}", folder.file_name().unwrap().to_string_lossy());
}
RUST
    fi
    (cd "$base/zipfix" && python3 -I -m zipfile -c "$base/www/public/app/runner/fx1/loader.zip" fx1) || exit 2
    zip=$base/www/public/app/runner/fx1/loader.zip
    bytes=$(wc -c < "$zip" | tr -d ' ')
    if command -v sha256sum >/dev/null 2>&1; then sum=$(sha256sum "$zip" | cut -d' ' -f1); else sum=$(shasum -a 256 "$zip" | cut -d' ' -f1); fi
    commit=0123456789abcdef0123456789abcdef01234567
    printf '{"release":"fx1","repositories":[{"name":"makepad","path":"makepad","commit":"%s"},{"name":"extra","path":"makepad/apps/extra","commit":""}],"loader":{"name":"loader.zip","bytes":%s,"sha256":"%s","commit":"%s"}}' \
        "$commit" "$bytes" "$sum" "$commit" > "$base/www/public/app/runner/latest"
}
# run OUTFILE: the script in a pty (the screen); plainrun OUTFILE: its output to a file (plain
# lines); resizerun OUTFILE: in a pty that takes the output slowly and changes size every few
# milliseconds, so that SIGWINCH interrupts the screen's writes again and again; its exit code in $code
plainrun() {
    [ "${MAKEPAD_LOADER_ROOT:-}" = "$base/install" ] || { echo "a run without its own MAKEPAD_LOADER_ROOT"; exit 2; }
    env PATH="$path" sh "$base/home/makepad.sh" > "$1" 2>&1 < /dev/null
    code=$?
    echo "=== $1 (exit $code)"; cat "$1"
}
run() {
    # every run here has a folder of its own, never the profile's
    [ "${MAKEPAD_LOADER_ROOT:-}" = "$base/install" ] || { echo "a run without its own MAKEPAD_LOADER_ROOT"; exit 2; }
    if [ "$(uname -s)" = Darwin ]; then
        env PATH="$path" TERM=xterm-256color script -q "$1" sh "$base/home/makepad.sh" > /dev/null
    else
        env PATH="$path" TERM=xterm-256color script -q -e -c "sh $base/home/makepad.sh" "$1" > /dev/null
    fi
    code=$?
    echo "=== $1 (exit $code)"; cat "$1"
}
resizerun() {
    [ "${MAKEPAD_LOADER_ROOT:-}" = "$base/install" ] || { echo "a run without its own MAKEPAD_LOADER_ROOT"; exit 2; }
    python3 -I - "$1" "$base/home/makepad.sh" "$path" <<'PY'
import os, sys, time, struct, fcntl, termios, select
out, script = sys.argv[1], sys.argv[2]
env = dict(os.environ, PATH=sys.argv[3], TERM='xterm-256color')
master, slave = os.openpty()
fcntl.ioctl(slave, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100, 0, 0))
pid = os.fork()
if pid == 0:
    os.close(master)
    os.setsid()
    fcntl.ioctl(slave, termios.TIOCSCTTY, 0)
    for fd in (0, 1, 2):
        os.dup2(slave, fd)
    os.execvpe('sh', ['sh', script], env)
os.close(slave)
resizes, status, deadline = 0, None, 0
with open(out, 'wb') as f:
    while True:
        resizes += 1
        fcntl.ioctl(master, termios.TIOCSWINSZ, struct.pack('HHHH', 30, 100 - resizes % 2, 0, 0))
        if select.select([master], [], [], 0.002)[0]:
            try:
                b = os.read(master, 256)
            except OSError:
                b = b''
            if not b:
                break
            f.write(b)
            time.sleep(0.004)
        elif status is None:
            done, st = os.waitpid(pid, os.WNOHANG)
            if done:
                status, deadline = os.waitstatus_to_exitcode(st), time.time() + 2
        elif time.time() > deadline:
            break
print('%d resizes' % resizes)
sys.exit(os.waitstatus_to_exitcode(os.waitpid(pid, 0)[1]) if status is None else status)
PY
    code=$?
    echo "=== $1 (exit $code), its end:"; tail -c 2000 "$1"; echo
}

publish one
port=$((20000 + $$ % 20000))
python3 -I -m http.server "$port" --bind 127.0.0.1 --directory "$base/www" > server.log 2>&1 &
server=$!
echo "$server" > server.pid
sleep 1
cp "$here/makepad.sh" "$base/home/makepad.sh"
case "$mode" in
    clean)
        # a PATH of links to the system tools the script needs, and no Rust
        mkdir "$base/bin" || exit 2
        for tool in chmod ln rmdir sort id printf clang nproc tee sh dash bash curl tar cc gcc ld as gzip awk sed grep cut tr wc cat ls date sleep mkdir rm mv cp kill dirname basename df head tail sha256sum shasum python3 unzip bsdtar diff find touch env uname mktemp script ps stty dd xcode-select xcrun git; do
            # the file on PATH, also for a tool the shell has built in (printf)
            found=$(IFS=:; for dir in $PATH; do [ -f "$dir/$tool" ] && [ -x "$dir/$tool" ] && { echo "$dir/$tool"; break; }; done)
            [ -z "$found" ] || ln -s "$found" "$base/bin/$tool"
        done
        path=$base/bin ;;
    *) path=$PATH ;;
esac
export MAKEPAD_LOADER_SERVICE=http://127.0.0.1:$port MAKEPAD_LOADER_ROOT=$base/install

# a test service without a folder of its own stops before it makes anything (HOME a folder of the
# test's, in case it did not)
(unset MAKEPAD_LOADER_ROOT; HOME=$base/profile PATH=$path sh "$base/home/makepad.sh") > "$base/run0.out" 2>&1
code=$?
check "no loader root: refused" "$base/run0.out" "MAKEPAD_LOADER_SERVICE is set without MAKEPAD_LOADER_ROOT"
if [ "$code" = 1 ] && [ ! -e "$base/profile" ] && [ "$(ls -A "$base/home")" = makepad.sh ]; then
    echo "PASS no loader root: nothing made"
else
    echo "FAIL no loader root: exit $code, or it made something"; failures=$((failures + 1))
fi

run "$base/run1.out"; first=$code
check "first run exits 0" "$base/run1.out" "Runner         [^ ]* starting"
[ "$first" = 0 ] || failures=$((failures + 1))
check "screen: header" "$base/run1.out" "Makepad.* Runner"
check "screen: keys" "$base/run1.out" "ctrl+c stops · run makepad.sh again to continue"
check "screen: ready" "$base/run1.out" "Ready in [0-9]*:[0-9][0-9]\. The Runner opens in a moment\."
check "progress line, the real total" "$base/run1.out" "[0-9] / 3 crates"
check "built: all crates" "$base/run1.out" "compiled *[^ ]*in [0-9]*:[0-9][0-9] · 3 crates"
[ "$mode" != clean ] || check "rust downloaded" "$base/run1.out" "downloaded and unpacked in [0-9]*:[0-9][0-9]"
# the downloaded Rust's rust-objcopy finds its LLVM: macOS release builds strip with it, unwarned
if [ "$mode" = clean ]; then
    for objcopy in "$base"/install/rust/lib/rustlib/*/bin/rust-objcopy; do "$objcopy" --version > "$base/objcopy.out" 2>&1; done
    check "rust-objcopy runs" "$base/objcopy.out" "llvm-objcopy"
fi
if grep -q 'rust-objcopy' "$base/install/cache/logs/build.log"; then
    echo "FAIL release build: a rust-objcopy warning"; failures=$((failures + 1))
else
    echo "PASS release build: no rust-objcopy warning"
fi
check "loader started" "$base/install/cache/logs/makepad.log" "fixture loader: one launch record present: true"
service=$MAKEPAD_LOADER_SERVICE
cat > "$base/expected.launch" <<RECORD
title	Makepad
arg	run
arg	--quiet
arg	--cached
arg	$service/public/git/runner/makepad.git#fx1@0123456789abcdef0123456789abcdef01234567
arg	--mount
arg	apps/extra=$service/public/git/runner/extra.git#fx1
arg	makepad-runner
RECORD
if cmp -s "$base/expected.launch" "$base/install/cache/loader/src/runner.launch"; then echo "PASS Runner record text"
else echo "FAIL Runner record text"; failures=$((failures + 1)); fi
echo "--- sizes (KB), first run"; du -sk "$base/install/rust" "$base/install/cache/downloads" "$base/install" 2>/dev/null

# a folder whose Rust came before the LLVM link gets it on its next start
if [ "$mode" = clean ]; then
    for link in "$base"/install/rust/lib/rustlib/*/lib/libLLVM*; do [ ! -L "$link" ] || rm -f "$link"; done
fi
# Put the built loader where older installs kept it, with no source marker.
loader=$base/install/cache/loader
targets=$base/install/target/loader
check "built loader: source commit" "$loader/src/.built" "^$commit "
check "built loader: source marker" "$targets/src/source" '^\.\./\.\./\.\./cache/loader/src$'
mv "$targets/src" "$loader/src/target" || exit 2
rm -f "$loader/src/target/source" || exit 2
# Block the move without changing permissions (the test also runs as root).
rmdir "$targets" && : > "$targets" || exit 2
: > "$base/install/cache/logs/makepad.log"
# The installed folder's own record also starts offline, past obsolete install files.
mkdir -p "$base/install/apps" "$base/install/cache/runner" || exit 2
printf 'old record\n' > "$base/install/apps/runner.launch"
printf 'old release\n' > "$base/install/cache/runner/release"
mv "$base/www/public/app/runner/latest" "$base/offline-latest" || exit 2
plainrun "$base/skipped.out"
mv "$base/offline-latest" "$base/www/public/app/runner/latest" || exit 2
if cmp -s "$base/expected.launch" "$loader/src/runner.launch" && [ ! -e "$base/install/apps/runner.launch" ] && [ ! -e "$base/install/cache/runner/release" ]; then
    echo "PASS offline: folder record kept and old files removed"
else echo "FAIL offline: Runner pin"; failures=$((failures + 1)); fi
check "skipped move: notice" "$base/skipped.out" "Skipped moving .*src/target"
check "skipped move: no rebuild" "$base/skipped.out" "^  Loader build: up to date"
check "skipped move: old loader started" "$base/install/cache/logs/makepad.log" "fixture loader: one launch record present: true"
if [ "$code" = 0 ] && [ -f "$targets" ] && [ -d "$loader/src/target" ] && [ ! -L "$loader/src/target" ] && [ ! -e "$loader/src/target/source" ]; then
    echo "PASS skipped move: exit 0 with the old tree in place"
else
    echo "FAIL skipped move: exit $code or wrong build path"; failures=$((failures + 1))
fi

# A release change while the move is still blocked rebuilds in the old Cargo tree.
publish two
run "$base/run3.out"
check "changed loader: rebuilt" "$base/run3.out" "compiled *[^ ]*in [0-9]*:[0-9][0-9] · "
check "changed loader: new text" "$base/install/cache/logs/makepad.log" "fixture loader: two"
if [ -e "$loader/src/target/source" ]; then echo "FAIL old build: unexpected source marker"; failures=$((failures + 1)); fi
source=$loader/src
if [ -f "$source/flip/inner" ] && [ -f "$source/flop" ] && [ "$(cat "$source/.version")" = "$sum" ]; then
    echo "PASS changed loader: a file became a folder, a folder a file"
else
    echo "FAIL changed loader: flip and flop were not replaced"; failures=$((failures + 1))
fi

rm "$targets" || exit 2
# A reused build from the older installer has the ZIP checksum in its first field.
awk -v sha="$sum" '{print sha, $2, $3}' "$source/.built" > "$source/.built.part" && mv "$source/.built.part" "$source/.built" || exit 2
plainrun "$base/run2.out"
check "reused loader: source commit replaces old archive stamp" "$source/.built" "^$commit "
if [ "$mode" = clean ]; then
    for objcopy in "$base"/install/rust/lib/rustlib/*/bin/rust-objcopy; do "$objcopy" --version > "$base/objcopy2.out" 2>&1; done
    check "rust-objcopy runs after an older install" "$base/objcopy2.out" "llvm-objcopy"
fi
check "migrated loader: no rebuild" "$base/run2.out" "^  Loader build: up to date · for this processor$"
if [ "$code" = 0 ] && [ "$(readlink "$loader/src/target")" = "../../../target/loader/src" ] && [ -x "$loader/src/target/host/makepad" ] && [ -x "$targets/src/host/makepad-launcher" ]; then
    echo "PASS migrated loader: build moved"
else
    echo "FAIL migrated loader: build paths"; failures=$((failures + 1))
fi
check "migrated loader: source marker" "$targets/src/source" '^\.\./\.\./\.\./cache/loader/src$'
check "plain: the source" "$base/run2.out" "^  Loader source: up to date · "
check "plain: ready" "$base/run2.out" "^  Ready in [0-9]*:[0-9][0-9]\. The Runner opens in a moment\.$"
if grep -q "$(printf '\033')" "$base/run2.out"; then echo "FAIL plain: no escape codes"; failures=$((failures + 1)); else echo "PASS plain: no escape codes"; fi

# An older Runner switches current before the bootstrap runs again. The stable entry follows it.
hex=0123456789abcdef0123456789abcdef01234567
cp -RP "$loader/src" "$loader/$hex" && rm "$loader/$hex/target" || exit 2
cp -R "$targets/src" "$loader/$hex/target" && rm "$loader/$hex/target/source" || exit 2
printf '#!/bin/sh\necho hexfolder\n' > "$loader/$hex/target/host/makepad"; chmod 755 "$loader/$hex/target/host/makepad"
printf '%s\n' src > "$loader/previous"; printf '%s\n' "$hex" > "$loader/current"
"$base/install/makepad" > "$base/old-current.out"
check "current folder: stable entry finds the old build" "$base/old-current.out" "^hexfolder$"
# Pending changes which pointer the stable entry reads; the entry only selects a binary.
unbuilt=bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
mkdir "$loader/$unbuilt" || exit 2
check_pending_entry() {
    for previous in "$hex" ../outside "$unbuilt"; do
        if [ "$previous" = "$hex" ]; then selected=src; expected='^hexfolder$'
        else selected=$hex; expected='^fixture loader folder: src$'; fi
        printf '%s\n' "$selected" > "$loader/current" && printf '%s\n' "$previous" > "$loader/previous" &&
            printf '%s\n' "$hex" > "$loader/pending" || exit 2
        "$base/install/makepad" > "$base/pending-entry.out" || { echo "FAIL pending entry: exit $?"; failures=$((failures + 1)); }
        check "pending entry ($1): previous $previous" "$base/pending-entry.out" "$expected"
        if [ "$(cat "$loader/current")" != "$selected" ] || [ "$(cat "$loader/previous")" != "$previous" ] ||
            [ "$(cat "$loader/pending")" != "$hex" ] || [ -e "$loader/rolled-back" ]; then
            echo "FAIL pending entry ($1): switch state changed"; failures=$((failures + 1))
        fi
    done
    rm -f "$loader/pending" "$loader/previous" || exit 2
}
check_pending_entry "old target"
run "$base/run5.out"
check "current folder: up to date" "$base/run5.out" "Loader build   [^ ]* up to date"
check "current folder: its binary runs" "$base/install/cache/logs/makepad.log" "hexfolder"
check_pending_entry "shared target"
rmdir "$loader/$unbuilt" || exit 2
cp "$targets/src/host/makepad" "$loader/$hex/target/host/makepad" || exit 2
# A code-only trial uses the selected folder. Keep the service offline so only recovery can restore the pin.
record=$loader/$hex/runner.launch
cp "$record" "$record.previous" || exit 2
printf 'unconfirmed trial\n' > "$record"
printf '%s\n' "$hex" > "$loader/previous"; printf '%s\n' "$hex" > "$loader/pending"
mv "$base/www/public/app/runner/latest" "$base/offline-latest" || exit 2
: > "$base/install/cache/logs/makepad.log" || exit 2
plainrun "$base/code-rollback.out"
# nothing tries this switch: recovery settles it at once, with no waiting line
if grep -q 'Waiting for the update' "$base/code-rollback.out"; then echo "FAIL code rollback: waited with no open Makepad"; failures=$((failures + 1)); else echo "PASS code rollback: no wait with no open Makepad"; fi
check "code rollback: selected folder runs" "$base/install/cache/logs/makepad.log" "^fixture loader folder: $hex$"
mv "$base/offline-latest" "$base/www/public/app/runner/latest" || exit 2
if [ "$code" = 0 ] && cmp -s "$base/expected.launch" "$record" && [ "$(cat "$loader/current")" = "$hex" ] \
    && [ ! -e "$record.previous" ] && [ ! -e "$loader/pending" ] && [ -s "$loader/rolled-back" ]; then
    echo "PASS code rollback: old record restored on the same loader"
else echo "FAIL code rollback: record or pointers"; failures=$((failures + 1)); fi
rm -f "$loader/rolled-back" || exit 2
# an unconfirmed switch rolls back to previous (src: current deleted) with a note
printf '%s\n' src > "$loader/previous"
printf '%s\n' "$hex" > "$loader/pending"
: > "$base/install/cache/logs/makepad.log" || exit 2
run "$base/run6.out"
check "rollback: note" "$loader/rolled-back" '^The update did not confirm its first frame\.$'
check "rollback: src binary runs" "$base/install/cache/logs/makepad.log" "fixture loader: two"
check "rollback: selected folder" "$base/install/cache/logs/makepad.log" '^fixture loader folder: src$'
[ ! -e "$loader/pending" ] && [ ! -e "$loader/current" ] && [ -s "$loader/rolled-back" ] && echo "PASS rollback: pointers" || { echo "FAIL rollback: pointers"; failures=$((failures + 1)); }

# The coding-agent form, from a shell with no terminal: plain lines, the stable entry
# ($root/makepad), then the hand-over to `makepad open <id>`; a mistyped id stops before setup.
openrun() { # openrun OUTFILE ARGS...
    out=$1; shift
    [ "${MAKEPAD_LOADER_ROOT:-}" = "$base/install" ] || { echo "a run without its own MAKEPAD_LOADER_ROOT"; exit 2; }
    env PATH="$path" sh "$base/home/makepad.sh" "$@" > "$out" 2>&1 < /dev/null
    code=$?
    echo "=== $out (exit $code)"; cat "$out"
}
openrun "$base/open1.out" open fab
check "open: hands over" "$base/open1.out" "Handing over to makepad open fab\."
check "open: the loader gets open fab" "$base/open1.out" 'fixture loader: two launch record present: true args: \["open", "fab"\]'
env -i HOME="$base/profile" PATH=/usr/bin:/bin "$base/install/makepad" open fab > "$base/open2.out" 2>&1 < /dev/null
check "open: the stable entry runs the loader" "$base/open2.out" 'fixture loader: two launch record present: true args: \["open", "fab"\]'
openrun "$base/open3.out" open 'fab;x'
[ "$code" != 0 ] || { echo "FAIL open: a bad id exited 0"; failures=$((failures + 1)); }
check "open: a bad id refused" "$base/open3.out" "Invalid app id"

# Rust installed elsewhere (where rustup puts it) and a folder from before: a changed loader compiles
# while the terminal takes the screen slowly and is resized every few milliseconds. The interrupted
# writes leave nothing behind (bash 3.2 kept their rest and put it into the next value the script
# read: a shell error mid-build), and nothing goes to stderr meanwhile.
installed=$base/rustup/toolchains/stable
if [ "$mode" = clean ]; then
    mkdir -p "$base/rustup/toolchains" && mv "$base/install/rust" "$installed" || exit 2
    unmoved=$path
    path=$installed/bin:$path
fi
publish resized
resizerun "$base/run8.out"
if [ "$mode" = clean ]; then
    path=$unmoved
    mv "$installed" "$base/install/rust" || exit 2
fi
[ "$code" = 0 ] && echo "PASS resized: exits 0" || { echo "FAIL resized: exit $code"; failures=$((failures + 1)); }
[ "$mode" != clean ] || check "resized: installed Rust" "$base/run8.out" "installed, "
check "resized: rebuilt" "$base/run8.out" "compiled *[^ ]*in [0-9]*:[0-9][0-9] · "
check "resized: ready" "$base/run8.out" "Ready in [0-9]*:[0-9][0-9]\."
check "resized: new text" "$base/install/cache/logs/makepad.log" "fixture loader: resized"
if grep -a -q -E 'write error|syntax error|Interrupted system call|line [0-9]+:' "$base/run8.out"; then
    echo "FAIL resized: a shell error"; failures=$((failures + 1))
else
    echo "PASS resized: no shell error"
fi
if [ -s "$base/install/cache/logs/stderr.log" ]; then
    echo "FAIL resized: stderr while the screen was up:"; cat "$base/install/cache/logs/stderr.log"; failures=$((failures + 1))
else
    echo "PASS resized: nothing on stderr"
fi

# A shell error while the screen is up (here the clock's time read back as no number, mid-build): a
# failure screen with the build's row crossed out, the shell's message below it, the build stopped.
mkdir "$base/stub2" || exit 2
printf '#!/bin/sh\nif [ "$1" = +%%s ] && [ -f "%s" ] && [ ! -f "%s" ] && [ ! -f "%s" ]; then : > "%s"; echo "12 34"; exit 0; fi\nexec %s "$@"\n' \
    "$base/install/cache/jobs/build.pid" "$base/install/cache/jobs/build.done" "$base/stub2/once" "$base/stub2/once" \
    "$(command -v date)" > "$base/stub2/date"
chmod 755 "$base/stub2/date"
publish six
unstubbed=$path
path=$base/stub2:$path
run "$base/run9.out"
path=$unstubbed
[ "$code" != 0 ] || { echo "FAIL shell error: exited 0"; failures=$((failures + 1)); }
if cmp -s "$base/expected.launch" "$loader/src/runner.launch"; then echo "PASS source update kept the Runner record"
else echo "FAIL source update lost the Runner record"; failures=$((failures + 1)); fi
check "shell error: the screen says so" "$base/run9.out" "makepad.sh stopped on an error of its own"
check "shell error: its row" "$base/run9.out" "Loader build   [^ ]* failed"
check "shell error: the shell's message" "$base/run9.out" "12 34"
if ps -axo pid=,command= | grep -F "$base/install/" | grep -v -e grep -e test.sh > "$base/left.out"; then
    echo "FAIL shell error: the build left running:"; cat "$base/left.out"; failures=$((failures + 1))
else
    echo "PASS shell error: the build stopped"
fi

# A line of diff's this doesn't know means a path was not updated: no stamp of the new version.
stamped=$(cat "$source/.version")
mkdir "$base/stub" || exit 2
printf '#!/bin/sh\necho "A line diff never said"\nexit 1\n' > "$base/stub/diff"
chmod 755 "$base/stub/diff"
publish four
unstubbed=$path
path=$base/stub:$path
run "$base/run7.out"
path=$unstubbed
[ "$code" != 0 ] || { echo "FAIL unknown diff line exited 0"; failures=$((failures + 1)); }
check "unknown diff line: error" "$base/run7.out" "A line diff never said"
if [ "$(cat "$source/.version")" = "$stamped" ]; then
    echo "PASS unknown diff line: not stamped"
else
    echo "FAIL unknown diff line: stamped"; failures=$((failures + 1))
fi

publish three broken
run "$base/run4.out"
[ "$code" != 0 ] || { echo "FAIL broken loader exited 0"; failures=$((failures + 1)); }
check "broken loader: error line" "$base/run4.out" "The loader did not compile\."
check "broken loader: its row" "$base/run4.out" "Loader build   [^ ]* failed"
check "broken loader: log path" "$base/run4.out" "Full log: .*build.log"
check "broken loader: error text" "$base/run4.out" "error"

kill "$server" 2>/dev/null
echo "failures: $failures"
[ "$failures" = 0 ]
