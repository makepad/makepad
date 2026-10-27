#!/bin/bash
# The devhub NODE on a macOS box (the Mac mini), run as its desktop user in its login session (the Desk Host
# pattern: a LaunchAgent, started at login and restarted when it dies). Never on the user's Mac: that one is
# the master (tools/devhub/install-mac.sh).
#   tools/devhub/install-node-mac.sh [--bin <built makepad-app-devhub>] [--name mini230] [--host <LAN address>]
# 1. builds makepad-app-devhub (cargo --release, unless --bin names a build) and puts it at
#    ~/.makepad/devhub/bin/makepad-app-devhub;
# 2. writes ~/.makepad/devhub/node.conf when there is none (name, host, port 8797, discovery = off: a LAN
#    broadcast would ask the Local Network question, which nobody answers on a headless Mac; listening and
#    accepting does not ask, and the master dials the node from its nodes.conf);
# 3. (re)loads the LaunchAgent nl.makepad.devhub-node with MAKEPAD_DEVHUB_NODE_CONF set (node mode).
# The node proves itself with the machine's fleet identity (~/.makepad/fleet, tools/fleet-identity.md);
# without one it serves this machine's JITs locally only, and its log says so.
# Role and budgets are the master's: `makepad devhub node mini230 role bench` (kept in lease.conf here).
# Undo: launchctl bootout gui/$(id -u)/nl.makepad.devhub-node; delete ~/Library/LaunchAgents/nl.makepad.devhub-node.plist
# and ~/.makepad/devhub/bin.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ID=nl.makepad.devhub-node
H=${MAKEPAD_HOME:-$HOME/.makepad}
BIN=$H/devhub/bin/makepad-app-devhub
CONF=$H/devhub/node.conf
AGENT=$HOME/Library/LaunchAgents/$ID.plist
LOG=$H/devhub/node.log
bin=; name=; host=
while [ $# -gt 0 ]; do
  case $1 in
    --bin) bin=$2; shift 2;;
    --name) name=$2; shift 2;;
    --host) host=$2; shift 2;;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac
done
fail() { echo "DEVHUB-NODE-INSTALL-FAILED: $*"; exit 1; }
[ "$(uname -s)" = Darwin ] || fail "this is the macOS node installer"

# 1. the build
if [ -z "$bin" ]; then
  (cd "$ROOT" && CARGO_INCREMENTAL=0 cargo build --release -p makepad-app-devhub) || fail "cargo build"
  bin=${CARGO_TARGET_DIR:-$ROOT/target}/release/makepad-app-devhub
fi
[ -x "$bin" ] || fail "no binary at $bin"
mkdir -p "$H/devhub/bin"
chmod 700 "$H/devhub"
cp "$bin" "$BIN.new" && mv "$BIN.new" "$BIN" || fail "copy $bin"
codesign -f -s - --identifier "$ID" "$BIN" 2>/dev/null || true

# 2. the node's conf (kept when it exists)
if [ ! -s "$CONF" ]; then
  {
    echo "# devhub node (tools/devhub/install-node-mac.sh)"
    [ -n "$name" ] && echo "name = $name"
    [ -n "$host" ] && echo "host = $host"
    echo "port = 8797"
    echo "discovery = off"
  } > "$CONF"
  echo "wrote $CONF"
else
  echo "kept $CONF"
fi

# 3. the LaunchAgent
mkdir -p "$HOME/Library/LaunchAgents"
cat > "$AGENT.new" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$ID</string>
  <key>ProgramArguments</key><array><string>$BIN</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>MAKEPAD_DEVHUB_NODE_CONF</key><string>$CONF</string>
    <key>PATH</key><string>$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ProcessType</key><string>Interactive</string>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>StandardOutPath</key><string>$LOG</string>
  <key>StandardErrorPath</key><string>$LOG</string>
</dict>
</plist>
PLIST
plutil -lint "$AGENT.new" > /dev/null || fail "plist lint"
mv "$AGENT.new" "$AGENT"
launchctl bootout "gui/$(id -u)/$ID" 2> /dev/null
sleep 1
launchctl bootstrap "gui/$(id -u)" "$AGENT" || fail "launchctl bootstrap"
sleep 4
launchctl print "gui/$(id -u)/$ID" | grep -E "^\s+(state|pid) =" | head -2
tail -3 "$LOG" 2>/dev/null
echo "DEVHUB-NODE-INSTALL-OK"
