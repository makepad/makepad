
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use makepad_live_id::*;

pub type ScriptTypeId = std::any::TypeId;

// this we implement
pub trait ScriptHook{
    fn on_new(&mut self, _vm:&mut ScriptVm){}
    fn on_before_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut ApplyScope, _value:ScriptValue){}
    fn on_after_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut ApplyScope, _value:ScriptValue){}
    fn on_skip_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut ApplyScope, _value:ScriptValue)->bool{false}
    fn on_type_check(_heap:&ScriptHeap, _value:ScriptValue)->bool{false}
    fn on_proto_build(_vm:&mut ScriptVm, _obj:ScriptObject, _props:&mut ScriptTypeProps){}
    fn on_proto_methods(_vm:&mut ScriptVm, _obj:ScriptObject){}
}

pub trait ScriptHookDeref {
    fn on_deref_before_apply(&mut self,_vm:&mut ScriptVm, _apply:&mut ApplyScope, _value:ScriptValue){}
    fn on_deref_after_apply(&mut self,_vm:&mut ScriptVm, _apply:&mut ApplyScope, _value:ScriptValue){}
}

#[derive(Default)]
pub struct ScriptTypeProps{
    pub(crate) props: LiveIdMap<LiveId, ScriptTypeId>
}

pub struct ScriptTypeObject{
    pub(crate) type_id: ScriptTypeId,
    pub(crate) check: Box<dyn Fn(&ScriptHeap, ScriptValue)->bool>,
    pub(crate) proto: ScriptValue,
}

pub struct ScriptTypeCheck{
    pub(crate) props: ScriptTypeProps,
    pub(crate) object: Option<ScriptTypeObject>,
}

#[derive(Copy, Clone)]
pub struct ScriptTypeIndex(pub(crate) u32);


// implementation is procmacro generated
pub trait ScriptNew:  ScriptApply + ScriptHook where Self:'static{
    
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool;
    fn script_type_id_static()->ScriptTypeId;
    fn script_new(vm:&mut ScriptVm)->Self;
    fn script_default(vm:&mut ScriptVm)->ScriptValue;
    
    // default impls    
    
    fn script_from_value(vm:&mut ScriptVm, value:ScriptValue)->Self where Self:Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, &mut ApplyScope::default(), value);
        s
    }    
    
    fn script_new_apply(vm:&mut ScriptVm, apply:&mut ApplyScope, value:ScriptValue)->Self where Self: Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, apply, value);
        s
    }
    
    fn script_proto(vm:&mut ScriptVm)->ScriptValue{  
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
    
    fn script_proto_build(vm:&mut ScriptVm, props:&mut ScriptTypeProps)->ScriptValue{
        let proto = vm.heap.new();
        // build prototype here
        Self::script_proto_props(vm, proto, props);
        Self::on_proto_build(vm, proto, props);
        Self::on_proto_methods(vm, proto);
        proto.into()
    }
    
    fn script_proto_props(_vm:&mut ScriptVm, _object:ScriptObject, _props:&mut ScriptTypeProps){}
    
    fn script_api(vm:&mut ScriptVm)->ScriptValue{
        let val = Self::script_proto(vm);
        vm.heap.freeze_api(val.into());
        val
    }
    
    fn script_component(vm:&mut ScriptVm)->ScriptValue{
        let val = Self::script_proto(vm);
        vm.heap.freeze_component(val.into());
        val
    }
    
    fn script_enum_lookup_variant(vm:&mut ScriptVm, variant:LiveId)->ScriptValue{
        let rt = vm.heap.registered_type(Self::script_type_id_static()).unwrap();
        let obj = rt.object.as_ref().unwrap().proto.into();
        vm.heap.value(obj, variant.into(), &vm.thread.trap)
    }
}

// this as well
pub trait ScriptApply{
    fn script_type_id(&self)->ScriptTypeId;
    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&mut ApplyScope, value:ScriptValue);
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue;
}

pub trait ScriptReset{
    fn script_reset(&mut self, vm:&mut ScriptVm, apply:&mut ApplyScope, value:ScriptValue);
}


#[derive(Default)]
pub struct ApplyScope{
}