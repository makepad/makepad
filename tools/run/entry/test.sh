#!/bin/sh
# First-run test of makepad.sh in a fresh folder, in a pty, against a local release service
# serving tiny fixture loaders (a two-crate workspace whose `makepad` binary only reports that it
# started). Runs: first, unchanged (no rebuild), changed loader (rebuild), broken loader (error
# with log excerpt and path). Usage: sh test.sh BASE_DIR [clean|host]   (BASE_DIR: a new, empty folder)
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
    mkdir -p "$base/zipfix/fx1/dep-a" || exit 2
    cat > "$base/zipfix/fx1/Cargo.toml" <<'TOML'
[package]
name = "makepad-run"
version = "0.1.0"
edition = "2021"
[dependencies]
dep-a = { path = "dep-a" }
[[bin]]
name = "makepad"
path = "main.rs"
[profile.host]
inherits = "release"
TOML
    cat > "$base/zipfix/fx1/Cargo.lock" <<'LOCK'
version = 4

[[package]]
name = "dep-a"
version = "0.1.0"

[[package]]
name = "makepad-run"
version = "0.1.0"
dependencies = [
 "dep-a",
]
LOCK
    printf '[package]\nname = "dep-a"\nversion = "0.1.0"\nedition = "2021"\n' > "$base/zipfix/fx1/dep-a/Cargo.toml"
    printf 'pub fn text() -> &'"'"'static str { "%s" }\n' "$1" > "$base/zipfix/fx1/dep-a/lib.rs"
    printf '\n[lib]\npath = "lib.rs"\n' >> "$base/zipfix/fx1/dep-a/Cargo.toml"
    if [ "${2:-}" = broken ]; then
        printf 'fn main() { this is not rust }\n' > "$base/zipfix/fx1/main.rs"
    else
        printf 'fn main() {\n    let home = std::env::var("MAKEPAD_RUN_HOME").unwrap_or_default();\n    let record = std::path::Path::new(&home).join("apps/runner.launch");\n    println!("fixture loader: {} launch record present: {}", dep_a::text(), record.is_file());\n}\n' > "$base/zipfix/fx1/main.rs"
    fi
    (cd "$base/zipfix" && python3 -I -m zipfile -c "$base/www/public/app/runner/fx1/loader.zip" fx1) || exit 2
    zip=$base/www/public/app/runner/fx1/loader.zip
    bytes=$(wc -c < "$zip" | tr -d ' ')
    if command -v sha256sum >/dev/null 2>&1; then sum=$(sha256sum "$zip" | cut -d' ' -f1); else sum=$(shasum -a 256 "$zip" | cut -d' ' -f1); fi
    commit=0123456789abcdef0123456789abcdef01234567
    printf '{"release":"fx1","repositories":[{"name":"makepad","path":"makepad","commit":"%s"}],"loader":{"name":"loader.zip","bytes":%s,"sha256":"%s","commit":"%s"}}' \
        "$commit" "$bytes" "$sum" "$commit" > "$base/www/public/app/runner/latest"
}
# run OUTFILE: the script in a pty; its exit code in $code
run() {
    if [ "$(uname -s)" = Darwin ]; then
        env PATH="$path" script -q "$1" sh "$base/home/makepad.sh" > /dev/null
    else
        env PATH="$path" script -q -e -c "sh $base/home/makepad.sh" "$1" > /dev/null
    fi
    code=$?
    echo "=== $1 (exit $code)"; cat "$1"
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
        for tool in chmod ln rmdir sort id clang nproc tee sh dash bash curl tar cc gcc ld as gzip awk sed grep cut tr wc cat ls date sleep mkdir rm mv cp kill dirname basename df head tail sha256sum shasum python3 unzip bsdtar diff find touch env uname mktemp script xcode-select xcrun git; do
            found=$(command -v "$tool" 2>/dev/null) && case "$found" in /*) ln -s "$found" "$base/bin/$tool" ;; esac
        done
        path=$base/bin ;;
    *) path=$PATH ;;
esac
export MAKEPAD_LOADER_SERVICE=http://127.0.0.1:$port MAKEPAD_LOADER_ROOT=$base/install

run "$base/run1.out"; first=$code
check "first run exits 0" "$base/run1.out" "starting"
[ "$first" = 0 ] || failures=$((failures + 1))
check "progress line" "$base/run1.out" "Building the JIT .* [0-9]*/2 crates"
check "loader started" "$base/install/cache/logs/makepad.log" "fixture loader: one launch record present: true"
echo "--- sizes (KB), first run"; du -sk "$base/install/rust" "$base/install/cache/downloads" "$base/install" 2>/dev/null

run "$base/run2.out"
check "unchanged loader: no rebuild" "$base/run2.out" "up to date"

publish two
run "$base/run3.out"
check "changed loader: rebuilt" "$base/run3.out" "compiled in"
check "changed loader: new text" "$base/install/cache/logs/makepad.log" "fixture loader: two"

# a runner switch to a commit folder: the script runs on the folder `current` names
loader=$base/install/cache/loader
hex=0123456789abcdef0123456789abcdef01234567
cp -R "$loader/src" "$loader/$hex" || exit 2
printf '#!/bin/sh\necho hexfolder\n' > "$loader/$hex/target/host/makepad"; chmod 755 "$loader/$hex/target/host/makepad"
printf '%s\n' src > "$loader/previous"; printf '%s\n' "$hex" > "$loader/current"
run "$base/run5.out"
check "current folder: up to date" "$base/run5.out" "up to date"
check "current folder: its binary runs" "$base/install/cache/logs/makepad.log" "hexfolder"
# an unconfirmed switch rolls back to previous (src: current deleted) with a note
printf '%s\n' "$hex" > "$loader/pending"
run "$base/run6.out"
check "rollback: line" "$base/run6.out" "JIT: went back to src"
check "rollback: src binary runs" "$base/install/cache/logs/makepad.log" "fixture loader: two"
[ ! -e "$loader/pending" ] && [ ! -e "$loader/current" ] && [ -s "$loader/rolled-back" ] && echo "PASS rollback: pointers" || { echo "FAIL rollback: pointers"; failures=$((failures + 1)); }

publish three broken
run "$base/run4.out"
[ "$code" != 0 ] || { echo "FAIL broken loader exited 0"; failures=$((failures + 1)); }
check "broken loader: error line" "$base/run4.out" "Error: the JIT did not compile"
check "broken loader: log path" "$base/run4.out" "Full log: .*build.log"
check "broken loader: error text" "$base/run4.out" "error"

kill "$server" 2>/dev/null
echo "failures: $failures"
[ "$failures" = 0 ]
