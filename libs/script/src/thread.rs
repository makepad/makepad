use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::vm::*;
use crate::object::*;
use crate::trap::*;
use crate::json::*;
use std::any::Any;

#[derive(Debug, Default)]
pub struct StackBases{
    pub loops: usize,
    pub tries: usize,
    pub stack: usize,
    pub scope: usize,
    pub mes: usize,
}

#[derive(Debug)]
pub struct LoopValues{
    pub value_id: LiveId,
    pub key_id: Option<LiveId>,
    pub index_id: Option<LiveId>,
    pub source: ScriptValue,
    pub index: f64,
}

#[derive(Debug)]
pub struct TryFrame{
    pub start_ip: u32,
    pub jump: u32,
    pub bases: StackBases,
}

#[derive(Debug)]
pub struct LoopFrame{
    pub values: Option<LoopValues>,
    pub start_ip: u32,
    pub jump: u32,
    pub bases: StackBases,
}

pub struct CallFrame{
    pub bases: StackBases,
    pub args: OpcodeArgs,
    pub return_ip: Option<ScriptIp>,
}

pub enum ScriptMe{
    Object(ScriptObject),
    Call{this:Option<ScriptValue>, args:ScriptObject},
    Array(ScriptArray),
}

impl Into<ScriptValue> for ScriptMe{
    fn into(self)->ScriptValue{
        match self{
            Self::Object(v)=>v.into(),
            Self::Call{args,..}=>args.into(),
            Self::Array(v)=>v.into(),
        }
    }
}

pub struct ScriptThreadId(pub usize);

#[allow(unused)]
pub struct ScriptThread{
    pub(crate) stack_limit: usize,
    pub(crate) tries: Vec<TryFrame>,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) scopes: Vec<ScriptObject>,
    pub(crate) stack: Vec<ScriptValue>,
    pub(crate) calls: Vec<CallFrame>,
    pub(crate) mes: Vec<ScriptMe>,
    pub trap: ScriptTrap,
    pub(crate) last_err: ScriptValue,
    pub(crate) json_parser: JsonParserThread
}

impl ScriptThread{
    
    pub fn new()->Self{
        Self{
            last_err: NIL,
            scopes: vec![],
            tries: vec![],
            stack_limit: 1_000_000,
            loops: vec![],
            stack: vec![],
            calls: vec![],
            mes: vec![],
            trap: ScriptTrap::default(),
            json_parser: Default::default(),
        }
    }
    
    pub fn new_bases(&self)->StackBases{
        StackBases{
            tries: self.tries.len(),
            loops: self.loops.len(),
            stack: self.stack.len(),
            scope: self.scopes.len(),
            mes: self.mes.len()
        }
    }
    
    pub fn truncate_bases(&mut self, bases:StackBases, heap:&mut ScriptHeap){
        self.tries.truncate(bases.tries);
        self.loops.truncate(bases.loops);
        self.stack.truncate(bases.stack);
        self.free_unreffed_scopes(&bases, heap);
        self.mes.truncate(bases.mes);
    }
    
    pub fn free_unreffed_scopes(&mut self, bases:&StackBases, heap:&mut ScriptHeap){
        while self.scopes.len() > bases.scope{
            heap.free_object_if_unreffed(self.scopes.pop().unwrap());
        }
    }
        
    pub fn pop_to_me(&mut self, heap:&mut ScriptHeap){
                
        let value = self.pop_stack_value();
        if self.call_has_me(){
                        
            let (key, value) = if let Some(id) = value.as_id(){
                if value.is_escaped_id(){ (NIL, value) }
                else{(value, self.scope_value(heap, id))}
            }else{(NIL,value)};
                        
            match self.mes.last().unwrap(){
                ScriptMe::Call{args,..}=>{
                    heap.unnamed_fn_arg(*args, value, &self.trap);
                }
                ScriptMe::Object(obj)=>{
                    if !value.is_nil() && !value.is_err(){
                        heap.vec_push(*obj, key, value, &self.trap);       
                    }
                }
                ScriptMe::Array(arr)=>{
                    heap.array_push(*arr, value, &self.trap)
                }
            }
        }
    }
    
    pub fn pop_stack_resolved(&mut self, heap:&ScriptHeap)->ScriptValue{
        if let Some(val) = self.stack.pop(){
            if let Some(id) = val.as_id(){
                if val.is_escaped_id(){
                    return val
                }
                return self.scope_value(heap, id)
            }
            return val    
        }
        else{
            self.trap.err_stack_underflow()
        }
    }
    
    pub fn peek_stack_resolved(&mut self, heap:&ScriptHeap)->ScriptValue{
        if let Some(val) = self.stack.last(){
            if let Some(id) = val.as_id(){
                if val.is_escaped_id(){
                    return *val
                }
                return self.scope_value(heap, id)
            }
            return *val    
        }
        else{
            self.trap.err_stack_underflow()
        }
    }
    
    pub fn peek_stack_value(&mut self)->ScriptValue{
        if let Some(value) = self.stack.last(){
            return *value
        }
        else{
            self.trap.err_stack_underflow()
        }
    }
    
    pub fn pop_stack_value(&mut self)->ScriptValue{
        if let Some(value) = self.stack.pop(){
            return value
        }
        else{
            self.trap.err_stack_underflow()
        }
    }
    
    pub fn push_stack_value(&mut self, value:ScriptValue){
        if self.stack.len() > self.stack_limit{
            self.trap.err_stack_overflow();
        }
        else{
            self.stack.push(value);
        }
    }
    
