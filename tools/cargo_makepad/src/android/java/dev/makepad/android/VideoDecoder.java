package dev.makepad.android;

import android.media.MediaCodec;
import android.media.MediaCodecList;
import android.media.MediaCodecInfo;
import android.media.MediaExtractor;
import android.media.MediaFormat;
import android.media.Image;
import java.nio.ByteBuffer;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.LinkedList;
import java.util.Arrays;

import android.app.Activity;
import java.lang.ref.WeakReference;

import android.util.Log;

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

            MediaCodecInfo[] codecInfos = new MediaCodecList(MediaCodecList.ALL_CODECS).getCodecInfos();
            String videoMimeType = "video/avc";  // Example MIME type for H.264.

            String selectedCodecName = null;
            boolean isHWCodec = false;

            for (MediaCodecInfo codecInfo : codecInfos) {
                String codecName = codecInfo.getName();

                // Check if the codec is a decoder and supports our desired MIME type
                if (!codecInfo.isEncoder() && Arrays.asList(codecInfo.getSupportedTypes()).contains(videoMimeType)) {
                    // Only then proceed with checking if it's a hardware codec and has the desired color format
                    if (codecName.toLowerCase().contains("omx")) {
                        MediaCodecInfo.CodecCapabilities capabilities = codecInfo.getCapabilitiesForType(videoMimeType);
                        for (int color : capabilities.colorFormats) {
                            Log.e("Makepad", "Supported Color Format: " + color);
                            if (color == MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible) {
                                selectedCodecName = codecName;
                                isHWCodec = true;
                                break;
                            }
                        }

                        if (selectedCodecName != null) {
                            break;
                        }
                    }
                }
            }

            if (selectedCodecName != null) {
                mCodec = MediaCodec.createByCodecName(selectedCodecName);
            } else {
                mCodec = MediaCodec.createDecoderByType(mime);
            }

            mCodec.configure(format, null, null, 0);
            mCodec.start();
            
            Log.e("Makepad", "Using Codec: " + mCodec.getName());

            mInfo = new MediaCodec.BufferInfo();
            mInputEos = false;
            mOutputEos = false;

            mVideoWidth = format.getInteger(MediaFormat.KEY_WIDTH);
            mVideoHeight = format.getInteger(MediaFormat.KEY_HEIGHT);

            int colorFormat = format.containsKey(MediaFormat.KEY_COLOR_FORMAT) 
                ? format.getInteger(MediaFormat.KEY_COLOR_FORMAT) 
                : -1;

            String colorFormatString = getColorFormatString(colorFormat);

            Activity activity = mActivityReference.get();
            if (activity != null) {
                activity.runOnUiThread(() -> {
                    Makepad.onVideoDecodingInitialized(mCx, 
                        mVideoId,
                        mFrameRate,
                        mVideoWidth,
                        mVideoHeight,
                        colorFormatString,
                        duration,
                        (Makepad.Callback)mView.getContext());
                });
            }
        } catch (Exception e) {
            Log.e("Makepad", "Error initializing video decoding", e);
        }
    }

    private String getColorFormatString(int colorFormat) {
        switch (colorFormat) {
            case MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible:
                return "YUV420PlanarFlexible";
            default:
                Log.e("Makepad", "colorFormat unknown: " + colorFormat);
                return "Unknown";
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

                Image outputImage = mCodec.getOutputImage(outputBufferIndex);
                Image.Plane yPlane = outputImage.getPlanes()[0];
                int yStride = yPlane.getRowStride();
                Image.Plane uvPlane = outputImage.getPlanes()[1];
                int uvStride = uvPlane.getRowStride();

                mCodec.releaseOutputBuffer(outputBufferIndex, false);

                if ((mInfo.flags & MediaCodec.BUFFER_FLAG_END_OF_STREAM) != 0) {
                    mOutputEos = true;
                }

                Activity activity = mActivityReference.get();
                if (activity != null) {
                    activity.runOnUiThread(() -> {
                        Makepad.onVideoStream(mCx, mVideoId, pixelData, yStride, uvStride, mInfo.presentationTimeUs, mOutputEos, (Makepad.Callback)mView.getContext());
                    });
                }

                releaseBuffer(pixelData);
                framesDecodedThisChunk++;
            }
        }
    }

    private byte[] acquireBuffer(int size) {
        synchronized(mBufferPool) {
            if (!mBufferPool.isEmpty()) {
                byte[] buffer = mBufferPool.poll();
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
        synchronized(mBufferPool) {
            if (mBufferPool.size() < MAX_POOL_SIZE) {
                mBufferPool.offer(buffer);
            }
            // else let it get garbage collected
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

    // buffer management
    private static final int MAX_POOL_SIZE = 10; 
    private LinkedList<byte[]> mBufferPool = new LinkedList<>();

    // decoding
    private ExecutorService mExecutor = Executors.newSingleThreadExecutor(); 
    private MediaExtractor mExtractor;
    private MediaCodec mCodec;
    private MediaCodec.BufferInfo mInfo;

    // metadata
    private int mFrameRate;
    private boolean mInputEos = false;
    private boolean mOutputEos = false;
    private boolean mIsFlexibleFormat = false;
    private int mVideoWidth;
    private int mVideoHeight;
    
    // input
    private int mChunkSize;
    private byte[] mVideoData;
    private long mVideoId;

    // context
    private WeakReference<Activity> mActivityReference;
    MakepadSurfaceView mView;
    private long mCx;
}
