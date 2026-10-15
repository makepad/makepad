//! Linux file encoder: GStreamer pipeline behind the same facade as the
//! VideoToolbox / Media Foundation arms. `gst_parse_launch` builds
//! `appsrc ! videoconvert ! x264enc ! mp4mux ! filesink`; frames are
//! pushed as tightly packed NV12 produced by the SAME `nv12` converters
//! the other platforms use, so colours match across arms.
//!
//! Software x264 by policy of least surprise: at the sizes this seam
//! serves (thumbnails, small clips) software encode is milliseconds, and
//! `x264enc` ships in the stock `gstreamer1.0-plugins-ugly` package that
//! `tools/linux_deps.sh` already installs. H.265 maps to `x265enc`
//! (plugins-bad) and fails loudly where that element is absent.
//!
//! WRITTEN-UNTESTED: compiles cfg(linux); the fleet's Linux verification
//! pass owns the first live run.

use crate::linux_gst_sys::*;
use crate::nv12;
use crate::{VideoFileCodec, VideoFileEncoderOptions, VideoFileError, VideoTransformInfo};
use std::ffi::{c_void, CString};

const HNS_PER_SECOND: u128 = 10_000_000;

pub(crate) struct LinuxVideoFileEncoder {
    gst: &'static LibGst,
    pipeline: *mut GstElement,
    appsrc: *mut GstElement,
    /// The audio appsrc (PCM16 -> AAC -> the same MP4), when the options ask for audio.
    audio_src: *mut GstElement,
    audio_rate: u32,
    audio_channels: u32,
    audio_samples_pushed: u64,
    bus: *mut GstBus,
    width: u32,
    height: u32,
    fps_num: u32,
    fps_den: u32,
    frame_index: u64,
    finalized: bool,
    nv12_scratch: Vec<u8>,
    transform: VideoTransformInfo,
}

// The pipeline is owned and driven from whichever single thread calls the
// facade; GStreamer objects are internally thread-safe.
unsafe impl Send for LinuxVideoFileEncoder {}

