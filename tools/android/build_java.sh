#!/bin/bash

mkdir -p build
rm -rf build/*

SDK_DIR="./android_33_darwin_x86_64_to_aarch64"

pushd build
    # Compile Java to JVM bytecode. This generates one or more class files.
    echo "Compiling java"
    JAVA_HOME=`$pwd`../$SDK_DIR/jbr ../$SDK_DIR/jbr/bin/javac -classpath ../$SDK_DIR/android-33-ext4/android.jar -d . $(find ../java -name "*.java")
    # Convert JVM bytecode to Dalvik bytecode. This generates a classes.dex file.
    echo "Creating dex file"
    JAVA_HOME=`$pwd`../$SDK_DIR/jbr ../$SDK_DIR/android-13/d8 --classpath ../$SDK_DIR/android-33-ext4/android.jar $(find . -name "*.class")

    # Create an empty .apk file and add both classes.dex and libmakepad.so to it.
    echo "Creating apk file"
    ../$SDK_DIR/android-13/aapt package -F makepad_android.apk -I ../$SDK_DIR/android-33-ext4/android.jar -M ../AndroidManifest.xml
    ../$SDK_DIR/android-13/aapt add makepad_android.apk classes.dex
    
    cp makepad_android.apk makepad_android_build.apk

    #../android-13/aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so

    # Sign our .apk file with the debug key.
    #echo "Signing apk file"
    #../android-13/apksigner sign -ks ../debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk
popd
