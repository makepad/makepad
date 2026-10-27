//! The host API of the hardware stream decoders: what `makepad_video::VideoStreamDecoder` drives
//! on Android (MediaCodec, `backend::android::media::stream_decode`). JIT-compiled code reaches
//! the phone's decoders only through these functions. A decoder is named by a number; a
//! failure's text is the calling thread's last error (`hal_stream_decoder_error`).

#[cfg(target_os = "android")]
use crate::backend::android::media::stream_decode::{Picture, StreamDecoder};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex};

/// Flag of `hal_stream_decoder_open`: H.265 instead of H.264.
pub const HEVC: u32 = 2;
/// Flag of `hal_stream_decoder_open`: pictures stay on the GPU (a picture is read as a texture
/// import ticket, `texture_handle::VIDEO`, released with `hal_stream_decoder_release`).
pub const GPU: u32 = 4;

/// GPU pictures read and not released, by ticket: the decoder's image stays out of its reader
/// (its buffer is not decoded into again) until the ticket is released.
#[cfg(target_os = "android")]
static HELD: Mutex<Option<HashMap<u64, crate::backend::android::media::stream_decode::GpuPicture>>> = Mutex::new(None);

#[cfg(target_os = "android")]
struct Open {
    decoder: StreamDecoder,
    /// Pictures decoded and not read yet, oldest first.
    out: VecDeque<Picture>,
}
#[cfg(not(target_os = "android"))]
struct Open {
    out: VecDeque<()>,
}

static DECODERS: Mutex<Option<HashMap<u64, Arc<Mutex<Open>>>>> = Mutex::new(None);
static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);

fn get(decoder: u64) -> Option<Arc<Mutex<Open>>> {
    crate::lock(&DECODERS).as_ref()?.get(&decoder).cloned()
}

thread_local! {
    static LAST_ERROR: std::cell::RefCell<String> = const { std::cell::RefCell::new(String::new()) };
}

fn failed(error: String) {
    LAST_ERROR.with(|e| *e.borrow_mut() = error);
}

/// The text of this thread's last failed stream-decoder op (its first `out.len()` bytes); its
/// length is returned.
#[makepad_relax_api::relax_api]
pub fn hal_stream_decoder_error(out: &mut [u8]) -> u64 {
    LAST_ERROR.with(|e| {
        let e = e.borrow();
        let n = e.len().min(out.len());
        out[..n].copy_from_slice(&e.as_bytes()[..n]);
        e.len() as u64
    })
}

/// Opens a decoder for a stream of about `width` x `height` pictures (`flags`: `HEVC`). Its
/// number, 0 when this machine cannot (the error says why).
#[makepad_relax_api::relax_api]
pub fn hal_stream_decoder_open(width: u32, height: u32, flags: u32) -> u64 {
    #[cfg(target_os = "android")]
    let opened = StreamDecoder::open(width, height, flags & HEVC != 0, flags & GPU != 0).map(|decoder| Open { decoder, out: VecDeque::new() });
    #[cfg(not(target_os = "android"))]
    let opened: Result<Open, String> = {
        let _ = (width, height, flags);
        Err("the HAL stream decoder is Android's".into())
    };
    match opened {
        Ok(open) => {
            let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            crate::lock(&DECODERS).get_or_insert_with(HashMap::new).insert(id, Arc::new(Mutex::new(open)));
            id
        }
        Err(e) => {
            failed(e);
            0
        }
    }
}

/// The decoder's name (its first `out.len()` bytes); its length.
#[makepad_relax_api::relax_api]
pub fn hal_stream_decoder_name(decoder: u64, out: &mut [u8]) -> u64 {
    let Some(open) = get(decoder) else { return 0 };
    #[cfg(target_os = "android")]
    let name = crate::lock(&open).decoder.info.clone();
    #[cfg(not(target_os = "android"))]
    let name = { let _ = open; String::new() };
    let n = name.len().min(out.len());
    out[..n].copy_from_slice(&name.as_bytes()[..n]);
    name.len() as u64
}

/// Queues one Annex-B access unit with presentation time `pts_us` and collects the pictures
/// decoded, waiting up to `wait_us` for one. The number of pictures waiting to be read, -1 when
/// it failed.
#[makepad_relax_api::relax_api]
pub fn hal_stream_decoder_push(decoder: u64, annex_b: &[u8], pts_us: i64, wait_us: u32) -> i64 {
    let Some(open) = get(decoder) else {
        failed("no such decoder".into());
        return -1;
    };
    let mut open = crate::lock(&open);
    #[cfg(target_os = "android")]
    {
        match open.decoder.push(annex_b, pts_us, std::time::Duration::from_micros(wait_us as u64)) {
            Ok(pictures) => {
                open.out.extend(pictures);
                open.out.len() as i64
            }
            Err(e) => {
                failed(e);
                -1
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (annex_b, pts_us, wait_us, &mut open);
        -1
    }
}

/// The oldest picture waiting: a 24-byte header (width u32, height u32, pts_us i64, ticket u64)
/// then, for a CPU picture (ticket 0), its tightly packed NV12, into `out`. A GPU picture's
/// ticket names it to the texture import (`texture_handle::VIDEO`) until it is released
/// (`hal_stream_decoder_release`). Its length; 0 when none waits, more than `out.len()` when
/// `out` is short (the picture stays).
#[makepad_relax_api::relax_api]
pub fn hal_stream_decoder_read(decoder: u64, out: &mut [u8]) -> u64 {
    let Some(open) = get(decoder) else { return 0 };
    let mut open = crate::lock(&open);
    #[cfg(target_os = "android")]
    {
        let Some(p) = open.out.front() else { return 0 };
        let need = 24 + p.nv12.len() as u64;
        if (out.len() as u64) < need {
            return need;
        }
        let p = open.out.pop_front().unwrap();
        let ticket = match p.gpu {
            Some(gpu) => match crate::backend::vulkan::picture_ticket(gpu.buffer) {
                Ok(ticket) => {
                    crate::lock(&HELD).get_or_insert_with(HashMap::new).insert(ticket, gpu);
                    ticket
                }
                Err(e) => {
                    failed(e);
                    return 0;
                }
            },
            None => 0,
        };
        out[0..4].copy_from_slice(&p.width.to_le_bytes());
        out[4..8].copy_from_slice(&p.height.to_le_bytes());
        out[8..16].copy_from_slice(&p.pts_us.to_le_bytes());
        out[16..24].copy_from_slice(&ticket.to_le_bytes());
        out[24..24 + p.nv12.len()].copy_from_slice(&p.nv12);
        need
    }
    #[cfg(not(target_os = "android"))]
    {
        let _ = (out, open.out.pop_front());
        0
    }
}

/// A GPU picture is done with (every frame that drew it finished): its image goes back to the
/// decoder.
#[makepad_relax_api::relax_api]
pub fn hal_stream_decoder_release(ticket: u64) {
    #[cfg(target_os = "android")]
    {
        let held = crate::lock(&HELD).as_mut().and_then(|h| h.remove(&ticket));
        crate::backend::vulkan::release_picture_ticket(ticket);
        drop(held);
    }
    #[cfg(not(target_os = "android"))]
    let _ = ticket;
}

#[makepad_relax_api::relax_api]
pub fn hal_stream_decoder_close(decoder: u64) {
    let open = crate::lock(&DECODERS).as_mut().and_then(|d| d.remove(&decoder));
    drop(open);
}
