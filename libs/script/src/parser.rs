use crate::tokenizer::*;
use crate::makepad_live_id::live_id::*;
use crate::value::*;
use crate::opcode::*;
use crate::makepad_live_id::makepad_live_id_macros::*;
use crate::makepad_error_log::*;

macro_rules! error {
    ($self:expr, $tokenizer:expr, $($arg:tt)*) => {
        $self.report_error($tokenizer, format!("{} (from: {}:{})", format!($($arg)*), file!(), line!()))
    }
}

#[derive(Debug, Eq, PartialEq)]
enum State{
    BeginStmt{last_was_sep:bool},
    BeginExpr{required:bool},
    EndExpr,
    EndStmt{last:u32},
    
    EscapedId,
    
    ForIdent{idents:u32, index:u32},
    ForBody{idents:u32, index:u32},
    ForExpr{code_start:u32},
    ForBlock{code_start:u32},
    Loop{index:u32},
    While{index:u32},
    WhileTest{code_start:u32},
    
    IfTest{index: u32},
    IfTrueExpr{if_start:u32},
    IfTrueBlock{if_start:u32, last_was_sep:bool},
    IfMaybeElse{if_start:u32, was_block:bool},
    IfElse{else_start:u32},
    IfElseExpr{else_start:u32},
    IfElseBlock{else_start:u32, last_was_sep:bool},
    
    OkTest{index: u32},
    OkTestBlock{ok_start:u32, last_was_sep:bool},
    OkTestExpr{ok_start:u32},
      
    TryTest{index: u32},
    TryTestBlock{try_start:u32, last_was_sep:bool},
    TryTestExpr{try_start:u32},
    TryErrBlockOrExpr,
    TryErrBlock{err_start:u32, last_was_sep:bool},
    TryErrExpr{err_start:u32},
    TryOk{was_block:bool},
    TryOkBlockOrExpr,
    TryOkBlock{ok_start:u32, last_was_sep:bool},
    TryOkExpr{ok_start:u32},
    
    FnMaybeLet{index:u32},
    FnLetMaybeArgs,
    FnArgList{lambda:bool},
    FnArgMaybeType{lambda:bool,index:u32},
    FnArgType{lambda:bool,index:u32},
    FnArgTypeAssign{lambda:bool,index:u32},
          
    FnBody{lambda:bool},
    FnReturnType{lambda:bool},
    FnBodyTyped{lambda:bool},
    EndFnBlock{fn_slot:u32, last_was_sep:bool, index:u32},
    EndFnExpr{fn_slot:u32, index:u32},
    EmitFnArgTyped{index:u32},
    EmitFnArgDyn{index:u32},
    
    EmitUnary{what_op:LiveId, index:u32},
    EmitSplat{index:u32},
    EmitOp{what_op:LiveId, index:u32},
    EmitFieldAssign{what_op:LiveId, index:u32},
    EmitIndexAssign{what_op:LiveId, index:u32},
    
    EndBare,
    EndBareSquare,
    EndProto,
    EndProtoInherit,
    EndScopeInherit,
    EndFieldInherit,
    EndIndexInherit,
    EndRound,
    
    CallMaybeDo{is_method:bool, index:u32},
    EmitCallFromDo{is_method:bool, index:u32},
    EndCall{is_method:bool, index:u32},
    ArrayIndex,
    
    EmitDelete{index:u32},
    EmitReturn{index:u32},
    EmitBreak{index:u32},
    EmitContinue{index:u32},
    
    Use{index:u32},
    Let{index:u32},
    LetDynOrTyped{index:u32},
    LetType{index:u32},
    LetTypedAssign{index:u32},
    EmitLetDyn{index:u32},
    EmitLetTyped{index:u32},
    
    // Destructuring patterns
    // We collect binding ids in the code stream, then after RHS we emit extraction opcodes
    // Defaults are stored separately and ?= is emitted after extraction
    LetArrayDestruct{index:u32, count:u32, ids_start:u32},      // parsing [x, y, ...] pattern
    LetArrayDestructEl{index:u32, count:u32, binding_id:LiveId, ids_start:u32},  // after identifier, maybe = default
    LetArrayDestructDefault{index:u32, count:u32, binding_id:LiveId, ids_start:u32, default_start:u32},  // parsing default expr
    LetArrayDestructRhs{index:u32, count:u32, ids_start:u32, defaults_start:u32},   // after ], before =
    EmitLetArrayDestruct{index:u32, count:u32, ids_start:u32, defaults_start:u32},  // after RHS, emit opcodes
    
    // Nested object pattern inside array: let [{x}] = ...
    LetArrayDestructNestedObject{index:u32, outer_count:u32, outer_ids_start:u32, nested_count:u32},
    LetArrayDestructNestedObjectEl{index:u32, outer_count:u32, outer_ids_start:u32, nested_count:u32, binding_id:LiveId},
    
    // Nested array pattern inside array: let [[x]] = ...
    LetArrayDestructNestedArray{index:u32, outer_count:u32, outer_ids_start:u32, nested_count:u32},
    LetArrayDestructNestedArrayEl{index:u32, outer_count:u32, outer_ids_start:u32, nested_count:u32, binding_id:LiveId},
    
    LetObjectDestruct{index:u32, count:u32, ids_start:u32},     // parsing {x, y, ...} pattern
    LetObjectDestructEl{index:u32, count:u32, binding_id:LiveId, ids_start:u32},   // after identifier, maybe = default
    LetObjectDestructDefault{index:u32, count:u32, binding_id:LiveId, ids_start:u32, default_start:u32},  // parsing default expr
    LetObjectDestructRhs{index:u32, count:u32, ids_start:u32, defaults_start:u32},  // after }, before =
    EmitLetObjectDestruct{index:u32, count:u32, ids_start:u32, defaults_start:u32}, // after RHS, emit opcodes
    
    // Nested array pattern inside object: let {a: [x]} = ...
    LetObjectDestructNestedArray{index:u32, outer_count:u32, outer_ids_start:u32, key:LiveId, nested_count:u32},
    LetObjectDestructNestedArrayEl{index:u32, outer_count:u32, outer_ids_start:u32, key:LiveId, nested_count:u32, binding_id:LiveId},
    
    // Nested object pattern inside object: let {a: {x}} = ...
    LetObjectDestructNestedObject{index:u32, outer_count:u32, outer_ids_start:u32, key:LiveId, nested_count:u32},
    LetObjectDestructNestedObjectEl{index:u32, outer_count:u32, outer_ids_start:u32, key:LiveId, nested_count:u32, binding_id:LiveId},
    Var{index:u32},
    VarDynOrTyped{index:u32},
    VarType{index:u32},
    VarTypedAssign{index:u32},
    EmitVarDyn{index:u32},
    EmitVarTyped{index:u32},
    
    MatchSubject{temp_id:LiveId, index:u32},
    MatchBlock{temp_id:LiveId, index:u32},
    MatchArmPattern{temp_id:LiveId, first:bool, prev_else_start:u32, index:u32},
    MatchArmArrow{temp_id:LiveId, prev_else_start:u32, index:u32},
    MatchArmBody{temp_id:LiveId, if_start:u32, prev_else_start:u32, index:u32},
    MatchArmBlock{temp_id:LiveId, if_start:u32, prev_else_start:u32, last_was_sep:bool, index:u32},
    MatchWildcardArrow{prev_else_start:u32, index:u32},
    MatchWildcardBody{prev_else_start:u32, index:u32},
    MatchWildcardBlock{prev_else_start:u32, last_was_sep:bool, index:u32},
    MatchMaybeArm{temp_id:LiveId, if_start:u32, prev_else_start:u32, index:u32},
    MatchWildcardEnd{prev_else_start:u32, index:u32},
    
    // Short-circuit evaluation - patches TEST opcode jump after second operand
    ShortCircuitEnd{test_slot:u32},
    // Short-circuit ?= - emits ASSIGN after RHS, then patches jump
    ShortCircuitAssignEnd{test_slot:u32, index:u32},
}

impl State {
    fn is_short_circuit_op(op: LiveId) -> bool {
        op == id!(&&) || op == id!(||) || op == id!(|?)
    }
    
    fn short_circuit_opcode(op: LiveId) -> Opcode {
        match op {
            id!(&&) => Opcode::LOGIC_AND_TEST,
            id!(||) => Opcode::LOGIC_OR_TEST,
            id!(|?) => Opcode::NIL_OR_TEST,
            _ => Opcode::NOP,
        }
    }
}

// we have a stack, and we have operations
// operators:
/*
Order list from highest prio to lowest
1 Identifierpath 
2 Method calls
3 Field expression
4 Functioncall, array index
5 ?
6 unary - ! * borrow
7 as
8 * /  %
9 + -
10 << >>
11 &
12 ^
13 | 
14 == != < > <= >=
15 &&
16 ||
17 = += -= *= /= %=
18 &= |= ^= <<= >>=
19 return break
*/

impl State{
    fn operator_order(op:LiveId)->usize{
        match op{
            id!(.) => 3,
            id!(.?) => 3,
            id!(*) | id!(/) | id!(%) => 8,
            id!(+) | id!(-) => 9,
            id!(<<) | id!(>>) => 10,
            id!(&)  => 11,
            id!(^)  => 12,
            id!(|)  => 13,
            id!(-:) => 14,
            id!(++) => 14,
            id!(===) | id!(!==) |id!(==) | id!(!=)  | id!(<) | id!(>) | id!(<=) | id!(>=) => 15,
            id!(is) => 15,
            id!(&&)  => 16,
            id!(||) | id!(|?)  => 17,
            id!(..) =>  18,
            id!(:) | id!(=) | id!(>:) | id!(<:) | id!(^:) | id!(+:) | id!(+=)  | id!(-=) | id!(*=) | id!(/=) | id!(%=) => 19,
            id!(&=) | id!(|=)  | id!(^=) | id!(<<=) | id!(>>=) | id!(?=) => 20,
            _=>0
        }
    }
    
    fn is_assign_operator(op:LiveId)->bool{
        match op{
            id!(=) | id!(:) | id!(+=) | id!(<:) | id!(+:) | id!(+=) |
            id!(-=) | id!(*=) | id!(/=) |
            id!(%=) | id!(&=) | id!(|=) | 
            id!(^=) | id!(<<=) | id!(>>=) | 
            id!(?=)  => true,
            _=>false
        }
    }
    
    fn operator_supports_inline_number(op:LiveId)->bool{
        match op{
            id!(*) | id!(/) | id!(%) |
            id!(+)| id!(-) | id!(<<) |id!(>>) | 
            id!(&) | id!(^) | id!(|) | 
            id!(<) | id!(>) | id!(<=) | id!(>=) => true,
            _=> false
        }
    }
    
    fn operator_to_field_assign(op:LiveId)->ScriptValue{
        match op{
            id!(=) | id!(:) => Opcode::ASSIGN_FIELD,
            id!(+=) => Opcode::ASSIGN_FIELD_ADD,
            id!(-=) => Opcode::ASSIGN_FIELD_SUB,
            id!(*=) => Opcode::ASSIGN_FIELD_MUL,
            id!(/=) => Opcode::ASSIGN_FIELD_DIV,
            id!(%=) => Opcode::ASSIGN_FIELD_MOD, 
            id!(&=) => Opcode::ASSIGN_FIELD_AND,
            id!(|=) => Opcode::ASSIGN_FIELD_OR,
            id!(^=) => Opcode::ASSIGN_FIELD_XOR,
            id!(<<=) => Opcode::ASSIGN_FIELD_SHL,
            id!(>>=) => Opcode::ASSIGN_FIELD_SHR,
            id!(?=) => Opcode::ASSIGN_FIELD_IFNIL,
            _=>Opcode::NOP,
        }.into()
    }
    
