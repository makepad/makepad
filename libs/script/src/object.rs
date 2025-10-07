
use crate::value::*;
use crate::heap::*;
use std::collections::BTreeMap;

#[derive(Default, Clone, Copy)]
pub struct Field{
    pub key: Value,
    pub value: Value
}
    
#[derive(Default)]
pub struct Object{
    pub tag: ObjectTag,
    pub proto: Value,
    pub map: BTreeMap<Value, Value>,
    pub vec: Vec<Value>
}

impl Object{
    pub fn set_type(&mut self, ty_new:ObjectType){
        let ty_now = self.tag.get_type();
        // block flipping from raw data mode to gc'ed mode
        if !ty_now.is_gc() && ty_new.is_gc(){
            self.vec.clear();
        }
        if !ty_now.has_paired_vec() && ty_new.has_paired_vec(){
            if self.vec.len() & 1 != 0{
                self.vec.push(Value::NIL)
            }
        }
        self.tag.set_type_unchecked(ty_new)
    }
    //const DONT_RECYCLE_WHEN: usize = 1000;
    pub fn with_proto(proto:Value)->Self{
        Self{
            proto,
            ..Default::default()
        }
    }
    
    pub fn clear(&mut self){
        self.proto = Value::NIL;
        self.tag.clear();
        self.map.clear();
        self.vec.clear();
    }
}