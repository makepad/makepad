use crate::makepad_live_id::live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::makepad_live_id_macros::*;
use crate::native::*;
use crate::*;

pub fn define_math_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let math = heap.new_module(id!(math));
    
    native.add_fn(heap, math, id!(sin), script_args!(x=0.0), |vm, args|{
        script_value_f64!(vm, args.x).sin().into()
    });
}

pub fn define_std_module(heap:&mut ScriptHeap, native:&mut ScriptNative){
    let std = heap.new_module(id!(std));
            
    native.add_fn(heap, std, id!(assert), script_args!(v= NIL), |vm, args|{
        if let Some(x) = script_value!(vm, args.v).as_bool(){
            if x == true{
                return NIL
            }
        }
        vm.thread.trap.in_rust = false;
        vm.thread.trap.err_assert_fail()
    });
    
    native.add_fn(heap, std, id!(err), script_args!(), |vm, _args|{
        return vm.thread.last_err
    });
            
    let range = heap.new_with_proto(id!(range).into());
    heap.set_value_def(std, id!(Range).into(), range.into());
            
    native.add_fn(heap, range, id!(step), script_args!(x= 0.0), |vm, args|{
        if let Some(this) = script_value!(vm, args.this).as_object(){
            if let Some(x) = script_value!(vm, args.x).as_f64(){
                script_set_value!(vm, this.step = x);
            }
            return this.into()
        }
        NIL
    });
}


pub struct ScriptBuiltins{
    pub range: ScriptObject,
}

impl ScriptBuiltins{
    pub fn new(heap:&mut ScriptHeap)->Self{
        Self{
            range: heap.value_path(heap.modules, ids!(std.Range),&mut Default::default()).as_object().unwrap()
        }
    }
}