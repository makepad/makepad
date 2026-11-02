#!/bin/sh
# A local release service for whole-flow runner checks: the real web server on loopback, serving
# the runner release (public, with its loader ZIP) and one licensed app (scope) cut from THIS
# source tree, and the entry script run against it. Not a fake: the same release, catalog, pack and
# loader-ZIP code as makepad.nl; only the git sources are local snapshots of this tree (the server
# is told fake GitHub URLs with pinned commits whose objects are already in its git cache).
#
#   fixture.sh BASE init                  snapshot this tree (run it in the tree's root) into git repos
#                                         under BASE and write the service's source settings
#   fixture.sh BASE variant NAME          a newer snapshot: app (a licensed app's files change), tree (this tree again),
#                                         loader (the JIT's source changes), broken (the JIT's source
#                                         does not compile), rebase (the tree again, as a new commit)
#   fixture.sh BASE up SERVER_BIN         start the service and the entry script, detached (PIDs in BASE/state)
#   fixture.sh BASE log [N [entry|server]]   the tail of a log;   down: stop both by their PIDs
# SERVER_BIN is the web server (webserver repository) built for this machine. Environment: FIXTURE_PORT (default 47831). Run on a pool node; never against a real installation.
set -u
base=${1:?usage: fixture.sh BASE init|variant|up|log|down}
cmd=${2:-}
here=$(cd -P -- "$(dirname -- "$0")" && pwd -P)
port=${FIXTURE_PORT:-47831}
service=http://127.0.0.1:$port
api=$service/api/loader
data=$base/data/private
git_env() { GIT_AUTHOR_NAME=fixture GIT_AUTHOR_EMAIL=fixture@localhost GIT_COMMITTER_NAME=fixture GIT_COMMITTER_EMAIL=fixture@localhost "$@"; }

cache_dir() { # cache_dir URL BRANCH NAME
    key=$(printf '%s\n%s' "$1" "$2" | shasum -a 256 | cut -c1-16)
    echo "$data/git-cache/$key/$3"
}

# the sources a release is cut from: pin them in the git caches, write the settings with their commits
write_sources() { # write_sources MAKEPAD_SHA COMMERCIAL_SHA
    msha=$1; csha=$2
    mk=https://github.com/makepad/makepad.git
    cm=https://github.com/makepad/commercial.git
    for spec in "$mk work makepad" "$cm main commercial"; do
        set -- $spec
        dir=$(cache_dir "$1" "$2" "$3")
        mkdir -p "$dir" || exit 2
        [ -f "$dir/HEAD" ] || git -C "$dir" init -q --bare . || exit 2
        git -C "$dir" fetch -q --no-tags "$base/src/$3" "$2" || exit 2
    done
    mkdir -p "$data/apps/runner" "$data/apps/scope" || exit 2
    cat > "$data/apps/runner/source.json.new" <<EOF
{"title":"Makepad","rust":"1.98.0","package":"makepad-runner","binary":"makepad-runner","workspace":"makepad/apps/commercial/runner",
 "platforms":["aarch64-apple-darwin","x86_64-apple-darwin","x86_64-pc-windows-gnu","x86_64-unknown-linux-gnu","aarch64-unknown-linux-gnu"],
 "cuda":false,"public":true,"loader":{"repository":"makepad","roots":["makepad-run"]},
 "repositories":[
  {"name":"makepad","path":"makepad","url":"$mk","branch":"work","commit":"$msha"},
  {"name":"commercial","path":"makepad/apps/commercial","url":"$cm","branch":"main","commit":"$csha","roots":["makepad-runner"]}]}
EOF
    cat > "$data/apps/scope/source.json.new" <<EOF
{"title":"Makepad Scope","rust":"1.98.0","package":"makepad-scope","binary":"scope","workspace":"makepad/apps/commercial/scope",
 "platforms":["aarch64-apple-darwin","x86_64-apple-darwin","x86_64-pc-windows-msvc","x86_64-unknown-linux-gnu"],
 "cuda":false,
 "repositories":[
  {"name":"makepad","path":"makepad","url":"$mk","branch":"work","commit":"$msha"},
  {"name":"commercial","path":"makepad/apps/commercial","url":"$cm","branch":"main","commit":"$csha","roots":["makepad-scope"]}]}
EOF
    mv "$data/apps/runner/source.json.new" "$data/apps/runner/source.json"
    mv "$data/apps/scope/source.json.new" "$data/apps/scope/source.json"
    echo "$msha" > "$base/state/makepad.sha"
    echo "$csha" > "$base/state/commercial.sha"
}

