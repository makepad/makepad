#!/bin/bash

APK_DIR="./target/android_apk/build"
REL_ROOT="../../.."
SDK_DIR="$REL_ROOT/tools/android/android_33_darwin_x86_64_to_aarch64"
JAVA_DIR="$REL_ROOT/platform/src/os/linux/android/java/nl/makepad/android"
JAVA_FILES="$JAVA_DIR/Makepad.java $JAVA_DIR/MakepadActivity.java $JAVA_DIR/MakepadSurfaceView.java"
MANIFEST_FILE="$REL_ROOT/platform/src/os/linux/android/xml/AndroidManifest.xml"
JBR_DIR=$SDK_DIR/jbr

mkdir -p $APK_DIR
rm -rf $APK_DIR/*

pushd $APK_DIR
    # Compile Java to JVM bytecode. This generates one or more class files.
    echo "Compiling java"
    JAVA_HOME=$JBR_DIR $JBR_DIR/bin/javac -classpath $SDK_DIR/android-33-ext4/android.jar -d . $JAVA_FILES
    # Convert JVM bytecode to Dalvik bytecode. This generates a classes.dex file.
    echo "Creating dex file"
    JAVA_HOME=$JBR_DIR $SDK_DIR/android-13/d8 --classpath $SDK_DIR/android-33-ext4/android.jar $(find . -name "*.class")

    # Create an empty .apk file and add both classes.dex and libmakepad.so to it.
    echo "Creating apk file"
    $SDK_DIR/android-13/aapt package -F makepad_android_java.apk -I $SDK_DIR/android-33-ext4/android.jar -M $MANIFEST_FILE
    $SDK_DIR/android-13/aapt add makepad_android_java.apk classes.dex

    #../android-13/aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so

    # Sign our .apk file with the debug key.
    #echo "Signing apk file"
    #../android-13/apksigner sign -ks ../debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk
popd
