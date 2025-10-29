use crate::tokenizer::*;
use crate::makepad_live_id::live_id::*;
use crate::value::*;
use crate::opcode::*;
use crate::makepad_live_id::makepad_live_id_macros::*;

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
        
    FnArgList,
    FnArgMaybeType{index:u32},
    FnArgType{index:u32},
    FnBody,
    EndFnBlock{fn_slot:u32, last_was_sep:bool, index:u32},
    EndFnExpr{fn_slot:u32, index:u32},
    EmitFnArgTyped{index:u32},
    EmitFnArgDyn{index:u32},
    
    EmitUnary{what_op:LiveId, index:u32},
    EmitOp{what_op:LiveId, index:u32},
    EmitFieldAssign{what_op:LiveId, index:u32},
    EmitIndexAssign{what_op:LiveId, index:u32},
    
    EndBare,
    EndBareSquare,
    EndProto,
    EndRound,
    
    CallMaybeDo{is_method:bool, index:u32},
    EmitCall{is_method:bool, index:u32},
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
            id!(:) | id!(=) | id!(>:) | id!(<:) | id!(^:) | id!(+=)  | id!(-=) | id!(*=) | id!(/=) | id!(%=) => 19,
            id!(&=) | id!(|=)  | id!(^=) | id!(<<=) | id!(>>=) | id!(?=) => 20,
            _=>0
        }
    }
    
    fn is_assign_operator(op:LiveId)->bool{
        match op{
            id!(=) | id!(:) | id!(+=) | id!(<:) | id!(+=) |
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
        /*
            id!(is) |
            id!(==) | id!(!=) | 
            id!(===) => Opcode::SHALLOW_EQ,
            id!(!==) => Opcode::SHALLOW_NEQ,
                                    
            id!(&&) => Opcode::LOGIC_AND,
            id!(||)  => Opcode::LOGIC_OR,
            id!(|?) => Opcode::NIL_OR,
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
        }*/
    }
    
    fn operator_to_field_assign(op:LiveId)->ScriptValue{
        match op{
            id!(=) => Opcode::ASSIGN_FIELD,
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
                        
            id!(&&) => Opcode::LOGIC_AND,
            id!(||)  => Opcode::LOGIC_OR,
            id!(|?) => Opcode::NIL_OR,
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



pub struct ScriptParser{
    pub index: u32,
    pub opcodes: Vec<ScriptValue>,
    pub source_map: Vec<Option<u32>>,
    
    state: Vec<State>,
}

impl Default for ScriptParser{
    fn default()->Self{
        Self{
            index: 0,
            opcodes: Default::default(),
            source_map: Default::default(),
            state: vec![State::BeginStmt{last_was_sep:false}],
        }
    }
}

impl ScriptParser{
    
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
            code.has_opcode_args_pop_to_me()
        }
        else{
            false
        }
    }
    
    fn clear_pop_to_me(&mut self){
        if let Some(code) = self.opcodes.last_mut(){
            code.clear_opcode_args_pop_to_me();
        }
    }
    
    fn set_opcode_args(&mut self, index:u32, args: OpcodeArgs){
        self.opcodes[index as usize].set_opcode_args(args);
    }
    
    fn parse_step(&mut self, tok:ScriptToken, values: &[ScriptValue])->u32{
        
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
                        println!("Too many identifiers in for");
                        return 0
                    }
                }
                if op == id!(,){ // eat the commas
                    self.state.push(State::ForIdent{idents, index});
                    return 1
                }
                println!("Unexpected state in parsing for");
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
                    println!("Wrong number of identifiers for for loop {idents}");
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
                    println!("Expected }} not found in for");
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
                        println!("Error use expected field operation")
                    }
                }
            }
            State::Let{index}=>{
                if id.not_empty(){ // lets expect an assignment expression
                    // push the id on to the stack
                    self.push_code(id.into(), self.index);
                    self.state.push(State::LetDynOrTyped{index});
                    return 1
                }
                else{ // unknown
                    println!("Let expected identifier");
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
                    println!("Let type expected");
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
            State::EndRound=>{
                // we expect a ) here
                //self.code.push(Opcode::END_FRAG.into());
                if tok.is_close_round(){
                    self.state.push(State::EndExpr);
                    return 1
                }
                else{
                    println!("Expected )")
                }
            }
            State::EmitFnArgTyped{index}=>{
                self.push_code(ScriptValue::from_opcode_args(Opcode::FN_ARG_TYPED, OpcodeArgs::NIL), index);
            }
            State::EmitFnArgDyn{index}=>{
                self.push_code(ScriptValue::from_opcode_args(Opcode::FN_ARG_DYN, OpcodeArgs::NIL), index);
            }
            State::FnArgType{index}=>{
                if id.not_empty(){
                    self.push_code(id.into(), self.index);
                    self.state.push(State::EmitFnArgTyped{index:self.index});
                    return 1
                }
                else{
                    self.state.push(State::EmitFnArgDyn{index});
                    println!("Argument type expected in function")
                }
            }
            State::FnArgMaybeType{index}=>{
                if op == id!(:){
                    self.state.push(State::FnArgType{index});
                    return 1
                }
                self.state.push(State::EmitFnArgDyn{index});
            }
            State::FnArgList=>{
                if id.not_empty(){ // ident
                    self.push_code(id.into(), self.index);
                    self.state.push(State::FnArgList);
                    self.state.push(State::FnArgMaybeType{index:self.index});
                    return 1
                }
                if op == id!(|){
                    self.state.push(State::FnBody);
                    return 1
                }
                // unexpected token, but just stay in the arg list mode
                println!("Unexpected token in function argument list {:?}", tok);
                self.state.push(State::FnArgList);
                return 1
            }
            State::FnBody=>{
                let fn_slot = self.code_len() as _ ;
                self.push_code(Opcode::FN_BODY.into(), self.index);
                if tok.is_open_curly(){ // function body
                    self.state.push(State::EndFnBlock{fn_slot, last_was_sep:false, index:self.index});
                    self.state.push(State::BeginStmt{last_was_sep:false});
                    return 1
                }
                else{ // function body is expression
                    self.state.push(State::EndFnExpr{fn_slot, index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                }
            }
            State::EscapedId=>{
                if id.not_empty(){ // ident
                    self.push_code(id.escape(), self.index);
                    return 1
                }
                else{
                    println!("Expected identifier after @");
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
                    println!("Expected }} not found");
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
                            }
                            return 0
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
                    println!("Expected ] not found");
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
                    println!("Expected }} not found");
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
                    println!("Expected }} not found");
                    return 0
                }
            }
            State::EmitCall{is_method, index}=>{
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
                    self.state.push(State::EmitCall{is_method, index});
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
                    println!("Expected ) not found");
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
                    println!("Expected ] not found");
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
                    println!("Expected }} not found");
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
                    println!("Expected }} not found");
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
                    println!("Unexpected else, use {{}} to disambiguate");
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
                    println!("Expected }} not found");
                    return 0
                }
            }
            State::IfMaybeElse{if_start, was_block}=>{
                if id == id!(elif){
                    let else_start = self.code_len() as u32;
                    self.push_code(Opcode::IF_ELSE.into(), self.index);
                    self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 -if_start) );

                    self.state.push(State::IfElse{else_start});
                    self.state.push(State::IfTest{index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if id == id!(else){
                    let else_start = self.code_len() as u32;
                    self.push_code(Opcode::IF_ELSE.into(), self.index);
                    self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 -if_start) );
                    self.state.push(State::IfElse{else_start});
                    return 1
                }
                self.set_opcode_args(if_start, OpcodeArgs::from_u32(self.code_len() as u32 -if_start) );
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
                    println!("Expected }} not found");
                    return 0
                }
            }
            State::BeginExpr{required}=>{
                if let Some(index) = tok.as_rust_value(){
                    self.push_code(values[index as usize], self.index);
                    return 1
                }
                if tok.is_open_curly(){
                    /*
                    if let Some(State::EmitUnary{what_op:id!(+),..}) = self.state.last(){
                        self.state.pop();
                        if let Some(State::EmitOp{what_op:id!(:),..}) = self.state.last(){
                            // ok so we need to emit BEGIN_PROTO_ME
                            self.push_code(Opcode::BEGIN_PROTO_ME.into(), self.index);
                            self.state.push(State::EndBare);
                            self.state.push(State::BeginStmt(false));
                            return 1
                        }
                        else{
                            println!("Found +{{ protoinherit. Left hand side must be field:")
                        }
                    }*/
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
                if let Some(v) = tok.as_number(){
                    self.push_code(ScriptValue::from_f64(v), self.index);
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
                if id == id!(use){
                    self.state.push(State::Use{index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if id == id!(let){
                    // we have to have an identifier after let
                    self.state.push(State::Let{index:self.index});
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
                    self.push_code(ScriptValue::from_id(id), self.index);
                    if starts_with_ds{
                        self.push_code(Opcode::SEARCH_TREE.into(), self.index);
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
                    self.state.push(State::FnBody);
                    return 1
                }
                if op == id!(|){
                    self.push_code(Opcode::FN_ARGS.into(), self.index);
                    self.state.push(State::FnArgList);
                    return 1
                }
                if op == id!(.){
                    self.state.push(State::EmitOp{what_op:id!(me.), index:self.index});
                    self.state.push(State::BeginExpr{required:true});
                    return 1
                }
                if !required && (sep == id!(;) || sep == id!(,)){
                   // self.push_code(NIL, self.index);
                }
                if required{
                    println!("Expected expression after {:?} found {:?}", self.state, tok);
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
                
                if State::operator_order(op) != 0{
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
                                for pair in self.opcodes.rchunks_mut(2){
                                    if pair[0] == Opcode::FIELD.into() && pair[1].is_id(){
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
                            self.state.push(State::EmitOp{what_op:op, index:self.index});
                            self.state.push(State::BeginExpr{required:true});
                            self.state.push(last);
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
                    println!("Parser stuck on character {:?}, skipping", tok);
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
    
    pub fn parse(&mut self, tokens:&[ScriptTokenPos], values: &[ScriptValue]){
        // wait for the tokens to be consumed
        let mut steps_zero = 0;
        while self.index < tokens.len() as u32 && self.state.len()>0{
            
            let tok = if let Some(tok) = tokens.get(self.index as usize){
                tok.token.clone()
            }
            else{
                ScriptToken::StreamEnd
            };
            
            let step = self.parse_step(tok, values);
            if step == 0{
                steps_zero += 1;
            }
            else{
                steps_zero = 0;
            }
           // println!("{:?} {:?}", self.code, self.state);
            if self.state.len()<=1 && steps_zero > 1000{
                println!("Parser stuck {:?} {} {:?}", self.state, step, tokens[self.index as usize]);
                break;
            }
            self.index += step;
        }
        //println!("MADE CODE: {:?}", self.opcodes);
    }
}
