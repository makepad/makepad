use {
    self::super::{
        super::{
            egl_sys::{self},
            gl_sys::{self, LibGl},
        },
        x11_sys,
        xlib_window::XlibWindow,
    },
    crate::{
        cx::Cx,
        event::*,
        makepad_math::Vec2d,
        opengl_cx::OpenglCx,
        os::shared_framebuf::{LinuxOwnedImage, LinuxOwnedImagePlane},
        texture::{CxTexture, Texture},
        window::WindowId,
    },
    std::{
        mem,
        os::{
            self,
            fd::{AsRawFd as _, FromRawFd as _},
            raw::{c_long, c_void},
        },
    },
};

impl Cx {
    pub fn share_texture_for_presentable_image(
        &mut self,
        texture: &Texture,
    ) -> Option<LinuxOwnedImage> {
        let opengl_cx = self.os.opengl_cx.as_ref().unwrap();
        // RunView swapchains can be created during UI draw before the normal
        // repaint path calls `make_current`, especially on Wayland hosts.
        opengl_cx.make_current();

        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.gl());

        unsafe {
            // Ensure texture allocation/upload work is visible before export.
            (self.os.gl().glFinish)();

            let egl_image = (opengl_cx.libegl.eglCreateImageKHR.unwrap())(
                opengl_cx.egl_display,
                opengl_cx.egl_context,
                egl_sys::EGL_GL_TEXTURE_2D_KHR,
                cxtexture.os.gl_texture.unwrap() as egl_sys::EGLClientBuffer,
                std::ptr::null(),
            );
            if egl_image.is_null() {
                crate::error!("eglCreateImageKHR failed for shared texture export");
                return None;
            }

            let (mut fourcc, mut num_planes) = (0, 0);
            if (opengl_cx
                .libegl
                .eglExportDMABUFImageQueryMESA
                .expect("eglExportDMABUFImageQueryMESA unsupported"))(
                opengl_cx.egl_display,
                egl_image,
                &mut fourcc as *mut u32 as *mut i32,
                &mut num_planes,
                std::ptr::null_mut(),
            ) == 0
            {
                crate::error!("eglExportDMABUFImageQueryMESA failed");
                let _ = (opengl_cx.libegl.eglDestroyImageKHR.unwrap())(
                    opengl_cx.egl_display,
                    egl_image,
                );
                return None;
            }
            if num_planes != 1 {
                crate::error!(
                    "unsupported planar DRM format {:?} ({:#x}), num_planes={}",
                    std::str::from_utf8(&u32::to_le_bytes(fourcc)),
                    fourcc,
                    num_planes
                );
                let _ = (opengl_cx.libegl.eglDestroyImageKHR.unwrap())(
                    opengl_cx.egl_display,
                    egl_image,
                );
                return None;
            }

            // HACK(eddyb) `modifiers` are reported per-plane, so to avoid UB,
            // a second query call is used *after* the `num_planes == 1` check.
            let mut modifiers = 0;
            if (opengl_cx.libegl.eglExportDMABUFImageQueryMESA.unwrap())(
                opengl_cx.egl_display,
                egl_image,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                &mut modifiers,
            ) == 0
            {
                crate::error!("eglExportDMABUFImageQueryMESA(modifiers) failed");
                let _ = (opengl_cx.libegl.eglDestroyImageKHR.unwrap())(
                    opengl_cx.egl_display,
                    egl_image,
                );
                return None;
            }

            let (mut dma_buf_fd, mut offset, mut stride) = (0, 0, 0);
            if (opengl_cx.libegl.eglExportDMABUFImageMESA.unwrap())(
                opengl_cx.egl_display,
                egl_image,
                &mut dma_buf_fd,
                &mut stride as *mut u32 as *mut i32,
                &mut offset as *mut u32 as *mut i32,
            ) == 0
            {
                crate::error!("eglExportDMABUFImageMESA failed");
                let _ = (opengl_cx.libegl.eglDestroyImageKHR.unwrap())(
                    opengl_cx.egl_display,
                    egl_image,
                );
                return None;
            }
            if dma_buf_fd < 0 {
                use std::sync::atomic::{AtomicBool, Ordering};
                static LOG_INVALID_DMABUF_FD: AtomicBool = AtomicBool::new(false);
                if !LOG_INVALID_DMABUF_FD.swap(true, Ordering::Relaxed) {
                    crate::warning!(
                        "eglExportDMABUFImageMESA returned invalid fd (fourcc={:#x} modifiers={:#x} stride={} offset={})",
                        fourcc,
                        modifiers,
                        stride,
                        offset
                    );
                }
                let _ = (opengl_cx.libegl.eglDestroyImageKHR.unwrap())(
                    opengl_cx.egl_display,
                    egl_image,
                );
                return None;
            }

            if (opengl_cx.libegl.eglDestroyImageKHR.unwrap())(opengl_cx.egl_display, egl_image) == 0
            {
                crate::error!("eglDestroyImageKHR failed after DMA-BUF export");
                return None;
            }

            Some(LinuxOwnedImage {
                drm_format: crate::os::linux::dma_buf::DrmFormat { fourcc, modifiers },
                plane: LinuxOwnedImagePlane {
                    dma_buf_fd: os::fd::OwnedFd::from_raw_fd(dma_buf_fd),
                    offset,
                    stride,
                },
            })
        }
    }

    pub fn upload_presentable_image_software_buffer(
        &mut self,
        texture: &Texture,
        width: u32,
        height: u32,
        pixels: &[u8],
    ) {
        let expected_len = width as usize * height as usize * 4;
        if pixels.len() < expected_len {
            crate::error!(
                "software fallback frame too small: got {} bytes, need {}",
                pixels.len(),
                expected_len
            );
            return;
        }
        // Upload pixels without row-flip.  OpenGL FBO content has
        // bottom-left origin; the RunView shader flips Y when sampling
        // (via the y_flip instance), keeping both the DMA-buf and
        // software-fallback paths consistent.
        let opengl_cx = self.os.opengl_cx.as_ref().unwrap();
        opengl_cx.make_current();

        let cxtexture = &mut self.textures[texture.texture_id()];
        cxtexture.update_shared_texture(self.os.gl());

        unsafe {
            let gl = self.os.gl();
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, cxtexture.os.gl_texture.unwrap());
            (gl.glPixelStorei)(gl_sys::UNPACK_ALIGNMENT, 1);
            (gl.glPixelStorei)(gl_sys::UNPACK_ROW_LENGTH, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_PIXELS, 0);
            (gl.glPixelStorei)(gl_sys::UNPACK_SKIP_ROWS, 0);
            (gl.glTexSubImage2D)(
                gl_sys::TEXTURE_2D,
                0,
                0,
                0,
                width as i32,
                height as i32,
                gl_sys::RGBA,
                gl_sys::UNSIGNED_BYTE,
                pixels.as_ptr() as *const c_void,
            );
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
        }
    }
}

