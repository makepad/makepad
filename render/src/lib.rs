#![allow(dead_code)]

// renderer specific modules
/*
#[cfg(target_os = "macos")]
mod cx_glsl; 
#[cfg(target_os = "macos")]
mod cx_opengl; 
#[cfg(target_os = "macos")]
mod cx_xlib; 
*/
#[cfg(target_os = "linux")]
mod cx_opengl; 
#[cfg(target_os = "linux")]
mod cx_xlib; 

#[cfg(target_os = "macos")]
mod cx_mtl; 
#[cfg(target_os = "macos")]
mod cx_mtlsl; 
#[cfg(target_os = "macos")]
mod cx_cocoa; 

#[cfg(target_os = "windows")]
mod cx_dx11; 
#[cfg(target_os = "windows")]
mod cx_hlsl;  
#[cfg(target_os = "windows")]
mod cx_win32; 

#[cfg(target_arch = "wasm32")]
mod cx_webgl; 

#[cfg(any(target_arch = "wasm32", target_os = "linux"))]
mod cx_glsl; 

#[cfg(any(target_os = "linux", target_os="macos", target_os="windows"))]
mod cx_desktop; 

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
