use crate::makepad_live_id::*;
use crate::opcode::*;
use std::fmt;

#[derive(PartialEq, Eq, Clone, Copy, Hash, Ord, PartialOrd)]
pub struct ScriptValue(u64);

pub const NIL:ScriptValue = ScriptValue::NIL;
pub const TRUE:ScriptValue = ScriptValue::TRUE;
pub const FALSE:ScriptValue = ScriptValue::FALSE;

impl Default for ScriptValue{
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
    pub const fn from_u40(value:u64)->Self{
        Self{
            body: ((value >> 28)&0xFFF) as u16,
            index: ((value) & 0xFFF_FFFF) as u32
        }
    }
    pub const fn to_u40(&self)->u64{
        ((self.body as u64)<<28) | self.index as u64
    }
}

#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ScriptObject{
    pub(crate) index: u32    
}


#[derive(Default, Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ScriptArray{
    pub(crate) index: u32    
}
impl ScriptObject{
    pub const ZERO:ScriptObject = ScriptObject{index:0};
}

impl From<ScriptObject> for ScriptValue{
    fn from(v:ScriptObject) -> Self{
        ScriptValue::from_object(v)
    }
}

impl From<ScriptArray> for ScriptValue{
    fn from(v:ScriptArray) -> Self{
        ScriptValue::from_array(v)
    }
}

impl From<ScriptValue> for ScriptObject{
    fn from(v:ScriptValue) -> Self{
        if let Some(obj) = v.as_object(){
            obj
        }
        else{
            ScriptObject{index:0}
        }
    }
}

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ScriptString{
    pub index: u32    
}

impl From<ScriptString> for ScriptValue{
    fn from(v:ScriptString) -> Self{
        ScriptValue::from_string(v)
    }
}

impl From<f64> for ScriptValue{
    fn from(v:f64) -> Self{
        ScriptValue::from_f64(v)
    }
}

impl From<ScriptValue> for f64{
    fn from(v:ScriptValue) -> Self{
        v.as_f64().unwrap_or(0.0) as _
    }
}

impl From<u32> for ScriptValue{
    fn from(v:u32) -> Self{
        ScriptValue::from_f64(v as f64)
    }
}

impl From<ScriptValue> for u32{
    fn from(v:ScriptValue) -> Self{
        v.as_f64().unwrap_or(0.0) as _
    }
}

impl From<i32> for ScriptValue{
    fn from(v:i32) -> Self{
        ScriptValue::from_f64(v as f64)
    }
}

impl From<ScriptValue> for i32{
    fn from(v:ScriptValue) -> Self{
        v.as_f64().unwrap_or(0.0) as _
    }
}

impl From<u16> for ScriptValue{
    fn from(v:u16) -> Self{
        ScriptValue::from_f64(v as f64)
    }
}

impl From<ScriptValue> for u16{
    fn from(v:ScriptValue) -> Self{
        v.as_f64().unwrap_or(0.0) as _
    }
}

impl From<u8> for ScriptValue{
    fn from(v:u8) -> Self{
        ScriptValue::from_f64(v as f64)
    }
}

impl From<ScriptValue> for u8{
    fn from(v:ScriptValue) -> Self{
        v.as_f64().unwrap_or(0.0) as _
    }
}

impl From<f32> for ScriptValue{
    fn from(v:f32) -> Self{
        ScriptValue::from_f64(v as f64)
    }
}

impl From<ScriptValue> for f32{
    fn from(v:ScriptValue) -> Self{
        v.as_f64().unwrap_or(0.0) as _
    }
}

impl From<usize> for ScriptValue{
    fn from(v:usize) -> Self{
        ScriptValue::from_f64(v as f64)
    }
}

impl From<ScriptValue> for usize{
    fn from(v:ScriptValue) -> Self{
        v.as_f64().unwrap_or(0.0) as _
    }
}

impl From<bool> for ScriptValue{
    fn from(v:bool) -> Self{
        ScriptValue::from_bool(v)
    }
}

impl From<LiveId> for ScriptValue{
    fn from(v:LiveId) -> Self{
        ScriptValue::from_id(v)
    }
}

