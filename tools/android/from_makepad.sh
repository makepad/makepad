#!/bin/zsh
APK_DIR="./target/android_apk/build"
REL_ROOT="../../.."
SDK_DIR="./tools/android/android_33_darwin_x86_64_to_aarch64"
PKG="makepad-example-ironfish"
FILE="makepad_example_ironfish"
JDK_DIR=$SDK_DIR/openjdk

CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$SDK_DIR/NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android33-clang" cargo +nightly rustc --lib --crate-type=cdylib -p $PKG --release --target=aarch64-linux-android --message-format=json
if [[ $? -ne 0 ]]; then
    exit 0
fi
pushd $APK_DIR >/dev/null
    # Make sure libmakepad.so ends up in the correct location in the .apk file.
    mkdir -p lib/arm64-v8a
    cp ../../aarch64-linux-android/release/lib$FILE.so lib/arm64-v8a/libmakepad.so

    cp makepad_android_java.apk makepad_android.apk
    $REL_ROOT/$SDK_DIR/android-13/aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so >/dev/null

    mkdir -p assets/makepad/makepad_widgets/resources
    cp $REL_ROOT/widgets/resources/IBMPlexSans-Text.ttf assets/makepad/makepad_widgets/resources
    cp $REL_ROOT/widgets/resources/IBMPlexSans-SemiBold.ttf assets/makepad/makepad_widgets/resources
    cp $REL_ROOT/widgets/resources//LiberationMono-Regular.ttf assets/makepad/makepad_widgets/resources
    $REL_ROOT/$SDK_DIR/android-13/aapt add makepad_android.apk assets/makepad/makepad_widgets/resources/IBMPlexSans-Text.ttf >/dev/null
    $REL_ROOT/$SDK_DIR/android-13/aapt add makepad_android.apk assets/makepad/makepad_widgets/resources/IBMPlexSans-SemiBold.ttf >/dev/null
    $REL_ROOT/$SDK_DIR/android-13/aapt add makepad_android.apk assets/makepad/makepad_widgets/resources/LiberationMono-Regular.ttf >/dev/null
    mkdir -p assets/makepad/resources
    cp $REL_ROOT/examples/ironfish/resources/tinrs.png assets/makepad/resources
    $REL_ROOT/$SDK_DIR/android-13/aapt add makepad_android.apk assets/makepad/resources/tinrs.png >/dev/null

    # Sign our .apk file with the debug key.
    JAVA_HOME=$REL_ROOT/$JDK_DIR $REL_ROOT/$SDK_DIR/android-13/apksigner sign -v -ks $REL_ROOT/tools/android/debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk>/dev/null
popd >/dev/null

$SDK_DIR/platform-tools/adb install -r ./target/android_apk/build/makepad_android.apk &>/dev/null
$SDK_DIR/platform-tools/adb shell am start -n nl.makepad.android/nl.makepad.android.MakepadActivity &>/dev/null
PID=0
while [[ $PID -eq 0 ]]
do
    PID=$($SDK_DIR/platform-tools/adb shell pidof nl.makepad.android)
done
echo "--------------------------------------------------  Application $PID running --------------------------------------------------"
$SDK_DIR/platform-tools/adb logcat --pid $PID "*:S Makepad:D"
