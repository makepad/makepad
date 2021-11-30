use {
    std::fmt::Write,
    std::ops::Deref,
    makepad_derive_live::{
        live_object
    },
    crate::{
        liveid::{LiveId, LiveModuleId, LivePtr},
        token::TokenId,
        math::{Vec2, Vec3, Vec4},
    }
};

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Copy, Hash)]
pub struct LiveType(pub core::any::TypeId);

#[derive(Clone)]
pub enum LiveTypeKind {
    Class,
    Enum,
    Object,
    Primitive,
    DrawVars,
}

#[derive(Clone)]
pub struct LiveTypeInfo {
    pub live_type: LiveType,
    pub type_name: LiveId,
    pub module_id: LiveModuleId,
    pub kind: LiveTypeKind,
    pub fields: Vec<LiveTypeField>
}

#[derive(Clone)]
pub struct LiveTypeField {
    pub id: LiveId,
    pub live_type_info: LiveTypeInfo,
    pub live_field_kind: LiveFieldKind
}

#[derive(Copy, Clone)]
pub enum LiveFieldKind {
    Calc,
    Live,
    LiveOption
}

#[derive(Clone, Debug, PartialEq)]
pub struct LiveNode { // 40 bytes. Don't really see ways to compress
    pub token_id: Option<TokenId>,
    pub id: LiveId,
    pub value: LiveValue,
}

#[derive(Clone, Debug, PartialEq)]
pub enum LiveValue {
    None,
    // string types
    Str(&'static str),
    DocumentString {
        string_start: usize,
        string_count: usize
    },
    FittedString(FittedString),
    InlineString(InlineString),
    // bare values
    Bool(bool),
    Int(i64),
    Float(f64),
    Color(u32),
    Vec2(Vec2),
    Vec3(Vec3),
    Vec4(Vec4),
    Id(LiveId),
    
    // enum thing
    BareEnum {base: LiveId, variant: LiveId},
    // tree items
    Array,
    TupleEnum {base: LiveId, variant: LiveId},
    NamedEnum {base: LiveId, variant: LiveId},
    Object,
    Clone(LiveId),
    Class {live_type: LiveType, class_parent: Option<LivePtr>},
    Close,
    
    // shader code and other DSLs
    DSL {
        token_start: u32,
        token_count: u32,
        scope_start: u32,
        scope_count: u32
    },
    Use (LiveModuleId),
}

#[derive(Debug)]
pub struct FittedString {
    buffer: *mut u8,
    length: usize,
}

impl PartialEq for FittedString {
    fn eq(&self, other: &FittedString) -> bool {
        self.as_str() == other.as_str()
    }
    
    fn ne(&self, other: &FittedString) -> bool {
        self.as_str() != other.as_str()
    }
}

impl FittedString {
    pub fn from_string(mut inp: String) -> Self {
        inp.shrink_to_fit();
        let mut s = std::mem::ManuallyDrop::new(inp);
        let buffer = s.as_mut_ptr();
        let length = s.len();
        let capacity = s.capacity();
        if length != capacity {
            panic!()
        }
        FittedString {buffer, length}
    }
    
    pub fn to_string(self) -> String {
        unsafe {String::from_raw_parts(self.buffer, self.length, self.length)}
    }
    
    pub fn as_str<'a>(&'a self) -> &'a str {
        unsafe {std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.buffer, self.length))}
    }
}

impl Drop for FittedString {
    fn drop(&mut self) {
        unsafe {String::from_raw_parts(self.buffer, self.length, self.length)};
    }
}

impl Clone for FittedString {
    fn clone(&self) -> Self {
        Self::from_string(self.as_str().to_string())
    }
}

const INLINE_STRING_BUFFER_SIZE: usize = 22;
#[derive(Clone, Debug, PartialEq)]
pub struct InlineString {
    length: u8,
    buffer: [u8; INLINE_STRING_BUFFER_SIZE]
}

impl InlineString {
    pub fn from_str(inp: &str) -> Option<Self> {
        let bytes = inp.as_bytes();
        if bytes.len()<INLINE_STRING_BUFFER_SIZE {
            let mut buffer = [0u8; INLINE_STRING_BUFFER_SIZE];
            for i in 0..bytes.len() {
                buffer[i] = bytes[i];
            }
            return Some(Self {length: bytes.len() as u8, buffer})
        }
            None
    }
    
