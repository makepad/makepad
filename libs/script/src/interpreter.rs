use crate::parser::ScriptParser;
use crate::value::*;
use crate::heap::*;

#[derive(Default)]
enum This{
    _Heap(usize),
    _Stack(usize),
    #[default]
    Nil
}

// ok how do function args work
// we make a new 'args' object prototypically inherited
// from the closure scope object
// every time we create a closure we also store a reference to the scope
//

pub struct CallFrame{
    pub scope: ObjectPtr,
    pub stack_base: usize,
    pub return_ip: usize,
}

pub struct ScriptInterpreter{
    stack: Vec<Value>,
    calls: Vec<CallFrame>,
    pub heap: ScriptHeap,
    pub ip: usize
}

impl ScriptInterpreter{
    pub fn global_object(&self)->ObjectPtr{
        self.stack[0].as_object().unwrap()
    }
    
    pub fn new()->Self{
        let mut heap = ScriptHeap::default();
        Self{
            stack: vec![heap.new_dyn_object().into()],
            calls: vec![],
            heap,
            ip: 0
        }
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
        let scope = self.heap.new_dyn_object();
        let call = CallFrame{
            scope,
            stack_base: 0,
            return_ip: 0,
        };
        self.calls.push(call);
        
        for i in 0..parser.code.len(){
            self.ip = i;
            self.step(parser);
        }
        self.calls.pop();
        //self.heap.free_object(scope);
    }
    
    pub fn step(&mut self, parser: &ScriptParser){
        let code = parser.code[self.ip];
        if code.is_opcode(){
            match code{
                Value::OP_ADD=>{
                    self.op_add();
                }
                Value::OP_CONCAT=>{
                    self.op_concat();
                }
                Value::OP_ASSIGN=>{
                   // self.op_assign();
                }
                Value::OP_BEGIN_BARE=>{
                    // lets make anew object
                    let _obj = self.heap.new_dyn_object();
                    // lets store our constructor function including a scope clone
                    
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