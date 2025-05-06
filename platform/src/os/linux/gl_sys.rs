#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

#[macro_export]
macro_rules!gl_log_error {
    ($libgl:ident) => {
        {
            let mut ret = false;
            let mut i= 0;
            loop{
                #[allow(unused_unsafe)]
                let err = unsafe{($libgl.glGetError)()};
                if err!=0{
                    crate::log!("Caught GL Error({i}) {:x} ", err);
                    ret = true;
                }
                else{break}
                i += 1;
            }
            ret
        }
    };
}

#[macro_export]
macro_rules!gl_flush_error {
    ($libgl:ident) => {
        {
            loop{
                #[allow(unused_unsafe)]
                let err = unsafe{($libgl.glGetError)()};
                if err!=0{}
                else{break}
            }
        }
    };
}

use std::os::raw;

pub type GLenum = raw::c_uint;
pub type GLsizei = raw::c_int;
pub type GLuint = raw::c_uint;
pub type GLint = raw::c_int;
pub type GLdouble = raw::c_double;
pub type GLfloat = raw::c_float;
pub type GLbitfield = raw::c_uint;
pub type GLboolean = raw::c_uchar;
pub type GLchar = raw::c_char;
pub type GLubyte = raw::c_uchar;
pub type GLsizeiptr = isize; 

pub const TRUE: GLboolean = 1;
pub const ARRAY_BUFFER: GLenum = 0x8892;
pub const ELEMENT_ARRAY_BUFFER: GLenum = 0x8893;
pub const TEXTURE0: GLenum = 0x84C0;
pub const TEXTURE_2D: GLenum = 0x0DE1;
pub const TRIANGLES: GLenum = 0x0004;
pub const UNSIGNED_INT: GLenum = 0x1405;
pub const DEPTH_TEST: GLenum = 0x0B71;
pub const LEQUAL: GLenum = 0x0203;
pub const FUNC_ADD: GLenum = 0x8006;
pub const ONE: GLenum = 1;
pub const ONE_MINUS_SRC_ALPHA: GLenum = 0x0303;
pub const BLEND: GLenum = 0x0BE2;
pub const FRAMEBUFFER: GLenum = 0x8D40;
pub const COLOR_BUFFER_BIT: GLenum = 0x00004000;
pub const DEPTH_BUFFER_BIT: GLenum = 0x00000100;
pub const RENDERBUFFER: GLenum = 0x8D41;
pub const DEPTH_COMPONENT16: GLenum = 0x81A5;
pub const DEPTH_COMPONENT24: GLenum = 0x81A6;
pub const DYNAMIC_DRAW:GLenum = 0x88E8;
pub const FRAMEBUFFER_SRGB_EXT: GLenum = 0x8DB9;
pub const DEPTH_ATTACHMENT: GLenum = 0x8D00;
pub const COLOR_ATTACHMENT0: GLenum = 0x8CE0;
pub const INFO_LOG_LENGTH: GLenum = 0x8B84;
pub const COMPILE_STATUS: GLenum = 0x8B81;
pub const LINK_STATUS: GLenum = 0x8B82;
pub const VERTEX_SHADER: GLenum = 0x8B31;
pub const FRAGMENT_SHADER: GLenum = 0x8B30;
pub const TEXTURE_MIN_FILTER: GLenum = 0x2801;
pub const LINEAR: GLenum = 0x2601;
pub const LINEAR_MIPMAP_LINEAR: GLenum = 0x2703;
pub const TEXTURE_BASE_LEVEL: GLenum = 0x813C;
pub const TEXTURE_MAX_LEVEL: GLenum = 0x813D;
pub const TEXTURE_MAG_FILTER: GLenum = 0x2800;
pub const TEXTURE_WIDTH: GLenum = 0x1000;
pub const TEXTURE_HEIGHT: GLenum = 0x1001;
pub const TEXTURE_2D_ARRAY: GLenum = 0x8C1A;
pub const TEXTURE_BORDER_COLOR: GLenum = 0x1004;
pub const DEBUG_OUTPUT: GLenum = 0x92E0;

