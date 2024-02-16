package dev.makepad.android;

import android.util.Log;

import android.app.Activity;
import java.lang.ref.WeakReference;

import android.graphics.SurfaceTexture;
import android.view.Surface;
import android.os.Handler;
import android.os.HandlerThread;

import java.util.concurrent.atomic.AtomicInteger;
import android.media.MediaPlayer;

import java.io.FileInputStream;
import java.io.File;

import dev.makepad.android.MakepadNative;

public class VideoPlayer {
    public VideoPlayer(Activity activity, long videoId) {
        mActivityReference = new WeakReference<>(activity);
        mVideoId = videoId;
    }

    public void beginPlayback() {
        if (mMediaPlayer != null && !mMediaPlayer.isPlaying()) {
            mMediaPlayer.start();
        }
    }

    public void prepareVideoPlayback() {
        try {
            mSurfaceTexture = new SurfaceTexture(mExternalTextureHandle);

            mHandlerThread = new HandlerThread("GLHandlerThread");
            mHandlerThread.start();
            mGlHandler = new Handler(mHandlerThread.getLooper());
            mSurfaceTexture.setOnFrameAvailableListener(new SurfaceTexture.OnFrameAvailableListener() {
                @Override 
                public void onFrameAvailable(SurfaceTexture surfaceTexture) {
                    mAvailableFrames.incrementAndGet();
                }
            }, mGlHandler);

            Surface surface = new Surface(mSurfaceTexture);

            mMediaPlayer = new MediaPlayer();
            mMediaPlayer.setSurface(surface);

            if (mSource instanceof byte[]) {
                ByteArrayMediaDataSource dataSource = new ByteArrayMediaDataSource((byte[]) mSource);
                mMediaPlayer.setDataSource(dataSource);
            } else if (mSource instanceof String) {
                String dataString = (String) mSource;
                if (dataString.startsWith("http://") || dataString.startsWith("https://")) {
                    // Source is a network URL
                    mMediaPlayer.setDataSource(dataString);
                } else {
                    // Source is a url pointing to the local filesystem
                    mMediaPlayer.setDataSource(new FileInputStream(new File(dataString)).getFD());
                }
            }
            
            mMediaPlayer.setLooping(mShouldLoop);

            mMediaPlayer.prepareAsync();
            mMediaPlayer.setOnPreparedListener(new MediaPlayer.OnPreparedListener() {
                @Override
                public void onPrepared(MediaPlayer mp) {
                    Activity activity = mActivityReference.get();
                    if (activity != null) {
                        activity.runOnUiThread(() -> {
                            MakepadNative.onVideoPlaybackPrepared( 
                                mVideoId,
                                mMediaPlayer.getVideoWidth(),
                                mMediaPlayer.getVideoHeight(),
                                mMediaPlayer.getDuration(),
                                VideoPlayer.this);
                        });
                    }
                    
                    mIsPrepared = true;
                    if (mAutoplay) {
                        mp.start();
                    }
                }
            });

            mMediaPlayer.setOnCompletionListener(new MediaPlayer.OnCompletionListener() {
                @Override
                public void onCompletion(MediaPlayer mp) {
                    // Video playback has finished
                    Activity activity = mActivityReference.get();
                    if (activity != null) {
                        activity.runOnUiThread(() -> {
                            MakepadNative.onVideoPlaybackCompleted(mVideoId);
                        });
                    }
                }
            });
        } catch (Exception e) {
            String message = e.getMessage() != null? e.getMessage() : ("Error decoding video: " + e.toString());
            MakepadNative.onVideoDecodingError(mVideoId, message);
        }
    }


    public boolean maybeUpdateTexImage() {
        boolean updated = false;
        if (!mIsDecoding) {
            mIsDecoding = true;
            return false;
        }

        if (mAvailableFrames.get() > 0) {
            mSurfaceTexture.updateTexImage();

            mAvailableFrames.decrementAndGet();
            updated = true;
        }
        return updated;
    }

    public void pausePlayback() {
        if (mMediaPlayer != null && mMediaPlayer.isPlaying()) {
            mMediaPlayer.pause();
        }
    }

    public void resumePlayback() {
        if (mMediaPlayer != null && !mMediaPlayer.isPlaying()) {
            mMediaPlayer.start();
        }
    }

    public void mute() {
        if (mMediaPlayer != null) {
            mMediaPlayer.setVolume(0, 0);
        }
    }

    public void unmute() {
        if (mMediaPlayer != null) {
            mMediaPlayer.setVolume(1, 1);
        }
    }


    public void stopAndCleanup() {
        // stop and release MediaPlayer
        if (mMediaPlayer != null) {
            mMediaPlayer.stop();
            mMediaPlayer.release();
            mMediaPlayer = null;
        }

        mSource = null;

        // release the SurfaceTexture and Surface
        if (mSurfaceTexture != null) {
            mSurfaceTexture.release();
            mSurfaceTexture = null;
        }

        // stop the HandlerThread
        if (mHandlerThread != null) {
            mHandlerThread.quitSafely();
            try {
                mHandlerThread.join();
                mHandlerThread = null;
                mGlHandler = null;
            } catch (InterruptedException e) {
                e.printStackTrace();
            }
        }

        Activity activity = mActivityReference.get();
        if (activity != null) {
            activity.runOnUiThread(() -> {
                MakepadNative.onVideoPlayerReleased(mVideoId);
            });
        }
    }

    public void setExternalTextureHandle(int textureHandle) {
        mExternalTextureHandle = textureHandle;
    }

    public void setAutoplay(boolean autoplay) {
        mAutoplay = autoplay;
    }

    public void setShouldLoop(boolean shouldLoop) {
        mShouldLoop = shouldLoop;
    }

    public void setSource(Object source) {
        mSource = source;
    }

    private long mVideoId;

    // player
    private MediaPlayer mMediaPlayer;
    private boolean mIsPrepared = false; 
    private boolean mIsDecoding = false;
    private Object mSource;
    private int mExternalTextureHandle;
    private SurfaceTexture mSurfaceTexture;
    private AtomicInteger mAvailableFrames = new AtomicInteger(0);
    private Handler mGlHandler;
    private HandlerThread mHandlerThread;

    // playback
    private boolean mAutoplay = false;
    private boolean mShouldLoop = false;
    
    // context
    private WeakReference<Activity> mActivityReference;
}
