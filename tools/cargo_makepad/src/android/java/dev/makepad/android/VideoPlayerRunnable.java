package dev.makepad.android;

import java.nio.ByteBuffer;

public class VideoPlayerRunnable implements Runnable {
    private final VideoPlayer mVideoPlayer;
    private final byte[] mVideoData;
    private boolean mIsPrepared = false;

    public VideoPlayerRunnable(byte[] videoData, VideoPlayer VideoPlayer) {
        mVideoData = videoData;
        mVideoPlayer = VideoPlayer;
    }

    @Override
    public void run() {
        if(!mIsPrepared) {
            mVideoPlayer.prepareVideoPlayback(mVideoData);
            mIsPrepared = true;
        }
    }

    public void pausePlayback() {
        mVideoPlayer.pausePlayback();
    }

    public void resumePlayback() {
        mVideoPlayer.resumePlayback();
    }

    public void endPlayback() {
        mVideoPlayer.endPlayback();
    }
}

