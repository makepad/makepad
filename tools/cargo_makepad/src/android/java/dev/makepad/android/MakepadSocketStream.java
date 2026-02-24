package dev.makepad.android;

import android.os.Build;

import java.io.InputStream;
import java.io.OutputStream;
import java.net.InetSocketAddress;
import java.net.Socket;
import java.security.SecureRandom;
import java.security.cert.X509Certificate;

import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLParameters;
import javax.net.ssl.SSLSocket;
import javax.net.ssl.SSLSocketFactory;
import javax.net.ssl.TrustManager;
import javax.net.ssl.X509TrustManager;

public class MakepadSocketStream {
    private static final int CONNECT_TIMEOUT_MS = 60 * 1000;

    private Socket mSocket;
    private InputStream mInputStream;
    private OutputStream mOutputStream;

    public synchronized boolean connect(String host, int port, boolean useTls, boolean ignoreSslCert) {
        try {
            Socket socket = new Socket();
            socket.connect(new InetSocketAddress(host, port), CONNECT_TIMEOUT_MS);
            socket.setKeepAlive(true);

            if (useTls) {
                socket = createTlsSocket(socket, host, port, ignoreSslCert);
            }

            mSocket = socket;
            mInputStream = socket.getInputStream();
            mOutputStream = socket.getOutputStream();
            return true;
        } catch (Exception e) {
            close();
            return false;
        }
    }

    public synchronized byte[] read(int maxBytes) {
        if (mInputStream == null) {
            return null;
        }
        if (maxBytes <= 0) {
            return new byte[0];
        }

        try {
            byte[] buffer = new byte[maxBytes];
            int readBytes = mInputStream.read(buffer);
            if (readBytes < 0) {
                return null;
            }
            if (readBytes == buffer.length) {
                return buffer;
            }
            byte[] result = new byte[readBytes];
            System.arraycopy(buffer, 0, result, 0, readBytes);
            return result;
        } catch (Exception e) {
            return null;
        }
    }

    public synchronized int write(byte[] message) {
        if (mOutputStream == null || message == null) {
            return -1;
        }

        try {
            mOutputStream.write(message, 0, message.length);
            mOutputStream.flush();
            return message.length;
        } catch (Exception e) {
            return -1;
        }
    }

    public synchronized void setReadTimeout(int timeoutMs) {
        if (mSocket == null) {
            return;
        }
        try {
            mSocket.setSoTimeout(Math.max(0, timeoutMs));
        } catch (Exception e) {
        }
    }

    public synchronized void setWriteTimeout(int timeoutMs) {
        // java.net.Socket has no write-timeout setter.
        // Keep API parity with other platforms.
    }

    public synchronized void close() {
        try {
            if (mInputStream != null) {
                mInputStream.close();
            }
        } catch (Exception e) {
        }
        try {
            if (mOutputStream != null) {
                mOutputStream.close();
            }
        } catch (Exception e) {
        }
        try {
            if (mSocket != null) {
                mSocket.close();
            }
        } catch (Exception e) {
        }
        mInputStream = null;
        mOutputStream = null;
        mSocket = null;
    }

    private Socket createTlsSocket(Socket plainSocket, String host, int port, boolean ignoreSslCert) throws Exception {
        SSLContext sslContext = SSLContext.getInstance("TLS");

        if (ignoreSslCert) {
            TrustManager[] trustManagers = new TrustManager[] {
                new X509TrustManager() {
                    @Override
                    public void checkClientTrusted(X509Certificate[] chain, String authType) {
                    }

                    @Override
                    public void checkServerTrusted(X509Certificate[] chain, String authType) {
                    }

                    @Override
                    public X509Certificate[] getAcceptedIssuers() {
                        return new X509Certificate[0];
                    }
                }
            };
            sslContext.init(null, trustManagers, new SecureRandom());
        } else {
            sslContext.init(null, null, new SecureRandom());
        }

        SSLSocketFactory factory = sslContext.getSocketFactory();
        SSLSocket tlsSocket = (SSLSocket) factory.createSocket(plainSocket, host, port, true);

        if (!ignoreSslCert && Build.VERSION.SDK_INT >= Build.VERSION_CODES.N) {
            SSLParameters params = tlsSocket.getSSLParameters();
            params.setEndpointIdentificationAlgorithm("HTTPS");
            tlsSocket.setSSLParameters(params);
        }

        tlsSocket.startHandshake();
        return tlsSocket;
    }
}
