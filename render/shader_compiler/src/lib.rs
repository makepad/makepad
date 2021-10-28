#![allow(dead_code)]

pub mod shaderparser;
pub mod shaderast;
pub mod shaderregistry;
//pub mod env;
pub mod analyse;
pub mod builtin;
pub mod const_eval;
pub mod const_gather;
pub mod dep_analyse;
pub mod ty_check;
pub mod lhs_check;
pub mod swizzle;
pub mod util;
pub mod generate;

//#[cfg(any(target_os = "linux", target_arch = "wasm32", test))]
pub mod generate_glsl;
//#[cfg(any(target_os = "macos", test))]
pub mod generate_metal;
//#[cfg(any(target_os = "windows", test))]
pub mod generate_hlsl;

pub use crate::shaderregistry::ShaderRegistry;
pub use crate::shaderast::Ty;
//pub use crate::shaderregistry::DrawShaderInput;
