#![allow(unused_variables)]
use crate::id::Id;
use crate::id::ModulePath;
use crate::token::TokenId;
use crate::math::{Vec2, Vec3, Vec4};
use std::fmt::Write;

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Copy, Hash)]
pub struct LiveType(pub core::any::TypeId);

#[derive(Clone)]
pub struct LiveTypeInfo{
    pub live_type:LiveType,
    pub type_name:Id,
    pub module_path:ModulePath,
    pub fields:Vec<LiveTypeField>
}

#[derive(Clone)]
pub struct LiveTypeField {
    pub id: Id,
    pub live_type_info: LiveTypeInfo,
    pub live_or_calc: LiveOrCalc
}

#[derive(Copy, Clone)]
pub enum LiveOrCalc {
    Calc,
    Live,
}

#[derive(Clone, Debug)]
pub struct LiveNode { // 3x u64
    pub token_id: Option<TokenId>,
    pub id: Id,
    pub value: LiveValue,
}


#[derive(Clone, Debug)]
pub enum LiveValue {
    None,
    Str(&'static str),
    //String(String),
    DocumentString {
        string_start: usize,
        string_count: usize
    },
    FittedString(FittedString),
    InlineString(InlineString),
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),

    Id(Id),

    BareEnum {base: Id, variant: Id},
    // stack items
    Array,
    TupleEnum {base: Id, variant: Id},
    NamedEnum {base: Id, variant: Id},
    Object, // subnodes including this one
    Clone(Id), // subnodes including this one
    Class(LiveType), // subnodes including this one
    Close,
    // the shader code types
    DSL {
        token_start: u32,
        token_count: u32,
        scope_start: u32,
        scope_count: u32
    },
    Use {
        crate_id: Id,
        module_id: Id,
    }
}

#[derive(Debug)]
pub struct FittedString{
    buffer: *mut u8,
    length: usize,
}

impl FittedString{
    pub fn from_string(mut inp:String)->Self{
        inp.shrink_to_fit();
        let mut s =  std::mem::ManuallyDrop::new(inp);
        let buffer = s.as_mut_ptr();
        let length = s.len();
        let capacity = s.capacity();
        if length != capacity{
            panic!()
        }
        FittedString{buffer, length}
    }
    
    pub fn to_string(self)->String{
        unsafe{String::from_raw_parts(self.buffer, self.length, self.length)}
    }
    
    pub fn as_str<'a>(&'a self)->&'a str{
        unsafe{std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.buffer, self.length))}
    }
}

impl Drop for FittedString{
    fn drop(&mut self){
        unsafe{String::from_raw_parts(self.buffer, self.length, self.length)};
    }
}

impl Clone for FittedString{
    fn clone(&self)->Self{
        Self::from_string(self.as_str().to_string())
    }
}

const INLINE_STRING_BUFFER_SIZE:usize = 22;
#[derive(Clone, Debug)]
pub struct InlineString{
    length:u8,
    buffer:[u8;INLINE_STRING_BUFFER_SIZE]
}

impl InlineString{
    pub fn from_str(inp:&str)->Option<Self>{
        let bytes = inp.as_bytes();
        if bytes.len()<INLINE_STRING_BUFFER_SIZE{
            let mut buffer= [0u8;INLINE_STRING_BUFFER_SIZE];
            for i in 0..bytes.len(){
                buffer[i] = bytes[i];
            }
            return Some(Self{length:bytes.len() as u8, buffer})
        }
        None
    }
    
    pub fn as_str<'a>(&'a self)->&'a str{
        unsafe{std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.buffer.as_ptr(), self.length as usize))}
    }
}

impl LiveValue {
    pub fn is_open(&self) -> bool {
        match self {
            Self::Array |
            Self::TupleEnum {..} |
            Self::NamedEnum {..} |
            Self::Object | // subnodes including this one
            Self::Clone {..} | // subnodes including this one
            Self::Class {..} => true, // subnodes including this one        
            _ => false
        }
    }
    
    pub fn is_close(&self) -> bool {
        match self {
            Self::Close => true,
            _ => false
        }
    }
    
    pub fn is_enum(&self) -> bool {
        match self {
            Self::BareEnum {..} |
            Self::TupleEnum {..} |
            Self::NamedEnum {..} => true,
            _ => false
        }
    }
    
