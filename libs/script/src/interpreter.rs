use makepad_script_derive::*;
use crate::id::*;
use crate::parser::ScriptParser;
use crate::value::*;
use crate::heap::*;

pub struct CallFrame{
    pub scope: ObjectPtr,
    pub stack_base: usize,
    pub return_ip: usize,
}

pub struct ScriptThread{
    stack: Vec<Value>,
    calls: Vec<CallFrame>,
    mes: Vec<ObjectPtr>,
    pub ip: usize
}
    
pub struct ScriptInterpreter{
    pub threads: Vec<ScriptThread>,
    pub heap: ScriptHeap,
    pub global: ObjectPtr,
}

impl ScriptInterpreter{
    pub fn new()->Self{
        let mut heap = ScriptHeap::default();
        Self{
            threads: vec![ScriptThread::new()],
            global: heap.new_dyn_object(),
            heap: heap,
        }
    }
    pub fn run(&mut self, parser: &ScriptParser){
        self.threads[0].run(parser, &mut self.heap, self.global)
    }
}

impl ScriptThread{
    
    pub fn new()->Self{
        Self{
            stack: vec![],
            calls: vec![],
            mes: vec![],
            ip: 0
        }
    }
    
    // lets resolve an id to a Value
    pub fn resolve(&self, id: Id)->Value{
        if id == id!(me){
            if let Some(me) = self.mes.last(){
                return (*me).into()
            }
            return Value::NIL
        }
        if id == id!(scope){
            if let Some(call) = self.calls.last(){
                return (call.scope).into()
            }
            return Value::NIL
        }
        /*
        if id == id!(this){
            if let Some(it) = self.calls.last(){
                return (*it).into()
            }
        }*/
        // look up id on the scope object
        
        Value::NIL
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
    
    pub fn op_assign_field(&mut self, heap:&mut ScriptHeap){
        let field = self.stack.pop().unwrap();
        let value = self.stack.pop().unwrap();
        if let Some(me) = self.mes.last(){
            heap.set_object_value(*me, field, value);
        }
    }
    
    pub fn op_assign(&mut self, heap:&mut ScriptHeap){
        let field = self.stack.pop().unwrap();
        let value = self.stack.pop().unwrap();
        if let Some(me) = self.mes.last(){
            heap.set_object_value(*me, field, value);
        }
    }
    
    pub fn run(&mut self, parser: &ScriptParser, heap:&mut ScriptHeap, global:ObjectPtr){
        let scope = heap.new_dyn_shallow_object();
        
        
        let call = CallFrame{
            scope,
            stack_base: 0,
            return_ip: 0,
        };
        self.mes.push(global);
        self.calls.push(call);
        for i in 0..parser.code.len(){
            self.ip = i;
            self.step(parser, heap);
        }
        self.calls.pop();
        self.mes.pop();
                
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
                    self.op_assign_field(heap);
                }
                Value::OP_BEGIN_BARE=>{ // bare object
                    let it = heap.new_dyn_object();
                    self.mes.push(it);
                }
                Value::OP_END_BARE=>{
                    self.stack.push(self.mes.pop().unwrap().into());
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