#![allow(dead_code)]
use crate::tokenizer::*;
use crate::id::*;
use makepad_script_derive::*;

#[derive(Default)]
enum State{
    #[default]
    ClosureArgs,
    Stmt,
    Expr,
    Index,
    Inherit(Id),
    CloseRound,
    For,
    ForIdent,
    ForRange,
    If,
}

pub struct ScriptCode{
    pub index: usize,
    pub doc: ScriptDoc,
    pub code: Vec<u64>,
    state: Vec<State>,
}

impl Default for ScriptCode{
    fn default()->Self{
        Self{
            index: 0,
            doc: Default::default(),
            code: Default::default(),
            state: vec![State::Stmt],
        }
    }
}

impl ScriptCode{
    fn emit_code(){
    }
    
    
    fn ct(&self)->ScriptToken{
        if let Some(tok) = self.doc.tokens.get(self.index){
            tok.token.clone()
        }
        else{
            ScriptToken::StreamEnd
        }
    }
    
    fn nt(&self)->ScriptToken{
        if let Some(tok) = self.doc.tokens.get(self.index+1){
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
            State::Index=>{},
            State::For=>{},
            State::ForIdent=>{}
            State::ForRange=>{}
            State::If=>{},
            State::Inherit(_id)=>{
            }
            State::CloseRound=>{}
            State::ClosureArgs=>{
                // we're parsing ident, ident, ident, 
                // now we expect {
            }
            State::Expr=>{
                // (expr)
                if ct.is_open_round(){
                    self.state.pop();
                    self.state.push(State::CloseRound);
                    self.state.push(State::Expr);
                    return 1
                }
                if cop == id!(|){
                    self.state.push(State::ClosureArgs);
                    return 1
                }
                if cid != id!(){ // its an identifier
                    
                }
                // what if we run into +
                
                // (expr) // pop self, push paren, push expr
                // ident terminate: ,;)]}ident
                // ident[]
                // ident.
                // ident+
                // ident-
            }
            State::Stmt => {
                let cid = ct.identifier();
                if cid == id!(for){
                    self.state.push(State::For);
                    self.state.push(State::ForIdent);
                    return 1
                }
                else if cid == id!(if){
                    self.state.push(State::If);
                    self.state.push(State::Expr);
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
                let op = self.nt().operator();
                if op == id!(.){ // property operator
                    
                }
                if op == id!(:) || op == id!(=){ // assignment to identifier
                    
                }
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
    
    fn parse(&mut self, new_code:&str){
        self.doc.parse(new_code);
        // wait for the tokens to be consumed
        while self.index < self.doc.tokens.len(){
            let step = self.handle();
            if step == 0{
                break
            }
            self.index += step;
        }
    }
}
