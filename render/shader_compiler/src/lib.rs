#![allow(dead_code)]

pub mod analyse;
pub mod ast;
pub mod builtin;
pub mod const_eval;
pub mod dep_analyse;
pub mod env;
pub mod error;
pub mod generate;
pub mod ident;
pub mod lex;
pub mod lit;
pub mod parse;
pub mod span;
pub mod swizzle;
pub mod token;
pub mod ty;
pub mod ty_check;
pub mod util;
pub mod val;
#[macro_use]
pub mod shader;
pub mod colors;

#[cfg(all(target_os = "linux"))]
pub mod generate_glsl;
#[cfg(all(target_os = "linux"))]
pub use gen_glsl::*;

#[cfg(all(target_os = "macos"))]
pub mod gen_metal;
#[cfg(all(target_os = "macos"))]
pub use gen_metal::*;
