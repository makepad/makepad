
//use crate::object::*;
//use crate::parser::*;
//use crate::tokenizer::*;
use crate::parser::ScriptParser;
use crate::value::Value;
use crate::object::*;

#[derive(Default)]
enum This{
    _Heap(usize),
    _Stack(usize),
    #[default]
    Nil
}

#[derive(Default)]
pub struct ScriptInterpreter{
    temp: String,
    stack: Vec<Value>,
    pub heap: ScriptHeap,
    _this: Option<usize>,
    pub ip: usize
}

impl ScriptInterpreter{
    pub fn tokenizer_strings(&mut self)->&mut Vec<HeapString>{
        &mut self.heap.zones[0].strings
    }
    
    pub fn pop_free(&mut self){
        // lets pop values off the stack and free associated values
    }
    
    pub fn op_add(&mut self){
        // ok we're popping 2 values off the stack
        // if these things are 'stack' strings/objects we need to pop them off as well
        // we want this things value tho but its invalid now because of the stackpop
        let op1 = self.stack.pop().unwrap();
        let op2 = self.stack.pop().unwrap();
        
        if let Some(v1) = op1.as_f64(){
            if op2.is_string(){ // string concat
                // get str value
                let _op2s = self.heap.value_string(op2);
                
                self.temp.clear();
                //write!(self.temp,"{} {}")
            }
            // otherwise hardcast
            let v2 = op2.to_f64();
            self.stack.push(Value::from_f64(v1 + v2));
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
            self.stack.push(code);
        }
    }
}