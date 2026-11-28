//! Linux stream DECODE arm of [`crate::VideoStreamDecoder`]: GStreamer through
//! the same dlopen table as the file arm (`linux_gst_sys`). Annex-B access
//! units go into an `appsrc`, `decodebin` picks the installed decoder (a VA
//! one where present, else libav), and tight NV12 comes out of an `appsink`.
//!
//! Push and pull are decoupled inside GStreamer, so a pushed unit's frame can
//! come out with a later push: `push_packet` returns what is ready by then
//! (a decoder with frame threads holds a few frames: tens of milliseconds of
//! latency, fine for a camera view; not for a sub-frame realtime stream).
//! The low-latency decoder waits briefly in each push. Later pictures can be polled.
//! Planes are read where the buffer's layout puts them and repacked tight.

use crate::linux_decoder::{caps_field, colorimetry_to_color, sample_nv12_into, BUS_LOOK_NS};
use crate::linux_gst_sys::*;
use crate::mp4_first_frame::FirstAccessUnit;
use crate::stream_decoder::{DecodedFrame, PictureBuffers, StreamVideoCodec};
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
    /// How long a push waits for a picture when none is out yet (0: it doesn't).
    wait_ns: u64,
    buffers: PictureBuffers,
}

unsafe impl Send for LinuxStreamDecoder {}

/// How long a low-latency decoder's push waits for its picture: decoding takes about a
/// millisecond. A busy decoder can deliver the picture later through `ready_into`.
const PICTURE_WAIT_NS: u64 = 20_000_000;

const GST_RUNTIME_MISSING: &str =
    "GStreamer runtime not found (install gstreamer1.0-plugins-base, -plugins-good, -plugins-bad and -libav)";

impl LinuxStreamDecoder {
    pub fn new(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        Self::with_decoder(codec, "decodebin")
    }

    /// Decodes each picture as soon as its unit arrives. Requires GStreamer's
    /// `avdec_h264` or `avdec_h265` (gstreamer1.0-libav) and returns an error if absent.
    pub fn new_low_latency(codec: StreamVideoCodec) -> Result<Self, VideoFileError> {
        let software = match codec { StreamVideoCodec::H264 => "avdec_h264", StreamVideoCodec::Hevc => "avdec_h265" };
        Self::new_low_latency_with(codec, software)
    }

    fn new_low_latency_with(codec: StreamVideoCodec, decoder: &str) -> Result<Self, VideoFileError> {
        let gst = LibGst::get().ok_or_else(|| VideoFileError::new(GST_RUNTIME_MISSING))?;
        if !gst.has_element(decoder) {
            let codec_name = match codec { StreamVideoCodec::H264 => "H.264", StreamVideoCodec::Hevc => "HEVC" };
            return Err(VideoFileError::new(format!(
                "low-latency {codec_name} decode requires GStreamer's {decoder} element (install gstreamer1.0-libav)"
            )));
        }
        let mut this = Self::with_decoder(codec, decoder)?;
        this.wait_ns = PICTURE_WAIT_NS;
        Ok(this)
    }