impl From<&LiveId> for ScriptValue{
    fn from(v:&LiveId) -> Self{
        ScriptValue::from_id(*v)
    }
}

impl From<Opcode> for ScriptValue{
    fn from(v:Opcode) -> Self{
        ScriptValue::from_opcode(v)
    }
}
// NaN box value

#[derive(Debug, Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ValueError{
    pub ty: ScriptValueType,
    pub ip: ScriptIp
}

#[derive(Copy, Clone, Eq, PartialEq, Ord, PartialOrd)]
pub struct ScriptValueType(u8);

impl ScriptValueType{
    pub const NUMBER: Self = Self(0);
    pub const NAN: Self = Self(1);
    pub const BOOL: Self = Self(2);
    pub const NIL: Self = Self(3);
    pub const COLOR: Self = Self(4);
    pub const OBJECT: Self = Self(5);
    pub const ARRAY: Self = Self(6);
    pub const OPCODE: Self = Self(7);
    pub const STRING: Self = Self(8);
        
    pub const REDUX_MARKER: Self = Self(9);
    pub const INLINE_STRING_0: Self = Self(9);
    pub const INLINE_STRING_1: Self = Self(10);
    pub const INLINE_STRING_2: Self = Self(11);
    pub const INLINE_STRING_3: Self = Self(12);
    pub const INLINE_STRING_4: Self = Self(13);
    pub const INLINE_STRING_5: Self = Self(14);
    pub const INLINE_STRING_END: Self = Self(15);
    
    pub const ERR_FIRST: Self = Self(16);
    pub const ERR_NOT_FOUND: Self = Self(16);
    pub const ERR_NOT_FN: Self = Self(17);
    pub const ERR_NOT_INDEX: Self = Self(19);
    pub const ERR_NOT_OBJECT: Self = Self(20);
    pub const ERR_STACK_UNDERFLOW: Self = Self(21);
    pub const ERR_STACK_OVERFLOW: Self = Self(22);
    pub const ERR_INVALID_ARGS: Self = Self(23);
    pub const ERR_NOT_ASSIGNABLE: Self = Self(24);
    pub const ERR_UNEXPECTED: Self = Self(25);
    pub const ERR_ASSERT_FAIL: Self = Self(26);
    pub const ERR_NOT_IMPL: Self = Self(27);
    pub const ERR_FROZEN: Self = Self(28);
    pub const ERR_VEC_FROZEN: Self = Self(29);
    pub const ERR_INVALID_PROP_TYPE: Self = Self(30);
    pub const ERR_INVALID_PROP_NAME: Self = Self(31);
    pub const ERR_KEY_ALREADY_EXISTS: Self = Self(32);
    pub const ERR_INVALID_KEY_TYPE: Self = Self(33);
    pub const ERR_INVALID_VAR_NAME: Self = Self(34);
    pub const ERR_USER: Self = Self(35);
    pub const ERR_VEC_BOUND: Self = Self(36);
    pub const ERR_INVALID_ARG_TYPE: Self = Self(37);
    pub const ERR_INVALID_ARG_NAME: Self = Self(38);
    pub const ERR_INVALID_ARG_COUNT: Self = Self(39);
    pub const ERR_NOT_PROTO: Self = Self(40);
    pub const ERR_TYPE_NOT_REGISTERED: Self = Self(41);
    pub const ERR_ENUM_UNKNOWN_VARIANT: Self = Self(42);
    pub const ERR_NOT_ALLOWED_IN_ARRAY: Self = Self(43);
    pub const ERR_NOT_ALLOWED_IN_ARGUMENTS: Self = Self(44);
    pub const ERR_ARRAY_BOUND: Self = Self(45);
    pub const ERR_WRONG_TYPE_IN_APPLY: Self = Self(46);
    pub const ERR_LAST: Self = Self(46);
    
    pub const ID: Self = Self(0x80);
        
