#![allow(dead_code)]

#[macro_use]
mod cx;
#[macro_use]
mod live;

#[cfg(target_os = "linux")]
mod cx_opengl;
#[cfg(target_os = "linux")]
mod cx_xlib;
#[cfg(target_os = "linux")]
mod cx_linux;

#[cfg(target_os = "macos")]
mod cx_metal;
#[cfg(target_os = "macos")]
mod cx_cocoa;
#[cfg(target_os = "macos")]
mod cx_macos;
#[cfg(target_os = "macos")]
mod cx_apple;

#[cfg(target_os = "windows")]
mod cx_dx11;
#[cfg(target_os = "windows")]
mod cx_win32;
#[cfg(target_os = "windows")]
mod cx_windows;

#[cfg(target_arch = "wasm32")]
mod cx_webgl;
#[macro_use]
#[cfg(target_arch = "wasm32")]
mod cx_wasm32;

#[macro_use]
#[cfg(any(target_os = "linux", target_os="macos", target_os="windows"))]
mod cx_desktop;

mod cx_style;

mod turtle;
//mod fonts;
mod cursor;
mod window;
mod view;
mod pass;
mod texture;
//mod layouttypes;
//mod animator;
//mod elements;
mod area;
mod geometrygen;

mod drawquad;
//mod drawtext;
mod drawcolor;
//mod drawcube;
//mod drawimage;
mod events;
mod menu; 
mod geometry;
mod drawshader;
mod shader_std;
mod gpuinfo;

pub use crate::cx::*;
pub use crate::drawquad::*;
//pub use crate::drawtext::*;
pub use crate::drawcolor::*;
//pub use crate::drawcube::*;
//pub use crate::drawimage::*;
//pub use crate::elements::*;

