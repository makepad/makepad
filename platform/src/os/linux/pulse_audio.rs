#![allow(dead_code)]
use {
    self::super::{alsa_audio::AlsaAudioAccess, libc_sys::timeval, pulse_sys::*},
    crate::{audio::*, makepad_live_id::*, thread::SignalToUI},
    std::collections::HashSet,
    std::ffi::{CStr, CString},
    std::os::raw::{c_char, c_int, c_void},
    std::sync::atomic::{AtomicBool, AtomicU32, Ordering},
    std::sync::{Arc, Mutex},
    std::time::{Duration, SystemTime, UNIX_EPOCH},
};

/// How long we are willing to block on a PulseAudio server that does not
/// answer. A server that accepts our connection but never completes a request -
/// a container with a stub sink, a server that is shutting down - must not hang
/// the UI thread.
const PULSE_TIMEOUT_MS: u64 = 3000;

/// Values of `PulseContextState::state`.
const CONTEXT_CONNECTING: u32 = 0;
const CONTEXT_READY: u32 = 1;
const CONTEXT_FAILED: u32 = 2;

/// Values of the `ready_state` of an input/output stream.
const STREAM_CONNECTING: u32 = 0;
const STREAM_READY: u32 = 1;
const STREAM_FAILED: u32 = 2;

struct PulseAudioDesc {
    name: String,
    desc: AudioDeviceDesc,
}
/*
struct PulseAudioDevice {
}
*/

struct AlsaAudioDeviceRef {
    device_id: AudioDeviceId,
    _is_terminated: bool,
}

/// State shared with the pulse mainloop thread through a raw pointer.
///
/// This is deliberately lock-free: the mainloop thread runs the context state
/// callback while our thread can be sitting inside `pa_threaded_mainloop_wait`
/// holding the `PulseAudioAccess` mutex, so a callback that took that mutex
/// would deadlock both threads.
struct PulseContextState {
    state: AtomicU32,
    main_loop: *mut pa_threaded_mainloop,
    change_signal: SignalToUI,
}

struct PulseTimeoutState {
    expired: AtomicBool,
    main_loop: *mut pa_threaded_mainloop,
}

/// A one shot mainloop timer that wakes up `pa_threaded_mainloop_wait`, so that
/// every wait in here is bounded.
///
/// Created and freed with the mainloop lock held, which is also the only thing
/// that keeps the callback from overlapping with either.
struct PulseTimeout {
    api: *mut pa_mainloop_api,
    event: *mut pa_time_event,
    state: Box<PulseTimeoutState>,
}

impl PulseTimeout {
    unsafe fn new(
        api: *mut pa_mainloop_api,
        main_loop: *mut pa_threaded_mainloop,
        millis: u64,
    ) -> Self {
        let state = Box::new(PulseTimeoutState {
            expired: AtomicBool::new(false),
            main_loop,
        });
        let deadline = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            + Duration::from_millis(millis);
        let tv = timeval {
            tv_sec: deadline.as_secs() as _,
            tv_usec: deadline.subsec_micros() as _,
        };
        let event = match (*api).time_new {
            Some(time_new) => time_new(
                api,
                &tv,
                Some(Self::timeout_callback),
                &*state as *const _ as *mut _,
            ),
            None => std::ptr::null_mut(),
        };
        if event.is_null() {
            crate::error!("PulseAudio: could not arm a timeout, not waiting on the server");
        }
        Self { api, event, state }
    }

    /// Also true when there is no timer at all: without one we cannot bound the
    /// wait, and giving up beats freezing the app.
    fn expired(&self) -> bool {
        self.event.is_null() || self.state.expired.load(Ordering::Acquire)
    }

    unsafe fn free(self) {
        if !self.event.is_null() {
            if let Some(time_free) = (*self.api).time_free {
                time_free(self.event);
            }
        }
    }

    unsafe extern "C" fn timeout_callback(
        _api: *mut pa_mainloop_api,
        _event: *mut pa_time_event,
        _tv: *const timeval,
        state_ptr: *mut c_void,
    ) {
        let state = &*(state_ptr as *const PulseTimeoutState);
        state.expired.store(true, Ordering::Release);
        pa_threaded_mainloop_signal(state.main_loop, 0);
    }
}

unsafe fn cstr_to_string(ptr: *const c_char) -> Option<String> {
    if ptr.is_null() {
        None
    } else {
        Some(CStr::from_ptr(ptr).to_string_lossy().into_owned())
    }
}

unsafe fn context_error(context: *mut pa_context) -> String {
    if context.is_null() {
        return "no context".to_string();
    }
    let errno = pa_context_errno(context);
    cstr_to_string(pa_strerror(errno)).unwrap_or_else(|| format!("error {}", errno))
}

