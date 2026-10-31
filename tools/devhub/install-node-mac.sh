#!/bin/bash
# The devhub NODE on a macOS box (the Mac mini), run as its desktop user in its login session (the Desk Host
# pattern: a LaunchAgent, started at login and restarted when it dies). Never on the user's Mac: that one is
# the master (the devhub Dock app: `makepad run makepad-app-devhub`, its "Open at login" LaunchAgent).
#   tools/devhub/install-node-mac.sh [--tree <dir>] [--name mini] [--host <LAN address>]
# 1. writes ~/.makepad/devhub/node-start.sh: `cargo run --release -p makepad-app-devhub` in the node's one tree
#    (--tree; default ~/devhub/tree when it exists, the tree the leases run in, else this checkout), so every
#    restart runs the tree's current code (no installed copy goes stale);
# 2. writes ~/.makepad/devhub/node.conf when there is none (name, host, port 8797, discovery = off: a LAN
#    broadcast would ask the Local Network question, which nobody answers on a headless Mac; listening and
#    accepting does not ask, and the master dials the node from its nodes.conf);
# 3. (re)loads the LaunchAgent nl.makepad.devhub-node with MAKEPAD_DEVHUB_NODE_CONF set (node mode), restarted on
#    every exit (a clean one too: a restart for new code is a clean exit).
# The node proves itself with the machine's fleet identity (~/.makepad/fleet, tools/fleet-identity.md);
# without one it serves this machine's JITs locally only, and its log says so.
# Role and budgets are the master's: `makepad devhub node mini role bench` (kept in lease.conf here).
# Undo: launchctl bootout gui/$(id -u)/nl.makepad.devhub-node; delete ~/Library/LaunchAgents/nl.makepad.devhub-node.plist
# and ~/.makepad/devhub/node-start.sh.
set -u
ROOT=$(cd "$(dirname "$0")/../.." && pwd)
ID=nl.makepad.devhub-node
H=${MAKEPAD_HOME:-$HOME/.makepad}
START=$H/devhub/node-start.sh
CONF=$H/devhub/node.conf
AGENT=$HOME/Library/LaunchAgents/$ID.plist
LOG=$H/devhub/node.log
tree=; name=; host=
while [ $# -gt 0 ]; do
  case $1 in
    --tree) tree=$2; shift 2;;
    --name) name=$2; shift 2;;
    --host) host=$2; shift 2;;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac
done
fail() { echo "DEVHUB-NODE-INSTALL-FAILED: $*"; exit 1; }
[ "$(uname -s)" = Darwin ] || fail "this is the macOS node installer"

# 1. the start script: the hub through cargo, from the node's one tree
if [ -z "$tree" ]; then
  if [ -e "$HOME/devhub/tree/apps/devhub/Cargo.toml" ]; then tree=$HOME/devhub/tree; else tree=$ROOT; fi
fi
[ -e "$tree/apps/devhub/Cargo.toml" ] || fail "no makepad tree at $tree"
mkdir -p "$H/devhub"
chmod 700 "$H/devhub"
cat > "$START.new" <<START
#!/bin/bash
# the devhub node (tools/devhub/install-node-mac.sh): cargo run in the node's tree, so a restart runs its current
# code; launchd restarts it on every exit
cd "$tree" || exit 1
echo "---- \$(date) devhub node start in $tree"
# a tree a lease left at a commit from before this package has no hub to start, and this would loop for good
grep -q '"apps/devhub"' Cargo.toml || git checkout -q --detach "\$(sed -n 's/^makepad //p' FLEET-REVISION.txt)"
export CARGO_INCREMENTAL=0 CARGO_NET_OFFLINE=true
exec cargo run --release -p makepad-app-devhub
START
chmod 755 "$START.new" && mv "$START.new" "$START" || fail "write $START"

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
  <key>ProgramArguments</key><array><string>/bin/bash</string><string>$START</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>MAKEPAD_DEVHUB_NODE_CONF</key><string>$CONF</string>
    <key>PATH</key><string>$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>5</integer>
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
