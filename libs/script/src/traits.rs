
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
    fn on_type_check(_heap:&ScriptHeap, _value:Value)->bool{false}
    fn on_proto_build(_vm:&mut Vm, _obj:Object, _props:&mut ScriptTypeProps){}
    fn on_proto_methods(_vm:&mut Vm, _obj:Object){}
}

pub trait ScriptHookDeref {
    fn on_deref_before_apply(&mut self,_vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
    fn on_deref_after_apply(&mut self,_vm:&mut Vm, _apply:&mut ApplyScope, _value:Value){}
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
pub trait ScriptNew:  ScriptApply + ScriptHook where Self:'static{
    
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool;
    fn script_type_id_static()->ScriptTypeId;
    fn script_new(vm:&mut Vm)->Self;
    fn script_default(vm:&mut Vm)->Value;
    
    // default impls    
    
    fn script_from_value(vm:&mut Vm, value:Value)->Self where Self:Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, &mut ApplyScope::default(), value);
        s
    }    
    
    fn script_new_apply(vm:&mut Vm, apply:&mut ApplyScope, value:Value)->Self where Self: Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, apply, value);
        s
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
    
    fn script_enum_lookup_variant(vm:&mut Vm, variant:Id)->Value{
        let rt = vm.heap.registered_type(Self::script_type_id_static()).unwrap();
        let obj = rt.object.as_ref().unwrap().proto.into();
        vm.heap.value(obj, variant.into(), &vm.thread.trap)
    }
}

// this as well
pub trait ScriptApply{
    fn script_type_id(&self)->ScriptTypeId;
    fn script_apply(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value);
    fn script_to_value(&self, vm:&mut Vm)->Value;
}

pub trait ScriptReset{
    fn script_reset(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value);
}


#[derive(Default)]
pub struct ApplyScope{
}