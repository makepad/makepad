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
mod cx_mouse_cursor;
mod anims;
mod elements;
mod math;
mod colors;
mod shader;
mod area;
mod view;
mod shadergen;
mod quad;
mod triangle;
mod text;
mod events;
mod style;

pub use crate::cx::*;
pub use crate::quad::*;
pub use crate::triangle::*;
pub use crate::text::*;
pub use crate::elements::*;
