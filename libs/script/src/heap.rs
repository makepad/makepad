use std::fmt::Write;
use crate::value::*;
use crate::object::*;

#[derive(Default)]
pub struct HeapTag(u32);

impl HeapTag{
    const FREE:u32 = 0x1;
    const ARRAY:u32 = 0x2;
    const MARK:u32 = 0x4;
    
    // means all data is packed 
    pub fn is_array(&self)->bool{
        self.0&Self::ARRAY != 0
    }
    
    pub fn is_free(&self)->bool{
        return self.0 & Self::FREE != 0
    }
    
    pub fn set_free(&mut self){
        self.0 = Self::FREE;
    }
        
    pub fn clear_free(&mut self){
        self.0 = Self::FREE;
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
            self.strings[index].tag.clear_free();
            StringPtr{
                zone: self.zone,
                index: index as _
            }
        }
        else{
            let index = self.strings.len();
            self.strings.push(Default::default());
            StringPtr{
                zone: self.zone,
                index: index as _
            }
        }
    }
    
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
        if obj.tag.is_marked() || obj.tag.is_free(){
            return;
        }
        obj.tag.set_mark();
        let len = obj.fields.len();
        for i in 0..len{
            let value = self.objects[index].fields[i];
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
            if !obj.tag.is_marked() && !obj.tag.is_free(){
                obj.tag.set_free();
                obj.fields.clear();
                self.objects_free.push(i);
            }
            else{
                obj.tag.clear_mark();
            }
        }
        // always leave the empty null string at 0
        for i in 1..self.strings.len(){
            let str = &mut self.strings[i];
            if !str.tag.is_marked() && !str.tag.is_free(){
                str.tag.set_free();
                str.string.clear();
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
    
    pub fn alloc_static_string(&mut self, string:String)->StringPtr{
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
    
    pub fn freeze(&mut self, _index:usize){
        // move object tree at index to frozen heapzone
    }
    
    pub fn unfreeze(&mut self, _index:usize){
       // self.mark_vec.clear();
        // unfreeze/unmanual an entire tree so it can be gc'ed again
    }
}
