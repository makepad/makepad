#!/bin/bash

#
#  This file generates a stripped down version of all buildtooling needed to build
#  an android application with the makepad Rust stack.
#  Currently it is macos only.
#

# To create the stripped down version you need the following origin data:

# android-33-ext
# Downloading https://dl.google.com/android/repository/platform-33-ext4_r01.zip
# put in $SRC/android-33-ext4

# android-13:
# Downloading https://dl.google.com/android/repository/build-tools_r33.0.1-macosx.zip
# Downloading https://dl.google.com/android/repository/build-tools_r33.0.1-linux.zip
# Downloading https://dl.google.com/android/repository/build-tools_r33.0.1-windows.zip
# put in $SRC/android-13

# platform tools (adb)
# Downloading https://dl.google.com/android/repository/platform-tools_r33.0.3-darwin.zip
# Downloading https://dl.google.com/android/repository/platform-tools_r33.0.3-linux.zip
# Downloading https://dl.google.com/android/repository/platform-tools_r33.0.3-windows.zip
# put in $SRC/platform-tools

# NDK comes from
# https://dl.google.com/android/repository/android-ndk-r25c-darwin.dmg
# https://dl.google.com/android/repository/android-ndk-r25c-windows.zip
# https://dl.google.com/android/repository/android-ndk-r25c-linux.zip
# put in $SRC/NDK

# android v4 support, rename aar to zip, unpack, rename classes.jar to support/android-support-v4-28.0.0.jar
# https://maven.google.com/web/index.html?q=v4#com.android.support:support-compat:28.0.0

# $SRC/openjdk/ is the openJDK distribution for m1 from https://jdk.java.net/archive/ version 17.0.2
# https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_windows-x64_bin.zip
# https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_macos-aarch64_bin.tar.gz
# https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_macos-x64_bin.tar.gz
# https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_linux-aarch64_bin.tar.gz
# https://download.java.net/java/GA/jdk17.0.2/dfd4a8d0985749f896bed50d7138ee7f/8/GPL/openjdk-17.0.2_linux-x64_bin.tar.gz

# version 19 had this error:  Unsupported class file major version 63 with d8. 
# version 17 also has this warning: One or more classes has class file version >= 56 which is not officially supported.
# however atleast we get an M1 native build with 16,
# openJDK 11 which is apparently in android studio doesnt have an m1 build.
# Copy the Contents/Home/* directory to openjdk/*

# this is the input directory where you expand all the files above
SRC=android_33_aarch64_apple_darwin

# this is the destination directory
DST=android_33_aarch64_apple_darwin_to_aarch64_linux_android

# lets download the sources and unzip them


rm -rf $DST
mkdir -p $DST

# openJDK files

mkdir -p $DST/openjdk/bin
cp $SRC/openjdk/bin/java $DST/openjdk/bin
cp $SRC/openjdk/bin/jar $DST/openjdk/bin
cp $SRC/openjdk/bin/javac $DST/openjdk/bin

mkdir -p $DST/openjdk/lib/jli
cp $SRC/openjdk/lib/libjli.dylib $DST/openjdk/lib
cp $SRC/openjdk/lib/jvm.cfg $DST/openjdk/lib/

mkdir -p $DST/openjdk/lib/server
cp $SRC/openjdk/lib/server/libjsig.dylib $DST/openjdk/lib/server
cp $SRC/openjdk/lib/server/libjvm.dylib $DST/openjdk/lib/server

cp $SRC/openjdk/lib/modules $DST/openjdk/lib

cp $SRC/openjdk/lib/tzdb.dat $DST/openjdk/lib
cp $SRC/openjdk/lib/libjava.dylib $DST/openjdk/lib
cp $SRC/openjdk/lib/libjimage.dylib $DST/openjdk/lib
cp $SRC/openjdk/lib/libnet.dylib $DST/openjdk/lib
cp $SRC/openjdk/lib/libnio.dylib $DST/openjdk/lib
cp $SRC/openjdk/lib/libverify.dylib $DST/openjdk/lib
cp $SRC/openjdk/lib/libzip.dylib $DST/openjdk/lib

mkdir -p $DST/openjdk/conf/security 
cp -a $SRC/openjdk/conf/security/* $DST/openjdk/conf/security/

mkdir -p $DST/support
cp -a $SRC/support/* $DST/support

# build tools

mkdir $DST/android-13
cp $SRC/android-13/aapt $DST/android-13
cp $SRC/android-13/apksigner $DST/android-13
cp $SRC/android-13/zipalign $DST/android-13
cp $SRC/android-13/d8 $DST/android-13

mkdir $DST/android-13/lib
cp $SRC/android-13/lib/apksigner.jar $DST/android-13/lib
cp $SRC/android-13/lib/d8.jar $DST/android-13/lib  

# something ext

mkdir $DST/android-33-ext4
cp $SRC/android-33-ext4/android.jar $DST/android-33-ext4

# platform tools

mkdir $DST/platform-tools
cp $SRC/platform-tools/adb $DST/platform-tools

# NDK

BIN=NDK/toolchains/llvm/prebuilt/darwin-x86_64/bin

mkdir -p $DST/$BIN
cp $SRC/$BIN/aarch64-linux-android33-clang $DST/$BIN
cp $SRC/$BIN/clang $DST/$BIN
cp $SRC/$BIN/ld $DST/$BIN

LIB64=NDK/toolchains/llvm/prebuilt/darwin-x86_64/lib64

mkdir -p $DST/$LIB64
cp $SRC/$LIB64/libxml2.2.* $DST/$LIB64

SYSLIB=NDK/toolchains/llvm/prebuilt/darwin-x86_64/sysroot/usr/lib/aarch64-linux-android/33

mkdir -p $DST/$SYSLIB
cp $SRC/$SYSLIB/crtbegin_so.o $DST/$SYSLIB    
cp $SRC/$SYSLIB/crtend_so.o $DST/$SYSLIB 
cp $SRC/$SYSLIB/libc.so $DST/$SYSLIB    
cp $SRC/$SYSLIB/libGLESv2.so $DST/$SYSLIB    
cp $SRC/$SYSLIB/libm.so $DST/$SYSLIB    
cp $SRC/$SYSLIB/liblog.so $DST/$SYSLIB    
cp $SRC/$SYSLIB/libEGL.so $DST/$SYSLIB    
cp $SRC/$SYSLIB/libdl.so $DST/$SYSLIB    
cp $SRC/$SYSLIB/libaaudio.so $DST/$SYSLIB   
cp $SRC/$SYSLIB/libamidi.so $DST/$SYSLIB

# these files are needed by the rust linker but are actually no-ops so we just copy libc to stand in as a fake 
cp $DST/$SYSLIB/libc.so $DST/$SYSLIB/libgcc.so
cp $DST/$SYSLIB/libc.so $DST/$SYSLIB/libunwind.so
