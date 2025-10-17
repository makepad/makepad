use crate::makepad_id::id::*;
use crate::value::*;
use crate::makepad_id_derive::*;
use std::fmt::Write;
use crate::object::*;
use crate::string::*;

pub struct ScriptHeap{
    pub modules: ObjectPtr,
    pub(crate) mark_vec: Vec<usize>,
    pub(crate) objects: Vec<Object>,
    pub(crate) roots: Vec<usize>,
    pub(crate) objects_free: Vec<usize>,
    pub(crate) strings: Vec<HeapString>,
    pub(crate) strings_free: Vec<usize>
}

impl ScriptHeap{
    
    pub fn empty()->Self{
        let mut v = Self{
            modules: ObjectPtr{index:0},
            mark_vec: Default::default(),
            objects: Default::default(),
            roots: vec![0],
            objects_free: Default::default(),
            // index 0 is always an empty string
            strings: vec![Default::default()],
            strings_free: Default::default(),
        };
        v.modules = v.new_with_proto(id!(mod).into()); 
        v
    }
    
    
    // New objects
    
    
    
    pub fn new(&mut self, flags: u64)->ObjectPtr{
        if let Some(index) = self.objects_free.pop(){
            let object = &mut self.objects[index];
            object.tag.set_flags(flags | ObjectTag::ALLOCED);
            object.proto = id!(object).into();
            ObjectPtr{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = Object::default();
            object.tag.set_flags(flags | ObjectTag::ALLOCED);
            object.proto = id!(object).into();
            self.objects.push(object);
            ObjectPtr{index: index as _}
        }
    }
    
             
    pub fn new_with_proto(&mut self, proto:Value)->ObjectPtr{
        let (proto_fwd, proto_index) = if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            object.tag.set_reffed();
            (object.tag.proto_fwd(), ptr.index as usize)
        }
        else{
            let ptr = self.new(0);
            self.objects[ptr.index as usize].proto = proto;
            return ptr
        };
                        
        if let Some(index) = self.objects_free.pop(){
            let (object, proto_object) = if index > proto_index{
                let (o1, o2) = self.objects.split_at_mut(index);
                (&mut o2[0], &mut o1[proto_index])                    
            }else{
                let (o1, o2) = self.objects.split_at_mut(proto_index);
                (&mut o1[index], &mut o2[0])                    
            };
            object.tag.set_proto_fwd(proto_fwd);
            object.proto = proto;
            // only copy vec if we are 'auto' otherwise we proto inherit normally
            if proto_object.tag.get_type().is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            ObjectPtr{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = Object::with_proto(proto);
            object.tag.set_proto_fwd(proto_fwd);
            let proto_object = &self.objects[proto_index];
            if proto_object.tag.get_type().is_auto(){
                object.vec.extend_from_slice(&proto_object.vec);
            }
            self.objects.push(object);
            ObjectPtr{index: index as _}
        }
    }
    
    pub fn new_if_reffed(&mut self, ptr:ObjectPtr)->ObjectPtr{
        let obj = &self.objects[ptr.index as usize];
        if obj.tag.is_reffed(){
            let proto = obj.proto;
            return self.new_with_proto(proto);
        }
        return ptr;
    }
    
    pub fn new_module(&mut self, id:Id)->ObjectPtr{
        let md = self.new_with_proto(id.into());
        self.set_value(self.modules, id.into(), md.into());
        md
    }
    
    pub fn new_empty_string(&mut self)->StringPtr{
        if let Some(index) = self.strings_free.pop(){
            self.strings[index].tag.set_alloced();
            StringPtr{index: index as _}
        }
        else{
            let index = self.strings.len();
            let mut string = HeapString::default();
            string.tag.set_alloced();
            self.strings.push(string);
            StringPtr{index: index as _}
        }
    }
        
    pub fn new_string_from_str(&mut self,value:&str)->StringPtr{
        self.new_string_with(|_,out|{
            out.push_str(value);
        })
    }
            
    pub fn new_string_with<F:FnOnce(&mut Self, &mut String)>(&mut self,cb:F)->StringPtr{
        let mut out = String::new();
        let ptr = self.new_empty_string();
        std::mem::swap(&mut out, &mut self.strings[ptr.index as usize].string);
        cb(self, &mut out);
        std::mem::swap(&mut out, &mut self.strings[ptr.index as usize].string);
        ptr
    }
            
    pub fn new_string_from_string(&mut self, string:String)->StringPtr{
        let index = self.strings.len();
        self.strings.push(HeapString{
            tag: Default::default(),
            string
        });
        StringPtr{index: index as u32}
    }
    
    pub fn null_string(&self)->StringPtr{
        StringPtr{index: 0}
    }
        
    
    
    // Accessors
    
            
    pub fn has_proto(&mut self, ptr:ObjectPtr, rhs:Value)->bool{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if object.proto == rhs{
                return true
            }            
            if let Some(object) = object.proto.as_object(){
                ptr = object
            }
            else{
                return false
            }
        }
    }
     
