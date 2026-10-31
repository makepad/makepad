//! Android file decoder: the phone's NDK MediaExtractor + MediaCodec behind the same facade as
//! the AVAssetReader / Media Foundation / GStreamer arms -- a video file in, tightly packed NV12
//! frames and 16-bit PCM out. The codecs live in the HAL host (platform/hal backend/android/media
//! video_file.rs); this side talks to them through the `hal_video_file_*` ops, which is how code
//! the phone's `makepad run` compiles reaches the platform. Also the call ids and result transfer
//! the HEVC still ops share.

use crate::{DecodedAudioChunk, DecodedVideoFrame, VideoColor, VideoFileCodec, VideoFileError, VideoFileInfo, VideoMatrix, VideoPrimaries, VideoRange, VideoTransfer};
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
    fn i32(&mut self) -> Result<i32, VideoFileError> {
        Ok(i32::from_le_bytes(self.bytes(4)?.try_into().unwrap()))
    }
    fn i64(&mut self) -> Result<i64, VideoFileError> {
        Ok(i64::from_le_bytes(self.bytes(8)?.try_into().unwrap()))
    }
}

/// MediaFormat KEY_COLOR_STANDARD values (MediaFormat.COLOR_STANDARD_*).
const STANDARD_BT709: i32 = 1;
const STANDARD_BT601_PAL: i32 = 2;
const STANDARD_BT601_NTSC: i32 = 4;
const STANDARD_BT2020: i32 = 6;
/// KEY_COLOR_RANGE values (COLOR_RANGE_*).
const RANGE_FULL: i32 = 1;
const RANGE_LIMITED: i32 = 2;
/// KEY_COLOR_TRANSFER values (COLOR_TRANSFER_*).
const TRANSFER_ST2084: i32 = 6;
const TRANSFER_HLG: i32 = 7;

fn range_from_android(range: i32, stream: VideoRange) -> VideoRange {
    match range {
        RANGE_FULL => VideoRange::Full,
        RANGE_LIMITED => VideoRange::Limited,
        _ => stream,
    }
}

/// The colour a MediaFormat's keys name (0 or unknown: guessed from the size).
fn color_from_android(standard: i32, range: i32, transfer: i32, width: u32, height: u32) -> VideoColor {
    let guess = VideoColor::guess(width, height);
    let (matrix, primaries) = match standard {
        STANDARD_BT709 => (VideoMatrix::Bt709, VideoPrimaries::Bt709),
        STANDARD_BT601_PAL => (VideoMatrix::Bt601, VideoPrimaries::Bt470bg),
        STANDARD_BT601_NTSC => (VideoMatrix::Bt601, VideoPrimaries::Smpte170m),
        STANDARD_BT2020 => (VideoMatrix::Bt2020, VideoPrimaries::Bt2020),
        _ => (guess.matrix, guess.primaries),
    };
    let transfer = match transfer {
        TRANSFER_ST2084 => VideoTransfer::Pq,
        TRANSFER_HLG => VideoTransfer::Hlg,
        // linear (1), SDR video (3), sRGB (2), gamma 2.2 (4), unknown
        _ => VideoTransfer::Sdr,
    };
    VideoColor { matrix, range: range_from_android(range, guess.range), primaries, transfer }
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
        let (standard, range, transfer) = (f.i32()?, f.i32()?, f.i32()?);
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
            color: color_from_android(standard, range, transfer, width, height),
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
        let (time_us, range) = (f.i64()?, f.i32()?);
        let nv12 = f.0.to_vec();
        if nv12.len() != crate::nv12::nv12_frame_size(self.info.width, self.info.height) {
            return Err(VideoFileError::new("a picture of the wrong size from the platform decoder"));
        }
        let color = self.info.color.with_range(range_from_android(range, self.info.color.range));
        Ok(Some(DecodedVideoFrame { pts_100ns: time_us * 10, width: self.info.width, height: self.info.height, nv12, color }))
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn android_keys_map_to_video_color() {
        let c = color_from_android(STANDARD_BT709, RANGE_LIMITED, 3, 1920, 1080);
        assert_eq!(c, VideoColor::BT709);
        let c = color_from_android(STANDARD_BT601_PAL, RANGE_FULL, 3, 720, 576);
        assert_eq!((c.matrix, c.primaries, c.range), (VideoMatrix::Bt601, VideoPrimaries::Bt470bg, VideoRange::Full));
        let c = color_from_android(STANDARD_BT601_NTSC, 0, 0, 640, 480);
        assert_eq!((c.primaries, c.range), (VideoPrimaries::Smpte170m, VideoRange::Limited));
        let c = color_from_android(STANDARD_BT2020, RANGE_LIMITED, TRANSFER_ST2084, 3840, 2160);
        assert_eq!((c.matrix, c.transfer), (VideoMatrix::Bt2020, VideoTransfer::Pq));
        assert_eq!(color_from_android(STANDARD_BT2020, 0, TRANSFER_HLG, 3840, 2160).transfer, VideoTransfer::Hlg);
        assert_eq!(color_from_android(0, 0, 0, 1280, 720), VideoColor::BT709);
        // the delivered range overrides the stream's; 0 keeps it
        assert_eq!(range_from_android(RANGE_FULL, VideoRange::Limited), VideoRange::Full);
        assert_eq!(range_from_android(0, VideoRange::Full), VideoRange::Full);
    }
}
