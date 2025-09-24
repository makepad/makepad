
//use crate::object::*;
//use crate::parser::*;
//use crate::tokenizer::*;
use crate::parser::ScriptParser;
use crate::object::*;
use crate::value::Value;

#[derive(Default)]
struct Arenas{
    _string: Vec<String>,
    _object: Vec<Object>,
    value: Vec<Value>,
}

#[derive(Default)]
enum This{
    _Heap(usize),
    _Stack(usize),
    #[default]
    Nil
}

#[derive(Default)]
pub struct ScriptInterpreter{
    stack: Arenas,
    _heap: Arenas,
    _this: This,
    pub ip: usize
}

impl ScriptInterpreter{
    pub fn op_add(&mut self){
        let op1 = self.stack.value.pop().unwrap();
        let op2 = self.stack.value.pop().unwrap();
        
        if let Some(v1) = op1.as_f64(){
            if op2.is_string(){ // string concat
                
            }
            // otherwise hardcast
            let v2 = op2.to_f64();
            self.stack.value.push(Value::from_f64(v1 + v2));
            return
        }
    }
    
    pub fn run(&mut self, parser: &ScriptParser){
        for i in 0..parser.code.len(){
            self.ip = i;
            self.step(parser);
        }
    }
    
    pub fn step(&mut self, parser: &ScriptParser){
        let code = parser.code[self.ip];
        if code.is_opcode(){
            match code{
                Value::OP_PROP=>{
                }
                Value::OP_ADD=>{
                    self.op_add();
                }
                _=>{
                    // unknown instruction
                }
            }
        }
        else{ // its a direct value-to-stack?
            self.stack.value.push(code);
        }
    }
}