use crate::heap::*;
use crate::makepad_live_id::live_id::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::shader_builtins::*;

pub fn define_math_module(heap: &mut ScriptHeap, native: &mut ScriptNative) {
    let math = heap.new_module(id!(math));
    define_shader_builtins(heap, math, native);
}
