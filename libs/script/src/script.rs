
use crate::vm::*;
use crate::value::*;

// this we implement
pub trait ScriptHook{
    fn on_new(&mut self, _vm:&mut Vm){}
    fn on_before_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
    fn on_after_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
    fn on_skip_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, _value:Value)->bool{false}
    fn on_script_def(_vm:&mut Vm, _obj:Object){}
}

pub trait ScriptHookDeref {
    fn on_deref_before_apply(&mut self,_vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
    fn on_deref_after_apply(&mut self,_vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
}

pub trait FromValue{
    fn from_value(vm:&mut Vm, value:Value)->Self;   
}

impl<T:ScriptNew> FromValue for T{
    fn from_value(vm:&mut Vm, value:Value)->Self where Self:Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, &mut ApplyScope::default(), value);
        s
    }    
}

// implementation is procmacro generated
pub trait ScriptNew: ScriptApply + ScriptHook{
    fn script_new(vm:&mut Vm)->Self;
    fn script_new_apply(vm:&mut Vm, apply:&mut ApplyScope, value:Value)->Self where Self: Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, apply, value);
        s
    }
    
    fn script_def(vm:&mut Vm)->Value{
        let obj = vm.heap.new_tracked();
        Self::script_def_props(vm, obj);
        Self::on_script_def(vm, obj);
        obj.into()
    }
    
    fn script_def_props(_vm:&mut Vm, _object:Object){}
        
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
pub trait ScriptApply{
    fn script_apply(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value);
}

pub trait ScriptReset{
    fn script_reset(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value);
}

pub trait ToValue{
    fn to_value(&self, vm:&mut Vm)->Value;   
}

#[derive(Default)]
pub struct ApplyScope{
}

// f64

impl ToValue for f64{fn to_value(&self, _vm:&mut Vm)->Value{Value::from_f64(*self)}}
impl ScriptHook for f64{}
impl ScriptNew for f64{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{Value::from_f64(0.0)}
}
impl ScriptApply for f64{
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if !value.is_nil(){
            *self = vm.cast_to_f64(value);
        }
    }
}

// Object

impl ToValue for Object{fn to_value(&self, _vm:&mut Vm)->Value{Value::from_object(*self)}}
impl ScriptHook for Object{}
impl ScriptNew for Object{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{Value::OBJECT_ZERO}
}
impl ScriptApply for Object{
    fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if let Some(obj) = value.as_object(){
            *self = obj
        }
    }
}


// Value


impl ToValue for Value{fn to_value(&self, _vm:&mut Vm)->Value{*self}}
impl ScriptHook for Value{}
impl ScriptNew for Value{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{Value::NIL}
}
impl ScriptApply for Value{
    fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        *self = value;
    }
}
