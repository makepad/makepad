use {
    crate::{
        liveid::{LiveId, LiveModuleId, LivePtr},
        token::TokenId,
        math::{Vec2, Vec3, Vec4},
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
    ExprCall{ident:LiveId, args:usize},
    // enum thing
    BareEnum {base: LiveId, variant: LiveId},
    // tree items
    Array,
    Expr,
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
    },
    Use (LiveModuleId),
}

impl LiveNode{
    pub fn empty()->Self{
        Self{
            origin:LiveNodeOrigin::empty(),
            id: LiveId(0),
            value: LiveValue::None
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct LiveNodeOrigin(u64);

impl LiveNodeOrigin{
    pub fn empty()->Self{
        Self(0)
    }
    
    pub fn from_token_id(token_id:TokenId)->Self{
        Self(0x8000_0000_0000_0000 | token_id.to_bits() as u64)
    }
    /*
    pub fn unwrap_token_id(&self)->TokenId{
        if self.0&0x8000_0000_0000_0000 != 0{
            return TokenId::from_bits((self.0&0xffff_ffff) as u32)
        }
        else{
            panic!()
        }
    }*/
    
    pub fn token_id(&self)->Option<TokenId>{
        if self.0&0x8000_0000_0000_0000 != 0{
            return Some(TokenId::from_bits((self.0&0xffff_ffff) as u32))
        }
        else{
            None
        }
    }
    
    pub fn set_node_index(&mut self, index:usize){
        self.0 = (((index as u64)&0x3fff_ffff) << 32) | 0x4000_0000_0000_0000 | ((self.0) & 0x8000_0000_ffff_ffff);
    }
    
    pub fn node_index(&self)->Option<usize>{
        if self.0&0x4000_0000_0000_0000 != 0{
            return Some(((self.0>>32)&0x3fff_ffff) as usize)
        }
        else{
            None
        }
    }
    
    pub fn has_node_index(&mut self)->bool{
        self.0&0x4000_0000_0000_0000 != 0
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Copy, Hash)]
pub struct LiveType(pub core::any::TypeId);

#[derive(Clone, Debug)]
pub enum LiveTypeKind {
    Class, 
    Enum,
    Object,
    Primitive,
    DrawVars,
}

#[derive(Clone, Debug)]
pub struct LiveTypeInfo {
    pub live_type: LiveType,
    pub type_name: LiveId,
    pub module_id: LiveModuleId,
    pub kind: LiveTypeKind,
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
            Self::Expr |
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
    pub fn is_expr(&self) -> bool {
        match self {
            Self::Expr => true,
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
            Self::ExprCall{..} => 16,
            
            Self::BareEnum {..} => 17,
            Self::Array => 18,
            Self::Expr => 19,
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