    /// A stream decoder through the `decoder` element (`decodebin`: the one it plugs).
    fn with_decoder(codec: StreamVideoCodec, decoder: &str) -> Result<Self, VideoFileError> {
        let gst = LibGst::get().ok_or_else(|| VideoFileError::new(GST_RUNTIME_MISSING))?;
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
                // a missing element leaves a pipeline that never decodes: refuse it too
                let text = std::ffi::CStr::from_ptr((*err).message).to_string_lossy().into_owned();
                (gst.g_error_free)(err);
                if !pipeline.is_null() { (gst.gst_object_unref)(pipeline as *mut c_void); }
                return Err(VideoFileError::new(format!("stream decoder pipeline: {text}")));
            }
            if pipeline.is_null() { return Err(VideoFileError::new("stream decoder pipeline failed")); }
            let src_name = CString::new("src").unwrap();
            let sink_name = CString::new("sink").unwrap();
            let appsrc = (gst.gst_bin_get_by_name)(pipeline, src_name.as_ptr());
            let appsink = (gst.gst_bin_get_by_name)(pipeline, sink_name.as_ptr());
            let bus = (gst.gst_element_get_bus)(pipeline);
            gst.give_shared_contexts(pipeline);
            let this = Self { gst, pipeline, appsrc, appsink, bus, wait_ns: 0, buffers: PictureBuffers::default() };
            if appsrc.is_null() || appsink.is_null() { return Err(VideoFileError::new("stream decoder pipeline lacks its ends")); }
            if (gst.gst_element_set_state)(pipeline, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE {
                let text = gst.bus_error(bus).unwrap_or_else(|| "pipeline refused PLAYING".into());
                return Err(VideoFileError::new(format!("stream decoder start: {text}")));
            }
            Ok(this)
        }
    }

    pub fn push_packet_into(&mut self, annex_b: &[u8], pts_100ns: i64, out: &mut Vec<DecodedFrame>) -> Result<(), VideoFileError> {
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
        let start = out.len();
        self.drain_into(0, out)?;
        if self.wait_ns > 0 {
            // Wait for this push's own picture, not just any: one that missed an earlier push's
            // wait (a slow first picture after a rebuild) is ready now, and returning with only
            // it would leave every later picture one push late.
            let own = |frames: &[DecodedFrame]| frames.iter().any(|f| f.pts_100ns == pts_100ns.max(0));
            let deadline = std::time::Instant::now() + std::time::Duration::from_nanos(self.wait_ns);
            while !own(&out[start..]) {
                let left = deadline.saturating_duration_since(std::time::Instant::now()).as_nanos() as u64;
                if left == 0 {
                    break;
                }
                let before = out.len();
                self.drain_into(left, out)?;
                if out.len() == before {
                    break;
                }
            }
        }
        Ok(())
    }

    pub fn ready_into(&mut self, out: &mut Vec<DecodedFrame>) -> Result<(), VideoFileError> {
        if let Some(text) = self.gst.bus_drain(self.bus, false) {
            return Err(VideoFileError::new(format!("stream decoder: {text}")));
        }
        self.drain_into(0, out)
    }

    /// Frames ready now; `wait_ns` bounds the wait for the first one.
    fn drain_into(&mut self, wait_ns: u64, out: &mut Vec<DecodedFrame>) -> Result<(), VideoFileError> {
        let mut wait = wait_ns;
        loop {
            let sample = unsafe { (self.gst.gst_app_sink_try_pull_sample)(self.appsink, wait) };
            if sample.is_null() { break; }
            wait = 0;
            let frame = unsafe { self.frame(sample) };
            unsafe { (self.gst.gst_mini_object_unref)(sample as *mut c_void) };
            out.push(frame?);
        }
        Ok(())
    }

    unsafe fn frame(&mut self, sample: *mut GstSample) -> Result<DecodedFrame, VideoFileError> {
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
        let nv12 = sample_nv12_into(self.gst, sample, width, height, || self.buffers.take())?;
        let pts = (*((self.gst.gst_sample_get_buffer)(sample) as *const GstBufferRepr)).pts;
        Ok(DecodedFrame { width, height, nv12, pts_100ns: if pts == GST_CLOCK_TIME_NONE { 0 } else { (pts / 100) as i64 }, gpu: None, color })
    }

    pub fn flush_into(&mut self, out: &mut Vec<DecodedFrame>) -> Result<(), VideoFileError> {
        self.drain_into(20_000_000, out)
    }

    pub fn recycle(&mut self, nv12: Vec<u8>) {
        self.buffers.recycle(nv12);
    }

    pub fn allocated_buffers(&self) -> u64 {
        self.buffers.allocated
    }

    /// Pushes one still's access unit and waits for its picture. If it arrives during
    /// the first bus interval, the decoder can take another still. Otherwise the codec
    /// may need more input to release its picture, so end the stream to drain it.
    fn decode_one(mut self, annex_b: &[u8], pts_100ns: i64) -> Result<(DecodedFrame, Option<Self>), VideoFileError> {
        let mut frames = Vec::new();
        self.wait_ns = BUS_LOOK_NS;
        self.push_packet_into(annex_b, pts_100ns, &mut frames)?;
        let mut draining = false;
        loop {
            if let Some(i) = frames.iter().position(|f| f.pts_100ns == pts_100ns) {
                return Ok((frames.swap_remove(i), (!draining).then_some(self)));
            }
            if let Some(text) = self.gst.bus_drain(self.bus, true) {
                return Err(VideoFileError::new(format!("stream decoder: {text}")));
            }
            if unsafe { (self.gst.gst_app_sink_is_eos)(self.appsink) } != 0 {
                return Err(VideoFileError::new("stream decoder ended with no picture for the access unit"));
            }
            if !draining {
                if unsafe { (self.gst.gst_app_src_end_of_stream)(self.appsrc) } != GST_FLOW_OK {
                    return Err(VideoFileError::new("stream decoder refused end of stream"));
                }
                draining = true;
            }
            frames.clear();
            self.drain_into(BUS_LOOK_NS, &mut frames)?;
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
/// running pipeline cannot change it. A drained or failed decoder is dropped.
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
    let decoder = match out.decoder.take() {
        Some(decoder) => decoder,
        None => LinuxStreamDecoder::with_decoder(au.codec, &out.element)?,
    };
    let (frame, reusable) = decoder.decode_one(&au.annex_b, STILL_PTS.fetch_add(1, Ordering::Relaxed))?;
    out.decoder = reusable;
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
    fn software_avdec_h264_ready_into_collects_late_picture_and_decodes_delta() {
        use crate::annex_b::{nal_unit_type, split_annex_b, NAL_TYPE_IDR};
        let gst = LibGst::get().expect("GStreamer runtime");
        let units = encoded_test_units(gst, 2).expect("GStreamer test-source and x264 encoder pipeline");
        assert_eq!(units.len(), 2, "the encoder supplies an IDR followed by a delta");
        let mut decoder = LinuxStreamDecoder::new_low_latency(StreamVideoCodec::H264).expect("low-latency H.264 decoder");
        decoder.wait_ns = 0;
        for (i, unit) in units.iter().enumerate() {
            let pts = i as i64 * 333_333;
            let idr = split_annex_b(unit).into_iter().any(|nal| nal_unit_type(nal) == NAL_TYPE_IDR);
            assert_eq!(idr, i == 0, "only the first unit can decode independently");
            let mut out = Vec::new();
            decoder.push_packet_into(unit, pts, &mut out).expect("push one unit");
            loop {
                decoder.ready_into(&mut out).expect("collect the picture without another push");
                if !out.is_empty() { break }
                std::thread::sleep(std::time::Duration::from_millis(1));
            }
            assert_eq!(out.len(), 1, "each push produces exactly one picture");
            assert_eq!((out[0].pts_100ns, out[0].width, out[0].height), (pts, 320, 240), "the picture matches this unit");
        }
    }

    #[test]
    fn new_low_latency_reports_missing_decoder_element_without_falling_back() {
        let error = LinuxStreamDecoder::new_low_latency_with(StreamVideoCodec::H264, "makepad_missing_avdec_test")
            .err().expect("a missing low-latency element must fail to open");
        assert_eq!(error.to_string(),
            "low-latency H.264 decode requires GStreamer's makepad_missing_avdec_test element (install gstreamer1.0-libav)",
            "the error identifies the missing element and its package");
    }

    #[test]
    fn decodes_a_live_h264_stream_where_gstreamer_is_installed() {
        let gst = LibGst::get().expect("GStreamer runtime");
        let units = encoded_test_units(gst, 30).expect("GStreamer test-source and x264 encoder pipeline");
        assert_eq!(units.len(), 30);
        let mut decoder = crate::VideoStreamDecoder::new(StreamVideoCodec::H264).expect("stream decoder");
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

        // The low-latency decoder: each picture out of its own push (a busy box may make one
        // late). The first push doesn't wait, so its picture is late on purpose, as a slow first
        // picture after a decoder rebuild is: it comes out with the second push, and the later
        // pictures still come out of their own pushes, not one push late.
        // Every picture's buffer goes back after each push, so the decoder allocates only when
        // more pictures come out of one push than it has spares: as many as the largest batch.
        let mut decoder = LinuxStreamDecoder::new_low_latency(StreamVideoCodec::H264).expect("low-latency stream decoder");
        let (mut own, mut first_by_second) = (0, false);
        let (mut out, mut returned, mut reused, mut largest) = (Vec::new(), Vec::new(), 0, 0);
        for (i, unit) in units.iter().enumerate() {
            let pts = i as i64 * 333_333;
            decoder.wait_ns = if i == 0 { 0 } else { PICTURE_WAIT_NS };
            decoder.push_packet_into(unit, pts, &mut out).expect("push");
            first_by_second |= i <= 1 && out.iter().any(|f| f.pts_100ns == 0);
            own += out.iter().filter(|f| i > 0 && f.pts_100ns == pts).count();
            largest = largest.max(out.len());
            for f in out.drain(..) {
                assert_eq!(f.nv12.len(), 320 * 240 * 3 / 2, "low latency: picture at {}", f.pts_100ns);
                reused += usize::from(returned.contains(&f.nv12.as_ptr()));
                returned.push(f.nv12.as_ptr());
                decoder.recycle(f.nv12);
            }
        }
        assert!(first_by_second, "low latency: the late first picture didn't come out by the second push");
        assert!(own >= units.len() - 4, "low latency: {own} of {} later pictures came out of their own push", units.len() - 1);
        assert_eq!(decoder.allocated_buffers(), largest as u64, "low latency: buffers allocated against the largest batch of {largest}");
        assert_eq!(reused, returned.len() - largest, "low latency: every picture after the first batch reuses a returned buffer");
    }
}
