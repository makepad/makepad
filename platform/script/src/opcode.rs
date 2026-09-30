use std::fmt;

#[derive(Copy, Clone, PartialEq, Eq, Default)]
pub struct OpcodeArgs(pub(crate) u32);

impl OpcodeArgs {
    pub const TYPE_NONE: u32 = 0;
    pub const TYPE_NIL: u32 = 1 << 28;
    pub const TYPE_NUMBER: u32 = 2 << 28;
    pub const TYPE_MASK: u32 = 3 << 28;
    pub const NEED_NIL_FLAG: u32 = 1 << 31;
    pub const POP_TO_ME_FLAG: u32 = 1 << 30;
    pub const MAX_U32: u32 = (1 << 28) - 1;

    pub const NONE: Self = Self(0);
    pub const NIL: Self = Self(Self::TYPE_NIL);
    /// FIELD's and ARRAY_INDEX's argument in the left operand of `??`: a
    /// missing field or element, or one of nil, reads as nil without an
    /// error.
    pub const OPTIONAL_FIELD: Self = Self(Self::TYPE_NUMBER | 1);

    pub fn raw(&self) -> u32 {
        self.0
    }

    pub fn from_u32(jump_to_next: u32) -> Self {
        Self(Self::TYPE_NUMBER | (jump_to_next & 0x0fff_ffff))
    }

    /// The arguments without the NEED_NIL and POP_TO_ME flags.
    pub fn without_flags(self) -> Self {
        Self(self.0 & !(Self::NEED_NIL_FLAG | Self::POP_TO_ME_FLAG))
    }

    pub fn set_need_nil(self) -> Self {
        Self(self.0 | Self::NEED_NIL_FLAG)
    }

    pub fn to_u32(&self) -> u32 {
        self.0 & 0x1fff_ffff
    }

    pub fn arg_type(&self) -> u32 {
        self.0 & Self::TYPE_MASK
    }

    // pub fn is_statement(&self)->bool{
    //     self.0 & Self::STATEMENT_FLAG != 0
    // }

    pub fn is_need_nil(&self) -> bool {
        self.0 & Self::NEED_NIL_FLAG != 0
    }

    pub fn is_pop_to_me(&self) -> bool {
        self.0 & Self::POP_TO_ME_FLAG != 0
    }

    pub fn is_nil(&self) -> bool {
        self.0 & Self::TYPE_MASK == Self::TYPE_NIL
    }

    pub fn is_u32(&self) -> bool {
        self.0 & Self::TYPE_MASK == Self::TYPE_NUMBER
    }
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct Opcode(pub u8);
impl Opcode {
    pub fn raw(&self) -> u8 {
        self.0
    }

    pub const NOP: Self = Self(0);
    pub const NOT: Self = Self(1);
    pub const NEG: Self = Self(2);
    pub const MUL: Self = Self(3);
    pub const DIV: Self = Self(4);
    pub const MOD: Self = Self(5);
    pub const ADD: Self = Self(6);
    pub const SUB: Self = Self(7);
    pub const SHL: Self = Self(8);
    pub const SHR: Self = Self(9);
    pub const AND: Self = Self(10);
    pub const OR: Self = Self(11);
    pub const XOR: Self = Self(12);

    pub const CONCAT: Self = Self(13);
    pub const EQ: Self = Self(14);
    pub const NEQ: Self = Self(15);
    pub const LT: Self = Self(16);
    pub const GT: Self = Self(17);
    pub const LEQ: Self = Self(18);
    pub const GEQ: Self = Self(19);
    // Short-circuit evaluation opcodes (these replace simple binary ops)
    pub const LOGIC_AND_TEST: Self = Self(20); // If falsy, skip and keep value; else pop and continue
    pub const LOGIC_OR_TEST: Self = Self(21); // If truthy, skip and keep value; else pop and continue
    pub const NIL_OR_TEST: Self = Self(22); // If not nil, skip and keep value; else pop and continue
    pub const SHALLOW_EQ: Self = Self(23);
    pub const SHALLOW_NEQ: Self = Self(24);

    pub const fn is_assign(self) -> bool {
        self.0 >= Opcode::ASSIGN_ME.0 && self.0 <= Opcode::ASSIGN_INDEX_IFNIL.0
    }

