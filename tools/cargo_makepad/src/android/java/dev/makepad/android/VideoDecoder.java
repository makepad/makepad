package dev.makepad.android;

import android.media.MediaCodec;
import android.media.MediaExtractor;
import android.media.MediaFormat;
import java.nio.ByteBuffer;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import android.util.Log;
import java.util.LinkedList;

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

    // Buffer pooling logic
    private LinkedList<byte[]> bufferPool = new LinkedList<>();
    private static final int MAX_POOL_SIZE = 10; 

    private byte[] acquireBuffer(int size) {
        synchronized(bufferPool) {
            if (!bufferPool.isEmpty()) {
                byte[] buffer = bufferPool.poll();
                if (buffer.length == size) {
                    return buffer;
                } else {
                    // Resize or just create a new buffer
                    return new byte[size];
                }
            } else {
                return new byte[size];
            }
        }
    }

    private void releaseBuffer(byte[] buffer) {
        synchronized(bufferPool) {
            if (bufferPool.size() < MAX_POOL_SIZE) {
                bufferPool.offer(buffer);
            }
            // else let it get garbage collected
        }
    }

    public void decodeVideoChunk(long startTimestampUs, long endTimestampUs) {
        if (mExtractor == null || mCodec == null) {
            throw new IllegalStateException("Decoding hasn't been initialized");
        }

        mExtractor.seekTo(startTimestampUs, MediaExtractor.SEEK_TO_CLOSEST_SYNC);

        long framesDecodedThisChunk = 0;

        while (!mOutputEos && framesDecodedThisChunk < mChunkSize) {
            if (!mInputEos) {
                int inputBufferIndex = mCodec.dequeueInputBuffer(2000);
                if (inputBufferIndex >= 0) {
                    ByteBuffer inputBuffer = mCodec.getInputBuffer(inputBufferIndex);
                    int sampleSize = mExtractor.readSampleData(inputBuffer, 0);

                    long presentationTimeUs = mExtractor.getSampleTime();
                    
                    if (sampleSize < 0 || presentationTimeUs > endTimestampUs) {
                        mCodec.queueInputBuffer(inputBufferIndex, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                        mInputEos = true;
                    } else {
                        mCodec.queueInputBuffer(inputBufferIndex, 0, sampleSize, presentationTimeUs, 0);
                        mExtractor.advance();
                    }
                }
            }

            int outputBufferIndex = mCodec.dequeueOutputBuffer(mInfo, 2000);
            if (outputBufferIndex >= 0) {
                ByteBuffer outputBuffer = mCodec.getOutputBuffer(outputBufferIndex);
                byte[] pixelData = acquireBuffer(mInfo.size);
                outputBuffer.get(pixelData);
                mCodec.releaseOutputBuffer(outputBufferIndex, false);

                if ((mInfo.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                    mOutputEos = true;
                    // this.cleanup();
                }

                Activity activity = mActivityReference.get();
                if (activity != null) {
                    activity.runOnUiThread(() -> {
                        Makepad.onVideoStream(mCx, mVideoId, pixelData, mInfo.presentationTimeUs, mOutputEos, (Makepad.Callback)mView.getContext());
                    });
                }

                releaseBuffer(pixelData);
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
