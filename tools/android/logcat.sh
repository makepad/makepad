SDK_DIR=./tools/android/android_33_darwin_x86_64_to_aarch64

PID=""
while [$PID -eq ""]
do
    PID=$($SDK_DIR/platform-tools/adb shell pidof nl.makepad.android)
done
$SDK_DIR/platform-tools/adb logcat --pid $PID *:S Makepad:D
