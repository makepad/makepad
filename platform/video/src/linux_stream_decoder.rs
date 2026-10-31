//! Linux stream DECODE arm of [`crate::VideoStreamDecoder`]: GStreamer through
//! the same dlopen table as the file arm (`linux_gst_sys`). Annex-B access
//! units go into an `appsrc`, `decodebin` picks the installed decoder (a VA
//! one where present, else libav), and tight NV12 comes out of an `appsink`.
//!
//! Push and pull are decoupled inside GStreamer, so a pushed unit's frame can
//! come out with a later push: `push_packet` returns what is ready by then
//! (a decoder with frame threads holds a few frames: tens of milliseconds of
//! latency, fine for a camera view; not for a sub-frame realtime stream).
//! Rows padded to a stride are repacked tight.

use crate::linux_decoder::{caps_field, colorimetry_to_color};
use crate::linux_gst_sys::*;
use crate::stream_decoder::{DecodedFrame, StreamVideoCodec};
use crate::{VideoColor, VideoFileError};
use std::ffi::{c_void, CString};

pub(crate) struct LinuxStreamDecoder {
    gst: &'static LibGst,
    pipeline: *mut GstElement,
    appsrc: *mut GstElement,
    appsink: *mut GstElement,
    bus: *mut GstBus,
}

unsafe impl Send for LinuxStreamDecoder {}

/// The NV12 layout GStreamer gives a `width` x `height` frame: (luma stride, chroma offset).
fn gst_nv12_layout(width: u32, height: u32) -> (usize, usize) {
    let stride = (width as usize + 3) & !3;
    (stride, stride * ((height as usize + 1) & !1))
}

/// Copies an NV12 frame laid out as GStreamer pads it into tight rows.
pub(crate) fn repack_nv12(src: &[u8], width: u32, height: u32) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let tight = w * h + w * h.div_ceil(2);
    if src.len() == tight { return Some(src.to_vec()); }
    let (stride, chroma) = gst_nv12_layout(width, height);
    if src.len() < chroma + stride * h.div_ceil(2) { return None; }
    let mut out = Vec::with_capacity(tight);
    for row in 0..h { out.extend_from_slice(&src[row * stride..row * stride + w]); }
    for row in 0..h.div_ceil(2) { out.extend_from_slice(&src[chroma + row * stride..chroma + row * stride + w]); }
    Some(out)
}

