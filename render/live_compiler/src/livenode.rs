#![allow(unused_variables)]
use crate::id::Id;
use crate::token::TokenId;
use crate::math::{Vec2, Vec3};

#[derive(Clone, Debug, Eq, PartialEq, Copy, Hash)]
pub struct LiveType(pub core::any::TypeId);

#[derive(Clone, Debug)]
pub struct LiveNode { // 3x u64
    pub token_id: Option<TokenId>,
    pub id: Id,
    pub value: LiveValue,
}

impl LiveValue {
    pub fn is_tree(&self) -> bool {
        match self {
            Self::Array |
            Self::TupleEnum {..} |
            Self::NamedEnum {..} |
            Self::BareClass | // subnodes including this one
            Self::NamedClass {..} => true, // subnodes including this one          
            _ => false
        }
    }
    
    pub fn is_close(&self) -> bool {
        match self {
            Self::Close => true,
            _ => false
        }
    }
    
    pub fn variant_id(&self) -> usize {
        match &self {
            Self::Str(_) => 1,
            Self::String(_) => 2,
            Self::StringRef {..} => 3,
            Self::Bool(_) => 4,
            Self::Int(_) => 5,
            Self::Float(_) => 6,
            Self::Color(_) => 7,
            Self::Vec2(_) => 8,
            Self::Vec3(_) => 9,
            Self::LiveType(_) => 10,
            Self::BareEnum {..} => 11,
            Self::Array => 12,
            Self::TupleEnum {..} => 13,
            Self::NamedEnum {..} => 14,
            Self::BareClass => 15,
            Self::NamedClass {..} => 16,
            Self::Close => 17,
            Self::Fn {..} => 18,
            Self::Const {..} => 19,
            Self::VarDef {..} => 20,
            Self::Use {..} => 21
        }
    }
}

#[derive(Clone, Debug)]
pub enum LiveValue {
    Str(&'static str),
    String(String),
    StringRef {
        string_start: usize,
        string_count: usize
    },
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    LiveType(LiveType),
    
    BareEnum {base: Id, variant: Id},
    // stack items
    Array,
    TupleEnum {base: Id, variant: Id},
    NamedEnum {base: Id, variant: Id},
    BareClass, // subnodes including this one
    NamedClass {class: Id}, // subnodes including this one
    Close,
    // the shader code types
    Fn {
        token_start: usize,
        token_count: usize,
        scope_start: usize,
        scope_count: u32
    },
    Const {
        token_start: usize,
        token_count: usize,
        scope_start: usize,
        scope_count: u32
    },
    VarDef { //instance/uniform def
        token_start: usize,
        token_count: usize,
        scope_start: usize,
        scope_count: u32
    },
    Use {
        crate_id: Id,
        module_id: Id,
        object_id: Id,
    }
}

pub trait LiveNodeSlice {
    fn seek_parent(&self, index: usize) -> Option<usize>;
    fn seek_child_append(&self, index: usize) -> usize;
    fn seek_last_child(&self, index: usize) -> Option<usize>;
    fn seek_child_by_index(&self, index: usize, child_index: usize) -> Option<usize>;
    fn seek_child_by_name(&self, index: usize, name: Id) -> Result<usize, usize>;
    fn count_children(&self, index: usize) -> usize;
    fn skip_value(&self, index: usize) -> usize;
}

pub trait LiveNodeVec {
    fn clone_children_from(&mut self, from_index: usize, insert_start: Option<usize>, other: &[LiveNode]) -> usize;
    fn clone_children_self(&mut self, from_index: usize, insert_start: Option<usize>) -> usize;
}

//macro_rules!impl_live_node_slice {
//    ( $ for_type: ty) => {
// accessing the Gen structure like a tree
impl LiveNodeSlice for Vec<LiveNode> {
    fn seek_parent(&self, index: usize) -> Option<usize> {
        if self.len() == 0 {
            return None
        }
        let mut stack_depth = 0;
        let mut index = index;
        // we are going to scan backwards
        loop {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    if stack_depth == 0 {
                        return Some(index)
                    }
                    stack_depth -= 1;
                }
                LiveValue::Close => {
                    stack_depth += 1;
                }
                _ => {}
            }
            if index == 0 {
                break
            }
            index -= 1;
        }
        Some(0)
    }
    
    fn seek_child_by_index(&self, index: usize, child_index: usize) -> Option<usize> {
        let mut stack_depth = 0;
        let mut index = index;
        let mut child_count = 0;
        while index < self.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    if stack_depth == 1 {
                        if child_index == child_count {
                            return Some(index);
                        }
                        child_count += 1;
                    }
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        return None
                    }
                }
                _ => {
                    if stack_depth == 1 {
                        if child_index == child_count {
                            return Some(index);
                        }
                        child_count += 1;
                    }
                    else if stack_depth == 0 {
                        return None
                    }
                }
            }
            index += 1;
        }
        None
    }
    
    fn seek_last_child(&self, index: usize) -> Option<usize> {
        let mut stack_depth = 0;
        let mut index = index;
        let mut child_count = 0;
        let mut found_child = None;
        while index < self.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    if stack_depth == 1 {
                        found_child = Some(index);
                        child_count += 1;
                    }
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        return found_child
                    }
                }
                _ => {
                    if stack_depth == 1 {
                        found_child = Some(index);
                        child_count += 1;
                    }
                    else if stack_depth == 0 {
                        return found_child
                    }
                }
            }
            index += 1;
        }
        None
    }
    
    fn seek_child_append(&self, index: usize) -> usize {
        let mut stack_depth = 0;
        let mut index = index;
        while index < self.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        return index
                    }
                }
                _ => {}
            }
            index += 1;
        }
        0
    }
    
    fn seek_child_by_name(&self, index: usize, child_name: Id) -> Result<usize, usize> {
        
        let mut stack_depth = 0;
        let mut index = index;
        while index < self.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    if stack_depth == 1 {
                        if self[index].id == child_name {
                            return Ok(index);
                        }
                    }
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        return Err(index)
                    }
                }
                _ => {
                    if stack_depth == 1 {
                        if self[index].id == child_name {
                            return Ok(index);
                        }
                    }
                    else if stack_depth == 0 {
                        return Err(index)
                    }
                }
            }
            index += 1;
        }
        Err(index)
    }
    
    fn count_children(&self, index: usize) -> usize {
        
        let mut stack_depth = 0;
        let mut index = index;
        let mut count = 0;
        while index < self.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    if stack_depth == 1 {
                        count += 1;
                    }
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        return count
                    }
                }
                _ => {
                    if stack_depth == 1 {
                        count += 1;
                    }
                    else if stack_depth == 0 {
                        return count
                    }
                }
            }
            index += 1;
        }
        0
    }
    
    fn skip_value(&self, index: usize) -> usize {
        let mut index = index;
        let mut stack_depth = 0;
        while index < self.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        index += 1;
                        return index;
                    }
                }
                _ => {
                    if stack_depth == 0 {
                        index += 1;
                        return index
                    }
                }
            }
            index += 1;
        }
        index
    }
}

