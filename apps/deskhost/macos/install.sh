#!/bin/bash
# Desk Host on a macOS desktop, all in one, run as the desktop's user in its login session:
#   apps/deskhost/macos/install.sh [--bin <built host>] [--host <LAN address>] [--port 8790] [--fps 120]
#   apps/deskhost/macos/install.sh --settings-only [--host ..] [--port ..] [--fps ..]   (only sets the keys and restarts the running host)
# 1. builds the host (cargo --release, unless --bin names a build) and puts it at ~/.makepad/deskhost/bin/deskhost;
# 2. "Desk Host.app" (bundle id nl.makepad.deskhost, in /Applications, else ~/Applications) is a small launcher
#    (macos/launcher.c) that starts that host as its child. macOS keeps the Screen Recording and Accessibility
#    grants for the app's code identity; the launcher is built once per Mac and never touched again, so the
#    grants survive every host update (a child asks with its parent's grants). An app made anew (--new-app, or a
#    deleted app) needs the grants again;
# 3. writes ~/.makepad/deskhost.conf when there is none, or sets the keys given (--host/--port/--fps);
# 4. (re)loads the LaunchAgent nl.makepad.deskhost: started at login, restarted when it dies.
# It never asks for a grant: the user gives them in System Settings > Privacy & Security (Screen & System
# Audio Recording, Accessibility: "Desk Host"). The host says which one is missing in ~/.makepad/deskhost.status,
# its log (~/.makepad/deskhost.log) and to every viewer.
# Undo: launchctl bootout gui/$(id -u)/nl.makepad.deskhost; then delete ~/Library/LaunchAgents/nl.makepad.deskhost.plist,
# the app and ~/.makepad/deskhost/bin.
set -u
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
ID=nl.makepad.deskhost
HOSTBIN=$HOME/.makepad/deskhost/bin/deskhost
AGENT=$HOME/Library/LaunchAgents/$ID.plist
CONF=$HOME/.makepad/deskhost.conf
bin=; host=; port=; fps=; only=0; new_app=0
while [ $# -gt 0 ]; do
  case $1 in
    --bin) bin=$2; shift 2;;
    --host) host=$2; shift 2;;
    --port) port=$2; shift 2;;
    --fps) fps=$2; shift 2;;
    --settings-only) only=1; shift;;
    --new-app) new_app=1; shift;;
    *) echo "unknown argument $1" >&2; exit 2;;
  esac
done
fail() { echo "DESKHOST-INSTALL-FAILED: $*"; exit 1; }

