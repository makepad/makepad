package dev.makepad.android;

import java.nio.ByteBuffer;

public class VideoDecoderRunnable implements Runnable {
    private final VideoDecoder mVideoDecoder;
    private final byte[] mVideoData;
    private final int mChunkSize;
    private int mMaxFramesToDecode;
    private boolean mIsInitialized = false;

    public VideoDecoderRunnable(byte[] videoData, int chunkSize, VideoDecoder videoDecoder) {
        mVideoData = videoData;
        mChunkSize = chunkSize;
        mVideoDecoder = videoDecoder;
    }

    public void setMaxFramesToDecode(int maxFramesToDecode) {
        mMaxFramesToDecode = maxFramesToDecode;
    }

    @Override
    public void run() {
        if(!mIsInitialized) {
            mVideoDecoder.initializeVideoDecoding(mVideoData, mChunkSize);
            mIsInitialized = true;
        } else {
            mVideoDecoder.decodeVideoChunk(mMaxFramesToDecode);
        }
    }

    public void cleanup() {
        mVideoDecoder.cleanup();
    }

    public void releaseBuffer(ByteBuffer buffer) {
        mVideoDecoder.releaseBuffer(buffer);
    }
}

