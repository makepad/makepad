
use crate::value::ScriptValue;

#[derive(Default)]
pub struct ScriptArrayTag(u64); 

impl ScriptArrayTag{
    pub const MARK:u64 = 0x1<<40;
    pub const ALLOCED:u64 = 0x2<<40;
    
    pub fn is_alloced(&self)->bool{
        return self.0 & Self::ALLOCED != 0
    }
    
    pub fn set_alloced(&mut self){
        self.0 |= Self::ALLOCED
    }
    
    pub fn clear(&mut self){
        self.0 = 0;
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

pub enum ScriptArrayStorage{
    ScriptValue(Vec<ScriptValue>),
    F64(Vec<f64>),
    U32(Vec<u32>),
    I32(Vec<i32>),
    U16(Vec<u16>),
    I16(Vec<i16>),
    U8(Vec<u8>),
    I8(Vec<i8>),
}

pub struct ScriptArrayData{
    pub tag: ScriptArrayTag,
    pub storage: ScriptArrayStorage
}

impl Default for ScriptArrayData{
    fn default()->Self{
        Self{
            tag: ScriptArrayTag::default(),
            storage: ScriptArrayStorage::ScriptValue(vec![])
        }
    }
}

impl ScriptArrayData{
    pub fn is_value_array(&self)->bool{
        if let ScriptArrayStorage::ScriptValue(_) = &self.storage{
            true
        }
        else{
            false
        }
    }
}