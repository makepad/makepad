package dev.makepad.android;

import android.view.Surface;
import android.view.MotionEvent;

public class MakepadNative {
    // belongs to MakepadActivity class
    public native static void activityOnCreate(Object activity);
    public native static void activityOnStart();
    public native static void activityOnResume();
    public native static void activityOnPause();
    public native static void activityOnStop();
    public native static void activityOnDestroy();
    public native static void activityOnWindowFocusChanged(boolean has_focus);
    public static native void onAndroidParams(String cache_path, float dentify, boolean isEmulator, String androidVersion, String buildNumber,
        String kernelVersion);

    public native static void initChoreographer(float deviceRefreshRate, int sdkVersion);

    public native static void onBackPressed();

    // belongs to QuadSurface class
    public native static void surfaceOnSurfaceCreated(Surface surface);
    public native static void surfaceOnSurfaceDestroyed(Surface surface);
    public static native void surfaceOnLongClick(float x, float y, int pointerId, long timeMillis);
    public static native void surfaceOnTouch(MotionEvent event);
    public native static void surfaceOnSurfaceChanged(Surface surface, int width, int height);
    public native static void surfaceOnKeyDown(int keycode, int meta_state);
    public native static void surfaceOnKeyUp(int keycode, int meta_state);
    public native static void surfaceOnCharacter(int character);
    public native static void surfaceOnResizeTextIME(int keyboard_height, boolean is_open);

    // networking
    public native static void onHttpResponse(long id, long metadata_id, int status_code, String headers, byte[] body);
    public native static void onHttpRequestError(long id, long metadata_id, String error);
    public native static void onWebSocketMessage(byte[] message, long callback);
    public native static void onWebSocketClosed(long callback);
    public native static void onWebSocketError(String error, long callback);


    // midi
    public native static void onMidiDeviceOpened(String name, Object midi_device);

    // video playback
    public static native void onVideoPlaybackPrepared(long videoId, int videoWidth, int videoHeight, long duration, VideoPlayer surfaceTexture);
    public static native void onVideoPlaybackCompleted(long videoId);
    public static native void onVideoPlayerReleased(long videoId);
    public static native void onVideoDecodingError(long videoId, String error);
}