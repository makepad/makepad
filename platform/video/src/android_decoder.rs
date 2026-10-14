//! Android file decoder: the phone's NDK MediaExtractor + MediaCodec behind the same facade as
//! the AVAssetReader / Media Foundation / GStreamer arms -- a video file in, tightly packed NV12
//! frames and 16-bit PCM out. The codecs live in the HAL host (platform/hal backend/android/media
//! video_file.rs); this side talks to them through the `hal_video_file_*` ops, which is how code
//! the phone's `makepad run` compiles reaches the platform. Also the call ids and result transfer
//! the HEVC still ops share.

use crate::{DecodedAudioChunk, DecodedVideoFrame, VideoFileCodec, VideoFileError, VideoFileInfo};
use std::sync::atomic::{AtomicI64, Ordering};

/// Call ids unique in this process (the HAL keeps a result under its id until taken; a file
/// decoder keeps its open id for its life).
pub(crate) fn next_id() -> i64 {
    static NEXT: AtomicI64 = AtomicI64::new(1);
    NEXT.fetch_add(1, Ordering::Relaxed)
}

/// The result the HAL keeps under `id` for a call that answered `len` (>= 0: the bytes;
/// < 0: -1 - the length of the error text).
pub(crate) fn take(id: i64, len: i64) -> Result<Vec<u8>, VideoFileError> {
    let size = if len >= 0 { len } else { -1 - len } as usize;
    let mut out = vec![0u8; size];
    let got = makepad_hal::hal_media_take(id, &mut out);
    if got < 0 {
        return Err(VideoFileError::new("the platform's result went missing"));
    }
    out.truncate(got as usize);
    if len < 0 {
        return Err(VideoFileError::new(String::from_utf8_lossy(&out).to_string()));
    }
    Ok(out)
}

/// Little-endian fields read off the front of a result.
struct Fields<'a>(&'a [u8]);
impl<'a> Fields<'a> {
    fn bytes(&mut self, n: usize) -> Result<&'a [u8], VideoFileError> {
        if self.0.len() < n {
            return Err(VideoFileError::new("a short result from the platform decoder"));
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Ok(head)
    }
    fn u16(&mut self) -> Result<u16, VideoFileError> {
        Ok(u16::from_le_bytes(self.bytes(2)?.try_into().unwrap()))
    }
    fn u32(&mut self) -> Result<u32, VideoFileError> {
        Ok(u32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, VideoFileError> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
}

pub(crate) struct AndroidVideoFileDecoder {
    id: i64,
    info: VideoFileInfo,
}

impl AndroidVideoFileDecoder {
    pub fn open(path: &str) -> Result<Self, VideoFileError> {
        Self::open_with(path, false)
    }

    pub fn open_audio(path: &str) -> Result<Self, VideoFileError> {
        Self::open_with(path, true)
    }

    fn open_with(path: &str, audio_only: bool) -> Result<Self, VideoFileError> {
        let id = next_id();
        let bytes = take(id, makepad_hal::hal_video_file_open(id, path, audio_only as u32))?;
        let mut f = Fields(&bytes);
        let (width, height, fps_num, fps_den) = (f.u32()?, f.u32()?, f.u32()?, f.u32()?);
        let duration_us = f.i64()?;
        let mime_len = f.u32()? as usize;
        let mime = String::from_utf8_lossy(f.bytes(mime_len)?).to_string();
        let has_audio = f.bytes(1)?[0] != 0;
        let (audio_sample_rate, audio_channels) = (f.u32()?, f.u16()?);
        let (video_codec, video_codec_fourcc) = match mime.as_str() {
            "video/hevc" => (Some(VideoFileCodec::H265), u32::from_be_bytes(*b"hvc1")),
            "video/avc" => (Some(VideoFileCodec::H264), u32::from_be_bytes(*b"avc1")),
            _ => (None, 0),
        };
        let info = VideoFileInfo {
            width,
            height,
            fps_num,
            fps_den,
            duration_100ns: duration_us * 10,
            video_codec,
            video_codec_fourcc,
            has_audio,
            audio_sample_rate,
            audio_channels,
        };
        Ok(Self { id, info })
    }

    pub fn info(&self) -> &VideoFileInfo {
        &self.info
    }

    pub fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>, VideoFileError> {
        let len = makepad_hal::hal_video_file_next_frame(self.id);
        if len == 0 {
            return Ok(None);
        }
        let bytes = take(self.id, len)?;
        let mut f = Fields(&bytes);
        let time_us = f.i64()?;
        let nv12 = f.0.to_vec();
        if nv12.len() != crate::nv12::nv12_frame_size(self.info.width, self.info.height) {
            return Err(VideoFileError::new("a picture of the wrong size from the platform decoder"));
        }
        Ok(Some(DecodedVideoFrame { pts_100ns: time_us * 10, width: self.info.width, height: self.info.height, nv12 }))
    }

    pub fn next_audio(&mut self) -> Result<Option<DecodedAudioChunk>, VideoFileError> {
        let len = makepad_hal::hal_video_file_next_audio(self.id);
        if len == 0 {
            return Ok(None);
        }
        let bytes = take(self.id, len)?;
        let mut f = Fields(&bytes);
        let (time_us, sample_rate, channels) = (f.i64()?, f.u32()?, f.u16()?);
        let samples = f.0.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
        Ok(Some(DecodedAudioChunk { pts_100ns: time_us * 10, sample_rate, channels, samples }))
    }

    pub fn seek_to(&mut self, target_100ns: i64) -> Result<(), VideoFileError> {
        let status = makepad_hal::hal_video_file_seek(self.id, target_100ns.max(0) / 10);
        if status != 0 {
            take(self.id, status)?;
        }
        Ok(())
    }
}

impl Drop for AndroidVideoFileDecoder {
    fn drop(&mut self) {
        makepad_hal::hal_video_file_close(self.id);
    }
}