/// Disconnects a stream and frees the boxed callback state that belongs to it.
///
/// Must be called with the mainloop lock held, so that no callback can be
/// running on the mainloop thread. All callbacks are cleared first, which makes
/// us the sole owner of the box: libpulse will not hand it back to us through a
/// TERMINATED state change afterwards.
///
/// A stream the server has not finished creating cannot be disconnected at all
/// (`pa_stream_disconnect` is a no-op returning PA_ERR_BADSTATE until the create
/// reply arrives), and merely dropping our reference leaves it alive and
/// attached to the device: "If you only unreference it, then it will live on"
/// (pulse/stream.h). That is exactly what the open timeout does, so those are
/// parked and disconnected later instead - otherwise a timed out input stream
/// stays latched onto the microphone for the life of the process.
unsafe fn destroy_stream<T>(pulse: &PulseAudioAccess, stream: *mut pa_stream, state_ptr: *mut T) {
    if stream.is_null() {
        return;
    }
    pa_stream_set_state_callback(stream, None, std::ptr::null_mut());
    pa_stream_set_read_callback(stream, None, std::ptr::null_mut());
    pa_stream_set_write_callback(stream, None, std::ptr::null_mut());
    if pa_stream_disconnect(stream) != 0
        && pa_stream_get_state(stream) == PA_STREAM_CREATING
        && pulse.is_context_ready()
    {
        pulse.zombie_streams.lock().unwrap().push(stream);
    } else {
        pa_stream_unref(stream);
    }
    // the callbacks are gone, so nothing can reach the box any more whether the
    // stream itself is parked or not
    if !state_ptr.is_null() {
        drop(Box::from_raw(state_ptr));
    }
}

/// Waits until a freshly connected stream is ready, and returns whether it is.
///
/// Must be called with the mainloop lock held. A device that never answers
/// (pulse happily reports sinks that cannot actually be opened, e.g. the dummy
/// sink of a sound server without any hardware) gives up on the timeout instead
/// of blocking the UI thread for good.
///
/// `ready_state` is a raw pointer into the stream's boxed state rather than a
/// reference: the read/write callbacks take `&mut` to that same box, and they
/// run on the mainloop thread whenever `pa_threaded_mainloop_wait` below drops
/// the lock, so holding a live shared reference across the wait would alias a
/// `&mut`.
unsafe fn wait_for_stream(
    pulse: &PulseAudioAccess,
    ready_state: *const AtomicU32,
    what: &str,
    name: &str,
) -> bool {
    let timeout = PulseTimeout::new(pulse.main_loop_api, pulse.main_loop, PULSE_TIMEOUT_MS);
    let ready = loop {
        match (*ready_state).load(Ordering::Acquire) {
            STREAM_READY => break true,
            STREAM_FAILED => {
                crate::error!(
                    "PulseAudio: {} {} could not be started ({})",
                    what,
                    name,
                    context_error(pulse.context)
                );
                break false;
            }
            _ => (),
        }
        // once the server is gone nothing will ever signal us again
        if !pulse.is_context_ready() {
            break false;
        }
        if timeout.expired() {
            crate::error!("PulseAudio: timed out opening {} {}", what, name);
            break false;
        }
        pa_threaded_mainloop_wait(pulse.main_loop);
    };
    timeout.free();
    ready
}

struct PulseInputStream {
    device_id: AudioDeviceId,
    stream: *mut pa_stream,
    input_ptr: *mut PulseInputStruct,
}

struct PulseInputStruct {
    device_id: AudioDeviceId,
    input_fn: Arc<Mutex<Option<AudioInputFn>>>,
    audio_buffer: AudioBuffer,
    ready_state: AtomicU32,
    main_loop: *mut pa_threaded_mainloop,
}

impl PulseInputStream {
    unsafe fn new(
        device_id: AudioDeviceId,
        name: &str,
        index: usize,
        pulse: &PulseAudioAccess,
    ) -> Option<PulseInputStream> {
        if !pulse.is_context_ready() {
            return None;
        }
        let device_name = match CString::new(name) {
            Ok(v) => v,
            Err(_) => {
                crate::error!("PulseAudio: invalid input device name {:?}", name);
                return None;
            }
        };
        pa_threaded_mainloop_lock(pulse.main_loop);
        let sample_spec = pa_sample_spec {
            format: PA_SAMPLE_FLOAT32LE,
            rate: 48000,
            channels: 2,
        };

        let stream = pa_stream_new(
            pulse.context,
            "makepad input stream\0".as_ptr(),
            &sample_spec,
            std::ptr::null(),
        );
        if stream.is_null() {
            crate::error!(
                "PulseAudio: pa_stream_new failed for input device {} ({})",
                name,
                context_error(pulse.context)
            );
            pa_threaded_mainloop_unlock(pulse.main_loop);
            return None;
        }
        let input_ptr = Box::into_raw(Box::new(PulseInputStruct {
            device_id,
            ready_state: AtomicU32::new(STREAM_CONNECTING),
            input_fn: pulse.audio_input_cb[index].clone(),
            audio_buffer: AudioBuffer::default(),
            main_loop: pulse.main_loop,
        }));
        pa_stream_set_state_callback(
            stream,
            Some(Self::recording_stream_state_callback),
            input_ptr as *mut _,
        );
        pa_stream_set_read_callback(
            stream,
            Some(Self::recording_stream_read_callback),
            input_ptr as *mut _,
        );

        let buffer_attr = pa_buffer_attr {
            maxlength: std::u32::MAX,
            tlength: (8 * pulse.buffer_frames) as u32,
            prebuf: 0,
            minreq: std::u32::MAX,
            fragsize: std::u32::MAX,
        };
        let flags = PA_STREAM_ADJUST_LATENCY;

        if pa_stream_connect_record(stream, device_name.as_ptr() as *const u8, &buffer_attr, flags)
            != 0
        {
            crate::error!(
                "PulseAudio: could not open input device {} ({})",
                name,
                context_error(pulse.context)
            );
            destroy_stream(pulse, stream, input_ptr);
            pa_threaded_mainloop_unlock(pulse.main_loop);
            return None;
        }

        let ready = wait_for_stream(pulse, &raw const (*input_ptr).ready_state, "input device", name);
        if !ready {
            destroy_stream(pulse, stream, input_ptr);
            pa_threaded_mainloop_unlock(pulse.main_loop);
            return None;
        }

        pa_threaded_mainloop_unlock(pulse.main_loop);
        Some(Self {
            device_id,
            stream,
            input_ptr,
        })
    }

