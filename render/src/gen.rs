#![allow(unused_variables)]
use crate::cx::*;
use makepad_live_parser::LiveValue;

#[derive(Debug, Clone)]
pub struct GenNode {
    pub id: Id,
    pub value: GenValue
}

#[derive(Debug, Clone)]
pub enum GenValue {
    None,
    Str(&'static str),
    String(String),
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    Id(Id),
    EnumBare {base: Id, variant: Id},
    // stack items
    Array,
    EnumTuple {base: Id, variant: Id},
    EnumNamed {base: Id, variant: Id},
    ClassBare, // subnodes including this one
    ClassNamed {class: Id}, // subnodes including this one
    
    Close // closes call/class
}

impl GenValue{
    pub fn is_tree(&self)->bool{
        match self{
            Self::Array | 
            Self::EnumTuple {..}| 
            Self::EnumNamed {..}| 
            Self::ClassBare| 
            Self::ClassNamed {..}| 
            Self::Close  =>true,
            _=>false
        }
    }
}

pub trait GenNodeSlice{
    fn seek_last_child(&self, index:usize)->Option<usize>;
    fn seek_child_by_index(&self, index:usize, child_index:usize)->Option<usize>;
    fn seek_child_by_name(&self, index:usize, name:Id)->Option<usize>;
    fn count_children(&self, index: usize) -> usize;
    fn skip_value(&self, index: &mut usize);
    fn clone_value(&self, index: usize, output: &mut Vec<GenNode>) -> usize;
}

// accessing the Gen structure like a tree
impl GenNodeSlice for &[GenNode]{
    fn seek_child_by_index(&self, index: usize, child_index: usize) -> Option<usize> {
        let mut stack_depth = 0;
        let mut index = index;
        let mut child_count = 0;
        loop {
            match &self[index].value {
                GenValue::EnumTuple {..} | GenValue::EnumNamed {..} | GenValue::ClassBare | GenValue::ClassNamed {..} | GenValue::Array => {
                    if stack_depth == 1 {
                        if child_index == child_count {
                            return Some(index);
                        }
                        child_count += 1;
                    }
                    stack_depth += 1;
                }
                GenValue::Close => {
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
    }
    
    fn seek_last_child(&self, index: usize) -> Option<usize> {
        let mut stack_depth = 0;
        let mut index = index;
        let mut child_count = 0;
        let mut found_child = None;
        loop {
            match &self[index].value {
                GenValue::EnumTuple {..} | GenValue::EnumNamed {..} | GenValue::ClassBare | GenValue::ClassNamed {..} | GenValue::Array => {
                    if stack_depth == 1 {
                        found_child = Some(index);
                        child_count += 1;
                    }
                    stack_depth += 1;
                }
                GenValue::Close => {
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
    }
    
    fn seek_child_by_name(&self, index: usize, child_name:Id) -> Option<usize> {
        let mut stack_depth = 0;
        let mut index = index;
        loop {
            match &self[index].value {
                GenValue::EnumTuple {..} | GenValue::EnumNamed {..} | GenValue::ClassBare | GenValue::ClassNamed {..} | GenValue::Array => {
                    if stack_depth == 1 {
                        if self[index].id == child_name{
                            return Some(index);
                        }
                    }
                    stack_depth += 1;
                }
                GenValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        return None
                    }
                }
                _ => {
                    if stack_depth == 1 {
                        if self[index].id == child_name{
                            return Some(index);
                        }
                    }
                    else if stack_depth == 0 {
                        return None
                    }
                }
            }
            index += 1;
        }
    }
    
    fn count_children(&self, index: usize) -> usize {
        let mut stack_depth = 0;
        let mut index = index;
        let mut count = 0;
        loop {
            match &self[index].value {
                GenValue::EnumTuple {..} | GenValue::EnumNamed {..} | GenValue::ClassBare | GenValue::ClassNamed {..} | GenValue::Array => {
                    if stack_depth == 1 {
                        count += 1;
                    }
                    stack_depth += 1;
                }
                GenValue::Close => {
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
    }
    
    fn skip_value(&self, index: &mut usize) {
        let mut stack_depth = 0;
        loop {
            match &self[*index].value {
                GenValue::EnumTuple {..} | GenValue::EnumNamed {..} | GenValue::ClassBare | GenValue::ClassNamed {..} | GenValue::Array => {
                    stack_depth += 1;
                }
                GenValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        *index += 1;
                        return
                    }
                }
                _ => {
                    if stack_depth == 0 {
                        *index += 1;
                        return
                    }
                }
            }
            *index += 1;
        }
    }
    
    fn clone_value(&self, index: usize, output: &mut Vec<GenNode>) -> usize {
        let mut stack_depth = 0;
        let mut index = index;
        let pos = output.len();
        loop {
            output.push(self[index].clone());
            match &self[index].value {
                GenValue::EnumTuple {..} | GenValue::EnumNamed {..} | GenValue::ClassBare | GenValue::ClassNamed {..} | GenValue::Array => {
                    stack_depth += 1;
                }
                GenValue::Close => {
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        return pos
                    }
                }
                _ => {
                    if stack_depth == 0 {
                        return pos
                    }
                }
            }
            index += 1;
        }
    }
}

impl GenValue {
    pub fn is_close(&self) -> bool {
        if let Self::Close = self {
            true
        }
        else {
            false
        }
    }
}

impl GenNode {
    
    pub fn convert_live_to_gen(cx: &Cx, live_ptr: LivePtr, out: &mut Vec<GenNode>) {
        // OK! SO now what.
        let node = cx.resolve_ptr(live_ptr);
        match &node.value {
            LiveValue::String {string_start, string_count} => {
                let mut s = String::new();
                let origin_doc = cx.shader_registry.live_registry.get_origin_doc_from_token_id(node.token_id);
                origin_doc.get_string(*string_start, *string_count, &mut s);
                out.push(GenNode {id: node.id, value: GenValue::String(s)});
            },
            LiveValue::Bool(val) => {
                out.push(GenNode {id: node.id, value: GenValue::Bool(*val)});
            },
            LiveValue::Int(val) => {
                out.push(GenNode {id: node.id, value: GenValue::Int(*val)});
            },
            LiveValue::Float(val) => {
                out.push(GenNode {id: node.id, value: GenValue::Float(*val)});
            },
            LiveValue::Color(val) => {
                out.push(GenNode {id: node.id, value: GenValue::Color(*val)});
            },
            LiveValue::Vec2(val) => {
                out.push(GenNode {id: node.id, value: GenValue::Vec2(*val)});
            },
            LiveValue::Vec3(val) => {
                out.push(GenNode {id: node.id, value: GenValue::Vec3(*val)});
            },
            LiveValue::MultiPack(val) => {
                // ok this could be an enum.
                if let Some((base, variant)) = cx.find_enum_origin(*val, node.id) {
                    out.push(GenNode {id: node.id, value: GenValue::EnumBare {base, variant}});
                }
                else{
                    out.push(GenNode {id: node.id, value: GenValue::Id(val.as_single_id())});
                }
            },
            LiveValue::Call {target, node_start, node_count} => {
                if let Some((base, variant)) = cx.find_enum_origin(*target, node.id) {
                    // we are an enum
                    out.push(GenNode {id: node.id, value: GenValue::EnumTuple {base, variant}});
                    let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start as usize, *node_count as usize);
                    while let Some((id, live_ptr)) = iter.next_id(&cx.shader_registry.live_registry) {
                        Self::convert_live_to_gen(cx, live_ptr, out);
                    }
                    out.push(GenNode {id: node.id, value: GenValue::Close});
                }
                else { // unknown. cant convert
                    todo!();
                }
            },
            LiveValue::Array {node_start, node_count} => {
                out.push(GenNode {id: node.id, value: GenValue::Array});
                let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start as usize, *node_count as usize);
                while let Some((id, live_ptr)) = iter.next_id(&cx.shader_registry.live_registry) {
                    Self::convert_live_to_gen(cx, live_ptr, out);
                }
                out.push(GenNode {id: node.id, value: GenValue::Close});
            },
            LiveValue::ClassOverride {node_start, node_count} => {
                // should never have this thing
                panic!();
            },
            LiveValue::Class {class, node_start, node_count} => {
                if let Some((base, variant)) = cx.find_enum_origin(*class, node.id) {
                    
                    out.push(GenNode {id: node.id, value: GenValue::EnumNamed {base, variant}});
                    let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start as usize, *node_count as usize);
                    while let Some((id, live_ptr)) = iter.next_id(&cx.shader_registry.live_registry) {
                        Self::convert_live_to_gen(cx, live_ptr, out);
                    }
                    out.push(GenNode {id: node.id, value: GenValue::Close});
                }
                else { // class thing
                    if class.is_zero_class() {
                        out.push(GenNode {id: node.id, value: GenValue::ClassBare});
                    }
                    else { // so whats this class name gonna be.... single step target name?
                        todo!();
                    }
                    let mut iter = cx.shader_registry.live_registry.live_object_iterator(live_ptr, *node_start as usize, *node_count as usize);
                    while let Some((id, live_ptr)) = iter.next_id(&cx.shader_registry.live_registry) {
                        Self::convert_live_to_gen(cx, live_ptr, out);
                    }
                    out.push(GenNode {id: node.id, value: GenValue::Close});
                }
            },
            _ => ()
        }
    }
}