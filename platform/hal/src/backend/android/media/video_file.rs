//! makepad_video's file decoder on the phone: a video file (anything the platform demuxes, mp4 /
//! mov / mkv / webm with the codecs the phone has) read by NDK MediaExtractor, its picture decoded
//! by MediaCodec into an ImageReader (tightly packed NV12 out), its sound decoded to 16-bit PCM.
//! One extractor per stream, so pictures and sound are pulled independently, the way the other
//! platforms' decoders behind `makepad_video::VideoFileDecoder` hand them out. The program
//! reaches it through the `hal_video_file_*` ops.
use super::mediacodec::{checked, ndk, BufferInfo, Codec, Extractor, Format, Reader, FLAG_END_OF_STREAM, OUTPUT_FORMAT_CHANGED};
use std::ffi::CString;
use std::time::{Duration, Instant};

/// How long a pull may go without the codec making progress.
const STALL: Duration = Duration::from_secs(5);

/// What `VideoFileInfo` is made of.
#[derive(Default, Clone)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub fps_num: u32,
    pub fps_den: u32,
    pub duration_us: i64,
    /// The picture's MIME type (`video/hevc`, `video/avc`, ...).
    pub video_mime: Option<String>,
    pub has_audio: bool,
    pub sample_rate: u32,
    pub channels: u16,
    /// MediaFormat KEY_COLOR_STANDARD / KEY_COLOR_RANGE / KEY_COLOR_TRANSFER of the picture
    /// track (0: the file does not say).
    pub color_standard: i32,
    pub color_range: i32,
    pub color_transfer: i32,
}

/// The colour keys of a MediaFormat; 0 for one it does not carry.
fn color_keys(format: &Format) -> (i32, i32, i32) {
    let get = |key: &std::ffi::CStr| format.get_int(key).unwrap_or(0);
    (get(c"color-standard"), get(c"color-range"), get(c"color-transfer"))
}

/// One stream: its demuxer, its codec and where the two are.
struct Stream {
    extractor: Extractor,
    codec: Codec,
    input_done: bool,
    output_done: bool,
    /// KEY_COLOR_RANGE of the codec's output format (0: not stated).
    output_range: i32,
}

impl Stream {
    fn open(path: &str, prefix: &str, surface: *mut std::ffi::c_void) -> Result<Option<(Stream, Format)>, String> {
        let extractor = Extractor::open(path)?;
        let Some((index, format, mime)) = (0..extractor.track_count()).find_map(|i| {
            let format = extractor.track_format(i);
            let mime = format.get_string(c"mime")?;
            mime.starts_with(prefix).then_some((i, format, mime))
        }) else {
            return Ok(None);
        };
        extractor.select(index)?;
        let mime_c = CString::new(mime.clone()).map_err(|_| "bad MIME type".to_string())?;
        let codec = Codec::decoder(&mime_c)?;
        unsafe {
            checked(ndk::AMediaCodec_configure(codec.0, format.0, surface, std::ptr::null_mut(), 0), &format!("configure the {mime} decoder"))?;
            checked(ndk::AMediaCodec_start(codec.0), &format!("start the {mime} decoder"))?;
        }
        Ok(Some((Stream { extractor, codec, input_done: false, output_done: false, output_range: 0 }, format)))
    }

    /// Hand the codec every sample it has room for.
    fn feed(&mut self) -> Result<(), String> {
        while !self.input_done {
            let index = unsafe { ndk::AMediaCodec_dequeueInputBuffer(self.codec.0, 0) };
            if index < 0 {
                return Ok(());
            }
            let mut capacity = 0usize;
            let buffer = unsafe { ndk::AMediaCodec_getInputBuffer(self.codec.0, index as usize, &mut capacity) };
            if buffer.is_null() {
                return Err("the decoder gave no input memory".into());
            }
            let input = unsafe { std::slice::from_raw_parts_mut(buffer, capacity) };
            let status = match self.extractor.read(input) {
                Some((size, time)) => unsafe { ndk::AMediaCodec_queueInputBuffer(self.codec.0, index as usize, 0, size, time.max(0) as u64, 0) },
                None => {
                    self.input_done = true;
                    unsafe { ndk::AMediaCodec_queueInputBuffer(self.codec.0, index as usize, 0, 0, 0, FLAG_END_OF_STREAM) }
                }
            };
            checked(status, "queue a sample")?;
        }
        Ok(())
    }