    pub fn as_str<'a>(&'a self) -> &'a str {
        unsafe {std::str::from_utf8_unchecked(std::slice::from_raw_parts(self.buffer.as_ptr(), self.length as usize))}
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
    pub fn is_id(&self) -> bool {
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
            Self::DSL {..} => true,
            _ => false
        }
    }
    
    pub fn is_value_type(&self) -> bool {
        match self {
            Self::Str(_) |
            Self::FittedString(_) |
            Self::InlineString {..} |
            Self::DocumentString {..} |
            Self::Bool(_) |
            Self::Int(_) |
            Self::Float(_) |
            Self::Color(_) |
            Self::Vec2(_) |
            Self::Vec3(_) |
            Self::Vec4(_) |
            Self::Id {..} => true,
            _ => false
        }
    }
    
    pub fn is_structy_type(&self) -> bool {
        match self {
            Self::Object | // subnodes including this one
            Self::Clone {..} | // subnodes including this one
            Self::Class {..} => true, // subnodes including this one
            _ => false
        }
    }
    
    pub fn is_float_type(&self) -> bool {
        match self {
            Self::Float(_) |
            Self::Color(_) |
            Self::Vec2(_) |
            Self::Vec3(_) |
            Self::Vec4(_) => true,
            _ => false
        }
    }
    /*
    pub fn named_class_id(&self) -> Option<Id> {
        match self {
            Self::Class {class} => Some(*class),
            _ => None
        }
    }*/
    
    pub fn enum_base_id(&self) -> Option<LiveId> {
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
    
    pub fn set_clone_name(&mut self, name: LiveId) {
        match self {
            Self::Clone(clone) => *clone = name,
            _ => ()
        }
    }
    
    pub fn get_clone_name(&self) -> LiveId {
        match self {
            Self::Clone(clone) => *clone,
            _ => LiveId(0)
        }
    }
    
    pub fn variant_id(&self) -> usize {
        match self {
            Self::None => 0,
            Self::Str(_) => 1,
            Self::FittedString(_) => 2,
            Self::InlineString {..} => 3,
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
    fn next_child(&self, child_index: usize) -> Option<usize>;
    fn node_slice(&self, parent_index: usize) -> &[LiveNode];
    fn children_slice(&self, parent_index: usize) -> &[LiveNode];

    fn child_by_number(&self, parent_index: usize, child_number: usize) -> Option<usize>;
    fn child_or_append_index_by_name(&self, parent_index: usize, name: LiveId) -> Result<usize, usize>;
    fn child_by_name(&self, parent_index: usize, name: LiveId) ->Option<usize>;
    fn child_by_path(&self, parent_index: usize, path: &[LiveId]) ->Option<usize>;
    fn child_value_by_path(&self, parent_index: usize, path: &[LiveId]) ->Option<&LiveValue>;

    fn scope_up_by_name(&self, parent_index: usize, name: LiveId) -> Option<usize>;
    fn count_children(&self, parent_index: usize) -> usize;
    fn skip_node(&self, node_index: usize) -> usize;
    fn clone_child(&self, parent_index: usize, out_vec: &mut Vec<LiveNode>);
    fn to_string(&self, parent_index: usize, max_depth: usize) -> String;
}

pub trait LiveNodeVec {
    fn insert_node_from_other(&mut self, from_index: usize, insert_start: Option<usize>, other: &[LiveNode]) -> usize;
    fn insert_node_from_self(&mut self, from_index: usize, insert_start: Option<usize>) -> usize;
    
    fn insert_children_from_other(&mut self, from_index: usize, insert_start: Option<usize>, other: &[LiveNode]);
    fn insert_children_from_self(&mut self, from_index: usize, insert_start: Option<usize>) -> bool;
    
    fn replace_or_insert_node_by_path(&mut self, start_index: usize, path:&[LiveId], other: &[LiveNode]);
    
    fn push_live(&mut self, v: &[LiveNode]);
    fn push_str(&mut self, id: LiveId, v: &'static str);
    fn push_string(&mut self, id: LiveId, v: &str);
    fn push_bool(&mut self, id: LiveId, v: bool);
    fn push_int(&mut self, id: LiveId, v: i64);
    fn push_float(&mut self, id: LiveId, v: f64);
    fn push_color(&mut self, id: LiveId, v: u32);
    fn push_vec2(&mut self, id: LiveId, v: Vec2);
    fn push_vec3(&mut self, id: LiveId, v: Vec3);
    fn push_vec4(&mut self, id: LiveId, v: Vec4);
    fn push_id(&mut self, id: LiveId, v: LiveId);
    fn push_bare_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId);
    
    fn open_tuple_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId);
    fn open_named_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId);
    fn open_object(&mut self, id: LiveId);
    fn open_clone(&mut self, id: LiveId, clone: LiveId);
    fn open_array(&mut self, id: LiveId);
    
    fn open(&mut self);
    fn close(&mut self);
}

