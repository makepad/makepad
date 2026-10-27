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
use std::ffi::{c_void, CStr, CString};
use std::time::{Duration, Instant};

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
/// How long a picture the decoder released takes at most to reach its reader.
const READER_WAIT: Duration = Duration::from_millis(20);

pub struct StreamDecoder {
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
        Ok(StreamDecoder { codec, width, height, stride: width, slice_height: height, crop: [0, 0, width, height], color: COLOR_NV12, info, reader, logged: Default::default() })
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
        loop {
            let left = if out.is_empty() { wait.saturating_sub(start.elapsed()) } else { Duration::ZERO };
            let mut info = BufferInfo::default();
            let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(self.codec.0, &mut info, left.as_micros() as i64) };
            if index == OUTPUT_FORMAT_CHANGED {
                self.output_format();
                continue;
            }
            if index < 0 {
                if out.is_empty() && !left.is_zero() {
                    continue;
                }
                return Ok(out);
            }
            let index = index as usize;
            if let Some(reader) = &self.reader {
                // rendered into the reader: its image is the picture
                let shown = info.flags & FLAG_CODEC_CONFIG == 0;
                unsafe { ndk::AMediaCodec_releaseOutputBuffer(self.codec.0, index, shown) };
                if shown {
                    match self.reader_image(reader, info.presentation_time_us) {
                        Ok(p) => out.push(p),
                        Err(e) => eprintln!("[hal-android-stream-decode] {e}"),
                    }
                }
                continue;
            }
            if info.size > 0 && info.flags & FLAG_CODEC_CONFIG == 0 {
                let mut capacity = 0usize;
                let buffer = unsafe { ndk::AMediaCodec_getOutputBuffer(self.codec.0, index, &mut capacity) };
                if !buffer.is_null() && (info.offset + info.size) as usize <= capacity {
                    let bytes = unsafe { std::slice::from_raw_parts(buffer.add(info.offset as usize), info.size as usize) };
                    out.push(self.nv12(bytes, info.presentation_time_us));
                }
            }
            unsafe { ndk::AMediaCodec_releaseOutputBuffer(self.codec.0, index, false) };
        }
    }

    /// The reader's image of the picture of `pts_us` (the one just released to it), waiting
    /// up to READER_WAIT for it; older images still in the reader are dropped.
    fn reader_image(&self, reader: &Reader, pts_us: i64) -> Result<Picture, String> {
        let start = Instant::now();
        loop {
            let mut image = std::ptr::null_mut();
            let status = unsafe { ndk::AImageReader_acquireNextImage(reader.0, &mut image) };
            if status == 0 && !image.is_null() {
                let image = Image(image);
                let mut time = 0i64;
                unsafe { ndk::AImage_getTimestamp(image.0, &mut time) };
                // (the decoder stamps its surface frames with the presentation time, in ns)
                if time / 1000 < pts_us {
                    continue;
                }
                let mut buffer = std::ptr::null_mut();
                checked(unsafe { ndk::AImage_getHardwareBuffer(image.0, &mut buffer) }, "the picture's hardware buffer")?;
                // the size of the image's buffer (what a texture of it spans: the decoder's coded
                // size, not the image's cropped one); the picture is its top-left part
                let mut desc = BufferDesc::default();
                unsafe { AHardwareBuffer_describe(buffer, &mut desc) };
                let (w, h) = (desc.width, desc.height);
                if self.logged.swap(true, std::sync::atomic::Ordering::Relaxed) == false {
                    // (the buffer's format and usage: a vendor format with the private UBWC bit,
                    // 1 << 28, is the compressed layout the GPU reads at a fraction of the bytes)
                    super::super::xr::log(&format!("stream decode {}: GPU pictures {w}x{h} (crop {:?}), buffer format {:#x} usage {:#x} stride {}", self.info, self.crop, desc.format, desc.usage, desc.stride));
                }
                return Ok(Picture { width: w, height: h, nv12: Vec::new(), pts_us, gpu: Some(GpuPicture { _image: image, buffer }) });
            }
            if start.elapsed() > READER_WAIT {
                return Err(format!("the decoded picture never reached the image reader ({status})"));
            }
            std::thread::sleep(Duration::from_micros(300));
        }
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