    fn operator_to_index_assign(op:LiveId)->ScriptValue{
        match op{
            id!(=) => Opcode::ASSIGN_INDEX,
            id!(+=) => Opcode::ASSIGN_INDEX_ADD,
            id!(-=) => Opcode::ASSIGN_INDEX_SUB,
            id!(*=) => Opcode::ASSIGN_INDEX_MUL,
            id!(/=) => Opcode::ASSIGN_INDEX_DIV,
            id!(%=) => Opcode::ASSIGN_INDEX_MOD, 
            id!(&=) => Opcode::ASSIGN_INDEX_AND,
            id!(|=) => Opcode::ASSIGN_INDEX_OR,
            id!(^=) => Opcode::ASSIGN_INDEX_XOR,
            id!(<<=) => Opcode::ASSIGN_INDEX_SHL,
            id!(>>=) => Opcode::ASSIGN_INDEX_SHR,
            id!(?=) => Opcode::ASSIGN_INDEX_IFNIL,
            _=>Opcode::NOP,
        }.into()
    }
    
    fn operator_to_unary(op:LiveId)->ScriptValue{
        match op{
            id!(~)=> Opcode::LOG,
            id!(!)=> Opcode::NOT,
            id!(-)=> Opcode::NEG,
            id!(+)=> Opcode::NOP,
            _=>Opcode::NOP
        }.into()
    }
    
    fn operator_to_opcode(op:LiveId)->ScriptValue{
        match op{
            id!(*) => Opcode::MUL,
            id!(/) => Opcode::DIV,
            id!(%) => Opcode::MOD,
            id!(+) => Opcode::ADD,
            id!(-) => Opcode::SUB,
            id!(<<) => Opcode::SHL,
            id!(>>) => Opcode::SHR,
            id!(&)  => Opcode::AND,
            id!(^) => Opcode::XOR,
            id!(|)  => Opcode::OR,
            id!(++)  => Opcode::CONCAT,
            id!(is) => Opcode::IS,
            id!(==) => Opcode::EQ,
            id!(!=) => Opcode::NEQ,
            id!(<) => Opcode::LT,
            id!(>) => Opcode::GT,
            id!(<=) => Opcode::LEQ,
            id!(>=) => Opcode::GEQ,
            id!(===) => Opcode::SHALLOW_EQ,
            id!(!==) => Opcode::SHALLOW_NEQ,
                        
            // &&, ||, |? are handled specially for short-circuit evaluation
            id!(&&) => Opcode::LOGIC_AND_TEST,
            id!(||)  => Opcode::LOGIC_OR_TEST,
            id!(|?) => Opcode::NIL_OR_TEST,
            id!(:) => Opcode::ASSIGN_ME,
            id!(<:) => Opcode::ASSIGN_ME_BEFORE,
            id!(>:) => Opcode::ASSIGN_ME_AFTER,
            id!(^:) => Opcode::ASSIGN_ME_BEGIN,
            id!(=) => Opcode::ASSIGN,
            id!(+=) => Opcode::ASSIGN_ADD,
            id!(-=) => Opcode::ASSIGN_SUB,
            id!(*=) => Opcode::ASSIGN_MUL,
            id!(/=) => Opcode::ASSIGN_DIV,
            id!(%=) => Opcode::ASSIGN_MOD,
            id!(&=) => Opcode::ASSIGN_AND,
            id!(|=) => Opcode::ASSIGN_OR,
            id!(^=) => Opcode::ASSIGN_XOR,
            id!(<<=) => Opcode::ASSIGN_SHL,
            id!(>>=)  => Opcode::ASSIGN_SHR,
            id!(?=)  => Opcode::ASSIGN_IFNIL,
            id!(..) => Opcode::RANGE,
            id!(.)  => Opcode::FIELD,
            id!(.?)  => Opcode::FIELD_NIL,
            id!(me.) => Opcode::ME_FIELD,
            id!(?) => Opcode::RETURN_IF_ERR,
            _=> Opcode::NOP,
        }.into()
    }
    
    fn is_heq_prio(&self, other:State)->bool{
        match self{
            Self::EmitOp{what_op:op1,..}=>{
                match other{
                    Self::EmitOp{what_op:op2,..}=>{
                        if Self::is_assign_operator(*op1) && Self::is_assign_operator(op2){
                            return false
                        }
                        Self::operator_order(*op1) <= Self::operator_order(op2)
                    }
                    _=>false
                }
            },
            _=>false
        }
    }
}



/// Represents a nested destructuring pattern (one level deep for now)
#[derive(Clone, Debug)]
pub enum NestedPattern {
    /// Nested object pattern like {x, y} - stores binding ids
    Object(Vec<LiveId>),
    /// Nested array pattern like [x, y] - stores binding ids
    Array(Vec<LiveId>),
}

pub struct ScriptParser{
    pub index: u32,
    pub opcodes: Vec<ScriptValue>,
    pub source_map: Vec<Option<u32>>,
    
    state: Vec<State>,
    pub file: String,
    pub line_offset: usize,
    pub col_offset: usize,
    
    // Temporary storage for destructuring defaults (binding_id, value_code, value_map)
    destruct_defaults: Vec<(LiveId, Vec<ScriptValue>, Vec<Option<u32>>)>,
    
    // Storage for nested patterns during parsing
    // Each entry is (pattern_info). The index into this vec is encoded in the ids list.
    nested_patterns: Vec<NestedPattern>,
}

impl Default for ScriptParser{
    fn default()->Self{
        Self{
            index: 0,
            opcodes: Default::default(),
            source_map: Default::default(),
            state: vec![State::BeginStmt{last_was_sep:false}],
            file: String::new(),
            line_offset: 0,
            col_offset: 0,
            destruct_defaults: Default::default(),
            nested_patterns: Default::default(),
        }
    }
}

impl ScriptParser{
    pub fn report_error(&self, tokenizer:&ScriptTokenizer, msg: String){
        let (line, col) = tokenizer.token_index_to_row_col(self.index).unwrap_or((0,0));
        log_with_level(&self.file, line as u32 + self.line_offset as u32, col as u32 + self.col_offset as u32, line as u32 + self.line_offset as u32, col as u32 + self.col_offset as u32, msg, LogLevel::Error);
    }
    
    fn code_len(&self)->u32{
        self.opcodes.len() as _
    }
    
    fn code_last(&self)->Option<&ScriptValue>{
        self.opcodes.last()
    }
    
    fn pop_code(&mut self){
        self.opcodes.pop();
        self.source_map.pop();
    }
    
    fn push_code(&mut self, code: ScriptValue, index: u32){
        self.opcodes.push(code);
        self.source_map.push(Some(index));
    }
    
    fn push_code_none(&mut self, code: ScriptValue){
        self.opcodes.push(code);
        self.source_map.push(None);
    }
    
    fn set_pop_to_me(&mut self){
        if let Some(code) = self.opcodes.last_mut(){
            if let Some((opcode, _args)) = code.as_opcode(){
                if opcode == Opcode::RETURN{
                    self.push_code(Opcode::POP_TO_ME.into(), self.index)
                }
                else{
                    code.set_opcode_args_pop_to_me();
                }
            }
            else{
                self.push_code(Opcode::POP_TO_ME.into(), self.index)
            }
        }
    }
    
    fn has_pop_to_me(&self)->bool{
        if let Some(code) = self.opcodes.last(){
            if let Some((opcode,args)) = code.as_opcode(){
                if opcode == Opcode::POP_TO_ME{
                    return true
                }
                return args.0 & OpcodeArgs::POP_TO_ME_FLAG != 0
            }
        }
        false
    }
    
    fn clear_pop_to_me(&mut self){
        if let Some(code) = self.opcodes.last_mut(){
            if let Some((opcode,_)) = code.as_opcode(){
                if opcode == Opcode::POP_TO_ME{
                    self.pop_code();
                    return;
                }
            }
            code.clear_opcode_args_pop_to_me();
        }
    }
    
    fn set_opcode_args(&mut self, index:u32, args: OpcodeArgs){
        self.opcodes[index as usize].set_opcode_args(args);
    }
    
