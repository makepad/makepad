#![allow(dead_code)]
use crate::tokenizer::*;
use crate::id::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use makepad_script_derive::*;

#[derive(Default, Debug, Eq, PartialEq)]
enum State{
    #[default]
    BeginStmt,
    BeginExpr,
    EndExpr,
    EndStmt,
    
    EscapedId,
    
    IfTest,
    IfTrueExpr(usize),
    IfTrueBlock(usize),
    IfMaybeElse(bool),
    IfElse(usize),
    IfElseExpr(usize),
    IfElseBlock(usize),
    
    FnArgList,
    FnArgMaybeType,
    FnArgType,
    FnBody,
    EndFnBlock(usize),
    EndFnExpr(usize),
    EmitFnArgTyped,
    EmitFnArgDyn,
    
    EmitUnary(Id),
    EmitOp(Id),
    EmitFieldAssign(Id),
    EmitIndexAssign(Id),
    EndBare,
    EndBareSquare,
    EndProto,
    EndRound,
    
    CallMaybeDo,
    EmitCall,
    EndCall,
    ArrayIndex,
    
    For,
    ForIdent,
    ForRange,
    
    Let,
    LetDynOrTyped,
    LetType,
    LetTypedAssign,
    EmitLetDyn,
    EmitLetTyped
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
    fn operator_order(op:Id)->usize{
        match op{
            id!(.) => 3,
            id!(*) | id!(/) | id!(%) => 8,
            id!(+) | id!(-) => 9,
            id!(<<) | id!(>>) => 10,
            id!(&)  => 11,
            id!(^)  => 12,
            id!(|)  => 13,
            id!(++) => 14,
            id!(==) | id!(!=)  | id!(<) | id!(>) | id!(<=) | id!(>=) => 15,
            id!(&&)  => 16,
            id!(||)  => 17,
            id!(:) | id!(=) | id!(+=)  | id!(-=) | id!(*=) | id!(/=) | id!(%=) => 18,
            id!(&=) | id!(|=)  | id!(^=) | id!(<<=) | id!(>>=) => 19,
            _=>0
        }
    }
    
    fn is_assign_operator(op:Id)->bool{
        match op{
            id!(=) | id!(:) | id!(+=) | 
            id!(-=) | id!(*=) | id!(/=) |
            id!(%=) | id!(&=) | id!(|=) | 
            id!(^=) | id!(<<=) | id!(>>=) | 
            id!(?=)  => true,
            _=>false
        }
    }
    
