//! The host API of the hardware stream encoders: what `makepad_video::VideoStreamEncoder` drives on
//! Linux (Vulkan Video, `backend::vulkan::video_encode`) and Android (MediaCodec,
//! `backend::android::media::stream_encode`). JIT-compiled code (makepad run) reaches the
//! process's encoders only through these functions, like the DMA-BUF ops (`crate::dmabuf`). An
//! encoder is named by a number; a failure's text is the calling thread's last error
//! (`hal_stream_encoder_error`). Each encode call is synchronous: it returns once the picture's
//! bitstream is ready (`hal_stream_encoder_read` takes it). Texture pictures are Linux's.

#[cfg(target_os = "android")]
use crate::backend::android::media::stream_encode::{Codec, StreamEncoder, Tuning};
#[cfg(target_os = "linux")]
use crate::backend::vulkan::{share::copy, video_encode::{Codec, StreamEncoder, Tuning}};
use std::collections::HashMap;
#[cfg(target_os = "linux")]
use std::os::fd::{FromRawFd, OwnedFd};
use std::sync::{Arc, Mutex};

struct Open {
    encoder: StreamEncoder,
    /// The last picture's Annex-B bytes, until read.
    out: Vec<u8>,
}

/// Each encoder behind its own lock: the tiles of one picture encode at once, each on its own
/// engine (the map's lock is held only to find one).
static ENCODERS: Mutex<Option<HashMap<u64, Arc<Mutex<Open>>>>> = Mutex::new(None);
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn get(encoder: u64) -> Option<Arc<Mutex<Open>>> {
    crate::lock(&ENCODERS).as_ref()?.get(&encoder).cloned()
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn failed(error: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = error);
}

/// Flag of the encode calls: this picture is a key frame (IDR, parameter sets first).
pub const KEY: u32 = 1;
/// Flag of `hal_stream_encoder_texture`: the same picture as the last one again at a
/// near-lossless QP (a still desktop's refinement).
pub const REFINE: u32 = 2;

/// The text of this thread's last failed stream-encoder op (its first `out.len()` bytes); its
/// length is returned.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_error(out: &mut [u8]) -> u64 {
    LAST_ERROR.with(|e| {
        let e = e.borrow();
        let n = e.len().min(out.len());
        out[..n].copy_from_slice(&e.as_bytes()[..n]);
        e.len() as u64
    })
}

/// A tool without a window (the encoder bench): makes the process's Vulkan device offscreen
/// when nothing made one yet. Never call it before a windowed backend starts (its device would
/// lack the window system's extensions). False when no device could be made.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_offscreen_device() -> bool {
    #[cfg(target_os = "linux")]
    return StreamEncoder::offscreen_device();
    // MediaCodec needs no GPU device
    #[cfg(target_os = "android")]
    return true;
}

/// How many hardware encode engines the GPU has (its encode queues): the tiles a picture is
/// best split into. 0 when this machine has no hardware encoder.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_engines() -> u32 {
    #[cfg(target_os = "linux")]
    return StreamEncoder::engines();
    // one codec a stream (MediaCodec does not split pictures)
    #[cfg(target_os = "android")]
    return 1;
}

/// The largest picture one stream encodes (`flags`: `FULL_CHROMA`, `HEVC`), width in the high 32
/// bits, height in the low; 0 when this machine has no hardware encoder (for that codec). A
/// larger picture is split into tiles (columns and bands) of at most this size.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_max_size(flags: u32) -> u64 {
    #[cfg(target_os = "linux")]
    return StreamEncoder::max_size(codec_of(flags), flags & FULL_CHROMA != 0).map_or(0, |[w, h]| (w as u64) << 32 | h as u64);
    // not asked of the codec
    #[cfg(target_os = "android")]
    {
        let _ = flags;
        return 0;
    }
}

