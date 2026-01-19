
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use makepad_live_id::*;

pub trait ScriptDeriveMarker{}

pub type ScriptTypeId = std::any::TypeId;

// sself we implement
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

#[derive(Clone, Copy, Debug)]
pub struct ScriptTypeProp {
    pub order: u32,
    pub ty: ScriptTypeId,
}

#[derive(Default, Debug)]
pub struct ScriptTypeProps{
    pub props: LiveIdMap<LiveId, ScriptTypeProp>
}

impl ScriptTypeProps {
    pub fn insert(&mut self, id: LiveId, ty: ScriptTypeId) {
        let order = self.props.len() as u32;
        self.props.insert(id, ScriptTypeProp { order, ty });
    }
    
    pub fn iter_ordered(&self) -> impl Iterator<Item = (LiveId, ScriptTypeId)> + '_ {
        let mut ordered: Vec<_> = self.props.iter().map(|(k, v)| (*k, *v)).collect();
        ordered.sort_by_key(|(_, prop)| prop.order);
        ordered.into_iter().map(|(id, prop)| (id, prop.ty))
    }
}

pub struct ScriptTypeObject{
    pub(crate) type_id: ScriptTypeId,
    pub(crate) check: Box<dyn Fn(&ScriptHeap, ScriptValue)->bool>,
    pub(crate) proto: ScriptValue,
}

pub struct ScriptTypeCheck{
    pub props: ScriptTypeProps,
    pub object: Option<ScriptTypeObject>,
}

#[derive(Copy, Clone)]
pub struct ScriptTypeIndex(pub(crate) u32);


// implementation is procmacro generated
pub trait ScriptNew:  ScriptApply + ScriptHook where Self:'static{
    
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if  <Self as ScriptHook>::on_type_check(heap, value){
            return true
        }
        if let Some(o) = value.as_object(){
            heap.type_matches_id(o, Self::script_type_id_static())
        }
        else{
            false
        }
    }
    
    /// Builds a pod struct type from the macro-generated type reflection.
    /// This iterates through the ScriptTypeProps in order and generates
    /// a ScriptPodTy::Struct with fields matching the struct's layout.
    fn script_pod(vm: &mut ScriptVm) -> Option<ScriptPodType> where Self: Sized {
        use crate::pod::*;
        
        // First ensure the proto is built so type reflection is available
        Self::script_proto(vm);
        
        let type_id = Self::script_type_id_static();
        let type_check = vm.heap.registered_type(type_id)?;
        
        // Build pod fields from the type props
        let mut fields = Vec::new();
        
        for (field_name, field_type_id) in type_check.props.iter_ordered() {
            // Try to get the pod type for this field's type
            if let Some(pod_type) = vm.heap.type_id_to_pod_type(field_type_id, &vm.code.builtins.pod) {
                let pod_type_data = vm.heap.pod_type_ref(pod_type);
                
                fields.push(ScriptPodField {
                    name: field_name,
                    ty: ScriptPodTypeInline {
                        self_ref: pod_type,
                        data: pod_type_data.clone(),
                    },
                    default: pod_type_data.default,
                });
            } else {
                // Field type doesn't have a corresponding pod type
                return None;
            }
        }
        
        // Create the pod type using the centralized layout calculation
        let pod_obj = vm.heap.new_with_proto(id!(pod_struct).into());
        vm.heap.set_object_storage_vec2(pod_obj);
        vm.heap.set_notproto(pod_obj);
        
        let pod_ty = ScriptPodTy::new_struct(fields);
        
        let pt = vm.heap.new_pod_type(pod_obj, None, pod_ty, NIL);
        vm.heap.set_object_pod_type(pod_obj, pt);
        vm.heap.freeze(pod_obj);
        
        Some(pt)
    }
    
    fn script_from_dirty(vm:&mut ScriptVm, object:ScriptValue, id:LiveId)->Option<Self> where Self:Sized{
        if let Some(value) = vm.heap.value_apply_if_dirty(object, id.into()){
            Some(ScriptNew::script_from_value(vm, value))
        }
        else{
            None
        }
    }
    
    fn script_default(vm:&mut ScriptVm)->ScriptValue where Self:Sized{
        Self::script_proto(vm);
        Self::script_new(vm).script_to_value(vm)
    }
        
    fn script_type_id_static()->ScriptTypeId{ ScriptTypeId::of::<Self>()}
    fn script_new(vm:&mut ScriptVm)->Self;
    
    fn from_script_run(vm:&mut ScriptVm, f:fn(&mut ScriptVm)->ScriptValue)->Self where Self:Sized{
        let value = f(vm);
        Self::script_from_value(vm, value)
    }
    
    // default impls    
    
    fn script_from_value(vm:&mut ScriptVm, value:ScriptValue)->Self where Self:Sized{
        let mut s = Self::script_new(vm);
        s.on_new(vm);
        s.script_apply(vm, &mut ApplyScope::default(), value);
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
            vm.heap.set_type(obj, ty_index);
        }
        proto
    }
    
    fn script_proto_build(vm:&mut ScriptVm, props:&mut ScriptTypeProps)->ScriptValue{
        let proto = vm.heap.new_object();
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
    
    fn script_shader(vm:&mut ScriptVm)->ScriptValue{
        let val = Self::script_proto(vm);
        
        vm.heap.freeze_shader(val.into());
        val
    }
    
    fn script_enum_lookup_variant(vm:&mut ScriptVm, variant:LiveId)->ScriptValue{
        let rt = vm.heap.registered_type(Self::script_type_id_static()).unwrap();
        let obj = rt.object.as_ref().unwrap().proto.into();
        vm.heap.value(obj, variant.into(), &vm.thread.trap)
    }
}

// sself as well
pub trait ScriptApply{
    fn script_type_id(&self)->ScriptTypeId where Self:'static { ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut ApplyScope, _value:ScriptValue){}
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{NIL}
    fn script_to_value_props(&self, _vm:&mut ScriptVm, _obj:ScriptObject){}
}

pub trait ScriptReset{
    fn script_reset(&mut self, vm:&mut ScriptVm, apply:&mut ApplyScope, value:ScriptValue);
}


#[derive(Default)]
pub struct ApplyScope{
}