    pub fn is_array(&self) -> bool {
        match self {
            Self::Array => true,
            _ => false
        }
    }
    /*
    pub fn is_class(&self) -> bool {
        match self {
            Self::BareClass |
            Self::NamedClass {..} => true,
            _ => false
        }
    }*/
    
    pub fn is_class(&self) -> bool {
        match self {
            Self::Class {..} => true,
            _ => false
        }
    }

    pub fn is_clone(&self) -> bool {
        match self {
            Self::Clone {..} => true,
            _ => false
        }
    }
    
    pub fn is_object(&self) -> bool {
        match self {
            Self::Object => true,
            _ => false
        }
    }

    pub fn is_dsl(&self) -> bool {
        match self {
            Self::DSL{..} => true,
            _ => false
        }
    }
    
    pub fn is_value_type(&self)->bool{
        match self{
            Self::Str(_) |
            Self::FittedString(_) |
            Self::InlineString{..} |
            Self::DocumentString {..} |
            Self::Id {..} |
            Self::Bool(_) |
            Self::Int(_) |
            Self::Float(_) |
            Self::Color(_) |
            Self::Vec2(_) |
            Self::Vec3(_) |
            Self::Vec4(_)  => true,
            _=>false
        }
    }
    
    pub fn is_structy_type(&self)->bool{
        match self{
            Self::Object | // subnodes including this one
            Self::Clone {..} | // subnodes including this one
            Self::Class {..} => true, // subnodes including this one        
            _ => false
        }
    }

    pub fn is_float_type(&self)->bool{
        match self{
            Self::Float(_) |
            Self::Color(_) |
            Self::Vec2(_) |
            Self::Vec3(_) |
            Self::Vec4(_) => true,
            _=>false
        }
    }
    /*
    pub fn named_class_id(&self) -> Option<Id> {
        match self {
            Self::Class {class} => Some(*class),
            _ => None
        }
    }*/
    
    pub fn enum_base_id(&self) -> Option<Id> {
        match self {
            Self::BareEnum {base, ..} => Some(*base),
            Self::TupleEnum {base, ..} => Some(*base),
            Self::NamedEnum {base, ..} => Some(*base),
            _ => None
        }
    }
    
    pub fn set_scope(&mut self, in_scope_start: usize, in_scope_count: u32) {
        match self {
            Self::DSL {scope_start, scope_count, ..} => {*scope_start = in_scope_start as u32; *scope_count = in_scope_count;},
            //lf::Const {scope_start, scope_count, ..} => {*scope_start = in_scope_start; *scope_count = in_scope_count;},
            //Self::VarDef {scope_start, scope_count, ..} => {*scope_start = in_scope_start; *scope_count = in_scope_count;},
            _ => ()
        }
    }
    
    pub fn set_clone_name(&mut self, name: Id) {
        match self {
            Self::Clone(clone) => *clone = name,
            _ => ()
        }
    }
    
    pub fn get_clone_name(&self)->Id{
        match self {
            Self::Clone(clone) => *clone,
            _ => Id(0)
        }
    }
    
    pub fn variant_id(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Str(_) => 1,
            Self::FittedString(_) => 2,
            Self::InlineString{..} => 3,
            Self::DocumentString {..} => 4,
            Self::Bool(_) => 5,
            Self::Int(_) => 6,
            Self::Float(_) => 7,
            Self::Color(_) => 8,
            Self::Vec2(_) => 9,
            Self::Vec3(_) => 10,
            Self::Vec4(_) => 11,
            
            Self::Id(_) => 12,
            
            Self::BareEnum {..} => 13,
            Self::Array => 14,
            Self::TupleEnum {..} => 15,
            Self::NamedEnum {..} => 16,
            Self::Object => 17,
            Self::Clone {..} => 18,
            Self::Class {..} => 19,
            Self::Close => 20,
            
            Self::DSL {..} => 21,
            Self::Use {..} => 22
        }
    }
}


