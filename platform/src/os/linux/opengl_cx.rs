use std::ffi::{c_void, CString};

use crate::{egl_sys::{self, LibEgl}, gl_sys::{self, LibGl}, Cx, PassClearColor, PassClearDepth, PassId};

pub struct OpenglCx {
    pub libegl: LibEgl,
    pub libgl: LibGl,
    pub egl_display: egl_sys::EGLDisplay,
    pub egl_config: egl_sys::EGLConfig,
    pub egl_context: egl_sys::EGLContext,

    pub egl_platform: egl_sys::EGLenum,
    pub egl_platform_display: *mut c_void,
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
            egl_sys::EGL_CONTEXT_MAJOR_VERSION,
            #[cfg(use_gles_3)]
            3,
            #[cfg(not(use_gles_3))]
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

        let libgl = LibGl::try_load(| s | {
            for s in s{
                let s = CString::new(*s).unwrap();
                let p = unsafe{libegl.eglGetProcAddress.unwrap()(s.as_ptr())};
                if !p.is_null(){
                    return p
                }
            }
            0 as * const _
        }).expect("Cant load openGL functions");

        OpenglCx {
            libegl,
            libgl,
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

impl Cx {
        pub fn draw_pass_to_window(
            &mut self,
            pass_id: PassId,
            egl_surface: egl_sys::EGLSurface,
            pix_width: f64,
            pix_height: f64,
        ) {
            let draw_list_id = self.passes[pass_id].main_draw_list_id.unwrap();

            self.setup_render_pass(pass_id);

            self.passes[pass_id].paint_dirty = false;

            let gl = self.os.gl();
            unsafe {
                let opengl_cx = self.os.opengl_cx.as_ref().unwrap();
                (opengl_cx.libegl.eglMakeCurrent.unwrap())(opengl_cx.egl_display, egl_surface, egl_surface, opengl_cx.egl_context);
                (gl.glViewport)(0, 0, pix_width.floor() as i32, pix_height.floor() as i32);
                println!("viewport set to {}x{}", pix_width.floor() as i32, pix_height.floor() as i32);
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
                println!("clear pass for pass {:?}", pass_id);
                unsafe {
                    (gl.glBindFramebuffer)(gl_sys::FRAMEBUFFER, 0);
                    (gl.glClearDepthf)(clear_depth as f32);
                    (gl.glClearColor)(clear_color.x, clear_color.y, clear_color.z, clear_color.w);
                    (gl.glClear)(gl_sys::COLOR_BUFFER_BIT | gl_sys::DEPTH_BUFFER_BIT);
                }
            }
            Cx::set_default_depth_and_blend_mode(self.os.gl());

            let mut zbias = 0.0;
            let zbias_step = self.passes[pass_id].zbias_step;

            self.render_view(
                pass_id,
                draw_list_id,
                &mut zbias,
                zbias_step,
            );

            unsafe {
                let opengl_cx = self.os.opengl_cx.as_ref().unwrap();
                (opengl_cx.libegl.eglSwapBuffers.unwrap())(opengl_cx.egl_display, egl_surface);
            }
        }
}