pub const RGBA: GLenum = 0x1908;
pub const BGRA: GLenum = 0x80E1;
pub const RED: GLenum = 0x1903;
pub const RG: GLenum =  0x8227;
pub const R8: GLenum =  0x8229;
pub const UNSIGNED_BYTE: GLenum = 0x1401;
pub const HALF_FLOAT: GLenum =  0x140B;
pub const FLOAT: GLenum = 0x1406;
pub const SRGB8_ALPHA8L: GLenum =  0x8C43;

pub const DEPTH_COMPONENT32F: GLenum = 0x8CAC;
pub const STATIC_DRAW: GLenum = 0x88E4;
pub const NEAREST: GLenum = 0x2600;
pub const TEXTURE_WRAP_S: GLenum = 0x2802;
pub const TEXTURE_WRAP_T: GLenum = 0x2803;
pub const CLAMP_TO_EDGE: GLenum = 0x812F;
pub const CLAMP_TO_BORDER: GLenum = 0x812D;
pub const PROGRAM_BINARY_LENGTH: GLenum = 0x8741;
pub const NO_ERROR: GLenum = 0x0;
pub const UNPACK_ALIGNMENT: GLenum = 0x0CF5;
pub const UNPACK_ROW_LENGTH: GLenum = 0x0CF2;
pub const UNPACK_SKIP_PIXELS: GLenum = 0x0CF4;
pub const UNPACK_SKIP_ROWS: GLenum = 0x0CF3;
pub const DRAW_FRAMEBUFFER:GLenum = 0x8CA9;
pub const TEXTURE_EXTERNAL_OES: GLenum = 0x8D65;
pub const EXTENSIONS: GLenum = 0x1F03;
pub const VENDOR: GLenum = 0x1F00;
pub const RENDERER: GLenum = 0x1F01;
pub const SCISSOR_TEST: GLenum = 0x0C11;
pub const CULL_FACE:GLenum = 0x0B44;
pub const DONT_CARE:GLenum = 0x1100;
pub const UNIFORM_BUFFER:GLenum = 0x8A11;

