package dev.makepad.android;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLSocketFactory;
import java.net.Socket;
import java.net.InetSocketAddress;
import java.net.URI;
import java.net.URISyntaxException;

import java.io.BufferedWriter;
import java.io.BufferedReader;
import java.io.OutputStream;
import java.io.InputStream;
import java.io.OutputStreamWriter;
import java.io.InputStreamReader;
import java.io.IOException;
import java.util.Random;
import android.util.Log;

import dev.makepad.android.MakepadNative;

public class MakepadWebSocket {
    private long mMakepadRequestId;
    private long mCallback;
    private String mUrl;

    private Socket mSocket;
    private boolean mIsConnected = false;

    private static final int ONE_MIN = 60 * 1000;
    
    public MakepadWebSocket(long makepadRequestId, String url, long callback) {
        mMakepadRequestId = makepadRequestId;
        mUrl = url;
        mCallback = callback;
    }

    public void connect() {
        try {
            URI uri = new URI(mUrl);
            String host = uri.getHost();
            int port = uri.getPort() == -1 ? 443 : uri.getPort();

            InetSocketAddress address = new InetSocketAddress(host, port);

            mSocket = new Socket();
            mSocket.setSoTimeout(0);
            mSocket.connect(address, ONE_MIN);
            //Log.d("Makepad", "Socket connected " + uri.getScheme());

            mSocket.setKeepAlive(true);

            if (uri.getScheme().equals("wss")) {
                this.convertToSSLSocket(host, port);
            }
            doHandshake();
        } catch (Exception e) {
            MakepadNative.onWebSocketError(e.toString(), mCallback);
            // throw new RuntimeException(e);
        }
    }

    private void convertToSSLSocket(String host, int port) throws Exception {
        SSLContext sslContext = SSLContext.getInstance("TLSv1.2");
        sslContext.init(null, null, null);
        SSLSocketFactory factory = sslContext.getSocketFactory();
        mSocket = factory.createSocket(mSocket, host, port, true);
        //Log.d("Makepad", "SSL Socket connected");
    }

    private void doHandshake() throws IOException {
        BufferedWriter socketWriter = new BufferedWriter(new OutputStreamWriter(mSocket.getOutputStream()));
        BufferedReader socketReader = new BufferedReader(new InputStreamReader(mSocket.getInputStream()));

        try {
            String request = this.buildHandshakeRequest();
            socketWriter.write(request);
            socketWriter.flush();

            char[] dataArray = new char[1024];
            int length;
            do {
                String line = socketReader.readLine();
                length = line.length();
            } while (length > 0);

            mIsConnected = true;
        } catch(Exception e) {
            MakepadNative.onWebSocketError(e.toString(), mCallback);
            Log.e("Makepad", "exception: " + e.getMessage());             
            Log.e("Makepad", "exception: " + e.toString());
        }
    }

    public void sendMessage(byte[] frame) {
        try {
            OutputStream ostream = mSocket.getOutputStream();
            ostream.write(frame, 0, frame.length);
            ostream.flush();
        } catch(Exception e) {
            MakepadNative.onWebSocketError(e.toString(), mCallback);
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

    public OutputStream getOutputStream() throws IOException {
        return mSocket.getOutputStream();
    }

    public long getMakepadRequestId() {
        return mMakepadRequestId;
    }

    public long getMakepadCallback() {
        return mCallback;
    }

    public void closeSocketAndClearCallback() {
        try {
            mSocket.close();
        }
        catch(Exception e) {}
        mCallback = 0;
    }

    private String buildHandshakeRequest() throws URISyntaxException {
        URI uri = new URI(mUrl);
        String host = uri.getHost();
        String path = uri.getPath();
        String query = uri.getQuery() == null ? "" : uri.getQuery();
        int port =  uri.getPort() == -1 ? 443 : uri.getPort();

        String content = "GET " + path + (query.isEmpty() ? "" : "?" + query) + " HTTP/1.1\r\n" +
            "Host: " + host + "\r\n" +
            "Upgrade: websocket\r\n" +
            "Connection: Upgrade\r\n" +
            "Sec-WebSocket-Key: " + this.randomStringKey(22) + "==\r\n" +
            "Sec-WebSocket-Version: 13\r\n" +
            "Accept-Encoding: gzip, deflate\r\n" +
            "Accept-Language: en-US,en;q=0.9,es;q=0.8\r\n" +
            "Cache-Control: no-cache\r\n" +
            "Origin: localhost\r\n" +
            "\r\n";

            //Log.d("Makepad", "content: " + content);

        return content;
    }

    private static final String ALLOWED_CHARACTERS = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789-_";

    public static String randomStringKey(int length) {
        Random generator = new Random();
        StringBuilder randomStringBuilder = new StringBuilder();
        char tempChar;
        int ramdomIndex;
        for (int i = 0; i < length; i++){
            ramdomIndex = generator.nextInt(ALLOWED_CHARACTERS.length());
            tempChar = ALLOWED_CHARACTERS.charAt(ramdomIndex);
            randomStringBuilder.append(tempChar);
        }
        return randomStringBuilder.toString();
    }
}