pub trait LiveNodeSlice {
    fn parent(&self, child_index: usize) -> Option<usize>;
    fn append_child_index(&self, parent_index: usize) -> usize;
    fn first_child(&self, parent_index: usize) -> Option<usize>;
    fn last_child(&self, parent_index: usize) -> Option<usize>;
    fn child_range(&self, parent_index: usize) -> (usize,usize);
    fn next_child(&self, child_index: usize) -> Option<usize>;
    fn child_by_number(&self, parent_index: usize, child_number: usize) -> Option<usize>;
    fn child_by_name(&self, parent_index: usize, name: Id) -> Result<usize, usize>;
    fn count_children(&self, parent_index: usize) -> usize;
    fn skip_node(&self, node_index: usize)->usize;
    fn clone_child(&self, parent_index: usize, out_vec:&mut Vec<LiveNode>);
    fn to_string(&self, parent_index:usize, max_depth:usize)->String;
}

pub trait LiveNodeVec {

    fn clone_node_from(&mut self, from_index: usize, insert_start: Option<usize>, other: &[LiveNode])->usize;
    fn clone_node_self(&mut self, from_index: usize, insert_start: Option<usize>)->usize;

    fn clone_children_from(&mut self, from_index: usize, insert_start: Option<usize>, other: &[LiveNode]);
    fn clone_children_self(&mut self, from_index: usize, insert_start: Option<usize>)->bool;
}

// accessing the Gen structure like a tree
impl<T> LiveNodeSlice for T where T:AsRef<[LiveNode]> {

    fn parent(&self, child_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref.len() == 0 {
            return None
        }
        let mut stack_depth = 0;
        let mut index = child_index;
        // we are going to scan backwards
        loop {
            if self_ref[index].value.is_open(){
                if stack_depth == 0 {
                    return Some(index)
                }
                stack_depth -= 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth += 1;
            }
            if index == 0 {
                break
            }
            index -= 1;
        }
        Some(0)
    }
    
