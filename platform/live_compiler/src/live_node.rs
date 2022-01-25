use {
    std::{
        fmt,
        ops::Deref,
        ops::DerefMut,
    },
    crate::{
        makepad_math::{
            Vec2,
            Vec3,
            Vec4
        },
        makepad_live_tokenizer::{LiveId},
        live_ptr::{LiveModuleId, LivePtr},
        live_token::{LiveToken, LiveTokenId},
    }
};

#[derive(Clone, Debug, PartialEq)]
pub struct LiveNode { // 40 bytes. Don't really see ways to compress
    pub origin: LiveNodeOrigin,
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
    ExprBinOp(LiveBinOp),
    ExprUnOp(LiveUnOp),
    ExprMember(LiveId),
    ExprCall {ident: LiveId, args: usize},
    // enum thing
    BareEnum {base: LiveId, variant: LiveId},
    // tree items
    Array,
    Expr{expand_index: Option<u32>},
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
        expand_index: Option<u32>
    },
    Use (LiveModuleId),
}

impl LiveValue {
    pub fn update_from_live_token(&mut self, token: &LiveToken) -> bool {
        match self {
            Self::DocumentString {string_start, string_count} => {
                if let LiveToken::String {index, len} = token {
                    *string_start = *index as usize;
                    *string_count = *len as usize;
                    return true
                }
            },
            Self::Color(o) => if let LiveToken::Color(i) = token {
                *o = *i;
                return true
            },
            Self::Bool(o) => if let LiveToken::Bool(i) = token {
                *o = *i;
                return true
            },
            Self::Int(o) => {
                if let LiveToken::Int(i) = token {
                    *o = *i;
                    return true
                }
                if let LiveToken::Float(v) = token {
                    *self = LiveValue::Float(*v);
                    return true
                }
            }
            Self::Float(o) => {
                if let LiveToken::Float(i) = token {
                    *o = *i;
                    return true
                }
                if let LiveToken::Int(v) = token {
                    *self = LiveValue::Int(*v);
                    return true
                }
            }
            _ => ()
            
        }
        false
    }
}

impl LiveNode {
    
    pub fn empty() -> Self {
        Self {
            origin: LiveNodeOrigin::empty(),
            id: LiveId(0),
            value: LiveValue::None
        }
    }

    pub fn is_token_id_inside_dsl(&self, other_token: LiveTokenId) -> bool {
        if let Some(token_id) = self.origin.token_id(){
            if token_id.file_id() != other_token.file_id() {
                return false
            }
        }
        else{
            return false;
        }
        match &self.value {
            LiveValue::DSL {token_start, token_count, ..} => {
                let token_index = other_token.token_index();
                token_index as u32 >= *token_start && (token_index as u32) < token_start + token_count
            }
            _ => false
        }
    }
    
}

#[derive(Copy, Clone, PartialEq)]
pub struct LiveNodeOrigin(u64);

impl fmt::Debug for LiveNodeOrigin {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "token_id:{:?} first_def:{:?} edit_info:{:?} non_unique:{}", self.token_id(), self.first_def(), self.edit_info(), self.id_non_unique())
    }
}

// this layout can be reshuffled and just be made bigger.
// However it keeps the LiveNode size at 40 bytes which is nice for now.

// 10 bit file id (1024)
// 18 bit token id (262k tokens, avg tokensize: 5 = 1.25 megs of code used to find original token of property

// 10 bit first def file_id
// 18 bit first def token_id

// 7 bits (128) edit_info index
// 1 bit 'id_is_nonunique'

// ok if we are a DSL node then what else do we need. we need a node index pointer.

impl LiveNodeOrigin {
    pub fn empty() -> Self {
        Self (0)
    }
    
    pub fn from_token_id(token_id: LiveTokenId) -> Self {
        Self( (token_id.to_bits() as u64) |  ((token_id.to_bits() as u64)<<28) )
    }
    
    pub fn token_id(&self) -> Option<LiveTokenId> {
        LiveTokenId::from_bits((self.0 & 0x0fff_ffff) as u32)
    }

    pub fn set_first_def(&mut self, token_id:Option<LiveTokenId>)->&mut Self{
        if let Some(token_id) = token_id{
            self.0 = (self.0 &0xff00_0000_0fff_ffff) |  ((token_id.to_bits() as u64)<<28);
        }
        self
    }

    pub fn first_def(&self)->Option<LiveTokenId>{
        LiveTokenId::from_bits(((self.0>>28) & 0x0fff_ffff) as u32)
    }
    
    pub fn set_edit_info(&mut self, edit_info: Option<LiveEditInfo>)->&mut Self{
        if let Some(edit_info) = edit_info {
            self.0 = (self.0 & 0x80FF_FFFF_FFFF_FFFF) | ((edit_info.to_bits() as u64) << 56);
        }
        self
    }
    
    pub fn with_edit_info(mut self, edit_info: Option<LiveEditInfo>)->Self{
        self.set_edit_info(edit_info);
        self
    }
    
    pub fn edit_info(&self) -> Option<LiveEditInfo> {
        LiveEditInfo::from_bits(((self.0 & 0x7f00_0000_0000_0000) >> 56) as u32)
    }
    
    pub fn set_node_has_prefix(&mut self, node_has_prefix: bool) {
        if node_has_prefix {
            self.0 |= 0x4000_0000_0000_0000;
        }
    }
    
