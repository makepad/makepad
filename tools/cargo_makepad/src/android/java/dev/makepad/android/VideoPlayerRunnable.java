package dev.makepad.android;

import java.nio.ByteBuffer;

public class VideoPlayerRunnable implements Runnable {
    private VideoPlayer mVideoPlayer;
    private boolean mIsPrepared = false;

    public VideoPlayerRunnable(VideoPlayer VideoPlayer) {
        mVideoPlayer = VideoPlayer;
    }

    @Override
    public void run() {
        if(!mIsPrepared) {
            mVideoPlayer.prepareVideoPlayback();
            mIsPrepared = true;
        }
    }

    public void beginPlayback() {
        mVideoPlayer.beginPlayback();
    }

    public void pausePlayback() {
        mVideoPlayer.pausePlayback();
    }

    public void resumePlayback() {
        mVideoPlayer.resumePlayback();
    }

    public void mute() {
        mVideoPlayer.mute();
    }

    public void unmute() {
        mVideoPlayer.unmute();
    }

    public void cleanupVideoPlaybackResources() {
        mVideoPlayer.stopAndCleanup();
        mVideoPlayer = null;
    }
}
