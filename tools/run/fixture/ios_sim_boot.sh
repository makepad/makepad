#!/bin/bash
# ios_sim_boot.sh [<package>...]: makepad run on the iOS simulator. Per package: the host app
# (`cargo makepad apple ios build-host-sim`: the `makepad` binary for aarch64-apple-ios-sim, the
# package's packed source, the conf), signed ad hoc, installed on a simulator and launched with a
# HAL script that quits after the first frame. The app compiles on its first start and runs in
# no-codegen mode (the interpreter), as on a device. Prints per package the bundle's sizes, the
# time to the first frame and the exit status, then "IOSSIM-OK" or "IOSSIM-FAIL: <what>".
# Run from the workspace root on a Mac with Xcode (a devhub job on the CI Mac); it boots an
# existing iPhone simulator when none is booted (and shuts it down again), never creates or
# erases one.
set -u
export PATH=$HOME/.cargo/bin:$PATH CARGO_INCREMENTAL=0
pkgs=${*:-makepad-example-counter}
# Xcode's first-launch setup (its system components) asks for an administrator's password: a
# simctl before it is done puts an authorization dialog on the box and waits. Never start one then.
xcodebuild -checkFirstLaunchStatus > /dev/null 2>&1 || { echo "IOSSIM-FAIL: Xcode needs its first-launch setup here (sudo xcodebuild -runFirstLaunch, an administrator's step); simctl would prompt"; exit 75; }
xcrun simctl help > /dev/null 2>&1 || { echo "IOSSIM-FAIL: no simctl (Xcode) here"; exit 75; }
udid=$(xcrun simctl list devices booted | sed -n 's/.*(\([0-9A-F-]\{36\}\)) (Booted).*/\1/p' | head -1)
booted_here=
if [ -z "$udid" ]; then
    udid=$(xcrun simctl list devices available | grep -E 'iPhone' | sed -n 's/.*(\([0-9A-F-]\{36\}\)) (Shutdown).*/\1/p' | head -1)
    [ -n "$udid" ] || { echo "IOSSIM-FAIL: no iPhone simulator on this Mac"; exit 75; }
    xcrun simctl boot "$udid" || { echo "IOSSIM-FAIL: simctl boot $udid"; exit 1; }
    xcrun simctl bootstatus "$udid" -b > /dev/null
    booted_here=1
fi
echo "simulator: $(xcrun simctl list devices | grep "$udid" | sed 's/^ *//')"
work=$(mktemp -d /tmp/iossim.XXXXXX) || exit 2
printf 'wait_frame 1\n' > "$work/quit.hal"
fail=
cargo build --release -q -p cargo-makepad || { echo "IOSSIM-FAIL: cargo-makepad build"; exit 1; }
for pkg in $pkgs; do
    echo "building the host app for $pkg"
    if ! target/release/cargo-makepad apple ios --org=dev.makepad build-host-sim "$pkg" > "$work/$pkg.build" 2>&1; then
        grep -E '^error|FAILED|failed' -A4 "$work/$pkg.build" | head -20
        fail="$fail build:$pkg"; continue
    fi
    grep 'Host app built' "$work/$pkg.build"
    app=$(sed -n 's/^Host app built: \(.*\.app\) (.*/\1/p' "$work/$pkg.build")
    id=$(/usr/libexec/PlistBuddy -c 'Print CFBundleIdentifier' "$app/Info.plist")
    echo "bundle $app: $(du -sh "$app" | cut -f1), binary $(du -h "$app/makepad" | cut -f1)"
    codesign --force --timestamp=none --sign - "$app" > /dev/null 2>&1 || { fail="$fail sign:$pkg"; continue; }
    xcrun simctl install "$udid" "$app" || { fail="$fail install:$pkg"; continue; }
    t0=$(date +%s)
    SIMCTL_CHILD_MAKEPAD_HAL_SCRIPT="$work/quit.hal" SIMCTL_CHILD_MAKEPAD_MUTE=1 SIMCTL_CHILD_MAKEPAD_RUN_INTERP_STATS=1 \
        perl -e 'alarm 1200; exec @ARGV' xcrun simctl launch --console-pty --terminate-running-process "$udid" "$id" 2>&1 | tee "$work/$pkg.out" | grep -a --line-buffered -E 'makepad-run: |interp|panic|error|frame'
    rc=${PIPESTATUS[0]}
    # (a launch cut short by the alarm leaves the app running)
    xcrun simctl terminate "$udid" "$id" > /dev/null 2>&1
    echo "run $pkg: rc $rc in $(( $(date +%s) - t0 ))s"
    grep -a -E 'makepad-run interp:|first frame|boot-trace: first|panicked|error' "$work/$pkg.out" | tail -8 | cut -c1-240
    if [ "$rc" != 0 ]; then
        grep -a -v '^\s*$' "$work/$pkg.out" | tail -20 | cut -c1-240
        fail="$fail run:$pkg"
    fi
    xcrun simctl uninstall "$udid" "$id"
done
[ -n "$booted_here" ] && xcrun simctl shutdown "$udid"
rm -r -- "${work:?}"
if [ -n "$fail" ]; then echo "IOSSIM-FAIL:$fail"; exit 1; fi
echo "IOSSIM-OK"
