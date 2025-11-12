use makepad_live_id::*;
use crate::value::*;
use crate::heap::*;
use crate::trap::*;
use crate::mod_pod::*;
use crate::pod::*;


impl ScriptHeap{
        
    pub fn new_pod_type(&mut self, ty:ScriptPodTy, default:ScriptValue)->ScriptPodType{
        if let Some(ptr) = self.pod_types_free.pop(){
            let pod_type = &mut self.pod_types[ptr.index as usize];
            //pod_type.cached_align_of = ty.align_of();
            pod_type.ty = ty;
            pod_type.default = default;
            ptr
        }
        else{
            let ptr = ScriptPodType{index: self.pod_types.len() as u32};
            self.pod_types.push(ScriptPodTypeData{
                //cached_align_of: ty.align_of(),
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
    
    pub fn pod_def_atom(&mut self, pod_module:ScriptObject, name: LiveId, ty: ScriptPodTy, helper_name:LiveId, default:ScriptValue)->ScriptPodType{
        let pod_obj = self.new_with_proto(helper_name.into());
        if ty != ScriptPodTy::UndefinedStruct && 
           ty != ScriptPodTy::UndefinedArray{
            self.set_notproto(pod_obj);
        }
        let pt = self.new_pod_type(ty, default);
        self.set_object_storage_vec2(pod_obj);
        self.set_object_pod_type(pod_obj, pt); 
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        pt
    }
    
    pub fn pod_def_vec(&mut self, pod_module:ScriptObject, name:LiveId, builtin: ScriptPodVec)->ScriptPodType{
        let pod_obj = self.new_with_proto(name.into());
        let vec_ty = self.new_pod_type(ScriptPodTy::Vec(builtin), NIL);
        self.set_object_pod_type(pod_obj, vec_ty);
        self.set_notproto(pod_obj);
        self.freeze(pod_obj);
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        return vec_ty
    }
        
    pub fn pod_def_mat(&mut self, pod_module:ScriptObject, name:LiveId, builtin:ScriptPodMat)->ScriptPodType{
        let pod_obj = self.new_with_proto(name.into());
        let mat_ty = self.new_pod_type(ScriptPodTy::Mat(builtin), NIL);
        self.set_object_pod_type(pod_obj, mat_ty);
        self.set_notproto(pod_obj);
        self.freeze(pod_obj);
        self.set_value_def(pod_module, name.into(), pod_obj.into());
        return mat_ty
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
        let _pod = &self.pods[pod.index as usize];
        println!("WANT TO SET FIELD {}", field);
        todo!()
    }
        
        
    pub fn pod_pop_to_me(&mut self,  pod_ptr:ScriptPod, offset:&mut ScriptPodOffset, _field:ScriptValue, value:ScriptValue, trap:&ScriptTrap){
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
                    if rem != 0{ // align offset
                        offset.offset_of += align_bytes - rem
                    }
                    
                    match &field.ty.data.ty{
                        ScriptPodTy::NIL | ScriptPodTy::UndefinedArray | ScriptPodTy::UndefinedStruct =>{
                            trap.err_unexpected();
                            return 
                        }
                        ScriptPodTy::Bool=>{
                            if let Some(value) = value.as_bool(){
                                out_data[offset.offset_of>>2] = if value{1} else {0}
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::U32 | ScriptPodTy::AtomicU32=>{
                            if let Some(value) = value.as_number(){
                                out_data[offset.offset_of>>2] = value as u32;
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::I32 |ScriptPodTy::AtomicI32=>{
                            if let Some(value) = value.as_number(){
                                out_data[offset.offset_of>>2] = (value as i32) as u32;
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::F32=>{
                            if let Some(value) = value.as_number(){
                                out_data[offset.offset_of>>2] = (value as f32).to_bits();
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
                            println!("setting a vec via pop");
                        }
                        ScriptPodTy::Mat(_mt)=>{
                            println!("setting a mat via pop");
                        }
                        ScriptPodTy::F16=>{
                            if let Some(value) = value.as_number(){
                                let u = f32_to_f16(value as f32);
                                if offset.offset_of&3 >= 2{
                                    out_data[offset.offset_of>>2] |= (u as u32) << 16;
                                }
                                else{
                                    out_data[offset.offset_of>>2] = u as u32;
                                }
                            }
                            else { // error?
                                trap.err_pod_type_not_matching();
                            }
                        }
                        ScriptPodTy::Struct{..}=>{
                            if let Some(other_pod) = value.as_pod(){
                                let other_pod = &self.pods[other_pod.index as usize];
                                if other_pod.ty == field.ty.self_ref{
                                    let o = offset.offset_of>>2;
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
                        ScriptPodTy::FixedArray{..}=>{
                            todo!()
                        }
                        ScriptPodTy::VariableArray{..}=>{
                            todo!()
                        }
                    }
                    // alright lets do the align and all that
                    offset.field_index += 1;
                    offset.offset_of += field.ty.data.ty.size_of();
                }
                else{
                    trap.err_pod_too_much_data();
                }
            },
            ScriptPodTy::FixedArray{align_of:_,size_of:_,len:_,ty:_}=>{
                
            },
            ScriptPodTy::Vec(ot)=>{
                if let Some(value) = value.as_number(){
                    if offset.offset_of >= ot.elem_size() * ot.dims(){
                        trap.err_pod_too_much_data();
                    }
                    else{
                        let o = offset.offset_of;
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
                        }
                        offset.offset_of += ot.elem_size()
                    }
                }
                else if let Some(in_pod) = value.as_pod(){
                    let in_pod = &self.pods[in_pod.index as usize];
                    let in_pod_ty = &self.pod_types[in_pod.ty.index as usize];
                    if let ScriptPodTy::Vec(it) = &in_pod_ty.ty{
                        if offset.offset_of + it.dims() * ot.elem_size() > ot.elem_size() * ot.dims(){
                            trap.err_pod_too_much_data();
                        }
                        else{
                            // output type
                            let o = offset.offset_of;
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
                                    todo!()
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
                                        ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i=>for i in 0..it.dims(){
                                            out_data[o2+i] = in_pod.data[i];
                                        }
                                    }
                                }
                            }
                            offset.offset_of += it.dims() * ot.elem_size();
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
                    print!("f16:{}", f16_to_f32((data[offset_of>>2]>>16) as u16))
                }
                else{
                    print!("f16:{}", f16_to_f32(data[offset_of>>2] as u16))
                }
            }
            ScriptPodTy::Vec(vt)=>{
                print!("{}(", vt.name());
                let mut offset_of = offset_of;
                for i in 0..vt.dims(){
                    if i>0{
                        print!(" ");
                    }
                    if vt.elem_size() == 2{
                        if offset_of&3>=2{
                            print!("{}",f16_to_f32((data[offset_of>>2]>>16) as u16));
                        }
                        else{
                            print!("{}",f16_to_f32(data[offset_of>>2] as u16));
                        }
                    }
                    else{
                        print!("{}",f32::from_bits(data[offset_of>>2]));
                    }
                    offset_of += vt.elem_size();
                }
                print!(")");
            }
            ScriptPodTy::Mat(mt)=>{
                print!("{}(", mt.name());
                let (dim_x,dim_y) = mt.dims();
                let mut offset_of = offset_of;
                for _y in 0..dim_y{
                    print!("[");
                    for _x in 0..dim_x{
                        print!("{} ",f32::from_bits(data[offset_of>>2]));
                        offset_of += mt.elem_size();
                    }
                    print!("]")
                }
                print!(")")
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
                        offset_of += align_of - rem
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
                        offset_of += align_of - rem
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
                        offset_of += align_of - rem
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
        if val.is_f64(){
            let pod_type = &self.pod_types[pod_builtins.pod_f32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_f32,
                data: pod_type.clone()
            }));
        }
        if val.is_f32(){
            let pod_type = &self.pod_types[pod_builtins.pod_f32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_f32,
                data: pod_type.clone()
            }));
        }
        if val.is_u32(){
            let pod_type = &self.pod_types[pod_builtins.pod_u32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_u32,
                data: pod_type.clone()
            }));
        }
        if val.is_i32(){
            let pod_type = &self.pod_types[pod_builtins.pod_i32.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_i32,
                data: pod_type.clone()
            }));
        }
        if val.is_f16(){
            let pod_type = &self.pod_types[pod_builtins.pod_f16.index as usize];
            return Some((val, ScriptPodTypeInline{
                self_ref: pod_builtins.pod_f16,
                data: pod_type.clone()
            }));
        }
        if val.is_bool(){
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
                                let size_of = if rem != 0{size_of + (align_of - rem)}else{size_of};
                                
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
                            offset_of += align_bytes - rem
                        }
                        
                        offset_of += size_bytes;
                    }
                    // align final offset
                    let rem = offset_of % align_of;
                    if rem != 0{
                        offset_of += align_of - rem
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
                _x=>{
                    trap.err_pod_type_not_extendable();
                    return
                }
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

