package dev.makepad.android;

import java.io.*;
import java.util.Arrays;
import android.util.Log;
import android.app.Activity;
import java.lang.ref.WeakReference;

import dev.makepad.android.MakepadNative;

public class MakepadWebSocketReader implements Runnable {
    private MakepadWebSocket mWebSocket;
    private WeakReference<Activity> mActivityReference;
    
    public MakepadWebSocketReader(Activity activity, MakepadWebSocket webSocket) {
        mActivityReference = new WeakReference<>(activity);
        mWebSocket = webSocket;
    }

    @Override
    public void run() {
        readMessages();
    }

    public void readMessages() {
        try {
            Activity activity = mActivityReference.get();
            if (activity == null) {
                Log.e("Makepad", "Activity not found");
                return;
            }

            byte[] rawbuffer = new byte[16384];
            int readBytes;

            while ((readBytes = mWebSocket.getInputStream().read(rawbuffer)) != -1) {
                // TODO intercept PONG and other special messages
                byte[] message = Arrays.copyOfRange(rawbuffer, 0, readBytes);
                activity.runOnUiThread(() -> {
                    MakepadNative.onWebSocketMessage(mWebSocket.getMakepadRequestId(), message);
                });
            }
            Log.i("Makepad", "Websocket connection was closed by server.");     
        } catch(Exception e) {
            Log.e("Makepad", "exception: " + e.getMessage());             
            Log.e("Makepad", "exception: " + e.toString());
        }
    }
}