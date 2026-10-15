// Shader-language constants intentionally preserve their published numeric
// spellings rather than depending on Rust's host-side constants.
#![allow(clippy::approx_constant)]

pub use makepad_error_log;
pub use makepad_live_id;
pub use makepad_live_id::makepad_live_id_macros;
pub use makepad_math;
pub use makepad_math::makepad_micro_serde;
pub use makepad_regex;
pub use makepad_script_derive;

#[macro_export]
macro_rules! script_eval {
    ($vm:expr, { $($tt:tt)* } $(,)?) => {{
        ($vm).with_vm(|vm|{let b = $crate::script! { $($tt)* };vm.eval(b)})
    }};
}

pub mod census;
pub mod heap_tag;
pub mod domain;
pub mod lazy_modules;
pub mod colorhex;
mod clock;
pub mod docs;
pub mod inspect_registry;
pub mod equality;
pub mod gen_index;
pub mod heap;
pub mod literal;
pub mod mod_gc;
pub mod mod_html;
pub mod mod_math;
pub mod mod_pod;
pub mod mod_regex;
pub mod mod_shader;
pub mod mod_std;
pub mod object;
pub mod object_heap;
pub mod parser;
pub mod regex;
pub mod regex_heap;
pub mod string;
pub mod string_heap;
pub mod syntax;
pub mod tokenizer;
#[macro_use]
pub mod native;
pub mod apply;
pub mod array;
pub mod array_heap;
pub mod bounded;
pub mod function;
pub mod gc;
pub mod glyph_coverage;
pub mod handle;
pub mod json;
mod key_code;
pub mod numeric;
pub mod opcode;
pub mod opcodes;
pub mod opcodes_assign;
pub mod opcodes_calls;
pub mod opcodes_control;
pub mod opcodes_loops;
pub mod opcodes_ops;
pub mod opcodes_slots;
pub mod opcodes_vars;
pub mod pod;
pub mod pod_heap;
pub mod prims;
pub mod roots;
pub mod shader;
mod shader_ast;
pub mod shader_backend;
pub mod shader_builtins;
pub use makepad_shader::shader_ir_hlsl;
pub use makepad_shader::shader_ir_metal;
pub use makepad_shader::shader_ir_rust;
pub mod shader_output;
pub mod shader_compile;
pub mod shader_reflection;
pub mod shader_pack;
pub use makepad_shader::shader_ir;
pub mod shader_ir_draw;
pub mod shader_ir_compute;
pub use makepad_shader::shader_ir_spirv;
pub use makepad_shader::shader_ir_glsl;
pub mod shader_ir_wgsl;
pub mod shader_lower;
pub mod shader_lower_calls;
mod shader_lower_cooperative;
pub mod shader_lower_control;
pub mod shader_lower_ops;
pub mod shader_lower_vars;
pub mod shader_tables;
pub mod suggest;
pub mod test;
pub mod thread;
pub mod traits;
pub mod trap;
pub mod value;
pub mod value_map;
pub mod vec_prims;
pub mod vm;
pub mod vm_api;
pub mod vm_ctl;
pub mod vm_query;

pub use apply::*;
pub use array::*;
pub use bounded::*;
pub use docs::*;
pub use function::*;
pub use gc::*;
pub use handle::*;
pub use heap::*;
pub use makepad_live_id::*;
pub use makepad_script_derive::*;
pub use object::*;
pub use roots::{script_root_release, script_root_retain, ScriptRoots};
pub use string::*;
pub use string_heap::*;
pub use thread::*;
pub use traits::*;
pub use trap::*;
pub use value::*;
pub use vm::*;
pub use vm_api::{ScriptCrateManifests, ScriptModOverrides};

pub use makepad_shader;
mod shader_vm_types;
use makepad_shader::shader_ids;
use makepad_shader::shader_schema;
use makepad_shader::shader_types;

pub mod host;
