#!/bin/bash
# Installs the AI Hub as a macOS menu-bar app: ~/Applications/AI Hub.app.
#
#   bash tools/aihub/install-mac.sh          (from anywhere; it finds the repository by its own path)
#
# 1. cargo build --release -p makepad-app-aihub (the native build, this Mac's default features),
# 2. assembles "AI Hub.app" under target/aihub-install/: Contents/MacOS/makepad-app-aihub,
#    Info.plist (nl.makepad.aihub, LSUIElement: a menu-bar app, no Dock icon, no menu bar),
#    icon.icns made from apps/aihub/resources/icon_1024.png with the system's sips + iconutil,
# 3. signs it (tools/macos/sign-app.sh) as nl.makepad.aihub with the local identity "makepad local
#    signing" when this Mac has it (made once by tools/macos/make-signing-identity.sh; the same code
#    identity every build, so macOS privacy answers stay), else ad-hoc; MAKEPAD_SIGN_IDENTITY overrides,
# 4. stops the installed copy (by the pid it recorded in ~/.makepad/ai-hub/app.pid, and only
#    when that pid really runs the installed executable), replaces ~/Applications/AI Hub.app,
# 5. prints how to start it.
#
# The AI Hub reads the Claude login from the keychain like the headless hub always did; macOS
# may ask once to allow it (click "Always Allow"); with the local signing identity that answer
# holds across reinstalls.
set -eu
REPO=$(cd "$(dirname "$0")/../.." && pwd)
APPS="$HOME/Applications"
DEST="$APPS/AI Hub.app"
EXE=makepad-app-aihub
ID=nl.makepad.aihub
STAGE="$REPO/target/aihub-install"
APP="$STAGE/AI Hub.app"
ICONSET="$STAGE/icon.iconset"
PNG="$REPO/apps/aihub/resources/icon_1024.png"
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO/apps/aihub/Cargo.toml" | head -1)
TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target}

cd "$REPO"
CARGO_INCREMENTAL=0 cargo build --release -p makepad-app-aihub

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
    <string>AI Hub</string>
    <key>CFBundleDisplayName</key>
    <string>AI Hub</string>
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
</dict>
</plist>
PLIST

# the stable local identity when present (tools/macos/make-signing-identity.sh), else ad-hoc
bash "$REPO/tools/macos/sign-app.sh" "$APP" "$ID"

# stop the installed copy: its recorded pid, and only if that pid runs the installed executable
PIDFILE="${MAKEPAD_HOME:-$HOME/.makepad}/ai-hub/app.pid"
if [ -f "$PIDFILE" ]; then
    PID=$(tr -dc '0-9' < "$PIDFILE")
    if [ -n "$PID" ] && [ "$(ps -p "$PID" -o comm= 2>/dev/null || true)" = "$DEST/Contents/MacOS/$EXE" ]; then
        echo "stopping the running AI Hub (pid $PID)"
        kill "$PID"
        for _ in $(seq 1 40); do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "$PID" 2>/dev/null; then
            echo "the running AI Hub (pid $PID) did not stop; quit it from its menu-bar icon and run this again" >&2
            exit 1
        fi
    fi
fi

mkdir -p "$APPS"
if [ -e "$DEST" ]; then
    rm -rf "$HOME/Applications/AI Hub.app"
fi
ditto "$APP" "$DEST"
codesign --verify --strict "$DEST"

echo
echo "installed $DEST (version $VERSION)"
echo "start it:  open ~/Applications/\"AI Hub.app\""
echo "its icon appears in the menu bar; click it for the panel, right-click for the menu."
echo "Open at login is a switch in the panel."
