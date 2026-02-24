use {
    crate::{
        cx::Cx,
        texture::{Texture, TextureFormat, TextureSize},
    },
    std::ffi::{c_void, CString},
};

// EGL types
type EGLBoolean = u32;
type EGLint = i32;
type EGLenum = u32;
type EGLAttrib = isize;
type EGLDisplay = *mut c_void;
type EGLConfig = *mut c_void;
type EGLContext = *mut c_void;
type EGLSurface = *mut c_void;
type EGLDeviceEXT = *mut c_void;

// EGL constants
const EGL_TRUE: EGLBoolean = 1;
const EGL_NONE: EGLint = 0x3038;
const EGL_NO_CONTEXT: EGLContext = std::ptr::null_mut();
const EGL_NO_SURFACE: EGLSurface = std::ptr::null_mut();
const EGL_OPENGL_ES_API: EGLenum = 0x30A0;
const EGL_OPENGL_ES2_BIT: EGLint = 0x0004;
const EGL_RED_SIZE: EGLint = 0x3024;
const EGL_GREEN_SIZE: EGLint = 0x3025;
const EGL_BLUE_SIZE: EGLint = 0x3026;
const EGL_ALPHA_SIZE: EGLint = 0x3028;
const EGL_DEPTH_SIZE: EGLint = 0x3025;
const EGL_STENCIL_SIZE: EGLint = 0x3026;
const EGL_RENDERABLE_TYPE: EGLint = 0x3040;
const EGL_CONTEXT_MAJOR_VERSION: EGLint = 0x3098;
const EGL_CONTEXT_MINOR_VERSION: EGLint = 0x30FB;

// ANGLE extension constants
const EGL_D3D11_DEVICE_ANGLE: EGLenum = 0x33A1;
const EGL_PLATFORM_DEVICE_EXT: EGLenum = 0x313F;
const EGL_D3D11_TEXTURE_ANGLE: EGLenum = 0x3484;

// GL constants
const GL_TEXTURE_2D: u32 = 0x0DE1;

// EGL function pointer types
type FnGetProcAddress = unsafe extern "system" fn(*const i8) -> *const c_void;
type FnGetPlatformDisplayEXT = unsafe extern "system" fn(EGLenum, *mut c_void, *const EGLint) -> EGLDisplay;
type FnInitialize = unsafe extern "system" fn(EGLDisplay, *mut EGLint, *mut EGLint) -> EGLBoolean;
type FnTerminate = unsafe extern "system" fn(EGLDisplay) -> EGLBoolean;
type FnBindAPI = unsafe extern "system" fn(EGLenum) -> EGLBoolean;
type FnChooseConfig = unsafe extern "system" fn(EGLDisplay, *const EGLint, *mut EGLConfig, EGLint, *mut EGLint) -> EGLBoolean;
type FnCreateContext = unsafe extern "system" fn(EGLDisplay, EGLConfig, EGLContext, *const EGLint) -> EGLContext;
type FnDestroyContext = unsafe extern "system" fn(EGLDisplay, EGLContext) -> EGLBoolean;
type FnMakeCurrent = unsafe extern "system" fn(EGLDisplay, EGLSurface, EGLSurface, EGLContext) -> EGLBoolean;
type FnCreateDeviceANGLE = unsafe extern "system" fn(EGLenum, *mut c_void, *const EGLAttrib) -> EGLDeviceEXT;
type FnReleaseDeviceANGLE = unsafe extern "system" fn(EGLDeviceEXT) -> EGLBoolean;

/// EGL function table loaded from libEGL.dll at runtime.
struct EglFns {
    get_proc_address: FnGetProcAddress,
    get_platform_display_ext: FnGetPlatformDisplayEXT,
    initialize: FnInitialize,
    terminate: FnTerminate,
    bind_api: FnBindAPI,
    choose_config: FnChooseConfig,
    create_context: FnCreateContext,
    destroy_context: FnDestroyContext,
    make_current: FnMakeCurrent,
    create_device_angle: FnCreateDeviceANGLE,
    release_device_angle: FnReleaseDeviceANGLE,
}

