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

#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub struct OpcodeArgs(u32);

impl OpcodeArgs{
    pub const TYPE_NONE: u32 = 0;
    pub const TYPE_NIL:u32 =  1 <<29;
    pub const TYPE_NUMBER:u32 =  2 <<29;
    pub const TYPE_MASK: u32 = 3 <<29;
    pub const STATEMENT_FLAG:u32 =  1 <<31;
    
    pub const NONE: OpcodeArgs = OpcodeArgs(0);
    pub const NIL: OpcodeArgs = OpcodeArgs(Self::TYPE_NIL);
    
    pub fn from_u32(jump_to_next:u32)->Self{
        Self(Self::TYPE_NUMBER | (jump_to_next&0x1fff_ffff))
    }
    
    pub fn to_u32(&self)->u32{
        self.0 & 0x1fff_ffff
    }
    
    pub fn arg_type(&self)->u32{
        self.0 & Self::TYPE_MASK
    }
    
    pub fn is_statement(&self)->bool{
        self.0 & Self::STATEMENT_FLAG != 0
    }
    
    pub fn is_nil(&self)->bool{
        self.0 & Self::TYPE_MASK == Self::TYPE_NIL
    }
    
    pub fn is_number(&self)->bool{
        self.0 & Self::TYPE_MASK == Self::TYPE_NUMBER
    }
}


impl From<Opcode> for Value{
    fn from(v:Opcode) -> Self{
        Value::from_opcode(v)
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd)]
pub struct Opcode(u8);
impl Opcode{
    pub const NOP:Self = Self(0);
    pub const NOT:Self = Self(1);
    pub const NEG:Self = Self(2);
    pub const MUL:Self = Self(3);
    pub const DIV:Self = Self(4);
    pub const MOD:Self = Self(5);
    pub const ADD:Self = Self(6);
    pub const SUB:Self = Self(7);
    pub const SHL:Self = Self(8);
    pub const SHR:Self = Self(9);
    pub const AND:Self = Self(10);
    pub const OR:Self = Self(11);
    pub const XOR:Self = Self(12);
        
    pub const CONCAT:Self = Self(13);
    pub const EQ:Self = Self(14);
    pub const NEQ:Self = Self(15);
    pub const LT:Self = Self(16);
    pub const GT:Self = Self(17);
    pub const LEQ:Self = Self(18);
    pub const GEQ:Self = Self(19);
    pub const LOGIC_AND:Self = Self(20);
    pub const LOGIC_OR:Self = Self(21);
        
    pub const ASSIGN_FIRST:Self = Self(22);
        
    pub const ASSIGN_ME:Self = Self(22);
    pub const ASSIGN:Self = Self(23);
    pub const ASSIGN_ADD:Self = Self(24);
    pub const ASSIGN_SUB:Self = Self(25);
    pub const ASSIGN_MUL:Self = Self(26);
    pub const ASSIGN_DIV:Self = Self(27);
    pub const ASSIGN_MOD:Self = Self(28);
    pub const ASSIGN_AND:Self = Self(29);
    pub const ASSIGN_OR:Self = Self(30);
    pub const ASSIGN_XOR:Self = Self(31);
    pub const ASSIGN_SHL:Self = Self(32);
    pub const ASSIGN_SHR:Self = Self(33);
    pub const ASSIGN_IFNIL:Self = Self(34);
        
    pub const ASSIGN_FIELD:Self = Self(35);
    pub const ASSIGN_FIELD_ADD:Self = Self(36);
    pub const ASSIGN_FIELD_SUB:Self = Self(37);
    pub const ASSIGN_FIELD_MUL:Self = Self(38);
    pub const ASSIGN_FIELD_DIV:Self = Self(39);
    pub const ASSIGN_FIELD_MOD:Self = Self(40);
    pub const ASSIGN_FIELD_AND:Self = Self(41);
    pub const ASSIGN_FIELD_OR:Self = Self(42);
    pub const ASSIGN_FIELD_XOR:Self = Self(43);
    pub const ASSIGN_FIELD_SHL:Self = Self(44);
    pub const ASSIGN_FIELD_SHR:Self = Self(45);
        
