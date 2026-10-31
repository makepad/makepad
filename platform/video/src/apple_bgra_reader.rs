//! A file's pictures decoded as BGRA, alpha included (an HEVC-with-alpha
//! file's alpha layer): what a check of a transparent export reads. The
//! decoder facade's frames are NV12, which has no alpha.

use {
    crate::apple_decoder::{cmtime_to_100ns, nserror_to_video_error, AutoreleasePool},
    crate::VideoFileError,
    makepad_apple_sys::*,
    std::ffi::c_void,
};

/// The video track of a file, read frame by frame as tight BGRA
/// (`width * 4` bytes a row), as the OS decoder gives it.
pub struct BgraFrameReader {
    reader: ObjcId,
    output: ObjcId,
}

unsafe impl Send for BgraFrameReader {}

impl BgraFrameReader {
    pub fn open(path: &str) -> Result<Self, VideoFileError> {
        if !std::path::Path::new(path).is_file() {
            return Err(VideoFileError::new(format!("file not found: {path}")));
        }
        let _pool = AutoreleasePool::new();
        unsafe {
            let url: ObjcId = msg_send![class!(NSURL), fileURLWithPath: str_to_nsstring(path)];
            let asset: ObjcId = msg_send![class!(AVURLAsset), URLAssetWithURL: url options: nil];
            if asset == nil {
                return Err(VideoFileError::new(format!("AVURLAsset failed to open {path}")));
            }
            let tracks: ObjcId = msg_send![asset, tracksWithMediaType: AVMediaTypeVideo];
            let count: usize = msg_send![tracks, count];
            if count == 0 {
                return Err(VideoFileError::new("no video stream in file"));
            }
            let track: ObjcId = msg_send![tracks, objectAtIndex: 0usize];
            let mut error: ObjcId = nil;
            let reader: ObjcId = msg_send![class!(AVAssetReader), assetReaderWithAsset: asset error: &mut error];
            if reader == nil {
                return Err(nserror_to_video_error("AVAssetReader init", error));
            }
            let bgra: ObjcId = msg_send![class!(NSNumber), numberWithUnsignedInt: kCVPixelFormatType_32BGRA];
            let keys: &[ObjcId] = &[kCVPixelBufferPixelFormatTypeKey as ObjcId];
            let values: &[ObjcId] = &[bgra];
            let settings: ObjcId = msg_send![class!(NSDictionary), dictionaryWithObjects: values.as_ptr() forKeys: keys.as_ptr() count: 1usize];
            let output: ObjcId = msg_send![class!(AVAssetReaderTrackOutput), alloc];
            let output: ObjcId = msg_send![output, initWithTrack: track outputSettings: settings];
            if output == nil {
                return Err(VideoFileError::new("AVAssetReaderTrackOutput(video BGRA) init failed"));
            }
            let can_add: BOOL = msg_send![reader, canAddOutput: output];
            if can_add == NO {
                let () = msg_send![output, release];
                return Err(VideoFileError::new("AVAssetReader rejected the BGRA output"));
            }
            let () = msg_send![reader, addOutput: output];
            let started: BOOL = msg_send![reader, startReading];
            if started == NO {
                let error: ObjcId = msg_send![reader, error];
                let () = msg_send![output, release];
                return Err(nserror_to_video_error("AVAssetReader startReading", error));
            }
            let () = msg_send![reader, retain];
            Ok(Self { reader, output })
        }
    }

    /// The next picture: `(pts_100ns, width, height, bgra)`; `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<(i64, u32, u32, Vec<u8>)>, VideoFileError> {
        let _pool = AutoreleasePool::new();
        unsafe {
            let sample: CMSampleBufferRef = msg_send![self.output, copyNextSampleBuffer];
            if sample.is_null() {
                let status: i64 = msg_send![self.reader, status];
                // AVAssetReaderStatusFailed
                if status == 3 {
                    let error: ObjcId = msg_send![self.reader, error];
                    return Err(nserror_to_video_error("copyNextSampleBuffer(BGRA)", error));
                }
                return Ok(None);
            }
            let pts = cmtime_to_100ns(CMSampleBufferGetPresentationTimeStamp(sample));
            let image = CMSampleBufferGetImageBuffer(sample);
            if image.is_null() {
                CFRelease(sample as *const c_void);
                return Err(VideoFileError::new("video sample has no image buffer"));
            }
            const READ_ONLY: CVPixelBufferLockFlags = 1;
            CVPixelBufferLockBaseAddress(image, READ_ONLY);
            let (width, height) = (CVPixelBufferGetWidth(image), CVPixelBufferGetHeight(image));
            let base = CVPixelBufferGetBaseAddress(image) as *const u8;
            let stride = CVPixelBufferGetBytesPerRow(image);
            let mut bgra = vec![0u8; width * height * 4];
            if !base.is_null() {
                for row in 0..height {
                    bgra[row * width * 4..(row + 1) * width * 4].copy_from_slice(std::slice::from_raw_parts(base.add(row * stride), width * 4));
                }
            }
            CVPixelBufferUnlockBaseAddress(image, READ_ONLY);
            CFRelease(sample as *const c_void);
            Ok(Some((pts, width as u32, height as u32, bgra)))
        }
    }
}

impl Drop for BgraFrameReader {
    fn drop(&mut self) {
        unsafe {
            let () = msg_send![self.reader, cancelReading];
            let () = msg_send![self.output, release];
            let () = msg_send![self.reader, release];
        }
    }
}
