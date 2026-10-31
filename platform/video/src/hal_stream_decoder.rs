//! Android backend of [`crate::VideoStreamDecoder`]: the HAL's hardware stream decoder
//! (`makepad_hal::stream_decode`, the phone's MediaCodec). CPU NV12 out, or (`new_with` gpu)
//! pictures that stay on the GPU (`DecodedFrame::gpu`: the decoder's hardware buffer). The
//! decoder is opened for the size hint ([`crate::VideoStreamDecoder::with_size_hint`]; the
//! stream's own size wins), and a picture comes back with the presentation time its access
//! unit went in with.

use crate::stream_decoder::{DecodedFrame, DecodedPicture, StreamVideoCodec};
use crate::{VideoColor, VideoFileError};
use makepad_hal::stream_decode as hal;

fn last_error() -> VideoFileError {
    let mut buf = vec![0u8; 1024];
    let n = hal::hal_stream_decoder_error(&mut buf) as usize;
    buf.truncate(n.min(1024));
    VideoFileError::new(String::from_utf8_lossy(&buf).into_owned())
}

/// How long a push waits for its picture (a low-latency decoder answers in a few ms).
const PICTURE_WAIT_US: u32 = 30_000;

pub(crate) struct HalStreamDecoder {
    id: u64,
    out: Vec<u8>,
}

impl HalStreamDecoder {
    pub fn new(codec: StreamVideoCodec, size: [u32; 2]) -> Result<Self, VideoFileError> {
        Self::new_with(codec, size, false)
    }

    /// `gpu`: pictures stay in the decoder's GPU buffers (`DecodedFrame::gpu`), else CPU NV12.
    pub fn new_with(codec: StreamVideoCodec, size: [u32; 2], gpu: bool) -> Result<Self, VideoFileError> {
        let flags = if gpu { hal::GPU } else { 0 } | if codec == StreamVideoCodec::Hevc { hal::HEVC } else { 0 };
        let id = hal::hal_stream_decoder_open(size[0], size[1], flags);
        if id == 0 {
            return Err(last_error());
        }
        Ok(Self { id, out: Vec::new() })
    }

    pub fn name(&self) -> String {
        let mut buf = vec![0u8; 256];
        let n = hal::hal_stream_decoder_name(self.id, &mut buf) as usize;
        buf.truncate(n.min(256));
        String::from_utf8_lossy(&buf).into_owned()
    }

    pub fn push_packet(&mut self, annex_b: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>, VideoFileError> {
        if hal::hal_stream_decoder_push(self.id, annex_b, pts_100ns / 10, PICTURE_WAIT_US) < 0 {
            return Err(last_error());
        }
        self.take()
    }

    fn take(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        let mut frames = Vec::new();
        loop {
            let n = hal::hal_stream_decoder_read(self.id, &mut self.out) as usize;
            if n == 0 {
                return Ok(frames);
            }
            if n > self.out.len() {
                self.out.resize(n, 0);
                continue;
            }
            let b = &self.out;
            let width = u32::from_le_bytes(b[0..4].try_into().unwrap());
            let height = u32::from_le_bytes(b[4..8].try_into().unwrap());
            let pts_us = i64::from_le_bytes(b[8..16].try_into().unwrap());
            let ticket = u64::from_le_bytes(b[16..24].try_into().unwrap());
            let gpu = (ticket != 0).then(|| DecodedPicture { handle: ticket, full_chroma: false, _keep: std::sync::Arc::new(Held(ticket)) });
            let nv12 = if ticket != 0 { Vec::new() } else { b[24..n].to_vec() };
            frames.push(DecodedFrame { width, height, nv12, pts_100ns: pts_us * 10, gpu, color: VideoColor::BT709 });
        }
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        self.take()
    }
}

/// A GPU picture's ticket, released (its image back to the decoder) when the last clone of its
/// `DecodedPicture` is dropped.
struct Held(u64);
impl Drop for Held {
    fn drop(&mut self) {
        hal::hal_stream_decoder_release(self.0);
    }
}

impl Drop for HalStreamDecoder {
    fn drop(&mut self) {
        hal::hal_stream_decoder_close(self.id);
    }
}
