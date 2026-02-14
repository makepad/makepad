#![warn(missing_docs, missing_debug_implementations)]
#![forbid(improper_ctypes, unsafe_op_in_unsafe_fn)]

//! EGL utilities
//!
//! This module contains bindings to the `libwayland-egl.so` library.
//!
//! This library is used to interface with the OpenGL stack, and creating
//! EGL surfaces from a wayland surface.
//!
//! See [`WlEglSurface`] documentation for details.

use std::{fmt, os::raw::c_void};

use wayland_backend::client::ObjectId;
use wayland_sys::{client::wl_proxy, egl::*, ffi_dispatch};

/// Checks if the wayland-egl lib is available and can be used
///
/// Trying to create an [`WlEglSurface`] while this function returns
/// [`false`] will result in a panic.
pub fn is_available() -> bool {
    is_lib_available()
}

/// EGL surface
///
/// This object is a simple wrapper around a `wl_surface` to add the EGL
/// capabilities. Just use the [`ptr()`][WlEglSurface::ptr()] method once this object
/// is created to get the window pointer your OpenGL library is needing to initialize
/// the EGL context (you'll most likely need the display ptr as well, that you can
/// get via the [`ObjectId::as_ptr()`] method on of the `wl_display` ID).
#[derive(Debug)]
pub struct WlEglSurface {
    ptr: *mut wl_egl_window,
}

impl WlEglSurface {
    /// Create an EGL surface from a wayland surface
    ///
    /// This method will check that the provided [`ObjectId`] is still alive and from the
    /// correct interface (`wl_surface`).
    ///
    /// You must always destroy the [`WlEglSurface`] *before* the underling `wl_surface`
    /// protocol object.
    pub fn new(surface: ObjectId, width: i32, height: i32) -> Result<Self, Error> {
        if surface.interface().name != "wl_surface" {
            return Err(Error::InvalidId);
        }

        let ptr = surface.as_ptr();
        if ptr.is_null() {
            // ObjectId::as_ptr() returns NULL if the surface is no longer alive
            Err(Error::InvalidId)
        } else {
            // SAFETY: We are sure the pointer is valid and the interface is correct.
            unsafe { Self::new_from_raw(ptr, width, height) }
        }
    }

    /// Create an EGL surface from a raw pointer to a wayland surface.
    ///
    /// # Safety
    ///
    /// The provided pointer must be a valid `wl_surface` pointer from `libwayland-client`.
    pub unsafe fn new_from_raw(
        surface: *mut wl_proxy,
        width: i32,
        height: i32,
    ) -> Result<Self, Error> {
        if width <= 0 || height <= 0 {
            return Err(Error::InvalidSize);
        }
        let ptr = ffi_dispatch!(wayland_egl_handle(), wl_egl_window_create, surface, width, height);
        if ptr.is_null() {
            panic!("egl window allocation failed");
        }
        Ok(Self { ptr })
    }

    /// Fetch current size of the EGL surface
    pub fn get_size(&self) -> (i32, i32) {
        let mut w = 0i32;
        let mut h = 0i32;
        unsafe {
            ffi_dispatch!(
                wayland_egl_handle(),
                wl_egl_window_get_attached_size,
                self.ptr,
                &mut w as *mut i32,
                &mut h as *mut i32
            );
        }
        (w, h)
    }

    /// Resize the EGL surface
    ///
    /// The two first arguments `(width, height)` are the new size of
    /// the surface, the two others `(dx, dy)` represent the displacement
    /// of the top-left corner of the surface. It allows you to control the
    /// direction of the resizing if necessary.
    pub fn resize(&self, width: i32, height: i32, dx: i32, dy: i32) {
        unsafe {
            ffi_dispatch!(
                wayland_egl_handle(),
                wl_egl_window_resize,
                self.ptr,
                width,
                height,
                dx,
                dy
            )
        }
    }

    /// Raw pointer to the EGL surface
    ///
    /// You'll need this pointer to initialize the EGL context in your
    /// favourite OpenGL lib.
    pub fn ptr(&self) -> *const c_void {
        self.ptr as *const c_void
    }
}

// SAFETY: We own the pointer to the wl_egl_window and can therefore be transferred to another thread.
unsafe impl Send for WlEglSurface {}
// Note that WlEglSurface is !Sync. This is because the pointer performs no internal synchronization.

impl Drop for WlEglSurface {
    fn drop(&mut self) {
        unsafe {
            ffi_dispatch!(wayland_egl_handle(), wl_egl_window_destroy, self.ptr);
        }
    }
}

/// EGL surface creation error.
#[derive(Debug)]
pub enum Error {
    /// Surface width or height are <= 0.
    InvalidSize,
    /// Passed surface object is not a surface.
    InvalidId,
}

impl std::error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::InvalidSize => write!(f, "surface width or height is <= 0"),
            Error::InvalidId => write!(f, "object id is not a surface"),
        }
    }
}
