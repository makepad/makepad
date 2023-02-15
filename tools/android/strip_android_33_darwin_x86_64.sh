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
# put in $SRC/android-13

# platform tools (adb)
# Downloading https://dl.google.com/android/repository/platform-tools_r33.0.3-darwin.zip
# put in $SRC/platform-tools

# NDK comes from
# https://dl.google.com/android/repository/android-ndk-r25c-darwin.dmg
# put in $SRC/NDK

# $SRC/openjdk/ is the openJDK distribution for m1 from https://jdk.java.net/archive/ version 16
# version 19 had this error:  Unsupported class file major version 63 with d8. 
# version 16 also has this warning: One or more classes has class file version >= 56 which is not officially supported.
# however atleast we get an M1 native build with 16,
# openJDK 11 which is apparently in android studio doesnt have an m1 build.
# Copy the Contents/Home/* directory to openjdk/*
 
# this is the input directory where you expand all the files above
SRC=android_33_darwin_x86_64

# this is the destination directory
DST=android_33_darwin_x86_64_to_aarch64

rm -rf $DST
mkdir -p $DST

# openJDK files

mkdir -p $DST/openjdk/bin
cp $SRC/openjdk/bin/java $DST/openjdk/bin
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
