#!/bin/bash
APK_DIR="./target/android_apk/build"
REL_ROOT="../../.."
SDK_DIR="./tools/android/android_33_darwin_x86_64_to_aarch64"
PKG="makepad-example-ironfish"
FILE="makepad_example_ironfish"

#CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="$SDK_DIR/NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android33-clang" cargo +nightly rustc --lib --crate-type=cdylib -p $PKG --release --target=aarch64-linux-android

pushd $APK_DIR
    # Make sure libmakepad.so ends up in the correct location in the .apk file.
    mkdir -p lib/arm64-v8a
    cp ../../aarch64-linux-android/release/lib$FILE.so lib/arm64-v8a/libmakepad.so

    echo "Creating apk file"
    cp makepad_android_java.apk makepad_android.apk
    $REL_ROOT/$SDK_DIR/android-13/aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so

    # Sign our .apk file with the debug key.
    echo "Signing apk file"
    JAVA_HOME=`$pwd`$REL_ROOT/$SDK_DIR/jbr $REL_ROOT/$SDK_DIR/android-13/apksigner sign -ks $REL_ROOT/tools/android/debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk
popd
