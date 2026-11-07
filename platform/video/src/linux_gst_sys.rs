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
//! push / appsink pull, buffer map/fill, caps-to-string, the bus and a
//! decoded picture's plane layout (libgstvideo's meta and caps info).
//!
//! The structures below use the public, ABI-stable GStreamer 1.x layout
//! (`GstBuffer`'s pts field and `GstMessage`'s type field). The audio decoder's
//! timed preroll binding has been source-reviewed only; Linux build and
//! runtime verification are pending.

#![allow(non_camel_case_types)]

use std::ffi::{c_char, c_int, c_uint, c_void, CString};
use std::sync::{Mutex, OnceLock};

pub type GstElement = c_void;
pub type GstBus = c_void;
pub type GstSample = c_void;
pub type GstBuffer = c_void;
pub type GstCaps = c_void;
pub type GstMessage = c_void;
pub type GstContext = c_void;

/// `GList` (public struct): a doubly linked list node.
#[repr(C)]
pub struct GList {
    pub data: *mut c_void,
    pub next: *mut GList,
    pub prev: *mut GList,
}

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
pub const GST_STATE_CHANGE_ASYNC: c_int = 2;
// GstFormat
pub const GST_FORMAT_TIME: c_int = 3;
// GstSeekFlags
pub const GST_SEEK_FLAG_FLUSH: c_uint = 1 << 0;
pub const GST_SEEK_FLAG_KEY_UNIT: c_uint = 1 << 2;
pub const GST_SEEK_FLAG_SNAP_BEFORE: c_uint = 1 << 5;
// GstMessageType (bitmask)
pub const GST_MESSAGE_EOS: c_uint = 1 << 0;
pub const GST_MESSAGE_ERROR: c_uint = 1 << 1;
pub const GST_MESSAGE_WARNING: c_uint = 1 << 2;
pub const GST_MESSAGE_ANY: c_uint = !0;
// GstMapFlags
pub const GST_MAP_READ: c_uint = 1 << 0;
pub const GST_MAP_WRITE: c_uint = 1 << 1;
// GstFlowReturn
pub const GST_FLOW_OK: c_int = 0;
// GstClockTime
pub const GST_CLOCK_TIME_NONE: u64 = u64::MAX;

// GstElementFactoryListType bits, GstRank, GstPadDirection
pub const GST_ELEMENT_FACTORY_TYPE_DECODER: u64 = 1 << 0;
pub const GST_RANK_MARGINAL: c_uint = 64;
pub const GST_PAD_SINK: c_uint = 2;

pub const GST_ITERATOR_OK: c_int = 1;
pub const GST_ITERATOR_RESYNC: c_int = 2;

/// `GValue` (public struct, filled by `gst_iterator_next`): its type and two
/// words of data; all zero is the unset value.
#[repr(C)]
#[derive(Default)]
pub struct GValue {
    pub g_type: usize,
    pub data: [u64; 2],
}

/// `G_TYPE_UINT`: fundamental type 7, shifted by `G_TYPE_FUNDAMENTAL_SHIFT`.
pub const G_TYPE_UINT: usize = 7 << 2;

/// The head of `GParamSpec` (public struct): the class pointer every instance
/// starts with, the property's name and its flags (an int, padded to a word),
/// then the type of its value.
#[repr(C)]
struct GParamSpecHead {
    _class_name_flags: [usize; 3],
    value_type: usize,
}

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

/// `GstVideoMeta` head — PUBLIC ABI (video-meta.h, stable across 1.x): the
/// plane offsets and strides of a buffer laid out other than its caps'
/// default, read through `gst_buffer_get_meta`. Only the fields read are declared.
#[repr(C)]
pub struct GstVideoMetaRepr {
    pub meta_flags: c_uint,
    pub meta_info: *const c_void,
    pub buffer: *mut GstBuffer,
    pub flags: c_uint,
    pub format: c_int,
    pub id: c_int,
    pub width: c_uint,
    pub height: c_uint,
    pub n_planes: c_uint,
    pub offset: [usize; 4],
    pub stride: [c_int; 4],
}

