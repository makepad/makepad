#![allow(unused_imports)]
use std::fs::File;
use std::os::fd::AsFd;

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
use wayland_protocols::xdg::shell::client::{xdg_surface, xdg_toplevel, xdg_wm_base};

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
        qhandle: &QueueHandle<WaylandState>,
        opengl_cx: &OpenglCx,
        inner_size: Vec2d,
        position: Option<Vec2d>,
        title: &str,
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
        toplevel.set_app_id("Makepad".to_owned());

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
        self.base_surface.destroy();
        if let Some(decoration) = self.decoration.take() {
            decoration.destroy();
        }
        if let Some(viewport) = self.viewport.take() {
            viewport.destroy();
        }
        if let Some(fractional_scale) = self.fractional_scale.take() {
            fractional_scale.destroy();
        }
        self.toplevel.destroy();
        self.xdg_surface.destroy();
    }
}

impl Drop for WaylandWindow {
    fn drop(&mut self) {
        self.close_window();
    }
}
