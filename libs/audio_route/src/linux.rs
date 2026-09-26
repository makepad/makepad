//! Linux: PulseAudio's client API, served by PulseAudio itself or by
//! PipeWire's `pipewire-pulse`.
//!
//! A route always records the tapped application's stream by itself (the
//! stream's own monitor, `pa_stream_set_monitor_stream`: what it plays,
//! wherever it plays to) and hands every block to the processor. It has
//! two modes ([`Route::set_processing`]):
//!
//! - Monitoring: the application plays to its own device untouched; the
//!   recording is only a copy (meters, pictures). Nothing else exists.
//! - Processing: the route loads a null sink of its own
//!   (`makepad_route_<pid>_<n>`), moves the application's streams onto it
//!   and plays what the processor leaves to the output device. Moving is
//!   what silences the direct path: a muted stream records as silence, so
//!   muting it would silence the tap too. The stream's own recording goes
//!   on through the move, so switching modes never interrupts it; the
//!   route's output fades in as the move lands and out as the streams go
//!   back, so the switch has no gap and no doubled stretch.
//!
//! The route follows the application: a stream it opens later (a relaunch,
//! a new output) is recorded when the old one ends, and moved while
//! processing. A route whose process died leaves its sink behind (modules
//! outlive their client); the next route opened on the machine unloads it,
//! and the server sends what played there back to the default device.
//!
//! [`Mute::HearOriginal`] adds a loopback from the route's sink to the device
//! the application played to, so while processing the application keeps
//! being heard as well.

use crate::{AudioProcess, Error, FrameInfo, Mute, OutputDevice, Processor, Result, RouteConfig, Source};
use std::cell::UnsafeCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int, c_void};
use std::ptr;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::Mutex;

/// Applications known under their macOS bundle id, as Linux names their
/// audio streams (`application.process.binary`, `application.name`). A host
/// names its player once, by bundle id, on every platform.
const KNOWN_APPS: &[(&str, &[&str])] = &[("com.spotify.client", &["spotify"])];

/// A stream whose volume is not known.
const NO_VOLUME: pa_cvolume = pa_cvolume { channels: 0, values: [0; 32] };

/// How long a request to the sound server may take before it counts as
/// failed. A server that accepts the connection and never answers must not
/// hang the caller.
const TIMEOUT_MS: u64 = 3000;

/// Recorded block: 10 ms. Playback keeps about six of them queued: the
/// recording arrives a graph cycle (21 ms at 1024 frames) at a time, and a
/// shorter queue overflows and then runs dry (measured).
const BLOCK_MS: u32 = 10;
const QUEUE_BLOCKS: u32 = 6;
/// The processor's block at most, in frames; longer reads are split.
const MAX_BLOCK_FRAMES: usize = 4096;
const CHANNELS: u8 = 2;

// ---- libpulse ----

#[allow(non_camel_case_types)]
mod ffi {
    use std::os::raw::{c_char, c_int, c_void};

    pub type pa_usec_t = u64;
    pub type pa_volume_t = u32;

    #[repr(C)]
    pub struct pa_threaded_mainloop {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct pa_context {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct pa_operation {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct pa_stream {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct pa_proplist {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct pa_time_event {
        _p: [u8; 0],
    }
    #[repr(C)]
    pub struct pa_format_info {
        _p: [u8; 0],
    }

    /// Only `time_free` is called; the rest is layout.
    #[repr(C)]
    pub struct pa_mainloop_api {
        pub userdata: *mut c_void,
        pub io_new: *const c_void,
        pub io_enable: *const c_void,
        pub io_free: *const c_void,
        pub io_set_destroy: *const c_void,
        pub time_new: *const c_void,
        pub time_restart: *const c_void,
        pub time_free: Option<unsafe extern "C" fn(e: *mut pa_time_event)>,
        pub time_set_destroy: *const c_void,
        pub defer_new: *const c_void,
        pub defer_enable: *const c_void,
        pub defer_free: *const c_void,
        pub defer_set_destroy: *const c_void,
        pub quit: *const c_void,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct pa_sample_spec {
        pub format: c_int,
        pub rate: u32,
        pub channels: u8,
    }

    #[repr(C)]
    #[derive(Clone, Copy)]
    pub struct pa_channel_map {
        pub channels: u8,
        pub map: [c_int; 32],
    }

    #[repr(C)]
    #[derive(Clone, Copy, Debug, PartialEq)]
    pub struct pa_cvolume {
        pub channels: u8,
        pub values: [pa_volume_t; 32],
    }

    #[repr(C)]
    pub struct pa_buffer_attr {
        pub maxlength: u32,
        pub tlength: u32,
        pub prebuf: u32,
        pub minreq: u32,
        pub fragsize: u32,
    }

    #[repr(C)]
    pub struct pa_server_info {
        pub user_name: *const c_char,
        pub host_name: *const c_char,
        pub server_version: *const c_char,
        pub server_name: *const c_char,
        pub sample_spec: pa_sample_spec,
        pub default_sink_name: *const c_char,
        pub default_source_name: *const c_char,
        pub cookie: u32,
        pub channel_map: pa_channel_map,
    }

    /// The fields before `proplist`; the rest is never read.
    #[repr(C)]
    pub struct pa_sink_info {
        pub name: *const c_char,
        pub index: u32,
        pub description: *const c_char,
        pub sample_spec: pa_sample_spec,
        pub channel_map: pa_channel_map,
        pub owner_module: u32,
        pub volume: pa_cvolume,
        pub mute: c_int,
        pub monitor_source: u32,
        pub monitor_source_name: *const c_char,
        pub latency: pa_usec_t,
        pub driver: *const c_char,
        pub flags: u32,
        pub proplist: *mut pa_proplist,
    }

    #[repr(C)]
    pub struct pa_sink_input_info {
        pub index: u32,
        pub name: *const c_char,
        pub owner_module: u32,
        pub client: u32,
        pub sink: u32,
        pub sample_spec: pa_sample_spec,
        pub channel_map: pa_channel_map,
        pub volume: pa_cvolume,
        pub buffer_usec: pa_usec_t,
        pub sink_usec: pa_usec_t,
        pub resample_method: *const c_char,
        pub driver: *const c_char,
        pub mute: c_int,
        pub proplist: *mut pa_proplist,
        pub corked: c_int,
        pub has_volume: c_int,
        pub volume_writable: c_int,
        pub format: *mut pa_format_info,
    }

    #[repr(C)]
    pub struct pa_source_output_info {
        pub index: u32,
        pub name: *const c_char,
        pub owner_module: u32,
        pub client: u32,
        pub source: u32,
        pub sample_spec: pa_sample_spec,
        pub channel_map: pa_channel_map,
        pub buffer_usec: pa_usec_t,
        pub source_usec: pa_usec_t,
        pub resample_method: *const c_char,
        pub driver: *const c_char,
        pub proplist: *mut pa_proplist,
        pub corked: c_int,
    }

    pub type pa_context_notify_cb_t = Option<unsafe extern "C" fn(c: *mut pa_context, userdata: *mut c_void)>;
    pub type pa_context_success_cb_t = Option<unsafe extern "C" fn(c: *mut pa_context, success: c_int, userdata: *mut c_void)>;
    pub type pa_context_subscribe_cb_t = Option<unsafe extern "C" fn(c: *mut pa_context, t: u32, idx: u32, userdata: *mut c_void)>;
    pub type pa_server_info_cb_t = Option<unsafe extern "C" fn(c: *mut pa_context, i: *const pa_server_info, userdata: *mut c_void)>;
    pub type pa_sink_info_cb_t = Option<unsafe extern "C" fn(c: *mut pa_context, i: *const pa_sink_info, eol: c_int, userdata: *mut c_void)>;
    pub type pa_sink_input_info_cb_t = Option<unsafe extern "C" fn(c: *mut pa_context, i: *const pa_sink_input_info, eol: c_int, userdata: *mut c_void)>;
    pub type pa_source_output_info_cb_t =
        Option<unsafe extern "C" fn(c: *mut pa_context, i: *const pa_source_output_info, eol: c_int, userdata: *mut c_void)>;
    pub type pa_stream_notify_cb_t = Option<unsafe extern "C" fn(s: *mut pa_stream, userdata: *mut c_void)>;
    pub type pa_stream_request_cb_t = Option<unsafe extern "C" fn(s: *mut pa_stream, nbytes: usize, userdata: *mut c_void)>;
    pub type pa_time_event_cb_t =
        Option<unsafe extern "C" fn(a: *mut pa_mainloop_api, e: *mut pa_time_event, tv: *const c_void, userdata: *mut c_void)>;

    pub const PA_CONTEXT_READY: c_int = 4;
    pub const PA_CONTEXT_FAILED: c_int = 5;
    pub const PA_CONTEXT_TERMINATED: c_int = 6;
    pub const PA_CONTEXT_NOAUTOSPAWN: u32 = 1;

    pub const PA_STREAM_READY: c_int = 2;
    pub const PA_STREAM_FAILED: c_int = 3;
    pub const PA_STREAM_TERMINATED: c_int = 4;
    pub const PA_STREAM_INTERPOLATE_TIMING: u32 = 0x0002;
    pub const PA_STREAM_AUTO_TIMING_UPDATE: u32 = 0x0008;
    pub const PA_STREAM_DONT_MOVE: u32 = 0x0200;
    pub const PA_STREAM_ADJUST_LATENCY: u32 = 0x2000;
    pub const PA_STREAM_START_UNMUTED: u32 = 0x10000;

