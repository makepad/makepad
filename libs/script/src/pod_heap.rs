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
            pod_type.cached_align_bytes = ty.align_bytes();
            pod_type.ty = ty;
            pod_type.default = default;
            ptr
        }
        else{
            let ptr = ScriptPodType{index: self.pod_types.len() as u32};
            self.pod_types.push(ScriptPodTypeData{
                cached_align_bytes: ty.align_bytes(),
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
    
    pub fn pod_def_vec(&mut self, pod_module:ScriptObject, name:LiveId, components:usize,  ty:ScriptPodType)->ScriptPodType{
        let pod_obj = self.new_with_proto(name.into());
        let pod_type = &self.pod_types[ty.index as usize];
        // lets make a struct
        let names = [id!(x), id!(y), id!(z), id!(w)];
        let mut fields = Vec::new();
        for i in 0..components{
            fields.push(ScriptPodField{
                name: names[i],
                ty: ScriptPodTypeInline{
                    self_ref: ty,
                    data: pod_type.clone()
                },
                default:NIL
            });
        }
        let mut size_bytes = components * pod_type.ty.align_bytes();
        
        // do the custom vec3 haxery blegh.
        let mut align_bytes = size_bytes;
        if size_bytes == 12{align_bytes = 16}
        else if size_bytes == 6{align_bytes = 8}
        
        let vec_ty = self.new_pod_type(ScriptPodTy::Struct{
            align_bytes,
            size_bytes,
            fields,
        }, NIL);
        self.set_object_pod_type(pod_obj, vec_ty);
        self.set_notproto(pod_obj);
        self.freeze(pod_obj);
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        return vec_ty
    }
    
        
    pub fn pod_def_mat(&mut self, pod_module:ScriptObject, name:LiveId, x:usize, y:usize,  ty:ScriptPodType)->ScriptPodType{
        let pod_obj = self.new_with_proto(name.into());
        let pod_type = &self.pod_types[ty.index as usize];
        // lets make a struct
        let names = [id!(a), id!(b), id!(c), id!(d), id!(e), id!(f), id!(g), id!(h), id!(i), id!(j),id!(k),id!(l),id!(m),id!(n),id!(o),id!(p)];
        let mut fields = Vec::new();
        for i in 0..(x*y){
            fields.push(ScriptPodField{
                name: names[i],
                ty: ScriptPodTypeInline{
                    self_ref: ty,
                    data: pod_type.clone()
                },
                default:NIL
            });
        }
        let mut size_bytes = (x*y) * pod_type.ty.align_bytes();
        
        // align of / size of table copied from wgsl spec
        let (align_bytes, size_bytes) = match (x,y){
            (2,2)=>(8,16),
            (3,2)=>(8,24),
            (4,2)=>(8,32),
            (2,3)=>(16,32),
            (3,3)=>(16,48),
            (4,3)=>(16,64),
            (2,4)=>(16,32),
            (3,4)=>(16,48),
            (4,4)=>(16,64),
            _=>panic!()
        };
                
        let vec_ty = self.new_pod_type(ScriptPodTy::Struct{
            align_bytes,
            size_bytes,
            fields,
        }, NIL);
        self.set_object_pod_type(pod_obj, vec_ty);
        self.set_notproto(pod_obj);
        self.freeze(pod_obj);
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        return vec_ty
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
        
    pub fn set_pod_field(&self, pod:ScriptPod, field:ScriptValue, _value:ScriptValue, _trap:&ScriptTrap)->ScriptValue{
        let pod = &self.pods[pod.index as usize];
        println!("WANT TO SET FIELD {}", field);
        todo!()
    }
        
        
    pub fn pod_pop_to_me(&mut self,  pod:ScriptPod, _offset:&mut ScriptPodOffset, field:ScriptValue, _value:ScriptValue, _trap:&ScriptTrap){
        let pod = &self.pods[pod.index as usize];
        let pod_type = &self.pod_types[pod.ty.index as usize];
        // alright lets write 'value' into our current offset slot
        // how do we find out which field we are at?
        
        //
        // we are at 'offset'. offset should be at the right place 'now'.
        // we have to check what type we have at 'offset'
        
        // alright so we have a struct, now we have to 'align' it
        
        // how do we do that
        // the alignment depends on the type
        // lets store the alignment in 32 bits?
        // ok how did std140 alignment go again
        // std140 is aligned to 
        // alright we have an offset, and a value
        println!("pop to me {}", _value);
        todo!()
    }
        
    pub fn pod_check_arg_total(&mut self,  _pod:ScriptPod, _offset:ScriptPodOffset, _trap:&ScriptTrap){
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
                    let mut align_bytes = 0;
                    for kv in kvs.iter().rev(){
                        if let Some((default,ty)) = self.pod_type_inline(kv.value, pod_builtins){
                            if let Some(name) = kv.key.as_id(){
                                align_bytes = align_bytes.max(ty.data.ty.align_bytes());
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
                    
                    let mut offset = 0;
                    for field in &fields{
                        let align_bytes = field.ty.data.ty.align_bytes();
                        let size_bytes =  field.ty.data.ty.size_bytes();
                        let rem = offset % align_bytes;
                        if rem != 0{ // align offset
                            offset += (align_bytes - rem)
                        }
                        offset += size_bytes;
                    }
                    // align final offset
                    let rem = offset % align_bytes;
                    if rem != 0{
                        offset += (align_bytes - rem)
                    }
                    
                    let pt = self.new_pod_type(ScriptPodTy::Struct{
                        align_bytes,
                        size_bytes: offset,
                        fields,
                    }, NIL);
                    self.set_object_pod_type(ptr, pt);
                    self.set_notproto(ptr);
                    self.freeze(ptr);
                }
                x=>{
                    trap.err_pod_type_not_extendable();
                    return
                }
            }
        }
    }
}
