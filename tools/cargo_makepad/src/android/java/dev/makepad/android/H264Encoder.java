package dev.makepad.android;

import android.app.Activity;
import android.media.MediaCodec;
import android.media.MediaCodecInfo;
import android.media.MediaCodecList;
import android.media.MediaFormat;
import android.os.Build;
import android.os.Bundle;

import java.lang.ref.WeakReference;
import java.nio.ByteBuffer;

public class H264Encoder {
    private final long mEncoderId;
    private final WeakReference<Activity> mActivityReference;

    private MediaCodec mCodec;
    private boolean mStarted = false;
    private int mWidth = 0;
    private int mHeight = 0;

    public H264Encoder(Activity activity, long encoderId) {
        mActivityReference = new WeakReference<>(activity);
        mEncoderId = encoderId;
    }

    private static boolean codecLooksSoftware(MediaCodecInfo info) {
        if (Build.VERSION.SDK_INT >= 29) {
            if (info.isSoftwareOnly()) return true;
            if (info.isHardwareAccelerated()) return false;
        }
        String name = info.getName().toLowerCase();
        return name.startsWith("omx.google.") || name.startsWith("c2.android.") || name.contains("sw");
    }

    private static MediaCodecInfo chooseEncoder() {
        MediaCodecList list = new MediaCodecList(MediaCodecList.ALL_CODECS);
        for (MediaCodecInfo info : list.getCodecInfos()) {
            if (!info.isEncoder()) continue;
            boolean supportsAvc = false;
            for (String t : info.getSupportedTypes()) {
                if ("video/avc".equalsIgnoreCase(t)) {
                    supportsAvc = true;
                    break;
                }
            }
            if (!supportsAvc) continue;
            if (!codecLooksSoftware(info)) {
                return info;
            }
        }
        return null;
    }

    public boolean start(int width, int height, int fps, int bitrate, int keyintSeconds) {
        try {
            MediaCodecInfo info = chooseEncoder();
            if (info == null) {
                MakepadNative.onH264EncoderError(mEncoderId, "No H264 encoder codec");
                return false;
            }

            mCodec = MediaCodec.createByCodecName(info.getName());
            MediaFormat format = MediaFormat.createVideoFormat("video/avc", width, height);
            format.setInteger(MediaFormat.KEY_COLOR_FORMAT, MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible);
            format.setInteger(MediaFormat.KEY_BIT_RATE, Math.max(32_000, bitrate));
            format.setInteger(MediaFormat.KEY_FRAME_RATE, Math.max(1, fps));
            format.setInteger(MediaFormat.KEY_I_FRAME_INTERVAL, Math.max(1, keyintSeconds));
            format.setInteger(MediaFormat.KEY_MAX_INPUT_SIZE, width * height * 3 / 2);

            mCodec.configure(format, null, null, MediaCodec.CONFIGURE_FLAG_ENCODE);
            mCodec.start();
            mWidth = width;
            mHeight = height;
            mStarted = true;
            return true;
        } catch (Throwable t) {
            MakepadNative.onH264EncoderError(mEncoderId, "H264 start failed: " + t);
            stop();
            return false;
        }
    }

    private static byte[] copyByteBuffer(ByteBuffer src) {
        ByteBuffer dup = src.duplicate();
        dup.position(0);
        byte[] out = new byte[dup.remaining()];
        dup.get(out);
        return out;
    }

    private void emitFormatCsd(MediaFormat format) {
        try {
            ByteBuffer csd0 = format.getByteBuffer("csd-0");
            ByteBuffer csd1 = format.getByteBuffer("csd-1");
            byte[] b0 = csd0 != null ? copyByteBuffer(csd0) : new byte[0];
            byte[] b1 = csd1 != null ? copyByteBuffer(csd1) : new byte[0];
            if (b0.length == 0 && b1.length == 0) {
                return;
            }
            byte[] combined = new byte[b0.length + b1.length];
            System.arraycopy(b0, 0, combined, 0, b0.length);
            System.arraycopy(b1, 0, combined, b0.length, b1.length);
            MakepadNative.onH264EncoderPacket(
                mEncoderId,
                0,
                MediaCodec.BUFFER_FLAG_CODEC_CONFIG,
                combined
            );
        } catch (Throwable ignored) {}
    }