    fn parse_step(&mut self, tokenizer: &ScriptTokenizer, tok:ScriptToken, values: &[ScriptValue])->u32{
        
        let op = tok.operator();
        let sep = tok.separator();
        let (id,starts_with_ds) = tok.identifier();
        match self.state.pop().unwrap(){
            State::ForIdent{idents, index}=>{
                // we push k and v
                if id.not_empty(){
                    if id == id!(in){
                        // alright we move on to parsing the range expr
                        self.state.push(State::ForBody{idents, index});
                        self.state.push(State::BeginExpr{required:true});
                        return 1
                    }
                    else if idents < 3{
                        self.push_code(id.into(), self.index);
                        self.state.push(State::ForIdent{idents: idents + 1, index});
                        return 1
                    }
                    else{
                        error!(self, tokenizer, "Too many identifiers in for");
                        return 0
                    }
                }
                if op == id!(,){ // eat the commas
                    self.state.push(State::ForIdent{idents, index});
                    return 1
                }
                error!(self, tokenizer, "Unexpected state in parsing for");
            }
            State::ForBody{idents, index}=>{
                // alright lets emit a for instruction
                
                let code_start = self.code_len();
                if idents == 1{
                    self.push_code(Opcode::FOR_1.into(), index);
                }
                else if idents == 2{
                    self.push_code(Opcode::FOR_2.into(), index);
                }
                else if idents == 3{
                    self.push_code(Opcode::FOR_3.into(), index);
                }
                else{
                    error!(self, tokenizer, "Wrong number of identifiers for for loop {idents}");
                    return 0
                }
                if tok.is_open_curly(){
                    self.state.push(State::ForBlock{code_start});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                else{
                    self.state.push(State::ForExpr{code_start});
                    self.state.push(State::BeginExpr{required:true});
                }
            }
            State::Loop{index}=>{
                let code_start = self.code_len();
                self.push_code(Opcode::LOOP.into(), index);
                if tok.is_open_curly(){
                    self.state.push(State::ForBlock{code_start});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                else{
                    self.state.push(State::ForExpr{code_start});
                    self.state.push(State::BeginExpr{required:true});
                }
            }
            State::While{index}=>{
                let code_start = self.code_len();
                self.push_code(Opcode::LOOP.into(), index);
                self.state.push(State::WhileTest{code_start});
                self.state.push(State::BeginExpr{required:true});
            }
            State::WhileTest{code_start}=>{
                self.push_code(Opcode::BREAKIFNOT.into(), self.index);
                if tok.is_open_curly(){
                    self.state.push(State::ForBlock{code_start});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                else{
                    self.state.push(State::ForExpr{code_start});
                    self.state.push(State::BeginExpr{required:true});
                }
            }
            // Match expression desugaring:
            // match x { 1 => true, 2 => {false} }
            // becomes:
            // let $mN = x
            // if $mN == 1 true else if $mN == 2 {false}
            State::MatchSubject{temp_id, index}=>{
                // Subject expression has been parsed
                // Order is now: [temp_id, subject_value] - emit LET_DYN
                self.push_code(Opcode::LET_DYN.into(), index);
                self.state.push(State::MatchBlock{temp_id, index});
            }
            State::MatchBlock{temp_id, index}=>{
                if tok.is_open_curly(){
                    self.state.push(State::MatchArmPattern{temp_id, first:true, prev_else_start:0, index});
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected {{ after match expression");
                }
            }
            State::MatchArmPattern{temp_id, first:_, prev_else_start, index}=>{
                if tok.is_close_curly(){
                    // Empty match - patch any pending IF_ELSE and push nil
                    if prev_else_start > 0 {
                        self.set_opcode_args(prev_else_start, OpcodeArgs::from_u32(self.code_len() as u32 - prev_else_start));
                    }
                    self.push_code(NIL, index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                // Check for wildcard pattern `_`
                if id == id!(_) {
                    // Wildcard pattern - just expect => and body
                    self.state.push(State::MatchWildcardArrow{prev_else_start, index});
                    return 1
                }
                // Emit temp_id to push the match subject value
                self.push_code(ScriptValue::from_id(temp_id), index);
                // Parse the pattern expression
                self.state.push(State::MatchArmArrow{temp_id, prev_else_start, index});
                self.state.push(State::BeginExpr{required:true});
            }
            State::MatchArmArrow{temp_id, prev_else_start, index}=>{
                // Pattern expression done, emit EQ and IF_TEST
                self.push_code(Opcode::EQ.into(), index);
                let if_start = self.code_len();
                self.push_code(Opcode::IF_TEST.into(), index);
                // Expect =>
                if op == id!(=>){
                    // Parse arm body - check for block or expression
                    self.state.push(State::MatchArmBody{temp_id, if_start, prev_else_start, index});
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected => after match pattern, got {:?}", tok);
                }
            }
            State::MatchArmBody{temp_id, if_start, prev_else_start, index}=>{
                // Check if arm body is a block or expression (like IfTest does)
                if tok.is_open_curly(){
                    // Block body - use BeginStmt like if blocks
                    self.state.push(State::MatchArmBlock{temp_id, if_start, prev_else_start, last_was_sep:false, index});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                // Expression body
                self.state.push(State::MatchMaybeArm{temp_id, if_start, prev_else_start, index});
                self.state.push(State::BeginExpr{required:true});
            }
            State::MatchArmBlock{temp_id, if_start, prev_else_start, last_was_sep, index}=>{
                // Block body done, expect }
                if tok.is_close_curly(){
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::MatchMaybeArm{temp_id, if_start, prev_else_start, index});
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected }} in match arm block");
                }
            }
            State::MatchWildcardArrow{prev_else_start, index}=>{
                // Wildcard pattern done, expect =>
                if op == id!(=>){
                    // Parse wildcard arm body - check for block or expression
                    self.state.push(State::MatchWildcardBody{prev_else_start, index});
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected => after _ wildcard pattern, got {:?}", tok);
                }
            }
            State::MatchWildcardBody{prev_else_start, index}=>{
                // Check if wildcard body is a block or expression
                if tok.is_open_curly(){
                    // Block body
                    self.state.push(State::MatchWildcardBlock{prev_else_start, last_was_sep:false, index});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                // Expression body
                self.state.push(State::MatchWildcardEnd{prev_else_start, index});
                self.state.push(State::BeginExpr{required:true});
            }
            State::MatchWildcardBlock{prev_else_start, last_was_sep, index}=>{
                // Wildcard block body done, expect }
                if tok.is_close_curly(){
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::MatchWildcardEnd{prev_else_start, index});
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected }} in wildcard arm block");
                }
            }
            State::MatchWildcardEnd{prev_else_start, index:_}=>{
                // Wildcard body done - patch any pending IF_ELSE to jump here
                if prev_else_start > 0 {
                    self.set_opcode_args(prev_else_start, OpcodeArgs::from_u32(self.code_len() as u32 - prev_else_start));
                }
                // Expect }
                if tok.is_close_curly(){
                    self.state.push(State::EndExpr);
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected }} after wildcard arm (no more arms allowed after _)");
                }
            }
            State::MatchMaybeArm{temp_id, if_start, prev_else_start, index}=>{
                // Arm body parsed. Now emit IF_ELSE and patch IF_TEST.
                
                // First, patch any previous arm's IF_ELSE to jump here (to current position)
                if prev_else_start > 0 {
                    self.set_opcode_args(prev_else_start, OpcodeArgs::from_u32(self.code_len() as u32 - prev_else_start));
                }
                
                if tok.is_close_curly(){
                    // End of match - no more arms, patch IF_TEST with need_nil flag
                    self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 - if_start).set_need_nil());
                    self.state.push(State::EndExpr);
                    return 1
                }
                
                // More arms coming - emit IF_ELSE to skip remaining arms after this arm's body
                let else_start = self.code_len();
                self.push_code(Opcode::IF_ELSE.into(), index);
                
                // Patch IF_TEST to jump here (after arm body and IF_ELSE) to next arm
                self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 - if_start));
                
                // Check for wildcard or regular arm
                if id == id!(_) {
                    // Next arm is wildcard - it's the final else body
                    self.state.push(State::MatchWildcardArrow{prev_else_start: else_start, index});
                    return 1
                }
                
