package dev.makepad.android;

import android.graphics.SurfaceTexture;
import android.os.Handler;
import android.os.HandlerThread;
import android.util.Log;
import android.view.Surface;

import java.util.concurrent.atomic.AtomicInteger;

/**
 * GL_TEXTURE_EXTERNAL_OES + SurfaceTexture + Surface for MediaCodec
 * zero-copy decode. The OES texture must already exist on Makepad's
 * GL context; this class only wraps it for decoder output.
 *
 * {@link #drainTexImage()} must be called on the GL thread that owns the
 * texture (Makepad render/poll thread).
 */
public class OesDecodeSurface {
    private static final String TAG = "MakepadOesDecode";

    private final int mOesTexId;
    private SurfaceTexture mSurfaceTexture;
    private Surface mSurface;
    private final AtomicInteger mAvailableFrames = new AtomicInteger(0);
    private final float[] mTransform = new float[] {
        1f, 0f, 0f, 0f,
        0f, 1f, 0f, 0f,
        0f, 0f, 1f, 0f,
        0f, 0f, 0f, 1f,
    };
    private HandlerThread mHandlerThread;
    private Handler mHandler;
    private boolean mReady;

    public OesDecodeSurface(int oesTextureId) {
        mOesTexId = oesTextureId;
        if (oesTextureId == 0) {
            Log.e(TAG, "OES texture id is 0");
            return;
        }
        try {
            mSurfaceTexture = new SurfaceTexture(oesTextureId);
            mHandlerThread = new HandlerThread("MakepadOesDecode");
            mHandlerThread.start();
            mHandler = new Handler(mHandlerThread.getLooper());
            mSurfaceTexture.setOnFrameAvailableListener(
                new SurfaceTexture.OnFrameAvailableListener() {
                    @Override
                    public void onFrameAvailable(SurfaceTexture surfaceTexture) {
                        mAvailableFrames.incrementAndGet();
                    }
                },
                mHandler
            );
            mSurface = new Surface(mSurfaceTexture);
            mReady = true;
        } catch (Exception e) {
            Log.e(TAG, "failed to create OES decode surface", e);
            release();
        }
    }

    public boolean isReady() {
        return mReady && mSurface != null && mSurfaceTexture != null;
    }

    public Surface getSurface() {
        return mSurface;
    }

    public int getOesTexId() {
        return mOesTexId;
    }

    /**
     * Hint the producer buffer size once video dimensions are known.
     * Safe to call multiple times.
     */
    public void setDefaultBufferSize(int width, int height) {
        if (!mReady || mSurfaceTexture == null || width <= 0 || height <= 0) {
            return;
        }
        try {
            mSurfaceTexture.setDefaultBufferSize(width, height);
        } catch (Exception e) {
            Log.e(TAG, "setDefaultBufferSize failed", e);
        }
    }

    /**
     * Drain all pending SurfaceTexture frames onto the OES texture.
     * Must run on the GL thread. Returns how many frames were applied.
     * Updates {@link #getTransformMatrix()} from the last drained frame.
     */
    public int drainTexImage() {
        if (!mReady || mSurfaceTexture == null) {
            return 0;
        }
        int drained = 0;
        while (true) {
            int remaining = mAvailableFrames.decrementAndGet();
            if (remaining < 0) {
                mAvailableFrames.incrementAndGet();
                break;
            }
            try {
                mSurfaceTexture.updateTexImage();
                mSurfaceTexture.getTransformMatrix(mTransform);
                drained++;
            } catch (Exception e) {
                Log.e(TAG, "updateTexImage failed", e);
                break;
            }
        }
        return drained;
    }

    /** Column-major 4x4 SurfaceTexture transform (valid after a successful drain). */
    public float[] getTransformMatrix() {
        return mTransform;
    }

    public void release() {
        mReady = false;
        if (mSurface != null) {
            mSurface.release();
            mSurface = null;
        }
        if (mSurfaceTexture != null) {
            mSurfaceTexture.release();
            mSurfaceTexture = null;
        }
        if (mHandlerThread != null) {
            mHandlerThread.quitSafely();
            mHandler = null;
            // Do not join on the GL/UI thread; callbacks can stall us.
            mHandlerThread = null;
        }
        mAvailableFrames.set(0);
    }
}
