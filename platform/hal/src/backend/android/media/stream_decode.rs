//! The phone's hardware stream decoder (NDK AMediaCodec): what `crate::stream_decode` drives on
//! Android, the decode sibling of `stream_encode`. H.264 (or H.265) Annex-B access units in, as
//! the Desk Link wire carries them (parameter sets in band before key frames), tightly packed
//! NV12 out, read from the codec's ByteBuffers at the stride, slice height and crop its output
//! format names (a ByteBuffer is linear; the vendor's tiled layouts are surface-only).
//!
//! Low latency: the vendor's low-latency decoder when it has one (`<name>.low_latency`, Quest:
//! `c2.qti.hevc.decoder.low_latency`), else the default one with `low-latency` 1; priority 0.
//! Each push queues one access unit and waits a short while for pictures; a picture is handed
//! out with the presentation time it came in with, so a caller tags pictures by that.
//!
//! GPU output (`open` with `gpu`): the decoder renders into an image reader whose images are
//! GPU-sampled hardware buffers; a picture is that image (no CPU copy), held until the caller
//! drops it (the reader then reuses its buffer).
use super::mediacodec::{checked, ndk, BufferInfo, Codec as MediaCodec, Format, Image, Reader, FLAG_CODEC_CONFIG, OUTPUT_FORMAT_CHANGED};
use crate::backend::android::native_layer::{poll, PollFd};
use crate::stream_decode::{drain_reader_with_budget, FenceWait, PendingGpu, ReaderStop};
use std::ffi::{c_void, CStr, CString};
use std::mem::ManuallyDrop;
use std::os::fd::{AsRawFd, FromRawFd, IntoRawFd, OwnedFd};
use std::time::{Duration, Instant};

relax_api::option_layouts! {
    GpuPicture,
}

/// MediaCodecInfo.CodecCapabilities.COLOR_FormatYUV420Planar / SemiPlanar.
const COLOR_I420: i32 = 19;
const COLOR_NV12: i32 = 21;
/// How long an input buffer is waited for.
const INPUT_WAIT: Duration = Duration::from_millis(50);

pub struct Picture {
    pub width: u32,
    pub height: u32,
    pub nv12: Vec<u8>,
    pub pts_us: i64,
    /// GPU output: the decoded image (its hardware buffer), returned to the reader on drop.
    pub gpu: Option<GpuPicture>,
}

/// A decoded picture in a hardware buffer of the decoder's image reader.
pub struct GpuPicture {
    _image: Image,
    /// The image's AHardwareBuffer (owned by the image).
    pub buffer: *mut c_void,
}
unsafe impl Send for GpuPicture {}

/// `AHardwareBuffer_Desc`.
#[repr(C)]
#[derive(Default)]
struct BufferDesc { width: u32, height: u32, layers: u32, format: u32, usage: u64, stride: u32, rfu0: u32, rfu1: u64 }
#[link(name = "android")]
extern "C" {
    fn AHardwareBuffer_describe(buffer: *const c_void, desc: *mut BufferDesc);
}

/// AImageReader formats and AHardwareBuffer usage.
const AIMAGE_FORMAT_YUV_420_888: i32 = 0x23;
const AIMAGE_FORMAT_PRIVATE: i32 = 0x22;
/// The first vendor usage bit (Qualcomm's gralloc: allocate compressed UBWC).
const USAGE_VENDOR_0: u64 = 1 << 28;
const USAGE_GPU_SAMPLED_IMAGE: u64 = 1 << 8;
/// Images of a GPU-output reader: the shown picture, the two before it (frames on the GPU), up to
/// six decoded and waiting for their display frame, and two for the decoder itself.
const READER_IMAGES: i32 = 11;
const INFO_TRY_AGAIN_LATER: isize = -1;
const OUTPUT_BUFFERS_CHANGED: isize = -3;
const AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE: i32 = -30001;
const AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED: i32 = -30002;
/// A push shares this wait across all reader handoffs and acquire fences.
const READER_WAIT: Duration = Duration::from_millis(20);
const READER_POLL_STEP: Duration = Duration::from_micros(300);
const POLLIN: i16 = 0x0001;
const POLLERR: i16 = 0x0008;
const POLLHUP: i16 = 0x0010;
const POLLNVAL: i16 = 0x0020;

