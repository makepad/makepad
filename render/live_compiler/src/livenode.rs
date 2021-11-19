#![allow(unused_variables)]
use crate::id::Id;
use crate::token::TokenId;
use crate::math::{Vec2, Vec3};
use std::fmt::Write;

#[derive(Clone, Debug, Eq, PartialEq, Copy, Hash)]
pub struct LiveType(pub core::any::TypeId);

#[derive(Clone, Debug)]
pub struct LiveNode { // 3x u64
    pub token_id: Option<TokenId>,
    pub id: Id,
    pub value: LiveValue,
}

impl LiveValue {
    pub fn is_open(&self) -> bool {
        match self {
            Self::Array |
            Self::TupleEnum {..} |
            Self::NamedEnum {..} |
            Self::Object | // subnodes including this one
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
    
    pub fn is_object(&self) -> bool {
        match self {
            Self::Object => true,
            _ => false
        }
    }

    pub fn is_var_def(&self) -> bool {
        match self {
            Self::VarDef{..} => true,
            _ => false
        }
    }
    
    pub fn is_value_type(&self)->bool{
        match self{
            Self::Str(_) |
            Self::String(_) |
            Self::StringRef {..} |
            Self::Id {..} |
            Self::Bool(_) |
            Self::Int(_) |
            Self::Float(_) |
            Self::Color(_) |
            Self::Vec2(_) |
            Self::Vec3(_) => true,
            _=>false
        }
    }

    pub fn is_float_type(&self)->bool{
        match self{
            Self::Float(_) |
            Self::Color(_) |
            Self::Vec2(_) |
            Self::Vec3(_) => true,
            _=>false
        }
    }
    
    pub fn named_class_id(&self) -> Option<Id> {
        match self {
            Self::Class {class} => Some(*class),
            _ => None
        }
    }
    
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
            Self::Fn {scope_start, scope_count, ..} => {*scope_start = in_scope_start; *scope_count = in_scope_count;},
            Self::Const {scope_start, scope_count, ..} => {*scope_start = in_scope_start; *scope_count = in_scope_count;},
            Self::VarDef {scope_start, scope_count, ..} => {*scope_start = in_scope_start; *scope_count = in_scope_count;},
            _ => ()
        }
    }
    
    pub fn set_class_name(&mut self, name: Id) {
        match self {
            Self::Class {class} => *class = name,
            _ => ()
        }
    }
    
    pub fn get_class_name(&self)->Id{
        match self {
            Self::Class {class} => *class,
            _ => Id(0)
        }
    }
    
    pub fn variant_id(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Str(_) => 1,
            Self::String(_) => 2,
            Self::StringRef {..} => 3,
            Self::Bool(_) => 4,
            Self::Int(_) => 5,
            Self::Float(_) => 6,
            Self::Color(_) => 7,
            Self::Vec2(_) => 8,
            Self::Vec3(_) => 9,
            
            Self::Id(_) => 10,
            Self::LiveType(_) => 11,
            
            Self::BareEnum {..} => 12,
            Self::Array => 13,
            Self::TupleEnum {..} => 14,
            Self::NamedEnum {..} => 15,
            Self::Object => 16,
            Self::Class {..} => 17,
            Self::Close => 18,
            Self::Fn {..} => 19,
            Self::Const {..} => 20,
            Self::VarDef {..} => 21,
            Self::Use {..} => 22
        }
    }
}

#[derive(Clone, Debug)]
pub enum LiveValue {
    None,
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

    Id(Id),
    LiveType(LiveType),

    BareEnum {base: Id, variant: Id},
    // stack items
    Array,
    TupleEnum {base: Id, variant: Id},
    NamedEnum {base: Id, variant: Id},
    Object, // subnodes including this one
    Class {class: Id}, // subnodes including this one
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
    fn bare(&self) -> &[LiveNode];
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
    fn clone_children_from(&mut self, from_index: usize, insert_start: Option<usize>, other: &[LiveNode]);
    fn clone_children_self(&mut self, from_index: usize, insert_start: Option<usize>)->bool;
}

