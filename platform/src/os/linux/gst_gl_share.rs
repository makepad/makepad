//! Share Makepad's EGL display/context with GStreamer for GLMemory / glupload.
//!
//! Without this, `video/x-raw(memory:GLMemory)` textures live in a different GL
//! namespace and cannot be sampled by Makepad.
//!
//! Important: we do **not** wrap Makepad's live context. Wrapping the UI thread's
//! current context deadlocks when GStreamer's streaming thread tries to
//! `eglMakeCurrent` it during `glupload` preroll (READY→PAUSED stalls forever).
//! Instead we create a **sibling** EGL context that shares with Makepad's, wrap
//! that sibling as `gst.gl.app_context`, and let GStreamer create its own
//! contexts in the same share group.

use super::egl_sys;
use super::gstreamer_sys::*;
use super::opengl_cx::OpenglCx;
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
    /// Sibling EGL context shared with Makepad's (not Makepad's own handle).
    egl_share_context: egl_sys::EGLContext,
    egl_display: egl_sys::EGLDisplay,
    /// Makepad's live EGL context — released during GLMemory preroll so GStreamer
    /// can activate share-group contexts without deadlocking on NVIDIA.
    egl_makepad_context: egl_sys::EGLContext,
    egl_make_current: egl_sys::PFNEGLMAKECURRENTPROC,
    egl_destroy_context: egl_sys::PFNEGLDESTROYCONTEXTPROC,
}

impl GstGlShare {
    /// Create a shared EGL sibling of Makepad's context and wrap it for GStreamer.
    ///
    /// `opengl_cx` must come from Makepad's active window GL context.
    pub fn try_new(gst: &LibGStreamer, opengl_cx: &OpenglCx) -> Option<Self> {
        let new_wrapped = gst.gst_gl_context_new_wrapped?;
        let egl_display = opengl_cx.egl_display;
        let egl_context = opengl_cx.egl_context;
        let egl_config = opengl_cx.egl_config;
        let egl_create = opengl_cx.libegl.eglCreateContext?;
        let egl_destroy = opengl_cx.libegl.eglDestroyContext;
        let egl_make_current = opengl_cx.libegl.eglMakeCurrent;
        if egl_make_current.is_none() {
            return None;
        }
        if egl_display.is_null() || egl_context.is_null() || egl_config.is_null() {
            return None;
        }
        // Match Makepad's GLES3 context so the share group stays compatible.
        let ctx_attribs = [egl_sys::EGL_CONTEXT_MAJOR_VERSION, 3, egl_sys::EGL_NONE];
        // Ensure Makepad's context is current before creating a shared sibling
        // (some drivers require the share context to be current at create time).
        opengl_cx.make_current();
        let egl_share_context = unsafe {
            egl_create(
                egl_display,
                egl_config,
                egl_context,
                ctx_attribs.as_ptr() as _,
            )
        };
        if egl_share_context.is_null() {
            crate::log!("VIDEO: eglCreateContext (GStreamer share sibling) failed");
            return None;
        }
        unsafe {
            let display = if opengl_cx.egl_platform == egl_sys::EGL_PLATFORM_WAYLAND_KHR
                && !opengl_cx.egl_platform_display.is_null()
            {
                if let Some(wayland_new) = gst.gst_gl_display_wayland_new_with_display {
                    wayland_new(opengl_cx.egl_platform_display)
                } else if let Some(egl_new) = gst.gst_gl_display_egl_new_with_egl_display {
                    egl_new(opengl_cx.egl_display)
                } else {
                    if let Some(destroy) = egl_destroy {
                        destroy(egl_display, egl_share_context);
                    }
                    return None;
                }
            } else if let Some(egl_new) = gst.gst_gl_display_egl_new_with_egl_display {
                egl_new(opengl_cx.egl_display)
            } else {
                if let Some(destroy) = egl_destroy {
                    destroy(egl_display, egl_share_context);
                }
                return None;
            };
            if display.is_null() {
                if let Some(destroy) = egl_destroy {
                    destroy(egl_display, egl_share_context);
                }
                return None;
            }
            // Wrap the sibling — never Makepad's live UI context.
            let app_context = new_wrapped(
                display,
                egl_share_context as usize,
                GST_GL_PLATFORM_EGL,
                GST_GL_API_GLES2,
            );
            if app_context.is_null() {
                (gst.gst_object_unref)(display as *mut c_void);
                if let Some(destroy) = egl_destroy {
                    destroy(egl_display, egl_share_context);
                }
                return None;
            }
            // fill_info needs the wrapped sibling current; Makepad's context stays free.
            if let Some(activate) = gst.gst_gl_context_activate {
                let _ = activate(app_context, 1);
                if let Some(fill_info) = gst.gst_gl_context_fill_info {
                    let mut error: *mut GError = std::ptr::null_mut();
                    if fill_info(app_context, &mut error) == 0 {
                        if !error.is_null() {
                            let msg = CStr::from_ptr((*error).message)
                                .to_string_lossy()
                                .to_string();
                            crate::log!(
                                "VIDEO: gst_gl_context_fill_info failed on shared EGL sibling: {}",
                                msg
                            );
                            (gst.g_error_free)(error);
                        } else {
                            crate::log!(
                                "VIDEO: gst_gl_context_fill_info failed on shared EGL sibling"
                            );
                        }
                        (gst.gst_object_unref)(app_context as *mut c_void);
                        (gst.gst_object_unref)(display as *mut c_void);
                        if let Some(destroy) = egl_destroy {
                            destroy(egl_display, egl_share_context);
                        }
                        opengl_cx.make_current();
                        return None;
                    }
                    if !error.is_null() {
                        (gst.g_error_free)(error);
                    }
                }
                let _ = activate(app_context, 0);
            }
            // Restore Makepad's context for the UI thread.
            opengl_cx.make_current();
            if let Some(filter) = gst.gst_gl_display_filter_gl_api {
                filter(display, GST_GL_API_GLES2);
            }
            // Do NOT gst_gl_display_add_context(wrapped): glupload would pick the
            // wrapped context as its render context and fail with
            // "Subclass failed to initialize". Wrapped contexts are only valid as
            // gst.gl.app_context (share parent); GStreamer must create its own
            // GstGLContextEGL that shares with them.
            Some(Self {
                display,
                app_context,
                egl_share_context,
                egl_display,
                egl_makepad_context: egl_context,
                egl_make_current,
                egl_destroy_context: egl_destroy,
            })
        }
    }

