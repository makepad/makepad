//! Share Makepad's EGL display/context with GStreamer for GLMemory / glupload.
//!
//! Without this, `video/x-raw(memory:GLMemory)` textures live in a different GL
//! namespace and cannot be sampled by Makepad.

use super::gstreamer_sys::*;
use std::ffi::{c_void, CStr, CString};
use std::os::raw::{c_char, c_uint};

pub const GST_GL_DISPLAY_CONTEXT_TYPE: &str = "gst.gl.GLDisplay";
pub const GST_GL_APP_CONTEXT_TYPE: &str = "gst.gl.app_context";

pub const GST_GL_PLATFORM_EGL: c_uint = 1 << 0;
pub const GST_GL_API_GLES2: c_uint = 1 << 16;

/// Owns a wrapped GstGLDisplay + application GstGLContext for the lifetime of
/// a player (released via [`GstGlShare::release`]).
pub struct GstGlShare {
    pub display: *mut GstGLDisplay,
    pub app_context: *mut GstGLContext,
}

impl GstGlShare {
    /// Wrap Makepad's current EGL display/context for GStreamer.
    ///
    /// `egl_display` / `egl_context` must come from Makepad's `OpenglCx`.
    pub fn try_new(
        gst: &LibGStreamer,
        egl_display: *mut c_void,
        egl_context: *mut c_void,
    ) -> Option<Self> {
        let new_display = gst.gst_gl_display_egl_new_with_egl_display?;
        let new_wrapped = gst.gst_gl_context_new_wrapped?;
        if egl_display.is_null() || egl_context.is_null() {
            return None;
        }
        unsafe {
            let display = new_display(egl_display);
            if display.is_null() {
                return None;
            }
            let app_context = new_wrapped(
                display,
                egl_context as usize,
                GST_GL_PLATFORM_EGL,
                GST_GL_API_GLES2,
            );
            if app_context.is_null() {
                (gst.gst_object_unref)(display as *mut c_void);
                return None;
            }
            // Ensure the wrapped context has valid GL info filled in.
            if let Some(activate) = gst.gst_gl_context_activate {
                let _ = activate(app_context, 1);
                let _ = activate(app_context, 0);
            }
            if let Some(filter) = gst.gst_gl_display_filter_gl_api {
                filter(display, GST_GL_API_GLES2);
            }
            Some(Self {
                display,
                app_context,
            })
        }
    }

    /// Push display + app-context onto `element` (typically the playbin root).
    /// Setting context on playbin propagates to children that NEED_CONTEXT.
    pub fn apply_to_element(&self, gst: &LibGStreamer, element: *mut GstElement) {
        if element.is_null() {
            return;
        }
        unsafe {
            // gst.gl.GLDisplay
            let type_display = CString::new(GST_GL_DISPLAY_CONTEXT_TYPE).unwrap();
            let ctx_display = (gst.gst_context_new)(type_display.as_ptr(), 1);
            if !ctx_display.is_null() {
                if let Some(set_display) = gst.gst_context_set_gl_display {
                    set_display(ctx_display, self.display);
                }
                (gst.gst_element_set_context)(element, ctx_display);
                (gst.gst_context_unref)(ctx_display);
            }

            // gst.gl.app_context { context: GstGLContext }
            let type_app = CString::new(GST_GL_APP_CONTEXT_TYPE).unwrap();
            let ctx_app = (gst.gst_context_new)(type_app.as_ptr(), 1);
            if !ctx_app.is_null() {
                if let (Some(writable), Some(gl_type), Some(structure_set)) = (
                    gst.gst_context_writable_structure,
                    gst.gst_gl_context_get_type,
                    gst.gst_structure_set_ptr,
                ) {
                    let structure = writable(ctx_app);
                    if !structure.is_null() {
                        let key = CString::new("context").unwrap();
                        let gtype = gl_type();
                        structure_set(
                            structure,
                            key.as_ptr(),
                            gtype,
                            self.app_context,
                            std::ptr::null(),
                        );
                    }
                }
                (gst.gst_element_set_context)(element, ctx_app);
                (gst.gst_context_unref)(ctx_app);
            }
        }
    }

    /// Returns true when `msg` is a GL display/app NEED_CONTEXT request.
    pub fn is_gl_need_context_message(&self, gst: &LibGStreamer, msg: *mut GstMessage) -> bool {
        if msg.is_null() {
            return false;
        }
        let parse = match gst.gst_message_parse_context_type {
            Some(f) => f,
            None => return false,
        };
        unsafe {
            let mut context_type: *const c_char = std::ptr::null();
            parse(msg, &mut context_type);
            if context_type.is_null() {
                return false;
            }
            let ty = CStr::from_ptr(context_type).to_string_lossy();
            ty == GST_GL_DISPLAY_CONTEXT_TYPE || ty == GST_GL_APP_CONTEXT_TYPE
        }
    }

    pub fn release(&mut self, gst: &LibGStreamer) {
        unsafe {
            if !self.app_context.is_null() {
                (gst.gst_object_unref)(self.app_context as *mut c_void);
                self.app_context = std::ptr::null_mut();
            }
            if !self.display.is_null() {
                (gst.gst_object_unref)(self.display as *mut c_void);
                self.display = std::ptr::null_mut();
            }
        }
    }
}

/// Prefer modern VA-API decoders when the `va` plugin registered features.
pub fn bump_va_decoder_ranks(gst: &LibGStreamer) {
    let (Some(get_registry), Some(lookup), Some(set_rank)) = (
        gst.gst_registry_get,
        gst.gst_registry_lookup_feature,
        gst.gst_plugin_feature_set_rank,
    ) else {
        return;
    };
    // Above typical primary software ranks so decodebin prefers VA when present.
    const RANK: c_uint = 300;
    unsafe {
        let registry = get_registry();
        if registry.is_null() {
            return;
        }
        for name in [
            "vah264dec",
            "vah265dec",
            "vavp9dec",
            "vavp8dec",
            "vaav1dec",
            "vajpegdec",
            "vampeg2dec",
            "vapostproc",
        ] {
            let cname = CString::new(name).unwrap();
            let feature = lookup(registry, cname.as_ptr());
            if !feature.is_null() {
                set_rank(feature, RANK);
                (gst.gst_object_unref)(feature as *mut c_void);
            }
        }
    }
}
