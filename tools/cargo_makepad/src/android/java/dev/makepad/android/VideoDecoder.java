package dev.makepad.android;

import android.media.MediaCodec;
import android.media.MediaExtractor;
import android.media.MediaFormat;

import android.app.Activity;
import java.lang.ref.WeakReference;

import android.util.Log;

import dev.makepad.android.MakepadNative;

import android.graphics.SurfaceTexture;
import android.view.Surface;
import android.os.Handler;
import android.os.HandlerThread;

import java.util.concurrent.atomic.AtomicInteger;
import android.media.MediaPlayer;

import android.opengl.GLES20;
import android.opengl.GLES11Ext;

public class VideoDecoder {
    public VideoDecoder(Activity activity, long videoId) {
        mActivityReference = new WeakReference<>(activity);
        mVideoId = videoId;
    }

    public void beginPlayback() {
        if (mMediaPlayer != null && !mMediaPlayer.isPlaying()) {
            mMediaPlayer.start();
        }
    }

    public void prepareVideoPlayback(byte[] video) {
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
            
            ByteArrayMediaDataSource dataSource = new ByteArrayMediaDataSource(video);
            mMediaPlayer.setDataSource(dataSource);
            
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
                                VideoDecoder.this);
                        });
                    }
                    
                    mIsPrepared = true;
                    if (mAutoplay) {
                        mp.start();
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
            int processedFrames = mFramesProcessed.incrementAndGet();
            updated = true;

            if (mPauseFirstFrame && processedFrames > 0) {
                mMediaPlayer.pause();
                mPauseFirstFrame = false;
            }
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

    public void endPlayback() {
        mMediaPlayer.stop();
        mMediaPlayer.release();
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

    public void setPauseFirstFrame(boolean pauseFirstFrame) {
        mPauseFirstFrame = pauseFirstFrame;
    }

    private long mVideoId;

    // player
    private MediaPlayer mMediaPlayer;
    private boolean mIsPrepared = false; 
    private boolean mIsDecoding = false;
    private int mExternalTextureHandle;

    // playback
    private boolean mAutoplay = false;
    private boolean mShouldLoop = false;
    private boolean mPauseFirstFrame = false;

    private SurfaceTexture mSurfaceTexture;

    private AtomicInteger mAvailableFrames = new AtomicInteger(0);
    private AtomicInteger mFramesProcessed = new AtomicInteger(0);
    
    // context
    private WeakReference<Activity> mActivityReference;
}
