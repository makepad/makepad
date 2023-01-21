#![allow(non_snake_case)]
#![allow(non_upper_case_globals)]

use std::mem;
use std::os::raw;

pub mod types {
    use super::raw;
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
}
pub use types::*;

pub const TRUE: types::GLboolean = 1;
pub const ARRAY_BUFFER: types::GLenum = 0x8892;
pub const FLOAT: types::GLenum = 0x1406;
pub const ELEMENT_ARRAY_BUFFER: types::GLenum = 0x8893;
pub const TEXTURE0: types::GLenum = 0x84C0;
pub const TEXTURE_2D: types::GLenum = 0x0DE1;
pub const TRIANGLES: types::GLenum = 0x0004;
pub const UNSIGNED_INT: types::GLenum = 0x1405;
pub const DEPTH_TEST: types::GLenum = 0x0B71;
pub const LEQUAL: types::GLenum = 0x0203;
pub const FUNC_ADD: types::GLenum = 0x8006;
pub const ONE: types::GLenum = 1;
pub const ONE_MINUS_SRC_ALPHA: types::GLenum = 0x0303;
pub const BLEND: types::GLenum = 0x0BE2;
pub const FRAMEBUFFER: types::GLenum = 0x8D40;
pub const COLOR_BUFFER_BIT: types::GLenum = 0x00004000;
pub const DEPTH_BUFFER_BIT: types::GLenum = 0x00000100;
pub const RENDERBUFFER: types::GLenum = 0x8D41;
pub const DEPTH_COMPONENT16: types::GLenum = 0x81A5;
pub const DEPTH_ATTACHMENT: types::GLenum = 0x8D00;
pub const COLOR_ATTACHMENT0: types::GLenum = 0x8CE0;
pub const INFO_LOG_LENGTH: types::GLenum = 0x8B84;
pub const COMPILE_STATUS: types::GLenum = 0x8B81;
pub const LINK_STATUS: types::GLenum = 0x8B82;
pub const VERTEX_SHADER: types::GLenum = 0x8B31;
pub const FRAGMENT_SHADER: types::GLenum = 0x8B30;
pub const TEXTURE_MIN_FILTER: types::GLenum = 0x2801;
pub const LINEAR: types::GLenum = 0x2601;
pub const TEXTURE_MAG_FILTER: types::GLenum = 0x2800;
pub const RGBA: types::GLenum = 0x1908;
pub const UNSIGNED_BYTE: types::GLenum = 0x1401;
pub const DEPTH_COMPONENT32F: types::GLenum = 0x8CAC;
pub const STATIC_DRAW: types::GLenum = 0x88E4;
pub const NEAREST: types::GLenum = 0x2600;
pub const TEXTURE_WRAP_S: types::GLenum = 0x2802;
pub const TEXTURE_WRAP_T: types::GLenum = 0x2803;
pub const CLAMP_TO_EDGE: types::GLenum = 0x812F;

