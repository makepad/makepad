
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
    pub tag: HeapTag,
    pub proto: Value,
    pub fields: SmallVec<[Field;2]>
}

impl Object{
    pub fn clear(&mut self){
        self.proto = Value::NIL;
        self.tag.clear();
        self.fields.clear();
    }
}