    pub const REDUX_NUMBER: usize = 0;
    pub const REDUX_NAN: usize = 1;
    pub const REDUX_BOOL: usize = 2;
    pub const REDUX_NIL: usize = 3;
    pub const REDUX_COLOR: usize = 4;
    pub const REDUX_OBJECT: usize = 5;
    pub const REDUX_ARRAY: usize = 6;
    pub const REDUX_OPCODE: usize = 7;
    pub const REDUX_STRING: usize = 8;
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


impl fmt::Debug for ScriptValueType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}


impl fmt::Display for ScriptValueType {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self{
            Self::NUMBER=>write!(f,"number"),
            Self::NAN=>write!(f,"nan"),
            Self::BOOL=>write!(f,"bool"),
            Self::NIL=>write!(f,"nil"),
            Self::COLOR=>write!(f,"color"),
            Self::STRING=>write!(f,"string"),
            Self::OBJECT=>write!(f,"object"),
            Self::ARRAY=>write!(f,"array"),
            Self::OPCODE=>write!(f,"opcode"),
            Self::INLINE_STRING_0=>write!(f,"string0"),
            Self::INLINE_STRING_1=>write!(f,"string1"),
            Self::INLINE_STRING_2=>write!(f,"string2"),
            Self::INLINE_STRING_3=>write!(f,"string3"),
            Self::INLINE_STRING_4=>write!(f,"string4"),
            Self::INLINE_STRING_5=>write!(f,"string5"),
            Self::ERR_NOT_FOUND=>write!(f,"NotFound"),
            Self::ERR_NOT_FN=>write!(f,"NotAFunction"),
            Self::ERR_NOT_INDEX=>write!(f,"IndexNotFound"),
            Self::ERR_NOT_OBJECT=>write!(f,"NotAnObject"),
            Self::ERR_STACK_UNDERFLOW=>write!(f,"StackUnderflow"),
            Self::ERR_STACK_OVERFLOW=>write!(f,"StackOverflow"),
            Self::ERR_INVALID_ARGS=>write!(f,"InvalidArgs"),
            Self::ERR_NOT_ASSIGNABLE=>write!(f,"NotAssignable"),
            Self::ERR_UNEXPECTED=>write!(f,"Unexpected"),
            Self::ERR_ASSERT_FAIL=>write!(f,"AssertFailure"),
            Self::ERR_NOT_IMPL=>write!(f,"NotImplemented"),
            Self::ERR_FROZEN=>write!(f,"ObjectFrozen"),
            Self::ERR_VEC_FROZEN=>write!(f,"VecFrozen"),
            Self::ERR_INVALID_PROP_TYPE=>write!(f,"InvalidPropertyType"),
            Self::ERR_INVALID_PROP_NAME=>write!(f,"InvalidPropertyName"),
            Self::ERR_KEY_ALREADY_EXISTS=>write!(f,"KeyAlreadyExists"),
            Self::ERR_INVALID_KEY_TYPE=>write!(f,"UnsupportedKeyType"),
            Self::ERR_VEC_BOUND=>write!(f,"VecIndexOutOfBounds"),
            Self::ERR_INVALID_ARG_TYPE=>write!(f,"InvalidArgumentType"),
            Self::ERR_INVALID_ARG_NAME=>write!(f,"InvalidArgumentName"),
            Self::ERR_INVALID_ARG_COUNT=>write!(f,"InvalidArgumentCount"),
            Self::ERR_INVALID_VAR_NAME=>write!(f,"InvalidVariableName"),
            Self::ERR_NOT_PROTO=>write!(f,"NotAllowedAsPrototype"),
            Self::ERR_TYPE_NOT_REGISTERED=>write!(f,"TypeNotRegistered"),
            Self::ERR_ENUM_UNKNOWN_VARIANT=>write!(f,"EnumUnknownVariant"),
            Self::ERR_NOT_ALLOWED_IN_ARRAY=>write!(f,"NotAllowedInArray"),
            Self::ERR_NOT_ALLOWED_IN_ARGUMENTS=>write!(f,"NotAllowedInArguments"),
            Self::ERR_ARRAY_BOUND=>write!(f,"ArrayIndexOutOfBounds"),
            Self::ERR_WRONG_TYPE_IN_APPLY=>write!(f,"WrongTypeInApply"),
            Self::ERR_USER=>write!(f,"UserGenerated"),
            x if x.0 >= Self::ID.0=>write!(f,"id"),
            _=>write!(f,"ScriptValueType?")
        }
    }
}

