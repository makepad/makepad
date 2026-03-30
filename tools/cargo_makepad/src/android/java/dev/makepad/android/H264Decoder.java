package dev.makepad.android;

import android.app.Activity;
import android.graphics.ImageFormat;
import android.graphics.SurfaceTexture;
import android.hardware.HardwareBuffer;
import android.media.Image;
import android.media.ImageReader;
import android.media.MediaCodec;
import android.media.MediaFormat;
import android.os.Build;
import android.os.Handler;
import android.os.HandlerThread;
import android.view.Surface;

import java.lang.ref.WeakReference;
import java.nio.ByteBuffer;
import java.util.ArrayDeque;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;

public class H264Decoder {
    private final long mDecoderId;
    private final WeakReference<Activity> mActivityReference;
    private final boolean mUseImageReader;

    private MediaCodec mCodec;
    private SurfaceTexture mSurfaceTexture;
    private ImageReader mImageReader;
    private Surface mSurface;
    private HandlerThread mHandlerThread;
    private Handler mDecoderHandler;
    private final AtomicInteger mAvailableFrames = new AtomicInteger(0);
    private final Object mImageLock = new Object();
    private final Object mCodecLock = new Object();
    private final ArrayDeque<QueuedPacket> mPendingPackets = new ArrayDeque<>();
    private final ArrayDeque<Integer> mAvailableInputBuffers = new ArrayDeque<>();
    private Image mLatestImage;
    /// Image last handed to native via acquireLatestHardwareBuffer; must stay open until the next
    /// acquire or shutdown so the returned HardwareBuffer stays valid (see Image#getHardwareBuffer).
    private Image mHeldDecodeImage;

    private int mExternalTextureHandle;
    private final int mWidthHint;
    private final int mHeightHint;
    private boolean mPrepared = false;
    private boolean mStarted = false;
    private int mWidth = 0;
    private int mHeight = 0;
    private int mGeneration = 0;
    private boolean mStopping = false;
    private boolean mStatusPreparedReported = false;
    private boolean mStatusConfiguredReported = false;
    private boolean mStatusOutputFormatReported = false;
    private boolean mStatusImageReported = false;

    /// Must exceed concurrent decoder output + mLatestImage + mHeldDecodeImage; too few causes
    /// ImageReader_JNI "Unable to acquire a buffer item" and can destabilize the codec surface.
    private static final int IMAGE_READER_MAX_IMAGES = 12;

    private static final class QueuedPacket {
        final byte[] data;
        final long ptsUs;
        final int flags;

        QueuedPacket(byte[] data, long ptsUs, int flags) {
            this.data = data;
            this.ptsUs = ptsUs;
            this.flags = flags;
        }
    }

    public H264Decoder(
        Activity activity,
        long decoderId,
        int widthHint,
        int heightHint,
        boolean useImageReader
    ) {
        mActivityReference = new WeakReference<>(activity);
        mDecoderId = decoderId;
        mWidthHint = Math.max(16, widthHint);
        mHeightHint = Math.max(16, heightHint);
        mUseImageReader = useImageReader;
    }

    public void setExternalTextureHandle(int textureHandle) {
        mExternalTextureHandle = textureHandle;
    }

    private void reportStatus(String status) {
        MakepadNative.onH264DecoderStatus(mDecoderId, status);
    }

