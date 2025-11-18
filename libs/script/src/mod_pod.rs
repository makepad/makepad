#![allow(unused)]
use makepad_live_id::*;
use crate::value::*;
use crate::heap::*;
use crate::native::*;
use crate::pod::*;

#[macro_export]
macro_rules! script_pod_def{
    ($heap:expr, $pod: expr, $ty: ident, $id:ident, $pod_ty:expr, $pod_def:expr )=>{
        {
            let pod_obj = $heap.new_with_proto(id_lut!($ty).into());
            let pt = $heap.new_pod_type($pod_ty, $pod_def);
            $heap.set_object_storage_vec2(pod_obj);
            $heap.set_object_pod_type(pod_obj, pt); 
            $heap.set_value_def($pod, id!($id).into(), pod_obj.into());
            pt
        }
    };
}

pub struct ScriptPodBuiltins{
    pub pod_struct: ScriptPodType,
    pub pod_array: ScriptPodType,
    pub pod_bool: ScriptPodType,
    pub pod_f32: ScriptPodType,
    pub pod_f16: ScriptPodType,
    pub pod_u32: ScriptPodType,
    pub pod_i32: ScriptPodType,
    pub pod_atomic_u32: ScriptPodType, 
    pub pod_atomic_i32: ScriptPodType,
    pub pod_vec2f: ScriptPodType,
    pub pod_vec3f: ScriptPodType,
    pub pod_vec4f: ScriptPodType,
    pub pod_vec2h: ScriptPodType,
    pub pod_vec3h: ScriptPodType,
    pub pod_vec4h: ScriptPodType,
    pub pod_vec2u: ScriptPodType,
    pub pod_vec3u: ScriptPodType,
    pub pod_vec4u: ScriptPodType,
    pub pod_vec2i: ScriptPodType,
    pub pod_vec3i: ScriptPodType,
    pub pod_vec4i: ScriptPodType,
    pub pod_vec2b: ScriptPodType,
    pub pod_vec3b: ScriptPodType,
    pub pod_vec4b: ScriptPodType,
    pub pod_mat2x2f: ScriptPodType,
    pub pod_mat2x3f: ScriptPodType,
    pub pod_mat2x4f: ScriptPodType,
    pub pod_mat3x2f: ScriptPodType,
    pub pod_mat3x3f: ScriptPodType,
    pub pod_mat3x4f: ScriptPodType,
    pub pod_mat4x2f: ScriptPodType,
    pub pod_mat4x3f: ScriptPodType,
    pub pod_mat4x4f: ScriptPodType,
}

impl ScriptPodBuiltins{
    pub fn value_to_exact_type(&self, val:ScriptValue)->Option<ScriptPodType>{
        if val.is_f32(){
            return Some(self.pod_f32)
        }
        if val.is_u32(){
            return Some(self.pod_u32)
        }
        if val.is_i32(){
            return Some(self.pod_i32)
        }
        if val.is_f16(){
            return Some(self.pod_f16)
        }
        if val.is_bool(){
            return Some(self.pod_bool)
        }
        None
    }
}

