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
    pub pod_vec2u: ScriptPodType,
    pub pod_vec3u: ScriptPodType,
    pub pod_vec4u: ScriptPodType,
    pub pod_vec2i: ScriptPodType,
    pub pod_vec3i: ScriptPodType,
    pub pod_vec4i: ScriptPodType,
    pub pod_vec2h: ScriptPodType,
    pub pod_vec3h: ScriptPodType,
    pub pod_vec4h: ScriptPodType,
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

pub fn define_pod_module(heap:&mut ScriptHeap, _native:&mut ScriptNative)->ScriptPodBuiltins{
    
    let pod = heap.new_module(id!(pod));
    
    let pod_struct = script_pod_def!(heap, pod, pod_struct, struct, ScriptPodTy::UndefinedStruct, ScriptValue::NIL);
    let pod_array = script_pod_def!(heap, pod, pod_array, array, ScriptPodTy::UndefinedArray, ScriptValue::NIL);
    let pod_bool = script_pod_def!(heap, pod, pod_bool, bool, ScriptPodTy::Bool, ScriptValue::from_bool(false));
    let pod_f32 = script_pod_def!(heap, pod, pod_f32, f32, ScriptPodTy::F32, ScriptValue::from_f32(0.0));
    let pod_f16 = script_pod_def!(heap, pod, pod_f16, f16, ScriptPodTy::F16, ScriptValue::from_f16(0.0));
    let pod_u32 = script_pod_def!(heap, pod, pod_u32, u32, ScriptPodTy::U32, ScriptValue::from_u32(0));
    let pod_i32 = script_pod_def!(heap, pod, pod_i32, i32, ScriptPodTy::I32, ScriptValue::from_i32(0));
    let pod_atomic_u32 = script_pod_def!(heap, pod, pod_atomic_u32, atomic_u32, ScriptPodTy::AtomicU32, ScriptValue::from_u32(0));
    let pod_atomic_i32 = script_pod_def!(heap, pod, pod_atomic_i32, atomic_i32, ScriptPodTy::AtomicI32, ScriptValue::from_i32(0));
    let pod_vec2f = heap.pod_def_vec(pod, id_lut!(vec2f), 2, pod_f32);
    let pod_vec3f = heap.pod_def_vec(pod, id_lut!(vec3f), 3, pod_f32);
    let pod_vec4f = heap.pod_def_vec(pod, id_lut!(vec4f), 4, pod_f32);
    let pod_vec2u = heap.pod_def_vec(pod, id_lut!(vec2u), 2, pod_u32);
    let pod_vec3u = heap.pod_def_vec(pod, id_lut!(vec3u), 3, pod_u32);
    let pod_vec4u = heap.pod_def_vec(pod, id_lut!(vec4u), 4, pod_u32);
    let pod_vec2i = heap.pod_def_vec(pod, id_lut!(vec2i), 2, pod_i32);
    let pod_vec3i = heap.pod_def_vec(pod, id_lut!(vec3i), 3, pod_i32);
    let pod_vec4i = heap.pod_def_vec(pod, id_lut!(vec4i), 4, pod_i32);
    let pod_vec2h = heap.pod_def_vec(pod, id_lut!(vec2h), 2, pod_f16);
    let pod_vec3h = heap.pod_def_vec(pod, id_lut!(vec3h), 3, pod_f16);
    let pod_vec4h = heap.pod_def_vec(pod, id_lut!(vec4h), 4, pod_f16);
    
    let pod_mat2x2f = heap.pod_def_mat(pod, id_lut!(mat2x2f), 2, 2, pod_f32);
    let pod_mat2x3f = heap.pod_def_mat(pod, id_lut!(mat2x3f), 2, 3, pod_f32);
    let pod_mat2x4f = heap.pod_def_mat(pod, id_lut!(mat2x4f), 2, 4, pod_f32);
    let pod_mat3x2f = heap.pod_def_mat(pod, id_lut!(mat3x2f), 3, 2, pod_f32);
    let pod_mat3x3f = heap.pod_def_mat(pod, id_lut!(mat3x3f), 3, 3, pod_f32);
    let pod_mat3x4f = heap.pod_def_mat(pod, id_lut!(mat3x4f), 3, 4, pod_f32);
    let pod_mat4x2f = heap.pod_def_mat(pod, id_lut!(mat4x2f), 4, 2, pod_f32);
    let pod_mat4x3f = heap.pod_def_mat(pod, id_lut!(mat4x3f), 4, 3, pod_f32);
    let pod_mat4x4f = heap.pod_def_mat(pod, id_lut!(mat4x4f), 4, 4, pod_f32);
                
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
        pod_vec2u, 
        pod_vec3u, 
        pod_vec4u, 
        pod_vec2i, 
        pod_vec3i, 
        pod_vec4i, 
        pod_vec2h, 
        pod_vec3h, 
        pod_vec4h,
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
