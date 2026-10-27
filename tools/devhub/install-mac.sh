#!/bin/bash
# Installs devhub (the master: inbox + explorer of every JIT on the LAN) as a macOS menu-bar app:
# ~/Applications/devhub.app.
#
#   bash tools/devhub/install-mac.sh          (from anywhere; it finds the repository by its own path)
#
# 1. cargo build --release -p makepad-app-devhub (the native build, this Mac's default features),
# 2. assembles "devhub.app" under target/devhub-install/: Contents/MacOS/makepad-app-devhub,
#    Info.plist (nl.makepad.devhub, LSUIElement: a menu-bar app, no Dock icon, no menu bar),
#    icon.icns made from apps/devhub/resources/icon_1024.png with the system's sips + iconutil,
# 3. signs it (tools/macos/sign-app.sh) as nl.makepad.devhub with the local identity "makepad local
#    signing" when this Mac has it (made once by tools/macos/make-signing-identity.sh; the same code
#    identity every build, so macOS privacy answers stay), else ad-hoc; MAKEPAD_SIGN_IDENTITY overrides,
# 4. writes the master's node list ~/.makepad/devhub/nodes.conf when there is none (an existing
#    one is never touched): pool nodes dev=on, the user's WMs dev=off,
# 5. stops the installed copy (by the pid it recorded in ~/.makepad/devhub/app.pid, and only
#    when that pid really runs the installed executable), replaces ~/Applications/devhub.app,
# 6. prints how to start it.
#
# The master dials the node hubs (TCP to 10.0.0.x). macOS asks "devhub would like to find devices on
# your local network" -> Allow: once when signed with the local identity, after every install when ad-hoc.
set -eu
REPO=$(cd "$(dirname "$0")/../.." && pwd)
APPS="$HOME/Applications"
DEST="$APPS/devhub.app"
EXE=makepad-app-devhub
ID=nl.makepad.devhub
STAGE="$REPO/target/devhub-install"
APP="$STAGE/devhub.app"
ICONSET="$STAGE/icon.iconset"
PNG="$REPO/apps/devhub/resources/icon_1024.png"
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO/apps/devhub/Cargo.toml" | head -1)
TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target}
HOME_DIR="${MAKEPAD_HOME:-$HOME/.makepad}/devhub"

cd "$REPO"
CARGO_INCREMENTAL=0 cargo build --release -p makepad-app-devhub

mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$ICONSET"
cp "$TARGET_DIR/release/$EXE" "$APP/Contents/MacOS/$EXE"

for size in 16 32 64 128 256 512 1024; do
    sips -z $size $size "$PNG" --out "$ICONSET/s$size.png" > /dev/null
done
# the iconset names iconutil wants: the 1x and 2x pictures of each point size
cp "$ICONSET/s16.png" "$ICONSET/icon_16x16.png"
cp "$ICONSET/s32.png" "$ICONSET/icon_16x16@2x.png"
cp "$ICONSET/s32.png" "$ICONSET/icon_32x32.png"
cp "$ICONSET/s64.png" "$ICONSET/icon_32x32@2x.png"
cp "$ICONSET/s128.png" "$ICONSET/icon_128x128.png"
cp "$ICONSET/s256.png" "$ICONSET/icon_128x128@2x.png"
cp "$ICONSET/s256.png" "$ICONSET/icon_256x256.png"
cp "$ICONSET/s512.png" "$ICONSET/icon_256x256@2x.png"
cp "$ICONSET/s512.png" "$ICONSET/icon_512x512.png"
cp "$ICONSET/s1024.png" "$ICONSET/icon_512x512@2x.png"
iconutil -c icns "$ICONSET" -o "$APP/Contents/Resources/icon.icns"

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>devhub</string>
    <key>CFBundleDisplayName</key>
    <string>devhub</string>
    <key>CFBundleIdentifier</key>
    <string>$ID</string>
    <key>CFBundleExecutable</key>
    <string>$EXE</string>
    <key>CFBundlePackageType</key>
    <string>APPL</string>
    <key>CFBundleInfoDictionaryVersion</key>
    <string>6.0</string>
    <key>CFBundleShortVersionString</key>
    <string>$VERSION</string>
    <key>CFBundleVersion</key>
    <string>$VERSION</string>
    <key>CFBundleIconFile</key>
    <string>icon</string>
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
    <key>NSLocalNetworkUsageDescription</key>
    <string>devhub connects to the devhub nodes on your local network to show their running programs and events.</string>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" > /dev/null

# the stable local identity when present (tools/macos/make-signing-identity.sh), else ad-hoc
bash "$REPO/tools/macos/sign-app.sh" "$APP" "$ID"

# the master's node list (its seeds and each node's dev switch); never overwrites the user's
if [ ! -f "$HOME_DIR/nodes.conf" ]; then
    mkdir -p "$HOME_DIR"
    cat > "$HOME_DIR/nodes.conf" <<'CONF'
# devhub master: the node hubs it dials, and each node's dev switch (dev=on: leases for
# development and testing may run there; off: observe only).
# <name> addr=<ip[:port]> dev=on|off      (port 8797 when left out)
100 addr=10.0.0.100 dev=on    # pool
203 addr=10.0.0.203 dev=on    # pool (suite; also hosts fleet tests)
217 addr=10.0.0.217 dev=on    # pool
165 addr=10.0.0.165 dev=off   # the user's WM
166 addr=10.0.0.166 dev=off   # the user's WM
235 addr=10.0.0.235 dev=off   # the user's WM
# devices (phones, headsets, the Mac mini) are added here with dev=off
CONF
    echo "wrote $HOME_DIR/nodes.conf"
fi

# stop the installed copy: its recorded pid, and only if that pid runs the installed executable
PIDFILE="$HOME_DIR/app.pid"
if [ -f "$PIDFILE" ]; then
    PID=$(tr -dc '0-9' < "$PIDFILE")
    if [ -n "$PID" ] && [ "$(ps -p "$PID" -o comm= 2>/dev/null || true)" = "$DEST/Contents/MacOS/$EXE" ]; then
        echo "stopping the running devhub (pid $PID)"
        kill "$PID"
        for _ in $(seq 1 40); do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "$PID" 2>/dev/null; then
            echo "the running devhub (pid $PID) did not stop; quit it from its menu-bar icon and run this again" >&2
            exit 1
        fi
    fi
fi

mkdir -p "$APPS"
if [ -e "$DEST" ]; then
    rm -rf "$HOME/Applications/devhub.app"
fi
ditto "$APP" "$DEST"
codesign --verify --strict "$DEST"

echo
echo "installed $DEST (version $VERSION)"
echo "start it:  open ~/Applications/devhub.app"
echo "its icon (an eye with three nodes) appears in the menu bar; click it for the inbox and the explorer."
echo "Open at login is a switch in the panel."
echo "macOS asks once: \"devhub\" would like to find devices on your local network -> Allow."
echo "(it dials the node hubs in $HOME_DIR/nodes.conf; System Settings > Privacy & Security > Local Network lists it)"