    pub fn proto(&self, ptr:ObjectPtr)->Value{
        self.objects[ptr.index as usize].proto
    }
                
    //pub fn object(&self, ptr:ObjectPtr)->&Object{
    //    &self.objects[ptr.index as usize]
    //}
        
            
    pub fn value_string(&self, value: Value)->&str{
        if let Some(ptr) = value.as_string(){
            return self.string(ptr)
        }
        else{
            ""
        }
    }
    
    pub fn string(&self, ptr: StringPtr)->&str{
        &self.strings[ptr.index as usize].string
    }
        
    pub fn cast_to_string(&self, v:Value, out:&mut String){
        if v.as_inline_string(|s|{write!(out, "{s}")}).is_some(){
        }
        else if let Some(v) = v.as_string(){
            let str = self.string(v);
            out.push_str(str);
        }
        else if let Some(v) = v.as_f64(){
            write!(out, "{v}").ok();
        }
        else if let Some(v) = v.as_bool(){
            write!(out, "{v}").ok();
        }
        else if let Some(v) = v.as_id(){
            write!(out, "{v}").ok();
        }
        else if let Some(_v) = v.as_object(){
            write!(out, "[Object]").ok();
        }
        else if let Some(v) = v.as_color(){
            write!(out, "#{:08x}", v).ok();
        }
        else if v.is_nil(){
        }
        else if v.is_opcode(){
            write!(out, "[Opcode]").ok();
        }
        else if v.is_err(){
            write!(out, "[Error:{}]", v).ok();
        }
        else{
            write!(out, "[Unknown]").ok();
        }
    }
        
    pub fn cast_to_f64(&self, v:Value, ip:ScriptIp)->f64{
        if let Some(v) = v.as_f64(){
            v
        }
        else if let Some(v) = v.as_string(){
            let str = self.string(v);
            if let Ok(v) = str.parse::<f64>(){
                return v
            }
            else{
                return 0.0
            }
        }
        else if let Some(v) = v.as_bool(){
            return if v{1.0}else{0.0}
        }
        else if let Some(v) = v.as_color(){
            return v as f64
        }
        else if v.is_nil(){
            0.0
        }
        else {
            Value::from_f64_traced_nan(f64::NAN, ip).as_f64().unwrap()
        }
    }
    
    pub fn cast_to_bool(&self, v:Value)->bool{
        if let Some(b) = v.as_bool(){
            return b
        }
        if v.is_nil(){
            return false
        }
        if let Some(v) = v.as_f64(){
            return v != 0.0
        }
        if let Some(_v) = v.as_object(){
            return true
        }
        if v.inline_string_not_empty(){
            return true
        }
        if let Some(v) = v.as_string(){
            return self.string(v).len() != 0
        }
        if let Some(_v) = v.as_id(){
            return true
        }
        if let Some(_v) = v.as_color(){
            return true
        }
        if v.is_opcode(){
            return true
        }
        false
    }
    
    pub fn swap_string(&mut self, ptr: StringPtr, swap:&mut String){
        std::mem::swap(swap, &mut self.strings[ptr.index as usize].string);
    }
    
    pub fn mut_string(&mut self, ptr: StringPtr)->&mut String{
        &mut self.strings[ptr.index as usize].string
    }
    
    
    
    // Setting object flags
    
    
    
    
    pub fn set_object_deep(&mut self, ptr:ObjectPtr){
         self.objects[ptr.index as usize].tag.set_deep()
    }
    
    pub fn set_object_type(&mut self, ptr:ObjectPtr, ty: ObjectType){
        self.objects[ptr.index as usize].set_type(ty)
    }
        
    pub fn clear_object_deep(&mut self, ptr:ObjectPtr){
        self.objects[ptr.index as usize].tag.clear_deep()
    }
    
        
        
        
        
    // Writing object values 
        
        
    
