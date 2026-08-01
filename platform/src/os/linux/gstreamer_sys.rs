//! GStreamer FFI bindings loaded dynamically via dlopen.
//!
//! If GStreamer is not installed, `LibGStreamer::try_load()` returns `None`
//! and video playback is gracefully unavailable.

use super::module_loader::ModuleLoader;
use std::ffi::c_void;
use std::os::raw::{c_char, c_int, c_uint, c_ulong};

// Opaque GStreamer types
pub type GstElement = c_void;
pub type GstBus = c_void;
pub type GstSample = c_void;
pub type GstBuffer = c_void;
pub type GstCaps = c_void;
pub type GstStructure = c_void;
pub type GstMessage = c_void;
pub type GObject = c_void;
pub type GstMiniObject = c_void;
pub type GstMemory = c_void;
pub type GstContext = c_void;
pub type GstEvent = c_void;
pub type GstStreamCollection = c_void;
pub type GstStream = c_void;
pub type GstTagList = c_void;
pub type GstRegistry = c_void;
pub type GstPluginFeature = c_void;
pub type GstGLDisplay = c_void;
pub type GstGLContext = c_void;
pub type GList = c_void;
pub type GstPad = c_void;
pub type GType = c_ulong;

// GLib GError — domain (GQuark/u32) + code (gint/i32) + message (*gchar)
#[repr(C)]
pub struct GError {
    pub domain: u32,
    pub code: c_int,
    pub message: *const c_char,
}

// GStreamer state enum values
pub const GST_STATE_NULL: c_uint = 1;
pub const GST_STATE_PAUSED: c_uint = 3;
pub const GST_STATE_PLAYING: c_uint = 4;

pub const GST_STATE_CHANGE_FAILURE: c_int = 0;
pub const GST_STATE_CHANGE_SUCCESS: c_int = 1;
pub const GST_STATE_CHANGE_ASYNC: c_int = 2;
pub const GST_STATE_CHANGE_NO_PREROLL: c_int = 3;

/// Wait until state change completes (`gst_element_get_state` timeout).
pub const GST_CLOCK_TIME_NONE: GstClockTime = u64::MAX;

// GstFormat
pub const GST_FORMAT_TIME: c_int = 3;

// GstSeekFlags
pub const GST_SEEK_FLAG_FLUSH: c_uint = 1 << 0;
pub const GST_SEEK_FLAG_ACCURATE: c_uint = 1 << 1;
pub const GST_SEEK_FLAG_KEY_UNIT: c_uint = 1 << 2;

// GstSeekType
pub const GST_SEEK_TYPE_NONE: c_int = 0;
pub const GST_SEEK_TYPE_SET: c_int = 1;

// GstMessageType (bitmask)
pub const GST_MESSAGE_EOS: c_uint = 1 << 0;
pub const GST_MESSAGE_ERROR: c_uint = 1 << 1;
pub const GST_MESSAGE_WARNING: c_uint = 1 << 2;
pub const GST_MESSAGE_BUFFERING: c_uint = 1 << 5;
pub const GST_MESSAGE_ELEMENT: c_uint = 1 << 15;
pub const GST_MESSAGE_NEED_CONTEXT: c_uint = 1 << 29;
pub const GST_MESSAGE_HAVE_CONTEXT: c_uint = 1 << 30;
/// GST_MESSAGE_EXTENDED | 4
pub const GST_MESSAGE_STREAM_COLLECTION: c_uint = 0x8000_0004;
/// GST_MESSAGE_EXTENDED | 5
pub const GST_MESSAGE_STREAMS_SELECTED: c_uint = 0x8000_0005;

// GstStreamType
pub const GST_STREAM_TYPE_AUDIO: c_uint = 1 << 1;
pub const GST_STREAM_TYPE_VIDEO: c_uint = 1 << 2;
pub const GST_STREAM_TYPE_TEXT: c_uint = 1 << 4;

// GstMapFlags
pub const GST_MAP_READ: c_uint = 1 << 0;

// GstMapInfo — sized struct we need to pass by pointer
#[repr(C)]
pub struct GstMapInfo {
    pub memory: *mut c_void,
    pub flags: c_uint,
    pub data: *mut u8,
    pub size: usize,
    pub maxsize: usize,
    // user_data[4] + _gst_reserved[4] from the real GstMapInfo
    _padding: [*mut c_void; 8],
}

