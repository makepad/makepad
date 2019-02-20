#![allow(dead_code)]

// renderer specific modules
#[cfg(feature = "ogl")]
#[path="cx_ogl.rs"]
mod cx; 
#[cfg(feature = "ogl")]
#[path="cxshaders_ogl.rs"]
mod cxshaders; 
#[cfg(feature = "ogl")]
#[path="cxtextures_ogl.rs"]
mod cxtextures;

#[cfg(feature = "mtl")]
#[path="cx_mtl.rs"]
mod cx; 
#[cfg(feature = "mtl")]
#[path="cxshaders_mtl.rs"]
mod cxshaders; 
#[cfg(feature = "mtl")]
#[path="cxtextures_mtl.rs"]
mod cxtextures;

mod cxshaders_shared;

// shared modules
mod cxdrawing;
mod cxfonts;
mod cxturtle;

mod math;
mod shader;

mod rect;
mod text;
mod button;

pub use crate::cx::*;
pub use crate::cxdrawing::*;
pub use crate::button::*;