    pub fn with_node_has_prefix(mut self, node_has_prefix: bool) -> Self {
        self.set_node_has_prefix(node_has_prefix);
        self
    }
    
    pub fn node_has_prefix(&self) -> bool {
        self.0 & 0x4000_0000_0000_0000 != 0
    }
    
    pub fn with_id_non_unique(mut self, non_unique: bool) -> Self {
        if non_unique {
            self.0 |= 0x8000_0000_0000_0000;
        }
        self
    }
    
    pub fn id_non_unique(&self) -> bool {
        self.0 & 0x8000_0000_0000_0000 != 0
    }
    
    pub fn inherit_origin(&mut self, origin:Self){
        let edit_info = origin.edit_info();
        let first_def = origin.first_def();
        let node_has_prefix = origin.node_has_prefix();
        self.set_edit_info(edit_info);
        self.set_first_def(first_def);
        self.set_node_has_prefix(node_has_prefix);
    }
}

pub struct LiveEditInfo(u32);

impl fmt::Debug for LiveEditInfo {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{:?}", self.edit_info_index())
    }
}

impl LiveEditInfo {
    pub fn new(edit_info_index: usize) -> Self {
        if edit_info_index & 0xf != 0 || edit_info_index > 0x3e0 {
            panic!();
        }
        LiveEditInfo(((edit_info_index as u32)>>4)+1)
    }
    
    pub fn edit_info_index(&self) -> usize {
        (((self.0) as usize - 1) << 4) & 0x3f0
    }
    
    pub fn to_bits(&self) -> u32 {self.0}
    pub fn from_bits(v: u32) -> Option<Self> {
        if (v & 0xFFFF_FF00) != 0 {
            panic!();
        }
        if v == 0 {
            return None
        }
        return Some(Self (v))
    }
}

pub type LiveType = std::any::TypeId;

#[derive(Clone, Debug)]
pub struct LiveTypeInfo {
    pub live_type: LiveType,
    pub type_name: LiveId,
    pub module_id: LiveModuleId,
    pub fields: Vec<LiveTypeField>
}

#[derive(Clone, Debug)]
pub struct LiveTypeField {
    pub id: LiveId,
    pub live_type_info: LiveTypeInfo,
    pub live_field_kind: LiveFieldKind
}

#[derive(Copy, Clone, Debug)]
pub enum LiveFieldKind {
    Calc,
    Live,
    LiveOption
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LiveBinOp {
    Or,
    And,
    Eq,
    Ne,
    Lt,
    Le,
    Gt,
    Ge,
    Add,
    Sub,
    Mul,
    Div,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum LiveUnOp {
    Not,
    Neg,
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
            Self::Expr{..} |
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
    
    pub fn is_expr(&self) -> bool {
        match self {
            Self::Expr{..} => true,
            _ => false
        }
    }
    
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
    pub fn set_dsl_expand_index_if_none(&mut self, index:usize) {
        match self {
            Self::DSL {expand_index,..} => if expand_index.is_none(){
                *expand_index = Some(index as u32)
            },
            _ => ()
        }
    }
    
    pub fn set_expr_expand_index_if_none(&mut self, index:usize) {
        match self {
            Self::Expr {expand_index,..} => if expand_index.is_none(){
                *expand_index = Some(index as u32)
            },
            _ => ()
        }
    }
    
    pub fn get_expr_expand_index(&self)->Option<u32>{
        match self {
            Self::Expr {expand_index,..} => *expand_index,
            _ => None
        }
    }
    
    pub fn is_id(&self)->bool{
        match self{
            Self::Id(_)=>true,
            _=>false
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
            Self::Vec4(_) => true,
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
    
    pub fn is_number_type(&self) -> bool {
        match self {
            Self::Int(_) |
            Self::Float(_) => true,
            _ => false
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(v) => Some(*v),
            Self::Int(v) => Some(*v as f64),
            _ => None
        }
    }
    
    pub fn as_vec2(&self) -> Option<Vec2> {
        match self {
            Self::Vec2(v) => Some(*v),
            _ => None
        }
    }    
    pub fn as_vec3(&self) -> Option<Vec3> {
        match self {
            Self::Vec3(v) => Some(*v),
            _ => None
        }
    }    

    pub fn as_vec4(&self) -> Option<Vec4> {
        match self {
            Self::Vec4(v) => Some(*v),
            Self::Color(c) => Some(Vec4::from_u32(*c)),
            _ => None
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
            Self::ExprBinOp(_) => 13,
            Self::ExprUnOp(_) => 14,
            Self::ExprMember(_) => 15,
            Self::ExprCall {..} => 16,
            
            Self::BareEnum {..} => 17,
            Self::Array => 18,
            Self::Expr{..} => 19,
            Self::TupleEnum {..} => 20,
            Self::NamedEnum {..} => 21,
            Self::Object => 22,
            Self::Clone {..} => 23,
            Self::Class {..} => 24,
            Self::Close => 25,
            
            Self::DSL {..} => 26,
            Self::Use {..} => 27
        }
    }
}

impl Deref for LiveNode {
    type Target = LiveValue;
    fn deref(&self) -> &Self::Target {&self.value}
}

impl DerefMut for LiveNode {
    fn deref_mut(&mut self) -> &mut Self::Target {&mut self.value}
}

