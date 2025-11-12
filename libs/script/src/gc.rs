use crate::value::*;
use crate::heap::*;
use crate::array::*;
use crate::object::*;
use crate::function::*;

use std::collections::hash_map::Entry;
use std::sync::Arc;

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
            $self.strings[ptr.index as usize].as_mut().unwrap().tag.set_mark();
        }
        else if let Some(ptr) = $val.as_array(){
            $self.mark_vec.push(ScriptGcMark::Array(ptr));
        }
        else if let Some(ptr) = $val.as_pod(){
            
            $self.pods[ptr.index as usize].tag.set_mark();
        }
        else if let Some(ptr) = $val.as_handle(){
            $self.handles[ptr.index as usize].as_mut().unwrap().tag.set_mark();
        }
    };
}

impl ScriptHeap{
            
    pub fn new_object_ref(&mut self, obj:ScriptObject)->ScriptObjectRef{
        let mut roots = self.root_objects.borrow_mut();
        match roots.entry(obj) {
            Entry::Occupied(mut occ) => {
                *occ.get_mut() += 1;
                ScriptObjectRef{
                    roots: self.root_objects.clone(),
                    obj: obj
                }
            }
            Entry::Vacant(vac) => {
                vac.insert(1);
                ScriptObjectRef{
                    roots: self.root_objects.clone(),
                    obj: obj
                }
            }
        }
    }
    
    pub fn new_array_ref(&mut self, array:ScriptArray)->ScriptArrayRef{
        let mut roots = self.root_arrays.borrow_mut();
        match roots.entry(array) {
            Entry::Occupied(mut occ) => {
                *occ.get_mut() += 1;
                ScriptArrayRef{
                    roots: self.root_arrays.clone(),
                    array: array
                }
            }
            Entry::Vacant(vac) => {
                vac.insert(1);
                ScriptArrayRef{
                    roots: self.root_arrays.clone(),
                    array: array
                }
            }
        }
    }
    
    pub fn new_fn_ref(&mut self, obj:ScriptObject)->ScriptFnRef{
        ScriptFnRef(self.new_object_ref(obj))
    }
        
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
        let roots = self.root_objects.borrow();
        for item in roots.keys(){
            self.mark_vec.push(ScriptGcMark::Object(*item));
        }
        drop(roots);
        let roots = self.root_arrays.borrow();
        for item in roots.keys(){
            self.mark_vec.push(ScriptGcMark::Array(*item));
        }
        drop(roots);
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
                if let Some(pod_ty) = obj.tag.as_pod_type(){
                    self.pod_types_free.push(pod_ty);
                }
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
            if let Some(str) = &mut self.strings[i]{
                if !str.tag.is_marked(){
                    if let Some((k,_)) = self.string_intern.remove_entry(&str.string){
                        self.strings[i] = None;
                        if let Some(s) = Arc::into_inner(k.0){
                            self.strings_reuse.push(s);
                        }
                        self.strings_free.push(ScriptString{index:i as _})
                    }
                }
                else {
                    str.tag.clear_mark();
                }
            }
        }
        for i in 1..self.handles.len(){
            if let Some(handle) = &mut self.handles[i]{
                if !handle.tag.is_marked(){
                    let handle = self.handles[i].take().unwrap();
                    handle.gc()
                }
                else{
                    handle.tag.clear_mark();
                }
            }
        }
        for i in 1..self.pods.len(){
            let pod = &mut self.pods[i];
            if !pod.tag.is_marked() && pod.tag.is_alloced(){
                pod.clear();
                self.pods_free.push(ScriptPod{index: i as _});
            }
            else{
                pod.tag.clear_mark();
            }
        }
    }
    
        
    pub fn free_object_if_unreffed(&mut self, ptr:ScriptObject){
        let obj = &mut self.objects[ptr.index as usize];
        if !obj.tag.is_reffed(){
            if let Some(pod_ty) = obj.tag.as_pod_type(){
                self.pod_types_free.push(pod_ty);
            }
            obj.clear();
            self.objects_free.push(ptr);
        }
    }
        
}