    pub unsafe fn terminate(self, pulse: &PulseAudioAccess) {
        pa_threaded_mainloop_lock(pulse.main_loop);
        destroy_stream(pulse, self.stream, self.input_ptr);
        pa_threaded_mainloop_unlock(pulse.main_loop);
    }

    /// Whether the server has torn this stream down since we opened it.
    fn has_failed(&self) -> bool {
        unsafe { (*self.input_ptr).ready_state.load(Ordering::Acquire) == STREAM_FAILED }
    }

    unsafe extern "C" fn recording_stream_read_callback(
        stream: *mut pa_stream,
        _nbytes: usize,
        input_ptr: *mut c_void,
    ) {
        let input = &mut *(input_ptr as *mut PulseInputStruct);
        let mut read_ptr: *const c_void = std::ptr::null();
        let mut byte_count = 0;
        if pa_stream_peek(stream, &mut read_ptr, &mut byte_count) != 0 {
            crate::error!("PulseAudio: pa_stream_peek failed");
            return;
        }
        if byte_count == 0 {
            return;
        }
        // a null buffer with a non-zero size is a hole in the recording stream
        if read_ptr.is_null() {
            pa_stream_drop(stream);
            return;
        }
        let mut input_fn = (*input).input_fn.lock().unwrap();
        if let Some(input_fn) = &mut *input_fn {
            let interleaved = std::slice::from_raw_parts(read_ptr as *const f32, byte_count / 4);
            input.audio_buffer.copy_from_interleaved(2, interleaved);
            input_fn(
                AudioInfo {
                    device_id: input.device_id,
                    time: None,
                    sample_rate: 48000.0,
                },
                &input.audio_buffer,
            );
        }
        pa_stream_drop(stream);
    }

    unsafe extern "C" fn recording_stream_state_callback(
        stream: *mut pa_stream,
        input_ptr: *mut c_void,
    ) {
        let input_ptr = input_ptr as *mut PulseInputStruct;
        let state = pa_stream_get_state(stream);
        match state {
            PA_STREAM_READY => (*input_ptr)
                .ready_state
                .store(STREAM_READY, Ordering::Release),
            // the box is owned by PulseInputStream and freed in destroy_stream,
            // so a terminated stream only has to stop anyone waiting on it
            PA_STREAM_FAILED | PA_STREAM_TERMINATED => (*input_ptr)
                .ready_state
                .store(STREAM_FAILED, Ordering::Release),
            _ => (),
        }
        pa_threaded_mainloop_signal((*input_ptr).main_loop, 0);
    }
}

struct PulseOutputStream {
    device_id: AudioDeviceId,
    stream: *mut pa_stream,
    output_ptr: *mut PulseOutputStruct,
}

struct PulseOutputStruct {
    device_id: AudioDeviceId,
    output_fn: Arc<Mutex<Option<AudioOutputFn>>>,
    write_byte_count: usize,
    clear_on_read: bool,
    write_error: bool,
    ready_state: AtomicU32,
    audio_buffer: AudioBuffer,
    main_loop: *mut pa_threaded_mainloop,
}

