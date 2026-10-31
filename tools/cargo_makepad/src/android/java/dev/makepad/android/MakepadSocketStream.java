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

/**
 * A TCP (or TLS) stream for native code. One reader thread per stream blocks in the socket's
 * read (big reads, no timeout) and fills a ring; native code takes what is there in one call
 * ({@link #readInto}: straight into its own memory, no array made per read) and waits on the
 * ring's monitor up to its read timeout when it is empty (no SocketTimeoutException per poll).
 * Writes never wait for a read (their own lock).
 */
public class MakepadSocketStream {
    private static final int CONNECT_TIMEOUT_MS = 60 * 1000;
    /** What one socket read asks for, and what the ring holds before the reader waits. */
    private static final int CHUNK = 256 << 10;
    private static final int RING = 4 << 20;

    private Socket mSocket;
    private InputStream mInputStream;
    private OutputStream mOutputStream;
    private final Object mWriteLock = new Object();

    /** The ring (guarded by itself): bytes from mStart, mCount of them. */
    private final byte[] mRing = new byte[RING];
    private int mStart;
    private int mCount;
    /** The reader ended (end of stream or an error): once the ring is empty, reads fail. */
    private boolean mEnded;
    /** How long a read waits for bytes (ms; 0: until some come). */
    private volatile int mReadTimeoutMs;
    private Thread mReader;

    public synchronized boolean connect(String host, int port, boolean useTls, boolean ignoreSslCert) {
        try {
            Socket socket = new Socket();
            socket.connect(new InetSocketAddress(host, port), CONNECT_TIMEOUT_MS);
            socket.setKeepAlive(true);
            // small messages (a headset's poses, input) go out at once, never held back for an
            // acknowledgement (Nagle with the peer's delayed ACK held them up to ~40 ms)
            socket.setTcpNoDelay(true);

            if (useTls) {
                socket = createTlsSocket(socket, host, port, ignoreSslCert);
            }

            mSocket = socket;
            mInputStream = socket.getInputStream();
            mOutputStream = socket.getOutputStream();
            startReader();
            return true;
        } catch (Exception e) {
            android.util.Log.e("MakepadSocket", "connect " + host + ":" + port + " tls=" + useTls + " failed", e);
            close();
            return false;
        }
    }

    private void startReader() {
        final InputStream in = mInputStream;
        mReader = new Thread(() -> {
            byte[] chunk = new byte[CHUNK];
            try {
                while (true) {
                    int n = in.read(chunk, 0, CHUNK);
                    if (n < 0) {
                        break;
                    }
                    synchronized (mRing) {
                        int off = 0;
                        while (off < n) {
                            while (mCount == RING && !mEnded) {
                                mRing.wait();
                            }
                            if (mEnded) {
                                return;
                            }
                            int end = (mStart + mCount) % RING;
                            int len = Math.min(n - off, Math.min(RING - mCount, RING - end));
                            System.arraycopy(chunk, off, mRing, end, len);
                            mCount += len;
                            off += len;
                            mRing.notifyAll();
                        }
                    }
                }
            } catch (Exception e) {
                // closed or failed: the reads see the end once the ring is empty
            }
            synchronized (mRing) {
                mEnded = true;
                mRing.notifyAll();
            }
        }, "makepad-socket-read");
        mReader.setDaemon(true);
        mReader.start();
    }

    /**
     * Moves what the ring holds (up to dst's remaining room) into dst, waiting up to the read
     * timeout for bytes when it is empty. The count moved; 0 when the timeout passed with none;
     * -1 when the stream ended.
     */
    public int readInto(java.nio.ByteBuffer dst) {
        synchronized (mRing) {
            try {
                if (mCount == 0 && !mEnded) {
                    int timeout = mReadTimeoutMs;
                    if (timeout <= 0) {
                        while (mCount == 0 && !mEnded) {
                            mRing.wait();
                        }
                    } else {
                        long until = System.nanoTime() + timeout * 1_000_000L;
                        long left;
                        while (mCount == 0 && !mEnded && (left = until - System.nanoTime()) > 0) {
                            mRing.wait(left / 1_000_000L, (int) (left % 1_000_000L));
                        }
                    }
                }
            } catch (InterruptedException e) {
                return 0;
            }
            if (mCount == 0) {
                return mEnded ? -1 : 0;
            }
            int n = Math.min(mCount, dst.remaining());
            int first = Math.min(n, RING - mStart);
            dst.put(mRing, mStart, first);
            if (n > first) {
                dst.put(mRing, 0, n - first);
            }
            mStart = (mStart + n) % RING;
            mCount -= n;
            mRing.notifyAll();
            return n;
        }
    }

    /** DER of the server's leaf certificate after a TLS handshake, or null. */
    public synchronized byte[] peerCertificate() {
        try {
            if (!(mSocket instanceof SSLSocket)) {
                return null;
            }
            java.security.cert.Certificate[] chain = ((SSLSocket) mSocket).getSession().getPeerCertificates();
            if (chain == null || chain.length == 0) {
                return null;
            }
            return chain[0].getEncoded();
        } catch (Exception e) {
            return null;
        }
    }

    public int write(byte[] message) {
        OutputStream out = mOutputStream;
        if (out == null || message == null) {
            return -1;
        }
        synchronized (mWriteLock) {
            try {
                out.write(message, 0, message.length);
                out.flush();
                return message.length;
            } catch (Exception e) {
                return -1;
            }
        }
    }

    /** How long a read waits for bytes (the reader thread itself never times out). */
    public void setReadTimeout(int timeoutMs) {
        mReadTimeoutMs = Math.max(0, timeoutMs);
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
        synchronized (mRing) {
            mEnded = true;
            mRing.notifyAll();
        }
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