    fn seek(&mut self, time_us: i64) -> Result<(), String> {
        self.extractor.seek(time_us)?;
        checked(unsafe { ndk::AMediaCodec_flush(self.codec.0) }, "flush")?;
        self.input_done = false;
        self.output_done = false;
        Ok(())
    }
}

/// An open file.
pub struct FileDecoder {
    info: Info,
    // Field order is drop order: the codec stops before the reader it renders into goes.
    video: Option<Stream>,
    reader: Option<Reader>,
    audio: Option<Stream>,
}

impl FileDecoder {
    /// Open `path` for its picture and sound, or (`audio_only`) for its sound alone.
    pub fn open(path: &str, audio_only: bool) -> Result<FileDecoder, String> {
        let mut info = Info { fps_num: 30, fps_den: 1, ..Info::default() };
        let (mut video, mut reader) = (None, None);
        if !audio_only {
            // The picture's size first: the reader the decoder renders into has it.
            let probe = Extractor::open(path)?;
            let format = (0..probe.track_count())
                .map(|i| probe.track_format(i))
                .find(|f| f.get_string(c"mime").is_some_and(|m| m.starts_with("video/")))
                .ok_or_else(|| format!("{path} has no picture"))?;
            let (w, h) = (format.get_int(c"width").unwrap_or(0), format.get_int(c"height").unwrap_or(0));
            if w <= 1 || h <= 1 {
                return Err(format!("{path}: picture of {w}x{h}"));
            }
            info.width = w as u32 & !1;
            info.height = h as u32 & !1;
            if let Some(fps) = format.get_int(c"frame-rate").filter(|f| *f > 0) {
                info.fps_num = fps as u32;
            }
            let (r, window) = Reader::yuv(info.width, info.height, 3)?;
            let (stream, format) = Stream::open(path, "video/", window)?.ok_or_else(|| format!("{path} has no picture"))?;
            info.video_mime = format.get_string(c"mime");
            (info.color_standard, info.color_range, info.color_transfer) = color_keys(&format);
            info.duration_us = format.get_long(c"durationUs").unwrap_or(0);
            video = Some(stream);
            reader = Some(r);
        }
        let audio = match Stream::open(path, "audio/", std::ptr::null_mut()) {
            Ok(Some((stream, format))) => {
                info.has_audio = true;
                info.sample_rate = format.get_int(c"sample-rate").unwrap_or(0).max(0) as u32;
                info.channels = format.get_int(c"channel-count").unwrap_or(0).max(0) as u16;
                info.duration_us = info.duration_us.max(format.get_long(c"durationUs").unwrap_or(0));
                Some(stream)
            }
            Ok(None) if audio_only => return Err(format!("{path} has no sound")),
            Ok(None) => None,
            // A sound track the phone cannot decode leaves the picture: the file plays silent.
            Err(e) if !audio_only => {
                eprintln!("[hal-android-video-file] {path}: sound not decoded: {e}");
                None
            }
            Err(e) => return Err(e),
        };
        Ok(FileDecoder { info, video, reader, audio })
    }

    pub fn info(&self) -> &Info {
        &self.info
    }