    private void drain(boolean finalDrain) {
        if (mCodec == null) return;
        MediaCodec.BufferInfo info = new MediaCodec.BufferInfo();
        while (true) {
            int outIndex = mCodec.dequeueOutputBuffer(info, finalDrain ? 10_000 : 0);
            if (outIndex == MediaCodec.INFO_TRY_AGAIN_LATER) {
                break;
            }
            if (outIndex == MediaCodec.INFO_OUTPUT_FORMAT_CHANGED) {
                emitFormatCsd(mCodec.getOutputFormat());
                continue;
            }
            if (outIndex < 0) {
                continue;
            }

            ByteBuffer out = mCodec.getOutputBuffer(outIndex);
            byte[] packet = new byte[Math.max(0, info.size)];
            if (out != null && info.size > 0) {
                out.position(info.offset);
                out.limit(info.offset + info.size);
                out.get(packet);
            }

            MakepadNative.onH264EncoderPacket(
                mEncoderId,
                info.presentationTimeUs,
                info.flags,
                packet
            );

            mCodec.releaseOutputBuffer(outIndex, false);

            if ((info.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                break;
            }
        }
    }

    private static int copyPlane(ByteBuffer src, int srcStride, ByteBuffer dst, int rowWidth, int rows) {
        if (src == null || dst == null || rowWidth <= 0 || rows <= 0 || srcStride < rowWidth) {
            return 0;
        }
        ByteBuffer srcDup = src.duplicate();
        int copied = 0;
        for (int row = 0; row < rows; row++) {
            int srcStart = row * srcStride;
            int srcEnd = srcStart + rowWidth;
            if (srcStart < 0 || srcEnd > srcDup.capacity() || dst.remaining() < rowWidth) {
                break;
            }
            srcDup.position(srcStart);
            srcDup.limit(srcEnd);
            dst.put(srcDup);
            copied += rowWidth;
        }
        return copied;
    }

    public void queueFrame(byte[] yuvI420, long ptsUs) {
        if (!mStarted || mCodec == null || yuvI420 == null) {
            return;
        }
        try {
            int inIndex = mCodec.dequeueInputBuffer(0);
            if (inIndex >= 0) {
                ByteBuffer in = mCodec.getInputBuffer(inIndex);
                if (in != null) {
                    in.clear();
                    int copy = Math.min(in.remaining(), yuvI420.length);
                    in.put(yuvI420, 0, copy);
                    mCodec.queueInputBuffer(inIndex, 0, copy, Math.max(0, ptsUs), 0);
                } else {
                    mCodec.queueInputBuffer(inIndex, 0, 0, Math.max(0, ptsUs), 0);
                }
            }
            drain(false);
        } catch (Throwable t) {
            MakepadNative.onH264EncoderError(mEncoderId, "H264 queueFrame failed: " + t);
        }
    }

    public void queueFrameI420(
        ByteBuffer yPlane,
        int yStride,
        ByteBuffer uPlane,
        int uStride,
        ByteBuffer vPlane,
        int vStride,
        long ptsUs
    ) {
        if (!mStarted || mCodec == null || yPlane == null || uPlane == null || vPlane == null) {
            return;
        }
        try {
            int inIndex = mCodec.dequeueInputBuffer(0);
            if (inIndex >= 0) {
                ByteBuffer in = mCodec.getInputBuffer(inIndex);
                if (in != null) {
                    in.clear();
                    int chromaWidth = (mWidth + 1) / 2;
                    int chromaHeight = (mHeight + 1) / 2;
                    int size = 0;
                    size += copyPlane(yPlane, yStride, in, mWidth, mHeight);
                    size += copyPlane(uPlane, uStride, in, chromaWidth, chromaHeight);
                    size += copyPlane(vPlane, vStride, in, chromaWidth, chromaHeight);
                    mCodec.queueInputBuffer(inIndex, 0, size, Math.max(0, ptsUs), 0);
                } else {
                    mCodec.queueInputBuffer(inIndex, 0, 0, Math.max(0, ptsUs), 0);
                }
            }
            drain(false);
        } catch (Throwable t) {
            MakepadNative.onH264EncoderError(mEncoderId, "H264 queueFrameI420 failed: " + t);
        }
    }

    public void requestKeyframe() {
        if (!mStarted || mCodec == null) {
            return;
        }
        try {
            Bundle params = new Bundle();
            params.putInt(MediaCodec.PARAMETER_KEY_REQUEST_SYNC_FRAME, 0);
            mCodec.setParameters(params);
        } catch (Throwable t) {
            MakepadNative.onH264EncoderError(mEncoderId, "H264 requestKeyframe failed: " + t);
        }
    }

    public void stop() {
        if (mCodec != null) {
            try {
                drain(true);
            } catch (Throwable ignored) {}
            try {
                mCodec.stop();
            } catch (Throwable ignored) {}
            try {
                mCodec.release();
            } catch (Throwable ignored) {}
            mCodec = null;
        }
        mStarted = false;
        mWidth = 0;
        mHeight = 0;
    }
}
