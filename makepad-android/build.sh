#!/bin/bash

pushd rust
    # Make sure libmakepad.so is up to date.
    cargo build
popd

mkdir -p build
rm -rf build/*
pushd build
    # Compile Java to JVM bytecode. This generates one or more class files.
    javac -classpath $ANDROID_HOME/platforms/android-33/android.jar -d . $(find ../java -name "*.java")

    # Convert JVM bytecode to Dalvik bytecode. This generates a classes.dex file.
    d8 --classpath $ANDROID_HOME/platforms/android-33/android.jar $(find . -name "*.class")

    # Make sure libmakepad.so ends up in the correct location in the .apk file.
    mkdir -p lib/arm64-v8a
    cp ../rust/target/aarch64-linux-android/debug/libmakepad.so lib/arm64-v8a

    # Create an empty .apk file and add both classes.dex and libmakepad.so to it.
    aapt package -F makepad_android.apk -I $ANDROID_HOME/platforms/android-33/android.jar -M ../AndroidManifest.xml
    aapt add makepad_android.apk classes.dex
    aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so

    # Sign our .apk file with the debug key.
    apksigner sign -ks ../debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk
popd