pub trait IdExt{
    fn escape(&self)->ScriptValue;
}

impl IdExt for LiveId{
    fn escape(&self)->ScriptValue{
        ScriptValue::from_escaped_id(*self)
    }
}

macro_rules! err_fn{
    ($name:ident, $cnst:ident)=>{
        pub const fn $name(ip:ScriptIp)->Self{Self(ScriptValueType::$cnst.to_u64() | ip.to_u40())}
    }
    
}
impl ScriptValue{
    pub const TYPE_MASK: u64 = 0xFFFF_FF00_0000_0000;
        
    pub const TYPE_NAN: u64 = ScriptValueType::NAN.to_u64();
    pub const TYPE_TRACED_NAN_MAX: u64 = ScriptValueType::NAN.to_u64() | 0xFF_FFFF_FFFF;
    pub const NAN: ScriptValue = ScriptValue( Self::TYPE_NAN);
    
    pub const TYPE_BOOL: u64 = ScriptValueType::BOOL.to_u64();
    pub const FALSE: ScriptValue = ScriptValue( Self::TYPE_BOOL | 0x0000_0000);
    pub const TRUE: ScriptValue = ScriptValue(Self::TYPE_BOOL | 0x0000_0001);
    pub const EMPTY_STRING: ScriptValue = ScriptValue(Self::TYPE_INLINE_STRING_0);
    pub const TYPE_NIL: u64 = ScriptValueType::NIL.to_u64();
    pub const NIL: ScriptValue = ScriptValue(Self::TYPE_NIL);
    pub const OBJECT_ZERO: ScriptValue = ScriptValue::from_object(ScriptObject::ZERO);
    pub const TYPE_COLOR: u64 = ScriptValueType::COLOR.to_u64();
    pub const TYPE_STRING: u64 = ScriptValueType::STRING.to_u64();
    pub const TYPE_OBJECT: u64 = ScriptValueType::OBJECT.to_u64();
    pub const TYPE_ARRAY: u64 = ScriptValueType::ARRAY.to_u64();
    
    pub const TYPE_INLINE_STRING_0: u64 = ScriptValueType::INLINE_STRING_0.to_u64();
    pub const TYPE_INLINE_STRING_1: u64 = ScriptValueType::INLINE_STRING_1.to_u64();
    pub const TYPE_INLINE_STRING_2: u64 = ScriptValueType::INLINE_STRING_2.to_u64();
    pub const TYPE_INLINE_STRING_3: u64 = ScriptValueType::INLINE_STRING_3.to_u64();
    pub const TYPE_INLINE_STRING_4: u64 = ScriptValueType::INLINE_STRING_4.to_u64();
    pub const TYPE_INLINE_STRING_5: u64 = ScriptValueType::INLINE_STRING_5.to_u64();
    pub const TYPE_INLINE_STRING_END: u64 = ScriptValueType::INLINE_STRING_END.to_u64();

    pub const TYPE_ID: u64 = ScriptValueType::ID.to_u64();
    