    public boolean prepare() {
        try {
            if (!mUseImageReader && mExternalTextureHandle == 0) {
                MakepadNative.onH264DecoderError(mDecoderId, "Missing external texture handle");
                return false;
            }

            mHandlerThread = new HandlerThread("H264DecoderSurfaceTexture");
            mHandlerThread.start();
            mDecoderHandler = new Handler(mHandlerThread.getLooper());

            if (mUseImageReader) {
                mImageReader = ImageReader.newInstance(
                    mWidthHint,
                    mHeightHint,
                    ImageFormat.PRIVATE,
                    IMAGE_READER_MAX_IMAGES,
                    HardwareBuffer.USAGE_GPU_SAMPLED_IMAGE
                );
                mImageReader.setOnImageAvailableListener(
                    reader -> {
                        Image image = null;
                        try {
                            image = reader.acquireLatestImage();
                        } catch (Throwable t) {
                            MakepadNative.onH264DecoderError(
                                mDecoderId,
                                "H264 decoder image acquire failed: " + t
                            );
                        }
                        if (image == null) {
                            return;
                        }
                        synchronized (mImageLock) {
                            Image oldImage = mLatestImage;
                            mLatestImage = image;
                            mWidth = image.getWidth();
                            mHeight = image.getHeight();
                            if (!mStatusImageReported) {
                                mStatusImageReported = true;
                                reportStatus("imagereader image " + mWidth + "x" + mHeight);
                            }
                            if (oldImage != null) {
                                oldImage.close();
                            }
                        }
                    },
                    mDecoderHandler
                );
                mSurface = mImageReader.getSurface();
            } else {
                mSurfaceTexture = new SurfaceTexture(mExternalTextureHandle);
                mSurfaceTexture.setOnFrameAvailableListener(
                    surfaceTexture -> mAvailableFrames.incrementAndGet(),
                    mDecoderHandler
                );
                mSurface = new Surface(mSurfaceTexture);
            }

            synchronized (mCodecLock) {
                mPrepared = true;
                mStopping = false;
                mGeneration++;
            }
            if (!mStatusPreparedReported) {
                mStatusPreparedReported = true;
                reportStatus(
                    mUseImageReader
                        ? "decoder prepared (ImageReader)"
                        : "decoder prepared (SurfaceTexture)"
                );
            }
            return true;
        } catch (Throwable t) {
            MakepadNative.onH264DecoderError(mDecoderId, "H264 decoder prepare failed: " + t);
            stopAndCleanup();
            return false;
        }
    }

    public void queuePacket(byte[] data, long ptsUs, int flags) {
        if (data == null) {
            return;
        }
        final Handler handler = mDecoderHandler;
        final int generation;
        synchronized (mCodecLock) {
            if (!mPrepared || mStopping) {
                return;
            }
            generation = mGeneration;
        }
        if (handler == null) {
            MakepadNative.onH264DecoderError(
                mDecoderId,
                "H264 decoder queue failed: decoder handler unavailable"
            );
            return;
        }
        final byte[] packetData = data.clone();
        handler.post(() -> queuePacketOnDecoderThread(packetData, ptsUs, flags, generation));
    }

    private void queuePacketOnDecoderThread(byte[] data, long ptsUs, int flags, int generation) {
        synchronized (mCodecLock) {
            if (generation != mGeneration || mStopping || !mPrepared) {
                return;
            }
            try {
                boolean isConfig = (flags & MediaCodec.BUFFER_FLAG_CODEC_CONFIG) != 0;
                if (isConfig && !mStarted) {
                    if (!configureCodecFromAnnexB(data, generation)) {
                        MakepadNative.onH264DecoderError(mDecoderId, "Missing SPS/PPS in H264 config");
                    }
                    return;
                }
                if (!mStarted || mCodec == null) {
                    return;
                }
                mPendingPackets.addLast(new QueuedPacket(data, ptsUs, flags));
                pumpInputQueueLocked(generation);
            } catch (Throwable t) {
                MakepadNative.onH264DecoderError(mDecoderId, "H264 decoder queue failed: " + t);
            }
        }
    }

