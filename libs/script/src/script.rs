
use crate::vm::*;
use crate::value::*;
use crate::makepad_id::*;

pub type ScriptTypeId = std::any::TypeId;

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

pub trait ScriptFromValue{
    fn script_from_value(vm:&mut Vm, value:Value)->Self;   
}

impl<T:ScriptNew> ScriptFromValue for T{
    fn script_from_value(vm:&mut Vm, value:Value)->Self where Self:Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, &mut ApplyScope::default(), value);
        s
    }    
}

pub struct ScriptTypeData{
    _type_id: ScriptTypeId,
    _check: Box<dyn Fn(&Vm, Value)->bool>,
    _fields: IdMap<Id, ScriptTypeData>
}

#[derive(Default)]
pub struct ScriptTypeIndex(pub(crate) u32);

pub trait ScriptTypeInfo{
    fn script_type_index(_vm:&mut Vm)->ScriptTypeIndex{
        ScriptTypeIndex(0)
    }
}

// implementation is procmacro generated
pub trait ScriptNew: ScriptApply + ScriptHook + ScriptTypeInfo{
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
    fn script_type_id(&self)->ScriptTypeId;
    fn script_apply(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value);
}

pub trait ScriptReset{
    fn script_reset(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value);
}

pub trait ScriptToValue{
    fn script_to_value(&self, vm:&mut Vm)->Value{
        let obj = vm.heap.new_tracked();
        self.script_to_value_props(vm, obj);
        obj.into()
    }
    
    fn script_to_value_props(&self, _vm:&mut Vm, _object:Object){
    } 
}

#[derive(Default)]
pub struct ApplyScope{
}


// u32

impl ScriptTypeInfo for u32{fn script_type_index(_vm:&mut Vm)->ScriptTypeIndex{ScriptTypeIndex(0)}}
impl ScriptToValue for u32{fn script_to_value(&self, _vm:&mut Vm)->Value{Value::from_f64(*self as f64)}}
impl ScriptHook for u32{}
impl ScriptNew for u32{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{
        // first we check if our     
        
        Value::from_f64(0.0)
        
    }
}
impl ScriptApply for u32{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if !value.is_nil(){
            *self = vm.cast_to_f64(value) as u32;
        }
    }
}


// f64


impl ScriptTypeInfo for f64{fn script_type_index(_vm:&mut Vm)->ScriptTypeIndex{ScriptTypeIndex(0)}}
impl ScriptToValue for f64{fn script_to_value(&self, _vm:&mut Vm)->Value{Value::from_f64(*self)}}
impl ScriptHook for f64{}
impl ScriptNew for f64{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{Value::from_f64(0.0)}
}
impl ScriptApply for f64{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if !value.is_nil(){
            *self = vm.cast_to_f64(value);
        }
    }
}

// Object

impl ScriptTypeInfo for Object{fn script_type_index(_vm:&mut Vm)->ScriptTypeIndex{ScriptTypeIndex(0)}}
impl ScriptToValue for Object{fn script_to_value(&self, _vm:&mut Vm)->Value{Value::from_object(*self)}}
impl ScriptHook for Object{}
impl ScriptNew for Object{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{Value::OBJECT_ZERO}
}
impl ScriptApply for Object{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if let Some(obj) = value.as_object(){
            *self = obj
        }
    }
}


// Value


impl ScriptTypeInfo for Value{fn script_type_index(_vm:&mut Vm)->ScriptTypeIndex{ScriptTypeIndex(0)}}
impl ScriptToValue for Value{fn script_to_value(&self, _vm:&mut Vm)->Value{*self}}
impl ScriptHook for Value{}
impl ScriptNew for Value{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{Value::NIL}
}
impl ScriptApply for Value{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        *self = value;
    }
}

/*
impl<T:ScriptToValue> ScriptToValue for Vec<T>{
    fn script_to_value(&self, vm:&mut Vm)->Value{
        *self
    }
}

impl<T:ScriptHook> ScriptHook for Vec<T>{}
impl<T:ScriptNew> ScriptNew for Vec<T>{
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_def(_vm:&mut Vm)->Value{
    }
}

impl<T:ScriptApply> ScriptApply for Vec<T>{
    fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        *self = value;
    }
}*/
