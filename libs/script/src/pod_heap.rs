#![allow(unused)]
use makepad_live_id::*;
use crate::value::*;
use crate::heap::*;
use crate::value::*;
use crate::trap::*;
use crate::mod_pod::*;
use crate::pod::*;

impl ScriptHeap{
        
    pub fn new_pod_type(&mut self, ty:ScriptPodTy, default:ScriptValue)->ScriptPodType{
        if let Some(ptr) = self.pod_types_free.pop(){
            let pod_type = &mut self.pod_types[ptr.index as usize];
            pod_type.ty = ty;
            pod_type.default = default;
            ptr
        }
        else{
            let ptr = ScriptPodType{index: self.pod_types.len() as u32};
            self.pod_types.push(ScriptPodTypeData{
                ty,
                default
            });
            ptr
        }
    }
            
    pub fn new_pod(&mut self, ty:ScriptPodType)->ScriptPod{
        if let Some(ptr) = self.pods_free.pop(){
            let pod = &mut self.pods[ptr.index as usize];
            pod.ty = ty;
            pod.tag.set_alloced();
            ptr
        }
        else{
            let ptr = ScriptPod{index: self.pods.len() as u32};
            self.pods.push(ScriptPodData{
                ty,
                ..Default::default()
            });
            ptr
        }
    }
            
    pub fn pod_type(&mut self, ty:ScriptValue)->Option<ScriptPodType>{
        if let Some(obj) = ty.as_object(){
            let object = &self.objects[obj.index as usize];
            return object.tag.as_pod_type()
        }
        None
    }
        
    pub fn pod_field(&self, _pod:ScriptPod, _field:ScriptValue, _trap:&ScriptTrap)->ScriptValue{
        todo!()
    }
        
    pub fn set_pod_field(&self, _pod:ScriptPod, _field:ScriptValue, _value:ScriptValue, _trap:&ScriptTrap)->ScriptValue{
        todo!()
    }
        
        
    pub fn pod_pop_to_me(&mut self,  _pod:ScriptPod, _offset:&mut usize, _field:ScriptValue, _value:ScriptValue, _trap:&ScriptTrap){
        todo!()
    }
        
    pub fn pod_check_arg_total(&mut self,  _pod:ScriptPod, _offset:usize, _trap:&ScriptTrap){
        todo!()
    }
    
    fn pod_type_inline(&self, val:ScriptValue, pod_builtins:&ScriptPodBuiltins)->Option<(ScriptValue,ScriptPodTypeInline)>{
        if let Some(obj) = val.as_object(){
            let object = &self.objects[obj.index as usize];
            if let Some(pt) = object.tag.as_pod_type(){
                return Some((NIL,ScriptPodTypeInline{
                    self_ref: pt,
                    data: self.pod_types[pt.index as usize].clone()
                }));
            }
        }
        if let Some(pod_ptr) = val.as_pod(){
            let pod = &self.pods[pod_ptr.index as usize];
            let object = &self.objects[pod.ty.index as usize];
            if let Some(pt) = object.tag.as_pod_type(){
                return Some((val, ScriptPodTypeInline{
                    self_ref: pt,
                    data: self.pod_types[pt.index as usize].clone()
                }));
            }
        }
        if let Some(v) = val.as_f64(){
            let pod_type = &self.pod_types[pod_builtins.pod_f32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_f32,
                data: pod_type.clone()
            }));
        }
        if let Some(v) = val.as_f32(){
            let pod_type = &self.pod_types[pod_builtins.pod_f32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_f32,
                data: pod_type.clone()
            }));
        }
        if let Some(v) = val.as_u32(){
            let pod_type = &self.pod_types[pod_builtins.pod_u32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_u32,
                data: pod_type.clone()
            }));
        }
        if let Some(v) = val.as_i32(){
            let pod_type = &self.pod_types[pod_builtins.pod_i32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_i32,
                data: pod_type.clone()
            }));
        }
        if let Some(v) = val.as_f16(){
            let pod_type = &self.pod_types[pod_builtins.pod_f16.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_f16,
                data: pod_type.clone()
            }));
        }
        if let Some(v) = val.as_bool(){
            let pod_type = &self.pod_types[pod_builtins.pod_bool.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_bool,
                data: pod_type.clone()
            }));
        }
        None
    }
    pub fn finalize_maybe_pod_type(&mut self, ptr:ScriptObject, pod_builtins:&ScriptPodBuiltins, trap:&ScriptTrap){
        let object = &self.objects[ptr.index as usize];
        if object.tag.is_pod_type(){
            let mut kvs = Vec::new();
            let mut pod_type = id!(pod_unknown);
            let mut walk = ptr;
            loop{
                let object = &self.objects[walk.index as usize];
                if object.tag.is_vec2(){
                    for kv in object.vec.iter().rev(){
                        kvs.push(kv);
                    }
                }
                if let Some(next_ptr) = object.proto.as_object(){
                    walk = next_ptr
                }
                else {
                    pod_type = object.proto.as_id().unwrap_or(pod_type);
                    break;
                } 
            }
            // alright we have our properties
            // now lets build a pod_type from it
            match pod_type{
                id!(pod_array)=>{
                    if kvs.len() == 1{
                        if let Some((_,ty)) = self.pod_type_inline(kvs[0].value, pod_builtins){
                            let pt = self.new_pod_type(ScriptPodTy::VariableArray{
                                ty: Box::new(ty)
                            }, NIL);
                            self.set_object_pod_type(ptr, pt);
                            self.set_notproto(ptr);
                            self.freeze(ptr);
                            return
                        }
                    }
                    else if kvs.len() ==2 {
                        if let Some((_,ty)) = self.pod_type_inline(kvs[1].value, pod_builtins){
                            if let Some(len) = kvs[0].value.as_number(){
                                let pt = self.new_pod_type(ScriptPodTy::FixedArray{
                                    ty: Box::new(ty),
                                    len: len as _
                                }, NIL);
                                self.set_object_pod_type(ptr, pt);
                                self.set_notproto(ptr);
                                self.freeze(ptr);
                                return
                            }
                        }
                    }
                    trap.err_pod_array_def_incorrect();
                    return
                }
                id!(pod_struct)=>{
                    // alright lets build a struct
                    let mut fields = Vec::new();
                    for kv in kvs.iter().rev(){
                        if let Some((default,ty)) = self.pod_type_inline(kv.value, pod_builtins){
                            if let Some(name) = kv.key.as_id(){
                                fields.push(ScriptPodField{
                                    name,
                                    ty,
                                    default
                                });
                                continue
                            }
                        }
                        trap.err_pod_field_not_pod();
                    }
                    
                    let pt = self.new_pod_type(ScriptPodTy::Struct{
                        fields,
                    }, NIL);
                    self.set_object_pod_type(ptr, pt);
                    self.set_notproto(ptr);
                    self.freeze(ptr);
                    // alright how are we going to do the memory alignment/writes
                    
                }
                x=>{
                    trap.err_pod_type_not_extendable();
                    return
                }
            }
        }
    }
}