    private boolean configureCodecFromAnnexB(byte[] data, int generation) {
        try {
            List<byte[]> nals = splitAnnexBNals(data);
            byte[] sps = null;
            byte[] pps = null;
            for (byte[] nal : nals) {
                if (nal.length == 0) {
                    continue;
                }
                int nalType = nal[0] & 0x1F;
                if (nalType == 7 && sps == null) {
                    sps = nal;
                } else if (nalType == 8 && pps == null) {
                    pps = nal;
                }
            }
            if (sps == null || pps == null) {
                return false;
            }

            mCodec = MediaCodec.createDecoderByType("video/avc");
            mCodec.setCallback(
                new MediaCodec.Callback() {
                    @Override
                    public void onInputBufferAvailable(MediaCodec codec, int index) {
                        synchronized (mCodecLock) {
                            if (
                                generation != mGeneration
                                || mStopping
                                || !mStarted
                                || codec != mCodec
                            ) {
                                return;
                            }
                            mAvailableInputBuffers.addLast(index);
                            pumpInputQueueLocked(generation);
                        }
                    }

                    @Override
                    public void onOutputBufferAvailable(
                        MediaCodec codec,
                        int index,
                        MediaCodec.BufferInfo info
                    ) {
                        synchronized (mCodecLock) {
                            if (generation != mGeneration || codec != mCodec) {
                                return;
                            }
                            try {
                                codec.releaseOutputBuffer(index, true);
                            } catch (Throwable t) {
                                MakepadNative.onH264DecoderError(
                                    mDecoderId,
                                    "H264 decoder output release failed: " + t
                                );
                            }
                        }
                    }

                    @Override
                    public void onOutputFormatChanged(MediaCodec codec, MediaFormat format) {
                        synchronized (mCodecLock) {
                            if (generation != mGeneration || codec != mCodec) {
                                return;
                            }
                            if (format.containsKey(MediaFormat.KEY_WIDTH)) {
                                mWidth = format.getInteger(MediaFormat.KEY_WIDTH);
                            }
                            if (format.containsKey(MediaFormat.KEY_HEIGHT)) {
                                mHeight = format.getInteger(MediaFormat.KEY_HEIGHT);
                            }
                            if (!mStatusOutputFormatReported) {
                                mStatusOutputFormatReported = true;
                                reportStatus("output format " + mWidth + "x" + mHeight);
                            }
                        }
                    }

                    @Override
                    public void onError(MediaCodec codec, MediaCodec.CodecException error) {
                        MakepadNative.onH264DecoderError(
                            mDecoderId,
                            "H264 decoder codec callback error: " + error
                        );
                    }
                },
                mDecoderHandler
            );
            MediaFormat format = MediaFormat.createVideoFormat("video/avc", mWidthHint, mHeightHint);
            format.setByteBuffer("csd-0", ByteBuffer.wrap(withStartCode(sps)));
            format.setByteBuffer("csd-1", ByteBuffer.wrap(withStartCode(pps)));
            if (Build.VERSION.SDK_INT >= 30) {
                format.setInteger(MediaFormat.KEY_LOW_LATENCY, 1);
            } else {
                format.setInteger("low-latency", 1);
            }
            if (mUseImageReader && Build.VERSION.SDK_INT >= 31) {
                format.setInteger(MediaFormat.KEY_ALLOW_FRAME_DROP, 0);
            }
            mCodec.configure(format, mSurface, null, 0);
            mCodec.start();
            mStarted = true;
            if (!mStatusConfiguredReported) {
                mStatusConfiguredReported = true;
                reportStatus(
                    "codec configured from annexb "
                        + mWidthHint
                        + "x"
                        + mHeightHint
                        + " sps="
                        + sps.length
                        + " pps="
                        + pps.length
                );
            }
            return true;
        } catch (Throwable t) {
            MakepadNative.onH264DecoderError(mDecoderId, "H264 decoder configure failed: " + t);
            stopAndCleanup();
            return false;
        }
    }

    private static List<byte[]> splitAnnexBNals(byte[] data) {
        ArrayList<byte[]> out = new ArrayList<>();
        int i = 0;
        while (i < data.length) {
            int start = findStartCode(data, i);
            if (start < 0) {
                break;
            }
            int prefix = startCodeLength(data, start);
            int nalStart = start + prefix;
            int next = findStartCode(data, nalStart);
            int nalEnd = next >= 0 ? next : data.length;
            int nalLen = nalEnd - nalStart;
            if (nalLen > 0) {
                byte[] nal = new byte[nalLen];
                System.arraycopy(data, nalStart, nal, 0, nalLen);
                out.add(nal);
            }
            i = nalEnd;
        }
        return out;
    }

    private static int findStartCode(byte[] data, int from) {
        for (int i = Math.max(0, from); i <= data.length - 3; i++) {
            if (data[i] == 0 && data[i + 1] == 0) {
                if (data[i + 2] == 1) {
                    return i;
                }
                if (i + 3 < data.length && data[i + 2] == 0 && data[i + 3] == 1) {
                    return i;
                }
            }
        }
        return -1;
    }

    private static int startCodeLength(byte[] data, int index) {
        if (
            index + 3 < data.length
            && data[index] == 0
            && data[index + 1] == 0
            && data[index + 2] == 0
            && data[index + 3] == 1
        ) {
            return 4;
        }
        return 3;
    }

    private static byte[] withStartCode(byte[] nal) {
        byte[] out = new byte[nal.length + 4];
        out[0] = 0;
        out[1] = 0;
        out[2] = 0;
        out[3] = 1;
        System.arraycopy(nal, 0, out, 4, nal.length);
        return out;
    }

