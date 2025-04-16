package dev.makepad.android;

import android.app.Activity;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;
import android.bluetooth.BluetoothManager;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ApplicationInfo;
import android.graphics.Color;
import android.graphics.Insets;
import android.graphics.Rect;
import android.media.AudioDeviceInfo;
import android.media.AudioManager;
import android.media.midi.MidiDevice;
import android.media.midi.MidiDeviceInfo;
import android.media.midi.MidiManager;
import android.os.Build;
import android.os.Bundle;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;
import android.os.SystemClock;
import android.util.Log;
import android.view.Display;
import android.view.KeyEvent;
import android.view.MotionEvent;
import android.view.Surface;
import android.view.SurfaceHolder;
import android.view.SurfaceView;
import android.view.View;
import android.view.ViewConfiguration;
import android.view.ViewTreeObserver;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowManager;
import android.view.WindowManager.LayoutParams;
import android.view.inputmethod.EditorInfo;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.InputMethodManager;
import android.widget.LinearLayout;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.util.ArrayList;
import java.util.HashMap;
import java.util.Set;
import java.util.concurrent.CompletableFuture;

// note: //% is a special miniquad's pre-processor for plugins
// when there are no plugins - //% whatever will be replaced to an empty string
// before compiling

//% IMPORTS

class MakepadSurface
    extends
        SurfaceView
    implements
        View.OnTouchListener,
        View.OnKeyListener,
        View.OnLongClickListener,
        ViewTreeObserver.OnGlobalLayoutListener,
        SurfaceHolder.Callback
{

    // The X,Y coordinates and pointer ID of the most recent ACTION_DOWN touch.
    private float latestDownTouchX;
    private float latestDownTouchY;
    private int latestDownTouchPointerId;

    // The X,Y coordinates and pointer ID of the most recent non-ACTION_DOWN touch event.
    private float latestTouchX;
    private float latestTouchY;
    private int latestTouchPointerId;


    public MakepadSurface(Context context){
        super(context);
        getHolder().addCallback(this);

        setFocusable(true);
        setFocusableInTouchMode(true);
        requestFocus();
        setOnTouchListener(this);
        setOnKeyListener(this);
        setOnLongClickListener(this);        

        getViewTreeObserver().addOnGlobalLayoutListener(this);
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        Log.i("SAPP", "surfaceCreated");
        Surface surface = holder.getSurface();
        //surface.setFrameRate(120f,0);
        MakepadNative.surfaceOnSurfaceCreated(surface);
    }

    @Override
    public void surfaceDestroyed(SurfaceHolder holder) {
        Log.i("SAPP", "surfaceDestroyed");
        Surface surface = holder.getSurface();
        MakepadNative.surfaceOnSurfaceDestroyed(surface);
    }

    @Override
    public void surfaceChanged(SurfaceHolder holder,
                               int format,
                               int width,
                               int height) {
        Log.i("SAPP", "surfaceChanged");
        Surface surface = holder.getSurface();
        //surface.setFrameRate(120f,0);
        MakepadNative.surfaceOnSurfaceChanged(surface, width, height);

    }

    @Override
    public boolean onTouch(View view, MotionEvent event) {
        // By default, we return false so that `onLongClick` will trigger.
        boolean retval = false;

        int actionMasked = event.getActionMasked();
        int index = event.getActionIndex();
        int pointerId = event.getPointerId(index);

        // Save the details of the latest touch-down event,
        // such that we can use them in the `onLongClick` method.
        if (actionMasked == MotionEvent.ACTION_DOWN) {
            latestDownTouchX = event.getX(index);
            latestDownTouchY = event.getY(index);
            latestDownTouchPointerId = pointerId;
            latestTouchPointerId = -1; // invalidate the previous latestTouchX/Y values.
        }
        else if (actionMasked == MotionEvent.ACTION_MOVE) {
            latestTouchX = event.getX(index);
            latestTouchY = event.getY(index);
            latestTouchPointerId = pointerId;
            if (pointerId == latestDownTouchPointerId) {
                if (isTouchBeyondSlopDistance(view)) {
                    retval = true;
                }
            }
        }

        MakepadNative.surfaceOnTouch(event);
        return retval;
    }

    @Override
    public boolean onLongClick(View view) {
        long timeMillis = SystemClock.uptimeMillis();

        // If the touch has moved more than the touch slop, ignore this long click.
        if (isTouchBeyondSlopDistance(view)) {
            // Returning false here indicates that we have not handled the long click event,
            // which does *not* trigger the haptic feedback (vibration motor) to buzz.
            return false;
        }

        // Here: a valid long click did occur, and we sholud send that event to makepad.

        // Use the latest touch coordinates if they're the same pointer ID as the initial down touch.
        if (latestTouchPointerId == latestDownTouchPointerId) {
            MakepadNative.surfaceOnLongClick(latestTouchX, latestTouchY, latestDownTouchPointerId, timeMillis);
        }
        // Otherwise, use the coordinates from the original down touch.
        else {
            MakepadNative.surfaceOnLongClick(latestDownTouchX, latestDownTouchY, latestDownTouchPointerId, timeMillis);
        }

        // Returning true here indicates that we have handled the long click event,
        // which triggers the haptic feedback (vibration motor) to buzz.
        return true;
    }

    // Returns true if the distance from the latest touch event to the prior down-touch event
    // is greated than the touch slop distance.
    //
    // If true, this indicates that the touch event shouldn't be considered a press/tap,
    // and is likely a drag or swipe.
    private boolean isTouchBeyondSlopDistance(View view) {
        int touchSlop = ViewConfiguration.get(view.getContext()).getScaledTouchSlop();
        float deltaX = latestTouchX - latestDownTouchX;
        float deltaY = latestTouchY - latestDownTouchY;
        double dist = Math.sqrt((deltaX * deltaX) + (deltaY * deltaY));
        return dist > touchSlop;
    }

    @Override
    public void onGlobalLayout() {
        WindowInsets insets = this.getRootWindowInsets();
        if (insets == null) {
            return;
        }

        Rect r = new Rect();
        this.getWindowVisibleDisplayFrame(r);
        int screenHeight = this.getRootView().getHeight();
        int visibleHeight = r.height();
        int keyboardHeight = screenHeight - visibleHeight;

        MakepadNative.surfaceOnResizeTextIME(keyboardHeight, insets.isVisible(WindowInsets.Type.ime()));
    }

    // docs says getCharacters are deprecated
    // but somehow on non-latyn input all keyCode and all the relevant fields in the KeyEvent are zeros
    // and only getCharacters has some usefull data
    @SuppressWarnings("deprecation")
    @Override
    public boolean onKey(View v, int keyCode, KeyEvent event) {
        if (event.getAction() == KeyEvent.ACTION_DOWN && keyCode != 0) {
            int metaState = event.getMetaState();
            MakepadNative.surfaceOnKeyDown(keyCode, metaState);
        }

        if (event.getAction() == KeyEvent.ACTION_UP && keyCode != 0) {
            int metaState = event.getMetaState();
            MakepadNative.surfaceOnKeyUp(keyCode, metaState);
        }

        if (event.getAction() == KeyEvent.ACTION_UP || event.getAction() == KeyEvent.ACTION_MULTIPLE) {
            int character = event.getUnicodeChar();
            if (character == 0) {
                String characters = event.getCharacters();
                if (characters != null && characters.length() >= 0) {
                    character = characters.charAt(0);
                }
            }

            if (character != 0) {
                MakepadNative.surfaceOnCharacter(character);
            }
        }

        // if ((keyCode == KeyEvent.KEYCODE_BACK)) {
        //     Log.d("Makepad", "KEYCODE_BACK");
        // }

        if ((keyCode == KeyEvent.KEYCODE_VOLUME_UP) || (keyCode == KeyEvent.KEYCODE_VOLUME_DOWN)) {
            return super.onKeyUp(keyCode, event);
        }

        return true;
    }

    // There is an Android bug when screen is in landscape,
    // the keyboard inset height is reported as 0.
    // This code is a workaround which fixes the bug.
    // See https://groups.google.com/g/android-developers/c/50XcWooqk7I
    // For some reason it only works if placed here and not in the parent layout.
    @Override
    public InputConnection onCreateInputConnection(EditorInfo outAttrs) {
        InputConnection connection = super.onCreateInputConnection(outAttrs);
        outAttrs.imeOptions |= EditorInfo.IME_FLAG_NO_FULLSCREEN;
        return connection;
    }

    public Surface getNativeSurface() {
        return getHolder().getSurface();
    }
}

