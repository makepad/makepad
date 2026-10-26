package dev.makepad.android;

import java.math.BigInteger;
import java.nio.ByteBuffer;
import java.security.AlgorithmParameters;
import java.security.KeyFactory;
import java.security.KeyPair;
import java.security.KeyPairGenerator;
import java.security.KeyStore;
import java.security.PrivateKey;
import java.security.PublicKey;
import java.security.Signature;
import java.security.cert.Certificate;
import java.security.cert.CertificateFactory;
import java.security.interfaces.ECPrivateKey;
import java.security.interfaces.ECPublicKey;
import java.security.spec.ECGenParameterSpec;
import java.security.spec.ECParameterSpec;
import java.security.spec.ECPoint;
import java.security.spec.ECPrivateKeySpec;
import java.security.spec.ECPublicKeySpec;
import javax.crypto.KeyAgreement;
import javax.net.ssl.KeyManagerFactory;
import javax.net.ssl.SSLContext;
import javax.net.ssl.SSLEngine;
import javax.net.ssl.SSLEngineResult;
import javax.net.ssl.SSLSession;

/**
 * Fleet identity on Android (makepad_network::tls through the JIT host): P-256 keys, ECDSA,
 * ECDH and a TLS server session from the platform's security providers. Native code has no
 * libcrypto/libssl of its own. Keys are X9.63 (04 || X || Y || d, 97 bytes), points 65 bytes.
 * The host calls the static methods; a session is driven by unwrap/wrap/close.
 */
public final class FleetTls {
    private static ECParameterSpec params() throws Exception {
        AlgorithmParameters p = AlgorithmParameters.getInstance("EC");
        p.init(new ECGenParameterSpec("secp256r1"));
        return p.getParameterSpec(ECParameterSpec.class);
    }

    private static byte[] fixed32(BigInteger v) {
        byte[] b = v.toByteArray();
        byte[] out = new byte[32];
        if (b.length >= 32) {
            System.arraycopy(b, b.length - 32, out, 0, 32);
        } else {
            System.arraycopy(b, 0, out, 32 - b.length, b.length);
        }
        return out;
    }

    private static PrivateKey privateKey(byte[] key) throws Exception {
        BigInteger d = new BigInteger(1, java.util.Arrays.copyOfRange(key, 65, 97));
        return KeyFactory.getInstance("EC").generatePrivate(new ECPrivateKeySpec(d, params()));
    }

    private static PublicKey publicKey(byte[] point) throws Exception {
        BigInteger x = new BigInteger(1, java.util.Arrays.copyOfRange(point, 1, 33));
        BigInteger y = new BigInteger(1, java.util.Arrays.copyOfRange(point, 33, 65));
        return KeyFactory.getInstance("EC").generatePublic(new ECPublicKeySpec(new ECPoint(x, y), params()));
    }

    public static byte[] generateP256() throws Exception {
        KeyPairGenerator g = KeyPairGenerator.getInstance("EC");
        g.initialize(new ECGenParameterSpec("secp256r1"));
        KeyPair kp = g.generateKeyPair();
        ECPoint w = ((ECPublicKey) kp.getPublic()).getW();
        byte[] out = new byte[97];
        out[0] = 4;
        System.arraycopy(fixed32(w.getAffineX()), 0, out, 1, 32);
        System.arraycopy(fixed32(w.getAffineY()), 0, out, 33, 32);
        System.arraycopy(fixed32(((ECPrivateKey) kp.getPrivate()).getS()), 0, out, 65, 32);
        return out;
    }

    public static byte[] sign(byte[] key, byte[] msg) throws Exception {
        Signature s = Signature.getInstance("SHA256withECDSA");
        s.initSign(privateKey(key));
        s.update(msg);
        return s.sign();
    }

    public static boolean verify(byte[] point, byte[] msg, byte[] sig) {
        try {
            Signature s = Signature.getInstance("SHA256withECDSA");
            s.initVerify(publicKey(point));
            s.update(msg);
            return s.verify(sig);
        } catch (Exception e) {
            return false;
        }
    }

    public static byte[] ecdh(byte[] key, byte[] point) throws Exception {
        KeyAgreement a = KeyAgreement.getInstance("ECDH");
        a.init(privateKey(key));
        a.doPhase(publicKey(point), true);
        return a.generateSecret();
    }

    /** The battery from BatteryManager's sticky broadcast: level 0..100, bit 8 = charging; -1 unknown. */
    public static int battery(android.content.Context ctx) {
        android.content.Intent i = ctx.registerReceiver(null, new android.content.IntentFilter(android.content.Intent.ACTION_BATTERY_CHANGED));
        if (i == null) return -1;
        int level = i.getIntExtra(android.os.BatteryManager.EXTRA_LEVEL, -1);
        int scale = i.getIntExtra(android.os.BatteryManager.EXTRA_SCALE, -1);
        if (level < 0 || scale <= 0) return -1;
        int status = i.getIntExtra(android.os.BatteryManager.EXTRA_STATUS, -1);
        boolean charging = status == android.os.BatteryManager.BATTERY_STATUS_CHARGING || status == android.os.BatteryManager.BATTERY_STATUS_FULL;
        return (level * 100 / scale) | (charging ? 0x100 : 0);
    }

