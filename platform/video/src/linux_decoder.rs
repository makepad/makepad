//! Linux file decoder: GStreamer `uridecodebin` behind the same facade as
//! the AVAssetReader / Media Foundation arms — an mp4 (or anything the
//! installed plugins demux, `gstreamer1.0-libav` included) in, tightly
//! packed NV12 frames and interleaved 16-bit PCM audio out. Picture and
//! sound use independent pipelines, so reading either cannot block the
//! other. `open_audio` negotiates only sound, including in picture-less files.
//!
//! `info()` does not name the compressed video codec. Frames whose
//! negotiated NV12 layout is not tightly packed are refused loudly
//! (never silently sheared), as are incomplete interleaved PCM frames.
//!
//! The audio implementation has been source-reviewed only; Linux build
//! and runtime verification are pending.

use crate::linux_gst_sys::*;
use crate::nv12;
use crate::{DecodedAudioChunk, DecodedVideoFrame, VideoColor, VideoFileError, VideoFileInfo};
use std::ffi::{c_void, CString};

pub(crate) struct LinuxVideoFileDecoder {
    gst: &'static LibGst,
    pipeline: *mut GstElement,
    appsink: *mut GstElement,
    bus: *mut GstBus,
    audio_pipeline: *mut GstElement,
    audio_sink: *mut GstElement,
    audio_bus: *mut GstBus,
    /// Owned first chunk; its duplicate in the PLAYING queue is consumed at open.
    audio_preroll: *mut GstSample,
    info: VideoFileInfo,
    /// The colour of the delivered bytes (the negotiated caps' colorimetry).
    frame_color: VideoColor,
}

unsafe impl Send for LinuxVideoFileDecoder {}

/// Pull `key=(type)value` out of a caps string like
/// `video/x-raw, format=(string)NV12, width=(int)256, ...`.
pub(crate) fn caps_field<'a>(caps: &'a str, key: &str) -> Option<&'a str> {
    for part in caps.split(',') {
        let part = part.trim();
        let Some(rest) = part.strip_prefix(key) else { continue };
        let Some(rest) = rest.trim_start().strip_prefix('=') else { continue };
        let rest = rest.trim_start();
        // Skip an optional "(type)" tag.
        let rest = if let Some(stripped) = rest.strip_prefix('(') {
            stripped.split_once(')').map(|(_, v)| v).unwrap_or(rest)
        } else {
            rest
        };
        return Some(rest.trim().trim_end_matches(';'));
    }
    None
}

/// GStreamer colorimetry presets as `range:matrix:transfer:primaries`
/// (gst-plugins-base video-color.c, the `gst_video_colorimetry_to_string`
/// table). Case-insensitive; `jpeg` is the legacy name of `1:4:7:1`.
fn colorimetry_preset(name: &str) -> Option<&'static str> {
    Some(match name {
        "bt709" => "2:3:5:1",
        "bt601" => "2:4:5:4",
        "smpte240m" => "2:5:6:5",
        "srgb" => "1:1:7:1",
        "jpeg" => "1:4:7:1",
        "bt2020" => "2:6:11:7",
        "bt2020-10" => "2:6:13:7",
        "bt2100-pq" => "2:6:14:7",
        "bt2100-hlg" => "2:6:15:7",
        _ => return None,
    })
}

