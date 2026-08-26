//! Minimal GStreamer FFI for the Linux file-codec arm, loaded at runtime
//! with `dlopen` — the same discipline as the UI platform's
//! `gstreamer_sys.rs` (no -dev packages, no link-time dependency; the
//! runtime libraries are what `tools/linux_deps.sh` already installs). If
//! the libraries are missing, `LibGst::get()` returns `None` and the
//! encoder/decoder report a clear error instead of failing to start the
//! process.
//!
//! Scope is deliberately tiny: `gst_parse_launch` builds the pipelines, so
//! the only calls needed are init, element lookup, state changes, appsrc
//! push / appsink pull, buffer map/fill, caps-to-string and the bus.
//!
//! WRITTEN-UNTESTED FLAG: this arm compiles against public, ABI-stable
//! GStreamer 1.x structures (`GstBuffer`'s pts field, `GstMessage`'s type
//! field — both public headers since 1.0) but has not yet run on a Linux
//! box; the fleet verification pass owns that.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::sync::OnceLock;

pub type GstElement = c_void;
pub type GstBus = c_void;
pub type GstSample = c_void;
pub type GstBuffer = c_void;
pub type GstCaps = c_void;
pub type GstMessage = c_void;

#[repr(C)]
pub struct GError {
    pub domain: u32,
    pub code: c_int,
    pub message: *const c_char,
}

// GstState
pub const GST_STATE_NULL: c_uint = 1;
pub const GST_STATE_PAUSED: c_uint = 3;
pub const GST_STATE_PLAYING: c_uint = 4;
pub const GST_STATE_CHANGE_FAILURE: c_int = 0;
// GstFormat
pub const GST_FORMAT_TIME: c_int = 3;
// GstSeekFlags
pub const GST_SEEK_FLAG_FLUSH: c_uint = 1 << 0;
pub const GST_SEEK_FLAG_KEY_UNIT: c_uint = 1 << 2;
pub const GST_SEEK_FLAG_SNAP_BEFORE: c_uint = 1 << 5;
// GstMessageType (bitmask)
pub const GST_MESSAGE_EOS: c_uint = 1 << 0;
pub const GST_MESSAGE_ERROR: c_uint = 1 << 1;
// GstMapFlags
pub const GST_MAP_READ: c_uint = 1 << 0;
// GstFlowReturn
pub const GST_FLOW_OK: c_int = 0;
// GstClockTime
pub const GST_CLOCK_TIME_NONE: u64 = u64::MAX;

/// `GstMapInfo` (public struct, passed by pointer).
#[repr(C)]
pub struct GstMapInfo {
    pub memory: *mut c_void,
    pub flags: c_uint,
    pub data: *mut u8,
    pub size: usize,
    pub maxsize: usize,
    _padding: [*mut c_void; 8],
}

impl Default for GstMapInfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

/// `GstMiniObject` — PUBLIC ABI (gstminiobject.h, stable across 1.x).
/// 64 bytes on LP64.
#[repr(C)]
pub struct GstMiniObjectRepr {
    pub gtype: usize,
    pub refcount: c_int,
    pub lockstate: c_int,
    pub flags: c_uint,
    pub copy_fn: *mut c_void,
    pub dispose: *mut c_void,
    pub free: *mut c_void,
    pub priv_uint: c_uint,
    pub priv_pointer: *mut c_void,
}

/// `GstBuffer` — PUBLIC ABI (gstbuffer.h): `GST_BUFFER_PTS()` in C is
/// exactly this field access.
#[repr(C)]
pub struct GstBufferRepr {
    pub mini: GstMiniObjectRepr,
    pub pool: *mut c_void,
    pub pts: u64,
    pub dts: u64,
    pub duration: u64,
    pub offset: u64,
    pub offset_end: u64,
}

/// `GstMessage` head — PUBLIC ABI (gstmessage.h): `GST_MESSAGE_TYPE()` is
/// this field access.
#[repr(C)]
pub struct GstMessageRepr {
    pub mini: GstMiniObjectRepr,
    pub mtype: c_uint,
    _pad: c_uint,
    pub timestamp: u64,
    pub src: *mut c_void,
    pub seqnum: u32,
}

// dlopen, straight from glibc — no libc crate in this tree.
extern "C" {
    fn dlopen(filename: *const c_char, flag: c_int) -> *mut c_void;
    fn dlsym(handle: *mut c_void, symbol: *const c_char) -> *mut c_void;
}
const RTLD_NOW: c_int = 2;
const RTLD_GLOBAL: c_int = 0x100;

/// The dlopen handles, held only during symbol lookup (the libraries stay
/// mapped for the process lifetime — never dlclosed).
struct Libs {
    gst: *mut c_void,
    gstapp: *mut c_void,
    glib: *mut c_void,
}

