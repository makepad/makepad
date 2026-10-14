//! The NDK media plumbing the phone's codecs share (libmediandk): owned MediaCodec, MediaFormat,
//! MediaExtractor, ImageReader and Image handles that free themselves, and a decoded picture out
//! of a YUV_420_888 image as tightly packed NV12. Used by `hevc` (single stills) and `video_file`
//! (makepad_video's file decoder on Android).
use std::ffi::{c_char, c_void, CStr};
use std::time::{Duration, Instant};

/// MediaCodec.BUFFER_FLAG_*.
pub const FLAG_CODEC_CONFIG: u32 = 2;
pub const FLAG_END_OF_STREAM: u32 = 4;
/// AMediaCodec_dequeueOutputBuffer's "the output format changed".
pub const OUTPUT_FORMAT_CHANGED: isize = -2;
/// AIMAGE_FORMAT_YUV_420_888.
pub const YUV_420_888: u32 = 0x23;

pub fn checked(status: i32, what: &str) -> Result<(), String> {
    if status == 0 { Ok(()) } else { Err(format!("MediaCodec: {what}: {status}")) }
}

#[repr(C)]
#[derive(Default)]
pub struct BufferInfo {
    pub offset: i32,
    pub size: i32,
    pub presentation_time_us: i64,
    pub flags: u32,
}

/// A codec; stopped and deleted on drop.
pub struct Codec(pub *mut c_void);
// SAFETY: an AMediaCodec may be used from any thread, one call at a time; the owner serialises.
unsafe impl Send for Codec {}
impl Codec {
    pub fn decoder(mime: &CStr) -> Result<Codec, String> {
        let codec = unsafe { ndk::AMediaCodec_createDecoderByType(mime.as_ptr()) };
        if codec.is_null() { Err(format!("no decoder for {}", mime.to_string_lossy())) } else { Ok(Codec(codec)) }
    }
    pub fn encoder(mime: &CStr) -> Result<Codec, String> {
        let codec = unsafe { ndk::AMediaCodec_createEncoderByType(mime.as_ptr()) };
        if codec.is_null() { Err(format!("no encoder for {}", mime.to_string_lossy())) } else { Ok(Codec(codec)) }
    }
}
impl Drop for Codec {
    fn drop(&mut self) {
        unsafe {
            ndk::AMediaCodec_stop(self.0);
            ndk::AMediaCodec_delete(self.0);
        }
    }
}

/// A media format; deleted on drop.
pub struct Format(pub *mut c_void);
unsafe impl Send for Format {}
impl Format {
    pub fn new() -> Format {
        Format(unsafe { ndk::AMediaFormat_new() })
    }
    pub fn string(&self, key: &CStr, value: &CStr) {
        unsafe { ndk::AMediaFormat_setString(self.0, key.as_ptr(), value.as_ptr()) }
    }
    pub fn int(&self, key: &CStr, value: i32) {
        unsafe { ndk::AMediaFormat_setInt32(self.0, key.as_ptr(), value) }
    }
    pub fn get_int(&self, key: &CStr) -> Option<i32> {
        let mut value = 0;
        (!self.0.is_null() && unsafe { ndk::AMediaFormat_getInt32(self.0, key.as_ptr(), &mut value) }).then_some(value)
    }
    pub fn get_long(&self, key: &CStr) -> Option<i64> {
        let mut value = 0;
        (!self.0.is_null() && unsafe { ndk::AMediaFormat_getInt64(self.0, key.as_ptr(), &mut value) }).then_some(value)
    }
    /// A string value, copied (the format owns the original).
    pub fn get_string(&self, key: &CStr) -> Option<String> {
        let mut value: *const c_char = std::ptr::null();
        if self.0.is_null() || !unsafe { ndk::AMediaFormat_getString(self.0, key.as_ptr(), &mut value) } || value.is_null() {
            return None;
        }
        Some(unsafe { CStr::from_ptr(value) }.to_string_lossy().into_owned())
    }
}
impl Drop for Format {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe { ndk::AMediaFormat_delete(self.0) };
        }
    }
}