macro_rules!impl_live_node_slice {
    ( $ for_type: ty) => {
        // accessing the Gen structure like a tree
        impl LiveNodeSlice for $ for_type {
//        impl LiveNodeSlice for &[LiveNode] {
            fn bare(&self) -> &[LiveNode]{
                &self[1..self.len()-2]
            }
    
            fn parent(&self, child_index: usize) -> Option<usize> {
                if self.len() == 0 {
                    return None
                }
                let mut stack_depth = 0;
                let mut index = child_index;
                // we are going to scan backwards
                loop {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
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
            
            fn child_by_number(&self, parent_index: usize, child_number: usize) -> Option<usize> {
                let mut stack_depth = 0;
                let mut index = parent_index;
                let mut child_count = 0;
                if !self[index].value.is_open(){
                    panic!()
                }
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
                            if stack_depth == 1 {
                                if child_number == child_count {
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
                                if child_number == child_count {
                                    return Some(index);
                                }
                                child_count += 1;
                            }
                            else if stack_depth == 0 {
                                panic!()
                            }
                        }
                    }
                    index += 1;
                }
                panic!()
            }
            
            fn first_child(&self, parent_index: usize) -> Option<usize> {
                match &self[parent_index].value {
                    LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
                        if self[parent_index + 1].value.is_close(){
                            return None
                        }
                        return Some(parent_index + 1) // our first child
                    }
                    _ => {
                        panic!()
                    }
                }
            }
            
            fn next_child(&self, child_index: usize) -> Option<usize> {
                let mut index = child_index;
                let mut stack_depth = 0;
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
                            if stack_depth == 0 && index != child_index{ 
                                return Some(index)
                            }
                            stack_depth += 1;
                        }
                        LiveValue::Close => { 
                            if stack_depth == 0 { // double close
                                return None;
                            }
                            stack_depth -= 1;
                        }
                        _ => { //normal value
                            if stack_depth == 0 && index != child_index{ 
                                return Some(index)
                            }
                        }
                    }
                    index += 1;
                }
                panic!()
            }
            
            fn last_child(&self, parent_index: usize) -> Option<usize> {
                let mut stack_depth = 0;
                let mut index = parent_index;
                let mut child_count = 0;
                let mut found_child = None;
                if !self[index].value.is_open(){
                    panic!()
                }
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
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
                                panic!()
                            }
                        }
                    }
                    index += 1;
                }
                None
            }
            
            // the entire replaceable childrange.
            fn child_range(&self, parent_index: usize) -> (usize,usize) {
                let mut stack_depth = 0;
                let mut index = parent_index;
                let mut child_count = 0;
                let mut first_child = None;
                if !self[index].value.is_open(){
                    panic!()
                }
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
                            if stack_depth == 1 {
                                if first_child.is_none(){
                                    first_child = Some(index)
                                }
                                child_count += 1;
                            }
                            stack_depth += 1;
                        }
                        LiveValue::Close => {
                            stack_depth -= 1;
                            if first_child.is_none(){
                                first_child = Some(index)
                            }
                            if stack_depth == 0 {
                                return (first_child.unwrap(), index);
                            }
                        }
                        _ => {
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
                    }
                    index += 1;
                }
                panic!()
            }
            
            fn append_child_index(&self, parent_index: usize) -> usize {
                let mut stack_depth = 0;
                let mut index = parent_index;
                if !self[index].value.is_open(){
                    panic!()
                }
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
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
                index
            }
            
            fn child_by_name(&self, parent_index: usize, child_name: Id) -> Result<usize, usize> {
                let mut stack_depth = 0;
                let mut index = parent_index;
                if !self[index].value.is_open(){
                    panic!()
                }
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
                            if stack_depth == 1 {
                                if child_name != Id::empty() && self[index].id == child_name {
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
                                if child_name != Id::empty() && self[index].id == child_name {
                                    return Ok(index);
                                }
                            }
                            if stack_depth == 0 {
                                panic!()
                            }
                        }
                    }
                    index += 1;
                }
                Err(index)
            }
            
            fn count_children(&self, parent_index: usize) -> usize {
                let mut stack_depth = 0;
                let mut index = parent_index;
                let mut count = 0;
                if !self[index].value.is_open(){
                    panic!()
                }
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
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
                                panic!()
                            }
                        }
                    }
                    index += 1;
                }
                panic!()
            }
            
            fn skip_node(&self, node_index: usize)->usize{
                let mut index = node_index;
                let mut stack_depth = 0;
                while index < self.len() {
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
                            stack_depth += 1;
                        }
                        LiveValue::Close => {
                            stack_depth -= 1;
                            if stack_depth == 0 {
                                index += 1;
                                return index
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
                return index
            }
            
            fn clone_child(&self, parent_index: usize, out:&mut Vec<LiveNode>){
                let mut index = parent_index;
                let mut stack_depth = 0;
                while index < self.len() {
                    out.push(self[index].clone());
                    match &self[index].value {
                        LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
                            stack_depth += 1;
                        }
                        LiveValue::Close => {
                            stack_depth -= 1;
                            if stack_depth == 0 {
                                return
                            }
                        }
                        _ => {
                            if stack_depth == 0 {
                                return
                            }
                        }
                    }
                    index += 1;
                }
                return
            }
                
            fn to_string(&self, parent_index:usize, max_depth:usize)->String{
                let mut stack_depth = 0;
                let mut f = String::new();
                let mut index = parent_index;
                while index < self.len(){ 
                    let node = &self[index];
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
                        LiveValue::String(s)=>{
                            writeln!(f, "{}: <String> {}", node.id, s).unwrap();
                        },
                        LiveValue::StringRef {string_start, string_count}=>{
                            writeln!(f, "{}: <StringRef> string_start:{}, string_end:{}", node.id, string_start, string_count).unwrap();
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
                        LiveValue::LiveType(v)=>{
                            writeln!(f, "{}: <LiveType> {:?}", node.id, v).unwrap();
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
                        LiveValue::Class {class}=>{
                            writeln!(f, "{}: <Class> {}", node.id, class).unwrap();
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
                        LiveValue::Fn {
                            token_start,
                            token_count,
                            scope_start,
                            scope_count
                        }=>{
                            writeln!(f, "<Fn> {} :token_start:{}, token_count:{}, scope_start:{}, scope_end:{}", node.id, token_start, token_count, scope_start,scope_count).unwrap();
                        },
                        LiveValue::Const {
                            token_start,
                            token_count,
                            scope_start,
                            scope_count
                        }=>{
                            writeln!(f, "<Const> {} :token_start:{}, token_count:{}, scope_start:{}, scope_end:{}", node.id, token_start, token_count, scope_start,scope_count).unwrap();
                        },
                        LiveValue::VarDef { //instance/uniform def
                            token_start,
                            token_count,
                            scope_start,
                            scope_count
                        }=>{
                            writeln!(f, "<VarDef> {} : token_start:{}, token_count:{}, scope_start:{}, scope_end:{}", node.id, token_start, token_count, scope_start,scope_count).unwrap();
                        },
                        LiveValue::Use{
                            crate_id,
                            module_id,
                            object_id
                        }=>{
                            writeln!(f, "<Use> {}::{}::{}", crate_id, module_id, object_id).unwrap();
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
    }
}
impl_live_node_slice!(&[LiveNode]);
impl_live_node_slice!(&mut [LiveNode]);
impl_live_node_slice!(Vec<LiveNode>);

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
            match &other[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
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
                        return
                    }
                }
                _ => {
                    self.insert(insert_point, other[index].clone());
                    insert_point += 1;
                    if stack_depth == 0 {
                        return
                    }
                }
            }
            index += 1;
        }
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
            match &self[index].value {
                LiveValue::TupleEnum {..} | LiveValue::NamedEnum {..} | LiveValue::Object | LiveValue::Class {..} | LiveValue::Array => {
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
                        return false
                    }
                }
                _ => {
                    self.insert(insert_point, self[index].clone());
                    insert_point += 1;
                    if stack_depth == 0 {
                        return false
                    }
                }
            }
            if stack_depth > MAX_CLONE_STACK_DEPTH_SAFETY{
                return true
            }
            index += 1;
        }
        false
    }
}
const MAX_CLONE_STACK_DEPTH_SAFETY:usize = 100;