macro_rules! gst_fns {
    ($( $lib:ident . $name:ident : fn( $($arg:ty),* ) $(-> $ret:ty)? ; )*) => {
        pub struct LibGst {
            $( pub $name: unsafe extern "C" fn( $($arg),* ) $(-> $ret)?, )*
        }
        impl LibGst {
            fn load() -> Option<LibGst> {
                unsafe {
                    let libs = Libs {
                        gst: open_lib("libgstreamer-1.0.so.0")?,
                        gstapp: open_lib("libgstapp-1.0.so.0")?,
                        glib: open_lib("libglib-2.0.so.0")?,
                    };
                    Some(LibGst {
                        $( $name: {
                            let sym = CString::new(stringify!($name)).ok()?;
                            let ptr = dlsym(libs.$lib, sym.as_ptr());
                            if ptr.is_null() { return None }
                            std::mem::transmute::<*mut c_void, unsafe extern "C" fn( $($arg),* ) $(-> $ret)?>(ptr)
                        }, )*
                    })
                }
            }
        }
    };
}

unsafe fn open_lib(name: &str) -> Option<*mut c_void> {
    let c = CString::new(name).ok()?;
    let handle = dlopen(c.as_ptr(), RTLD_NOW | RTLD_GLOBAL);
    if handle.is_null() {
        None
    } else {
        Some(handle)
    }
}

gst_fns! {
    gst.gst_init: fn(*mut c_int, *mut *mut *mut c_char);
    gst.gst_parse_launch: fn(*const c_char, *mut *mut GError) -> *mut GstElement;
    gst.gst_bin_get_by_name: fn(*mut GstElement, *const c_char) -> *mut GstElement;
    gst.gst_element_set_state: fn(*mut GstElement, c_uint) -> c_int;
    gst.gst_element_get_state: fn(*mut GstElement, *mut c_uint, *mut c_uint, u64) -> c_int;
    gst.gst_element_get_bus: fn(*mut GstElement) -> *mut GstBus;
    gst.gst_bus_timed_pop_filtered: fn(*mut GstBus, u64, c_uint) -> *mut GstMessage;
    gst.gst_message_parse_error: fn(*mut GstMessage, *mut *mut GError, *mut *mut c_char);
    gst.gst_mini_object_unref: fn(*mut c_void);
    gst.gst_object_unref: fn(*mut c_void);
    gst.gst_buffer_new_allocate: fn(*mut c_void, usize, *mut c_void) -> *mut GstBuffer;
    gst.gst_buffer_fill: fn(*mut GstBuffer, usize, *const c_void, usize) -> usize;
    gst.gst_buffer_map: fn(*mut GstBuffer, *mut GstMapInfo, c_uint) -> c_int;
    gst.gst_buffer_unmap: fn(*mut GstBuffer, *mut GstMapInfo);
    gst.gst_sample_get_buffer: fn(*mut GstSample) -> *mut GstBuffer;
    gst.gst_sample_get_caps: fn(*mut GstSample) -> *mut GstCaps;
    gst.gst_caps_to_string: fn(*mut GstCaps) -> *mut c_char;
    gst.gst_element_query_duration: fn(*mut GstElement, c_int, *mut i64) -> c_int;
    gst.gst_element_seek_simple: fn(*mut GstElement, c_int, c_uint, i64) -> c_int;
    gst.gst_filename_to_uri: fn(*const c_char, *mut *mut GError) -> *mut c_char;
    gstapp.gst_app_src_push_buffer: fn(*mut GstElement, *mut GstBuffer) -> c_int;
    gstapp.gst_app_src_end_of_stream: fn(*mut GstElement) -> c_int;
    gstapp.gst_app_sink_pull_sample: fn(*mut GstElement) -> *mut GstSample;
    gstapp.gst_app_sink_pull_preroll: fn(*mut GstElement) -> *mut GstSample;
    gstapp.gst_app_sink_is_eos: fn(*mut GstElement) -> c_int;
    glib.g_free: fn(*mut c_void);
    glib.g_error_free: fn(*mut GError);
}

static LIB: OnceLock<Option<LibGst>> = OnceLock::new();

impl LibGst {
    /// The process-wide loaded runtime, `None` when GStreamer is not
    /// installed. `gst_init` has run exactly once on success.
    pub fn get() -> Option<&'static LibGst> {
        LIB.get_or_init(|| {
            let lib = LibGst::load()?;
            unsafe { (lib.gst_init)(std::ptr::null_mut(), std::ptr::null_mut()) };
            Some(lib)
        })
        .as_ref()
    }

    /// Drain the bus without blocking; the first pending ERROR becomes a
    /// `String`.
    pub fn bus_error(&self, bus: *mut GstBus) -> Option<String> {
        unsafe {
            let msg = (self.gst_bus_timed_pop_filtered)(bus, 0, GST_MESSAGE_ERROR);
            if msg.is_null() {
                return None;
            }
            let text = self.take_error_text(msg);
            (self.gst_mini_object_unref)(msg as *mut c_void);
            Some(text)
        }
    }

    /// Parse + free an ERROR message's GError into readable text.
    pub fn take_error_text(&self, msg: *mut GstMessage) -> String {
        unsafe {
            let mut err: *mut GError = std::ptr::null_mut();
            let mut debug: *mut c_char = std::ptr::null_mut();
            (self.gst_message_parse_error)(msg, &mut err, &mut debug);
            let mut text = String::from("gstreamer error");
            if !err.is_null() {
                if !(*err).message.is_null() {
                    text = std::ffi::CStr::from_ptr((*err).message)
                        .to_string_lossy()
                        .into_owned();
                }
                (self.g_error_free)(err);
            }
            if !debug.is_null() {
                (self.g_free)(debug as *mut c_void);
            }
            text
        }
    }
}