impl EglFns {
    /// Load EGL functions from libEGL.dll.
    ///
    /// The DLL is expected to be in the executable directory (shipped by servo/havi
    /// via mozangle's build_dlls feature). The library handle is intentionally leaked
    /// so the DLL stays loaded for the process lifetime.
    fn load() -> Self {
        use windows::Win32::System::LibraryLoader::{LoadLibraryA, GetProcAddress};
        use windows::core::PCSTR;

        let module = unsafe { LoadLibraryA(PCSTR::from_raw(b"libEGL.dll\0".as_ptr())) }
            .expect("Failed to load libEGL.dll — ANGLE DLLs must be present");

        let get = |name: &str| -> *const c_void {
            let cname = CString::new(name).unwrap();
            let ptr = unsafe { GetProcAddress(module, PCSTR::from_raw(cname.as_ptr() as *const u8)) };
            match ptr {
                Some(f) => f as *const c_void,
                None => panic!("libEGL.dll missing symbol: {name}"),
            }
        };

        let get_proc_address: FnGetProcAddress = unsafe { std::mem::transmute(get("eglGetProcAddress")) };

        // Some ANGLE extension functions are only available via eglGetProcAddress.
        let get_ext = |name: &str| -> *const c_void {
            let cname = CString::new(name).unwrap();
            let ptr = unsafe { (get_proc_address)(cname.as_ptr()) };
            assert!(!ptr.is_null(), "eglGetProcAddress returned null for: {name}");
            ptr
        };

        EglFns {
            get_proc_address,
            get_platform_display_ext: unsafe { std::mem::transmute(get_ext("eglGetPlatformDisplayEXT")) },
            initialize: unsafe { std::mem::transmute(get("eglInitialize")) },
            terminate: unsafe { std::mem::transmute(get("eglTerminate")) },
            bind_api: unsafe { std::mem::transmute(get("eglBindAPI")) },
            choose_config: unsafe { std::mem::transmute(get("eglChooseConfig")) },
            create_context: unsafe { std::mem::transmute(get("eglCreateContext")) },
            destroy_context: unsafe { std::mem::transmute(get("eglDestroyContext")) },
            make_current: unsafe { std::mem::transmute(get("eglMakeCurrent")) },
            create_device_angle: unsafe { std::mem::transmute(get_ext("eglCreateDeviceANGLE")) },
            release_device_angle: unsafe { std::mem::transmute(get_ext("eglReleaseDeviceANGLE")) },
        }
    }

    /// Resolve an EGL/GL extension function by name.
    fn get_proc_address(&self, name: &str) -> *const c_void {
        let cname = CString::new(name).unwrap();
        unsafe { (self.get_proc_address)(cname.as_ptr()) }
    }
}

/// ANGLE-based EGL render bridge on Windows.
/// Creates an EGL display on top of makepad's D3D11 device.
/// Loads ANGLE from libEGL.dll / libGLESv2.dll at runtime (shipped by servo).
pub struct AngleRenderBridge {
    egl: EglFns,
    egl_device: EGLDeviceEXT,
    egl_display: EGLDisplay,
    egl_config: EGLConfig,
    egl_context: EGLContext,
    _d3d11_device: windows::Win32::Graphics::Direct3D11::ID3D11Device,
}

