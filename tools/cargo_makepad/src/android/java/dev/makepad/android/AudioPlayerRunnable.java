package dev.makepad.android;

import android.os.Handler;
import android.os.Looper;

public class AudioPlayerRunnable implements Runnable {
    private AudioPlayer mAudioPlayer;
    private Handler mHandler;

    // Looper prepare parameters
    private String mAudioUrlOrPath;
    private boolean mIsNetwork;
    private boolean mAutoplay;
    private boolean mLoopAudio;

    public AudioPlayerRunnable(AudioPlayer audioPlayer, String audioUrlOrPath, boolean isNetwork, boolean autoplay, boolean loopAudio) {
        mAudioPlayer = audioPlayer;
        mAudioUrlOrPath = audioUrlOrPath;
        mIsNetwork = isNetwork;
        mAutoplay = autoplay;
        mLoopAudio = loopAudio;
    }

    @Override
    public void run() {
        if (Looper.myLooper() == null) {
            Looper.prepare();
        }
        // mHandler = new Handler(Looper.myLooper()); // Handler can be created here if needed for future tasks within this runnable
        mAudioPlayer.prepareAudioPlaybackInternal(mAudioUrlOrPath, mIsNetwork, mAutoplay, mLoopAudio);
        if (Looper.myLooper() != Looper.getMainLooper()) { // Don't quit if on main looper
            Looper.loop();
        }
    }

    // Methods to delegate actions to AudioPlayer, ensuring they run on this HandlerThread
    public void beginPlayback() {
        // TODO: Implement posting to handler if direct call from different thread is an issue
        mAudioPlayer.beginPlayback();
    }

    public void pausePlayback() {
        mAudioPlayer.pausePlayback();
    }

    public void stopPlayback() {
        mAudioPlayer.stopPlayback();
    }
    
    public void resumePlayback() {
        mAudioPlayer.resumePlayback();
    }

    public void seekPlayback(int timeMs) {
        mAudioPlayer.seekPlayback(timeMs);
    }

    public void setVolume(float left, float right) {
        mAudioPlayer.setVolume(left, right);
    }
    
    public void setLooping(boolean loopAudio) {
        mAudioPlayer.setLooping(loopAudio);
    }

    public void release() {
        mAudioPlayer.release();
        // After release, if this runnable's looper was prepared by this runnable, we can quit it.
        if (Looper.myLooper() != null && Looper.myLooper() != Looper.getMainLooper()) {
            Looper.myLooper().quitSafely();
        }
    }
}
