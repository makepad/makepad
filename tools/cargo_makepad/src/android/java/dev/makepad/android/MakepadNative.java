package dev.makepad.android;

import android.view.Surface;
import android.view.MotionEvent;
import java.nio.ByteBuffer;

public class MakepadNative {
    // belongs to MakepadActivity class
    public native static void activityOnCreate(Object activity);
    public native static void activityOnResume();
    public native static void activityOnPause();
    public native static void activityOnStop();
    public native static void activityOnDestroy();
    public static native void onAndroidParams(String cache_path, float dentify);

    // belongs to QuadSurface class
    public native static void surfaceOnSurfaceCreated(Surface surface);
    public native static void surfaceOnSurfaceDestroyed(Surface surface);
    public static native void surfaceOnTouch(MotionEvent event);
    public native static void surfaceOnSurfaceChanged(Surface surface, int width, int height);
    public native static void surfaceOnKeyDown(int keycode, int meta_state);
    public native static void surfaceOnKeyUp(int keycode);
    public native static void surfaceOnCharacter(int character);
    public native static void surfaceOnResizeTextIME(int keyboard_height, boolean is_open);

    // networking
    public native static void onHttpResponse(long id, long metadata_id, int status_code, String headers, byte[] body);
    public native static void onHttpRequestError(long id, long metadata_id, String error);

    // midi
    public native static void onMidiDeviceOpened(String name, Object midi_device);

    // video decoding
    public static native void onVideoDecodingInitialized(long videoId, int frameRate, int videoWidth, int videoHeight, String colorFormat, long duration);
    public static native void onVideoStream(long videoId, ByteBuffer frameGroup);
    public static native void onVideoChunkDecoded(long videoId);
}