impl AngleRenderBridge {
    pub fn new(d3d11_device: windows::Win32::Graphics::Direct3D11::ID3D11Device) -> Self {
        let egl = EglFns::load();

        unsafe {
            // Create EGL device from D3D11 device
            let d3d11_raw = std::mem::transmute_copy::<
                windows::Win32::Graphics::Direct3D11::ID3D11Device,
                *mut c_void,
            >(&d3d11_device);
            let egl_device = (egl.create_device_angle)(
                EGL_D3D11_DEVICE_ANGLE,
                d3d11_raw,
                std::ptr::null(),
            );
            assert!(!egl_device.is_null(), "eglCreateDeviceANGLE failed");

            // Create EGL display from device
            let display_attribs: &[EGLint] = &[EGL_NONE];
            let egl_display = (egl.get_platform_display_ext)(
                EGL_PLATFORM_DEVICE_EXT,
                egl_device,
                display_attribs.as_ptr(),
            );
            assert!(!egl_display.is_null(), "eglGetPlatformDisplayEXT failed");

            // Initialize
            let mut major: EGLint = 0;
            let mut minor: EGLint = 0;
            let ok = (egl.initialize)(egl_display, &mut major, &mut minor);
            assert_eq!(ok, EGL_TRUE, "eglInitialize failed");

            // Bind OpenGL ES API
            (egl.bind_api)(EGL_OPENGL_ES_API);

            // Choose config
            let config_attribs: &[EGLint] = &[
                EGL_RED_SIZE, 8,
                EGL_GREEN_SIZE, 8,
                EGL_BLUE_SIZE, 8,
                EGL_ALPHA_SIZE, 8,
                EGL_DEPTH_SIZE, 24,
                EGL_STENCIL_SIZE, 8,
                EGL_RENDERABLE_TYPE, EGL_OPENGL_ES2_BIT,
                EGL_NONE,
            ];

            let mut egl_config: EGLConfig = std::ptr::null_mut();
            let mut num_configs: EGLint = 0;
            let ok = (egl.choose_config)(
                egl_display,
                config_attribs.as_ptr(),
                &mut egl_config,
                1,
                &mut num_configs,
            );
            assert_eq!(ok, EGL_TRUE, "eglChooseConfig failed");
            assert!(num_configs > 0, "No matching EGL configs");

            // Create GLES 3.0 context
            let context_attribs: &[EGLint] = &[
                EGL_CONTEXT_MAJOR_VERSION, 3,
                EGL_CONTEXT_MINOR_VERSION, 0,
                EGL_NONE,
            ];

            let egl_context = (egl.create_context)(
                egl_display,
                egl_config,
                EGL_NO_CONTEXT,
                context_attribs.as_ptr(),
            );
            assert!(!egl_context.is_null(), "eglCreateContext failed");

            AngleRenderBridge {
                egl,
                egl_device,
                egl_display,
                egl_config,
                egl_context,
                _d3d11_device: d3d11_device,
            }
        }
    }

    pub fn make_current(&self) {
        unsafe {
            let ok = (self.egl.make_current)(
                self.egl_display,
                EGL_NO_SURFACE,
                EGL_NO_SURFACE,
                self.egl_context,
            );
            assert_eq!(ok, EGL_TRUE, "eglMakeCurrent failed");
        }
    }

    pub fn get_proc_address(&self, name: &str) -> *const c_void {
        self.egl.get_proc_address(name)
    }

    pub fn gl_api(&self) -> crate::gl_render_bridge::GlApi {
        crate::gl_render_bridge::GlApi::GLES
    }

    pub fn egl_display(&self) -> *mut c_void {
        self.egl_display
    }

    pub fn egl_config(&self) -> *mut c_void {
        self.egl_config
    }

    pub fn egl_context(&self) -> *mut c_void {
        self.egl_context
    }

