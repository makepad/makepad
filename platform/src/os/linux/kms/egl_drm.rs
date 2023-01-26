use {
    std::ffi::{CStr, CString},
    self::super::{
        drm_sys::*,
        gbm_sys::*,
        egl_sys::*,
    },
    self::super::super::{
        gl_sys,
        libc_sys,
    },
};


#[allow(dead_code)]
pub struct Drm {
    pub width: u32,
    pub height: u32,
    crtc_index: i32,
    fourcc_format: u32,
    drm_fd: std::os::raw::c_int,
    drm_mode: drmModeModeInfoPtr,
    drm_resources: drmModeResPtr,
    drm_connector: drmModeConnectorPtr,
    drm_encoder: drmModeEncoderPtr,
    gbm_dev: *mut gbm_device,
    gbm_surface: *mut gbm_surface,
    gbm_bo: Option<*mut gbm_bo>,
}

impl Drm {
    pub unsafe fn new(mode_want: &str) -> Option<Self> {
        let fourcc_format = GBM_FORMAT_XRGB8888;
        
        let mut drm_devices: [drmDevicePtr; MAX_DRM_DEVICES] = std::mem::zeroed();
        let num_devices = drmGetDevices2(0, drm_devices.as_mut_ptr(), MAX_DRM_DEVICES as _) as usize;
        
        let mut found_drm = None;
        'outer: for i in 0..num_devices {
            let drm_device = *drm_devices[i];
            if drm_device.available_nodes & (1 << DRM_NODE_PRIMARY) == 0 {
                continue;
            }
            // alright lets get the resources
            let drm_fd = libc_sys::open(*drm_device.nodes.offset(DRM_NODE_PRIMARY as _), libc_sys::O_RDWR);
            let drm_resources = drmModeGetResources(drm_fd);
            if drm_resources == std::ptr::null_mut() {
                libc_sys::close(drm_fd);
                continue;
            }
            for j in 0..(*drm_resources).count_connectors {
                let connector_idx = *(*drm_resources).connectors.offset(j as _);
                let drm_connector = drmModeGetConnector(drm_fd, connector_idx);
                if drm_connector == std::ptr::null_mut() {
                    libc_sys::close(drm_fd);
                    continue;
                }
                if (*drm_connector).connection == DRM_MODE_CONNECTED {
                    found_drm = Some((drm_fd, drm_resources, drm_connector));
                    break 'outer;
                }
                drmModeFreeConnector(drm_connector);
            }
            drmModeFreeResources(drm_resources);
        }
        if found_drm.is_none() {
            return None
        }
        let (drm_fd, drm_resources, drm_connector) = found_drm.unwrap();
        
        // find a mode
        let mut found_drm_mode = None;
        for i in 0..(*drm_connector).count_modes {
            let drm_mode = (*drm_connector).modes.offset(i as _);
            let name = CStr::from_ptr((*drm_mode).name.as_ptr()).to_str().unwrap();
            let mode_name = format!("{}-{}", name, (*drm_mode).vrefresh);
            //println!("{}", mode_name);
            if mode_name == mode_want {
                found_drm_mode = Some(drm_mode);
            }
        }
        if found_drm_mode.is_none() {
            drmModeFreeConnector(drm_connector);
            drmModeFreeResources(drm_resources);
            return None
        }
        
        // find encoder
        let mut found_drm_encoder = None;
        for i in 0..(*drm_resources).count_encoders {
            let drm_encoder = drmModeGetEncoder(drm_fd, *(*drm_resources).encoders.offset(i as _));
            if (*drm_encoder).encoder_id == (*drm_connector).encoder_id {
                found_drm_encoder = Some(drm_encoder);
                break;
            }
            drmModeFreeEncoder(drm_encoder);
        }
        if found_drm_encoder.is_none() {
            drmModeFreeConnector(drm_connector);
            drmModeFreeResources(drm_resources);
            return None
        }
        println!("FOUND ENCODER");
        let drm_encoder = found_drm_encoder.unwrap();
        let mut crtc_index = 0;
        for i in 0..(*drm_resources).count_crtcs {
            if *(*drm_resources).crtcs.offset(i as _) == (*drm_encoder).crtc_id {
                crtc_index = i;
            }
        }
        