/// A caps `colorimetry` string (a preset name or the 4-part numeric form)
/// to a [`VideoColor`]. Numeric values are the gst-video enums:
///
/// * GstVideoColorRange: 0 unknown, 1 = 0_255 (full), 2 = 16_235 (limited)
/// * GstVideoColorMatrix: 0 unknown, 1 RGB, 2 FCC, 3 BT709, 4 BT601,
///   5 SMPTE240M, 6 BT2020
/// * GstVideoTransferFunction: 0 unknown, 1..4 gamma 1.0/1.8/2.0/2.2,
///   5 BT709, 6 SMPTE240M, 7 SRGB, 8 gamma 2.8, 9 LOG100, 10 LOG316,
///   11 BT2020_12, 12 ADOBERGB, 13 BT2020_10, 14 SMPTE2084 (PQ),
///   15 ARIB_STD_B67 (HLG), 16 BT601
/// * GstVideoColorPrimaries: 0 unknown, 1 BT709, 2 BT470M, 3 BT470BG,
///   4 SMPTE170M, 5 SMPTE240M, 6 FILM, 7 BT2020, 8 ADOBERGB, 9 SMPTEST428,
///   10 SMPTERP431, 11 SMPTEEG432, 12 EBU3213
///
/// Anything unknown or missing falls back as [`VideoColor::from_h273`] does.
pub(crate) fn colorimetry_to_color(colorimetry: &str, width: u32, height: u32) -> VideoColor {
    let lower = colorimetry.trim().trim_matches('"').to_ascii_lowercase();
    let numeric = colorimetry_preset(&lower).unwrap_or(&lower);
    let mut parts = numeric.split(':').map(|p| p.trim().parse::<u32>().ok());
    let (range, matrix, transfer, primaries) = match (parts.next(), parts.next(), parts.next(), parts.next(), parts.next()) {
        (Some(Some(r)), Some(Some(m)), Some(Some(t)), Some(Some(p)), None) => (r, m, t, p),
        _ => (0, 0, 0, 0),
    };
    let matrix = match matrix {
        3 => 1,
        4 => 6,
        5 => 7,
        6 => 9,
        _ => 2,
    };
    let transfer = match transfer {
        5 | 16 => 1,
        6 => 7,
        7 => 13,
        11 => 15,
        13 => 14,
        14 => 16,
        15 => 18,
        _ => 2,
    };
    let primaries = match primaries {
        1 => 1,
        3 => 5,
        4 | 5 => 6,
        7 => 9,
        _ => 2,
    };
    VideoColor::from_h273(primaries, transfer, matrix, range == 1, width, height)
}

impl LinuxVideoFileDecoder {
    pub fn open_audio(path: &str) -> Result<Self, VideoFileError> {
        Self::open_file(path, false)
    }

    pub fn open(path: &str) -> Result<Self, VideoFileError> {
        Self::open_file(path, true)
    }

    fn open_file(path: &str, want_video: bool) -> Result<Self, VideoFileError> {
        let gst = LibGst::get().ok_or_else(|| {
            VideoFileError::new(
                "GStreamer runtime not found (apt-get install gstreamer1.0-plugins-base \
                 gstreamer1.0-plugins-good gstreamer1.0-libav)",
            )
        })?;
        unsafe {
            let path_c = CString::new(path)
                .map_err(|_| VideoFileError::new("path contains NUL"))?;
            let mut err: *mut GError = std::ptr::null_mut();
            let uri_c = (gst.gst_filename_to_uri)(path_c.as_ptr(), &mut err);
            if uri_c.is_null() {
                if !err.is_null() {
                    (gst.g_error_free)(err);
                }
                return Err(VideoFileError::new(format!("bad media path: {path}")));
            }
            let uri = std::ffi::CStr::from_ptr(uri_c).to_string_lossy().into_owned();
            (gst.g_free)(uri_c as *mut c_void);
            let mut decoder = Self {
                gst,
                pipeline: std::ptr::null_mut(),
                appsink: std::ptr::null_mut(),
                bus: std::ptr::null_mut(),
                audio_pipeline: std::ptr::null_mut(),
                audio_sink: std::ptr::null_mut(),
                audio_bus: std::ptr::null_mut(),
                audio_preroll: std::ptr::null_mut(),
                info: VideoFileInfo {
                    width: 0,
                    height: 0,
                    fps_num: 0,
                    fps_den: 1,
                    duration_100ns: 0,
                    video_codec: None,
                    video_codec_fourcc: 0,
                    has_audio: false,
                    audio_sample_rate: 0,
                    audio_channels: 0,
                    color: VideoColor::BT709,
                },
                frame_color: VideoColor::BT709,
            };
            if want_video {
                decoder.open_video_pipeline(&uri)?;
            }
            if let Err(error) = decoder.open_audio_pipeline(&uri) {
                if !want_video {
                    return Err(error);
                }
                // Audio is optional for the picture door. In particular, a
                // failed delayed audio link must not keep a pipeline alive.
                decoder.close_audio_pipeline();
            }
            Ok(decoder)
        }
    }