# post JSON to the admin API (loopback, the admin address file names 127.0.0.1)
admin() { # admin PATH JSON
    curl -fsS -X POST -H 'Content-Type: application/json' -H 'X-Makepad-Admin: 1' -H "Origin: $service" \
        --data "$2" "$service/api/loader-admin/$1" > /dev/null
}

commit_all() { # commit_all REPO MESSAGE
    git -C "$1" add -A && git_env git -C "$1" commit -q --allow-empty -m "$2" && git -C "$1" rev-parse HEAD
}

case "$cmd" in
init)
    tree=$(pwd -P)
    [ -d "$tree/apps/commercial/runner" ] || { echo "run it in the makepad tree's root"; exit 2; }
    [ ! -e "$base/src" ] || { echo "$base/src exists"; exit 2; }
    mkdir -p "$base/src/makepad" "$base/src/commercial" "$base/state" "$data" "$base/web" || exit 2
    rsync -a --exclude target --exclude .git --exclude /apps/commercial --exclude /tmp --exclude /tools/run/webfx --exclude /local --exclude Cargo.lock "$tree/" "$base/src/makepad/" || exit 2
    rsync -a --exclude target --exclude .git "$tree/apps/commercial/" "$base/src/commercial/" || exit 2
    for r in makepad commercial; do
        git -C "$base/src/$r" init -q -b "$([ $r = makepad ] && echo work || echo main)" || exit 2
    done
    m=$(commit_all "$base/src/makepad" "release 1") || exit 2
    c=$(commit_all "$base/src/commercial" "release 1") || exit 2
    write_sources "$m" "$c"
    echo 127.0.0.1 > "$data/admin_ip"
    echo "init: makepad $m commercial $c" ;;
variant)
    name=${3:?variant NAME}
    m=$(cat "$base/state/makepad.sha"); c=$(cat "$base/state/commercial.sha")
    case "$name" in
    app)
        # upstream edits to the licensed app: a clean-merge line, a same-line conflict, a new file
        f=$base/src/commercial/scope/README.md
        { printf 'Upstream release 2 note.\n'; sed -n '2,$p' "$f"; } > "$f.new" && mv "$f.new" "$f"
        printf '\nUpstream appendix (release 2).\n' >> "$base/src/commercial/scope/AGENTS.md"
        printf 'added upstream in release 2\n' > "$base/src/commercial/scope/UPSTREAM-R2.txt"
        c=$(commit_all "$base/src/commercial" "release 2 (app)") || exit 2 ;;
    app2)
        f=$base/src/commercial/scope/README.md
        { printf 'Upstream release 3 note.\n'; cat "$f"; } > "$f.new" && mv "$f.new" "$f"
        c=$(commit_all "$base/src/commercial" "release 3 (app)") || exit 2 ;;
    loader)
        printf '\n// fixture: a loader change (release marker %s)\n' "$(date +%s)" >> "$base/src/makepad/tools/run/src/main.rs"
        m=$(commit_all "$base/src/makepad" "release (loader)") || exit 2 ;;
    broken)
        printf '\nthis is not rust at all\n' >> "$base/src/makepad/tools/run/src/main.rs"
        m=$(commit_all "$base/src/makepad" "release (broken loader)") || exit 2 ;;
    tree)
        # this tree again, as the next release (pick up a fix made since init)
        tree=$(pwd -P)
        rsync -a --delete --exclude target --exclude .git --exclude /apps/commercial --exclude /tmp --exclude /tools/run/webfx --exclude /tools/run/scratch --exclude /local --exclude Cargo.lock "$tree/" "$base/src/makepad/" || exit 2
        rsync -a --delete --exclude target --exclude .git "$tree/apps/commercial/" "$base/src/commercial/" || exit 2
        m=$(commit_all "$base/src/makepad" "release (tree)") || exit 2
        c=$(commit_all "$base/src/commercial" "release (tree)") || exit 2 ;;
    rebase)
        c=$(commit_all "$base/src/commercial" "release (touch)") || exit 2 ;;
    *) echo "unknown variant $name"; exit 2 ;;
    esac
    write_sources "$m" "$c"
    # a running service: publish now (the release is cut and packed on this call)
    if curl -fsS -m 3 "$service/api/loader/public/app/runner/latest" > /dev/null 2>&1; then
        admin refresh '{"app":"runner"}' && admin refresh '{"app":"scope"}' && echo "published"
    fi
    echo "variant $name: makepad $m commercial $c" ;;
