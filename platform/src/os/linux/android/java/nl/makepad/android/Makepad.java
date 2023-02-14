package nl.makepad.android;

import android.view.MotionEvent;

public class Makepad {
    interface Callback {
        void swapBuffers();
        void scheduleRedraw();
        void scheduleTimeout(long id, long delay);
        void cancelTimeout(long id);
        byte[] readAsset(String path);
        String[] getAudioDevices(long flag);
    }

    static {
        System.loadLibrary("makepad");
    }

    static native long newCx();
    static native void dropCx(long cx);
    static native void init(long cx, String cache_path, Callback callback);
    static native void resize(long cx, int width, int height, Callback callback);
    static native void draw(long cx, Callback callback);
    static native void touch(long cx, MotionEvent event, Callback callback);
    static native void timeout(long cx, long id, Callback callback);
}
