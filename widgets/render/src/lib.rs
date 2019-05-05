#![allow(dead_code)]

// renderer specific modules
#[cfg(feature = "ogl")]
mod cx_ogl; 

#[cfg(feature = "mtl")]
mod cx_mtl; 
#[cfg(feature = "mtl")]
mod cx_mtlsl; 
#[cfg(feature = "mtl")]
mod cx_cocoa; 

#[cfg(feature = "webgl")]
mod cx_webgl; 

#[cfg(any(feature = "webgl", feature = "ogl"))]
mod cx_glsl; 

#[cfg(any(feature = "ogl", feature="mtl"))]
mod cx_desktop; 

// shared modules
mod cx; 
mod cx_turtle;
mod cx_fonts;
mod cx_cursor;
mod cx_drawlist; 
mod animator;
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
pub use crate::elements::*;