impl PulseOutputStream {
    unsafe fn new(
        device_id: AudioDeviceId,
        name: &str,
        index: usize,
        pulse: &PulseAudioAccess,
    ) -> Option<PulseOutputStream> {
        if !pulse.is_context_ready() {
            return None;
        }
        let device_name = match CString::new(name) {
            Ok(v) => v,
            Err(_) => {
                crate::error!("PulseAudio: invalid output device name {:?}", name);
                return None;
            }
        };
        pa_threaded_mainloop_lock(pulse.main_loop);
        let sample_spec = pa_sample_spec {
            format: PA_SAMPLE_FLOAT32LE,
            rate: 48000,
            channels: 2,
        };

        let stream = pa_stream_new(
            pulse.context,
            "makepad output stream\0".as_ptr(),
            &sample_spec,
            std::ptr::null(),
        );
        if stream.is_null() {
            crate::error!(
                "PulseAudio: pa_stream_new failed for output device {} ({})",
                name,
                context_error(pulse.context)
            );
            pa_threaded_mainloop_unlock(pulse.main_loop);
            return None;
        }

        let output_ptr = Box::into_raw(Box::new(PulseOutputStruct {
            device_id,
            clear_on_read: true,
            output_fn: pulse.audio_output_cb[index].clone(),
            write_byte_count: 0,
            write_error: false,
            ready_state: AtomicU32::new(STREAM_CONNECTING),
            audio_buffer: AudioBuffer::default(),
            main_loop: pulse.main_loop,
        }));
        pa_stream_set_state_callback(
            stream,
            Some(Self::playback_stream_state_callback),
            output_ptr as *mut _,
        );

        let buffer_attr = pa_buffer_attr {
            maxlength: std::u32::MAX,
            tlength: (8 * pulse.buffer_frames) as u32,
            prebuf: 0,
            minreq: std::u32::MAX,
            fragsize: std::u32::MAX,
        };
        let flags = PA_STREAM_ADJUST_LATENCY | PA_STREAM_START_CORKED | PA_STREAM_START_UNMUTED;

        if pa_stream_connect_playback(
            stream,
            device_name.as_ptr() as *const u8,
            &buffer_attr,
            flags,
            std::ptr::null(),
            std::ptr::null_mut(),
        ) != 0
        {
            crate::error!(
                "PulseAudio: could not open output device {} ({})",
                name,
                context_error(pulse.context)
            );
            destroy_stream(pulse, stream, output_ptr);
            pa_threaded_mainloop_unlock(pulse.main_loop);
            return None;
        }

        let ready = wait_for_stream(pulse, &raw const (*output_ptr).ready_state, "output device", name);
        if !ready {
            destroy_stream(pulse, stream, output_ptr);
            pa_threaded_mainloop_unlock(pulse.main_loop);
            return None;
        }

        (*output_ptr).write_byte_count = pa_stream_writable_size(stream);

        pa_stream_set_write_callback(
            stream,
            Some(Self::playback_stream_write_callback),
            output_ptr as *mut _,
        );

        let op = pa_stream_cork(stream, 0, None, std::ptr::null_mut());
        if op.is_null() {
            crate::error!(
                "PulseAudio: pa_stream_cork failed for output device {} ({})",
                name,
                context_error(pulse.context)
            );
            destroy_stream(pulse, stream, output_ptr);
            pa_threaded_mainloop_unlock(pulse.main_loop);
            return None;
        }
        pa_operation_unref(op);

        pa_threaded_mainloop_unlock(pulse.main_loop);
        Some(Self {
            device_id,
            stream,
            output_ptr,
        })
    }

    pub fn terminate(&self, pulse: &PulseAudioAccess) {
        unsafe {
            pa_threaded_mainloop_lock(pulse.main_loop);
            destroy_stream(pulse, self.stream, self.output_ptr);
            pa_threaded_mainloop_unlock(pulse.main_loop);
        }
    }

    /// Whether the server has torn this stream down since we opened it.
    fn has_failed(&self) -> bool {
        unsafe { (*self.output_ptr).ready_state.load(Ordering::Acquire) == STREAM_FAILED }
    }

    unsafe extern "C" fn playback_stream_write_callback(
        stream: *mut pa_stream,
        _nbytes: usize,
        output_ptr: *mut c_void,
    ) {
        let output = &mut *(output_ptr as *mut PulseOutputStruct);
        let mut write_ptr = std::ptr::null_mut();
        let mut write_byte_count = output.write_byte_count;
        if pa_stream_begin_write(stream, &mut write_ptr, &mut write_byte_count) != 0
            || write_ptr.is_null()
        {
            if !output.write_error {
                output.write_error = true;
                crate::error!("PulseAudio: pa_stream_begin_write failed");
            }
            return;
        }
        if write_byte_count == output.write_byte_count {
            let mut output_fn = (*output).output_fn.lock().unwrap();
            output.audio_buffer.resize(output.write_byte_count / 8, 2);
            if let Some(output_fn) = &mut *output_fn {
                output_fn(
                    AudioInfo {
                        device_id: output.device_id,
                        time: None,
                        sample_rate: 48000.0,
                    },
                    &mut output.audio_buffer,
                );
                // lets copy it to interleaved format
                let interleaved = std::slice::from_raw_parts_mut(
                    write_ptr as *mut f32,
                    output.write_byte_count / 4,
                );
                output.audio_buffer.copy_to_interleaved(interleaved);
            } else {
                std::ptr::write_bytes(write_ptr as *mut u8, 0, write_byte_count);
            }
        } else {
            // we got handed a differently sized buffer than we prepared for,
            // play silence rather than whatever happens to be in it
            std::ptr::write_bytes(write_ptr as *mut u8, 0, write_byte_count);
            if !output.write_error {
                output.write_error = true;
                crate::error!("PulseAudio: audio buffer size unexpected");
            }
        }
        let flags = if output.clear_on_read {
            output.clear_on_read = false;
            PA_SEEK_RELATIVE_ON_READ
        } else {
            PA_SEEK_RELATIVE
        };

        if pa_stream_write(stream, write_ptr, write_byte_count, None, 0, flags) != 0 {
            if !output.write_error {
                output.write_error = true;
                crate::error!("PulseAudio: pa_stream_write failed");
            }
            // a stream we cannot write to is dead even if the server has not
            // said so; let it be reaped rather than silently playing nothing
            output.ready_state.store(STREAM_FAILED, Ordering::Release);
        }
    }

