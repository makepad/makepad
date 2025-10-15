use std::fmt;

#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub struct OpcodeArgs(pub(crate) u32);

impl OpcodeArgs{
    pub const TYPE_NONE: u32 = 0;
    pub const TYPE_NIL:u32 =  1 <<28;
    pub const TYPE_NUMBER:u32 =  2 <<28;
    pub const TYPE_MASK: u32 = 3 <<28;
    pub const STATEMENT_FLAG:u32 =  1 <<31;
    pub const POP_TO_ME_FLAG:u32 =  1 <<30;
    pub const MAX_U32: u32 = (1<<28) - 1;
    
    pub const NONE: Self = Self(0);
    pub const NIL: Self = Self(Self::TYPE_NIL);
    
    pub fn raw(&self)->u32{
        self.0
    }
        
    pub fn from_u32(jump_to_next:u32)->Self{
        Self(Self::TYPE_NUMBER | (jump_to_next&0x0fff_ffff))
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
    
    pub fn is_pop_to_me(&self)->bool{
        self.0 & Self::POP_TO_ME_FLAG != 0
    }
        
    pub fn is_nil(&self)->bool{
        self.0 & Self::TYPE_MASK == Self::TYPE_NIL
    }
        
    pub fn is_u32(&self)->bool{
        self.0 & Self::TYPE_MASK == Self::TYPE_NUMBER
    }
}


#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Opcode(pub u8);
impl Opcode{
        
    pub fn raw(&self)->u8{
        self.0
    }
    
    
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
    pub const NIL_OR:Self = Self(22);
    pub const SHALLOW_EQ:Self = Self(23);
    pub const SHALLOW_NEQ:Self = Self(24);
         
    pub const fn is_assign(self)->bool{self.0 >= Opcode::ASSIGN_ME.0 && self.0 <= Opcode::ASSIGN_INDEX_IFNIL.0}
                    
    pub const ASSIGN_ME:Self = Self(25);
    pub const ASSIGN_ME_BEFORE:Self = Self(26);
    pub const ASSIGN_ME_AFTER:Self = Self(27);
    pub const ASSIGN_ME_BEGIN:Self = Self(28);
            
    pub const ASSIGN:Self = Self(29);
    pub const ASSIGN_ADD:Self = Self(30);
    pub const ASSIGN_SUB:Self = Self(31);
    pub const ASSIGN_MUL:Self = Self(32);
    pub const ASSIGN_DIV:Self = Self(33);
    pub const ASSIGN_MOD:Self = Self(34);
    pub const ASSIGN_AND:Self = Self(35);
    pub const ASSIGN_OR:Self = Self(36);
    pub const ASSIGN_XOR:Self = Self(37);
    pub const ASSIGN_SHL:Self = Self(38);
    pub const ASSIGN_SHR:Self = Self(39);
    pub const ASSIGN_IFNIL:Self = Self(40);
            
    pub const ASSIGN_FIELD:Self = Self(41);
    pub const ASSIGN_FIELD_ADD:Self = Self(42);
    pub const ASSIGN_FIELD_SUB:Self = Self(43);
    pub const ASSIGN_FIELD_MUL:Self = Self(44);
    pub const ASSIGN_FIELD_DIV:Self = Self(45);
    pub const ASSIGN_FIELD_MOD:Self = Self(46);
    pub const ASSIGN_FIELD_AND:Self = Self(47);
    pub const ASSIGN_FIELD_OR:Self = Self(48);
    pub const ASSIGN_FIELD_XOR:Self = Self(49);
    pub const ASSIGN_FIELD_SHL:Self = Self(50);
    pub const ASSIGN_FIELD_SHR:Self = Self(51);
            
    pub const ASSIGN_FIELD_IFNIL:Self = Self(52);
                
    pub const ASSIGN_INDEX:Self = Self(53);
    pub const ASSIGN_INDEX_ADD:Self = Self(54);
    pub const ASSIGN_INDEX_SUB:Self = Self(55);
    pub const ASSIGN_INDEX_MUL:Self = Self(56);
    pub const ASSIGN_INDEX_DIV:Self = Self(57);
    pub const ASSIGN_INDEX_MOD:Self = Self(58);
    pub const ASSIGN_INDEX_AND:Self = Self(59);
    pub const ASSIGN_INDEX_OR:Self = Self(60);
    pub const ASSIGN_INDEX_XOR:Self = Self(61);
    pub const ASSIGN_INDEX_SHL:Self = Self(62);
    pub const ASSIGN_INDEX_SHR:Self = Self(63);
    pub const ASSIGN_INDEX_IFNIL:Self = Self(64);    

    pub const BEGIN_PROTO:Self = Self(65);
    pub const BEGIN_PROTO_ME:Self = Self(66);
    pub const END_PROTO:Self = Self(67);
    pub const BEGIN_BARE:Self = Self(68);
    pub const END_BARE:Self = Self(69);
    pub const BEGIN_ARRAY:Self = Self(70);
    pub const END_ARRAY:Self = Self(71);
    