    pub const PA_OPERATION_RUNNING: c_int = 0;
    pub const PA_SAMPLE_FLOAT32LE: c_int = 5;
    pub const PA_CHANNEL_POSITION_FRONT_LEFT: c_int = 1;
    pub const PA_CHANNEL_POSITION_FRONT_RIGHT: c_int = 2;
    pub const PA_SEEK_RELATIVE: c_int = 0;

    pub const PA_SUBSCRIPTION_MASK_SINK_INPUT: u32 = 0x0004;
    pub const PA_SUBSCRIPTION_EVENT_FACILITY_MASK: u32 = 0x000f;
    pub const PA_SUBSCRIPTION_EVENT_SINK_INPUT: u32 = 0x0002;
    pub const PA_SUBSCRIPTION_EVENT_TYPE_MASK: u32 = 0x0030;
    pub const PA_SUBSCRIPTION_EVENT_REMOVE: u32 = 0x0020;

    #[link(name = "pulse")]
    extern "C" {
        pub fn pa_threaded_mainloop_new() -> *mut pa_threaded_mainloop;
        pub fn pa_threaded_mainloop_free(m: *mut pa_threaded_mainloop);
        pub fn pa_threaded_mainloop_start(m: *mut pa_threaded_mainloop) -> c_int;
        pub fn pa_threaded_mainloop_stop(m: *mut pa_threaded_mainloop);
        pub fn pa_threaded_mainloop_lock(m: *mut pa_threaded_mainloop);
        pub fn pa_threaded_mainloop_unlock(m: *mut pa_threaded_mainloop);
        pub fn pa_threaded_mainloop_wait(m: *mut pa_threaded_mainloop);
        pub fn pa_threaded_mainloop_signal(m: *mut pa_threaded_mainloop, wait_for_accept: c_int);
        pub fn pa_threaded_mainloop_get_api(m: *mut pa_threaded_mainloop) -> *mut pa_mainloop_api;

        pub fn pa_context_new(api: *mut pa_mainloop_api, name: *const c_char) -> *mut pa_context;
        pub fn pa_context_set_state_callback(c: *mut pa_context, cb: pa_context_notify_cb_t, userdata: *mut c_void);
        pub fn pa_context_connect(c: *mut pa_context, server: *const c_char, flags: u32, api: *const c_void) -> c_int;
        pub fn pa_context_get_state(c: *const pa_context) -> c_int;
        pub fn pa_context_errno(c: *const pa_context) -> c_int;
        pub fn pa_context_disconnect(c: *mut pa_context);
        pub fn pa_context_unref(c: *mut pa_context);
        pub fn pa_context_rttime_new(c: *const pa_context, usec: pa_usec_t, cb: pa_time_event_cb_t, userdata: *mut c_void) -> *mut pa_time_event;
        pub fn pa_rtclock_now() -> pa_usec_t;
        pub fn pa_context_rttime_restart(c: *const pa_context, e: *mut pa_time_event, usec: pa_usec_t);

        pub fn pa_context_get_server_info(c: *mut pa_context, cb: pa_server_info_cb_t, userdata: *mut c_void) -> *mut pa_operation;
        pub fn pa_context_get_sink_info_list(c: *mut pa_context, cb: pa_sink_info_cb_t, userdata: *mut c_void) -> *mut pa_operation;
        pub fn pa_context_get_sink_input_info_list(c: *mut pa_context, cb: pa_sink_input_info_cb_t, userdata: *mut c_void) -> *mut pa_operation;
        pub fn pa_context_get_sink_input_info(c: *mut pa_context, idx: u32, cb: pa_sink_input_info_cb_t, userdata: *mut c_void) -> *mut pa_operation;
        pub fn pa_context_get_source_output_info_list(c: *mut pa_context, cb: pa_source_output_info_cb_t, userdata: *mut c_void) -> *mut pa_operation;
        pub fn pa_context_subscribe(c: *mut pa_context, mask: u32, cb: pa_context_success_cb_t, userdata: *mut c_void) -> *mut pa_operation;
        pub fn pa_context_set_subscribe_callback(c: *mut pa_context, cb: pa_context_subscribe_cb_t, userdata: *mut c_void);

        pub fn pa_operation_get_state(o: *const pa_operation) -> c_int;
        pub fn pa_operation_cancel(o: *mut pa_operation);
        pub fn pa_operation_unref(o: *mut pa_operation);

        pub fn pa_proplist_gets(p: *const pa_proplist, key: *const c_char) -> *const c_char;

        pub fn pa_stream_new(c: *mut pa_context, name: *const c_char, ss: *const pa_sample_spec, map: *const pa_channel_map) -> *mut pa_stream;
        pub fn pa_stream_set_state_callback(s: *mut pa_stream, cb: pa_stream_notify_cb_t, userdata: *mut c_void);
        pub fn pa_stream_set_read_callback(s: *mut pa_stream, cb: pa_stream_request_cb_t, userdata: *mut c_void);
        pub fn pa_stream_get_state(s: *const pa_stream) -> c_int;
        pub fn pa_stream_connect_record(s: *mut pa_stream, dev: *const c_char, attr: *const pa_buffer_attr, flags: u32) -> c_int;
        pub fn pa_stream_connect_playback(
            s: *mut pa_stream,
            dev: *const c_char,
            attr: *const pa_buffer_attr,
            flags: u32,
            volume: *const pa_cvolume,
            sync: *mut pa_stream,
        ) -> c_int;
        pub fn pa_stream_peek(s: *mut pa_stream, data: *mut *const c_void, nbytes: *mut usize) -> c_int;
        pub fn pa_stream_drop(s: *mut pa_stream) -> c_int;
        pub fn pa_stream_writable_size(s: *const pa_stream) -> usize;
        pub fn pa_stream_write(s: *mut pa_stream, data: *const c_void, nbytes: usize, free_cb: *const c_void, offset: i64, seek: c_int) -> c_int;
        pub fn pa_stream_disconnect(s: *mut pa_stream) -> c_int;
        pub fn pa_stream_unref(s: *mut pa_stream);
        pub fn pa_stream_get_index(s: *const pa_stream) -> u32;
        pub fn pa_context_set_sink_input_volume(c: *mut pa_context, idx: u32, volume: *const pa_cvolume, cb: pa_context_success_cb_t, userdata: *mut c_void) -> *mut pa_operation;
        pub fn pa_sw_volume_from_linear(v: f64) -> pa_volume_t;
        pub fn pa_sw_volume_to_linear(v: pa_volume_t) -> f64;
        pub fn pa_stream_get_latency(s: *mut pa_stream, usec: *mut pa_usec_t, negative: *mut c_int) -> c_int;
        pub fn pa_stream_set_monitor_stream(s: *mut pa_stream, sink_input_idx: u32) -> c_int;
    }
}

use ffi::*;

extern "C" {
    fn getpid() -> c_int;
}

fn own_pid() -> u32 {
    // SAFETY: getpid cannot fail.
    unsafe { getpid() as u32 }
}

fn text(p: *const c_char) -> String {
    if p.is_null() {
        String::new()
    } else {
        // SAFETY: libpulse hands out NUL-terminated strings valid for the callback.
        unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
    }
}

fn prop(list: *const pa_proplist, key: &CStr) -> String {
    if list.is_null() {
        return String::new();
    }
    // SAFETY: a proplist handed to a callback, read during it.
    text(unsafe { pa_proplist_gets(list, key.as_ptr()) })
}

fn stereo_map() -> pa_channel_map {
    let mut map = [0; 32];
    map[0] = PA_CHANNEL_POSITION_FRONT_LEFT;
    map[1] = PA_CHANNEL_POSITION_FRONT_RIGHT;
    pa_channel_map { channels: CHANNELS, map }
}

fn fail(step: &'static str, context: *const pa_context) -> Error {
    let status = if context.is_null() { -1 } else { unsafe { pa_context_errno(context) } };
    Error::System { status, step }
}

// ---- what the server lists, owned ----

#[derive(Clone, Debug)]
struct SinkInput {
    index: u32,
    pid: Option<u32>,
    binary: String,
    app_name: String,
    app_id: String,
    corked: bool,
    /// Per channel, as the server applies it (sink inputs only).
    volume: pa_cvolume,
}

#[derive(Clone, Debug)]
struct Sink {
    name: String,
    description: String,
    channels: u8,
    rate: u32,
}

impl SinkInput {
    /// # Safety
    /// `info` is the pointer a sink input callback was handed.
    unsafe fn read(info: &pa_sink_input_info) -> Self {
        let pid = prop(info.proplist, c"application.process.id").parse().ok();
        SinkInput {
            index: info.index,
            pid,
            binary: prop(info.proplist, c"application.process.binary"),
            app_name: prop(info.proplist, c"application.name"),
            app_id: first_nonempty([prop(info.proplist, c"application.id"), prop(info.proplist, c"pipewire.access.portal.app_id")]),
            corked: info.corked != 0,
            volume: info.volume,
        }
    }
}

fn first_nonempty<const N: usize>(items: [String; N]) -> String {
    items.into_iter().find(|item| !item.is_empty()).unwrap_or_default()
}

// ---- which streams a route takes ----

/// The parent of `pid`, from `/proc`.
fn parent_of(pid: u32) -> Option<u32> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    // The command name can hold spaces and parentheses; the fields after
    // its last ')' are fixed: state, then the parent pid.
    let rest = &stat[stat.rfind(')')? + 1..];
    rest.split_whitespace().nth(1)?.parse().ok()
}

/// `pid` is `root` or started (transitively) by it.
fn descends_from(pid: u32, root: u32) -> bool {
    let mut at = pid;
    for _ in 0..64 {
        if at == root {
            return true;
        }
        match parent_of(at) {
            Some(parent) if parent > 1 && parent != at => at = parent,
            _ => return false,
        }
    }
    false
}

/// Whether a stream belongs to the application `wanted` names: its bundle
/// id (via [`KNOWN_APPS`]), or the stream's binary, application name or
/// application id (a Flatpak's `com.spotify.Client`) spelled as given.
fn matches_bundle(wanted: &str, binary: &str, app_name: &str, app_id: &str) -> bool {
    let same = |have: &str, want: &str| !have.is_empty() && have.eq_ignore_ascii_case(want);
    let aliases = KNOWN_APPS.iter().find(|(bundle, _)| bundle.eq_ignore_ascii_case(wanted)).map_or(&[][..], |(_, names)| *names);
    std::iter::once(wanted).chain(aliases.iter().copied()).any(|name| same(binary, name) || same(app_name, name) || same(app_id, name))
}

struct Matcher {
    sources: Vec<Source>,
    own_pid: u32,
}

impl Matcher {
    fn wants(&self, input: &SinkInput) -> bool {
        if input.pid == Some(self.own_pid) {
            return false;
        }
        self.sources.iter().any(|source| match source {
            Source::Pid(pid) => input.pid.is_some_and(|have| descends_from(have, *pid)),
            Source::BundleId(bundle) => matches_bundle(bundle, &input.binary, &input.app_name, &input.app_id),
        })
    }
}

// ---- a connection ----

/// A threaded mainloop and a context connected on it. Every call below is
/// made with the mainloop lock held ([`Pulse::lock`]).
struct Pulse {
    mainloop: *mut pa_threaded_mainloop,
    context: *mut pa_context,
}

struct Locked<'a>(&'a Pulse);

impl Drop for Locked<'_> {
    fn drop(&mut self) {
        unsafe { pa_threaded_mainloop_unlock(self.0.mainloop) };
    }
}

unsafe extern "C" fn wake_context(_c: *mut pa_context, userdata: *mut c_void) {
    pa_threaded_mainloop_signal(userdata as *mut pa_threaded_mainloop, 0);
}

unsafe extern "C" fn wake_stream(_s: *mut pa_stream, userdata: *mut c_void) {
    pa_threaded_mainloop_signal(userdata as *mut pa_threaded_mainloop, 0);
}

struct Timer {
    expired: AtomicBool,
    mainloop: *mut pa_threaded_mainloop,
}

unsafe extern "C" fn timer_fired(_a: *mut pa_mainloop_api, _e: *mut pa_time_event, _tv: *const c_void, userdata: *mut c_void) {
    let timer = &*(userdata as *const Timer);
    timer.expired.store(true, Ordering::Release);
    pa_threaded_mainloop_signal(timer.mainloop, 0);
}

/// What a list or single-answer request collects, with the mainloop to wake.
struct Answer<T> {
    mainloop: *mut pa_threaded_mainloop,
    items: Vec<T>,
    done: bool,
    ok: bool,
}

impl<T> Answer<T> {
    fn new(mainloop: *mut pa_threaded_mainloop) -> Self {
        Answer { mainloop, items: Vec::new(), done: false, ok: true }
    }
}

unsafe fn answer_end<T>(userdata: *mut c_void, eol: c_int) {
    let answer = &mut *(userdata as *mut Answer<T>);
    answer.done = true;
    answer.ok = eol > 0;
    pa_threaded_mainloop_signal(answer.mainloop, 0);
}

unsafe extern "C" fn on_sink_input(_c: *mut pa_context, info: *const pa_sink_input_info, eol: c_int, userdata: *mut c_void) {
    if eol != 0 || info.is_null() {
        return answer_end::<SinkInput>(userdata, eol);
    }
    (*(userdata as *mut Answer<SinkInput>)).items.push(SinkInput::read(&*info));
}

unsafe extern "C" fn on_source_output(_c: *mut pa_context, info: *const pa_source_output_info, eol: c_int, userdata: *mut c_void) {
    if eol != 0 || info.is_null() {
        return answer_end::<SinkInput>(userdata, eol);
    }
    let info = &*info;
    (*(userdata as *mut Answer<SinkInput>)).items.push(SinkInput {
        index: info.index,
        pid: prop(info.proplist, c"application.process.id").parse().ok(),
        binary: prop(info.proplist, c"application.process.binary"),
        app_name: prop(info.proplist, c"application.name"),
        app_id: prop(info.proplist, c"application.id"),
        corked: info.corked != 0,
        volume: NO_VOLUME,
    });
}

unsafe extern "C" fn on_sink(_c: *mut pa_context, info: *const pa_sink_info, eol: c_int, userdata: *mut c_void) {
    if eol != 0 || info.is_null() {
        return answer_end::<Sink>(userdata, eol);
    }
    let info = &*info;
    (*(userdata as *mut Answer<Sink>)).items.push(Sink {
        name: text(info.name),
        description: text(info.description),
        channels: info.sample_spec.channels,
        rate: info.sample_spec.rate,
    });
}

unsafe extern "C" fn on_server(_c: *mut pa_context, info: *const pa_server_info, userdata: *mut c_void) {
    let answer = &mut *(userdata as *mut Answer<String>);
    if info.is_null() {
        answer.ok = false;
    } else {
        answer.items.push(text((*info).default_sink_name));
    }
    answer.done = true;
    pa_threaded_mainloop_signal(answer.mainloop, 0);
}

unsafe extern "C" fn on_success(_c: *mut pa_context, success: c_int, userdata: *mut c_void) {
    let answer = &mut *(userdata as *mut Answer<()>);
    answer.ok = success != 0;
    answer.done = true;
    pa_threaded_mainloop_signal(answer.mainloop, 0);
}

impl Pulse {
    fn connect(name: &str) -> Result<Self> {
        let name = CString::new(name).unwrap_or_default();
        unsafe {
            let mainloop = pa_threaded_mainloop_new();
            if mainloop.is_null() {
                return Err(Error::System { status: -1, step: "pa_threaded_mainloop_new" });
            }
            let context = pa_context_new(pa_threaded_mainloop_get_api(mainloop), name.as_ptr());
            if context.is_null() {
                pa_threaded_mainloop_free(mainloop);
                return Err(Error::System { status: -1, step: "pa_context_new" });
            }
            let pulse = Pulse { mainloop, context };
            pa_context_set_state_callback(context, Some(wake_context), mainloop as *mut c_void);
            if pa_context_connect(context, ptr::null(), PA_CONTEXT_NOAUTOSPAWN, ptr::null()) < 0 {
                return Err(fail("pa_context_connect", context));
            }
            if pa_threaded_mainloop_start(mainloop) < 0 {
                return Err(Error::System { status: -1, step: "pa_threaded_mainloop_start" });
            }
            let lock = pulse.lock();
            let ready = pulse.wait(|| matches!(pa_context_get_state(context), PA_CONTEXT_READY | PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED));
            if !ready || pa_context_get_state(context) != PA_CONTEXT_READY {
                drop(lock);
                return Err(fail("connecting to the sound server", context));
            }
            drop(lock);
            Ok(pulse)
        }
    }