                // Continue with next regular arm, passing this arm's else_start
                self.state.push(State::MatchArmPattern{temp_id, first:false, prev_else_start: else_start, index});
            }
            State::ForExpr{code_start}=>{
                self.set_pop_to_me();
                //self.push_code_none(Opcode::POP_TO_ME.into());
                self.push_code_none(Opcode::FOR_END.into());
                let jump_to = (self.code_len() - code_start) as _;
                self.set_opcode_args(code_start, OpcodeArgs::from_u32(jump_to));
                return 0
            }
            State::ForBlock{code_start}=>{
                if tok.is_close_curly() {
                    self.push_code_none(Opcode::FOR_END.into());
                    let jump_to = (self.code_len() - code_start) as _;
                    self.set_opcode_args(code_start, OpcodeArgs::from_u32(jump_to));
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found in for");
                    return 0
                }
            }
            State::Use{index}=>{
                if let Some(code) = self.opcodes.last(){
                    if let Some((Opcode::FIELD,_)) = code.as_opcode(){
                        self.pop_code();
                        self.push_code(Opcode::USE.into(), index)
                    }
                    else{
                        error!(self, tokenizer, "Error use expected field operation")
                    }
                }
            }
            State::Let{index}=>{
                if id == id!(mut) {
                    // "let mut" is treated as "var"
                    self.state.push(State::Var{index});
                    return 1
                }
                else if tok.is_open_square() {
                    // Array destructuring: let [x, y] = ...
                    let ids_start = self.code_len();
                    self.state.push(State::LetArrayDestruct{index, count: 0, ids_start});
                    return 1
                }
                else if tok.is_open_curly() {
                    // Object destructuring: let {x, y} = ...
                    let ids_start = self.code_len();
                    self.state.push(State::LetObjectDestruct{index, count: 0, ids_start});
                    return 1
                }
                else if id.not_empty(){ // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetDynOrTyped{index});
                    return 1
                }
                else{ // unknown
                    error!(self, tokenizer, "Let expected identifier");
                }
            }
            State::LetDynOrTyped{index}=>{
                if op == id!(=){ // assignment following
                    self.state.push(State::EmitLetDyn{index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else if op == id!(:){ // type following
                    self.state.push(State::LetType{index});
                    return 1
                }
                else{
                    self.push_code(ScriptValue::from_opcode_args(Opcode::LET_DYN, OpcodeArgs::NIL), index);
                }
            }
            State::LetType{index}=>{
                if id.not_empty(){ // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetTypedAssign{index});
                    return 1
                }
                else{ // unknown
                    error!(self, tokenizer, "Let type expected");
                }
            }
            State::LetTypedAssign{index}=>{
                if op == id!(=){ // assignment following
                    self.state.push(State::EmitLetTyped{index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else{
                    self.push_code(ScriptValue::from_opcode_args(Opcode::LET_TYPED, OpcodeArgs::NIL), index);
                }
            }
            State::EmitLetDyn{index}=>{
                self.push_code(Opcode::LET_DYN.into(), index);
            }
            State::EmitLetTyped{index}=>{
                self.push_code(Opcode::LET_TYPED.into(), index);
            }
            
            // ====== Array Destructuring: let [x, y = default, ...] = expr ======
            // Layout during parsing: [ids..., defaults_with_ifnil..., rhs]
            // Final generated code: [rhs, id_0, EXTRACT(0), id_1, EXTRACT(1), ..., DROP, defaults_with_ifnil]
            State::LetArrayDestruct{index, count, ids_start}=>{
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestruct{index, count, ids_start});
                    return 1
                }
                if id.not_empty() {
                    // Just push the identifier
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetArrayDestructEl{index, count: count + 1, binding_id: id, ids_start});
                    return 1
                }
                else if tok.is_open_curly() {
                    // Nested object pattern: let [{x, y}] = ...
                    // Push a marker that will be replaced with the nested pattern index
                    self.push_code(Opcode::NOP.into(), self.index); // placeholder
                    self.state.push(State::LetArrayDestructNestedObject{index, outer_count: count + 1, outer_ids_start: ids_start, nested_count: 0});
                    return 1
                }
                else if tok.is_open_square() {
                    // Nested array pattern: let [[x, y]] = ...
                    self.push_code(Opcode::NOP.into(), self.index); // placeholder
                    self.state.push(State::LetArrayDestructNestedArray{index, outer_count: count + 1, outer_ids_start: ids_start, nested_count: 0});
                    return 1
                }
                else if tok.is_close_square() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetArrayDestructRhs{index, count, ids_start, defaults_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected identifier in array destructuring pattern");
                }
            }
            State::LetArrayDestructEl{index, count, binding_id, ids_start}=>{
                if op == id!(=) {
                    // Default value follows - parse it
                    let default_start = self.code_len();
                    self.state.push(State::LetArrayDestructDefault{index, count, binding_id, ids_start, default_start});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestruct{index, count, ids_start});
                    return 1
                }
                else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetArrayDestructEl{index, count: count + 1, binding_id: id, ids_start});
                    return 1
                }
                else if tok.is_close_square() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetArrayDestructRhs{index, count, ids_start, defaults_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected '=' or identifier in array destructuring pattern");
                }
            }
            State::LetArrayDestructDefault{index, count, binding_id, ids_start, default_start}=>{
                // Default was parsed (value is in code stream at default_start..current)
                // Store it for later emission, don't emit inline
                let default_start = default_start as usize;
                let value_code: Vec<_> = self.opcodes[default_start..].to_vec();
                let value_map: Vec<_> = self.source_map[default_start..].to_vec();
                self.opcodes.truncate(default_start);
                self.source_map.truncate(default_start);
                
                // Store the default for later
                self.destruct_defaults.push((binding_id, value_code, value_map));
                
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestruct{index, count, ids_start});
                    return 1
                }
                else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetArrayDestructEl{index, count: count + 1, binding_id: id, ids_start});
                    return 1
                }
                else if tok.is_close_square() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetArrayDestructRhs{index, count, ids_start, defaults_start});
                    return 1
                }
                else {
                    self.state.push(State::LetArrayDestruct{index, count, ids_start});
                    return 0
                }
            }
            // Nested object pattern inside array: let [{x, y}] = ...
            State::LetArrayDestructNestedObject{index, outer_count, outer_ids_start, nested_count}=>{
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedObject{index, outer_count, outer_ids_start, nested_count});
                    return 1
                }
                if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedObjectEl{index, outer_count, outer_ids_start, nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_curly() {
                    // Nested pattern complete - bindings were collected in NestedObjectEl
                    if let Some(NestedPattern::Object(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            // Pattern is complete, update the placeholder with the pattern index
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                            // Encode the nested pattern as a special marker: NOP with args = pattern_index + 1 (so 0 means "not nested")
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                        }
                    }
                    self.state.push(State::LetArrayDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected identifier in nested object pattern");
                }
            }
            State::LetArrayDestructNestedObjectEl{index, outer_count, outer_ids_start, nested_count, binding_id}=>{
                // Store the binding in the nested pattern
                if nested_count == 1 {
                    // First binding - create new pattern
                    self.nested_patterns.push(NestedPattern::Object(vec![binding_id]));
                } else {
                    // Add to existing pattern
                    if let Some(NestedPattern::Object(ref mut bindings)) = self.nested_patterns.last_mut() {
                        bindings.push(binding_id);
                    }
                }
                
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedObject{index, outer_count, outer_ids_start, nested_count});
                    return 1
                }
                else if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedObjectEl{index, outer_count, outer_ids_start, nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_curly() {
                    // Pattern complete - update placeholder
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                    self.state.push(State::LetArrayDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    self.state.push(State::LetArrayDestructNestedObject{index, outer_count, outer_ids_start, nested_count});
                    return 0
                }
            }
            
            // Nested array pattern inside array: let [[x, y]] = ...
            State::LetArrayDestructNestedArray{index, outer_count, outer_ids_start, nested_count}=>{
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedArray{index, outer_count, outer_ids_start, nested_count});
                    return 1
                }
                if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedArrayEl{index, outer_count, outer_ids_start, nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_square() {
                    // Nested pattern complete
                    if let Some(NestedPattern::Array(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                        }
                    }
                    self.state.push(State::LetArrayDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected identifier in nested array pattern");
                }
            }
            State::LetArrayDestructNestedArrayEl{index, outer_count, outer_ids_start, nested_count, binding_id}=>{
                // Store the binding
                if nested_count == 1 {
                    self.nested_patterns.push(NestedPattern::Array(vec![binding_id]));
                } else {
                    if let Some(NestedPattern::Array(ref mut bindings)) = self.nested_patterns.last_mut() {
                        bindings.push(binding_id);
                    }
                }
                
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetArrayDestructNestedArray{index, outer_count, outer_ids_start, nested_count});
                    return 1
                }
                else if id.not_empty() {
                    self.state.push(State::LetArrayDestructNestedArrayEl{index, outer_count, outer_ids_start, nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_square() {
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                    self.state.push(State::LetArrayDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    self.state.push(State::LetArrayDestructNestedArray{index, outer_count, outer_ids_start, nested_count});
                    return 0
                }
            }
            
            State::LetArrayDestructRhs{index, count, ids_start, defaults_start}=>{
                if op == id!(=) {
                    self.state.push(State::EmitLetArrayDestruct{index, count, ids_start, defaults_start});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected '=' after destructuring pattern");
                }
            }
            State::EmitLetArrayDestruct{index, count, ids_start, defaults_start}=>{
                // Layout: [ids..., rhs_code], defaults stored in destruct_defaults
                // Generate: [rhs, id_0, EXTRACT(0), ..., DROP, defaults_code...]
                // For nested patterns: [rhs, DUP, idx, ARRAY_INDEX_NIL, nested_bindings..., DROP, ...]
                
                let ids_start = ids_start as usize;
                let defaults_start = defaults_start as usize;
                let count = count as usize;
                
                let ids: Vec<_> = self.opcodes[ids_start..ids_start + count].to_vec();
                let ids_map: Vec<_> = self.source_map[ids_start..ids_start + count].to_vec();
                let rhs: Vec<_> = self.opcodes[defaults_start..].to_vec();
                let rhs_map: Vec<_> = self.source_map[defaults_start..].to_vec();
                
                self.opcodes.truncate(ids_start);
                self.source_map.truncate(ids_start);
                
                // RHS first
                self.opcodes.extend(rhs);
                self.source_map.extend(rhs_map);
                
                // Process each element - check for nested patterns
                for (i, (id_code, id_map)) in ids.into_iter().zip(ids_map).enumerate() {
                    // Check if this is a nested pattern marker (NOP with non-zero args)
                    if let Some((opcode, args)) = id_code.as_opcode() {
                        if opcode == Opcode::NOP && args.to_u32() > 0 {
                            // This is a nested pattern
                            let pattern_idx = (args.to_u32() - 1) as usize;
                            if let Some(pattern) = self.nested_patterns.get(pattern_idx).cloned() {
                                // Emit: DUP, index, ARRAY_INDEX_NIL, nested extraction, DROP
                                self.push_code(Opcode::DUP.into(), index);
                                self.push_code(i.into(), index);
                                self.push_code(Opcode::ARRAY_INDEX_NIL.into(), index);
                                
                                match pattern {
                                    NestedPattern::Object(bindings) => {
                                        // For nested object: id, LET_DESTRUCT_OBJECT_EL for each binding
                                        for binding_id in bindings {
                                            self.push_code(binding_id.into(), index);
                                            self.push_code(Opcode::LET_DESTRUCT_OBJECT_EL.into(), index);
                                        }
                                    }
                                    NestedPattern::Array(bindings) => {
                                        // For nested array: id, LET_DESTRUCT_ARRAY_EL(j) for each binding
                                        for (j, binding_id) in bindings.into_iter().enumerate() {
                                            self.push_code(binding_id.into(), index);
                                            self.push_code(ScriptValue::from_opcode_args(Opcode::LET_DESTRUCT_ARRAY_EL, OpcodeArgs::from_u32(j as u32)), index);
                                        }
                                    }
                                }
                                
                                // DROP the nested source
                                self.push_code(Opcode::DROP.into(), index);
                                continue;
                            }
                        }
                    }
                    
                    // Simple identifier - emit normally
                    self.opcodes.push(id_code);
                    self.source_map.push(id_map);
                    self.push_code(ScriptValue::from_opcode_args(Opcode::LET_DESTRUCT_ARRAY_EL, OpcodeArgs::from_u32(i as u32)), index);
                }
                
                // DROP outer source
                self.push_code(Opcode::DROP.into(), index);
                
                // Emit stored defaults with lazy evaluation: [id, ASSIGN_IFNIL(jump), value, ASSIGN]
                // jump_dist = value_code.len() + 2 to skip past value_code AND the ASSIGN opcode
                let defaults = std::mem::take(&mut self.destruct_defaults);
                for (binding_id, value_code, value_map) in defaults {
                    let jump_dist = (value_code.len() + 2) as u32;
                    self.push_code(binding_id.into(), index);
                    self.push_code(ScriptValue::from_opcode_args(Opcode::ASSIGN_IFNIL, OpcodeArgs::from_u32(jump_dist)), index);
                    self.opcodes.extend(value_code);
                    self.source_map.extend(value_map);
                    self.push_code(Opcode::ASSIGN.into(), index);
                }
                
                // Clear nested patterns for next destructuring
                self.nested_patterns.clear();
                
                // This is a let statement, go directly to BeginStmt to avoid pop_to_me being set on DROP
                self.state.push(State::BeginStmt{last_was_sep:false});
                return 0
            }
            
            // ====== Object Destructuring: let {x, y = default, ...} = expr ======
            State::LetObjectDestruct{index, count, ids_start}=>{
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestruct{index, count, ids_start});
                    return 1
                }
                if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetObjectDestructEl{index, count: count + 1, binding_id: id, ids_start});
                    return 1
                }
                else if tok.is_close_curly() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetObjectDestructRhs{index, count, ids_start, defaults_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected identifier in object destructuring pattern");
                }
            }
            State::LetObjectDestructEl{index, count, binding_id, ids_start}=>{
                if op == id!(=) {
                    // Default value follows - parse it
                    let default_start = self.code_len();
                    self.state.push(State::LetObjectDestructDefault{index, count, binding_id, ids_start, default_start});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestruct{index, count, ids_start});
                    return 1
                }
                else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetObjectDestructEl{index, count: count + 1, binding_id: id, ids_start});
                    return 1
                }
                else if tok.is_close_curly() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetObjectDestructRhs{index, count, ids_start, defaults_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected '=' or identifier in object destructuring pattern");
                }
            }
            State::LetObjectDestructDefault{index, count, binding_id, ids_start, default_start}=>{
                // Default was parsed - store it for later emission, don't emit inline
                let default_start = default_start as usize;
                let value_code: Vec<_> = self.opcodes[default_start..].to_vec();
                let value_map: Vec<_> = self.source_map[default_start..].to_vec();
                self.opcodes.truncate(default_start);
                self.source_map.truncate(default_start);
                
                // Store the default for later
                self.destruct_defaults.push((binding_id, value_code, value_map));
                
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestruct{index, count, ids_start});
                    return 1
                }
                else if id.not_empty() {
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetObjectDestructEl{index, count: count + 1, binding_id: id, ids_start});
                    return 1
                }
                else if tok.is_close_curly() {
                    let defaults_start = self.code_len();
                    self.state.push(State::LetObjectDestructRhs{index, count, ids_start, defaults_start});
                    return 1
                }
                else {
                    self.state.push(State::LetObjectDestruct{index, count, ids_start});
                    return 0
                }
            }
            // Nested array pattern inside object: let {a: [x, y]} = ...
            State::LetObjectDestructNestedArray{index, outer_count, outer_ids_start, key, nested_count}=>{
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedArray{index, outer_count, outer_ids_start, key, nested_count});
                    return 1
                }
                if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedArrayEl{index, outer_count, outer_ids_start, key, nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_square() {
                    // Pattern complete
                    if let Some(NestedPattern::Array(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                        }
                    }
                    self.state.push(State::LetObjectDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected identifier in nested array pattern");
                }
            }
            State::LetObjectDestructNestedArrayEl{index, outer_count, outer_ids_start, key: _, nested_count, binding_id}=>{
                if nested_count == 1 {
                    self.nested_patterns.push(NestedPattern::Array(vec![binding_id]));
                } else {
                    if let Some(NestedPattern::Array(ref mut bindings)) = self.nested_patterns.last_mut() {
                        bindings.push(binding_id);
                    }
                }
                
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedArray{index, outer_count, outer_ids_start, key: LiveId::empty(), nested_count});
                    return 1
                }
                else if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedArrayEl{index, outer_count, outer_ids_start, key: LiveId::empty(), nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_square() {
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                    self.state.push(State::LetObjectDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    self.state.push(State::LetObjectDestructNestedArray{index, outer_count, outer_ids_start, key: LiveId::empty(), nested_count});
                    return 0
                }
            }
            
            // Nested object pattern inside object: let {a: {x, y}} = ...
            State::LetObjectDestructNestedObject{index, outer_count, outer_ids_start, key, nested_count}=>{
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedObject{index, outer_count, outer_ids_start, key, nested_count});
                    return 1
                }
                if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedObjectEl{index, outer_count, outer_ids_start, key, nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_curly() {
                    // Pattern complete
                    if let Some(NestedPattern::Object(ref bindings)) = self.nested_patterns.last() {
                        if bindings.len() == nested_count as usize {
                            let pattern_idx = self.nested_patterns.len() - 1;
                            let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                            self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                        }
                    }
                    self.state.push(State::LetObjectDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected identifier in nested object pattern");
                }
            }
            State::LetObjectDestructNestedObjectEl{index, outer_count, outer_ids_start, key: _, nested_count, binding_id}=>{
                if nested_count == 1 {
                    self.nested_patterns.push(NestedPattern::Object(vec![binding_id]));
                } else {
                    if let Some(NestedPattern::Object(ref mut bindings)) = self.nested_patterns.last_mut() {
                        bindings.push(binding_id);
                    }
                }
                
                if sep == id!(,) || sep == id!(;) {
                    self.state.push(State::LetObjectDestructNestedObject{index, outer_count, outer_ids_start, key: LiveId::empty(), nested_count});
                    return 1
                }
                else if id.not_empty() {
                    self.state.push(State::LetObjectDestructNestedObjectEl{index, outer_count, outer_ids_start, key: LiveId::empty(), nested_count: nested_count + 1, binding_id: id});
                    return 1
                }
                else if tok.is_close_curly() {
                    let pattern_idx = self.nested_patterns.len() - 1;
                    let placeholder_pos = outer_ids_start as usize + outer_count as usize - 1;
                    self.opcodes[placeholder_pos] = ScriptValue::from_opcode_args(Opcode::NOP, OpcodeArgs::from_u32((pattern_idx + 1) as u32));
                    self.state.push(State::LetObjectDestruct{index, count: outer_count, ids_start: outer_ids_start});
                    return 1
                }
                else {
                    self.state.push(State::LetObjectDestructNestedObject{index, outer_count, outer_ids_start, key: LiveId::empty(), nested_count});
                    return 0
                }
            }
            
            State::LetObjectDestructRhs{index, count, ids_start, defaults_start}=>{
                if op == id!(=) {
                    self.state.push(State::EmitLetObjectDestruct{index, count, ids_start, defaults_start});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected '=' after destructuring pattern");
                }
            }
            State::EmitLetObjectDestruct{index, count, ids_start, defaults_start}=>{
                // Layout: [ids..., rhs_code], defaults stored in destruct_defaults
                // Generate: [rhs, id_0, EXTRACT, ..., DROP, defaults_code...]
                
                let ids_start = ids_start as usize;
                let defaults_start = defaults_start as usize;
                let count = count as usize;
                
                let ids: Vec<_> = self.opcodes[ids_start..ids_start + count].to_vec();
                let ids_map: Vec<_> = self.source_map[ids_start..ids_start + count].to_vec();
                let rhs: Vec<_> = self.opcodes[defaults_start..].to_vec();
                let rhs_map: Vec<_> = self.source_map[defaults_start..].to_vec();
                
                self.opcodes.truncate(ids_start);
                self.source_map.truncate(ids_start);
                
                // RHS first
                self.opcodes.extend(rhs);
                self.source_map.extend(rhs_map);
                
                // id, EXTRACT for each
                for (id_code, id_map) in ids.into_iter().zip(ids_map) {
                    self.opcodes.push(id_code);
                    self.source_map.push(id_map);
                    self.push_code(Opcode::LET_DESTRUCT_OBJECT_EL.into(), index);
                }
                
                // DROP source
                self.push_code(Opcode::DROP.into(), index);
                
                // Emit stored defaults with lazy evaluation: [id, ASSIGN_IFNIL(jump), value, ASSIGN]
                // jump_dist = value_code.len() + 2 to skip past value_code AND the ASSIGN opcode
                let defaults = std::mem::take(&mut self.destruct_defaults);
                for (binding_id, value_code, value_map) in defaults {
                    let jump_dist = (value_code.len() + 2) as u32;
                    self.push_code(binding_id.into(), index);
                    self.push_code(ScriptValue::from_opcode_args(Opcode::ASSIGN_IFNIL, OpcodeArgs::from_u32(jump_dist)), index);
                    self.opcodes.extend(value_code);
                    self.source_map.extend(value_map);
                    self.push_code(Opcode::ASSIGN.into(), index);
                }
                
                // This is a let statement, go directly to BeginStmt to avoid pop_to_me being set on DROP
                self.state.push(State::BeginStmt{last_was_sep:false});
                return 0
            }
            
            State::Var{index}=>{
                if id.not_empty(){ // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::VarDynOrTyped{index});
                    return 1
                }
                else{ // unknown
                    error!(self, tokenizer, "Var expected identifier");
                }
            }
            State::VarDynOrTyped{index}=>{
                if op == id!(=){ // assignment following
                    self.state.push(State::EmitVarDyn{index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else if op == id!(:){ // type following
                    self.state.push(State::VarType{index});
                    return 1
                }
                else{
                    self.push_code(ScriptValue::from_opcode_args(Opcode::VAR_DYN, OpcodeArgs::NIL), index);
                }
            }
            State::VarType{index}=>{
                if id.not_empty(){ // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::VarTypedAssign{index});
                    return 1
                }
                else{ // unknown
                    error!(self, tokenizer, "Var type expected");
                }
            }
            State::VarTypedAssign{index}=>{
                if op == id!(=){ // assignment following
                    self.state.push(State::EmitVarTyped{index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else{
                    self.push_code(ScriptValue::from_opcode_args(Opcode::VAR_TYPED, OpcodeArgs::NIL), index);
                }
            }
            State::EmitVarDyn{index}=>{
                self.push_code(Opcode::VAR_DYN.into(), index);
            }
            State::EmitVarTyped{index}=>{
                self.push_code(Opcode::VAR_TYPED.into(), index);
            }
            State::EndRound=>{
                // we expect a ) here
                //self.code.push(Opcode::END_FRAG.into());
                if tok.is_close_round(){
                    self.state.push(State::EndExpr);
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected )")
                }
            }
            State::EmitFnArgTyped{index}=>{
                self.push_code(Opcode::FN_ARG_TYPED.into(), index);
            }
            State::EmitFnArgDyn{index}=>{
                self.push_code(Opcode::FN_ARG_DYN.into(), index);
            }
            State::FnMaybeLet{index}=>{
                if id.not_empty(){
                    // ok we did fn id
                    self.push_code(id.into(), self.index);
                    self.push_code(Opcode::FN_LET_ARGS.into(), self.index);
                    self.state.push(State::FnLetMaybeArgs);
                    return 1
                }
                else if tok.is_open_curly(){ // zero args function
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    let fn_slot = self.code_len();
                    self.push_code(Opcode::FN_BODY_DYN.into(), self.index);
                    self.state.push(State::EndFnBlock{fn_slot, last_was_sep:false, index:self.index});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                // immediate args
                else if tok.is_open_round(){
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    self.state.push(State::FnArgList{lambda: false});
                    return 1
                }
                else{
                    self.state.push(State::EmitFnArgDyn{index});
                    error!(self, tokenizer, "Argument type expected in function");
                }
            }
            State::FnLetMaybeArgs=>{
                // no args
                if tok.is_open_curly(){
                    let fn_slot = self.code_len();
                    self.push_code(Opcode::FN_BODY_DYN.into(), self.index);
                    self.state.push(State::EndFnBlock{fn_slot, last_was_sep:false, index:self.index});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                else if tok.is_open_round(){
                    self.state.push(State::FnArgList{lambda: false});
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected either {{ or ( in function definition");
                }
            }
            State::FnArgType{lambda, index}=>{
                if id.not_empty(){
                    self.push_code(id.into(), self.index);
                    self.state.push(State::FnArgTypeAssign{lambda,index});
                    return 1
                }
                else{
                    self.state.push(State::EmitFnArgDyn{index});
                    error!(self, tokenizer, "Argument type expected in function")
                }
            }
            State::FnArgTypeAssign{lambda, index}=>{
                if !lambda && op == id!(=){ // assignment following
                    self.state.push(State::EmitFnArgTyped{index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else{
                    self.push_code(ScriptValue::from_opcode_args(Opcode::FN_ARG_TYPED, OpcodeArgs::NIL), index);
                }
            }
            State::FnArgMaybeType{lambda, index}=>{
                if !lambda && op == id!(=){ // assignment following
                    self.state.push(State::EmitFnArgDyn{index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if op == id!(:){
                    self.state.push(State::FnArgType{lambda, index});
                    return 1
                }
                self.push_code(ScriptValue::from_opcode_args(Opcode::FN_ARG_DYN, OpcodeArgs::NIL), index);
            }
            State::FnArgList{lambda}=>{
                if id.not_empty(){ // ident
                    self.push_code(id.into(), self.index);
                    self.state.push(State::FnArgList{lambda});
                    self.state.push(State::FnArgMaybeType{lambda, index:self.index});
                    return 1
                }
                if lambda && op == id!(|){
                    self.state.push(State::FnBody{lambda});
                    return 1
                }
                else if !lambda && tok.is_close_round(){
                    self.state.push(State::FnBody{lambda});
                    return 1
                }
                if sep == id!(,){
                    self.state.push(State::FnArgList{lambda});
                    return 1
                }
                // unexpected token, but just stay in the arg list mode
                error!(self, tokenizer, "Unexpected token in function argument list {:?}", tok);
                self.state.push(State::FnArgList{lambda});
                return 1
            }
            State::FnBody{lambda}=>{
                // Check for return type annotation with ->
                if op == id!(->){
                    self.state.push(State::FnReturnType{lambda});
                    return 1
                }
                let fn_slot = self.code_len() as _ ;
                self.push_code(Opcode::FN_BODY_DYN.into(), self.index);
                if tok.is_open_curly(){ // function body
                    self.state.push(State::EndFnBlock{fn_slot, last_was_sep:false, index:self.index});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                else if lambda{ // function body can be expression expression
                    self.state.push(State::EndFnExpr{fn_slot, index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                }
                else{
                    error!(self, tokenizer, "Unexpected token in function definition, expected {{ {:?}", tok);
                }
            }
            State::FnReturnType{lambda}=>{
                if id.not_empty(){ // we have a return type identifier
                    self.push_code(id.into(), self.index);
                    self.state.push(State::FnBodyTyped{lambda});
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected return type after ->");
                }
            }
            State::FnBodyTyped{lambda}=>{
                let fn_slot = self.code_len() as _ ;
                self.push_code(Opcode::FN_BODY_TYPED.into(), self.index);
                if tok.is_open_curly(){ // function body
                    self.state.push(State::EndFnBlock{fn_slot, last_was_sep:false, index:self.index});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                else if lambda{ // function body can be expression expression
                    self.state.push(State::EndFnExpr{fn_slot, index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                }
                else{
                    error!(self, tokenizer, "Unexpected token in function definition, expected {{ {:?}", tok);
                }
            }
            State::EscapedId=>{
                if id.not_empty(){ // ident
                    self.push_code(id.escape(), self.index);
                    return 1
                }
                else{
                    error!(self, tokenizer, "Expected identifier after @");
                }
            }
            State::EndFnExpr{fn_slot, index}=>{
                self.push_code(Opcode::RETURN.into(), index);
                self.set_opcode_args(fn_slot as _, OpcodeArgs::from_u32(self.code_len() as u32 -fn_slot));
            }
            State::EndFnBlock{fn_slot, last_was_sep, index}=>{
                
               /* if !last_was_sep && Some(&Opcode::POP_TO_ME.into()) == self.code_last(){
                    self.pop_code();
                    self.push_code(Opcode::RETURN.into(), index);
                }*/
                if !last_was_sep && self.has_pop_to_me(){
                    self.clear_pop_to_me();
                    self.push_code(Opcode::RETURN.into(), index);
                }
                                 
                else{
                    self.push_code(ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL), index);
                }
                self.set_opcode_args(fn_slot as _, OpcodeArgs::from_u32(self.code_len() as u32 -fn_slot));
                
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            // alright we parsed a + b * c
            State::EmitFieldAssign{what_op, index}=>{
                self.push_code(State::operator_to_field_assign(what_op), index);
            }
            State::EmitIndexAssign{what_op, index}=>{
                self.push_code(State::operator_to_index_assign(what_op), index);
            }
            State::EmitOp{what_op, index}=>{
                if State::operator_supports_inline_number(what_op){
                    if let Some(code) = self.code_last(){
                        if let Some(vf64) = code.as_f64(){
                            let num = vf64 as u64;
                            if vf64.fract() == 0.0 && num <= OpcodeArgs::MAX_U32 as u64{
                                self.pop_code();
                                let mut value = State::operator_to_opcode(what_op);
                                value.set_opcode_args(OpcodeArgs::from_u32(num as u32));
                                self.push_code(value, index);
                                return 0
                            }
                        }
                    }
                }
                self.push_code(State::operator_to_opcode(what_op), index);
                return 0
            }
            State::EmitUnary{what_op, index}=>{
                self.push_code(State::operator_to_unary(what_op), index);
                return 0
            }
            State::EmitSplat{index}=>{
                self.push_code(Opcode::ME_SPLAT.into(), index);
                return 0
            }
            State::ShortCircuitEnd{test_slot}=>{
                // Patch the TEST opcode's jump to skip to current position (after second operand)
                self.set_opcode_args(test_slot, OpcodeArgs::from_u32(self.code_len() - test_slot));
                return 0
            }
            State::ShortCircuitAssignEnd{test_slot, index}=>{
                // Emit ASSIGN after RHS, then patch the jump
                self.push_code(Opcode::ASSIGN.into(), index);
                self.set_opcode_args(test_slot, OpcodeArgs::from_u32(self.code_len() - test_slot));
                return 0
            }
            State::EmitReturn{index}=>{
                self.push_code(Opcode::RETURN.into(), index);
                return 0
            }
            State::EmitBreak{index}=>{
                self.push_code(Opcode::BREAK.into(), index);
                return 0
            }
            State::EmitContinue{index}=>{
                self.push_code(Opcode::CONTINUE.into(), index);
                return 0
            }
            State::EmitDelete{index}=>{
                self.push_code(Opcode::DELETE.into(), index);
                return 0       
            }
            State::EndBareSquare=>{
                self.push_code(Opcode::END_ARRAY.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_square() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected ] not found");
                    return 0
                }
            }
            State::EndBare=>{
                self.push_code(Opcode::END_BARE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            // emit the create prototype instruction
            State::EndProto=>{
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            // emit prototype instruction + proto inherit write (for +: operator)
            State::EndProtoInherit=>{
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::PROTO_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            // emit prototype instruction + scope inherit write (for value += {} operator)
            State::EndScopeInherit=>{
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::SCOPE_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            // emit prototype instruction + field inherit write (for obj.field += {} operator)
            State::EndFieldInherit=>{
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::FIELD_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            // emit prototype instruction + index inherit write (for obj[index] += {} operator)
            State::EndIndexInherit=>{
                self.push_code(Opcode::END_PROTO.into(), self.index);
                self.push_code(Opcode::INDEX_INHERIT_WRITE.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            State::EmitCallFromDo{is_method, index}=>{
                self.set_pop_to_me();
                if is_method{
                    self.push_code(Opcode::METHOD_CALL_EXEC.into(), index);
                }
                else{
                    self.push_code(Opcode::CALL_EXEC.into(), index);
                }
                self.state.push(State::EndExpr);
            }
            State::CallMaybeDo{is_method, index}=>{
                if id == id!(do){
                    self.state.push(State::EmitCallFromDo{is_method, index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                else{
                    if is_method{
                        self.push_code(Opcode::METHOD_CALL_EXEC.into(), index);
                    }
                    else{
                        self.push_code(Opcode::CALL_EXEC.into(), index);
                    }
                    self.state.push(State::EndExpr);
                    return 0
                }
            }
            State::EndCall{is_method, index}=>{
                // expect )
                self.state.push(State::CallMaybeDo{is_method, index});
                if tok.is_close_round() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected ) not found");
                    return 0
                }
            }
            State::ArrayIndex=>{
                self.push_code(Opcode::ARRAY_INDEX.into(), self.index);
                self.state.push(State::EndExpr);
                if tok.is_close_square() {
                    return 1
                }
                else {
                    error!(self, tokenizer, "AT TOK {:?}", tok);
                    error!(self, tokenizer, "Expected ] not found");
                    return 0
                }
            }
                        
            State::TryTest{index}=>{
                let try_start = self.code_len() as _ ;
                self.push_code(Opcode::TRY_TEST.into(), index);
                if tok.is_open_curly(){
                    self.state.push(State::TryTestBlock{try_start, last_was_sep:false});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                self.state.push(State::TryTestExpr{try_start});
                self.state.push(State::BeginExpr{required:true});
                return 0
            }
            State::OkTest{index}=>{
                let ok_start = self.code_len() as _ ;
                self.push_code(Opcode::OK_TEST.into(), index);
                if tok.is_open_curly(){
                    self.state.push(State::OkTestBlock{ok_start, last_was_sep:false});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                self.state.push(State::OkTestExpr{ok_start});
                self.state.push(State::BeginExpr{required:true});
                return 0
            }
            State::OkTestExpr{ok_start}=>{
                self.set_opcode_args(ok_start, OpcodeArgs::from_u32(self.code_len() as u32 -ok_start) );
                self.push_code(Opcode::OK_END.into(), self.index);
                return 0
            }
            State::OkTestBlock{ok_start, last_was_sep}=>{
                self.set_opcode_args(ok_start, OpcodeArgs::from_u32(self.code_len() as u32 -ok_start) );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    self.push_code(Opcode::OK_END.into(), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                else {
                    self.push_code(Opcode::OK_END.into(), self.index);
                    return 0
                }
            }
            State::TryTestExpr{try_start}=>{
                self.set_opcode_args(try_start, OpcodeArgs::from_u32(self.code_len() as u32 -try_start) );
                self.state.push(State::TryErrBlockOrExpr);
                return 0
            }
            State::TryTestBlock{try_start, last_was_sep}=>{
                
                self.set_opcode_args(try_start, OpcodeArgs::from_u32(self.code_len() as u32 -try_start) );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::TryErrBlockOrExpr);
                    return 1
                }
                else {
                    self.state.push(State::TryErrBlockOrExpr);
                    return 0
                }
            }
            State::TryErrBlockOrExpr=>{
                let err_start = self.code_len() as _ ;
                self.push_code(Opcode::TRY_ERR.into(), self.index);
                if tok.is_open_curly(){
                    self.state.push(State::TryErrBlock{err_start, last_was_sep:false});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                self.state.push(State::TryErrExpr{err_start});
                self.state.push(State::BeginExpr{required:true});
                return 0
            }
            State::TryErrExpr{err_start}=>{
                
                self.set_opcode_args(err_start, OpcodeArgs::from_u32(self.code_len() as u32 -err_start) );
                self.state.push(State::TryOk{was_block:false});
            }
            State::TryErrBlock{err_start, last_was_sep}=>{
                
                self.set_opcode_args(err_start, OpcodeArgs::from_u32(self.code_len() as u32 -err_start) );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::TryOk{was_block: true});
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    self.state.push(State::TryOk{was_block:false});
                    return 0
                }
            }
            State::TryOk{was_block}=>{
                if id == id!(ok){
                    self.state.push(State::TryOkBlockOrExpr);
                    return 1
                }
                if was_block{
                    self.state.push(State::EndExpr)
                }
                return 0
            }
            State::TryOkBlockOrExpr=>{
                let ok_start = self.code_len() as _ ;
                self.push_code(Opcode::TRY_OK.into(), self.index);
                if tok.is_open_curly(){
                    self.state.push(State::TryOkBlock{ok_start, last_was_sep:false});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                self.state.push(State::TryOkExpr{ok_start});
                self.state.push(State::BeginExpr{required:true});
                return 0
            }
            State::TryOkExpr{ok_start}=>{
                self.set_opcode_args(ok_start, OpcodeArgs::from_u32(self.code_len() as u32 -ok_start) );
            }
            State::TryOkBlock{ok_start, last_was_sep}=>{
                self.set_opcode_args(ok_start, OpcodeArgs::from_u32(self.code_len() as u32 -ok_start) );
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::EndExpr);
                    return 1
                }
                else {
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            State::IfTest{index}=>{
                let if_start = self.code_len() as _ ;
                self.push_code(Opcode::IF_TEST.into(), index);
                if tok.is_open_curly(){
                    self.state.push(State::IfTrueBlock{if_start, last_was_sep:false});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                if id == id!(else){ 
                    error!(self, tokenizer, "Unexpected else, use {{}} to disambiguate");
                    return 1
                }
                self.state.push(State::IfTrueExpr{if_start});
                self.state.push(State::BeginExpr{required:true});
                return 0
            }
            State::IfTrueExpr{if_start}=>{
                self.state.push(State::IfMaybeElse{if_start, was_block:false});
                return 0
            }
            State::IfTrueBlock{if_start, last_was_sep}=>{
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    self.state.push(State::IfMaybeElse{if_start, was_block:true});
                    return 1
                }
                else {
                    self.state.push(State::EndExpr);
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            State::IfMaybeElse{if_start, was_block}=>{
                if id == id!(elif){
                    self.push_code(Opcode::IF_ELSE.into(), self.index);
                    self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 - if_start) );

                    self.state.push(State::IfMaybeElse{if_start, was_block});
                    self.state.push(State::IfTest{index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if id == id!(else){
                    let else_start = self.code_len() as u32;
                    self.push_code(Opcode::IF_ELSE.into(), self.index);
                    self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 - if_start));
                    self.state.push(State::IfElse{else_start});
                    return 1
                }
                self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 - if_start).set_need_nil() );
               // self.push_code_none(NIL);
                if was_block{ // allow expression to chain
                    self.state.push(State::EndExpr)
                }
            }
            State::IfElse{else_start}=>{
                if tok.is_open_curly(){
                    self.state.push(State::IfElseBlock{else_start, last_was_sep:false});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                self.state.push(State::IfElseExpr{else_start});
                self.state.push(State::BeginExpr{required:true});
                return 0
            }
            State::IfElseExpr{else_start}=>{
                self.set_opcode_args(else_start, OpcodeArgs::from_u32(self.code_len() as u32 -else_start) );
                return 0
            }
            State::IfElseBlock{else_start, last_was_sep}=>{
                if tok.is_close_curly() {
                    if !last_was_sep && self.has_pop_to_me(){
                        self.clear_pop_to_me();
                    }
                    /*
                    if !last_was_sep{
                        if Some(&Opcode::POP_TO_ME.into()) == self.code_last(){
                            self.pop_code();
                        }
                    }*/
                    self.set_opcode_args(else_start, OpcodeArgs::from_u32(self.code_len() as u32 -else_start) );
                    self.state.push(State::EndExpr);
                    return 1
                }
                else {
                    self.state.push(State::EndExpr);
                    error!(self, tokenizer, "Expected }} not found");
                    return 0
                }
            }
            State::BeginExpr{required}=>{
                if let Some(index) = tok.as_rust_value(){
                    self.push_code(values[index as usize], self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if tok.is_open_curly(){
                    // Check if there's a pending +: operator for proto-inherit
                    if let Some(State::EmitOp{what_op:id!(+:),..}) = self.state.last(){
                        self.state.pop();
                        // Proto-inherit operator: field +: { ... }
                        // Emit PROTO_INHERIT_READ to read field and push proto value
                        self.push_code(Opcode::PROTO_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndProtoInherit);
                        self.state.push(State::BeginStmt{last_was_sep:false});
                        return 1
                    }
                    // Check if there's a pending += operator for scope-inherit
                    if let Some(State::EmitOp{what_op:id!(+=),..}) = self.state.last(){
                        self.state.pop();
                        // Scope-inherit operator: value += { ... }
                        // Emit SCOPE_INHERIT_READ to read variable and push proto value
                        self.push_code(Opcode::SCOPE_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndScopeInherit);
                        self.state.push(State::BeginStmt{last_was_sep:false});
                        return 1
                    }
                    // Check if there's a pending field += for field-inherit
                    if let Some(State::EmitFieldAssign{what_op:id!(+=),..}) = self.state.last(){
                        self.state.pop();
                        // Field-inherit operator: obj.field += { ... }
                        // Emit FIELD_INHERIT_READ to read field and push proto value
                        self.push_code(Opcode::FIELD_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndFieldInherit);
                        self.state.push(State::BeginStmt{last_was_sep:false});
                        return 1
                    }
                    // Check if there's a pending index += for index-inherit
                    if let Some(State::EmitIndexAssign{what_op:id!(+=),..}) = self.state.last(){
                        self.state.pop();
                        // Index-inherit operator: obj[index] += { ... }
                        // Emit INDEX_INHERIT_READ to read index and push proto value
                        self.push_code(Opcode::INDEX_INHERIT_READ.into(), self.index);
                        self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                        self.state.push(State::EndIndexInherit);
                        self.state.push(State::BeginStmt{last_was_sep:false});
                        return 1
                    }
                    self.push_code(Opcode::BEGIN_BARE.into(), self.index);
                    self.state.push(State::EndBare);
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                if tok.is_open_square(){
                    self.push_code(Opcode::BEGIN_ARRAY.into(), self.index);
                    self.state.push(State::EndBareSquare);
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                if tok.is_open_round(){
                    //self.code.push(Opcode::BEGIN_FRAG.into());
                    self.state.push(State::EndRound);
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if let Some(v) = tok.as_f64(){
                    self.push_code(ScriptValue::from_f64(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.as_u40(){
                    self.push_code(ScriptValue::from_u40(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.as_f32(){
                    self.push_code(ScriptValue::from_f32(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.as_u32(){
                    self.push_code(ScriptValue::from_u32(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.as_i32(){
                    self.push_code(ScriptValue::from_i32(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.as_f16(){
                    self.push_code(ScriptValue::from_f16(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if id == id!(if){ // do if as an expression
                    self.state.push(State::IfTest{index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if id == id!(try){ // do if as an expression
                    self.state.push(State::TryTest{index:self.index});
                    return 1
                }
                if id == id!(ok){ // do if as an expression
                    self.state.push(State::OkTest{index:self.index});
                    return 1
                }
                if id == id!(for){
                    self.state.push(State::ForIdent{idents:0, index:self.index});
                    return 1
                }
                if id == id!(loop){
                    self.state.push(State::Loop{index:self.index});
                    return 1
                }
                if id == id!(while){
                    self.state.push(State::While{index:self.index});
                    return 1
                }
                if id == id!(match){
                    // Generate temp variable name based on code position
                    let temp_name = format!("_match_{}", self.code_len());
                    // Use from_str_with_lut to register the temp name in the LUT for lookup
                    let temp_id = LiveId::from_str_with_lut(&temp_name).unwrap_or_else(|_| LiveId::from_str(&temp_name));
                    // Emit temp_id FIRST, then parse subject expression, then LET_DYN
                    self.push_code(ScriptValue::from_id(temp_id), self.index);
                    self.state.push(State::MatchSubject{temp_id, index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if id == id!(use){
                    self.state.push(State::Use{index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if id == id!(fn){
                    self.state.push(State::FnMaybeLet{index:self.index});
                    return 1
                }
                if id == id!(let){
                    // we have to have an identifier after let
                    self.state.push(State::Let{index:self.index});
                    return 1
                }
                if id == id!(var){
                    // we have to have an identifier after var
                    self.state.push(State::Var{index:self.index});
                    return 1
                }
                if id == id!(return){
                    self.state.push(State::EmitReturn{index:self.index});
                    self.state.push(State::BeginExpr{required:false});
                    return 1;
                }
                if id == id!(break){
                    self.state.push(State::EmitBreak{index:self.index});
                    self.state.push(State::BeginExpr{required:false});
                    return 1;
                }
                if id == id!(continue){
                    self.state.push(State::EmitContinue{index:self.index});
                    return 1;
                }
                if id == id!(delete){
                    self.state.push(State::EmitDelete{index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1;
                }
                if id == id!(true){
                    self.push_code(ScriptValue::from_bool(true), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id == id!(false){
                    self.push_code(ScriptValue::from_bool(false), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if id == id!(me){
                    self.push_code(Opcode::ME.into(), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if id == id!(scope){
                    self.push_code(Opcode::SCOPE.into(), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if id == id!(nil){
                    self.push_code(NIL, self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if id.not_empty(){
                    if starts_with_ds{
                        // $identifier escapes by default - no scope resolution
                        self.push_code(id.escape(), self.index);
                    } else {
                        self.push_code(ScriptValue::from_id(id), self.index);
                    }
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.as_color(){
                    self.push_code(ScriptValue::from_color(v), self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(value) = tok.as_string(){
                    self.push_code(value, self.index);
                    self.state.push(State::EndExpr);
                    return 1
                }
                if op == id!(*){
                    self.push_code(ScriptValue::from_id(id!(*)), self.index);
                    self.state.push(State::EndExpr);
                    return 1;
                }
                if op == id!(-) || op == id!(+) || op == id!(!) || op == id!(~){
                    self.state.push(State::EmitUnary{what_op:op, index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if op == id!(@){
                    self.state.push(State::EndExpr);
                    self.state.push(State::EscapedId);
                    return 1
                }
                if op == id!(||){
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    self.state.push(State::FnBody{lambda:true});
                    return 1
                }
                if op == id!(|){
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    self.state.push(State::FnArgList{lambda:true});
                    return 1
                }
                if op == id!(.){
                    self.state.push(State::EmitOp{what_op:id!(me.), index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if op == id!(..){
                    // Prefix .. is the splat operator
                    self.state.push(State::EmitSplat{index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if !required && (sep == id!(;) || sep == id!(,)){
                   // self.push_code(NIL, self.index);
                }
                if required{
                    error!(self, tokenizer, "Expected expression after {:?} found {:?}", self.state, tok);
                    self.push_code_none(NIL);
                }
            }
                        
            State::EndExpr=>{
                if op == id!(~){return 0}
                if op == id!(?){ // we have a post op return if err
                    if let Some(State::EmitOp{what_op,index}) = self.state.last(){
                        if *what_op == id!(.) || *what_op == id!(.?){
                            self.push_code(State::operator_to_opcode(*what_op), *index);
                            self.state.pop();
                        }
                    }
                    self.push_code(State::operator_to_opcode(id!(?)), self.index);
                    return 1
                }
                // named operators
                let op = if id == id!(is){id!(is)}
                else if id == id!(and){id!(&&)}
                else if id == id!(or){id!(||)} 
                else{op};
                
                // Handle short-circuit operators (&&, ||, |?) specially
                // These need the TEST opcode emitted BEFORE the second operand
                if State::is_short_circuit_op(op) {
                    // Emit any pending unary operators first - they bind tighter than all binary ops
                    loop {
                        if let Some(State::EmitUnary{what_op, index}) = self.state.last() {
                            let (what_op, index) = (*what_op, *index);
                            self.state.pop();
                            self.push_code(State::operator_to_unary(what_op), index);
                        } else { break; }
                    }
                    
                    let op_order = State::operator_order(op);
                    
                    // First, process any pending EmitOp with higher or equal precedence
                    // (this ensures proper operator precedence)
                    while let Some(last) = self.state.last() {
                        if let State::EmitOp{what_op, index} = last {
                            if State::operator_order(*what_op) <= op_order {
                                let what_op = *what_op;
                                let index = *index;
                                self.state.pop();
                                self.push_code(State::operator_to_opcode(what_op), index);
                            } else {
                                break;
                            }
                        } else if let State::ShortCircuitEnd{test_slot} = last {
                            // Patch any higher-precedence short-circuit ops
                            let test_slot = *test_slot;
                            self.state.pop();
                            self.set_opcode_args(test_slot, OpcodeArgs::from_u32(self.code_len() - test_slot));
                        } else {
                            break;
                        }
                    }
                    
                    // Emit the TEST opcode with placeholder jump
                    let test_slot = self.code_len();
                    self.push_code(State::short_circuit_opcode(op).into(), self.index);
                    
                    // Push state to patch the jump after second operand is parsed
                    self.state.push(State::ShortCircuitEnd{test_slot});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                
                // Handle ?= with lazy evaluation for SIMPLE variable assignments only
                // Field/index assignments (x.f?=, x[i]?=) use their own opcodes with eager evaluation
                // Emits: [id, ASSIGN_IFNIL(jump), rhs_code, ASSIGN]
                if op == id!(?=) {
                    // Only use lazy eval for simple variable assignment (last code is an identifier)
                    // For field/index assignments, fall through to normal operator handling
                    let is_simple_var_assign = self.code_last().map(|c| c.is_id()).unwrap_or(false)
                        && self.state.last().map(|s| !matches!(s, State::EmitOp{what_op: id!(.) | id!(.?), ..})).unwrap_or(true)
                        && self.code_last() != Some(&Opcode::ARRAY_INDEX.into());
                    
                    if is_simple_var_assign {
                        // Emit any pending unary operators first
                        loop {
                            if let Some(State::EmitUnary{what_op, index}) = self.state.last() {
                                let (what_op, index) = (*what_op, *index);
                                self.state.pop();
                                self.push_code(State::operator_to_unary(what_op), index);
                            } else { break; }
                        }
                        
                        // Emit ASSIGN_IFNIL with placeholder jump
                        let test_slot = self.code_len();
                        self.push_code(Opcode::ASSIGN_IFNIL.into(), self.index);
                        
                        // Push state to emit ASSIGN and patch jump after RHS is parsed
                        self.state.push(State::ShortCircuitAssignEnd{test_slot, index: self.index});
                        self.state.push(State::BeginExpr{required:true});
                        return 1
                    }
                    // else: fall through to normal operator handling for field/index ?=
                }
                
                if State::operator_order(op) != 0{
                    // Emit any pending unary operators, but only if this binary op has lower
                    // precedence than unary (order >= 6). Field access (order 3) has higher
                    // precedence than unary, so -x.y should parse as -(x.y), not (-x).y
                    if State::operator_order(op) >= 6 {
                        loop {
                            if let Some(State::EmitUnary{what_op, index}) = self.state.last() {
                                let (what_op, index) = (*what_op, *index);
                                self.state.pop();
                                self.push_code(State::operator_to_unary(what_op), index);
                            } else { break; }
                        }
                    }
                    
                    let next_state = State::EmitOp{what_op:op, index:self.index};
                    // check if we have a ..[] = 
                    if Some(&Opcode::ARRAY_INDEX.into()) == self.code_last(){
                        if State::is_assign_operator(op){
                            self.pop_code();
                            self.state.push(State::EmitIndexAssign{what_op:op, index:self.index});
                            self.state.push(State::BeginExpr{required:true});
                            return 1
                        }
                    }
                    // check if we need to generate proto_field ops
                    if let Some(last) = self.state.pop(){
                        if let State::EmitOp{what_op:id!(.)|id!(.?),..} = last{
                            if State::is_assign_operator(op){
                                // For : operator with field chain like field.sub: value
                                // transform to me.field.sub = value
                                if op == id!(:) {
                                    // Find the start of the field chain by walking backwards
                                    // The chain structure is: [id, id, (FIELD, id)*]
                                    // e.g. field.sub -> [id(field), id(sub)]
                                    // e.g. field.sub.sub2 -> [id(field), id(sub), FIELD, id(sub2)]
                                    let mut chain_start = self.opcodes.len();
                                    
                                    // Walk backwards through ids and FIELDs
                                    while chain_start > 0 {
                                        let prev = chain_start - 1;
                                        if self.opcodes[prev].is_id() || self.opcodes[prev] == Opcode::FIELD.into() {
                                            chain_start = prev;
                                        } else {
                                            break;
                                        }
                                    }
                                    
                                    // Now insert ME at chain_start
                                    self.opcodes.insert(chain_start, Opcode::ME.into());
                                    self.source_map.insert(chain_start, Some(self.index));
                                    
                                    // Insert PROTO_FIELD after the first id (which is now at chain_start + 1)
                                    // The first id is at chain_start + 1, so PROTO_FIELD goes at chain_start + 2
                                    self.opcodes.insert(chain_start + 2, Opcode::PROTO_FIELD.into());
                                    self.source_map.insert(chain_start + 2, Some(self.index));
                                }
                                
                                // Patch remaining FIELD to PROTO_FIELD
                                for pair in self.opcodes.rchunks_mut(2){
                                    if pair[0].is_id() && pair[1] == Opcode::FIELD.into(){
                                        pair[1] = Opcode::PROTO_FIELD.into()
                                    }
                                    else if pair[1].is_id() && pair[0] == Opcode::FIELD.into(){
                                        pair[0] = Opcode::PROTO_FIELD.into()
                                    }
                                    else{
                                        break
                                    }
                                }
                                self.state.push(State::EmitFieldAssign{what_op:op, index:self.index});
                                self.state.push(State::BeginExpr{required:true});
                                return 1
                            }
                        }
                        if last.is_heq_prio(next_state){
                            // Push `last` back first
                            self.state.push(last);
                            
                            // Find the correct position to insert the new operator.
                            // It should be inserted below ALL EmitOp states that have higher or equal priority.
                            // This ensures correct operator precedence for expressions like t.x*t.y + t.z*t.w
                            let op_order = State::operator_order(op);
                            let is_assign = State::is_assign_operator(op);
                            let mut insert_pos = self.state.len();
                            for i in (0..self.state.len()).rev() {
                                if let State::EmitOp{what_op, ..} = &self.state[i] {
                                    // If both are assignment operators, don't treat as heq_prio (right-to-left associativity)
                                    let pending_is_assign = State::is_assign_operator(*what_op);
                                    if pending_is_assign && is_assign {
                                        break;
                                    }
                                    if State::operator_order(*what_op) <= op_order {
                                        insert_pos = i;
                                    } else {
                                        break;
                                    }
                                } else {
                                    break;
                                }
                            }
                            
                            // Insert new operator and BeginExpr at the found position
                            self.state.insert(insert_pos, State::BeginExpr{required:true});
                            self.state.insert(insert_pos, State::EmitOp{what_op:op, index:self.index});
                            return 1
                        }
                        else{
                            self.state.push(last);
                        }
                    }
                    self.state.push(State::EmitOp{what_op:op, index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                                
                if tok.is_open_curly() {
                                        
                    for state in self.state.iter().rev(){
                        if let State::EmitOp{..} = state{}
                        else if let State::EmitUnary{..} = state{}
                        else if let State::ShortCircuitEnd{..} = state{}
                        else if let State::IfTest{..} = state{
                            return 0
                        }
                        else if let State::TryTestExpr{..} = state{
                            return 0
                        }
                        else if let State::WhileTest{..} = state{
                            return 0
                        }
                        else if let State::ForBody{..} = state{
                            return 0
                        }
                        else if let State::MatchSubject{..} = state{
                            return 0
                        }
                        else{
                            break;
                        }
                    }
                    if let Some(last) = self.state.pop(){
                        if let State::EmitOp{what_op:id!(.),index} = last{
                            self.push_code(State::operator_to_opcode(id!(.)), index);
                        }
                        else if let State::EmitOp{what_op:id!(.?),index} = last{
                            self.push_code(State::operator_to_opcode(id!(.?)), index);
                        }
                        else if let State::EmitOp{what_op:id!(+:),..} = last{
                            // Proto-inherit operator: field +: Proto { ... }
                            // Emit PROTO_INHERIT_READ to read field and push proto value
                            self.push_code(Opcode::PROTO_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndProtoInherit);
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 1
                        }
                        else if let State::EmitOp{what_op:id!(+=),..} = last{
                            // Scope-inherit operator: value += Proto { ... }
                            // Emit SCOPE_INHERIT_READ to read variable and push proto value
                            self.push_code(Opcode::SCOPE_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndScopeInherit);
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 1
                        }
                        else if let State::EmitFieldAssign{what_op:id!(+=),..} = last{
                            // Field-inherit operator: obj.field += Proto { ... }
                            // Emit FIELD_INHERIT_READ to read field and push proto value
                            self.push_code(Opcode::FIELD_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndFieldInherit);
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 1
                        }
                        else if let State::EmitIndexAssign{what_op:id!(+=),..} = last{
                            // Index-inherit operator: obj[index] += Proto { ... }
                            // Emit INDEX_INHERIT_READ to read index and push proto value
                            self.push_code(Opcode::INDEX_INHERIT_READ.into(), self.index);
                            self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                            self.state.push(State::EndIndexInherit);
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 1
                        }
                        else{
                            self.state.push(last);
                        }
                    }
                    self.push_code(Opcode::BEGIN_PROTO.into(), self.index);
                    self.state.push(State::EndProto);
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                if tok.is_open_round(){ 
                    if let Some(last) = self.state.pop(){
                        if let State::EmitOp{what_op:id!(.)|id!(.?),..} = last{
                            self.push_code(Opcode::METHOD_CALL_ARGS.into(), self.index);
                            self.state.push(State::EndCall{is_method:true, index:self.index});
                            self.state.push(State::BeginStmt{last_was_sep:false});
                        }
                        else{
                            self.state.push(last);
                            self.push_code(Opcode::CALL_ARGS.into(), self.index);
                            self.state.push(State::EndCall{is_method:false, index:self.index});
                            self.state.push(State::BeginStmt{last_was_sep:false});
                        }
                    }
                    return 1
                }
                if tok.is_open_square(){
                    if let Some(last) = self.state.pop(){
                        if let State::EmitOp{what_op:id!(.),index} = last{
                            self.push_code(State::operator_to_opcode(id!(.)), index);
                        }
                        else if let State::EmitOp{what_op:id!(.?),index} = last{
                            self.push_code(State::operator_to_opcode(id!(.?)), index);
                        }
                        else{
                            self.state.push(last);
                        }
                    }
                    self.state.push(State::ArrayIndex);
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                return 0
            }
            State::BeginStmt{last_was_sep} => {
                
                if sep == id!(;) || sep == id!(,){ // just eat it
                    // we can pop all operator emits
                    self.state.push(State::BeginStmt{last_was_sep:true});
                    // we should also force a 'nil' in if/else/fn calls just like Rust
                    return 1
                }
                if tok.is_close_round() || tok.is_close_curly() || tok.is_close_square(){
                   
                    if last_was_sep{
                        if let Some(State::TryTestBlock{last_was_sep,..}) = self.state.last_mut(){*last_was_sep = true}
                        if let Some(State::TryErrBlock{last_was_sep,..}) = self.state.last_mut(){*last_was_sep = true}
                        if let Some(State::TryOkBlock{last_was_sep,..}) = self.state.last_mut(){*last_was_sep = true}
                        if let Some(State::IfTrueBlock{last_was_sep,..}) = self.state.last_mut(){*last_was_sep = true}
                        if let Some(State::IfElseBlock{last_was_sep,..}) = self.state.last_mut(){*last_was_sep = true}
                        if let Some(State::EndFnBlock{last_was_sep,..}) = self.state.last_mut(){*last_was_sep = true}
                    }
                    // pop and let the stack handle it
                    return 0
                }
                // lets do an expression statement as fallthrough
                self.state.push(State::EndStmt{last:self.index});
                self.state.push(State::BeginExpr{required:false});
                return 0;
            }
            State::EndStmt{last}=>{
                if last == self.index{
                    error!(self, tokenizer, "Parser stuck on character {:?}, skipping", tok);
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                // in a function call we need the 
                
                if let Some(code) = self.opcodes.last_mut(){
                    if let Some((opcode,_)) = code.as_opcode(){
                        if opcode == Opcode::FOR_END{
                            //code.set_opcode_is_statement();
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 0;
                        }
                        if opcode == Opcode::ASSIGN_ME{
                            //code.set_opcode_is_statement();
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 0;
                        }
                        if opcode == Opcode::BREAK || opcode == Opcode::CONTINUE{
                            //code.set_opcode_is_statement();
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 0;
                        }
                        if opcode == Opcode::ME_SPLAT{
                            // ME_SPLAT already handles merging into me, no pop_to_me needed
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 0;
                        }
                        if code.is_let_opcode(){
                            self.state.push(State::BeginStmt{last_was_sep:false});
                            return 0;
                        }
                    }
                }
                // otherwise pop to me
                self.set_pop_to_me();
                //self.push_code_none(Opcode::POP_TO_ME.into());
                self.state.push(State::BeginStmt{last_was_sep:false});
                return 0
            }
        }
        0
    }
    
    pub fn parse(&mut self, tokenizer: &ScriptTokenizer, file: &str, offsets: (usize, usize), values: &[ScriptValue]){
        self.file = file.to_string();
        self.line_offset = offsets.0;
        self.col_offset = offsets.1;
        // wait for the tokens to be consumed
        let mut steps_zero = 0;
        let tokens = &tokenizer.tokens;
        while self.index < tokens.len() as u32 && self.state.len()>0{
            
            let tok = if let Some(tok) = tokens.get(self.index as usize){
                tok.token.clone()
            }
            else{
                ScriptToken::StreamEnd
            };
            
            let step = self.parse_step(tokenizer, tok, values);
            if step == 0{
                steps_zero += 1;
            }
            else{
                steps_zero = 0;
            }
           // println!("{:?} {:?}", self.code, self.state);
            if self.state.len()<=1 && steps_zero > 1000{
                error!(self, tokenizer, "Parser stuck {:?} {} {:?}", self.state, step, tokens[self.index as usize]);
                break;
            }
            self.index += step;
        }
        
        // Handle the last value as the script's return value, similar to function blocks
        if self.has_pop_to_me(){
            self.clear_pop_to_me();
            self.push_code(Opcode::RETURN.into(), self.index.saturating_sub(1));
        }
        else if self.opcodes.len() > 0 {
            // Check if last opcode already returns or is a statement that doesn't produce a value
            let needs_nil_return = if let Some(code) = self.code_last(){
                if let Some((opcode,_)) = code.as_opcode(){
                    opcode != Opcode::RETURN
                }
                else{
                    true
                }
            }
            else{
                true
            };
            if needs_nil_return{
                self.push_code(ScriptValue::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL), self.index.saturating_sub(1));
            }
        }
        
        //println!("{:?}", self.opcodes)
    }
    
    pub fn dump_opcodes(&self) {
        println!("=== OPCODES ({} total) ===", self.opcodes.len());
        for (i, op) in self.opcodes.iter().enumerate() {
            println!("{:3}: {:?}", i, op);
        }
        println!("=== END OPCODES ===");
    }
}