/// `GstVideoInfo` head — PUBLIC ABI (video-info.h): a caps' default plane
/// layout. Allocated by `gst_video_info_new`, so only the fields up to the
/// strides are declared.
#[repr(C)]
pub struct GstVideoInfoRepr {
    pub finfo: *const c_void,
    pub interlace_mode: c_int,
    pub flags: c_uint,
    pub width: c_int,
    pub height: c_int,
    pub size: usize,
    pub views: c_int,
    pub chroma_site: c_uint,
    pub colorimetry: [c_int; 4],
    pub par_n: c_int,
    pub par_d: c_int,
    pub fps_n: c_int,
    pub fps_d: c_int,
    pub offset: [usize; 4],
    pub stride: [c_int; 4],
}

/// Where a two-plane (NV12) picture's planes sit in its buffer: byte offset
/// and row stride of the luma plane and of the interleaved chroma plane.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Nv12Layout {
    pub luma: (usize, usize),
    pub chroma: (usize, usize),
}

impl Nv12Layout {
    /// The first two planes of a GStreamer offset/stride table; None for a
    /// negative (bottom-up) stride.
    fn of_planes(offset: &[usize; 4], stride: &[c_int; 4]) -> Option<Nv12Layout> {
        let plane = |i: usize| Some((offset[i], usize::try_from(stride[i]).ok()?));
        Some(Nv12Layout { luma: plane(0)?, chroma: plane(1)? })
    }

