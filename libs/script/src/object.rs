
use smallvec::*;
use crate::value::*;
//use std::collections::HashMap;
//use crate::id::Id;
use std::rc::Rc;
use std::cell::RefCell;

pub trait RustComponent{
}

pub struct ScriptCode{
    pub code: Vec<u8>
}

pub struct ScriptArenas{
    pub code: Vec<ScriptCode>,
    pub rust: Vec<RustComponentRef>,
    pub object: Vec<ScriptObject>,
}

pub struct RustComponentRef{
    pub component: Rc<RefCell<Option<Box<dyn RustComponent>>>>,
}

pub struct ObjectField{
    pub key: ScriptValue,
    pub value: ScriptValue,
}

pub struct ScriptObject{
    pub fields: SmallVec<[ObjectField; 4]>
}

impl ScriptObject{
    pub fn get(&self, key:ScriptValue)->Option<ScriptValue>{
        for ov in &self.fields{
            if ov.key == key{
                return Some(ov.value)
            }
        }
        None
    }
    
    pub fn set(&self, _key:ScriptValue, _value: ScriptValue){
    }
}