    fn child_by_number(&self, parent_index: usize, child_number: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut child_count = 0;
        if !self_ref[index].value.is_open(){
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                if stack_depth == 1 {
                    if child_number == child_count {
                        return Some(index);
                    }
                    child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth == 0 {
                    return None
                }
            }
            else{
                if stack_depth == 1 {
                    if child_number == child_count {
                        return Some(index);
                    }
                    child_count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        panic!()
    }
    
    fn first_child(&self, parent_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref[parent_index].value.is_open(){
            if self_ref[parent_index + 1].value.is_close(){
                return None
            }
            return Some(parent_index + 1) // our first child
        }
        panic!()
    }
    
    fn next_child(&self, child_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut index = child_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                if stack_depth == 0 && index != child_index{ 
                    return Some(index)
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                if stack_depth == 0 { // double close
                    return None;
                }
                stack_depth -= 1;
            }
            else{
                if stack_depth == 0 && index != child_index{ 
                    return Some(index)
                }
            }
            index += 1;
        }
        panic!()
    }
    
    fn last_child(&self, parent_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut child_count = 0;
        let mut found_child = None;
        if !self_ref[index].value.is_open(){
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                if stack_depth == 1 {
                    found_child = Some(index);
                    child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth == 0 {
                    return found_child
                }
            }
            else{
               if stack_depth == 1 {
                    found_child = Some(index);
                    child_count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        None
    }
    
    // the entire replaceable childrange.
    fn child_range(&self, parent_index: usize) -> (usize,usize) {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut child_count = 0;
        let mut first_child = None;
        if !self_ref[index].value.is_open(){
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                if stack_depth == 1 {
                    if first_child.is_none(){
                        first_child = Some(index)
                    }
                    child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if first_child.is_none(){
                    first_child = Some(index)
                }
                if stack_depth == 0 {
                    return (first_child.unwrap(), index);
                }
            }
            else {
                if stack_depth == 1 {
                    if first_child.is_none(){
                        first_child = Some(index)
                    }
                    child_count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        panic!()
    }
    
    fn append_child_index(&self, parent_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        if !self_ref[index].value.is_open(){
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth == 0 {
                    return index
                }
            }
            index += 1;
        }
        index
    }
    
    fn child_by_name(&self, parent_index: usize, child_name: Id) -> Result<usize, usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        if !self_ref[index].value.is_open(){
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                if stack_depth == 1 {
                    if child_name != Id::empty() && self_ref[index].id == child_name {
                        return Ok(index);
                    }
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth == 0 {
                    return Err(index)
                }
            }
            else {
                if stack_depth == 1 {
                    if child_name != Id::empty() && self_ref[index].id == child_name {
                        return Ok(index);
                    }
                }
                if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        Err(index)
    }
    
    fn count_children(&self, parent_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut count = 0;
        if !self_ref[index].value.is_open(){
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                if stack_depth == 1 {
                    count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth == 0 {
                    return count
                }
            }
            else{
                if stack_depth == 1 {
                    count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        panic!()
    }
    
    fn skip_node(&self, node_index: usize)->usize{
        let self_ref = self.as_ref();
        let mut index = node_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            if self_ref[index].value.is_open(){
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth == 0 {
                    index += 1;
                    return index
                }
            }
            else {
                if stack_depth == 0 {
                    index += 1;
                    return index
                }
            }
            index += 1;
        }
        return index
    }
    
    fn clone_child(&self, parent_index: usize, out:&mut Vec<LiveNode>){
        let self_ref = self.as_ref();
        let mut index = parent_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            out.push(self_ref[index].clone());
            if self_ref[index].value.is_open(){
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth == 0 {
                    return
                }
            }
            else {
                if stack_depth == 0 {
                    return
                }
            }
            index += 1;
        }
        return
    }
        
    fn to_string(&self, parent_index:usize, max_depth:usize)->String{
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut f = String::new();
        let mut index = parent_index;
        while index < self_ref.len(){ 
            let node = &self_ref[index];
            if stack_depth > max_depth{
                if node.value.is_open(){
                    stack_depth +=1;
                }
                else if node.value.is_close(){
                    stack_depth -= 1;
                }
                index += 1;
                continue
            }
            for _ in 0..stack_depth {
                write!(f, "|   ").unwrap();
            }
            match &node.value{
                LiveValue::None=>{
                   writeln!(f, "{}: <None>", node.id).unwrap();
                },
                LiveValue::Str(s)=>{
                   writeln!(f, "{}: <Str> {}", node.id, s).unwrap();
                },
                LiveValue::InlineString(s)=>{
                    writeln!(f, "{}: <InlineString> {}", node.id, s.as_str()).unwrap();
                },
                LiveValue::FittedString(s)=>{
                    writeln!(f, "{}: <FittedString> {}", node.id, s.as_str()).unwrap();
                },
                LiveValue::DocumentString {string_start, string_count}=>{
                    writeln!(f, "{}: <DocumentString> string_start:{}, string_end:{}", node.id, string_start, string_count).unwrap();
                },
                LiveValue::Bool(v)=>{
                    writeln!(f, "{}: <Bool> {}", node.id, v).unwrap();
                }
                LiveValue::Int(v)=>{
                    writeln!(f, "{}: <Int> {}", node.id, v).unwrap();
                }
                LiveValue::Float(v)=>{
                    writeln!(f, "{}: <Float> {}", node.id, v).unwrap();
                },
                LiveValue::Color(v)=>{
                    writeln!(f, "{}: <Color>{}", node.id, v).unwrap();
                },
                LiveValue::Vec2(v)=>{
                    writeln!(f, "{}: <Vec2> {:?}", node.id, v).unwrap();
                },
                LiveValue::Vec3(v)=>{
                    writeln!(f, "{}: <Vec3> {:?}", node.id, v).unwrap();
                },
                LiveValue::Vec4(v)=>{
                    writeln!(f, "{}: <Vec4> {:?}", node.id, v).unwrap();
                },
                LiveValue::Id(id)=>{
                   writeln!(f, "{}: <Id> {}", node.id, id).unwrap();
                },
                LiveValue::BareEnum {base, variant}=>{
                    writeln!(f, "{}: <BareEnum> {}::{}", node.id, base, variant).unwrap();
                },
                // stack items
                LiveValue::Array=>{
                    writeln!(f, "{}: <Array>", node.id).unwrap();
                    stack_depth += 1;
                },
                LiveValue::TupleEnum {base, variant}=>{
                    writeln!(f, "{}: <TupleEnum> {}::{}", node.id, base, variant).unwrap();
                    stack_depth += 1;
                },
                LiveValue::NamedEnum {base, variant}=>{
                    writeln!(f, "{}: <NamedEnum> {}::{}", node.id, base, variant).unwrap();
                    stack_depth += 1;
                },
                LiveValue::Object=>{
                    writeln!(f, "{}: <Object>", node.id).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Clone(clone)=>{
                    writeln!(f, "{}: <Clone> {}", node.id, clone).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Class(live_type)=>{
                    writeln!(f, "{}: <Class> {:?}", node.id, live_type).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Close=>{
                    writeln!(f, "<Close> {}", node.id).unwrap();
                    stack_depth -= 1;
                    if stack_depth == 0{
                        break;
                    }
                },
                // the shader code types
                LiveValue::DSL {
                    token_start,
                    token_count,
                    scope_start,
                    scope_count
                }=>{
                    writeln!(f, "<DSL> {} :token_start:{}, token_count:{}, scope_start:{}, scope_end:{}", node.id, token_start, token_count, scope_start,scope_count).unwrap();
                },
                LiveValue::Use{
                    crate_id,
                    module_id,
                }=>{
                    writeln!(f, "<Use> {}::{}::{}", crate_id, module_id, node.id).unwrap();
                }
                
            }
            index += 1;
        }
        if stack_depth != 0{
            writeln!(f, "[[ERROR Stackdepth not 0 at end {}]]", stack_depth).unwrap()
        }
        f
    }
}
//}
/*
impl_live_node_slice!(&[LiveNode]);
impl_live_node_slice!(&mut [LiveNode]);
impl_live_node_slice!(Vec<LiveNode>);
*/
impl LiveNodeVec for Vec<LiveNode> {
    fn clone_children_from(&mut self, index: usize, insert_start: Option<usize>, other: &[LiveNode]) {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            other.len()
        };
        while index < other.len() {
            if other[index].value.is_open(){
                if stack_depth >= 1 {
                    self.insert(insert_point, other[index].clone());
                    insert_point += 1;
                }
                stack_depth += 1;
            }
            else if other[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth >= 1 {
                    self.insert(insert_point, other[index].clone());
                    insert_point += 1;
                }
                if stack_depth == 0 {
                    return
                }
            }
            else{
                self.insert(insert_point, other[index].clone());
                insert_point += 1;
                if stack_depth == 0 {
                    return
                }
            }
            index += 1;
        }
        panic!();
    }
    
    
    fn clone_children_self(&mut self, index: usize, insert_start: Option<usize>)->bool {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            self.len()
        };
        //let mut insert_start = insert_point;
        while index < self.len() {
            if self[index].value.is_open(){
                if stack_depth >= 1 {
                    self.insert(insert_point, self[index].clone());
                    insert_point += 1;
                }
                stack_depth += 1;
            }
            else if self[index].value.is_close(){
                stack_depth -= 1;
                if stack_depth >= 1 {
                    self.insert(insert_point, self[index].clone());
                    insert_point += 1;
                }
                if stack_depth == 0 {
                    return false
                }
            }
            else{
                self.insert(insert_point, self[index].clone());
                insert_point += 1;
                if stack_depth == 0 {
                    return false
                }
            }
            if stack_depth > MAX_CLONE_STACK_DEPTH_SAFETY{
                return true
            }
            index += 1;
        }
        false
    }
    
    
    fn clone_node_self(&mut self, index: usize, insert_start: Option<usize>)->usize {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            self.len()
        };
        if insert_point < index{
            panic!();
        }
        while index < self.len() {
            self.insert(insert_point, self[index].clone());
            if self[index].value.is_open(){
                insert_point += 1;
                stack_depth += 1;
            }
            else if self[index].value.is_close(){
                stack_depth -= 1;
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            else{
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            index += 1;
        }
        panic!();
    }


    fn clone_node_from(&mut self, index: usize, insert_start: Option<usize>, other: &[LiveNode])->usize {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            other.len()
        };
        while index < other.len() {
            self.insert(insert_point, other[index].clone());
            if other[index].value.is_open(){
                insert_point += 1;
                stack_depth += 1;
            }
            else if other[index].value.is_close(){
                stack_depth -= 1;
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            else{
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            index += 1;
        }
        panic!();
    }
}
const MAX_CLONE_STACK_DEPTH_SAFETY:usize = 100;