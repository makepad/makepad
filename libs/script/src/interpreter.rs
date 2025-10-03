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

macro_rules! f64_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op1 = $obj.pop_stack_resolved($heap);
        let op2 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1);
        let v2 = $heap.cast_to_f64(op2);
        $obj.stack.push(Value::from_f64(v1 $op v2));
    }}
}

macro_rules! fu64_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op1 = $obj.pop_stack_resolved($heap);
        let op2 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1) as u64;
        let v2 = $heap.cast_to_f64(op2) as u64;
        $obj.stack.push(Value::from_f64((v1 $op v2) as f64));
    }}
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
    
    pub fn pop_stack_resolved(&mut self, heap:&ScriptHeap)->Value{
        let val = self.stack.pop().unwrap();
        if let Some(id) = val.as_id(){
            return self.resolve(id, heap)
        }
        val    
    }
    
    // lets resolve an id to a Value
    pub fn resolve(&self, id: Id, heap:&ScriptHeap)->Value{
        if id == id!(me){
            if let Some(me) = self.mes.last(){
                return (*me).into()
            }
            return Value::NIL
        }
        if let Some(call) = self.calls.last(){
            if id == id!(scope){
                return (call.scope).into()
            }
            return heap.object_value(call.scope, id.into())
        }
        Value::NIL
    }
    
    pub fn opcode(&mut self,index: u64, _parser: &ScriptParser, heap:&mut ScriptHeap){
        match index{
            Value::OI_POP_TO_ME=>{
                let value = self.stack.pop().unwrap();
                if !value.is_nil(){
                    let (key, value) = if let Some(id) = value.as_id(){
                        (value, self.resolve(id, heap))
                    }
                    else{
                        (Value::NIL, value)
                    };
                    if !value.is_nil(){
                        if let Some(me) = self.mes.last(){
                            heap.push_object_value(*me, key, value);
                        }
                    }
                }
            }
            Value::OI_NOT=>{
                let v = heap.cast_to_f64(self.pop_stack_resolved(heap)) as u64;
                self.stack.push(Value::from_f64((!v) as f64));
            },
            Value::OI_NEG=>{
                let v = heap.cast_to_f64(self.pop_stack_resolved(heap));
                self.stack.push(Value::from_f64(-v));
            },
            Value::OI_MUL=>f64_op_impl!(self, heap, *),
            Value::OI_DIV=>f64_op_impl!(self, heap, /),
            Value::OI_MOD=>f64_op_impl!(self, heap, %),
            Value::OI_ADD=>f64_op_impl!(self, heap, +),
            Value::OI_SUB=>f64_op_impl!(self, heap, -),
            Value::OI_SHL=>fu64_op_impl!(self, heap, >>),
            Value::OI_SHR=>fu64_op_impl!(self, heap, <<),
            Value::OI_AND=>fu64_op_impl!(self, heap,&),
            Value::OI_OR=>fu64_op_impl!(self, heap, |),
            Value::OI_XOR=>fu64_op_impl!(self, heap, ^),

            Value::OI_CONCAT=>{
                let op1 = self.pop_stack_resolved(heap);
                let op2 = self.pop_stack_resolved(heap);
                let ptr = heap.new_dyn_string_with(|heap, out|{
                    heap.cast_to_string(op1, out);
                    heap.cast_to_string(op2, out);
                });
                self.stack.push(ptr.into());
            }
            Value::OI_ASSIGN_ME=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.stack.pop().unwrap();
                if let Some(me) = self.mes.last(){
                    heap.set_object_value(*me, field, value);
                }
                self.stack.push(Value::NIL);
            }
            Value::OI_ASSIGN_FIELD=>{
                
            }
            Value::OI_BEGIN_PROTO=>{
                let proto = self.pop_stack_resolved(heap);
                let me = heap.new_dyn_object_with_proto(proto);
                self.mes.push(me);
            }
            Value::OI_END_PROTO=>{
                self.stack.push(self.mes.pop().unwrap().into());
            }
            Value::OI_BEGIN_BARE=>{ // bare object
                let me = heap.new_dyn_object();
                self.mes.push(me);
            }
            Value::OI_END_BARE=>{
                self.stack.push(self.mes.pop().unwrap().into());
            }
            Value::OI_LET_DYN=>{
                let value = self.pop_stack_resolved(heap);
                let id = self.stack.pop().unwrap().as_id().unwrap();
                let call = self.calls.last_mut().unwrap();
                heap.push_object_value(call.scope, id.into(), value);
                self.stack.push(Value::NIL);
            }
            Value::OI_LET_DYN_NIL=>{
                let id = self.stack.pop().unwrap().as_id().unwrap();
                let call = self.calls.last_mut().unwrap();
                heap.push_object_value(call.scope, id.into(), Value::NIL);
                self.stack.push(Value::NIL);
            }
            Value::OI_LET_TYPED=>{
                let value = self.pop_stack_resolved(heap);
                let _ty = self.stack.pop();
                let id = self.stack.pop().unwrap().as_id().unwrap();
                let call = self.calls.last_mut().unwrap();
                heap.push_object_value(call.scope, id.into(), value);
                self.stack.push(Value::NIL);
            }
            Value::OI_LET_TYPED_NIL=>{
                let _ty = self.stack.pop();
                let id = self.stack.pop().unwrap().as_id().unwrap();
                let call = self.calls.last_mut().unwrap();
                heap.push_object_value(call.scope, id.into(), Value::NIL);
                self.stack.push(Value::NIL);
            }
            _=>{
                // unknown instruction
            }
        }
    }
    
    pub fn step(&mut self, parser: &ScriptParser, heap:&mut ScriptHeap){ 
        let code = parser.code[self.ip];
        if let Some(index) = code.as_opcode_index(){
            self.opcode(index, parser, heap);   
        }
        else{ // its a direct value-to-stack?
            self.stack.push(code);
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
        // lets have a look at our scope
        let call = self.calls.pop().unwrap();
        print!("Scope:");
        heap.print_object(call.scope);
        self.mes.pop();
        print!("\nGlobal:");
        heap.print_object(global);
        println!("");                                
        //self.heap.free_object(scope);
    }
}