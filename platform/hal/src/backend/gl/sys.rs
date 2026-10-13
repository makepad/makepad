//! The GLES 3.0 entry points the backend calls, loaded through eglGetProcAddress once per
//! context. Only the render thread (and an interop context of the same share group) calls them.
#![allow(non_snake_case)]
use std::ffi::{c_void, CStr};

pub type GLenum = u32;
pub type GLuint = u32;
pub type GLint = i32;
pub type GLsizei = i32;
pub type GLfloat = f32;
pub type GLboolean = u8;
pub type GLbitfield = u32;
pub type GLsync = *const c_void;

macro_rules! gl_functions {
    ($($name:ident ($($arg:ty),*) $(-> $ret:ty)?;)*) => {
        pub struct Gl {
            $(pub $name: unsafe extern "C" fn($($arg),*) $(-> $ret)?,)*
            /// KHR_parallel_shader_compile: links finish on driver threads while frames render.
            pub glMaxShaderCompilerThreadsKHR: Option<unsafe extern "C" fn(GLuint)>,
        }
        impl Gl {
            pub fn load(get: impl Fn(&CStr) -> *mut c_void) -> Result<Self, String> {
                Ok(Self {
                    $($name: {
                        let f = get(unsafe { CStr::from_bytes_with_nul_unchecked(concat!(stringify!($name), "\0").as_bytes()) });
                        if f.is_null() { return Err(concat!("missing GLES 3.0 function ", stringify!($name)).into()); }
                        unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn($($arg),*) $(-> $ret)?>(f) }
                    },)*
                    glMaxShaderCompilerThreadsKHR: {
                        let f = get(c"glMaxShaderCompilerThreadsKHR");
                        (!f.is_null()).then(|| unsafe { std::mem::transmute::<*mut c_void, unsafe extern "C" fn(GLuint)>(f) })
                    },
                })
            }
        }
    }
}

