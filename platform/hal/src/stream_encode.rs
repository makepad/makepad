//! The host API of the Linux hardware stream encoder (`backend::vulkan::video_encode`): what
//! `makepad_video::VideoStreamEncoder` drives on Linux. JIT-compiled code (makepad run) reaches
//! the process's Vulkan device only through these functions, like the DMA-BUF ops
//! (`crate::dmabuf`). An encoder is named by a number; a failure's text is the calling thread's
//! last error (`hal_stream_encoder_error`). Each encode call is synchronous: it returns once
//! the picture's bitstream is ready (`hal_stream_encoder_read` takes it).

use crate::backend::vulkan::{share::copy, video_encode::StreamEncoder};
use std::collections::HashMap;
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

/// How many hardware encode engines the GPU has (its encode queues): the tiles a picture is
/// best split into. 0 when this machine has no hardware encoder.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_engines() -> u32 {
    StreamEncoder::engines()
}

/// The largest picture one stream encodes (`flags`: `FULL_CHROMA`), width in the high 32 bits,
/// height in the low; 0 when this machine has no hardware encoder. A larger picture is split
/// into tiles (columns and bands) of at most this size.
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_max_size(flags: u32) -> u64 {
    StreamEncoder::max_size(flags & FULL_CHROMA != 0).map_or(0, |[w, h]| (w as u64) << 32 | h as u64)
}

/// Flag of `hal_stream_encoder_open`: 4:4:4 (High 4:4:4 Predictive); the texture pictures hold
/// full-resolution chroma (see `hal_stream_encoder_texture`).
pub const FULL_CHROMA: u32 = 1;

/// Opens an H.264 stream of `width` x `height` pictures (width % 4 == 0, height even) planned
/// for `fps` at `kbps` (0: a rate for the size), encoded on engine `engine`, `flags`
/// (`FULL_CHROMA`). Its number, 0 when this machine cannot (the error says why).
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_open(width: u32, height: u32, fps: u32, kbps: u32, engine: u32, flags: u32) -> u64 {
    match StreamEncoder::open(width, height, fps, kbps, engine as usize, flags & FULL_CHROMA != 0) {
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

/// Encodes tightly packed NV12 bytes. The bitstream's length, -1 when it failed.
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

/// The rate for the pictures from the next one on (kbit/s).
#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_bitrate(encoder: u64, kbps: u32) {
    if let Some(open) = get(encoder) {
        crate::lock(&open).encoder.set_kbps(kbps);
    }
}

#[makepad_relax_api::relax_api]
pub fn hal_stream_encoder_close(encoder: u64) {
    let open = crate::lock(&ENCODERS).as_mut().and_then(|e| e.remove(&encoder));
    drop(open);
}
