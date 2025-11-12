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
}

pub fn define_pod_module(heap:&mut ScriptHeap, _native:&mut ScriptNative)->ScriptPodBuiltins{
    
    let pod = heap.new_module(id!(pod));
    let ps = ScriptPodBuiltins{
        pod_struct: script_pod_def!(heap, pod, pod_struct, struct, ScriptPodTy::UndefinedStruct, ScriptValue::NIL),
        pod_array: script_pod_def!(heap, pod, pod_array, array, ScriptPodTy::UndefinedArray, ScriptValue::NIL),
        pod_bool: script_pod_def!(heap, pod, pod_bool, bool, ScriptPodTy::Bool, ScriptValue::from_bool(false)),        
        pod_f32: script_pod_def!(heap, pod, pod_f32, f32, ScriptPodTy::F32, ScriptValue::from_f32(0.0)),
        pod_f16: script_pod_def!(heap, pod, pod_f16, f16, ScriptPodTy::F16, ScriptValue::from_f16(0.0)),
        pod_u32: script_pod_def!(heap, pod, pod_u32, u32, ScriptPodTy::U32, ScriptValue::from_u32(0)),
        pod_i32: script_pod_def!(heap, pod, pod_i32, i32, ScriptPodTy::I32, ScriptValue::from_i32(0)),
        pod_atomic_u32: script_pod_def!(heap, pod, pod_atomic_u32, atomic_u32, ScriptPodTy::AtomicU32, ScriptValue::from_u32(0)),
        pod_atomic_i32: script_pod_def!(heap, pod, pod_atomic_i32, atomic_i32, ScriptPodTy::AtomicI32, ScriptValue::from_i32(0))
    };
    ps
    // alright pod module.
    // lets define the f32 type
    // and vec2
}