/// Owns the image and the producer's fence until it is safe to sample or return the image.
struct FencedImage {
    image: ManuallyDrop<Image>,
    fence: Option<OwnedFd>,
}

// The decoder serializes access to the image and its fence.
unsafe impl Send for FencedImage {}

impl FencedImage {
    fn new(image: Image, fence: Option<OwnedFd>) -> Self {
        Self { image: ManuallyDrop::new(image), fence }
    }

    fn raw_image(&self) -> *mut c_void {
        self.image.0
    }

    fn has_fence(&self) -> bool {
        self.fence.is_some()
    }

    fn fence_fd(&self) -> i32 {
        self.fence.as_ref().map_or(-1, AsRawFd::as_raw_fd)
    }

    fn drop_invalid_fence(&mut self) {
        if let Some(fd) = self.fence.take() {
            // POLLNVAL: the descriptor is already closed.
            let _ = fd.into_raw_fd();
        }
    }

    fn into_signaled(mut self) -> Image {
        self.fence = None;
        // Suppress our Drop before moving the image to its picture.
        let mut this = ManuallyDrop::new(self);
        unsafe { ManuallyDrop::take(&mut this.image) }
    }
}

impl Drop for FencedImage {
    fn drop(&mut self) {
        let image = unsafe { ManuallyDrop::take(&mut self.image) };
        if let Some(fd) = self.fence.take() {
            image.delete_async(fd);
        }
    }
}

enum FenceState { Signaled, Pending }

fn poll_fence(fd: i32, timeout_ms: i32) -> Result<FenceState, (bool, String)> {
    if fd < 0 {
        return Ok(FenceState::Signaled);
    }
    let mut pfd = PollFd { fd, events: POLLIN, revents: 0 };
    let result = unsafe { poll(&mut pfd, 1, timeout_ms) };
    if result < 0 {
        let error = std::io::Error::last_os_error();
        return match error.kind() {
            std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock => Ok(FenceState::Pending),
            _ => Err((false, format!("poll image acquire fence: {error}"))),
        };
    }
    if pfd.revents & (POLLNVAL | POLLERR | POLLHUP) != 0 {
        return Err((pfd.revents & POLLNVAL != 0, format!("poll image acquire fence: revents {:#x}", pfd.revents)));
    }
    if pfd.revents & POLLIN != 0 { Ok(FenceState::Signaled) } else { Ok(FenceState::Pending) }
}

fn poll_fence_once(fd: i32) -> Result<FenceState, (bool, String)> {
    poll_fence(fd, 0)
}

fn wait_fence_fd(fd: i32, limit: Duration) -> Result<FenceWait, (bool, String)> {
    let start = Instant::now();
    loop {
        let remaining = limit.saturating_sub(start.elapsed());
        let timeout_ms = remaining.as_millis() as i32; // limit is at most READER_WAIT
        if timeout_ms > 0 {
            if let FenceState::Signaled = poll_fence(fd, timeout_ms)? {
                return Ok(FenceWait::Signaled(start.elapsed()));
            }
        } else {
            if let FenceState::Signaled = poll_fence_once(fd)? {
                return Ok(FenceWait::Signaled(start.elapsed()));
            }
            if remaining.is_zero() {
                return Ok(FenceWait::TimedOut);
            }
            // poll's timeout is in whole milliseconds. Use only the remaining fraction.
            std::thread::sleep(READER_POLL_STEP.min(limit.saturating_sub(start.elapsed())));
        }
    }
}

