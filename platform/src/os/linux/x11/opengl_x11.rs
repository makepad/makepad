use {
    std::{
        mem,
        os::raw::{c_long, c_void},
        ffi::{CString},
    },
    self::super::{
        x11_sys,
        xlib_window::XlibWindow,
    },
    self::super::super::{
        egl_sys::{self, LibEgl},
        gl_sys,
    },
    crate::{
        cx::Cx,
        window::WindowId,
        makepad_math::{DVec2},
        pass::{PassClearColor, PassClearDepth, PassId},
        event::*,
    },
};

impl Cx {
    
    pub fn draw_pass_to_window(
        &mut self,
        pass_id: PassId,
        opengl_window: &mut OpenglWindow,
        opengl_cx: &OpenglCx,
    ) {
        let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();
        
        self.setup_render_pass(pass_id);
        
        let egl_surface = opengl_window.egl_surface;
        
        self.passes[pass_id].paint_dirty = false;
         
        let pix_width = opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor;
        let pix_height = opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor;
        unsafe {
            (opengl_cx.libegl.eglMakeCurrent.unwrap())(opengl_cx.egl_display, egl_surface, egl_surface, opengl_cx.egl_context);
            gl_sys::Viewport(0, 0, pix_width.floor() as i32, pix_height.floor() as i32);
        }
        
        let clear_color = if self.passes[pass_id].color_textures.len() == 0 {
            self.passes[pass_id].clear_color 
        }
        else {
            match self.passes[pass_id].color_textures[0].clear_color {
                PassClearColor::InitWith(color) => color,
                PassClearColor::ClearWith(color) => color
            }
        };
        let clear_depth = match self.passes[pass_id].clear_depth {
            PassClearDepth::InitWith(depth) => depth,
            PassClearDepth::ClearWith(depth) => depth
        };
        
        if !self.passes[pass_id].dont_clear {
            unsafe {
                gl_sys::BindFramebuffer(gl_sys::FRAMEBUFFER, 0);
                gl_sys::ClearDepthf(clear_depth as f32);
                gl_sys::ClearColor(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                gl_sys::Clear(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
            }
        }
        Self::set_default_depth_and_blend_mode();
        
        let mut zbias = 0.0;
        let zbias_step = self.passes[pass_id].zbias_step;
        
        self.render_view(
            pass_id,
            draw_list_id,
            &mut zbias,
            zbias_step,
        );
        
        unsafe {
            (opengl_cx.libegl.eglSwapBuffers.unwrap())(opengl_cx.egl_display, egl_surface);
        }
    }
}

// FIXME(eddyb) move this out of `linux::x11`, since it's mostly generic EGL.
pub struct OpenglCx {
    libegl: LibEgl,
    egl_display: egl_sys::EGLDisplay,
    egl_config: egl_sys::EGLConfig,
    egl_context: egl_sys::EGLContext,

    egl_platform: egl_sys::EGLenum,
    egl_platform_display: *mut c_void,
}

impl OpenglCx {
    pub unsafe fn from_egl_platform_display<T>(
        egl_platform: egl_sys::EGLenum,
        egl_platform_display: *mut T,
    ) -> OpenglCx {
        let egl_platform_display = egl_platform_display as *mut c_void;

        // Load EGL function pointers.
        let libegl = LibEgl::try_load().expect("can't load LibEGL");

        let mut major = 0;
        let mut minor = 0;

        let egl_display = (libegl.eglGetPlatformDisplayEXT.unwrap())(
            egl_platform,
            egl_platform_display,
            std::ptr::null(),
        );
        assert!(!egl_display.is_null(), "can't get EGL platform display");

        assert!(
            (libegl.eglInitialize.unwrap())(egl_display, &mut major, &mut minor) != 0,
            "can't initialize EGL",
        );

        assert!(
            (libegl.eglBindAPI.unwrap())(egl_sys::EGL_OPENGL_ES_API) != 0,
            "can't bind EGL_OPENGL_ES_API",
        );

        // Choose framebuffer configuration.
        let cfg_attribs = [
            egl_sys::EGL_RED_SIZE,
            8,
            egl_sys::EGL_GREEN_SIZE,
            8,
            egl_sys::EGL_BLUE_SIZE,
            8,
            egl_sys::EGL_ALPHA_SIZE,
            8,
            // egl_sys::EGL_DEPTH_SIZE,
            // 24,
            // egl_sys::EGL_STENCIL_SIZE,
            // 8,
            egl_sys::EGL_RENDERABLE_TYPE,
            egl_sys::EGL_OPENGL_ES2_BIT,
            egl_sys::EGL_NONE
        ];

        let mut egl_config = 0 as egl_sys::EGLConfig;
        let mut matched_egl_configs = 0;
        assert!(
            (libegl.eglChooseConfig.unwrap())(
                egl_display,
                cfg_attribs.as_ptr() as _,
                &mut egl_config,
                1,
                &mut matched_egl_configs
            ) != 0 && matched_egl_configs == 1,
            "eglChooseConfig failed",
        );

        // Create EGL context.
        let ctx_attribs = [
            egl_sys::EGL_CONTEXT_CLIENT_VERSION,
            2,
            egl_sys::EGL_NONE
        ];

        let egl_context = (libegl.eglCreateContext.unwrap())(
            egl_display,
            egl_config,
            egl_sys::EGL_NO_CONTEXT,
            ctx_attribs.as_ptr() as _,
        );
        assert!(!egl_context.is_null(), "eglCreateContext failed");

        // Load GL function pointers.
        gl_sys::load_with(|symbol| {
            let s = CString::new(symbol).unwrap();
            (libegl.eglGetProcAddress.unwrap())(s.as_ptr())
        });

        OpenglCx {
            libegl,
            egl_display,
            egl_config,
            egl_context,

            egl_platform,
            egl_platform_display,
        }
    }

    pub fn make_current(&self) {
        unsafe {
            (self.libegl.eglMakeCurrent.unwrap())(
                self.egl_display,
                egl_sys::EGL_NO_SURFACE,
                egl_sys::EGL_NO_SURFACE,
                self.egl_context,
            );
        }
    }
}

#[derive(Clone)]
pub struct OpenglWindow {
    pub first_draw: bool,
    pub window_id: WindowId,
    pub window_geom: WindowGeom,
    pub opening_repaint_count: u32,
    pub cal_size: DVec2,
    pub xlib_window: Box<XlibWindow>,
    pub egl_surface: egl_sys::EGLSurface,
}

impl OpenglWindow {
    pub fn new(
        window_id: WindowId,
        opengl_cx: &OpenglCx,
        inner_size: DVec2,
        position: Option<DVec2>,
        title: &str
    ) -> OpenglWindow {
        // Checked "downcast" of the EGL platform display to a X11 display.
        assert_eq!(opengl_cx.egl_platform, egl_sys::EGL_PLATFORM_X11_EXT);
        let display = opengl_cx. egl_platform_display as *mut x11_sys::Display;

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
        xlib_window.init(title, inner_size, position, visual_info, custom_window_chrome);

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
            cal_size: DVec2::default(),
            window_geom: xlib_window.get_window_geom(),
            xlib_window,
            egl_surface,
        }
    }
    
    pub fn resize_buffers(&mut self, _opengl_cx: &OpenglCx) -> bool {
        let cal_size = DVec2 {
            x: self.window_geom.inner_size.x * self.window_geom.dpi_factor,
            y: self.window_geom.inner_size.y * self.window_geom.dpi_factor
        };
        if self.cal_size != cal_size {
            self.cal_size = cal_size;
            // resize the framebuffer
            true
        }
        else {
            false
        }
    }
    
}