    unsafe extern "C" fn playback_stream_state_callback(
        stream: *mut pa_stream,
        output_ptr: *mut c_void,
    ) {
        let output_ptr = output_ptr as *mut PulseOutputStruct;
        let state = pa_stream_get_state(stream);
        match state {
            PA_STREAM_READY => (*output_ptr)
                .ready_state
                .store(STREAM_READY, Ordering::Release),
            // the box is owned by PulseOutputStream and freed in destroy_stream,
            // so a terminated stream only has to stop anyone waiting on it
            PA_STREAM_FAILED | PA_STREAM_TERMINATED => (*output_ptr)
                .ready_state
                .store(STREAM_FAILED, Ordering::Release),
            _ => (),
        }
        pa_threaded_mainloop_signal((*output_ptr).main_loop, 0);
    }
}

pub struct PulseAudioAccess {
    pub audio_input_cb: [Arc<Mutex<Option<AudioInputFn>>>; MAX_AUDIO_DEVICE_INDEX],
    pub audio_output_cb: [Arc<Mutex<Option<AudioOutputFn>>>; MAX_AUDIO_DEVICE_INDEX],

    buffer_frames: usize,

    audio_outputs: Vec<PulseOutputStream>,
    audio_inputs: Vec<PulseInputStream>,
    device_query: Option<PulseDeviceQuery>,

    device_descs: Vec<PulseAudioDesc>,
    change_signal: SignalToUI,
    context_state: *mut PulseContextState,
    main_loop: *mut pa_threaded_mainloop,
    main_loop_api: *mut pa_mainloop_api,
    context: *mut pa_context,

    failed_devices: HashSet<AudioDeviceId>,
    /// Devices asked for by the last call, used to tell an explicit request
    /// apart from the automatic re-selection that drives the retry loop.
    last_input_request: Vec<AudioDeviceId>,
    last_output_request: Vec<AudioDeviceId>,
    /// Streams awaiting a disconnect they were not allowed to do yet, see
    /// [`destroy_stream`]. Behind its own lock because streams are torn down
    /// through a `&PulseAudioAccess`.
    zombie_streams: Mutex<Vec<*mut pa_stream>>,
}

struct PulseDeviceDesc {
    name: String,
    description: String,
    _index: u32,
}

#[derive(Default)]
struct PulseDeviceQuery {
    main_loop: Option<*mut pa_threaded_mainloop>,
    sink_list: Vec<PulseDeviceDesc>,
    source_list: Vec<PulseDeviceDesc>,
    default_sink: Option<String>,
    default_source: Option<String>,
}

impl PulseDeviceQuery {
    unsafe fn signal(&self) {
        if let Some(main_loop) = self.main_loop {
            pa_threaded_mainloop_signal(main_loop, 0);
        }
    }
}

impl PulseAudioAccess {
    /// Connects to the PulseAudio server.
    ///
    /// Returns `None` when there is no usable server (headless machines,
    /// containers without a sound server, and so on); the caller is expected to
    /// carry on with ALSA only. Nothing in here may panic or abort: a machine
    /// without audio is a perfectly normal machine to run on.
    pub fn new(change_signal: SignalToUI, alsa_audio: &AlsaAudioAccess) -> Option<Arc<Mutex<Self>>> {
        unsafe {
            let main_loop = pa_threaded_mainloop_new();
            if main_loop.is_null() {
                crate::error!("PulseAudio: pa_threaded_mainloop_new failed, using ALSA only");
                return None;
            }
            let main_loop_api = pa_threaded_mainloop_get_api(main_loop);
            if main_loop_api.is_null() {
                crate::error!("PulseAudio: pa_threaded_mainloop_get_api failed, using ALSA only");
                pa_threaded_mainloop_free(main_loop);
                return None;
            }
            let prop_list = pa_proplist_new();
            let context =
                pa_context_new_with_proplist(main_loop_api, "makepad\0".as_ptr(), prop_list);
            if !prop_list.is_null() {
                pa_proplist_free(prop_list);
            }
            if context.is_null() {
                crate::error!("PulseAudio: pa_context_new failed, using ALSA only");
                pa_threaded_mainloop_free(main_loop);
                return None;
            }

            let context_state = Box::into_raw(Box::new(PulseContextState {
                state: AtomicU32::new(CONTEXT_CONNECTING),
                main_loop,
                change_signal: change_signal.clone(),
            }));
            pa_context_set_state_callback(
                context,
                Some(Self::context_state_callback),
                context_state as *mut _,
            );

            // NOAUTOSPAWN: without it libpulse forks and execs a sound daemon
            // when none is running, synchronously, inside this call - before the
            // mainloop exists and before any of our timeouts are armed, so it
            // blocks this thread for as long as the spawn takes (measured: 7s
            // for a daemon that takes 7s to fail). Enumerating audio devices
            // must not freeze the UI, and must not start a sound server on a
            // machine that deliberately has none.
            if pa_context_connect(
                context,
                std::ptr::null(),
                PA_CONTEXT_NOAUTOSPAWN,
                std::ptr::null(),
            ) != 0
            {
                crate::error!(
                    "PulseAudio: pa_context_connect failed ({}), using ALSA only",
                    context_error(context)
                );
                Self::destroy_context(main_loop, context, context_state);
                return None;
            }
            if pa_threaded_mainloop_start(main_loop) != 0 {
                crate::error!("PulseAudio: pa_threaded_mainloop_start failed, using ALSA only");
                Self::destroy_context(main_loop, context, context_state);
                return None;
            }

            pa_threaded_mainloop_lock(main_loop);
            let timeout = PulseTimeout::new(main_loop_api, main_loop, PULSE_TIMEOUT_MS);
            while (*context_state).state.load(Ordering::Acquire) == CONTEXT_CONNECTING
                && !timeout.expired()
            {
                pa_threaded_mainloop_wait(main_loop);
            }
            timeout.free();
            pa_threaded_mainloop_unlock(main_loop);

            match (*context_state).state.load(Ordering::Acquire) {
                CONTEXT_READY => (),
                CONTEXT_CONNECTING => {
                    crate::error!(
                        "PulseAudio: timed out connecting to a sound server, using ALSA only"
                    );
                    Self::destroy_context(main_loop, context, context_state);
                    return None;
                }
                _ => {
                    crate::error!(
                        "PulseAudio: could not connect to a sound server ({}), using ALSA only",
                        context_error(context)
                    );
                    Self::destroy_context(main_loop, context, context_state);
                    return None;
                }
            }

            Some(Arc::new(Mutex::new(PulseAudioAccess {
                // ~20ms at 48k. Matching the old 256 (~5ms) underran under soft
                // decode; PipeWire-pulse is happier with a slightly larger tlength.
                buffer_frames: 1024,
                audio_inputs: Vec::new(),
                audio_outputs: Vec::new(),
                change_signal,
                device_query: None,
                context,
                main_loop,
                main_loop_api,
                context_state,
                audio_input_cb: alsa_audio.audio_input_cb.clone(),
                audio_output_cb: alsa_audio.audio_output_cb.clone(),
                device_descs: Default::default(),
                failed_devices: Default::default(),
                last_input_request: Vec::new(),
                last_output_request: Vec::new(),
                zombie_streams: Default::default(),
            })))
        }
    }