impl LinuxVideoFileEncoder {
    pub fn new(path: &str, options: &VideoFileEncoderOptions) -> Result<Self, VideoFileError> {
        let gst = LibGst::get().ok_or_else(|| {
            VideoFileError::new(
                "GStreamer runtime not found (apt-get install gstreamer1.0-plugins-base \
                 gstreamer1.0-plugins-good gstreamer1.0-plugins-ugly)",
            )
        })?;
        if options.width % 4 != 0 {
            // GStreamer's default NV12 layout pads rows to 4 bytes; the
            // facade contract is tightly packed. Every current caller is
            // mod-16 anyway.
            return Err(VideoFileError::new(
                "width must be a multiple of 4 on the Linux GStreamer arm",
            ));
        }
        let kbps = (options.video_bitrate_bps / 1000).max(64);
        let gop = if options.keyframe_only { 1 } else { 60 };
        let codec = match options.codec {
            VideoFileCodec::H264 => format!(
                "x264enc bitrate={kbps} speed-preset=veryfast key-int-max={gop} ! h264parse"
            ),
            VideoFileCodec::H265 => format!(
                "x265enc bitrate={kbps} speed-preset=veryfast key-int-max={gop} ! h265parse"
            ),
        };
        // With an audio track the muxer is named so a second branch can feed it:
        // appsrc (S16LE PCM) ! audioconvert ! audioresample ! AAC ! aacparse ! mux.
        let audio_branch = match options.audio {
            Some(audio) => format!(
                " appsrc name=asrc block=true max-bytes=0 format=time caps=\"audio/x-raw,format=S16LE,\
                 layout=interleaved,rate={r},channels={c}\" ! audioconvert ! audioresample ! \
                 avenc_aac bitrate={b} ! aacparse ! mux.",
                r = audio.sample_rate,
                c = audio.channels,
                b = audio.aac_bitrate_bps.max(32_000),
            ),
            None => String::new(),
        };
        let launch = format!(
            "appsrc name=src block=true max-bytes=0 format=time caps=\"video/x-raw,format=NV12,width={w},\
             height={h},framerate={n}/{d}\" ! videoconvert ! {codec} ! mp4mux name=mux ! \
             filesink location=\"{path}\"{audio_branch}",
            w = options.width,
            h = options.height,
            n = options.fps_num,
            d = options.fps_den,
            path = path.replace('\\', "\\\\").replace('"', "\\\""),
        );
        let launch_c = CString::new(launch)
            .map_err(|_| VideoFileError::new("pipeline string contains NUL"))?;
        unsafe {
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
                return Err(VideoFileError::new(format!("encoder pipeline: {text}")));
            }
            if !err.is_null() {
                (gst.g_error_free)(err);
            }
            let name = CString::new("src").unwrap();
            let appsrc = (gst.gst_bin_get_by_name)(pipeline, name.as_ptr());
            if appsrc.is_null() {
                (gst.gst_object_unref)(pipeline as *mut c_void);
                return Err(VideoFileError::new("encoder pipeline has no appsrc"));
            }
            let mut audio_src: *mut GstElement = std::ptr::null_mut();
            if options.audio.is_some() {
                let aname = CString::new("asrc").unwrap();
                audio_src = (gst.gst_bin_get_by_name)(pipeline, aname.as_ptr());
                if audio_src.is_null() {
                    (gst.gst_object_unref)(appsrc as *mut c_void);
                    (gst.gst_object_unref)(pipeline as *mut c_void);
                    return Err(VideoFileError::new("encoder pipeline has no audio appsrc"));
                }
            }
            let bus = (gst.gst_element_get_bus)(pipeline);
            if (gst.gst_element_set_state)(pipeline, GST_STATE_PLAYING)
                == GST_STATE_CHANGE_FAILURE
            {
                let text = gst
                    .bus_error(bus)
                    .unwrap_or_else(|| "pipeline refused PLAYING".to_string());
                (gst.gst_object_unref)(appsrc as *mut c_void);
                if !audio_src.is_null() {
                    (gst.gst_object_unref)(audio_src as *mut c_void);
                }
                (gst.gst_object_unref)(bus as *mut c_void);
                (gst.gst_object_unref)(pipeline as *mut c_void);
                return Err(VideoFileError::new(format!("encoder start: {text}")));
            }
            Ok(Self {
                gst,
                pipeline,
                appsrc,
                audio_src,
                audio_rate: options.audio.map_or(0, |a| a.sample_rate),
                audio_channels: options.audio.map_or(0, |a| a.channels as u32),
                audio_samples_pushed: 0,
                bus,
                width: options.width,
                height: options.height,
                fps_num: options.fps_num,
                fps_den: options.fps_den,
                frame_index: 0,
                finalized: false,
                nv12_scratch: Vec::new(),
                transform: VideoTransformInfo {
                    name: match options.codec {
                        VideoFileCodec::H264 => "GStreamer x264enc (software)".to_string(),
                        VideoFileCodec::H265 => "GStreamer x265enc (software)".to_string(),
                    },
                    is_hardware: false,
                },
            })
        }
    }

    pub fn video_transform(&self) -> Option<&VideoTransformInfo> {
        Some(&self.transform)
    }

    fn frame_pts(&self, index: u64) -> i64 {
        (index as u128 * HNS_PER_SECOND * self.fps_den as u128 / self.fps_num as u128) as i64
    }

    pub fn push_frame_rgb(
        &mut self,
        rgb: &[u8],
        pixel_stride: usize,
        pts_100ns: Option<i64>,
    ) -> Result<(), VideoFileError> {
        let mut scratch = std::mem::take(&mut self.nv12_scratch);
        nv12::rgbx_to_nv12(rgb, self.width, self.height, pixel_stride, &mut scratch);
        let result = self.push_frame_nv12(&scratch, pts_100ns);
        self.nv12_scratch = scratch;
        result
    }

    pub fn push_frame_nv12(
        &mut self,
        nv12_bytes: &[u8],
        pts_100ns: Option<i64>,
    ) -> Result<(), VideoFileError> {
        if self.finalized {
            return Err(VideoFileError::new("encoder already finalized"));
        }
        let pts = pts_100ns.unwrap_or_else(|| self.frame_pts(self.frame_index));
        let duration =
            (HNS_PER_SECOND as u64 * self.fps_den as u64 / self.fps_num as u64) * 100;
        unsafe {
            if let Some(text) = self.gst.bus_error(self.bus) {
                return Err(VideoFileError::new(format!("encoder: {text}")));
            }
            let buffer = (self.gst.gst_buffer_new_allocate)(
                std::ptr::null_mut(),
                nv12_bytes.len(),
                std::ptr::null_mut(),
            );
            if buffer.is_null() {
                return Err(VideoFileError::new("gst_buffer_new_allocate failed"));
            }
            (self.gst.gst_buffer_fill)(
                buffer,
                0,
                nv12_bytes.as_ptr() as *const c_void,
                nv12_bytes.len(),
            );
            // Public-ABI field writes (GST_BUFFER_PTS in C is this access).
            let repr = buffer as *mut GstBufferRepr;
            (*repr).pts = (pts as u64) * 100;
            (*repr).dts = GST_CLOCK_TIME_NONE;
            (*repr).duration = duration;
            // push_buffer takes ownership of the buffer either way.
            if (self.gst.gst_app_src_push_buffer)(self.appsrc, buffer) != GST_FLOW_OK {
                let text = self
                    .gst
                    .bus_error(self.bus)
                    .unwrap_or_else(|| "appsrc push refused".to_string());
                return Err(VideoFileError::new(format!("encoder push: {text}")));
            }
        }
        self.frame_index += 1;
        Ok(())
    }

    pub fn push_audio_i16(&mut self, samples: &[i16]) -> Result<(), VideoFileError> {
        if self.audio_src.is_null() {
            return Err(VideoFileError::new(
                "push_audio_i16 called but encoder was created without an audio track",
            ));
        }
        if self.finalized {
            return Err(VideoFileError::new("encoder already finalized"));
        }
        let channels = self.audio_channels as u64;
        let frames = samples.len() as u64 / channels;
        // Timestamps follow the running sample count, as the other arms do.
        let pts_ns = self.audio_samples_pushed * 1_000_000_000 / self.audio_rate as u64;
        let duration_ns = frames * 1_000_000_000 / self.audio_rate as u64;
        unsafe {
            if let Some(text) = self.gst.bus_error(self.bus) {
                return Err(VideoFileError::new(format!("encoder: {text}")));
            }
            let bytes = samples.len() * 2;
            let buffer = (self.gst.gst_buffer_new_allocate)(std::ptr::null_mut(), bytes, std::ptr::null_mut());
            if buffer.is_null() {
                return Err(VideoFileError::new("gst_buffer_new_allocate failed"));
            }
            (self.gst.gst_buffer_fill)(buffer, 0, samples.as_ptr() as *const c_void, bytes);
            let repr = buffer as *mut GstBufferRepr;
            (*repr).pts = pts_ns;
            (*repr).dts = GST_CLOCK_TIME_NONE;
            (*repr).duration = duration_ns;
            if (self.gst.gst_app_src_push_buffer)(self.audio_src, buffer) != GST_FLOW_OK {
                let text = self
                    .gst
                    .bus_error(self.bus)
                    .unwrap_or_else(|| "audio appsrc push refused".to_string());
                return Err(VideoFileError::new(format!("encoder audio push: {text}")));
            }
        }
        self.audio_samples_pushed += frames;
        Ok(())
    }

    pub fn finish(&mut self) -> Result<(), VideoFileError> {
        if self.finalized {
            return Ok(());
        }
        self.finalized = true;
        unsafe {
            (self.gst.gst_app_src_end_of_stream)(self.appsrc);
            if !self.audio_src.is_null() {
                (self.gst.gst_app_src_end_of_stream)(self.audio_src);
            }
            // Wait (bounded) for the muxer to write the container.
            let msg = (self.gst.gst_bus_timed_pop_filtered)(
                self.bus,
                30_000_000_000, // 30s in ns
                GST_MESSAGE_EOS | GST_MESSAGE_ERROR,
            );
            let result = if msg.is_null() {
                Err(VideoFileError::new("encoder finalize timed out"))
            } else {
                let repr = msg as *const GstMessageRepr;
                let is_error = ((*repr).mtype & GST_MESSAGE_ERROR) != 0;
                let out = if is_error {
                    Err(VideoFileError::new(format!(
                        "encoder finalize: {}",
                        self.gst.take_error_text(msg)
                    )))
                } else {
                    Ok(())
                };
                (self.gst.gst_mini_object_unref)(msg as *mut c_void);
                out
            };
            (self.gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            result
        }
    }
}

