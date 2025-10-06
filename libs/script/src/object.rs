
use crate::value::*;
use crate::heap::*;
use std::collections::BTreeMap;

#[derive(Default)]
pub struct Field{
    pub key: Value,
    pub value: Value
}
    
#[derive(Default)]
pub struct Object{
    pub tag: ObjectTag,
    pub proto: Value,
    pub map: BTreeMap<Value, Value>,
    pub vec: Vec<Field>
}

impl Object{
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
      //  if self.map.len()>Self::DONT_RECYCLE_WHEN{
      //      let mut map = Default::default();
      //      std::mem::swap(&mut self.map, &mut map);
      //  }
      //  else{
            self.map.clear();
      //  }
      //  if self.vec.len()>Self::DONT_RECYCLE_WHEN{
      //      let mut map = Default::default();
      //      std::mem::swap(&mut self.map, &mut map);
      //  }
      //  else{
            self.vec.clear();
      //  }
    }
}