    pub(crate) fn force_value_in_map(&mut self, ptr:ObjectPtr, key: Value, this:Value){
        let object = &mut self.objects[ptr.index as usize];
        object.map.insert(key, this);
    }            
        
    fn set_value_index(&mut self, ptr: ObjectPtr, index:Value, value: Value, ip:ScriptIp)->Value{
        // alright so. now what.
        let object = &mut self.objects[ptr.index as usize];
        let ty = object.tag.get_type();
        
        if object.tag.has_rw(){ // has rw flags
            if object.tag.is_frozen(){
                
            }
        }
        
        if ty.uses_vec2(){
            let index = index.as_index() * 2;
            if index + 1 >= object.vec.len(){
                object.vec.resize(index + 2, NIL);
            }
            object.vec[index] = NIL;
            object.vec[index+1] = value; 
            return NIL
        }
        if ty.is_vec1(){
            let index = index.as_index();
            if index>= object.vec.len(){
                object.vec.resize(index, NIL);
            }
            object.vec[index] = value;
            return NIL 
        }
        if ty.is_map(){
            object.map.insert(index, value);
            return NIL
        }
        if ty.is_typed(){ // typed array
            println!("Implement typed array set value");
            //todo IMPLEMENT IT
            return NIL
        }
        Value::err_internal(ip)
    }
            
    fn set_value_prefixed(&mut self, ptr: ObjectPtr, key: Value, value: Value, _ip:ScriptIp)->Value{
        let object = &mut self.objects[ptr.index as usize];
        for chunk in object.vec.rchunks_mut(2){
            if chunk[0] == key{
                chunk[1] = value;
                return NIL
            }
        }
        // just append it
        object.vec.extend_from_slice(&[key, value]);
        NIL
    }
        
