#![allow(dead_code)]

#[cfg(all(not(feature="ipc"),target_os = "linux"))]
mod cx_opengl; 
#[cfg(all(not(feature="ipc"),target_os = "linux"))]
mod cx_xlib; 
#[cfg(all(not(feature="ipc"),any(target_os = "linux")))]
mod cx_linux;

#[cfg(all(not(feature="ipc"),target_os = "macos"))]
mod cx_metal; 
#[cfg(all(not(feature="ipc"),target_os = "macos"))]
mod cx_metalsl; 
#[cfg(all(not(feature="ipc"),target_os = "macos"))]
mod cx_cocoa; 
#[cfg(all(not(feature="ipc"),any(target_os = "macos")))]
mod cx_macos;

#[cfg(all(not(feature="ipc"),target_os = "windows"))]
mod cx_dx11; 
#[cfg(all(not(feature="ipc"),target_os = "windows"))]
mod cx_hlsl;  
#[cfg(all(not(feature="ipc"),target_os = "windows"))]
mod cx_win32; 
#[cfg(all(not(feature="ipc"),any(target_os = "windows")))]
mod cx_win10;

#[cfg(all(not(feature="ipc"),target_arch = "wasm32"))]
mod cx_webgl; 
#[cfg(all(not(feature="ipc"),target_arch = "wasm32"))]
mod cx_wasm32; 

#[cfg(all(not(feature="ipc"),any(target_arch = "wasm32", target_os = "linux")))]
mod cx_glsl; 

#[cfg(all(not(feature="ipc"),any(target_os = "linux", target_os="macos", target_os="windows")))]
mod cx_desktop; 

#[cfg(feature="ipc")]
mod cx_ipc_child;

#[cfg(feature="ipc")]
pub use crate::cx_ipc_child::*;

#[cfg(all(feature="ipc",target_arch = "wasm32"))]
mod cx_ipc_wasm32;

#[cfg(all(feature="ipc",target_arch = "wasm32"))]
pub use crate::cx_ipc_wasm32::*;

#[cfg(all(feature="ipc",any(target_os = "linux", target_os = "macos")))]
mod cx_ipc_posix;

#[cfg(all(feature="ipc",any(target_os = "linux", target_os = "macos")))]
pub use crate::cx_ipc_posix::*;

#[cfg(all(feature="ipc",target_os = "windows"))]
mod cx_ipc_win32;

#[cfg(all(feature="ipc",target_os = "windows"))]
pub use crate::cx_ipc_win32::*;


// shared modules
#[macro_use]
mod cx; 
mod cx_turtle;
mod cx_fonts;
mod cx_cursor;
mod cx_window; 
mod cx_view; 
mod cx_pass;
mod cx_texture;
mod cx_shader;
mod animator;
mod elements;
mod math;
mod colors;
mod area;
mod shadergen;
mod quad;
mod blit;
mod text;
mod events;

pub use crate::cx::*;
pub use crate::quad::*;
pub use crate::blit::*;
pub use crate::text::*;
pub use crate::elements::*;
