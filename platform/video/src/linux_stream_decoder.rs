//! Linux stream DECODE arm of [`crate::VideoStreamDecoder`]: GStreamer through
//! the same dlopen table as the file arm (`linux_gst_sys`). Annex-B access
//! units go into an `appsrc`, `decodebin` picks the installed decoder (a VA
//! one where present, else libav), and tight NV12 comes out of an `appsink`.
//!
//! Push and pull are decoupled inside GStreamer, so a pushed unit's frame can
//! come out with a later push: `push_packet` returns what is ready by then
//! (a decoder with frame threads holds a few frames: tens of milliseconds of
//! latency, fine for a camera view; not for a sub-frame realtime stream).
//! Planes are read where the buffer's layout puts them and repacked tight.

use crate::linux_decoder::{caps_field, colorimetry_to_color, sample_nv12, BUS_LOOK_NS};
use crate::linux_gst_sys::*;
use crate::mp4_first_frame::FirstAccessUnit;
use crate::stream_decoder::{DecodedFrame, StreamVideoCodec};
use crate::{VideoColor, VideoFileError};
use std::ffi::{c_void, CString};
use std::sync::atomic::{AtomicI64, Ordering};
use std::sync::{Condvar, Mutex};

pub(crate) struct LinuxStreamDecoder {
    gst: &'static LibGst,
    pipeline: *mut GstElement,
    appsrc: *mut GstElement,
    appsink: *mut GstElement,
    bus: *mut GstBus,
}

unsafe impl Send for LinuxStreamDecoder {}

impl LinuxStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        Self::with_decoder(codec, "decodebin")
    }

    /// A stream decoder through the `decoder` element (`decodebin`: the one it plugs).
    fn with_decoder(codec: StreamVideoCodec, decoder: &str) -> Result<Self, VideoFileError> {
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
             {decoder} ! videoconvert ! video/x-raw,format=NV12 ! appsink name=sink sync=false max-buffers=4 drop=true"
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
            gst.give_shared_contexts(pipeline);
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
            if let Some(text) = self.gst.bus_drain(self.bus, false) {
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
        let nv12 = sample_nv12(self.gst, sample, width, height)?;
        let pts = (*((self.gst.gst_sample_get_buffer)(sample) as *const GstBufferRepr)).pts;
        Ok(DecodedFrame { width, height, nv12, pts_100ns: if pts == GST_CLOCK_TIME_NONE { 0 } else { (pts / 100) as i64 }, gpu: None, color })
    }

    pub fn flush(&mut self) -> Result<Vec<DecodedFrame>, VideoFileError> {
        self.drain(20_000_000)
    }

    /// Pushes one access unit and waits for its picture, the frame stamped
    /// `pts_100ns`, however long the decoder takes. A frame stamped otherwise
    /// (one a failed picture left behind) is dropped; an error or warning on
    /// the bus (a decoder drops a picture it cannot decode with a warning)
    /// fails the picture, and so does the stream's end.
    fn decode_one(&mut self, annex_b: &[u8], pts_100ns: i64) -> Result<DecodedFrame, VideoFileError> {
        let mut frames = self.push_packet(annex_b, pts_100ns)?;
        loop {
            if let Some(i) = frames.iter().position(|f| f.pts_100ns == pts_100ns) {
                return Ok(frames.swap_remove(i));
            }
            if let Some(text) = self.gst.bus_drain(self.bus, true) {
                return Err(VideoFileError::new(format!("stream decoder: {text}")));
            }
            if unsafe { (self.gst.gst_app_sink_is_eos)(self.appsink) } != 0 {
                return Err(VideoFileError::new("stream decoder ended before the picture"));
            }
            frames = self.drain(BUS_LOOK_NS)?;
        }
    }
}

