#![allow(unused)]
use makepad_live_id::*;
use crate::value::*;

pub struct ScriptPodField{
    pub name: LiveId,
    pub ty: ScriptPodType,
}

pub struct ScriptPodEnum{
    pub name: LiveId,
    pub variant: ScriptPodEnumVariant
}

pub enum ScriptPodEnumVariant{
    Bare,
    Tuple{
        items: Vec<ScriptPodType>,
    },
    Named{
        fields: Vec<ScriptPodField>
    }
}

// we're going to try to follow std140 datamapping for wgsl
#[derive(Default)]
pub struct ScriptPodTypeData{
    pub tag: ScriptPodTypeTag,
    pub default: ScriptValue,
    pub ty: ScriptPodTy
}

impl ScriptPodTypeData{
    pub fn clear(&mut self){
        self.tag.clear();
    }
}

#[derive(Default)]
pub enum ScriptPodTy{
    #[default]
    VOID,
    U8,
    U32,
    I32,
    F32,
    Struct{
        fields:Vec<ScriptPodField>
    },
    Enum{
        variants:Vec<ScriptPodEnum>
    },
    Array{
        len: usize,
        ty: Box<ScriptPodTy>,
    }
}

#[derive(Default)]
pub struct ScriptPodTypeTag(u64);

#[derive(Default)]
pub struct ScriptPodTag(u64);

impl ScriptPodTypeTag{
    const MARK:u64 = 0x1;
    const ALLOCED:u64 = 0x2;
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
                    
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
                    
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
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

impl ScriptPodTag{
    const MARK:u64 = 0x1;
    const ALLOCED:u64 = 0x2;
    pub fn is_marked(&self)->bool{
        self.0 & Self::MARK != 0
    }
                
    pub fn set_mark(&mut self){
        self.0 |= Self::MARK
    }
                
    pub fn clear_mark(&mut self){
        self.0 &= !Self::MARK
    }
    
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

#[derive(Default)]
pub struct ScriptPodData{
    pub tag: ScriptPodTag,
    pub ty: ScriptPodType,
    pub data: Vec<u64>
}

impl ScriptPodData{
    pub fn clear(&mut self){
        self.tag.clear();
        self.data.clear();
    }
}