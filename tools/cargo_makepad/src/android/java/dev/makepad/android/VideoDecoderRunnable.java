package dev.makepad.android;
import android.util.Log;

public class VideoDecoderRunnable implements Runnable {
    private final VideoDecoder mVideoDecoder;
    private final byte[] mVideoData;
    private final int mChunkSize;
    private long mStartTimestampUs;
    private long mEndTimestampUs;
    private boolean mIsInitialized = false;

    public VideoDecoderRunnable(byte[] videoData, int chunkSize, VideoDecoder videoDecoder) {
        mVideoData = videoData;
        mChunkSize = chunkSize;
        mVideoDecoder = videoDecoder;
    }

    public void setTimestamps(long startTimestampUs, long endTimestampUs) {
        mStartTimestampUs = startTimestampUs;
        mEndTimestampUs = endTimestampUs;
    }

    @Override
    public void run() {
        if(!mIsInitialized) {
            mVideoDecoder.initializeVideoDecoding(mVideoData, mChunkSize);
            mIsInitialized = true;
        } else {
            mVideoDecoder.decodeVideoChunk(mStartTimestampUs, mEndTimestampUs);
        }
    }

    public void cleanup() {
        mVideoDecoder.cleanup();
    }
}