    /// Tears down a context that never made it into a `PulseAudioAccess`.
    unsafe fn destroy_context(
        main_loop: *mut pa_threaded_mainloop,
        context: *mut pa_context,
        context_state: *mut PulseContextState,
    ) {
        // stop the mainloop first, so no callback can reference the state below
        pa_threaded_mainloop_stop(main_loop);
        pa_context_set_state_callback(context, None, std::ptr::null_mut());
        pa_context_disconnect(context);
        pa_context_unref(context);
        pa_threaded_mainloop_free(main_loop);
        drop(Box::from_raw(context_state));
    }

    fn context_state(&self) -> u32 {
        unsafe { (*self.context_state).state.load(Ordering::Acquire) }
    }

    /// Whether the server connection is alive.
    ///
    /// Once it is not, every `pa_context_*` call returns a null `pa_operation`,
    /// and passing one of those to `pa_operation_get_state`/`pa_operation_unref`
    /// aborts the process inside libpulse.
    fn is_context_ready(&self) -> bool {
        self.context_state() == CONTEXT_READY
    }

    unsafe extern "C" fn subscribe_callback(
        _c: *mut pa_context,
        _event_bits: pa_subscription_event_type_t,
        _index: u32,
        state_ptr: *mut c_void,
    ) {
        let state = &*(state_ptr as *const PulseContextState);
        state.change_signal.set();
        pa_threaded_mainloop_signal(state.main_loop, 0);
    }

    unsafe extern "C" fn context_state_callback(c: *mut pa_context, state_ptr: *mut c_void) {
        let context_state = &*(state_ptr as *const PulseContextState);

        match pa_context_get_state(c) {
            PA_CONTEXT_READY => context_state.state.store(CONTEXT_READY, Ordering::Release),
            PA_CONTEXT_FAILED | PA_CONTEXT_TERMINATED => {
                context_state.state.store(CONTEXT_FAILED, Ordering::Release)
            }
            _ => (),
        }
        // always signal: whoever is waiting has to re-check both the context
        // state and whatever else it was waiting for
        pa_threaded_mainloop_signal(context_state.main_loop, 0);
    }

