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
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.LinkedBlockingQueue;

import android.app.Activity;
import java.lang.ref.WeakReference;

import android.util.Log;

import dev.makepad.android.MakepadNative;

public class VideoDecoder {
    public VideoDecoder(Activity activity, long videoId, BlockingQueue<ByteBuffer> videoFrameQueue) {
        mActivityReference = new WeakReference<>(activity);
        mVideoId = videoId;
        mVideoFrameQueue = videoFrameQueue;
    }

    public void initializeVideoDecoding(byte[] video, int chunkSize) {
        mExtractor = new MediaExtractor();
        mChunkSize = chunkSize;

        try {
            Activity activity = mActivityReference.get();

            ByteArrayMediaDataSource dataSource = new ByteArrayMediaDataSource(video);
            mExtractor.setDataSource(dataSource);

            int trackIndex = selectTrack(mExtractor);
            if (trackIndex < 0) {
                if (activity != null) {
                    activity.runOnUiThread(() -> {
                        MakepadNative.onVideoDecodingError(mVideoId, "No video track found in video");
                    });
                }
                return;
            }
            mExtractor.selectTrack(trackIndex);
            MediaFormat format = mExtractor.getTrackFormat(trackIndex);

            long duration = format.getLong(MediaFormat.KEY_DURATION); 
            mFrameRate = format.containsKey(MediaFormat.KEY_FRAME_RATE) 
                ? format.getInteger(MediaFormat.KEY_FRAME_RATE) 
                : 30; 

            String mime = format.getString(MediaFormat.KEY_MIME);

            MediaCodecInfo[] codecInfos = new MediaCodecList(MediaCodecList.ALL_CODECS).getCodecInfos();
            String videoMimeType = "video/avc";  // MIME type for H.264.

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

            MediaFormat outputFormat = mCodec.getOutputFormat();
            int colorFormat = outputFormat.containsKey(MediaFormat.KEY_COLOR_FORMAT) 
                ? outputFormat.getInteger(MediaFormat.KEY_COLOR_FORMAT) 
                : -1;

            String colorFormatString = getColorFormatString(colorFormat);
            
            Log.e("Makepad", "Using Codec: " + mCodec.getName());

            mInfo = new MediaCodec.BufferInfo();
            mInputEos = false;
            mOutputEos = false;

            mVideoWidth = format.getInteger(MediaFormat.KEY_WIDTH);
            mVideoHeight = format.getInteger(MediaFormat.KEY_HEIGHT);

            if (activity != null) {
                activity.runOnUiThread(() -> {
                    MakepadNative.onVideoDecodingInitialized( 
                        mVideoId,
                        mFrameRate,
                        mVideoWidth,
                        mVideoHeight,
                        colorFormatString,
                        duration);
                });
            }
        } catch (Exception e) {
            Log.e("Makepad", "Error initializing video decoding", e);
            MakepadNative.onVideoDecodingError(mVideoId, e.getMessage());
        }
    }