set_keys() {
  for kv in "host=$host" "port=$port" "fps=$fps"; do
    k=${kv%%=*}; v=${kv#*=}
    [ -n "$v" ] || continue
    if grep -q "^$k *=" "$CONF"; then
      sed "s|^$k *=.*|$k = $v|" "$CONF" > "$CONF.new" && mv "$CONF.new" "$CONF"
    else
      echo "$k = $v" >> "$CONF"
    fi
    echo "set $k = $v"
  done
}
if [ $only = 1 ]; then
  [ -s "$CONF" ] || fail "no $CONF"
  set_keys
  launchctl kickstart -k "gui/$(id -u)/$ID" || fail "kickstart"
  sleep 3
  echo "status: $(cat "$HOME/.makepad/deskhost.status" 2>/dev/null)"
  echo "DESKHOST-SETTINGS-OK"
  exit 0
fi

# 1. the build
if [ -z "$bin" ]; then
  (cd "$ROOT" && CARGO_INCREMENTAL=0 cargo build --release -p makepad-app-deskhost) || fail "build"
  bin=${CARGO_TARGET_DIR:-$ROOT/target}/release/makepad-app-deskhost
fi
[ -x "$bin" ] || fail "no host build at $bin"

# 2. the host itself, swapped in (a running host keeps its own copy)
mkdir -p "$(dirname "$HOSTBIN")" || fail "bin dir"
cp "$bin" "$HOSTBIN.new" && mv "$HOSTBIN.new" "$HOSTBIN" || fail "copy host"

# the app: made once, then left alone (its identity holds the grants)
if [ -w /Applications ]; then APPS=/Applications; else APPS=$HOME/Applications; mkdir -p "$APPS"; fi
APP="$APPS/Desk Host.app"
if [ $new_app = 1 ] || [ ! -x "$APP/Contents/MacOS/Desk Host" ]; then
  STAGE="$APPS/.Desk Host.app.new"
  rm -rf "$STAGE"
  mkdir -p "$STAGE/Contents/MacOS" || fail "stage"
  cc -O2 -Wall -o "$STAGE/Contents/MacOS/Desk Host" "$ROOT/apps/deskhost/macos/launcher.c" || fail "launcher build"
  cat > "$STAGE/Contents/Info.plist" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleIdentifier</key><string>$ID</string>
  <key>CFBundleName</key><string>Desk Host</string>
  <key>CFBundleDisplayName</key><string>Desk Host</string>
  <key>CFBundleExecutable</key><string>Desk Host</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>1</string>
  <key>CFBundleVersion</key><string>1</string>
  <key>LSMinimumSystemVersion</key><string>13.0</string>
  <key>LSUIElement</key><true/>
  <key>NSScreenCaptureUsageDescription</key><string>Desk Host shows this screen to your Desk Link viewers.</string>
</dict>
</plist>
EOF
  codesign --force --sign - --identifier "$ID" "$STAGE" || fail "codesign"
  launchctl bootout "gui/$(id -u)/$ID" 2>/dev/null
  rm -rf "$APP"
  mv "$STAGE" "$APP" || fail "app"
  echo "made $APP (new identity: give it the grants)"
fi
codesign --verify "$APP" || fail "codesign verify"
echo "app identity: $(codesign -d -r- "$APP" 2>&1 | sed -n "s/^# designated => //p")"

# 3. settings (the fleet identity itself is installed apart, never by this script: tools/fleet-identity.md)
mkdir -p "$HOME/.makepad"
if [ ! -s "$CONF" ]; then
  cat > "$CONF" <<EOF
# Desk Host (written by apps/deskhost/macos/install.sh); keys in apps/deskhost/src/settings.rs
enabled = on
host = ${host:-0.0.0.0}
port = ${port:-8790}
fps = ${fps:-120}
EOF
  echo "wrote $CONF"
else
  set_keys
fi
fleet=${MAKEPAD_FLEET_DIR:-$HOME/.makepad/fleet}
[ -s "$fleet/id.key" ] && [ -s "$fleet/id.cred" ] && [ -s "$fleet/root.pub" ] || echo "note: no fleet identity (id.key, id.cred, root.pub) in $fleet yet: the host waits for it (checks every 10 s)"

# 4. (re)load the agent: the same plist restarts the running host; a new one is loaded anew
# (bootout returns before launchd has let go of the job: wait for that, a bootstrap at once fails)
loaded() { launchctl print "gui/$(id -u)/$ID" >/dev/null 2>&1; }
mkdir -p "$HOME/Library/LaunchAgents"
cat > "$AGENT.new" <<EOF
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>Label</key><string>$ID</string>
  <key>ProgramArguments</key><array><string>$APP/Contents/MacOS/Desk Host</string></array>
  <key>RunAtLoad</key><true/>
  <key>KeepAlive</key><dict><key>SuccessfulExit</key><false/></dict>
  <key>ThrottleInterval</key><integer>10</integer>
  <key>ProcessType</key><string>Interactive</string>
  <key>LimitLoadToSessionType</key><string>Aqua</string>
  <key>StandardErrorPath</key><string>$HOME/.makepad/deskhost.stderr.log</string>
</dict>
</plist>
EOF
if loaded && cmp -s "$AGENT.new" "$AGENT"; then
  rm -f "$AGENT.new"
  launchctl kickstart -k "gui/$(id -u)/$ID" || fail "kickstart"
else
  launchctl bootout "gui/$(id -u)/$ID" 2>/dev/null
  for i in 1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 20; do loaded || break; sleep 0.5; done
  mv "$AGENT.new" "$AGENT" || fail "agent plist"
  ok=0
  for i in 1 2 3 4 5; do launchctl bootstrap "gui/$(id -u)" "$AGENT" 2>/dev/null && { ok=1; break; }; sleep 1; done
  [ $ok = 1 ] || fail "launchctl bootstrap"
fi
sleep 3
echo "status: $(cat "$HOME/.makepad/deskhost.status" 2>/dev/null)"
echo "DESKHOST-INSTALL-OK $APP"
