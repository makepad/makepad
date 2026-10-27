#!/bin/bash
# Installs Desk Link as a regular macOS app: ~/Applications/Desk Link.app (Dock icon, menu bar,
# can be kept in the Dock).
#
#   bash tools/desklink/install-mac.sh          (from anywhere; it finds the repository by its own path)
#
# 1. cargo build --release -p makepad-app-desklink (the native build, this Mac's default features),
# 2. assembles "Desk Link.app" under target/desklink-install/: Contents/MacOS/makepad-app-desklink,
#    Info.plist (nl.makepad.desklink, a regular app: no LSUIElement), icon.icns made from
#    apps/desklink/resources/icon_1024.png (rendered from icon.svg by tools/app_icons/render.sh)
#    with the system's sips + iconutil,
# 3. signs it (tools/macos/sign-app.sh) as nl.makepad.desklink with the local identity "makepad local
#    signing" when this Mac has it (made once by tools/macos/make-signing-identity.sh; the same code
#    identity every build, so macOS privacy answers stay), else ad-hoc; MAKEPAD_SIGN_IDENTITY overrides,
# 4. stops the installed copy (by the pid it recorded in ~/.makepad/desklink/app.pid, and only
#    when that pid really runs the installed executable), replaces ~/Applications/Desk Link.app,
# 5. prints how to start it.
#
# Opening the app again (Dock, Finder, open) brings the running one to the front; closing its
# window hides it and the app stays in the Dock, a click on the Dock icon shows it again; Cmd+Q quits.
set -eu
REPO=$(cd "$(dirname "$0")/../.." && pwd)
APPS="$HOME/Applications"
DEST="$APPS/Desk Link.app"
EXE=makepad-app-desklink
ID=nl.makepad.desklink
STAGE="$REPO/target/desklink-install"
APP="$STAGE/Desk Link.app"
ICONSET="$STAGE/icon.iconset"
PNG="$REPO/apps/desklink/resources/icon_1024.png"
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO/apps/desklink/Cargo.toml" | head -1)
TARGET_DIR=${CARGO_TARGET_DIR:-$REPO/target}

cd "$REPO"
CARGO_INCREMENTAL=0 cargo build --release -p makepad-app-desklink

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
    <string>Desk Link</string>
    <key>CFBundleDisplayName</key>
    <string>Desk Link</string>
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
    <key>LSApplicationCategoryType</key>
    <string>public.app-category.utilities</string>
    <key>NSHumanReadableCopyright</key>
    <string>Makepad, MIT licence</string>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST
plutil -lint "$APP/Contents/Info.plist" > /dev/null

# the stable local identity when present (tools/macos/make-signing-identity.sh), else ad-hoc
bash "$REPO/tools/macos/sign-app.sh" "$APP" "$ID"

# stop the installed copy: its recorded pid, and only if that pid runs the installed executable
PIDFILE="${MAKEPAD_HOME:-$HOME/.makepad}/desklink/app.pid"
if [ -f "$PIDFILE" ]; then
    PID=$(tr -dc '0-9' < "$PIDFILE")
    if [ -n "$PID" ] && [ "$(ps -p "$PID" -o comm= 2>/dev/null || true)" = "$DEST/Contents/MacOS/$EXE" ]; then
        echo "stopping the running Desk Link (pid $PID)"
        kill "$PID"
        for _ in $(seq 1 40); do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "$PID" 2>/dev/null; then
            echo "the running Desk Link (pid $PID) did not stop; quit it (Cmd+Q) and run this again" >&2
            exit 1
        fi
    fi
fi

mkdir -p "$APPS"
if [ -e "$DEST" ]; then
    rm -rf "$HOME/Applications/Desk Link.app"
fi
ditto "$APP" "$DEST"
codesign --verify --strict "$DEST"
# Launch Services learns the new bundle now (its name and icon in the Dock and Finder)
/System/Library/Frameworks/CoreServices.framework/Frameworks/LaunchServices.framework/Support/lsregister -f "$DEST" || true

echo
echo "installed $DEST (version $VERSION)"
echo "start it:  open ~/Applications/\"Desk Link.app\""
echo "keep it in the Dock: right-click its Dock icon, Options, Keep in Dock."