        let drm_mode = found_drm_mode.unwrap();
        
        // init gbm
        let width = (*drm_mode).hdisplay as u32;
        let height = (*drm_mode).vdisplay as u32;
        
        println!("CREATING DEVICE");
        let gbm_dev = gbm_create_device(drm_fd);

        if gbm_dev == std::ptr::null_mut() {
            println!("Cannot create gbm device");
            return None
        }
        
        println!("CREATING SURFACE");
        let gbm_surface = gbm_surface_create(
            gbm_dev,
            width,
            height,
            fourcc_format,
            GBM_BO_USE_SCANOUT | GBM_BO_USE_RENDERING
        );
        
        if gbm_surface == std::ptr::null_mut() {
            println!("Cannot create gbm surface");
            return None
        }
        println!("Initialized drm/gbm at {} {}", width, height);
        
        Some(Drm {
            gbm_bo: None,
            width,
            height,
            fourcc_format,
            crtc_index,
            drm_encoder,
            drm_mode,
            drm_fd,
            drm_resources,
            drm_connector,
            gbm_dev,
            gbm_surface
        })
    }
    
    pub unsafe fn set_mode(&mut self) {
        let bo = gbm_surface_lock_front_buffer(self.gbm_surface);
        let handle = gbm_bo_get_handle(bo);
        let stride = gbm_bo_get_stride(bo);
        let mut fb_id = 0;
        
        if drmModeAddFB2(
            self.drm_fd,
            self.width,
            self.height,
            self.fourcc_format,
            [handle.u32_, 0, 0, 0].as_ptr(),
            [stride, 0, 0, 0].as_ptr(),
            [0, 0, 0, 0].as_ptr(),
            &mut fb_id,
            0
        ) != 0 {
            println!("Error running drmModeAddFB2");
            return
        }
        
        self.gbm_bo = Some(bo);
        
        println!("GOT FB {}", fb_id);
        
        let mut connector_id = (*self.drm_connector).connector_id;
        
        let ret = drmModeSetCrtc(
            self.drm_fd,
            self.crtc_index as _,
            fb_id,
            0,
            0,
            &mut connector_id,
            1,
            self.drm_mode
        );
        if ret != 0 {
            println!("Error running drmModeSetCrtc {}", ret);
            return
        }
        // lets get a fb_id
        //let handles =
        //drmModeAddFB2()
        
        //let fb = drm_fb_get_from_bo(bo);
    }
    
    pub unsafe fn flip_buffers_and_wait(&mut self, egl: &Egl){
        //egl.swap_buffers();
        //let next_bo = gbm_surface_lock_front_buffer(self.gbm_surface);
        
    }
}

#[allow(non_snake_case)]
#[allow(dead_code)]
pub struct Egl {
    egl_display: EGLDisplay,
    egl_surface: EGLSurface,
    egl_context: EGLContext,
    eglCreateImageKHR: PFNEGLCREATEIMAGEKHRPROC,
}

