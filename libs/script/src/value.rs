
#[derive(PartialEq, Eq, Clone, Copy)]
pub struct Value(u64);

// NaN box value

impl Value{
    pub const TYPE_NAN: u64 = 0xFFFF_0100_0000_0000;
    pub const NAN: Value = Value( Self::TYPE_NAN);
    
    pub const TYPE_BOOL: u64 = 0xFFFF_0200_0000_0000;
    pub const FALSE: Value = Value( Self::TYPE_BOOL | 0x0000_0000);
    pub const TRUE: Value = Value(Self::TYPE_BOOL | 0x0000_0001);
    
    pub const TYPE_NIL: u64 = 0xFFFF_0300_0000_0000;
    pub const NIL: Value = Value(Self::TYPE_NIL);
    
    pub const TYPE_ID: u64 = 0xFFFF_8000_0000_0000;
    
    // opcodes
    pub const TYPE_OPCODE: u64 = 0xFFFF_0400_0000_0000;
    pub const OP_PROP: Value = Value(Self::TYPE_OPCODE | 0x0000_0001);
    pub const OP_ADD: Value = Value(Self::TYPE_OPCODE | 0x0000_0002);
    
    // TODO: make this behave like javascript as much as is sensible
    
    pub fn from_f64(val:f64)->Self{
        if val.is_nan(){
            Self::NAN
        }else{
            Self(val.to_bits())
        }
    }
    
    pub fn from_bool(val: bool)->Self{
        if val{Self::TRUE}
        else{Self::FALSE}
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
    
    pub fn is_f64(&self)->bool{
        self.0 <= Self::TYPE_NAN
    }
    
    pub fn is_bool(&self)->bool{
        (self.0 & Self::TYPE_BOOL) == Self::TYPE_BOOL
    }
    
    pub fn is_nil(&self)->bool{
        (self.0 & Self::TYPE_NIL) == Self::TYPE_NIL
    }
    
    pub fn is_id(&self)->bool{
        self.0 >= Self::TYPE_ID
    }
    
    pub fn is_opcode(&self)->bool{
        (self.0 & Self::TYPE_OPCODE) == Self::TYPE_OPCODE
    }
}