    fn operator_to_field_assign(op:Id)->Value{
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
    
    fn operator_to_index_assign(op:Id)->Value{
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
    
    fn operator_to_unary(op:Id)->Value{
        match op{
            id!(!)=> Opcode::NOT,
            id!(-)=> Opcode::NEG,
            _=>Opcode::NOP
        }.into()
    }
    
    fn operator_to_opcode(op:Id)->Value{
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
            id!(==) => Opcode::EQ,
            id!(!=) => Opcode::NEQ,
            id!(<) => Opcode::LT,
            id!(>) => Opcode::GT,
            id!(<=) => Opcode::LEQ,
            id!(>=) => Opcode::GEQ,
            id!(&&) => Opcode::LOGIC_AND,
            id!(||)  => Opcode::LOGIC_OR,
            id!(:) => Opcode::ASSIGN_ME,
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
            id!(.)  => Opcode::FIELD,
            _=> Opcode::NOP,
        }.into()
    }
    
    fn is_heq_prio(&self, other:State)->bool{
        match self{
            Self::EmitOp(op1)=>{
                match other{
                    Self::EmitOp(op2)=>{
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
    pub index: usize,
    pub tok: ScriptTokenizer,
    pub code: Vec<Value>,
    state: Vec<State>,
    opstack: Vec<Id>
}

impl Default for ScriptParser{
    fn default()->Self{
        Self{
            index: 0,
            tok: Default::default(),
            code: Default::default(),
            opstack: Default::default(),
            state: vec![State::BeginStmt],
        }
    }
}

impl ScriptParser{
    
    fn push_state(&mut self, state:State){
        self.state.push(state)
    }
    
    fn handle(&mut self, heap:&mut ScriptHeap)->usize{
        let tok = if let Some(tok) = self.tok.tokens.get(self.index){
            tok.token.clone()
        }
        else{
            ScriptToken::StreamEnd
        };
        let op = tok.operator();
        let (id,starts_with_ds) = tok.identifier();
        match self.state.pop().unwrap(){
            State::For=>{}
            State::ForIdent=>{}
            State::ForRange=>{}
            State::Let=>{
                if id != id!(){ // lets expect an assignment expression
                    // push the id on to the stack
                    self.code.push(id.into());
                    self.state.push(State::LetDynOrTyped);
                    return 1
                }
                else{ // unknown
                    println!("Let expected identifier");
                }
            }
            State::LetDynOrTyped=>{
                if op == id!(=){ // assignment following
                    self.state.push(State::EmitLetDyn);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                else if op == id!(:){ // type following
                    self.state.push(State::LetType);
                    return 1
                }
                else{
                    self.code.push(Value::from_opcode_args(Opcode::LET_DYN, OpcodeArgs::NIL));
                }
            }
            State::LetType=>{
                if id != id!(){ // lets expect an assignment expression
                    // push the id on to the stack
                    self.code.push(id.into());
                    self.state.push(State::LetTypedAssign);
                    return 1
                }
                else{ // unknown
                    println!("Let type expected");
                }
            }
            State::LetTypedAssign=>{
                if op == id!(=){ // assignment following
                    self.state.push(State::EmitLetTyped);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                else{
                    self.code.push(Value::from_opcode_args(Opcode::LET_TYPED, OpcodeArgs::NIL));
                }
            }
            State::EmitLetDyn=>{
                self.code.push(Opcode::LET_DYN.into());
            }
            State::EmitLetTyped=>{
                self.code.push(Opcode::LET_TYPED.into());
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
            State::EmitFnArgTyped=>{
                self.code.push(Value::from_opcode_args(Opcode::FN_ARG_TYPED, OpcodeArgs::NIL));
            }
            State::EmitFnArgDyn=>{
                self.code.push(Value::from_opcode_args(Opcode::FN_ARG_DYN, OpcodeArgs::NIL));
            }
            State::FnArgType=>{
                if id != id!(){
                    self.code.push(id.into());
                    self.state.push(State::EmitFnArgTyped);
                    return 1
                }
                else{
                    self.state.push(State::EmitFnArgDyn);
                    println!("Argument type expected in function")
                }
            }
            State::FnArgMaybeType=>{
                if op == id!(:){
                    self.state.push(State::FnArgType);
                    return 1
                }
                self.state.push(State::EmitFnArgDyn);
            }
            State::FnArgList=>{
                if id != id!(){ // ident
                    self.code.push(id.into());
                    self.state.push(State::FnArgList);
                    self.state.push(State::FnArgMaybeType);
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
                let fn_slot = self.code.len();
                self.code.push(Opcode::NOP.into());
                if tok.is_open_curly(){ // function body
                    self.state.push(State::EndFnBlock(fn_slot));
                    self.state.push(State::BeginStmt);
                    return 1
                }
                else{ // function body is expression
                    self.state.push(State::EndFnExpr(fn_slot));
                    self.state.push(State::BeginExpr);
                }
            }
            State::EscapedId=>{
                if id != id!(){ // ident
                    let value = Value::from_escaped_id(id);
                    self.code.push(value);
                    return 1
                }
                else{
                    println!("Expected identifier after @");
                }
            }
            State::EndFnExpr(fn_slot)=>{
                self.code.push(Opcode::RETURN.into());
                self.code[fn_slot] = Value::from_opcode_args(Opcode::FN_BODY, OpcodeArgs::from_u32((self.code.len()-fn_slot) as u32));
            }
            State::EndFnBlock(fn_slot)=>{
                self.code.push(Value::from_opcode_args(Opcode::RETURN, OpcodeArgs::NIL));
                self.code[fn_slot] = Value::from_opcode_args(Opcode::FN_BODY, OpcodeArgs::from_u32((self.code.len()-fn_slot) as u32));
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    println!("Expected }} not found");
                    return 0
                }
            }
            // alright we parsed a + b * c
            State::EmitFieldAssign(what_op)=>{
                self.code.push(State::operator_to_field_assign(what_op));
            }
            State::EmitIndexAssign(what_op)=>{
                self.code.push(State::operator_to_index_assign(what_op));
            }
            State::EmitOp(what_op)=>{
                self.code.push(State::operator_to_opcode(what_op));
                return 0
            }
            State::EmitUnary(what_op)=>{
                self.code.push(State::operator_to_unary(what_op));
                return 0
            }
            State::EndBareSquare=>{
                self.code.push(Opcode::END_BARE.into());
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
                self.code.push(Opcode::END_BARE.into());
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
                self.code.push(Opcode::END_PROTO.into());
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    println!("Expected }} not found");
                    return 0
                }
            }
            State::EmitCall=>{
                self.code.push(Opcode::CALL_EXEC.into());
                self.state.push(State::EndExpr);
            }
            State::CallMaybeDo=>{
                if id == id!(do){
                    println!("DO");
                    self.state.push(State::EmitCall);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                else{
                    self.code.push(Opcode::CALL_EXEC.into());
                    self.state.push(State::EndExpr);
                    return 0
                }
            }
            State::EndCall=>{
                // expect )
                self.state.push(State::CallMaybeDo);
                if tok.is_close_round() {
                    return 1
                }
                else {
                    println!("Expected ) not found");
                    return 0
                }
            }
            State::ArrayIndex=>{
                self.code.push(Opcode::ARRAY_INDEX.into());
                self.state.push(State::EndExpr);
                if tok.is_close_square() {
                    return 1
                }
                else {
                    println!("Expected ] not found");
                    return 0
                }
            }
            State::EndExpr=>{
                if State::operator_order(op) != 0{
                    /*if State::is_assign_operator(op){
                        // lets error on assignments in pure expression position
                        println!("{:?}", self.state);
                        println!("{:?}", self.code);
                    }*/
                    
                    let next_state = State::EmitOp(op);
                    // check if we have a ..[] = 
                    if Some(&Opcode::ARRAY_INDEX.into()) == self.code.last(){
                        if State::is_assign_operator(op){
                            self.code.pop();
                            self.state.push(State::EmitIndexAssign(op));
                            self.state.push(State::BeginExpr);
                            return 1
                        }
                    }
                    if let Some(last) = self.state.pop(){
                        if let State::EmitOp(id!(.)) = last{
                            if State::is_assign_operator(op){
                                for pair in self.code.rchunks_mut(2){
                                    if pair[0] == Opcode::FIELD.into() && pair[1].is_id(){
                                        pair[0] = Opcode::PROTO_FIELD.into()
                                    }
                                    else{
                                        break
                                    }
                                }
                                self.state.push(State::EmitFieldAssign(op));
                                self.state.push(State::BeginExpr);
                                return 1
                            }
                        }
                        if last.is_heq_prio(next_state){
                            self.state.push(State::EmitOp(op));
                            self.state.push(State::BeginExpr);
                            self.state.push(last);
                            return 1
                        }
                        else{
                            self.state.push(last);
                        }
                    }
                    self.state.push(State::EmitOp(op));
                    self.state.push(State::BeginExpr);
                    return 1
                }
                
                if tok.is_open_curly() {
                    for state in self.state.iter().rev(){
                        if let State::EmitOp(_) = state{}
                        else if let State::IfTest = state{
                            return 0
                        }
                        else{
                            break;
                        }
                    }
                    self.code.push(Opcode::BEGIN_PROTO.into());
                    self.state.push(State::EndProto);
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if tok.is_open_round(){ 
                    if let Some(last) = self.state.pop(){
                        if let State::EmitOp(id!(.)) = last{
                            self.code.push(State::operator_to_opcode(id!(.)));
                        }
                        else{
                            self.state.push(last);
                        }
                    }
                    self.code.push(Opcode::CALL_ARGS.into());
                    self.state.push(State::EndCall);
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if tok.is_open_square(){
                    if let Some(last) = self.state.pop(){
                        if let State::EmitOp(id!(.)) = last{
                            self.code.push(State::operator_to_opcode(id!(.)));
                        }
                        else{
                            self.state.push(last);
                        }
                    }
                    self.state.push(State::ArrayIndex);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                return 0
            }
            State::IfTest=>{
                let if_start = self.code.len();
                self.code.push(Opcode::NOP.into());
                if tok.is_open_curly(){
                    self.state.push(State::IfTrueBlock(if_start));
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if id == id!(else){ 
                    println!("Unexpected else, use {{}} to disambiguate");
                    return 1
                }
                self.state.push(State::IfTrueExpr(if_start));
                self.state.push(State::BeginExpr);
                return 0
            }
            State::IfTrueExpr(if_start)=>{
                self.code[if_start] = Value::from_opcode_args(Opcode::IF_TEST, OpcodeArgs::from_u32((self.code.len()-if_start) as u32));
                self.state.push(State::IfMaybeElse(false));
                return 0
            }
            State::IfTrueBlock(if_start)=>{
                if tok.is_close_curly() {
                    if Some(&Opcode::POP_TO_ME.into()) == self.code.last(){
                        self.code.pop();
                    }
                    self.code[if_start] = Value::from_opcode_args(Opcode::IF_TEST, OpcodeArgs::from_u32((self.code.len()-if_start) as u32));
                    self.state.push(State::IfMaybeElse(true));
                    return 1
                }
                else {
                    self.state.push(State::EndExpr);
                    println!("Expected }} not found");
                    return 0
                }
            }
            State::IfMaybeElse(was_block)=>{
                if id == id!(else){
                    let else_start = self.code.len();
                    self.code.push(Opcode::NOP.into());
                    self.state.push(State::IfElse(else_start));
                    return 1
                }
                if was_block{ // allow expression to chain
                    self.state.push(State::EndExpr)
                }
            }
            State::IfElse(else_start)=>{
                if tok.is_open_curly(){
                    self.state.push(State::IfElseBlock(else_start));
                    self.state.push(State::BeginStmt);
                    return 1
                }
                self.state.push(State::IfElseExpr(else_start));
                self.state.push(State::BeginExpr);
                return 0
            }
            State::IfElseExpr(else_start)=>{
                self.code[else_start] = Value::from_opcode_args(Opcode::IF_ELSE, OpcodeArgs::from_u32((self.code.len()-else_start) as u32));
                return 0
            }
            State::IfElseBlock(else_start)=>{
                if tok.is_close_curly() {
                    if Some(&Opcode::POP_TO_ME.into()) == self.code.last(){
                        self.code.pop();
                    }
                    self.code[else_start] = Value::from_opcode_args(Opcode::IF_ELSE, OpcodeArgs::from_u32((self.code.len()-else_start) as u32));
                    self.state.push(State::EndExpr);
                    return 1
                }
                else {
                    self.state.push(State::EndExpr);
                    println!("Expected }} not found");
                    return 0
                }
            }
            State::BeginExpr=>{
                if tok.is_open_curly(){
                    self.code.push(Opcode::BEGIN_BARE.into());
                    self.state.push(State::EndBare);
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if tok.is_open_square(){
                    self.code.push(Opcode::BEGIN_ARRAY.into());
                    self.state.push(State::EndBareSquare);
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if tok.is_open_round(){
                    //self.code.push(Opcode::BEGIN_FRAG.into());
                    self.state.push(State::EndRound);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                if let Some(v) = tok.maybe_number(){
                    self.code.push(Value::from_f64(v));
                    self.state.push(State::EndExpr);
                    return 1
                }
                if id == id!(if){ // do if as an expression
                    self.state.push(State::IfTest);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                if id != id!(){
                    self.code.push(Value::from_id(id));
                    if starts_with_ds{
                        self.code.push(Opcode::SEARCH_TREE.into());
                    }
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.maybe_color(){
                    self.code.push(Value::from_color(v));
                    return 1
                }
                if let Some(ptr) = tok.maybe_string(){
                    // maybe make the string inline
                    let str = heap.string(ptr);
                    if let Some(value) = Value::from_inline_string(str){
                        self.code.push(value);
                    }
                    else{
                        self.code.push(Value::from_string(ptr));
                    }
                    return 1
                }
                if op == id!(-) || op == id!(!) {
                    self.state.push(State::EmitUnary(op));
                    self.state.push(State::BeginExpr);
                    return 1
                }
                if op == id!(@){
                    self.state.push(State::EscapedId);
                    return 1
                }
                if op == id!(|){
                    self.code.push(Opcode::FN_ARGS.into());
                    self.state.push(State::FnArgList);
                    return 1
                }
                if op == id!(.){
                    self.code.push(id!(me).into());
                    self.state.push(State::EmitOp(op));
                    self.state.push(State::BeginExpr);
                    return 1
                }
            }
            State::BeginStmt => {
                if id == id!(for){
                    self.state.push(State::EndStmt);
                    self.state.push(State::For);
                    self.state.push(State::ForIdent);
                    return 1
                }
                else if id == id!(let){
                    // we have to have an identifier after let
                    self.state.push(State::EndStmt);
                    self.state.push(State::Let);
                    return 1
                }
                if op == id!(;) || op == id!(,){ // just eat it
                    // we can pop all operator emits
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if tok.is_close_round() || tok.is_close_curly() || tok.is_close_square(){
                    // pop and let the stack handle it
                    return 0
                }
                // lets do an expression statement as fallthrough
                self.state.push(State::EndStmt);
                self.state.push(State::BeginExpr);
                return 0;
            }
            State::EndStmt=>{
                // in a function call we need the 
                if let Some(code) = self.code.last_mut(){
                    if code.is_assign_opcode(){
                        code.set_opcode_is_statement();
                        self.state.push(State::BeginStmt);
                        return 0;
                    }
                    if code.is_let_opcode(){
                        self.state.push(State::BeginStmt);
                        return 0;
                    }
                }
                // otherwise pop to me
                self.code.push(Opcode::POP_TO_ME.into());
                self.state.push(State::BeginStmt);
                return 0
            }
        }
        0
    }
    
    pub fn parse(&mut self, new_code:&str, heap:&mut ScriptHeap){
        self.tok.tokenize(new_code, heap);
        
        // wait for the tokens to be consumed
        while self.index < self.tok.tokens.len() && self.state.len()>0{
            let step = self.handle(heap);
            self.index += step;
        }
        
        println!("MADE CODE: {:?}", self.code);
    }
}