    pub const ASSIGN_ME: Self = Self(25);
    pub const ASSIGN_ME_BEFORE: Self = Self(26);
    pub const ASSIGN_ME_AFTER: Self = Self(27);
    pub const ASSIGN_ME_BEGIN: Self = Self(28);
    pub const ASSIGN_ME_VEC: Self = Self(29);

    pub const ASSIGN: Self = Self(30);
    pub const ASSIGN_ADD: Self = Self(31);
    pub const ASSIGN_SUB: Self = Self(32);
    pub const ASSIGN_MUL: Self = Self(33);
    pub const ASSIGN_DIV: Self = Self(34);
    pub const ASSIGN_MOD: Self = Self(35);
    pub const ASSIGN_AND: Self = Self(36);
    pub const ASSIGN_OR: Self = Self(37);
    pub const ASSIGN_XOR: Self = Self(38);
    pub const ASSIGN_SHL: Self = Self(39);
    pub const ASSIGN_SHR: Self = Self(40);
    pub const ASSIGN_IFNIL: Self = Self(41);

    pub const ASSIGN_FIELD: Self = Self(42);
    pub const ASSIGN_FIELD_ADD: Self = Self(43);
    pub const ASSIGN_FIELD_SUB: Self = Self(44);
    pub const ASSIGN_FIELD_MUL: Self = Self(45);
    pub const ASSIGN_FIELD_DIV: Self = Self(46);
    pub const ASSIGN_FIELD_MOD: Self = Self(47);
    pub const ASSIGN_FIELD_AND: Self = Self(48);
    pub const ASSIGN_FIELD_OR: Self = Self(49);
    pub const ASSIGN_FIELD_XOR: Self = Self(50);
    pub const ASSIGN_FIELD_SHL: Self = Self(51);
    pub const ASSIGN_FIELD_SHR: Self = Self(52);

    pub const ASSIGN_FIELD_IFNIL: Self = Self(53);

    pub const ASSIGN_INDEX: Self = Self(54);
    pub const ASSIGN_INDEX_ADD: Self = Self(55);
    pub const ASSIGN_INDEX_SUB: Self = Self(56);
    pub const ASSIGN_INDEX_MUL: Self = Self(57);
    pub const ASSIGN_INDEX_DIV: Self = Self(58);
    pub const ASSIGN_INDEX_MOD: Self = Self(59);
    pub const ASSIGN_INDEX_AND: Self = Self(60);
    pub const ASSIGN_INDEX_OR: Self = Self(61);
    pub const ASSIGN_INDEX_XOR: Self = Self(62);
    pub const ASSIGN_INDEX_SHL: Self = Self(63);
    pub const ASSIGN_INDEX_SHR: Self = Self(64);
    pub const ASSIGN_INDEX_IFNIL: Self = Self(65);

    pub const BEGIN_PROTO: Self = Self(66);
    pub const PROTO_INHERIT_READ: Self = Self(67);
    pub const END_PROTO: Self = Self(68);
    pub const BEGIN_BARE: Self = Self(69);
    pub const END_BARE: Self = Self(70);
    pub const BEGIN_ARRAY: Self = Self(71);
    pub const END_ARRAY: Self = Self(72);

    pub const CALL_ARGS: Self = Self(73);
    pub const CALL_EXEC: Self = Self(74);
    pub const METHOD_CALL_ARGS: Self = Self(75);
    pub const METHOD_CALL_EXEC: Self = Self(76);

    pub const FN_ARGS: Self = Self(77);
    pub const FN_LET_ARGS: Self = Self(78);
    pub const FN_ARG_DYN: Self = Self(79);
    pub const FN_ARG_TYPED: Self = Self(80);
    pub const FN_BODY_DYN: Self = Self(81);
    pub const FN_BODY_TYPED: Self = Self(82);
    pub const RETURN: Self = Self(83);

    pub const IF_TEST: Self = Self(84);
    pub const IF_ELSE: Self = Self(85);

    pub const FIELD: Self = Self(86);
    pub const FIELD_NIL: Self = Self(87);
    pub const ME_FIELD: Self = Self(88);
    pub const ARRAY_INDEX: Self = Self(89);
    pub const PROTO_FIELD: Self = Self(90);
    pub const POP_TO_ME: Self = Self(91);

