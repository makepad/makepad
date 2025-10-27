
use crate::value::*;
use crate::object::*;

#[derive(Default)]
pub struct ScriptArrayTag(u64); 

impl ScriptArrayTag{
    pub const MARK:u64 = 0x1<<40;
    pub const ALLOCED:u64 = 0x2<<40;
    pub const DIRTY: u64 = 0x40<<40;
    pub const FROZEN: u64 = 0x100<<40;
    
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
    
    pub fn freeze(&mut self){
        self.0  |= Self::FROZEN
    }
    
    pub fn is_frozen(&self)->bool{
        self.0 & Self::FROZEN != 0
    }
    
    pub fn set_dirty(&mut self){
        self.0  |= Self::DIRTY
    }
            
    pub fn check_and_clear_dirty(&mut self)->bool{
        if self.0 & Self::DIRTY !=  0{
            self.0 &= !Self::DIRTY;
            true
        }
        else{
            false
        }
    }
}

#[derive(PartialEq)]
pub enum ScriptArrayStorage{
    ScriptValue(Vec<ScriptValue>),
    F32(Vec<f32>),
    U32(Vec<u32>),
    U16(Vec<u16>),
    U8(Vec<u8>),
}

impl ScriptArrayStorage{
    pub fn clear(&mut self){
        match self{
            Self::ScriptValue(v)=>v.clear(),
            Self::F32(v)=>v.clear(),
            Self::U32(v)=>v.clear(),
            Self::U16(v)=>v.clear(),
            Self::U8(v)=>v.clear(),
        }
    }
    pub fn len(&self)->usize{
        match self{
            Self::ScriptValue(v)=>v.len(),
            Self::F32(v)=>v.len(),
            Self::U32(v)=>v.len(),
            Self::U16(v)=>v.len(),
            Self::U8(v)=>v.len(),
        }
    }
    pub fn index(&self, index:usize)->Option<ScriptValue>{
        match self{
            Self::ScriptValue(v)=>if let Some(v) = v.get(index){(*v).into()} else {None},
            Self::F32(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
            Self::U32(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
            Self::U16(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
            Self::U8(v)=>if let Some(v) = v.get(index){Some((*v).into())} else {None},
        }
    }
    pub fn set_index(&mut self, index:usize, value:ScriptValue){
        match self{
            Self::ScriptValue(v)=>{if index>=v.len(){v.resize(index+1, NIL);}v[index] = value;},
            Self::F32(v)=>{if index>=v.len(){v.resize(index+1, 0.0);}v[index] = value.as_f64().unwrap_or(0.0) as f32;},
            Self::U32(v)=>{if index>=v.len(){v.resize(index+1, 0);}v[index] = value.as_f64().unwrap_or(0.0) as u32;},
            Self::U16(v)=>{if index>=v.len(){v.resize(index+1, 0);}v[index] = value.as_f64().unwrap_or(0.0) as u16;},
            Self::U8(v)=>{if index>=v.len(){v.resize(index+1, 0);}v[index] = value.as_f64().unwrap_or(0.0) as u8;},
        }
    }
    pub fn push(&mut self, value:ScriptValue){
        match self{
            Self::ScriptValue(v)=>v.push(value),
            Self::F32(v)=>v.push(value.as_f64().unwrap_or(0.0) as f32),
            Self::U32(v)=>v.push(value.as_f64().unwrap_or(0.0) as u32),
            Self::U16(v)=>v.push(value.as_f64().unwrap_or(0.0) as u16),
            Self::U8(v)=>v.push(value.as_f64().unwrap_or(0.0) as u8),
        }
    }
    pub fn push_vec(&mut self, vec:&[ScriptVecValue]){
        match self{
            Self::ScriptValue(v)=>for a in vec{v.push(a.value)},
            Self::F32(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as f32)},
            Self::U32(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as u32)},
            Self::U16(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as u16)},
            Self::U8(v)=>for a in vec{v.push(a.value.as_f64().unwrap_or(0.0) as u8)},
        }
    }
    pub fn pop(&mut self)->Option<ScriptValue>{
        match self{
            Self::ScriptValue(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::F32(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::U32(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::U16(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
            Self::U8(v)=>if let Some(v) = v.pop(){Some(v.into())}else{None},
        }
    }
    pub fn remove(&mut self, index:usize)->ScriptValue{
        match self{
            Self::ScriptValue(v)=>v.remove(index),
            Self::F32(v)=>v.remove(index).into(),
            Self::U32(v)=>v.remove(index).into(),
            Self::U16(v)=>v.remove(index).into(),
            Self::U8(v)=>v.remove(index).into(),
        }
    }
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
    pub fn clear(&mut self){
        self.storage.clear();
        self.tag.clear()
    }
    
    pub fn is_value_array(&self)->bool{
        if let ScriptArrayStorage::ScriptValue(_) = &self.storage{
            true
        }
        else{
            false
        }
    }
}