pub type TglGenVertexArrays = unsafe extern "C" fn(n: GLsizei, arrays: *mut GLuint) -> () ;
pub type TglBindVertexArray = unsafe extern "C" fn(array: GLuint) -> () ;
pub type TglBindBuffer = unsafe extern "C" fn(target: GLenum, buffer: GLuint) -> () ;
pub type TglVertexAttribPointer = unsafe extern "C" fn(index: GLuint, size: GLint, type_: GLenum, normalized: GLboolean, stride: GLsizei, pointer: *const raw::c_void) -> () ;
pub type TglEnableVertexAttribArray = unsafe extern "C" fn(index: GLuint) -> () ;
pub type TglVertexAttribDivisor = unsafe extern "C" fn(index: GLuint, divisor: GLuint) -> () ;
pub type TglUseProgram = unsafe extern "C" fn(program: GLuint) -> () ;
pub type TglActiveTexture = unsafe extern "C" fn(texture: GLenum) -> () ;
pub type TglBindTexture = unsafe extern "C" fn(target: GLenum, texture: GLuint) -> () ;
pub type TglDrawElementsInstanced = unsafe extern "C" fn(mode: GLenum, count: GLsizei, type_: GLenum, indices: *const raw::c_void, instancecount: GLsizei) -> () ;
pub type TglEnable = unsafe extern "C" fn(cap: GLenum) -> () ;
pub type TglDepthFunc = unsafe extern "C" fn(func: GLenum) -> () ;
pub type TglBlendEquationSeparate = unsafe extern "C" fn(modeRGB: GLenum, modeAlpha: GLenum) -> () ;
pub type TglBlendFuncSeparate = unsafe extern "C" fn(sfactorRGB: GLenum, dfactorRGB: GLenum, sfactorAlpha: GLenum, dfactorAlpha: GLenum) -> () ;
pub type TglViewport = unsafe extern "C" fn(x: GLint, y: GLint, width: GLsizei, height: GLsizei) -> () ;
pub type TglBindFramebuffer = unsafe extern "C" fn(target: GLenum, framebuffer: GLuint) -> () ;
pub type TglClearDepthf = unsafe extern "C" fn(d: GLfloat) -> () ;
pub type TglClearColor = unsafe extern "C" fn(red: GLfloat, green: GLfloat, blue: GLfloat, alpha: GLfloat) -> () ;
pub type TglClear = unsafe extern "C" fn(mask: GLbitfield) -> () ;
pub type TglGenFramebuffers = unsafe extern "C" fn(n: GLsizei, framebuffers: *mut GLuint) -> () ;
pub type TglGenRenderbuffers = unsafe extern "C" fn(n: GLsizei, renderbuffers: *mut GLuint) -> () ;
pub type TglBindRenderbuffer = unsafe extern "C" fn(target: GLenum, renderbuffer: GLuint) -> () ;
pub type TglRenderbufferStorage = unsafe extern "C" fn(target: GLenum, internalformat: GLenum, width: GLsizei, height: GLsizei) -> () ;
pub type TglDisable = unsafe extern "C" fn(cap: GLenum) -> () ;
pub type TglFramebufferRenderbuffer = unsafe extern "C" fn(target: GLenum, attachment: GLenum, renderbuffertarget: GLenum, renderbuffer: GLuint) -> () ;
pub type TglFramebufferTexture2D = unsafe extern "C" fn(target: GLenum, attachment: GLenum, textarget: GLenum, texture: GLuint, level: GLint) -> () ;
pub type TglGetShaderiv = unsafe extern "C" fn(shader: GLuint, pname: GLenum, params: *mut GLint) -> () ;
pub type TglGetProgramiv = unsafe extern "C" fn(program: GLuint, pname: GLenum, params: *mut GLint) -> () ;
pub type TglGetShaderInfoLog = unsafe extern "C" fn(shader: GLuint, bufSize: GLsizei, length: *mut GLsizei, infoLog: *mut GLchar) -> () ;
pub type TglGetProgramInfoLog = unsafe extern "C" fn(program: GLuint, bufSize: GLsizei, length: *mut GLsizei, infoLog: *mut GLchar) -> () ;
pub type TglGetAttribLocation = unsafe extern "C" fn(program: GLuint, name: *const GLchar) -> GLint ;
pub type TglGetUniformLocation = unsafe extern "C" fn(program: GLuint, name: *const GLchar) -> GLint ;
pub type TglCreateShader = unsafe extern "C" fn(type_: GLenum) -> GLuint ;
pub type TglShaderSource = unsafe extern "C" fn(shader: GLuint, count: GLsizei, string: *const *const GLchar, length: *const GLint) -> () ;
pub type TglCompileShader = unsafe extern "C" fn(shader: GLuint) -> () ;
pub type TglCreateProgram = unsafe extern "C" fn() -> GLuint ;
pub type TglAttachShader = unsafe extern "C" fn(program: GLuint, shader: GLuint) -> () ;
pub type TglLinkProgram = unsafe extern "C" fn(program: GLuint) -> () ;
pub type TglDeleteShader = unsafe extern "C" fn(shader: GLuint) -> () ;
pub type TglUniform1fv = unsafe extern "C" fn(location: GLint, count: GLsizei, value: *const GLfloat) -> () ;
pub type TglGenTextures = unsafe extern "C" fn(n: GLsizei, textures: *mut GLuint) -> () ;
pub type TglTexParameteri = unsafe extern "C" fn(target: GLenum, pname: GLenum, param: GLint) -> () ;
pub type TglTexParameterfv = unsafe extern "C" fn(target: GLenum, pname: GLenum, param: *const GLfloat) -> () ;
pub type TglTexImage2D = unsafe extern "C" fn(target: GLenum, level: GLint, internalformat: GLint, width: GLsizei, height: GLsizei, border: GLint, format: GLenum, type_: GLenum, pixels: *const raw::c_void) -> () ;
pub type TglTexSubImage2D = unsafe extern "C" fn(target: GLenum, level: GLint, xoffset: GLint, yoffset: GLint, width: GLsizei, height: GLsizei, format: GLenum, type_: GLenum, pixels: *const raw::c_void) -> () ;
pub type TglGetTexLevelParameteriv = unsafe extern "C" fn(target: GLenum, level: GLint, pname: GLenum, params: *mut GLint) -> ();
pub type TglDeleteTextures = unsafe extern "C" fn(n: GLsizei, textures: *const GLuint) -> () ;
pub type TglGenBuffers = unsafe extern "C" fn(n: GLsizei, buffers: *mut GLuint) -> () ;
pub type TglBufferData = unsafe extern "C" fn(target: GLenum, size: GLsizeiptr, data: *const raw::c_void, usage: GLenum) -> () ;
pub type TglUniform1i = unsafe extern "C" fn(location: GLint, v0: GLint) -> () ;
pub type TglGetError = unsafe extern "C" fn() -> GLenum ;
pub type TglFinish = unsafe extern "C" fn() -> () ;
pub type TglGetProgramBinary = unsafe extern "C" fn(program: GLuint, bufSize: GLsizei, length: *mut GLsizei, binaryFormat: *mut GLenum, binary: *mut raw::c_void) -> () ;
pub type TglProgramBinary = unsafe extern "C" fn(program: GLuint, binaryFormat: GLenum, binary: *const raw::c_void, length: GLsizei) -> () ;
pub type TglDeleteRenderbuffers = unsafe extern "C" fn(n: GLsizei, renderbuffers: *const GLuint) -> () ;
pub type TglDeleteBuffers = unsafe extern "C" fn(n: GLsizei, buffers: *const GLuint) -> () ;
pub type TglDeleteFramebuffers = unsafe extern "C" fn(n: GLsizei, framebuffers: *const GLuint) -> () ;
pub type TglDeleteVertexArrays = unsafe extern "C" fn(n: GLsizei, arrays: *const GLuint) -> () ;
pub type TglGenerateMipmap = unsafe extern "C" fn(target: GLenum) -> () ;
pub type TglPixelStorei = unsafe extern "C" fn(pname: GLenum, param: GLint) -> () ;
pub type TglGetString = unsafe extern "C" fn(name: GLenum) -> *const GLubyte ;
pub type TglTexStorage3D = unsafe extern "C" fn (target:GLenum, levels:GLsizei, internal_format: GLenum, width: GLsizei, height:GLsizei, depth:GLsizei);
pub type TglFramebufferTextureMultiviewOVR = unsafe extern "C" fn(target:GLenum , attachment:GLenum, texture:GLuint, level:GLint, base_view_index:GLint, num_views:GLsizei);
pub type TglColorMask = unsafe extern "C" fn(r: GLboolean, g:GLboolean, b:GLboolean, a:GLboolean);
pub type TglDepthMask = unsafe extern "C" fn(d: GLboolean);
pub type TglScissor = unsafe extern "C" fn(x:GLint, y:GLint, width:GLsizei, height:GLsizei);

