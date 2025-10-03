use crate::id::Id;

#[derive(PartialEq, Eq, Clone, Copy)]
pub struct Value(u64);

impl Default for Value{
    fn default()->Self{
        Self::NIL
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ObjectPtr{
    pub zone: u8,
    pub index: u32    
}

impl From<ObjectPtr> for Value{
    fn from(v:ObjectPtr) -> Self{
        Value::from_object(v)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct StringPtr{
    pub zone: u8,
    pub index: u32    
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
    pub const TYPE_STRING: u64 = 0xFFFF_0500_0000_0000;
    pub const TYPE_OBJECT: u64 = 0xFFFF_0600_0000_0000;
    
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
    pub const TYPE_OPCODE: u64 = 0xFFFF_0700_0000_0000;
    pub const OI_NOP: u64 = 0;pub const OP_NOP: Value = Value(Self::TYPE_OPCODE | Self::OI_NOP);
    
    pub const OI_NOT:u64 = 1;pub const OP_NOT: Value = Value(Self::TYPE_OPCODE | Self::OI_NOT);
    pub const OI_NEG:u64 = 2;pub const OP_NEG: Value = Value(Self::TYPE_OPCODE | Self::OI_NEG);
    pub const OI_MUL:u64 = 3;pub const OP_MUL: Value = Value(Self::TYPE_OPCODE | Self::OI_MUL);
    pub const OI_DIV:u64 = 4;pub const OP_DIV: Value = Value(Self::TYPE_OPCODE | Self::OI_DIV);
    pub const OI_MOD:u64 = 5;pub const OP_MOD: Value = Value(Self::TYPE_OPCODE | Self::OI_MOD);
    pub const OI_ADD:u64 = 6;pub const OP_ADD: Value = Value(Self::TYPE_OPCODE | Self::OI_ADD);
    pub const OI_SUB:u64 = 7;pub const OP_SUB: Value = Value(Self::TYPE_OPCODE | Self::OI_SUB);
    pub const OI_SHL:u64 = 8;pub const OP_SHL: Value = Value(Self::TYPE_OPCODE | Self::OI_SHL);
    pub const OI_SHR:u64 = 9;pub const OP_SHR: Value = Value(Self::TYPE_OPCODE | Self::OI_SHR);
    pub const OI_AND:u64 = 10;pub const OP_AND: Value = Value(Self::TYPE_OPCODE | Self::OI_AND);
    pub const OI_OR:u64 = 11;pub const OP_OR: Value = Value(Self::TYPE_OPCODE | Self::OI_OR);
    pub const OI_XOR:u64 = 12;pub const OP_XOR: Value = Value(Self::TYPE_OPCODE | Self::OI_XOR);
    
    pub const OI_CONCAT:u64 = 13;pub const OP_CONCAT: Value = Value(Self::TYPE_OPCODE | Self::OI_CONCAT);
    pub const OI_EQ:u64 = 14;pub const OP_EQ: Value = Value(Self::TYPE_OPCODE | Self::OI_EQ);
    pub const OI_NEQ:u64 = 15;pub const OP_NEQ: Value = Value(Self::TYPE_OPCODE | Self::OI_NEQ);
    pub const OI_LT:u64 = 16;pub const OP_LT: Value = Value(Self::TYPE_OPCODE | Self::OI_LT);
    pub const OI_GT:u64 = 17;pub const OP_GT: Value = Value(Self::TYPE_OPCODE | Self::OI_GT);
    pub const OI_LEQ:u64 = 18;pub const OP_LEQ: Value = Value(Self::TYPE_OPCODE | Self::OI_LEQ);
    pub const OI_GEQ:u64 = 19;pub const OP_GEQ: Value = Value(Self::TYPE_OPCODE | Self::OI_GEQ);
    pub const OI_LOGIC_AND:u64 = 20;pub const OP_LOGIC_AND: Value = Value(Self::TYPE_OPCODE | Self::OI_LOGIC_AND);
    pub const OI_LOGIC_OR:u64 = 21;pub const OP_LOGIC_OR: Value = Value(Self::TYPE_OPCODE | Self::OI_LOGIC_OR);
    
    pub const OI_ASSIGN_ME:u64 = 22;pub const OP_ASSIGN_ME: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_ME);
    pub const OI_ASSIGN:u64 = 23;pub const OP_ASSIGN: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN);
    pub const OI_ASSIGN_ADD:u64 = 24;pub const OP_ASSIGN_ADD: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_ADD);
    pub const OI_ASSIGN_SUB:u64 = 25;pub const OP_ASSIGN_SUB: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_SUB);
    pub const OI_ASSIGN_MUL:u64 = 26;pub const OP_ASSIGN_MUL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_MUL);
    pub const OI_ASSIGN_DIV:u64 = 27;pub const OP_ASSIGN_DIV: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_DIV);
    pub const OI_ASSIGN_MOD:u64 = 28;pub const OP_ASSIGN_MOD: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_MOD);
    pub const OI_ASSIGN_AND:u64 = 29;pub const OP_ASSIGN_AND: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_AND);
    pub const OI_ASSIGN_OR:u64 = 30;pub const OP_ASSIGN_OR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_OR);
    pub const OI_ASSIGN_XOR:u64 = 31;pub const OP_ASSIGN_XOR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_XOR);
    pub const OI_ASSIGN_SHL:u64 = 32;pub const OP_ASSIGN_SHL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_SHL);
    pub const OI_ASSIGN_SHR:u64 = 33;pub const OP_ASSIGN_SHR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_SHR);
    pub const OI_ASSIGN_IFNIL:u64 = 34;pub const OP_ASSIGN_IFNIL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_IFNIL);
    
    pub const OI_ASSIGN_FIELD:u64 = 35;pub const OP_ASSIGN_FIELD: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD);
    pub const OI_ASSIGN_FIELD_ADD:u64 = 36;pub const OP_ASSIGN_FIELD_ADD: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_ADD);
    pub const OI_ASSIGN_FIELD_SUB:u64 = 37;pub const OP_ASSIGN_FIELD_SUB: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_SUB);
    pub const OI_ASSIGN_FIELD_MUL:u64 = 38;pub const OP_ASSIGN_FIELD_MUL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_MUL);
    pub const OI_ASSIGN_FIELD_DIV:u64 = 39;pub const OP_ASSIGN_FIELD_DIV: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_DIV);
    pub const OI_ASSIGN_FIELD_MOD:u64 = 40;pub const OP_ASSIGN_FIELD_MOD: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_MOD);
    pub const OI_ASSIGN_FIELD_AND:u64 = 41;pub const OP_ASSIGN_FIELD_AND: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_AND);
    pub const OI_ASSIGN_FIELD_OR:u64 = 42;pub const OP_ASSIGN_FIELD_OR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_OR);
    pub const OI_ASSIGN_FIELD_XOR:u64 = 43;pub const OP_ASSIGN_FIELD_XOR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_XOR);
    pub const OI_ASSIGN_FIELD_SHL:u64 = 44;pub const OP_ASSIGN_FIELD_SHL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_SHL);
    pub const OI_ASSIGN_FIELD_SHR:u64 = 45;pub const OP_ASSIGN_FIELD_SHR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_SHR);
    pub const OI_ASSIGN_FIELD_IFNIL:u64 = 46;pub const OP_ASSIGN_FIELD_IFNIL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_FIELD_IFNIL);
        
    pub const OI_ASSIGN_INDEX:u64 = 47;pub const OP_ASSIGN_INDEX: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX);
    pub const OI_ASSIGN_INDEX_ADD:u64 = 48;pub const OP_ASSIGN_INDEX_ADD: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_ADD);
    pub const OI_ASSIGN_INDEX_SUB:u64 = 49;pub const OP_ASSIGN_INDEX_SUB: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_SUB);
    pub const OI_ASSIGN_INDEX_MUL:u64 = 50;pub const OP_ASSIGN_INDEX_MUL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_MUL);
    pub const OI_ASSIGN_INDEX_DIV:u64 = 51;pub const OP_ASSIGN_INDEX_DIV: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_DIV);
    pub const OI_ASSIGN_INDEX_MOD:u64 = 52;pub const OP_ASSIGN_INDEX_MOD: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_MOD);
    pub const OI_ASSIGN_INDEX_AND:u64 = 53;pub const OP_ASSIGN_INDEX_AND: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_AND);
    pub const OI_ASSIGN_INDEX_OR:u64 = 54;pub const OP_ASSIGN_INDEX_OR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_OR);
    pub const OI_ASSIGN_INDEX_XOR:u64 = 55;pub const OP_ASSIGN_INDEX_XOR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_XOR);
    pub const OI_ASSIGN_INDEX_SHL:u64 = 56;pub const OP_ASSIGN_INDEX_SHL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_SHL);
    pub const OI_ASSIGN_INDEX_SHR:u64 = 57;pub const OP_ASSIGN_INDEX_SHR: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_SHR);
    pub const OI_ASSIGN_INDEX_IFNIL:u64 = 58;pub const OP_ASSIGN_INDEX_IFNIL: Value = Value(Self::TYPE_OPCODE | Self::OI_ASSIGN_INDEX_IFNIL);    
    pub const OI_BEGIN_PROTO:u64 = 59;pub const OP_BEGIN_PROTO: Value = Value(Self::TYPE_OPCODE | Self::OI_BEGIN_PROTO);
    pub const OI_END_PROTO:u64 = 60;pub const OP_END_PROTO: Value = Value(Self::TYPE_OPCODE | Self::OI_END_PROTO);
    pub const OI_BEGIN_BARE:u64 = 61;pub const OP_BEGIN_BARE: Value = Value(Self::TYPE_OPCODE | Self::OI_BEGIN_BARE);
    pub const OI_END_BARE:u64 = 62;pub const OP_END_BARE: Value = Value(Self::TYPE_OPCODE | Self::OI_END_BARE);
    pub const OI_BEGIN_CALL:u64 = 63;pub const OP_BEGIN_CALL: Value = Value(Self::TYPE_OPCODE | Self::OI_BEGIN_CALL);
    pub const OI_END_CALL:u64 = 64;pub const OP_END_CALL: Value = Value(Self::TYPE_OPCODE | Self::OI_END_CALL);
    pub const OI_BEGIN_FRAG:u64 = 65;pub const OP_BEGIN_FRAG: Value = Value(Self::TYPE_OPCODE | Self::OI_BEGIN_FRAG);
    pub const OI_END_FRAG:u64 = 66;pub const OP_END_FRAG: Value = Value(Self::TYPE_OPCODE | Self::OI_END_FRAG);
    
    pub const OI_BEGIN_FN:u64 = 67;pub const OP_BEGIN_FN: Value = Value(Self::TYPE_OPCODE | Self::OI_BEGIN_FN);
    pub const OI_FN_ARG_DYN:u64 = 68;pub const OP_FN_ARG_DYN: Value = Value(Self::TYPE_OPCODE | Self::OI_FN_ARG_DYN);
    pub const OI_FN_ARG_TYPED:u64 = 69;pub const OP_FN_ARG_TYPED: Value = Value(Self::TYPE_OPCODE | Self::OI_FN_ARG_TYPED);
    pub const OI_FN_EXPR:u64 = 70;pub const OP_FN_EXPR: Value = Value(Self::TYPE_OPCODE | Self::OI_FN_EXPR);
    pub const OI_BEGIN_FN_BLOCK:u64 = 71;pub const OP_BEGIN_FN_BLOCK: Value = Value(Self::TYPE_OPCODE | Self::OI_BEGIN_FN_BLOCK);
    pub const OI_END_FN_BLOCK:u64 = 72;pub const OP_END_FN_BLOCK: Value = Value(Self::TYPE_OPCODE | Self::OI_END_FN_BLOCK);
    pub const OP_RETURN:  Value = Value(Self::TYPE_OPCODE | 73);
    
    pub const OI_FIELD:u64 = 74;pub const OP_FIELD: Value = Value(Self::TYPE_OPCODE | Self::OI_FIELD);
    pub const OI_ARRAY_INDEX:u64 = 75;pub const OP_ARRAY_INDEX: Value = Value(Self::TYPE_OPCODE | Self::OI_ARRAY_INDEX);
    // prototypically inherit the chain for deep prototype fields
    pub const OI_PROTO_FIELD:u64 = 76;pub const OP_PROTO_FIELD: Value = Value(Self::TYPE_OPCODE | Self::OI_PROTO_FIELD);
    pub const OI_POP_TO_ME:u64 = 77;pub const OP_POP_TO_ME: Value = Value(Self::TYPE_OPCODE | Self::OI_POP_TO_ME);
    
    pub const OI_LET_DYN_NIL:u64 = 78;pub const OP_LET_DYN_NIL: Value = Value(Self::TYPE_OPCODE | Self::OI_LET_DYN_NIL);
    pub const OI_LET_TYPED_NIL:u64 = 79;pub const OP_LET_TYPED_NIL: Value = Value(Self::TYPE_OPCODE | Self::OI_LET_TYPED_NIL);
    pub const OI_LET_TYPED:u64 = 80;pub const OP_LET_TYPED: Value = Value(Self::TYPE_OPCODE | Self::OI_LET_TYPED);
    pub const OI_LET_DYN:u64 = 81;pub const OP_LET_DYN: Value = Value(Self::TYPE_OPCODE | Self::OI_LET_DYN);
    
    pub const OI_ID_AS_VAR:u64 = 82;pub const OP_ID_AS_VAR: Value = Value(Self::TYPE_OPCODE | Self::OI_ID_AS_VAR);
        
    pub const OI_SEARCH_TREE:u64 = 83;pub const OP_SEARCH_TREE: Value = Value(Self::TYPE_OPCODE | Self::OI_SEARCH_TREE);
        
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
         Self(((ptr.zone as u64) << 32) | ptr.index as u64 | Self::TYPE_OBJECT)
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
         Self(((ptr.zone as u64) << 32) | ptr.index as u64 | Self::TYPE_STRING)
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
    
    pub fn with_inline_string<R,F:FnOnce(&str)->R>(&self, f:F)->Option<R>{
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
                zone: ((self.0 &0xff_0000_0000) >>32) as u8,
                index: (self.0 & 0xffff_ffff) as u32
            })
        }
        None
    }
    
        
    pub fn as_opcode_index(&self)->Option<u64>{
        if self.is_opcode(){
            return Some(self.0 & 0xff)
        }
        None
    }
    
    pub fn as_string(&self)->Option<StringPtr>{
        if self.is_string(){
            return Some(StringPtr{
                zone: ((self.0 & 0xff_0000_0000) >>32) as u8,
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
        if self.is_inline_string(){
            return self.with_inline_string(|s|{
                write!(f, "{s}")
            }).unwrap();
        }
        if let Some(_) = self.as_object(){
            return write!(f, "[Object]")
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
                Self::OP_ASSIGN_ME => return write!(f, ":"),
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
                
                Self::OP_ASSIGN_FIELD => return write!(f, ".="),
                Self::OP_ASSIGN_FIELD_ADD => return write!(f, ".+="),
                Self::OP_ASSIGN_FIELD_SUB => return write!(f, ".-="),
                Self::OP_ASSIGN_FIELD_MUL => return write!(f, ".*="),
                Self::OP_ASSIGN_FIELD_DIV => return write!(f, "./="),
                Self::OP_ASSIGN_FIELD_MOD => return write!(f, ".%="),
                Self::OP_ASSIGN_FIELD_AND => return write!(f, ".&="),
                Self::OP_ASSIGN_FIELD_OR => return write!(f, ".|="),
                Self::OP_ASSIGN_FIELD_XOR => return write!(f, ".^="),
                Self::OP_ASSIGN_FIELD_SHL => return write!(f, ".<<="),
                Self::OP_ASSIGN_FIELD_SHR => return write!(f, ".>>="),
                Self::OP_ASSIGN_FIELD_IFNIL => return write!(f, ".?="),
                
                Self::OP_ASSIGN_INDEX => return write!(f, "[]="),
                Self::OP_ASSIGN_INDEX_ADD => return write!(f, "[]+="),
                Self::OP_ASSIGN_INDEX_SUB => return write!(f, "[]-="),
                Self::OP_ASSIGN_INDEX_MUL => return write!(f, "[]*="),
                Self::OP_ASSIGN_INDEX_DIV => return write!(f, "[]/="),
                Self::OP_ASSIGN_INDEX_MOD => return write!(f, "[]%="),
                Self::OP_ASSIGN_INDEX_AND => return write!(f, "[]&="),
                Self::OP_ASSIGN_INDEX_OR => return write!(f, "[]|="),
                Self::OP_ASSIGN_INDEX_XOR => return write!(f, "[]^="),
                Self::OP_ASSIGN_INDEX_SHL => return write!(f, "[]<<="),
                Self::OP_ASSIGN_INDEX_SHR => return write!(f, "[]>>="),
                Self::OP_ASSIGN_INDEX_IFNIL => return write!(f, "[]?="),
                
                Self::OP_BEGIN_PROTO => return write!(f, "<proto>{{"),
                Self::OP_END_PROTO => return write!(f, "}}"),
                Self::OP_BEGIN_BARE => return write!(f, "<bare>{{"),
                Self::OP_END_BARE => return write!(f, "}}"),
                Self::OP_BEGIN_CALL => return write!(f, "<call>("),
                Self::OP_END_CALL => return write!(f, ")"),
                Self::OP_BEGIN_FRAG => return write!(f, "<frag>("),
                Self::OP_END_FRAG => return write!(f, ")"),
                
                Self::OP_BEGIN_FN=> return write!(f, "<fn>|"),
                Self::OP_FN_ARG_DYN=> return write!(f, "<arg dyn nil>"),
                Self::OP_FN_ARG_TYPED=> return write!(f, "<arg typed nil>"),
                Self::OP_FN_EXPR=> return write!(f, "|<fnexpr>"),
                Self::OP_BEGIN_FN_BLOCK=> return write!(f, "|<fnblock>{{"),
                Self::OP_END_FN_BLOCK=> return write!(f, "}}"),
                Self::OP_RETURN=> return write!(f, "<return>"),
                                
                Self::OP_FIELD => return write!(f, "."),
                Self::OP_ARRAY_INDEX => return write!(f, "[]"),
                                
                Self::OP_PROTO_FIELD=> return write!(f, "<proto>."),
                Self::OP_POP_TO_ME=> return write!(f, "<me>"),
                 
                Self::OP_LET_DYN_NIL=> return write!(f, "let dyn nil"),
                Self::OP_LET_TYPED_NIL=> return write!(f, "let typed nil"),
                Self::OP_LET_TYPED => return write!(f, "let typed"),
                Self::OP_LET_DYN => return write!(f, "let dyn"),
                Self::OP_ID_AS_VAR => return write!(f, "@"),
                
                Self::OP_SEARCH_TREE => return write!(f, "$"),
                _=>return write!(f, "OP?")
            }
        }
        write!(f, "?{:08x}", self.0)
    }
}