    @SuppressWarnings("deprecation")
    private String getColorFormatString(int colorFormat) {
        switch (colorFormat) {
            case MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Flexible:
                mIsFlexibleFormat = true;
                return "YUV420PlanarFlexible";
            case MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Planar:
                mIsPlanar = true;
                return "YUV420Planar";
            case MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420SemiPlanar:
                mIsPlanar = false;
                return "YUV420SemiPlanar";
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

    public void decodeVideoChunk(int maxFramesToDecode) {
        try {
            synchronized (this) {
                Activity activity = mActivityReference.get();

                if (mIsDecoding) {
                    return;
                }
                mIsDecoding = true;
                if (mExtractor == null || mCodec == null) {
                    if (activity != null) {
                        activity.runOnUiThread(() -> {
                            MakepadNative.onVideoDecodingError(mVideoId, "Decoding hasn't been initialized for this video");
                        });
                    }
                }

                long framesDecodedThisChunk = 0;

                while (framesDecodedThisChunk < maxFramesToDecode  && !mInputEos) {
                    int inputBufferIndex = mCodec.dequeueInputBuffer(2000);
                    if (inputBufferIndex >= 0) {
                        ByteBuffer inputBuffer = mCodec.getInputBuffer(inputBufferIndex);
                        int sampleSize = mExtractor.readSampleData(inputBuffer, 0);

                        long presentationTimeUs = mExtractor.getSampleTime();
                        int flags = mExtractor.getSampleFlags();

                        if (sampleSize < 0) {
                            mCodec.queueInputBuffer(inputBufferIndex, 0, 0, 0, MediaCodec.BUFFER_FLAG_END_OF_STREAM);
                            mInputEos = true;
                            mExtractor.advance();
                        } else {
                            mCodec.queueInputBuffer(inputBufferIndex, 0, sampleSize, presentationTimeUs, 0);
                            mExtractor.advance();
                        }
                    }

                    int outputBufferIndex = mCodec.dequeueOutputBuffer(mInfo, 2000);
                    if (outputBufferIndex >= 0) {
                        ByteBuffer outputBuffer = mCodec.getOutputBuffer(outputBufferIndex);

                        Image outputImage = mCodec.getOutputImage(outputBufferIndex);
                        int yStride =  outputImage.getPlanes()[0].getRowStride();
                        int uStride, vStride;
                        if (mIsPlanar) {
                            uStride = outputImage.getPlanes()[1].getRowStride();
                            vStride = outputImage.getPlanes()[2].getRowStride();
                        } else {
                            uStride = vStride = outputImage.getPlanes()[1].getRowStride();
                        }

                        // Construct the ByteBuffer for the frame and metadata
                        // | Timestamp (8B)  | Y Stride (4B) | U Stride (4B) | V Stride (4B) | isEoS (1B) | Frame data length (4B) | Pixel Data |
                        int metadataSize = 25;
                        int totalSize = metadataSize + mInfo.size;

                        ByteBuffer frameBuffer = acquireBuffer(totalSize);
                        frameBuffer.clear();
                        frameBuffer.putLong(mInfo.presentationTimeUs);
                        frameBuffer.putInt(yStride);
                        frameBuffer.putInt(uStride);
                        frameBuffer.putInt(vStride);
                        frameBuffer.put((byte) (mInputEos ? 1 : 0));
                        frameBuffer.putInt(mInfo.size);

                        int oldLimit = outputBuffer.limit();
                        outputBuffer.limit(outputBuffer.position() + mInfo.size);
                        frameBuffer.put(outputBuffer);
                        outputBuffer.limit(oldLimit);

                        frameBuffer.flip();

                        mVideoFrameQueue.add(frameBuffer);

                        mCodec.releaseOutputBuffer(outputBufferIndex, false);
                        outputImage.close();

                        framesDecodedThisChunk++;
                    }
                }

                if (mInputEos) {
                    mExtractor.seekTo(0, MediaExtractor.SEEK_TO_CLOSEST_SYNC);
                    mCodec.flush();
                    mInputEos = false;
                };

                mIsDecoding = false;
                if (activity != null) {
                    activity.runOnUiThread(() -> {
                        MakepadNative.onVideoChunkDecoded(mVideoId);
                    });
                }
            }
        } catch(Exception e) {
            Log.e("Makepad", "Exception in decodeVideoChunk: " + e.getMessage());
            Log.e("Makepad", "Exception in decodeVideoChunk: " + e.getStackTrace().toString());
        }
    }

    private ByteBuffer acquireBuffer(int size) {
        synchronized(mBufferPool) {
            if (!mBufferPool.isEmpty()) {
                ByteBuffer buffer = mBufferPool.poll();
                if (buffer.capacity() == size) {
                    return buffer;
                } else {
                    return ByteBuffer.allocate(size);
                }
            } else {
                return ByteBuffer.allocate(size);
            }
        }
    }

    public void releaseBuffer(ByteBuffer buffer) {
        synchronized(mBufferPool) {
            buffer.clear();
            mBufferPool.offer(buffer);
        }
    }

    public void cleanup() {
        if (mCodec != null) {
            mCodec.stop();
            mCodec.release();
            mCodec = null;
        }
        if (mExtractor != null) {
            mExtractor.release();
            mExtractor = null;
        }
        if (mExecutor != null) {
            mExecutor.shutdown();
            mExecutor = null;
        }
        if (mVideoFrameQueue != null) {
            mVideoFrameQueue.clear();
            mVideoFrameQueue = null;
        }
        if (mBufferPool != null) {
            mBufferPool.clear();
            mBufferPool = null;
        }
        mInfo = null;
    }

    // data
    private BlockingQueue<ByteBuffer> mVideoFrameQueue;

    // buffer management
    private static final int MAX_POOL_SIZE = 10; 
    private LinkedList<ByteBuffer> mBufferPool = new LinkedList<>();

    // decoding
    private ExecutorService mExecutor = Executors.newSingleThreadExecutor(); 
    private MediaExtractor mExtractor;
    private MediaCodec mCodec;
    private MediaCodec.BufferInfo mInfo;
    private boolean mIsDecoding = false;

    // metadata
    private int mFrameRate;
    private boolean mInputEos = false;
    private boolean mOutputEos = false;
    private int mVideoWidth;
    private int mVideoHeight;
    private boolean mIsFlexibleFormat = false;
    private boolean mIsPlanar = false;
    
    // input
    private int mChunkSize;
    private byte[] mVideoData;
    private long mVideoId;

    // context
    private WeakReference<Activity> mActivityReference;
}