pub struct StreamDecoder {
    // Return the acquired image (and any unsignaled fence) before destroying either owner.
    pending_gpu: PendingGpu<FencedImage>,
    codec: MediaCodec,
    /// The output layout (from the latest format change).
    width: u32,
    height: u32,
    stride: u32,
    slice_height: u32,
    crop: [u32; 4],
    color: i32,
    pub info: String,
    /// GPU output: the reader the decoder renders into (dropped after the codec).
    reader: Option<Reader>,
    logged: std::sync::atomic::AtomicBool,
}

fn codec_name(codec: &MediaCodec) -> String {
    unsafe {
        let mut name = std::ptr::null_mut();
        if ndk::AMediaCodec_getName(codec.0, &mut name) == 0 && !name.is_null() {
            let text = CStr::from_ptr(name).to_string_lossy().into_owned();
            ndk::AMediaCodec_releaseName(codec.0, name);
            text
        } else {
            "MediaCodec".to_string()
        }
    }
}

impl StreamDecoder {
    /// A decoder for pictures of about `width` x `height` (the stream's own size wins).
    pub fn open(width: u32, height: u32, hevc: bool, gpu: bool) -> Result<StreamDecoder, String> {
        let mime: &CStr = if hevc { c"video/hevc" } else { c"video/avc" };
        // the vendor's low-latency variant of its default decoder, when there is one
        let default = MediaCodec::decoder(mime)?;
        let base = codec_name(&default);
        let low = CString::new(format!("{base}.low_latency")).ok().and_then(|n| {
            let c = unsafe { ndk::AMediaCodec_createCodecByName(n.as_ptr()) };
            (!c.is_null()).then(|| MediaCodec(c))
        });
        let codec = match low {
            Some(low) => {
                drop(default);
                low
            }
            None => default,
        };
        let format = Format::new();
        format.string(c"mime", mime);
        format.int(c"width", width.max(64) as i32);
        format.int(c"height", height.max(64) as i32);
        format.int(c"max-width", width.max(64) as i32);
        format.int(c"max-height", height.max(64) as i32);
        format.int(c"max-input-size", (width as i32).saturating_mul(height as i32).max(1 << 20));
        format.int(c"color-format", COLOR_NV12);
        format.int(c"priority", 0);
        format.int(c"low-latency", 1);
        // the vendor's low-latency mode (without it the Qualcomm decoder holds pictures back) and
        // the decoder clocked for the highest rate
        format.int(c"vendor.qti-ext-dec-low-latency.enable", 1);
        format.int(c"operating-rate", i32::MAX);
        // GPU output: an image reader of GPU-sampled hardware buffers is the decoder's surface
        let (reader, window) = if gpu {
            let mut reader = std::ptr::null_mut();
            // (debug `debug.xrremote.ubwc` 1 at start: the reader asks for Qualcomm's compressed
            // layout, the private format and its vendor usage bit; the import's log line says what
            // the driver makes of it. Off: the linear YUV_420_888 pictures)
            let ubwc = super::super::xr::system_property("debug.xrremote.ubwc").is_some_and(|v| v.trim() == "1");
            let (fmt, usage) = if ubwc { (AIMAGE_FORMAT_PRIVATE, USAGE_GPU_SAMPLED_IMAGE | USAGE_VENDOR_0) } else { (AIMAGE_FORMAT_YUV_420_888, USAGE_GPU_SAMPLED_IMAGE) };
            checked(unsafe { ndk::AImageReader_newWithUsage(width.max(64) as i32, height.max(64) as i32, fmt, usage, READER_IMAGES, &mut reader) }, "make the decoder's image reader")?;
            let reader = Reader(reader);
            let mut window = std::ptr::null_mut();
            checked(unsafe { ndk::AImageReader_getWindow(reader.0, &mut window) }, "the image reader's window")?;
            (Some(reader), window)
        } else {
            (None, std::ptr::null_mut())
        };
        unsafe {
            checked(ndk::AMediaCodec_configure(codec.0, format.0, window, std::ptr::null_mut(), 0), "configure the stream decoder")?;
            checked(ndk::AMediaCodec_start(codec.0), "start the stream decoder")?;
        }
        let info = codec_name(&codec) + if gpu { " → GPU images" } else { "" };
        Ok(StreamDecoder { pending_gpu: PendingGpu::default(), codec, width, height, stride: width, slice_height: height, crop: [0, 0, width, height], color: COLOR_NV12, info, reader, logged: Default::default() })
    }

