#!/bin/bash

SDK_DIR=./android_33_darwin_x86_64_to_aarch64

pushd rust
    # Make sure libmakepad.so is up to date.
    cargo clean
    CARGO_TARGET_AARCH64_LINUX_ANDROID_LINKER="../$SDK_DIR/NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android33-clang"\
    RUSTFLAGS="--crate-type=dylib"\
    cargo +nightly build --release --target=aarch64-linux-android
popd
pushd build
    # Make sure libmakepad.so ends up in the correct location in the .apk file.
    mkdir -p lib/arm64-v8a
    cp ../rust/target/aarch64-linux-android/release/deps/libmakepad*.so lib/arm64-v8a/libmakepad.so

    echo "Creating apk file"
    cp makepad_android_build.apk makepad_android.apk
    ../$SDK_DIR/android-13/aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so

    # Sign our .apk file with the debug key.
    echo "Signing apk file"
    JAVA_HOME=`$pwd`../$SDK_DIR/jbr ../$SDK_DIR/android-13/apksigner sign -ks ../debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk
popd
