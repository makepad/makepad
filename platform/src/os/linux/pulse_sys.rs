#![allow(non_camel_case_types)]
#![allow(non_snake_case)]

use self::super::libc_sys::timeval;
use std::os::raw::{
    c_void,
    c_uint,
    c_char,
    c_int,
 };
 
pub type pa_io_event_flags = c_uint; 
pub use self::pa_io_event_flags as pa_io_event_flags_t;
pub type pa_context_flags = c_uint;
pub use self::pa_context_flags as pa_context_flags_t;
pub type pa_context_state = c_uint;
pub use self::pa_context_state as pa_context_state_t;
pub type pa_source_state = c_int;
pub use self::pa_source_state as pa_source_state_t;
pub type pa_sample_format = c_int;
pub use self::pa_sample_format as pa_sample_format_t;
pub type pa_channel_map_def = c_uint;
pub use self::pa_channel_map_def as pa_channel_map_def_t;
pub type pa_channel_position = c_int;
pub use self::pa_channel_position as pa_channel_position_t;
pub type pa_volume_t = u32;
pub type pa_usec_t = u64;
pub type pa_sink_flags = c_uint;
pub use self::pa_sink_flags as pa_sink_flags_t;
pub type pa_sink_state = c_int;
pub use self::pa_sink_state as pa_sink_state_t;
pub type pa_encoding = c_int;
pub use self::pa_encoding as pa_encoding_t;
pub type pa_source_flags = c_uint;
pub use self::pa_source_flags as pa_source_flags_t;
pub type pa_operation_state = c_uint;
pub use self::pa_operation_state as pa_operation_state_t;
pub type pa_subscription_event_type = c_uint;
pub use self::pa_subscription_event_type as pa_subscription_event_type_t;
pub type pa_stream_state = c_uint;
pub use self::pa_stream_state as pa_stream_state_t;
pub type pa_seek_mode = c_uint;
pub use self::pa_seek_mode as pa_seek_mode_t;
pub type pa_stream_flags = c_uint;
pub use self::pa_stream_flags as pa_stream_flags_t;

pub const PA_OPERATION_RUNNING: pa_operation_state = 0;
pub const PA_OPERATION_DONE: pa_operation_state = 1;
pub const PA_OPERATION_CANCELLED: pa_operation_state = 2;

pub const PA_CONTEXT_UNCONNECTED: pa_context_state = 0;
pub const PA_CONTEXT_CONNECTING: pa_context_state = 1;
pub const PA_CONTEXT_AUTHORIZING: pa_context_state = 2;
pub const PA_CONTEXT_SETTING_NAME: pa_context_state = 3;
pub const PA_CONTEXT_READY: pa_context_state = 4;
pub const PA_CONTEXT_FAILED: pa_context_state = 5;
pub const PA_CONTEXT_TERMINATED: pa_context_state = 6;

pub const PA_SAMPLE_FLOAT32LE: pa_sample_format = 5;

pub const PA_CHANNEL_POSITION_INVALID: pa_channel_position = -1;
pub const PA_CHANNEL_POSITION_LEFT: pa_channel_position = 1;
pub const PA_CHANNEL_POSITION_RIGHT: pa_channel_position = 2;

pub const PA_STREAM_UNCONNECTED: pa_stream_state = 0;
pub const PA_STREAM_CREATING: pa_stream_state = 1;
pub const PA_STREAM_READY: pa_stream_state = 2;
pub const PA_STREAM_FAILED: pa_stream_state = 3;
pub const PA_STREAM_TERMINATED: pa_stream_state = 4;

pub const PA_SEEK_RELATIVE: pa_seek_mode = 0;
pub const PA_SEEK_RELATIVE_ON_READ: pa_seek_mode = 2;

pub const PA_STREAM_START_CORKED: pa_stream_flags = 1;
pub const PA_STREAM_INTERPOLATE_TIMING: pa_stream_flags = 2;
pub const PA_STREAM_AUTO_TIMING_UPDATE: pa_stream_flags = 8;
pub const PA_STREAM_ADJUST_LATENCY: pa_stream_flags = 8192;
pub const PA_STREAM_START_UNMUTED: pa_stream_flags = 65536;

pub const PA_CHANNEL_MAP_DEFAULT: pa_channel_map_def = 0;


#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_time_event {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_defer_event {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_context {
    _unused: [u8; 0],
}

pub type pa_context_notify_cb_t = ::std::option::Option<
unsafe extern "C" fn(c: *mut pa_context, userdata: *mut c_void),
>;