    unsafe extern "C" fn sink_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_sink_info,
        eol: c_int,
        query_ptr: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query_ptr as *mut _);
        // eol > 0 is the end of the list, eol < 0 means the query failed;
        // in both cases `info` is not ours to read
        if eol != 0 || info.is_null() {
            query.signal();
            return;
        }
        if let Some(name) = cstr_to_string((*info).name) {
            let description = cstr_to_string((*info).description).unwrap_or_else(|| name.clone());
            query.sink_list.push(PulseDeviceDesc {
                name,
                description,
                _index: (*info).index,
            })
        }
    }

    unsafe extern "C" fn source_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_source_info,
        eol: c_int,
        query_ptr: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query_ptr as *mut _);
        if eol != 0 || info.is_null() {
            query.signal();
            return;
        }
        if let Some(name) = cstr_to_string((*info).name) {
            let description = cstr_to_string((*info).description).unwrap_or_else(|| name.clone());
            query.source_list.push(PulseDeviceDesc {
                name,
                description,
                _index: (*info).index,
            })
        }
    }

    unsafe extern "C" fn server_info_callback(
        _ctx: *mut pa_context,
        info: *const pa_server_info,
        query_ptr: *mut c_void,
    ) {
        let query: &mut PulseDeviceQuery = &mut *(query_ptr as *mut _);
        // both the info and the default device names are null when the query
        // failed or when the server has no default sink/source at all
        if !info.is_null() {
            query.default_sink = cstr_to_string((*info).default_sink_name);
            query.default_source = cstr_to_string((*info).default_source_name);
        }
        query.signal();
    }

    pub fn get_updated_descs(&mut self) -> Vec<AudioDeviceDesc> {
        // ok lets enumerate pulse audio
        unsafe {
            if !self.is_context_ready() {
                self.device_descs.clear();
                return Vec::new();
            }
            let mut query = PulseDeviceQuery::default();
            query.main_loop = Some(self.main_loop);
            pa_threaded_mainloop_lock(self.main_loop);
            let ops = [
                pa_context_get_sink_info_list(
                    self.context,
                    Some(Self::sink_info_callback),
                    &mut query as *mut _ as *mut _,
                ),
                pa_context_get_source_info_list(
                    self.context,
                    Some(Self::source_info_callback),
                    &mut query as *mut _ as *mut _,
                ),
                pa_context_get_server_info(
                    self.context,
                    Some(Self::server_info_callback),
                    &mut query as *mut _ as *mut _,
                ),
            ];
            // a null operation means the context died between the check above
            // and the call - libpulse asserts on those, so skip them entirely
            let timeout = PulseTimeout::new(self.main_loop_api, self.main_loop, PULSE_TIMEOUT_MS);
            loop {
                let running = ops
                    .iter()
                    .any(|op| !op.is_null() && pa_operation_get_state(*op) == PA_OPERATION_RUNNING);
                if !running || !self.is_context_ready() {
                    break;
                }
                if timeout.expired() {
                    crate::error!("PulseAudio: timed out enumerating devices");
                    break;
                }
                pa_threaded_mainloop_wait(self.main_loop);
            }
            timeout.free();
            for op in ops {
                if op.is_null() {
                    continue;
                }
                // cancelling detaches the callback, which points at `query` on
                // our stack - an unreffed but still running operation would call
                // it after we return
                if pa_operation_get_state(op) == PA_OPERATION_RUNNING {
                    pa_operation_cancel(op);
                }
                pa_operation_unref(op);
            }
            pa_threaded_mainloop_unlock(self.main_loop);
            // lets add some input/output devices
            let mut out = Vec::new();
            let mut device_descs = Vec::new();
            for source in query.source_list {
                let device_id = LiveId::from_str(&source.name).into();
                out.push(AudioDeviceDesc {
                    has_failed: self.failed_devices.contains(&device_id),
                    device_id,
                    device_type: AudioDeviceType::Input,
                    is_default: Some(&source.name) == query.default_source.as_ref(),
                    channel_count: 2,
                    name: format!("[Pulse Audio] {}", source.description),
                });
                device_descs.push(PulseAudioDesc {
                    name: source.name.clone(),
                    desc: out.last().cloned().unwrap(),
                });
            }
            for sink in query.sink_list {
                let device_id = LiveId::from_str(&sink.name).into();
                out.push(AudioDeviceDesc {
                    has_failed: self.failed_devices.contains(&device_id),
                    device_id,
                    device_type: AudioDeviceType::Output,
                    is_default: Some(&sink.name) == query.default_sink.as_ref(),
                    channel_count: 2,
                    name: format!("[Pulse Audio] {}", sink.description),
                });
                device_descs.push(PulseAudioDesc {
                    name: sink.name.clone(),
                    desc: out.last().cloned().unwrap(),
                });
            }
            // a device that failed to open is not tried again until something
            // changes - the server's device list changing is our hotplug event,
            // so re-arm every failure then. that is what lets a sink which was
            // merely busy, or a server that has just come up, work again.
            // compared as a set: a reordered enumeration is not a change, and
            // treating it as one would resurrect the retry loop.
            let device_set = |descs: &[PulseAudioDesc]| {
                descs
                    .iter()
                    .map(|v| (v.desc.device_id, v.desc.device_type))
                    .collect::<HashSet<_>>()
            };
            if device_set(&device_descs) != device_set(&self.device_descs) {
                self.failed_devices.clear();
                for desc in out.iter_mut() {
                    desc.has_failed = false;
                }
                for v in device_descs.iter_mut() {
                    v.desc.has_failed = false;
                }
            }
            self.device_descs = device_descs;
            out
        }
    }

    pub fn use_audio_inputs(&mut self, devices: &[AudioDeviceId]) {
        self.reap_zombie_streams();
        self.reap_failed_inputs();
        self.rearm_on_explicit_request(devices, true);
        let new = {
            let mut i = 0;
            while i < self.audio_inputs.len() {
                if !devices.contains(&self.audio_inputs[i].device_id) {
                    let item = self.audio_inputs.remove(i);
                    unsafe { item.terminate(self) };
                } else {
                    i += 1;
                }
            }
            if !self.is_context_ready() {
                return;
            }
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                // a device that already refused to open is reported back with
                // has_failed, retrying it on every device change would stall the
                // UI thread over and over
                if self.failed_devices.contains(device_id) {
                    continue;
                }
                if self
                    .audio_inputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                {
                    if let Some(v) = self
                        .device_descs
                        .iter()
                        .find(|v| v.desc.device_id == *device_id)
                    {
                        new.push((index, *device_id, &v.name))
                    }
                }
            }
            new
        };
        for (index, device_id, name) in new {
            if let Some(new_input) = unsafe { PulseInputStream::new(device_id, name, index, self) } {
                self.audio_inputs.push(new_input);
            } else {
                self.failed_devices.insert(device_id);
                self.change_signal.set();
            }
        }
    }

    /// Retries the disconnect of streams that were still being created when we
    /// gave up on them, once the server has answered. See [`destroy_stream`].
    fn reap_zombie_streams(&self) {
        let mut zombies = self.zombie_streams.lock().unwrap();
        if zombies.is_empty() {
            return;
        }
        unsafe {
            pa_threaded_mainloop_lock(self.main_loop);
            zombies.retain(|stream| {
                // still no answer from the server: keep waiting, unless the
                // server is gone, in which case it never will answer
                if pa_stream_get_state(*stream) == PA_STREAM_CREATING && self.is_context_ready() {
                    return true;
                }
                pa_stream_disconnect(*stream);
                pa_stream_unref(*stream);
                false
            });
            pa_threaded_mainloop_unlock(self.main_loop);
        }
    }

    /// Gives the requested devices another chance when the app asks for a
    /// different set than last time.
    ///
    /// The retry loop this guards against is an app re-requesting the *same*
    /// devices on every device change. A changed request is an explicit choice -
    /// a user picking a device, or a widget toggling its microphone back on -
    /// and silently ignoring it because the device failed once would leave that
    /// microphone dead for the rest of the process.
    fn rearm_on_explicit_request(&mut self, devices: &[AudioDeviceId], is_input: bool) {
        let last = if is_input {
            &mut self.last_input_request
        } else {
            &mut self.last_output_request
        };
        if last.as_slice() == devices {
            return;
        }
        *last = devices.to_vec();
        for device_id in devices {
            self.failed_devices.remove(device_id);
        }
    }

    /// Drops streams the server has torn down since we opened them.
    ///
    /// Without this a dead stream sits in the list forever: it is never
    /// recreated because it looks like it is still in use, and the app is never
    /// told, so audio just stops. Marking the device failed lets the app fall
    /// over to another one, and the device is re-armed when the device list
    /// next changes.
    fn reap_failed_streams(&mut self) {
        let mut i = 0;
        while i < self.audio_outputs.len() {
            if self.audio_outputs[i].has_failed() {
                let item = self.audio_outputs.remove(i);
                crate::error!("PulseAudio: output device stopped");
                item.terminate(self);
                self.failed_devices.insert(item.device_id);
                self.change_signal.set();
            } else {
                i += 1;
            }
        }
    }

    fn reap_failed_inputs(&mut self) {
        let mut i = 0;
        while i < self.audio_inputs.len() {
            if self.audio_inputs[i].has_failed() {
                let item = self.audio_inputs.remove(i);
                crate::error!("PulseAudio: input device stopped");
                let device_id = item.device_id;
                unsafe { item.terminate(self) };
                self.failed_devices.insert(device_id);
                self.change_signal.set();
            } else {
                i += 1;
            }
        }
    }

    pub fn use_audio_outputs(&mut self, devices: &[AudioDeviceId]) {
        self.reap_zombie_streams();
        self.reap_failed_streams();
        self.rearm_on_explicit_request(devices, false);
        let new = {
            let mut i = 0;
            while i < self.audio_outputs.len() {
                if !devices.contains(&self.audio_outputs[i].device_id) {
                    let item = self.audio_outputs.remove(i);
                    item.terminate(self);
                } else {
                    i += 1;
                }
            }
            if !self.is_context_ready() {
                return;
            }
            // create the new ones
            let mut new = Vec::new();
            for (index, device_id) in devices.iter().enumerate() {
                // a device that already refused to open is reported back with
                // has_failed, retrying it on every device change would stall the
                // UI thread over and over
                if self.failed_devices.contains(device_id) {
                    continue;
                }
                if self
                    .audio_outputs
                    .iter()
                    .find(|v| v.device_id == *device_id)
                    .is_none()
                {
                    if let Some(v) = self
                        .device_descs
                        .iter()
                        .find(|v| v.desc.device_id == *device_id)
                    {
                        new.push((index, *device_id, &v.name))
                    }
                }
            }
            new
        };
        for (index, device_id, name) in new {
            if let Some(new_output) =
                unsafe { PulseOutputStream::new(device_id, name, index, self) }
            {
                self.audio_outputs.push(new_output);
            } else {
                self.failed_devices.insert(device_id);
                self.change_signal.set();
            }
        }
    }
}
