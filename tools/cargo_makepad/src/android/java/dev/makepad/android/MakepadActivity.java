package dev.makepad.android;
import android.Manifest;

import android.os.Bundle;
import android.app.Activity;
import android.view.ViewGroup;
import android.widget.LinearLayout;
import android.widget.RelativeLayout;
import android.content.Context;
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

public class MakepadActivity extends Activity implements 
MidiManager.OnDeviceOpenedListener,
Makepad.Callback{
    @Override
    protected void onCreate(Bundle savedInstanceState) {
        // this causes a pause/resume cycle.
        if (checkSelfPermission(Manifest.permission.BLUETOOTH_CONNECT) != PackageManager.PERMISSION_GRANTED ||
            checkSelfPermission(Manifest.permission.CAMERA) != PackageManager.PERMISSION_GRANTED){
            requestPermissions(new String[]{Manifest.permission.BLUETOOTH_CONNECT, Manifest.permission.CAMERA}, 123);
        }

        super.onCreate(savedInstanceState);
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
        setContentView(mView);
        Makepad.onNewGL(mCx, this);
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

    Handler mHandler;
    HashMap<Long, Runnable> mRunnables;
    MakepadSurfaceView mView;
    long mCx;
}