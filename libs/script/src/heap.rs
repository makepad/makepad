use std::fmt::Write;
use crate::value::*;
use crate::object::*;

#[derive(Default)]
pub struct ObjectTag(u64);

impl ObjectTag{
    const MARK:u64 = 0x1;
    const ALLOCED:u64 = 0x2;
    const DEEP:u64 = 0x4;
    const FN: u64 = 0x8;
    const REFFED: u64 = 0x10;
    
    pub fn set_fn(&mut self, val: u32){
        self.0 |= ((val as u64)<<32) | Self::FN
    }
    
    pub fn get_fn(&self)->u32{
        (self.0 >> 32) as u32
    }
    
    pub fn is_fn(&self)->bool{
        self.0 & Self::FN != 0
    }
        
    pub fn set_deep(&mut self){
        self.0 |= Self::DEEP
    }
    
    pub fn set_reffed(&mut self){
        self.0 |= Self::REFFED
    }
    
    pub fn is_reffed(&self)->bool{
        self.0 & Self::REFFED != 0
    }
    
    pub fn clear_deep(&mut self){
        self.0 &= !Self::DEEP
    }
    
    pub fn is_deep(&self)->bool{
        self.0 & Self::DEEP != 0
    }
        
    pub fn is_alloced(&self)->bool{
        return self.0 & Self::ALLOCED != 0
    }
    
    pub fn set_alloced(&mut self){
        self.0 |= Self::ALLOCED
    }
    
    pub fn clear(&mut self){
        self.0 = 0;
    }
    
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
    
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
    
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
}


#[derive(Default)]
pub struct StringTag(u64);

impl StringTag{
    const MARK:u64 = 0x1;
    const ALLOCED:u64 = 0x2;
    
    pub fn is_alloced(&self)->bool{
        return self.0 & Self::ALLOCED != 0
    }
        
    pub fn set_alloced(&mut self){
        self.0 |= Self::ALLOCED
    }
        
    pub fn clear(&mut self){
        self.0 = 0;
    }
        
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
        
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
        
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
}

#[derive(Default)]
pub struct HeapString{
    pub tag: StringTag,
    pub string: String
}

