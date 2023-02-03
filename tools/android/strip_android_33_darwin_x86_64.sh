#!/bin/bash

# android-33-ext
# Downloading https://dl.google.com/android/repository/platform-33-ext4_r01.zip
# put in env_src/android-33-ext4

# android-13:
# Downloading https://dl.google.com/android/repository/build-tools_r33.0.1-macosx.zip
# put in env_src/android-13

# platform tools (adb)
# Downloading https://dl.google.com/android/repository/platform-tools_r33.0.3-darwin.zip
# put in env_src/platform-tools

# NDK comes from
# https://dl.google.com/android/repository/android-ndk-r25c-darwin.dmg
# put in env_src/NDK

# env_src/jbr/ comes from android studio application package Contents/jbr

SRC=android_33_darwin_x86_64
SDK_DIR=android_33_darwin_x86_64_to_aarch64

rm -rf $SDK_DIR
mkdir -p $SDK_DIR

pushd $SDK_DIR

    # Java Runtime

    rm -rf jbr
    mkdir jbr
    mkdir jbr/bin

    cp ../$SRC/jbr/bin/java jbr/bin

    mkdir jbr/lib
    mkdir jbr/lib/jli
    cp ../$SRC/jbr/lib/jli/libjli.dylib jbr/lib/jli
    cp ../$SRC/jbr/lib/jvm.cfg jbr/lib/

    mkdir jbr/lib/server
    cp ../$SRC/jbr/lib/server/libjsig.dylib jbr/lib/server
    cp ../$SRC/jbr/lib/server/libjvm.dylib jbr/lib/server

    cp ../$SRC/jbr/lib/modules jbr/lib

    cp ../$SRC/jbr/lib/libjava.dylib jbr/lib
    cp ../$SRC/jbr/lib/libjimage.dylib jbr/lib
    cp ../$SRC/jbr/lib/libnet.dylib jbr/lib
    cp ../$SRC/jbr/lib/libnio.dylib jbr/lib
    cp ../$SRC/jbr/lib/libverify.dylib jbr/lib
    cp ../$SRC/jbr/lib/libzip.dylib jbr/lib

    # build tools

    mkdir android-13
    cp ../$SRC/android-13/aapt android-13
    cp ../$SRC/android-13/apksigner android-13
    mkdir android-13/lib
    cp ../$SRC/android-13/lib/apksigner.jar android-13/lib

    # something ext

    mkdir android-33-ext4
    cp ../$SRC/android-33-ext4/android.jar android-33-ext4

    # platform tools

    mkdir platform-tools
    cp ../$SRC/platform-tools/adb platform-tools

    # NDK

    mkdir -p NDK/toolchains/llvm/prebuilt/darwin-x86_64

    mkdir NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin
    cp ../$SRC/NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/aarch64-linux-android33-clang \
            NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin
    cp ../$SRC/NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/clang \
            NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin
    cp ../$SRC/NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin/ld \
            NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin

    mkdir -p NDK/toolchains/llvm/prebuilt/darwin-x86_64/lib64
    cp ../$SRC/NDK/toolchains/llvm/prebuilt/darwin-x86_64/lib64/libxml2.2.* \
            NDK/toolchains/llvm/prebuilt/darwin-x86_64/lib64

    mkdir -p NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android
    cp -a ../$SRC/NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33 \
      NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/

    cp NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33/libc.so\
       NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33/libgcc.so
    cp NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33/libc.so\
       NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33/libunwind.so


    #only needed for java build (saves 10mb. not useful)
    cp ../$SRC/jbr/bin/javac jbr/bin
    cp ../$SRC/android-13/d8 android-13
    cp ../$SRC/jbr/lib/tzdb.dat jbr/lib
    cp ../$SRC/android-13/lib/d8.jar android-13/lib  

popd