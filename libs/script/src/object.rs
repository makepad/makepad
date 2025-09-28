
use smallvec::*;
use crate::value::*;
use crate::heap::*;

#[derive(Default)]
pub struct Object{
    pub tag: HeapTag,
    pub fields: SmallVec<[Value;2]>
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
