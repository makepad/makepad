#![allow(unused)]
use makepad_live_id::*;
use crate::value::*;
use crate::heap::*;
use crate::value::*;
use crate::trap::*;
use crate::mod_pod::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPodField{
    pub name: LiveId,
    pub default: ScriptValue,
    pub ty: ScriptPodTypeInline,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ScriptPodEnum{
    pub name: LiveId,
    pub variant: ScriptPodEnumVariant
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScriptPodEnumVariant{
    Bare,
    Tuple{
        items: Vec<ScriptPodTypeInline>,
    },
    Named{
        fields: Vec<ScriptPodField>
    }
}

// we're going to try to follow std140 datamapping for wgsl
#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ScriptPodTypeData{
    pub default: ScriptValue,
    pub ty: ScriptPodTy
}

#[derive(Default, Clone, Debug, PartialEq, Eq)]
pub struct ScriptPodTypeInline{
    pub self_ref: ScriptPodType,
    pub data: ScriptPodTypeData
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub enum ScriptPodTy{
    #[default]
    NIL,
    // limited to the types WGSL supports
    Bool,
    AtomicU32,
    AtomicI32,
    U32,
    I32,
    F32,
    F16,
    Struct{
        fields:Vec<ScriptPodField>
    },
    Enum{
        variants:Vec<ScriptPodEnum>
    },
    UndefinedArray,
    UndefinedStruct,
    FixedArray{
        len: usize,
        ty: Box<ScriptPodTypeInline>,
    },
    VariableArray{
        ty: Box<ScriptPodTypeInline>,
    }
}

#[derive(Default, Debug)]
pub struct ScriptPodTag(u64);

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