impl Drop for LinuxStreamDecoder {
    fn drop(&mut self) {
        unsafe {
            self.gst.keep_shared_contexts(self.pipeline);
            (self.gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            if !self.appsrc.is_null() { (self.gst.gst_object_unref)(self.appsrc as *mut c_void); }
            if !self.appsink.is_null() { (self.gst.gst_object_unref)(self.appsink as *mut c_void); }
            (self.gst.gst_object_unref)(self.bus as *mut c_void);
            (self.gst.gst_object_unref)(self.pipeline as *mut c_void);
        }
    }
}

/// The stream decoders kept for stills ([`decode_still`]), by decoder element: a pipeline
/// stands up once and decodes picture after picture whatever their sizes, where a file
/// pipeline built and tore down a decoder session per picture (on a desktop GPU 20 ms to open
/// and 7 ms to close one at a time, 160 ms and 150 ms with ten at once, against under 1 ms of
/// decoding).
///
/// A software decoder is kept for every still decoded at once (each is a CPU's work). A
/// hardware decoder is kept once: on that GPU one warm session decoded 5.3 ms a picture and ten
/// 6.8 ms (the engine was never the limit), each holding about 600 MB of video memory, so
/// a still that finds it busy waits for it.
struct Stills {
    idle: Vec<(String, LinuxStreamDecoder)>,
    /// The hardware elements whose decoder is out (decoding, or being made).
    busy: Vec<String>,
}

static STILLS: Mutex<Stills> = Mutex::new(Stills { idle: Vec::new(), busy: Vec::new() });
static STILL_FREED: Condvar = Condvar::new();

/// The timestamp of the next still: each picture's frame is told from any other by it.
static STILL_PTS: AtomicI64 = AtomicI64::new(1);

/// A hardware element's decoder out of [`STILLS`]: given back (or, failed, dropped) on drop,
/// a panic included, so a still waiting for it is never left waiting.
struct Out {
    element: String,
    hardware: bool,
    decoder: Option<LinuxStreamDecoder>,
}

impl Drop for Out {
    fn drop(&mut self) {
        let mut stills = STILLS.lock().unwrap_or_else(|e| e.into_inner());
        if self.hardware {
            if let Some(i) = stills.busy.iter().position(|e| *e == self.element) {
                stills.busy.swap_remove(i);
            }
        }
        if let Some(decoder) = self.decoder.take() {
            stills.idle.push((std::mem::take(&mut self.element), decoder));
        }
        STILL_FREED.notify_all();
    }
}

/// One picture's access unit (a single intra frame) decoded on a kept stream decoder of the
/// element that takes it, or a new one. The element is chosen per picture as `decodebin`
/// would for its file (a hardware decoder's size range rules some pictures out), since a
/// running pipeline cannot change it. A decoder that failed a picture is dropped.
pub(crate) fn decode_still(au: &FirstAccessUnit) -> Result<DecodedFrame, VideoFileError> {
    let gst = LibGst::get().ok_or_else(|| VideoFileError::new("GStreamer runtime not found"))?;
    let caps = format!(
        "{}, stream-format=(string)byte-stream, alignment=(string)au, width=(int){}, height=(int){}",
        match au.codec { StreamVideoCodec::H264 => "video/x-h264", StreamVideoCodec::Hevc => "video/x-h265" },
        au.width,
        au.height
    );
    let (element, hardware) = gst.best_decoder(&caps).ok_or_else(|| VideoFileError::new(format!("no installed decoder takes {caps}")))?;
    let mut out = {
        let mut stills = STILLS.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if let Some(i) = stills.idle.iter().position(|(e, _)| *e == element) {
                let decoder = stills.idle.swap_remove(i).1;
                if hardware {
                    stills.busy.push(element.clone());
                }
                break Out { element, hardware, decoder: Some(decoder) };
            }
            if !(hardware && stills.busy.contains(&element)) {
                if hardware {
                    stills.busy.push(element.clone());
                }
                break Out { element, hardware, decoder: None };
            }
            stills = STILL_FREED.wait(stills).unwrap_or_else(|e| e.into_inner());
        }
    };
    let mut decoder = match out.decoder.take() {
        Some(decoder) => decoder,
        None => LinuxStreamDecoder::with_decoder(au.codec, &out.element)?,
    };
    let frame = decoder.decode_one(&au.annex_b, STILL_PTS.fetch_add(1, Ordering::Relaxed))?;
    out.decoder = Some(decoder);
    Ok(frame)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Access units from GStreamer's own test source and encoder, decoded back.
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
        let gst = LibGst::get().expect("GStreamer runtime");
        let units = encoded_test_units(gst, 30).expect("GStreamer test-source and x264 encoder pipeline");
        assert_eq!(units.len(), 30);
        let mut decoder = LinuxStreamDecoder::new(StreamVideoCodec::H264).expect("stream decoder");
        let mut frames = Vec::new();
        // paced: a unit goes in while fewer than 4 of the pictures pushed are still to come out.
        // The live sink keeps 4 and drops the oldest, so the 30 pushed at once lost pictures
        // whenever a loaded box kept this thread from pulling (11-07); paced, none is lost however
        // slow the box. (A decoder still holding 4 after 10 s quiet takes the next one anyway.)
        for (i, unit) in units.iter().enumerate() {
            let mut quiet = 0;
            while i.saturating_sub(frames.len()) >= 4 && quiet < 500 {
                let more = decoder.flush().expect("flush");
                quiet = if more.is_empty() { quiet + 1 } else { 0 };
                frames.extend(more);
            }
            frames.extend(decoder.push_packet(unit, i as i64 * 333_333).expect("push"));
        }
        // every picture, until the decoder has gone 10 s (500 flushes of 20 ms) without one
        let mut quiet = 0;
        while frames.len() < units.len() && quiet < 500 {
            let more = decoder.flush().expect("flush");
            quiet = if more.is_empty() { quiet + 1 } else { 0 };
            frames.extend(more);
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
