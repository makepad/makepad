use crate::makepad_live_id::live_id::*;
use crate::heap::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::*;

pub fn define_math_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let math = heap.new_module(id!(math));
    
    native.add_method(heap, math, id!(sin), script_args!(x=0.0), |vm, args|{
        script_value_f64!(vm, args.x).sin().into()
    });
}