    pub const ESCAPED_ID: u64 = 0x0000_4000_0000_0000;
    
    
    pub const fn value_type(&self)->ScriptValueType{
        if self.is_non_nan_number(){
            return ScriptValueType::NUMBER
        }
        ScriptValueType::from_u64(self.0 & Self::TYPE_MASK)
    }
    
    
    
    
    // Errors
    
    
    
    
    err_fn!(err_not_found, ERR_NOT_FOUND);
    err_fn!(err_not_fn, ERR_NOT_FN);
    err_fn!(err_not_index, ERR_NOT_INDEX);
    err_fn!(err_not_object, ERR_NOT_OBJECT);
    err_fn!(err_stack_underflow, ERR_STACK_UNDERFLOW);
    err_fn!(err_stack_overflow, ERR_STACK_OVERFLOW);
    err_fn!(err_invalid_args, ERR_INVALID_ARGS);
    err_fn!(err_not_assignable, ERR_NOT_ASSIGNABLE);
    err_fn!(err_unexpected, ERR_UNEXPECTED);
    err_fn!(err_assert_fail, ERR_ASSERT_FAIL);
    err_fn!(err_not_impl, ERR_NOT_IMPL);
    err_fn!(err_frozen, ERR_FROZEN);
    err_fn!(err_vec_frozen, ERR_VEC_FROZEN);
    err_fn!(err_invalid_prop_type, ERR_INVALID_PROP_TYPE);
    err_fn!(err_invalid_prop_name, ERR_INVALID_PROP_NAME);
    err_fn!(err_key_already_exists, ERR_KEY_ALREADY_EXISTS);
    err_fn!(err_invalid_key_type, ERR_INVALID_KEY_TYPE);
    err_fn!(err_vec_bound, ERR_VEC_BOUND);
    err_fn!(err_invalid_arg_type, ERR_INVALID_ARG_TYPE);       
    err_fn!(err_invalid_arg_name, ERR_INVALID_ARG_NAME);
    err_fn!(err_invalid_arg_count, ERR_INVALID_ARG_COUNT);
    err_fn!(err_invalid_var_name, ERR_INVALID_VAR_NAME);
        
    err_fn!(err_user, ERR_USER);
    err_fn!(err_not_proto, ERR_NOT_PROTO);
    err_fn!(err_type_not_registered, ERR_TYPE_NOT_REGISTERED);
    
    err_fn!(err_enum_unknown_variant, ERR_ENUM_UNKNOWN_VARIANT);
    err_fn!(err_not_allowed_in_array, ERR_NOT_ALLOWED_IN_ARRAY);
    err_fn!(err_not_allowed_in_arguments, ERR_NOT_ALLOWED_IN_ARGUMENTS);
    err_fn!(err_array_bound, ERR_ARRAY_BOUND);
    err_fn!(err_wrong_type_in_apply, ERR_WRONG_TYPE_IN_APPLY);
    
    pub const fn is_err(&self)->bool{(self.0&Self::TYPE_MASK) >=ScriptValueType::ERR_FIRST.to_u64() &&(self.0&Self::TYPE_MASK) <= ScriptValueType::ERR_LAST.to_u64()}
    
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
        
        
        
    // opcodes
        
        
        
    pub const TYPE_OPCODE: u64 = ScriptValueType::OPCODE.to_u64();
        
    pub const fn from_opcode(op:Opcode)->Self{ Self(Self::TYPE_OPCODE | (op.0 as u64)<<32)}
            
    pub const fn is_opcode(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_OPCODE
    }
    
    pub const fn from_opcode_args(op:Opcode, args:OpcodeArgs)->Self{ Self(Self::TYPE_OPCODE | (op.0 as u64)<<32 | (args.0 as u64))}
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
    
    
    // NIL
    
    
    
    pub const fn is_nil(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_NIL
    }
    
    
    // f64
    
    
    
    
    pub const fn from_f64(val:f64)->Self{
        if val.is_nan(){
            Self::NAN
        }
        else{
            Self(val.to_bits())
        }
    }
        