    pub const LET_TYPED: Self = Self(92);
    pub const LET_DYN: Self = Self(93);

    pub const SEARCH_TREE: Self = Self(94);
    pub const STRING_STREAM: Self = Self(95);
    pub const LOG: Self = Self(96);

    pub const ME: Self = Self(97);
    pub const SCOPE: Self = Self(98);

    pub const FOR_1: Self = Self(99);
    pub const FOR_2: Self = Self(100);
    pub const FOR_3: Self = Self(101);
    pub const LOOP: Self = Self(102);
    pub const BREAKIFNOT: Self = Self(103);
    pub const FOR_END: Self = Self(104);
    pub const BREAK: Self = Self(105);
    pub const CONTINUE: Self = Self(106);
    pub const RANGE: Self = Self(107);
    pub const IS: Self = Self(108);
    pub const RETURN_IF_ERR: Self = Self(109);
    pub const TRY_TEST: Self = Self(110);
    pub const TRY_ERR: Self = Self(111);
    pub const TRY_OK: Self = Self(112);
    pub const OK_TEST: Self = Self(113);
    pub const OK_END: Self = Self(114);
    pub const USE: Self = Self(115);
    pub const VAR_TYPED: Self = Self(116);
    pub const VAR_DYN: Self = Self(117);

    // Inherit opcodes
    pub const PROTO_INHERIT_WRITE: Self = Self(118);
    pub const SCOPE_INHERIT_READ: Self = Self(119);
    pub const SCOPE_INHERIT_WRITE: Self = Self(120);
    pub const FIELD_INHERIT_READ: Self = Self(121);
    pub const FIELD_INHERIT_WRITE: Self = Self(122);
    pub const INDEX_INHERIT_READ: Self = Self(123);
    pub const INDEX_INHERIT_WRITE: Self = Self(124);
    pub const ME_SPLAT: Self = Self(125);

    // Destructuring opcodes
    pub const DUP: Self = Self(126);
    pub const DROP: Self = Self(127);
    pub const LET_DESTRUCT_ARRAY_EL: Self = Self(128); // arg = index; stack: [source, id] -> [source], binds id = source[index]
    pub const LET_DESTRUCT_OBJECT_EL: Self = Self(129); // stack: [source, id] -> [source], binds id = source[id]
    pub const ARRAY_INDEX_NIL: Self = Self(130);

    // Frame-slot opcodes: lexically resolved fn locals (see thread.slots).
    // Each slot form occupies exactly the same opcode-stream shape as the
    // dynamic form it replaces, so the parser can patch back in place when a
    // body turns out to be ineligible (closure, use, scope, ...).
    pub const SLOTS_FRAME: Self = Self(131); // arg = slot count; establish frame
    pub const ARGS_TO_SLOTS: Self = Self(132); // arg = declared arg count; copy args scope->slots 0..n
    pub const PUSH_SLOT: Self = Self(133); // arg = slot; push slots[base+slot]
    pub const LET_SLOT: Self = Self(134); // arg = slot; stack [id, value] -> [], slot = value
    pub const STORE_SLOT: Self = Self(135); // arg = slot; stack [id, value] -> [nil], slot = value
    pub const ASSIGN_SLOT_ADD: Self = Self(136); // arg = slot; stack [id, value] -> [nil]
    pub const ASSIGN_SLOT_SUB: Self = Self(137);
    pub const ASSIGN_SLOT_MUL: Self = Self(138);
    pub const ASSIGN_SLOT_DIV: Self = Self(139);
    pub const ASSIGN_SLOT_MOD: Self = Self(140);

    // Push nil: the value of an if/match arm that leaves none, where the
    // construct's value is used. The shader compiler ignores it.
    pub const NIL_ARM: Self = Self(141);
}

impl fmt::Debug for OpcodeArgs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl fmt::Display for OpcodeArgs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.arg_type() {
            Self::TYPE_NONE => {
                write!(f, "").ok();
            }
            Self::TYPE_NIL => {
                write!(f, "(nil)").ok();
            }
            Self::TYPE_NUMBER => {
                write!(f, "({})", self.to_u32()).ok();
            }
            _ => {}
        };
        //if self.is_statement(){
        //    write!(f,"<st>").ok();
        //}
        if self.is_pop_to_me() {
            write!(f, "<m>").ok();
        }
        write!(f, "")
    }
}

