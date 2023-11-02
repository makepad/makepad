package dev.makepad.android;

import java.nio.charset.StandardCharsets;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;

import java.net.Socket;
import java.net.InetSocketAddress;
import java.io.*;

import java.util.Arrays;
import android.util.Log;

import dev.makepad.android.MakepadNative;

import android.os.StrictMode;
import android.os.StrictMode.ThreadPolicy;

public class MakepadWebSocket implements Runnable {
    private long mMakepadRequestId;
    private String mUrl;

    private Socket mSocket;
    private OutputStream mSocketOutStream;
    private InputStream mSocketInpStream;
    private boolean mReadyForMessages = false;

    private static final int ONE_MIN = 60 * 1000;
    
    public MakepadWebSocket(long makepadRequestId, String url) {
        mMakepadRequestId = makepadRequestId;
        mUrl = url;
    }

    public void connect() {
        // TODO Refactor to connect the socket out of UI thread
        if (android.os.Build.VERSION.SDK_INT > 9) {
            StrictMode.ThreadPolicy gfgPolicy = 
                new StrictMode.ThreadPolicy.Builder().permitAll().build();
            StrictMode.setThreadPolicy(gfgPolicy);
        }

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
            
            mSocketOutStream = mSocket.getOutputStream();
            mSocketInpStream = mSocket.getInputStream();

            doHandshake();
            mReadyForMessages = true;
        } catch (Exception e) {
            throw new RuntimeException(e);
        }
    }

    private void doHandshake() throws IOException {
        BufferedWriter socketWriter = new BufferedWriter(new OutputStreamWriter(mSocketOutStream));
        BufferedReader socketReader = new BufferedReader(new InputStreamReader(mSocketInpStream));

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

    @Override
    public void run() {
        connect();
        readMessages();
    }

    public String readMessages() {
        if(mReadyForMessages) {
            try {
                byte[] rawbuffer = new byte[16384];
                int readBytes;

                while ((readBytes = mSocketInpStream.read(rawbuffer)) != -1) {
                    // TODO intercept PONG and other special messages
                    byte[] message = Arrays.copyOfRange(rawbuffer, 0, readBytes);
                    MakepadNative.onWebSocketMessage(mMakepadRequestId, message);
                }
                Log.i("Makepad", "Websocket connection was closed by server.");     
            } catch(Exception e) {
                Log.e("Makepad", "exception: " + e.getMessage());             
                Log.e("Makepad", "exception: " + e.toString());
            }
        }

        return null;
    }
}