/// A demuxer over a file; deleted on drop. It reads through our own descriptor: a path handed
/// to the platform is opened by the media server, which cannot reach the app's private files.
pub struct Extractor(pub *mut c_void, std::fs::File);
unsafe impl Send for Extractor {}
impl Extractor {
    pub fn open(path: &str) -> Result<Extractor, String> {
        use std::os::fd::AsRawFd;
        let file = std::fs::File::open(path).map_err(|e| format!("open {path}: {e}"))?;
        let len = file.metadata().map_err(|e| format!("{path}: {e}"))?.len() as i64;
        let ex = Extractor(unsafe { ndk::AMediaExtractor_new() }, file);
        if ex.0.is_null() {
            return Err("no media extractor".into());
        }
        checked(unsafe { ndk::AMediaExtractor_setDataSourceFd(ex.0, ex.1.as_raw_fd(), 0, len) }, &format!("demux {path}"))?;
        Ok(ex)
    }
    pub fn track_count(&self) -> usize {
        unsafe { ndk::AMediaExtractor_getTrackCount(self.0) }
    }
    pub fn track_format(&self, index: usize) -> Format {
        Format(unsafe { ndk::AMediaExtractor_getTrackFormat(self.0, index) })
    }
    pub fn select(&self, index: usize) -> Result<(), String> {
        checked(unsafe { ndk::AMediaExtractor_selectTrack(self.0, index) }, "select track")
    }
    /// The next sample into `out` and its time; `None` at the end of the track.
    pub fn read(&self, out: &mut [u8]) -> Option<(usize, i64)> {
        let n = unsafe { ndk::AMediaExtractor_readSampleData(self.0, out.as_mut_ptr(), out.len()) };
        if n < 0 {
            return None;
        }
        let time = unsafe { ndk::AMediaExtractor_getSampleTime(self.0) };
        unsafe { ndk::AMediaExtractor_advance(self.0) };
        Some((n as usize, time))
    }
    /// To the sync sample at or before `time_us`.
    pub fn seek(&self, time_us: i64) -> Result<(), String> {
        checked(unsafe { ndk::AMediaExtractor_seekTo(self.0, time_us, 0) }, "seek")
    }
}
impl Drop for Extractor {
    fn drop(&mut self) {
        unsafe { ndk::AMediaExtractor_delete(self.0) };
    }
}

/// An image reader (the surface a video decoder renders into); deleted on drop.
pub struct Reader(pub *mut c_void);
unsafe impl Send for Reader {}
impl Reader {
    pub fn yuv(width: u32, height: u32, max_images: i32) -> Result<(Reader, *mut c_void), String> {
        let mut reader = std::ptr::null_mut();
        checked(unsafe { ndk::AImageReader_new(width as i32, height as i32, YUV_420_888, max_images, &mut reader) }, "make the picture reader")?;
        let reader = Reader(reader);
        let mut window = std::ptr::null_mut();
        checked(unsafe { ndk::AImageReader_getWindow(reader.0, &mut window) }, "the picture reader's window")?;
        Ok((reader, window))
    }

    /// The next rendered picture as `width` x `height` NV12, waiting up to `timeout` for it.
    pub fn next_nv12(&self, width: u32, height: u32, timeout: Duration) -> Result<Vec<u8>, String> {
        let start = Instant::now();
        let image = loop {
            let mut image = std::ptr::null_mut();
            let status = unsafe { ndk::AImageReader_acquireNextImage(self.0, &mut image) };
            if status == 0 && !image.is_null() {
                break Image(image);
            }
            if start.elapsed() > timeout {
                return Err(format!("the decoded picture never reached the reader ({status})"));
            }
            std::thread::sleep(Duration::from_millis(1));
        };
        image.nv12(width as usize, height as usize)
    }
}
impl Drop for Reader {
    fn drop(&mut self) {
        unsafe { ndk::AImageReader_delete(self.0) }
    }
}

pub struct Image(pub *mut c_void);
impl Image {
    /// A YUV_420_888 picture as tightly packed NV12 of `w` x `h` (its top-left part).
    pub fn nv12(&self, w: usize, h: usize) -> Result<Vec<u8>, String> {
        let plane = |i: i32| -> Result<(&[u8], usize, usize), String> {
            let (mut data, mut len, mut row, mut pixel) = (std::ptr::null_mut(), 0, 0, 0);
            unsafe {
                checked(ndk::AImage_getPlaneData(self.0, i, &mut data, &mut len), "picture plane")?;
                checked(ndk::AImage_getPlaneRowStride(self.0, i, &mut row), "picture row stride")?;
                checked(ndk::AImage_getPlanePixelStride(self.0, i, &mut pixel), "picture pixel stride")?;
            }
            if data.is_null() || len <= 0 || row <= 0 || pixel <= 0 {
                return Err("an empty picture plane".into());
            }
            Ok((unsafe { std::slice::from_raw_parts(data, len as usize) }, row as usize, pixel as usize))
        };
        let (y, y_row, y_pixel) = plane(0)?;
        let (u, u_row, u_pixel) = plane(1)?;
        let (v, v_row, v_pixel) = plane(2)?;
        let at = |p: &[u8], i: usize| p.get(i).copied().unwrap_or(128);
        let mut nv12 = vec![0u8; w * h * 3 / 2];
        for row in 0..h {
            if y_pixel == 1 {
                let src = &y[(row * y_row).min(y.len())..];
                let n = w.min(src.len());
                nv12[row * w..row * w + n].copy_from_slice(&src[..n]);
            } else {
                for col in 0..w {
                    nv12[row * w + col] = at(y, row * y_row + col * y_pixel);
                }
            }
        }
        for row in 0..h / 2 {
            for col in 0..w / 2 {
                let o = w * h + row * w + col * 2;
                nv12[o] = at(u, row * u_row + col * u_pixel);
                nv12[o + 1] = at(v, row * v_row + col * v_pixel);
            }
        }
        Ok(nv12)
    }
}
impl Drop for Image {
    fn drop(&mut self) {
        unsafe { ndk::AImage_delete(self.0) }
    }
}