gl_functions! {
    glGetError() -> GLenum;
    glGetString(GLenum) -> *const u8;
    glGetStringi(GLenum, GLuint) -> *const u8;
    glGetIntegerv(GLenum, *mut GLint);
    glGenTextures(GLsizei, *mut GLuint);
    glDeleteTextures(GLsizei, *const GLuint);
    glBindTexture(GLenum, GLuint);
    glActiveTexture(GLenum);
    glTexStorage2D(GLenum, GLsizei, GLenum, GLsizei, GLsizei);
    glTexStorage3D(GLenum, GLsizei, GLenum, GLsizei, GLsizei, GLsizei);
    glTexSubImage2D(GLenum, GLint, GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *const c_void);
    glTexSubImage3D(GLenum, GLint, GLint, GLint, GLint, GLsizei, GLsizei, GLsizei, GLenum, GLenum, *const c_void);
    glCompressedTexSubImage2D(GLenum, GLint, GLint, GLint, GLsizei, GLsizei, GLenum, GLsizei, *const c_void);
    glCompressedTexSubImage3D(GLenum, GLint, GLint, GLint, GLint, GLsizei, GLsizei, GLsizei, GLenum, GLsizei, *const c_void);
    glTexParameteri(GLenum, GLenum, GLint);
    glGenerateMipmap(GLenum);
    glPixelStorei(GLenum, GLint);
    glGenBuffers(GLsizei, *mut GLuint);
    glDeleteBuffers(GLsizei, *const GLuint);
    glBindBuffer(GLenum, GLuint);
    glBufferData(GLenum, isize, *const c_void, GLenum);
    glBufferSubData(GLenum, isize, isize, *const c_void);
    glBindBufferBase(GLenum, GLuint, GLuint);
    glBindBufferRange(GLenum, GLuint, GLuint, isize, isize);
    glMapBufferRange(GLenum, isize, isize, GLbitfield) -> *mut c_void;
    glUnmapBuffer(GLenum) -> GLboolean;
    glGenVertexArrays(GLsizei, *mut GLuint);
    glDeleteVertexArrays(GLsizei, *const GLuint);
    glBindVertexArray(GLuint);
    glVertexAttribPointer(GLuint, GLint, GLenum, GLboolean, GLsizei, *const c_void);
    glVertexAttribIPointer(GLuint, GLint, GLenum, GLsizei, *const c_void);
    glEnableVertexAttribArray(GLuint);
    glDisableVertexAttribArray(GLuint);
    glVertexAttribDivisor(GLuint, GLuint);
    glCreateShader(GLenum) -> GLuint;
    glShaderSource(GLuint, GLsizei, *const *const u8, *const GLint);
    glCompileShader(GLuint);
    glGetShaderiv(GLuint, GLenum, *mut GLint);
    glGetShaderInfoLog(GLuint, GLsizei, *mut GLsizei, *mut u8);
    glDeleteShader(GLuint);
    glCreateProgram() -> GLuint;
    glAttachShader(GLuint, GLuint);
    glLinkProgram(GLuint);
    glGetProgramiv(GLuint, GLenum, *mut GLint);
    glGetProgramInfoLog(GLuint, GLsizei, *mut GLsizei, *mut u8);
    glDeleteProgram(GLuint);
    glUseProgram(GLuint);
    glGetAttribLocation(GLuint, *const u8) -> GLint;
    glGetActiveAttrib(GLuint, GLuint, GLsizei, *mut GLsizei, *mut GLint, *mut GLenum, *mut u8);
    glGetUniformLocation(GLuint, *const u8) -> GLint;
    glGetUniformBlockIndex(GLuint, *const u8) -> GLuint;
    glUniformBlockBinding(GLuint, GLuint, GLuint);
    glGetActiveUniformBlockiv(GLuint, GLuint, GLenum, *mut GLint);
    glUniform1i(GLint, GLint);
    glUniform1f(GLint, GLfloat);
    glUniform2ui(GLint, GLuint, GLuint);
    glGenSamplers(GLsizei, *mut GLuint);
    glDeleteSamplers(GLsizei, *const GLuint);
    glBindSampler(GLuint, GLuint);
    glSamplerParameteri(GLuint, GLenum, GLint);
    glGenFramebuffers(GLsizei, *mut GLuint);
    glDeleteFramebuffers(GLsizei, *const GLuint);
    glBindFramebuffer(GLenum, GLuint);
    glFramebufferTexture2D(GLenum, GLenum, GLenum, GLuint, GLint);
    glFramebufferTextureLayer(GLenum, GLenum, GLuint, GLint, GLint);
    glCheckFramebufferStatus(GLenum) -> GLenum;
    glDrawBuffers(GLsizei, *const GLenum);
    glReadBuffer(GLenum);
    glBlitFramebuffer(GLint, GLint, GLint, GLint, GLint, GLint, GLint, GLint, GLbitfield, GLenum);
    glReadPixels(GLint, GLint, GLsizei, GLsizei, GLenum, GLenum, *mut c_void);
    glClearBufferfv(GLenum, GLint, *const GLfloat);
    glClearBufferuiv(GLenum, GLint, *const GLuint);
    glClearBufferfi(GLenum, GLint, GLfloat, GLint);
    glViewport(GLint, GLint, GLsizei, GLsizei);
    glScissor(GLint, GLint, GLsizei, GLsizei);
    glEnable(GLenum);
    glDisable(GLenum);
    glDepthFunc(GLenum);
    glDepthMask(GLboolean);
    glColorMask(GLboolean, GLboolean, GLboolean, GLboolean);
    glBlendEquationSeparate(GLenum, GLenum);
    glBlendFuncSeparate(GLenum, GLenum, GLenum, GLenum);
    glCullFace(GLenum);
    glFrontFace(GLenum);
    glDrawElementsInstanced(GLenum, GLsizei, GLenum, *const c_void, GLsizei);
    glFlush();
    glFinish();
    glFenceSync(GLenum, GLbitfield) -> GLsync;
    glClientWaitSync(GLsync, GLbitfield, u64) -> GLenum;
    glWaitSync(GLsync, GLbitfield, u64);
    glDeleteSync(GLsync);
}

impl Gl {
    /// The context's extension names (GLES 3 lists them by index).
    pub fn extensions(&self) -> Vec<String> {
        let mut n = 0;
        unsafe { (self.glGetIntegerv)(NUM_EXTENSIONS, &mut n) };
        (0..n.max(0) as u32).filter_map(|i| {
            let s = unsafe { (self.glGetStringi)(EXTENSIONS, i) };
            (!s.is_null()).then(|| unsafe { CStr::from_ptr(s as *const _) }.to_string_lossy().into_owned())
        }).collect()
    }
    pub fn string(&self, name: GLenum) -> String {
        let s = unsafe { (self.glGetString)(name) };
        if s.is_null() { String::new() } else { unsafe { CStr::from_ptr(s as *const _) }.to_string_lossy().into_owned() }
    }
    pub fn integer(&self, name: GLenum) -> i32 {
        let mut v = 0;
        unsafe { (self.glGetIntegerv)(name, &mut v) };
        v
    }
}

