
use crate::vm::*;
use crate::value::*;
use crate::heap::*;
use crate::traits::*;
use crate::object::*;
use crate::handle::*;
use makepad_live_id::*;
use crate::function::*;
use crate::pod::*;

#[macro_export]
macro_rules!script_primitive {
    ( $ ty: ty, $new: item, $ type_check: item, $ apply: item, $ to_value: item) => {
        impl ScriptHook for $ty{}
        impl ScriptNew for $ty{
            $ new
            $ type_check
            fn script_default(vm:&mut ScriptVm)->ScriptValue{Self::script_new(vm).script_to_value(vm)}
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
    f32, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = vm.cast_to_f64(value) as _;
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self as _)}
); 

script_primitive!(
    f64, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = vm.cast_to_f64(value);
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self)}
);

script_primitive!(
    u64, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = vm.cast_to_f64(value) as u64;
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self as f64)}
);

script_primitive!(
    usize, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = vm.cast_to_f64(value) as usize;
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self as f64)}
);

script_primitive!(
    ScriptObjectRef, 
    fn script_new(vm:&mut ScriptVm)->Self{vm.heap.new_object_ref(ScriptObject::ZERO)},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_object()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(obj) = value.as_object(){
            *self = vm.heap.new_object_ref(obj)
        }
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{
        self.as_object().into()
    }
);


script_primitive!(
    ScriptFnRef, 
    fn script_new(vm:&mut ScriptVm)->Self{vm.heap.new_fn_ref(ScriptObject::ZERO)},
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if let Some(obj) = value.as_object(){
            heap.is_fn(obj)
        }
        else{
            false
        }
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(obj) = value.as_object(){
            if vm.heap.is_fn(obj){
                *self = vm.heap.new_fn_ref(obj)
            }
        }
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{
        self.as_object().into()
    }
);

script_primitive!(
    ScriptHandleRef, 
    fn script_new(vm:&mut ScriptVm)->Self{vm.heap.new_handle_ref(ScriptHandle::ZERO)},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{
        value.as_handle().is_some()
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(handle) = value.as_handle(){
            *self = vm.heap.new_handle_ref(handle)
        }
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{
        self.as_handle().into()
    }
);

script_primitive!(
    u32, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = vm.cast_to_f64(value) as u32;
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self as f64)}
);

script_primitive!(
    u16, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_number()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = vm.cast_to_f64(value) as u16;
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_f64(*self as f64)}
);


script_primitive!(
    bool, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_bool()},
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = vm.heap.cast_to_bool(value);
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{ScriptValue::from_bool(*self)}
);

script_primitive!(
    String, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{
        value.is_string_like()
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
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
    &'static str, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{
        value.is_string_like()
    },
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut Apply, _value:ScriptValue){
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
impl ScriptDeriveMarker for &'static str{}

script_primitive!(
    LiveId, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_id()},
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(id) = value.as_id(){
            *self = id
        }
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{self.into()}
);

script_primitive!(
    ScriptObject, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, value:ScriptValue)->bool{value.is_object()},
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(object) = value.as_object(){
            *self = object
        }
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{(*self).into()}
);


script_primitive!(
    ScriptValue, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(_heap:&ScriptHeap, _value:ScriptValue)->bool{true},
    fn script_apply(&mut self, _vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        *self = value
    },
    fn script_to_value(&self, _vm:&mut ScriptVm)->ScriptValue{*self}
);


