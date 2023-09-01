package dev.makepad.android;

public class VideoFrame {
    public byte[] pixelData;
    public long timestamp;
    public int yStride;
    public int uvStride;

    // Add constructor and getters/setters as needed
    public VideoFrame() {}

    // getters

    public byte[] getPixelData() {
        return pixelData;
    }
    
    public long getTimestamp() {
        return timestamp;
    }

    public int getYStride() {
        return yStride;
    }

    public int getUVStride() {
        return uvStride;
    }
}