    fn open_video_pipeline(&mut self, uri: &str) -> Result<(), VideoFileError> {
        let gst = self.gst;
        unsafe {
            // One queued frame is enough for pull decoding. A caller reading
            // only audio leaves this pipeline back-pressured, without drops
            // or accumulating the entire decoded picture track in memory.
            let launch = format!(
                "uridecodebin uri=\"{uri}\" ! queue ! videoconvert ! \
                 video/x-raw,format=NV12 ! appsink name=out sync=false max-buffers=1"
            );
            self.pipeline = self.parse_pipeline(&launch)?;
            let name = CString::new("out").unwrap();
            self.appsink = (gst.gst_bin_get_by_name)(self.pipeline, name.as_ptr());
            if self.appsink.is_null() {
                return Err(VideoFileError::new("decoder pipeline has no appsink"));
            }
            self.bus = (gst.gst_element_get_bus)(self.pipeline);
            if self.bus.is_null() {
                return Err(VideoFileError::new("decoder pipeline has no bus"));
            }
            if (gst.gst_element_set_state)(self.pipeline, GST_STATE_PAUSED)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(self.bus)
                    .unwrap_or_else(|| "pipeline refused PAUSED".to_string());
                return Err(VideoFileError::new(format!(
                    "no video stream in file (decoder start: {text})"
                )));
            }
            // Preroll: the first sample's caps carry the real geometry.
            let preroll = self.pull_preroll(self.appsink, self.bus, "video")?;
            let caps_text = self.sample_caps(preroll);
            (gst.gst_mini_object_unref)(preroll as *mut c_void);
            let width: u32 = caps_field(&caps_text, "width")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let height: u32 = caps_field(&caps_text, "height")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let (fps_num, fps_den) = caps_field(&caps_text, "framerate")
                .and_then(|v| v.split_once('/'))
                .and_then(|(n, d)| Some((n.trim().parse().ok()?, d.trim().parse().ok()?)))
                .unwrap_or((0, 1));
            if width == 0 || height == 0 {
                return Err(VideoFileError::new(format!(
                    "no video stream in file (decoder negotiated no geometry; caps: {caps_text})"
                )));
            }
            // videoconvert (when the decoder's own format is not NV12) keeps the
            // colorimetry: the capsfilter after it names only the format. So the
            // caps here are the stream's colour in the range delivered.
            let color = colorimetry_to_color(
                caps_field(&caps_text, "colorimetry").unwrap_or(""),
                width,
                height,
            );
            let mut duration_ns: i64 = 0;
            let _ =
                (gst.gst_element_query_duration)(self.pipeline, GST_FORMAT_TIME, &mut duration_ns);
            if (gst.gst_element_set_state)(self.pipeline, GST_STATE_PLAYING)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(self.bus)
                    .unwrap_or_else(|| "pipeline refused PLAYING".to_string());
                return Err(VideoFileError::new(format!("decoder play: {text}")));
            }
            self.info.width = width;
            self.info.height = height;
            self.info.fps_num = fps_num;
            self.info.fps_den = fps_den;
            self.info.duration_100ns = duration_ns.max(0) / 100;
            self.info.color = color;
            self.frame_color = color;
            Ok(())
        }
    }

    fn open_audio_pipeline(&mut self, uri: &str) -> Result<(), VideoFileError> {
        let gst = self.gst;
        unsafe {
            let launch = format!(
                "uridecodebin uri=\"{uri}\" caps=audio/x-raw ! audioconvert ! audioresample ! \
                 audio/x-raw,format=S16LE,layout=interleaved ! \
                 appsink name=aout sync=false async=false max-buffers=1"
            );
            self.audio_pipeline = self.parse_pipeline(&launch)?;
            let name = CString::new("aout").unwrap();
            self.audio_sink = (gst.gst_bin_get_by_name)(self.audio_pipeline, name.as_ptr());
            if self.audio_sink.is_null() {
                return Err(VideoFileError::new("audio decoder pipeline has no appsink"));
            }
            self.audio_bus = (gst.gst_element_get_bus)(self.audio_pipeline);
            if self.audio_bus.is_null() {
                return Err(VideoFileError::new("audio decoder pipeline has no bus"));
            }
            if (gst.gst_element_set_state)(self.audio_pipeline, GST_STATE_PAUSED)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(self.audio_bus)
                    .unwrap_or_else(|| "pipeline refused PAUSED".to_string());
                return Err(VideoFileError::new(format!(
                    "no audio stream in file (decoder start: {text})"
                )));
            }
            // The sink does not hold the state change for a first sample (async=false), so the pipeline
            // settles to PAUSED once the demuxer has exposed every stream: a sink still unlinked then
            // has no stream to wait for, and a file without sound fails here instead of waiting forever.
            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            if (gst.gst_element_get_state)(self.audio_pipeline, &mut state, &mut pending, GST_CLOCK_TIME_NONE)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(self.audio_bus)
                    .unwrap_or_else(|| "pipeline failed to pause".to_string());
                return Err(VideoFileError::new(format!("no audio stream in file (decoder start: {text})")));
            }
            let sink_pad = (gst.gst_element_get_static_pad)(self.audio_sink, c"sink".as_ptr());
            let linked = !sink_pad.is_null() && (gst.gst_pad_is_linked)(sink_pad) != 0;
            if !sink_pad.is_null() {
                (gst.gst_object_unref)(sink_pad);
            }
            if !linked {
                return Err(VideoFileError::new("no audio stream in file"));
            }
            self.audio_preroll = self.pull_preroll(self.audio_sink, self.audio_bus, "audio")?;
            let caps_text = self.sample_caps(self.audio_preroll);
            let rate: u32 = caps_field(&caps_text, "rate")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            let channels: u16 = caps_field(&caps_text, "channels")
                .and_then(|v| v.parse().ok())
                .unwrap_or(0);
            if rate == 0 || channels == 0 {
                return Err(VideoFileError::new(format!(
                    "audio decoder negotiated invalid rate/channels (caps: {caps_text})"
                )));
            }
            let mut duration_ns: i64 = 0;
            let _ = (gst.gst_element_query_duration)(
                self.audio_pipeline,
                GST_FORMAT_TIME,
                &mut duration_ns,
            );
            if (gst.gst_element_set_state)(self.audio_pipeline, GST_STATE_PLAYING)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(self.audio_bus)
                    .unwrap_or_else(|| "pipeline refused PLAYING".to_string());
                return Err(VideoFileError::new(format!("audio decoder play: {text}")));
            }
            // Appsink returns the preroll again as the first PLAYING sample.
            // Consume that queued reference now, keeping our owned preroll
            // for next_audio so the first chunk is delivered exactly once.
            let first = (gst.gst_app_sink_pull_sample)(self.audio_sink);
            if first.is_null() {
                let text = gst
                    .bus_error(self.audio_bus)
                    .unwrap_or_else(|| "no audio stream in file".to_string());
                return Err(VideoFileError::new(format!("audio decoder: {text}")));
            }
            (gst.gst_mini_object_unref)(first as *mut c_void);
            self.info.has_audio = true;
            self.info.audio_sample_rate = rate;
            self.info.audio_channels = channels;
            if self.pipeline.is_null() {
                self.info.duration_100ns = duration_ns.max(0) / 100;
            }
            Ok(())
        }
    }

    unsafe fn parse_pipeline(&self, launch: &str) -> Result<*mut GstElement, VideoFileError> {
        let launch_c = CString::new(launch)
            .map_err(|_| VideoFileError::new("pipeline string contains NUL"))?;
        let mut err: *mut GError = std::ptr::null_mut();
        let pipeline = (self.gst.gst_parse_launch)(launch_c.as_ptr(), &mut err);
        if !err.is_null() || pipeline.is_null() {
            let text = if err.is_null() {
                "gst_parse_launch failed".to_string()
            } else {
                let text = std::ffi::CStr::from_ptr((*err).message)
                    .to_string_lossy()
                    .into_owned();
                (self.gst.g_error_free)(err);
                text
            };
            // A recoverable parse error can still return a partial pipeline.
            if !pipeline.is_null() {
                (self.gst.gst_element_set_state)(pipeline, GST_STATE_NULL);
                (self.gst.gst_object_unref)(pipeline as *mut c_void);
            }
            return Err(VideoFileError::new(format!("decoder pipeline: {text}")));
        }
        Ok(pipeline)
    }

    unsafe fn pull_preroll(
        &self,
        appsink: *mut GstElement,
        bus: *mut GstBus,
        stream: &str,
    ) -> Result<*mut GstSample, VideoFileError> {
        loop {
            // This is a polling interval, not a deadline for slow decoders.
            // An unlinked sink may never receive EOS, so inspect the bus
            // between bounded pulls rather than waiting on that sink forever.
            let sample = (self.gst.gst_app_sink_try_pull_preroll)(appsink, 100_000_000);
            if !sample.is_null() {
                return Ok(sample);
            }
            let message = (self.gst.gst_bus_timed_pop_filtered)(
                bus,
                0,
                GST_MESSAGE_ERROR | GST_MESSAGE_EOS,
            );
            if !message.is_null() {
                let text = if (*(message as *const GstMessageRepr)).mtype == GST_MESSAGE_ERROR {
                    self.gst.take_error_text(message)
                } else {
                    "end of stream before preroll".to_string()
                };
                (self.gst.gst_mini_object_unref)(message as *mut c_void);
                return Err(VideoFileError::new(format!(
                    "no {stream} stream in file (decoder preroll: {text})"
                )));
            }
            if (self.gst.gst_app_sink_is_eos)(appsink) != 0 {
                return Err(VideoFileError::new(format!("no {stream} stream in file")));
            }
        }
    }

    unsafe fn sample_caps(&self, sample: *mut GstSample) -> String {
        let caps = (self.gst.gst_sample_get_caps)(sample);
        if caps.is_null() {
            return String::new();
        }
        let text = (self.gst.gst_caps_to_string)(caps);
        if text.is_null() {
            return String::new();
        }
        let caps_text = std::ffi::CStr::from_ptr(text).to_string_lossy().into_owned();
        (self.gst.g_free)(text as *mut c_void);
        caps_text
    }

    /// Stream time accounts for edit lists and remains media time after a seek.
    unsafe fn sample_pts_100ns(&self, sample: *mut GstSample, buffer: *mut GstBuffer) -> i64 {
        let pts_ns = (*(buffer as *const GstBufferRepr)).pts;
        if pts_ns == GST_CLOCK_TIME_NONE {
            return 0;
        }
        let segment = (self.gst.gst_sample_get_segment)(sample);
        if !segment.is_null() {
            let stream_ns =
                (self.gst.gst_segment_to_stream_time)(segment, GST_FORMAT_TIME, pts_ns);
            if stream_ns != GST_CLOCK_TIME_NONE {
                return (stream_ns / 100) as i64;
            }
        }
        (pts_ns / 100) as i64
    }

    pub fn info(&self) -> &VideoFileInfo {
        &self.info
    }

    pub fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>, VideoFileError> {
        if self.pipeline.is_null() {
            return Ok(None);
        }
        unsafe {
            if (self.gst.gst_app_sink_is_eos)(self.appsink) != 0 {
                if let Some(text) = self.gst.bus_error(self.bus) {
                    return Err(VideoFileError::new(format!("decoder: {text}")));
                }
                return Ok(None);
            }
            let sample = (self.gst.gst_app_sink_pull_sample)(self.appsink);
            if sample.is_null() {
                if let Some(text) = self.gst.bus_error(self.bus) {
                    return Err(VideoFileError::new(format!("decoder: {text}")));
                }
                return Ok(None);
            }
            let buffer = (self.gst.gst_sample_get_buffer)(sample);
            if buffer.is_null() {
                (self.gst.gst_mini_object_unref)(sample as *mut c_void);
                return Err(VideoFileError::new("sample without buffer"));
            }
            let expected = nv12::nv12_frame_size(self.info.width, self.info.height);
            let mut map = GstMapInfo::default();
            if (self.gst.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
                (self.gst.gst_mini_object_unref)(sample as *mut c_void);
                return Err(VideoFileError::new("buffer map failed"));
            }
            let result = if map.size != expected {
                Err(VideoFileError::new(format!(
                    "padded NV12 layout unsupported on the Linux arm ({} != {expected} bytes)",
                    map.size
                )))
            } else {
                let mut nv12_bytes = vec![0u8; expected];
                std::ptr::copy_nonoverlapping(map.data, nv12_bytes.as_mut_ptr(), expected);
                Ok(Some(DecodedVideoFrame {
                    pts_100ns: self.sample_pts_100ns(sample, buffer),
                    width: self.info.width,
                    height: self.info.height,
                    nv12: nv12_bytes,
                    color: self.frame_color,
                }))
            };
            (self.gst.gst_buffer_unmap)(buffer, &mut map);
            (self.gst.gst_mini_object_unref)(sample as *mut c_void);
            result
        }
    }

    pub fn next_audio(&mut self) -> Result<Option<DecodedAudioChunk>, VideoFileError> {
        if self.audio_pipeline.is_null() {
            return Ok(None);
        }
        unsafe {
            let sample = if !self.audio_preroll.is_null() {
                std::mem::replace(&mut self.audio_preroll, std::ptr::null_mut())
            } else {
                if (self.gst.gst_app_sink_is_eos)(self.audio_sink) != 0 {
                    if let Some(text) = self.gst.bus_error(self.audio_bus) {
                        return Err(VideoFileError::new(format!("audio decoder: {text}")));
                    }
                    return Ok(None);
                }
                (self.gst.gst_app_sink_pull_sample)(self.audio_sink)
            };
            if sample.is_null() {
                if let Some(text) = self.gst.bus_error(self.audio_bus) {
                    return Err(VideoFileError::new(format!("audio decoder: {text}")));
                }
                return Ok(None);
            }
            let buffer = (self.gst.gst_sample_get_buffer)(sample);
            if buffer.is_null() {
                (self.gst.gst_mini_object_unref)(sample as *mut c_void);
                return Err(VideoFileError::new("audio sample without buffer"));
            }
            let mut map = GstMapInfo::default();
            if (self.gst.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
                (self.gst.gst_mini_object_unref)(sample as *mut c_void);
                return Err(VideoFileError::new("audio buffer map failed"));
            }
            let channels = self.info.audio_channels;
            let frame_bytes = 2 * channels as usize;
            let result = if map.size % frame_bytes != 0 {
                Err(VideoFileError::new(format!(
                    "invalid interleaved S16LE audio: {} bytes is not a multiple of \
                     {frame_bytes} bytes ({channels} channels)",
                    map.size
                )))
            } else if map.size != 0 && map.data.is_null() {
                Err(VideoFileError::new("audio buffer map returned no data"))
            } else {
                let samples = if map.size == 0 {
                    Vec::new()
                } else {
                    // Decode bytes explicitly: GstMapInfo need not be i16
                    // aligned, and the negotiated format is little-endian.
                    std::slice::from_raw_parts(map.data, map.size)
                        .chunks_exact(2)
                        .map(|bytes| i16::from_le_bytes([bytes[0], bytes[1]]))
                        .collect()
                };
                Ok(Some(DecodedAudioChunk {
                    pts_100ns: self.sample_pts_100ns(sample, buffer),
                    sample_rate: self.info.audio_sample_rate,
                    channels,
                    samples,
                }))
            };
            (self.gst.gst_buffer_unmap)(buffer, &mut map);
            (self.gst.gst_mini_object_unref)(sample as *mut c_void);
            result
        }
    }

    pub fn seek_to(&mut self, target_100ns: i64) -> Result<(), VideoFileError> {
        unsafe {
            if !self.audio_preroll.is_null() {
                (self.gst.gst_mini_object_unref)(self.audio_preroll as *mut c_void);
                self.audio_preroll = std::ptr::null_mut();
            }
            for (pipeline, bus, stream) in [
                (self.pipeline, self.bus, "video"),
                (self.audio_pipeline, self.audio_bus, "audio"),
            ] {
                if pipeline.is_null() {
                    continue;
                }
                let ok = (self.gst.gst_element_seek_simple)(
                    pipeline,
                    GST_FORMAT_TIME,
                    GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_KEY_UNIT | GST_SEEK_FLAG_SNAP_BEFORE,
                    target_100ns.max(0).saturating_mul(100),
                );
                if ok == 0 {
                    return Err(VideoFileError::new(format!("{stream} seek refused")));
                }
                // Wait for the flush to settle so the next pull is post-seek.
                let mut state: u32 = 0;
                let mut pending: u32 = 0;
                if (self.gst.gst_element_get_state)(
                    pipeline,
                    &mut state,
                    &mut pending,
                    5_000_000_000,
                ) == GST_STATE_CHANGE_FAILURE
                {
                    let text = self.gst.bus_error(bus).unwrap_or_else(|| "seek failed".to_string());
                    return Err(VideoFileError::new(format!("{stream} decoder: {text}")));
                }
            }
            Ok(())
        }
    }

    fn close_audio_pipeline(&mut self) {
        unsafe {
            if !self.audio_pipeline.is_null() {
                (self.gst.gst_element_set_state)(self.audio_pipeline, GST_STATE_NULL);
            }
            if !self.audio_preroll.is_null() {
                (self.gst.gst_mini_object_unref)(self.audio_preroll as *mut c_void);
                self.audio_preroll = std::ptr::null_mut();
            }
            for object in [self.audio_sink, self.audio_bus, self.audio_pipeline] {
                if !object.is_null() {
                    (self.gst.gst_object_unref)(object as *mut c_void);
                }
            }
            self.audio_sink = std::ptr::null_mut();
            self.audio_bus = std::ptr::null_mut();
            self.audio_pipeline = std::ptr::null_mut();
            self.info.has_audio = false;
            self.info.audio_sample_rate = 0;
            self.info.audio_channels = 0;
        }
    }
}

