
//use crate::object::*;
//use crate::parser::*;
//use crate::tokenizer::*;
use crate::parser::ScriptParser;
use crate::value::Value;
use crate::heap::*;

#[derive(Default)]
enum This{
    _Heap(usize),
    _Stack(usize),
    #[default]
    Nil
}

#[derive(Default)]
pub struct ScriptInterpreter{
    stack: Vec<Value>,
    pub heap: ScriptHeap,
    _this: Option<usize>,
    pub ip: usize
}

impl ScriptInterpreter{
    pub fn pop_free(&mut self){
        // lets pop values off the stack and free associated values
    }
    
    pub fn op_add(&mut self){
        let op1 = self.stack.pop().unwrap();
        let op2 = self.stack.pop().unwrap();
        let v1 = self.heap.cast_to_f64(op1);
        let v2 = self.heap.cast_to_f64(op2);
        self.stack.push(Value::from_f64(v1 + v2));
    }
    
    pub fn op_concat(&mut self){
        let op1 = self.stack.pop().unwrap();
        let op2 = self.stack.pop().unwrap();
        let ptr = self.heap.new_dyn_string_with(|heap, out|{
            heap.cast_to_string(op1, out);
            heap.cast_to_string(op2, out);
        });
        self.stack.push(ptr.into());
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
                Value::OP_CONCAT=>{
                    self.op_concat();
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