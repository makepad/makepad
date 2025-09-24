#![allow(dead_code)]
use crate::tokenizer::*;
use crate::id::*;
use crate::value::*;
use makepad_script_derive::*;

#[derive(Default)]
enum State{
    #[default]
    ClosureArgs,
    Statement,
    Expression,
    ExprOp,
    Operation(Id),
    Index,
    Inherit(Id),
    CloseRound,
    For,
    ForIdent,
    ForRange,
    If,
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
            state: vec![State::Statement],
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
        match self.state.last().unwrap(){
            State::Index=>{
            }
            State::For=>{}
            State::ForIdent=>{}
            State::ForRange=>{}
            State::If=>{}
            State::Inherit(_id)=>{
            }
            State::CloseRound=>{}
            State::ClosureArgs=>{
                // we're parsing ident, ident, ident, 
            }
            State::Operation(_op)=>{
                
            }
            State::ExprOp=>{ // parse potential expression operations
                // if we find an identifier, we pop and terminate
                if cop == id!(+){
                    self.state.pop();
                    self.state.push(State::Expression);
                    self.state.push(State::Operation(cop));
                    return 1
                }
                if cid != id!(){ // its an identifier
                    self.state.pop();
                    return 0
                }
                if ct.is_close_round(){
                    self.state.pop();
                    return 0
                }
            }
            State::Expression=>{
                // (expr)
                if ct.is_open_round(){
                    self.state.pop();
                    self.state.push(State::CloseRound);
                    self.state.push(State::Expression);
                    return 1
                }
                if let Some(v) = ct.maybe_number(){
                    // just return the number
                    self.code.push(Value::from_f64(v));
                    self.state.pop();
                    self.state.push(State::ExprOp);
                    return 1
                }
                if let Some(v) = ct.maybe_color(){
                    self.code.push(Value::from_color(v));
                    return 1
                }
                if let Some(index) = ct.maybe_string(){
                    self.code.push(Value::from_static_string(index));
                    return 1
                }
                if cop == id!(|){
                    self.state.push(State::ClosureArgs);
                    return 1
                }
                if cid != id!(){ // its an identifier
                    self.code.push(Value::from_id(cid));
                    return 1
                }
            }
            State::Statement => {
                let cid = ct.identifier();
                if cid == id!(for){
                    self.state.push(State::For);
                    self.state.push(State::ForIdent);
                    return 1
                }
                else if cid == id!(if){
                    self.state.push(State::If);
                    self.state.push(State::Expression);
                    return 1
                }
                // otherwise we're going to parse an expression
                /*if nt.is_open_curly(){
                    // emit code where we inherit from ident
                    // upnext is a new object
                    self.state.push(State::Object);
                    return 2;
                }
                if nt.is_open_square(){
                    self.state.push(State::Index);
                    return 1
                }*/
                // end of object
                if self.ct().is_close_curly(){
                    // pop the state
                    self.state.pop();
                    return 1
                }
            }
        }
        0
    }
    
    pub fn parse(&mut self, new_code:&str){
        self.tok.tokenize(new_code);
        
        // wait for the tokens to be consumed
        while self.index < self.tok.tokens.len(){
            let step = self.handle();
            if step == 0{
                break
            }
            self.index += step;
        }
    }
}
