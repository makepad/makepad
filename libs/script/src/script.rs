
use crate::vm::*;
use crate::value::*;
use makepad_id::*;

// this we implement
pub trait ScriptHook{
    fn on_new(&mut self, _vm:&mut Vm){}
    fn on_before_apply(&mut self, _vm:&mut Vm, _apply:&mut ScriptApply, _value:Value){}
    fn on_after_apply(&mut self, _vm:&mut Vm, _apply:&mut ScriptApply, _value:Value){}
    fn on_skip_apply(&mut self, _vm:&mut Vm, _apply:&mut ScriptApply, _value:Value)->bool{false}
    fn on_call(&mut self, _vm:&mut Vm, _method:Id, _args:Object)->Value{
        NIL
    }
}

// this is generated
pub trait ScriptNew: Script + ScriptHook{
    fn script_new(vm:&mut Vm)->Self;
    fn script_new_apply(vm:&mut Vm, apply:&mut ScriptApply, value:Value)->Self where Self: Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, apply, value);
        s
    }
    fn script_def(vm:&mut Vm)->Value;
    fn script_api(vm:&mut Vm)->Value{
        let val = Self::script_def(vm);
        vm.heap.freeze_api(val.into());
        val
    }
    fn script_component(vm:&mut Vm)->Value{
        let val = Self::script_def(vm);
        vm.heap.freeze_component(val.into());
        val
    }
}

// this as well
pub trait Script{
    fn script_apply(&mut self, vm:&mut Vm, apply:&mut ScriptApply, value:Value);
    fn script_call(&mut self, _vm:&mut Vm, _method:Id, _args:Object)->Value{
        NIL
    }
}

pub enum ScriptApplyFrom{
}

pub struct ScriptApply{
}

impl ScriptHook for f64{}
impl ScriptNew for f64{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{Value::from_f64(0.0)}
}
impl Script for f64{
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ScriptApply, value:Value){
        if !value.is_nil(){
            *self = vm.cast_to_f64(value);
        }
    }
        
    fn script_call(&mut self, vm:&mut Vm, _method:Id, _args:Object)->Value{
        vm.thread.trap.err_notfn()
    }
}