impl LinuxStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        let gst = LibGst::get().ok_or_else(|| VideoFileError::new(
            "GStreamer runtime not found (install gstreamer1.0-plugins-base, -plugins-good and -libav)",
        ))?;
        let caps = match codec {
            StreamVideoCodec::H264 => "video/x-h264,stream-format=byte-stream,alignment=au",
            StreamVideoCodec::Hevc => "video/x-h265,stream-format=byte-stream,alignment=au",
        };
        let parse = match codec { StreamVideoCodec::H264 => "h264parse", StreamVideoCodec::Hevc => "h265parse" };
        let launch = format!(
            "appsrc name=src is-live=true format=time block=false max-bytes=8000000 caps=\"{caps}\" ! {parse} ! \
             decodebin ! videoconvert ! video/x-raw,format=NV12 ! appsink name=sink sync=false max-buffers=4 drop=true"
        );
        unsafe {
            let launch_c = CString::new(launch).map_err(|_| VideoFileError::new("pipeline string contains NUL"))?;
            let mut err: *mut GError = std::ptr::null_mut();
            let pipeline = (gst.gst_parse_launch)(launch_c.as_ptr(), &mut err);
            if !err.is_null() {
                let text = std::ffi::CStr::from_ptr((*err).message).to_string_lossy().into_owned();
                (gst.g_error_free)(err);
                if pipeline.is_null() { return Err(VideoFileError::new(format!("stream decoder pipeline: {text}"))); }
            }
            if pipeline.is_null() { return Err(VideoFileError::new("stream decoder pipeline failed")); }
            let src_name = CString::new("src").unwrap();
            let sink_name = CString::new("sink").unwrap();
            let appsrc = (gst.gst_bin_get_by_name)(pipeline, src_name.as_ptr());
            let appsink = (gst.gst_bin_get_by_name)(pipeline, sink_name.as_ptr());
            let bus = (gst.gst_element_get_bus)(pipeline);
            let this = Self { gst, pipeline, appsrc, appsink, bus };
            if appsrc.is_null() || appsink.is_null() { return Err(VideoFileError::new("stream decoder pipeline lacks its ends")); }
            if (gst.gst_element_set_state)(pipeline, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE {
                let text = gst.bus_error(bus).unwrap_or_else(|| "pipeline refused PLAYING".into());
                return Err(VideoFileError::new(format!("stream decoder start: {text}")));
            }
            Ok(this)
        }
    }

    pub fn push_packet(&mut self, annex_b: &[u8], pts_100ns: i64) -> Result<Vec<DecodedFrame>, VideoFileError> {
        unsafe {
            if let Some(text) = self.gst.bus_error(self.bus) {
                return Err(VideoFileError::new(format!("stream decoder: {text}")));
            }
            let buffer = (self.gst.gst_buffer_new_allocate)(std::ptr::null_mut(), annex_b.len(), std::ptr::null_mut());
            if buffer.is_null() { return Err(VideoFileError::new("gst_buffer_new_allocate failed")); }
            (self.gst.gst_buffer_fill)(buffer, 0, annex_b.as_ptr() as *const c_void, annex_b.len());
            let repr = buffer as *mut GstBufferRepr;
            (*repr).pts = pts_100ns.max(0) as u64 * 100;
            (*repr).dts = GST_CLOCK_TIME_NONE;
            (*repr).duration = GST_CLOCK_TIME_NONE;
            // push_buffer takes ownership either way.
            if (self.gst.gst_app_src_push_buffer)(self.appsrc, buffer) != GST_FLOW_OK {
                return Err(VideoFileError::new("stream decoder refused the packet"));
            }
        }
        self.drain(0)
    }

    /// Frames ready now; `wait_ns` bounds the wait for the first one.
    fn drain(&mut self, wait_ns: u64) -> Result<Vec<DecodedFrame>, VideoFileError> {
        let mut frames = Vec::new();
        let mut wait = wait_ns;
        loop {
            let sample = unsafe { (self.gst.gst_app_sink_try_pull_sample)(self.appsink, wait) };
            if sample.is_null() { break; }
            wait = 0;
            let frame = unsafe { self.frame(sample) };
            unsafe { (self.gst.gst_mini_object_unref)(sample as *mut c_void) };
            frames.push(frame?);
        }
        Ok(frames)
    }

    unsafe fn frame(&self, sample: *mut GstSample) -> Result<DecodedFrame, VideoFileError> {
        let caps = (self.gst.gst_sample_get_caps)(sample);
        let caps_text = if caps.is_null() { String::new() } else {
            let s = (self.gst.gst_caps_to_string)(caps);
            let text = std::ffi::CStr::from_ptr(s).to_string_lossy().into_owned();
            (self.gst.g_free)(s as *mut c_void);
            text
        };
        let width: u32 = caps_field(&caps_text, "width").and_then(|v| v.parse().ok()).unwrap_or(0);
        let height: u32 = caps_field(&caps_text, "height").and_then(|v| v.parse().ok()).unwrap_or(0);
        if width == 0 || height == 0 { return Err(VideoFileError::new("stream decoder negotiated no geometry")); }
        let color: VideoColor = colorimetry_to_color(caps_field(&caps_text, "colorimetry").unwrap_or(""), width, height);
        let buffer = (self.gst.gst_sample_get_buffer)(sample);
        if buffer.is_null() { return Err(VideoFileError::new("sample without buffer")); }
        let mut map = GstMapInfo::default();
        if (self.gst.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 { return Err(VideoFileError::new("buffer map failed")); }
        let bytes = std::slice::from_raw_parts(map.data, map.size);
        let nv12 = repack_nv12(bytes, width, height);
        let pts = (*(buffer as *const GstBufferRepr)).pts;
        (self.gst.gst_buffer_unmap)(buffer, &mut map);
        let nv12 = nv12.ok_or_else(|| VideoFileError::new("stream decoder gave an unexpected NV12 layout"))?;
        Ok(DecodedFrame { width, height, nv12, pts_100ns: if pts == GST_CLOCK_TIME_NONE { 0 } else { (pts / 100) as i64 }, gpu: None, color })
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        self.drain(20_000_000)
    }
}

