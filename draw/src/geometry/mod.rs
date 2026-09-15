use crate::makepad_platform::*;

pub mod geometry_gen;

#[cfg(test)]
mod typed_vertex_test;

//pub use geometry_gen::*;
pub fn script_mod(vm: &mut ScriptVm) -> ScriptValue {
    vm.bx.heap.new_module(id!(geom));
    self::geometry_gen::script_mod(vm);
    NIL
}