    /// Drop Makepad's EGL context from this thread so GStreamer's GL thread can
    /// `eglMakeCurrent` share-group contexts (NVIDIA deadlocks otherwise).
    pub fn release_makepad_current(&self) {
        if let Some(make_current) = self.egl_make_current {
            unsafe {
                make_current(
                    self.egl_display,
                    egl_sys::EGL_NO_SURFACE,
                    egl_sys::EGL_NO_SURFACE,
                    egl_sys::EGL_NO_CONTEXT,
                );
            }
        }
    }

    pub fn restore_makepad_current(&self) {
        if let Some(make_current) = self.egl_make_current {
            if !self.egl_makepad_context.is_null() {
                unsafe {
                    make_current(
                        self.egl_display,
                        egl_sys::EGL_NO_SURFACE,
                        egl_sys::EGL_NO_SURFACE,
                        self.egl_makepad_context,
                    );
                }
            }
        }
    }

    /// Push display + app-context onto `element` (typically the playbin root).
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

    /// Answer a `NEED_CONTEXT` bus message by pushing GL display/app context
    /// onto the element that posted the request (not only playbin).
    pub fn handle_need_context_message(
        &self,
        gst: &LibGStreamer,
        msg: *mut GstMessage,
        fallback: *mut GstElement,
    ) {
        if !self.is_gl_need_context_message(gst, msg) {
            return;
        }
        let target = if let Some(get_src) = gst.gst_message_get_src {
            unsafe {
                let src = get_src(msg);
                if src.is_null() {
                    fallback
                } else {
                    src as *mut GstElement
                }
            }
        } else {
            fallback
        };
        self.apply_to_element(gst, target);
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
            if !self.egl_share_context.is_null() {
                if let Some(destroy) = self.egl_destroy_context {
                    destroy(self.egl_display, self.egl_share_context);
                }
                self.egl_share_context = std::ptr::null_mut();
            }
        }
    }
}

/// Prefer modern `va` plugin decoders over legacy `vaapi` (VASurface) elements.
///
/// Legacy vaapi decoders do not negotiate cleanly with `vapostproc` DMA-Buf sinks
/// inside playbin and can stall preroll until our zero-copy timeout fires.
pub fn bump_va_decoder_ranks(gst: &LibGStreamer) {
    let (Some(get_registry), Some(lookup), Some(set_rank)) = (
        gst.gst_registry_get,
        gst.gst_registry_lookup_feature,
        gst.gst_plugin_feature_set_rank,
    ) else {
        return;
    };
    // Above primary (256) and vaapidecodebin (258); below NVIDIA primary+1 paths.
    const MODERN_RANK: c_uint = 320;
    const LEGACY_RANK: c_uint = 0;
    unsafe {
        // Ensure the `va` / `vaapi` plugin features are registered before lookup.
        for name in ["vah264dec", "vapostproc", "vaapih264dec", "vaapipostproc"] {
            let ty = CString::new(name).unwrap();
            let el = (gst.gst_element_factory_make)(ty.as_ptr(), std::ptr::null());
            if !el.is_null() {
                (gst.gst_object_unref)(el as *mut c_void);
            }
        }

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
                set_rank(feature, MODERN_RANK);
                (gst.gst_object_unref)(feature as *mut c_void);
            }
        }
        for name in [
            "vaapidecodebin",
            "vaapih264dec",
            "vaapih265dec",
            "vaapivp9dec",
            "vaapivp8dec",
            "vaapiav1dec",
            "vaapijpegdec",
            "vaapimpeg2dec",
            "vaapivc1dec",
            "avdec_h264",
            "avdec_h265",
            "avdec_hev1",
        ] {
            let cname = CString::new(name).unwrap();
            let feature = lookup(registry, cname.as_ptr());
            if !feature.is_null() {
                set_rank(feature, LEGACY_RANK);
                (gst.gst_object_unref)(feature as *mut c_void);
            }
        }
        // Prefer modern vapostproc when present; otherwise keep vaapipostproc ranked
        // for system-memory download paths (it cannot export DMA-Buf on NVIDIA).
        let vapost = {
            let cname = CString::new("vapostproc").unwrap();
            let feature = lookup(registry, cname.as_ptr());
            let present = !feature.is_null();
            if present {
                (gst.gst_object_unref)(feature as *mut c_void);
            }
            present
        };
        let cname = CString::new("vaapipostproc").unwrap();
        let feature = lookup(registry, cname.as_ptr());
        if !feature.is_null() {
            set_rank(feature, if vapost { LEGACY_RANK } else { MODERN_RANK });
            (gst.gst_object_unref)(feature as *mut c_void);
        }
    }
}
