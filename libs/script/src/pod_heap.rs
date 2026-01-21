use makepad_live_id::*;
use crate::value::*;
use crate::heap::*;
use crate::trap::*;
use crate::mod_pod::*;
use crate::pod::*;
use std::fmt::Write;

impl ScriptHeap{
    
    // POD TYPES
    
    pub fn pod_method(&self, ptr:ScriptPod, key:ScriptValue, trap:&ScriptTrap)->ScriptValue{
        let pod = &self.pods[ptr.index as usize];
        let pod_ty = &self.pod_types[pod.ty.index as usize];
        self.value(pod_ty.object, key, trap)
    }
    
    pub fn new_pod_type(&mut self, object:ScriptObject, name: Option<LiveId>, ty:ScriptPodTy, default:ScriptValue)->ScriptPodType{
        if let Some(ptr) = self.pod_types_free.pop(){
            let pod_type = &mut self.pod_types[ptr.index as usize];
            pod_type.object = object;
            pod_type.name = name;
            pod_type.ty = ty;
            pod_type.default = default;
            ptr
        }
        else{
            let ptr = ScriptPodType{index: self.pod_types.len() as u32};
            self.pod_types.push(ScriptPodTypeData{
                name,
                object,
                ty,
                default
            });
            ptr
        }
    }

    pub fn new_pod_array_type(&mut self, ty:ScriptPodTy, default:ScriptValue)->ScriptPodType{
        for (i, pod_type) in self.pod_types.iter().enumerate() {
            if pod_type.ty == ty {
                return ScriptPodType{index: i as u32};
            }
        }
        
        let dummy_obj = self.new_object();
        self.new_pod_type(dummy_obj, None, ty, default)
    }
    
    
    pub fn pod_type(&self, ty:ScriptValue)->Option<ScriptPodType>{
        if let Some(obj) = ty.as_object(){
            let object = &self.objects[obj.index as usize];
            return object.tag.as_pod_type()
        }
        None
    }
    
    pub fn pod_type_ref(&self, ty:ScriptPodType)->&ScriptPodTypeData{
        &self.pod_types[ty.index as usize]
    }    
        
    pub fn pod_type_name(&self, ty:ScriptPodType)->Option<LiveId>{
        let ty = &self.pod_types[ty.index as usize];
        ty.name
    }
    
    pub fn pod_type_name_set(&mut self, ty:ScriptPodType, name:LiveId){
        let ty = &mut self.pod_types[ty.index as usize];
        ty.name = Some(name);
    }
    
    pub fn pod_type_name_if_not_set(&mut self, ty:ScriptPodType, name:LiveId){
        let ty = &mut self.pod_types[ty.index as usize];
        if ty.name.is_none(){
            ty.name = Some(name);
        }
    }
    
    /// Maps a Rust TypeId to its corresponding ScriptPodType.
    /// This handles primitive types (f32, u32, i32, bool, f64),
    /// vector types (Vec2f, Vec3f, Vec4f), matrix types (Mat4f),
    /// and looks up registered struct types that have pod representations.
    pub fn type_id_to_pod_type(&self, type_id: crate::traits::ScriptTypeId, builtins: &ScriptPodBuiltins) -> Option<ScriptPodType> {
        use std::any::TypeId;
        use makepad_math::{Vec2f, Vec3f, Vec4f, Mat4f, Quat};
        
        // Check primitive types
        if type_id == TypeId::of::<f32>() {
            return Some(builtins.pod_f32);
        }
        if type_id == TypeId::of::<f64>() {
            return Some(builtins.pod_f32); // f64 maps to f32 in pod
        }
        if type_id == TypeId::of::<u32>() {
            return Some(builtins.pod_u32);
        }
        if type_id == TypeId::of::<i32>() {
            return Some(builtins.pod_i32);
        }
        if type_id == TypeId::of::<bool>() {
            return Some(builtins.pod_bool);
        }
        
        // Check vector types (f32)
        if type_id == TypeId::of::<Vec2f>() {
            return Some(builtins.pod_vec2f);
        }
        if type_id == TypeId::of::<Vec3f>() {
            return Some(builtins.pod_vec3f);
        }
        if type_id == TypeId::of::<Vec4f>() {
            return Some(builtins.pod_vec4f);
        }
        
        // Check matrix types (f32)
        if type_id == TypeId::of::<Mat4f>() {
            return Some(builtins.pod_mat4x4f);
        }
        
        // Quat has same layout as Vec4f (x, y, z, w)
        if type_id == TypeId::of::<Quat>() {
            return Some(builtins.pod_vec4f);
        }
        
        // Check if this type has a registered ScriptTypeCheck with a pod type
        if let Some(type_index) = self.type_index.get(&type_id) {
            let type_check = &self.type_check[type_index.0 as usize];
            if let Some(ref object) = type_check.object {
                // Check if the proto object has a pod type
                if let Some(proto_obj) = object.proto.as_object() {
                    let obj_data = &self.objects[proto_obj.index as usize];
                    if let Some(pod_type) = obj_data.tag.as_pod_type() {
                        return Some(pod_type);
                    }
                }
            }
        }
        
        None
    }
        
