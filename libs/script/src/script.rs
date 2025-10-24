
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use makepad_id::*;

pub type ScriptTypeId = std::any::TypeId;

// this we implement
pub trait ScriptHook{
    fn on_new(&mut self, _vm:&mut Vm){}
    fn on_before_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
    fn on_after_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
    fn on_skip_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, _value:Value)->bool{false}
    fn on_proto_build(_vm:&mut Vm, _obj:Object, _props:&mut ScriptTypeProps){}
    fn on_proto_methods(_vm:&mut Vm, _obj:Object){}
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

#[derive(Default)]
pub struct ScriptTypeProps{
    pub(crate) props: IdMap<Id, ScriptTypeId>
}

pub struct ScriptTypeObject{
    pub(crate) type_id: ScriptTypeId,
    pub(crate) check: Box<dyn Fn(&ScriptHeap, Value)->bool>,
    pub(crate) proto: Value,
}

pub struct ScriptTypeCheck{
    pub(crate) props: ScriptTypeProps,
    pub(crate) object: Option<ScriptTypeObject>,
}

#[derive(Copy, Clone)]
pub struct ScriptTypeIndex(pub(crate) u32);


// implementation is procmacro generated
pub trait ScriptNew: ScriptApply + ScriptHook where Self:'static{
    
    fn script_type_id_static()->ScriptTypeId;
    
    fn script_new(vm:&mut Vm)->Self;
    fn script_new_apply(vm:&mut Vm, apply:&mut ApplyScope, value:Value)->Self where Self: Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, apply, value);
        s
    }
    
    fn script_default(vm:&mut Vm)->Value{
        return Self::script_proto(vm);
    }
    
    fn script_proto(vm:&mut Vm)->Value{  
        let type_id = Self::script_type_id_static();
        if let Some(check) = vm.heap.registered_type(type_id){
            return check.object.as_ref().unwrap().proto
        }
        let mut props = ScriptTypeProps::default();
        let proto = Self::script_proto_build(vm, &mut props);
        let ty_check = ScriptTypeCheck{
            object: Some(ScriptTypeObject{
                type_id,
                proto,
                check: Box::new(Self::script_type_check),
            }),
            props
        };
        let ty_index = vm.heap.register_type(Some(type_id), ty_check);
        if let Some(obj) = proto.as_object(){
            vm.heap.freeze_with_type(obj, ty_index);
        }
        proto
    }
    
    fn script_proto_build(vm:&mut Vm, props:&mut ScriptTypeProps)->Value{
        let proto = vm.heap.new();
        // build prototype here
        Self::script_proto_props(vm, proto, props);
        Self::on_proto_build(vm, proto, props);
        Self::on_proto_methods(vm, proto);
        proto.into()
    }
    
    fn script_proto_props(_vm:&mut Vm, _object:Object, _props:&mut ScriptTypeProps){}
    
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool;
    
    fn script_api(vm:&mut Vm)->Value{
        let val = Self::script_proto(vm);
        vm.heap.freeze_api(val.into());
        val
    }
    fn script_component(vm:&mut Vm)->Value{
        let val = Self::script_proto(vm);
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

pub trait ScriptToValue: ScriptNew{
    fn script_to_value(&self, vm:&mut Vm)->Value{
        let proto = Self::script_proto(vm).into();
        let obj = vm.heap.new_with_proto(proto);
        self.script_to_value_props(vm, obj);
        obj.into()
    }
    
    fn script_to_value_props(&self, _vm:&mut Vm, _object:Object){
    } 
}

#[derive(Default)]
pub struct ApplyScope{
}


// f64


impl ScriptToValue for f64{fn script_to_value(&self, _vm:&mut Vm)->Value{Value::from_f64(*self)}}
impl ScriptHook for f64{}
impl ScriptNew for f64{
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{
        value.is_number()
    }
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_proto_build(_vm:&mut Vm, _props:&mut ScriptTypeProps)->Value{Value::from_f64(0.0)}
}
impl ScriptApply for f64{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if !value.is_nil(){
            *self = vm.cast_to_f64(value);
        }
    }
}
