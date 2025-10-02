#![allow(dead_code)]
use crate::tokenizer::*;
use crate::id::*;
use crate::heap::*;
use crate::value::*;
use makepad_script_derive::*;

#[derive(Default, Debug)]
enum State{
    #[default]
    FnArgs,
    FnBody,
    BeginStmt,
    BeginExpr,
    EndExpr,
    EndStmt,
    EmitOp(Id),
    EmitFieldAssign(Id),
    EmitIndexAssign(Id),
    EndBare,
    Call,
    EndProto,
    ArrayIndex,
    EndFrag,
    For,
    ForIdent,
    ForRange,
    If,
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
            id!(=) => Value::OP_ASSIGN_FIELD,
            id!(+=) => Value::OP_ASSIGN_FIELD_ADD,
            id!(-=) => Value::OP_ASSIGN_FIELD_SUB,
            id!(*=) => Value::OP_ASSIGN_FIELD_MUL,
            id!(/=) => Value::OP_ASSIGN_FIELD_DIV,
            id!(%=) => Value::OP_ASSIGN_FIELD_MOD, 
            id!(&=) => Value::OP_ASSIGN_FIELD_AND,
            id!(|=) => Value::OP_ASSIGN_FIELD_OR,
            id!(^=) => Value::OP_ASSIGN_FIELD_XOR,
            id!(<<=) => Value::OP_ASSIGN_FIELD_SHL,
            id!(>>=) => Value::OP_ASSIGN_FIELD_SHR,
            id!(?=) => Value::OP_ASSIGN_FIELD_IFNIL,
            _=>Value::OP_NOP,
        }
    }
    
    fn operator_to_index_assign(op:Id)->Value{
        match op{
            id!(=) => Value::OP_ASSIGN_INDEX,
            id!(+=) => Value::OP_ASSIGN_INDEX_ADD,
            id!(-=) => Value::OP_ASSIGN_INDEX_SUB,
            id!(*=) => Value::OP_ASSIGN_INDEX_MUL,
            id!(/=) => Value::OP_ASSIGN_INDEX_DIV,
            id!(%=) => Value::OP_ASSIGN_INDEX_MOD, 
            id!(&=) => Value::OP_ASSIGN_INDEX_AND,
            id!(|=) => Value::OP_ASSIGN_INDEX_OR,
            id!(^=) => Value::OP_ASSIGN_INDEX_XOR,
            id!(<<=) => Value::OP_ASSIGN_INDEX_SHL,
            id!(>>=) => Value::OP_ASSIGN_INDEX_SHR,
            id!(?=) => Value::OP_ASSIGN_INDEX_IFNIL,
            _=>Value::OP_NOP,
        }
    }
    fn operator_to_opcode(op:Id)->Value{
        match op{
            id!(*) => Value::OP_MUL,
            id!(/) => Value::OP_DIV,
            id!(%) => Value::OP_MOD,
            id!(+) => Value::OP_ADD,
            id!(-) => Value::OP_SUB,
            id!(<<) => Value::OP_SHL,
            id!(>>) => Value::OP_SHR,
            id!(&)  => Value::OP_AND,
            id!(^) => Value::OP_XOR,
            id!(|)  => Value::OP_OR,
            id!(++)  => Value::OP_CONCAT,
            id!(==) => Value::OP_EQ,
            id!(!=) => Value::OP_NEQ,
            id!(<) => Value::OP_LT,
            id!(>) => Value::OP_GT,
            id!(<=) => Value::OP_LEQ,
            id!(>=) => Value::OP_GEQ,
            id!(&&) => Value::OP_LOGIC_AND,
            id!(||)  => Value::OP_LOGIC_OR,
            id!(:) => Value::OP_ASSIGN_ME,
            id!(=) => Value::OP_ASSIGN,
            id!(+=) => Value::OP_ASSIGN_ADD,
            id!(-=) => Value::OP_ASSIGN_SUB,
            id!(*=) => Value::OP_ASSIGN_MUL,
            id!(/=) => Value::OP_ASSIGN_DIV,
            id!(%=) => Value::OP_ASSIGN_MOD,
            id!(&=) => Value::OP_ASSIGN_AND,
            id!(|=) => Value::OP_ASSIGN_OR,
            id!(^=) => Value::OP_ASSIGN_XOR,
            id!(<<=) => Value::OP_ASSIGN_SHL,
            id!(>>=)  => Value::OP_ASSIGN_SHR,
            id!(?=)  => Value::OP_ASSIGN_IFNIL,
            id!(.)  => Value::OP_FIELD,
            _=> Value::OP_NOP,
        }
    }
    
    fn is_heq_prio(&self, other:State)->bool{
        match self{
            Self::EmitOp(op1)=>{
                match other{
                    Self::EmitOp(op2)=>{
                        //if Self::is_assign_operator(*op1) && Self::is_assign_operator(op2){
                        //    return false
                        //}
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
    
    fn handle(&mut self)->usize{
        let tok = if let Some(tok) = self.tok.tokens.get(self.index){
            tok.token.clone()
        }
        else{
            ScriptToken::StreamEnd
        };
        let op = tok.operator();
        let id = tok.identifier();
        match self.state.pop().unwrap(){
            State::For=>{}
            State::ForIdent=>{}
            State::ForRange=>{}
            State::If=>{}
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
                    self.code.push(Value::OP_LET_DYN_NIL);
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
                    self.code.push(Value::OP_LET_TYPED_NIL);
                }
            }
            State::EmitLetDyn=>{
                self.code.push(Value::OP_LET_DYN);
            }
            State::EmitLetTyped=>{
                self.code.push(Value::OP_LET_TYPED);
            }
            State::EndFrag=>{
                // we expect a ) here
                self.code.push(Value::OP_END_FRAG);
                if tok.is_close_round(){
                    self.state.push(State::EndExpr);
                    return 1
                }
                else{
                    println!("PARSE ERROR")
                }
            }
            State::FnBody=>{
                
            }
            State::FnArgs=>{
                if id != id!(){
                    
                }
                if op == id!(|){
                    self.state.pop();
                    self.state.push(State::FnBody);
                    return 1
                }
                // we're parsing ident,? ident,? ident,? 
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
            State::EndBare=>{
                self.code.push(Value::OP_END_BARE);
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
                self.code.push(Value::OP_END_PROTO);
                self.state.push(State::EndExpr);
                if tok.is_close_curly() {
                    return 1
                }
                else {
                    println!("Expected }} not found");
                    return 0
                }
            }
            State::Call=>{
                // expect )
                self.code.push(Value::OP_END_CALL);
                self.state.push(State::EndExpr);
                if tok.is_close_round() {
                    return 1
                }
                else {
                    println!("Expected ) not found");
                    return 0
                }
            }
            State::ArrayIndex=>{
                self.code.push(Value::OP_ARRAY_INDEX);
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
                    if let Some(&Value::OP_ARRAY_INDEX) = self.code.last(){
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
                                    if pair[0] == Value::OP_FIELD && pair[1].is_id(){
                                        pair[0] = Value::OP_PROTO_FIELD
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
                
                if tok.is_open_curly(){
                    self.code.push(Value::OP_BEGIN_PROTO);
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
                    self.code.push(Value::OP_BEGIN_CALL);
                    self.state.push(State::Call);
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
            State::BeginExpr=>{
                if tok.is_open_curly(){
                    self.code.push(Value::OP_BEGIN_BARE);
                    self.state.push(State::EndBare);
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if tok.is_open_round(){
                    self.code.push(Value::OP_BEGIN_FRAG);
                    self.state.push(State::EndFrag);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                if let Some(v) = tok.maybe_number(){
                    self.code.push(Value::from_f64(v));
                    self.state.push(State::EndExpr);
                    return 1
                }
                if id != id!(){
                    self.code.push(Value::from_id(id));
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = tok.maybe_color(){
                    self.code.push(Value::from_color(v));
                    self.state.pop();
                    return 1
                }
                if let Some(ptr) = tok.maybe_string(){
                    self.code.push(Value::from_string(ptr));
                    self.state.pop();
                    return 1
                }
                if op == id!(|){
                    self.state.pop();
                    self.code.push(Value::OP_BEGIN_FN_ARGS);
                    self.state.push(State::FnArgs);
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
                    self.state.push(State::For);
                    self.state.push(State::ForIdent);
                    return 1
                }
                else if id == id!(if){
                    self.state.push(State::If);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                else if id == id!(let){
                    // we have to have an identifier after let
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
                // just start a new statement
                 
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
            let step = self.handle();
            self.index += step;
        }
        
        println!("MADE CODE: {:?}", self.code);
    }
}