    pub const fn as_f64(&self)->Option<f64>{
        if self.is_number(){
            return Some(f64::from_bits(self.0))
        }
        None    
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
    
        
    pub const fn is_number(&self)->bool{
        self.0 <= Self::TYPE_TRACED_NAN_MAX
    }
        
    pub const fn is_non_nan_number(&self)->bool{
        self.0 < Self::TYPE_NAN
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
        
    pub const fn is_index(&self)->bool{
        self.0 <= Self::TYPE_NIL
    }
        
    pub const fn is_nan(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_NAN
    }
    
    
    
    // Object
    
    
    
    
    pub const fn from_object(ptr: ScriptObject)->Self{
         Self(ptr.index as u64 | Self::TYPE_OBJECT)
    }
    
    pub const fn is_object(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_OBJECT
    }
    
    pub const fn as_object(&self)->Option<ScriptObject>{
        if self.is_object(){
            return Some(ScriptObject{
                index: (self.0 & 0xffff_ffff) as u32
            })
        }
        None
    }
    
    
    
    // bool
    
    
    
    pub const fn from_bool(val: bool)->Self{
        if val{Self::TRUE}
        else{Self::FALSE}
    }
        
    pub const fn as_bool(&self)->Option<bool>{
        if self.is_bool(){
            return Some(self.0 == Self::TRUE.0)
        }
        None
    }
        
    pub const fn is_bool(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_BOOL
    }
    
    
    
    // color
    
    
    
    pub const fn from_color(val: u32)->Self{
        Self(val as u64|Self::TYPE_COLOR)
    }
        
    pub const fn as_color(&self)->Option<u32>{
        if self.is_color(){
            return Some((self.0&0xffff_ffff) as u32)
        }
        None
    }
            
    pub const fn is_color(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_COLOR
    }
    
    
    
    
    // array
    
    
    
    pub const fn from_array(val: ScriptArray)->Self{
        Self((val.index as u64)|Self::TYPE_ARRAY)
    }
        
    pub const fn is_array(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_ARRAY
    }
    
    pub const fn as_array(&self)->Option<ScriptArray>{
        if self.is_array(){
            Some(ScriptArray{index:(self.0 &0xFFFF_FFFF) as u32})
        }
        else{
            None
        }
    }
    
    
    
    // id
    
    
    pub const fn from_id(val: LiveId)->Self{
        Self(val.0|Self::TYPE_ID)
    }
        
    pub const fn is_id(&self)->bool{
        self.0 >= Self::TYPE_ID
    }
    
    pub const fn from_escaped_id(val: LiveId)->Self{
        Self(val.0|Self::TYPE_ID|Self::ESCAPED_ID)
    }
    
    pub const fn as_id(&self)->Option<LiveId>{
        if self.is_id(){
            return Some(LiveId(self.0&0x0000_3fff_ffff_ffff))
        }
        None
    }
        
    pub const fn is_escaped_id(&self)->bool{
        self.0 >= Self::TYPE_ID | Self::ESCAPED_ID
    }
        
    pub const fn is_prefixed_id(&self)->bool{
        self.0 >= Self::TYPE_ID && self.0 & LiveId::PREFIXED != 0
    }
        
    pub const fn is_unprefixed_id(&self)->bool{
        self.0 >= Self::TYPE_ID && self.0 & LiveId::PREFIXED == 0
    }
    
    
    // string
    
    
    
    pub const fn from_string(ptr: ScriptString)->Self{
         Self(ptr.index as u64 | Self::TYPE_STRING)
    }
        
    pub const fn as_string(&self)->Option<ScriptString>{
        if self.is_string(){
            return Some(ScriptString{
                index: (self.0 & 0xffff_ffff) as u32
            })
        }
        None
    }
        
    pub const fn is_string(&self)->bool{
        (self.0 & Self::TYPE_MASK) == Self::TYPE_STRING
    }
    
    pub const fn is_string_like(&self)->bool{
        let ty = self.0 & Self::TYPE_MASK;
        ty >= Self::TYPE_STRING && ty < Self::TYPE_INLINE_STRING_END
    }
    
    pub const fn from_inline_string(str: &str)->Option<Self>{
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

    pub const fn inline_string_not_empty(&self)->bool{
        self.0 >= Self::TYPE_INLINE_STRING_1  && self.0 <= Self::TYPE_INLINE_STRING_END
    }
        
    pub const fn is_inline_string(&self)->bool{
        self.0 >= Self::TYPE_INLINE_STRING_0  && self.0 < Self::TYPE_INLINE_STRING_END
    }
}

impl fmt::Debug for ScriptValue {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}


impl fmt::Display for ScriptValue {
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
            return write!(f, "[ScriptObject:{}]",ptr.index)
        }
        if let Some(ptr) = self.as_array(){
            return write!(f, "[ScriptArray:{}]",ptr.index)
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