    fn lock(&self) -> Locked<'_> {
        unsafe { pa_threaded_mainloop_lock(self.mainloop) };
        Locked(self)
    }

    /// Wait (lock held) until `done`, at most [`TIMEOUT_MS`]. `false` on timeout.
    fn wait(&self, done: impl Fn() -> bool) -> bool {
        if done() {
            return true;
        }
        unsafe {
            let timer = Box::new(Timer { expired: AtomicBool::new(false), mainloop: self.mainloop });
            let event = pa_context_rttime_new(self.context, pa_rtclock_now() + TIMEOUT_MS * 1000, Some(timer_fired), &*timer as *const Timer as *mut c_void);
            while !done() && !timer.expired.load(Ordering::Acquire) {
                pa_threaded_mainloop_wait(self.mainloop);
            }
            if !event.is_null() {
                if let Some(free) = (*pa_threaded_mainloop_get_api(self.mainloop)).time_free {
                    free(event);
                }
            }
        }
        done()
    }

    /// Run a request (lock held) and wait for its answer.
    fn request<T>(&self, step: &'static str, start: impl FnOnce(*mut c_void) -> *mut pa_operation) -> Result<Vec<T>> {
        // The callbacks write through this pointer on the mainloop thread;
        // this side reads through it too, never through a reference held
        // across the wait.
        let answer = Box::into_raw(Box::new(Answer::<T>::new(self.mainloop)));
        let op = start(answer as *mut c_void);
        if op.is_null() {
            drop(unsafe { Box::from_raw(answer) });
            return Err(fail(step, self.context));
        }
        let finished = self.wait(|| unsafe { ptr::read_volatile(&(*answer).done) } || unsafe { pa_operation_get_state(op) } != PA_OPERATION_RUNNING);
        unsafe {
            if !finished {
                // Cancelled: its callback never runs after this.
                pa_operation_cancel(op);
            }
            pa_operation_unref(op);
        }
        let answer = unsafe { Box::from_raw(answer) };
        if !finished || !answer.ok {
            return Err(fail(step, self.context));
        }
        Ok(answer.items)
    }