#[inline] pub unsafe fn GenVertexArrays(n: types::GLsizei, arrays: *mut types::GLuint) -> () {mem::transmute::<_, extern "system" fn(types::GLsizei, *mut types::GLuint) -> ()>(storage::GenVertexArrays.f)(n, arrays)}
#[inline] pub unsafe fn BindVertexArray(array: types::GLuint) -> () {mem::transmute::<_, extern "system" fn(types::GLuint) -> ()>(storage::BindVertexArray.f)(array)}
#[inline] pub unsafe fn BindBuffer(target: types::GLenum, buffer: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLuint) -> ()>(storage::BindBuffer.f)(target, buffer) }
#[inline] pub unsafe fn VertexAttribPointer(index: types::GLuint, size: types::GLint, type_: types::GLenum, normalized: types::GLboolean, stride: types::GLsizei, pointer: *const raw::c_void) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLint, types::GLenum, types::GLboolean, types::GLsizei, *const raw::c_void) -> ()>(storage::VertexAttribPointer.f)(index, size, type_, normalized, stride, pointer) }
#[inline] pub unsafe fn EnableVertexAttribArray(index: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint) -> ()>(storage::EnableVertexAttribArray.f)(index) }
#[inline] pub unsafe fn VertexAttribDivisor(index: types::GLuint, divisor: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLuint) -> ()>(storage::VertexAttribDivisor.f)(index, divisor) }
#[inline] pub unsafe fn UseProgram(program: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint) -> ()>(storage::UseProgram.f)(program) }
#[inline] pub unsafe fn ActiveTexture(texture: types::GLenum) -> () { mem::transmute::<_, extern "system" fn(types::GLenum) -> ()>(storage::ActiveTexture.f)(texture) }
#[inline] pub unsafe fn BindTexture(target: types::GLenum, texture: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLuint) -> ()>(storage::BindTexture.f)(target, texture) }
#[inline] pub unsafe fn DrawElementsInstanced(mode: types::GLenum, count: types::GLsizei, type_: types::GLenum, indices: *const raw::c_void, instancecount: types::GLsizei) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLsizei, types::GLenum, *const raw::c_void, types::GLsizei) -> ()>(storage::DrawElementsInstanced.f)(mode, count, type_, indices, instancecount) }
#[inline] pub unsafe fn Enable(cap: types::GLenum) -> () { mem::transmute::<_, extern "system" fn(types::GLenum) -> ()>(storage::Enable.f)(cap) }
#[inline] pub unsafe fn DepthFunc(func: types::GLenum) -> () { mem::transmute::<_, extern "system" fn(types::GLenum) -> ()>(storage::DepthFunc.f)(func) }
#[inline] pub unsafe fn BlendEquationSeparate(modeRGB: types::GLenum, modeAlpha: types::GLenum) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLenum) -> ()>(storage::BlendEquationSeparate.f)(modeRGB, modeAlpha) }
#[inline] pub unsafe fn BlendFuncSeparate(sfactorRGB: types::GLenum, dfactorRGB: types::GLenum, sfactorAlpha: types::GLenum, dfactorAlpha: types::GLenum) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLenum, types::GLenum, types::GLenum) -> ()>(storage::BlendFuncSeparate.f)(sfactorRGB, dfactorRGB, sfactorAlpha, dfactorAlpha) }
#[inline] pub unsafe fn Viewport(x: types::GLint, y: types::GLint, width: types::GLsizei, height: types::GLsizei) -> () { mem::transmute::<_, extern "system" fn(types::GLint, types::GLint, types::GLsizei, types::GLsizei) -> ()>(storage::Viewport.f)(x, y, width, height) }
#[inline] pub unsafe fn BindFramebuffer(target: types::GLenum, framebuffer: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLuint) -> ()>(storage::BindFramebuffer.f)(target, framebuffer) }
#[inline] pub unsafe fn ClearDepth(depth: types::GLdouble) -> () { mem::transmute::<_, extern "system" fn(types::GLdouble) -> ()>(storage::ClearDepth.f)(depth) }
#[inline] pub unsafe fn ClearColor(red: types::GLfloat, green: types::GLfloat, blue: types::GLfloat, alpha: types::GLfloat) -> () { mem::transmute::<_, extern "system" fn(types::GLfloat, types::GLfloat, types::GLfloat, types::GLfloat) -> ()>(storage::ClearColor.f)(red, green, blue, alpha) }
#[inline] pub unsafe fn Clear(mask: types::GLbitfield) -> () { mem::transmute::<_, extern "system" fn(types::GLbitfield) -> ()>(storage::Clear.f)(mask) }
#[inline] pub unsafe fn GenFramebuffers(n: types::GLsizei, framebuffers: *mut types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLsizei, *mut types::GLuint) -> ()>(storage::GenFramebuffers.f)(n, framebuffers) }
#[inline] pub unsafe fn GenRenderbuffers(n: types::GLsizei, renderbuffers: *mut types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLsizei, *mut types::GLuint) -> ()>(storage::GenRenderbuffers.f)(n, renderbuffers) }
#[inline] pub unsafe fn BindRenderbuffer(target: types::GLenum, renderbuffer: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLuint) -> ()>(storage::BindRenderbuffer.f)(target, renderbuffer) }
#[inline] pub unsafe fn RenderbufferStorage(target: types::GLenum, internalformat: types::GLenum, width: types::GLsizei, height: types::GLsizei) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLenum, types::GLsizei, types::GLsizei) -> ()>(storage::RenderbufferStorage.f)(target, internalformat, width, height) }
#[inline] pub unsafe fn Disable(cap: types::GLenum) -> () { mem::transmute::<_, extern "system" fn(types::GLenum) -> ()>(storage::Disable.f)(cap) }
#[inline] pub unsafe fn FramebufferRenderbuffer(target: types::GLenum, attachment: types::GLenum, renderbuffertarget: types::GLenum, renderbuffer: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLenum, types::GLenum, types::GLuint) -> ()>(storage::FramebufferRenderbuffer.f)(target, attachment, renderbuffertarget, renderbuffer) }
#[inline] pub unsafe fn FramebufferTexture2D(target: types::GLenum, attachment: types::GLenum, textarget: types::GLenum, texture: types::GLuint, level: types::GLint) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLenum, types::GLenum, types::GLuint, types::GLint) -> ()>(storage::FramebufferTexture2D.f)(target, attachment, textarget, texture, level) }
#[inline] pub unsafe fn GetShaderiv(shader: types::GLuint, pname: types::GLenum, params: *mut types::GLint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLenum, *mut types::GLint) -> ()>(storage::GetShaderiv.f)(shader, pname, params) }
#[inline] pub unsafe fn GetProgramiv(program: types::GLuint, pname: types::GLenum, params: *mut types::GLint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLenum, *mut types::GLint) -> ()>(storage::GetProgramiv.f)(program, pname, params) }
#[inline] pub unsafe fn GetShaderInfoLog(shader: types::GLuint, bufSize: types::GLsizei, length: *mut types::GLsizei, infoLog: *mut types::GLchar) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLsizei, *mut types::GLsizei, *mut types::GLchar) -> ()>(storage::GetShaderInfoLog.f)(shader, bufSize, length, infoLog) }
#[inline] pub unsafe fn GetProgramInfoLog(program: types::GLuint, bufSize: types::GLsizei, length: *mut types::GLsizei, infoLog: *mut types::GLchar) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLsizei, *mut types::GLsizei, *mut types::GLchar) -> ()>(storage::GetProgramInfoLog.f)(program, bufSize, length, infoLog) }
#[inline] pub unsafe fn GetAttribLocation(program: types::GLuint, name: *const types::GLchar) -> types::GLint { mem::transmute::<_, extern "system" fn(types::GLuint, *const types::GLchar) -> types::GLint>(storage::GetAttribLocation.f)(program, name) }
#[inline] pub unsafe fn GetUniformLocation(program: types::GLuint, name: *const types::GLchar) -> types::GLint { mem::transmute::<_, extern "system" fn(types::GLuint, *const types::GLchar) -> types::GLint>(storage::GetUniformLocation.f)(program, name) }
#[inline] pub unsafe fn CreateShader(type_: types::GLenum) -> types::GLuint { mem::transmute::<_, extern "system" fn(types::GLenum) -> types::GLuint>(storage::CreateShader.f)(type_) }            
#[inline] pub unsafe fn ShaderSource(shader: types::GLuint, count: types::GLsizei, string: *const *const types::GLchar, length: *const types::GLint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLsizei, *const *const types::GLchar, *const types::GLint) -> ()>(storage::ShaderSource.f)(shader, count, string, length) }
#[inline] pub unsafe fn CompileShader(shader: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint) -> ()>(storage::CompileShader.f)(shader) }
#[inline] pub unsafe fn CreateProgram() -> types::GLuint { mem::transmute::<_, extern "system" fn() -> types::GLuint>(storage::CreateProgram.f)() }
#[inline] pub unsafe fn AttachShader(program: types::GLuint, shader: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint, types::GLuint) -> ()>(storage::AttachShader.f)(program, shader) }
#[inline] pub unsafe fn LinkProgram(program: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint) -> ()>(storage::LinkProgram.f)(program) }
#[inline] pub unsafe fn DeleteShader(shader: types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLuint) -> ()>(storage::DeleteShader.f)(shader) }
#[inline] pub unsafe fn Uniform1fv(location: types::GLint, count: types::GLsizei, value: *const types::GLfloat) -> () { mem::transmute::<_, extern "system" fn(types::GLint, types::GLsizei, *const types::GLfloat) -> ()>(storage::Uniform1fv.f)(location, count, value) }
#[inline] pub unsafe fn GenTextures(n: types::GLsizei, textures: *mut types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLsizei, *mut types::GLuint) -> ()>(storage::GenTextures.f)(n, textures) }
#[inline] pub unsafe fn TexParameteri(target: types::GLenum, pname: types::GLenum, param: types::GLint) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLenum, types::GLint) -> ()>(storage::TexParameteri.f)(target, pname, param) }
#[inline] pub unsafe fn TexImage2D(target: types::GLenum, level: types::GLint, internalformat: types::GLint, width: types::GLsizei, height: types::GLsizei, border: types::GLint, format: types::GLenum, type_: types::GLenum, pixels: *const raw::c_void) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLint, types::GLint, types::GLsizei, types::GLsizei, types::GLint, types::GLenum, types::GLenum, *const raw::c_void) -> ()>(storage::TexImage2D.f)(target, level, internalformat, width, height, border, format, type_, pixels) }
#[inline] pub unsafe fn DeleteTextures(n: types::GLsizei, textures: *const types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLsizei, *const types::GLuint) -> ()>(storage::DeleteTextures.f)(n, textures) }
#[inline] pub unsafe fn GenBuffers(n: types::GLsizei, buffers: *mut types::GLuint) -> () { mem::transmute::<_, extern "system" fn(types::GLsizei, *mut types::GLuint) -> ()>(storage::GenBuffers.f)(n, buffers) }
#[inline] pub unsafe fn BufferData(target: types::GLenum, size: types::GLsizeiptr, data: *const raw::c_void, usage: types::GLenum) -> () { mem::transmute::<_, extern "system" fn(types::GLenum, types::GLsizeiptr, *const raw::c_void, types::GLenum) -> ()>(storage::BufferData.f)(target, size, data, usage) }
#[inline] pub unsafe fn Uniform1i(location: types::GLint, v0: types::GLint) -> () { mem::transmute::<_, extern "system" fn(types::GLint, types::GLint) -> ()>(storage::Uniform1i.f)(location, v0) }
#[inline] pub unsafe fn GetError() -> types::GLenum { mem::transmute::<_, extern "system" fn() -> types::GLenum>(storage::GetError.f)() }

