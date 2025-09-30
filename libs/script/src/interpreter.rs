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
// in slint you have special variables
// we could say self.

pub struct CallFrame{
    pub scope: ObjectPtr,
    pub stack_base: usize,
    pub return_ip: usize,
}

pub struct ScriptThread{
    stack: Vec<Value>,
    calls: Vec<CallFrame>,
    its: Vec<ObjectPtr>,
    pub ip: usize
}
    
pub struct ScriptInterpreter{
    pub threads: Vec<ScriptThread>,
    pub heap: ScriptHeap,
}

impl ScriptInterpreter{
    pub fn new()->Self{
        Self{
            threads: vec![ScriptThread::new()],
            heap: ScriptHeap::default()
        }
    }
    pub fn run(&mut self, parser: &ScriptParser){
        self.threads[0].run(parser, &mut self.heap)
    }
}

impl ScriptThread{
    
    pub fn new()->Self{
        Self{
            stack: vec![],
            calls: vec![],
            its: vec![],
            ip: 0
        }
    }
    
    pub fn op_add(&mut self, heap:&mut ScriptHeap){
        let op1 = self.stack.pop().unwrap();
        let op2 = self.stack.pop().unwrap();
        let v1 = heap.cast_to_f64(op1);
        let v2 = heap.cast_to_f64(op2);
        self.stack.push(Value::from_f64(v1 + v2));
    }
    
    pub fn op_concat(&mut self, heap:&mut ScriptHeap){
        let op1 = self.stack.pop().unwrap();
        let op2 = self.stack.pop().unwrap();
        let ptr = heap.new_dyn_string_with(|heap, out|{
            heap.cast_to_string(op1, out);
            heap.cast_to_string(op2, out);
        });
        self.stack.push(ptr.into());
    }
    
    pub fn run(&mut self, parser: &ScriptParser, heap:&mut ScriptHeap){
        let scope = heap.new_dyn_object();
        let call = CallFrame{
            scope,
            stack_base: 0,
            return_ip: 0,
        };
        self.calls.push(call);
        
        for i in 0..parser.code.len(){
            self.ip = i;
            self.step(parser, heap);
        }
        
        self.calls.pop();
        
        //self.heap.free_object(scope);
    }
    
    pub fn step(&mut self, parser: &ScriptParser, heap:&mut ScriptHeap){
        let code = parser.code[self.ip];
        if code.is_opcode(){
            match code{
                Value::OP_ADD=>{
                    self.op_add(heap);
                }
                Value::OP_CONCAT=>{
                    self.op_concat(heap);
                }
                Value::OP_ASSIGN=>{
                    // ok we have to assign to something lhs
                    
                }
                Value::OP_ASSIGN_FIELD=>{
                    // alright what do we have on our left
                    // it has to be one ident or else we dont know what to do
                }
                Value::OP_BEGIN_BARE=>{
                    // lets make anew object
                    let it = heap.new_dyn_object();
                    self.its.push(it);
                }
                Value::OP_END_BARE=>{
                    self.stack.push(self.its.pop().unwrap().into());
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