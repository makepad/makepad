#!/bin/bash
SDK_DIR=./tools/android/android_33_darwin_x86_64_to_aarch64

$SDK_DIR/platform-tools/adb install -r ./target/android_apk/build/makepad_android.apk
