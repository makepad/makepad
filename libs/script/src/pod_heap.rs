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
            pod_type.cached_align_of = ty.align_of();
            pod_type.ty = ty;
            pod_type.default = default;
            ptr
        }
        else{
            let ptr = ScriptPodType{index: self.pod_types.len() as u32};
            self.pod_types.push(ScriptPodTypeData{
                cached_align_of: ty.align_of(),
                ty,
                default
            });
            ptr
        }
    }
            
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
        let mut size_of = components * pod_type.ty.align_of();
        
        // do the custom vec3 haxery blegh.
        let mut align_of = size_of;
        if size_of == 12{align_of = 16}
        else if size_of == 6{align_of = 8}
        
        let vec_ty = self.new_pod_type(ScriptPodTy::Struct{
            align_of,
            size_of,
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
        let mut size_bytes = (x*y) * pod_type.ty.align_of();
        
        // align of / size of table copied from wgsl spec
        let (align_of, size_of) = match (x,y){
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
            align_of,
            size_of,
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
        
        
    pub fn pod_pop_to_me(&mut self,  pod:ScriptPod, offset:&mut ScriptPodOffset, field:ScriptValue, value:ScriptValue, trap:&ScriptTrap){
        let pod = &mut self.pods[pod.index as usize];
        let pod_type = &self.pod_types[pod.ty.index as usize];
        // alright lets write 'value' into our current offset slot
        // our current offset slot is
        match &pod_type.ty{
            ScriptPodTy::Struct{align_of,size_of,fields}=>{
                // struct. ok so we are at field 
                if let Some(field) = fields.get(offset.field_index){
                    
                    // align the field offset
                    let align_bytes = field.ty.data.ty.align_of();
                    let rem = offset.offset_of % align_of;
                    if rem != 0{ // align offset
                        offset.offset_of += (align_bytes - rem)
                    }
                    
                    match &field.ty.data.ty{
                        ScriptPodTy::NIL | ScriptPodTy::UndefinedArray | ScriptPodTy::UndefinedStruct =>{
                            trap.err_unexpected();
                            return 
                        }
                        ScriptPodTy::Bool=>{
                            if let Some(value) = value.as_bool(){
                                pod.data[offset.offset_of>>2] = if value{1} else {0}
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::U32 | ScriptPodTy::AtomicU32=>{
                            if let Some(value) = value.as_number(){
                                pod.data[offset.offset_of>>2] = (value as u32);
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::I32 |ScriptPodTy::AtomicI32=>{
                            if let Some(value) = value.as_number(){
                                pod.data[offset.offset_of>>2] = (value as i32) as u32;
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::F32=>{
                            if let Some(value) = value.as_number(){
                                pod.data[offset.offset_of>>2] = (value as f32).to_bits();
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::F16=>{
                            todo!();
                            if let Some(value) = value.as_number(){
                                if offset.offset_of&3 >= 2{
                                    pod.data[offset.offset_of>>2] |= (value as f32).to_bits()<<16;
                                }
                                else{
                                    pod.data[offset.offset_of>>2] = (value as f32).to_bits();
                                }
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::Struct{fields,..}=>{
                            // we should check the type of what we are assigning
                            println!("ASSIGN TO STRUCT");
                                                        
                            // alright we have to assign to a struct.
                        }
                        ScriptPodTy::Enum{..}=>{
                            todo!()
                        }
                        ScriptPodTy::FixedArray{..}=>{
                            todo!()
                        }
                        ScriptPodTy::VariableArray{..}=>{
                            todo!()
                        }
                    }
                    // alright lets do the align and all that
                    offset.offset_of += field.ty.data.ty.size_of();
                }
                else{
                    trap.err_pod_too_much_data();
                    return
                }
            },
            ScriptPodTy::FixedArray{align_of,size_of,len,ty}=>{
                
            },
            _=>{
                trap.err_unexpected();
                return
            }
        }
    }
    
    pub fn pod_debug_print(&self, pod_type:&ScriptPodTypeData, offset_of: usize, data:&[u32]){
        // alright we have a range of data, and a podtype we should be able to print it
        match &pod_type.ty{
            ScriptPodTy::NIL=>{
                print!("ScriptPodTy::NIL");
            }
            ScriptPodTy::UndefinedArray=>{
                print!("ScriptPodTy::UndefinedArray");
            }
            ScriptPodTy::UndefinedStruct =>{
                print!("ScriptPodTy::UndefinedStruct");
            }
            ScriptPodTy::Bool=>{
                print!("bool:{}", if data[offset_of>>2]!=0{true}else{false})
            }
            ScriptPodTy::U32 | ScriptPodTy::AtomicU32=>{
                print!("u32:{}", data[offset_of>>2])
            }
            ScriptPodTy::I32 |ScriptPodTy::AtomicI32=>{
                print!("i32:{}", data[offset_of>>2] as i32)
            }
            ScriptPodTy::F32=>{
                print!("f32:{}", f32::from_bits(data[offset_of>>2]))
            }
            ScriptPodTy::F16=>{
                if offset_of&3>=2{
                    print!("f16:{}", data[offset_of>>2]>>16)
                }
                else{
                    print!("f16:{}", data[offset_of>>2])
                }
            }
            ScriptPodTy::Struct{fields, ..}=>{
                print!("struct{{");
                // keep a counter
                let mut offset_of = offset_of;
                let mut first = true;
                for field in fields{
                    if !first{
                        print!(", ")
                    }
                    first = false;
                    // align the field offset
                    let align_of = field.ty.data.ty.align_of();
                    let size_of = field.ty.data.ty.size_of();
                    let rem = offset_of % align_of;
                    if rem != 0{ // align offset
                        offset_of += (align_of - rem)
                    }
                    print!("{}:",field.name);
                    self.pod_debug_print(&field.ty.data, offset_of, data);
                    offset_of += size_of;
                }
                print!("}}");
            }
            ScriptPodTy::Enum{..}=>{
            }
            ScriptPodTy::FixedArray{len, ty, ..}=>{
                print!("array(");
                
                let mut offset_of = offset_of;
                let mut first = true;
                for i in 0..*len{
                    if !first{
                        print!(", ")
                    }
                    first = false;
                    // align the field offset
                    let align_of = ty.data.ty.align_of();
                    let size_of = ty.data.ty.size_of();
                    let rem = offset_of % align_of;
                    if rem != 0{ // align offset
                        offset_of += (align_of - rem)
                    }
                    print!("{}:",i);
                    self.pod_debug_print(&ty.data, offset_of, data);
                    offset_of += size_of;
                }
                print!(")");
            }
            ScriptPodTy::VariableArray{ty,..}=>{
                print!("var_array(");
                                
                let mut offset_of = offset_of;
                let mut first = true;
                let start = offset_of;
                for i in start..data.len()<<2{
                    if !first{
                        print!(", ")
                    }
                    first = false;
                    // align the field offset
                    let align_of = ty.data.ty.align_of();
                    let size_of = ty.data.ty.size_of();
                    let rem = offset_of % align_of;
                    if rem != 0{ // align offset
                        offset_of += (align_of - rem)
                    }
                    print!("{}:",(i - start)/ty.data.ty.align_of());
                    self.pod_debug_print(&ty.data, i, data);
                    offset_of += size_of;
                }
                print!(")");
            }
        }
    }
        
    pub fn pod_check_arg_total(&mut self,  pod:ScriptPod, offset:ScriptPodOffset, trap:&ScriptTrap){
        let pod = &mut self.pods[pod.index as usize];
        let pod_type = &self.pod_types[pod.ty.index as usize];
        let size_of = pod_type.ty.size_of();
        if size_of != offset.offset_of{
            trap.err_pod_not_enough_data();
        }
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
                        if let Some((_,ty)) = self.pod_type_inline(kvs[1].value, pod_builtins){
                            if let Some(len) = kvs[0].value.as_number(){
                                let len = len as usize;
                                let align_of = ty.data.ty.align_of();
                                let size_of = align_of * len;
                                let rem = size_of % align_of;
                                let size_bytes = if rem != 0{size_of + (align_of - rem)}else{size_of};
                                
                                let pt = self.new_pod_type(ScriptPodTy::FixedArray{
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
                    let mut align_of = 0;
                    for kv in kvs.iter().rev(){
                        if let Some((default,ty)) = self.pod_type_inline(kv.value, pod_builtins){
                            if let Some(name) = kv.key.as_id(){
                                align_of = align_of.max(ty.data.ty.align_of());
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
                    
                    let mut offset_of = 0;
                    for field in &fields{
                        let align_bytes = field.ty.data.ty.align_of();
                        let size_bytes =  field.ty.data.ty.size_of();
                        let rem = offset_of % align_bytes;
                        if rem != 0{ // align offset
                            offset_of += (align_bytes - rem)
                        }
                        
                        offset_of += size_bytes;
                    }
                    // align final offset
                    let rem = offset_of % align_of;
                    if rem != 0{
                        offset_of += (align_of - rem)
                    }
                    
                    let pt = self.new_pod_type(ScriptPodTy::Struct{
                        align_of,
                        size_of: offset_of,
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