    fn sink_inputs(&self) -> Result<Vec<SinkInput>> {
        self.request("listing streams", |ud| unsafe { pa_context_get_sink_input_info_list(self.context, Some(on_sink_input), ud) })
    }

    fn source_outputs(&self) -> Result<Vec<SinkInput>> {
        self.request("listing recordings", |ud| unsafe { pa_context_get_source_output_info_list(self.context, Some(on_source_output), ud) })
    }

    fn sinks(&self) -> Result<Vec<Sink>> {
        self.request("listing output devices", |ud| unsafe { pa_context_get_sink_info_list(self.context, Some(on_sink), ud) })
    }

    fn default_sink_name(&self) -> Result<String> {
        Ok(self.request::<String>("asking the server", |ud| unsafe { pa_context_get_server_info(self.context, Some(on_server), ud) })?.pop().unwrap_or_default())
    }

    /// The default output, never a route's own sink: while a route's sink
    /// is the server's default (a machine with no other real output picks
    /// it) the first other device stands in.
    fn default_output(&self) -> Result<Sink> {
        let default = self.default_sink_name()?;
        let sinks = self.sinks()?;
        sinks
            .iter()
            .find(|sink| sink.name == default)
            .or(sinks.first())
            .cloned()
            .ok_or(Error::NotFound)
    }
}

impl Drop for Pulse {
    fn drop(&mut self) {
        unsafe {
            pa_threaded_mainloop_lock(self.mainloop);
            pa_context_set_state_callback(self.context, None, ptr::null_mut());
            pa_context_disconnect(self.context);
            pa_context_unref(self.context);
            pa_threaded_mainloop_unlock(self.mainloop);
            pa_threaded_mainloop_stop(self.mainloop);
            pa_threaded_mainloop_free(self.mainloop);
        }
    }
}

fn output_device(sink: &Sink) -> OutputDevice {
    OutputDevice {
        uid: sink.name.clone(),
        name: if sink.description.is_empty() { sink.name.clone() } else { sink.description.clone() },
        channels: sink.channels as u16,
        sample_rate: sink.rate as f64,
    }
}

// ---- the public calls ----

pub fn permission() -> Option<crate::Permission> {
    // Any client of the sound server may record any stream; nothing asks.
    None
}

pub fn processes() -> Result<Vec<AudioProcess>> {
    let pulse = Pulse::connect("Makepad audio route")?;
    let _lock = pulse.lock();
    let own = own_pid();
    let mut out: Vec<AudioProcess> = Vec::new();
    let mut add = |stream: SinkInput, output: bool| {
        let pid = stream.pid.unwrap_or(0);
        if pid == own {
            return;
        }
        let key = if stream.binary.is_empty() { stream.app_name.clone() } else { stream.binary.clone() };
        let entry = match out.iter_mut().position(|p| p.pid == pid && p.bundle_id == key) {
            Some(at) => &mut out[at],
            None => {
                out.push(AudioProcess {
                    pid,
                    bundle_id: key,
                    name: if stream.app_name.is_empty() { stream.binary.clone() } else { stream.app_name.clone() },
                    output_running: false,
                    input_running: false,
                });
                out.last_mut().unwrap()
            }
        };
        if output {
            entry.output_running |= !stream.corked;
        } else {
            entry.input_running |= !stream.corked;
        }
    };
    for stream in pulse.sink_inputs()? {
        add(stream, true);
    }
    for stream in pulse.source_outputs()? {
        add(stream, false);
    }
    Ok(out)
}

pub fn outputs() -> Result<Vec<OutputDevice>> {
    let pulse = Pulse::connect("Makepad audio route")?;
    let _lock = pulse.lock();
    Ok(pulse.sinks()?.iter().map(output_device).collect())
}

pub fn default_output() -> Result<OutputDevice> {
    let pulse = Pulse::connect("Makepad audio route")?;
    let _lock = pulse.lock();
    pulse.default_output().map(|sink| output_device(&sink))
}

// ---- a route ----

/// The tapped stream's volume while processing: 2^-13 (-78.3 dB) of the
/// person's level. The recording is taken after the stream's volume, so it
/// is brought back by the inverse (x 8192) before the processor sees it.
const DUCK: f64 = 1.0 / 8192.0;
/// Past this, a brought-back sample is a mistake, not sound: the block is
/// passed on as recorded.
const BLAST_GUARD: f32 = 4.0;
/// Each volume step of the crossfade lasts the route's output latency (our
/// share of it leads the stream's own path by one step), within these.
const STEP_MIN_MS: u64 = 15;
const STEP_MAX_MS: u64 = 80;
/// Where on the stream's timeline a volume change lands, after the server
/// confirms it: this far past what has been recorded then, on top of the
/// recording's latency. Measured on PipeWire 1.6 (a sweep of -20..60 ms).
const LANDS_AFTER_MS: f64 = 0.0;
/// How fast our output's share follows its target: a full swing in 5 ms,
/// so a step is a short ramp, not a click.
const SHARE_RAMP_MS: f64 = 5.0;
/// Below a tenth, each step takes the stream this much further down (-3 dB):
/// a step that lands off where it was expected costs at most this much.
const TAIL_RATIO: f64 = 0.708;
/// A volume change lands within about a graph cycle of where it is
/// expected: the bring-back glides (in dB) across this window around it.
const GLIDE_MS: f64 = 40.0;
/// How long a new output stream plays (silence) before the duck starts.
const OUTPUT_WARMUP_MS: u64 = 120;
/// The output queue drops what would grow it past this.
const MAX_QUEUE_MS: u64 = 1000;
/// What a route keeps in [`RouteConfig::state_dir`] while it has a stream
/// ducked: binary, application name and the person's volume per channel.
const RECORD_FILE: &str = "audio-route-ducked-linux.txt";
/// How long the output stays after the way back, for what it still queues.
const LINGER_MS: u64 = 90;

/// The stream gains of the crossfade, from the person's level down to
/// [`DUCK`]: two quick steps to a tenth, where the two paths trade places
/// (kept short: a latency apart, they comb while both are heard), then
/// [`TAIL_RATIO`] steps, where only ours is heard.
fn duck_steps() -> Vec<f64> {
    let mut steps: Vec<f64> = vec![1.0, 0.5, 0.1];
    let mut gain = 0.1;
    loop {
        gain *= TAIL_RATIO;
        if gain <= DUCK {
            break;
        }
        steps.push(gain);
    }
    steps.push(DUCK);
    steps
}

/// Our output's share while the stream's own path is at `gain`: equal
/// power (the two paths are a latency apart, so they add as unrelated
/// sound, not in phase).
fn share_of(gain: f64) -> f32 {
    (1.0 - gain * gain).max(0.0).sqrt() as f32
}

/// `volume` with every channel at `gain` times its linear level.
fn scaled(volume: &pa_cvolume, gain: f64) -> pa_cvolume {
    let mut out = *volume;
    for value in out.values.iter_mut().take(volume.channels as usize) {
        *value = unsafe { pa_sw_volume_from_linear(pa_sw_volume_to_linear(*value) * gain) };
    }
    out
}

/// Two volumes the same to within the server's own rounding: a level it
/// remembered for the application comes back a few units off (2% linear).
fn near(a: &pa_cvolume, b: &pa_cvolume) -> bool {
    a.channels == b.channels
        && (0..a.channels as usize).all(|i| unsafe {
            let (x, y) = (pa_sw_volume_to_linear(a.values[i]), pa_sw_volume_to_linear(b.values[i]));
            (x - y).abs() <= 0.02 * x.max(y)
        })
}

/// Left and right of a volume, linear (a mono volume is both).
fn linear_pair(volume: &pa_cvolume) -> [f64; 2] {
    let channel = |i: usize| unsafe { pa_sw_volume_to_linear(volume.values[i.min(volume.channels.max(1) as usize - 1)]) };
    [channel(0), channel(1)]
}

/// What brings the recording of a stream at `now` back to the person's
/// level: per channel, exactly the inverse of what the server applies.
fn bring_back(original: &pa_cvolume, now: &pa_cvolume) -> [f32; 2] {
    let (full, now) = (linear_pair(original), linear_pair(now));
    let bring = |i: usize| if now[i] > 0.0 && full[i] > 0.0 { (full[i] / now[i]) as f32 } else { 1.0 };
    [bring(0), bring(1)]
}

/// The tapped stream while the route has changed its volume.
struct Duck {
    input: u32,
    binary: String,
    app_name: String,
    /// The person's volume, given back when processing ends.
    original: pa_cvolume,
    /// What this route set last.
    set: pa_cvolume,
    step: usize,
    /// The step our output's share is at: one ahead of `step` while the
    /// crossfade runs.
    share_step: usize,
}

/// Everything the mainloop thread works with; only it touches the cells.
struct Shared {
    context: *mut pa_context,
    mainloop: *mut pa_threaded_mainloop,
    /// The recording of the tapped stream (its own monitor: after its
    /// volume, before any other stream is mixed in).
    record: UnsafeCell<*mut pa_stream>,
    /// The sink input recorded; `u32::MAX` while none is.
    captured: AtomicU32,
    /// Its volume, as last reported.
    captured_volume: UnsafeCell<pa_cvolume>,
    /// Its binary and application name, for the crash record.
    captured_names: UnsafeCell<(String, String)>,
    /// The route's output stream; null while monitoring.
    play: UnsafeCell<*mut pa_stream>,
    play_index: AtomicU32,
    /// Processing asked for ([`Route::set_processing`]).
    want_processing: AtomicBool,
    /// Duck the stream while processing ([`Mute`] other than
    /// `HearOriginal`).
    duck_enabled: bool,
    duck: UnsafeCell<Option<Duck>>,
    steps: Vec<f64>,
    /// The step timer, while the crossfade runs.
    timer: UnsafeCell<*mut pa_time_event>,
    /// The bring-back in force, and the ones waiting for the frame their
    /// volume lands at.
    bring: UnsafeCell<[f32; 2]>,
    /// The bring-back on its way to a new level: frames it starts and ends
    /// at, from, to.
    glide: UnsafeCell<Option<(u64, u64, [f32; 2], [f32; 2])>>,
    pending: UnsafeCell<std::collections::VecDeque<(u64, [f32; 2])>>,
    /// Our output's share of what is heard: now, and where it is heading.
    share: UnsafeCell<f32>,
    share_target: UnsafeCell<f32>,
    /// The bring-back of each volume set not answered yet, in order.
    issued: UnsafeCell<std::collections::VecDeque<[f32; 2]>>,
    /// One crossfade step (the output's latency, see [`STEP_MIN_MS`]).
    step_ms: AtomicU64,
    /// When the output stream started: the duck waits until it plays.
    play_since: UnsafeCell<Option<std::time::Instant>>,
    matcher: Matcher,
    processor: Mutex<Box<dyn Processor>>,
    scratch: UnsafeCell<Vec<f32>>,
    spec: pa_sample_spec,
    sample_rate: f64,
    output: CString,
    state_dir: Option<std::path::PathBuf>,
    frames_in: AtomicU64,
    frames_dropped: AtomicU64,
    guarded: AtomicU64,
    external_changes: AtomicU32,
    /// The ducked stream's current step, for [`Route::describe`].
    step_now: AtomicU32,
    /// The person's level of a ducked stream that ended before it was
    /// given back: the application's next stream gets it.
    orphan: UnsafeCell<Option<pa_cvolume>>,
    /// Ticks the output has lingered after the way back.
    linger: UnsafeCell<u64>,
}

unsafe impl Send for Shared {}
unsafe impl Sync for Shared {}

pub struct Route {
    pulse: Pulse,
    shared: Box<Shared>,
}

unsafe impl Send for Route {}
unsafe impl Sync for Route {}

/// Take a stream down: callbacks off, disconnected, released.
unsafe fn close_stream(stream: *mut pa_stream) {
    if stream.is_null() {
        return;
    }
    pa_stream_set_read_callback(stream, None, ptr::null_mut());
    pa_stream_set_state_callback(stream, None, ptr::null_mut());
    pa_stream_disconnect(stream);
    pa_stream_unref(stream);
}

/// Record the sink input `stream` by itself, replacing the recording
/// there was. Mainloop thread (or lock held).
unsafe fn capture(shared: &Shared, stream: &SinkInput) -> *mut pa_stream {
    close_stream(std::mem::replace(&mut *shared.record.get(), ptr::null_mut()));
    shared.captured.store(stream.index, Ordering::Release);
    *shared.captured_volume.get() = stream.volume;
    *shared.captured_names.get() = (stream.binary.clone(), stream.app_name.clone());
    let map = stereo_map();
    let record = pa_stream_new(shared.context, c"Makepad route tap".as_ptr(), &shared.spec, &map);
    if record.is_null() {
        shared.captured.store(u32::MAX, Ordering::Release);
        return record;
    }
    let block_bytes = shared.spec.rate * BLOCK_MS / 1000 * 4 * CHANNELS as u32;
    let attr = pa_buffer_attr { maxlength: u32::MAX, tlength: u32::MAX, prebuf: u32::MAX, minreq: u32::MAX, fragsize: block_bytes };
    pa_stream_set_monitor_stream(record, stream.index);
    pa_stream_set_read_callback(record, Some(on_read), shared as *const Shared as *mut c_void);
    pa_stream_set_state_callback(record, Some(wake_stream), shared.mainloop as *mut c_void);
    if pa_stream_connect_record(record, ptr::null(), &attr, PA_STREAM_ADJUST_LATENCY | PA_STREAM_AUTO_TIMING_UPDATE | PA_STREAM_INTERPOLATE_TIMING) < 0 {
        pa_stream_unref(record);
        shared.captured.store(u32::MAX, Ordering::Release);
        return ptr::null_mut();
    }
    *shared.record.get() = record;
    record
}

/// Open the route's output stream (processing). Lock held.
unsafe fn open_play(shared: &Shared) -> *mut pa_stream {
    let map = stereo_map();
    let play = pa_stream_new(shared.context, c"Makepad route".as_ptr(), &shared.spec, &map);
    if play.is_null() {
        return play;
    }
    let block_bytes = shared.spec.rate * BLOCK_MS / 1000 * 4 * CHANNELS as u32;
    let attr = pa_buffer_attr { maxlength: u32::MAX, tlength: block_bytes * QUEUE_BLOCKS, prebuf: block_bytes * QUEUE_BLOCKS / 2, minreq: block_bytes, fragsize: u32::MAX };
    pa_stream_set_state_callback(play, Some(wake_stream), shared.mainloop as *mut c_void);
    if pa_stream_connect_playback(play, shared.output.as_ptr(), &attr, PA_STREAM_ADJUST_LATENCY | PA_STREAM_DONT_MOVE | PA_STREAM_START_UNMUTED | PA_STREAM_AUTO_TIMING_UPDATE | PA_STREAM_INTERPOLATE_TIMING, ptr::null(), ptr::null_mut()) < 0 {
        pa_stream_unref(play);
        return ptr::null_mut();
    }
    *shared.play.get() = play;
    play
}

// ---- the crash record ----

fn record_path(shared: &Shared) -> Option<std::path::PathBuf> {
    shared.state_dir.as_ref().map(|dir| dir.join(RECORD_FILE))
}

/// Keep the person's volume on disk before the stream is ducked, so a
/// route that dies mid-duck can be undone ([`restore_after_crash`]).
fn write_record(shared: &Shared, duck: Option<&Duck>) {
    let Some(path) = record_path(shared) else { return };
    let Some(duck) = duck else {
        let _ = std::fs::remove_file(path);
        return;
    };
    let values: Vec<String> = duck.original.values[..duck.original.channels as usize].iter().map(u32::to_string).collect();
    let text = format!("{}\t{}\t{}\n", duck.binary.replace(['\t', '\n'], " "), duck.app_name.replace(['\t', '\n'], " "), values.join(","));
    let _ = std::fs::create_dir_all(path.parent().unwrap_or(std::path::Path::new(".")));
    let temp = path.with_extension("tmp");
    if std::fs::write(&temp, text).is_ok() {
        let _ = std::fs::rename(temp, path);
    }
}

/// (binary, application name, the person's volume) as kept on disk.
fn read_record(path: &std::path::Path) -> Option<(String, String, pa_cvolume)> {
    let text = std::fs::read_to_string(path).ok()?;
    let mut fields = text.trim_end_matches('\n').split('\t');
    let (binary, app_name, values) = (fields.next()?.to_string(), fields.next()?.to_string(), fields.next()?);
    let mut volume = NO_VOLUME;
    for value in values.split(',') {
        if volume.channels as usize >= 32 {
            break;
        }
        volume.values[volume.channels as usize] = value.parse().ok()?;
        volume.channels += 1;
    }
    (volume.channels > 0).then_some((binary, app_name, volume))
}

/// Give back the volume a route that did not close (the app crashed or
/// was killed while processing) left ducked, as recorded in `state_dir`. A
/// stream the person has since set to another level is left as they set
/// it. The record stays while the application has no stream to give it to.
pub fn restore_after_crash(state_dir: &std::path::Path) {
    let path = state_dir.join(RECORD_FILE);
    let Some((binary, app_name, original)) = read_record(&path) else {
        let _ = std::fs::remove_file(path);
        return;
    };
    let Ok(pulse) = Pulse::connect("Makepad audio route") else { return };
    let _lock = pulse.lock();
    let Ok(streams) = pulse.sink_inputs() else { return };
    let ours: Vec<&SinkInput> = streams.iter().filter(|s| s.binary == binary && s.app_name == app_name && s.pid != Some(own_pid())).collect();
    if ours.is_empty() {
        return;
    }
    let steps = duck_steps();
    for stream in ours {
        if steps.iter().skip(1).any(|step| near(&scaled(&original, *step), &stream.volume)) {
            let _ = pulse.request::<()>("giving the volume back", |ud| unsafe {
                pa_context_set_sink_input_volume(pulse.context, stream.index, &original, Some(on_success), ud)
            });
        }
    }
    let _ = std::fs::remove_file(path);
}

// ---- the crossfade ----

/// The recording's timeline position a volume change confirmed now lands
/// at.
unsafe fn landing_frame(shared: &Shared) -> u64 {
    let record = *shared.record.get();
    let (mut usec, mut negative) = (0u64, 0);
    let latency = if !record.is_null() && pa_stream_get_latency(record, &mut usec, &mut negative) == 0 && negative == 0 { usec as f64 / 1e6 } else { 0.0 };
    shared.frames_in.load(Ordering::Relaxed) + ((latency + LANDS_AFTER_MS / 1000.0).max(0.0) * shared.sample_rate) as u64
}

/// How far our output runs behind the stream's own path, in ms: the
/// recording's latency plus the output's, as the streams report them (their
/// buffer targets until they do).
unsafe fn render_lag_ms(shared: &Shared) -> u64 {
    let latency = |stream: *mut pa_stream, fallback: u64| {
        let (mut usec, mut negative) = (0u64, 0);
        if !stream.is_null() && pa_stream_get_latency(stream, &mut usec, &mut negative) == 0 && negative == 0 {
            usec / 1000
        } else {
            fallback
        }
    };
    latency(*shared.record.get(), BLOCK_MS as u64) + latency(*shared.play.get(), (BLOCK_MS * QUEUE_BLOCKS) as u64)
}

/// The server took a volume this route set: its gains start where it lands.
unsafe extern "C" fn on_volume_set(_c: *mut pa_context, success: c_int, userdata: *mut c_void) {
    let shared = &*(userdata as *const Shared);
    // The server answers in the order it was asked.
    let Some(bring) = (*shared.issued.get()).pop_front() else { return };
    if success != 0 && !bring[0].is_nan() {
        (*shared.pending.get()).push_back((landing_frame(shared), bring));
    }
}

/// Set the ducked stream to step `step` of the crossfade.
unsafe fn set_step(shared: &Shared, step: usize) {
    let Some(duck) = (*shared.duck.get()).as_mut() else { return };
    duck.step = step;
    duck.set = scaled(&duck.original, shared.steps[step]);
    shared.step_now.store(step as u32, Ordering::Relaxed);
    let op = pa_context_set_sink_input_volume(shared.context, duck.input, &duck.set, Some(on_volume_set), shared as *const Shared as *mut c_void);
    if !op.is_null() {
        (*shared.issued.get()).push_back(bring_back(&duck.original, &duck.set));
        pa_operation_unref(op);
    }
}

/// Start the crossfade clock unless it runs.
unsafe fn kick(shared: &Shared) {
    if (*shared.timer.get()).is_null() {
        *shared.timer.get() = pa_context_rttime_new(shared.context, pa_rtclock_now(), Some(on_step), shared as *const Shared as *mut c_void);
    }
}

/// One step of the crossfade toward what is asked: down the steps while
/// processing (the stream ducked, our output coming up), back up while
/// monitoring. At the end of the way back the stream has the person's
/// volume and our output goes.
unsafe extern "C" fn on_step(api: *mut pa_mainloop_api, event: *mut pa_time_event, _tv: *const c_void, userdata: *mut c_void) {
    let shared = &*(userdata as *const Shared);
    if step_once(shared) {
        if let Some(free) = (*api).time_free {
            free(event);
        }
        *shared.timer.get() = ptr::null_mut();
        return;
    }
    let lag = render_lag_ms(shared).clamp(STEP_MIN_MS, STEP_MAX_MS);
    shared.step_ms.store(lag, Ordering::Relaxed);
    let step = lag;
    pa_context_rttime_restart(shared.context, event, pa_rtclock_now() + step * 1000);
}

/// One tick of the crossfade; `true` once the route is where it was asked
/// to be. Our output reaches the device one step (its latency) after what
/// it records, so in both directions its share moves a step ahead of the
/// stream's volume: the share for the next step is set on one tick, the
/// stream's volume follows on the next, and the two changes reach the
/// device together.
unsafe fn step_once(shared: &Shared) -> bool {
    let want = shared.want_processing.load(Ordering::Acquire);
    let captured = shared.captured.load(Ordering::Acquire);
    let duck = &mut *shared.duck.get();
    let share_target = &mut *shared.share_target.get();
    let pending = &*shared.pending.get();
    let steps = &shared.steps;
    let last = steps.len() - 1;
    // The application replaced its stream mid-duck (the server starts the
    // new one at the level it remembered, ours): the new stream takes the
    // duck over at its step, and the person's level stays the one to give
    // back. With no stream at all, the next one gets it.
    if let Some(state) = duck.as_mut() {
        if state.input != captured {
            if captured != u32::MAX {
                state.input = captured;
                let step = state.step;
                set_step(shared, step);
            } else if !want {
                *shared.orphan.get() = Some(state.original);
                *duck = None;
            }
        }
    }
    if want {
        *shared.linger.get() = 0;
        if !shared.duck_enabled {
            // Processing on top of the application: our output just comes.
            *share_target = 1.0;
            return true;
        }
        if duck.is_none() {
            // A new output fills its queue first; ducking before it plays
            // would leave a hole.
            if captured == u32::MAX || (*shared.play_since.get()).is_some_and(|since| since.elapsed() < std::time::Duration::from_millis(OUTPUT_WARMUP_MS)) {
                return captured == u32::MAX;
            }
            let (binary, app_name) = (*shared.captured_names.get()).clone();
            let original = *shared.captured_volume.get();
            *duck = Some(Duck { input: captured, binary, app_name, original, set: original, step: 0, share_step: 0 });
            // On disk before the first change, so a crash can be undone.
            write_record(shared, duck.as_ref());
        }
    }
    let Some(state) = duck.as_mut() else {
        // Monitoring, nothing ducked: once the output is silent and what it
        // still queues has played, it goes.
        *share_target = 0.0;
        if (*shared.play.get()).is_null() {
            return true;
        }
        if !pending.is_empty() || (*shared.share.get()) > 0.0 {
            return false;
        }
        let linger = &mut *shared.linger.get();
        *linger += 1;
        if *linger * shared.step_ms.load(Ordering::Relaxed) < LINGER_MS {
            return false;
        }
        *linger = 0;
        close_stream(std::mem::replace(&mut *shared.play.get(), ptr::null_mut()));
        shared.play_index.store(u32::MAX, Ordering::Relaxed);
        return true;
    };
    let target = if want { last } else { 0 };
    let led = state.share_step;
    if led != state.step {
        // Last tick's share lead: the stream's volume follows now.
        set_step(shared, led);
    }
    if led == target {
        if !want {
            // Back at the person's level with our share at zero.
            *duck = None;
            write_record(shared, None);
            return false;
        }
        return true;
    }
    let next = if led < target { led + 1 } else { led - 1 };
    if let Some(state) = duck.as_mut() {
        state.share_step = next;
    }
    *share_target = share_of(steps[next]);
    false
}

// ---- following the application ----

/// A matching stream that appears (or changes) while the route is open is
/// recorded when nothing is; a change to the ducked stream's volume that
/// is not this route's is the person's new level.
unsafe extern "C" fn on_event(context: *mut pa_context, event: u32, index: u32, userdata: *mut c_void) {
    if event & PA_SUBSCRIPTION_EVENT_FACILITY_MASK != PA_SUBSCRIPTION_EVENT_SINK_INPUT {
        return;
    }
    let shared = &*(userdata as *const Shared);
    let op = if event & PA_SUBSCRIPTION_EVENT_TYPE_MASK == PA_SUBSCRIPTION_EVENT_REMOVE {
        if shared.captured.load(Ordering::Acquire) != index {
            return;
        }
        // The recorded stream ended: record another of the application's,
        // if it has one.
        close_stream(std::mem::replace(&mut *shared.record.get(), ptr::null_mut()));
        shared.captured.store(u32::MAX, Ordering::Release);
        pa_context_get_sink_input_info_list(context, Some(on_event_stream), userdata)
    } else {
        pa_context_get_sink_input_info(context, index, Some(on_event_stream), userdata)
    };
    if !op.is_null() {
        pa_operation_unref(op);
    }
}

/// A new stream of the application that starts at a level this route left
/// it (the server remembers the last one) gets the person's level back.
unsafe fn give_back_orphan(shared: &Shared, stream: &SinkInput) -> Option<pa_cvolume> {
    let original = (*shared.orphan.get()).take()?;
    if (*shared.duck.get()).is_some() || !shared.steps.iter().skip(1).any(|step| near(&scaled(&original, *step), &stream.volume)) {
        return None;
    }
    let op = pa_context_set_sink_input_volume(shared.context, stream.index, &original, None, ptr::null_mut());
    if !op.is_null() {
        pa_operation_unref(op);
    }
    write_record(shared, None);
    Some(original)
}

unsafe extern "C" fn on_event_stream(_c: *mut pa_context, info: *const pa_sink_input_info, eol: c_int, userdata: *mut c_void) {
    if eol != 0 || info.is_null() {
        return;
    }
    let shared = &*(userdata as *const Shared);
    let mut stream = SinkInput::read(&*info);
    if stream.index == shared.play_index.load(Ordering::Relaxed) || !shared.matcher.wants(&stream) {
        return;
    }
    let captured = shared.captured.load(Ordering::Acquire);
    if captured == u32::MAX {
        if let Some(volume) = give_back_orphan(shared, &stream) {
            stream.volume = volume;
        }
        capture(shared, &stream);
        if shared.want_processing.load(Ordering::Acquire) || (*shared.duck.get()).is_some() {
            kick(shared);
        }
        return;
    }
    if stream.index != captured {
        return;
    }
    *shared.captured_volume.get() = stream.volume;
    let Some(duck) = (*shared.duck.get()).as_mut() else { return };
    if stream.volume == duck.set {
        return;
    }
    // Not our level: the person moved the player's volume. Our output goes
    // silent at once (the player is loud again), their level becomes the
    // one to give back, and the crossfade runs again from there.
    shared.external_changes.fetch_add(1, Ordering::Relaxed);
    duck.original = stream.volume;
    duck.set = stream.volume;
    duck.step = 0;
    duck.share_step = 0;
    write_record(shared, Some(duck));
    (*shared.pending.get()).clear();
    // Volume sets still unanswered were ours, from before: their answers
    // bring nothing.
    for bring in (*shared.issued.get()).iter_mut() {
        *bring = [f32::NAN; 2];
    }
    *shared.bring.get() = [1.0, 1.0];
    *shared.glide.get() = None;
    *shared.share.get() = 0.0;
    *shared.share_target.get() = 0.0;
    kick(shared);
}

fn monotonic_ns() -> u64 {
    #[repr(C)]
    struct Timespec {
        sec: i64,
        nsec: i64,
    }
    extern "C" {
        fn clock_gettime(clock: c_int, ts: *mut Timespec) -> c_int;
    }
    let mut ts = Timespec { sec: 0, nsec: 0 };
    // SAFETY: CLOCK_MONOTONIC (1) into a local.
    unsafe { clock_gettime(1, &mut ts) };
    ts.sec as u64 * 1_000_000_000 + ts.nsec as u64
}

/// The recording has data. Each frame is brought back to the person's
/// level (the inverse of the duck in force for it), each block goes
/// through the processor, and while processing on to the output at our
/// share of the crossfade. What the output cannot take now is dropped
/// rather than queued, so a slower output never builds latency.
unsafe extern "C" fn on_read(record: *mut pa_stream, _bytes: usize, userdata: *mut c_void) {
    let shared = &*(userdata as *const Shared);
    let scratch = &mut *shared.scratch.get();
    let bring = &mut *shared.bring.get();
    let pending = &mut *shared.pending.get();
    let share = &mut *shared.share.get();
    let share_target = *shared.share_target.get();
    let width = CHANNELS as usize;
    let ramp = (1000.0 / (SHARE_RAMP_MS * shared.sample_rate)) as f32;
    let glide = &mut *shared.glide.get();
    let half = (GLIDE_MS / 2000.0 * shared.sample_rate) as u64;
    loop {
        let mut data: *const c_void = ptr::null();
        let mut bytes = 0usize;
        if pa_stream_peek(record, &mut data, &mut bytes) < 0 || bytes == 0 {
            return;
        }
        if data.is_null() {
            // A hole in the recording: nothing to play for it.
            pa_stream_drop(record);
            continue;
        }
        let samples = std::slice::from_raw_parts(data as *const f32, bytes / 4);
        for chunk in samples.chunks(MAX_BLOCK_FRAMES * width) {
            let frames = chunk.len() / width;
            let first = shared.frames_in.fetch_add(frames as u64, Ordering::Relaxed);
            let block = &mut scratch[..frames * width];
            // Brought back frame by frame: a step lands mid-block.
            let mut shares = [0f32; MAX_BLOCK_FRAMES];
            let mut peak = 0f32;
            for (i, (out, input)) in block.chunks_mut(width).zip(chunk.chunks(width)).enumerate() {
                let frame = first + i as u64;
                if glide.is_none() {
                    if let Some(&(at, to)) = pending.front().filter(|(at, _)| *at <= frame + half) {
                        pending.pop_front();
                        *glide = Some((at.saturating_sub(half), at + half, *bring, to));
                    }
                }
                if let Some((start, end, from, to)) = *glide {
                    if frame >= end {
                        *bring = to;
                        *glide = None;
                    } else if frame >= start {
                        let t = (frame - start) as f32 / (end - start).max(1) as f32;
                        *bring = [from[0] * (to[0] / from[0]).powf(t), from[1] * (to[1] / from[1]).powf(t)];
                    }
                }
                out[0] = input[0] * bring[0];
                out[1] = input[1] * bring[1];
                peak = peak.max(out[0].abs()).max(out[1].abs());
                *share += (share_target - *share).clamp(-ramp, ramp);
                shares[i] = *share;
            }
            if peak > BLAST_GUARD {
                // Not sound: pass the block on as it came.
                block.copy_from_slice(&chunk[..frames * width]);
                shared.guarded.fetch_add(1, Ordering::Relaxed);
            }
            let info = FrameInfo { sample_rate: shared.sample_rate, channels: CHANNELS as u16, frames, host_time: monotonic_ns() };
            // The processor is being replaced: this block goes on as it came.
            if let Ok(mut processor) = shared.processor.try_lock() {
                processor.process(block, &info);
            }
            let play = *shared.play.get();
            if play.is_null() {
                continue;
            }
            for (frame, share) in block.chunks_mut(width).zip(shares.iter()) {
                for sample in frame {
                    *sample *= *share;
                }
            }
            let writable = pa_stream_writable_size(play);
            let want = block.len() * 4;
            // Past its target the queue takes a burst anyway (the recording
            // comes a graph cycle at a time); only a queue grown past
            // [`MAX_QUEUE_MS`] drops, so a slower output cannot build latency.
            let (mut usec, mut negative) = (0u64, 0);
            let queued_ms = if pa_stream_get_latency(play, &mut usec, &mut negative) == 0 && negative == 0 { usec / 1000 } else { 0 };
            let take = if writable == usize::MAX {
                0
            } else if queued_ms < MAX_QUEUE_MS {
                want
            } else {
                want.min(writable - writable % (4 * width))
            };
            if take > 0 {
                pa_stream_write(play, block.as_ptr() as *const c_void, take, ptr::null(), 0, PA_SEEK_RELATIVE);
            }
            if take < want {
                shared.frames_dropped.fetch_add(((want - take) / (4 * width)) as u64, Ordering::Relaxed);
            }
        }
        pa_stream_drop(record);
    }
}

impl Route {
    pub fn open(config: RouteConfig, processor: Box<dyn Processor>) -> Result<Self> {
        // What a route that died left ducked comes back first, so it is
        // not taken for the person's level.
        if let Some(dir) = &config.state_dir {
            restore_after_crash(dir);
        }
        let pulse = Pulse::connect("Makepad audio route")?;
        let lock = pulse.lock();
        let matcher = Matcher { sources: config.sources.clone(), own_pid: own_pid() };
        let targets: Vec<SinkInput> = pulse.sink_inputs()?.into_iter().filter(|stream| matcher.wants(stream)).collect();
        // The stream playing now is recorded; a paused one otherwise.
        let Some(first) = targets.iter().find(|stream| !stream.corked).or(targets.first()).cloned() else {
            return Err(Error::NotFound);
        };
        let output = match &config.output {
            Some(uid) => pulse.sinks()?.into_iter().find(|sink| sink.name == *uid).ok_or(Error::NotFound)?,
            None => pulse.default_output()?,
        };
        let rate = output.rate.max(8000);
        let shared = Box::new(Shared {
            context: pulse.context,
            mainloop: pulse.mainloop,
            record: UnsafeCell::new(ptr::null_mut()),
            captured: AtomicU32::new(u32::MAX),
            captured_volume: UnsafeCell::new(NO_VOLUME),
            captured_names: UnsafeCell::new((String::new(), String::new())),
            play: UnsafeCell::new(ptr::null_mut()),
            play_index: AtomicU32::new(u32::MAX),
            want_processing: AtomicBool::new(false),
            duck_enabled: config.mute != Mute::HearOriginal,
            duck: UnsafeCell::new(None),
            steps: duck_steps(),
            timer: UnsafeCell::new(ptr::null_mut()),
            bring: UnsafeCell::new([1.0, 1.0]),
            glide: UnsafeCell::new(None),
            pending: UnsafeCell::new(std::collections::VecDeque::with_capacity(64)),
            share: UnsafeCell::new(0.0),
            share_target: UnsafeCell::new(0.0),
            issued: UnsafeCell::new(std::collections::VecDeque::with_capacity(64)),
            step_ms: AtomicU64::new(STEP_MIN_MS),
            play_since: UnsafeCell::new(None),
            matcher,
            processor: Mutex::new(processor),
            scratch: UnsafeCell::new(vec![0.0; MAX_BLOCK_FRAMES * CHANNELS as usize]),
            spec: pa_sample_spec { format: PA_SAMPLE_FLOAT32LE, rate, channels: CHANNELS },
            sample_rate: rate as f64,
            output: CString::new(output.name.clone()).unwrap_or_default(),
            state_dir: config.state_dir.clone(),
            frames_in: AtomicU64::new(0),
            frames_dropped: AtomicU64::new(0),
            guarded: AtomicU64::new(0),
            external_changes: AtomicU32::new(0),
            step_now: AtomicU32::new(0),
            orphan: UnsafeCell::new(None),
            linger: UnsafeCell::new(0),
        });
        drop(lock);
        // From here on the route owns what is made: an early return tears it down.
        let route = Route { pulse, shared };
        {
            let _lock = route.pulse.lock();
            unsafe {
                let record = capture(&route.shared, &first);
                if record.is_null() {
                    return Err(fail("pa_stream_connect_record", route.pulse.context));
                }
                if !route.pulse.wait(|| matches!(pa_stream_get_state(record), PA_STREAM_READY | PA_STREAM_FAILED | PA_STREAM_TERMINATED)) || pa_stream_get_state(record) != PA_STREAM_READY {
                    return Err(fail("opening the tap", route.pulse.context));
                }
                let userdata = &*route.shared as *const Shared as *mut c_void;
                pa_context_set_subscribe_callback(route.pulse.context, Some(on_event), userdata);
                let _ = route.pulse.request::<()>("subscribing to streams", |ud| pa_context_subscribe(route.pulse.context, PA_SUBSCRIPTION_MASK_SINK_INPUT, Some(on_success), ud));
            }
        }
        if config.processing {
            route.set_processing(true)?;
        }
        Ok(route)
    }