    /// The bytes a `height`-row picture in this layout reaches.
    pub fn size(&self, height: u32) -> usize {
        (self.luma.0 + self.luma.1 * height as usize).max(self.chroma.0 + self.chroma.1 * (height as usize).div_ceil(2))
    }
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
    gstvideo: *mut c_void,
    glib: *mut c_void,
    gobject: *mut c_void,
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
                        gstvideo: open_lib("libgstvideo-1.0.so.0")?,
                        glib: open_lib("libglib-2.0.so.0")?,
                        gobject: open_lib("libgobject-2.0.so.0")?,
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
    gst.gst_element_set_locked_state: fn(*mut GstElement, c_int) -> c_int;
    gst.gst_element_get_state: fn(*mut GstElement, *mut c_uint, *mut c_uint, u64) -> c_int;
    gst.gst_element_get_static_pad: fn(*mut GstElement, *const c_char) -> *mut c_void;
    gst.gst_pad_is_linked: fn(*mut c_void) -> c_int;
    gst.gst_element_get_bus: fn(*mut GstElement) -> *mut GstBus;
    gst.gst_bus_timed_pop_filtered: fn(*mut GstBus, u64, c_uint) -> *mut GstMessage;
    gst.gst_message_parse_error: fn(*mut GstMessage, *mut *mut GError, *mut *mut c_char);
    gst.gst_message_parse_warning: fn(*mut GstMessage, *mut *mut GError, *mut *mut c_char);
    gst.gst_mini_object_unref: fn(*mut c_void);
    gst.gst_object_unref: fn(*mut c_void);
    gst.gst_buffer_new_allocate: fn(*mut c_void, usize, *mut c_void) -> *mut GstBuffer;
    gst.gst_buffer_fill: fn(*mut GstBuffer, usize, *const c_void, usize) -> usize;
    gst.gst_buffer_map: fn(*mut GstBuffer, *mut GstMapInfo, c_uint) -> c_int;
    gst.gst_buffer_unmap: fn(*mut GstBuffer, *mut GstMapInfo);
    gst.gst_buffer_get_meta: fn(*mut GstBuffer, usize) -> *mut c_void;
    gst.gst_element_set_context: fn(*mut GstElement, *mut GstContext);
    gst.gst_element_get_contexts: fn(*mut GstElement) -> *mut GList;
    gst.gst_context_get_context_type: fn(*const GstContext) -> *const c_char;
    gst.gst_sample_get_buffer: fn(*mut GstSample) -> *mut GstBuffer;
    gst.gst_sample_get_caps: fn(*mut GstSample) -> *mut GstCaps;
    gst.gst_sample_get_segment: fn(*mut GstSample) -> *const c_void;
    gst.gst_segment_to_stream_time: fn(*const c_void, c_int, u64) -> u64;
    gst.gst_caps_to_string: fn(*mut GstCaps) -> *mut c_char;
    gst.gst_caps_from_string: fn(*const c_char) -> *mut GstCaps;
    gst.gst_element_factory_list_get_elements: fn(u64, c_uint) -> *mut GList;
    gst.gst_element_factory_list_filter: fn(*mut GList, *const GstCaps, c_uint, c_int) -> *mut GList;
    gst.gst_plugin_feature_list_free: fn(*mut GList);
    gst.gst_plugin_feature_get_rank: fn(*mut c_void) -> c_uint;
    gst.gst_object_get_name: fn(*mut c_void) -> *mut c_char;
    gst.gst_element_query_duration: fn(*mut GstElement, c_int, *mut i64) -> c_int;
    gst.gst_element_seek_simple: fn(*mut GstElement, c_int, c_uint, i64) -> c_int;
    gst.gst_filename_to_uri: fn(*const c_char, *mut *mut GError) -> *mut c_char;
    gst.gst_bin_iterate_recurse: fn(*mut GstElement) -> *mut c_void;
    gst.gst_iterator_next: fn(*mut c_void, *mut GValue) -> c_int;
    gst.gst_iterator_resync: fn(*mut c_void);
    gst.gst_iterator_free: fn(*mut c_void);
    gst.gst_element_get_factory: fn(*mut GstElement) -> *mut c_void;
    gst.gst_element_factory_get_metadata: fn(*mut c_void, *const c_char) -> *const c_char;
    gst.gst_pad_get_current_caps: fn(*mut c_void) -> *mut GstCaps;
    gstapp.gst_app_src_push_buffer: fn(*mut GstElement, *mut GstBuffer) -> c_int;
    gstapp.gst_app_src_end_of_stream: fn(*mut GstElement) -> c_int;
    gstapp.gst_app_sink_try_pull_preroll: fn(*mut GstElement, u64) -> *mut GstSample;
    gstapp.gst_app_sink_try_pull_sample: fn(*mut GstElement, u64) -> *mut GstSample;
    gstapp.gst_app_sink_is_eos: fn(*mut GstElement) -> c_int;
    gstvideo.gst_video_meta_api_get_type: fn() -> usize;
    gstvideo.gst_video_info_new: fn() -> *mut GstVideoInfoRepr;
    gstvideo.gst_video_info_free: fn(*mut GstVideoInfoRepr);
    gstvideo.gst_video_info_from_caps: fn(*mut GstVideoInfoRepr, *const GstCaps) -> c_int;
    glib.g_free: fn(*mut c_void);
    glib.g_list_free: fn(*mut GList);
    glib.g_error_free: fn(*mut GError);
    gobject.g_value_get_object: fn(*const GValue) -> *mut c_void;
    gobject.g_value_unset: fn(*mut GValue);
    gobject.g_value_init: fn(*mut GValue, usize) -> *mut GValue;
    gobject.g_value_set_uint: fn(*mut GValue, c_uint);
    gobject.g_value_get_uint: fn(*const GValue) -> c_uint;
    gobject.g_object_class_find_property: fn(*mut c_void, *const c_char) -> *mut c_void;
    gobject.g_param_value_validate: fn(*mut c_void, *mut GValue) -> c_int;
    gobject.g_object_set_property: fn(*mut c_void, *const c_char, *const GValue);
}

static LIB: OnceLock<Option<LibGst>> = OnceLock::new();

