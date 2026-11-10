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
# --script-only: rewrites the start script alone (a lease can run this: the hub it runs under keeps running).
# --ai-hub: this node's AI hub instead, for a compute Mac no WM supervises, run the way a WM runs it
#   (apps/wm/src/hub_app.rs): the app (apps/aihub) with the node's settings (~/.makepad/wm/ai-hub.conf, written
#   when there is none: the Mac hub's port 8124 on every address), its pid and program in ~/.makepad/wm/ai-hub.state
#   (the update roll's restart reads them) and its output in ~/.makepad/wm/ai-hub.log, under the LaunchAgent
#   nl.makepad.ai-hub (restarted on every exit). It runs its kept copy ~/.makepad/devhub/kept/makepad-app-aihub,
#   never `cargo run`: with KeepAlive a tip that does not build would rebuild at every start with no hub serving.
#   The first copy is built here, from the tree; then the deployment's prebuild (sync.conf `build = apps`,
#   `prebuild = aihub`) replaces it only once its build succeeds (libs/devhub/src/sync/build.rs `keep`), and the
#   roll restarts the hub onto it. Undo: launchctl bootout gui/$(id -u)/nl.makepad.ai-hub; delete its plist,
#   ~/.makepad/devhub/ai-hub-start.sh and ~/.makepad/devhub/kept/makepad-app-aihub.
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
tree=; name=; host=; script_only=; ai_hub=
while [ $# -gt 0 ]; do
  case $1 in
    --tree) tree=$2; shift 2;;
    --name) name=$2; shift 2;;
    --host) host=$2; shift 2;;
    --script-only) script_only=1; shift;;
    --ai-hub) ai_hub=1; shift;;
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

if [ -n "$ai_hub" ]; then
  AID=nl.makepad.ai-hub
  AAGENT=$HOME/Library/LaunchAgents/$AID.plist
  ASTART=$H/devhub/ai-hub-start.sh
  ACONF=$H/wm/ai-hub.conf
  KEPT=$H/devhub/kept/makepad-app-aihub
  mkdir -p "$H/wm" "$H/devhub/kept"
  # the first kept copy: the prebuild's build of it (the registry's shared row: its package, no lane target
  # dir), in the tree's own target/
  if [ ! -x "$KEPT" ]; then
    ( cd "$tree" || exit 1
      [ -f "$H/devhub/node.env" ] && { set -a; . "$H/devhub/node.env"; set +a; }
      unset CARGO_TARGET_DIR
      CARGO_NET_OFFLINE=true cargo build --release -p makepad-app-aihub ) || fail "the AI hub does not build in $tree"
    cp -p "$tree/target/release/makepad-app-aihub" "$KEPT.new" && mv "$KEPT.new" "$KEPT" || fail "keep $KEPT"
  fi
  if [ ! -s "$ACONF" ]; then
    {
      echo "# this node's AI hub (tools/devhub/install-node-mac.sh --ai-hub; keys: libs/ai/hub/src/node_conf.rs)"
      echo "port = 8124"
      echo "host = 0.0.0.0"
      echo "nice = 0"
    } > "$ACONF"
    echo "wrote $ACONF"
  else
    echo "kept $ACONF"
  fi
  cat > "$ASTART.new" <<START
#!/bin/bash
# this node's AI hub (tools/devhub/install-node-mac.sh --ai-hub): its kept copy, the last build of it that
# succeeded, with the node's settings; launchd restarts it on every exit
echo "---- \$(date) AI hub start: $KEPT"
[ -f "$H/devhub/node.env" ] && { set -a; . "$H/devhub/node.env"; set +a; }
[ -x "$KEPT" ] || { echo "no kept AI hub at $KEPT: install-node-mac.sh --ai-hub builds the first"; exit 1; }
# what a WM's supervisor records: its pid (exec keeps it) and what it was started as
printf 'pid = %s\nidentity = %s\n' "\$\$" "$KEPT" > "$H/wm/ai-hub.state.new" && mv "$H/wm/ai-hub.state.new" "$H/wm/ai-hub.state"
export MAKEPAD_AI_HUB_NODE_CONF="$ACONF"
cd "$H" || exit 1
exec "$KEPT"
START
  chmod 755 "$ASTART.new" && mv "$ASTART.new" "$ASTART" || fail "write $ASTART"
  mkdir -p "$HOME/Library/LaunchAgents"
  cat > "$AAGENT.new" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$AID</string>
  <key>ProgramArguments</key><array><string>/bin/bash</string><string>$ASTART</string></array>
  <key>EnvironmentVariables</key><dict>
    <key>PATH</key><string>$HOME/.cargo/bin:/opt/homebrew/bin:/usr/local/bin:/usr/bin:/bin:/usr/sbin:/sbin</string>
  </dict>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><true/>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Interactive</string>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>StandardOutPath</key><string>$H/wm/ai-hub.log</string>
  <key>StandardErrorPath</key><string>$H/wm/ai-hub.log</string>
</dict>
</plist>
PLIST
  plutil -lint "$AAGENT.new" > /dev/null || fail "plist lint"
  mv "$AAGENT.new" "$AAGENT"
  launchctl bootout "gui/$(id -u)/$AID" 2> /dev/null
  sleep 1
  launchctl bootstrap "gui/$(id -u)" "$AAGENT" || fail "launchctl bootstrap $AID"
  sleep 4
  launchctl print "gui/$(id -u)/$AID" | grep -E "^\s+(state|pid) =" | head -2
  tail -3 "$H/wm/ai-hub.log" 2>/dev/null
  echo "DEVHUB-AI-HUB-INSTALL-OK"
  exit 0
fi
cat > "$START.new" <<START
#!/bin/bash
# the devhub node (tools/devhub/install-node-mac.sh): cargo run in the node's tree, so a restart runs its current
# code; launchd restarts it on every exit
cd "$tree" || exit 1
echo "---- \$(date) devhub node start in $tree"
# the workspace's release profile as it is (incremental): the hub builds itself in this environment
export CARGO_NET_OFFLINE=true
# this box's own settings (lines NAME=value, e.g. DEVELOPER_DIR for the toolchain its builds use)
[ -f "$H/devhub/node.env" ] && { set -a; . "$H/devhub/node.env"; set +a; }
# a tree a lease left at a commit the hub does not build at (from before its package) would loop for
# good: it goes to the shared commits, and the hub, once up, puts a lease still holding it back
to_synced() {
  to=\$(git rev-parse -q --verify refs/devhub/synced) || return 0
  [ "\$(git rev-parse -q --verify HEAD)" = "\$to" ] && return 0
  echo "the hub does not build at \$(git rev-parse --short HEAD) in \$PWD: back at the shared \$to"
  git read-tree --reset -u "\$to" && git update-ref --no-deref HEAD "\$to" && git update-ref refs/devhub/checkout "\$to"
}
if ! cargo build --release -p makepad-app-devhub; then
  to_synced
  [ -e apps/commercial/.git ] && (cd apps/commercial && to_synced)
fi
exec cargo run --release -p makepad-app-devhub
START
chmod 755 "$START.new" && mv "$START.new" "$START" || fail "write $START"
if [ -n "$script_only" ]; then echo "DEVHUB-NODE-INSTALL-OK"; exit 0; fi

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
