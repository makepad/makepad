#!/bin/bash

# android-33-ext
# Downloading https://dl.google.com/android/repository/platform-33-ext4_r01.zip
# put in $SRC/android-33-ext4

# android-13:
# Downloading https://dl.google.com/android/repository/build-tools_r33.0.1-macosx.zip
# put in $SRC/android-13

# platform tools (adb)
# Downloading https://dl.google.com/android/repository/platform-tools_r33.0.3-darwin.zip
# put in $SRC/platform-tools

# NDK comes from
# https://dl.google.com/android/repository/android-ndk-r25c-darwin.dmg
# put in $SRC/NDK

# $SRC/jbr/ comes from android studio application package Contents/jbr and is x86 
# $SRC/jbrm1/ is the openJDK distribution for m1. its about 9.25s->8.5s for signing an apk.

SRC=android_33_darwin_x86_64
DST=android_33_darwin_x86_64_to_aarch64

rm -rf $DST
mkdir -p $DST

pushd $DST

    # Java Runtime
    JBR=jbrm1

    mkdir -p jbr/bin
    cp ../$SRC/$JBR/bin/java jbr/bin
    cp ../$SRC/$JBR/bin/javac jbr/bin

    mkdir -p jbr/lib/jli
    cp ../$SRC/$JBR/lib/libjli.dylib jbr/lib
    #cp ../$SRC/$JBR/lib/jli/libjli.dylib jbr/lib/jli
    cp ../$SRC/$JBR/lib/jvm.cfg jbr/lib/

    mkdir -p jbr/lib/server
    cp ../$SRC/$JBR/lib/server/libjsig.dylib jbr/lib/server
    cp ../$SRC/$JBR/lib/server/libjvm.dylib jbr/lib/server

    cp ../$SRC/$JBR/lib/modules jbr/lib

    cp ../$SRC/$JBR/lib/tzdb.dat jbr/lib
    cp ../$SRC/$JBR/lib/libjava.dylib jbr/lib
    cp ../$SRC/$JBR/lib/libjimage.dylib jbr/lib
    cp ../$SRC/$JBR/lib/libnet.dylib jbr/lib
    cp ../$SRC/$JBR/lib/libnio.dylib jbr/lib
    cp ../$SRC/$JBR/lib/libverify.dylib jbr/lib
    cp ../$SRC/$JBR/lib/libzip.dylib jbr/lib

    mkdir -p jbr/conf/security 
    cp -a ../$SRC/$JBR/conf/security/* jbr/conf/security/

    # build tools

    mkdir android-13
    cp ../$SRC/android-13/aapt android-13
    cp ../$SRC/android-13/apksigner android-13
    cp ../$SRC/android-13/zipalign android-13
    cp ../$SRC/android-13/d8 android-13

    mkdir android-13/lib
    cp ../$SRC/android-13/lib/apksigner.jar android-13/lib
    cp ../$SRC/android-13/lib/d8.jar android-13/lib  

    # something ext

    mkdir android-33-ext4
    cp ../$SRC/android-33-ext4/android.jar android-33-ext4

    # platform tools

    mkdir platform-tools
    cp ../$SRC/platform-tools/adb platform-tools

    # NDK

    BIN=NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin

    mkdir -p $BIN
    cp ../$SRC/$BIN/aarch64-linux-android33-clang $BIN
    cp ../$SRC/$BIN/clang $BIN
    cp ../$SRC/$BIN/ld $BIN

    LIB64=NDK/toolchains/llvm/prebuilt/darwin-x86_64/lib64

    mkdir -p $LIB64
    cp ../$SRC/$LIB64/libxml2.2.* $LIB64

    SYSLIB=NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33

    mkdir -p $SYSLIB
    cp ../$SRC/$SYSLIB/crtbegin_so.o $SYSLIB    
    cp ../$SRC/$SYSLIB/crtend_so.o $SYSLIB 
    cp ../$SRC/$SYSLIB/libc.so $SYSLIB    
    cp ../$SRC/$SYSLIB/libGLESv2.so $SYSLIB    
    cp ../$SRC/$SYSLIB/libm.so $SYSLIB    
    cp ../$SRC/$SYSLIB/liblog.so $SYSLIB    
    cp ../$SRC/$SYSLIB/libEGL.so $SYSLIB    
    cp ../$SRC/$SYSLIB/libdl.so $SYSLIB    
    cp ../$SRC/$SYSLIB/libaaudio.so $SYSLIB   
    cp ../$SRC/$SYSLIB/libaaudio.so $SYSLIB

    # these files are needed by the rust linker but are actually no-ops so we just copy libc to stand in as a fake 
    cp $SYSLIB/libc.so $SYSLIB/libgcc.so
    cp $SYSLIB/libc.so $SYSLIB/libunwind.so

popd