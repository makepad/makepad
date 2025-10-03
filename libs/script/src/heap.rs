use std::fmt::Write;
use crate::value::*;
use crate::object::*;
use crate::id::*;
use makepad_script_derive::*;

#[derive(Default)]
pub struct HeapTag(u32);

impl HeapTag{
    const MARK:u32 = 0x1;
    const ALLOCED:u32 = 0x2;
    const SHALLOW_PROTO:u32 = 0x4;
    
    pub fn set_shallow_proto(&mut self){
        self.0 |= Self::SHALLOW_PROTO
    }
    
    pub fn is_shallow_proto(&self)->bool{
        self.0 & Self::SHALLOW_PROTO != 0
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
pub struct HeapString{
    pub tag: HeapTag,
    pub string: String
}

impl HeapString{
    fn clear(&mut self){
        self.tag.clear();
        self.string.clear()
    }
}

pub struct HeapZone{
    pub zone: u8,
    pub mark_vec: Vec<usize>,
    pub objects: Vec<Object>,
    pub roots: Vec<usize>,
    pub objects_free: Vec<usize>,
    pub strings: Vec<HeapString>,
    pub strings_free: Vec<usize>
}

impl HeapZone{
    pub fn new_string(&mut self)->StringPtr{
        if let Some(index) = self.strings_free.pop(){
            self.strings[index].tag.set_alloced();
            StringPtr{
                zone: self.zone,
                index: index as _
            }
        }
        else{
            let index = self.strings.len();
            let mut string = HeapString::default();
            string.tag.set_alloced();
            self.strings.push(string);
            StringPtr{
                zone: self.zone,
                index: index as _
            }
        }
    }
    
    pub fn new_object(&mut self)->ObjectPtr{
        if let Some(index) = self.objects_free.pop(){
            self.objects[index].tag.set_alloced();
            ObjectPtr{
                zone: self.zone,
                index: index as _
            }
        }
        else{
            let index = self.objects.len();
            let mut object = Object::default();
            object.tag.set_alloced();
            self.objects.push(object);
            ObjectPtr{
                zone: self.zone,
                index: index as _
            }
        }
    }
    
    pub fn new_shallow_object(&mut self)->ObjectPtr{
        if let Some(index) = self.objects_free.pop(){
            let object = &mut self.objects[index];
            object.tag.set_alloced();
            object.tag.set_shallow_proto();
            ObjectPtr{
                zone: self.zone,
                index: index as _
            }
        }
        else{
            let index = self.objects.len();
            let mut object = Object::default();
            object.tag.set_alloced();
            object.tag.set_shallow_proto();
            self.objects.push(object);
            ObjectPtr{
                zone: self.zone,
                index: index as _
            }
        }
    }
    /*
    pub fn free_object(&mut self, index: usize){
        self.objects[index].tag.clear();
        self.objects_free.push(index);
    }
     */
    fn new(zone:u8)->Self{
        Self{
            zone,
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
                if ptr.zone == self.zone{
                    self.mark_vec.push(ptr.index as usize);
                }
            }
            else if let Some(ptr) = field.key.as_string(){
                if ptr.zone == self.zone{
                    self.strings[ptr.index as usize].tag.set_mark();
                }
            }
            if let Some(ptr) = field.value.as_object(){
                if ptr.zone == self.zone{
                    self.mark_vec.push(ptr.index as usize);
                }
            }
            else if let Some(ptr) = field.value.as_string(){
                if ptr.zone == self.zone{
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
                if ptr.zone == self.zone{
                    self.mark_vec.push(ptr.index as usize);
                }
            }
            else if let Some(ptr) = value.as_string(){
                if ptr.zone == self.zone{
                    self.strings[ptr.index as usize].tag.set_mark();
                }
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
}

pub struct ScriptHeap{
    pub zones: [HeapZone;2],
}

impl Default for ScriptHeap{
    fn default()->Self{
        Self{
            zones: [HeapZone::new(0), HeapZone::new(1)]
        }
    }
}

impl ScriptHeap{
    const STATIC: usize = 0;
    const DYNAMIC: usize = 1;
    
    pub fn null_string(&self)->StringPtr{
        StringPtr{
            zone: Self::STATIC as u8,
            index: 0
        }
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
        &self.zones[ptr.zone as usize].strings[ptr.index as usize].string
    }
        
    pub fn cast_to_string(&self, v:Value, out:&mut String){
        if let Some(v) = v.as_string(){
            let str = self.string(v);
            out.push_str(str);
        }
        if let Some(v) = v.as_f64(){
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
    
    pub fn new_dyn_string(&mut self)->StringPtr{
        self.zones[Self::DYNAMIC].new_string()
    }
    
    pub fn new_dyn_string_from(&mut self,value:&str)->StringPtr{
        self.new_dyn_string_with(|_,out|{
            out.push_str(value);
        })
    }
    
    pub fn new_dyn_string_with<F:FnOnce(&mut Self, &mut String)>(&mut self,cb:F)->StringPtr{
        let mut out = String::new();
        let ptr = self.zones[Self::DYNAMIC].new_string();
        std::mem::swap(&mut out, &mut self.zones[ptr.zone as usize].strings[ptr.index as usize].string);
        cb(self, &mut out);
        std::mem::swap(&mut out, &mut self.zones[ptr.zone as usize].strings[ptr.index as usize].string);
        ptr
    }
    
    pub fn swap_string(&mut self, ptr: StringPtr, swap:&mut String){
        std::mem::swap(swap, &mut self.zones[ptr.zone as usize].strings[ptr.index as usize].string);
    }
    
    pub fn mut_string(&mut self, ptr: StringPtr)->&mut String{
        &mut self.zones[ptr.zone as usize].strings[ptr.index as usize].string
    }
    
    pub fn new_dyn_object(&mut self)->ObjectPtr{
        self.zones[Self::DYNAMIC].new_object()
    }
    
    pub fn new_dyn_shallow_object(&mut self)->ObjectPtr{
        self.zones[Self::DYNAMIC].new_shallow_object()
    }
        /*
    pub fn free_object(&mut self, ptr:ObjectPtr){
        self.zones[ptr.zone as usize].free_object(ptr.index as usize);
    }
    */
    pub fn new_static_string(&mut self, string:String)->StringPtr{
        let zone = &mut self.zones[Self::STATIC];
        let index = zone.strings.len();
        zone.strings.push(HeapString{
            tag: Default::default(),
            string
        });
        StringPtr{
            zone: Self::STATIC as u8,
            index: index as u32
        }
    }
    
    pub fn object_value(&self, set_ptr:ObjectPtr, key:Value)->Value{
        let mut ptr = set_ptr;
        loop{
            let object = &self.zones[ptr.zone as usize].objects[ptr.index as usize];
            if key == id!(proto).to_value(){
                return object.proto
            }
            for field in &object.fields{
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
    
    pub fn set_object_value(&mut self, set_ptr:ObjectPtr, key:Value, value:Value){
        let object = &mut self.zones[set_ptr.zone as usize].objects[set_ptr.index as usize];
        
        if key == id!(__prototype__).to_value(){
            object.proto = value;
            return
        }
        
        if key.is_nil(){ // array like push
            object.fields.push(Field{
                key,
                value
            })
        }
        
        if object.tag.is_shallow_proto(){
            let mut ptr = set_ptr;
            // scan up the chain to set the proto value
            loop{
                let object = &mut self.zones[ptr.zone as usize].objects[ptr.index as usize];
                for field in &mut object.fields{
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
            let object = &mut self.zones[set_ptr.zone as usize].objects[set_ptr.index as usize];
            object.fields.push(Field{
                key,
                value
            });
            return
        }
        
        for field in &mut object.fields{
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
}
