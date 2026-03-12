#![allow(unused_imports)]
use std::fs::File;
use std::os::fd::{AsFd, AsRawFd, FromRawFd};

use crate::egl_sys::{EGLNativeWindowType, EGLSurface, NativeWindowType};
use crate::makepad_math::Vec2d;
use wayland_client::protocol::__interfaces::WL_OUTPUT_INTERFACE;
use wayland_client::protocol::{wl_buffer, wl_compositor, wl_shm, wl_shm_pool, wl_surface};
use wayland_client::{Proxy, QueueHandle};
use wayland_egl::WlEglSurface;
use wayland_protocols::wp::fractional_scale::v1::client::{
    wp_fractional_scale_manager_v1, wp_fractional_scale_v1,
};
use wayland_protocols::wp::viewporter::client::{wp_viewport, wp_viewporter};
use wayland_protocols::xdg::decoration::zv1::client::{
    zxdg_decoration_manager_v1, zxdg_toplevel_decoration_v1,
};
use wayland_protocols::xdg::shell;
use wayland_protocols::xdg::shell::client::{
    xdg_popup, xdg_positioner, xdg_surface, xdg_toplevel, xdg_wm_base,
};
use wayland_protocols::xdg::toplevel_icon::v1::client::{
    xdg_toplevel_icon_manager_v1, xdg_toplevel_icon_v1,
};

use crate::opengl_cx::OpenglCx;
use crate::wayland::wayland_state::WaylandState;
use crate::{egl_sys, event::WindowGeom, WindowId};

pub(crate) struct WaylandWindow {
    pub window_id: WindowId,
    pub base_surface: wl_surface::WlSurface,
    pub toplevel: xdg_toplevel::XdgToplevel,
    pub decoration: Option<zxdg_toplevel_decoration_v1::ZxdgToplevelDecorationV1>,
    pub xdg_surface: xdg_surface::XdgSurface,
    pub viewport: Option<wp_viewport::WpViewport>,
    pub fractional_scale: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    pub configured: bool,
    pub window_geom: WindowGeom,
    pub cal_size: Vec2d,
    pub wl_egl_surface: WlEglSurface,
    pub egl_surface: EGLSurface,
}