    /// The next picture: its time (microseconds), the KEY_COLOR_RANGE the decoder delivers it in
    /// (the output format's, else the track's, 0 when neither says) and NV12; `None` at the end.
    pub fn next_frame(&mut self) -> Result<Option<(i64, i32, Vec<u8>)>, String> {
        let (Some(stream), Some(reader)) = (self.video.as_mut(), self.reader.as_ref()) else { return Ok(None) };
        let mut progress = Instant::now();
        while !stream.output_done {
            stream.feed()?;
            let mut out = BufferInfo::default();
            let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(stream.codec.0, &mut out, 5_000) };
            if index >= 0 {
                let end = out.flags & FLAG_END_OF_STREAM != 0;
                // An end-of-stream buffer with nothing in it carries no picture.
                let picture = !(end && out.size == 0);
                unsafe { ndk::AMediaCodec_releaseOutputBuffer(stream.codec.0, index as usize, picture) };
                stream.output_done = end;
                if picture {
                    let nv12 = reader.next_nv12(self.info.width, self.info.height, STALL)?;
                    let range = if stream.output_range != 0 { stream.output_range } else { self.info.color_range };
                    return Ok(Some((out.presentation_time_us, range, nv12)));
                }
                progress = Instant::now();
            } else if index == OUTPUT_FORMAT_CHANGED {
                let format = Format(unsafe { ndk::AMediaCodec_getOutputFormat(stream.codec.0) });
                stream.output_range = color_keys(&format).1;
                progress = Instant::now();
            } else if progress.elapsed() > STALL {
                return Err("the picture decoder stalled".into());
            }
        }
        Ok(None)
    }

    /// The next chunk of sound: its time (microseconds), rate, channels and interleaved 16-bit
    /// samples; `None` at the end (or when the file has none).
    pub fn next_audio(&mut self) -> Result<Option<(i64, u32, u16, Vec<i16>)>, String> {
        let Some(stream) = self.audio.as_mut() else { return Ok(None) };
        let mut progress = Instant::now();
        while !stream.output_done {
            stream.feed()?;
            let mut out = BufferInfo::default();
            let index = unsafe { ndk::AMediaCodec_dequeueOutputBuffer(stream.codec.0, &mut out, 5_000) };
            if index >= 0 {
                let mut capacity = 0usize;
                let buffer = unsafe { ndk::AMediaCodec_getOutputBuffer(stream.codec.0, index as usize, &mut capacity) };
                let mut samples = Vec::new();
                if !buffer.is_null() && out.size > 0 && (out.offset + out.size) as usize <= capacity {
                    let bytes = unsafe { std::slice::from_raw_parts(buffer.add(out.offset as usize), out.size as usize) };
                    samples = bytes.chunks_exact(2).map(|b| i16::from_le_bytes([b[0], b[1]])).collect();
                }
                unsafe { ndk::AMediaCodec_releaseOutputBuffer(stream.codec.0, index as usize, false) };
                stream.output_done = out.flags & FLAG_END_OF_STREAM != 0;
                if !samples.is_empty() {
                    return Ok(Some((out.presentation_time_us, self.info.sample_rate, self.info.channels, samples)));
                }
                progress = Instant::now();
            } else if index == OUTPUT_FORMAT_CHANGED {
                let format = Format(unsafe { ndk::AMediaCodec_getOutputFormat(stream.codec.0) });
                if let Some(rate) = format.get_int(c"sample-rate").filter(|r| *r > 0) {
                    self.info.sample_rate = rate as u32;
                }
                if let Some(channels) = format.get_int(c"channel-count").filter(|c| *c > 0) {
                    self.info.channels = channels as u16;
                }
                progress = Instant::now();
            } else if progress.elapsed() > STALL {
                return Err("the sound decoder stalled".into());
            }
        }
        Ok(None)
    }

    /// To the sync sample at or before `time_us` in every stream (the caller decodes forward).
    pub fn seek(&mut self, time_us: i64) -> Result<(), String> {
        if let Some(stream) = self.video.as_mut() {
            stream.seek(time_us)?;
            // Pictures rendered before the flush are the old position's.
            if let Some(reader) = &self.reader {
                while reader.next_nv12(self.info.width, self.info.height, Duration::ZERO).is_ok() {}
            }
        }
        if let Some(stream) = self.audio.as_mut() {
            stream.seek(time_us)?;
        }
        Ok(())
    }
}
