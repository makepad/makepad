//! Linux file decoder: GStreamer `uridecodebin` behind the same facade as
//! the AVAssetReader / Media Foundation arms — an mp4 (or anything the
//! installed plugins demux, `gstreamer1.0-libav` included) in, tightly
//! packed NV12 frames and interleaved 16-bit PCM audio out. Picture and
//! sound use independent pipelines, so reading either cannot block the
//! other. `open_audio` negotiates only sound, including in picture-less files.
//!
//! `info()` names the compressed video codec from the caps the demuxer
//! hands the picture decoder (`video_codec_fourcc` stays 0: caps carry no
//! FourCC). A decoded picture's planes are read where its buffer says they
//! are (its video meta, else its caps' default layout, whose rows are padded
//! to 4 bytes) and copied into tight rows; a buffer shorter than its layout
//! is refused loudly, as are incomplete interleaved PCM frames.

use crate::linux_gst_sys::*;
use crate::{DecodedAudioChunk, DecodedVideoFrame, VideoColor, VideoFileCodec, VideoFileError, VideoFileInfo};
use std::ffi::{c_void, CStr, CString};
use std::ptr;

pub(crate) struct LinuxVideoFileDecoder {
    gst: &'static LibGst,
    pipeline: *mut GstElement,
    appsink: *mut GstElement,
    bus: *mut GstBus,
    audio_pipeline: *mut GstElement,
    audio_sink: *mut GstElement,
    audio_bus: *mut GstBus,
    /// The first chunk, pulled at open for the stream's format; next_audio delivers it first.
    audio_first: *mut GstSample,
    info: VideoFileInfo,
    /// The colour of the delivered bytes (the negotiated caps' colorimetry).
    frame_color: VideoColor,
}

unsafe impl Send for LinuxVideoFileDecoder {}

/// How often a waiting sink checks for progress or a terminal bus message.
pub(crate) const BUS_LOOK_NS: u64 = 100_000_000;
/// Allow a quiet poll between one upstream task pausing and the next starting.
pub(crate) const QUIET_STALL_POLLS: u32 = 2;
/// Fresh starts allowed after a confirmed stall when the container is not
/// proved structurally unplayable.
pub(crate) const MAX_STALL_REOPENS: usize = 5;
/// Base retry spacing, multiplied by the 1-based reopen number.
const REOPEN_BACKOFF_MS: u64 = 20;
/// At the inspection bound, treat the file as potentially playable and reopen.
const MAX_ISO_BMFF_TOP_BOXES: usize = 1024;

pub(crate) fn display_file_name(path: &str) -> &str {
    std::path::Path::new(path)
        .file_name()
        .and_then(|name| name.to_str())
        .filter(|name| !name.is_empty())
        .unwrap_or("<unnamed>")
}

/// If a regular ISO-BMFF file starts with `ftyp` and its top-level boxes prove
/// it has no complete `moov`, return the reason. On I/O errors, allow a reopen.
pub(crate) fn unplayable_iso_bmff_reason(path: &str) -> Option<&'static str> {
    let mut file = std::fs::File::open(path).ok()?;
    let meta = file.metadata().ok()?;
    if !meta.is_file() {
        return None;
    }
    unplayable_iso_bmff_reader(&mut file, meta.len())
}