impl WaylandWindow {
    pub fn new(
        window_id: WindowId,
        compositer: &wl_compositor::WlCompositor,
        wm_base: &xdg_wm_base::XdgWmBase,
        decoration_manager: Option<&zxdg_decoration_manager_v1::ZxdgDecorationManagerV1>,
        scale_manager: Option<&wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
        viewporter: Option<&wp_viewporter::WpViewporter>,
        icon_manager: Option<&xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1>,
        shm: Option<&wl_shm::WlShm>,
        qhandle: &QueueHandle<WaylandState>,
        opengl_cx: &OpenglCx,
        inner_size: Vec2d,
        position: Option<Vec2d>,
        title: &str,
        app_id: &str,
        is_fullscreen: bool,
    ) -> WaylandWindow {
        // Checked "downcast" of the EGL platform display to a X11 display.
        assert_eq!(opengl_cx.egl_platform, egl_sys::EGL_PLATFORM_WAYLAND_KHR);

        let base_surface = compositer.create_surface(qhandle, ());
        let fractional_scale = scale_manager
            .map(|manager| manager.get_fractional_scale(&base_surface, qhandle, window_id));
        let viewport = viewporter.map(|vp| vp.get_viewport(&base_surface, qhandle, ()));

        let shell_surface = wm_base.get_xdg_surface(&base_surface, qhandle, window_id);
        let toplevel = shell_surface.get_toplevel(qhandle, window_id);
        toplevel.set_title(String::from(title));
        toplevel.set_app_id(app_id.to_owned());

        // Set window icon via xdg-toplevel-icon-v1 if compositor supports it
        Self::set_wayland_icon(icon_manager, shm, &toplevel, qhandle);

        let decoration = decoration_manager.map(|manager| {
            let decoration = manager.get_toplevel_decoration(&toplevel, qhandle, ());
            decoration.set_mode(zxdg_toplevel_decoration_v1::Mode::ClientSide);
            decoration
        });

        if is_fullscreen {
            toplevel.set_fullscreen(None);
        }
        base_surface.commit();

        let wl_egl_surface =
            WlEglSurface::new(base_surface.id(), inner_size.x as i32, inner_size.y as i32).unwrap();
        let egl_surface = unsafe {
            (opengl_cx.libegl.eglCreateWindowSurface.unwrap())(
                opengl_cx.egl_display,
                opengl_cx.egl_config,
                wl_egl_surface.ptr() as NativeWindowType,
                std::ptr::null(),
            )
        };
        assert!(!egl_surface.is_null(), "eglCreateWindowSurface failed");

        // let positioner = wm_base.create_positioner(qhandle, ());
        let position = position.unwrap_or_default();

        let geom = WindowGeom {
            xr_is_presenting: false,
            can_fullscreen: false,
            is_topmost: false,
            is_fullscreen: false,
            inner_size: inner_size,
            outer_size: inner_size,
            dpi_factor: 1.0,
            position: position,
        };
        Self {
            base_surface,
            toplevel,
            decoration,
            viewport,
            fractional_scale,
            configured: false,
            xdg_surface: shell_surface,
            window_id,
            cal_size: Vec2d::default(),
            window_geom: geom,
            wl_egl_surface,
            egl_surface,
        }
    }
    /// Set the toplevel icon via xdg-toplevel-icon-v1 protocol using shm pixel data.
    /// Silently skips if the compositor does not support the protocol.
    fn set_wayland_icon(
        icon_manager: Option<&xdg_toplevel_icon_manager_v1::XdgToplevelIconManagerV1>,
        shm: Option<&wl_shm::WlShm>,
        toplevel: &xdg_toplevel::XdgToplevel,
        qhandle: &QueueHandle<WaylandState>,
    ) {
        let (icon_manager, shm) = match (icon_manager, shm) {
            (Some(im), Some(s)) => (im, s),
            _ => return, // compositor doesn't support the protocol
        };

        let icon_data = crate::app_icon::window_icon();
        let buf = match icon_data.buffers.first() {
            Some(b) => b,
            None => return,
        };

        let width = buf.width as usize;
        let height = buf.height as usize;
        // Convert RGBA8 to ARGB8888 (Wayland native byte order)
        let pixel_count = width * height;
        let shm_size = pixel_count * 4;

        // Create anonymous shm file
        let name = std::ffi::CString::new("makepad-icon").unwrap();
        let fd =
            unsafe { crate::libc_sys::memfd_create(name.as_ptr(), crate::libc_sys::MFD_CLOEXEC) };
        if fd < 0 {
            return;
        }
        let fd = unsafe { std::os::fd::OwnedFd::from_raw_fd(fd) };
        if unsafe { crate::libc_sys::ftruncate(fd.as_raw_fd(), shm_size as i64) } != 0 {
            return;
        }

        // mmap and write ARGB data
        let map = unsafe {
            crate::libc_sys::mmap(
                std::ptr::null_mut(),
                shm_size as crate::libc_sys::size_t,
                crate::libc_sys::PROT_READ | crate::libc_sys::PROT_WRITE,
                crate::libc_sys::MAP_SHARED,
                fd.as_raw_fd(),
                0,
            )
        };
        if map == crate::libc_sys::MAP_FAILED {
            return;
        }
        let dst = unsafe { std::slice::from_raw_parts_mut(map as *mut u8, shm_size) };
        for i in 0..pixel_count {
            let r = buf.data[i * 4];
            let g = buf.data[i * 4 + 1];
            let b = buf.data[i * 4 + 2];
            let a = buf.data[i * 4 + 3];
            // ARGB8888 in native byte order
            let argb = u32::from_ne_bytes([b, g, r, a]);
            dst[i * 4..i * 4 + 4].copy_from_slice(&argb.to_ne_bytes());
        }
        unsafe {
            crate::libc_sys::munmap(map, shm_size as crate::libc_sys::size_t);
        }

        // Create wl_shm_pool and wl_buffer
        let pool = shm.create_pool(fd.as_fd(), shm_size as i32, qhandle, ());
        let wl_buf = pool.create_buffer(
            0,
            width as i32,
            height as i32,
            (width * 4) as i32,
            wl_shm::Format::Argb8888,
            qhandle,
            (),
        );

        // Create icon, add buffer, set on toplevel
        let icon = icon_manager.create_icon(qhandle, ());
        icon.add_buffer(&wl_buf, buf.scale);
        icon_manager.set_icon(toplevel, Some(&icon));

        // The icon object and buffer can be destroyed after set_icon
        icon.destroy();
        pool.destroy();
        // wl_buf kept alive until compositor reads it (destroyed on drop)
    }