pub fn define_pod_module(heap:&mut ScriptHeap, _native:&mut ScriptNative)->ScriptPodBuiltins{
    
    let pod = heap.new_module(id!(pod));
    
    let pod_struct = heap.pod_def_atom(pod, id_lut!(struct), ScriptPodTy::UndefinedStruct, id_lut!(pod_struct), ScriptValue::NIL);
    
    let pod_array = heap.pod_def_atom(pod, id_lut!(array), ScriptPodTy::UndefinedArray, id_lut!(pod_array), ScriptValue::NIL);
    
    let pod_bool = heap.pod_def_atom(pod, id_lut!(bool), ScriptPodTy::Bool, id_lut!(pod_bool), ScriptValue::from_bool(false));
    
    let pod_f32 = heap.pod_def_atom(pod, id_lut!(f32), ScriptPodTy::F32, id_lut!(pod_f32), ScriptValue::from_f32(0.0));
        
    let pod_f16 = heap.pod_def_atom(pod, id_lut!(f16), ScriptPodTy::F16, id_lut!(pod_f16), ScriptValue::from_f16(0.0));
    
    let pod_u32 = heap.pod_def_atom(pod, id_lut!(u32), ScriptPodTy::U32, id_lut!(pod_u32), ScriptValue::from_u32(0));    
    
    let pod_i32 = heap.pod_def_atom(pod, id_lut!(i32), ScriptPodTy::I32, id_lut!(pod_i32), ScriptValue::from_i32(0));    
    
    let pod_atomic_u32 = heap.pod_def_atom(pod, id_lut!(atomic_u32), ScriptPodTy::AtomicU32, id_lut!(pod_atomic_u32), ScriptValue::from_u32(0));    
    
    let pod_atomic_i32 = heap.pod_def_atom(pod, id_lut!(pod_atomic_i32), ScriptPodTy::AtomicI32, id_lut!(pod_atomic_i32), ScriptValue::from_i32(0));    
    
    let pod_vec2f = heap.pod_def_vec(pod, id_lut!(vec2f), ScriptPodVec::Vec2f);
    let pod_vec3f = heap.pod_def_vec(pod, id_lut!(vec3f), ScriptPodVec::Vec3f);
    let pod_vec4f = heap.pod_def_vec(pod, id_lut!(vec4f), ScriptPodVec::Vec4f);
    let pod_vec2u = heap.pod_def_vec(pod, id_lut!(vec2u), ScriptPodVec::Vec2u);
    let pod_vec3u = heap.pod_def_vec(pod, id_lut!(vec3u), ScriptPodVec::Vec3u);
    let pod_vec4u = heap.pod_def_vec(pod, id_lut!(vec4u), ScriptPodVec::Vec4u);
    let pod_vec2i = heap.pod_def_vec(pod, id_lut!(vec2i), ScriptPodVec::Vec2i);
    let pod_vec3i = heap.pod_def_vec(pod, id_lut!(vec3i), ScriptPodVec::Vec3i);
    let pod_vec4i = heap.pod_def_vec(pod, id_lut!(vec4i), ScriptPodVec::Vec4i);
    let pod_vec2h = heap.pod_def_vec(pod, id_lut!(vec2h), ScriptPodVec::Vec2h);
    let pod_vec3h = heap.pod_def_vec(pod, id_lut!(vec3h), ScriptPodVec::Vec3h);
    let pod_vec4h = heap.pod_def_vec(pod, id_lut!(vec4h), ScriptPodVec::Vec4h);
    let pod_vec2b = heap.pod_def_vec(pod, id_lut!(vec2b), ScriptPodVec::Vec2b);
    let pod_vec3b = heap.pod_def_vec(pod, id_lut!(vec3b), ScriptPodVec::Vec3b);
    let pod_vec4b = heap.pod_def_vec(pod, id_lut!(vec4b), ScriptPodVec::Vec4b);
        
    let pod_mat2x2f = heap.pod_def_mat(pod, id_lut!(mat2x2f), ScriptPodMat::Mat2x2f);
    let pod_mat2x3f = heap.pod_def_mat(pod, id_lut!(mat2x3f), ScriptPodMat::Mat2x3f);
    let pod_mat2x4f = heap.pod_def_mat(pod, id_lut!(mat2x4f), ScriptPodMat::Mat2x4f);
    let pod_mat3x2f = heap.pod_def_mat(pod, id_lut!(mat3x2f), ScriptPodMat::Mat3x2f);
    let pod_mat3x3f = heap.pod_def_mat(pod, id_lut!(mat3x3f), ScriptPodMat::Mat3x3f);
    let pod_mat3x4f = heap.pod_def_mat(pod, id_lut!(mat3x4f), ScriptPodMat::Mat3x4f);
    let pod_mat4x2f = heap.pod_def_mat(pod, id_lut!(mat4x2f), ScriptPodMat::Mat4x2f);
    let pod_mat4x3f = heap.pod_def_mat(pod, id_lut!(mat4x3f), ScriptPodMat::Mat4x3f);
    let pod_mat4x4f = heap.pod_def_mat(pod, id_lut!(mat4x4f), ScriptPodMat::Mat4x4f);
                
    let ps = ScriptPodBuiltins{
        pod_struct,
        pod_array,
        pod_bool,
        pod_f32,
        pod_f16,
        pod_u32,
        pod_i32,
        pod_atomic_u32,
        pod_atomic_i32,
        pod_vec2f, 
        pod_vec3f, 
        pod_vec4f, 
        pod_vec2h, 
        pod_vec3h, 
        pod_vec4h,
        pod_vec2u, 
        pod_vec3u, 
        pod_vec4u, 
        pod_vec2i, 
        pod_vec3i, 
        pod_vec4i, 
        pod_vec2b, 
        pod_vec3b, 
        pod_vec4b, 
        pod_mat2x2f,
        pod_mat2x3f,
        pod_mat2x4f,
        pod_mat3x2f,
        pod_mat3x3f,
        pod_mat3x4f,
        pod_mat4x2f,
        pod_mat4x3f,
        pod_mat4x4f,
    };
    ps
    // alright pod module.
    // lets define the f32 type
    // and vec2
}