pub mod ndk {
    use super::{c_char, c_void, BufferInfo};
    #[link(name = "mediandk")]
    extern "C" {
        pub fn AMediaFormat_new() -> *mut c_void;
        pub fn AMediaFormat_delete(format: *mut c_void) -> i32;
        pub fn AMediaFormat_setString(format: *mut c_void, name: *const c_char, value: *const c_char);
        pub fn AMediaFormat_setInt32(format: *mut c_void, name: *const c_char, value: i32);
        pub fn AMediaFormat_getInt32(format: *mut c_void, name: *const c_char, out: *mut i32) -> bool;
        pub fn AMediaFormat_getInt64(format: *mut c_void, name: *const c_char, out: *mut i64) -> bool;
        pub fn AMediaFormat_getString(format: *mut c_void, name: *const c_char, out: *mut *const c_char) -> bool;
        pub fn AMediaCodec_createEncoderByType(mime: *const c_char) -> *mut c_void;
        pub fn AMediaCodec_createDecoderByType(mime: *const c_char) -> *mut c_void;
        pub fn AMediaCodec_configure(codec: *mut c_void, format: *const c_void, surface: *mut c_void, crypto: *mut c_void, flags: u32) -> i32;
        pub fn AMediaCodec_start(codec: *mut c_void) -> i32;
        pub fn AMediaCodec_stop(codec: *mut c_void) -> i32;
        pub fn AMediaCodec_flush(codec: *mut c_void) -> i32;
        pub fn AMediaCodec_delete(codec: *mut c_void) -> i32;
        pub fn AMediaCodec_getInputFormat(codec: *mut c_void) -> *mut c_void;
        pub fn AMediaCodec_getOutputFormat(codec: *mut c_void) -> *mut c_void;
        pub fn AMediaCodec_dequeueInputBuffer(codec: *mut c_void, timeout_us: i64) -> isize;
        pub fn AMediaCodec_getInputBuffer(codec: *mut c_void, index: usize, size: *mut usize) -> *mut u8;
        pub fn AMediaCodec_queueInputBuffer(codec: *mut c_void, index: usize, offset: i64, size: usize, time_us: u64, flags: u32) -> i32;
        pub fn AMediaCodec_dequeueOutputBuffer(codec: *mut c_void, info: *mut BufferInfo, timeout_us: i64) -> isize;
        pub fn AMediaCodec_getOutputBuffer(codec: *mut c_void, index: usize, size: *mut usize) -> *mut u8;
        pub fn AMediaCodec_releaseOutputBuffer(codec: *mut c_void, index: usize, render: bool) -> i32;
        pub fn AMediaExtractor_new() -> *mut c_void;
        pub fn AMediaExtractor_delete(ex: *mut c_void) -> i32;
        pub fn AMediaExtractor_setDataSourceFd(ex: *mut c_void, fd: i32, offset: i64, length: i64) -> i32;
        pub fn AMediaExtractor_getTrackCount(ex: *mut c_void) -> usize;
        pub fn AMediaExtractor_getTrackFormat(ex: *mut c_void, index: usize) -> *mut c_void;
        pub fn AMediaExtractor_selectTrack(ex: *mut c_void, index: usize) -> i32;
        pub fn AMediaExtractor_readSampleData(ex: *mut c_void, buffer: *mut u8, capacity: usize) -> isize;
        pub fn AMediaExtractor_getSampleTime(ex: *mut c_void) -> i64;
        pub fn AMediaExtractor_advance(ex: *mut c_void) -> bool;
        pub fn AMediaExtractor_seekTo(ex: *mut c_void, time_us: i64, mode: i32) -> i32;
        pub fn AImageReader_new(width: i32, height: i32, format: u32, max_images: i32, reader: *mut *mut c_void) -> i32;
        pub fn AImageReader_getWindow(reader: *mut c_void, window: *mut *mut c_void) -> i32;
        pub fn AImageReader_delete(reader: *mut c_void);
        pub fn AImageReader_acquireNextImage(reader: *mut c_void, image: *mut *mut c_void) -> i32;
        pub fn AImage_getPlaneData(image: *const c_void, plane: i32, data: *mut *mut u8, length: *mut i32) -> i32;
        pub fn AImage_getPlaneRowStride(image: *const c_void, plane: i32, stride: *mut i32) -> i32;
        pub fn AImage_getPlanePixelStride(image: *const c_void, plane: i32, stride: *mut i32) -> i32;
        pub fn AImage_delete(image: *mut c_void);
    }
}