    pub const ASSIGN_FIELD_IFNIL:Self = Self(46);
            
    pub const ASSIGN_INDEX:Self = Self(47);
    pub const ASSIGN_INDEX_ADD:Self = Self(48);
    pub const ASSIGN_INDEX_SUB:Self = Self(49);
    pub const ASSIGN_INDEX_MUL:Self = Self(50);
    pub const ASSIGN_INDEX_DIV:Self = Self(51);
    pub const ASSIGN_INDEX_MOD:Self = Self(52);
    pub const ASSIGN_INDEX_AND:Self = Self(53);
    pub const ASSIGN_INDEX_OR:Self = Self(54);
    pub const ASSIGN_INDEX_XOR:Self = Self(55);
    pub const ASSIGN_INDEX_SHL:Self = Self(56);
    pub const ASSIGN_INDEX_SHR:Self = Self(57);
    pub const ASSIGN_INDEX_IFNIL:Self = Self(58);    
        
    pub const ASSIGN_LAST:Self = Self(58);
        
    pub const BEGIN_PROTO:Self = Self(59);
    pub const END_PROTO:Self = Self(60);
    pub const BEGIN_BARE:Self = Self(61);
    pub const END_BARE:Self = Self(62);
    pub const BEGIN_ARRAY:Self = Self(63);
    pub const END_ARRAY:Self = Self(64);
    pub const CALL_ARGS:Self = Self(65);
    pub const CALL_EXEC:Self = Self(66);
    pub const BEGIN_FRAG:Self = Self(67);
    pub const END_FRAG:Self = Self(68);
        
    pub const FN_ARGS:Self = Self(69);
    pub const FN_ARG_DYN:Self = Self(70);
    pub const FN_ARG_TYPED:Self = Self(71);
    pub const FN_BODY:Self = Self(72);
    pub const RETURN:Self = Self(73);
        
    pub const IF_TEST:Self = Self(74);
    pub const IF_ELSE:Self = Self(75);
    
    pub const FIELD:Self = Self(76);
    pub const ARRAY_INDEX:Self = Self(77);
    // prototypically inherit the chain for deep prototype fields
    pub const PROTO_FIELD:Self = Self(78);
    pub const POP_TO_ME:Self = Self(79);
        
    pub const LET_FIRST:Self = Self(80);
    pub const LET_TYPED:Self = Self(83);
    pub const LET_DYN:Self = Self(84);
    pub const LET_LAST:Self = Self(84);
            
    pub const SEARCH_TREE:Self = Self(85);
            
}

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
    pub const TYPE_INSTRUCTION: u64 = 0xFFFF_0700_0000_0000;
    
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

impl fmt::Debug for OpcodeArgs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for OpcodeArgs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.arg_type(){
            Self::TYPE_NONE=>{write!(f,"").ok();},
            Self::TYPE_NIL=>{write!(f,"(nil)").ok();},
            Self::TYPE_NUMBER=>{write!(f,"({})",self.to_u32()).ok();},
            _=>{}
        };
        if self.is_statement(){
            write!(f,"<Stmt>")
        }
        else{
            write!(f,"")
        }
    }
}
       
impl fmt::Debug for Value {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Debug for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match *self{
            Self::MUL => return write!(f, "*"),
            Self::DIV => return write!(f, "/"),
            Self::MOD => return write!(f, "%"),
            Self::ADD => return write!(f, "+"),
            Self::SUB => return write!(f, "-"),
            Self::SHL => return write!(f, "<<"),
            Self::SHR => return write!(f, ">>"),
            Self::AND => return write!(f, "&"),
            Self::XOR => return write!(f, "^"),
            Self::OR => return write!(f, "|"),
            Self::EQ => return write!(f, "=="),
            Self::NEQ => return write!(f, "!="),
            Self::LT => return write!(f, "<"),
            Self::GT => return write!(f, ">"),
            Self::LEQ => return write!(f, "<="),
            Self::GEQ => return write!(f, ">="),
            Self::LOGIC_AND => return write!(f, "&&"),
            Self::LOGIC_OR => return write!(f, "||"),
                            