impl CxTexture {
    fn update_shared_texture(&mut self, gl: &LibGl) {
        if !self.alloc_shared() {
            return;
        }
        let alloc = self.alloc.as_ref().unwrap();

        // HACK(eddyb) drain error queue, so that we can check erors below.
        while unsafe { (gl.glGetError)() } != 0 {}

        unsafe {
            if self.os.gl_texture.is_none() {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                self.os.gl_texture = Some(gl_texture.assume_init());
            }

            (gl.glBindTexture)(gl_sys::TEXTURE_2D, self.os.gl_texture.unwrap());

            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MIN_FILTER,
                gl_sys::NEAREST as i32,
            );
            (gl.glTexParameteri)(
                gl_sys::TEXTURE_2D,
                gl_sys::TEXTURE_MAG_FILTER,
                gl_sys::NEAREST as i32,
            );
            (gl.glTexImage2D)(
                gl_sys::TEXTURE_2D,
                0,
                gl_sys::RGBA as i32,
                alloc.width as i32,
                alloc.height as i32,
                0,
                gl_sys::RGBA,
                gl_sys::UNSIGNED_BYTE,
                std::ptr::null(),
            );
            assert_eq!(
                (gl.glGetError)(),
                0,
                "glTexImage2D({}, {}) failed",
                alloc.width,
                alloc.height
            );
            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
        }
    }

    pub fn update_from_shared_dma_buf_image(
        &mut self,
        gl: &LibGl,
        opengl_cx: &OpenglCx,
        dma_buf_image: &LinuxOwnedImage,
    ) {
        if !self.alloc_shared() {
            return;
        }
        let alloc = self.alloc.as_ref().unwrap();

        // HACK(eddyb) drain error queue, so that we can check erors below.
        while unsafe { (gl.glGetError)() } != 0 {}
        opengl_cx.make_current();
        while unsafe { (gl.glGetError)() } != 0 {}

        let drm_format = dma_buf_image.drm_format;
        let plane0 = &dma_buf_image.plane;

        let image_attribs = [
            egl_sys::EGL_LINUX_DRM_FOURCC_EXT,
            drm_format.fourcc,
            egl_sys::EGL_WIDTH,
            alloc.width as u32,
            egl_sys::EGL_HEIGHT,
            alloc.height as u32,
            egl_sys::EGL_DMA_BUF_PLANE0_FD_EXT,
            plane0.dma_buf_fd.as_raw_fd() as u32,
            egl_sys::EGL_DMA_BUF_PLANE0_OFFSET_EXT,
            plane0.offset,
            egl_sys::EGL_DMA_BUF_PLANE0_PITCH_EXT,
            plane0.stride,
            egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_LO_EXT,
            drm_format.modifiers as u32,
            egl_sys::EGL_DMA_BUF_PLANE0_MODIFIER_HI_EXT,
            (drm_format.modifiers >> 32) as u32,
            egl_sys::EGL_NONE,
        ];
        let egl_image = unsafe {
            (opengl_cx.libegl.eglCreateImageKHR.unwrap())(
                opengl_cx.egl_display,
                std::ptr::null_mut(),
                egl_sys::EGL_LINUX_DMA_BUF_EXT,
                std::ptr::null_mut(),
                image_attribs.as_ptr() as _,
            )
        };
        assert!(!egl_image.is_null(), "eglCreateImageKHR failed");

        unsafe {
            let gl_texture = *self.os.gl_texture.get_or_insert_with(|| {
                let mut gl_texture = std::mem::MaybeUninit::uninit();
                (gl.glGenTextures)(1, gl_texture.as_mut_ptr());
                assert_eq!((gl.glGetError)(), 0, "glGenTextures failed");
                gl_texture.assume_init()
            });

            (gl.glBindTexture)(gl_sys::TEXTURE_2D, gl_texture);
            assert_eq!((gl.glGetError)(), 0, "glBindTexture({gl_texture}) failed");

            (opengl_cx.libegl.glEGLImageTargetTexture2DOES.unwrap())(gl_sys::TEXTURE_2D, egl_image);
            assert_eq!((gl.glGetError)(), 0, "glEGLImageTargetTexture2DOES failed");

            (gl.glBindTexture)(gl_sys::TEXTURE_2D, 0);
        }
    }
}

