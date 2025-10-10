use makepad_script_derive::*;
use crate::id::*;
use std::fmt::Write;
use crate::value::*;
use crate::object::*;
use crate::string::*;

pub struct ScriptHeap{
    pub mark_vec: Vec<usize>,
    pub objects: Vec<Object>,
    pub roots: Vec<usize>,
    pub objects_free: Vec<usize>,
    pub strings: Vec<HeapString>,
    pub strings_free: Vec<usize>
}

impl ScriptHeap{
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
    
    pub fn new_object(&mut self, flags: u64)->ObjectPtr{
        if let Some(index) = self.objects_free.pop(){
            self.objects[index].tag.set_flags(flags | ObjectTag::ALLOCED);
            ObjectPtr{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = Object::default();
            object.tag.set_flags(flags | ObjectTag::ALLOCED);
            self.objects.push(object);
            ObjectPtr{index: index as _}
        }
    }
    /*
    pub fn free_object(&mut self, index: usize){
        self.objects[index].tag.clear();
        self.objects_free.push(index);
    }
    */
    pub fn new()->Self{
        Self{
            mark_vec: Default::default(),
            objects: Default::default(),
            roots: vec![0],
            objects_free: Default::default(),
            // index 0 is always an empty string
            strings: vec![Default::default()],
            strings_free: Default::default(),
        }
    }
        
    pub fn mark_inner(&mut self, index:usize){
        let obj = &mut self.objects[index];
        if obj.tag.is_marked() || !obj.tag.is_alloced(){
            return;
        }
        obj.tag.set_mark();
        for (key, value) in &obj.map{
            if let Some(ptr) = key.as_object(){
                self.mark_vec.push(ptr.index as usize);
            }
            else if let Some(ptr) = key.as_string(){
                self.strings[ptr.index as usize].tag.set_mark();
            }
            if let Some(ptr) = value.as_object(){
                self.mark_vec.push(ptr.index as usize);
            }
            else if let Some(ptr) = value.as_string(){
                self.strings[ptr.index as usize].tag.set_mark();
            }
        }
        let len = obj.vec.len();
        for i in 0..len{
            let object = &self.objects[index];
            if object.tag.get_type().is_gc(){
                let field = &object.vec[i];
                if let Some(ptr) = field.as_object(){
                    self.mark_vec.push(ptr.index as usize);
                }
                else if let Some(ptr) = field.as_string(){
                    self.strings[ptr.index as usize].tag.set_mark();
                }
            }
        }
    }
            
    pub fn mark(&mut self, stack:&[Value]){
        self.mark_vec.clear();
        for i in 0..self.roots.len(){
            self.mark_inner(self.mark_vec[i]);
        }
        for i in 0..stack.len(){
            let value = stack[i];
            if let Some(ptr) = value.as_object(){
                self.mark_vec.push(ptr.index as usize);
            }
            else if let Some(ptr) = value.as_string(){
                self.strings[ptr.index as usize].tag.set_mark();
            }
        }
        for i in 0..self.mark_vec.len(){
            self.mark_inner(self.mark_vec[i]);
        }
    }
            
    pub fn sweep(&mut self){
        for i in 0..self.objects.len(){
            let obj = &mut self.objects[i];
            if !obj.tag.is_marked() && obj.tag.is_alloced(){
                obj.clear();
                self.objects_free.push(i);
            }
            else{
                obj.tag.clear_mark();
            }
        }
        // always leave the empty null string at 0
        for i in 1..self.strings.len(){
            let str = &mut self.strings[i];
            if !str.tag.is_marked() && str.tag.is_alloced(){
                str.clear();
                self.strings_free.push(i)
            }
            else {
                str.tag.clear_mark();
            }
        }
    }
    
    pub fn free_object_if_unreffed(&mut self, ptr:ObjectPtr){
        let obj = &mut self.objects[ptr.index as usize];
        if !obj.tag.is_reffed(){
            obj.clear();
            self.objects_free.push(ptr.index as usize);
        }
    }
    
    pub fn new_object_if_reffed(&mut self, ptr:ObjectPtr)->ObjectPtr{
        let obj = &self.objects[ptr.index as usize];
        if obj.tag.is_reffed(){
            let proto = obj.proto;
            return self.new_object_with_proto(proto);
        }
        return ptr;
    }
    
    pub fn null_string(&self)->StringPtr{
        StringPtr{index: 0}
    }
    
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
        
        else{
            write!(out, "[Unknown]").ok();
        }
    }
        