up)
    # start the service and the entry script detached (recorded PIDs in BASE/state), return when
    # the runner is starting; `down` stops them by those PIDs
    bin=${3:?up SERVER_BIN}
    mkdir -p "$base/web" "$base/logs" "$base/install" "$base/home" "$base/apps" || exit 2
    if [ -f "$base/state/server.pid" ] && kill -0 "$(cat "$base/state/server.pid")" 2>/dev/null; then :; else
        MAKEPAD_LOADER_DATA="$data" nohup "$bin" --listen "127.0.0.1:$port" --root "$base/web" > "$base/logs/server.log" 2>&1 < /dev/null &
        echo $! > "$base/state/server.pid"
    fi
    n=0
    until curl -fsS -m 2 -o /dev/null "$api/public/app/runner/latest" 2>/dev/null; do
        n=$((n + 1)); [ $n -lt 300 ] || { echo "the service did not come up"; cat "$base/logs/server.log"; exit 2; }
        sleep 1
    done
    admin save '{"email":"fixture@example.org","limit":50,"apps":["scope"],"enabled":true}' || { echo "could not grant the test email"; exit 2; }
    echo "service up at $api"
    [ "${4:-}" = server-only ] && exit 0
    cp "$here/makepad.sh" "$base/home/makepad.sh"
    env MAKEPAD_LOADER_SERVICE="$api" MAKEPAD_LOADER_ROOT="$base/install" MAKEPAD_RUNNER_APPS_DIR="$base/apps" \
        MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_LOADER_EMAIL=fixture@example.org \
        nohup sh "$base/home/makepad.sh" > "$base/logs/entry.log" 2>&1 < /dev/null &
    echo $! > "$base/state/entry.pid"
    echo "entry script pid $(cat "$base/state/entry.pid")" ;;
runner)
    # start the installed loader directly (what the entry script ends with): the runner checks the
    # service for a newer JIT at its start
    root=$base/install
    env MAKEPAD_RUN_HOME="$root" MAKEPAD_LOADER_ROOT="$root" MAKEPAD_LOADER_SERVICE="$api" MAKEPAD_RUNNER_APPS_DIR="$base/apps" \
        MAKEPAD_HIDE_WINDOWS=1 SANDBOX_MUTE=1 MAKEPAD_REMOTE=1 MAKEPAD_LOADER_EMAIL=fixture@example.org \
        PATH="$root/rust/bin:$PATH" CARGO_HOME="$root/cache/cargo-home" RUSTC="$root/rust/bin/rustc" CARGO_INCREMENTAL=0 \
        nohup "$root/cache/loader/$(cat "$root/cache/loader/current" 2>/dev/null || echo src)/target/host/makepad" >> "$base/install/cache/logs/makepad.log" 2>&1 < /dev/null &
    echo $! > "$base/state/runner.pid"
    echo "runner loader pid $!" ;;
log)
    tail -n "${3:-30}" "$base/logs/${4:-entry}.log" ;;
down)
    for f in runner entry server; do
        if [ -f "$base/state/$f.pid" ]; then
            pid=$(cat "$base/state/$f.pid")
            kill -0 "$pid" 2>/dev/null && kill "$pid" && echo "stopped $f $pid"
            rm -f "$base/state/$f.pid"
        fi
    done ;;
*) echo "usage: fixture.sh BASE init|variant|up|log|down"; exit 2 ;;
esac