impl fmt::Debug for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

impl Opcode {
    /// The opcode's mnemonic, `None` for an unassigned byte.
    pub fn name(&self) -> Option<&'static str> {
        match *self {
            Self::NOP => Some("nop"),
            Self::NOT => Some("!"),
            Self::NEG => Some("neg"),
            Self::MUL => Some("*"),
            Self::DIV => Some("/"),
            Self::MOD => Some("%"),
            Self::ADD => Some("+"),
            Self::SUB => Some("-"),
            Self::SHL => Some("<<"),
            Self::SHR => Some(">>"),
            Self::AND => Some("&"),
            Self::XOR => Some("^"),
            Self::OR => Some("|"),
            Self::EQ => Some("=="),
            Self::NEQ => Some("!="),
            Self::LT => Some("<"),
            Self::GT => Some(">"),
            Self::LEQ => Some("<="),
            Self::GEQ => Some(">="),
            Self::LOGIC_AND_TEST => Some("&&?"),
            Self::LOGIC_OR_TEST => Some("||?"),
            Self::NIL_OR_TEST => Some("|??"),
            Self::SHALLOW_EQ => Some("==="),
            Self::SHALLOW_NEQ => Some("!=="),

            Self::ASSIGN => Some("="),
            Self::ASSIGN_ME => Some(":"),
            Self::ASSIGN_ME_VEC => Some(":="),
            Self::ASSIGN_ADD => Some("+="),
            Self::ASSIGN_SUB => Some("-="),
            Self::ASSIGN_MUL => Some("*="),
            Self::ASSIGN_DIV => Some("/="),
            Self::ASSIGN_MOD => Some("%="),
            Self::ASSIGN_AND => Some("&="),
            Self::ASSIGN_OR => Some("|="),
            Self::ASSIGN_XOR => Some("^="),
            Self::ASSIGN_SHL => Some("<<="),
            Self::ASSIGN_SHR => Some(">>="),
            Self::ASSIGN_IFNIL => Some("?="),

            Self::ASSIGN_FIELD => Some(".="),
            Self::ASSIGN_FIELD_ADD => Some(".+="),
            Self::ASSIGN_FIELD_SUB => Some(".-="),
            Self::ASSIGN_FIELD_MUL => Some(".*="),
            Self::ASSIGN_FIELD_DIV => Some("./="),
            Self::ASSIGN_FIELD_MOD => Some(".%="),
            Self::ASSIGN_FIELD_AND => Some(".&="),
            Self::ASSIGN_FIELD_OR => Some(".|="),
            Self::ASSIGN_FIELD_XOR => Some(".^="),
            Self::ASSIGN_FIELD_SHL => Some(".<<="),
            Self::ASSIGN_FIELD_SHR => Some(".>>="),
            Self::ASSIGN_FIELD_IFNIL => Some(".?="),

            Self::ASSIGN_INDEX => Some("[]="),
            Self::ASSIGN_INDEX_ADD => Some("[]+="),
            Self::ASSIGN_INDEX_SUB => Some("[]-="),
            Self::ASSIGN_INDEX_MUL => Some("[]*="),
            Self::ASSIGN_INDEX_DIV => Some("[]/="),
            Self::ASSIGN_INDEX_MOD => Some("[]%="),
            Self::ASSIGN_INDEX_AND => Some("[]&="),
            Self::ASSIGN_INDEX_OR => Some("[]|="),
            Self::ASSIGN_INDEX_XOR => Some("[]^="),
            Self::ASSIGN_INDEX_SHL => Some("[]<<="),
            Self::ASSIGN_INDEX_SHR => Some("[]>>="),
            Self::ASSIGN_INDEX_IFNIL => Some("[]?="),

            Self::BEGIN_PROTO => Some("<proto>{"),
            Self::PROTO_INHERIT_READ => Some("<proto_inh_rd>"),
            Self::END_PROTO => Some("}"),
            Self::PROTO_INHERIT_WRITE => Some("<proto_inh_wr>"),
            Self::SCOPE_INHERIT_READ => Some("<scope_inh_rd>"),
            Self::SCOPE_INHERIT_WRITE => Some("<scope_inh_wr>"),
            Self::FIELD_INHERIT_READ => Some("<field_inh_rd>"),
            Self::FIELD_INHERIT_WRITE => Some("<field_inh_wr>"),
            Self::INDEX_INHERIT_READ => Some("<idx_inh_rd>"),
            Self::INDEX_INHERIT_WRITE => Some("<idx_inh_wr>"),
            Self::BEGIN_BARE => Some("<bare>{"),
            Self::END_BARE => Some("}"),

            Self::METHOD_CALL_ARGS => Some("<methodcall>("),
            Self::METHOD_CALL_EXEC => Some(")<method_exec>"),
            Self::CALL_ARGS => Some("<call>("),
            Self::CALL_EXEC => Some(")<call_exec>"),

            Self::BEGIN_ARRAY => Some("["),
            Self::END_ARRAY => Some("]"),

            Self::FN_ARGS => Some("<fn>|"),
            Self::FN_LET_ARGS => Some("<fn_let>|"),
            Self::FN_ARG_DYN => Some("fnarg"),
            Self::FN_ARG_TYPED => Some("fnarg_ty"),
            Self::FN_BODY_DYN => Some("|<body>"),
            Self::FN_BODY_TYPED => Some("|<body_ty>"),
            Self::RETURN => Some("return"),

            Self::IF_TEST => Some("if"),
            Self::IF_ELSE => Some("else"),

            Self::FIELD => Some("."),
            Self::FIELD_NIL => Some(".?"),
            Self::ME_FIELD => Some("me."),
            Self::ARRAY_INDEX => Some("[]"),

            Self::PROTO_FIELD => Some("<proto>."),
            Self::POP_TO_ME => Some("<M>"),

            Self::LET_TYPED => Some("let_ty"),
            Self::LET_DYN => Some("let"),
            Self::VAR_TYPED => Some("var_ty"),
            Self::VAR_DYN => Some("var"),

            Self::SEARCH_TREE => Some("$"),
            Self::STRING_STREAM => Some("<STRINGSTREAM>"),
            Self::LOG => Some("log()"),
            Self::ME => Some("me"),
            Self::SCOPE => Some("scope"),

            Self::FOR_1 => Some("for1"),
            Self::FOR_2 => Some("for2"),
            Self::FOR_3 => Some("for2"),
            Self::FOR_END => Some("forend"),
            Self::LOOP => Some("loop"),
            Self::BREAKIFNOT => Some("breakifnot"),

            Self::BREAK => Some("break"),
            Self::CONTINUE => Some("continue"),

            Self::RANGE => Some(".."),
            Self::IS => Some("is"),
            Self::RETURN_IF_ERR => Some("?"),
            Self::TRY_TEST => Some("try_test"),
            Self::TRY_ERR => Some("try_err"),
            Self::TRY_OK => Some("try_ok"),
            Self::USE => Some("use"),
            Self::OK_TEST => Some("ok_test"),
            Self::OK_END => Some("ok_end"),
            Self::ME_SPLAT => Some("..splat"),
            Self::DUP => Some("dup"),
            Self::DROP => Some("drop"),
            Self::LET_DESTRUCT_ARRAY_EL => Some("let_arr_el"),
            Self::LET_DESTRUCT_OBJECT_EL => Some("let_obj_el"),
            Self::ARRAY_INDEX_NIL => Some("?[]"),
            Self::SLOTS_FRAME => Some("slots_frame"),
            Self::ARGS_TO_SLOTS => Some("args_to_slots"),
            Self::PUSH_SLOT => Some("push_slot"),
            Self::LET_SLOT => Some("let_slot"),
            Self::STORE_SLOT => Some("store_slot"),
            Self::ASSIGN_SLOT_ADD => Some("slot+="),
            Self::ASSIGN_SLOT_SUB => Some("slot-="),
            Self::ASSIGN_SLOT_MUL => Some("slot*="),
            Self::ASSIGN_SLOT_DIV => Some("slot/="),
            Self::ASSIGN_SLOT_MOD => Some("slot%="),
            Self::NIL_ARM => Some("nil_arm"),
            _ => None,
        }
    }
}

impl fmt::Display for Opcode {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self.name() {
            Some(name) => f.write_str(name),
            None => write!(f, "OP{}", self.0),
        }
    }
}
