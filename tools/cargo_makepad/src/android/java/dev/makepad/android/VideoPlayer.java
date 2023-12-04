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

            HandlerThread handlerThread = new HandlerThread("GLHandlerThread");
            handlerThread.start();
            Handler glHandler = new Handler(handlerThread.getLooper());
            mSurfaceTexture.setOnFrameAvailableListener(new SurfaceTexture.OnFrameAvailableListener() {
                @Override 
                public void onFrameAvailable(SurfaceTexture surfaceTexture) {
                    mAvailableFrames.incrementAndGet();
                }
            }, glHandler);

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

    public void stopAndCleanup() {
        mMediaPlayer.stop();
        mMediaPlayer.release();
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

    // playback
    private boolean mAutoplay = false;
    private boolean mShouldLoop = false;
    
    // context
    private WeakReference<Activity> mActivityReference;
}
