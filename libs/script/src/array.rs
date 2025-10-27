
use crate::value::ScriptValue;

#[derive(Default)]
pub struct ArrayTag(u64); 

impl ArrayTag{
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
    _storage: ScriptArrayStorage
}

impl ScriptArrayData{
    
}