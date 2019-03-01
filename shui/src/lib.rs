#![allow(dead_code)]

// renderer specific modules
#[cfg(feature = "ogl")]
mod cx_ogl; 

#[cfg(feature = "mtl")]
mod cx_mtl; 

#[cfg(feature = "webgl")]
mod cx_webgl; 

#[cfg(any(feature = "webgl", feature = "ogl"))]
mod cx_gl; 

#[cfg(any(feature = "mtl", feature = "ogl"))]
mod cx_winit; 

// shared modules
mod cx; 
mod cx_turtle;
mod cx_fonts;
mod animation;
mod elements;
mod math;
mod colors;
mod shader;
mod area;
mod view;
mod shadergen;
mod quad;
mod text;
mod events;

pub use crate::cx::*;
pub use crate::quad::*;
pub use crate::text::*;