    pub fn cast_to_f64(&self, v:Value)->f64{
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
        else if let Some(_v) = v.as_id(){
            return 0.0
        }
        else if let Some(_v) = v.as_object(){
            return 0.0
        }
        else if let Some(v) = v.as_color(){
            return v as f64
        }
        else if v.is_nil(){
            0.0
        }
        else if v.is_opcode(){
            0.0
        }
        else{
            0.0
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
        if v.inline_string_not_empty(){
            return true
        }
        if let Some(v) = v.as_string(){
            return self.string(v).len() != 0
        }
        if let Some(_v) = v.as_id(){
            return true
        }
        if let Some(_v) = v.as_object(){
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
    
    pub fn swap_string(&mut self, ptr: StringPtr, swap:&mut String){
        std::mem::swap(swap, &mut self.strings[ptr.index as usize].string);
    }
    
    pub fn mut_string(&mut self, ptr: StringPtr)->&mut String{
        &mut self.strings[ptr.index as usize].string
    }
        
    pub fn new_string_from_string(&mut self, string:String)->StringPtr{
        let index = self.strings.len();
        self.strings.push(HeapString{
            tag: Default::default(),
            string
        });
        StringPtr{index: index as u32}
    }
            
    pub fn new_object_with_proto(&mut self, proto:Value)->ObjectPtr{
        let (proto_fwd, proto_index) = if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            object.tag.set_reffed();
            (object.tag.proto_fwd(), ptr.index as usize)
        }
        else{
            let ptr = self.new_object(0);
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
    
    pub fn set_object_deep(&mut self, ptr:ObjectPtr){
         self.objects[ptr.index as usize].tag.set_deep()
    }
    
    pub fn set_object_type(&mut self, ptr:ObjectPtr, ty: ObjectType){
        self.objects[ptr.index as usize].set_type(ty)
    }
    
    pub fn set_object_system_fn(&mut self, ptr:ObjectPtr, val:u32){
        self.objects[ptr.index as usize].tag.set_system_fn(val)
    }
        
    pub fn clear_object_deep(&mut self, ptr:ObjectPtr){
        self.objects[ptr.index as usize].tag.clear_deep()
    }
    
    pub fn object_value_index(&self, ptr: ObjectPtr, index: Value)->Value{
        let object = &self.objects[ptr.index as usize];
        
        let ty = object.tag.get_type();
        // most used path
        if ty.uses_vec2(){
            let index = index.as_index();
            if let Some(value) = object.vec.get(index * 2 + 1){
                return *value
            }
            else{
                return Value::NIL
            }
        }
        if ty.is_vec1(){
            let index = index.as_index();
            if let Some(value) = object.vec.get(index){
                return *value
            }
            else{
                return Value::NIL
            }
        }
        if ty.is_typed(){ // typed access to the vec
            //todo IMPLEMENT IT
        }
        if ty.is_btree(){
            if let Some(value) = object.map.get(&index){
                return *value
            }
            else{
                return Value::NIL
            }
        }
        Value::NIL
    }
    
    pub fn object_value_prefixed(&self, ptr: ObjectPtr, key: Value)->Value{
        let object = &self.objects[ptr.index as usize];
        for chunk in object.vec.rchunks(2){
            if chunk[0] == key{
                return chunk[1]
            }
        }
        return Value::NIL
    }
    
    pub fn object_value_deep(&self, obj_ptr:ObjectPtr, key: Value)->Value{
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
        Value::NIL
    }
    
    pub fn object_method(&self, ptr:ObjectPtr, key:Value)->Value{
        let object = &self.objects[ptr.index as usize];
        if object.tag.has_methods(){
            self.object_value(ptr, key)
        }
        else{
            Value::NIL
        }
    }
    
    pub fn object_value(&self, ptr:ObjectPtr, key:Value)->Value{
        // hard array index
        if key.is_id(){
            return self.object_value_deep(ptr, key)
        }
        if key.is_index(){
            return self.object_value_index(ptr, key)
        }
        if key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            return self.object_value_deep(ptr, key)
        }
        // TODO implement string lookup
        Value::NIL
    }
    
    pub fn object_prototype(&self, ptr:ObjectPtr)->Value{
        self.objects[ptr.index as usize].proto
    }
    
    pub fn object(&self, ptr:ObjectPtr)->&Object{
        &self.objects[ptr.index as usize]
    }
    
    pub fn set_object_value_index(&mut self, ptr: ObjectPtr, index:Value, value: Value){
        // alright so. now what.
        let object = &mut self.objects[ptr.index as usize];
        let ty = object.tag.get_type();
        if ty.uses_vec2(){
            let index = index.as_index() * 2;
            if index + 1 >= object.vec.len(){
                object.vec.resize(index + 2, Value::NIL);
            }
            object.vec[index] = Value::NIL;
            object.vec[index+1] = value; 
            return 
        }
        if ty.is_vec1(){
            let index = index.as_index();
            if index>= object.vec.len(){
                object.vec.resize(index, Value::NIL);
            }
            object.vec[index] = value;
            return 
        }
        if ty == ObjectType::BTREE{
            object.map.insert(index, value);
            return
        }
        if ty.is_typed(){ // typed array
            println!("Implement typed array set value");
            //todo IMPLEMENT IT
            return
        }
    }
    
    pub fn push_object_vec_into_object_vec(&mut self, target:ObjectPtr, source:ObjectPtr){
        let (target, source) = if target.index > source.index{
            let (o1, o2) = self.objects.split_at_mut(target.index as _);
            (&mut o2[0], &mut o1[source.index as usize])                    
        }else{
            let (o1, o2) = self.objects.split_at_mut(source.index as _);
            (&mut o1[target.index as usize], &mut o2[0])                    
        };
        target.push_vec_from_other(source);
    }
    
    pub fn push_object_vec_of_vec_into_object_vec(&mut self, target:ObjectPtr, source:ObjectPtr, map:bool){
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
    }
    
    pub fn object_push_value(&mut self, ptr: ObjectPtr, key: Value, value: Value){
        let object = &mut self.objects[ptr.index as usize];
        let ty = object.tag.get_type();
        if ty.has_paired_vec(){
            object.vec.extend_from_slice(&[key, value]);
            //if key.is_unprefixed_id() && !object.tag.get_type().is_vec2(){ 
            //    object.map.insert(key, value);
            //}
        }
        else if ty.is_typed(){
            println!("IMPLEMENT TYPED PUSH VALUE")
        }
        else{
            object.vec.push(value);
        }
    }
    
    pub fn set_object_value_prefixed(&mut self, ptr: ObjectPtr, key: Value, value: Value){
        let object = &mut self.objects[ptr.index as usize];
        for chunk in object.vec.rchunks_mut(2){
            if chunk[0] == key{
                chunk[1] = value;
                return
            }
        }
        // just append it
        object.vec.extend_from_slice(&[key, value]);
    }
    
    pub fn set_object_value_deep(&mut self, ptr:ObjectPtr, key: Value, value: Value){
        let mut ptr = ptr;
        loop{
            let object = &mut self.objects[ptr.index as usize];
            if object.tag.get_type().has_paired_vec(){
                for chunk in object.vec.rchunks_mut(2){
                    if chunk[0] == key{
                        chunk[1] = value;
                        return
                    }
                }
            }
            if let Some(set_value) = object.map.get_mut(&key){
                *set_value = value;
                return
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
    }
    
    pub fn set_object_value_shallow(&mut self, ptr:ObjectPtr, key:Value, value:Value){
        let object = &mut self.objects[ptr.index as usize];
        if object.tag.get_type().is_vec2(){
            for chunk in object.vec.rchunks_mut(2){
                if chunk[0] == key{
                    chunk[1] = value;
                    return
                }
            }
            object.vec.extend_from_slice(&[key, value]);
            return
        }
        object.map.insert(key, value);
    }
    
    pub fn insert_object_value_at(&mut self, _ptr:ObjectPtr, _key:Value, _value:Value, _before:bool){
    }
    
    pub fn insert_object_value_begin(&mut self, _ptr:ObjectPtr, _key:Value, _value:Value){
    }
    
    pub fn set_object_value(&mut self, ptr:ObjectPtr, key:Value, value:Value){
        if key.is_index(){ // use vector
            return self.set_object_value_index(ptr, key, value);
        }
        if let Some(id) = key.as_id(){
            // mark object as having methods
            if let Some(obj) = value.as_object(){
                if self.object_is_fn(obj){
                    let object = &mut self.objects[ptr.index as usize];
                    object.tag.set_has_methods();                    
                }
            }
            if id.is_prefixed(){
                return self.set_object_value_prefixed(ptr, key, value)
            }
            // scan prototype chain for id
            let object = &self.objects[ptr.index as usize];
            if !object.tag.is_deep(){
                return self.set_object_value_shallow(ptr, key, value);
            }
            else{
                return self.set_object_value_deep(ptr, key, value)
            }
        }
        if key.is_object() || key.is_color() || key.is_bool(){ // scan protochain for object
            let object = &mut self.objects[ptr.index as usize];
            if !object.tag.is_deep(){
                return self.set_object_value_shallow(ptr, key, value);
            }
            else{
                return self.set_object_value_deep(ptr, key, value)
            }
        }
        println!("Cant set object value with key {:?}", key);
    }
    
    pub fn set_fn_this(&mut self, ptr:ObjectPtr, this:Value){
        let object = &mut self.objects[ptr.index as usize];
        object.map.insert(id!(this).into(), this);
    }
    
    pub fn fn_this(&mut self, ptr:ObjectPtr)->Value{
        let object = &mut self.objects[ptr.index as usize];
        if let Some(value) = object.map.get(&id!(this).into()){
            return *value
        }
        Value::NIL
    }
        
        
    pub fn push_object_value(&mut self, ptr:ObjectPtr, key:Value, value:Value){
        let object = &mut self.objects[ptr.index as usize];
        object.vec.extend_from_slice(&[key, value]);
    }
    
    pub fn set_object_is_fn(&mut self, ptr: ObjectPtr, ip: u32){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_fn(ip);
    }
    
    pub fn set_object_is_system_fn(&mut self, ptr: ObjectPtr, ip: u32){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_system_fn(ip);
    }
        
    pub fn get_object_as_fn(&self, ptr: ObjectPtr,)->Option<u32>{
        let object = &self.objects[ptr.index as usize];
        if object.tag.is_fn(){
            Some(object.tag.get_fn())
        }
        else{
            None
        }
    }
    
    pub fn object_is_fn(&self, ptr: ObjectPtr,)->bool{
        let object = &self.objects[ptr.index as usize];
        object.tag.is_fn()
    }
    
    
    pub fn parent_object_as_fn(&self, ptr: ObjectPtr,)->Option<(u32, bool)>{
        let object = &self.objects[ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            let fn_object = &self.objects[ptr.index as usize];
            if fn_object.tag.is_fn() || fn_object.tag.is_system_fn(){
                Some((fn_object.tag.get_fn(), fn_object.tag.is_system_fn()))
            }
            else{
                None
            }
        }
        else{
            None
        }
    }   
    
    pub fn push_fn_arg(&mut self, top_ptr:ObjectPtr, value:Value){
        let object = &self.objects[top_ptr.index as usize];
        let index = object.vec.len();
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            if let Some(key) = object.vec.get(index*2){
                let key = *key;
                self.objects[top_ptr.index as usize].vec.extend_from_slice(&[key, value]);
            }
            else{
                self.objects[top_ptr.index as usize].vec.extend_from_slice(&[Value::NIL, value]);
            }
        }
    }
    
    pub fn print_key_value(&self, key:Value, value:Value, deep:bool, str:&mut String){
        if let Some(obj) = value.as_object(){
            if !key.is_nil(){
                str.clear();self.cast_to_string(key, str);
                print!("{}:", str);
            }
            let object = &self.objects[obj.index as usize];
            if object.tag.is_fn(){
                print!("Fn");
                self.print_object(obj,false);
            }
            else{
                self.print_object(obj, deep);
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
    
    pub fn print_object(&self, set_ptr:ObjectPtr, deep:bool){
        let mut ptr = set_ptr;
        let mut str = String::new();
        // scan up the chain to set the proto value
        print!("{{");
        let mut first = true;
        loop{
            let object = &self.objects[ptr.index as usize];
            for (key, value) in &object.map{
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
}