class ResizingLayout
    extends
        LinearLayout
    implements
        View.OnApplyWindowInsetsListener {

    public ResizingLayout(Context context){
        super(context);
        // When viewing in landscape mode with keyboard shown, there are
        // gaps on both sides so we fill the negative space with black.
        setBackgroundColor(Color.BLACK);
        setOnApplyWindowInsetsListener(this);
    }

    @Override
    public WindowInsets onApplyWindowInsets(View v, WindowInsets insets) {
        Insets imeInsets = insets.getInsets(WindowInsets.Type.ime());
        v.setPadding(0, 0, 0, imeInsets.bottom);
        return insets;
    }
}

public class MakepadActivity
    extends Activity
    implements MidiManager.OnDeviceOpenedListener
{
    //% MAIN_ACTIVITY_BODY

    private MakepadSurface view;
    Handler mHandler;

    // video playback
    Handler mVideoPlaybackHandler;
    HashMap<Long, VideoPlayerRunnable> mVideoPlayerRunnables;

    // networking, make these static because of activity switching
    static Handler mWebSocketsHandler;
    static HashMap<Long, MakepadWebSocket> mActiveWebsockets = new HashMap<>();
    static HashMap<Long, MakepadWebSocketReader> mActiveWebsocketsReaders = new HashMap<>();

    static {
        System.loadLibrary("makepad");
    }

    @Override
    public void onCreate(Bundle savedInstanceState) {
        super.onCreate(savedInstanceState);
        
        this.requestWindowFeature(Window.FEATURE_NO_TITLE);

        view = new MakepadSurface(this);
        // Put it inside a parent layout which can resize it using padding
        ResizingLayout layout = new ResizingLayout(this);
        layout.addView(view);
        setContentView(layout);

        MakepadNative.activityOnCreate(this);

        HandlerThread decoderThreadHandler = new HandlerThread("VideoPlayerThread");
        decoderThreadHandler.start(); // TODO: only start this if its needed.
        mVideoPlaybackHandler = new Handler(decoderThreadHandler.getLooper());
        mVideoPlayerRunnables = new HashMap<Long, VideoPlayerRunnable>();

        HandlerThread webSocketsThreadHandler = new HandlerThread("WebSocketsThread");
        webSocketsThreadHandler.start();
        mWebSocketsHandler = new Handler(webSocketsThreadHandler.getLooper());

        String cache_path = this.getCacheDir().getAbsolutePath();
        float density = getResources().getDisplayMetrics().density;
        boolean isEmulator = this.isEmulator();
        String androidVersion = Build.VERSION.RELEASE;
        String buildNumber = Build.DISPLAY;
        String kernelVersion = this.getKernelVersion();
        int sdkVersion = Build.VERSION.SDK_INT;

        MakepadNative.onAndroidParams(cache_path, density, isEmulator, androidVersion, buildNumber, kernelVersion);

        // Set volume keys to control music stream, we might want make this flexible for app devs
        setVolumeControlStream(AudioManager.STREAM_MUSIC);

        float refreshRate = getDeviceRefreshRate();
        MakepadNative.initChoreographer(refreshRate, sdkVersion);
        //% MAIN_ACTIVITY_ON_CREATE
        
    }

    @Override
    protected void onStart() {
        super.onStart();

       // this forces a high framerate default 
           /*
        Window w = getWindow();
        WindowManager.LayoutParams p = w.getAttributes();
        Display.Mode[] modes = getDisplay().getSupportedModes();

        for(Display.Mode mode: modes){    
            if(mode.getRefreshRate() > 100.0){
                p.preferredDisplayModeId = mode.getModeId();
                w.setAttributes(p);
                Log.w("Makepad", "width"+mode.getRefreshRate()+" id "+mode.getModeId());
                break;
            }
        }
*/      
        MakepadNative.activityOnStart();
    }

    @Override
    protected void onResume() {
        super.onResume();
        MakepadNative.activityOnResume();

        //% MAIN_ACTIVITY_ON_RESUME
    }
    @Override
    protected void onPause() {
        super.onPause();
        MakepadNative.activityOnPause();

        //% MAIN_ACTIVITY_ON_PAUSE
    }

    @Override
    protected void onStop() {
        super.onStop();
        MakepadNative.activityOnStop();
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        MakepadNative.activityOnDestroy();
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        super.onBackPressed();
        MakepadNative.onBackPressed();
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        MakepadNative.activityOnWindowFocusChanged(hasFocus);
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        //% MAIN_ACTIVITY_ON_ACTIVITY_RESULT
    }

    @SuppressWarnings("deprecation")
    public void setFullScreen(final boolean fullscreen) {
        runOnUiThread(new Runnable() {
                @Override
                public void run() {
                    View decorView = getWindow().getDecorView();

                    if (fullscreen) {
                        getWindow().setFlags(LayoutParams.FLAG_LAYOUT_NO_LIMITS, LayoutParams.FLAG_LAYOUT_NO_LIMITS);
                        getWindow().getAttributes().layoutInDisplayCutoutMode = LayoutParams.LAYOUT_IN_DISPLAY_CUTOUT_MODE_SHORT_EDGES;
                        if (Build.VERSION.SDK_INT >= 30) {
                            getWindow().setDecorFitsSystemWindows(false);
                        } else {
                            int uiOptions = View.SYSTEM_UI_FLAG_HIDE_NAVIGATION | View.SYSTEM_UI_FLAG_FULLSCREEN | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY;
                            decorView.setSystemUiVisibility(uiOptions);
                        }
                    }
                    else {
                        if (Build.VERSION.SDK_INT >= 30) {
                            getWindow().setDecorFitsSystemWindows(true);
                        } else {
                          decorView.setSystemUiVisibility(0);
                        }

                    }
                }
            });
    }
    
    public void switchActivityClass(Class c){
        Intent intent = new Intent(getApplicationContext(), c);
        startActivity(intent);
        finish();
    }
    
    public void showKeyboard(final boolean show) {
        runOnUiThread(new Runnable() {
            @Override
            public void run() {
                if (show) {
                    InputMethodManager imm = (InputMethodManager)getSystemService(Context.INPUT_METHOD_SERVICE);
                    imm.showSoftInput(view, 0);
                } else {
                    InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
                    imm.hideSoftInputFromWindow(view.getWindowToken(),0);
                }
            }
        });
    }

    public void copyToClipboard(String content) {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        // User-facing description of the clipboard content
        String clipLabel = getApplicationName() + " clip";
        ClipData clip = ClipData.newPlainText(clipLabel, content);
        clipboard.setPrimaryClip(clip);
    }

    private String getApplicationName() {
        ApplicationInfo applicationInfo = getApplicationContext().getApplicationInfo();
        CharSequence appName = applicationInfo.loadLabel(getPackageManager());
        return appName.toString();
    }

    public void requestHttp(long id, long metadataId, String url, String method, String headers, byte[] body) {
        try {
            MakepadNetwork network = new MakepadNetwork();

            CompletableFuture<HttpResponse> future = network.performHttpRequest(url, method, headers, body);

            future.thenAccept(response -> {
                runOnUiThread(() -> MakepadNative.onHttpResponse(id, metadataId, response.getStatusCode(), response.getHeaders(), response.getBody()));
            }).exceptionally(ex -> {
                runOnUiThread(() -> MakepadNative.onHttpRequestError(id, metadataId, ex.toString()));
                return null;
            });
        } catch (Exception e) {
            MakepadNative.onHttpRequestError(id, metadataId, e.toString());
        }
    }

    public void openWebSocket(long id, String url, long callback) {
        MakepadWebSocket webSocket = new MakepadWebSocket(id, url, callback);
        mActiveWebsockets.put(id, webSocket);
        webSocket.connect();

        if (webSocket.isConnected()) {
            MakepadWebSocketReader reader = new MakepadWebSocketReader(this, webSocket);
            mWebSocketsHandler.post(reader);
            mActiveWebsocketsReaders.put(id, reader);
        }
    }

    public void sendWebSocketMessage(long id, byte[] message) {

        MakepadWebSocket webSocket = mActiveWebsockets.get(id);
        if (webSocket != null) {
            webSocket.sendMessage(message);
        }
    }

    public void closeWebSocket(long id) {
        MakepadWebSocketReader reader = mActiveWebsocketsReaders.get(id);
        if (reader != null) {
            mWebSocketsHandler.removeCallbacks(reader);
        }
        mActiveWebsocketsReaders.remove(id);
        mActiveWebsockets.remove(id);
    }

    public void webSocketConnectionDone(long id, long callback) {
        mActiveWebsockets.remove(id);
        MakepadNative.onWebSocketClosed(callback);
    }

    public String[] getAudioDevices(long flag){
        try{

            AudioManager am = (AudioManager)this.getSystemService(Context.AUDIO_SERVICE);
            AudioDeviceInfo[] devices = null;
            ArrayList<String> out = new ArrayList<String>();
            if(flag == 0){
                devices = am.getDevices(AudioManager.GET_DEVICES_INPUTS);
            }
            else{
                devices = am.getDevices(AudioManager.GET_DEVICES_OUTPUTS);
            }
            for(AudioDeviceInfo device: devices){
                int[] channel_counts = device.getChannelCounts();
                for(int cc: channel_counts){
                    out.add(String.format(
                        "%d$$%d$$%d$$%s",
                        device.getId(),
                        device.getType(),
                        cc,
                        device.getProductName().toString()
                    ));
                }
            }
            return out.toArray(new String[0]);
        }
        catch(Exception e){
            Log.e("Makepad", "exception: " + e.getMessage());
            Log.e("Makepad", "exception: " + e.toString());
            return null;
        }
    }

    @SuppressWarnings("deprecation")
    public void openAllMidiDevices(long delay){
        Runnable runnable = () -> {
            try{
                BluetoothManager bm = (BluetoothManager) this.getSystemService(Context.BLUETOOTH_SERVICE);
                BluetoothAdapter ba = bm.getAdapter();
                Set<BluetoothDevice> bluetooth_devices = ba.getBondedDevices();
                ArrayList<String> bt_names = new ArrayList<String>();
                MidiManager mm = (MidiManager)this.getSystemService(Context.MIDI_SERVICE);
                for(BluetoothDevice device: bluetooth_devices){
                    if(device.getType() == BluetoothDevice.DEVICE_TYPE_LE){
                        String name =device.getName();
                        bt_names.add(name);
                        mm.openBluetoothDevice(device, this, new Handler(Looper.getMainLooper()));
                    }
                }
                // this appears to give you nonworking BLE midi devices. So we skip those by name (not perfect but ok)
                for (MidiDeviceInfo info : mm.getDevices()){
                    String name = info.getProperties().getCharSequence(MidiDeviceInfo.PROPERTY_NAME).toString();
                    boolean found = false;
                    for (String bt_name : bt_names){
                        if (bt_name.equals(name)){
                            found = true;
                            break;
                        }
                    }
                    if(!found){
                        mm.openDevice(info, this, new Handler(Looper.getMainLooper()));
                    }
                }
            }
            catch(Exception e){
                Log.e("Makepad", "exception: " + e.getMessage());
                Log.e("Makepad", "exception: " + e.toString());
            }
        };
        if(delay != 0){
            mHandler.postDelayed(runnable, delay);
        }
        else{ // run now
            runnable.run();
        }
    }

    public void onDeviceOpened(MidiDevice device) {
        if(device == null){
            return;
        }
        MidiDeviceInfo info = device.getInfo();
        if(info != null){
            String name = info.getProperties().getCharSequence(MidiDeviceInfo.PROPERTY_NAME).toString();
            MakepadNative.onMidiDeviceOpened(name, device);
        }
    }

    public void prepareVideoPlayback(long videoId, Object source, int externalTextureHandle, boolean autoplay, boolean shouldLoop) {
        VideoPlayer VideoPlayer = new VideoPlayer(this, videoId);
        VideoPlayer.setSource(source);
        VideoPlayer.setExternalTextureHandle(externalTextureHandle);
        VideoPlayer.setAutoplay(autoplay);
        VideoPlayer.setShouldLoop(shouldLoop);
        VideoPlayerRunnable runnable = new VideoPlayerRunnable(VideoPlayer);

        mVideoPlayerRunnables.put(videoId, runnable);
        mVideoPlaybackHandler.post(runnable);
    }

    public void beginVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.beginPlayback();
        }
    }

    public void pauseVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.pausePlayback();
        }
    }

    public void resumeVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.resumePlayback();
        }
    }

    public void muteVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.mute();
        }
    }

    public void unmuteVideoPlayback(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.get(videoId);
        if(runnable != null) {
            runnable.unmute();
        }
    }

    public void cleanupVideoPlaybackResources(long videoId) {
        VideoPlayerRunnable runnable = mVideoPlayerRunnables.remove(videoId);
        if(runnable != null) {
            runnable.cleanupVideoPlaybackResources();
            runnable = null;
        }
    }
    
                
    public boolean isEmulator() {
        // hints that the app is running on emulator
        return Build.MODEL.startsWith("sdk")
            || "google_sdk".equals(Build.MODEL)
            || Build.MODEL.contains("Emulator")
            || Build.MODEL.contains("Android SDK")
            || Build.MODEL.toLowerCase().contains("droid4x")
            || Build.FINGERPRINT.startsWith("generic")
            || Build.PRODUCT == "sdk"
            || Build.PRODUCT == "google_sdk"
            || (Build.BRAND.startsWith("generic") && Build.DEVICE.startsWith("generic"));
    }

    private String getKernelVersion() {
        try {
            Process process = Runtime.getRuntime().exec("uname -r");
            BufferedReader reader = new BufferedReader(new InputStreamReader(process.getInputStream()));
            StringBuilder stringBuilder = new StringBuilder();
            String line;
            while ((line = reader.readLine()) != null) {
                stringBuilder.append(line);
            }
            return stringBuilder.toString();
        } catch (IOException e) {
            return "Unknown";
        }
    }
    
    

    @SuppressWarnings("deprecation")
    public float getDeviceRefreshRate() {
        float refreshRate = 60.0f;  // Default to a common refresh rate

        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.R) {
            // Use getDisplay() API on Android 11 and above
            Display display = getDisplay();
            if (display != null) {
                refreshRate = display.getRefreshRate();
            }
        } else {
            // Use the old method for Android 10 and below
            WindowManager windowManager = (WindowManager) getSystemService(Context.WINDOW_SERVICE);
            if (windowManager != null) {
                Display display = windowManager.getDefaultDisplay();
                refreshRate = display.getRefreshRate();
            }
        }

        return refreshRate;
    }
}