// accessing the Gen structure like a tree
impl<T> LiveNodeSlice for T where T: AsRef<[LiveNode]> {
    
    fn parent(&self, child_index: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref.len() == 0 {
            return None
        }
        let mut stack_depth = 0;
        let mut index = child_index;
        // we are going to scan backwards
        loop {
            if self_ref[index].value.is_open() {
                if stack_depth == 0 {
                    return Some(index)
                }
                stack_depth -= 1;
            }
            else if self_ref[index].value.is_close() {
                stack_depth += 1;
            }
            if index == 0 {
                break
            }
            index -= 1;
        }
        Some(0)
    }
    
    fn scope_up_by_name(&self, index: usize, name: LiveId) -> Option<usize> {
        let self_ref = self.as_ref();
        if self_ref.len() == 0 {
            return None
        }
        let mut stack_depth: isize = 0;
        let mut index = index;
        // scan backwards to find a node with this name
        loop {
            if self_ref[index].value.is_open() {
                if stack_depth>0 {
                    stack_depth -= 1;
                }
            }
            else if self_ref[index].value.is_close() {
                stack_depth += 1;
            }
            if stack_depth == 0 && self_ref[index].id == name && !self_ref[index].value.is_close() { // valuenode
                return Some(index)
            }
            
            if index == 0 {
                break
            }
            index -= 1;
        }
        None
    }
    
