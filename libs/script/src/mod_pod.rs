#![allow(unused)]
use makepad_live_id::*;
use crate::value::*;
use crate::heap::*;
use crate::native::*;
use crate::pod::*;

#[macro_export]
macro_rules! script_pod_def{
    ($heap:expr, $pod: expr, $id:ident, $pod_ty:expr, $pod_def:expr )=>{
        let pod_obj = $heap.new_object();
        let pt = $heap.new_pod_type($pod_ty, $pod_def);
        $heap.set_object_storage_vec2(pod_obj);
        $heap.set_object_pod_type(pod_obj, pt); 
        $heap.set_value_def($pod, id!($id).into(), pod_obj.into());
    };
}

pub fn define_pod_module(heap:&mut ScriptHeap, _native:&mut ScriptNative){
    
    let pod = heap.new_module(id!(pod));
    
    script_pod_def!(heap, pod, struct, ScriptPodTy::NIL, ScriptValue::NIL);
    script_pod_def!(heap, pod, f32, ScriptPodTy::F32, ScriptValue::NIL);
    // alright pod module.
    // lets define the f32 type
    // and vec2
}
