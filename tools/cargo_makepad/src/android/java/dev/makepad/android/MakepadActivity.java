package dev.makepad.android;

import javax.microedition.khronos.egl.EGLConfig;
import javax.microedition.khronos.opengles.GL10;

import android.app.Activity;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.InputMethodManager;
import android.content.Context;
import android.os.Handler;
import android.os.HandlerThread;
import android.os.Looper;

import java.util.HashMap;
import java.util.ArrayList;
import java.io.OutputStream;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.LinkedList;

import android.os.Bundle;
import android.os.Build;
import android.util.Log;

import android.view.View;
import android.view.Surface;
import android.view.Window;
import android.view.WindowInsets;
import android.view.WindowManager.LayoutParams;
import android.view.SurfaceView;
import android.view.SurfaceHolder;
import android.view.MotionEvent;
import android.view.KeyEvent;
import android.view.inputmethod.InputMethodManager;

import android.media.midi.MidiManager;
import android.media.midi.MidiDeviceInfo;
import android.media.midi.MidiDevice;
import android.media.midi.MidiReceiver;
import android.media.AudioManager;
import android.media.midi.MidiOutputPort;
import android.media.AudioDeviceInfo;

import android.bluetooth.BluetoothManager;
import android.bluetooth.BluetoothAdapter;
import android.bluetooth.BluetoothDevice;

import android.content.Context;
import android.content.Intent;
import android.os.Handler;
import android.os.Looper;

import android.graphics.Color;
import android.graphics.Insets;
import android.view.inputmethod.InputConnection;
import android.view.inputmethod.EditorInfo;
import android.widget.LinearLayout;

import android.view.ViewTreeObserver;
import android.view.WindowInsets;
import android.graphics.Rect;

import java.util.concurrent.CompletableFuture;
import java.util.ArrayList;
import java.util.Set;
import java.util.Iterator;

import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaExtractor;
import android.media.MediaFormat;

import java.nio.ByteBuffer;
import android.media.MediaDataSource;
import java.io.IOException;

import dev.makepad.android.MakepadNative;

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
        ViewTreeObserver.OnGlobalLayoutListener,
        SurfaceHolder.Callback {

    public MakepadSurface(Context context){
        super(context);
        getHolder().addCallback(this);

        setFocusable(true);
        setFocusableInTouchMode(true);
        requestFocus();
        setOnTouchListener(this);
        setOnKeyListener(this);
        getViewTreeObserver().addOnGlobalLayoutListener(this);
    }

    @Override
    public void surfaceCreated(SurfaceHolder holder) {
        Log.i("SAPP", "surfaceCreated");
        Surface surface = holder.getSurface();
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
        MakepadNative.surfaceOnSurfaceChanged(surface, width, height);

    }
    @Override
    public boolean onTouch(View view, MotionEvent event) {
        MakepadNative.surfaceOnTouch(event);
        return true;    
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

public class MakepadActivity extends Activity implements 
MidiManager.OnDeviceOpenedListener{
    //% MAIN_ACTIVITY_BODY

    private MakepadSurface view;
    Handler mHandler;

    // video decoding
    Handler mDecoderHandler;
    HashMap<Long, VideoDecoderRunnable> mDecoderRunnables;
    private HashMap<Long, BlockingQueue<ByteBuffer>> mVideoFrameQueues = new HashMap<>();
    private static final int VIDEO_CHUNK_BUFFER_POOL_SIZE = 5; 
    private LinkedList<ByteBuffer> mVideoChunkBufferPool = new LinkedList<>();

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

        HandlerThread decoderThreadHandler = new HandlerThread("VideoDecoderThread");
        decoderThreadHandler.start(); // TODO: only start this if its needed.
        mDecoderHandler = new Handler(decoderThreadHandler.getLooper());
        mDecoderRunnables = new HashMap<Long, VideoDecoderRunnable>();

        String cache_path = this.getCacheDir().getAbsolutePath();
        float density = getResources().getDisplayMetrics().density;

        MakepadNative.onAndroidParams(cache_path, density);

        //% MAIN_ACTIVITY_ON_CREATE
    }

    @Override
    protected void onResume() {
        super.onResume();
        MakepadNative.activityOnResume();

        //% MAIN_ACTIVITY_ON_RESUME
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        Log.w("SAPP", "onBackPressed");

        // TODO: here is the place to handle request_quit/order_quit/cancel_quit

        super.onBackPressed();
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
    protected void onPause() {
        super.onPause();
        MakepadNative.activityOnPause();

        //% MAIN_ACTIVITY_ON_PAUSE
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

    public void initializeVideoDecoding(long videoId, byte[] videoData) {
        BlockingQueue<ByteBuffer> videoFrameQueue = new LinkedBlockingQueue<>();
        mVideoFrameQueues.put(videoId, videoFrameQueue);

        VideoDecoder videoDecoder = new VideoDecoder(this, videoId, videoFrameQueue);
        VideoDecoderRunnable runnable = new VideoDecoderRunnable(videoData, videoDecoder);

        mDecoderRunnables.put(videoId, runnable);
        mDecoderHandler.post(runnable);
    }

    public void decodeNextVideoChunk(long videoId, int maxFramesToDecode) {
        VideoDecoderRunnable runnable = mDecoderRunnables.get(videoId);
        if(runnable != null) {
            runnable.setMaxFramesToDecode(maxFramesToDecode);
            mDecoderHandler.post(runnable);
        }
    } 

    public void fetchNextVideoFrames(long videoId, int numberFrames) {
        BlockingQueue<ByteBuffer> videoFrameQueue = mVideoFrameQueues.get(videoId);
        if (videoFrameQueue != null) {
            int totalBytes = 0;
            Iterator<ByteBuffer> iterator = videoFrameQueue.iterator();
            int frameCount = 0;
            while (iterator.hasNext() && frameCount < numberFrames) {
                totalBytes += iterator.next().remaining();
                frameCount++;
            }

            VideoDecoderRunnable runnable = mDecoderRunnables.get(videoId);
            ByteBuffer frameGroup = acquireBuffer(totalBytes);

            for (int i = 0; i < frameCount; i++) {
                ByteBuffer frame = videoFrameQueue.poll();
                if (frame != null) {
                    frameGroup.put(frame);
                    if (runnable != null) {
                        runnable.releaseBuffer(frame);
                    }
                }
            }

            frameGroup.flip();
            runOnUiThread(() -> MakepadNative.onVideoStream(videoId, frameGroup));
            releaseBuffer(frameGroup);
        }
    }

    public void cleanupVideoDecoding(long videoId) {
        VideoDecoderRunnable runnable = mDecoderRunnables.remove(videoId);
        if(runnable != null) {
            runnable.cleanup();
        }
        mVideoFrameQueues.remove(videoId);
    }

    private ByteBuffer acquireBuffer(int size) {
        synchronized(mVideoChunkBufferPool) {
            if (!mVideoChunkBufferPool.isEmpty()) {
                ByteBuffer buffer = mVideoChunkBufferPool.poll();
                if (buffer.capacity() == size) {
                    return buffer;
                } else {
                    return ByteBuffer.allocateDirect(size);
                }
            } else {
                return ByteBuffer.allocateDirect(size);
            }
        }
    }

    private void releaseBuffer(ByteBuffer buffer) {
        synchronized(mVideoChunkBufferPool) {
            if (mVideoChunkBufferPool.size() < VIDEO_CHUNK_BUFFER_POOL_SIZE) {
                buffer.clear();
                mVideoChunkBufferPool.offer(buffer);
            }
        }
    }
}