    /** An SSLContext serving this certificate and key (no client certificates). */
    public static Object serverContext(byte[] certDer, byte[] key) throws Exception {
        Certificate cert = CertificateFactory.getInstance("X.509").generateCertificate(new java.io.ByteArrayInputStream(certDer));
        KeyStore ks = KeyStore.getInstance(KeyStore.getDefaultType());
        ks.load(null, null);
        char[] pw = new char[0];
        ks.setKeyEntry("fleet", privateKey(key), pw, new Certificate[] { cert });
        KeyManagerFactory kmf = KeyManagerFactory.getInstance(KeyManagerFactory.getDefaultAlgorithm());
        kmf.init(ks, pw);
        SSLContext ctx = SSLContext.getInstance("TLS");
        ctx.init(kmf.getKeyManagers(), null, null);
        return ctx;
    }

    public static FleetTls session(Object ctx) {
        return new FleetTls(((SSLContext) ctx).createSSLEngine());
    }

    private final SSLEngine engine;
    private ByteBuffer netIn;
    private ByteBuffer appIn;
    private ByteBuffer netOut;
    private boolean closed;

    private FleetTls(SSLEngine engine) {
        this.engine = engine;
        engine.setUseClientMode(false);
        engine.setNeedClientAuth(false);
        SSLSession s = engine.getSession();
        netIn = ByteBuffer.allocate(s.getPacketBufferSize());
        appIn = ByteBuffer.allocate(s.getApplicationBufferSize());
        netOut = ByteBuffer.allocate(s.getPacketBufferSize());
    }

    private static ByteBuffer grow(ByteBuffer b, int more) {
        ByteBuffer n = ByteBuffer.allocate(b.capacity() + Math.max(more, b.capacity()));
        b.flip();
        n.put(b);
        return n;
    }

    private void wrapOnce(ByteBuffer src) throws Exception {
        while (true) {
            SSLEngineResult r = engine.wrap(src, netOut);
            if (r.getStatus() == SSLEngineResult.Status.BUFFER_OVERFLOW) {
                netOut = grow(netOut, engine.getSession().getPacketBufferSize());
                continue;
            }
            if (r.getStatus() == SSLEngineResult.Status.CLOSED) closed = true;
            return;
        }
    }

    /** Feeds ciphertext; runs the handshake as far as it goes. Plaintext is taken with takeApp,
     * ciphertext to send with takeNet. */
    public void feed(byte[] data) throws Exception {
        if (netIn.remaining() < data.length) netIn = grow(netIn, data.length);
        netIn.put(data);
        pump();
    }

    /** Starts (or continues) the handshake with no new input. */
    public void pump() throws Exception {
        ByteBuffer empty = ByteBuffer.allocate(0);
        while (true) {
            SSLEngineResult.HandshakeStatus hs = engine.getHandshakeStatus();
            if (hs == SSLEngineResult.HandshakeStatus.NEED_TASK) {
                Runnable t;
                while ((t = engine.getDelegatedTask()) != null) t.run();
                continue;
            }
            if (hs == SSLEngineResult.HandshakeStatus.NEED_WRAP) {
                wrapOnce(empty);
                if (closed) return;
                continue;
            }
            // NEED_UNWRAP, or the handshake is done: decrypt what came in
            netIn.flip();
            SSLEngineResult r;
            try {
                r = engine.unwrap(netIn, appIn);
            } finally {
                netIn.compact();
            }
            SSLEngineResult.Status st = r.getStatus();
            if (st == SSLEngineResult.Status.BUFFER_OVERFLOW) {
                appIn = grow(appIn, engine.getSession().getApplicationBufferSize());
                continue;
            }
            if (st == SSLEngineResult.Status.CLOSED) {
                closed = true;
                return;
            }
            if (st == SSLEngineResult.Status.BUFFER_UNDERFLOW) return;
            if (r.bytesConsumed() == 0 && r.bytesProduced() == 0 && engine.getHandshakeStatus() != SSLEngineResult.HandshakeStatus.NEED_TASK
                    && engine.getHandshakeStatus() != SSLEngineResult.HandshakeStatus.NEED_WRAP) return;
        }
    }

    public boolean handshaking() {
        SSLEngineResult.HandshakeStatus hs = engine.getHandshakeStatus();
        return hs != SSLEngineResult.HandshakeStatus.NOT_HANDSHAKING && hs != SSLEngineResult.HandshakeStatus.FINISHED;
    }

    public boolean closed() {
        return closed;
    }

    public byte[] takeApp() {
        appIn.flip();
        byte[] out = new byte[appIn.remaining()];
        appIn.get(out);
        appIn.clear();
        return out;
    }

    public byte[] takeNet() {
        netOut.flip();
        byte[] out = new byte[netOut.remaining()];
        netOut.get(out);
        netOut.clear();
        return out;
    }

    public byte[] wrap(byte[] app) throws Exception {
        ByteBuffer src = ByteBuffer.wrap(app);
        while (src.hasRemaining() && !closed) wrapOnce(src);
        return takeNet();
    }

    public byte[] close() {
        try {
            engine.closeOutbound();
            wrapOnce(ByteBuffer.allocate(0));
        } catch (Exception e) {
            // the peer is gone: nothing to say
        }
        return takeNet();
    }
}