impl Drop for LinuxVideoFileDecoder {
    fn drop(&mut self) {
        self.close_audio_pipeline();
        unsafe {
            if !self.pipeline.is_null() {
                (self.gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            }
            for object in [self.appsink, self.bus, self.pipeline] {
                if !object.is_null() {
                    (self.gst.gst_object_unref)(object as *mut c_void);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VideoMatrix, VideoPrimaries, VideoRange, VideoTransfer};

    #[test]
    fn colorimetry_presets_and_numeric() {
        let c = colorimetry_to_color("bt709", 1920, 1080);
        assert_eq!(c, VideoColor::BT709);
        let c = colorimetry_to_color("bt601", 720, 576);
        assert_eq!((c.matrix, c.primaries, c.range), (VideoMatrix::Bt601, VideoPrimaries::Smpte170m, VideoRange::Limited));
        let c = colorimetry_to_color("1:4:7:1", 1920, 1080);
        assert_eq!((c.matrix, c.primaries, c.range, c.transfer), (VideoMatrix::Bt601, VideoPrimaries::Bt709, VideoRange::Full, VideoTransfer::Sdr));
        assert_eq!(colorimetry_to_color("jpeg", 640, 480), c);
        let c = colorimetry_to_color("bt2100-pq", 3840, 2160);
        assert_eq!((c.matrix, c.primaries, c.transfer), (VideoMatrix::Bt2020, VideoPrimaries::Bt2020, VideoTransfer::Pq));
        assert_eq!(colorimetry_to_color("bt2100-hlg", 3840, 2160).transfer, VideoTransfer::Hlg);
        assert_eq!(colorimetry_to_color("2:6:13:7", 3840, 2160).transfer, VideoTransfer::Sdr);
        // missing or unknown: size guess
        assert_eq!(colorimetry_to_color("", 1920, 1080), VideoColor::BT709);
        assert_eq!(colorimetry_to_color("0:0:0:0", 720, 480).matrix, VideoMatrix::Bt601);
        assert_eq!(colorimetry_to_color("nonsense", 1280, 720), VideoColor::BT709);
    }
}
