package dev.makepad.android;
import android.util.Log;

public class VideoDecoderRunnable implements Runnable {
    private final VideoDecoder mVideoDecoder;
    private final byte[] mVideoData;
    private final int mChunkSize;
    private boolean mIsInitialized = false;

    public VideoDecoderRunnable(byte[] videoData, int chunkSize, VideoDecoder videoDecoder) {
        mVideoData = videoData;
        mChunkSize = chunkSize;
        mVideoDecoder = videoDecoder;
    }

    @Override
    public void run() {
        if(!mIsInitialized) {
            mVideoDecoder.initializeVideoDecoding(mVideoData, mChunkSize);
            mIsInitialized = true;
        } else {
            mVideoDecoder.decodeNextChunk();
        }
    }

    public void cleanup() {
        mVideoDecoder.cleanup();
    }
}
