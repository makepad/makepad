package dev.makepad.android;

import android.content.Context;
import android.media.MediaPlayer;
import android.net.Uri;
import android.util.Log;

import java.io.IOException;
import java.lang.ref.WeakReference;

public class AudioPlayer implements MediaPlayer.OnPreparedListener, MediaPlayer.OnCompletionListener, MediaPlayer.OnErrorListener {

    private static final String TAG = "AudioPlayer";

    private WeakReference<MakepadActivity> mActivityReference;
    private long mPlayerId;
    private MediaPlayer mMediaPlayer;
    private boolean mIsPrepared = false;
    private boolean mAutoplay = false; // Store autoplay state for onPrepared

    public AudioPlayer(MakepadActivity activity, long playerId) {
        mActivityReference = new WeakReference<>(activity);
        mPlayerId = playerId;
    }

    public void prepareAudioPlaybackInternal(String audioUrlOrPath, boolean isNetwork, boolean autoPlay, boolean loopAudio) {
        mAutoplay = autoPlay; // Store for use in onPrepared
        try {
            mMediaPlayer = new MediaPlayer();
            mMediaPlayer.setOnPreparedListener(this);
            mMediaPlayer.setOnCompletionListener(this);
            mMediaPlayer.setOnErrorListener(this);

            MakepadActivity activity = mActivityReference.get();
            if (activity == null) {
                Log.e(TAG, "Activity context is null, cannot prepare audio.");
                // Consider a callback to Rust about this error
                return;
            }

            if (isNetwork) {
                mMediaPlayer.setDataSource(audioUrlOrPath);
            } else {
                // For local files, it's safer to use Uri.parse if it's a proper file path
                // If it's an asset or resource, different handling would be needed,
                // but the API implies a file path here.
                Uri uri = Uri.parse(audioUrlOrPath);
                mMediaPlayer.setDataSource(activity.getApplicationContext(), uri);
            }
            
            mMediaPlayer.setLooping(loopAudio);
            mMediaPlayer.prepareAsync();
        } catch (IOException e) {
            Log.e(TAG, "Error setting data source or preparing MediaPlayer: " + e.getMessage());
            // Notify Rust about the error
            MakepadNative.onAudioPlaybackError(mPlayerId, "Error preparing: " + e.getMessage());
        } catch (IllegalArgumentException e) {
            Log.e(TAG, "Error setting data source (invalid URI or path): " + e.getMessage());
            MakepadNative.onAudioPlaybackError(mPlayerId, "Invalid URI or path: " + e.getMessage());
        } catch (SecurityException e) {
            Log.e(TAG, "Error setting data source (permission issue): " + e.getMessage());
            MakepadNative.onAudioPlaybackError(mPlayerId, "Permission issue: " + e.getMessage());
        }
    }

    @Override
    public void onPrepared(MediaPlayer mp) {
        mIsPrepared = true;
        int durationMs = mp.getDuration();

        // For simplicity, assume all are true. Platform specifics might change this.
        boolean canSeek = true; 
        boolean canPause = true;
        boolean canSetVolume = true;

        MakepadNative.onAudioPlaybackPrepared(mPlayerId, durationMs, canSeek, canPause, canSetVolume);

        if (mAutoplay) {
            mp.start();
            MakepadNative.onAudioPlaybackStarted(mPlayerId);
        }
    }

    @Override
    public void onCompletion(MediaPlayer mp) {
        MakepadNative.onAudioPlaybackCompleted(mPlayerId);
        // If not looping, we might want to reset mIsPrepared or player state here
        // if (mMediaPlayer != null && !mMediaPlayer.isLooping()) { }
    }

    @Override
    public boolean onError(MediaPlayer mp, int what, int extra) {
        Log.e(TAG, "MediaPlayer error: what=" + what + ", extra=" + extra);
        String errorMsg = "MediaPlayer error (what:" + what + ", extra:" + extra + ")";
        MakepadNative.onAudioPlaybackError(mPlayerId, errorMsg);
        release(); // Release resources on error
        return true; // Indicates the error has been handled
    }

    public void beginPlayback() {
        if (mMediaPlayer != null && mIsPrepared && !mMediaPlayer.isPlaying()) {
            mMediaPlayer.start();
            MakepadNative.onAudioPlaybackStarted(mPlayerId);
        } else if (mMediaPlayer == null || !mIsPrepared) {
            Log.w(TAG, "beginPlayback called before MediaPlayer is prepared or null.");
            // Optionally, queue this call or notify Rust about the invalid state
        }
    }

    public void pausePlayback() {
        if (mMediaPlayer != null && mMediaPlayer.isPlaying()) {
            mMediaPlayer.pause();
            MakepadNative.onAudioPlaybackPaused(mPlayerId);
        }
    }

    public void stopPlayback() {
        if (mMediaPlayer != null && mIsPrepared) {
            if (mMediaPlayer.isPlaying()) {
                 mMediaPlayer.stop();
            }
            // After stop, MediaPlayer needs to be prepared again to restart.
            // We could call prepareAsync() here or require explicit re-preparation from Rust.
            // For now, just stop and let Rust decide next steps.
            mIsPrepared = false; // Mark as not prepared after stop
            MakepadNative.onAudioPlaybackStopped(mPlayerId);
            // mMediaPlayer.prepareAsync(); // Or handle re-prepare elsewhere
        }
    }
    
    public void resumePlayback() {
        // Alias for beginPlayback as MediaPlayer's start() handles both starting and resuming.
        beginPlayback();
    }

    public void seekPlayback(int timeMs) {
        if (mMediaPlayer != null && mIsPrepared) {
            // MediaPlayer's seekTo operates on milliseconds
            mMediaPlayer.seekTo(timeMs); 
            // TODO: Consider adding a MakepadNative.onAudioPlaybackSeeked(mPlayerId, timeMs);
        }
    }

    public void setVolume(float leftVolume, float rightVolume) {
        if (mMediaPlayer != null && mIsPrepared) {
            mMediaPlayer.setVolume(leftVolume, rightVolume);
            // TODO: Consider adding a MakepadNative.onAudioVolumeChanged(mPlayerId, leftVolume, rightVolume);
        }
    }

    public void setLooping(boolean loopAudio) {
        if (mMediaPlayer != null) {
            mMediaPlayer.setLooping(loopAudio);
        }
    }

    public void release() {
        if (mMediaPlayer != null) {
            if (mMediaPlayer.isPlaying()) {
                mMediaPlayer.stop();
            }
            mMediaPlayer.release();
            mMediaPlayer = null;
            mIsPrepared = false;
            MakepadNative.onAudioPlaybackReleased(mPlayerId);
        }
    }
}
