#![allow(dead_code)]
use crate::tokenizer::*;
use crate::id::*;
use crate::value::*;
use makepad_script_derive::*;

#[derive(Default, Debug)]
enum State{
    #[default]
    ClosureArgs,
    BeginStmt,
    BeginExpr,
    EndExpr,
    EndStmt,
    EmitOp(Id),
    Bare,
    Call,
    Prototype,
    ArrayIndex,
    EndFrag,
    For,
    ForIdent,
    ForRange,
    If,
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
            id!(==) | id!(!=)  | id!(<) | id!(>) | id!(<=) | id!(>=) => 14,
            id!(&&)  => 15,
            id!(||)  => 16,
            id!(:) | id!(=) | id!(+=)  | id!(-=) | id!(*=) | id!(/=) | id!(%=) => 17,
            id!(&=) | id!(|=)  | id!(^=) | id!(<<=) | id!(>>=) => 18,
            _=>0
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
            id!(==) => Value::OP_EQ,
            id!(!=) => Value::OP_NEQ,
            id!(<) => Value::OP_LT,
            id!(>) => Value::OP_GT,
            id!(<=) => Value::OP_LEQ,
            id!(>=) => Value::OP_GEQ,
            id!(&&) => Value::OP_LOGIC_AND,
            id!(||)  => Value::OP_LOGIC_OR,
            id!(:) | id!(=) => Value::OP_ASSIGN,
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
    
    fn ct(&self)->ScriptToken{
        if let Some(tok) = self.tok.tokens.get(self.index){
            tok.token.clone()
        }
        else{
            ScriptToken::StreamEnd
        }
    }
    
    fn nt(&self)->ScriptToken{
        if let Some(tok) = self.tok.tokens.get(self.index+1){
            tok.token.clone()
        }
        else{
            ScriptToken::StreamEnd
        }
    }
    
    fn push_state(&mut self, state:State){
        self.state.push(state)
    }
    
    fn handle(&mut self)->usize{
        let ct = self.ct();
        let cop = ct.operator();
        let cid = ct.identifier();
        let _nt = self.nt();
        println!("LOOPIN {:?}",self.state);
        match self.state.pop().unwrap(){
            State::For=>{}
            State::ForIdent=>{}
            State::ForRange=>{}
            State::If=>{}
            State::EndFrag=>{
                // we expect a ) here
                self.code.push(Value::OP_END_FRAG);
                if ct.is_close_round(){
                    self.state.push(State::EndExpr);
                    return 1
                }
                else{
                    println!("PARSE ERROR")
                }
            }
            State::ClosureArgs=>{
                if cid != id!(){
                    
                }
                if cop == id!(|){
                    self.state.pop();
                    self.state.push(State::ClosureArgs);
                    return 1
                }
                // we're parsing ident,? ident,? ident,? 
            }
            // alright we parsed a + b * c
            State::EmitOp(cop)=>{
                self.code.push(State::operator_to_opcode(cop));
                return 0
            }
            State::Bare=>{
                self.code.push(Value::OP_END_BARE);
                self.state.push(State::EndExpr);
                if ct.is_close_curly() {
                    return 1
                }
                else {
                    println!("Expected }} not found");
                    return 0
                }
            }
            // emit the create prototype instruction
            State::Prototype=>{
                self.code.push(Value::OP_END_PROTO);
                self.state.push(State::EndExpr);
                if ct.is_close_curly() {
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
                if ct.is_close_round() {
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
                if ct.is_close_square() {
                    return 1
                }
                else {
                    println!("Expected ] not found");
                    return 0
                }
            }
            State::EndExpr=>{
                if State::operator_order(cop) != 0{
                    let next_state = State::EmitOp(cop);
                    if let Some(last) = self.state.pop(){
                        if last.is_heq_prio(next_state){
                            self.state.push(State::EmitOp(cop));
                            self.state.push(State::BeginExpr);
                            self.state.push(last);
                            return 1
                        }
                        else{
                            self.state.push(last);
                        }
                    }
                    self.state.push(State::EmitOp(cop));
                    self.state.push(State::BeginExpr);
                    return 1
                }
                
                if ct.is_open_curly(){
                    self.code.push(Value::OP_BEGIN_PROTO);
                    self.state.push(State::Prototype);
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if ct.is_open_round(){ 
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
                if ct.is_open_square(){
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
                if ct.is_open_curly(){
                    self.code.push(Value::OP_BEGIN_BARE);
                    self.state.push(State::EndExpr);
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if ct.is_open_round(){
                    self.code.push(Value::OP_BEGIN_FRAG);
                    self.state.push(State::EndFrag);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                if let Some(v) = ct.maybe_number(){
                    self.code.push(Value::from_f64(v));
                    self.state.push(State::EndExpr);
                    return 1
                }
                if cid != id!(){
                    self.code.push(Value::from_id(cid));
                    self.state.push(State::EndExpr);
                    return 1
                }
                if let Some(v) = ct.maybe_color(){
                    self.code.push(Value::from_color(v));
                    self.state.pop();
                    return 1
                }
                if let Some(index) = ct.maybe_string(){
                    self.code.push(Value::from_static_string(index));
                    self.state.pop();
                    return 1
                }
                if cop == id!(|){
                    self.state.pop();
                    self.state.push(State::ClosureArgs);
                    return 1
                }
            }
            State::BeginStmt => {
                let cid = ct.identifier();
                if cid == id!(for){
                    self.state.push(State::For);
                    self.state.push(State::ForIdent);
                    return 1
                }
                else if cid == id!(if){
                    self.state.push(State::If);
                    self.state.push(State::BeginExpr);
                    return 1
                }
                if cop == id!(;) || cop == id!(,){ // just eat it
                    // we can pop all operator emits
                    self.state.push(State::BeginStmt);
                    return 1
                }
                if ct.is_close_round() || ct.is_close_curly() || ct.is_close_square(){
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
    
    pub fn parse(&mut self, new_code:&str){
        self.tok.tokenize(new_code);
        
        // wait for the tokens to be consumed
        while self.index < self.tok.tokens.len() && self.state.len()>0{
            println!("AT TOKEN {:?}", self.tok.tokens[self.index].token);
            let step = self.handle();
            self.index += step;
        }
        
        println!("MADE CODE: {:?}", self.code);
    }
}
