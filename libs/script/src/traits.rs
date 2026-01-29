
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use crate::apply::*;
use makepad_live_id::*;


// ============================================================================
// Script traits
// ============================================================================

pub trait ScriptDeriveMarker{}

pub type ScriptTypeId = std::any::TypeId;

// sself we implement
pub trait ScriptHook{
    // these are the root entrypoints, and they by default dispatch to simpler lifecycle points
    fn on_before_apply(&mut self, _vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope, _value:ScriptValue){}
    
    fn on_before_dispatch(&mut self, vm:&mut ScriptVm, apply:&Apply, scope:&mut Scope, _value:ScriptValue){
        match apply{
            Apply::New=>self.on_before_new_scoped(vm, scope),
            Apply::Update=>self.on_before_update_scoped(vm, scope),
            Apply::Reload=>self.on_before_reload_scoped(vm, scope),
            _=>()
        }
    }
    
    fn on_after_apply(&mut self, _vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope,  _value:ScriptValue){}
    
    fn on_after_dispatch(&mut self, vm:&mut ScriptVm, apply:&Apply, scope:&mut Scope,  _value:ScriptValue){
        match apply{
            Apply::New=>self.on_after_new_scoped(vm, scope),
            Apply::Update=>self.on_after_update_scoped(vm, scope),
            Apply::Reload=>self.on_after_reload_scoped(vm, scope),
            _=>()
        }
        self.on_alive()
    }
    // allows you to provide a custom apply impl, return true to skip generated apply code    
    fn on_custom_apply(&mut self, _vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope,  _value:ScriptValue)->bool{false}
    
    // implemented by procmacro for reflection into script objects/type cchecking
    fn on_type_check(_heap:&ScriptHeap, _value:ScriptValue)->bool{false}
    fn on_proto_build(_vm:&mut ScriptVm, _obj:ScriptObject, _props:&mut ScriptTypeProps){}
    fn on_proto_methods(_vm:&mut ScriptVm, _obj:ScriptObject){}
    
    // Simple signatured lifecyclehooks
    fn on_alive(&self){} // use this hook to quickly check if your object is alive, useful for debugging
    fn on_before_new(&mut self, _vm:&mut ScriptVm){}
    fn on_before_reload(&mut self, _vm:&mut ScriptVm){}
    fn on_before_update(&mut self, _vm:&mut ScriptVm){}
    fn on_after_new(&mut self, _vm:&mut ScriptVm){}
    fn on_after_reload(&mut self, _vm:&mut ScriptVm){}
    fn on_after_update(&mut self, _vm:&mut ScriptVm){}
    
    // simple with scope
    fn on_before_new_scoped(&mut self, vm:&mut ScriptVm, _scope:&mut Scope){self.on_before_new(vm)}
    fn on_before_reload_scoped(&mut self, vm:&mut ScriptVm, _scope:&mut Scope){self.on_before_reload(vm)}
    fn on_before_update_scoped(&mut self, vm:&mut ScriptVm, _scope:&mut Scope){self.on_before_update(vm)}
    fn on_after_new_scoped(&mut self, vm:&mut ScriptVm, _scope:&mut Scope){self.on_after_new(vm)}
    fn on_after_reload_scoped(&mut self, vm:&mut ScriptVm, _scope:&mut Scope){self.on_after_reload(vm)}
    fn on_after_update_scoped(&mut self, vm:&mut ScriptVm, _scope:&mut Scope){self.on_after_update(vm)}
}

pub trait ScriptHookDeref {
    fn on_deref_before_apply(&mut self,_vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope,  _value:ScriptValue){}
    fn on_deref_after_apply(&mut self,_vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope, _value:ScriptValue){}
}

#[derive(Clone, Copy, Debug)]
pub struct ScriptTypeProp {
    pub order: u32,
    pub ty: ScriptTypeId,
}

#[derive(Default, Debug)]
pub struct ScriptTypeProps{
    pub props: LiveIdMap<LiveId, ScriptTypeProp>,
    /// Index marking where Rust instance fields begin in the props list.
    /// Fields with order < rust_instance_start are config fields (live fields before #[deref]).
    /// Fields with order >= rust_instance_start are instance fields (deref parent fields + child's fields after deref).
    /// The shader compiler uses iter_rust_instance_ordered() to process only instance fields.
    pub rust_instance_start: u32,
}

impl ScriptTypeProps {
    pub fn insert(&mut self, id: LiveId, ty: ScriptTypeId) {
        let order = self.props.len() as u32;
        self.props.insert(id, ScriptTypeProp { order, ty });
    }
    