impl HeapString{
    fn clear(&mut self){
        self.tag.clear();
        self.string.clear()
    }
}

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
    
    pub fn new_object(&mut self)->ObjectPtr{
        if let Some(index) = self.objects_free.pop(){
            self.objects[index].tag.set_alloced();
            ObjectPtr{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = Object::default();
            object.tag.set_alloced();
            self.objects.push(object);
            ObjectPtr{index: index as _}
        }
    }
        
    pub fn new_deep_object(&mut self)->ObjectPtr{
        if let Some(index) = self.objects_free.pop(){
            let object = &mut self.objects[index];
            object.tag.set_alloced();
            object.tag.set_deep();
            ObjectPtr{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = Object::default();
            object.tag.set_alloced();
            object.tag.set_deep();
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
        let len = obj.fields.len();
        for i in 0..len{
            let field = &self.objects[index].fields[i];
            if let Some(ptr) = field.key.as_object(){
                self.mark_vec.push(ptr.index as usize);
            }
            else if let Some(ptr) = field.key.as_string(){
                self.strings[ptr.index as usize].tag.set_mark();
            }
            if let Some(ptr) = field.value.as_object(){
                self.mark_vec.push(ptr.index as usize);
            }
            else if let Some(ptr) = field.value.as_string(){
                self.strings[ptr.index as usize].tag.set_mark();
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
        let deep = if let Some(ptr) = proto.as_object(){
            let object = &mut self.objects[ptr.index as usize];
            object.tag.set_reffed();
            object.tag.is_deep()
        }
        else{
            false
        };
        if let Some(index) = self.objects_free.pop(){
            let object = &mut self.objects[index];
            object.tag.set_alloced();
            if deep{object.tag.set_deep()}
            object.proto = proto;
            ObjectPtr{index: index as _}
        }
        else{
            let index = self.objects.len();
            let mut object = Object::with_proto(proto);
            object.tag.set_alloced();
            if deep{object.tag.set_deep()}
            self.objects.push(object);
            ObjectPtr{index: index as _}
        }
    }
    
    pub fn set_object_deep(&mut self, ptr:ObjectPtr){
         self.objects[ptr.index as usize].tag.set_deep()
    }
        
    pub fn clear_object_deep(&mut self, ptr:ObjectPtr){
        self.objects[ptr.index as usize].tag.clear_deep()
    }
    
    pub fn object_value(&self, set_ptr:ObjectPtr, key:Value)->Value{
        let mut ptr = set_ptr;
        loop{
            let object = &self.objects[ptr.index as usize];
            for field in object.fields.iter().rev(){
                if field.key == key{
                    return field.value
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
    
    pub fn push_object_value(&mut self, set_ptr:ObjectPtr, key: Value, value:Value){
        let object = &mut self.objects[set_ptr.index as usize];
        object.fields.push(Field{
            key,
            value
        });
    }
    
    pub fn set_object_value_top(&mut self, set_ptr:ObjectPtr, key:Value, value:Value){
        let object = &mut self.objects[set_ptr.index as usize];
        for field in object.fields.iter_mut().rev(){
            if field.key == key{
                field.value = value;
                return
            }
        }
        object.fields.push(Field{
            key,
            value
        });
    }
    
    pub fn set_object_is_fn(&mut self, ptr: ObjectPtr, ip: u32){
        let object = &mut self.objects[ptr.index as usize];
        object.tag.set_fn(ip);
    }
        
    pub fn get_object_is_fn(&self, ptr: ObjectPtr,)->Option<u32>{
        let object = &self.objects[ptr.index as usize];
        if object.tag.is_fn(){
            Some(object.tag.get_fn())
        }
        else{
            None
        }
    }
    
    pub fn get_parent_object_is_fn(&self, ptr: ObjectPtr,)->Option<u32>{
        let object = &self.objects[ptr.index as usize];
        if let Some(ptr) = object.proto.as_object(){
            let object = &self.objects[ptr.index as usize];
            if object.tag.is_fn(){
                Some(object.tag.get_fn())
            }
            else{
                None
            }
        }
        else{
            None
        }
    }   
               
    pub fn set_object_value(&mut self, set_ptr:ObjectPtr, key:Value, value:Value){
        let object = &mut self.objects[set_ptr.index as usize];
                
        if key.is_nil(){ // array like push
            object.fields.push(Field{
                key,
                value
            });
            return
        }
        
        // deep objects do value mutations on their prototypes
        if object.tag.is_deep(){
            let mut ptr = set_ptr;
            // scan up the chain to set the proto value
            loop{
                let object = &mut self.objects[ptr.index as usize];
                for field in object.fields.iter_mut().rev(){
                    if field.key == key{
                        field.value = value;
                        return
                    }
                }
                if let Some(next_ptr) = object.proto.as_object(){
                    ptr = next_ptr
                }
                else{
                    break;
                }
            }
            // append to current object
            let object = &mut self.objects[set_ptr.index as usize];
            object.fields.push(Field{
                key,
                value
            });
            return
        }
        
        for field in object.fields.iter_mut().rev(){
            if field.key == key{
                field.value = value;
                return
            }
        }
        
        object.fields.push(Field{
            key,
            value
        });
    }
    
    pub fn print_object(&self, set_ptr:ObjectPtr, deep:bool){
        let mut ptr = set_ptr;
        let mut str = String::new();
        // scan up the chain to set the proto value
        print!("{{");
        let mut first = true;
        loop{
            let object = &self.objects[ptr.index as usize];
            for field in object.fields.iter(){
                if !first{print!(",")}
                if let Some(obj) = field.value.as_object(){
                    if !field.key.is_nil(){
                        str.clear();self.cast_to_string(field.key, &mut str);
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
                    if !field.key.is_nil(){
                        str.clear();self.cast_to_string(field.key, &mut str);
                        print!("{}:",str);
                    }
                    str.clear();self.cast_to_string(field.value, &mut str);
                    print!("{}",str);
                }
                first = false;
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