    pub const CALL_ARGS:Self = Self(72);
    pub const CALL_EXEC:Self = Self(73);
    pub const METHOD_CALL_ARGS:Self = Self(74);
    pub const METHOD_CALL_EXEC:Self = Self(75);
    
    pub const FN_ARGS:Self = Self(76);
    pub const FN_ARG_DYN:Self = Self(77);
    pub const FN_ARG_TYPED:Self = Self(78);
    pub const FN_BODY:Self = Self(79);
    pub const RETURN:Self = Self(80);
            
    pub const IF_TEST:Self = Self(81);
    pub const IF_ELSE:Self = Self(82);
        
    pub const FIELD:Self = Self(83);
    pub const FIELD_NIL: Self = Self(84);
    pub const ME_FIELD:Self = Self(85);
    pub const ARRAY_INDEX:Self = Self(86);
    // prototypically inherit the chain for deep prototype fields
    pub const PROTO_FIELD:Self = Self(87);
    pub const POP_TO_ME:Self = Self(88);
            
    pub const LET_TYPED:Self = Self(89);
    pub const LET_DYN:Self = Self(90);
                
    pub const SEARCH_TREE:Self = Self(91);
    pub const STRING_STREAM:Self = Self(92);
    pub const LOG: Self = Self(93);
    
    pub const ME: Self = Self(94);
    pub const DELETE: Self = Self(95);
    pub const SCOPE: Self = Self(96);
        
    pub const FOR_1: Self = Self(97);
    pub const FOR_2: Self = Self(98);
    pub const FOR_3: Self = Self(99);
    pub const LOOP: Self = Self(101);
    pub const BREAKIFNOT: Self = Self(103);
    pub const FOR_END: Self = Self(104);
    pub const BREAK: Self = Self(105);
    pub const CONTINUE: Self = Self(106);
    pub const RANGE: Self = Self(107);
    pub const IS: Self = Self(108);
    pub const RETURN_IF_ERR:Self = Self(109);
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
            write!(f,"<st>").ok();
        }
        if self.is_pop_to_me(){
            write!(f,"<m>").ok();
        }
        write!(f,"")
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
            Self::NIL_OR => return write!(f, "|?"),
            Self::SHALLOW_EQ => return write!(f, "==="),
            Self::SHALLOW_NEQ => return write!(f, "!=="),
                                                    
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
            Self::ASSIGN_IFNIL => return write!(f, "?="),
                        
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
            Self::BEGIN_PROTO_ME => return write!(f, "<protome>{{"),
            Self::END_PROTO => return write!(f, "}}"),
            Self::BEGIN_BARE => return write!(f, "<bare>{{"),
            Self::END_BARE => return write!(f, "}}"),
            
            Self::METHOD_CALL_ARGS => return write!(f, "<methodcall>("),
            Self::METHOD_CALL_EXEC => return write!(f, ")"),
            Self::CALL_ARGS => return write!(f, "<call>("),
            Self::CALL_EXEC => return write!(f, ")"),
            
            Self::BEGIN_ARRAY => return write!(f, "["),
            Self::END_ARRAY => return write!(f, "]"),
                                        
            Self::FN_ARGS=> return write!(f, "<fn>|"),
            Self::FN_ARG_DYN=> return write!(f, "fnarg"),
            Self::FN_ARG_TYPED=> return write!(f, "fnarg_ty"),
            Self::FN_BODY=> return write!(f, "|<body>"),
            Self::RETURN=> return write!(f, "return"),
                        
            Self::IF_TEST => return write!(f, "if"),
            Self::IF_ELSE => return write!(f, "else"),
            
            Self::FIELD => return write!(f, "."),
            Self::FIELD_NIL => return write!(f, ".?"),
            Self::ME_FIELD => return write!(f, "me."),
            Self::ARRAY_INDEX => return write!(f, "[]"),
                                                        
            Self::PROTO_FIELD=> return write!(f, "<proto>."),
            Self::POP_TO_ME=> return write!(f, "<M>"),
                        
            Self::LET_TYPED => return write!(f, "let_ty"),
            Self::LET_DYN => return write!(f, "let"),
                                        
            Self::SEARCH_TREE => return write!(f, "$"),
            Self::STRING_STREAM => return write!(f, "<STRINGSTREAM>"),
            Self::LOG => return write!(f, "log()"),
            Self::ME => return write!(f, "me"),
            Self::SCOPE => return write!(f, "scope"),
                      
            Self::FOR_1 => return write!(f, "for1"),
            Self::FOR_2 => return write!(f, "for2"),
            Self::FOR_3 => return write!(f, "for2"),
            Self::FOR_END => return write!(f, "forend"),
            Self::LOOP => return write!(f, "loop"),
            Self::BREAKIFNOT => return write!(f, "breakifnot"),

            Self::BREAK => return write!(f, "break"),
            Self::CONTINUE => return write!(f, "continue"),
            
            
            Self::RANGE => return write!(f, ".."),
            Self::IS => return write!(f, "is"),
            Self::RETURN_IF_ERR => return write!(f, "?"),
            _=>return write!(f, "OP{}",self.0)
        }
    }
}
