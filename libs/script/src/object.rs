
use smallvec::*;
use crate::value::*;
use crate::heap::*;

#[derive(Default)]
pub struct Field{
    pub key: Value,
    pub value: Value
}
    
#[derive(Default)]
pub struct Object{
    pub tag: ObjectTag,
    pub proto: Value,
    pub fields: Vec<Field>
}

impl Object{
    pub fn with_proto(proto:Value)->Self{
        Self{
            proto,
            ..Default::default()
        }
    }
    
    pub fn clear(&mut self){
        self.proto = Value::NIL;
        self.tag.clear();
        self.fields.clear();
    }
}