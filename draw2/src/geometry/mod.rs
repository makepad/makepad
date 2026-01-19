
use {
    crate::{
        makepad_platform::*,
    },
};

pub mod geometry_gen;

//pub use geometry_gen::*;
pub fn script_run(vm:&mut ScriptVm)->ScriptValue{
    vm.heap.new_module(id!(geom));
    self::geometry_gen::script_run(vm);
    NIL
}