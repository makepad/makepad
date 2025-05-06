package dev.makepad.android;

import java.io.*;
import java.util.Arrays;
import android.util.Log;
import android.app.Activity;
import java.lang.ref.WeakReference;

import dev.makepad.android.MakepadNative;

public class MakepadWebSocketReader implements Runnable {
    private MakepadWebSocket mWebSocket;
    private WeakReference<MakepadActivity> mActivityReference;
    
    public MakepadWebSocketReader(MakepadActivity activity, MakepadWebSocket webSocket) {
        mActivityReference = new WeakReference<>(activity);
        mWebSocket = webSocket;
    }

    @Override
    public void run() {
        readMessages();
    }

    private static final byte[] CLOSE = {(byte) 0x88, 0};
    private static final byte[] PING = {(byte) 0x89, 0};
    private static final byte[] PONG = {(byte) 0x8A, 0};

    public void readMessages() {
        MakepadActivity activity = mActivityReference.get();
        if (activity == null) {
            Log.e("Makepad", "Activity not found");
            return;
        }

        
        try {
            byte[] rawbuffer = new byte[1024*1024];
            int readBytes;

            while ((readBytes = mWebSocket.getInputStream().read(rawbuffer)) != -1) {
                /*if (readBytes == 2 && rawbuffer[0] == PING[0] && rawbuffer[1] == PING[1]) {
                    OutputStream ostream = mWebSocket.getOutputStream();
                    ostream.write(PONG, 0, 2);
                    ostream.flush();
                    continue;
                }*/

                byte[] message = Arrays.copyOfRange(rawbuffer, 0, readBytes);
                activity.runOnUiThread(() -> {
                    long callback = mWebSocket.getMakepadCallback();
                    MakepadNative.onWebSocketMessage(message, callback);
                });
            }
            activity.runOnUiThread(() -> {
                long requestId = mWebSocket.getMakepadRequestId();
                long callback = mWebSocket.getMakepadCallback();
                activity.webSocketConnectionDone(requestId, callback);
            });
        } catch(Exception e) {
            activity.runOnUiThread(() -> {
                long callback = mWebSocket.getMakepadCallback();
                MakepadNative.onWebSocketError(e.toString(), callback);
            });

            Log.e("Makepad", "exception: " + e.getMessage());             
            Log.e("Makepad", "exception: " + e.toString());
        }
    }
}