    fn output_format(&mut self) {
        let f = Format(unsafe { ndk::AMediaCodec_getOutputFormat(self.codec.0) });
        let w = f.get_int(c"width").unwrap_or(self.width as i32).max(2) as u32;
        let h = f.get_int(c"height").unwrap_or(self.height as i32).max(2) as u32;
        self.width = w;
        self.height = h;
        self.stride = f.get_int(c"stride").filter(|s| *s as u32 >= w).map_or(w, |s| s as u32);
        self.slice_height = f.get_int(c"slice-height").filter(|s| *s as u32 >= h).map_or(h, |s| s as u32);
        let crop = [c"crop-left", c"crop-top", c"crop-right", c"crop-bottom"].map(|k| f.get_int(k));
        self.crop = match crop {
            [Some(l), Some(t), Some(r), Some(b)] if r >= l && b >= t => [l as u32, t as u32, (r - l + 1) as u32, (b - t + 1) as u32],
            _ => [0, 0, w, h],
        };
        self.color = f.get_int(c"color-format").unwrap_or(COLOR_NV12);
        eprintln!("[hal-android-stream-decode] {}: {}x{} stride {} slice {} crop {:?} color {:#x}", self.info, w, h, self.stride, self.slice_height, self.crop, self.color);
    }

    /// Queues one access unit (`pts_us` comes back on its picture) and collects what the codec
    /// has decoded, waiting up to `wait` for the first picture.
    pub fn push(&mut self, annex_b: &[u8], pts_us: i64, wait: Duration) -> Result<Vec<Picture>, String> {
        unsafe {
            let index = ndk::AMediaCodec_dequeueInputBuffer(self.codec.0, INPUT_WAIT.as_micros() as i64);
            if index < 0 {
                return Err(format!("the decoder gave no input buffer ({index})"));
            }
            let mut capacity = 0usize;
            let buffer = ndk::AMediaCodec_getInputBuffer(self.codec.0, index as usize, &mut capacity);
            if buffer.is_null() || capacity < annex_b.len() {
                return Err(format!("the decoder's input buffer holds {capacity} bytes, the access unit is {}", annex_b.len()));
            }
            std::slice::from_raw_parts_mut(buffer, annex_b.len()).copy_from_slice(annex_b);
            checked(ndk::AMediaCodec_queueInputBuffer(self.codec.0, index as usize, 0, annex_b.len(), pts_us.max(0) as u64, 0), "queue an access unit")?;
        }
        self.pictures(wait)
    }