mod storage {
    use super::FnPtr;
    pub static mut GenVertexArrays: FnPtr = FnPtr::default();
    pub static mut BindVertexArray: FnPtr = FnPtr::default();
    pub static mut BindBuffer: FnPtr = FnPtr::default();
    pub static mut VertexAttribPointer: FnPtr = FnPtr::default();
    pub static mut EnableVertexAttribArray: FnPtr = FnPtr::default();
    pub static mut VertexAttribDivisor: FnPtr = FnPtr::default();
    pub static mut UseProgram: FnPtr = FnPtr::default();
    pub static mut ActiveTexture: FnPtr = FnPtr::default();
    pub static mut BindTexture: FnPtr = FnPtr::default();
    pub static mut DrawElementsInstanced: FnPtr = FnPtr::default();
    pub static mut Enable: FnPtr = FnPtr::default();
    pub static mut DepthFunc: FnPtr = FnPtr::default();
    pub static mut BlendEquationSeparate: FnPtr = FnPtr::default();
    pub static mut BlendFuncSeparate: FnPtr = FnPtr::default();
    pub static mut Viewport: FnPtr = FnPtr::default();
    pub static mut BindFramebuffer: FnPtr = FnPtr::default();
    pub static mut ClearDepth: FnPtr = FnPtr::default();
    pub static mut ClearColor: FnPtr = FnPtr::default();
    pub static mut Clear: FnPtr = FnPtr::default();
    pub static mut GenFramebuffers: FnPtr = FnPtr::default();
    pub static mut GenRenderbuffers: FnPtr = FnPtr::default();
    pub static mut BindRenderbuffer: FnPtr = FnPtr::default();
    pub static mut RenderbufferStorage: FnPtr = FnPtr::default();
    pub static mut Disable: FnPtr = FnPtr::default();
    pub static mut FramebufferRenderbuffer: FnPtr = FnPtr::default();
    pub static mut FramebufferTexture2D: FnPtr = FnPtr::default();
    pub static mut GetShaderiv: FnPtr = FnPtr::default();
    pub static mut GetProgramiv: FnPtr = FnPtr::default();
    pub static mut GetShaderInfoLog: FnPtr = FnPtr::default();
    pub static mut GetProgramInfoLog: FnPtr = FnPtr::default();
    pub static mut GetAttribLocation: FnPtr = FnPtr::default();
    pub static mut GetUniformLocation: FnPtr = FnPtr::default();
    pub static mut CreateShader: FnPtr = FnPtr::default();
    pub static mut ShaderSource: FnPtr = FnPtr::default();
    pub static mut CompileShader: FnPtr = FnPtr::default();
    pub static mut CreateProgram: FnPtr = FnPtr::default();
    pub static mut AttachShader: FnPtr = FnPtr::default();
    pub static mut LinkProgram: FnPtr = FnPtr::default();
    pub static mut DeleteShader: FnPtr = FnPtr::default();
    pub static mut Uniform1fv: FnPtr = FnPtr::default();
    pub static mut GenTextures: FnPtr = FnPtr::default();
    pub static mut TexParameteri: FnPtr = FnPtr::default();
    pub static mut TexImage2D: FnPtr = FnPtr::default();
    pub static mut DeleteTextures: FnPtr = FnPtr::default();
    pub static mut GenBuffers: FnPtr = FnPtr::default();
    pub static mut BufferData: FnPtr = FnPtr::default();
    pub static mut Uniform1i: FnPtr = FnPtr::default();
    pub static mut GetError: FnPtr = FnPtr::default();
}