/// The device contexts (a CUDA context, a VA display, a GL display: one of
/// each type) this process's pipelines share, each held by one reference
/// for the process lifetime. A hardware decoder that finds none makes its own
/// and tears it down when its pipeline stops: on a desktop GPU a single-frame
/// file cost 145-190 ms to open and close that way, one file at a time across
/// threads (the driver serialises context setup), against under 1 ms of
/// decoding; with the context shared it costs 31-50 ms.
static SHARED_CONTEXTS: Mutex<Vec<(String, usize)>> = Mutex::new(Vec::new());

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

    /// Sets `element`'s unsigned integer property `name` to `value`, brought
    /// into the range the element declares for it: GObject refuses a value
    /// outside that range, and the property quietly keeps its default (an
    /// encoder asked for more bitrate than it takes would run at its
    /// default). Returns the value set; None when the element has no
    /// unsigned integer property of that name.
    pub unsafe fn set_uint_in_range(&self, element: *mut GstElement, name: &str, value: u32) -> Option<u32> {
        let name = CString::new(name).ok()?;
        // An instance starts with its class (G_OBJECT_GET_CLASS).
        let class = *(element as *mut *mut c_void);
        let pspec = (self.g_object_class_find_property)(class, name.as_ptr());
        if pspec.is_null() || (*(pspec as *const GParamSpecHead)).value_type != G_TYPE_UINT {
            return None;
        }
        let mut v = GValue::default();
        (self.g_value_init)(&mut v, G_TYPE_UINT);
        (self.g_value_set_uint)(&mut v, value);
        (self.g_param_value_validate)(pspec, &mut v);
        let set = (self.g_value_get_uint)(&v);
        (self.g_object_set_property)(element, name.as_ptr(), &v);
        (self.g_value_unset)(&mut v);
        Some(set)
    }

    /// The plane layout of a two-plane picture `buffer` of `caps`: its video
    /// meta's offsets and strides when it carries one (a decoder's own padded
    /// pool), else the default layout of the caps (rows padded to 4 bytes).
    /// None for a buffer that is neither, or a negative (flipped) stride.
    pub unsafe fn nv12_layout(&self, buffer: *mut GstBuffer, caps: *mut GstCaps) -> Option<Nv12Layout> {
        let meta = (self.gst_buffer_get_meta)(buffer, (self.gst_video_meta_api_get_type)()) as *const GstVideoMetaRepr;
        if !meta.is_null() {
            let meta = &*meta;
            if meta.n_planes < 2 {
                return None;
            }
            return Nv12Layout::of_planes(&meta.offset, &meta.stride);
        }
        self.caps_nv12_layout(caps)
    }

    /// The default plane layout of two-plane picture `caps` (GstVideoInfo's).
    pub unsafe fn caps_nv12_layout(&self, caps: *mut GstCaps) -> Option<Nv12Layout> {
        if caps.is_null() {
            return None;
        }
        let info = (self.gst_video_info_new)();
        if info.is_null() {
            return None;
        }
        let layout = if (self.gst_video_info_from_caps)(info, caps) != 0 { Nv12Layout::of_planes(&(*info).offset, &(*info).stride) } else { None };
        (self.gst_video_info_free)(info);
        layout
    }

    /// The video decoder `decodebin` would plug for a stream of `caps` (a caps
    /// string): the highest ranked one whose input takes them (a hardware
    /// decoder's size range included), the name breaking a tie. Its name, and
    /// whether its class calls it hardware.
    pub fn best_decoder(&self, caps: &str) -> Option<(String, bool)> {
        let caps = CString::new(caps).ok()?;
        unsafe {
            let caps = (self.gst_caps_from_string)(caps.as_ptr());
            if caps.is_null() {
                return None;
            }
            let all = (self.gst_element_factory_list_get_elements)(GST_ELEMENT_FACTORY_TYPE_DECODER, GST_RANK_MARGINAL);
            let fit = (self.gst_element_factory_list_filter)(all, caps, GST_PAD_SINK, 0);
            let mut best: Option<(c_uint, String, *mut c_void)> = None;
            let mut node = fit;
            while !node.is_null() {
                let factory = (*node).data;
                let rank = (self.gst_plugin_feature_get_rank)(factory);
                let name_c = (self.gst_object_get_name)(factory);
                if !name_c.is_null() {
                    let name = std::ffi::CStr::from_ptr(name_c).to_string_lossy().into_owned();
                    (self.g_free)(name_c as *mut c_void);
                    if best.as_ref().map_or(true, |(r, n, _)| rank > *r || (rank == *r && name < *n)) {
                        best = Some((rank, name, factory));
                    }
                }
                node = (*node).next;
            }
            let best = best.map(|(_, name, factory)| {
                let klass = (self.gst_element_factory_get_metadata)(factory, c"klass".as_ptr());
                let hardware = !klass.is_null() && std::ffi::CStr::from_ptr(klass).to_string_lossy().split('/').any(|c| c == "Hardware");
                (name, hardware)
            });
            (self.gst_plugin_feature_list_free)(fit);
            (self.gst_plugin_feature_list_free)(all);
            (self.gst_mini_object_unref)(caps);
            best
        }
    }

    /// Hands `pipeline` the device contexts earlier pipelines made, before it
    /// starts: its elements take them instead of making their own.
    pub unsafe fn give_shared_contexts(&self, pipeline: *mut GstElement) {
        let shared = SHARED_CONTEXTS.lock().unwrap_or_else(|e| e.into_inner());
        for (_, context) in shared.iter() {
            (self.gst_element_set_context)(pipeline, *context as *mut GstContext);
        }
    }

    /// Keeps the device contexts `pipeline`'s elements made (a running
    /// pipeline holds those its elements announced) for the pipelines after it:
    /// the first of each type is kept, the rest released.
    pub unsafe fn keep_shared_contexts(&self, pipeline: *mut GstElement) {
        let list = (self.gst_element_get_contexts)(pipeline);
        if list.is_null() {
            return;
        }
        let mut shared = SHARED_CONTEXTS.lock().unwrap_or_else(|e| e.into_inner());
        let mut node = list;
        while !node.is_null() {
            let context = (*node).data;
            let kind = (self.gst_context_get_context_type)(context);
            let kind = if kind.is_null() { String::new() } else { std::ffi::CStr::from_ptr(kind).to_string_lossy().into_owned() };
            if shared.iter().any(|(k, _)| *k == kind) {
                (self.gst_mini_object_unref)(context);
            } else {
                shared.push((kind, context as usize));
            }
            node = (*node).next;
        }
        (self.g_list_free)(list);
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

    /// Every message waiting on `bus`, taken off it (a long-lived pipeline's
    /// bus keeps each one until it is popped); the first ERROR's text, or
    /// with `warnings` the first ERROR's or WARNING's.
    pub fn bus_drain(&self, bus: *mut GstBus, warnings: bool) -> Option<String> {
        let mut trouble = None;
        unsafe {
            loop {
                let msg = (self.gst_bus_timed_pop_filtered)(bus, 0, GST_MESSAGE_ANY);
                if msg.is_null() {
                    return trouble;
                }
                let kind = (*(msg as *const GstMessageRepr)).mtype;
                if trouble.is_none() && (kind == GST_MESSAGE_ERROR || (warnings && kind == GST_MESSAGE_WARNING)) {
                    trouble = Some(self.take_error_text(msg));
                }
                (self.gst_mini_object_unref)(msg as *mut c_void);
            }
        }
    }

    /// Parse + free an ERROR (or WARNING) message's GError into readable text.
    pub fn take_error_text(&self, msg: *mut GstMessage) -> String {
        unsafe {
            let mut err: *mut GError = std::ptr::null_mut();
            let mut debug: *mut c_char = std::ptr::null_mut();
            if (*(msg as *const GstMessageRepr)).mtype == GST_MESSAGE_WARNING {
                (self.gst_message_parse_warning)(msg, &mut err, &mut debug);
            } else {
                (self.gst_message_parse_error)(msg, &mut err, &mut debug);
            }
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