    /// The pictures decoded so far, waiting up to `wait` for the first.
    pub fn pictures(&mut self, wait: Duration) -> Result<Vec<Picture>, String> {
        let start = Instant::now();
        let mut out = Vec::new();
        let max_reader_wait = READER_WAIT.min(wait);
        let mut reader_budget = max_reader_wait;
        if self.reader.is_some() && !self.pending_gpu.is_empty() {
            let fresh = self.pending_gpu.fresh_wait(start, max_reader_wait);
            let stop = self.drain_reader_wait(&mut out, &mut reader_budget, fresh, true)?;
            if matches!(stop, ReaderStop::MaxAcquired | ReaderStop::FencePending) {
                return Ok(out);
            }
        }
        loop {
            let left = if out.is_empty() { wait.saturating_sub(start.elapsed()) } else { Duration::ZERO };
            let mut info = BufferInfo::default();
            let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(self.codec.0, &mut info, left.as_micros() as i64) };
            match index {
                OUTPUT_FORMAT_CHANGED => {
                    self.output_format();
                    continue;
                }
                OUTPUT_BUFFERS_CHANGED => continue,
                INFO_TRY_AGAIN_LATER if out.is_empty() && !left.is_zero() => continue,
                INFO_TRY_AGAIN_LATER => break,
                index if index < 0 => return Err(format!("MediaCodec: dequeue output buffer: {index}")),
                _ => {}
            }
            let index = index as usize;
            if self.reader.is_some() {
                // rendered into the reader: its image is the picture
                let shown = info.flags & FLAG_CODEC_CONFIG == 0;
                checked(unsafe { ndk::AMediaCodec_releaseOutputBuffer(self.codec.0, index, shown) }, "release output buffer")?;
                if shown {
                    self.pending_gpu.push(info.presentation_time_us, Instant::now());
                    let no_buffer = reader_budget;
                    let stop = self.drain_reader_wait(&mut out, &mut reader_budget, no_buffer, false)?;
                    if matches!(stop, ReaderStop::MaxAcquired | ReaderStop::FencePending) {
                        return Ok(out);
                    }
                }
                continue;
            }
            if info.size > 0 && info.flags & FLAG_CODEC_CONFIG == 0 {
                let mut capacity = 0usize;
                let buffer = unsafe { ndk::AMediaCodec_getOutputBuffer(self.codec.0, index, &mut capacity) };
                if !buffer.is_null() && info.offset >= 0 && info.offset as usize + info.size as usize <= capacity {
                    let bytes = unsafe { std::slice::from_raw_parts(buffer.add(info.offset as usize), info.size as usize) };
                    out.push(self.nv12(bytes, info.presentation_time_us));
                }
            }
            checked(unsafe { ndk::AMediaCodec_releaseOutputBuffer(self.codec.0, index, false) }, "release output buffer")?;
        }
        if self.reader.is_some() && !self.pending_gpu.is_empty() {
            self.drain_reader_now(&mut out)?;
        }
        Ok(out)
    }

    fn gpu_picture(&self, image: Image, pts_us: i64) -> Result<Picture, String> {
        let mut buffer = std::ptr::null_mut();
        checked(unsafe { ndk::AImage_getHardwareBuffer(image.0, &mut buffer) }, "the picture's hardware buffer")?;
        // The texture spans the coded buffer. The cropped picture is at its top left.
        let mut desc = BufferDesc::default();
        unsafe { AHardwareBuffer_describe(buffer, &mut desc) };
        let (w, h) = (desc.width, desc.height);
        if !self.logged.swap(true, std::sync::atomic::Ordering::Relaxed) {
            super::super::xr::log(&format!("stream decode {}: GPU pictures {w}x{h} (crop {:?}), buffer format {:#x} usage {:#x} stride {}", self.info, self.crop, desc.format, desc.usage, desc.stride));
        }
        Ok(Picture { width: w, height: h, nv12: Vec::new(), pts_us, gpu: Some(GpuPicture { _image: image, buffer }) })
    }

    fn discard_failed_fence(&mut self, (invalid_fd, error): (bool, String)) -> String {
        let (mut image, _, _) = self.pending_gpu.take_acquired().unwrap();
        if invalid_fd {
            image.drop_invalid_fence();
        }
        error
    }

    fn drain_reader_now(&mut self, out: &mut Vec<Picture>) -> Result<ReaderStop, String> {
        loop {
            if let Some(image) = self.pending_gpu.front_acquired() {
                let state = poll_fence_once(image.fence_fd()).map_err(|e| self.discard_failed_fence(e))?;
                if let FenceState::Pending = state {
                    return Ok(ReaderStop::FencePending);
                }
                let (image, pts_us, _) = self.pending_gpu.take_acquired().unwrap();
                out.push(self.gpu_picture(image.into_signaled(), pts_us)?);
            }
            if !self.pending_gpu.has_released() {
                return Ok(ReaderStop::Done);
            }
            let reader = self.reader.as_ref().unwrap();
            let mut image = std::ptr::null_mut();
            let mut fence_fd = -1;
            let status = unsafe { ndk::AImageReader_acquireNextImageAsync(reader.0, &mut image, &mut fence_fd) };
            match status {
                AMEDIA_IMGREADER_NO_BUFFER_AVAILABLE => return Ok(ReaderStop::NoBuffer),
                AMEDIA_IMGREADER_MAX_IMAGES_ACQUIRED => return Ok(ReaderStop::MaxAcquired),
                _ => checked(status, "acquire reader image")?,
            }
            let fence = (fence_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(fence_fd) });
            if image.is_null() {
                return Ok(ReaderStop::NoBuffer);
            }
            let image = FencedImage::new(Image(image), fence);
            // The timestamp is BufferItem metadata. Only access the pixels after the fence.
            let mut time = 0;
            checked(unsafe { ndk::AImage_getTimestamp(image.raw_image(), &mut time) }, "the picture's timestamp")?;
            let Some((pts_us, released_at)) = self.pending_gpu.match_image(time / 1000) else { continue };
            if image.has_fence() {
                self.pending_gpu.hold_acquired(image, pts_us, released_at);
            } else {
                out.push(self.gpu_picture(image.into_signaled(), pts_us)?);
            }
        }
    }

    fn wait_acquired_fence(&mut self, limit: Duration) -> Result<FenceWait, String> {
        let fd = self.pending_gpu.front_acquired().unwrap().fence_fd();
        wait_fence_fd(fd, limit).map_err(|e| self.discard_failed_fence(e))
    }

    fn drain_reader_wait(
        &mut self,
        out: &mut Vec<Picture>,
        budget: &mut Duration,
        no_buffer_budget: Duration,
        stop_on_picture: bool,
    ) -> Result<ReaderStop, String> {
        drain_reader_with_budget(
            &mut (self, out), budget, no_buffer_budget, READER_POLL_STEP, stop_on_picture,
            |(decoder, out)| {
                let before = out.len();
                let stop = decoder.drain_reader_now(out)?;
                Ok((stop, out.len() - before))
            },
            |step| {
                let start = Instant::now();
                std::thread::sleep(step);
                start.elapsed()
            },
            |(decoder, _), limit| decoder.wait_acquired_fence(limit),
        )
    }

    /// The cropped picture of a ByteBuffer as tightly packed NV12.
    fn nv12(&self, bytes: &[u8], pts_us: i64) -> Picture {
        let [x0, y0, w, h] = self.crop;
        let (w, h) = (w as usize & !1, h as usize & !1);
        let (x0, y0) = (x0 as usize & !1, y0 as usize & !1);
        let (stride, slice) = (self.stride as usize, self.slice_height as usize);
        let mut nv12 = vec![128u8; w * h * 3 / 2];
        for row in 0..h {
            let at = (y0 + row) * stride + x0;
            if let Some(src) = bytes.get(at..at + w) {
                nv12[row * w..row * w + w].copy_from_slice(src);
            }
        }
        let chroma = stride * slice;
        if self.color == COLOR_I420 {
            let quarter = (stride / 2) * (slice / 2);
            for row in 0..h / 2 {
                for col in 0..w / 2 {
                    let at = (y0 / 2 + row) * (stride / 2) + x0 / 2 + col;
                    let o = w * h + row * w + col * 2;
                    nv12[o] = bytes.get(chroma + at).copied().unwrap_or(128);
                    nv12[o + 1] = bytes.get(chroma + quarter + at).copied().unwrap_or(128);
                }
            }
        } else {
            for row in 0..h / 2 {
                let at = chroma + (y0 / 2 + row) * stride + x0;
                if let Some(src) = bytes.get(at..at + w) {
                    nv12[w * h + row * w..w * h + row * w + w].copy_from_slice(src);
                }
            }
        }
        Picture { width: w as u32, height: h as u32, nv12, pts_us, gpu: None }
    }
}