pub type pa_defer_event_destroy_cb_t = ::std::option::Option<
unsafe extern "C" fn(
    a: *mut pa_mainloop_api,
    e: *mut pa_defer_event,
    userdata: *mut c_void,
),
>;

pub type pa_defer_event_cb_t = ::std::option::Option<
unsafe extern "C" fn(
    a: *mut pa_mainloop_api,
    e: *mut pa_defer_event,
    userdata: *mut c_void,
),
>;

pub type pa_time_event_destroy_cb_t = ::std::option::Option<
unsafe extern "C" fn(
    a: *mut pa_mainloop_api,
    e: *mut pa_time_event,
    userdata: *mut c_void,
),
>;

pub type pa_time_event_cb_t = ::std::option::Option<
unsafe extern "C" fn(
    a: *mut pa_mainloop_api,
    e: *mut pa_time_event,
    tv: *const timeval,
    userdata: *mut c_void,
),
>;

pub type pa_io_event_cb_t = ::std::option::Option<
unsafe extern "C" fn(
    ea: *mut pa_mainloop_api,
    e: *mut pa_io_event,
    fd: c_int,
    events: pa_io_event_flags_t,
    userdata: *mut c_void,
),
>;

pub type pa_io_event_destroy_cb_t = ::std::option::Option<
unsafe extern "C" fn(
    a: *mut pa_mainloop_api,
    e: *mut pa_io_event,
    userdata: *mut c_void,
),
>;

pub type pa_sink_info_cb_t = ::std::option::Option<
unsafe extern "C" fn(
    c: *mut pa_context,
    i: *const pa_sink_info,
    eol: c_int,
    userdata: *mut c_void,
),
>;

pub type pa_source_info_cb_t = ::std::option::Option<
    unsafe extern "C" fn(
        c: *mut pa_context,
        i: *const pa_source_info,
        eol: c_int,
        userdata: *mut c_void,
    ),
>;

pub type pa_context_subscribe_cb_t = ::std::option::Option<
    unsafe extern "C" fn(
        c: *mut pa_context,
        t: pa_subscription_event_type_t,
        idx: u32,
        userdata: *mut c_void,
    ),
>;

pub type pa_server_info_cb_t = ::std::option::Option<
    unsafe extern "C" fn(
        c: *mut pa_context,
        i: *const pa_server_info,
        userdata: *mut c_void,
    ),
>;

pub type pa_stream_notify_cb_t = ::std::option::Option<
    unsafe extern "C" fn(p: *mut pa_stream, userdata: *mut c_void),
>;

pub type pa_stream_success_cb_t = ::std::option::Option<
    unsafe extern "C" fn(
        s: *mut pa_stream,
        success: c_int,
        userdata: *mut c_void,
    ),
>;

pub type pa_stream_request_cb_t = ::std::option::Option<
    unsafe extern "C" fn(p: *mut pa_stream, nbytes: usize, userdata: *mut c_void),
>;