impl Drop for LinuxVideoFileEncoder {
    fn drop(&mut self) {
        unsafe {
            if !self.finalized {
                // Best-effort finalize, errors unreportable from Drop.
                let _ = self.finish();
            }
            (self.gst.gst_object_unref)(self.appsrc as *mut c_void);
            if !self.audio_src.is_null() {
                (self.gst.gst_object_unref)(self.audio_src as *mut c_void);
            }
            (self.gst.gst_object_unref)(self.bus as *mut c_void);
            (self.gst.gst_object_unref)(self.pipeline as *mut c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::PcmAudioTrackOptions;

    /// 24 frames of grey + one second of a 440 Hz tone, muxed to MP4: the file must
    /// carry both a video and an AAC audio stream (checked with ffprobe when present).
    #[test]
    fn video_and_audio_land_in_one_mp4() {
        if LibGst::get().is_none() {
            return;
        }
        let path = std::env::temp_dir().join(format!("linux-enc-av-{}.mp4", std::process::id()));
        let path_text = path.to_string_lossy().into_owned();
        let options = VideoFileEncoderOptions {
            width: 640,
            height: 352,
            fps_num: 24,
            fps_den: 1,
            audio: Some(PcmAudioTrackOptions { sample_rate: 48_000, channels: 2, aac_bitrate_bps: 128_000 }),
            ..Default::default()
        };
        let mut enc = LinuxVideoFileEncoder::new(&path_text, &options).expect("encoder with audio");
        // Many frames before the first audio sample: the muxer interleaves, so the
        // encoder must buffer rather than block the video branch (this deadlocked
        // an h3 clip: 124 frames, then audio).
        for i in 0..124u8 {
            enc.push_frame_rgb(&vec![i.wrapping_mul(8); 640 * 352 * 4], 4, None).expect("frame");
        }
        let tone: Vec<i16> = (0..48_000 * 5)
            .flat_map(|n| {
                let v = ((n as f32 * 440.0 * std::f32::consts::TAU / 48_000.0).sin() * 8000.0) as i16;
                [v, v]
            })
            .collect();
        enc.push_audio_i16(&tone).expect("audio");
        enc.finish().expect("finish");
        let bytes = std::fs::metadata(&path).expect("file").len();
        assert!(bytes > 2000, "mp4 too small: {bytes}");
        if let Ok(out) = std::process::Command::new("ffprobe")
            .args(["-v", "error", "-show_entries", "stream=codec_type,codec_name", "-of", "csv=p=0"])
            .arg(&path)
            .output()
        {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            assert!(text.contains("video"), "no video stream: {text}");
            assert!(text.contains("audio") && text.contains("aac"), "no aac audio stream: {text}");
        }
        let _ = std::fs::remove_file(&path);
    }
}
