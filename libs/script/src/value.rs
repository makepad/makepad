use crate::id::Id;
use crate::string_table::StringIndex;

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
    pub const OP_NOP: Value = Value(Self::TYPE_OPCODE | 0);
    pub const OP_PROP: Value = Value(Self::TYPE_OPCODE | 1);
    pub const OP_NOT: Value = Value(Self::TYPE_OPCODE | 2);
    pub const OP_NEG: Value = Value(Self::TYPE_OPCODE | 3);
    pub const OP_MUL: Value = Value(Self::TYPE_OPCODE | 4);
    pub const OP_DIV: Value = Value(Self::TYPE_OPCODE | 5);
    pub const OP_MOD: Value = Value(Self::TYPE_OPCODE | 6);
    pub const OP_ADD: Value = Value(Self::TYPE_OPCODE | 7);
    pub const OP_SUB: Value = Value(Self::TYPE_OPCODE | 8);
    pub const OP_SHL: Value = Value(Self::TYPE_OPCODE | 9);
    pub const OP_SHR: Value = Value(Self::TYPE_OPCODE | 10);
    pub const OP_AND: Value = Value(Self::TYPE_OPCODE | 11);
    pub const OP_OR: Value = Value(Self::TYPE_OPCODE | 12);
    pub const OP_XOR: Value = Value(Self::TYPE_OPCODE | 13);
    
    pub const OP_EQ: Value = Value(Self::TYPE_OPCODE | 14);
    pub const OP_NEQ: Value = Value(Self::TYPE_OPCODE | 15);
    pub const OP_LT: Value = Value(Self::TYPE_OPCODE | 16);
    pub const OP_GT: Value = Value(Self::TYPE_OPCODE | 17);
    pub const OP_LEQ: Value = Value(Self::TYPE_OPCODE | 18);
    pub const OP_GEQ: Value = Value(Self::TYPE_OPCODE | 19);
    pub const OP_LOGIC_AND: Value = Value(Self::TYPE_OPCODE | 20);
    pub const OP_LOGIC_OR: Value = Value(Self::TYPE_OPCODE | 21);
    
    pub const OP_ASSIGN: Value = Value(Self::TYPE_OPCODE | 22);
    pub const OP_ASSIGN_ADD: Value = Value(Self::TYPE_OPCODE | 23);
    pub const OP_ASSIGN_SUB: Value = Value(Self::TYPE_OPCODE | 24);
    pub const OP_ASSIGN_MUL: Value = Value(Self::TYPE_OPCODE | 25);
    pub const OP_ASSIGN_DIV: Value = Value(Self::TYPE_OPCODE | 26);
    pub const OP_ASSIGN_MOD: Value = Value(Self::TYPE_OPCODE | 27);
    pub const OP_ASSIGN_AND: Value = Value(Self::TYPE_OPCODE | 28);
    pub const OP_ASSIGN_OR: Value = Value(Self::TYPE_OPCODE | 29);
    pub const OP_ASSIGN_XOR: Value = Value(Self::TYPE_OPCODE | 30);
    pub const OP_ASSIGN_SHL: Value = Value(Self::TYPE_OPCODE | 31);
    pub const OP_ASSIGN_SHR: Value = Value(Self::TYPE_OPCODE | 32);
    pub const OP_ASSIGN_IFNIL: Value = Value(Self::TYPE_OPCODE | 33);
    pub const OP_BEGIN_PROTO: Value = Value(Self::TYPE_OPCODE | 34);
    pub const OP_END_PROTO: Value = Value(Self::TYPE_OPCODE | 35);
    pub const OP_BEGIN_BARE: Value = Value(Self::TYPE_OPCODE | 36);
    pub const OP_END_BARE: Value = Value(Self::TYPE_OPCODE | 37);
    pub const OP_BEGIN_CALL: Value = Value(Self::TYPE_OPCODE | 38);
    pub const OP_END_CALL: Value = Value(Self::TYPE_OPCODE | 39);
    pub const OP_BEGIN_FRAG: Value = Value(Self::TYPE_OPCODE | 40);
    pub const OP_END_FRAG: Value = Value(Self::TYPE_OPCODE | 41);
    pub const OP_BEGIN_FN_ARGS: Value = Value(Self::TYPE_OPCODE | 42);
    pub const OP_BEGIN_FN_BODY: Value = Value(Self::TYPE_OPCODE | 43);
    pub const OP_END_FN_BODY: Value = Value(Self::TYPE_OPCODE | 44);
    
    pub const OP_FIELD: Value = Value(Self::TYPE_OPCODE | 42);
    pub const OP_ARRAY_INDEX: Value = Value(Self::TYPE_OPCODE | 43);
    
    pub const TYPE_STRING: u64 = 0xFFFF_0500_0000_0000;
    pub const TYPE_STRING_MASK: u64 = 0xFFFF_FFFF_0000_0000;
    pub const TYPE_HEAP_STRING: u64 = 0xFFFF_0501_0000_0000;
    pub const TYPE_STACK_STRING: u64 = 0xFFFF_0502_0000_0000;
    pub const TYPE_STATIC_STRING: u64 = 0xFFFF_0503_0000_0000;
    
    
    pub const TYPE_OBJECT: u64 = 0xFFFF_0600_0000_0000;
    
    // TODO: make this behave like javascript as much as is sensible
    
    pub fn from_f64(val:f64)->Self{
        if val.is_nan(){
            Self::NAN
        }
        else{
            Self(val.to_bits())
        }
    }
    
    pub fn from_object(val: usize)->Self{
         Self((val as u64&0xffff_ffff) | Self::TYPE_OBJECT)
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
    
    pub fn from_static_string(index: StringIndex)->Self{
        Self((index.0 as u64 & 0xffff_ffff)|Self::TYPE_STATIC_STRING)
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
    
               
    pub fn as_object(&self)->Option<usize>{
        if self.is_object(){
            return Some(self.0 as usize & 0x0000_0000_ffff_ffff)
        }
        None
    }
    
    pub fn as_heap_string(&self)->Option<usize>{
        if self.is_heap_string(){
            return Some(self.0 as usize & 0x0000_0000_ffff_ffff)
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
    
    pub fn is_heap_string(&self)->bool{
        (self.0 & Self::TYPE_STRING_MASK) == Self::TYPE_HEAP_STRING
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
                Self::OP_MUL => return write!(f, "*"),
                Self::OP_DIV => return write!(f, "/"),
                Self::OP_MOD => return write!(f, "%"),
                Self::OP_ADD => return write!(f, "+"),
                Self::OP_SUB => return write!(f, "-"),
                Self::OP_SHL => return write!(f, "<<"),
                Self::OP_SHR => return write!(f, ">>"),
                Self::OP_AND => return write!(f, "&"),
                Self::OP_XOR => return write!(f, "^"),
                Self::OP_OR => return write!(f, "|"),
                Self::OP_EQ => return write!(f, "=="),
                Self::OP_NEQ => return write!(f, "!="),
                Self::OP_LT => return write!(f, "<"),
                Self::OP_GT => return write!(f, ">"),
                Self::OP_LEQ => return write!(f, "<="),
                Self::OP_GEQ => return write!(f, ">="),
                Self::OP_LOGIC_AND => return write!(f, "&&"),
                Self::OP_LOGIC_OR => return write!(f, "||"),
                Self::OP_ASSIGN => return write!(f, "="),
                Self::OP_ASSIGN_ADD => return write!(f, "+="),
                Self::OP_ASSIGN_SUB => return write!(f, "-="),
                Self::OP_ASSIGN_MUL => return write!(f, "*="),
                Self::OP_ASSIGN_DIV => return write!(f, "/="),
                Self::OP_ASSIGN_MOD => return write!(f, "%="),
                Self::OP_ASSIGN_AND => return write!(f, "&="),
                Self::OP_ASSIGN_OR => return write!(f, "|="),
                Self::OP_ASSIGN_XOR => return write!(f, "^="),
                Self::OP_ASSIGN_SHL => return write!(f, "<<="),
                Self::OP_ASSIGN_SHR => return write!(f, ">>="),
                Self::OP_BEGIN_PROTO => return write!(f, "Proto{{"),
                Self::OP_END_PROTO => return write!(f, "}}"),
                Self::OP_BEGIN_BARE => return write!(f, "Bare{{"),
                Self::OP_END_BARE => return write!(f, "}}"),
                Self::OP_BEGIN_CALL => return write!(f, "Call("),
                Self::OP_END_CALL => return write!(f, ")"),
                Self::OP_BEGIN_FRAG => return write!(f, "Frag("),
                Self::OP_END_FRAG => return write!(f, ")"),
                Self::OP_FIELD => return write!(f, "."),
                Self::OP_ARRAY_INDEX => return write!(f, "[]"),
                _=>return write!(f, "OP?")
            }
        }
        write!(f, "?{:08x}", self.0)
    }
}