pub unsafe fn load_with<F>(mut loadfn: F) where F: FnMut(&'static str) -> *const raw::c_void {
    storage::GenVertexArrays = FnPtr::new(metaloadfn(&mut loadfn, "glGenVertexArrays", &["glGenVertexArraysAPPLE", "glGenVertexArraysOES"]));
    storage::BindVertexArray = FnPtr::new(metaloadfn(&mut loadfn, "glBindVertexArray", &["glBindVertexArrayOES"]));
    storage::BindBuffer = FnPtr::new(metaloadfn(&mut loadfn, "glBindBuffer", &["glBindBufferARB"]));
    storage::VertexAttribPointer = FnPtr::new(metaloadfn(&mut loadfn, "glVertexAttribPointer", &["glVertexAttribPointerARB"]));
    storage::EnableVertexAttribArray = FnPtr::new(metaloadfn(&mut loadfn, "glEnableVertexAttribArray", &["glEnableVertexAttribArrayARB"]));
    storage::VertexAttribDivisor = FnPtr::new(metaloadfn(&mut loadfn, "glVertexAttribDivisor", &["glVertexAttribDivisorANGLE", "glVertexAttribDivisorARB", "glVertexAttribDivisorEXT", "glVertexAttribDivisorNV"]));
    storage::UseProgram = FnPtr::new(metaloadfn(&mut loadfn, "glUseProgram", &["glUseProgramObjectARB"]));
    storage::ActiveTexture = FnPtr::new(metaloadfn(&mut loadfn, "glActiveTexture", &["glActiveTextureARB"]));
    storage::BindTexture = FnPtr::new(metaloadfn(&mut loadfn, "glBindTexture", &["glBindTextureEXT"]));
    storage::DrawElementsInstanced = FnPtr::new(metaloadfn(&mut loadfn, "glDrawElementsInstanced", &["glDrawElementsInstancedANGLE", "glDrawElementsInstancedARB", "glDrawElementsInstancedEXT", "glDrawElementsInstancedNV"]));
    storage::Enable = FnPtr::new(metaloadfn(&mut loadfn, "glEnable", &[]));
    storage::DepthFunc = FnPtr::new(metaloadfn(&mut loadfn, "glDepthFunc", &[]));
    storage::BlendEquationSeparate = FnPtr::new(metaloadfn(&mut loadfn, "glBlendEquationSeparate", &["glBlendEquationSeparateEXT"]));
    storage::BlendFuncSeparate = FnPtr::new(metaloadfn(&mut loadfn, "glBlendFuncSeparate", &["glBlendFuncSeparateEXT", "glBlendFuncSeparateINGR"]));
    storage::Viewport = FnPtr::new(metaloadfn(&mut loadfn, "glViewport", &[]));
    storage::BindFramebuffer = FnPtr::new(metaloadfn(&mut loadfn, "glBindFramebuffer", &[]));
    storage::ClearDepth = FnPtr::new(metaloadfn(&mut loadfn, "glClearDepth", &[]));
    storage::ClearColor = FnPtr::new(metaloadfn(&mut loadfn, "glClearColor", &[]));
    storage::Clear = FnPtr::new(metaloadfn(&mut loadfn, "glClear", &[]));
    storage::GenFramebuffers = FnPtr::new(metaloadfn(&mut loadfn, "glGenFramebuffers", &["glGenFramebuffersEXT"]));
    storage::GenRenderbuffers = FnPtr::new(metaloadfn(&mut loadfn, "glGenRenderbuffers", &["glGenRenderbuffersEXT"]));
    storage::BindRenderbuffer = FnPtr::new(metaloadfn(&mut loadfn, "glBindRenderbuffer", &[]));
    storage::RenderbufferStorage = FnPtr::new(metaloadfn(&mut loadfn, "glRenderbufferStorage", &["glRenderbufferStorageEXT"]));
    storage::Disable = FnPtr::new(metaloadfn(&mut loadfn, "glDisable", &[]));
    storage::FramebufferRenderbuffer = FnPtr::new(metaloadfn(&mut loadfn, "glFramebufferRenderbuffer", &["glFramebufferRenderbufferEXT"]));
    storage::FramebufferTexture2D = FnPtr::new(metaloadfn(&mut loadfn, "glFramebufferTexture2D", &["glFramebufferTexture2DEXT"]));
    storage::GetShaderiv = FnPtr::new(metaloadfn(&mut loadfn, "glGetShaderiv", &[]));
    storage::GetProgramiv = FnPtr::new(metaloadfn(&mut loadfn, "glGetProgramiv", &[]));
    storage::GetShaderInfoLog = FnPtr::new(metaloadfn(&mut loadfn, "glGetShaderInfoLog", &[]));
    storage::GetProgramInfoLog = FnPtr::new(metaloadfn(&mut loadfn, "glGetProgramInfoLog", &[]));
    storage::GetAttribLocation = FnPtr::new(metaloadfn(&mut loadfn, "glGetAttribLocation", &["glGetAttribLocationARB"]));
    storage::GetUniformLocation = FnPtr::new(metaloadfn(&mut loadfn, "glGetUniformLocation", &["glGetUniformLocationARB"]));
    storage::CreateShader = FnPtr::new(metaloadfn(&mut loadfn, "glCreateShader", &["glCreateShaderObjectARB"]));
    storage::ShaderSource = FnPtr::new(metaloadfn(&mut loadfn, "glShaderSource", &["glShaderSourceARB"]));
    storage::CompileShader = FnPtr::new(metaloadfn(&mut loadfn, "glCompileShader", &["glCompileShaderARB"]));
    storage::CreateProgram = FnPtr::new(metaloadfn(&mut loadfn, "glCreateProgram", &["glCreateProgramObjectARB"]));
    storage::AttachShader = FnPtr::new(metaloadfn(&mut loadfn, "glAttachShader", &["glAttachObjectARB"]));
    storage::LinkProgram = FnPtr::new(metaloadfn(&mut loadfn, "glLinkProgram", &["glLinkProgramARB"]));
    storage::DeleteShader = FnPtr::new(metaloadfn(&mut loadfn, "glDeleteShader", &[]));
    storage::Uniform1fv = FnPtr::new(metaloadfn(&mut loadfn, "glUniform1fv", &["glUniform1fvARB"]));
    storage::GenTextures = FnPtr::new(metaloadfn(&mut loadfn, "glGenTextures", &[]));
    storage::TexParameteri = FnPtr::new(metaloadfn(&mut loadfn, "glTexParameteri", &[]));
    storage::TexImage2D = FnPtr::new(metaloadfn(&mut loadfn, "glTexImage2D", &[]));
    storage::DeleteTextures = FnPtr::new(metaloadfn(&mut loadfn, "glDeleteTextures", &[]));
    storage::GenBuffers = FnPtr::new(metaloadfn(&mut loadfn, "glGenBuffers", &["glGenBuffersARB"]));
    storage::BufferData = FnPtr::new(metaloadfn(&mut loadfn, "glBufferData", &["glBufferDataARB"]));
    storage::Uniform1i = FnPtr::new(metaloadfn(&mut loadfn, "glUniform1i", &["glUniform1iARB"]));
    storage::GetError = FnPtr::new(metaloadfn(&mut loadfn, "glGetError", &[]));
}

#[inline(never)]
fn metaloadfn(loadfn: &mut dyn FnMut(&'static str) -> *const raw::c_void, symbol: &'static str, fallbacks: &[&'static str]) -> *const raw::c_void {
    let mut ptr = loadfn(symbol);
    if ptr.is_null() {
        for &sym in fallbacks {
            ptr = loadfn(sym);
            if !ptr.is_null() {break;}
        }
    }
    ptr
}

pub struct FnPtr {
    f: *const raw::c_void,
}

impl FnPtr {
    /// Creates a `FnPtr` from a load attempt.
    pub fn new(ptr: *const raw::c_void) -> FnPtr {
        if ptr.is_null() {
            FnPtr {f: missing_fn_panic as *const raw::c_void}
        } else {
            FnPtr {f: ptr}
        }
    }
}

impl FnPtr{
    const fn default()->Self{Self {f: missing_fn_panic as *const raw::c_void}}
}

fn missing_fn_panic() -> !{
    panic!("gl function was not loaded")
}