#[derive(Clone)]
pub struct OpenglWindow {
    pub first_draw: bool,
    pub window_id: WindowId,
    pub window_geom: WindowGeom,
    pub opening_repaint_count: u32,
    pub cal_size: Vec2d,
    pub xlib_window: Box<XlibWindow>,
    pub egl_surface: egl_sys::EGLSurface,
}

impl OpenglWindow {
    pub fn new(
        window_id: WindowId,
        opengl_cx: &OpenglCx,
        inner_size: Vec2d,
        position: Option<Vec2d>,
        title: &str,
        is_fullscreen: bool,
    ) -> OpenglWindow {
        // Checked "downcast" of the EGL platform display to a X11 display.
        assert_eq!(opengl_cx.egl_platform, egl_sys::EGL_PLATFORM_X11_EXT);
        let display = opengl_cx.egl_platform_display as *mut x11_sys::Display;

        let mut xlib_window = Box::new(XlibWindow::new(window_id));

        // Get X11 visual from EGL configuration.
        let visual_info = unsafe {
            let mut native_visual_id = 0;
            assert!(
                (opengl_cx.libegl.eglGetConfigAttrib.unwrap())(
                    opengl_cx.egl_display,
                    opengl_cx.egl_config,
                    egl_sys::EGL_NATIVE_VISUAL_ID as _,
                    &mut native_visual_id,
                ) != 0,
                "eglGetConfigAttrib(EGL_NATIVE_VISUAL_ID) failed",
            );

            let mut visual_template = mem::zeroed::<x11_sys::XVisualInfo>();
            visual_template.visualid = native_visual_id as _;

            let mut count = 0;
            let visual_info_ptr = x11_sys::XGetVisualInfo(
                display,
                x11_sys::VisualIDMask as c_long,
                &mut visual_template,
                &mut count,
            );
            assert!(
                !visual_info_ptr.is_null() && count == 1,
                "can't get visual from EGL configuration with XGetVisualInfo",
            );

            let visual_info = *visual_info_ptr;
            x11_sys::XFree(visual_info_ptr as *mut c_void);
            visual_info
        };

        let custom_window_chrome = false;
        xlib_window.init(
            title,
            inner_size,
            position,
            is_fullscreen,
            visual_info,
            custom_window_chrome,
        );

        let egl_surface = unsafe {
            (opengl_cx.libegl.eglCreateWindowSurface.unwrap())(
                opengl_cx.egl_display,
                opengl_cx.egl_config,
                xlib_window.window.unwrap(),
                std::ptr::null(),
            )
        };
        assert!(!egl_surface.is_null(), "eglCreateWindowSurface failed");

        OpenglWindow {
            first_draw: true,
            window_id,
            opening_repaint_count: 0,
            cal_size: Vec2d::default(),
            window_geom: xlib_window.get_window_geom(),
            xlib_window,
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
            // resize the framebuffer
            true
        } else {
            false
        }
    }
}
