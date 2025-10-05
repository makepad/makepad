use crate::id::Id;
use crate::opcode::*;
use std::fmt;

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct Value(u64);

impl Default for Value{
    fn default()->Self{
        Self::NIL
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ObjectPtr{
    pub index: u32    
}

impl From<ObjectPtr> for Value{
    fn from(v:ObjectPtr) -> Self{
        Value::from_object(v)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct StringPtr{
    pub index: u32    
}

pub struct LocalPtr{
    pub rel: u16,
    pub index: u16
}

impl From<StringPtr> for Value{
    fn from(v:StringPtr) -> Self{
        Value::from_string(v)
    }
}

impl From<f64> for Value{
    fn from(v:f64) -> Self{
        Value::from_f64(v)
    }
}

impl From<Id> for Value{
    fn from(v:Id) -> Self{
        Value::from_id(v)
    }
}

impl From<Opcode> for Value{
    fn from(v:Opcode) -> Self{
        Value::from_opcode(v)
    }
}
// NaN box value

impl Value{
    pub const TYPE_MASK: u64 = 0xFFFF_FF00_0000_0000;
        
    pub const TYPE_NAN: u64 = 0xFFFF_0100_0000_0000;
    pub const NAN: Value = Value( Self::TYPE_NAN);
    
    pub const TYPE_BOOL: u64 = 0xFFFF_0200_0000_0000;
    pub const FALSE: Value = Value( Self::TYPE_BOOL | 0x0000_0000);
    pub const TRUE: Value = Value(Self::TYPE_BOOL | 0x0000_0001);
    
    pub const TYPE_NIL: u64 = 0xFFFF_0300_0000_0000;
    pub const NIL: Value = Value(Self::TYPE_NIL);
    pub const TYPE_COLOR: u64 = 0xFFFF_0400_0000_0000;
    pub const TYPE_STRING: u64 = 0xFFFF_0500_0000_0000;
    pub const TYPE_OBJECT: u64 = 0xFFFF_0600_0000_0000;
    pub const TYPE_LOCAL: u64 = 0xFFFF_0700_0000_0000;
    
    pub const TYPE_INLINE_STRING_0: u64 = 0xFFFF_0800_0000_0000;
    pub const TYPE_INLINE_STRING_1: u64 = 0xFFFF_0900_0000_0000;
    pub const TYPE_INLINE_STRING_2: u64 = 0xFFFF_0A00_0000_0000;
    pub const TYPE_INLINE_STRING_3: u64 = 0xFFFF_0B00_0000_0000;
    pub const TYPE_INLINE_STRING_4: u64 = 0xFFFF_0C00_0000_0000;
    pub const TYPE_INLINE_STRING_5: u64 = 0xFFFF_0D00_0000_0000;
    pub const TYPE_INLINE_STRING_END: u64 = 0xFFFF_0E00_0000_0000;
    
    pub const TYPE_ID: u64 = 0xFFFF_8000_0000_0000;
    
    pub const ESCAPED_ID: u64 = 0x0000_4000_0000_0000;
    
    // opcodes
    pub const TYPE_OPCODE: u64 = 0xFFFF_1000_0000_0000;
    
    pub const fn from_opcode(op:Opcode)->Self{ Self(Self::TYPE_OPCODE | (op.0 as u64)<<32)}
    
    pub const fn from_opcode_args(op:Opcode, args:OpcodeArgs)->Self{ Self(Self::TYPE_OPCODE | (op.0 as u64)<<32 | (args.0 as u64))}
        
    // TODO: make this behave like javascript as much as is sensible
    
    pub const fn from_f64(val:f64)->Self{
        if val.is_nan(){
            Self::NAN
        }
        else{
            Self(val.to_bits())
        }
    }
    
    pub fn from_object(ptr: ObjectPtr)->Self{
         Self(ptr.index as u64 | Self::TYPE_OBJECT)
    }
    
    pub fn from_local(local:LocalPtr)->Self{
        Self((local.rel as u64) << 16 | (local.index as u64) << 16 | Self::TYPE_LOCAL)
    }
            
    pub const fn from_bool(val: bool)->Self{
        if val{Self::TRUE}
        else{Self::FALSE}
    }
    
    pub const fn from_color(val: u32)->Self{
        Self(val as u64|Self::TYPE_COLOR)
    }
    
    pub const fn from_id(val: Id)->Self{
        Self(val.0|Self::TYPE_ID)
    }
    
    pub const fn from_escaped_id(val: Id)->Self{
        Self(val.0|Self::TYPE_ID|Self::ESCAPED_ID)
    }
        
    pub fn from_string(ptr: StringPtr)->Self{
         Self(ptr.index as u64 | Self::TYPE_STRING)
    }
    
    pub fn from_inline_string(str: &str)->Option<Self>{
        let bytes = str.as_bytes();
        if bytes.len()>5{
            return None
        }
        if bytes.len() == 0{
            Some(Self(Self::TYPE_INLINE_STRING_0))
        }
        else if bytes.len() == 1{
            Some(Self(Self::TYPE_INLINE_STRING_1 | bytes[0] as u64))
        }
        else if bytes.len() == 2{
            Some(Self(Self::TYPE_INLINE_STRING_2 | bytes[0] as u64 | ((bytes[1] as u64)<<8)))
        }
        else if bytes.len() == 3{
            Some(Self(Self::TYPE_INLINE_STRING_3 | bytes[0] as u64 | ((bytes[1] as u64)<<8) | ((bytes[2] as u64)<<16)))
        }
        else if bytes.len() == 4{
            Some(Self(Self::TYPE_INLINE_STRING_4 | bytes[0] as u64 | ((bytes[1] as u64)<<8) | ((bytes[2] as u64)<<16) | ((bytes[3] as u64)<<24)))
        }
        else{
            Some(Self(Self::TYPE_INLINE_STRING_5 | bytes[0] as u64 | ((bytes[1] as u64)<<8) | ((bytes[2] as u64)<<16) | ((bytes[3] as u64)<<24) | ((bytes[4] as u64)<<32)))
        }
    }
    
    pub fn as_inline_string<R,F:FnOnce(&str)->R>(&self, f:F)->Option<R>{
        if !self.is_inline_string(){
            return None
        }
        if self.0 < Self::TYPE_INLINE_STRING_1{
            return Some(f(""))
        }
        else if self.0 < Self::TYPE_INLINE_STRING_2{
            return Some(f(unsafe{std::str::from_utf8_unchecked(&[(self.0 & 0xff) as u8])}))
        }
        else if self.0 < Self::TYPE_INLINE_STRING_3{
            return Some(f(unsafe{std::str::from_utf8_unchecked(&[(self.0 & 0xff) as u8, ((self.0>>8) & 0xff) as u8])}))
        }
        else if self.0 < Self::TYPE_INLINE_STRING_4{
            return Some(f(unsafe{std::str::from_utf8_unchecked(&[(self.0 & 0xff) as u8, ((self.0>>8) & 0xff) as u8, ((self.0>>16) & 0xff) as u8])}))
        }
        else if self.0 < Self::TYPE_INLINE_STRING_5{
            return Some(f(unsafe{std::str::from_utf8_unchecked(&[(self.0 & 0xff) as u8, ((self.0>>8) & 0xff) as u8, ((self.0>>16) & 0xff) as u8, ((self.0>>24) & 0xff) as u8])}))
        }
        else{
            return Some(f(unsafe{std::str::from_utf8_unchecked(&[(self.0 & 0xff) as u8, ((self.0>>8) & 0xff) as u8, ((self.0>>16) & 0xff) as u8, ((self.0>>24) & 0xff) as u8, ((self.0>>32) & 0xff) as u8])}))
        }
    }
    
    pub fn inline_string_not_empty(&self)->bool{
        self.0 >= Self::TYPE_INLINE_STRING_1  && self.0 <= Self::TYPE_INLINE_STRING_END
    }
    
    pub fn as_bool(&self)->Option<bool>{
        if self.is_bool(){
            return Some(*self == Self::TRUE)
        }
        None
    }
    
    pub fn as_local(&self)->Option<LocalPtr>{
        if self.is_local(){
            Some(LocalPtr{
                rel: ((self.0 >> 16)&0xffff) as u16,
                index: ((self.0)&0xffff) as u16
            })
        }
        else{
            None
        }
    }
        
    pub fn as_f64(&self)->Option<f64>{
        if self.is_f64(){
            return Some(f64::from_bits(self.0))
        }
        None    
    }
        
    pub fn as_id(&self)->Option<Id>{
        if self.is_id(){
            return Some(Id(self.0&0x0000_3fff_ffff_ffff))
        }
        None
    }
    
    pub fn is_inline_string(&self)->bool{
        self.0 >= Self::TYPE_INLINE_STRING_0  && self.0 < Self::TYPE_INLINE_STRING_END
    }
    
    pub fn is_escaped_id(&self)->bool{
        self.0 >= Self::TYPE_ID | Self::ESCAPED_ID
    }
        
        
    pub fn as_object(&self)->Option<ObjectPtr>{
        if self.is_object(){
            return Some(ObjectPtr{
                index: (self.0 & 0xffff_ffff) as u32
            })
        }
        None
    }
        
    pub fn as_opcode(&self)->Option<(Opcode,OpcodeArgs)>{
        if self.is_opcode(){
            return Some((Opcode(((self.0>>32) & 0xff) as u8),OpcodeArgs((self.0 & 0xffff_ffff) as u32)))
        }
        None
    }
    
    pub fn is_assign_opcode(&self)->bool{
        if self.is_opcode(){
            let code = Opcode(((self.0>>32) & 0xff) as u8);
            return code >= Opcode::ASSIGN_FIRST && code <= Opcode::ASSIGN_LAST
        }
        false
    }
    
    pub fn is_let_opcode(&self)->bool{
        if self.is_opcode(){
            let code = Opcode(((self.0>>32) & 0xff) as u8);
            return code >= Opcode::LET_FIRST && code <= Opcode::LET_LAST
        }
        false
    }
    
    pub fn set_opcode_arg(&mut self, args:OpcodeArgs){
        if self.is_opcode(){
            self.0 |= args.0 as u64;
        }
    }
    
    pub fn set_opcode_is_statement(&mut self){
        if self.is_opcode(){
            self.0 |= OpcodeArgs::STATEMENT_FLAG as u64;
        }
    }
        
    pub fn as_string(&self)->Option<StringPtr>{
        if self.is_string(){
            return Some(StringPtr{
                index: (self.0 & 0xffff_ffff) as u32
            })
        }
        None
    }
        
    pub fn as_color(&self)->Option<u32>{
        if self.is_color(){
            return Some((self.0&0xffff_ffff) as u32)
        }
        None
    }
    
    pub fn is_f64(&self)->bool{
        self.0 <= Self::TYPE_NAN
    }
    
    pub fn is_bool(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_BOOL
    }
    
    pub fn is_nil(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_NIL
    }
    
    pub fn is_color(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_COLOR
    }
    
    pub fn is_local(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_LOCAL
    }
    
    pub fn is_id(&self)->bool{
        self.0 >= Self::TYPE_ID
    }
    
    pub fn is_opcode(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_OPCODE
    }
    
    pub fn is_string(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_STRING
    }
    
    pub fn is_object(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_OBJECT
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}


impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if let Some(v) = self.as_f64(){
            return write!(f, "{}", v)
        }
        if let Some(v) = self.as_id(){
            return write!(f, "{}", v)
        }
        if let Some(v) = self.as_bool(){
            return write!(f, "{}", v)
        }
        if let Some(_) = self.as_string(){
            return write!(f, "[String]")
        }
        if let Some(r) = self.as_inline_string(|s|{
                write!(f, "{s}")
            }){
            return r;
        }
        if let Some(ptr) = self.as_object(){
            return write!(f, "[Object:{}]",ptr.index)
        }
        if self.is_nil(){
            return write!(f, "nil")
        }
        if let Some((opcode, args)) = self.as_opcode(){
            return write!(f, "{opcode}{args}")
        }
        write!(f, "?{:08x}", self.0)
    }
}