/// Flag of `hal_stream_encoder_open`: 4:4:4 (High 4:4:4 Predictive / Format range extensions);
/// the texture pictures hold full-resolution chroma (see `hal_stream_encoder_texture`).
pub const FULL_CHROMA: u32 = 1;
/// Flag of `hal_stream_encoder_open` and `_max_size`: H.265 instead of H.264.
pub const HEVC: u32 = 2;
/// Flag of `hal_stream_encoder_open`: constant bit rate (else VBR).
pub const CBR: u32 = 4;
/// Flag of `hal_stream_encoder_open`: the encoder's best pictures per bit (ultra-low-latency
/// tuning) instead of its fastest encode (low-latency tuning).
pub const SHARP: u32 = 8;


fn codec_of(flags: u32) -> Codec {
    if flags & HEVC != 0 { Codec::H265 } else { Codec::H264 }
}

/// Opens an H.264 (or H.265: `HEVC`) stream of `width` x `height` pictures (width % 4 == 0,
/// height even) planned for `fps` at `kbps` (0: a rate for the size), encoded on engine
/// `engine`, `flags` (`FULL_CHROMA`, `HEVC`, `CBR`, `SHARP`), at the encoder's quality level `quality`
/// (0 its fastest; more than it has: its best) with a rate control buffer of `buffer_pictures`
/// pictures. Its number, 0 when this machine cannot (the error says why).
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_open(width: u32, height: u32, fps: u32, kbps: u32, engine: u32, flags: u32, quality: u32, buffer_pictures: u32) -> u64 {
    let tuning = Tuning { quality, constant: flags & CBR != 0, buffer_pictures: buffer_pictures.max(1), sharp: flags & SHARP != 0 };
    #[cfg(target_os = "linux")]
    let opened = StreamEncoder::open(width, height, fps, kbps, engine as usize, flags & FULL_CHROMA != 0, codec_of(flags), tuning);
    #[cfg(target_os = "android")]
    let opened = if flags & FULL_CHROMA != 0 || engine != 0 {
        Err("4:4:4 and tiled stream encoding are Linux's".to_string())
    } else {
        StreamEncoder::open(width, height, fps, kbps, codec_of(flags), tuning)
    };
    match opened {
        Ok(encoder) => {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::lock(&ENCODERS).get_or_insert_with(Default::default).insert(id, Arc::new(Mutex::new(Open { encoder, out: Vec::new() })));
            id
        }
        Err(e) => {
            failed(e);
            0
        }
    }
}

/// What the encoder is (codec, sizes, rate, GPU): the first `out.len()` bytes; its length.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_info(encoder: u64, out: &mut [u8]) -> u64 {
    let Some(open) = get(encoder) else { return 0 };
    let open = crate::lock(&open);
    let info = open.encoder.info.as_bytes();
    let n = info.len().min(out.len());
    out[..n].copy_from_slice(&info[..n]);
    info.len() as u64
}

fn encode(encoder: u64, f: impl FnOnce(&mut StreamEncoder) -> Result<Vec<u8>, String>) -> i64 {
    let Some(open) = get(encoder) else {
        failed("unknown stream encoder".into());
        return -1;
    };
    let mut open = crate::lock(&open);
    match f(&mut open.encoder) {
        Ok(bytes) => {
            open.out = bytes;
            open.out.len() as i64
        }
        Err(e) => {
            open.encoder.note_failure(&e);
            failed(e);
            -1
        }
    }
}

/// Encodes the picture in the registered DMA-BUF image `handle` (`hal_dmabuf_register`: YUV
/// packed into BGRA8 texels, `width/4` a row, the luma rows then the interleaved CbCr rows of a
/// `source_height` tall picture: 4:2:0 half as many chroma rows; 4:4:4 one per luma row, each two
/// texture rows), its pixels from column `x0` and row `y0` on (this encoder's size of them: a
/// tile). A picture split into columns packs each column's chroma rows within the column (4:4:4:
/// a column's chroma row is its own two texture rows of `column width/4` texels), so a tile is
/// one rectangle of texels,
/// once the sync_file `sync_fd` has signalled (taken over; -1 when the picture's GPU work is
/// complete). The bitstream's length, -1 when it failed.
#[cfg(target_os = "linux")]
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_texture(encoder: u64, handle: u64, source_height: u32, x0: u32, y0: u32, sync_fd: i32, flags: u32) -> i64 {
    let sync = (sync_fd >= 0).then(|| unsafe { OwnedFd::from_raw_fd(sync_fd) });
    let (image, size) = match copy::registered_image(handle) {
        Ok(image) => image,
        Err(e) => {
            failed(e);
            return -1;
        }
    };
    // `image` stays alive (and its memory bound) until the encode, which waits for the GPU,
    // returns: a release of `handle` meanwhile cannot free what the copy reads
    encode(encoder, |e| {
        if !e.on_device(&image.dev) {
            return Err("the GPU device was replaced: the encoder must be opened again".into());
        }
        e.encode_rows(image.image, size, source_height, x0, y0, sync, flags & KEY != 0, flags & REFINE != 0)
    })
}

