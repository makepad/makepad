
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

pub struct ObjectField{
    pub key: Value,
    pub value: Value,
}

pub struct Object{
    pub fields: SmallVec<[ObjectField; 4]>
}

impl Object{
    pub fn get(&self, key:Value)->Option<Value>{
        for ov in &self.fields{
            if ov.key == key{
                return Some(ov.value)
            }
        }
        None
    }
    
    pub fn set(&self, _key:Value, _value: Value){
    }
}
