#!/bin/bash
# Installs aires, the user's question inbox, as a macOS menu-bar app: ~/Applications/aires.app.
#
#   bash tools/aires/install-mac.sh          (from anywhere; it finds the repository by its own path)
#
# 1. cargo build --release -p makepad-app-aires (the native build; the executable is where cargo
#    says it put it, whatever the target dir),
# 2. assembles "aires.app" under target/aires-install/: Contents/MacOS/makepad-app-aires,
#    Info.plist (nl.makepad.aires, LSUIElement: a menu-bar app, no Dock icon, no menu bar),
#    icon.icns drawn from apps/aires/resources/icon.svg by the system's Quick Look (qlmanage),
#    sized with sips + iconutil (without it the bundle has no icon; a menu-bar app has no Dock tile),
# 3. signs it (tools/macos/sign-app.sh) as nl.makepad.aires with the local identity "makepad local
#    signing" when this Mac has it, else ad-hoc; MAKEPAD_SIGN_IDENTITY overrides. The bundle
#    identity is what macOS notifications need,
# 4. stops the installed copy: its launchd job (launchctl bootout), and a copy opened from
#    Spotlight by the pid it recorded in ~/.makepad/aires/app.pid (only when that pid really runs
#    the installed executable); replaces ~/Applications/aires.app,
# 5. writes its LaunchAgent ~/Library/LaunchAgents/nl.makepad.aires.plist (RunAtLoad; KeepAlive
#    after a crash, not after Quit; the plist aires's "Open at login" switch writes too) and
#    bootstraps it: launchd starts aires now, in launchd's own environment, and at every login.
#
# Nothing else starts aires: the devhub master counts an answer as the user's press in aires
# (`source=tray`) only when it comes from this installed executable started by launchd (this
# LaunchAgent, or Spotlight / Finder). A copy started from an agent's shell (`open` there passes
# the shell's environment) has its answers refused.
set -eu
REPO=$(cd "$(dirname "$0")/../.." && pwd)
APPS="$HOME/Applications"
DEST="$APPS/aires.app"
EXE=makepad-app-aires
ID=nl.makepad.aires
STAGE="$REPO/target/aires-install"
APP="$STAGE/aires.app"
ICONSET="$STAGE/icon.iconset"
SVG="$REPO/apps/aires/resources/icon.svg"
VERSION=$(sed -n 's/^version *= *"\(.*\)"/\1/p' "$REPO/apps/aires/Cargo.toml" | head -1)

cd "$REPO"
# the executable cargo reports (CARGO_TARGET_DIR or a config's target dir moves it)
BIN=$(CARGO_INCREMENTAL=0 cargo build --release -p makepad-app-aires --message-format=json-render-diagnostics \
    | sed -n "s/.*\"executable\":\"\([^\"]*\/$EXE\)\".*/\1/p" | tail -1)
[ -x "$BIN" ] || { echo "cargo reported no $EXE executable" >&2; exit 1; }

rm -rf "$APP" "$ICONSET"
mkdir -p "$APP/Contents/MacOS" "$APP/Contents/Resources" "$ICONSET"
cp "$BIN" "$APP/Contents/MacOS/$EXE"

ICON_KEY=
rm -f "$STAGE/icon.svg.png"
if qlmanage -t -s 1024 -o "$STAGE" "$SVG" > /dev/null 2>&1 && [ -s "$STAGE/icon.svg.png" ]; then
    for size in 16 32 64 128 256 512 1024; do
        sips -z $size $size "$STAGE/icon.svg.png" --out "$ICONSET/s$size.png" > /dev/null
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
    ICON_KEY="<key>CFBundleIconFile</key><string>icon</string>"
else
    echo "no app icon: Quick Look could not draw $SVG"
fi

cat > "$APP/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>CFBundleName</key>
    <string>aires</string>
    <key>CFBundleDisplayName</key>
    <string>aires</string>
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
    $ICON_KEY
    <key>LSUIElement</key>
    <true/>
    <key>NSHighResolutionCapable</key>
    <true/>
</dict>
</plist>
PLIST

# the stable local identity when present (tools/macos/make-signing-identity.sh), else ad-hoc
bash "$REPO/tools/macos/sign-app.sh" "$APP" "$ID"

LABEL=$ID
AGENTS_DIR=${MAKEPAD_LAUNCH_AGENTS_DIR:-$HOME/Library/LaunchAgents}
PLIST="$AGENTS_DIR/$LABEL.plist"
DOMAIN="gui/$(id -u)"

# stop the installed copy: its launchd job, then a copy opened from Spotlight by its recorded pid
# (only if that pid runs the installed executable)
launchctl bootout "$DOMAIN/$LABEL" 2>/dev/null || true
PIDFILE="${MAKEPAD_HOME:-$HOME/.makepad}/aires/app.pid"
if [ -f "$PIDFILE" ]; then
    PID=$(tr -dc '0-9' < "$PIDFILE")
    if [ -n "$PID" ] && [ "$(ps -p "$PID" -o comm= 2>/dev/null || true)" = "$DEST/Contents/MacOS/$EXE" ]; then
        echo "stopping the running aires (pid $PID)"
        kill "$PID"
        for _ in $(seq 1 40); do
            kill -0 "$PID" 2>/dev/null || break
            sleep 0.5
        done
        if kill -0 "$PID" 2>/dev/null; then
            echo "the running aires (pid $PID) did not stop; quit it from its menu-bar icon and run this again" >&2
            exit 1
        fi
    fi
fi

mkdir -p "$APPS"
if [ -e "$DEST" ]; then
    rm -rf "$HOME/Applications/aires.app"
fi
ditto "$APP" "$DEST"
codesign --verify --strict "$DEST"

# the LaunchAgent: the same plist as aires's "Open at login" switch (apps/aires/src/login.rs)
PROGRAM=$(printf '%s' "$DEST/Contents/MacOS/$EXE" | sed -e 's/&/\&amp;/g' -e 's/</\&lt;/g' -e 's/>/\&gt;/g')
mkdir -p "$AGENTS_DIR"
cat > "$PLIST" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>$LABEL</string>
    <key>ProgramArguments</key>
    <array>
        <string>$PROGRAM</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>LimitLoadToSessionType</key>
    <string>Aqua</string>
    <key>KeepAlive</key>
    <dict>
        <key>SuccessfulExit</key>
        <false/>
    </dict>
</dict>
</plist>
PLIST
plutil -lint "$PLIST" > /dev/null

echo
echo "installed $DEST (version $VERSION)"
if launchctl bootstrap "$DOMAIN" "$PLIST"; then
    echo "aires runs now: launchd started it, and starts it at every login."
else
    echo "launchd did not start it now: it starts at the next login, or open aires from Spotlight."
fi
echo "Its icon is in the menu bar with the count of open questions; click it for the questions."
echo "After Quit, open aires from Spotlight. Never start it from an agent's shell: its answers would be refused."
