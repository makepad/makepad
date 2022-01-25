#![allow(dead_code)]

pub mod shader_parser;
pub mod shader_ast;
pub mod shader_registry;
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

pub use makepad_live_compiler;
pub use makepad_live_compiler::makepad_math;
pub use makepad_live_compiler::makepad_live_tokenizer;
pub use makepad_live_compiler::makepad_derive_live;
pub use makepad_live_compiler::makepad_micro_serde;

pub use {
    crate::{
        shader_ast::{
            ShaderTy,
            DrawShaderPtr,
            DrawShaderDef,
            DrawShaderFieldKind,
            DrawShaderFlags,
            DrawShaderConstTable,
            ValuePtr,
        },
        shader_registry::{
            ShaderEnum,
            ShaderRegistry,
            DrawShaderQuery
        }
    }
};

//pub use crate::shaderregistry::DrawShaderInput;
