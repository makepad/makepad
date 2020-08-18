#![allow(dead_code)]

pub mod analyse;
pub mod ast;
pub mod builtin;
pub mod const_eval;
pub mod const_gather;
pub mod dep_analyse;
pub mod env;
pub mod error;
pub mod generate;
pub mod generate_glsl;
pub mod generate_metal;
pub mod generate_hlsl;
pub mod ident;
pub mod lex;
pub mod lhs_check;
pub mod lit;
pub mod math;
pub mod parse;
pub mod span;
pub mod swizzle;
pub mod token;
pub mod ty;
pub mod ty_check;
pub mod util;
pub mod val;
#[macro_use]
pub mod shadergen;
pub mod colors;
pub mod geometry;