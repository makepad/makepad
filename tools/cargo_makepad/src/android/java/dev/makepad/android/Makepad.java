package dev.makepad.android;

import android.view.MotionEvent;

public class Makepad {
    interface Callback {
        void swapBuffers();
        void scheduleRedraw();
        void scheduleTimeout(long id, long delay);
        void cancelTimeout(long id);
        byte[] readAsset(String path);
        String[] getAudioDevices(long flag);
        void openAllMidiDevices(long delay);
        void showTextIME();
    }

    static {
        System.loadLibrary("makepad");
    }
    // Event calls from Java to Rust
    static native long onNewCx();
    static native void onDropCx(long cx);
    static native long onPause(long cx, Callback callback);
    static native void onResume(long cx, Callback callback);
    static native long onNewGL(long cx, Callback callback);
    static native void onFreeGL(long cx, Callback callback);
    static native void onInit(long cx, String cache_path, float dentify, Callback callback);
    static native void onResize(long cx, int width, int height, Callback callback);
    static native void onDraw(long cx, Callback callback);
    static native void onTouch(long cx, MotionEvent event, Callback callback);
    static native void onTimeout(long cx, long id, Callback callback);
    static native void onMidiDeviceOpened(long cx, String name, Object midi_device, Callback callback);
}