    fn set_value_deep(&mut self, ptr:ObjectPtr, key: Value, value: Value, _ip:ScriptIp)->Value{
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.get_type().has_paired_vec(){
                for chunk in object.vec.rchunks_mut(2){
                    if chunk[0] == key{
                        chunk[1] = value;
                        return NIL
                    }
                }
            }
            if let Some(set_value) = object.map.get_mut(&key){
                *set_value = value;
                return NIL
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            } 
        }
        // alright nothing found
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.get_type().is_vec2(){
            object.vec.extend_from_slice(&[key, value]);
        }
        else{
            object.map.insert(key, value);
        }
        NIL
    }
        
    fn set_value_shallow(&mut self, ptr:ObjectPtr, key:Value, value:Value, _ip:ScriptIp)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.get_type().is_vec2(){
            for chunk in object.vec.rchunks_mut(2){
                if chunk[0] == key{
                    chunk[1] = value;
                    return NIL
                }
            }
            object.vec.extend_from_slice(&[key, value]);
            return NIL
        }
        object.map.insert(key, value);
        NIL
    }
            
    
    pub fn set_value(&mut self, ptr:ObjectPtr, key:Value, value:Value)->Value{
        self.set_value_ip(ptr, key, value, ScriptIp::default())
    }
    
    pub fn set_value_ip(&mut self, ptr:ObjectPtr, key:Value, value:Value, ip:ScriptIp)->Value{
        if let Some(obj) = value.as_object(){
            self.set_reffed(obj);
        }
        if key.is_index(){ // use vector
            return self.set_value_index(ptr, key, value, ip);
        }
        if let Some(id) = key.as_id(){
            if id.is_prefixed(){
                return self.set_value_prefixed(ptr, key, value, ip)
            }
            // scan prototype chain for id
            let object = &self.objects[ptr.index as usize];
            if !object.tag.is_deep(){
                return self.set_value_shallow(ptr, key, value, ip);
            }
            else{
                return self.set_value_deep(ptr, key, value, ip)
            }
        }
        if key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            let object = &mut self.objects[ptr.index as usize];
            if !object.tag.is_deep(){
                return self.set_value_shallow(ptr, key, value, ip);
            }
            else{
                return self.set_value_deep(ptr, key, value, ip)
            }
        }
        Value::err_wrongkey(ip)
    }
    
        
    
    
    // Reading object values
    
    
    
    fn value_index(&self, ptr: ObjectPtr, index: Value, def:Value)->Value{
        let object = &self.objects[ptr.index as usize];
        
        let ty = object.tag.get_type();
        // most used path
        if ty.uses_vec2(){
            let index = index.as_index();
            if let Some(value) = object.vec.get(index * 2 + 1){
                return *value
            }
            else{
                return def
            }
        }
        if ty.is_vec1(){
            let index = index.as_index();
            if let Some(value) = object.vec.get(index){
                return *value
            }
            else{
                return def
            }
        }
        if ty.is_typed(){ // typed access to the vec
            //todo IMPLEMENT IT
        }
        if ty.is_map(){
            if let Some(value) = object.map.get(&index){
                return *value
            }
            else{
                return def
            }
        }
        def
    }
    
    fn value_deep_map(&self, obj_ptr:ObjectPtr, key: Value, def:Value)->Value{
        let mut ptr = obj_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map.get(&key){
                return *value
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            }
        }
        def
    }
    
    fn value_deep(&self, obj_ptr:ObjectPtr, key: Value, def:Value)->Value{
        let mut ptr = obj_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            if let Some(value) = object.map.get(&key){
                return *value
            }
            if object.tag.get_type().has_paired_vec(){
                for chunk in object.vec.rchunks(2){
                    if chunk[0] == key{
                        return chunk[1]
                    }
                }
            }
            if let Some(next_ptr) = object.proto.as_object(){
                ptr = next_ptr
            }
            else{
                break;
            }
        }
        def
    }
    
    pub fn object_method(&self, ptr:ObjectPtr, key:Value, def:Value)->Value{
        self.value_map(ptr, key, def)
    }
    
    pub fn value_map(&self, ptr:ObjectPtr, key:Value, def:Value)->Value{
        // hard array index
        if key.is_id(){
            return self.value_deep_map(ptr, key, def)
        }
        if key.is_index(){
            return self.value_index(ptr, key, def)
        }
        if key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            return self.value_deep_map(ptr, key, def)
        }
        // TODO implement string lookup
        def
    }
    
    pub fn value_path(&self, ptr:ObjectPtr, keys:&[Id], def:Value)->Value{
        let mut value:Value = ptr.into();
        for key in keys{
            if let Some(obj) = value.as_object(){
                value = self.value(obj, key.into(), def);
            }
            else{
                return def;
            }
        }
        value
    }
    
    pub fn value(&self, ptr:ObjectPtr, key:Value, def:Value)->Value{
        // hard array index
        if key.is_id(){
            return self.value_deep(ptr, key, def)
        }
        if key.is_index(){
            return self.value_index(ptr, key, def)
        }
        if key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            return self.value_deep(ptr, key, def)
        }
        // TODO implement string lookup
        def
    }
    
    
    
    
    // Vec Reading
    
    
    
    pub fn vec_key_value(&self, ptr:ObjectPtr, index:usize)->(Value,Value){
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_type().has_paired_vec(){
            if let Some(value) = object.vec.get(index * 2 + 1){
                return (object.vec[index * 2], *value)
            }
        }
        else if object.tag.get_type().is_vec1(){
            if let Some(value) = object.vec.get(index){
                return (NIL,*value)
            }
        }
        (NIL, NIL)
    }
        
    pub fn vec_value(&self, ptr:ObjectPtr, index:usize)->Value{
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_type().has_paired_vec(){
            if let Some(value) = object.vec.get(index * 2 + 1){
                return *value
            }
        }
        else if object.tag.get_type().is_vec1(){
            if let Some(value) = object.vec.get(index){
                return *value
            }
        }
        NIL
    }
        
    pub fn vec_len(&self, ptr:ObjectPtr)->usize{
        let object = &self.objects[ptr.index as usize];
        if object.tag.get_type().has_paired_vec(){
            object.vec.len() >> 1
        }
        else if object.tag.get_type().is_vec1(){
            object.vec.len()
        }
        else{
            0
        }
    }
    
    
    
    // Vec Writing
    
    
        
    pub fn vec_insert_value_at(&mut self, _ptr:ObjectPtr, _key:Value, _value:Value, _before:bool, _ip:ScriptIp)->Value{
        NIL
    }
        
    pub fn vec_insert_value_begin(&mut self, _ptr:ObjectPtr, _key:Value, _value:Value,_ip:ScriptIp)->Value{
        NIL
    }
        
    pub fn vec_push_vec(&mut self, target:ObjectPtr, source:ObjectPtr)->Value{
        let (target, source) = if target.index > source.index{
            let (o1, o2) = self.objects.split_at_mut(target.index as _);
            (&mut o2[0], &mut o1[source.index as usize])                    
        }else{
            let (o1, o2) = self.objects.split_at_mut(source.index as _);
            (&mut o1[target.index as usize], &mut o2[0])                    
        };
        target.push_vec_from_other(source);
        NIL
    }
        
    pub fn vec_push_vec_of_vec(&mut self, target:ObjectPtr, source:ObjectPtr, map:bool)->Value{
        let len = self.objects[source.index as usize].vec.len();
        for i in 0..len{
            if let Some(source) = self.objects[source.index as usize].vec[i].as_object(){
                let (target, source) = if target.index > source.index{
                    let (o1, o2) = self.objects.split_at_mut(target.index as _);
                    (&mut o2[0], &mut o1[source.index as usize])
                }else{
                    let (o1, o2) = self.objects.split_at_mut(source.index as _);
                    (&mut o1[target.index as usize], &mut o2[0])
                };
                target.push_vec_from_other(source);
                if map{
                    target.merge_map_from_other(source);
                }
            }
        }
        NIL
    }
        
    pub fn vec_push(&mut self, ptr: ObjectPtr, key: Value, value: Value)->Value{
        let object = &mut self.objects[ptr.index as usize];
        let ty = object.tag.get_type();
        if ty.has_paired_vec(){
            object.vec.extend_from_slice(&[key, value]);
            if let Some(obj) = value.as_object(){
                let object = &mut self.objects[obj.index as usize];
                object.tag.set_reffed();
            }
        }
        else if ty.is_typed(){
            println!("IMPLEMENT TYPED PUSH VALUE")
        }
        else{
            object.vec.push(value);
        }
        NIL
    }
            
    pub fn vec_remove(&mut self, ptr:ObjectPtr, index:usize)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.get_type().has_paired_vec(){
            object.vec.remove(index * 2);
            return object.vec.remove(index * 2);
        }
        else if object.tag.get_type().is_vec1(){
            return object.vec.remove(index);
        }
        else{
            NIL
        }
    }
        
    pub fn vec_pop(&mut self, ptr:ObjectPtr)->Value{
        if let Some(value) = self.objects[ptr.index as usize].vec.pop(){
            value
        }
        else{
            NIL
        }
    }
    
    
    
        
    // Functions
        
        
        
    pub fn set_fn(&mut self, ptr: ObjectPtr, fnptr: ScriptFnPtr){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_fn(fnptr);
    }
            
    pub fn as_fn(&self, ptr: ObjectPtr,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        object.tag.as_fn()
    }
            
    pub fn is_fn(&self, ptr: ObjectPtr,)->bool{
        let object = &self.objects[ptr.index as usize];
        object.tag.is_fn()
    }
            
    pub fn set_reffed(&mut self, ptr: ObjectPtr,){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_reffed();
    }
            
    pub fn parent_as_fn(&self, ptr: ObjectPtr,)->Option<ScriptFnPtr>{
        let object = &self.objects[ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            let fn_object = &self.objects[ptr.index as usize];
            fn_object.tag.as_fn()
        }
        else{
            None
        }
    }   
        
    pub fn unnamed_fn_arg(&mut self, top_ptr:ObjectPtr, value:Value, _ip:ScriptIp)->Value{
        let object = &self.objects[top_ptr.index as usize];
        
        // which arg number?
        let index = object.map.len();
        
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            if let Some(key) = object.vec.get(index*2){
                let key = *key;
                self.objects[top_ptr.index as usize].map.insert(key, value);
                if let Some(obj) = value.as_object(){
                    let object = &mut self.objects[obj.index as usize];
                    object.tag.set_reffed();
                }
            }
            else{
                // only allow if we are varargs
                self.objects[top_ptr.index as usize].vec.extend_from_slice(&[NIL, value]);
            }
        }
        NIL
    }
    
        
    pub fn named_fn_arg(&mut self, top_ptr:ObjectPtr, _name:Value, value:Value, _ip:ScriptIp)->Value{
        let object = &self.objects[top_ptr.index as usize];
                
        // which arg number?
        let index = object.map.len();
                
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            if let Some(key) = object.vec.get(index*2){
                let key = *key;
                self.objects[top_ptr.index as usize].map.insert(key, value);
                if let Some(obj) = value.as_object(){
                    let object = &mut self.objects[obj.index as usize];
                    object.tag.set_reffed();
                }
            }
            else{
                // only allow if we are varargs
                self.objects[top_ptr.index as usize].vec.extend_from_slice(&[NIL, value]);
            }
        }
        NIL
    }
            
    pub fn push_all_fn_args(&mut self, top_ptr:ObjectPtr, args:&[Value]){
        let object = &self.objects[top_ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            for (index, value) in args.iter().enumerate(){
                let object = &self.objects[ptr.index as usize];
                if let Some(key) = object.vec.get(index*2){
                    let key = *key;
                    self.objects[top_ptr.index as usize].map.insert(key, *value);
                    if let Some(obj) = value.as_object(){
                        let object = &mut self.objects[obj.index as usize];
                        object.tag.set_reffed();
                    }
                }
                else{
                    self.objects[top_ptr.index as usize].vec.extend_from_slice(&[NIL, *value]);
                }
            }
        }
    }
    
    
    
    // Debug and utility
    
    
    
        
    pub fn deep_eq(&self, a:Value, b:Value)->bool{
        if a == b{
            return true
        }
        if a.is_object(){
            let mut aw = a;
            let mut bw = b;
            loop{
                if let Some(pa) = aw.as_object(){
                    if let Some(pb) = bw.as_object(){
                        let oa = &self.objects[pa.index as usize];
                        let ob = &self.objects[pb.index as usize];
                        if oa.vec.len() != ob.vec.len(){
                            return false
                        }
                        for (a,b) in oa.vec.iter().zip(ob.vec.iter()){
                            if !self.deep_eq(*a, *b){
                                return false
                            }
                        }
                        if oa.map.len() != ob.map.len(){
                            return false
                        }
                        for (a,b) in oa.map.iter().zip(ob.map.iter()){
                            if !self.deep_eq(*a.0, *b.0){
                                return false
                            }
                            if !self.deep_eq(*a.1, *b.1){
                                return false
                            }
                        }
                        aw = oa.proto;
                        bw = ob.proto;
                        if aw == bw{
                            return true
                        }
                    }
                    else{
                        return false
                    }
                }
                else{
                    return false
                }
            }
        }
        else {
            self.shallow_eq(a, b)
        }
    }
        
    pub fn shallow_eq(&self, a:Value, b:Value)->bool{
        if a == b{
            return true
        }
        if let Some(cmp) = a.as_inline_string(|a|{
            if let Some(cmp) = b.as_inline_string(|b|{
                a == b
            }){return cmp}
            else{
                if let Some(b)  = b.as_string(){
                    self.string(b) == a
                }
                else{
                    false
                }
            }
        }){return cmp}
        else if let Some(a) = a.as_string(){
            let a = self.string(a);
            if let Some(cmp) = b.as_inline_string(|b|{
                a == b
            }){return cmp}
            else{
                if let Some(b)  = b.as_string(){
                    return self.string(b) == a
                }
            }
        }
        false
    }
    
    pub fn print_key_value(&self, key:Value, value:Value, deep:bool, str:&mut String){
        if let Some(obj) = value.as_object(){
            if !key.is_nil(){
                str.clear();self.cast_to_string(key, str);
                print!("{}:", str);
            }
            let object = &self.objects[obj.index as usize];
            if object.tag.is_script_fn(){
                print!("Fn");
                self.print(obj,false);
            }
            else if object.tag.is_native_fn(){
                print!("Native");
                self.print(obj,false);
            }
            else{
                self.print(obj, deep);
            }
        }
        else{
            if !key.is_nil(){
                str.clear();self.cast_to_string(key, str);
                print!("{}:",str);
            }
            str.clear();self.cast_to_string(value, str);
            print!("{}",str);
        }
    }
    
    
    pub fn print(&self, set_ptr:ObjectPtr, deep:bool){
        let mut ptr = set_ptr;
        let mut str = String::new();
        // scan up the chain to set the proto value
        print!("{{");
        let mut first = true;
        loop{
            let object = &self.objects[ptr.index as usize];
            for (key, value) in object.map.iter(){
                if !first{print!(",")}
                self.print_key_value(*key, *value, deep, &mut str);
                first = false;
            }
            if object.tag.get_type().has_paired_vec(){
                for chunk in object.vec.chunks(2){
                    if !first{print!(",")}
                    self.print_key_value(chunk[0], chunk[1], deep, &mut str);
                    first = false;
                }
            }
            else if !object.tag.get_type().is_typed(){
            }else{
            }
                
            if let Some(next_ptr) = object.proto.as_object(){
                if !deep{
                    break
                }
                if !first{print!(",")}
                print!("^");
                ptr = next_ptr
            }
            else{
                break;
            }
        }
        print!("}}");
    }
    
    // memory  usage
    pub fn objects_len(&self)->usize{
        self.objects.len()
    }
}