script_primitive!(
    makepad_math::Vec2d, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if value.is_number(){return true}
        if let Some(pod) = value.as_pod(){
            let pod_data = &heap.pods[pod.index as usize];
            let pod_type = &heap.pod_types[pod_data.ty.index as usize];
            if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 return v.dims() == 2
            }
        }
        false
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(v) = value.as_f64(){
             self.x = v;
             self.y = v;
             return
        }
        if let Some(pod) = value.as_pod(){
             let pod_data = &vm.heap.pods[pod.index as usize];
             let pod_type = &vm.heap.pod_types[pod_data.ty.index as usize];
             if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 if v.dims() == 2 {
                     match v {
                        ScriptPodVec::Vec2f => {
                            self.x = f32::from_bits(pod_data.data[0]) as f64;
                            self.y = f32::from_bits(pod_data.data[1]) as f64;
                        },
                        ScriptPodVec::Vec2i => {
                            self.x = pod_data.data[0] as i32 as f64;
                            self.y = pod_data.data[1] as i32 as f64;
                        },
                        ScriptPodVec::Vec2u => {
                            self.x = pod_data.data[0] as f64;
                            self.y = pod_data.data[1] as f64;
                        },
                        ScriptPodVec::Vec2b => {
                            self.x = if pod_data.data[0]!=0 {1.0} else {0.0};
                            self.y = if pod_data.data[1]!=0 {1.0} else {0.0};
                        },
                        ScriptPodVec::Vec2h => {
                             self.x = crate::pod_heap::f16_to_f32(pod_data.data[0] as u16) as f64;
                             self.y = crate::pod_heap::f16_to_f32((pod_data.data[0] >> 16) as u16) as f64;
                        },
                        _ => ()
                     }
                     return
                 }
             }
        }
        let val = vm.cast_to_f64(value);
        if !val.is_nan(){
            self.x = val;
            self.y = val;
        }
    },
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let pod = vm.heap.new_pod(vm.code.builtins.pod.pod_vec2f);
        let pod_data = &mut vm.heap.pods[pod.index as usize];
        pod_data.data[0] = (self.x as f32).to_bits();
        pod_data.data[1] = (self.y as f32).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Vec2f, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if value.is_number(){return true}
        if let Some(pod) = value.as_pod(){
            let pod_data = &heap.pods[pod.index as usize];
            let pod_type = &heap.pod_types[pod_data.ty.index as usize];
            if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 return v.dims() == 2
            }
        }
        false
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(v) = value.as_f32(){
             self.x = v;
             self.y = v;
             return
        }
        if let Some(pod) = value.as_pod(){
             let pod_data = &vm.heap.pods[pod.index as usize];
             let pod_type = &vm.heap.pod_types[pod_data.ty.index as usize];
             if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 if v.dims() == 2 {
                     match v {
                        ScriptPodVec::Vec2f => {
                            self.x = f32::from_bits(pod_data.data[0]);
                            self.y = f32::from_bits(pod_data.data[1]);
                        },
                        ScriptPodVec::Vec2i => {
                            self.x = pod_data.data[0] as i32 as f32;
                            self.y = pod_data.data[1] as i32 as f32;
                        },
                        ScriptPodVec::Vec2u => {
                            self.x = pod_data.data[0] as f32;
                            self.y = pod_data.data[1] as f32;
                        },
                        ScriptPodVec::Vec2b => {
                            self.x = if pod_data.data[0]!=0 {1.0} else {0.0};
                            self.y = if pod_data.data[1]!=0 {1.0} else {0.0};
                        },
                        ScriptPodVec::Vec2h => {
                             self.x = crate::pod_heap::f16_to_f32(pod_data.data[0] as u16);
                             self.y = crate::pod_heap::f16_to_f32((pod_data.data[0] >> 16) as u16);
                        },
                        _ => ()
                     }
                     return
                 }
             }
        }
        let val = vm.cast_to_f64(value) as f32;
        if !val.is_nan(){
            self.x = val;
            self.y = val;
        }
    },
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let pod = vm.heap.new_pod(vm.code.builtins.pod.pod_vec2f);
        let pod_data = &mut vm.heap.pods[pod.index as usize];
        pod_data.data[0] = (self.x).to_bits();
        pod_data.data[1] = (self.y).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Vec3f, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if value.is_number(){return true}
        if value.is_color(){return true}
        if let Some(pod) = value.as_pod(){
            let pod_data = &heap.pods[pod.index as usize];
            let pod_type = &heap.pod_types[pod_data.ty.index as usize];
            if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 return v.dims() == 3
            }
        }
        false
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(v) = value.as_f32(){
             self.x = v;
             self.y = v;
             self.z = v;
             return
        }
        if let Some(c) = value.as_color(){
            let v = makepad_math::Vec4f::from_u32(c);
            self.x = v.x;
            self.y = v.y;
            self.z = v.z;
            return
        }
        if let Some(pod) = value.as_pod(){
             let pod_data = &vm.heap.pods[pod.index as usize];
             let pod_type = &vm.heap.pod_types[pod_data.ty.index as usize];
             if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 if v.dims() == 3 {
                     match v {
                        ScriptPodVec::Vec3f => {
                            self.x = f32::from_bits(pod_data.data[0]);
                            self.y = f32::from_bits(pod_data.data[1]);
                            self.z = f32::from_bits(pod_data.data[2]);
                        },
                        ScriptPodVec::Vec3i => {
                            self.x = pod_data.data[0] as i32 as f32;
                            self.y = pod_data.data[1] as i32 as f32;
                            self.z = pod_data.data[2] as i32 as f32;
                        },
                        ScriptPodVec::Vec3u => {
                            self.x = pod_data.data[0] as f32;
                            self.y = pod_data.data[1] as f32;
                            self.z = pod_data.data[2] as f32;
                        },
                        ScriptPodVec::Vec3b => {
                            self.x = if pod_data.data[0]!=0 {1.0} else {0.0};
                            self.y = if pod_data.data[1]!=0 {1.0} else {0.0};
                            self.z = if pod_data.data[2]!=0 {1.0} else {0.0};
                        },
                        ScriptPodVec::Vec3h => {
                             self.x = crate::pod_heap::f16_to_f32(pod_data.data[0] as u16);
                             self.y = crate::pod_heap::f16_to_f32((pod_data.data[0] >> 16) as u16);
                             self.z = crate::pod_heap::f16_to_f32(pod_data.data[1] as u16);
                        },
                        _ => ()
                     }
                     return
                 }
             }
        }
        let val = vm.cast_to_f64(value) as f32;
        if !val.is_nan(){
            self.x = val;
            self.y = val;
            self.z = val;
        }
    },
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let pod = vm.heap.new_pod(vm.code.builtins.pod.pod_vec3f);
        let pod_data = &mut vm.heap.pods[pod.index as usize];
        pod_data.data[0] = (self.x).to_bits();
        pod_data.data[1] = (self.y).to_bits();
        pod_data.data[2] = (self.z).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Vec4f, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if value.is_number(){return true}
        if value.is_color(){return true}
        if let Some(pod) = value.as_pod(){
            let pod_data = &heap.pods[pod.index as usize];
            let pod_type = &heap.pod_types[pod_data.ty.index as usize];
            if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 return v.dims() == 4
            }
        }
        false
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(v) = value.as_f32(){
             self.x = v;
             self.y = v;
             self.z = v;
             self.w = v;
             return
        }
        if let Some(c) = value.as_color(){
            let v = makepad_math::Vec4f::from_u32(c);
            *self = v;
            return
        }
        if let Some(pod) = value.as_pod(){
             let pod_data = &vm.heap.pods[pod.index as usize];
             let pod_type = &vm.heap.pod_types[pod_data.ty.index as usize];
             if let ScriptPodTy::Vec(v) = &pod_type.ty{
                 if v.dims() == 4 {
                     match v {
                        ScriptPodVec::Vec4f => {
                            self.x = f32::from_bits(pod_data.data[0]);
                            self.y = f32::from_bits(pod_data.data[1]);
                            self.z = f32::from_bits(pod_data.data[2]);
                            self.w = f32::from_bits(pod_data.data[3]);
                        },
                        ScriptPodVec::Vec4i => {
                            self.x = pod_data.data[0] as i32 as f32;
                            self.y = pod_data.data[1] as i32 as f32;
                            self.z = pod_data.data[2] as i32 as f32;
                            self.w = pod_data.data[3] as i32 as f32;
                        },
                        ScriptPodVec::Vec4u => {
                            self.x = pod_data.data[0] as f32;
                            self.y = pod_data.data[1] as f32;
                            self.z = pod_data.data[2] as f32;
                            self.w = pod_data.data[3] as f32;
                        },
                        ScriptPodVec::Vec4b => {
                            self.x = if pod_data.data[0]!=0 {1.0} else {0.0};
                            self.y = if pod_data.data[1]!=0 {1.0} else {0.0};
                            self.z = if pod_data.data[2]!=0 {1.0} else {0.0};
                            self.w = if pod_data.data[3]!=0 {1.0} else {0.0};
                        },
                        ScriptPodVec::Vec4h => {
                             self.x = crate::pod_heap::f16_to_f32(pod_data.data[0] as u16);
                             self.y = crate::pod_heap::f16_to_f32((pod_data.data[0] >> 16) as u16);
                             self.z = crate::pod_heap::f16_to_f32(pod_data.data[1] as u16);
                             self.w = crate::pod_heap::f16_to_f32((pod_data.data[1] >> 16) as u16);
                        },
                        _ => ()
                     }
                     return
                 }
             }
        }
        let val = vm.cast_to_f64(value) as f32;
        if !val.is_nan(){
            self.x = val;
            self.y = val;
            self.z = val;
            self.w = val;
        }
    },
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let pod = vm.heap.new_pod(vm.code.builtins.pod.pod_vec4f);
        let pod_data = &mut vm.heap.pods[pod.index as usize];
        pod_data.data[0] = (self.x).to_bits();
        pod_data.data[1] = (self.y).to_bits();
        pod_data.data[2] = (self.z).to_bits();
        pod_data.data[3] = (self.w).to_bits();
        pod.into()
    }
);