    fn child_by_number(&self, parent_index: usize, child_number: usize) -> Option<usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut child_count = 0;
        if !self_ref[index].value.is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open() {
                if stack_depth == 1 {
                    if child_number == child_count {
                        return Some(index);
                    }
                    child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return None
                }
            }
            else {
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
        if self_ref[parent_index].value.is_open() {
            if self_ref[parent_index + 1].value.is_close() {
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
            if self_ref[index].value.is_open() {
                if stack_depth == 0 && index != child_index {
                    return Some(index)
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                if stack_depth == 0 { // double close
                    return None;
                }
                stack_depth -= 1;
            }
            else {
                if stack_depth == 0 && index != child_index {
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
        //let mut child_count = 0;
        let mut found_child = None;
        if !self_ref[index].value.is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open() {
                if stack_depth == 1 {
                    found_child = Some(index);
                    //child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return found_child
                }
            }
            else {
                if stack_depth == 1 {
                    found_child = Some(index);
                    //child_count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        None
    }
    
    fn node_slice(&self, start_index: usize) -> &[LiveNode]{
        let next_index = self.skip_node(start_index);
        &self.as_ref()[start_index..next_index]
    }

    fn children_slice(&self, start_index: usize) -> &[LiveNode]{
        if !self.as_ref()[start_index].value.is_open(){
            &self.as_ref()[start_index..start_index]
        }
        else{
            let next_index = self.as_ref().skip_node(start_index);
            &self.as_ref()[start_index+1..next_index-1]
        }
    }
    /*
    // the entire replaceable childrange.
    fn child_range(&self, parent_index: usize) -> (usize, usize) {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        //let mut child_count = 0;
        let mut first_child = None;
        if !self_ref[index].value.is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open() {
                if stack_depth == 1 {
                    if first_child.is_none() {
                        first_child = Some(index)
                    }
                    //child_count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                stack_depth -= 1;
                if first_child.is_none() {
                    first_child = Some(index)
                }
                if stack_depth == 0 {
                    return (first_child.unwrap(), index);
                }
            }
            else {
                if stack_depth == 1 {
                    if first_child.is_none() {
                        first_child = Some(index)
                    }
                    //child_count += 1;
                }
                else if stack_depth == 0 {
                    panic!()
                }
            }
            index += 1;
        }
        panic!()
    }*/
    
    fn append_child_index(&self, parent_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        if !self_ref[index].value.is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open() {
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return index
                }
            }
            index += 1;
        }
        index
    }
    
    fn child_or_append_index_by_name(&self, parent_index: usize, child_name: LiveId) -> Result<usize, usize> {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        if !self_ref[index].value.is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open() {
                if stack_depth == 1 {
                    if child_name != LiveId::empty() && self_ref[index].id == child_name {
                        return Ok(index);
                    }
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return Err(index)
                }
            }
            else {
                if stack_depth == 1 {
                    if child_name != LiveId::empty() && self_ref[index].id == child_name {
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
    
    fn child_by_name(&self, parent_index: usize, name: LiveId) ->Option<usize>{
        if let Ok(value) = self.child_or_append_index_by_name(parent_index, name){
            Some(value)
        }
        else{
            None
        }
    }
    
    fn child_by_path(&self, parent_index: usize, path: &[LiveId]) ->Option<usize>{
        let mut index = parent_index;
        for level in path{
            if let Some(child) = self.child_by_name(index, *level){
                index = child
            }
            else{
                return None
            }
        }
        Some(index)
    }
    
    fn child_value_by_path(&self, parent_index: usize, path: &[LiveId]) ->Option<&LiveValue>{
        if let Some(index) = self.child_by_path(parent_index, path){
            Some(&self.as_ref()[index].value)
        }
        else{
            None
        }
    }
    
    fn count_children(&self, parent_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut index = parent_index;
        let mut count = 0;
        if !self_ref[index].value.is_open() {
            panic!()
        }
        while index < self_ref.len() {
            if self_ref[index].value.is_open() {
                if stack_depth == 1 {
                    count += 1;
                }
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                stack_depth -= 1;
                if stack_depth == 0 {
                    return count
                }
            }
            else {
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
    
    fn skip_node(&self, node_index: usize) -> usize {
        let self_ref = self.as_ref();
        let mut index = node_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            if self_ref[index].value.is_open() {
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
                if stack_depth == 0 {
                    panic!()
                }
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
    
    fn clone_child(&self, parent_index: usize, out: &mut Vec<LiveNode>) {
        let self_ref = self.as_ref();
        let mut index = parent_index;
        let mut stack_depth = 0;
        while index < self_ref.len() {
            out.push(self_ref[index].clone());
            if self_ref[index].value.is_open() {
                stack_depth += 1;
            }
            else if self_ref[index].value.is_close() {
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
    
    fn to_string(&self, parent_index: usize, max_depth: usize) -> String {
        let self_ref = self.as_ref();
        let mut stack_depth = 0;
        let mut f = String::new();
        let mut index = parent_index;
        while index < self_ref.len() {
            let node = &self_ref[index];
            if stack_depth > max_depth {
                if node.value.is_open() {
                    stack_depth += 1;
                }
                else if node.value.is_close() {
                    stack_depth -= 1;
                }
                index += 1;
                continue
            }
            for _ in 0..stack_depth {
                write!(f, "|   ").unwrap();
            }
            match &node.value {
                LiveValue::None => {
                    writeln!(f, "{}: <None>", node.id).unwrap();
                },
                LiveValue::Str(s) => {
                    writeln!(f, "{}: <Str> {}", node.id, s).unwrap();
                },
                LiveValue::InlineString(s) => {
                    writeln!(f, "{}: <InlineString> {}", node.id, s.as_str()).unwrap();
                },
                LiveValue::FittedString(s) => {
                    writeln!(f, "{}: <FittedString> {}", node.id, s.as_str()).unwrap();
                },
                LiveValue::DocumentString {string_start, string_count} => {
                    writeln!(f, "{}: <DocumentString> string_start:{}, string_end:{}", node.id, string_start, string_count).unwrap();
                },
                LiveValue::Bool(v) => {
                    writeln!(f, "{}: <Bool> {}", node.id, v).unwrap();
                }
                LiveValue::Int(v) => {
                    writeln!(f, "{}: <Int> {}", node.id, v).unwrap();
                }
                LiveValue::Float(v) => {
                    writeln!(f, "{}: <Float> {}", node.id, v).unwrap();
                },
                LiveValue::Color(v) => {
                    writeln!(f, "{}: <Color>{:08x}", node.id, v).unwrap();
                },
                LiveValue::Vec2(v) => {
                    writeln!(f, "{}: <Vec2> {:?}", node.id, v).unwrap();
                },
                LiveValue::Vec3(v) => {
                    writeln!(f, "{}: <Vec3> {:?}", node.id, v).unwrap();
                },
                LiveValue::Vec4(v) => {
                    writeln!(f, "{}: <Vec4> {:?}", node.id, v).unwrap();
                },
                LiveValue::Id(id) => {
                    writeln!(f, "{}: <Id> {}", node.id, id).unwrap();
                },
                LiveValue::BareEnum {base, variant} => {
                    writeln!(f, "{}: <BareEnum> {}::{}", node.id, base, variant).unwrap();
                },
                // stack items
                LiveValue::Array => {
                    writeln!(f, "{}: <Array>", node.id).unwrap();
                    stack_depth += 1;
                },
                LiveValue::TupleEnum {base, variant} => {
                    writeln!(f, "{}: <TupleEnum> {}::{}", node.id, base, variant).unwrap();
                    stack_depth += 1;
                },
                LiveValue::NamedEnum {base, variant} => {
                    writeln!(f, "{}: <NamedEnum> {}::{}", node.id, base, variant).unwrap();
                    stack_depth += 1;
                },
                LiveValue::Object => {
                    writeln!(f, "{}: <Object>", node.id).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Clone(clone) => {
                    writeln!(f, "{}: <Clone> {}", node.id, clone).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Class {live_type, ..} => {
                    writeln!(f, "{}: <Class> {:?}", node.id, live_type).unwrap();
                    stack_depth += 1;
                }, // subnodes including this one
                LiveValue::Close => {
                    writeln!(f, "<Close> {}", node.id).unwrap();
                    stack_depth -= 1;
                    if stack_depth == 0 {
                        break;
                    }
                },
                // the shader code types
                LiveValue::DSL {
                    token_start,
                    token_count,
                    scope_start,
                    scope_count
                } => {
                    writeln!(f, "<DSL> {} :token_start:{}, token_count:{}, scope_start:{}, scope_end:{}", node.id, token_start, token_count, scope_start, scope_count).unwrap();
                },
                LiveValue::Use(module_path) => {
                    writeln!(f, "<Use> {}::{}", module_path, node.id).unwrap();
                }
                
            }
            index += 1;
        }
        if stack_depth != 0 {
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
    fn insert_children_from_other(&mut self, index: usize, insert_start: Option<usize>, other: &[LiveNode]) {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            other.len()
        };
        while index < other.len() {
            if other[index].value.is_open() {
                if stack_depth >= 1 {
                    self.insert(insert_point, other[index].clone());
                    insert_point += 1;
                }
                stack_depth += 1;
            }
            else if other[index].value.is_close() {
                stack_depth -= 1;
                if stack_depth >= 1 {
                    self.insert(insert_point, other[index].clone());
                    insert_point += 1;
                }
                if stack_depth == 0 {
                    return
                }
            }
            else {
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
    
    
    fn insert_children_from_self(&mut self, index: usize, insert_start: Option<usize>) -> bool {
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
            if self[index].value.is_open() {
                if stack_depth >= 1 {
                    self.insert(insert_point, self[index].clone());
                    insert_point += 1;
                }
                stack_depth += 1;
            }
            else if self[index].value.is_close() {
                stack_depth -= 1;
                if stack_depth >= 1 {
                    self.insert(insert_point, self[index].clone());
                    insert_point += 1;
                }
                if stack_depth == 0 {
                    return false
                }
            }
            else {
                self.insert(insert_point, self[index].clone());
                insert_point += 1;
                if stack_depth == 0 {
                    return false
                }
            }
            if stack_depth > MAX_CLONE_STACK_DEPTH_SAFETY {
                return true
            }
            index += 1;
        }
        false
    }
    
    
    fn insert_node_from_self(&mut self, index: usize, insert_start: Option<usize>) -> usize {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            self.len()
        };
        if insert_point < index {
            panic!();
        }
        while index < self.len() {
            self.insert(insert_point, self[index].clone());
            if self[index].value.is_open() {
                insert_point += 1;
                stack_depth += 1;
            }
            else if self[index].value.is_close() {
                stack_depth -= 1;
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            else {
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            
            index += 1;
        }
        panic!();
    }
    
    fn insert_node_from_other(&mut self, index: usize, insert_start: Option<usize>, other: &[LiveNode]) -> usize {
        let mut stack_depth = 0;
        let mut index = index;
        let mut insert_point = if let Some(insert_start) = insert_start {
            insert_start
        }
        else {
            self.len()
        };
        while index < other.len() {
            self.insert(insert_point, other[index].clone());
            if other[index].value.is_open() {
                insert_point += 1;
                stack_depth += 1;
            }
            else if other[index].value.is_close() {
                stack_depth -= 1;
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            else {
                insert_point += 1;
                if stack_depth == 0 {
                    return insert_point
                }
            }
            index += 1;
        }
        panic!();
    }
    
    fn replace_or_insert_node_by_path(&mut self, start_index:usize, path:&[LiveId], other: &[LiveNode]) {
        let mut index = start_index;
        let mut depth = 0;
        while depth < path.len(){
            match self.child_or_append_index_by_name(index, path[depth]){
                Ok(found_index)=>{
                    index = found_index;
                    if depth == path.len() - 1{ // last
                        let next_index = self.skip_node(found_index);
                        self.splice(found_index..next_index, other.iter().cloned());
                        // overwrite id
                        self[found_index].id = path[depth];
                        return
                    }
                }
                Err(append_index)=>{
                    index = append_index;
                    if depth == path.len() - 1{ // last
                        self.splice(append_index..append_index, other.iter().cloned());
                        // lets overwrite the id
                        self[append_index].id = path[depth];
                        return
                    }
                    else{ // insert an empty object
                        self.splice(append_index..append_index, live_object!{
                            [path[depth]]:{}
                        }.iter().cloned());
                    }
                }
            }
            depth += 1;
        }
    }
    
    fn push_live(&mut self, v: &[LiveNode]) {self.extend_from_slice(v)}
    
    fn push_str(&mut self, id: LiveId, v: &'static str) {self.push(LiveNode {token_id: None, id, value: LiveValue::Str(v)})}
    fn push_string(&mut self, id: LiveId, v: &str) {
        //let bytes = v.as_bytes();
        if let Some(inline_str) = InlineString::from_str(v) {
            self.push(LiveNode {token_id: None, id, value: LiveValue::InlineString(inline_str)});
        }
        else {
            self.push(LiveNode {token_id: None, id, value: LiveValue::FittedString(FittedString::from_string(v.to_string()))});
        }
    }
    
    fn push_bool(&mut self, id: LiveId, v: bool) {self.push(LiveNode {token_id: None, id, value: LiveValue::Bool(v)})}
    fn push_int(&mut self, id: LiveId, v: i64) {self.push(LiveNode {token_id: None, id, value: LiveValue::Int(v)})}
    fn push_float(&mut self, id: LiveId, v: f64) {self.push(LiveNode {token_id: None, id, value: LiveValue::Float(v)})}
    fn push_color(&mut self, id: LiveId, v: u32) {self.push(LiveNode {token_id: None, id, value: LiveValue::Color(v)})}
    fn push_vec2(&mut self, id: LiveId, v: Vec2) {self.push(LiveNode {token_id: None, id, value: LiveValue::Vec2(v)})}
    fn push_vec3(&mut self, id: LiveId, v: Vec3) {self.push(LiveNode {token_id: None, id, value: LiveValue::Vec3(v)})}
    fn push_vec4(&mut self, id: LiveId, v: Vec4) {self.push(LiveNode {token_id: None, id, value: LiveValue::Vec4(v)})}
    fn push_id(&mut self, id: LiveId, v: LiveId) {self.push(LiveNode {token_id: None, id, value: LiveValue::Id(v)})}
    
    fn push_bare_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId) {self.push(LiveNode {token_id: None, id, value: LiveValue::BareEnum {base, variant}})}
    fn open_tuple_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId) {self.push(LiveNode {token_id: None, id, value: LiveValue::TupleEnum {base, variant}})}
    fn open_named_enum(&mut self, id: LiveId, base: LiveId, variant: LiveId) {self.push(LiveNode {token_id: None, id, value: LiveValue::NamedEnum {base, variant}})}
    fn open_object(&mut self, id: LiveId) {self.push(LiveNode {token_id: None, id, value: LiveValue::Object})}
    fn open_clone(&mut self, id: LiveId, clone: LiveId) {self.push(LiveNode {token_id: None, id, value: LiveValue::Clone(clone)})}
    fn open_array(&mut self, id: LiveId) {self.push(LiveNode {token_id: None, id, value: LiveValue::Array})}
    fn close(&mut self) {self.push(LiveNode {token_id: None, id: LiveId(0), value: LiveValue::Close})}
    fn open(&mut self) {self.push(LiveNode {token_id: None, id: LiveId(0), value: LiveValue::Object})}
}
const MAX_CLONE_STACK_DEPTH_SAFETY: usize = 100;

pub struct LiveNodeReader<'a> {
    eot: bool,
    depth: usize,
    index: usize,
    nodes: &'a[LiveNode]
}

impl<'a> LiveNodeReader<'a> {
    pub fn new(index: usize, nodes: &'a[LiveNode]) -> Self {
        
        Self {
            eot: false,
            depth: 0,
            index,
            nodes
        }
    }
    
    pub fn index_option(&self, index: Option<usize>, depth_change: isize) -> Option<Self> {
        if self.eot{panic!();}
        if let Some(index) = index {
            Some(Self {
                eot: self.eot,
                depth: (self.depth as isize + depth_change) as usize,
                index: index,
                nodes: self.nodes
            })
        }
        else {
            None
        }
    }
    
    pub fn value(&self) -> &LiveValue {
        if self.eot{panic!();}
        &self.nodes[self.index].value
    }
    
    pub fn node(&self) -> &LiveNode {
        if self.eot{panic!();}
        &self.nodes[self.index]
    }
    
    pub fn parent(&self) -> Option<Self> {self.index_option(self.nodes.parent(self.index), -1)}
    pub fn append_child_index(&self) -> usize {self.nodes.append_child_index(self.index)}
    pub fn first_child(&self) -> Option<Self> {self.index_option(self.nodes.first_child(self.index), 1)}
    pub fn last_child(&self) -> Option<Self> {self.index_option(self.nodes.last_child(self.index), 1)}
    pub fn next_child(&self) -> Option<Self> {self.index_option(self.nodes.next_child(self.index), 0)}

    pub fn node_slice(&self) -> &[LiveNode]{
        if self.eot{panic!()}
        self.nodes.node_slice(self.index)
    }
    
    pub fn children_slice(&self) -> &[LiveNode]{
        if self.eot{panic!()}
        self.nodes.children_slice(self.index)
    }

    pub fn child_by_number(&self, child_number: usize) -> Option<Self> {
        self.index_option(self.nodes.child_by_number(self.index, child_number), 1)
    }
    
    pub fn child_by_name(&self, name: LiveId)->Option<Self>{
        self.index_option(self.nodes.child_by_name(self.index, name), 1)
    }

    fn child_by_path(&self, path: &[LiveId]) ->Option<Self>{
        self.index_option(self.nodes.child_by_path(self.index, path), 1)
    }
    
    pub fn scope_up_by_name(&self, name: LiveId) -> Option<Self>{
        self.index_option(self.nodes.scope_up_by_name(self.index, name), 0)
    }
    
    pub fn count_children(&self)->usize{ self.nodes.count_children(self.index)}
    pub fn clone_child(&self, out_vec: &mut Vec<LiveNode>){
        if self.eot{panic!();}
        self.nodes.clone_child(self.index, out_vec)
    }

    pub fn to_string(&self, max_depth: usize)->String{
        if self.eot{panic!();}
        self.nodes.to_string(self.index, max_depth)
    }
    
    pub fn skip(&mut self){
        if self.eot{panic!();}
        self.index = self.nodes.skip_node(self.index);
        // check eot
        if self.nodes[self.index].value.is_close(){ // standing on a close node
            if self.depth == 1{
                self.eot = true;
                self.index += 1;
            }
        }
    }
    
    pub fn walk(&mut self){
        if self.eot{panic!();}
        if self.nodes[self.index].value.is_open() {
            self.depth += 1;
        }
        else if self.nodes[self.index].value.is_close() {
            if self.depth == 0{panic!()}
            self.depth -= 1;
            if self.depth == 0 {
                self.eot = true;
            }
        }
        self.index += 1;
    }
    
    pub fn is_eot(&self)->bool{
        return self.eot
    }
    
    pub fn id(&self)->LiveId{
        self.nodes[self.index].id
    }
    
    pub fn index(&self)->usize{
        self.index
    }
    
    pub fn depth(&self)->usize{
        self.depth
    }
    
    pub fn nodes(&self)->&[LiveNode]{
        self.nodes
    }
    
}

impl<'a> Deref for LiveNodeReader<'a> {
    type Target = LiveValue;
    fn deref(&self) -> &Self::Target {&self.nodes[self.index].value}
}