    pub fn resize_buffers(&mut self) -> bool {
        let cal_size = Vec2d {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor,
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            let pix_width = cal_size.x.max(1.0) as i32;
            let pix_height = cal_size.y.max(1.0) as i32;
            self.wl_egl_surface.resize(pix_width, pix_height, 0, 0);
            true
        } else {
            false
        }
    }
    pub fn close_window(&mut self) {
        // Destroy in protocol order: role-specific objects first, base
        // surface last.
        if let Some(decoration) = self.decoration.take() {
            decoration.destroy();
        }
        self.toplevel.destroy();
        self.xdg_surface.destroy();
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional_scale) = self.fractional_scale.take() {
            fractional_scale.destroy();
        }
        self.base_surface.destroy();
    }
}

pub(crate) struct WaylandPopupWindow {
    pub window_id: WindowId,
    pub parent_window_id: WindowId,
    pub base_surface: wl_surface::WlSurface,
    pub xdg_surface: xdg_surface::XdgSurface,
    pub xdg_popup: xdg_popup::XdgPopup,
    pub viewport: Option<wp_viewport::WpViewport>,
    pub fractional_scale: Option<wp_fractional_scale_v1::WpFractionalScaleV1>,
    pub wl_egl_surface: Option<WlEglSurface>,
    pub egl_surface: EGLSurface,
    pub egl_display: egl_sys::EGLDisplay,
    egl_destroy_surface_fn:
        unsafe extern "C" fn(egl_sys::EGLDisplay, EGLSurface) -> egl_sys::EGLBoolean,
    pub window_geom: WindowGeom,
    pub configured: bool,
    pub cal_size: Vec2d,
}

