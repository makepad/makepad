#![allow(unused_variables)]
use crate::id::Id;
use crate::token::TokenId;
use crate::math::{Vec2, Vec3};

#[derive(Copy, Clone)]
pub struct LiveNode { // 3x u64
    pub token_id: TokenId,
    pub id: Id,
    pub value: LiveValue,
}

impl LiveValue {
    pub fn is_simple(&self) -> bool {
        match self {
            LiveValue::Bool(_) => true,
            LiveValue::Int(_) => true,
            LiveValue::Float(_) => true,
            LiveValue::Color(_) => true,
            LiveValue::Vec2(_) => true,
            LiveValue::Vec3(_) => true,
            LiveValue::Id(_) => true,
            _ => false
        }
    }
}

#[derive(Clone, Copy)]
pub enum LiveValue {
    String {
        string_index: u32,
        string_len: u32
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
    },
    Use{
        crate_module: Id,
    },
    Class {
        class: Id,
        node_start: u32, // how about
        node_count: u16 //65535 class items is plenty keeps this structure at 24 bytes
    },
}



//so we start walking the base 'truth'
//and every reference we run into we need to look up
// then we need to make a list of 'overrides'
// then walk the original, checking against overrides.
// all the while writing a new document as output