pub type TglInvalidateFramebuffer = unsafe extern "C" fn(target:GLenum, num_attachments:GLsizei, attachments: *const GLenum);
pub type TglDebugMessageCallback = unsafe extern "C" fn(ptr: TglDebugMessageCallbackFn, param: *const raw::c_void);
pub type TglDebugMessageCallbackFn = unsafe extern "C" fn(source: GLenum, ty: GLenum, id: GLuint, severity:GLenum, length:GLsizei, msg: *const GLchar, param: *const raw::c_void);
pub type TglGetUniformBlockIndex = unsafe extern "C" fn(program:GLuint, uniform_block_name: *const GLchar)->GLuint;
pub type TglUniformBlockBinding = unsafe extern "C" fn(program: GLuint, block_index: GLuint, binding: GLuint);
pub type TglBindBufferBase = unsafe extern "C" fn(target:GLenum, index: GLuint, buffer:GLuint);

pub type TglFramebufferTextureMultisampleMultiviewOVR = unsafe extern "C" fn(target:GLenum , attachment:GLenum, texture:GLuint, level:GLint, samples:GLsizei, base_view_index:GLint, num_views:GLsizei);

pub type TglGetDebugMessageLog = unsafe extern "C" fn(count:GLuint,
    buf_size:GLsizei,
    sources: *mut GLenum,
    types: &mut GLenum,
    ids: *mut GLuint,
    severities: *mut GLenum,
    lengths: *mut GLsizei,
    message_log: *mut GLchar);
    