    /// Monitoring (`false`): the application plays to its own device
    /// untouched; the processor sees a copy and nothing plays. Processing
    /// (`true`): the application's stream is ducked to 2^-13, the recording
    /// is brought back by the inverse, and what the processor leaves plays
    /// to the output. The switch is a crossfade in volume steps: our output
    /// comes up exactly as the stream's own level goes down, and back.
    pub fn set_processing(&self, processing: bool) -> Result<()> {
        let _lock = self.pulse.lock();
        let shared = &*self.shared;
        shared.want_processing.store(processing, Ordering::Release);
        unsafe {
            if processing && (*shared.play.get()).is_null() {
                let play = open_play(shared);
                if play.is_null() {
                    return Err(fail("pa_stream_connect_playback", self.pulse.context));
                }
                if !self.pulse.wait(|| matches!(pa_stream_get_state(play), PA_STREAM_READY | PA_STREAM_FAILED | PA_STREAM_TERMINATED)) || pa_stream_get_state(play) != PA_STREAM_READY {
                    close_stream(std::mem::replace(&mut *shared.play.get(), ptr::null_mut()));
                    return Err(fail("opening the output stream", self.pulse.context));
                }
                shared.play_index.store(pa_stream_get_index(play), Ordering::Relaxed);
                *shared.play_since.get() = Some(std::time::Instant::now());
                // A queue full of silence: the output starts at once and never
                // waits on its first blocks.
                let silence = vec![0u8; pa_stream_writable_size(play).min(1 << 20)];
                if !silence.is_empty() {
                    pa_stream_write(play, silence.as_ptr() as *const c_void, silence.len() - silence.len() % (4 * CHANNELS as usize), ptr::null(), 0, PA_SEEK_RELATIVE);
                }
            }
            kick(shared);
        }
        Ok(())
    }

