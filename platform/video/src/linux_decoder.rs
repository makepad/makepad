//! Linux file decoder: GStreamer `uridecodebin` behind the same facade as
//! the AVAssetReader / Media Foundation arms — an mp4 (or anything the
//! installed plugins demux, `gstreamer1.0-libav` included) in, tightly
//! packed NV12 frames out.
//!
//! Limitations of this first arm, stated rather than hidden: the audio
//! track is not wired (`next_audio` reports end-of-stream, and opening a
//! file FOR its sound refuses rather than opening something that could
//! only fail later), `info()` cannot name the compressed codec, and
//! frames whose negotiated NV12 layout is not tightly packed are refused
//! loudly (never silently sheared). The VJ thumbnail cache — 30-frame 256x160 H.264, no audio —
//! sits comfortably inside all three.
//!
//! WRITTEN-UNTESTED: compiles cfg(linux); the fleet's Linux verification
//! pass owns the first live run.

use crate::linux_gst_sys::*;
use crate::nv12;
use crate::{DecodedAudioChunk, DecodedVideoFrame, VideoColor, VideoFileError, VideoFileInfo};
use std::ffi::{c_void, CString};

pub(crate) struct LinuxVideoFileDecoder {
    gst: &'static LibGst,
    pipeline: *mut GstElement,
    appsink: *mut GstElement,
    bus: *mut GstBus,
    info: VideoFileInfo,
    /// The colour of the delivered bytes (the negotiated caps' colorimetry).
    frame_color: VideoColor,
}

unsafe impl Send for LinuxVideoFileDecoder {}

/// Pull `key=(type)value` out of a caps string like
/// `video/x-raw, format=(string)NV12, width=(int)256, ...`.
fn caps_field<'a>(caps: &'a str, key: &str) -> Option<&'a str> {
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
    /// This arm has no audio at all (see `next_audio` below and the note
    /// at the top of the module): opening a file for its sound would only
    /// be a way of failing later, so it fails here and says why.
    pub fn open_audio(_path: &str) -> Result<Self, VideoFileError> {
        Err(VideoFileError::new(
            "audio-only decode is not implemented on this platform yet",
        ))
    }

    pub fn open(path: &str) -> Result<Self, VideoFileError> {
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
            let launch = format!(
                "uridecodebin uri=\"{uri}\" ! queue ! videoconvert ! \
                 video/x-raw,format=NV12 ! appsink name=out sync=false"
            );
            let launch_c = CString::new(launch)
                .map_err(|_| VideoFileError::new("pipeline string contains NUL"))?;
            let mut err: *mut GError = std::ptr::null_mut();
            let pipeline = (gst.gst_parse_launch)(launch_c.as_ptr(), &mut err);
            if pipeline.is_null() {
                let text = if err.is_null() {
                    "gst_parse_launch failed".to_string()
                } else {
                    let t = std::ffi::CStr::from_ptr((*err).message)
                        .to_string_lossy()
                        .into_owned();
                    (gst.g_error_free)(err);
                    t
                };
                return Err(VideoFileError::new(format!("decoder pipeline: {text}")));
            }
            if !err.is_null() {
                (gst.g_error_free)(err);
            }
            let name = CString::new("out").unwrap();
            let appsink = (gst.gst_bin_get_by_name)(pipeline, name.as_ptr());
            if appsink.is_null() {
                (gst.gst_object_unref)(pipeline as *mut c_void);
                return Err(VideoFileError::new("decoder pipeline has no appsink"));
            }
            let bus = (gst.gst_element_get_bus)(pipeline);
            let fail = |gst: &LibGst, text: String| {
                (gst.gst_element_set_state)(pipeline, GST_STATE_NULL);
                (gst.gst_object_unref)(appsink as *mut c_void);
                (gst.gst_object_unref)(bus as *mut c_void);
                (gst.gst_object_unref)(pipeline as *mut c_void);
                VideoFileError::new(text)
            };
            if (gst.gst_element_set_state)(pipeline, GST_STATE_PAUSED)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(bus)
                    .unwrap_or_else(|| "pipeline refused PAUSED".to_string());
                return Err(fail(gst, format!("decoder start: {text}")));
            }
            // Preroll: the first sample's caps carry the real geometry.
            let preroll = (gst.gst_app_sink_pull_preroll)(appsink);
            if preroll.is_null() {
                let text = gst
                    .bus_error(bus)
                    .unwrap_or_else(|| "no decodable video stream".to_string());
                return Err(fail(gst, format!("decoder preroll: {text}")));
            }
            let caps = (gst.gst_sample_get_caps)(preroll);
            let caps_text = if caps.is_null() {
                String::new()
            } else {
                let s = (gst.gst_caps_to_string)(caps);
                let text = std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned();
                (gst.g_free)(s as *mut c_void);
                text
            };
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
                return Err(fail(
                    gst,
                    format!("decoder negotiated no geometry (caps: {caps_text})"),
                ));
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
                (gst.gst_element_query_duration)(pipeline, GST_FORMAT_TIME, &mut duration_ns);
            if (gst.gst_element_set_state)(pipeline, GST_STATE_PLAYING)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(bus)
                    .unwrap_or_else(|| "pipeline refused PLAYING".to_string());
                return Err(fail(gst, format!("decoder play: {text}")));
            }
            Ok(Self {
                gst,
                pipeline,
                appsink,
                bus,
                info: VideoFileInfo {
                    width,
                    height,
                    fps_num,
                    fps_den,
                    duration_100ns: duration_ns.max(0) / 100,
                    // Not dug out of the demuxer on this arm.
                    video_codec: None,
                    video_codec_fourcc: 0,
                    has_audio: false,
                    audio_sample_rate: 0,
                    audio_channels: 0,
                    color,
                },
                frame_color: color,
            })
        }
    }

    pub fn info(&self) -> &VideoFileInfo {
        &self.info
    }

    pub fn next_frame(&mut self) -> Result<Option<DecodedVideoFrame>, VideoFileError> {
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
                let pts_ns = (*(buffer as *const GstBufferRepr)).pts;
                Ok(Some(DecodedVideoFrame {
                    pts_100ns: if pts_ns == GST_CLOCK_TIME_NONE {
                        0
                    } else {
                        (pts_ns / 100) as i64
                    },
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
        // Audio is not wired on this arm; the facade treats None as
        // end-of-stream, which matches `info.has_audio == false`.
        Ok(None)
    }

    pub fn seek_to(&mut self, target_100ns: i64) -> Result<(), VideoFileError> {
        unsafe {
            let ok = (self.gst.gst_element_seek_simple)(
                self.pipeline,
                GST_FORMAT_TIME,
                GST_SEEK_FLAG_FLUSH | GST_SEEK_FLAG_KEY_UNIT | GST_SEEK_FLAG_SNAP_BEFORE,
                target_100ns.max(0) * 100,
            );
            if ok == 0 {
                return Err(VideoFileError::new("seek refused"));
            }
            // Wait for the flush to settle so the next pull is post-seek.
            let mut state: u32 = 0;
            let mut pending: u32 = 0;
            (self.gst.gst_element_get_state)(
                self.pipeline,
                &mut state,
                &mut pending,
                5_000_000_000,
            );
            Ok(())
        }
    }
}

impl Drop for LinuxVideoFileDecoder {
    fn drop(&mut self) {
        unsafe {
            (self.gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            (self.gst.gst_object_unref)(self.appsink as *mut c_void);
            (self.gst.gst_object_unref)(self.bus as *mut c_void);
            (self.gst.gst_object_unref)(self.pipeline as *mut c_void);
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
