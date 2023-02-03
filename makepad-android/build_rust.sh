#!/bin/bash

pushd rust
    # Make sure libmakepad.so is up to date.
    cargo clean
    cargo build --release
popd
pushd build
    # Make sure libmakepad.so ends up in the correct location in the .apk file.
    mkdir -p lib/arm64-v8a
    cp ../rust/target/aarch64-linux-android/release/libmakepad.so lib/arm64-v8a

    echo "Creating apk file"
    cp makepad_android_build.apk makepad_android.apk
    ../env_strip/android-13/aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so

    # Sign our .apk file with the debug key.
    echo "Signing apk file"
    JAVA_HOME=`$pwd`../env_strip/jbr ../env_strip/android-13/apksigner sign -ks ../debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk
popd