pub const TRUE: GLboolean = 1;
pub const NONE: GLenum = 0;
pub const NO_ERROR: GLenum = 0;
pub const TRIANGLES: GLenum = 0x0004;
pub const BYTE: GLenum = 0x1400;
pub const UNSIGNED_BYTE: GLenum = 0x1401;
pub const SHORT: GLenum = 0x1402;
pub const UNSIGNED_SHORT: GLenum = 0x1403;
pub const INT: GLenum = 0x1404;
pub const UNSIGNED_INT: GLenum = 0x1405;
pub const FLOAT: GLenum = 0x1406;
pub const HALF_FLOAT: GLenum = 0x140B;
pub const UNSIGNED_INT_24_8: GLenum = 0x84FA;
pub const CW: GLenum = 0x0900;
pub const BACK: GLenum = 0x0405;
pub const CULL_FACE: GLenum = 0x0B44;
pub const DEPTH_TEST: GLenum = 0x0B71;
pub const BLEND: GLenum = 0x0BE2;
pub const SCISSOR_TEST: GLenum = 0x0C11;
pub const LEQUAL: GLenum = 0x0203;
pub const ONE: GLenum = 1;
pub const ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
pub const FUNC_ADD: GLenum = 0x8006;
pub const MAX: GLenum = 0x8008;
pub const RENDERER: GLenum = 0x1F01;
pub const VERSION: GLenum = 0x1F02;
pub const EXTENSIONS: GLenum = 0x1F03;
pub const NUM_EXTENSIONS: GLenum = 0x821D;
pub const MAX_TEXTURE_SIZE: GLenum = 0x0D33;
pub const MAX_ARRAY_TEXTURE_LAYERS: GLenum = 0x88FF;
pub const MAX_RENDERBUFFER_SIZE: GLenum = 0x84E8;
pub const UNIFORM_BUFFER_OFFSET_ALIGNMENT: GLenum = 0x8A34;
pub const MAX_UNIFORM_BLOCK_SIZE: GLenum = 0x8A30;
pub const UNIFORM_BLOCK_DATA_SIZE: GLenum = 0x8A40;
pub const INVALID_INDEX: GLuint = u32::MAX;
// textures
pub const TEXTURE_2D: GLenum = 0x0DE1;
pub const TEXTURE_2D_ARRAY: GLenum = 0x8C1A;
pub const TEXTURE_CUBE_MAP: GLenum = 0x8513;
pub const TEXTURE_CUBE_MAP_POSITIVE_X: GLenum = 0x8515;
pub const TEXTURE0: GLenum = 0x84C0;
pub const TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const TEXTURE_WRAP_S: GLenum = 0x2802;
pub const TEXTURE_WRAP_T: GLenum = 0x2803;
pub const TEXTURE_WRAP_R: GLenum = 0x8072;
pub const TEXTURE_MAX_LEVEL: GLenum = 0x813D;
pub const TEXTURE_SWIZZLE_R: GLenum = 0x8E42;
pub const TEXTURE_SWIZZLE_B: GLenum = 0x8E44;
pub const TEXTURE_COMPARE_MODE: GLenum = 0x884C;
pub const TEXTURE_COMPARE_FUNC: GLenum = 0x884D;
pub const COMPARE_REF_TO_TEXTURE: GLenum = 0x884E;
pub const NEAREST: GLenum = 0x2600;
pub const LINEAR: GLenum = 0x2601;
pub const NEAREST_MIPMAP_LINEAR: GLenum = 0x2702;
pub const LINEAR_MIPMAP_LINEAR: GLenum = 0x2703;
pub const REPEAT: GLenum = 0x2901;
pub const CLAMP_TO_EDGE: GLenum = 0x812F;
pub const CLAMP_TO_BORDER: GLenum = 0x812D;
pub const MIRRORED_REPEAT: GLenum = 0x8370;
pub const RED: GLenum = 0x1903;
pub const BLUE: GLenum = 0x1905;
pub const RGBA: GLenum = 0x1908;
pub const RG: GLenum = 0x8227;
pub const RED_INTEGER: GLenum = 0x8D94;
pub const RGBA_INTEGER: GLenum = 0x8D99;
pub const DEPTH_COMPONENT: GLenum = 0x1902;
pub const DEPTH_STENCIL: GLenum = 0x84F9;
pub const R8: GLenum = 0x8229;
pub const RG8: GLenum = 0x822B;
pub const RGBA8: GLenum = 0x8058;
pub const R32F: GLenum = 0x822E;
pub const RG16F: GLenum = 0x822F;
pub const RGBA16F: GLenum = 0x881A;
pub const RGBA32F: GLenum = 0x8814;
pub const R32UI: GLenum = 0x8236;
pub const DEPTH_COMPONENT32F: GLenum = 0x8CAC;
pub const DEPTH24_STENCIL8: GLenum = 0x88F0;
pub const COMPRESSED_RGBA_BPTC_UNORM: GLenum = 0x8E8C;
pub const COMPRESSED_RED_GREEN_RGTC2: GLenum = 0x8DBD;
pub const COMPRESSED_RGBA_ASTC_4X4: GLenum = 0x93B0;
pub const UNPACK_ALIGNMENT: GLenum = 0x0CF5;
pub const PACK_ALIGNMENT: GLenum = 0x0D05;
// buffers
pub const ARRAY_BUFFER: GLenum = 0x8892;
pub const ELEMENT_ARRAY_BUFFER: GLenum = 0x8893;
pub const UNIFORM_BUFFER: GLenum = 0x8A11;
pub const PIXEL_PACK_BUFFER: GLenum = 0x88EB;
pub const PIXEL_UNPACK_BUFFER: GLenum = 0x88EC;
pub const STREAM_DRAW: GLenum = 0x88E0;
pub const STREAM_READ: GLenum = 0x88E1;
pub const DYNAMIC_DRAW: GLenum = 0x88E8;
pub const MAP_READ_BIT: GLbitfield = 0x0001;
// shaders
pub const VERTEX_SHADER: GLenum = 0x8B31;
pub const FRAGMENT_SHADER: GLenum = 0x8B30;
pub const COMPILE_STATUS: GLenum = 0x8B81;
pub const LINK_STATUS: GLenum = 0x8B82;
pub const INFO_LOG_LENGTH: GLenum = 0x8B84;
pub const ACTIVE_ATTRIBUTES: GLenum = 0x8B89;
pub const INT_VEC2: GLenum = 0x8B53;
pub const INT_VEC3: GLenum = 0x8B54;
pub const INT_VEC4: GLenum = 0x8B55;
pub const UNSIGNED_INT_VEC2: GLenum = 0x8DC6;
pub const UNSIGNED_INT_VEC3: GLenum = 0x8DC7;
pub const UNSIGNED_INT_VEC4: GLenum = 0x8DC8;
pub const COMPLETION_STATUS_KHR: GLenum = 0x91B1;
// framebuffers
pub const FRAMEBUFFER: GLenum = 0x8D40;
pub const READ_FRAMEBUFFER: GLenum = 0x8CA8;
pub const DRAW_FRAMEBUFFER: GLenum = 0x8CA9;
pub const FRAMEBUFFER_COMPLETE: GLenum = 0x8CD5;
pub const COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const DEPTH_ATTACHMENT: GLenum = 0x8D00;
pub const DEPTH_STENCIL_ATTACHMENT: GLenum = 0x821A;
pub const COLOR: GLenum = 0x1800;
pub const DEPTH: GLenum = 0x1801;
pub const COLOR_BUFFER_BIT: GLbitfield = 0x4000;
// sync
pub const SYNC_GPU_COMMANDS_COMPLETE: GLenum = 0x9117;
pub const SYNC_FLUSH_COMMANDS_BIT: GLbitfield = 0x0001;
pub const ALREADY_SIGNALED: GLenum = 0x911A;
pub const CONDITION_SATISFIED: GLenum = 0x911C;
pub const WAIT_FAILED: GLenum = 0x911D;
pub const TIMEOUT_IGNORED: u64 = u64::MAX;
