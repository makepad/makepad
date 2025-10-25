
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use crate::traits::*;
use makepad_id::*;

#[macro_export]
macro_rules!script_primitive {
    ( $ ty: ty, $ type_check: item, $ apply: item, $ to_value: item) => {
        impl ScriptHook for $ty{}
        impl ScriptNew for $ty{
            fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
            $ type_check
            fn script_default(vm:&mut Vm)->Value{Self::script_new(vm).script_to_value(vm)}
            fn script_new(_vm:&mut Vm)->Self{Default::default()}
            fn script_proto_build(vm:&mut Vm, _props:&mut ScriptTypeProps)->Value{
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
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        *self = vm.cast_to_f64(value);
    },
    fn script_to_value(&self, _vm:&mut Vm)->Value{Value::from_f64(*self)}
);

script_primitive!(
    u32, 
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        *self = vm.cast_to_f64(value) as u32;
    },
    fn script_to_value(&self, _vm:&mut Vm)->Value{Value::from_f64(*self as f64)}
);

script_primitive!(
    bool, 
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{value.is_bool()},
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        *self = vm.heap.cast_to_bool(value);
    },
    fn script_to_value(&self, _vm:&mut Vm)->Value{Value::from_bool(*self)}
);

script_primitive!(
    String, 
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{value.is_string()},
    fn script_apply(&mut self, vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        self.clear();
        vm.heap.cast_to_string(value,self);
    },
    fn script_to_value(&self, vm:&mut Vm)->Value{
        vm.heap.new_string_from_str(self).into()
    }
);

script_primitive!(
    Id, 
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{value.is_id()},
    fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if let Some(id) = value.as_id(){
            *self = id
        }
    },
    fn script_to_value(&self, _vm:&mut Vm)->Value{self.into()}
);

script_primitive!(
    Object, 
    fn script_type_check(_heap:&ScriptHeap, value:Value)->bool{value.is_object()},
    fn script_apply(&mut self, _vm:&mut Vm, _apply:&mut ApplyScope, value:Value){
        if let Some(object) = value.as_object(){
            *self = object
        }
    },
    fn script_to_value(&self, _vm:&mut Vm)->Value{(*self).into()}
);


// Option



impl<T> ScriptHook for Option<T> where T: ScriptApply + ScriptNew  + 'static{}
impl<T> ScriptNew for  Option<T> where T: ScriptApply + ScriptNew + 'static{
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_type_check(heap:&ScriptHeap, value:Value)->bool{
        value.is_nil() || T::script_type_check(heap, value)
    }
    fn script_default(_vm:&mut Vm)->Value{NIL}
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_proto_build(_vm:&mut Vm, _props:&mut ScriptTypeProps)->Value{NIL}
}
impl<T> ScriptApply for Option<T> where T: ScriptApply + ScriptNew  + 'static{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value){
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
    fn script_to_value(&self, vm:&mut Vm)->Value{
        if let Some(s) = self{
            s.script_to_value(vm)
        }
        else{
            NIL
        }
    } 
}


// Vec



impl<T> ScriptHook for Vec<T> where T: ScriptApply + ScriptNew + 'static{}
impl<T> ScriptNew for  Vec<T> where T: ScriptApply + ScriptNew + 'static{
    fn script_type_id_static()->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_type_check(heap:&ScriptHeap, value:Value)->bool{
        if let Some(obj) = value.as_object(){
            for i in 0..heap.vec_len(obj){
                if let Some(v) = heap.vec_value_if_exist(obj, i){
                    if !T::script_type_check(heap, v){
                        return false
                    }
                }
            }
            return true
        }
        else{
            value.is_nil()
        }
    }
    fn script_default(vm:&mut Vm)->Value{
        vm.heap.new().into()
    }
    fn script_new(_vm:&mut Vm)->Self{Default::default()}
    fn script_proto_build(vm:&mut Vm, _props:&mut ScriptTypeProps)->Value{
        vm.heap.new().into()
    }
}
impl<T> ScriptApply for Vec<T> where T: ScriptApply + ScriptNew + 'static{
    fn script_type_id(&self)->ScriptTypeId{ScriptTypeId::of::<Self>()}
    fn script_apply(&mut self, vm:&mut Vm, apply:&mut ApplyScope, value:Value){
        if let Some(obj) = value.as_object(){
            let len = vm.heap.vec_len(obj);
            self.resize_with(len, || ScriptNew::script_new(vm));
            for i in 0..len{
                if let Some(v) = vm.heap.vec_value_if_exist(obj, i){
                    self[i].script_apply(vm, apply, v);
                }
            }
        }
        else if value.is_nil(){
            self.clear()
        }
    }
    fn script_to_value(&self, vm:&mut Vm)->Value{
        let obj = vm.heap.new();
        for v in self.iter(){
            let v = v.script_to_value(vm);
            vm.heap.vec_push(obj, NIL, v, &vm.thread.trap);
        }
        obj.into()
    } 
}