impl LiveNodeVec for Vec<LiveNode> {
    fn clone_children_from(&mut self, index: usize, insert_start: Option<usize>, other: &[LiveNode]) -> usize {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            other.len()
        };
        while index < other.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    if stack_depth >= 1 {
                        self.insert(insert_point, other[index].clone());
                        insert_point += 1;
                    }
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth >= 1 {
                        self.insert(insert_point, other[index].clone());
                        insert_point += 1;
                    }
                    if stack_depth == 0 {
                        return insert_point
                    }
                }
                _ => {
                    self.insert(insert_point, other[index].clone());
                    insert_point += 1;
                    if stack_depth == 0 {
                        return insert_point
                    }
                }
            }
            index += 1;
        }
        0
    }
    
    fn clone_children_self(&mut self, index: usize, insert_start: Option<usize>) -> usize {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            self.len()
        };
        while index < self.len() {
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::BareClass | LiveValue::NamedClass {..} | LiveValue::Array => {
                    if stack_depth >= 1 {
                        self.insert(insert_point, self[index].clone());
                        insert_point += 1;
                    }
                    stack_depth += 1;
                }
                LiveValue::Close => {
                    stack_depth -= 1;
                    if stack_depth >= 1 {
                        self.insert(insert_point, self[index].clone());
                        insert_point += 1;
                    }
                    if stack_depth == 0 {
                        return insert_point
                    }
                }
                _ => {
                    self.insert(insert_point, self[index].clone());
                    insert_point += 1;
                    if stack_depth == 0 {
                        return insert_point
                    }
                }
            }
            index += 1;
        }
        0
    }
}
//    }
//}
//impl_live_node_slice!(&[LiveNode]);
//impl_live_node_slice!(Vec<LiveNode>);


/*
#[derive(Clone, Copy, Debug)]
pub enum ShaderRef {
    DrawInput,
    DefaultGeometry
}
*/
/*
#[derive(Clone, Debug)]
pub enum LiveValue {
    Str(&'static str),
    String(String),
    StringRef {
        string_start: u32,
        string_count: u32
    },
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    LiveType(LiveType),
    MultiPack(MultiPack),
    // ok so since these things are 
    EnumBare {base: Id, variant: Id},
    // stack items
    Array,
    EnumTuple {base: Id, variant: Id},
    EnumNamed {base: Id, variant: Id},
    ClassBare, // subnodes including this one
    ClassNamed {class: Id}, // subnodes including this one
    
    Close,
    // the shader code types
    Fn {
        token_start: u32,
        token_count: u32,
    },
    Const {
        token_start: u32,
        token_count: u32,
    },
    VarDef { //instance/uniform def
        token_start: u32,
        token_count: u32,
    },
    Use{
        crate_id:Id
        module_id:Id,
        object_id:Id
    }
}*/
// you can reconstitute a live pointer from the token_id + index


//so we start walking the base 'truth'
//and every reference we run into we need to look up
// then we need to make a list of 'overrides'
// then walk the original, checking against overrides.
// all the while writing a new document as output