    /// The mode asked for last ([`Route::set_processing`]); the crossfade
    /// to it may still run.
    pub fn processing(&self) -> bool {
        self.shared.want_processing.load(Ordering::Acquire)
    }

    pub fn set_processor(&self, processor: Box<dyn Processor>) {
        // The audio thread only ever try-locks: while this holds the lock it
        // passes a block on unprocessed rather than wait. The old processor
        // is dropped here, off the audio thread.
        let old = match self.shared.processor.lock() {
            Ok(mut current) => std::mem::replace(&mut *current, processor),
            Err(_) => return,
        };
        drop(old);
    }

    /// Replaced processors are dropped by [`Route::set_processor`] itself.
    pub fn reclaim(&self) {}

    pub fn sample_rate(&self) -> f64 {
        self.shared.sample_rate
    }

    pub fn channels(&self) -> u16 {
        CHANNELS as u16
    }

    pub fn describe(&self) -> String {
        let shared = &*self.shared;
        let _lock = self.pulse.lock();
        let captured = shared.captured.load(Ordering::Acquire);
        let recording = if captured == u32::MAX { "no stream recorded".to_string() } else { format!("sink input #{captured} recorded by itself") };
        let (step, last) = (shared.step_now.load(Ordering::Relaxed) as usize, shared.steps.len() - 1);
        let mode = match (unsafe { (*shared.duck.get()).is_some() }, self.processing()) {
            (false, false) => "monitoring (the player plays by itself)".to_string(),
            (_, true) if !shared.duck_enabled => "processing on top of the player (HearOriginal)".to_string(),
            (true, true) if step == last => "processing (the player's stream at 2^-13, brought back x 8192)".to_string(),
            (_, want) => format!("crossfading to {} (step {step}/{last})", if want { "processing" } else { "monitoring" }),
        };
        format!(
            "pulse: {mode}; {recording}; float32 {CHANNELS} ch at {:.0} Hz -> '{}', crossfade steps of {} ms; {} frames in, {} dropped, {} blocks guarded, {} volume changes by the person",
            shared.sample_rate,
            shared.output.to_string_lossy(),
            shared.step_ms.load(Ordering::Relaxed),
            shared.frames_in.load(Ordering::Relaxed),
            shared.frames_dropped.load(Ordering::Relaxed),
            shared.guarded.load(Ordering::Relaxed),
            shared.external_changes.load(Ordering::Relaxed),
        )
    }
}

impl Drop for Route {
    /// The person's volume comes back at once (the crossfade needs the
    /// route alive), then everything the route made goes.
    fn drop(&mut self) {
        let _lock = self.pulse.lock();
        let shared = &*self.shared;
        unsafe {
            pa_context_set_subscribe_callback(self.pulse.context, None, ptr::null_mut());
            let timer = std::mem::replace(&mut *shared.timer.get(), ptr::null_mut());
            if !timer.is_null() {
                if let Some(free) = (*pa_threaded_mainloop_get_api(self.pulse.mainloop)).time_free {
                    free(timer);
                }
            }
            if let Some(duck) = (*shared.duck.get()).take() {
                let _ = self.pulse.request::<()>("giving the volume back", |ud| pa_context_set_sink_input_volume(self.pulse.context, duck.input, &duck.original, Some(on_success), ud));
                write_record(shared, None);
            }
            close_stream(std::mem::replace(&mut *shared.record.get(), ptr::null_mut()));
            close_stream(std::mem::replace(&mut *shared.play.get(), ptr::null_mut()));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_players_match_by_bundle_binary_or_name() {
        assert!(matches_bundle("com.spotify.client", "spotify", "", ""));
        assert!(matches_bundle("com.spotify.client", "", "Spotify", ""));
        // A Flatpak names the application by its own id.
        assert!(matches_bundle("com.spotify.client", "", "", "com.spotify.Client"));
        assert!(matches_bundle("spotify", "spotify", "", ""));
        assert!(!matches_bundle("com.spotify.client", "firefox", "Firefox", ""));
        assert!(!matches_bundle("com.spotify.client", "", "", ""));
    }

    #[test]
    fn own_streams_are_never_taken() {
        let matcher = Matcher { sources: vec![Source::BundleId("spotify".into())], own_pid: 42 };
        let stream = |pid| SinkInput { index: 1, pid: Some(pid), binary: "spotify".into(), app_name: String::new(), app_id: String::new(), corked: false, volume: NO_VOLUME };
        assert!(matcher.wants(&stream(7)));
        assert!(!matcher.wants(&stream(42)));
    }

    fn stereo(value: u32) -> pa_cvolume {
        let mut volume = NO_VOLUME;
        volume.channels = 2;
        volume.values[0] = value;
        volume.values[1] = value;
        volume
    }

    #[test]
    fn the_duck_ends_at_two_to_the_minus_13_and_brings_back_exactly() {
        let steps = duck_steps();
        assert_eq!(steps[0], 1.0);
        assert_eq!(*steps.last().unwrap(), DUCK);
        assert!(steps.windows(2).all(|pair| pair[1] < pair[0]));
        // 70%, as a person sets it, ducked and brought back: the product is 1
        // to float precision, whatever the rounding of the ducked volume.
        let original = stereo(45875);
        let ducked = scaled(&original, DUCK);
        let bring = bring_back(&original, &ducked);
        let applied = unsafe { pa_sw_volume_to_linear(ducked.values[0]) / pa_sw_volume_to_linear(original.values[0]) };
        assert!((bring[0] as f64 * applied - 1.0).abs() < 1e-6);
        assert!((bring[0] - 8192.0).abs() / 8192.0 < 0.01);
        assert_eq!(share_of(1.0), 0.0);
        assert!((share_of(DUCK) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn a_remembered_level_a_few_units_off_is_still_ours() {
        // The server gave a new stream 3032 for a step of 3036.
        assert!(near(&stereo(3036), &stereo(3032)));
        assert!(!near(&stereo(3036), &stereo(3200)));
        assert!(!near(&stereo(3036), &NO_VOLUME));
    }

    #[test]
    fn the_crash_record_reads_back() {
        let dir = std::env::temp_dir().join(format!("audio-route-test-{}", own_pid()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join(RECORD_FILE);
        std::fs::write(&path, "spotify\tSpotify\t45875,45000\n").unwrap();
        let (binary, app_name, volume) = read_record(&path).unwrap();
        assert_eq!((binary.as_str(), app_name.as_str()), ("spotify", "Spotify"));
        assert_eq!((volume.channels, volume.values[0], volume.values[1]), (2, 45875, 45000));
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_process_descends_from_itself_and_its_parent() {
        let me = own_pid();
        assert!(descends_from(me, me));
        let parent = parent_of(me).unwrap();
        assert!(descends_from(me, parent));
        assert!(!descends_from(parent, me));
    }
}