pub type pa_free_cb_t = ::std::option::Option<unsafe extern "C" fn(p: *mut ::std::os::raw::c_void)>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_cvolume {
    pub channels: u8,
    pub values: [pa_volume_t; 32usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_channel_map {
    pub channels: u8,
    pub map: [pa_channel_position_t; 32usize],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_sample_spec {
    pub format: pa_sample_format_t,
    pub rate: u32,
    pub channels: u8,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_proplist {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_format_info {
    pub encoding: pa_encoding_t,
    pub plist: *mut pa_proplist,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_sink_port_info {
    pub name: *const c_char,
    pub description: *const c_char,
    pub priority: u32,
    pub available: c_int,
    pub availability_group: *const c_char,
    pub type_: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
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
    pub flags: pa_sink_flags_t,
    pub proplist: *mut pa_proplist,
    pub configured_latency: pa_usec_t,
    pub base_volume: pa_volume_t,
    pub state: pa_sink_state_t,
    pub n_volume_steps: u32,
    pub card: u32,
    pub n_ports: u32,
    pub ports: *mut *mut pa_sink_port_info,
    pub active_port: *mut pa_sink_port_info,
    pub n_formats: u8,
    pub formats: *mut *mut pa_format_info,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_source_info {
    pub name: *const c_char,
    pub index: u32,
    pub description: *const c_char,
    pub sample_spec: pa_sample_spec,
    pub channel_map: pa_channel_map,
    pub owner_module: u32,
    pub volume: pa_cvolume,
    pub mute: c_int,
    pub monitor_of_sink: u32,
    pub monitor_of_sink_name: *const c_char,
    pub latency: pa_usec_t,
    pub driver: *const c_char,
    pub flags: pa_source_flags_t,
    pub proplist: *mut pa_proplist,
    pub configured_latency: pa_usec_t,
    pub base_volume: pa_volume_t,
    pub state: pa_source_state_t,
    pub n_volume_steps: u32,
    pub card: u32,
    pub n_ports: u32,
    pub ports: *mut *mut pa_source_port_info,
    pub active_port: *mut pa_source_port_info,
    pub n_formats: u8,
    pub formats: *mut *mut pa_format_info,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_source_port_info {
    pub name: *const c_char,
    pub description: *const c_char,
    pub priority: u32,
    pub available: c_int,
    pub availability_group: *const c_char,
    pub type_: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_io_event {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_threaded_mainloop {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_spawn_api {
    pub prefork: ::std::option::Option<unsafe extern "C" fn()>,
    pub postfork: ::std::option::Option<unsafe extern "C" fn()>,
    pub atfork: ::std::option::Option<unsafe extern "C" fn()>,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_operation {
    _unused: [u8; 0],
}



#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_server_info {
    pub user_name: *const ::std::os::raw::c_char,
    pub host_name: *const ::std::os::raw::c_char,
    pub server_version: *const ::std::os::raw::c_char,
    pub server_name: *const ::std::os::raw::c_char,
    pub sample_spec: pa_sample_spec,
    pub default_sink_name: *const ::std::os::raw::c_char,
    pub default_source_name: *const ::std::os::raw::c_char,
    pub cookie: u32,
    pub channel_map: pa_channel_map,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_stream {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_buffer_attr {
    pub maxlength: u32,
    pub tlength: u32,
    pub prebuf: u32,
    pub minreq: u32,
    pub fragsize: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct pa_mainloop_api {
    pub userdata: *mut c_void,
    pub io_new: ::std::option::Option<
    unsafe extern "C" fn(
        a: *mut pa_mainloop_api,
        fd: c_int,
        events: pa_io_event_flags_t,
        cb: pa_io_event_cb_t,
        userdata: *mut c_void,
    ) -> *mut pa_io_event,
    >,
    pub io_enable: ::std::option::Option<
    unsafe extern "C" fn(e: *mut pa_io_event, events: pa_io_event_flags_t),
    >,
    pub io_free: ::std::option::Option<unsafe extern "C" fn(e: *mut pa_io_event)>,
    pub io_set_destroy: ::std::option::Option<
    unsafe extern "C" fn(e: *mut pa_io_event, cb: pa_io_event_destroy_cb_t),
    >,
    pub time_new: ::std::option::Option<
    unsafe extern "C" fn(
        a: *mut pa_mainloop_api,
        tv: *const timeval,
        cb: pa_time_event_cb_t,
        userdata: *mut c_void,
    ) -> *mut pa_time_event,
    >,
    pub time_restart:
    ::std::option::Option<unsafe extern "C" fn(e: *mut pa_time_event, tv: *const timeval)>,
    pub time_free: ::std::option::Option<unsafe extern "C" fn(e: *mut pa_time_event)>,
    pub time_set_destroy: ::std::option::Option<
    unsafe extern "C" fn(e: *mut pa_time_event, cb: pa_time_event_destroy_cb_t),
    >,
    pub defer_new: ::std::option::Option<
    unsafe extern "C" fn(
        a: *mut pa_mainloop_api,
        cb: pa_defer_event_cb_t,
        userdata: *mut c_void,
    ) -> *mut pa_defer_event,
    >,
    pub defer_enable: ::std::option::Option<
    unsafe extern "C" fn(e: *mut pa_defer_event, b: c_int),
    >,
    pub defer_free: ::std::option::Option<unsafe extern "C" fn(e: *mut pa_defer_event)>,
    pub defer_set_destroy: ::std::option::Option<
    unsafe extern "C" fn(e: *mut pa_defer_event, cb: pa_defer_event_destroy_cb_t),
    >,
    pub quit: ::std::option::Option<
    unsafe extern "C" fn(a: *mut pa_mainloop_api, retval: c_int),
    >,
}


#[link(name = "pulse")]
extern "C" {
    pub fn pa_context_connect(
        c: *mut pa_context,
        server: *const c_char,
        flags: pa_context_flags_t,
        api: *const pa_spawn_api,
    ) -> c_int;

    pub fn pa_context_new(
        mainloop: *mut pa_mainloop_api,
        name: *const c_char,
    ) -> *mut pa_context;
    
    pub fn pa_context_set_state_callback(
        c: *mut pa_context,
        cb: pa_context_notify_cb_t,
        userdata: *mut c_void,
    );
    pub fn pa_context_get_state(c: *const pa_context) -> pa_context_state_t;

    pub fn pa_context_disconnect(c: *mut pa_context);
    
    pub fn pa_context_unref(c: *mut pa_context);
    
    pub fn pa_context_get_sink_info_list(
        c: *mut pa_context,
        cb: pa_sink_info_cb_t,
        userdata: *mut c_void,
    ) -> *mut pa_operation;
    
    pub fn pa_operation_get_state(o: *const pa_operation) -> pa_operation_state_t;
    pub fn pa_operation_unref(o: *mut pa_operation);
    
    pub fn pa_context_get_source_info_list(
        c: *mut pa_context,
        cb: pa_source_info_cb_t,
        userdata: *mut c_void,
    ) -> *mut pa_operation;

    pub fn pa_threaded_mainloop_get_api(m: *mut pa_threaded_mainloop) -> *mut pa_mainloop_api;

    pub fn pa_threaded_mainloop_new() -> *mut pa_threaded_mainloop;

    pub fn pa_threaded_mainloop_signal(
        m: *mut pa_threaded_mainloop,
        wait_for_accept:c_int,
    );

    pub fn pa_threaded_mainloop_start(m: *mut pa_threaded_mainloop) -> c_int;
    pub fn pa_threaded_mainloop_lock(m: *mut pa_threaded_mainloop);
    pub fn pa_threaded_mainloop_unlock(m: *mut pa_threaded_mainloop);
    pub fn pa_threaded_mainloop_wait(m: *mut pa_threaded_mainloop);
    pub fn pa_threaded_mainloop_stop(m: *mut pa_threaded_mainloop);
    pub fn pa_threaded_mainloop_free(m: *mut pa_threaded_mainloop);
    pub fn pa_context_get_server_info(
        c: *mut pa_context,
        cb: pa_server_info_cb_t,
        userdata: *mut c_void,
    ) -> *mut pa_operation;
    pub fn pa_proplist_new() -> *mut pa_proplist;
    pub fn pa_context_new_with_proplist(
        mainloop: *mut pa_mainloop_api,
        name: *const u8,
        proplist: *const pa_proplist,
    ) -> *mut pa_context;

    pub fn pa_stream_new(
        c: *mut pa_context,
        name: *const u8,
        ss: *const pa_sample_spec,
        map: *const pa_channel_map,
    ) -> *mut pa_stream;

    pub fn pa_stream_set_state_callback(
        s: *mut pa_stream,
        cb: pa_stream_notify_cb_t,
        userdata: *mut c_void,
    );

    pub fn pa_stream_get_state(p: *const pa_stream) -> pa_stream_state_t;

   pub fn pa_stream_cork(
        s: *mut pa_stream,
        b: c_int,
        cb: pa_stream_success_cb_t,
        userdata: *mut c_void,
    ) -> *mut pa_operation;
    
    pub fn pa_stream_set_write_callback(
        p: *mut pa_stream,
        cb: pa_stream_request_cb_t,
        userdata: *mut ::std::os::raw::c_void,
    );
    pub fn pa_stream_disconnect(s: *mut pa_stream) -> ::std::os::raw::c_int;
    pub fn pa_stream_unref(s: *mut pa_stream);
    pub fn pa_stream_begin_write(
        p: *mut pa_stream,
        data: *mut *mut c_void,
        nbytes: *mut usize,
    ) -> c_int;
    pub fn pa_stream_write(
        p: *mut pa_stream,
        data: *const c_void,
        nbytes: usize,
        free_cb: pa_free_cb_t,
        offset: i64,
        seek: pa_seek_mode_t,
    ) -> c_int;

    pub fn pa_stream_connect_playback(
        s: *mut pa_stream,
        dev: *const u8,
        attr: *const pa_buffer_attr,
        flags: pa_stream_flags_t,
        volume: *const pa_cvolume,
        sync_stream: *mut pa_stream,
    ) -> c_int;
    pub fn pa_stream_writable_size(p: *const pa_stream) -> usize;
    
    pub fn pa_stream_set_read_callback(
        p: *mut pa_stream,
        cb: pa_stream_request_cb_t,
        userdata: *mut c_void,
    );

    pub fn pa_stream_connect_record(
        s: *mut pa_stream,
        dev: *const u8,
        attr: *const pa_buffer_attr,
        flags: pa_stream_flags_t,
    ) -> c_int;
    
    pub fn pa_stream_peek(
        p: *mut pa_stream,
        data: *mut *const c_void,
        nbytes: *mut usize,
    ) -> c_int;

    pub fn pa_stream_drop(p: *mut pa_stream) -> c_int;

}