    private void pumpInputQueueLocked(int generation) {
        if (generation != mGeneration || mStopping || !mStarted || mCodec == null) {
            return;
        }
        while (!mPendingPackets.isEmpty() && !mAvailableInputBuffers.isEmpty()) {
            int inIndex = mAvailableInputBuffers.removeFirst();
            QueuedPacket packet = mPendingPackets.removeFirst();
            try {
                ByteBuffer input = mCodec.getInputBuffer(inIndex);
                if (input == null) {
                    mCodec.queueInputBuffer(
                        inIndex,
                        0,
                        0,
                        Math.max(0, packet.ptsUs),
                        packet.flags
                    );
                    continue;
                }
                input.clear();
                if (packet.data.length > input.remaining()) {
                    MakepadNative.onH264DecoderError(
                        mDecoderId,
                        "Input packet too large for codec buffer: " + packet.data.length
                    );
                    continue;
                }
                input.put(packet.data, 0, packet.data.length);
                mCodec.queueInputBuffer(
                    inIndex,
                    0,
                    packet.data.length,
                    Math.max(0, packet.ptsUs),
                    packet.flags
                );
            } catch (Throwable t) {
                MakepadNative.onH264DecoderError(mDecoderId, "H264 decoder input queue failed: " + t);
                return;
            }
        }
    }

    public boolean maybeUpdateTexImage() {
        if (mUseImageReader || !mStarted || mSurfaceTexture == null) {
            return false;
        }
        if (mAvailableFrames.get() <= 0) {
            return false;
        }
        mSurfaceTexture.updateTexImage();
        mAvailableFrames.decrementAndGet();
        return true;
    }

    public int getWidth() {
        return mWidth;
    }

    public int getHeight() {
        return mHeight;
    }

    public HardwareBuffer acquireLatestHardwareBuffer() {
        if (!mUseImageReader) {
            return null;
        }
        synchronized (mImageLock) {
            if (mLatestImage == null) {
                return null;
            }
            if (mHeldDecodeImage != null) {
                try {
                    mHeldDecodeImage.close();
                } catch (Throwable ignored) {}
                mHeldDecodeImage = null;
            }
            Image image = mLatestImage;
            mLatestImage = null;
            mWidth = image.getWidth();
            mHeight = image.getHeight();
            HardwareBuffer buffer = image.getHardwareBuffer();
            if (buffer == null) {
                MakepadNative.onH264DecoderError(
                    mDecoderId,
                    "ImageReader image returned null HardwareBuffer at "
                        + mWidth
                        + "x"
                        + mHeight
                );
                try {
                    image.close();
                } catch (Throwable ignored) {}
                return null;
            }
            mHeldDecodeImage = image;
            return buffer;
        }
    }

    public void stopAndCleanup() {
        synchronized (mCodecLock) {
            mStopping = true;
            mPrepared = false;
            mGeneration++;
            mPendingPackets.clear();
            mAvailableInputBuffers.clear();
            if (mDecoderHandler != null) {
                try {
                    mDecoderHandler.removeCallbacksAndMessages(null);
                } catch (Throwable ignored) {}
            }

            synchronized (mImageLock) {
                if (mHeldDecodeImage != null) {
                    try {
                        mHeldDecodeImage.close();
                    } catch (Throwable ignored) {}
                    mHeldDecodeImage = null;
                }
                if (mLatestImage != null) {
                    try {
                        mLatestImage.close();
                    } catch (Throwable ignored) {}
                    mLatestImage = null;
                }
            }

            if (mCodec != null) {
                try {
                    mCodec.stop();
                } catch (Throwable ignored) {}
                try {
                    mCodec.release();
                } catch (Throwable ignored) {}
                mCodec = null;
            }

            if (mSurface != null) {
                try {
                    mSurface.release();
                } catch (Throwable ignored) {}
                mSurface = null;
            }

            if (mImageReader != null) {
                try {
                    mImageReader.close();
                } catch (Throwable ignored) {}
                mImageReader = null;
            }

            if (mSurfaceTexture != null) {
                try {
                    mSurfaceTexture.release();
                } catch (Throwable ignored) {}
                mSurfaceTexture = null;
            }

            HandlerThread thread = mHandlerThread;
            if (thread != null) {
                thread.quitSafely();
                if (Thread.currentThread() != thread) {
                    try {
                        thread.join();
                    } catch (InterruptedException ignored) {}
                }
                mHandlerThread = null;
                mDecoderHandler = null;
            }

            mStarted = false;
            mWidth = 0;
            mHeight = 0;
            mStatusPreparedReported = false;
            mStatusConfiguredReported = false;
            mStatusOutputFormatReported = false;
            mStatusImageReported = false;
            mStopping = false;
        }
    }
}