pub(crate) fn unplayable_iso_bmff_reader<R: std::io::Read + std::io::Seek>(
    reader: &mut R,
    file_len: u64,
) -> Option<&'static str> {
    use std::io::SeekFrom;
    if file_len < 16 {
        return None;
    }
    let mut head = [0u8; 16];
    reader.seek(SeekFrom::Start(0)).ok()?;
    reader.read_exact(&mut head[..8]).ok()?;
    if &head[4..8] != b"ftyp" {
        return None;
    }
    let ftyp_size = match u32::from_be_bytes(head[0..4].try_into().unwrap()) {
        0 => return Some("ISO-BMFF container has only an ftyp box and no moov box"),
        1 => {
            if file_len < 24 {
                return Some("truncated ISO-BMFF ftyp box");
            }
            reader.read_exact(&mut head[8..16]).ok()?;
            let size = u64::from_be_bytes(head[8..16].try_into().unwrap());
            if size < 24 || size > file_len {
                return Some("invalid or truncated ISO-BMFF ftyp box");
            }
            size
        }
        size if (size as u64) < 16 || (size as u64) > file_len => {
            return Some("invalid or truncated ISO-BMFF ftyp box");
        }
        size => size as u64,
    };

    let mut offset = ftyp_size;
    for _ in 0..MAX_ISO_BMFF_TOP_BOXES {
        if offset == file_len {
            return Some("ISO-BMFF container has no moov box");
        }
        let remaining = file_len.checked_sub(offset)?;
        if remaining < 8 {
            return Some("truncated ISO-BMFF box header before moov");
        }
        reader.seek(SeekFrom::Start(offset)).ok()?;
        reader.read_exact(&mut head[..8]).ok()?;
        let size32 = u32::from_be_bytes(head[0..4].try_into().unwrap());
        let fourcc: [u8; 4] = head[4..8].try_into().unwrap();
        let (header_len, box_size) = match size32 {
            0 => {
                if &fourcc == b"moov" && remaining >= 16 {
                    return None;
                }
                return Some("ISO-BMFF container ends without a moov box");
            }
            1 => {
                if remaining < 16 {
                    return Some("truncated ISO-BMFF extended box header before moov");
                }
                reader.read_exact(&mut head[8..16]).ok()?;
                let size64 = u64::from_be_bytes(head[8..16].try_into().unwrap());
                if size64 < 16 {
                    return Some("invalid ISO-BMFF box size before moov");
                }
                if size64 > remaining {
                    return Some("truncated ISO-BMFF box before moov");
                }
                (16u64, size64)
            }
            2..=7 => return Some("invalid ISO-BMFF box size before moov"),
            n => {
                let size = n as u64;
                if size > remaining {
                    return Some("truncated ISO-BMFF box before moov");
                }
                (8u64, size)
            }
        };
        if &fourcc == b"moov" {
            return if box_size >= header_len + 8 {
                None
            } else {
                Some("ISO-BMFF moov box is empty")
            };
        }
        offset = offset.checked_add(box_size)?;
    }
    None
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PrerollProbe {
    pub saw_bus_activity: bool,
    pub pad_linked: bool,
    pub source_settled_paused: bool,
    pub saw_upstream_task: bool,
    pub active_upstream_tasks: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StallReason {
    NoStreamSettled,
    UpstreamPaused,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum StallAction {
    Reopen { next_attempt: usize, backoff_ms: u64 },
    Fail(String),
}

impl StallReason {
    pub(crate) fn decide(
        self,
        stream: &str,
        attempt: usize,
        detail: &str,
        unplayable: Option<&str>,
    ) -> StallAction {
        match self {
            Self::NoStreamSettled => StallAction::Fail(format!(
                "no {stream} stream in file (decoder settled without exposing a stream: {detail})"
            )),
            Self::UpstreamPaused => {
                if let Some(reason) = unplayable {
                    return StallAction::Fail(format!(
                        "no {stream} stream in file ({reason}; decoder preroll stalled: {detail})"
                    ));
                }
                if attempt < MAX_STALL_REOPENS {
                    let next_attempt = attempt + 1;
                    StallAction::Reopen {
                        next_attempt,
                        backoff_ms: REOPEN_BACKOFF_MS * next_attempt as u64,
                    }
                } else {
                    StallAction::Fail(format!(
                        "no {stream} stream in file (decoder preroll stalled without error or EOS after {MAX_STALL_REOPENS} reopens: {detail})"
                    ))
                }
            }
        }
    }

    fn handle(
        self,
        stream: &str,
        path: &str,
        attempt: usize,
        detail: &str,
    ) -> Result<u64, VideoFileError> {
        let file = display_file_name(path);
        let unplayable = matches!(self, Self::UpstreamPaused)
            .then(|| unplayable_iso_bmff_reason(path))
            .flatten();
        match self.decide(stream, attempt, detail, unplayable) {
            StallAction::Reopen { next_attempt, backoff_ms } => {
                eprintln!(
                    "makepad-video: linux {stream} preroll stalled on {file} ({detail}); reopening ({next_attempt}/{MAX_STALL_REOPENS})"
                );
                Ok(backoff_ms)
            }
            StallAction::Fail(message) => {
                if let Some(reason) = unplayable {
                    eprintln!(
                        "makepad-video: linux {stream} preroll stalled on {file} ({detail}); not reopening ({reason})"
                    );
                } else if matches!(self, Self::UpstreamPaused) {
                    eprintln!(
                        "makepad-video: linux {stream} preroll stalled on {file} after {MAX_STALL_REOPENS} reopens ({detail})"
                    );
                }
                Err(VideoFileError::new(message))
            }
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StallState {
    Progressing,
    Stalled(StallReason),
}

#[derive(Default, Debug)]
pub(crate) struct PrerollStallDetector {
    quiet_stalls: u32,
}

impl PrerollStallDetector {
    pub(crate) fn observe(&mut self, probe: PrerollProbe) -> StallState {
        if probe.saw_bus_activity || probe.pad_linked || probe.active_upstream_tasks > 0
            || !(probe.source_settled_paused || probe.saw_upstream_task)
        {
            self.quiet_stalls = 0;
            return StallState::Progressing;
        }
        self.quiet_stalls = self.quiet_stalls.saturating_add(1);
        if self.quiet_stalls < QUIET_STALL_POLLS {
            StallState::Progressing
        } else if probe.source_settled_paused {
            StallState::Stalled(StallReason::NoStreamSettled)
        } else {
            StallState::Stalled(StallReason::UpstreamPaused)
        }
    }
}

/// An owned element or pad reference used only while opening a pipeline.
struct GstRef {
    gst: &'static LibGst,
    ptr: *mut c_void,
}

impl GstRef {
    unsafe fn element(gst: &'static LibGst, pipeline: *mut GstElement, name: &CStr) -> Result<Self, VideoFileError> {
        let ptr = (gst.gst_bin_get_by_name)(pipeline, name.as_ptr());
        if ptr.is_null() {
            return Err(VideoFileError::new(format!("decoder pipeline has no {}", name.to_string_lossy())));
        }
        Ok(Self { gst, ptr })
    }

    unsafe fn sink_pad(&self) -> Result<Self, VideoFileError> {
        let ptr = (self.gst.gst_element_get_static_pad)(self.ptr, c"sink".as_ptr());
        if ptr.is_null() {
            return Err(VideoFileError::new("decoder link target has no sink pad"));
        }
        Ok(Self { gst: self.gst, ptr })
    }
}

impl Drop for GstRef {
    fn drop(&mut self) {
        unsafe { (self.gst.gst_object_unref)(self.ptr) }
    }
}

enum BusPoll {
    Quiet,
    Activity,
    Error(String),
    Eos,
}

impl BusPoll {
    fn activity(self, stream: &str) -> Result<bool, VideoFileError> {
        let text = match self {
            Self::Quiet => return Ok(false),
            Self::Activity => return Ok(true),
            Self::Error(text) => text,
            Self::Eos => "end of stream before preroll".to_string(),
        };
        Err(VideoFileError::new(format!("no {stream} stream in file (decoder preroll: {text})")))
    }
}

struct TrackedTask {
    task: *mut c_void,
    owner: String,
}

/// Tracks tasks announced on the bus. The downstream queue waits on its unlinked
/// sink with its task started, so only upstream owners can signal progress.
struct PrerollWatch {
    gst: &'static LibGst,
    task_type: usize,
    ignored_owner: *mut GstElement,
    tasks: Vec<TrackedTask>,
    saw_upstream_task: bool,
    detector: PrerollStallDetector,
}

impl PrerollWatch {
    unsafe fn new(gst: &'static LibGst, ignored_owner: *mut GstElement) -> Self {
        Self {
            gst,
            task_type: (gst.gst_task_get_type)(),
            ignored_owner,
            tasks: Vec::new(),
            saw_upstream_task: false,
            detector: PrerollStallDetector::default(),
        }
    }

    unsafe fn stream_status(&mut self, msg: *mut GstMessage) {
        let gst = self.gst;
        let mut status = 0;
        let mut owner = ptr::null_mut();
        (gst.gst_message_parse_stream_status)(msg, &mut status, &mut owner);
        if owner.is_null() || owner == self.ignored_owner {
            return;
        }
        let value = (gst.gst_message_get_stream_status_object)(msg);
        if value.is_null() || (*value).g_type != self.task_type {
            return;
        }
        let task = (gst.g_value_get_object)(value);
        if task.is_null() {
            return;
        }
        let index = self.tasks.iter().position(|tracked| tracked.task == task);
        match (status, index) {
            (GST_STREAM_STATUS_TYPE_CREATE | GST_STREAM_STATUS_TYPE_ENTER, None) => {
                let name = (gst.gst_object_get_name)(owner);
                let owner = if name.is_null() {
                    "<unnamed>".to_string()
                } else {
                    let text = CStr::from_ptr(name).to_string_lossy().into_owned();
                    (gst.g_free)(name.cast());
                    text
                };
                self.tasks.push(TrackedTask { task: (gst.gst_object_ref)(task), owner });
                self.saw_upstream_task = true;
            }
            (GST_STREAM_STATUS_TYPE_DESTROY, Some(index)) => {
                (gst.gst_object_unref)(self.tasks.swap_remove(index).task);
            }
            _ => {}
        }
    }

    unsafe fn wait_bus(&mut self, bus: *mut GstBus, timeout_ns: u64) -> BusPoll {
        let mut msg = (self.gst.gst_bus_timed_pop_filtered)(bus, timeout_ns, GST_MESSAGE_ANY);
        if msg.is_null() {
            return BusPoll::Quiet;
        }
        let mut poll = BusPoll::Activity;
        loop {
            match (*(msg as *const GstMessageRepr)).mtype {
                GST_MESSAGE_ERROR if !matches!(poll, BusPoll::Error(_)) => {
                    poll = BusPoll::Error(self.gst.take_error_text(msg));
                }
                GST_MESSAGE_EOS if !matches!(poll, BusPoll::Error(_)) => poll = BusPoll::Eos,
                GST_MESSAGE_STREAM_STATUS => self.stream_status(msg),
                _ => {}
            }
            (self.gst.gst_mini_object_unref)(msg);
            msg = (self.gst.gst_bus_timed_pop_filtered)(bus, 0, GST_MESSAGE_ANY);
            if msg.is_null() {
                return poll;
            }
        }
    }

    unsafe fn probe(&self, bus_activity: bool, dec: *mut GstElement, link_pad: *mut c_void) -> PrerollProbe {
        let (mut state, mut pending) = (0, 0);
        let change = (self.gst.gst_element_get_state)(dec, &mut state, &mut pending, 0);
        PrerollProbe {
            saw_bus_activity: bus_activity,
            pad_linked: (self.gst.gst_pad_is_linked)(link_pad) != 0,
            source_settled_paused: matches!(change, GST_STATE_CHANGE_SUCCESS | GST_STATE_CHANGE_NO_PREROLL)
                && state == GST_STATE_PAUSED && pending == GST_STATE_VOID_PENDING,
            saw_upstream_task: self.saw_upstream_task,
            active_upstream_tasks: self.tasks.iter().filter(|t| (self.gst.gst_task_get_state)(t.task) == GST_TASK_STARTED).count(),
        }
    }

    unsafe fn observe(&mut self, bus_activity: bool, dec: *mut GstElement, link_pad: *mut c_void) -> StallState {
        self.detector.observe(self.probe(bus_activity, dec, link_pad))
    }

    unsafe fn describe(&self, dec: *mut GstElement) -> String {
        fn state_name(state: u32) -> &'static str {
            match state {
                GST_STATE_VOID_PENDING => "VOID_PENDING",
                GST_STATE_NULL => "NULL",
                GST_STATE_READY => "READY",
                GST_STATE_PAUSED => "PAUSED",
                GST_STATE_PLAYING => "PLAYING",
                _ => "unknown",
            }
        }
        let (mut state, mut pending) = (0, 0);
        (self.gst.gst_element_get_state)(dec, &mut state, &mut pending, 0);
        let tasks = self.tasks.iter().map(|t| {
            let state = match (self.gst.gst_task_get_state)(t.task) {
                GST_TASK_STARTED => "started",
                GST_TASK_PAUSED => "paused",
                GST_TASK_STOPPED => "stopped",
                _ => "unknown",
            };
            format!("{}={state}", t.owner)
        }).collect::<Vec<_>>();
        format!("sink unlinked, decoder {}, pending {}, upstream tasks [{}]", state_name(state), state_name(pending), tasks.join(", "))
    }
}

impl Drop for PrerollWatch {
    fn drop(&mut self) {
        for tracked in &self.tasks {
            unsafe { (self.gst.gst_object_unref)(tracked.task) }
        }
    }
}

enum PrerollOutcome {
    Sample(*mut GstSample),
    Stalled(StallReason, String),
}

enum SettleOutcome {
    Settled,
    Stalled(StallReason, String),
}

/// The codec a compressed picture stream's caps name (`video/x-h264, ...`).
fn compressed_codec(caps: &str) -> Option<VideoFileCodec> {
    match caps.split(',').next().unwrap_or("").trim() {
        "video/x-h264" => Some(VideoFileCodec::H264),
        "video/x-h265" => Some(VideoFileCodec::H265),
        _ => None,
    }
}

/// The `width` x `height` NV12 picture laid out in `bytes` as `layout`,
/// copied into tight rows (`height` luma rows, then `height.div_ceil(2)`
/// chroma rows, each `width` bytes). None when a plane reaches past `bytes`.
#[cfg(test)]
fn tight_nv12(bytes: &[u8], layout: Nv12Layout, width: u32, height: u32) -> Option<Vec<u8>> {
    tight_nv12_into(bytes, layout, width, height, Vec::new)
}

fn tight_nv12_into(bytes: &[u8], layout: Nv12Layout, width: u32, height: u32, buffer: impl FnOnce() -> Vec<u8>) -> Option<Vec<u8>> {
    let (w, h) = (width as usize, height as usize);
    let planes = [(layout.luma, h), (layout.chroma, h.div_ceil(2))];
    for ((offset, stride), rows) in planes {
        if rows > 0 && (stride < w || offset + stride * (rows - 1) + w > bytes.len()) {
            return None;
        }
    }
    let mut out = buffer();
    out.clear();
    out.reserve_exact(w * (h + h.div_ceil(2)));
    for ((offset, stride), rows) in planes {
        if stride == w {
            out.extend_from_slice(&bytes[offset..offset + w * rows]);
        } else {
            for row in 0..rows {
                out.extend_from_slice(&bytes[offset + row * stride..][..w]);
            }
        }
    }
    Some(out)
}

/// A decoded NV12 sample's `width` x `height` picture in tight rows,
/// its planes found by [`LibGst::nv12_layout`].
pub(crate) unsafe fn sample_nv12(gst: &LibGst, sample: *mut GstSample, width: u32, height: u32) -> Result<Vec<u8>, VideoFileError> {
    sample_nv12_into(gst, sample, width, height, Vec::new)
}

pub(crate) unsafe fn sample_nv12_into(gst: &LibGst, sample: *mut GstSample, width: u32, height: u32, buffer_for_picture: impl FnOnce() -> Vec<u8>) -> Result<Vec<u8>, VideoFileError> {
    let buffer = (gst.gst_sample_get_buffer)(sample);
    if buffer.is_null() {
        return Err(VideoFileError::new("sample without buffer"));
    }
    let Some(layout) = gst.nv12_layout(buffer, (gst.gst_sample_get_caps)(sample)) else {
        return Err(VideoFileError::new("decoded picture has no NV12 plane layout (no video meta, no video caps)"));
    };
    let mut map = GstMapInfo::default();
    if (gst.gst_buffer_map)(buffer, &mut map, GST_MAP_READ) == 0 {
        return Err(VideoFileError::new("buffer map failed"));
    }
    let bytes = if map.size == 0 { &[][..] } else { std::slice::from_raw_parts(map.data, map.size) };
    let nv12 = tight_nv12_into(bytes, layout, width, height, buffer_for_picture);
    let size = map.size;
    (gst.gst_buffer_unmap)(buffer, &mut map);
    nv12.ok_or_else(|| VideoFileError::new(format!("{width}x{height} NV12 laid out as {layout:?} reaches past its {size}-byte buffer")))
}

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
                audio_first: std::ptr::null_mut(),
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
                decoder.open_video_pipeline(path, &uri)?;
            }
            if let Err(error) = decoder.open_audio_pipeline(path, &uri) {
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

    /// The picture pipeline of `uri`, started and prerolled. A start that fails names the picture
    /// decoder decodebin had plugged by then (a hardware one such as `nvh264dec`, or libav's): the
    /// error alone says which element stopped the stream, not what was decoding it.
    fn open_video_pipeline(&mut self, path: &str, uri: &str) -> Result<(), VideoFileError> {
        self.start_video_pipeline(path, uri).map_err(|error| match unsafe { self.plugged_picture_decoder() } {
            Some(decoder) => VideoFileError::new(format!("{error} (picture decoder {decoder})")),
            None => error,
        })
    }

    fn start_video_pipeline(&mut self, path: &str, uri: &str) -> Result<(), VideoFileError> {
        let gst = self.gst;
        unsafe {
            // One queued frame is enough for pull decoding. A caller reading
            // only audio leaves this pipeline back-pressured, without drops
            // or accumulating the entire decoded picture track in memory.
            let launch = format!(
                "uridecodebin name=dec uri=\"{uri}\" ! queue name=q ! videoconvert ! \
                 video/x-raw,format=NV12 ! appsink name=out sync=false max-buffers=1"
            );
            for attempt in 0..=MAX_STALL_REOPENS {
                let outcome = {
                    self.pipeline = Self::parse_pipeline(gst, &launch)?;
                    gst.give_shared_contexts(self.pipeline);
                    self.appsink = (gst.gst_bin_get_by_name)(self.pipeline, c"out".as_ptr());
                    if self.appsink.is_null() {
                        return Err(VideoFileError::new("decoder pipeline has no appsink"));
                    }
                    self.bus = (gst.gst_element_get_bus)(self.pipeline);
                    if self.bus.is_null() {
                        return Err(VideoFileError::new("decoder pipeline has no bus"));
                    }
                    let dec = GstRef::element(gst, self.pipeline, c"dec")?;
                    let queue = GstRef::element(gst, self.pipeline, c"q")?;
                    let link_pad = queue.sink_pad()?;
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
                    self.pull_preroll(self.appsink, self.bus, dec.ptr, queue.ptr, link_pad.ptr, "video")?
                };
                match outcome {
                    PrerollOutcome::Sample(sample) => return self.start_prerolled_video(sample),
                    PrerollOutcome::Stalled(reason, detail) => {
                        let backoff_ms = reason.handle("video", path, attempt, &detail)?;
                        self.close_video_pipeline();
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                    }
                }
            }
            unreachable!("the final stalled attempt returns an error")
        }
    }

    fn start_prerolled_video(&mut self, preroll: *mut GstSample) -> Result<(), VideoFileError> {
        let gst = self.gst;
        unsafe {
            gst.keep_shared_contexts(self.pipeline);
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
            let video_codec = self.compressed_video_codec();
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
            self.info.video_codec = video_codec;
            self.info.color = color;
            self.frame_color = color;
            Ok(())
        }
    }

    fn open_audio_pipeline(&mut self, path: &str, uri: &str) -> Result<(), VideoFileError> {
        let gst = self.gst;
        unsafe {
            // Only the sound: a stream that cannot become audio/x-raw is dropped at the demuxer
            // (expose-all-streams=false), not decoded to find that out. Exposed, a movie's picture
            // put an H.264 decoder in every audio pipeline, a second hardware one beside the
            // picture door's own, and the pipeline could not pause until it had decoded a frame.
            let launch = format!(
                "uridecodebin name=dec uri=\"{uri}\" caps=audio/x-raw expose-all-streams=false ! audioconvert name=aconv ! audioresample ! \
                 audio/x-raw,format=S16LE,layout=interleaved ! \
                 appsink name=aout sync=false async=false max-buffers=1"
            );
            for attempt in 0..=MAX_STALL_REOPENS {
                let outcome = {
                    self.audio_pipeline = Self::parse_pipeline(gst, &launch)?;
                    gst.give_shared_contexts(self.audio_pipeline);
                    self.audio_sink = (gst.gst_bin_get_by_name)(self.audio_pipeline, c"aout".as_ptr());
                    if self.audio_sink.is_null() {
                        return Err(VideoFileError::new("audio decoder pipeline has no appsink"));
                    }
                    self.audio_bus = (gst.gst_element_get_bus)(self.audio_pipeline);
                    if self.audio_bus.is_null() {
                        return Err(VideoFileError::new("audio decoder pipeline has no bus"));
                    }
                    let dec = GstRef::element(gst, self.audio_pipeline, c"dec")?;
                    let aconv = GstRef::element(gst, self.audio_pipeline, c"aconv")?;
                    let link_pad = aconv.sink_pad()?;
                    // The sink plays from before the stream starts, its state locked so that the pipeline's
                    // state changes pass it by until close. A sink's change into PLAYING takes the lock it
                    // renders under, and a render with its one sample queued waits, holding that lock, for
                    // this thread to pull: playing the pipeline once samples flow could wait for ever
                    // (pulling the preroll, then set_state(PLAYING), lost that race on a loaded box and a
                    // render test sat 70 min in it, 11-07). Upstream of the sink PAUSED streams, so the
                    // pipeline never needs PLAYING.
                    (gst.gst_element_set_locked_state)(self.audio_sink, 1);
                    if (gst.gst_element_set_state)(self.audio_sink, GST_STATE_PLAYING) == GST_STATE_CHANGE_FAILURE {
                        let text = gst.bus_error(self.audio_bus).unwrap_or_else(|| "it refused PLAYING".to_string());
                        return Err(VideoFileError::new(format!("audio decoder sink: {text}")));
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
                    let outcome = self.settle_audio(self.audio_pipeline, self.audio_bus, dec.ptr, link_pad.ptr)?;
                    if matches!(outcome, SettleOutcome::Settled) && (gst.gst_pad_is_linked)(link_pad.ptr) == 0 {
                        return Err(VideoFileError::new("no audio stream in file"));
                    }
                    outcome
                };
                match outcome {
                    SettleOutcome::Settled => break,
                    SettleOutcome::Stalled(reason, detail) => {
                        let backoff_ms = reason.handle("audio", path, attempt, &detail)?;
                        self.close_audio_pipeline();
                        std::thread::sleep(std::time::Duration::from_millis(backoff_ms));
                    }
                }
            }
            // The first chunk names the stream's format; next_audio delivers it first.
            let Some(first) = self.pull(self.audio_sink, self.audio_bus, "audio decoder")? else {
                return Err(VideoFileError::new("no audio stream in file (it ended before its first sample)"));
            };
            self.audio_first = first;
            gst.keep_shared_contexts(self.audio_pipeline);
            let caps_text = self.sample_caps(self.audio_first);
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
            self.info.has_audio = true;
            self.info.audio_sample_rate = rate;
            self.info.audio_channels = channels;
            if self.pipeline.is_null() {
                self.info.duration_100ns = duration_ns.max(0) / 100;
            }
            Ok(())
        }
    }

    unsafe fn parse_pipeline(gst: &LibGst, launch: &str) -> Result<*mut GstElement, VideoFileError> {
        let launch_c = CString::new(launch)
            .map_err(|_| VideoFileError::new("pipeline string contains NUL"))?;
        let mut err: *mut GError = std::ptr::null_mut();
        let pipeline = (gst.gst_parse_launch)(launch_c.as_ptr(), &mut err);
        if !err.is_null() || pipeline.is_null() {
            let text = if err.is_null() {
                "gst_parse_launch failed".to_string()
            } else {
                let text = std::ffi::CStr::from_ptr((*err).message)
                    .to_string_lossy()
                    .into_owned();
                (gst.g_error_free)(err);
                text
            };
            // A recoverable parse error can still return a partial pipeline.
            if !pipeline.is_null() {
                (gst.gst_element_set_state)(pipeline, GST_STATE_NULL);
                (gst.gst_object_unref)(pipeline as *mut c_void);
            }
            return Err(VideoFileError::new(format!("decoder pipeline: {text}")));
        }
        Ok(pipeline)
    }

    unsafe fn settle_audio(
        &self,
        pipeline: *mut GstElement,
        bus: *mut GstBus,
        dec: *mut GstElement,
        link_pad: *mut c_void,
    ) -> Result<SettleOutcome, VideoFileError> {
        let mut watch = PrerollWatch::new(self.gst, ptr::null_mut());
        loop {
            match (self.gst.gst_element_get_state)(pipeline, ptr::null_mut(), ptr::null_mut(), 0) {
                GST_STATE_CHANGE_FAILURE => {
                    let text = self.gst.bus_error(bus).unwrap_or_else(|| "pipeline failed to pause".to_string());
                    return Err(VideoFileError::new(format!("no audio stream in file (decoder start: {text})")));
                }
                GST_STATE_CHANGE_ASYNC => {}
                _ => return Ok(SettleOutcome::Settled),
            }
            let poll = watch.wait_bus(bus, BUS_LOOK_NS);
            if let BusPoll::Error(text) = poll {
                return Err(VideoFileError::new(format!("no audio stream in file (decoder preroll: {text})")));
            }
            match (self.gst.gst_element_get_state)(pipeline, ptr::null_mut(), ptr::null_mut(), 0) {
                GST_STATE_CHANGE_FAILURE => {
                    let text = self.gst.bus_error(bus).unwrap_or_else(|| "pipeline failed to pause".to_string());
                    return Err(VideoFileError::new(format!("no audio stream in file (decoder start: {text})")));
                }
                GST_STATE_CHANGE_ASYNC => {}
                _ => return Ok(SettleOutcome::Settled),
            }
            let activity = poll.activity("audio")?;
            if let StallState::Stalled(reason) = watch.observe(activity, dec, link_pad) {
                return Ok(SettleOutcome::Stalled(reason, watch.describe(dec)));
            }
        }
    }

    /// The next sample of `sink`: None at its end of stream, the error its pipeline posts on
    /// `bus` (`what: <error>`) when a stream fails, which sends the sink no end of stream; a
    /// blocking pull would wait for that forever ([`BUS_LOOK_NS`]).
    unsafe fn pull(&self, sink: *mut GstElement, bus: *mut GstBus, what: &str) -> Result<Option<*mut GstSample>, VideoFileError> {
        loop {
            let sample = (self.gst.gst_app_sink_try_pull_sample)(sink, BUS_LOOK_NS);
            if !sample.is_null() {
                return Ok(Some(sample));
            }
            if let Some(text) = self.gst.bus_error(bus) {
                return Err(VideoFileError::new(format!("{what}: {text}")));
            }
            if (self.gst.gst_app_sink_is_eos)(sink) != 0 {
                return Ok(None);
            }
        }
    }

    unsafe fn pull_preroll(
        &self,
        appsink: *mut GstElement,
        bus: *mut GstBus,
        dec: *mut GstElement,
        ignored_owner: *mut GstElement,
        link_pad: *mut c_void,
        stream: &str,
    ) -> Result<PrerollOutcome, VideoFileError> {
        let mut watch = PrerollWatch::new(self.gst, ignored_owner);
        loop {
            let sample = (self.gst.gst_app_sink_try_pull_preroll)(appsink, 0);
            if !sample.is_null() {
                return Ok(PrerollOutcome::Sample(sample));
            }
            let poll = watch.wait_bus(bus, BUS_LOOK_NS);
            if let BusPoll::Error(text) = poll {
                return Err(VideoFileError::new(format!("no {stream} stream in file (decoder preroll: {text})")));
            }
            let sample = (self.gst.gst_app_sink_try_pull_preroll)(appsink, 0);
            if !sample.is_null() {
                return Ok(PrerollOutcome::Sample(sample));
            }
            let activity = poll.activity(stream)?;
            if (self.gst.gst_app_sink_is_eos)(appsink) != 0 {
                return Err(VideoFileError::new(format!("no {stream} stream in file")));
            }
            if let StallState::Stalled(reason) = watch.observe(activity, dec, link_pad) {
                return Ok(PrerollOutcome::Stalled(reason, watch.describe(dec)));
            }
        }
    }

    /// The picture track's compressed codec, read from the prerolled pipeline:
    /// the caps on the picture decoder's input are the stream as the file holds
    /// it (`video/x-h264, stream-format=avc, ...`). None for a codec we do not
    /// name and for a file of raw pictures (no decoder).
    unsafe fn compressed_video_codec(&self) -> Option<VideoFileCodec> {
        compressed_codec(&self.find_element(|element| self.picture_decoder_input(element))?)
    }

    /// The factory name of the picture decoder the picture pipeline plugged (`nvh264dec`,
    /// `avdec_h264`), None before it plugged one.
    unsafe fn plugged_picture_decoder(&self) -> Option<String> {
        let gst = self.gst;
        self.find_element(|element| {
            let factory = (gst.gst_element_get_factory)(element);
            if factory.is_null() || !self.is_picture_decoder(factory) {
                return None;
            }
            let name = (gst.gst_object_get_name)(factory);
            if name.is_null() {
                return None;
            }
            let text = std::ffi::CStr::from_ptr(name).to_string_lossy().into_owned();
            (gst.g_free)(name as *mut c_void);
            Some(text)
        })
    }

    /// The first answer `look` gives for an element of the picture pipeline, its bins walked.
    unsafe fn find_element<T>(&self, mut look: impl FnMut(*mut GstElement) -> Option<T>) -> Option<T> {
        let gst = self.gst;
        if self.pipeline.is_null() {
            return None;
        }
        let elements = (gst.gst_bin_iterate_recurse)(self.pipeline);
        if elements.is_null() {
            return None;
        }
        let mut found = None;
        let mut value = GValue::default();
        loop {
            match (gst.gst_iterator_next)(elements, &mut value) {
                GST_ITERATOR_OK => {
                    found = look((gst.g_value_get_object)(&value));
                    (gst.g_value_unset)(&mut value);
                    if found.is_some() {
                        break;
                    }
                }
                // The bin changed while it was walked: walk it again.
                GST_ITERATOR_RESYNC => (gst.gst_iterator_resync)(elements),
                _ => break,
            }
        }
        (gst.gst_iterator_free)(elements);
        found
    }

    /// Whether element `factory`'s class names a picture decoder (`Codec/Decoder/Video`).
    unsafe fn is_picture_decoder(&self, factory: *mut c_void) -> bool {
        let klass = (self.gst.gst_element_factory_get_metadata)(factory, c"klass".as_ptr());
        if klass.is_null() {
            return false;
        }
        let klass = std::ffi::CStr::from_ptr(klass).to_string_lossy();
        let class: Vec<&str> = klass.split('/').collect();
        class.contains(&"Decoder") && class.contains(&"Video")
    }

    /// The caps on `element`'s input when it is a picture decoder (its class
    /// names a video decoder), else None.
    unsafe fn picture_decoder_input(&self, element: *mut GstElement) -> Option<String> {
        let gst = self.gst;
        let factory = (gst.gst_element_get_factory)(element);
        if factory.is_null() || !self.is_picture_decoder(factory) {
            return None;
        }
        let name = CString::new("sink").unwrap();
        let pad = (gst.gst_element_get_static_pad)(element, name.as_ptr());
        if pad.is_null() {
            return None;
        }
        let caps = (gst.gst_pad_get_current_caps)(pad);
        (gst.gst_object_unref)(pad);
        if caps.is_null() {
            return None;
        }
        let text = self.caps_text(caps);
        (gst.gst_mini_object_unref)(caps);
        Some(text)
    }

    unsafe fn sample_caps(&self, sample: *mut GstSample) -> String {
        let caps = (self.gst.gst_sample_get_caps)(sample);
        if caps.is_null() {
            return String::new();
        }
        self.caps_text(caps)
    }

    unsafe fn caps_text(&self, caps: *mut GstCaps) -> String {
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
            let Some(sample) = self.pull(self.appsink, self.bus, "decoder")? else {
                return Ok(None);
            };
            let result = sample_nv12(self.gst, sample, self.info.width, self.info.height).map(|nv12| {
                Some(DecodedVideoFrame {
                    pts_100ns: self.sample_pts_100ns(sample, (self.gst.gst_sample_get_buffer)(sample)),
                    width: self.info.width,
                    height: self.info.height,
                    nv12,
                    color: self.frame_color,
                })
            });
            (self.gst.gst_mini_object_unref)(sample as *mut c_void);
            result
        }
    }

    pub fn next_audio(&mut self) -> Result<Option<DecodedAudioChunk>, VideoFileError> {
        if self.audio_pipeline.is_null() {
            return Ok(None);
        }
        unsafe {
            let sample = if !self.audio_first.is_null() {
                std::mem::replace(&mut self.audio_first, std::ptr::null_mut())
            } else {
                match self.pull(self.audio_sink, self.audio_bus, "audio decoder")? {
                    Some(sample) => sample,
                    None => return Ok(None),
                }
            };
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
            if !self.audio_first.is_null() {
                (self.gst.gst_mini_object_unref)(self.audio_first as *mut c_void);
                self.audio_first = std::ptr::null_mut();
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
            // The sink first: the pipeline's state changes pass its locked state by. Going down it
            // wakes a render that waits for room (PLAYING to PAUSED unlocks it) and flushes it
            // (PAUSED to READY), so the stream it blocked stops with the pipeline.
            if !self.audio_sink.is_null() {
                (self.gst.gst_element_set_state)(self.audio_sink, GST_STATE_NULL);
            }
            if !self.audio_pipeline.is_null() {
                (self.gst.gst_element_set_state)(self.audio_pipeline, GST_STATE_NULL);
            }
            if !self.audio_first.is_null() {
                (self.gst.gst_mini_object_unref)(self.audio_first as *mut c_void);
                self.audio_first = std::ptr::null_mut();
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

    fn close_video_pipeline(&mut self) {
        unsafe {
            if !self.pipeline.is_null() {
                (self.gst.gst_element_set_state)(self.pipeline, GST_STATE_NULL);
            }
            for object in [self.appsink, self.bus, self.pipeline] {
                if !object.is_null() {
                    (self.gst.gst_object_unref)(object as *mut c_void);
                }
            }
            self.appsink = ptr::null_mut();
            self.bus = ptr::null_mut();
            self.pipeline = ptr::null_mut();
        }
    }
}

impl Drop for LinuxVideoFileDecoder {
    fn drop(&mut self) {
        self.close_audio_pipeline();
        self.close_video_pipeline();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{VideoMatrix, VideoPrimaries, VideoRange, VideoTransfer};

    const PAUSED_TASK: PrerollProbe = PrerollProbe {
        saw_bus_activity: false,
        pad_linked: false,
        source_settled_paused: false,
        saw_upstream_task: true,
        active_upstream_tasks: 0,
    };

    #[test]
    fn preroll_stall_detector_never_triggers_while_tasks_or_bus_are_active() {
        let mut detector = PrerollStallDetector::default();
        let active = PrerollProbe { active_upstream_tasks: 1, ..PAUSED_TASK };
        let linked = PrerollProbe { pad_linked: true, ..PAUSED_TASK };
        let activity = PrerollProbe { saw_bus_activity: true, ..PAUSED_TASK };
        let startup = PrerollProbe { saw_upstream_task: false, ..PAUSED_TASK };
        for probe in [active, linked, startup] {
            for _ in 0..1000 {
                assert_eq!(detector.observe(probe), StallState::Progressing, "{probe:?}");
            }
        }
        for _ in 0..1000 {
            assert_eq!(detector.observe(activity), StallState::Progressing, "bus activity");
            assert_eq!(detector.observe(PAUSED_TASK), StallState::Progressing, "one quiet poll");
        }
        assert_eq!(detector.observe(active), StallState::Progressing, "the next task started");
        assert_eq!(detector.observe(PAUSED_TASK), StallState::Progressing, "the handoff reset the count");
    }

    #[test]
    fn preroll_stall_detector_flags_paused_upstream_and_settled_empty_source() {
        let mut detector = PrerollStallDetector::default();
        assert_eq!(detector.observe(PrerollProbe { saw_bus_activity: true, ..PAUSED_TASK }), StallState::Progressing);
        assert_eq!(detector.observe(PAUSED_TASK), StallState::Progressing);
        assert_eq!(detector.observe(PAUSED_TASK), StallState::Stalled(StallReason::UpstreamPaused));

        let mut detector = PrerollStallDetector::default();
        let settled = PrerollProbe { source_settled_paused: true, saw_upstream_task: false, ..PAUSED_TASK };
        assert_eq!(detector.observe(settled), StallState::Progressing);
        assert_eq!(detector.observe(settled), StallState::Stalled(StallReason::NoStreamSettled));
    }

    #[test]
    fn iso_bmff_inspector_distinguishes_header_only_stubs_from_playable_and_other_containers() {
        use std::io::Cursor;

        const MP4_FTYP: &[u8] = b"\0\0\0\x18ftypisom\0\0\x02\0isomiso2";
        const MOV_FTYP: &[u8] = b"\0\0\0\x14ftypqt  \0\0\0\0qt  ";
        const MOOV: &[u8] = b"\0\0\0\x10moov\0\0\0\x08free";
        let inspect = |bytes: &[u8]| unplayable_iso_bmff_reader(&mut Cursor::new(bytes), bytes.len() as u64);

        for (name, ftyp, tail) in [
            ("header-only MP4", MP4_FTYP, &[][..]),
            ("padded MP4", MP4_FTYP, &[0; 8]),
            ("padded MOV", MOV_FTYP, &[0; 8]),
            ("truncated moov", MP4_FTYP, b"\0\0\x03\xe8moov\0\0\0\0"),
            ("invalid free size", MP4_FTYP, b"\0\0\0\x04free"),
            ("empty moov", MP4_FTYP, b"\0\0\0\x08moov"),
        ] {
            assert!(inspect(&[ftyp, tail].concat()).is_some(), "{name} should be refused");
        }

        let extended_mdat = b"\0\0\0\x01mdat\0\0\0\0\0\0\0\x18abcdefgh";
        for (name, bytes) in [
            ("moov before mdat", [MP4_FTYP, MOOV, b"\0\0\0\x08mdat"].concat()),
            ("moov after extended mdat", [MP4_FTYP, extended_mdat, MOOV].concat()),
            ("moov to EOF", [MP4_FTYP, b"\0\0\0\0moov\0\0\0\x08free"].concat()),
            ("classic MOV", MOOV.to_vec()),
            ("Matroska", b"\x1a\x45\xdf\xa3\x9f\x42\x86\x81\x01\x42\xf7\x81\x01\x42\xf2\x81".to_vec()),
            ("plain text", b"a plain text file".to_vec()),
            ("empty input", Vec::new()),
        ] {
            assert_eq!(inspect(&bytes), None, "{name} should remain eligible for reopen");
        }

        assert_eq!(display_file_name("/tmp/dir/valid.mp4"), "valid.mp4");
        assert_eq!(display_file_name("header.mp4"), "header.mp4");
        assert_eq!(display_file_name(""), "<unnamed>");
    }

    #[test]
    fn stall_policy_fails_unplayable_immediately_and_reopens_potentially_playable() {
        let settled = StallReason::NoStreamSettled.decide("video", 0, "settled", None);
        assert!(matches!(settled, StallAction::Fail(message) if message.starts_with("no video stream in file")));

        let paused = StallReason::UpstreamPaused;
        let reason = "ISO-BMFF container has no moov box";
        let stub = paused.decide("video", 0, "paused", Some(reason));
        assert!(matches!(stub, StallAction::Fail(message)
            if message.starts_with("no video stream in file") && message.contains(reason)));

        for attempt in 0..MAX_STALL_REOPENS {
            let next_attempt = attempt + 1;
            assert_eq!(paused.decide("video", attempt, "paused", None), StallAction::Reopen {
                next_attempt,
                backoff_ms: REOPEN_BACKOFF_MS * next_attempt as u64,
            });
        }
        let exhausted = paused.decide("video", MAX_STALL_REOPENS, "paused", None);
        assert!(matches!(exhausted, StallAction::Fail(message)
            if message.starts_with("no video stream in file") && message.contains("after 5 reopens")));
    }

    #[test]
    fn preroll_watch_tracks_real_gstreamer_task_lifecycle_without_sleeping() {
        struct Pipeline(GstRef);
        impl Drop for Pipeline {
            fn drop(&mut self) {
                unsafe { (self.0.gst.gst_element_set_state)(self.0.ptr, GST_STATE_NULL) };
            }
        }

        let gst = LibGst::get().expect("GStreamer runtime");
        unsafe {
            let launch = "appsrc name=src ! fakesink  queue name=q ! videoconvert ! \
                          video/x-raw,format=NV12 ! appsink name=out sync=false max-buffers=1";
            let pipeline = Pipeline(GstRef { gst, ptr: LinuxVideoFileDecoder::parse_pipeline(gst, launch).unwrap() });
            let bin = pipeline.0.ptr;
            let src = GstRef::element(gst, bin, c"src").unwrap();
            let queue = GstRef::element(gst, bin, c"q").unwrap();
            let link_pad = queue.sink_pad().unwrap();
            let bus = (gst.gst_element_get_bus)(bin);
            assert!(!bus.is_null(), "pipeline has a bus");
            let bus = GstRef { gst, ptr: bus };
            let mut watch = PrerollWatch::new(gst, queue.ptr);
            assert_eq!((gst.gst_element_set_state)(bin, GST_STATE_PAUSED), GST_STATE_CHANGE_ASYNC);
            let activity = watch.wait_bus(bus.ptr, 0).activity("video").unwrap();
            let probe = watch.probe(activity, bin, link_pad.ptr);
            assert!(probe.saw_upstream_task && probe.active_upstream_tasks >= 1, "appsrc started: {probe:?}");
            assert!(!probe.pad_linked, "the queue has no upstream link");
            assert_eq!(watch.observe(activity, bin, link_pad.ptr), StallState::Progressing);

            assert_eq!((gst.gst_element_set_state)(src.ptr, GST_STATE_NULL), GST_STATE_CHANGE_SUCCESS);
            watch.wait_bus(bus.ptr, 0).activity("video").unwrap();
            let probe = watch.probe(false, bin, link_pad.ptr);
            assert_eq!(probe.active_upstream_tasks, 0, "only the downstream queue is running");
            assert_eq!(watch.observe(false, bin, link_pad.ptr), StallState::Progressing);
            assert_eq!(watch.observe(false, bin, link_pad.ptr), StallState::Stalled(StallReason::UpstreamPaused));
        }
    }

    #[test]
    fn planes_are_read_where_the_layout_puts_them() {
        // 6 x 3 laid out as GStreamer's default: rows padded to a stride of 8, the
        // chroma plane after 4 luma rows (the height rounded up to even).
        let (w, h) = (6u32, 3u32);
        let padded = Nv12Layout { luma: (0, 8), chroma: (32, 8) };
        let mut src = vec![0xEEu8; 32 + 8 * 2];
        for r in 0..3 {
            for c in 0..6 {
                src[r * 8 + c] = (r * 10 + c) as u8;
            }
        }
        for r in 0..2 {
            for c in 0..6 {
                src[32 + r * 8 + c] = (100 + r * 10 + c) as u8;
            }
        }
        let out = tight_nv12(&src, padded, w, h).expect("padded rows");
        assert_eq!(out.len(), 6 * 3 + 6 * 2, "tight size, the last chroma row kept for an odd height");
        assert_eq!(&out[..6], &[0, 1, 2, 3, 4, 5], "first luma row");
        assert_eq!(&out[12..18], &[20, 21, 22, 23, 24, 25], "last luma row");
        assert_eq!(&out[18..24], &[100, 101, 102, 103, 104, 105], "first chroma row");
        assert!(!out.contains(&0xEE), "no padding byte copied");
        // A decoder pool's height padding: the chroma plane starts rows later than the picture ends.
        let tall = Nv12Layout { luma: (0, 4), chroma: (4 * 6, 4) };
        let src: Vec<u8> = (0..4 * 6 + 4 * 2).map(|i| i as u8).collect();
        let out = tight_nv12(&src, tall, 4, 4).expect("height-padded planes");
        assert_eq!(&out[16..], &src[24..], "chroma read from its own offset");
        assert_eq!(tight_nv12(&src, Nv12Layout { luma: (0, 4), chroma: (16, 4) }, 4, 4).map(|v| v.len()), Some(24), "tight layout");
        assert_eq!(tight_nv12(&src, Nv12Layout { luma: (0, 4), chroma: (30, 4) }, 4, 4), None, "a plane past the buffer");
        assert_eq!(tight_nv12(&src, Nv12Layout { luma: (0, 3), chroma: (16, 4) }, 4, 4), None, "a stride narrower than a row");
    }

    #[test]
    fn compressed_caps_name_their_codec() {
        let h264 = "video/x-h264, stream-format=(string)avc, alignment=(string)au, level=(string)3.1, profile=(string)high";
        assert_eq!(compressed_codec(h264), Some(VideoFileCodec::H264));
        assert_eq!(compressed_codec("video/x-h265, stream-format=(string)hvc1, alignment=(string)au"), Some(VideoFileCodec::H265));
        assert_eq!(compressed_codec("video/x-h265"), Some(VideoFileCodec::H265));
        assert_eq!(compressed_codec("video/x-vp9, width=(int)640"), None);
        assert_eq!(compressed_codec(""), None);
    }

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