    /// Mark the current position as where Rust instance fields begin.
    /// Called by the derive macro just before processing the #[deref] field.
    /// Config fields (live fields before #[deref]) are added to props before this call,
    /// then parent fields and child's own fields are added after.
    pub fn mark_rust_instance_start(&mut self) {
        self.rust_instance_start = self.props.len() as u32;
    }
    
    pub fn iter_ordered(&self) -> impl Iterator<Item = (LiveId, ScriptTypeId)> + '_ {
        let mut ordered: Vec<_> = self.props.iter().map(|(k, v)| (*k, *v)).collect();
        ordered.sort_by_key(|(_, prop)| prop.order);
        ordered.into_iter().map(|(id, prop)| (id, prop.ty))
    }
    
    /// Iterate over props that are part of the Rust instance data.
    /// Skips config fields (live fields before #[deref]) and returns instance fields in order:
    /// deref parent fields first, then child's own fields after deref.
    /// Used by the shader compiler to build the RustInstance struct layout.
    pub fn iter_rust_instance_ordered(&self) -> impl Iterator<Item = (LiveId, ScriptTypeId)> + '_ {
        let rust_instance_start = self.rust_instance_start;
        let mut ordered: Vec<_> = self.props.iter()
            .filter(|(_, prop)| prop.order >= rust_instance_start)
            .map(|(k, v)| (*k, *v))
            .collect();
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

#[derive(Copy, Clone, Hash, Eq, PartialEq)]
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
    /// Uses iter_rust_instance_ordered() to skip config fields before #[deref].
    fn script_pod(vm: &mut ScriptVm) -> Option<ScriptPodType> where Self: Sized {
        use crate::pod::*;
        
        // First ensure the proto is built so type reflection is available
        Self::script_proto(vm);
        
        let type_id = Self::script_type_id_static();
        let type_check = vm.heap.registered_type(type_id)?;
        
        // Build pod fields from the type props
        // Use iter_rust_instance_ordered to skip config fields (live fields before #[deref])
        let mut fields = Vec::new();
        
        for (field_name, field_type_id) in type_check.props.iter_rust_instance_ordered() {
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
    
    fn script_from_apply_value(vm:&mut ScriptVm, object:ScriptValue, id:LiveId)->Option<Self> where Self:Sized{
        if let Some(value) = vm.heap.value_for_apply(object, id.into()){
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
    
    fn script_new_with_default(vm:&mut ScriptVm)->Self where Self:Sized{
        let type_id = Self::script_type_id_static();
        if let Some(default_obj) = vm.heap.type_default_for_id(type_id){
            Self::script_from_value(vm, default_obj.into())
        }
        else{
            Self::script_new(vm)
        }
    }
    
    
    fn from_script_mod(vm:&mut ScriptVm, f:fn(&mut ScriptVm)->ScriptValue)->Self where Self:Sized{
        let value = f(vm);
        Self::script_from_value(vm, value)
    }
    
    // default impls    
    
    fn script_from_value(vm:&mut ScriptVm, value:ScriptValue)->Self where Self:Sized{
        let mut s = Self::script_new(vm);
        s.script_apply(vm, &Apply::New, &mut Scope::empty(), value);
        s
    }
    
    fn script_from_value_scoped(vm:&mut ScriptVm, scope: &mut Scope, value:ScriptValue)->Self where Self:Sized{
        let mut s = Self::script_new(vm);
        s.script_apply(vm, &Apply::New, scope, value);
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
    
    fn script_ext(vm:&mut ScriptVm)->ScriptValue{
        let val = Self::script_proto(vm);
        vm.heap.freeze_ext(val.into());
        val
    }
        
    fn script_enum_lookup_variant(vm:&mut ScriptVm, variant:LiveId)->ScriptValue{
        let rt = vm.heap.registered_type(Self::script_type_id_static()).unwrap();
        let obj = rt.object.as_ref().unwrap().proto.into();
        vm.heap.value(obj, variant.into(), vm.thread.trap.pass())
    }
}

pub trait ScriptApply{
    fn script_type_id(&self)->ScriptTypeId where Self:'static { ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope, _value:ScriptValue){}
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{NIL}
    fn script_to_value_props(&self, _vm:&mut ScriptVm, _obj:ScriptObject){}
}

pub trait ScriptApplyDefault{
    fn script_apply_default(&mut self, _vm:&mut ScriptVm, _apply:&Apply, _scope:&mut Scope, _value:ScriptValue)->Option<ScriptValue>{None}
}

pub trait ScriptReset{
    fn script_reset(&mut self, vm:&mut ScriptVm, apply:&Apply, value:ScriptValue);
}
