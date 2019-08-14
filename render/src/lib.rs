#![allow(dead_code)]

#[cfg(target_os = "linux")]
mod cx_opengl; 
#[cfg(target_os = "linux")]
mod cx_xlib; 
#[cfg(any(target_os = "linux"))]
mod cx_linux;

#[cfg(target_os = "macos")]
mod cx_metal; 
#[cfg(target_os = "macos")]
mod cx_metalsl; 
#[cfg(target_os = "macos")]
mod cx_cocoa; 
#[cfg(any(target_os = "macos"))]
mod cx_macos;

#[cfg(target_os = "windows")]
mod cx_dx12; 
#[cfg(target_os = "windows")]
mod cx_hlsl;  
#[cfg(target_os = "windows")]
mod cx_win32; 
#[cfg(any(target_os = "windows"))]
mod cx_win10;

#[cfg(target_arch = "wasm32")]
mod cx_webgl; 
#[cfg(target_arch = "wasm32")]
mod cx_wasm32; 

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
