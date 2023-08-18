package dev.makepad.android;

import android.media.MediaCodec;
import android.media.MediaExtractor;
import android.media.MediaFormat;
import java.nio.ByteBuffer;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import android.util.Log;

import android.app.Activity;
import java.lang.ref.WeakReference;


public class VideoDecoder {
    public VideoDecoder(long cx, MakepadSurfaceView view, long videoId, Activity activity) {
        mCx = cx;
        mVideoId = videoId;
        mView = view;
        mActivityReference = new WeakReference<>(activity);
    }

    public void initializeVideoDecoding(byte[] video, int chunkSize) {
        mExtractor = new MediaExtractor();
        mChunkSize = chunkSize;

        try {
            ByteArrayMediaDataSource dataSource = new ByteArrayMediaDataSource(video);
            mExtractor.setDataSource(dataSource);

            int trackIndex = selectTrack(mExtractor);
            if (trackIndex < 0) {
                throw new RuntimeException("No video track found in video");
            }
            mExtractor.selectTrack(trackIndex);
            MediaFormat format = mExtractor.getTrackFormat(trackIndex);

            long duration = format.getLong(MediaFormat.KEY_DURATION); 
            mFrameRate = format.containsKey(MediaFormat.KEY_FRAME_RATE) 
                ? format.getInteger(MediaFormat.KEY_FRAME_RATE) 
                : 30; 

            String mime = format.getString(MediaFormat.KEY_MIME);
            mCodec = MediaCodec.createDecoderByType(mime);
            mCodec.configure(format, null, null, 0);
            mCodec.start();

            mInfo = new MediaCodec.BufferInfo();
            mInputEos = false;
            mOutputEos = false;

            int colorFormat = format.containsKey(MediaFormat.KEY_COLOR_FORMAT) 
                ? format.getInteger(MediaFormat.KEY_COLOR_FORMAT) 
                : -1;

            mVideoWidth = format.getInteger(MediaFormat.KEY_WIDTH);
            mVideoHeight = format.getInteger(MediaFormat.KEY_HEIGHT);

            Activity activity = mActivityReference.get();
            if (activity != null) {
                activity.runOnUiThread(() -> {
                    Makepad.onVideoDecodingInitialized(mCx, mVideoId, mFrameRate, mVideoWidth, mVideoHeight, colorFormat, duration, (Makepad.Callback)mView.getContext());
                });
            }
        } catch (Exception e) {
            Log.e("Makepad", "Error initializing video decoding", e);
        }
    }

    private int selectTrack(MediaExtractor extractor) {
        int numTracks = extractor.getTrackCount();
        for (int i = 0; i < numTracks; i++) {
            MediaFormat format = extractor.getTrackFormat(i);
            String mime = format.getString(MediaFormat.KEY_MIME);
            if (mime.startsWith("video/")) {
                return i;
            }
        }
        return -1;
    }

    public void decodeNextChunk() {
        if (mExtractor == null || mCodec == null) {
            throw new IllegalStateException("Decoding hasn't been initialized");
        }
        
        long framesDecodedThisChunk = 0;

        while (!mOutputEos && framesDecodedThisChunk < mChunkSize) {
            if (!mInputEos) {
                int inputBufferIndex = mCodec.dequeueInputBuffer(2000);
                if (inputBufferIndex >= 0) {
                    ByteBuffer inputBuffer = mCodec.getInputBuffer(inputBufferIndex);
                    int sampleSize = mExtractor.readSampleData(inputBuffer, 0);
                    if (sampleSize < 0) {
                        mCodec.queueInputBuffer(inputBufferIndex, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                        mInputEos = true;
                    } else {
                        long presentationTimeUs = mExtractor.getSampleTime();
                        mCodec.queueInputBuffer(inputBufferIndex, 0, sampleSize, presentationTimeUs, 0);
                        mExtractor.advance();
                    }
                }
            }

            int outputBufferIndex = mCodec.dequeueOutputBuffer(mInfo, 2000);
            if (outputBufferIndex >= 0) {
                ByteBuffer outputBuffer = mCodec.getOutputBuffer(outputBufferIndex);
                byte[] pixelData = new byte[mInfo.size]; // TODO: might need to re-use buffers / object pooling, GC seemed a bit slow at claiming these byte arrays
                outputBuffer.get(pixelData);
                mCodec.releaseOutputBuffer(outputBufferIndex, false);

                if ((mInfo.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                    mOutputEos = true;
                    this.cleanup(); // TODO might call this from rust instead
                }

                Activity activity = mActivityReference.get();
                if (activity != null) {
                    activity.runOnUiThread(() -> {
                        Makepad.onVideoStream(mCx, mVideoId, pixelData, mInfo.presentationTimeUs, mOutputEos, (Makepad.Callback)mView.getContext());
                    });
                }

                framesDecodedThisChunk++;
            }
        }
    }

    public void cleanup() {
        if (mCodec != null) {
            mCodec.stop();
            mCodec.release();
        }
        if (mExtractor != null) {
            mExtractor.release();
        }
        if (mExecutor != null) {
            mExecutor.shutdown();
        }

        mExtractor = null;
        mCodec = null;
        mInfo = null;
    }

    private ExecutorService mExecutor = Executors.newSingleThreadExecutor(); 
    private WeakReference<Activity> mActivityReference;

    private MediaExtractor mExtractor;
    private MediaCodec mCodec;
    private MediaCodec.BufferInfo mInfo;
    private int mFrameRate;
    private boolean mInputEos = false;
    private boolean mOutputEos = false;
    private int mVideoWidth;
    private int mVideoHeight;
    private int mChunkSize;
    private byte[] mVideoData;
    private long mVideoId;
    MakepadSurfaceView mView;
    private long mCx;
}
