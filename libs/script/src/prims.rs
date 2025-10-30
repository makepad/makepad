
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use crate::traits::*;
use makepad_live_id::*;

#[macro_export]
macro_rules!script_primitive {
    ( $ ty: ty, $ type_check: item, $ apply: item, $ to_value: item) => {
        impl ScriptHook for $ty{}
        impl ScriptNew for $ty{
            fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
            $ type_check
            fn script_default(vm:&mut ScriptVm)->ScriptValue{Self::script_new(vm).script_to_value(vm)}
            fn script_new(_vm:&mut ScriptVm)->Self{Default::default()}
            fn script_proto_build(vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{
                 Self::script_default(vm)
            }
        }
        impl ScriptApply for $ty{
            fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
            $apply
            $to_value
        }
    }
}

script_primitive!(
    f64, 
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        *self = vm.cast_to_f64(value);
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self)}
);

script_primitive!(
    u32, 
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        *self = vm.cast_to_f64(value) as u32;
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self as f64)}
);

script_primitive!(
    bool, 
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_bool()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        *self = vm.heap.cast_to_bool(value);
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_bool(*self)}
);

script_primitive!(
    String, 
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{
        value.is_string_like()
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        self.clear();
        vm.heap.cast_to_string(value,self);
    },
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        if let Some(val) = ScriptValue::from_inline_string(&self){
            return val
        }
        else{
            vm.heap.new_string_from_str(self).into()
        }
    }
);
impl ScriptDeriveMarker for String{}

script_primitive!(
    LiveId, 
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_id()},
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        if let Some(id) = value.as_id(){
            *self = id
        }
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{self.into()}
);

script_primitive!(
    ScriptObject, 
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_object()},
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut ApplyScope, value:ScriptValue){
        if let Some(object) = value.as_object(){
            *self = object
        }
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{(*self).into()}
);


// Option



impl<T> ScriptHook for Option<T> where T: ScriptApply + ScriptNew  + 'static{}
impl<T> ScriptNew for  Option<T> where T: ScriptApply + ScriptNew + 'static{
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        value.is_nil() || T::script_type_check(heap, value)
    }
    fn script_default(_vm:&mut ScriptVm)->ScriptValue{NIL}
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()}
    fn script_proto_build(_vm:&mut ScriptVm, _props:&mut ScriptTypeProps)->ScriptValue{NIL}
}
impl<T> ScriptApply for Option<T> where T: ScriptApply + ScriptNew  + 'static{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&mut ApplyScope, value:ScriptValue){
        if let Some(v) = self{
            if value.is_nil(){
                *self = None
            }
            else{
                v.script_apply(vm, apply, value);
            }
        }
        else{
            if !value.is_nil(){
                let mut inner = T::script_new(vm);
                inner.script_apply(vm, apply, value);
                *self = Some(inner);
            }
        }
    }
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        if let Some(s) = self{
            s.script_to_value(vm)
        }
        else{
            NIL
        }
    } 
}