impl Default for GstMapInfo {
    fn default() -> Self {
        unsafe { std::mem::zeroed() }
    }
}

// GstClockTime
pub type GstClockTime = u64;

pub struct LibGStreamer {
    // Keep module loaders alive so the .so files stay loaded
    _gst: ModuleLoader,
    _gstapp: ModuleLoader,
    _gobject: ModuleLoader,
    _glib: ModuleLoader,
    _gstgl: Option<ModuleLoader>,
    _gstallocators: Option<ModuleLoader>,
    _gstvideo: Option<ModuleLoader>,

    // libgstreamer-1.0.so.0
    pub gst_init: unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char),
    pub gst_element_factory_make:
        unsafe extern "C" fn(*const c_char, *const c_char) -> *mut GstElement,
    pub gst_element_set_state: unsafe extern "C" fn(*mut GstElement, c_uint) -> c_int,
    pub gst_element_get_state:
        unsafe extern "C" fn(*mut GstElement, *mut c_uint, *mut c_uint, GstClockTime) -> c_int,
    pub gst_element_query_position: unsafe extern "C" fn(*mut GstElement, c_int, *mut i64) -> c_int,
    pub gst_element_query_duration: unsafe extern "C" fn(*mut GstElement, c_int, *mut i64) -> c_int,
    pub gst_element_seek_simple: unsafe extern "C" fn(*mut GstElement, c_int, c_uint, i64) -> c_int,
    pub gst_element_seek:
        unsafe extern "C" fn(*mut GstElement, f64, c_int, c_uint, c_int, i64, c_int, i64) -> c_int,
    pub gst_element_query: unsafe extern "C" fn(*mut GstElement, *mut c_void) -> c_int,
    pub gst_element_send_event: unsafe extern "C" fn(*mut GstElement, *mut GstEvent) -> c_int,
    pub gst_element_set_context: unsafe extern "C" fn(*mut GstElement, *mut GstContext),
    // Building a custom audio-sink bin (`volume ! pulsesink`).
    pub gst_bin_new: Option<unsafe extern "C" fn(*const c_char) -> *mut GstElement>,
    pub gst_bin_add: Option<unsafe extern "C" fn(*mut GstElement, *mut GstElement) -> c_int>,
    pub gst_element_link: Option<unsafe extern "C" fn(*mut GstElement, *mut GstElement) -> c_int>,
    pub gst_element_get_static_pad:
        Option<unsafe extern "C" fn(*mut GstElement, *const c_char) -> *mut GstPad>,
    pub gst_ghost_pad_new:
        Option<unsafe extern "C" fn(*const c_char, *mut GstPad) -> *mut GstPad>,
    pub gst_element_add_pad: Option<unsafe extern "C" fn(*mut GstElement, *mut GstPad) -> c_int>,
    pub gst_query_new_seeking: unsafe extern "C" fn(c_int) -> *mut c_void,
    pub gst_query_parse_seeking:
        unsafe extern "C" fn(*mut c_void, *mut c_int, *mut c_int, *mut i64, *mut i64),
    pub gst_query_new_buffering: unsafe extern "C" fn(c_int) -> *mut c_void,
    pub gst_query_get_n_buffering_ranges: unsafe extern "C" fn(*mut c_void) -> c_uint,
    pub gst_query_parse_nth_buffering_range:
        unsafe extern "C" fn(*mut c_void, c_uint, *mut i64, *mut i64) -> c_int,
    pub gst_element_get_bus: unsafe extern "C" fn(*mut GstElement) -> *mut GstBus,
    pub gst_bus_pop_filtered: unsafe extern "C" fn(*mut GstBus, c_uint) -> *mut GstMessage,
    pub gst_message_parse_error:
        unsafe extern "C" fn(*mut GstMessage, *mut *mut GError, *mut *mut c_char),
    pub gst_message_parse_warning:
        unsafe extern "C" fn(*mut GstMessage, *mut *mut GError, *mut *mut c_char),
    pub gst_message_parse_buffering:
        Option<unsafe extern "C" fn(*mut GstMessage, *mut c_int)>,
    pub gst_message_parse_context_type:
        Option<unsafe extern "C" fn(*mut GstMessage, *mut *const c_char)>,
    pub gst_message_parse_stream_collection:
        Option<unsafe extern "C" fn(*mut GstMessage, *mut *mut GstStreamCollection)>,
    pub gst_object_unref: unsafe extern "C" fn(*mut c_void),
    pub gst_object_ref: unsafe extern "C" fn(*mut c_void) -> *mut c_void,
    pub gst_sample_get_buffer: unsafe extern "C" fn(*mut GstSample) -> *mut GstBuffer,
    pub gst_sample_get_caps: unsafe extern "C" fn(*mut GstSample) -> *mut GstCaps,
    pub gst_buffer_peek_memory: unsafe extern "C" fn(*mut GstBuffer, c_uint) -> *mut GstMemory,
    pub gst_buffer_n_memory: Option<unsafe extern "C" fn(*mut GstBuffer) -> c_uint>,
    pub gst_buffer_map: unsafe extern "C" fn(*mut GstBuffer, *mut GstMapInfo, c_uint) -> c_int,
    pub gst_buffer_unmap: unsafe extern "C" fn(*mut GstBuffer, *mut GstMapInfo),
    pub gst_caps_from_string: unsafe extern "C" fn(*const c_char) -> *mut GstCaps,
    pub gst_caps_unref: unsafe extern "C" fn(*mut GstCaps),
    pub gst_caps_get_structure: unsafe extern "C" fn(*mut GstCaps, c_uint) -> *mut GstStructure,
    pub gst_structure_get_int:
        unsafe extern "C" fn(*mut GstStructure, *const c_char, *mut c_int) -> c_int,
    pub gst_structure_get_string:
        unsafe extern "C" fn(*mut GstStructure, *const c_char) -> *const c_char,
    pub gst_structure_set_ptr: Option<
        unsafe extern "C" fn(
            *mut GstStructure,
            *const c_char,
            GType,
            *mut c_void,
            *const c_void,
        ),
    >,
    pub gst_mini_object_unref: unsafe extern "C" fn(*mut GstMiniObject),
    pub gst_context_new: unsafe extern "C" fn(*const c_char, c_int) -> *mut GstContext,
    pub gst_context_unref: unsafe extern "C" fn(*mut GstContext),
    pub gst_context_writable_structure:
        Option<unsafe extern "C" fn(*mut GstContext) -> *mut GstStructure>,
    pub gst_event_new_select_streams: Option<unsafe extern "C" fn(*mut GList) -> *mut GstEvent>,
    pub gst_stream_collection_get_size: Option<unsafe extern "C" fn(*mut GstStreamCollection) -> c_uint>,
    pub gst_stream_collection_get_stream:
        Option<unsafe extern "C" fn(*mut GstStreamCollection, c_uint) -> *mut GstStream>,
    pub gst_stream_get_stream_type: Option<unsafe extern "C" fn(*mut GstStream) -> c_uint>,
    pub gst_stream_get_stream_id:
        Option<unsafe extern "C" fn(*mut GstStream) -> *const c_char>,
    pub gst_stream_get_tags: Option<unsafe extern "C" fn(*mut GstStream) -> *mut GstTagList>,
    pub gst_tag_list_get_string: Option<
        unsafe extern "C" fn(*const GstTagList, *const c_char, *mut *mut c_char) -> c_int,
    >,
    pub gst_registry_get: Option<unsafe extern "C" fn() -> *mut GstRegistry>,
    pub gst_registry_lookup_feature:
        Option<unsafe extern "C" fn(*mut GstRegistry, *const c_char) -> *mut GstPluginFeature>,
    pub gst_plugin_feature_set_rank:
        Option<unsafe extern "C" fn(*mut GstPluginFeature, c_uint)>,

    // libgstapp-1.0.so.0
    pub gst_app_sink_try_pull_preroll:
        unsafe extern "C" fn(*mut GstElement, GstClockTime) -> *mut GstSample,
    pub gst_app_sink_try_pull_sample:
        unsafe extern "C" fn(*mut GstElement, GstClockTime) -> *mut GstSample,
    pub gst_app_sink_is_eos: unsafe extern "C" fn(*mut GstElement) -> c_int,
    pub gst_app_sink_set_caps: unsafe extern "C" fn(*mut GstElement, *const GstCaps),

    // libgstgl-1.0.so.0 (optional)
    pub gst_is_gl_memory: Option<unsafe extern "C" fn(*mut GstMemory) -> c_int>,
    pub gst_gl_memory_get_texture_id: Option<unsafe extern "C" fn(*mut GstMemory) -> u32>,
    pub gst_gl_display_egl_new_with_egl_display:
        Option<unsafe extern "C" fn(*mut c_void) -> *mut GstGLDisplay>,
    pub gst_gl_context_new_wrapped: Option<
        unsafe extern "C" fn(*mut GstGLDisplay, usize, c_uint, c_uint) -> *mut GstGLContext,
    >,
    pub gst_gl_context_activate: Option<unsafe extern "C" fn(*mut GstGLContext, c_int) -> c_int>,
    pub gst_gl_display_filter_gl_api: Option<unsafe extern "C" fn(*mut GstGLDisplay, c_uint)>,
    pub gst_context_set_gl_display:
        Option<unsafe extern "C" fn(*mut GstContext, *mut GstGLDisplay)>,
    pub gst_gl_context_get_type: Option<unsafe extern "C" fn() -> GType>,

    // libgstallocators-1.0.so.0 (optional, DMA-Buf)
    pub gst_is_dmabuf_memory: Option<unsafe extern "C" fn(*mut GstMemory) -> c_int>,
    pub gst_dmabuf_memory_get_fd: Option<unsafe extern "C" fn(*mut GstMemory) -> c_int>,

    // libgstvideo-1.0.so.0 (optional, DMA-DRM fourcc helpers)
    pub gst_video_dma_drm_fourcc_from_string: Option<
        unsafe extern "C" fn(*const c_char, *mut u64) -> u32,
    >,
    pub gst_buffer_get_video_meta: Option<unsafe extern "C" fn(*mut GstBuffer) -> *mut c_void>,
    pub gst_video_meta_get_plane_height:
        Option<unsafe extern "C" fn(*mut c_void, c_uint, *mut c_uint) -> c_int>,

    // libgobject-2.0.so.0  — variadic, we load it once and cast to different signatures
    pub g_object_set_string:
        unsafe extern "C" fn(*mut GObject, *const c_char, *const c_char, *const c_void),
    pub g_object_set_int: unsafe extern "C" fn(*mut GObject, *const c_char, c_int, *const c_void),
    pub g_object_set_ptr:
        unsafe extern "C" fn(*mut GObject, *const c_char, *mut c_void, *const c_void),
    pub g_object_set_double: unsafe extern "C" fn(*mut GObject, *const c_char, f64, *const c_void),
    pub g_object_set_int64: unsafe extern "C" fn(*mut GObject, *const c_char, i64, *const c_void),
    pub g_object_get_int:
        unsafe extern "C" fn(*mut GObject, *const c_char, *mut c_int, *const c_void),
    pub g_object_get_ptr:
        unsafe extern "C" fn(*mut GObject, *const c_char, *mut *mut c_void, *const c_void),

    // libglib-2.0.so.0
    pub g_free: unsafe extern "C" fn(*mut c_void),
    pub g_error_free: unsafe extern "C" fn(*mut GError),
    pub g_list_append: Option<unsafe extern "C" fn(*mut GList, *mut c_void) -> *mut GList>,
    pub g_list_free: Option<unsafe extern "C" fn(*mut GList)>,
    pub g_strcmp0: Option<unsafe extern "C" fn(*const c_char, *const c_char) -> c_int>,
}

