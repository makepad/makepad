package dev.makepad.android;

import java.nio.ByteBuffer;

public class VideoDecoderRunnable implements Runnable {
    private final VideoDecoder mVideoDecoder;
    private final byte[] mVideoData;
    private boolean mIsPrepared = false;

    public VideoDecoderRunnable(byte[] videoData, VideoDecoder videoDecoder) {
        mVideoData = videoData;
        mVideoDecoder = videoDecoder;
    }

    @Override
    public void run() {
        if(!mIsPrepared) {
            mVideoDecoder.prepareVideoPlayback(mVideoData);
            mIsPrepared = true;
        }
    }

    public void pausePlayback() {
        mVideoDecoder.pausePlayback();
    }

    public void resumePlayback() {
        mVideoDecoder.resumePlayback();
    }

    public void endPlayback() {
        mVideoDecoder.endPlayback();
    }
}

