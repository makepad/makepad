package dev.makepad.android;
import android.Manifest;

import android.os.Bundle;
import android.app.Activity;
import android.view.Gravity;
import android.view.View;
import android.view.ViewGroup;
import android.view.inputmethod.InputMethodManager;
import android.view.MenuInflater;
import android.view.ContextMenu;
import android.view.ContextMenu.ContextMenuInfo;
import android.view.MenuItem;
import android.content.Context;
import android.content.ClipData;
import android.content.ClipDescription;
import android.content.ClipboardManager;
import android.util.Log;
import android.content.pm.PackageManager;
import android.content.res.AssetManager;
import android.os.Handler;
import android.os.Looper;

import java.util.HashMap;
import java.util.ArrayList;
import java.util.Set;
import java.util.UUID;
import java.io.File;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.io.OutputStream;

import android.os.Bundle;
import android.os.ParcelUuid;

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

import java.util.concurrent.CompletableFuture;
import java.util.concurrent.ExecutionException;

import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaExtractor;
import android.media.MediaFormat;

import java.nio.ByteBuffer;

import android.media.MediaDataSource;
import java.io.IOException;

public class MakepadActivity extends Activity implements 
MidiManager.OnDeviceOpenedListener,
View.OnCreateContextMenuListener,
Makepad.Callback{
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // this causes a pause/resume cycle.
        if (checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) != PackageManager.PERMISSION_GRANTED ||
            checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED){
            requestPermissions(
                new String[]{
                    Manifest.permission.BLUETOOTH_CONNECT, 
                    Manifest.permission.CAMERA,
                    Manifest.permission.INTERNET
                    }, 
                123
            );
        }

        super.onCreate(savedInstanceState);
        Makepad.onHookPanic();
        mCx = Makepad.onNewCx();

        mHandler = new Handler(Looper.getMainLooper());
        mRunnables = new HashMap<Long, Runnable>();

        String cache_path = this.getCacheDir().getAbsolutePath();
        float density = getResources().getDisplayMetrics().density;

        Makepad.onInit(mCx, cache_path, density, this);
    }

    @Override
    protected void onStart() {
        super.onStart();
        mView = new MakepadSurfaceView(this, mCx);

        // This is a requirement to have access to the IME (soft keyword)
        mView.setFocusable(true);
        mView.setFocusableInTouchMode(true);

        setContentView(mView);
        Makepad.onNewGL(mCx, this);

        registerForContextMenu(mView);
    }

    @Override
    protected void onPause() {
         super.onPause();
        Makepad.onPause(mCx, this);
    }

    @Override
    protected void onStop() {
        super.onStop();
        Makepad.onFreeGL(mCx, this);
    }

    @Override
    protected void onResume() {
        super.onResume();
        if(mCx != 0){
            //mView = new MakepadSurfaceView(this, mCx);
            //setContentView(mView);
            Makepad.onResume(mCx, this);
        }
    }

    @Override
    protected void onDestroy() {
        super.onDestroy();
        Makepad.onDropCx(mCx);
    }

    public void scheduleTimeout(long id, long delay) {
        Runnable runnable = () -> {
            mRunnables.remove(id);
            Makepad.onTimeout(mCx, id, this);
        };
        mRunnables.put(id, runnable);
        mHandler.postDelayed(runnable, delay);
    }

    public void cancelTimeout(long id) {
        mHandler.removeCallbacks(mRunnables.get(id));
        mRunnables.remove(id);
    }

    public void scheduleRedraw() {
        mView.invalidate();
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
            Makepad.onMidiDeviceOpened(mCx, name, device, this);
        }
    }

    public byte[] readAsset(String path){
       try{
            InputStream in = this.getAssets().open(path);
            ByteArrayOutputStream out = new ByteArrayOutputStream();
            int byteCount = 0;
            byte[] buffer = new byte[4096];
            while (true) {
                int read = in.read(buffer);
                if (read == -1) {
                    break;
                }
                out.write(buffer, 0, read);
                byteCount += read;
            }
            return out.toByteArray();
        }catch(Exception e){
            return null;
        }
    }

    public void swapBuffers() {
        mView.swapBuffers();
    }

    public void showTextIME() {
        if (mView.requestFocus()) {
            InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
            imm.showSoftInput(mView, 0);
        } else {
            Log.e("Makepad", "can not display software keyboard (IME)");
        }
    }

    public void hideTextIME() {
        InputMethodManager imm = (InputMethodManager) getSystemService(Context.INPUT_METHOD_SERVICE);
        imm.hideSoftInputFromWindow(mView.getWindowToken(), 0);
    }

    public void showClipboardActions(String selected) {
        mSelectedText = selected;
        mView.showContextMenu();
    }

    public void copyToClipboard(String selected) {
        mSelectedText = selected;
        copySelectedTextToClipboard();
    }

    public void pasteFromClipboard(){
        String text = getTextFromClipboard();
        Makepad.onPasteFromClipboard(mCx, text, (Makepad.Callback)mView.getContext());
    }

    @Override
    public void onCreateContextMenu(ContextMenu menu, View v, ContextMenuInfo menuInfo) {
        super.onCreateContextMenu(menu, v, menuInfo);

        MenuInflater inflater = getMenuInflater();
        inflater.inflate(R.menu.context_menu, menu);

        MenuItem copyItem = menu.findItem(R.id.menu_copy);
        MenuItem cutItem = menu.findItem(R.id.menu_cut);
        if (mSelectedText == null || mSelectedText.isEmpty()) {
            copyItem.setVisible(false);
            cutItem.setVisible(false);
        } else {
            copyItem.setVisible(true);
            cutItem.setVisible(true);
        }

        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        MenuItem pasteItem = menu.findItem(R.id.menu_paste);
        if (!(clipboard.hasPrimaryClip())) {
            pasteItem.setVisible(false);
        } else if (
            clipboard.getPrimaryClipDescription().hasMimeType(ClipDescription.MIMETYPE_TEXT_PLAIN) ||
            clipboard.getPrimaryClipDescription().hasMimeType(ClipDescription.MIMETYPE_TEXT_HTML)
        ) {
            pasteItem.setVisible(true);
        } else {
            pasteItem.setVisible(false);
        }
    }

    @Override
    public boolean onContextItemSelected(MenuItem item) {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        ClipData clip;
        switch (item.getItemId()) {
            case R.id.menu_copy:
                copySelectedTextToClipboard();
                return true;

            case R.id.menu_cut:
                copySelectedTextToClipboard();
                Makepad.onCutToClipboard(mCx, (Makepad.Callback)mView.getContext());
                return true;

            case R.id.menu_paste:
                String text = getTextFromClipboard();
                Makepad.onPasteFromClipboard(mCx, text, (Makepad.Callback)mView.getContext());
                return true;
                
            default:
                return super.onContextItemSelected(item);
        }
    }

    public void requestHttp(long id, String url, String method, String headers, byte[] body) {
        try {
            MakepadNetwork network = new MakepadNetwork();

            CompletableFuture<HttpResponse> future = network.performHttpRequest(url, method, headers, body);

            future.thenAccept(response -> {
                runOnUiThread(() -> Makepad.onHttpResponse(mCx, id, response.getStatusCode(), response.getHeaders(), response.getBody(), (Makepad.Callback) mView.getContext()));
            }).exceptionally(ex -> {
                runOnUiThread(() -> Makepad.onHttpRequestError(mCx, id, ex.toString(), (Makepad.Callback) mView.getContext()));
                return null;
            });
        } catch (Exception e) {
            Makepad.onHttpRequestError(mCx, id, e.toString(), (Makepad.Callback) mView.getContext());
        }
    }

    private void copySelectedTextToClipboard() {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        ClipData clip = ClipData.newPlainText("Makepad", mSelectedText);
        clipboard.setPrimaryClip(clip);
    }
        
    private String getTextFromClipboard() {
        ClipboardManager clipboard = (ClipboardManager) getSystemService(Context.CLIPBOARD_SERVICE);
        try{
            String text;
            if (clipboard.hasPrimaryClip()) {
                ClipData.Item cb_item = clipboard.getPrimaryClip().getItemAt(0);
                text = cb_item.getText().toString();
            } else {
                text = null;
            }

            return text;
        }
        catch(Exception e){
            Log.e("Makepad", "exception: " + e.getMessage());
            Log.e("Makepad", "exception: " + e.toString());

            return null;
        }
    }

    public void decodeVideo(byte[] video) {
        MediaExtractor extractor = new MediaExtractor();
        try {
            ByteArrayMediaDataSource dataSource = new ByteArrayMediaDataSource(video);

            extractor.setDataSource(dataSource);

            int trackIndex = selectTrack(extractor);
            if (trackIndex < 0) {
                throw new RuntimeException("No video track found in video");
            }
            extractor.selectTrack(trackIndex);
            MediaFormat format = extractor.getTrackFormat(trackIndex);

            long duration = format.getLong(MediaFormat.KEY_DURATION); // in microseconds
            int frameRate = format.containsKey(MediaFormat.KEY_FRAME_RATE) 
                ? format.getInteger(MediaFormat.KEY_FRAME_RATE) 
                : 30; // defaulting to 30 fps
            long totalFrames = (duration / 1_000_000) * frameRate;

            // Assuming you have a method to send the estimated total number of frames to Rust:
            // Makepad.sendTotalFramesEstimation(mCx, totalFrames);

            String mime = format.getString(MediaFormat.KEY_MIME);
            MediaCodec codec = MediaCodec.createDecoderByType(mime);
            codec.configure(format, null, null, 0);
            codec.start();

            MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
            int videoWidth = format.getInteger(MediaFormat.KEY_WIDTH);
            int videoHeight = format.getInteger(MediaFormat.KEY_HEIGHT);

            boolean inputEos = false;
            boolean outputEos = false;

            while (!outputEos) {
                if (!inputEos) {
                    int inputBufferIndex = codec.dequeueInputBuffer(2000);
                    if (inputBufferIndex >= 0) {
                        ByteBuffer inputBuffer = codec.getInputBuffer(inputBufferIndex);
                        int sampleSize = extractor.readSampleData(inputBuffer, 0);
                        if (sampleSize < 0) {
                            codec.queueInputBuffer(inputBufferIndex, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                            inputEos = true;
                        } else {
                            long presentationTimeUs = extractor.getSampleTime();
                            codec.queueInputBuffer(inputBufferIndex, 0, sampleSize, presentationTimeUs, 0);
                            extractor.advance();
                        }
                    }
                }

                int outputBufferIndex = codec.dequeueOutputBuffer(info, 2000);
                if (outputBufferIndex >= 0) {
                    ByteBuffer outputBuffer = codec.getOutputBuffer(outputBufferIndex);
                    byte[] pixelData = new byte[info.size];
                    outputBuffer.get(pixelData);
                    codec.releaseOutputBuffer(outputBufferIndex, false);

                    if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                        outputEos = true;
                    }

                    // TODO: Send color format to rust 
                    // MediaFormat outputFormat = codec.getOutputFormat();
                    // int actualColorFormat = outputFormat.getInteger(MediaFormat.KEY_COLOR_FORMAT);

                    Makepad.onVideoStream(mCx, 
                        pixelData, 
                        videoWidth, 
                        videoHeight, 
                        frameRate, 
                        info.presentationTimeUs, 
                        outputEos,
                        (Makepad.Callback)mView.getContext()
                    );
                }
            }

            codec.stop();
            codec.release();
        } catch (Exception e) {
            e.printStackTrace();
        } finally {
            extractor.release();
        }
    }

    private int selectTrack(MediaExtractor extractor) {
        int numTracks = extractor.getTrackCount();
        for (int i = 0; i < numTracks; i++) {
            MediaFormat format = extractor.getTrackFormat(i);
            String mime = format.getString(MediaFormat.KEY_MIME);
            if (mime.startsWith("video/")) {
                return i;
            }
        }
        return -1;
    }

    Handler mHandler;
    HashMap<Long, Runnable> mRunnables;
    MakepadSurfaceView mView;
    long mCx;
    String mSelectedText;
}