    /// Create a D3D11 render target texture and import it into ANGLE as a GL texture.
    /// The D3D11 texture is stored in the makepad Texture's CxOsTexture.
    /// Returns (Texture, GL texture ID).
    pub fn create_render_texture(
        &self,
        cx: &mut Cx,
        width: usize,
        height: usize,
    ) -> (Texture, u32) {
        // Make ANGLE context current
        self.make_current();

        // Create a makepad render target texture (D3D11 texture with RTV + SRV)
        let texture = Texture::new_with_format(cx, TextureFormat::RenderBGRAu8 {
            size: TextureSize::Fixed { width, height },
            initial: true,
        });

        // Force allocation of the D3D11 texture
        {
            let d3d11_device = cx.os.d3d11_device.as_ref().unwrap();
            let cxtexture = &mut cx.textures[texture.texture_id()];

            use windows::Win32::Graphics::Direct3D11::{
                D3D11_TEXTURE2D_DESC, D3D11_BIND_RENDER_TARGET, D3D11_BIND_SHADER_RESOURCE,
                D3D11_USAGE_DEFAULT, ID3D11Resource,
            };
            use windows::Win32::Graphics::Dxgi::Common::{
                DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_SAMPLE_DESC,
            };
            use windows::core::Interface;

            let texture_desc = D3D11_TEXTURE2D_DESC {
                Width: width as u32,
                Height: height as u32,
                MipLevels: 1,
                ArraySize: 1,
                Format: DXGI_FORMAT_B8G8R8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                Usage: D3D11_USAGE_DEFAULT,
                BindFlags: (D3D11_BIND_RENDER_TARGET.0 | D3D11_BIND_SHADER_RESOURCE.0) as u32,
                CPUAccessFlags: 0,
                MiscFlags: 0,
            };

            let mut d3d11_texture = None;
            unsafe {
                d3d11_device.CreateTexture2D(&texture_desc, None, Some(&mut d3d11_texture)).unwrap();
            }
            let d3d11_texture = d3d11_texture.unwrap();

            // Create shader resource view for makepad rendering
            let resource: ID3D11Resource = d3d11_texture.clone().cast().unwrap();
            let mut shader_resource_view = None;
            unsafe {
                d3d11_device.CreateShaderResourceView(&resource, None, Some(&mut shader_resource_view)).unwrap();
            }

            cxtexture.os.texture = Some(d3d11_texture.clone());
            cxtexture.os.shader_resource_view = shader_resource_view;

            // Import the D3D11 texture into ANGLE via EGLImage
            let d3d11_texture_raw: *mut c_void = unsafe { std::mem::transmute_copy(&d3d11_texture) };

            // Load extension functions via eglGetProcAddress
            type EglCreateImageKHRFn = unsafe extern "system" fn(
                *mut c_void, *mut c_void, u32, *mut c_void, *const i32,
            ) -> *mut c_void;
            type GlEGLImageTargetTexture2DOESFn = unsafe extern "system" fn(u32, *mut c_void);
            type GlGenTexturesFn = unsafe extern "system" fn(i32, *mut u32);
            type GlBindTextureFn = unsafe extern "system" fn(u32, u32);

            let egl_create_image_khr: EglCreateImageKHRFn = unsafe {
                std::mem::transmute(self.egl.get_proc_address("eglCreateImageKHR"))
            };
            let gl_egl_image_target_texture_2d_oes: GlEGLImageTargetTexture2DOESFn = unsafe {
                std::mem::transmute(self.egl.get_proc_address("glEGLImageTargetTexture2DOES"))
            };
            let gl_gen_textures: GlGenTexturesFn = unsafe {
                std::mem::transmute(self.egl.get_proc_address("glGenTextures"))
            };
            let gl_bind_texture: GlBindTextureFn = unsafe {
                std::mem::transmute(self.egl.get_proc_address("glBindTexture"))
            };

            // Create EGLImage from D3D11 texture
            let image_attribs: &[EGLint] = &[EGL_NONE];
            let egl_image = unsafe {
                egl_create_image_khr(
                    self.egl_display,
                    EGL_NO_CONTEXT,
                    EGL_D3D11_TEXTURE_ANGLE,
                    d3d11_texture_raw,
                    image_attribs.as_ptr(),
                )
            };
            assert!(!egl_image.is_null(), "eglCreateImageKHR failed");

            // Create GL texture and bind the EGLImage to it
            let mut gl_texture_id: u32 = 0;
            unsafe {
                gl_gen_textures(1, &mut gl_texture_id);
                gl_bind_texture(GL_TEXTURE_2D, gl_texture_id);
                gl_egl_image_target_texture_2d_oes(GL_TEXTURE_2D, egl_image);
                gl_bind_texture(GL_TEXTURE_2D, 0);
            }

            // Force the alloc info so makepad knows the dimensions
            cxtexture.alloc = Some(crate::texture::TextureAlloc {
                category: crate::texture::TextureCategory::Render,
                pixel: crate::texture::TexturePixel::BGRAu8,
                width,
                height,
            });

            return (texture, gl_texture_id);
        }
    }
}

impl Drop for AngleRenderBridge {
    fn drop(&mut self) {
        unsafe {
            (self.egl.make_current)(
                self.egl_display,
                EGL_NO_SURFACE,
                EGL_NO_SURFACE,
                EGL_NO_CONTEXT,
            );
            (self.egl.destroy_context)(self.egl_display, self.egl_context);
            (self.egl.terminate)(self.egl_display);
            (self.egl.release_device_angle)(self.egl_device);
        }
    }
}
