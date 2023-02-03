#!/bin/bash

mkdir -p build
rm -rf build/*

pushd build
    JAVA_HOME=`$pwd`../env_strip/jbr
    # Compile Java to JVM bytecode. This generates one or more class files.
    echo "Compiling java"
    $JAVA_HOME/bin/javac -classpath ../env_strip/android-33-ext4/android.jar -d . $(find ../java -name "*.java")
    # Convert JVM bytecode to Dalvik bytecode. This generates a classes.dex file.
    echo "Creating dex file"
    ../env_strip/android-13/d8 --classpath ../env_strip/android-33-ext4/android.jar $(find . -name "*.class")

    # Create an empty .apk file and add both classes.dex and libmakepad.so to it.
    echo "Creating apk file"
    ../env_strip/android-13/aapt package -F makepad_android.apk -I ../env_strip/android-33-ext4/android.jar -M ../AndroidManifest.xml
    ../env_strip/android-13/aapt add makepad_android.apk classes.dex
    
    cp makepad_android.apk makepad_android_build.apk

    #../android-13/aapt add makepad_android.apk lib/arm64-v8a/libmakepad.so

    # Sign our .apk file with the debug key.
    #echo "Signing apk file"
    #../android-13/apksigner sign -ks ../debug.keystore --ks-key-alias androiddebugkey --ks-pass pass:android makepad_android.apk
popd