pub type TglDebugMessageControl = unsafe extern "C" fn(
    source:GLenum,
    ty: GLenum,
    severity: GLenum,
    count: GLsizei,
    ids: *const GLuint,
    enabled: GLboolean);
    
pub struct LibGl{
    pub glGenVertexArrays: TglGenVertexArrays,
    pub glBindVertexArray: TglBindVertexArray,
    pub glBindBuffer: TglBindBuffer,
    pub glVertexAttribPointer: TglVertexAttribPointer,
    pub glEnableVertexAttribArray: TglEnableVertexAttribArray,
    pub glVertexAttribDivisor: TglVertexAttribDivisor,
    pub glUseProgram: TglUseProgram,
    pub glActiveTexture: TglActiveTexture,
    pub glBindTexture: TglBindTexture,
    pub glDrawElementsInstanced: TglDrawElementsInstanced,
    pub glEnable: TglEnable,
    pub glDepthFunc: TglDepthFunc,
    pub glBlendEquationSeparate: TglBlendEquationSeparate,
    pub glBlendFuncSeparate: TglBlendFuncSeparate,
    pub glViewport: TglViewport,
    pub glBindFramebuffer: TglBindFramebuffer,
    pub glClearDepthf: TglClearDepthf,
    pub glClearColor: TglClearColor,
    pub glClear: TglClear,
    pub glGenFramebuffers: TglGenFramebuffers,
    pub glGenRenderbuffers: TglGenRenderbuffers,
    pub glBindRenderbuffer: TglBindRenderbuffer,
    pub glRenderbufferStorage: TglRenderbufferStorage,
    pub glDisable: TglDisable,
    pub glFramebufferRenderbuffer: TglFramebufferRenderbuffer,
    pub glFramebufferTexture2D: TglFramebufferTexture2D,
    pub glGetShaderiv: TglGetShaderiv,
    pub glGetProgramiv: TglGetProgramiv,
    pub glGetShaderInfoLog: TglGetShaderInfoLog,
    pub glGetProgramInfoLog: TglGetProgramInfoLog,
    pub glGetAttribLocation: TglGetAttribLocation,
    pub glGetUniformLocation: TglGetUniformLocation,
    pub glCreateShader: TglCreateShader,
    pub glShaderSource: TglShaderSource,
    pub glCompileShader: TglCompileShader,
    pub glCreateProgram: TglCreateProgram,
    pub glAttachShader: TglAttachShader,
    pub glLinkProgram: TglLinkProgram,
    pub glDeleteShader: TglDeleteShader,
    pub glUniform1fv: TglUniform1fv,
    pub glGenTextures: TglGenTextures,
    pub glTexParameteri: TglTexParameteri,
    pub glTexParameterfv: TglTexParameterfv,
    pub glTexImage2D: TglTexImage2D,
    pub glTexSubImage2D: TglTexSubImage2D,
    pub glGetTexLevelParameteriv: TglGetTexLevelParameteriv,
    pub glGenBuffers: TglGenBuffers,
    pub glBufferData: TglBufferData,
    pub glUniform1i: TglUniform1i,
    pub glGetError: TglGetError,
    pub glFinish: TglFinish,
    pub glGetProgramBinary: TglGetProgramBinary,
    pub glProgramBinary: TglProgramBinary,
    pub glDeleteTextures: TglDeleteTextures,
    pub glDeleteRenderbuffers: TglDeleteRenderbuffers,
    pub glDeleteBuffers: TglDeleteBuffers,
    pub glDeleteFramebuffers: TglDeleteFramebuffers,
    pub glDeleteVertexArrays: TglDeleteVertexArrays,
    pub glGenerateMipmap: TglGenerateMipmap,
    pub glPixelStorei: TglPixelStorei,
    pub glGetString: TglGetString,
    pub glTexStorage3D: TglTexStorage3D,
    pub glColorMask: TglColorMask,
    pub glDepthMask: TglDepthMask,
    pub glScissor: TglScissor,
    pub glInvalidateFramebuffer: TglInvalidateFramebuffer,
    pub glDebugMessageCallback: TglDebugMessageCallback,
    pub glGetDebugMessageLog: TglGetDebugMessageLog,
    pub glDebugMessageControl: TglDebugMessageControl,
    pub glGetUniformBlockIndex: TglGetUniformBlockIndex,
    pub glUniformBlockBinding: TglUniformBlockBinding,
    pub glBindBufferBase: TglBindBufferBase,
    pub glFramebufferTextureMultiviewOVR: Option<TglFramebufferTextureMultiviewOVR>,
    pub glFramebufferTextureMultisampleMultiviewOVR: Option<TglFramebufferTextureMultisampleMultiviewOVR>,
}



