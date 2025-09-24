use crate::id::Id;

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct Value(u64);
use std::fmt;
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
    
    pub const TYPE_ID: u64 = 0xFFFF_8000_0000_0000;
    
        
    // opcodes
    pub const TYPE_OPCODE: u64 = 0xFFFF_0500_0000_0000;
    pub const OP_PROP: Value = Value(Self::TYPE_OPCODE | 0x0000_0001);
    pub const OP_ADD: Value = Value(Self::TYPE_OPCODE | 0x0000_0002);
    
    pub const TYPE_STRING: u64 = 0xFFFF_0500_0000_0000;
    pub const TYPE_STRING_MASK: u64 = 0xFFFF_FFFF_0000_0000;
    pub const TYPE_HEAP_STRING: u64 = 0xFFFF_0501_0000_0000;
    pub const TYPE_STACK_STRING: u64 = 0xFFFF_0502_0000_0000;
    pub const TYPE_STATIC_STRING: u64 = 0xFFFF_0503_0000_0000;
    
    // TODO: make this behave like javascript as much as is sensible
    
    pub fn from_f64(val:f64)->Self{
        if val.is_nan(){
            Self::NAN
        }
        else{
            Self(val.to_bits())
        }
    }
    
    pub fn from_bool(val: bool)->Self{
        if val{Self::TRUE}
        else{Self::FALSE}
    }
    
    pub fn from_color(val: u32)->Self{
        Self(val as u64|Self::TYPE_COLOR)
    }
    
    pub fn from_id(val: Id)->Self{
        Self(val.0|Self::TYPE_ID)
    }
    
    pub fn from_static_string(index: usize)->Self{
        Self((index as u64 & 0xffff_ffff)|Self::TYPE_STATIC_STRING)
    }
    
    pub fn to_bool(&self)->bool{
        if self.is_bool(){
            return *self == Self::TRUE
        }
        self.to_f64() != 0.0
    }
    
    pub fn to_f64(&self)->f64{
        if self.is_f64(){
            return f64::from_bits(self.0)
        }
        if *self == Self::TRUE{
            return 1.0
        }
        0.0
    }
    
    pub fn to_id(&self)->Id{
        if self.is_id(){
            return Id(self.0&0x0000_7fff_ffff_ffff)
        }
        Id(0)
    }
    
    pub fn to_color(&self)->u32{
        if self.is_color(){
            return (self.0&0xffff_ffff) as u32
        }
        0
    }
    
    pub fn as_bool(&self)->Option<bool>{
        if self.is_bool(){
            return Some(*self == Self::TRUE)
        }
        None
    }
        
    pub fn as_f64(&self)->Option<f64>{
        if self.is_f64(){
            return Some(f64::from_bits(self.0))
        }
        None    
    }
        
    pub fn as_id(&self)->Option<Id>{
        if self.is_id(){
            return Some(Id(self.0&0x0000_7fff_ffff_ffff))
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
    
    pub fn is_id(&self)->bool{
        self.0 >= Self::TYPE_ID
    }
    
    pub fn is_opcode(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_OPCODE
    }
    
    pub fn is_string(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_STRING
    }
}

impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        if self.is_f64(){
            return write!(f, "{}", self.to_f64())
        }
        if self.is_id(){
            return write!(f, "{}", self.to_id())
        }
        if self.is_bool(){
            return write!(f, "{}", self.to_bool())
        }
        if self.is_nil(){
            return write!(f, "nil")
        }
        if self.is_opcode(){
            match *self{
                Self::OP_PROP=>return write!(f, "OP_PROP"),
                Self::OP_ADD=>return write!(f, "OP_ADD"),
                _=>return write!(f, "OP?")
            }
        }
        write!(f, "?{:08x}", self.0)
    }
}