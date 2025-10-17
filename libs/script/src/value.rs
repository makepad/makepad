use crate::makepad_id::*;
use crate::opcode::*;
use std::fmt;

#[derive(PartialEq, Eq, Clone, Copy, Hash, Ord, PartialOrd)]
pub struct Value(u64);

pub const NIL:Value = Value::NIL;

impl Default for Value{
    fn default()->Self{
        Self::NIL
    }
}

#[derive(Debug, Default, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ScriptIp{
    pub body: u16,
    pub index: u32,
}

impl ScriptIp{
    const fn from_u40(value:u64)->Self{
        Self{
            body: ((value >> 28)&0xFFF) as u16,
            index: ((value) & 0xFFF_FFFF) as u32
        }
    }
    const fn to_u40(&self)->u64{
        ((self.body as u64)<<28) | self.index as u64
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ObjectPtr{
    pub index: u32    
}

impl From<ObjectPtr> for Value{
    fn from(v:ObjectPtr) -> Self{
        Value::from_object(v)
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct StringPtr{
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

impl From<u32> for Value{
    fn from(v:u32) -> Self{
        Value::from_f64(v as f64)
    }
}

impl From<i32> for Value{
    fn from(v:i32) -> Self{
        Value::from_f64(v as f64)
    }
}

impl From<usize> for Value{
    fn from(v:usize) -> Self{
        Value::from_f64(v as f64)
    }
}

impl From<bool> for Value{
    fn from(v:bool) -> Self{
        Value::from_bool(v)
    }
}

impl From<Id> for Value{
    fn from(v:Id) -> Self{
        Value::from_id(v)
    }
}

impl From<&Id> for Value{
    fn from(v:&Id) -> Self{
        Value::from_id(*v)
    }
}

impl From<Opcode> for Value{
    fn from(v:Opcode) -> Self{
        Value::from_opcode(v)
    }
}
// NaN box value

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ValueError{
    pub ty: ValueType,
    pub ip: ScriptIp
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ValueType(u8);

impl ValueType{
    pub const NUMBER: Self = Self(0);
    pub const NAN: Self = Self(1);
    pub const BOOL: Self = Self(2);
    pub const NIL: Self = Self(3);
    pub const COLOR: Self = Self(4);
    pub const STRING: Self = Self(5);
    pub const OBJECT: Self = Self(6);
    pub const RSID: Self = Self(7);
    pub const OPCODE: Self = Self(8);
    
    pub const REDUX_MARKER: Self = Self(9);
    pub const INLINE_STRING_0: Self = Self(9);
    pub const INLINE_STRING_1: Self = Self(10);
    pub const INLINE_STRING_2: Self = Self(11);
    pub const INLINE_STRING_3: Self = Self(12);
    pub const INLINE_STRING_4: Self = Self(13);
    pub const INLINE_STRING_5: Self = Self(14);
    pub const INLINE_STRING_END: Self = Self(15);
    
    pub const ERR_FIRST: Self = Self(16);
    pub const ERR_NOTFOUND: Self = Self(16);
    pub const ERR_NOTFN: Self = Self(17);
    pub const ERR_NOTFIELD: Self = Self(18);
    pub const ERR_NOTINDEX: Self = Self(19);
    pub const ERR_NOTOBJECT: Self = Self(20);
    pub const ERR_STACKUNDERFLOW: Self = Self(21);
    pub const ERR_STACKOVERFLOW: Self = Self(22);
    pub const ERR_INVALIDARGS: Self = Self(23);
    pub const ERR_NOTASSIGNABLE: Self = Self(24);
    pub const ERR_INTERNAL: Self = Self(25);
    pub const ERR_ASSERTFAIL: Self = Self(26);
    pub const ERR_NOTIMPL: Self = Self(27);
    pub const ERR_FROZEN: Self = Self(28);
    pub const ERR_VALIDATION: Self = Self(29);
    pub const ERR_INVKEY: Self = Self(30);
    
    pub const ERR_USER: Self = Self(31);
    pub const ERR_LAST: Self = Self(31);
            
    pub const ID: Self = Self(0x80);
    
    pub const REDUX_NUMBER: usize = 0;
    pub const REDUX_NAN: usize = 1;
    pub const REDUX_BOOL: usize = 2;
    pub const REDUX_NIL: usize = 3;
    pub const REDUX_COLOR: usize = 4;
    pub const REDUX_STRING: usize = 5;
    pub const REDUX_OBJECT: usize = 6;
    pub const REDUX_RSID: usize = 7;
    pub const REDUX_OPCODE: usize = 8;
    pub const REDUX_ERR: usize = 9;
    pub const REDUX_ID: usize = 10;
    
    pub const fn to_u64(&self)->u64{ ((self.0 as u64) << 40) | 0xFFFF_0000_0000_0000 }
    pub const fn from_u64(val:u64)->Self{
        let val = ((val>>40)&0xff) as u8;
        if val > Self::ID.0{
            return Self::ID
        }
        Self(val)
    }
    
    pub const fn to_redux(&self)->usize{
        if self.0 >= Self::REDUX_MARKER.0{
            if self.0 >= Self::ID.0{
                return Self::REDUX_ID
            }
            else if self.0 >= Self::ERR_FIRST.0{
                Self::REDUX_ERR as usize
            }
            else{
                Self::REDUX_STRING as usize 
            }
        }
        else if self.0 > 0{
            (self.0) as usize 
        }
        else{
            0
        }
    }
}


impl fmt::Debug for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}


impl fmt::Display for ValueType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self{
            Self::NUMBER=>write!(f,"number"),
            Self::NAN=>write!(f,"nan"),
            Self::BOOL=>write!(f,"bool"),
            Self::NIL=>write!(f,"nil"),
            Self::COLOR=>write!(f,"color"),
            Self::STRING=>write!(f,"string"),
            Self::OBJECT=>write!(f,"object"),
            Self::RSID=>write!(f,"rsid"),
            Self::OPCODE=>write!(f,"opcode"),
            Self::INLINE_STRING_0=>write!(f,"string0"),
            Self::INLINE_STRING_1=>write!(f,"string1"),
            Self::INLINE_STRING_2=>write!(f,"string2"),
            Self::INLINE_STRING_3=>write!(f,"string3"),
            Self::INLINE_STRING_4=>write!(f,"string4"),
            Self::INLINE_STRING_5=>write!(f,"string5"),
            Self::ERR_NOTFOUND=>write!(f,"NotFoundOnScope"),
            Self::ERR_NOTFN=>write!(f,"NotAFunction"),
            Self::ERR_NOTFIELD=>write!(f,"FieldNotFound"),
            Self::ERR_NOTINDEX=>write!(f,"IndexNotFound"),
            Self::ERR_NOTOBJECT=>write!(f,"NotAnObject"),
            Self::ERR_STACKUNDERFLOW=>write!(f,"StackUnderflow"),
            Self::ERR_STACKOVERFLOW=>write!(f,"StackOverflow"),
            Self::ERR_INVALIDARGS=>write!(f,"InvalidArgs"),
            Self::ERR_NOTASSIGNABLE=>write!(f,"NotAssignable"),
            Self::ERR_INTERNAL=>write!(f,"Internal"),
            Self::ERR_ASSERTFAIL=>write!(f,"AssertFailure"),
            Self::ERR_NOTIMPL=>write!(f,"NotImplemented"),
            Self::ERR_USER=>write!(f,"UserGenerated"),
            x if x.0 >= Self::ID.0=>write!(f,"id"),
            _=>write!(f,"ValueType?")
        }
    }
}

pub trait IdExt{
    fn escape(&self)->Value;
}

impl IdExt for Id{
    fn escape(&self)->Value{
        Value::from_escaped_id(*self)
    }
}

impl Value{
    pub const TYPE_MASK: u64 = 0xFFFF_FF00_0000_0000;
        
    pub const TYPE_NAN: u64 = ValueType::NAN.to_u64();
    pub const TYPE_TRACED_NAN_MAX: u64 = ValueType::NAN.to_u64() | 0xFF_FFFF_FFFF;
    pub const NAN: Value = Value( Self::TYPE_NAN);
    
    pub const TYPE_BOOL: u64 = ValueType::BOOL.to_u64();
    pub const FALSE: Value = Value( Self::TYPE_BOOL | 0x0000_0000);
    pub const TRUE: Value = Value(Self::TYPE_BOOL | 0x0000_0001);
    
    pub const TYPE_NIL: u64 = ValueType::NIL.to_u64();
    pub const NIL: Value = Value(Self::TYPE_NIL);
    
    pub const TYPE_COLOR: u64 = ValueType::COLOR.to_u64();
    pub const TYPE_STRING: u64 = ValueType::STRING.to_u64();
    pub const TYPE_OBJECT: u64 = ValueType::OBJECT.to_u64();
    pub const TYPE_RSID: u64 = ValueType::RSID.to_u64();
    
    pub const TYPE_INLINE_STRING_0: u64 = ValueType::INLINE_STRING_0.to_u64();
    pub const TYPE_INLINE_STRING_1: u64 = ValueType::INLINE_STRING_1.to_u64();
    pub const TYPE_INLINE_STRING_2: u64 = ValueType::INLINE_STRING_2.to_u64();
    pub const TYPE_INLINE_STRING_3: u64 = ValueType::INLINE_STRING_3.to_u64();
    pub const TYPE_INLINE_STRING_4: u64 = ValueType::INLINE_STRING_4.to_u64();
    pub const TYPE_INLINE_STRING_5: u64 = ValueType::INLINE_STRING_5.to_u64();
    pub const TYPE_INLINE_STRING_END: u64 = ValueType::INLINE_STRING_END.to_u64();

    pub const TYPE_ID: u64 = ValueType::ID.to_u64();
    
    pub const ESCAPED_ID: u64 = 0x0000_4000_0000_0000;
    
    // opcodes
    pub const TYPE_OPCODE: u64 = ValueType::OPCODE.to_u64();
    
    pub const fn from_opcode(op:Opcode)->Self{ Self(Self::TYPE_OPCODE | (op.0 as u64)<<32)}
    
    pub const fn from_opcode_args(op:Opcode, args:OpcodeArgs)->Self{ Self(Self::TYPE_OPCODE | (op.0 as u64)<<32 | (args.0 as u64))}
        
    // TODO: make this behave like javascript as much as is sensible
        
    pub const fn err_notfound(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTFOUND.to_u64() | ip.to_u40())}
    pub const fn err_notfn(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTFN.to_u64() | ip.to_u40())}
    pub const fn err_notfield(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTFIELD.to_u64() | ip.to_u40())}
    pub const fn err_notindex(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTINDEX.to_u64() | ip.to_u40())}
    pub const fn err_notobject(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTOBJECT.to_u64()| ip.to_u40())}
    pub const fn err_stackunderflow(ip:ScriptIp)->Self{Self(ValueType::ERR_STACKUNDERFLOW.to_u64() | ip.to_u40())}
    pub const fn err_stackoverflow(ip:ScriptIp)->Self{Self(ValueType::ERR_STACKOVERFLOW.to_u64() | ip.to_u40())}
    pub const fn err_invalidargs(ip:ScriptIp)->Self{Self(ValueType::ERR_INVALIDARGS.to_u64() | ip.to_u40())}
    pub const fn err_notassignable(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTASSIGNABLE.to_u64() | ip.to_u40())}
    pub const fn err_internal(ip:ScriptIp)->Self{Self(ValueType::ERR_INTERNAL.to_u64() | ip.to_u40())}
    pub const fn err_assertfail(ip:ScriptIp)->Self{Self(ValueType::ERR_ASSERTFAIL.to_u64() | ip.to_u40())}
    pub const fn err_notimpl(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTIMPL.to_u64() | ip.to_u40())}
    pub const fn err_frozen(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTIMPL.to_u64() | ip.to_u40())}
    pub const fn err_validation(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTIMPL.to_u64() | ip.to_u40())}
    pub const fn err_wrongkey(ip:ScriptIp)->Self{Self(ValueType::ERR_NOTIMPL.to_u64() | ip.to_u40())}
        
    pub const ERR_FROZEN: Self = Self(28);
    pub const ERR_VALIDATION: Self = Self(29);
    pub const ERR_INVKEY: Self = Self(30);
    
    pub const fn err_user(ip:ScriptIp)->Self{Self(ValueType::ERR_USER.to_u64() | ip.to_u40())}
    
    pub const fn is_err(&self)->bool{(self.0&Self::TYPE_MASK) >=ValueType::ERR_FIRST.to_u64() &&(self.0&Self::TYPE_MASK) <= ValueType::ERR_LAST.to_u64()}
    
    pub const fn as_err(&self)->Option<ValueError>{
        if self.is_err(){
            Some(ValueError{
                ty: self.value_type(),
                ip: ScriptIp::from_u40(self.0)
            })
        }
        else{
            None
        }
    }
        
    pub const fn value_type(&self)->ValueType{
        if self.is_non_nan_number(){
            return ValueType::NUMBER
        }
        ValueType::from_u64(self.0 & Self::TYPE_MASK)
    }
    
    pub const fn from_f64(val:f64)->Self{
        if val.is_nan(){
            Self::NAN
        }
        else{
            Self(val.to_bits())
        }
    }
    
    pub const fn as_f64_traced_nan(&self)->Option<ScriptIp>{
        if self.is_nan(){
            Some(ScriptIp::from_u40(self.0))
        }
        else{
            None
        }
    }
    
    pub  fn from_f64_traced_nan(val:f64, ip:ScriptIp)->Self{
        let bits = val.to_bits();
        if val.is_nan(){
            if bits >= Self::TYPE_NAN && bits <= Self::TYPE_TRACED_NAN_MAX{
                Self(bits)
            }
            else{
                Self(Self::TYPE_NAN | ip.to_u40())
            }
        }
        else{
            Self(bits)
        }
    }
    
    pub fn from_object(ptr: ObjectPtr)->Self{
         Self(ptr.index as u64 | Self::TYPE_OBJECT)
    }
        
    pub const fn from_bool(val: bool)->Self{
        if val{Self::TRUE}
        else{Self::FALSE}
    }
    
    pub const fn from_color(val: u32)->Self{
        Self(val as u64|Self::TYPE_COLOR)
    }
    
    pub const fn from_rsid(val: u64)->Self{
        Self(val as u64|(Self::TYPE_RSID&0xFF_FFFF_FFFF))
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
    
    pub const fn as_bool(&self)->Option<bool>{
        if self.is_bool(){
            return Some(self.0 == Self::TRUE.0)
        }
        None
    }
    
    pub const fn as_f64(&self)->Option<f64>{
        if self.is_number(){
            return Some(f64::from_bits(self.0))
        }
        None    
    }
    
    pub const fn as_index(&self)->usize{
        if let Some(f) = self.as_f64(){
            return f as usize
        }
        if let Some(b) = self.as_bool(){
            return if b{1} else{0}
        }
        0
    }
        
    pub const fn as_id(&self)->Option<Id>{
        if self.is_id(){
            return Some(Id(self.0&0x0000_3fff_ffff_ffff))
        }
        None
    }
    
    pub const fn is_inline_string(&self)->bool{
        self.0 >= Self::TYPE_INLINE_STRING_0  && self.0 < Self::TYPE_INLINE_STRING_END
    }
    
    pub const fn is_escaped_id(&self)->bool{
        self.0 >= Self::TYPE_ID | Self::ESCAPED_ID
    }
        
    pub const fn as_object(&self)->Option<ObjectPtr>{
        if self.is_object(){
            return Some(ObjectPtr{
                index: (self.0 & 0xffff_ffff) as u32
            })
        }
        None
    }
        
    pub const fn as_opcode(&self)->Option<(Opcode,OpcodeArgs)>{
        if self.is_opcode(){
            return Some((Opcode(((self.0>>32) & 0xff) as u8),OpcodeArgs((self.0 & 0xffff_ffff) as u32)))
        }
        None
    }
    
    pub const fn set_opcode_args(&mut self, args:OpcodeArgs){
        if self.is_opcode(){
            self.0 = (self.0 & 0xffff_ffff_0000_0000) | (args.0 as u64);
        }
    }
    
    pub const fn set_opcode_args_pop_to_me(&mut self){
        if self.is_opcode(){
            self.0 |= OpcodeArgs::POP_TO_ME_FLAG as u64;
        }
    }
    
    pub const fn clear_opcode_args_pop_to_me(&mut self){
        if self.is_opcode(){
            self.0 &= !(OpcodeArgs::POP_TO_ME_FLAG as u64);
        }
    }
    
    pub const fn has_opcode_args_pop_to_me(&self)->bool{
        if self.is_opcode(){
            self.0 & (OpcodeArgs::POP_TO_ME_FLAG as u64) != 0
        }
        else{
            false
        }
    }
        
    pub const fn is_assign_opcode(&self)->bool{
        if self.is_opcode(){
            let code = Opcode(((self.0>>32) & 0xff) as u8);
            return code.is_assign()
        }
        false
    }
    
    pub const fn is_let_opcode(&self)->bool{
        if self.is_opcode(){
            let code = Opcode(((self.0>>32) & 0xff) as u8);
            return code.0 == Opcode::LET_TYPED.0 || code.0 == Opcode::LET_DYN.0
        }
        false
    }
    /*
    pub const fn set_opcode_is_statement(&mut self){
        if self.is_opcode(){
            self.0 |= OpcodeArgs::STATEMENT_FLAG as u64;
        }
    }*/
        
        
    pub const fn as_string(&self)->Option<StringPtr>{
        if self.is_string(){
            return Some(StringPtr{
                index: (self.0 & 0xffff_ffff) as u32
            })
        }
        None
    }
        
    pub const fn as_color(&self)->Option<u32>{
        if self.is_color(){
            return Some((self.0&0xffff_ffff) as u32)
        }
        None
    }
    
    pub const fn as_rsid(&self)->Option<u64>{
        if self.is_rsid(){
            return Some((self.0&0xff_ffff_ffff) as u64)
        }
        None
    }
    
    pub const fn is_number(&self)->bool{
        self.0 <= Self::TYPE_TRACED_NAN_MAX
    }
    
    pub const fn is_non_nan_number(&self)->bool{
        self.0 < Self::TYPE_NAN
    }
    
    pub const fn is_index(&self)->bool{
        self.0 <= Self::TYPE_NIL
    }
    
    pub const fn is_bool(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_BOOL
    }
    
    pub const fn is_nan(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_NAN
    }
    
    pub const fn is_nil(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_NIL
    }
    
    pub const fn is_color(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_COLOR
    }
    
    pub const fn is_id(&self)->bool{
        self.0 >= Self::TYPE_ID
    }
    
    pub const fn is_prefixed_id(&self)->bool{
        self.0 >= Self::TYPE_ID && self.0 & Id::PREFIXED != 0
    }
    
    pub const fn is_unprefixed_id(&self)->bool{
        self.0 >= Self::TYPE_ID && self.0 & Id::PREFIXED == 0
    }
            
    pub const fn is_opcode(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_OPCODE
    }
    
    pub const fn is_string(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_STRING
    }
    
    pub const fn is_object(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_OBJECT
    }
    
    pub const fn is_rsid(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_RSID
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
        if let Some(index) = self.as_rsid(){
            return write!(f, "[RsID:{}]",index)
        }
        if let Some(error) = self.as_err(){
            return write!(f, "{}", error.ty)
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
