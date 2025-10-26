use crate::value::*;
use crate::heap::*;

impl ScriptHeap{
        
    pub fn mark_inner(&mut self, index:usize){
        let obj = &mut self.objects[index];
        if obj.tag.is_marked() || !obj.tag.is_alloced(){
            return;
        }
        obj.tag.set_mark();
        obj.map_iter(|key,value|{
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
        });
        let len = obj.vec.len();
        for i in 0..len{
            let object = &self.objects[index];
            if object.tag.get_storage_type().is_gc(){
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
                
    pub fn mark(&mut self, stack:&[ScriptValue]){
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
                if let Some((mut k,_)) = self.string_intern.remove_entry(&str.string){
                    k.clear();
                    self.string_intern_free.push(k);
                }
                str.clear();
                self.strings_free.push(i)
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
            self.objects_free.push(ptr.index as usize);
        }
    }
        
}