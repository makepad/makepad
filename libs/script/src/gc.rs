use crate::value::*;
use crate::heap::*;
use crate::array::*;

#[derive(Copy,Clone)]
pub enum ScriptGcMark{
    Object(ScriptObject),
    Array(ScriptArray)
}

macro_rules! mark{
    ($self:ident, $val:expr)=>{
        if let Some(ptr) = $val.as_object(){
            $self.mark_vec.push(ScriptGcMark::Object(ptr));
        }
        else if let Some(ptr) = $val.as_string(){
            $self.strings[ptr.index as usize].tag.set_mark();
        }
        else if let Some(ptr) = $val.as_array(){
            $self.mark_vec.push(ScriptGcMark::Array(ptr));
        }
    };
}

impl ScriptHeap{
        
    pub fn mark_inner(&mut self, value:ScriptGcMark){
        match value{
            ScriptGcMark::Object(obj)=>{
                let object = &mut self.objects[obj.index as usize];
                if object.tag.is_marked() || !object.tag.is_alloced(){
                    return;
                }
                object.tag.set_mark();      
                object.map_iter(|key,value|{
                    mark!(self, key);
                    mark!(self, value);
                });
                let len = object.vec.len();
                for i in 0..len{
                    let object = &self.objects[obj.index as usize];
                    let kv = &object.vec[i];
                    mark!(self, kv.key);
                    mark!(self, kv.value);
                }
            }
            ScriptGcMark::Array(arr)=>{
                let tag = &self.arrays[arr.index as usize].tag;
                if tag.is_marked() || !tag.is_alloced(){
                    return
                }
                self.arrays[arr.index as usize].tag.set_mark();
                if let ScriptArrayStorage::ScriptValue(values) = &self.arrays[arr.index as usize].storage{
                    for v in values{
                        mark!(self, v);
                    }
                }
            }
        }
        
    }
                
    pub fn mark(&mut self, stack:&[ScriptValue]){
        self.mark_vec.clear();
        for i in 0..self.type_check.len(){
            if let Some(object) = &self.type_check[i].object{
                if let Some(object) = object.proto.as_object(){
                    self.mark_inner(ScriptGcMark::Object(object));
                }
            }                
        }
        for i in 0..self.roots.len(){
            self.mark_inner(self.mark_vec[i]);
        }
        for i in 0..stack.len(){
            let value = stack[i];
            mark!(self, value)
        }
        for i in 0..self.mark_vec.len(){
            self.mark_inner(self.mark_vec[i]);
        }
    }
                
    pub fn sweep(&mut self){
        for i in 1..self.objects.len(){
            let obj = &mut self.objects[i];
            if !obj.tag.is_marked() && obj.tag.is_alloced(){
                obj.clear();
                self.objects_free.push(ScriptObject{index: i as _});
            }
            else{
                obj.tag.clear_mark();
            }
        }
        for i in 1..self.arrays.len(){
            let array = &mut self.arrays[i];
            if !array.tag.is_marked() && array.tag.is_alloced(){
                array.clear();
                self.arrays_free.push(ScriptArray{index: i as _});
            }
            else{
                array.tag.clear_mark();
            }
        }
        // always leave the empty null string at 0
        for i in 1..self.strings.len(){
            let str = &mut self.strings[i];
            if !str.tag.is_marked() && str.tag.is_alloced(){
                if let Some((mut k,_)) = self.string_intern.remove_entry(&str.string){
                    k.clear();
                    self.string_intern_free.push(k);
                }
                str.clear();
                self.strings_free.push(ScriptString{index:i as _})
            }
            else {
                str.tag.clear_mark();
            }
        }
    }
    
        
    pub fn free_object_if_unreffed(&mut self, ptr:ScriptObject){
        let obj = &mut self.objects[ptr.index as usize];
        if !obj.tag.is_reffed(){
            obj.clear();
            self.objects_free.push(ptr);
        }
    }
        
}