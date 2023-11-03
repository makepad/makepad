package dev.makepad.android;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;

import java.net.Socket;
import java.net.InetSocketAddress;
import java.io.*;

import android.util.Log;

import dev.makepad.android.MakepadNative;

public class MakepadWebSocket {
    private long mMakepadRequestId;
    private String mUrl;

    private Socket mSocket;
    private boolean mIsConnected = false;

    private static final int ONE_MIN = 60 * 1000;
    
    public MakepadWebSocket(long makepadRequestId, String url) {
        mMakepadRequestId = makepadRequestId;
        mUrl = url;
    }

    public void connect() {
        try {
            InetSocketAddress address = new InetSocketAddress(
                "socketsbay.com",
                443
                );

            mSocket = new Socket();
            mSocket.setSoTimeout(0);
            mSocket.connect(address, ONE_MIN);
            Log.d("Makepad", "Socket connected");

            mSocket.setKeepAlive(true);

            // TODO Check if the url is wss before doing the following

            SSLContext sslContext = SSLContext.getInstance("TLSv1.2");
            sslContext.init(null, null, null);
            SSLSocketFactory factory = sslContext.getSocketFactory();
            mSocket = factory.createSocket(mSocket, "socketsbay.com", 443, true);
            Log.d("Makepad", "SSL Socket connected");

            doHandshake();
            mIsConnected = true;
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    private void doHandshake() throws IOException {
        BufferedWriter socketWriter = new BufferedWriter(new OutputStreamWriter(mSocket.getOutputStream()));
        BufferedReader socketReader = new BufferedReader(new InputStreamReader(mSocket.getInputStream()));

        try {
            String data = "GET /wss/v2/1/demo/ HTTP/1.1\r\nHost: socketsbay.com\r\nUpgrade: websocket\r\nConnection: Upgrade\r\nSec-WebSocket-Key: dtaSo/23Job/Yr4kcBZlng==\r\nSec-WebSocket-Version: 13\r\nAccept-Encoding: gzip, deflate\r\nAccept-Language: en-US,en;q=0.9,es;q=0.8\r\nCache-Control: no-cache\r\nOrigin: localhost\r\n\r\n";
            socketWriter.write(data);
            socketWriter.flush();

            char[] dataArray = new char[1024];
            int length;
            do {
                String responseLine = socketReader.readLine();
                Log.d("Makepad", "Handshake response: " + responseLine);
                length = responseLine.length();
            } while (length > 0);
        } catch(Exception e) {
            Log.e("Makepad", "exception: " + e.getMessage());             
            Log.e("Makepad", "exception: " + e.toString());
        }
    }

    public boolean isConnected() {
        return mIsConnected;
    }
 
    public InputStream getInputStream() throws IOException {
        return mSocket.getInputStream();
    }

    public long getMakepadRequestId() {
        return mMakepadRequestId;
    }
}