    pub fn push_stack_unchecked(&mut self, value:ScriptValue){
        self.stack.push(value);
    }
    
    pub fn call_has_me(&self)->bool{
        self.mes.len() > self.calls.last().unwrap().bases.mes
    }
    
    pub fn call_has_try(&self)->bool{
        self.tries.len() > self.calls.last().unwrap().bases.tries
    }
    
    // lets resolve an id to a ScriptValue
    pub fn scope_value(&mut  self, heap:&ScriptHeap, id: LiveId)->ScriptValue{
        heap.scope_value(*self.scopes.last().unwrap(), id.into(),&self.trap)
    }
    
    pub fn set_scope_value(&mut self, heap:&mut ScriptHeap, id: LiveId, value:ScriptValue)->ScriptValue{
        heap.set_scope_value(*self.scopes.last().unwrap(), id.into(),value,&self.trap)
    }
    
    pub fn def_scope_value(&mut self, heap:&mut ScriptHeap, id: LiveId, value:ScriptValue){
        // alright if we are shadowing a value, we need to make a new scope
        if let Some(new_scope) = heap.def_scope_value(*self.scopes.last().unwrap(), id, value){
            self.scopes.push(new_scope);
        }
    }
    
    pub fn call(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any, fnobj:ScriptValue, args:&[ScriptValue])->ScriptValue{
        let scope = heap.new_with_proto(fnobj);
        
        heap.clear_object_deep(scope);
        if fnobj.is_err(){
            return fnobj
        }
        
        let err = heap.push_all_fn_args(scope, args, &self.trap);
        if err.is_err(){
            return err
        }
        
        heap.set_object_deep(scope);
        heap.set_object_storage_type(scope, ScriptObjectStorageType::AUTO);
                
        if let Some(fnptr) = heap.parent_as_fn(scope){
            match fnptr{
                ScriptFnPtr::Native(ni)=>{
                    self.trap.in_rust = true;
                    return (*code.native.borrow().fn_table[ni.index as usize].fn_ptr)(&mut ScriptVm{
                        host,
                        heap,
                        thread:self,
                        code
                    }, scope);
                }
                ScriptFnPtr::Script(sip)=>{
                    self.trap.in_rust = false;
                    let call = CallFrame{
                        bases: self.new_bases(),
                        args: OpcodeArgs::default(),
                        return_ip: None
                    };
                    self.scopes.push(scope);
                    self.calls.push(call);
                    self.trap.ip = sip;
                    self.trap.in_rust = true;
                    return self.run_core(heap, code, host);
                }
            }
        }
        else{
            return self.trap.err_not_fn()
        }
    }
    
    pub fn run_core(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any)->ScriptValue{
        self.trap.in_rust = false;
        let bodies = code.bodies.borrow();
        let mut body = &bodies[self.trap.ip.body as usize];
        while (self.trap.ip.index as usize) < body.parser.opcodes.len(){
            let opcode = body.parser.opcodes[self.trap.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(opcode, args, heap, code, host);
                // if exception tracing
                if let Some(trap) = self.trap.on.take(){
                    match trap{
                        ScriptTrapOn::Error{value, in_rust}=>{
                            // check if we have a try clause
                            if self.call_has_try(){
                                let try_frame = self.tries.pop().unwrap();
                                self.truncate_bases(try_frame.bases, heap);
                                self.trap.goto(try_frame.start_ip + try_frame.jump);
                                self.last_err = value;
                            }
                            else{
                                if let Some(ptr) = value.as_err(){
                                    if let Some(loc2) = code.ip_to_loc(ptr.ip){
                                        if in_rust{
                                            println!("{}(in rust) {}", value, loc2);
                                        }
                                        else{
                                            println!("{} {}", value, loc2);
                                        }
                                    }
                                }
                            }
                        }
                        ScriptTrapOn::Return(value)=>{
                            return value
                        }
                    }
                }
            }
            else{ // its a direct value-to-stack?
                self.push_stack_value(opcode);
                self.trap.goto_next();
            }
            body = &bodies[self.trap.ip.body as usize];
        }
        NIL
    }
    
    pub fn run_root(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any, body_id: u16)->ScriptValue{
        
        self.calls.push(CallFrame{
            bases: StackBases{
                tries: 0,
                loops: 0,
                stack: 0,
                scope: 0,
                mes: 0,
            },
            args: Default::default(),
            return_ip: None,
        });
        
        let bodies = code.bodies.borrow();
        
        self.scopes.push(bodies[body_id as usize].scope);
        self.mes.push(ScriptMe::Object(bodies[body_id as usize].me));
        
        self.trap.ip.body = body_id;
        self.trap.ip.index = 0;
        //let mut profile: std::collections::BTreeMap<Opcode, f64> = Default::default();
        
        // the main interpreter loop
        let value = self.run_core(heap, code, host);
        //println!("{:?}", profile);
        // lets have a look at our scope
        let _call = self.calls.last();
        let _scope = self.scopes.last();
        //opcodes.sort_by(|a,b| a.count.cmp(&b.count));
        //println!("{:?}", opcodes);
        println!("Allocated objects:{:?}", heap.objects_len());
        //heap.print(*scope, true);
        //print!("Global:");
        //heap.print(global, true);
        //println!("");                                
        //self.heap.free_object(scope);
        value
    }
}