            Self::ASSIGN => return write!(f, "="),
            Self::ASSIGN_ME => return write!(f, ":"),
            Self::ASSIGN_ADD => return write!(f, "+="),
            Self::ASSIGN_SUB => return write!(f, "-="),
            Self::ASSIGN_MUL => return write!(f, "*="),
            Self::ASSIGN_DIV => return write!(f, "/="),
            Self::ASSIGN_MOD => return write!(f, "%="),
            Self::ASSIGN_AND => return write!(f, "&="),
            Self::ASSIGN_OR => return write!(f, "|="),
            Self::ASSIGN_XOR => return write!(f, "^="),
            Self::ASSIGN_SHL => return write!(f, "<<="),
            Self::ASSIGN_SHR => return write!(f, ">>="),
                            
            Self::ASSIGN_FIELD => return write!(f, ".="),
            Self::ASSIGN_FIELD_ADD => return write!(f, ".+="),
            Self::ASSIGN_FIELD_SUB => return write!(f, ".-="),
            Self::ASSIGN_FIELD_MUL => return write!(f, ".*="),
            Self::ASSIGN_FIELD_DIV => return write!(f, "./="),
            Self::ASSIGN_FIELD_MOD => return write!(f, ".%="),
            Self::ASSIGN_FIELD_AND => return write!(f, ".&="),
            Self::ASSIGN_FIELD_OR => return write!(f, ".|="),
            Self::ASSIGN_FIELD_XOR => return write!(f, ".^="),
            Self::ASSIGN_FIELD_SHL => return write!(f, ".<<="),
            Self::ASSIGN_FIELD_SHR => return write!(f, ".>>="),
            Self::ASSIGN_FIELD_IFNIL => return write!(f, ".?="),
                            
            Self::ASSIGN_INDEX => return write!(f, "[]="),
            Self::ASSIGN_INDEX_ADD => return write!(f, "[]+="),
            Self::ASSIGN_INDEX_SUB => return write!(f, "[]-="),
            Self::ASSIGN_INDEX_MUL => return write!(f, "[]*="),
            Self::ASSIGN_INDEX_DIV => return write!(f, "[]/="),
            Self::ASSIGN_INDEX_MOD => return write!(f, "[]%="),
            Self::ASSIGN_INDEX_AND => return write!(f, "[]&="),
            Self::ASSIGN_INDEX_OR => return write!(f, "[]|="),
            Self::ASSIGN_INDEX_XOR => return write!(f, "[]^="),
            Self::ASSIGN_INDEX_SHL => return write!(f, "[]<<="),
            Self::ASSIGN_INDEX_SHR => return write!(f, "[]>>="),
            Self::ASSIGN_INDEX_IFNIL => return write!(f, "[]?="),
                            
            Self::BEGIN_PROTO => return write!(f, "<proto>{{"),
            Self::END_PROTO => return write!(f, "}}"),
            Self::BEGIN_BARE => return write!(f, "<bare>{{"),
            Self::END_BARE => return write!(f, "}}"),
            Self::CALL_ARGS => return write!(f, "<call>("),
            Self::CALL_EXEC => return write!(f, ")"),
            Self::BEGIN_FRAG => return write!(f, "<frag>("),
            Self::END_FRAG => return write!(f, ")"),
                            
            Self::FN_ARGS=> return write!(f, "<fn>|"),
            Self::FN_ARG_DYN=> return write!(f, "fn arg dyn"),
            Self::FN_ARG_TYPED=> return write!(f, "fn arg typed"),
            Self::FN_BODY=> return write!(f, "|<fnbody>"),
            Self::RETURN=> return write!(f, "<return>"),
            
            Self::IF_TEST => return write!(f, "if"),
            Self::IF_ELSE => return write!(f, "else"),
                                                                        
            Self::FIELD => return write!(f, "."),
            Self::ARRAY_INDEX => return write!(f, "[]"),
                                            
            Self::PROTO_FIELD=> return write!(f, "<proto>."),
            Self::POP_TO_ME=> return write!(f, "<me>"),
            
            Self::LET_TYPED => return write!(f, "let typed"),
            Self::LET_DYN => return write!(f, "let dyn"),
                            
            Self::SEARCH_TREE => return write!(f, "$"),
            _=>return write!(f, "OP{}",self.0)
        }
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