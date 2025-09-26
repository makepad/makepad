
use smallvec::*;
use crate::value::*;
//use std::collections::HashMap;
//use crate::id::Id;
use std::rc::Rc;
use std::cell::RefCell;

pub trait RustComponent{
}
 
pub struct RustComponentRef{
    pub component: Rc<RefCell<Option<Box<dyn RustComponent>>>>,
}

pub struct Tag(u32);

impl Tag{
    const FREE:u32 = 0x1;
    const ARRAY:u32 = 0x2;
    //const FRAG:u32 = 0x4;
    //const SINGLE:u32 = 0x10;
    const MARK:u32 = 0x20;
    
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

pub struct Object{
    tag: Tag,
    pub fields: SmallVec<[Value;2]>
}

pub struct HeapString{
    pub tag: Tag,
    pub string: String
}

pub struct Heap{
    mark_vec: Vec<usize>,
    objects: Vec<Object>,
    roots: Vec<usize>,
    objects_free: Vec<usize>,
    strings: Vec<HeapString>,
    strings_free: Vec<usize>
}

impl Heap{
    pub fn mark_inner(&mut self, index:usize){
        let obj = &mut self.objects[index];
        if obj.tag.is_marked() || obj.tag.is_free(){
            return;
        }
        obj.tag.set_mark();
        let len = obj.fields.len();
        for i in 0..len{
            let value = self.objects[index].fields[i];
            if let Some(index) = value.as_object(){
                self.mark_vec.push(index);
            }
            else if let Some(index) = value.as_heap_string(){
                self.strings[index].tag.set_mark();
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
            if let Some(index) = value.as_object(){
                self.mark_vec.push(index);
            }
            else if let Some(index) = value.as_heap_string(){
                self.strings[index].tag.set_mark();
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
        for i in 0..self.strings.len(){
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

impl Object{
    
    pub fn get(&self, key:Value)->Option<Value>{
        if self.tag.is_array(){
            // treat key as array index
        }
        else{
            for i in (0..self.fields.len()).step_by(2){
                if self.fields[i] == key{
                    return Some(self.fields[i+1])
                }
            }
        }
        None
    }
    
    pub fn set(&self, _key:Value, _value: Value){
        // if we are arraylke and we are setting a key we switch to object-like
    }
}
