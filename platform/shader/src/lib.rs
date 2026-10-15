//! Shared shader IR, reflection and native backend emission. Splash owns its frontend.
#![allow(clippy::approx_constant)]
pub use makepad_live_id;
pub mod shader_ids;
pub mod shader_schema;
pub mod shader_backend;
pub mod uniform_packing;
pub mod shader_ir;
pub mod shader_ir_draw;
pub mod shader_ir_glsl;
pub mod shader_ir_hlsl;
pub mod shader_ir_metal;
pub mod shader_ir_rust;
pub mod shader_ir_compute;
pub mod shader_ir_spirv;
pub mod shader_ir_text;
pub mod shader_ir_wgsl;
pub mod spirv;
pub mod shader_types;
pub mod shader_output;