    fn pod_type_inline(&self, val:ScriptValue, builtins:&ScriptPodBuiltins)->Option<ScriptPodTypeInline>{
        if let Some(obj) = val.as_object(){
            let object = &self.objects[obj.index as usize];
            if let Some(pt) = object.tag.as_pod_type(){
                return Some(ScriptPodTypeInline{
                    self_ref: pt,
                    data: self.pod_types[pt.index as usize].clone()
                });
            }
        }
        if let Some(pod_ptr) = val.as_pod(){
            let pod = &self.pods[pod_ptr.index as usize];
            let object = &self.objects[pod.ty.index as usize];
            if let Some(pt) = object.tag.as_pod_type(){
                return Some(ScriptPodTypeInline{
                    self_ref: pt,
                    data: self.pod_types[pt.index as usize].clone()
                });
            }
        }
        if let Some(f) = val.as_f64(){
            if f.fract() == 0.0 && f >= i32::MIN as f64 && f <= i32::MAX as f64 {
                let pod_type = &self.pod_types[builtins.pod_i32.index as usize];
                return Some(ScriptPodTypeInline{
                    self_ref: builtins.pod_i32,
                    data: pod_type.clone()
                });
            }
            else{
                let pod_type = &self.pod_types[builtins.pod_f32.index as usize];
                return Some(ScriptPodTypeInline{
                    self_ref: builtins.pod_f32,
                    data: pod_type.clone()
                });
            }
        }
        if val.is_f32(){
            let pod_type = &self.pod_types[builtins.pod_f32.index as usize];
            return Some(ScriptPodTypeInline{
                self_ref: builtins.pod_f32,
                data: pod_type.clone()
            });
        }
        if val.is_u32(){
            let pod_type = &self.pod_types[builtins.pod_u32.index as usize];
            return Some(ScriptPodTypeInline{
                self_ref: builtins.pod_u32,
                data: pod_type.clone()
            });
        }
        if val.is_i32(){
            let pod_type = &self.pod_types[builtins.pod_i32.index as usize];
            return Some(ScriptPodTypeInline{
                self_ref: builtins.pod_i32,
                data: pod_type.clone()
            });
        }
        if val.is_f16(){
            let pod_type = &self.pod_types[builtins.pod_f16.index as usize];
            return Some(ScriptPodTypeInline{
                self_ref: builtins.pod_f16,
                data: pod_type.clone()
            });
        }
        if val.is_bool(){
            let pod_type = &self.pod_types[builtins.pod_bool.index as usize];
            return Some(ScriptPodTypeInline{
                self_ref: builtins.pod_bool,
                data: pod_type.clone()
            });
        }
        None
    }
    pub fn finalize_maybe_pod_type(&mut self, ptr:ScriptObject, builtins:&ScriptPodBuiltins, trap:&ScriptTrap){
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
                        if let Some(ty) = self.pod_type_inline(kvs[0].value, builtins){
                            let pt = self.new_pod_type(ptr, None, ScriptPodTy::VariableArray{
                                align_of: ty.data.ty.align_of(),
                                ty: Box::new(ty),
                            }, NIL);
                            self.set_object_pod_type(ptr, pt);
                            self.set_notproto(ptr);
                            self.freeze(ptr);
                            return
                        }
                    }
                    else if kvs.len() ==2 {
                        if let Some(ty) = self.pod_type_inline(kvs[1].value, builtins){
                            if let Some(len) = kvs[0].value.as_number(){
                                let len = len as usize;
                                let align_of = ty.data.ty.align_of();
                                let size_of = align_of * len;
                                let rem = size_of % align_of;
                                let size_of = if rem != 0{size_of + (align_of - rem)}else{size_of};
                                                                
                                let pt = self.new_pod_type(ptr, None, ScriptPodTy::FixedArray{
                                    ty: Box::new(ty),
                                    align_of,
                                    size_of,
                                    len
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
                    let mut methods = Vec::new();
                    for kv in kvs.iter().rev(){
                        if let Some(ty) = self.pod_type_inline(kv.value, builtins){
                            if let Some(name) = kv.key.as_id(){
                                fields.push(ScriptPodField{
                                    name,
                                    ty,
                                    default: kv.value
                                });
                                continue
                            }
                        }
                        if let Some(obj) = kv.value.as_object(){
                            if self.is_fn(obj){
                                // functions are methods
                                methods.push(obj);
                                continue
                            }
                        }
                        
                        trap.err_pod_field_not_pod();
                    }
                    
                    // Use centralized layout calculation
                    let pt = self.new_pod_type(ptr, None, ScriptPodTy::new_struct(fields), NIL);
                    self.set_object_pod_type(ptr, pt);
                    self.set_notproto(ptr);
                    self.freeze(ptr);
                }
                _x=>{
                    trap.err_pod_type_not_extendable();
                    return
                }
            }
        }
    }
        
    pub fn pod_def_atom(&mut self, pod_module:ScriptObject, name: LiveId, alias:Option<LiveId>, ty: ScriptPodTy, helper_name:LiveId, default:ScriptValue)->ScriptPodType{
        let pod_obj = self.new_with_proto(helper_name.into());
        if ty != ScriptPodTy::UndefinedStruct && 
        ty != ScriptPodTy::ArrayBuilder{
            self.set_notproto(pod_obj);
        }
        let pt = self.new_pod_type(pod_obj, Some(name), ty, default);
        self.set_object_storage_vec2(pod_obj);
        self.set_object_pod_type(pod_obj, pt); 
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        if let Some(alias) = alias{
            self.set_value_def(pod_module, alias.into(), pod_obj.into());
        }
        pt
    }
        
    pub fn pod_def_vec(&mut self, pod_module:ScriptObject, name:LiveId, alias:Option<LiveId>, builtin: ScriptPodVec)->ScriptPodType{
        let pod_obj = self.new_with_proto(name.into());
        let vec_ty = self.new_pod_type(pod_obj, Some(name), ScriptPodTy::Vec(builtin), NIL);
        self.set_object_pod_type(pod_obj, vec_ty);
        self.set_notproto(pod_obj);
        self.freeze(pod_obj);
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        if let Some(alias) = alias{
            self.set_value_def(pod_module, alias.into(), pod_obj.into());
        }
        return vec_ty
    }
    
    pub fn pod_def_mat(&mut self, pod_module:ScriptObject, name:LiveId, builtin:ScriptPodMat)->ScriptPodType{
        let pod_obj = self.new_with_proto(name.into());
        let mat_ty = self.new_pod_type(pod_obj, Some(name), ScriptPodTy::Mat(builtin), NIL);
        self.set_object_pod_type(pod_obj, mat_ty);
        self.set_notproto(pod_obj);
        self.freeze(pod_obj);
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        return mat_ty
    }
    
    
    // PODS
    
    
    
    pub fn new_pod(&mut self, ty:ScriptPodType)->ScriptPod{
        let pod_ty = &self.pod_types[ty.index as usize];
        if let Some(ptr) = self.pods_free.pop(){
            let pod = &mut self.pods[ptr.index as usize];
            pod.ty = ty;
            pod.tag.set_alloced();
            pod.data.resize(pod_ty.ty.size_of().next_multiple_of(4)>>2, 0);
            ptr
        }
        else{
            let ptr = ScriptPod{index: self.pods.len() as u32};
            self.pods.push(ScriptPodData{
                ty,
                ..Default::default()
            });
            self.pods[ptr.index as usize].data.resize(pod_ty.ty.size_of().next_multiple_of(4)>>2, 0);
            ptr
        }
    }
    
    
    // POD writing
    
                
    pub fn set_pod_field(&self, pod:ScriptPod, field:ScriptValue, _value:ScriptValue, _trap:&ScriptTrap)->ScriptValue{
        let _pod = &self.pods[pod.index as usize];
        todo!("WANT TO SET FIELD {}", field);
    }
        
    pub fn pod_pop_to_me(&mut self,  pod_ptr:ScriptPod, offset:&mut ScriptPodOffset, _field:ScriptValue, value:ScriptValue, builtins:&ScriptPodBuiltins, trap:&ScriptTrap){
        let pod_ty_index = self.pods[pod_ptr.index as usize].ty.index as usize;
        
        // if we are constructing an array, set the type here
        if let ScriptPodTy::ArrayBuilder = &self.pod_types[pod_ty_index].ty {
             let ty = if let Some(ty) = self.pod_type_inline(value, builtins) { 
                 ty
             }
             else{
                 trap.err_pod_type_not_matching();
                 return
             };
             if let Some(last_ty) = self.pods[pod_ptr.index as usize].tag.as_array_builder(){
                 if last_ty != ty.self_ref{
                     trap.err_pod_type_not_matching();
                     return
                 }
             }
             else{
                 self.pods[pod_ptr.index as usize].tag.set_array_builder(ty.self_ref);
             }
        }

        let pod = &mut self.pods[pod_ptr.index as usize];
        let mut out_data = Vec::new();
        std::mem::swap(&mut out_data, &mut pod.data);
        let pod_ty = pod.ty;
        let pod_type = &self.pod_types[pod_ty.index as usize];
        // alright lets write 'value' into our current offset slot
        // our current offset slot is
        match &pod_type.ty{
            ScriptPodTy::Struct{align_of,fields,..}=>{
                // struct. ok so we are at field 
                if let Some(field) = fields.get(offset.field_index){
                    // align the field offset
                    let align_bytes = field.ty.data.ty.align_of();
                    let rem = offset.offset_of % align_of;
                    if rem != 0{
                        offset.offset_of += align_bytes - rem
                    }
                                        
                    self.pod_write_field(&field.ty, offset.offset_of, &mut out_data, value, trap);
                    // alright lets do the align and all that
                    offset.field_index += 1;
                    offset.offset_of += field.ty.data.ty.size_of();
                }
                else{
                    trap.err_pod_too_much_data();
                }
            },
            ScriptPodTy::ArrayBuilder=>{
                if let Some(ty) = self.pods[pod_ptr.index as usize].tag.as_array_builder(){
                    let pod_type = &self.pod_types[ty.index as usize];
                    let align_of = pod_type.ty.align_of();
                    let rem = offset.offset_of % align_of;
                    if rem != 0{
                        offset.offset_of += align_of - rem
                    }
                    
                    let no_u32 = (offset.offset_of + align_of)>>2;
                    if (offset.offset_of + align_of)&3>0{
                        out_data.resize(no_u32 + 1, 0);
                    }
                    else{
                        out_data.resize(no_u32, 0);
                    }
                    let element_ty = ScriptPodTypeInline{
                        self_ref: ty,
                        data: pod_type.clone()
                    };
                    self.pod_write_field(&element_ty, offset.offset_of, &mut out_data, value, trap);
                    offset.offset_of += align_of;
                }
            }
            ScriptPodTy::FixedArray{align_of:_,size_of:_,len:_,ty:_}=>{
            },
            ScriptPodTy::VariableArray{align_of:_, ty:_}=>{
            }
            ScriptPodTy::Vec(ot)=>{
                offset.offset_of += self.pod_write_vec(ot, offset.offset_of, &mut out_data, value, trap);
            }
            ScriptPodTy::Mat(mt)=>{
                if let Some(value) = value.as_number(){
                    if offset.offset_of >= mt.elem_size() * mt.dim(){
                        trap.err_pod_too_much_data();
                    }
                    else{
                        out_data[offset.offset_of>>2] = (value as f32).to_bits();
                        offset.offset_of += mt.elem_size()
                    }
                }
                else if let Some(_in_pod) = value.as_pod(){
                    trap.err_pod_type_not_matching();
                }
                else{
                    trap.err_pod_type_not_matching();
                }
            }
            _=>{
                trap.err_unexpected();
            }
        }
        std::mem::swap(&mut out_data, &mut self.pods[pod_ptr.index as usize].data);
    }
    
          
    pub fn pod_check_arg_total(&mut self,  pod:ScriptPod, offset:ScriptPodOffset, trap:&ScriptTrap){
        let pod = &mut self.pods[pod.index as usize];
        let pod_type = &self.pod_types[pod.ty.index as usize];
        let size_of = pod_type.ty.size_of();
        if offset.offset_of == 0{ // empty. lets do it zero'ed
            return
        }
        // not enough
                        
        if size_of != offset.offset_of{
            match &pod_type.ty{
                ScriptPodTy::ArrayBuilder=>{
                    return
                }
                ScriptPodTy::Vec(vt)=>{
                    if offset.offset_of == vt.elem_size(){
                        if vt.elem_size() == 2{
                            let fill = pod.data[0]&0xffff;
                            for i in 0..pod_type.ty.size_of()>>2{
                                pod.data[i] = (fill) | (fill<<16)
                            }
                        }
                        else{
                            let fill = pod.data[0];
                            for i in 1..pod_type.ty.size_of()>>2{
                                pod.data[i] = fill
                            }
                        }
                        return
                    }
                }
                ScriptPodTy::Mat(mt)=>{
                    if offset.offset_of == mt.elem_size(){
                        if mt.elem_size() == 2{
                            let fill = pod.data[0]&0xffff;
                            for i in 0..pod_type.ty.size_of()>>2{
                                pod.data[i] = (fill) | (fill<<16)
                            }
                        }
                        else{
                            let fill = pod.data[0];
                            for i in 1..pod_type.ty.size_of()>>2{
                                pod.data[i] = fill
                            }
                        }
                        return
                    }
                }
                _=>{
                }
            }
        }
        else{
            return
        }
        trap.err_pod_not_enough_data();
    }
        
    
    fn pod_write_vec(&self, ot:&ScriptPodVec, offset_of:usize, out_data:&mut[u32], value:ScriptValue, trap:&ScriptTrap)->usize{
        if let Some(value) = value.as_number(){
            if offset_of >= ot.elem_size() * ot.dims(){
                trap.err_pod_too_much_data();
                return 0
            }
            else{
                let o = offset_of;
                let o2 = o>>2;
                match ot{
                    ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>{
                        out_data[o2] = (value as f32).to_bits();
                    }
                    ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>{
                        let u = f32_to_f16(value as f32);
                        if o & 3 >= 2{
                            out_data[o>>2] |= (u as u32) << 16;
                        }
                        else{
                            out_data[o>>2] = u as u32;
                        }
                    }
                    ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>{
                        out_data[o2] = value as u32;
                    }
                    ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>{
                        out_data[o2] = value as i32 as u32;
                    }
                    ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b=>{
                        out_data[o2] = value as i32 as u32;
                    }
                }
                return ot.elem_size()
            }
        }
        else if let Some(value) = value.as_bool(){
            if offset_of >= ot.elem_size() * ot.dims(){
                trap.err_pod_too_much_data();
                return 0
            }
            else{
                let o = offset_of;
                let o2 = o>>2;
                match ot{
                    ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b=>{
                        out_data[o2] = if value{1} else {0}
                    }
                    _=>{
                        trap.err_pod_type_not_matching();
                    }
                }
                return ot.elem_size()
            }
        }
        else if let Some(in_pod) = value.as_pod(){
            let in_pod = &self.pods[in_pod.index as usize];
            let in_pod_ty = &self.pod_types[in_pod.ty.index as usize];
            if let ScriptPodTy::Vec(it) = &in_pod_ty.ty{
                if offset_of + it.dims() * ot.elem_size() > ot.elem_size() * ot.dims(){
                    trap.err_pod_too_much_data();
                    return 0
                }
                else{
                    // output type
                    let o = offset_of;
                    let o2 = o >> 2;
                    match ot{
                        ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>{
                            match it{
                                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>for i in 0..it.dims(){
                                    out_data[o2+i] = in_pod.data[i];
                                }
                                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>for i in 0..it.dims(){
                                    if i&1 == 1{
                                        out_data[o2+i] = f16_to_f32((in_pod.data[i>>1]>>16) as u16).to_bits();
                                    }
                                    else{
                                        out_data[o2+i] = f16_to_f32(in_pod.data[i>>1] as u16).to_bits()
                                    }
                                }
                                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>for i in 0..it.dims(){
                                    out_data[o2+i] = (in_pod.data[i] as f32).to_bits();
                                }
                                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b|
                                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>for i in 0..it.dims(){
                                    out_data[o2+i] = (in_pod.data[i] as i32 as f32).to_bits();
                                }
                            }
                        }
                        ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>{
                            match it{
                                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>for i in 0..it.dims(){
                                    let u = f32_to_f16(f32::from_bits(in_pod.data[i]));
                                    let op = o + (i<<1);
                                    if op & 3 >= 2{
                                        out_data[op>>2] |= (u as u32) << 16;
                                    }
                                    else{
                                        out_data[op>>2] = u as u32;
                                    }
                                }
                                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>for i in (0..it.dims()).step_by(2){
                                    out_data[o+i>>1] = in_pod.data[i>>1];
                                }
                                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>for i in 0..it.dims(){
                                    let u = f32_to_f16(in_pod.data[i] as f32);
                                    let op = o + (i<<1);
                                    if op & 3 >= 2{
                                        out_data[op>>2] |= (u as u32) << 16;
                                    }
                                    else{
                                        out_data[op>>2] = u as u32;
                                    }
                                }
                                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b|
                                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>for i in 0..it.dims(){
                                    let u = f32_to_f16(in_pod.data[i] as i32 as f32);
                                    let op = o + (i<<1);
                                    if op & 3 >= 2{
                                        out_data[op>>2] |= (u as u32) << 16;
                                    }
                                    else{
                                        out_data[op>>2] = u as u32;
                                    }
                                }
                            }
                        }
                        ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>{
                            match it{
                                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>for i in 0..it.dims(){
                                    out_data[o2+i] = f32::from_bits(in_pod.data[i]) as u32;
                                }
                                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>for i in 0..it.dims(){
                                    if i&1 == 1{
                                        out_data[o2+i] = f16_to_f32((in_pod.data[i>>1]>>16) as u16) as u32;
                                    }
                                    else{
                                        out_data[o2+i] = f16_to_f32(in_pod.data[i>>1] as u16) as u32;
                                    }
                                }
                                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>for i in 0..it.dims(){
                                    out_data[o2+i] = in_pod.data[i];
                                }
                                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b| ScriptPodVec::Vec4b|
                                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>for i in 0..it.dims(){
                                    out_data[o2+i] = in_pod.data[i] as i32 as u32;
                                }
                            }
                        }
                        ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>{
                            match it{
                                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>for i in 0..it.dims(){
                                    out_data[o2+i] = f32::from_bits(in_pod.data[i]) as i32 as u32;
                                }
                                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>for i in 0..it.dims(){
                                    if i&1 == 1{
                                        out_data[o2+i] = f16_to_f32((in_pod.data[i>>1]>>16) as u16) as i32 as u32
                                    }
                                    else{
                                        out_data[o2+i] = f16_to_f32(in_pod.data[i>>1] as u16) as i32 as u32
                                    }
                                }
                                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>for i in 0..it.dims(){
                                    out_data[o2+i] = in_pod.data[i] as i32 as u32;
                                }
                                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b| ScriptPodVec::Vec4b|
                                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>for i in 0..it.dims(){
                                    out_data[o2+i] = in_pod.data[i];
                                }
                            }
                        }
                        ScriptPodVec::Vec2b | ScriptPodVec::Vec3b| ScriptPodVec::Vec4b=>{
                            match it{
                                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>for i in 0..it.dims(){
                                    out_data[o2+i] = if f32::from_bits(in_pod.data[i]) as u32 != 0{1} else {0};
                                }
                                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>for i in 0..it.dims(){
                                    if i&1 == 1{
                                        out_data[o2+i] = if f16_to_f32((in_pod.data[i>>1]>>16) as u16) as i32 as u32 != 0{1} else{0}
                                    }
                                    else{
                                        out_data[o2+i] = if f16_to_f32(in_pod.data[i>>1] as u16) as i32 as u32 != 0{1} else{0}
                                    }
                                }
                                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>for i in 0..it.dims(){
                                    out_data[o2+i] = if in_pod.data[i] as i32 as u32 != 0{1} else {0};
                                }
                                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>for i in 0..it.dims(){
                                    out_data[o2+i] = if in_pod.data[i] != 0{1} else {0};
                                }
                                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b| ScriptPodVec::Vec4b=>for i in 0..it.dims(){
                                    out_data[o2+i] = in_pod.data[i];
                                }
                            }
                        }
                    }
                    return  it.dims() * ot.elem_size();
                }                            
            }
            else{
                trap.err_pod_type_not_matching();
            }
        }
        else{
            trap.err_pod_type_not_matching();
        }
        0
    }        
        
    fn pod_write_field(&self, field_ty:&ScriptPodTypeInline, offset_of:usize, out_data:&mut Vec<u32>, value:ScriptValue, trap:&ScriptTrap){    
        
        match &field_ty.data.ty{
            ScriptPodTy::Void | ScriptPodTy::ArrayBuilder | ScriptPodTy::UndefinedStruct  =>{
                trap.err_unexpected();
                return 
            }
            ScriptPodTy::Bool=>{
                if let Some(value) = value.as_bool(){
                    out_data[offset_of>>2] = if value{1} else {0}
                }
                else { // error?
                    trap.err_pod_type_not_matching();
                }
            }
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32=>{
                if let Some(value) = value.as_number(){
                    out_data[offset_of>>2] = value as u32;
                }
                else { // error?
                    trap.err_pod_type_not_matching();
                }
            }
            ScriptPodTy::I32 |ScriptPodTy::AtomicI32=>{
                if let Some(value) = value.as_number(){
                    out_data[offset_of>>2] = (value as i32) as u32;
                }
                else { // error?
                    trap.err_pod_type_not_matching();
                }
            }
            ScriptPodTy::F32=>{
                if let Some(value) = value.as_number(){
                    out_data[offset_of>>2] = (value as f32).to_bits();
                }
                else if let Some(other_pod) = value.as_pod(){
                    let other_pod = &self.pods[other_pod.index as usize];
                    let _other_pod_ty = &self.pod_types[other_pod.ty.index as usize];
                    // we should only allow splatting vecs into vecs
                    // how do we figure out we are a vec
                }
                else{
                    trap.err_pod_type_not_matching();
                }
            }
            ScriptPodTy::Vec(_vt)=>{ 
                if let Some(other_pod) = value.as_pod(){
                    let other_pod = &self.pods[other_pod.index as usize];
                    if other_pod.ty == field_ty.self_ref{
                        let o = offset_of>>2;
                        for i in 0..other_pod.data.len(){
                            out_data[o+i] = other_pod.data[i]
                        }
                        return
                    }
                }
                trap.err_pod_type_not_matching();
            }
            ScriptPodTy::Mat(_mt)=>{
                if let Some(other_pod) = value.as_pod(){
                    let other_pod = &self.pods[other_pod.index as usize];
                    if other_pod.ty == field_ty.self_ref{
                        let o = offset_of>>2;
                        for i in 0..other_pod.data.len(){
                            out_data[o+i] = other_pod.data[i]
                        }
                        return
                    }
                }
                trap.err_pod_type_not_matching();
            }
            ScriptPodTy::F16=>{
                if let Some(value) = value.as_number(){
                    let u = f32_to_f16(value as f32);
                    if offset_of&3 >= 2{
                        out_data[offset_of>>2] |= (u as u32) << 16;
                    }
                    else{
                        out_data[offset_of>>2] = u as u32;
                    }
                }
                else { // error?
                    trap.err_pod_type_not_matching();
                }
            }
            ScriptPodTy::Struct{..}=>{
                if let Some(other_pod) = value.as_pod(){
                    let other_pod = &self.pods[other_pod.index as usize];
                    if other_pod.ty == field_ty.self_ref{
                        let o = offset_of>>2;
                        for i in 0..other_pod.data.len(){
                            out_data[o+i] = other_pod.data[i]
                        }
                    }
                    else{
                        trap.err_pod_type_not_matching();
                    }
                }
                else{
                    trap.err_pod_type_not_matching();
                }
            }
            ScriptPodTy::Enum{..}=>{
                todo!()
            }
            ScriptPodTy::FixedArray{ty, len, size_of, ..}=>{
                // alright we're writing to 'fixed array'.
                // lets accept an ArrayBuilder, check if the type fits and the length of the array
                // ifso write it in.
                if let Some(other_pod_ptr) = value.as_pod() {
                    let other_pod = &self.pods[other_pod_ptr.index as usize];
                    let other_pod_ty = &self.pod_types[other_pod.ty.index as usize];
                    
                    if let ScriptPodTy::ArrayBuilder = &other_pod_ty.ty {
                        // Check element type
                        if let Some(elem_ty) = other_pod.tag.as_array_builder() {
                            if elem_ty != ty.self_ref {
                                trap.err_pod_type_not_matching();
                                return;
                            }
                        }
                        else if *len > 0 {
                             trap.err_pod_type_not_matching();
                             return;
                        }
                        
                        // Check size match
                        // heuristic check based on ArrayBuilder implementation
                        if other_pod.data.len() * 4 < *size_of {
                            trap.err_pod_not_enough_data();
                             return;
                        }
                        let elem_align = ty.data.ty.align_of();
                        if other_pod.data.len() > *size_of + elem_align {
                             trap.err_pod_too_much_data();
                             return;
                        }
                        
                        // Copy data
                        let u32_count = *size_of >> 2;
                        let start = offset_of >> 2;
                        if start + u32_count > out_data.len() {
                             trap.err_pod_too_much_data(); 
                             return;
                        }
                        for i in 0..u32_count {
                            out_data[start + i] = other_pod.data[i];
                        }
                        return;
                    }
                }
                trap.err_pod_type_not_matching();
            }
            ScriptPodTy::VariableArray{ty, ..}=>{
                if let Some(other_pod_ptr) = value.as_pod() {
                    let other_pod = &self.pods[other_pod_ptr.index as usize];
                    let other_pod_ty = &self.pod_types[other_pod.ty.index as usize];
                    
                    if let ScriptPodTy::ArrayBuilder = &other_pod_ty.ty {
                        if let Some(elem_ty) = other_pod.tag.as_array_builder() {
                             if elem_ty != ty.self_ref {
                                 trap.err_pod_type_not_matching();
                                 return;
                             }
                        }
                        else if other_pod.data.len() > 0 {
                             trap.err_pod_type_not_matching();
                             return;
                        }
                        
                        let elem_count_u32 = other_pod.data.len();
                        let start_u32 = offset_of >> 2;
                        
                        out_data.resize(start_u32 + elem_count_u32, 0);
                        
                        for i in 0..elem_count_u32{
                            out_data[start_u32 + i] = other_pod.data[i];
                        }
                        return
                    }
                }
                trap.err_pod_type_not_matching();
            }
        }
    }
    
    
    
    // POD Reading
    
    
    
    pub fn pod_array_index(&mut self, pod_ptr:ScriptPod, index:usize, builtins:&ScriptPodBuiltins, trap:&ScriptTrap)->ScriptValue{
        let pod = &mut self.pods[pod_ptr.index as usize];
        let pod_ty = pod.ty;
        let pod_type = &self.pod_types[pod_ty.index as usize];
        
        let (elem_ty, align_of) = match &pod_type.ty{
            ScriptPodTy::ArrayBuilder=>{
                if let Some(ty) = pod.tag.as_array_builder(){
                    let pod_type = &self.pod_types[ty.index as usize];
                    (Some(ty), pod_type.ty.align_of())
                }
                else{
                    (None, 0)
                }
            }
            ScriptPodTy::FixedArray{ty, align_of, ..} | ScriptPodTy::VariableArray{ty, align_of, ..}=>{
                (Some(ty.self_ref), *align_of)
            }
            _=>{
                trap.err_not_an_array();
                return NIL;
            }
        };
        
        if let Some(elem_ty) = elem_ty{
            let offset_of = index * align_of;
            // bounds check
            let pod = &self.pods[pod_ptr.index as usize];
            if offset_of + align_of > pod.data.len() * 4{
                 trap.err_index_out_of_bounds();
                 return NIL;
            }
            
            let elem_pod_type = &self.pod_types[elem_ty.index as usize];
            match &elem_pod_type.ty{
                ScriptPodTy::Bool=>{
                    if pod.data[offset_of>>2]==1{return TRUE} else {return FALSE}
                }
                ScriptPodTy::U32 | ScriptPodTy::AtomicU32=>{
                    return ScriptValue::from_u32(pod.data[offset_of>>2])
                }
                ScriptPodTy::I32 | ScriptPodTy::AtomicI32=>{
                    return ScriptValue::from_i32(pod.data[offset_of>>2] as i32)
                }
                ScriptPodTy::F32=>{
                    return ScriptValue::from_f32(f32::from_bits(pod.data[offset_of>>2]))
                }
                ScriptPodTy::F16=>{
                    if offset_of&3 >= 2{
                        return ScriptValue::from_f16(f16_to_f32((pod.data[offset_of>>2] >> 16) as u16))
                    }
                    else{
                        return ScriptValue::from_f16(f16_to_f32(pod.data[offset_of>>2] as u16))
                    }
                }
                ScriptPodTy::Vec(vt)=>{
                    let range = (offset_of>>2)..((offset_of + vt.align_of())>>2);
                    let pod_type = vt.builtin(builtins);
                    let out_pod_ptr = self.new_pod(pod_type);
                    let mut out_data = Vec::new();
                    std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                    out_data.clear();
                    out_data.extend(&self.pods[pod_ptr.index as usize].data[range]);
                    std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                    return out_pod_ptr.into()
                }
                ScriptPodTy::Mat(mt)=>{
                    let range = (offset_of>>2)..((offset_of + mt.size_of())>>2);
                    let pod_type = mt.builtin(builtins);
                    let out_pod_ptr = self.new_pod(pod_type);
                    let mut out_data = Vec::new();
                    std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                    out_data.clear();
                    out_data.extend(&self.pods[pod_ptr.index as usize].data[range]);
                    std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                    return out_pod_ptr.into()
                }
                 ScriptPodTy::FixedArray{size_of,..} | ScriptPodTy::Struct{size_of,..}=>{
                    let range = (offset_of>>2)..((offset_of + size_of)>>2);
                    let pod_type = elem_ty;
                    let out_pod_ptr = self.new_pod(pod_type);
                    let mut out_data = Vec::new();
                    std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                    out_data.clear();
                    out_data.extend(&self.pods[pod_ptr.index as usize].data[range]);
                    std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                    return out_pod_ptr.into()
                }
                _=>{}
            }
        }
        
        NIL
    }
        
    pub fn pod_field_type(&self, pod_ty:ScriptPodType, field_name:LiveId, builtins:&ScriptPodBuiltins)->Option<ScriptPodType>{
        let pod_ty = &self.pod_types[pod_ty.index as usize];
        match &pod_ty.ty{
            ScriptPodTy::Struct{fields,..}=>{
                for field in fields{
                    if field.name == field_name{
                        return Some(field.ty.self_ref)
                    }
                }
                None
            }
            ScriptPodTy::Vec(vt)=>{
                return makepad_script_derive::pod_swizzle_vec_type!();
            }
            _=>None
        }
    }
    
    pub fn pod_read_field(&mut self, pod_ptr:ScriptPod, field:ScriptValue, builtins:&ScriptPodBuiltins, trap:&ScriptTrap)->ScriptValue{
        // alright lets get a field
        let field_name = if let Some(id) = field.as_id(){id}
        else{
            return trap.err_pod_invalid_field_name()
        };
        let pod = &mut self.pods[pod_ptr.index as usize];
        let pod_ty = pod.ty;
        let pod_type = &self.pod_types[pod_ty.index as usize];
                        
        // alright lets write 'value' into our current offset slot
        // our current offset slot is
        match &pod_type.ty{
            ScriptPodTy::Struct{align_of,fields,..}=>{
                let mut offset_of = 0;
                // alright we have to return a field now
                // what do we do
                // do we 'new' pod types? or provide a 'window'
                for field in fields{
                    let align_bytes = field.ty.data.ty.align_of();
                    let rem = offset_of % align_of;
                    if rem != 0{
                        offset_of += align_bytes - rem
                    }
                    if field.name == field_name{
                                                
                        match &field.ty.data.ty{
                            ScriptPodTy::Void | ScriptPodTy::ArrayBuilder | ScriptPodTy::UndefinedStruct => {
                                trap.err_unexpected();
                                return NIL
                            }
                            ScriptPodTy::Bool=>{
                                if pod.data[offset_of>>2]==1{return TRUE} else {return FALSE}
                            }
                            ScriptPodTy::U32 | ScriptPodTy::AtomicU32=>{
                                return ScriptValue::from_u32(pod.data[offset_of>>2])
                            }
                            ScriptPodTy::I32 | ScriptPodTy::AtomicI32=>{
                                return ScriptValue::from_i32(pod.data[offset_of>>2] as i32)
                            }
                            ScriptPodTy::F32=>{
                                return ScriptValue::from_f32(f32::from_bits(pod.data[offset_of>>2]))
                            }
                            ScriptPodTy::F16=>{
                                if offset_of&3 >= 2{
                                    return ScriptValue::from_f16(f16_to_f32((pod.data[offset_of>>2] >> 16) as u16))
                                }
                                else{
                                    return ScriptValue::from_f16(f16_to_f32(pod.data[offset_of>>2] as u16))
                                }
                            }
                            ScriptPodTy::Vec(vt)=>{
                                let range = (offset_of>>2)..((offset_of + vt.align_of())>>2);
                                let pod_type = vt.builtin(builtins);
                                let out_pod_ptr = self.new_pod(pod_type);
                                let mut out_data = Vec::new();
                                std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                                out_data.clear();
                                out_data.extend(&self.pods[pod_ptr.index as usize].data[range]);
                                std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                                return out_pod_ptr.into()
                            }
                            ScriptPodTy::Mat(mt)=>{
                                let range = (offset_of>>2)..((offset_of + mt.size_of())>>2);
                                let pod_type = mt.builtin(builtins);
                                let out_pod_ptr = self.new_pod(pod_type);
                                let mut out_data = Vec::new();
                                std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                                out_data.clear();
                                out_data.extend(&self.pods[pod_ptr.index as usize].data[range]);
                                std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                                return out_pod_ptr.into()
                            }
                             ScriptPodTy::FixedArray{size_of,..} | ScriptPodTy::Struct{size_of,..}=>{
                                let range = (offset_of>>2)..((offset_of + size_of)>>2);
                                let pod_type = field.ty.self_ref;
                                let out_pod_ptr = self.new_pod(pod_type);
                                let mut out_data = Vec::new();
                                std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                                out_data.clear();
                                out_data.extend(&self.pods[pod_ptr.index as usize].data[range]);
                                std::mem::swap(&mut self.pods[out_pod_ptr.index as usize].data, &mut out_data);
                                return out_pod_ptr.into()
                            }
                            ScriptPodTy::Enum{..}=>{
                                todo!()
                            }
                            ScriptPodTy::VariableArray{..}=>{
                                todo!()
                            }
                        }
                        
                    }
                    offset_of += field.ty.data.ty.size_of();
                }
                return trap.err_pod_invalid_field_name()
            },
            ScriptPodTy::Vec(vt)=>{
                // we support reading fields and swizzles
                let mut data = [0u32;4];
                let data_in = &pod.data;
                for i in 0..data.len().min(data_in.len()){
                    data[i] = data_in[i]
                }
                return makepad_script_derive::pod_swizzle_vec_match!();
            }
            ScriptPodTy::Mat(_mt)=>{
            }
            _=>{
                                                                
            }
        }
        NIL
    }
    
        
        
        
    // Swizzle reading
        
        
        
    pub fn pod_swizzle_vec1(&self, vec:ScriptPodVec, data:[u32;4], x:usize, trap:&ScriptTrap)->ScriptValue{
        if x >= vec.dims(){
            return trap.err_pod_invalid_field_name()
        }
        match vec{
            ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>{
                return ScriptValue::from_f32(f32::from_bits(data[x]))
            },
            ScriptPodVec::Vec2h | ScriptPodVec::Vec3h  | ScriptPodVec::Vec4h=>{
                if x&1 == 1{
                    return ScriptValue::from_f16(f16_to_f32((data[x>>1] >> 16) as u16))
                }
                else{
                    return ScriptValue::from_f16(f16_to_f32(data[x>>1] as u16))
                }
            },
            ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>{
                return ScriptValue::from_u32(data[x])
            },
            ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>{
                return ScriptValue::from_i32(data[x] as i32)
            },
            ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b=>{
                return ScriptValue::from_bool(data[x] as i32 != 0)
            },
        }
    }
        
    pub fn pod_swizzle_vec<const N:usize>(&mut self, vec:ScriptPodVec, data:[u32;4], swiz:[usize;N], builtins:&ScriptPodBuiltins, _trap:&ScriptTrap)->ScriptValue{
        let pod_type = match N{
            2=> match vec{
                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>builtins.pod_vec2f,
                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>builtins.pod_vec2h,
                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>builtins.pod_vec2u,
                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>builtins.pod_vec2i,
                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b=>builtins.pod_vec2b,
            }
            3=>match vec{
                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>builtins.pod_vec3f,
                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>builtins.pod_vec3h,
                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>builtins.pod_vec3u,
                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>builtins.pod_vec3i,
                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b=>builtins.pod_vec3b,
            }
            4=>match vec{
                ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f=>builtins.pod_vec4f,
                ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>builtins.pod_vec4h,
                ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u=>builtins.pod_vec4u,
                ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>builtins.pod_vec4i,
                ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b=>builtins.pod_vec4b,
            }
            _=>panic!()
        };
        // lets create a pod with type_name
        let pod_ptr = self.new_pod(pod_type);
        let pod = &mut self.pods[pod_ptr.index as usize];
        let pod_data = &mut pod.data;
                
        match vec{
            ScriptPodVec::Vec2f | ScriptPodVec::Vec3f | ScriptPodVec::Vec4f |
            ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u | 
            ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i |
            ScriptPodVec::Vec2b | ScriptPodVec::Vec3b | ScriptPodVec::Vec4b =>{
                for (i, swiz) in swiz.iter().enumerate(){
                    pod_data[i] = data[*swiz];
                }
                pod_ptr.into()
            },
            ScriptPodVec::Vec2h | ScriptPodVec::Vec3h | ScriptPodVec::Vec4h=>{
                // alright lets swizzle a half float into u32
                pod_ptr.into()
            },
        }
    }
    
    
    // Shader compiler use
    
    
    pub fn pod_check_abstract_constructor_arg(&self, pod_ty:ScriptPodType, offset: &mut ScriptPodOffset, trap:&ScriptTrap){
        let pod_ty = &self.pod_types[pod_ty.index as usize];
        match &pod_ty.ty{
            // cross casting numbers
            ScriptPodTy::F32 | ScriptPodTy::F16 | ScriptPodTy::U32 | ScriptPodTy::I32=>{ 
                if offset.field_index > 0{
                    trap.err_pod_too_much_data();
                    return
                }
                offset.field_index += 1;
                return
            }
            ScriptPodTy::Vec(v1)=>{ // what do we 
                // check field counter
                if offset.field_index + 1 > v1.dims(){
                    trap.err_pod_too_much_data();
                }
                offset.field_index += 1;
                return
            }
            ScriptPodTy::Mat(m)=>{ // what do we 
                // check field counter
                if offset.field_index + 1 > m.dim(){
                    trap.err_pod_too_much_data();
                }
                offset.field_index += 1;
                return
            }
            ScriptPodTy::Struct{fields,..}=>{
                if offset.field_index >= fields.len(){
                    trap.err_pod_too_much_data();
                    return
                }
                if !fields[offset.field_index].ty.data.ty.is_number(){
                    trap.err_invalid_constructor_arg();
                    return
                }
                offset.field_index += 1;
            }
            ScriptPodTy::FixedArray{ty, len,..}=>{
                if offset.field_index >= *len{
                    trap.err_pod_too_much_data();
                    return
                }
                if ty.data.ty.is_number(){
                    trap.err_invalid_constructor_arg();
                    return
                }
                offset.field_index += 1;
            }
            ScriptPodTy::VariableArray{ty,..}=>{
                if ty.data.ty.is_number(){
                    trap.err_invalid_constructor_arg();
                    return
                }
                offset.field_index += 1;
            }
            _=>{
                trap.err_invalid_constructor_arg();
            }
        }
    }
    
    
    pub fn pod_check_constructor_arg_count(&self, pod_ty:ScriptPodType, offset: &ScriptPodOffset, trap:&ScriptTrap){
        let pod_ty = &self.pod_types[pod_ty.index as usize];
        match &pod_ty.ty{
            ScriptPodTy::F32 | ScriptPodTy::F16 | ScriptPodTy::U32 | ScriptPodTy::I32 | ScriptPodTy::Bool=>{
                if offset.field_index != 1{
                    trap.err_pod_not_enough_data();
                }
            }
            ScriptPodTy::Vec(v1)=>{
                if offset.field_index != 1 && offset.field_index <v1.dims(){
                    trap.err_pod_not_enough_data();
                }
            }
            ScriptPodTy::Struct{fields,..}=>{
                if offset.field_index < fields.len(){
                    trap.err_pod_not_enough_data();
                }
            }
            ScriptPodTy::FixedArray{len,..}=>{
                if offset.field_index < *len{
                    trap.err_pod_not_enough_data();
                }
            }
            ScriptPodTy::Mat(m)=>{
                if offset.field_index != 1 && offset.field_index <m.dim(){
                    trap.err_pod_not_enough_data();
                }
            }
            _=>()
        }
    }
    
    pub fn pod_check_constructor_arg(&self, pod_ty:ScriptPodType, pod_ty_arg:ScriptPodType, offset: &mut ScriptPodOffset, trap:&ScriptTrap){
        let pod_ty = &self.pod_types[pod_ty.index as usize];
        let pod_ty_arg = &self.pod_types[pod_ty_arg.index as usize];
        match &pod_ty.ty{
            // cross casting numbers
            ScriptPodTy::F32 | ScriptPodTy::F16 | ScriptPodTy::U32 | ScriptPodTy::I32=>{ // were casting to f32
                if offset.field_index > 0{
                    trap.err_pod_too_much_data();
                    return
                }
                offset.field_index += 1;
                match &pod_ty_arg.ty{
                    ScriptPodTy::F32 | ScriptPodTy::F16 | ScriptPodTy::U32 | ScriptPodTy::I32 =>{return;} 
                    _=>{trap.err_invalid_constructor_arg();}
                }
            }
            ScriptPodTy::Bool=>{ // were casting to f32
                if offset.field_index > 0{
                    trap.err_pod_too_much_data();
                    return
                }
                offset.field_index += 1;
                match &pod_ty_arg.ty{
                    ScriptPodTy::Bool=>{return;} 
                    _=>{trap.err_invalid_constructor_arg();}
                }
            }
            ScriptPodTy::AtomicU32=>{trap.err_invalid_constructor_arg();}
            ScriptPodTy::AtomicI32=>{trap.err_invalid_constructor_arg();}
            ScriptPodTy::Vec(v1)=>{ // what do we 
                match pod_ty_arg.ty{
                    // single component
                    ScriptPodTy::F32 | ScriptPodTy::F16 | ScriptPodTy::U32 | ScriptPodTy::I32 | ScriptPodTy::Bool =>{
                        if v1.elem_ty() != pod_ty_arg.ty{
                            trap.err_invalid_constructor_arg();
                        }
                        // check field counter
                        if offset.field_index + 1 > v1.dims(){
                            trap.err_pod_too_much_data();
                        }
                        offset.field_index += 1;
                        return
                    } 
                    // multi component
                    ScriptPodTy::Vec(v2)=>{
                        if v1.elem_ty() != v2.elem_ty() {
                            trap.err_invalid_constructor_arg();
                        }
                        // check field counter
                        if offset.field_index + v2.dims() > v1.dims(){
                            trap.err_pod_too_much_data();
                            return
                        }
                        offset.field_index += v2.dims();
                    }
                    _=>{
                        trap.err_invalid_constructor_arg();
                        offset.field_index += 1;
                        return
                    }
                }
            }
            ScriptPodTy::Mat(m)=>{
                 match pod_ty_arg.ty{
                    ScriptPodTy::F32 =>{
                        if offset.field_index >= m.dim(){
                        // check field counter
                            trap.err_pod_too_much_data();
                            return
                        }
                        offset.field_index += 1;
                    } 
                    _=>{
                        trap.err_invalid_constructor_arg();
                        return
                    }
                }
            }
            ScriptPodTy::Struct{fields,..}=>{
                if offset.field_index >= fields.len(){
                    trap.err_pod_too_much_data();
                    return
                }
                if fields[offset.field_index].ty.data.ty  != pod_ty_arg.ty{
                    trap.err_invalid_constructor_arg();
                    return
                }
                offset.field_index += 1;
            }
            ScriptPodTy::FixedArray{ty, len,..}=>{
                if offset.field_index >= *len{
                    trap.err_pod_too_much_data();
                    return
                }
                if ty.data.ty != pod_ty_arg.ty{
                    trap.err_invalid_constructor_arg();
                    return
                }
                offset.field_index += 1;
            }
            ScriptPodTy::VariableArray{ty,..}=>{
                if ty.data.ty != pod_ty_arg.ty{
                    trap.err_invalid_constructor_arg();
                    return
                }
                offset.field_index += 1;
            }
            ScriptPodTy::Enum{..}=>todo!(),
            _=>{
                trap.err_invalid_constructor_arg();
            }
        }
    }  
    
    
    
    // Debug printing
    
    
    
    pub fn pod_debug(&self, out:&mut String, pod_type:&ScriptPodTypeData, offset_of: usize, data:&[u32]){
        // alright we have a range of data, and a podtype we should be able to print it
        match &pod_type.ty{
            ScriptPodTy::Void=>{
                write!(out, "ScriptPodTy::Void").ok();
            }
            ScriptPodTy::ArrayBuilder=>{
                write!(out, "ScriptPodTy::ArrayBuilder").ok();
            }
            ScriptPodTy::UndefinedStruct =>{
                write!(out, "ScriptPodTy::UndefinedStruct").ok();
            }
            ScriptPodTy::Bool=>{
                write!(out, "bool:{}", if data[offset_of>>2]!=0{true}else{false}).ok();
            }
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32=>{
                write!(out, "u32:{}", data[offset_of>>2]).ok();
            }
            ScriptPodTy::I32 |ScriptPodTy::AtomicI32=>{
                write!(out, "i32:{}", data[offset_of>>2] as i32).ok();
            }
            ScriptPodTy::F32=>{
                write!(out, "f32:{}", f32::from_bits(data[offset_of>>2])).ok();
            }
            ScriptPodTy::F16=>{
                if offset_of&3>=2{
                    write!(out, "f16:{}", f16_to_f32((data[offset_of>>2]>>16) as u16)).ok();
                }
                else{
                    write!(out, "f16:{}", f16_to_f32(data[offset_of>>2] as u16)).ok();
                }
            }
            ScriptPodTy::Vec(vt)=>{
                write!(out, "{}(", vt.name()).ok();
                let mut offset_of = offset_of;
                for i in 0..vt.dims(){
                    if i>0{
                        print!(" ");
                    }
                    if vt.elem_size() == 2{
                        if offset_of&3>=2{
                            write!(out, "{}",f16_to_f32((data[offset_of>>2]>>16) as u16)).ok();
                        }
                        else{
                            write!(out, "{}",f16_to_f32(data[offset_of>>2] as u16)).ok();
                        }
                    }
                    else{
                        write!(out, "{}",f32::from_bits(data[offset_of>>2])).ok();
                    }
                    offset_of += vt.elem_size();
                }
                write!(out, ")").ok();
            }
            ScriptPodTy::Mat(mt)=>{
               write!(out, "{}(", mt.name()).ok();
                let (dim_x,dim_y) = mt.dims();
                let mut offset_of = offset_of;
                for _y in 0..dim_y{
                    write!(out, "[").ok();
                    for _x in 0..dim_x{
                       write!(out, "{} ",f32::from_bits(data[offset_of>>2])).ok();
                        offset_of += mt.elem_size();
                    }
                    write!(out, "]").ok();
                }
                write!(out, ")").ok();
            }
            ScriptPodTy::Struct{fields, ..}=>{
               write!(out, "struct{{").ok();
                // keep a counter
                let mut offset_of = offset_of;
                let mut first = true;
                for field in fields{
                    if !first{
                        write!(out, ", ").ok();
                    }
                    first = false;
                    // align the field offset
                    let align_of = field.ty.data.ty.align_of();
                    let size_of = field.ty.data.ty.size_of();
                    let rem = offset_of % align_of;
                    if rem != 0{ // align offset
                        offset_of += align_of - rem
                    }
                    write!(out, "{}:",field.name).ok();
                    self.pod_debug(out, &field.ty.data, offset_of, data);
                    offset_of += size_of;
                }
                write!(out, "}}").ok();
            }
            ScriptPodTy::Enum{..}=>{
            }
            ScriptPodTy::FixedArray{len, ty, ..}=>{
                write!(out, "array(").ok();
                
                let mut offset_of = offset_of;
                let mut first = true;
                for i in 0..*len{
                    if !first{
                        write!(out, ", ").ok();
                    }
                    first = false;
                    // align the field offset
                    let align_of = ty.data.ty.align_of();
                    let size_of = ty.data.ty.size_of();
                    let rem = offset_of % align_of;
                    if rem != 0{ // align offset
                        offset_of += align_of - rem
                    }
                    write!(out, "{}:",i).ok();
                    self.pod_debug(out, &ty.data, offset_of, data);
                    offset_of += size_of;
                }
               write!(out, ")").ok();
            }
            ScriptPodTy::VariableArray{ty,..}=>{
               write!(out, "var_array(").ok();
                                
                let mut offset_of = offset_of;
                let mut first = true;
                let start = offset_of;
                for i in start..data.len()<<2{
                    if !first{
                        write!(out, ", ").ok();
                    }
                    first = false;
                    // align the field offset
                    let align_of = ty.data.ty.align_of();
                    let size_of = ty.data.ty.size_of();
                    let rem = offset_of % align_of;
                    if rem != 0{ // align offset
                        offset_of += align_of - rem
                    }
                    write!(out, "{}:",(i - start)/ty.data.ty.align_of()).ok();
                    self.pod_debug(out, &ty.data, i, data);
                    offset_of += size_of;
                }
                write!(out, ")").ok();
            }
        }
    }
    
      
}


// AI generated f16/f32 conversions. They look correct at first glance/test

pub fn f16_to_f32(h: u16) -> f32 {
    // Extract sign, exponent, and mantissa
    let sign = (h as u32) >> 15;
    let exponent = (h >> 10) & 0x1F;
    let mantissa = (h & 0x03FF) as u32;
        
    let bits = if exponent == 0x1F {
        // Infinity or NaN
        let new_mantissa = if mantissa != 0 { 0x400000 } else { 0 }; // Preserve NaN
        (sign << 31) | 0x7F800000 | new_mantissa
    } else if exponent == 0 {
        // Zero or Subnormal
        if mantissa == 0 {
            // Zero
            sign << 31
        } else {
            // Subnormal number
            // Count leading zeros in the 10-bit mantissa
            // We use `(mantissa as u16).leading_zeros() - 6` because we're interested
            // in the position within the 10 bits, not the full 16 bits of the u16.
            let shift = (mantissa as u16).leading_zeros() as u32 - 6;
                        
            // Re-bias exponent and shift mantissa
            let new_exponent = 127 - 15 - shift;
            let new_mantissa = (mantissa << (shift + 1)) & 0x7FFFFF;
                        
            (sign << 31) | (new_exponent << 23) | (new_mantissa << 13)
        }
    } else {
        // Normal number
        // Re-bias exponent from 15 to 127
        let new_exponent = (exponent as u32 - 15) + 127;
        // Scale the mantissa
        let new_mantissa = mantissa << 13;
        (sign << 31) | (new_exponent << 23) | new_mantissa
    };
        
    f32::from_bits(bits)
}

pub fn f32_to_f16(f: f32) -> u16 {
    let bits: u32 = f.to_bits();
    // Extract the sign, exponent, and mantissa from the f32
    let sign = (bits >> 31) & 0x1;
    let exponent = (bits >> 23) & 0xff;
    let mantissa = bits & 0x7fffff;
        
    // Handle special cases: NaN and Infinity
    if exponent == 0xff {
        // NaN or Infinity
        let new_mantissa = if mantissa != 0 { 0x200 } else { 0 }; // Preserve NaN-ness
        return ((sign as u16) << 15) | 0x7c00 | new_mantissa;
    }
        
    // Re-bias the exponent from f32's bias (127) to f16's bias (15)
    let new_exponent = exponent as i32 - 127 + 15;
        
    if new_exponent >= 31 {
        // Overflow to infinity
        return ((sign as u16) << 15) | 0x7c00;
    }
        
    if new_exponent <= 0 {
        if new_exponent < -10 {
            // Underflow to zero
            return (sign as u16) << 15;
        }
        // Handle subnormal numbers
        let new_mantissa = (mantissa | 0x800000) >> (1 - new_exponent);
        return ((sign as u16) << 15) | (new_mantissa >> 13) as u16;
    }
        
    // Normal number
    let new_mantissa = mantissa >> 13;
    ((sign as u16) << 15) | ((new_exponent as u16) << 10) | (new_mantissa as u16)
}