impl Egl {
    pub unsafe fn new(drm: &Drm) -> Option<Self> {
        let mut major = 0;
        let mut minor = 0;
        
        #[allow(non_snake_case)]
        let eglGetPlatformDisplayEXT: PFNEGLGETPLATFORMDISPLAYEXTPROC = std::mem::transmute(eglGetProcAddress("eglGetPlatformDisplayEXT\0".as_ptr()));
        #[allow(non_snake_case)]
        let eglInitialize: PFNEGLINITIALIZEPROC = std::mem::transmute(eglGetProcAddress("eglInitialize\0".as_ptr()));
        #[allow(non_snake_case)]
        let eglGetConfigs: PFNEGLGETCONFIGSPROC = std::mem::transmute(eglGetProcAddress("eglGetConfigs\0".as_ptr()));
        #[allow(non_snake_case)]
        let eglChooseConfig: PFNEGLCHOOSECONFIGPROC = std::mem::transmute(eglGetProcAddress("eglChooseConfig\0".as_ptr()));
        #[allow(non_snake_case)]
        let eglGetConfigAttrib: PFNEGLGETCONFIGATTRIBPROC = std::mem::transmute(eglGetProcAddress("eglGetConfigAttrib\0".as_ptr()));
        #[allow(non_snake_case)]
        let eglCreateImageKHR: PFNEGLCREATEIMAGEKHRPROC = std::mem::transmute(eglGetProcAddress("eglCreateImageKHR\0".as_ptr()));
        
        let egl_display = (eglGetPlatformDisplayEXT.unwrap())(EGL_PLATFORM_GBM_KHR, drm.gbm_dev as *mut _, std::ptr::null());
        if egl_display == std::ptr::null_mut() {
            println!("Could not get platform display");
            return None
        }
        
        if (eglInitialize.unwrap())(egl_display, &mut major, &mut minor) == 0 {
            println!("Could not initialize egl");
            return None;
        }
        
        println!("Initialized EGL version {}.{}", major, minor);
        
        if eglBindAPI(EGL_OPENGL_ES_API) == 0 {
            println!("Could not bind EGL_OPENGL_ES_API");
            return None;
        }
        
        let mut cfg_count = 0;
        if (eglGetConfigs.unwrap())(egl_display, std::ptr::null_mut(), 0, &mut cfg_count) == 0 || cfg_count == 0 {
            println!("eglGetConfigs failed");
            return None;
        };
        
        let cfg_attribs = [
            EGL_SURFACE_TYPE,
            EGL_WINDOW_BIT,
            EGL_RED_SIZE,
            1,
            EGL_GREEN_SIZE,
            1,
            EGL_BLUE_SIZE,
            1,
            EGL_ALPHA_SIZE,
            0,
            EGL_RENDERABLE_TYPE,
            EGL_OPENGL_ES2_BIT,
            EGL_NONE
        ];
        
        let mut configs: Vec<EGLConfig> = Vec::new();
        configs.resize(cfg_count as usize, 0 as EGLConfig);
        
        let mut matched = 0;
        if (eglChooseConfig.unwrap())(
            egl_display,
            cfg_attribs.as_ptr(),
            configs.as_mut_ptr(),
            cfg_count,
            &mut matched
        ) == 0
            || matched == 0 {
            println!("eglChooseConfig failed");
            return None;
        }
        
        // find the native visual config
        let mut egl_config = None;
        for i in 0..cfg_count as usize {
            let mut native_id = 0;
            if (eglGetConfigAttrib.unwrap())(egl_display, configs[i], EGL_NATIVE_VISUAL_ID, &mut native_id) == 0 {
                continue;
            }
            if native_id == drm.fourcc_format {
                egl_config = Some(configs[i]);
                break;
            }
        }
        
        if egl_config.is_none() {
            println!("eglGetConfigAttrib cannot match native id");
            return None;
        }
        let egl_config = egl_config.unwrap();
        
        let ctx_attribs = [
            EGL_CONTEXT_CLIENT_VERSION,
            2,
            EGL_NONE
        ];
        
        let egl_context = eglCreateContext(egl_display, egl_config, EGL_NO_CONTEXT, ctx_attribs.as_ptr());
        if egl_context == std::ptr::null_mut() {
            println!("eglCreateContext failed");
            return None;
        }
        
        let egl_surface = eglCreateWindowSurface(egl_display, egl_config, drm.gbm_surface as _, std::ptr::null());
        if egl_surface == std::ptr::null_mut() {
            println!("eglCreateWindowSurface failed");
            return None;
        }
        
        if eglMakeCurrent(egl_display, egl_surface, egl_surface, egl_context) == 0 {
            println!("eglMakeCurrent failed");
            return None;
        }
        
        gl_sys::load_with( | s | {
            let s = CString::new(s).unwrap();
            unsafe {eglGetProcAddress(s.as_ptr())}
        });
        
        Some(Self {
            eglCreateImageKHR,
            egl_display,
            egl_surface,
            egl_context
        })
    }
    
    pub fn make_current(&self) {
        if unsafe {eglMakeCurrent(self.egl_display, self.egl_surface, self.egl_surface, self.egl_context)} == 0 {
            println!("eglMakeCurrent failed");
        }
    }
    
    pub fn swap_buffers(&self) {
        if unsafe {eglSwapBuffers(self.egl_display, self.egl_surface)} == 0 {
            println!("eglSwapBuffers failed")
        }
    }
}
