#![allow(unused_variables)]
use crate::id::Id;
use crate::token::TokenId;
use crate::math::{Vec2, Vec3};

#[derive(Copy, Clone, Debug)]
pub struct LiveNode { // 3x u64
    pub token_id: TokenId,
    pub id: Id,
    pub value: LiveValue,
}

impl LiveValue {
    pub fn is_simple(&self) -> bool {
        match self {
            Self::Bool(_) => true,
            Self::Int(_) => true,
            Self::Float(_) => true,
            Self::Color(_) => true,
            Self::Vec2(_) => true,
            Self::Vec3(_) => true,
            Self::Id(_) => true,
            _ => false
        }
    }
    pub fn get_type_nr(&self)->usize{
        match self {
            Self::String {..}=>1,
            Self::Bool(_)=>2,
            Self::Int(_)=>3,
            Self::Float(_)=>4,
            Self::Color(_)=>5,
            Self::Vec2(_)=>6,
            Self::Vec3(_)=>7,
            Self::Id(_)=>8,
            Self::Call {..}=>9,
            Self::Array {..}=>10,
            Self::Object {..}=>11,
            Self::Fn {..}=>12,
            Self::VarDef {..}=>13,
            Self::Use{..} => 14,
            Self::Class {..}=>15,
        }
    }
    
    pub fn is_var_def(&self)->bool{
        match self{
            Self::VarDef{..}=>true,
            _=>false
        }
    }
}


#[derive(Clone, Copy, Debug)]
pub enum ShaderRef {
    DrawInput,
    DefaultGeometry
}

#[derive(Clone, Copy, Debug)]
pub enum LiveValue {
    String {
        string_start: u32,
        string_count: u32
    },
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    Id(Id),
    Call {
        target: Id,
        node_start: u32,
        node_count: u16
    },
    Array {
        node_start: u32,
        node_count: u32
    },
    Object {
        node_start: u32,
        node_count: u32
    },
    Fn {
        token_start: u32,
        token_count: u32,
        scope_start: u32,
        scope_count: u16
    },
    VarDef {
        token_start: u32,
        token_count: u32,
        scope_start: u32,
        scope_count: u16
    },
    Use{
        crate_module: Id,
    },
    Class {
        class: Id, // target class , we can reuse this Id on clone
        node_start: u32, // how about
        node_count: u16 //65535 class items is plenty keeps this structure at 24 bytes
    },
}



//so we start walking the base 'truth'
//and every reference we run into we need to look up
// then we need to make a list of 'overrides'
// then walk the original, checking against overrides.
// all the while writing a new document as output