macro_rules! load {
    ($loadfn:expr, $ty:ident, $($sym:expr),*) => {
        unsafe{
            let syms = [$($sym,)*];
            let ptr = $loadfn(&syms);
            if ptr.is_null(){
                Err(format!("Cannot find symbol {}", stringify!($ty)))
            }
            else{
                Ok({ std::mem::transmute_copy::<_, $ty>(&ptr) })
            }
        }
    };
}


impl LibGl{
    pub fn enable_debugging(&self){
        unsafe{(self.glEnable)(self::DEBUG_OUTPUT)};
                                
        unsafe extern "C" fn debug(_source: self::GLenum, _ty: self::GLenum, _id: self::GLuint, _severity:self::GLenum, _length:self::GLsizei, msg: *const self::GLchar, _param: *const std::ffi::c_void){
            crate::log!("GL Debug info: {:?}", std::ffi::CStr::from_ptr(msg));
        }
                                
        unsafe{(self.glDebugMessageControl)(
            self::DONT_CARE,
            self::DONT_CARE,
            self::DONT_CARE,
            0,
            0 as * const _,
            self::TRUE
        )};
        unsafe{(self.glDebugMessageCallback)(debug, 0 as *const _)};
    }
    
    pub fn try_load<F>(mut loadfn: F)->Result<LibGl, String>
    where F: FnMut(&[&'static str]) -> *const raw::c_void
    {
        Ok(Self{
            glGenVertexArrays: load!(loadfn, TglGenVertexArrays, "glGenVertexArrays", "glGenVertexArraysAPPLE", "glGenVertexArraysOES")?,
            glBindVertexArray: load!(loadfn, TglBindVertexArray, "glBindVertexArray", "glBindVertexArrayOES")?,
            glBindBuffer: load!(loadfn, TglBindBuffer, "glBindBuffer", "glBindBufferARB")?,
            glVertexAttribPointer: load!(loadfn, TglVertexAttribPointer, "glVertexAttribPointer", "glVertexAttribPointerARB")?,
            glEnableVertexAttribArray: load!(loadfn, TglEnableVertexAttribArray, "glEnableVertexAttribArray", "glEnableVertexAttribArrayARB")?,
            glVertexAttribDivisor: load!(loadfn, TglVertexAttribDivisor, "glVertexAttribDivisor", "glVertexAttribDivisorANGLE", "glVertexAttribDivisorARB", "glVertexAttribDivisorEXT", "glVertexAttribDivisorNV")?,
            glUseProgram: load!(loadfn, TglUseProgram, "glUseProgram", "glUseProgramObjectARB")?,
            glActiveTexture: load!(loadfn, TglActiveTexture, "glActiveTexture", "glActiveTextureARB")?,
            glBindTexture: load!(loadfn, TglBindTexture, "glBindTexture", "glBindTextureEXT")?,
            glDrawElementsInstanced: load!(loadfn, TglDrawElementsInstanced, "glDrawElementsInstanced", "glDrawElementsInstancedANGLE", "glDrawElementsInstancedARB", "glDrawElementsInstancedEXT", "glDrawElementsInstancedNV")?,
            glEnable: load!(loadfn, TglEnable, "glEnable")?,
            glDepthFunc: load!(loadfn, TglDepthFunc, "glDepthFunc")?,
            glBlendEquationSeparate: load!(loadfn, TglBlendEquationSeparate, "glBlendEquationSeparate", "glBlendEquationSeparateEXT")?,
            glBlendFuncSeparate: load!(loadfn, TglBlendFuncSeparate, "glBlendFuncSeparate", "glBlendFuncSeparateEXT", "glBlendFuncSeparateINGR")?,
            glViewport: load!(loadfn, TglViewport, "glViewport")?,
            glBindFramebuffer: load!(loadfn, TglBindFramebuffer, "glBindFramebuffer")?,
            glClearColor: load!(loadfn, TglClearColor, "glClearColor")?,
            glClear: load!(loadfn, TglClear, "glClear")?,
            glGenFramebuffers: load!(loadfn, TglGenFramebuffers, "glGenFramebuffers", "glGenFramebuffersEXT")?,
            glGenRenderbuffers: load!(loadfn, TglGenRenderbuffers, "glGenRenderbuffers", "glGenRenderbuffersEXT")?,
            glBindRenderbuffer: load!(loadfn, TglBindRenderbuffer, "glBindRenderbuffer")?,
            glRenderbufferStorage: load!(loadfn, TglRenderbufferStorage, "glRenderbufferStorage", "glRenderbufferStorageEXT")?,
            glDisable: load!(loadfn, TglDisable, "glDisable")?,
            glFramebufferRenderbuffer: load!(loadfn, TglFramebufferRenderbuffer, "glFramebufferRenderbuffer", "glFramebufferRenderbufferEXT")?,
            glFramebufferTexture2D: load!(loadfn, TglFramebufferTexture2D, "glFramebufferTexture2D", "glFramebufferTexture2DEXT")?,
            glGetShaderiv: load!(loadfn, TglGetShaderiv, "glGetShaderiv")?,
            glGetProgramiv: load!(loadfn, TglGetProgramiv, "glGetProgramiv")?,
            glGetShaderInfoLog: load!(loadfn, TglGetShaderInfoLog, "glGetShaderInfoLog")?,
            glGetProgramInfoLog: load!(loadfn, TglGetProgramInfoLog, "glGetProgramInfoLog")?,
            glGetAttribLocation: load!(loadfn, TglGetAttribLocation, "glGetAttribLocation", "glGetAttribLocationARB")?,
            glGetUniformLocation: load!(loadfn, TglGetUniformLocation, "glGetUniformLocation", "glGetUniformLocationARB")?,
            glCreateShader: load!(loadfn, TglCreateShader, "glCreateShader", "glCreateShaderObjectARB")?,
            glShaderSource: load!(loadfn, TglShaderSource, "glShaderSource", "glShaderSourceARB")?,
            glCompileShader: load!(loadfn, TglCompileShader, "glCompileShader", "glCompileShaderARB")?,
            glCreateProgram: load!(loadfn, TglCreateProgram, "glCreateProgram", "glCreateProgramObjectARB")?,
            glAttachShader: load!(loadfn, TglAttachShader, "glAttachShader", "glAttachObjectARB")?,
            glLinkProgram: load!(loadfn, TglLinkProgram, "glLinkProgram", "glLinkProgramARB")?,
            glDeleteShader: load!(loadfn, TglDeleteShader, "glDeleteShader")?,
            glUniform1fv: load!(loadfn, TglUniform1fv, "glUniform1fv", "glUniform1fvARB")?,
            glGenTextures: load!(loadfn, TglGenTextures, "glGenTextures")?,
            glTexParameteri: load!(loadfn, TglTexParameteri, "glTexParameteri")?,
            glTexParameterfv: load!(loadfn, TglTexParameterfv, "glTexParameterfv")?,
            glTexImage2D: load!(loadfn, TglTexImage2D, "glTexImage2D")?,
            glTexSubImage2D: load!(loadfn, TglTexSubImage2D, "glTexSubImage2D")?,
            glGetTexLevelParameteriv: load!(loadfn, TglGetTexLevelParameteriv, "glGetTexLevelParameteriv" )?,
            glDeleteTextures: load!(loadfn, TglDeleteTextures, "glDeleteTextures")?,
            glGenBuffers: load!(loadfn, TglGenBuffers, "glGenBuffers", "glGenBuffersARB")?,
            glBufferData: load!(loadfn, TglBufferData, "glBufferData", "glBufferDataARB")?,
            glUniform1i: load!(loadfn, TglUniform1i, "glUniform1i", "glUniform1iARB")?,
            glGetError: load!(loadfn, TglGetError, "glGetError")?,
            glFinish: load!(loadfn, TglFinish, "glFinish")?,
            glClearDepthf: load!(loadfn, TglClearDepthf, "glClearDepthf", "glClearDepthfOES")?,
            glGetProgramBinary: load!(loadfn, TglGetProgramBinary, "glGetProgramBinary", "glGetProgramBinaryOES")?,
            glProgramBinary: load!(loadfn, TglProgramBinary, "glProgramBinary", "glProgramBinaryOES")?,
            glDeleteRenderbuffers: load!(loadfn, TglDeleteRenderbuffers, "glDeleteRenderbuffers", "glDeleteRenderbuffersEXT")?,
            glDeleteBuffers: load!(loadfn, TglDeleteBuffers, "glDeleteBuffers", "glDeleteBuffersARB")?,
            glDeleteFramebuffers: load!(loadfn, TglDeleteFramebuffers, "glDeleteFramebuffers", "glDeleteFramebuffersEXT")?,
            glDeleteVertexArrays: load!(loadfn, TglDeleteVertexArrays, "glDeleteVertexArrays", "glDeleteVertexArraysAPPLE", "glDeleteVertexArraysOES")?,
            glGenerateMipmap: load!(loadfn, TglGenerateMipmap, "glGenerateMipmap")?,
            glPixelStorei: load!(loadfn, TglPixelStorei, "glPixelStorei")?,
            glGetString: load!(loadfn, TglGetString, "glGetString")?,
            glTexStorage3D: load!(loadfn, TglTexStorage3D, "glTexStorage3D")?,
            glColorMask: load!(loadfn, TglColorMask, "glColorMask")?,
            glDepthMask: load!(loadfn, TglDepthMask, "glDepthMask")?,
            glScissor: load!(loadfn, TglScissor, "glScissor")?,
            glInvalidateFramebuffer: load!(loadfn, TglInvalidateFramebuffer, "glInvalidateFramebuffer")?,
            glDebugMessageCallback: load!(loadfn, TglDebugMessageCallback, "glDebugMessageCallback")?,
            glGetDebugMessageLog: load!(loadfn, TglGetDebugMessageLog, "glGetDebugMessageLog")?,
            glDebugMessageControl: load!(loadfn, TglDebugMessageControl, "glDebugMessageControl")?,
            glGetUniformBlockIndex: load!(loadfn, TglGetUniformBlockIndex, "glGetUniformBlockIndex")?,
            glUniformBlockBinding: load!(loadfn, TglUniformBlockBinding, "glUniformBlockBinding")?,
            glBindBufferBase: load!(loadfn, TglBindBufferBase, "glBindBufferBase")?,
            
            // optional fns
            glFramebufferTextureMultiviewOVR: load!(loadfn, TglFramebufferTextureMultiviewOVR, "glFramebufferTextureMultiviewOVR").ok(),
            glFramebufferTextureMultisampleMultiviewOVR: load!(loadfn, TglFramebufferTextureMultisampleMultiviewOVR, "glFramebufferTextureMultisampleMultiviewOVR").ok()
        })
    }
}