/// Encodes tightly packed NV12 bytes (NV24 for a 4:4:4 stream). The bitstream's length, -1 when
/// it failed.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_nv12(encoder: u64, nv12: &[u8], flags: u32) -> i64 {
    encode(encoder, |e| e.encode_nv12(nv12, flags & KEY != 0))
}

/// Takes the last picture's bitstream into `out` (its length, -1 when `out` is short).
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_read(encoder: u64, out: &mut [u8]) -> i64 {
    let Some(open) = get(encoder) else { return -1 };
    let mut open = crate::lock(&open);
    if out.len() < open.out.len() {
        return -1;
    }
    let n = open.out.len();
    out[..n].copy_from_slice(&open.out);
    open.out.clear();
    n as i64
}

/// The last picture's encode on the video queue, from GPU timestamps (ns; 0 when not measured).
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_last_gpu_ns(encoder: u64) -> u64 {
    get(encoder).map_or(0, |open| crate::lock(&open).encoder.last_gpu_ns())
}

/// The rate for the pictures from the next one on (kbit/s).
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_bitrate(encoder: u64, kbps: u32) {
    if let Some(open) = get(encoder) {
        crate::lock(&open).encoder.set_kbps(kbps);
    }
}

/// The highest QP the encoder may use from its next picture on (0: its own range): a quality
/// floor under constant rate (a plain picture then takes more of the rate instead of
/// undershooting it). Vulkan Video only.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_max_qp(encoder: u64, qp: u32) {
    #[cfg(target_os = "linux")]
    if let Some(open) = get(encoder) {
        crate::lock(&open).encoder.set_max_qp(qp.min(51));
    }
    #[cfg(not(target_os = "linux"))]
    let _ = (encoder, qp);
}

#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_close(encoder: u64) {
    let open = crate::lock(&ENCODERS).as_mut().and_then(|e| e.remove(&encoder));
    drop(open);
}

/// The encoder's QP delta map grid into `out` (columns, rows, texel width, texel height (px),
/// least and greatest delta): true when it takes one (Linux, VK_KHR_video_encode_quantization_map).
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_qp_grid(encoder: u64, out: &mut [i32]) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Some(open) = get(encoder) else { return false };
        let grid = crate::lock(&open).encoder.qp_grid();
        let Some(([cols, rows], [tw, th], [min, max])) = grid else { return false };
        if out.len() < 6 {
            return false;
        }
        out[..6].copy_from_slice(&[cols as i32, rows as i32, tw as i32, th as i32, min, max]);
        true
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (encoder, out);
        false
    }
}

/// QP deltas (signed bytes, one per texel of `hal_stream_encoder_qp_grid`, row by row) for the
/// pictures from the next one on: negative spends more bits there, positive fewer.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_qp_map(encoder: u64, deltas: &[u8]) -> bool {
    #[cfg(target_os = "linux")]
    {
        let Some(open) = get(encoder) else { return false };
        // the bytes are the deltas as they lie (i8)
        let deltas: &[i8] = unsafe { std::slice::from_raw_parts(deltas.as_ptr().cast(), deltas.len()) };
        let set = crate::lock(&open).encoder.set_qp_map(deltas);
        set
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = (encoder, deltas);
        false
    }
}