impl Drop for LinuxStreamDecoder {
    fn drop(&mut self) {
        unsafe {
            (self.gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            if !self.appsrc.is_null() { (self.gst.gst_object_unref)(self.appsrc as *mut c_void); }
            if !self.appsink.is_null() { (self.gst.gst_object_unref)(self.appsink as *mut c_void); }
            (self.gst.gst_object_unref)(self.bus as *mut c_void);
            (self.gst.gst_object_unref)(self.pipeline as *mut c_void);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn padded_rows_repack_tight() {
        // 6 x 3: stride 8, chroma at 8 * 4 = 32, two chroma rows.
        let (w, h) = (6u32, 3u32);
        let (stride, chroma) = gst_nv12_layout(w, h);
        assert_eq!((stride, chroma), (8, 32));
        let mut src = vec![0xEEu8; chroma + stride * 2];
        for r in 0..3 { for c in 0..6 { src[r * stride + c] = (r * 10 + c) as u8; } }
        for r in 0..2 { for c in 0..6 { src[chroma + r * stride + c] = (100 + r * 10 + c) as u8; } }
        let out = repack_nv12(&src, w, h).unwrap();
        assert_eq!(out.len(), 6 * 3 + 6 * 2);
        assert_eq!(&out[..6], &[0, 1, 2, 3, 4, 5]);
        assert_eq!(&out[12..18], &[20, 21, 22, 23, 24, 25]);
        assert_eq!(&out[18..24], &[100, 101, 102, 103, 104, 105]);
        assert!(!out.contains(&0xEE));
        let tight = vec![1u8; 1920 * 1080 * 3 / 2];
        assert_eq!(repack_nv12(&tight, 1920, 1080).unwrap().len(), tight.len());
        assert!(repack_nv12(&[0; 10], 6, 3).is_none());
    }

    /// Access units from GStreamer's own test source and encoder (where installed), decoded back.
    fn encoded_test_units(gst: &'static LibGst, frames: u32) -> Option<Vec<Vec<u8>>> {
        let launch = format!("videotestsrc num-buffers={frames} ! video/x-raw,width=320,height=240,framerate=30/1 ! \
            x264enc tune=zerolatency key-int-max=10 ! video/x-h264,stream-format=byte-stream,alignment=au ! appsink name=sink sync=false");
        unsafe {
            let launch_c = CString::new(launch).unwrap();
            let mut err: *mut GError = std::ptr::null_mut();
            let pipeline = (gst.gst_parse_launch)(launch_c.as_ptr(), &mut err);
            if !err.is_null() { (gst.g_error_free)(err); }
            if pipeline.is_null() { return None; }
            let name = CString::new("sink").unwrap();
            let sink = (gst.gst_bin_get_by_name)(pipeline, name.as_ptr());
            (gst.gst_element_set_state)(pipeline, GST_STATE_PLAYING);
            let mut units = Vec::new();
            loop {
                let sample = (gst.gst_app_sink_try_pull_sample)(sink, 5_000_000_000);
                if sample.is_null() { break; }
                let buffer = (gst.gst_sample_get_buffer)(sample);
                let mut map = GstMapInfo::default();
                if (gst.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) != 0 {
                    units.push(std::slice::from_raw_parts(map.data, map.size).to_vec());
                    (gst.gst_buffer_unmap)(buffer, &mut map);
                }
                (gst.gst_mini_object_unref)(sample as *mut c_void);
            }
            (gst.gst_element_set_state)(pipeline, GST_STATE_NULL);
            (gst.gst_object_unref)(sink as *mut c_void);
            (gst.gst_object_unref)(pipeline as *mut c_void);
            Some(units)
        }
    }

    #[test]
    fn decodes_a_live_h264_stream_where_gstreamer_is_installed() {
        let Some(gst) = LibGst::get() else { eprintln!("no GStreamer: stream decode not exercised"); return };
        let Some(units) = encoded_test_units(gst, 30).filter(|u| !u.is_empty()) else { eprintln!("no x264enc: stream decode not exercised"); return };
        assert_eq!(units.len(), 30);
        let mut decoder = LinuxStreamDecoder::new(StreamVideoCodec::H264).expect("stream decoder");
        let mut frames = Vec::new();
        for (i, unit) in units.iter().enumerate() {
            frames.extend(decoder.push_packet(unit, i as i64 * 333_333).expect("push"));
        }
        for _ in 0..50 {
            if frames.len() >= units.len() { break; }
            frames.extend(decoder.flush().expect("flush"));
        }
        assert!(frames.len() >= 25, "decoded {} of {} pictures", frames.len(), units.len());
        let f = &frames[0];
        assert_eq!((f.width, f.height), (320, 240));
        assert_eq!(f.nv12.len(), 320 * 240 * 3 / 2);
        // videotestsrc's first bar (white-ish) at the top left: bright luma.
        assert!(f.nv12[10] > 150, "luma {}", f.nv12[10]);
        eprintln!("linux stream decoder: {} of {} pictures", frames.len(), units.len());
    }
}