script_primitive!(
    makepad_math::Mat4f, 
    fn script_new(_vm:&mut ScriptVm)->Self{Default::default()},
    fn script_type_check(heap:&ScriptHeap, value:ScriptValue)->bool{
        if value.is_number(){return true}
        if let Some(pod) = value.as_pod(){
            let pod_data = &heap.pods[pod.index as usize];
            let pod_type = &heap.pod_types[pod_data.ty.index as usize];
            if let ScriptPodTy::Mat(m) = &pod_type.ty{
                 return m.dims() == (4, 4)
            }
        }
        false
    },
    fn script_apply(&mut self, vm:&mut ScriptVm, _apply:&mut Apply, value:ScriptValue){
        if let Some(v) = value.as_f32(){
             for i in 0..16 {
                 self.v[i] = v;
             }
             return
        }
        if let Some(pod) = value.as_pod(){
             let pod_data = &vm.heap.pods[pod.index as usize];
             let pod_type = &vm.heap.pod_types[pod_data.ty.index as usize];
             if let ScriptPodTy::Mat(m) = &pod_type.ty{
                 if m.dims() == (4, 4) {
                     match m {
                        ScriptPodMat::Mat4x4f => {
                            for i in 0..16 {
                                self.v[i] = f32::from_bits(pod_data.data[i]);
                            }
                        },
                        _ => ()
                     }
                     return
                 }
             }
        }
        let val = vm.cast_to_f64(value) as f32;
        if !val.is_nan(){
            for i in 0..16 {
                self.v[i] = val;
            }
        }
    },
    fn script_to_value(&self, vm:&mut ScriptVm)->ScriptValue{
        let pod = vm.heap.new_pod(vm.code.builtins.pod.pod_mat4x4f);
        let pod_data = &mut vm.heap.pods[pod.index as usize];
        for i in 0..16 {
            pod_data.data[i] = self.v[i].to_bits();
        }
        pod.into()
    }
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
    fn script_apply(&mut self, vm:&mut ScriptVm, apply:&mut Apply, value:ScriptValue){
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