impl WaylandPopupWindow {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        window_id: WindowId,
        parent_window_id: WindowId,
        compositer: &wl_compositor::WlCompositor,
        wm_base: &xdg_wm_base::XdgWmBase,
        parent_xdg_surface: &xdg_surface::XdgSurface,
        _seat: Option<&wayland_client::protocol::wl_seat::WlSeat>,
        _pointer_serial: Option<u32>,
        _keyboard_serial: Option<u32>,
        scale_manager: Option<&wp_fractional_scale_manager_v1::WpFractionalScaleManagerV1>,
        viewporter: Option<&wp_viewporter::WpViewporter>,
        qhandle: &QueueHandle<WaylandState>,
        opengl_cx: &OpenglCx,
        size: Vec2d,
        position: Vec2d,
        _grab_keyboard: bool,
    ) -> WaylandPopupWindow {
        assert_eq!(opengl_cx.egl_platform, egl_sys::EGL_PLATFORM_WAYLAND_KHR);

        let base_surface = compositer.create_surface(qhandle, ());
        let fractional_scale = scale_manager
            .map(|manager| manager.get_fractional_scale(&base_surface, qhandle, window_id));
        let viewport = viewporter.map(|vp| vp.get_viewport(&base_surface, qhandle, ()));

        let xdg_surface = wm_base.get_xdg_surface(&base_surface, qhandle, window_id);
        let positioner = wm_base.create_positioner(qhandle, ());
        positioner.set_size(size.x.max(1.0) as i32, size.y.max(1.0) as i32);
        positioner.set_anchor_rect(position.x as i32, position.y as i32, 1, 1);
        positioner.set_anchor(xdg_positioner::Anchor::TopLeft);
        positioner.set_gravity(xdg_positioner::Gravity::BottomRight);
        positioner.set_constraint_adjustment(
            xdg_positioner::ConstraintAdjustment::FlipX
                | xdg_positioner::ConstraintAdjustment::FlipY
                | xdg_positioner::ConstraintAdjustment::SlideX
                | xdg_positioner::ConstraintAdjustment::SlideY,
        );

        let xdg_popup =
            xdg_surface.get_popup(Some(parent_xdg_surface), &positioner, qhandle, window_id);
        // Do NOT grab the popup. Without a grab the compositor will not
        // auto-dismiss the popup on outside clicks, giving the app full
        // control over popup lifetime via explicit close.
        base_surface.commit();
        positioner.destroy();

        let wl_egl_surface = WlEglSurface::new(
            base_surface.id(),
            size.x.max(1.0) as i32,
            size.y.max(1.0) as i32,
        )
        .unwrap();
        let egl_surface = unsafe {
            (opengl_cx.libegl.eglCreateWindowSurface.unwrap())(
                opengl_cx.egl_display,
                opengl_cx.egl_config,
                wl_egl_surface.ptr() as NativeWindowType,
                std::ptr::null(),
            )
        };
        assert!(!egl_surface.is_null(), "eglCreateWindowSurface failed");

        let geom = WindowGeom {
            xr_is_presenting: false,
            can_fullscreen: false,
            is_topmost: false,
            is_fullscreen: false,
            inner_size: size,
            outer_size: size,
            dpi_factor: 1.0,
            position,
        };

        Self {
            window_id,
            parent_window_id,
            base_surface,
            xdg_surface,
            xdg_popup,
            viewport,
            fractional_scale,
            wl_egl_surface: Some(wl_egl_surface),
            egl_surface,
            egl_display: opengl_cx.egl_display,
            egl_destroy_surface_fn: opengl_cx.libegl.eglDestroySurface.unwrap(),
            window_geom: geom,
            configured: false,
            cal_size: Vec2d::default(),
        }
    }

    pub fn resize_buffers(&mut self) -> bool {
        let cal_size = Vec2d {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor,
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            if let Some(ref wl_egl_surface) = self.wl_egl_surface {
                wl_egl_surface.resize(cal_size.x.max(1.0) as i32, cal_size.y.max(1.0) as i32, 0, 0);
            }
            true
        } else {
            false
        }
    }

    pub fn close_window(&mut self) {
        // Destroy EGL surface first — it holds a reference to the wl_egl_surface.
        if !self.egl_surface.is_null() {
            unsafe {
                (self.egl_destroy_surface_fn)(self.egl_display, self.egl_surface);
            }
            self.egl_surface = std::ptr::null_mut();
        }
        // Drop wl_egl_surface before Wayland objects — wl_egl_window_destroy
        // accesses the underlying wl_surface.
        self.wl_egl_surface.take();
        // Destroy Wayland objects in protocol order: role-specific first,
        // then the base surface last.
        self.xdg_popup.destroy();
        self.xdg_surface.destroy();
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional_scale) = self.fractional_scale.take() {
            fractional_scale.destroy();
        }
        self.base_surface.destroy();
    }
}

impl Drop for WaylandWindow {
    fn drop(&mut self) {
        self.close_window();
    }
}

impl Drop for WaylandPopupWindow {
    fn drop(&mut self) {
        self.close_window();
    }
}