impl LibGStreamer {
    pub fn try_load() -> Option<Self> {
        let gst = ModuleLoader::load("libgstreamer-1.0.so.0").ok()?;
        let gstapp = ModuleLoader::load("libgstapp-1.0.so.0").ok()?;
        let gobject = ModuleLoader::load("libgobject-2.0.so.0").ok()?;
        let glib = ModuleLoader::load("libglib-2.0.so.0").ok()?;
        let gstgl = ModuleLoader::load("libgstgl-1.0.so.0").ok();
        let gstallocators = ModuleLoader::load("libgstallocators-1.0.so.0").ok();
        let gstvideo = ModuleLoader::load("libgstvideo-1.0.so.0").ok();

        Some(LibGStreamer {
            gst_init: gst.get_symbol("gst_init").ok()?,
            gst_element_factory_make: gst.get_symbol("gst_element_factory_make").ok()?,
            gst_element_set_state: gst.get_symbol("gst_element_set_state").ok()?,
            gst_element_get_state: gst.get_symbol("gst_element_get_state").ok()?,
            gst_element_query_position: gst.get_symbol("gst_element_query_position").ok()?,
            gst_element_query_duration: gst.get_symbol("gst_element_query_duration").ok()?,
            gst_element_seek_simple: gst.get_symbol("gst_element_seek_simple").ok()?,
            gst_element_seek: gst.get_symbol("gst_element_seek").ok()?,
            gst_element_query: gst.get_symbol("gst_element_query").ok()?,
            gst_element_send_event: gst.get_symbol("gst_element_send_event").ok()?,
            gst_element_set_context: gst.get_symbol("gst_element_set_context").ok()?,
            gst_bin_new: gst.get_symbol("gst_bin_new").ok(),
            gst_bin_add: gst.get_symbol("gst_bin_add").ok(),
            gst_element_link: gst.get_symbol("gst_element_link").ok(),
            gst_element_get_static_pad: gst.get_symbol("gst_element_get_static_pad").ok(),
            gst_ghost_pad_new: gst.get_symbol("gst_ghost_pad_new").ok(),
            gst_element_add_pad: gst.get_symbol("gst_element_add_pad").ok(),
            gst_query_new_seeking: gst.get_symbol("gst_query_new_seeking").ok()?,
            gst_query_parse_seeking: gst.get_symbol("gst_query_parse_seeking").ok()?,
            gst_query_new_buffering: gst.get_symbol("gst_query_new_buffering").ok()?,
            gst_query_get_n_buffering_ranges: gst
                .get_symbol("gst_query_get_n_buffering_ranges")
                .ok()?,
            gst_query_parse_nth_buffering_range: gst
                .get_symbol("gst_query_parse_nth_buffering_range")
                .ok()?,
            gst_element_get_bus: gst.get_symbol("gst_element_get_bus").ok()?,
            gst_bus_pop_filtered: gst.get_symbol("gst_bus_pop_filtered").ok()?,
            gst_message_parse_error: gst.get_symbol("gst_message_parse_error").ok()?,
            gst_message_parse_warning: gst.get_symbol("gst_message_parse_warning").ok()?,
            gst_message_parse_buffering: gst.get_symbol("gst_message_parse_buffering").ok(),
            gst_message_parse_context_type: gst.get_symbol("gst_message_parse_context_type").ok(),
            gst_message_parse_stream_collection: gst
                .get_symbol("gst_message_parse_stream_collection")
                .ok(),
            gst_object_unref: gst.get_symbol("gst_object_unref").ok()?,
            gst_object_ref: gst.get_symbol("gst_object_ref").ok()?,
            gst_sample_get_buffer: gst.get_symbol("gst_sample_get_buffer").ok()?,
            gst_sample_get_caps: gst.get_symbol("gst_sample_get_caps").ok()?,
            gst_buffer_peek_memory: gst.get_symbol("gst_buffer_peek_memory").ok()?,
            gst_buffer_n_memory: gst.get_symbol("gst_buffer_n_memory").ok(),
            gst_buffer_map: gst.get_symbol("gst_buffer_map").ok()?,
            gst_buffer_unmap: gst.get_symbol("gst_buffer_unmap").ok()?,
            gst_caps_from_string: gst.get_symbol("gst_caps_from_string").ok()?,
            gst_caps_unref: gst.get_symbol("gst_caps_unref").ok()?,
            gst_caps_get_structure: gst.get_symbol("gst_caps_get_structure").ok()?,
            gst_structure_get_int: gst.get_symbol("gst_structure_get_int").ok()?,
            gst_structure_get_string: gst.get_symbol("gst_structure_get_string").ok()?,
            gst_structure_set_ptr: gst.get_symbol("gst_structure_set").ok(),
            gst_mini_object_unref: gst.get_symbol("gst_mini_object_unref").ok()?,
            gst_context_new: gst.get_symbol("gst_context_new").ok()?,
            gst_context_unref: gst.get_symbol("gst_context_unref").ok()?,
            gst_context_writable_structure: gst.get_symbol("gst_context_writable_structure").ok(),
            gst_event_new_select_streams: gst.get_symbol("gst_event_new_select_streams").ok(),
            gst_stream_collection_get_size: gst.get_symbol("gst_stream_collection_get_size").ok(),
            gst_stream_collection_get_stream: gst
                .get_symbol("gst_stream_collection_get_stream")
                .ok(),
            gst_stream_get_stream_type: gst.get_symbol("gst_stream_get_stream_type").ok(),
            gst_stream_get_stream_id: gst.get_symbol("gst_stream_get_stream_id").ok(),
            gst_stream_get_tags: gst.get_symbol("gst_stream_get_tags").ok(),
            gst_tag_list_get_string: gst.get_symbol("gst_tag_list_get_string").ok(),
            gst_registry_get: gst.get_symbol("gst_registry_get").ok(),
            gst_registry_lookup_feature: gst.get_symbol("gst_registry_lookup_feature").ok(),
            gst_plugin_feature_set_rank: gst.get_symbol("gst_plugin_feature_set_rank").ok(),

            gst_app_sink_try_pull_preroll: gstapp
                .get_symbol("gst_app_sink_try_pull_preroll")
                .ok()?,
            gst_app_sink_try_pull_sample: gstapp.get_symbol("gst_app_sink_try_pull_sample").ok()?,
            gst_app_sink_is_eos: gstapp.get_symbol("gst_app_sink_is_eos").ok()?,
            gst_app_sink_set_caps: gstapp.get_symbol("gst_app_sink_set_caps").ok()?,

            gst_is_gl_memory: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_is_gl_memory").ok()),
            gst_gl_memory_get_texture_id: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_gl_memory_get_texture_id").ok()),
            gst_gl_display_egl_new_with_egl_display: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_gl_display_egl_new_with_egl_display").ok()),
            gst_gl_context_new_wrapped: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_gl_context_new_wrapped").ok()),
            gst_gl_context_activate: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_gl_context_activate").ok()),
            gst_gl_display_filter_gl_api: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_gl_display_filter_gl_api").ok()),
            gst_context_set_gl_display: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_context_set_gl_display").ok()),
            gst_gl_context_get_type: gstgl
                .as_ref()
                .and_then(|m| m.get_symbol("gst_gl_context_get_type").ok()),

            gst_is_dmabuf_memory: gstallocators
                .as_ref()
                .and_then(|m| m.get_symbol("gst_is_dmabuf_memory").ok()),
            gst_dmabuf_memory_get_fd: gstallocators
                .as_ref()
                .and_then(|m| m.get_symbol("gst_dmabuf_memory_get_fd").ok()),

            gst_video_dma_drm_fourcc_from_string: gstvideo
                .as_ref()
                .and_then(|m| m.get_symbol("gst_video_dma_drm_fourcc_from_string").ok()),
            gst_buffer_get_video_meta: gstvideo
                .as_ref()
                .and_then(|m| m.get_symbol("gst_buffer_get_video_meta").ok()),
            gst_video_meta_get_plane_height: gstvideo
                .as_ref()
                .and_then(|m| m.get_symbol("gst_video_meta_get_plane_height").ok()),

            g_object_set_string: gobject.get_symbol("g_object_set").ok()?,
            g_object_set_int: gobject.get_symbol("g_object_set").ok()?,
            g_object_set_ptr: gobject.get_symbol("g_object_set").ok()?,
            g_object_set_double: gobject.get_symbol("g_object_set").ok()?,
            g_object_set_int64: gobject.get_symbol("g_object_set").ok()?,
            g_object_get_int: gobject.get_symbol("g_object_get").ok()?,
            g_object_get_ptr: gobject.get_symbol("g_object_get").ok()?,

            g_free: glib.get_symbol("g_free").ok()?,
            g_error_free: glib.get_symbol("g_error_free").ok()?,
            g_list_append: glib.get_symbol("g_list_append").ok(),
            g_list_free: glib.get_symbol("g_list_free").ok(),
            g_strcmp0: glib.get_symbol("g_strcmp0").ok(),

            _gst: gst,
            _gstapp: gstapp,
            _gobject: gobject,
            _glib: glib,
            _gstgl: gstgl,
            _gstallocators: gstallocators,
            _gstvideo: gstvideo,
        })
    }

    pub fn init(&self) {
        unsafe {
            (self.gst_init)(std::ptr::null_mut(), std::ptr::null_mut());
        }
        super::gst_gl_share::bump_va_decoder_ranks(self);
    }

    pub fn has_gl_share_support(&self) -> bool {
        self.gst_gl_display_egl_new_with_egl_display.is_some()
            && self.gst_gl_context_new_wrapped.is_some()
            && self.gst_is_gl_memory.is_some()
            && self.gst_gl_memory_get_texture_id.is_some()
    }

    pub fn has_dmabuf_support(&self) -> bool {
        self.gst_is_dmabuf_memory.is_some() && self.gst_dmabuf_memory_get_fd.is_some()
    }
}

// LibGStreamer is created once and stored in CxOs; it's only used from the main thread.
unsafe impl Send for LibGStreamer {}
unsafe impl Sync for LibGStreamer {}
