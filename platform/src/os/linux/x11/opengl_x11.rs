use {
    std::{
        mem,
        os::raw::{c_ulong, c_void},
        ptr,
        ffi::{CStr, CString},
    },
    self::super::{
        glx_sys,
        x11_sys,
        xlib_window::XlibWindow,
    },
    self::super::super::{ 
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
        
        let window = opengl_window.xlib_window.window.unwrap();
        
        self.passes[pass_id].paint_dirty = false;
         
        let pix_width = opengl_window.window_geom.inner_size.x * opengl_window.window_geom.dpi_factor;
        let pix_height = opengl_window.window_geom.inner_size.y * opengl_window.window_geom.dpi_factor;
        unsafe {
            glx_sys::glXMakeCurrent(opengl_cx.display, window, opengl_cx.context);
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
            glx_sys::glXSwapBuffers(opengl_cx.display, window);
        }
    }
}

pub struct OpenglCx {
    pub display: *mut x11_sys::Display,
    pub context: glx_sys::GLXContext,
    pub visual_info: x11_sys::XVisualInfo,
    pub hidden_window: x11_sys::Window,
}

impl OpenglCx {
    pub fn new(display: *mut x11_sys::Display) -> OpenglCx {
        unsafe {
            // Query GLX version.
            let mut major = 0;
            let mut minor = 0;
            assert!(
                glx_sys::glXQueryVersion(display, &mut major, &mut minor) >= 0,
                "can't query GLX version"
            );
            
            // Check that GLX version number is 1.4 or higher.
            assert!(
                major > 1 || major == 1 && minor >= 4,
                "GLX version must be 1.4 or higher, got {}.{}",
                major,
                minor,
            );
            
            let screen = x11_sys::XDefaultScreen(display);
            
            // Query extensions string
            let supported_extensions = glx_sys::glXQueryExtensionsString(display, screen);
            assert!(
                !supported_extensions.is_null(),
                "can't query GLX extensions string"
            );
            let supported_extensions = CStr::from_ptr(supported_extensions).to_str().unwrap();
            
            // Check that required extensions are supported.
            let required_extensions = &["GLX_ARB_get_proc_address", "GLX_ARB_create_context"];
            for required_extension in required_extensions {
                assert!(
                    supported_extensions.contains(required_extension),
                    "extension {} is required, but not supported",
                    required_extension,
                );
            }
            
            // Load GLX function pointers.
            #[allow(non_snake_case)]
            let glXCreateContextAttribsARB = mem::transmute::<
                _,
                glx_sys::PFNGLXCREATECONTEXTATTRIBSARBPROC,
            >(glx_sys::glXGetProcAddressARB(
                CString::new("glXCreateContextAttribsARB")
                    .unwrap()
                    .to_bytes_with_nul()
                    .as_ptr(),
            ))
                .expect("can't load glXCreateContextAttribsARB function pointer");
            
            // Load GL function pointers.
            gl_sys::load_with( | symbol | {
                glx_sys::glXGetProcAddressARB(
                    CString::new(symbol).unwrap().to_bytes_with_nul().as_ptr(),
                )
                    .map_or(ptr::null(), | ptr | ptr as *const c_void)
            });
            
            // Choose framebuffer configuration.
            let config_attribs = &[
                glx_sys::GLX_DOUBLEBUFFER as i32,
                glx_sys::True as i32,
                glx_sys::GLX_RED_SIZE as i32,
                8,
                glx_sys::GLX_GREEN_SIZE as i32,
                8,
                glx_sys::GLX_BLUE_SIZE as i32,
                8,
                //glx_sys::GLX_ALPHA_SIZE as i32,
                //8,
                glx_sys::GLX_DEPTH_SIZE as i32,
                24,
                glx_sys::None as i32,
            ];
            let mut config_count = 0;
            let configs = glx_sys::glXChooseFBConfig(
                display,
                x11_sys::XDefaultScreen(display),
                config_attribs.as_ptr(),
                &mut config_count,
            );
            if configs.is_null() {
                panic!("can't choose framebuffer configuration");
            }
            let config = *configs;
            x11_sys::XFree(configs as *mut c_void);
            
            // Create GLX context.
            let context_attribs = &[
                glx_sys::GLX_CONTEXT_MAJOR_VERSION_ARB as i32,
                3,
                glx_sys::GLX_CONTEXT_MINOR_VERSION_ARB as i32,
                0,
                glx_sys::GLX_CONTEXT_PROFILE_MASK_ARB as i32,
                glx_sys::GLX_CONTEXT_ES_PROFILE_BIT_EXT as i32,
                glx_sys::None as i32
            ];
            let context = glXCreateContextAttribsARB(
                display,
                config,
                ptr::null_mut(),
                glx_sys::True as i32,
                context_attribs.as_ptr(),
            );
            
            // Get visual from framebuffer configuration.
            let visual_info_ptr = glx_sys::glXGetVisualFromFBConfig(display, config);
            assert!(
                !visual_info_ptr.is_null(),
                "can't get visual from framebuffer configuration"
            );
            let visual_info = *visual_info_ptr;
            x11_sys::XFree(visual_info_ptr as *mut c_void);
            
            let root_window = x11_sys::XRootWindow(display, screen);
            
            // Create hidden window compatible with visual
            //
            // We need a hidden window because we sometimes want to create OpenGL resources, such as
            // shaders, when Makepad does not have any windows open. In cases such as these, we need
            // *some* window to make the OpenGL context current on.
            let mut attributes = mem::zeroed::<x11_sys::XSetWindowAttributes>();
            
            // We need a color map that is compatible with our visual. Otherwise, the call to
            // XCreateWindow below will fail.
            attributes.colormap = x11_sys::XCreateColormap(
                display,
                root_window,
                visual_info.visual,
                x11_sys::AllocNone as i32
            );
            let hidden_window = x11_sys::XCreateWindow(
                display,
                root_window,
                0,
                0,
                16,
                16,
                0,
                visual_info.depth,
                x11_sys::InputOutput as u32,
                visual_info.visual,
                x11_sys::CWColormap as c_ulong,
                &mut attributes,
            );
            
            // To make sure the window stays hidden, we simply never call XMapWindow on it.
            
            OpenglCx {
                display,
                context,
                visual_info,
                hidden_window,
            }
        }
    }

    pub fn make_current(&self){
        unsafe {glx_sys::glXMakeCurrent(self.display, self.hidden_window, self.context);}
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
}

impl OpenglWindow {
    pub fn new(
        window_id: WindowId,
        opengl_cx: &OpenglCx,
        inner_size: DVec2,
        position: Option<DVec2>,
        title: &str
    ) -> OpenglWindow {
        
        let mut xlib_window = Box::new(XlibWindow::new(window_id));
        
        let visual_info = unsafe {mem::transmute(opengl_cx.visual_info)};
        let custom_window_chrome = false;
        xlib_window.init(title, inner_size, position, visual_info, custom_window_chrome);
        
        OpenglWindow {
            first_draw: true,
            window_id,
            opening_repaint_count: 0,
            cal_size: DVec2::default(),
            window_geom: xlib_window.get_window_geom(),
            xlib_window
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
