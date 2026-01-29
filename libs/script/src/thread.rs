use crate::makepad_live_id::*;
use crate::makepad_error_log::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::vm::*;
use crate::function::*;
use crate::trap::*;
use crate::json::*;
use crate::pod::*;
use std::any::Any;
use crate::*;

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
    pub push_nil: bool,
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

#[derive(Debug)]
pub enum ScriptMe{
    Object(ScriptObject),
    Call{sself:Option<ScriptValue>, args:ScriptObject},
    Pod{pod:ScriptPod, offset:ScriptPodOffset},
    Array(ScriptArray),
}

impl Into<ScriptValue> for ScriptMe{
    fn into(self)->ScriptValue{
        match self{
            Self::Object(v)=>v.into(),
            Self::Call{args,..}=>args.into(),
            Self::Pod{pod,..}=>pod.into(),
            Self::Array(v)=>v.into(),
        }
    }
}

#[derive(Default, Debug, Clone, Copy)]
pub struct ScriptThreadId(pub(crate) u32);

impl ScriptThreadId{
    pub fn to_index(&self)->usize{self.0 as usize}
}

#[allow(unused)]
pub struct ScriptThread{
    pub(crate) is_paused: bool,
    pub(crate) stack_limit: usize,
    pub(crate) tries: Vec<TryFrame>,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) scopes: Vec<ScriptObject>,
    pub(crate) stack: Vec<ScriptValue>,
    pub(crate) calls: Vec<CallFrame>,
    pub(crate) mes: Vec<ScriptMe>,
    pub trap: ScriptTrapInner,
    //pub(crate) last_err: ScriptValue,
    pub(crate) json_parser: JsonParserThread,
    pub(crate) thread_id: ScriptThreadId,
}

impl ScriptThread{
    
    pub fn new(thread_id:ScriptThreadId)->Self{
        Self{
            thread_id,
            is_paused: false,
            //last_err: NIL,
            scopes: vec![],
            tries: vec![],
            stack_limit: 1_000_000,
            loops: vec![],
            stack: vec![],
            calls: vec![],
            mes: vec![],
            trap: ScriptTrapInner::default(),
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
    
    pub fn pause(&mut self)->ScriptThreadId{
        self.trap.on.set(Some(ScriptTrapOn::Pause));
        self.is_paused = true;
        self.thread_id
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
            err_stack_underflow!(self.trap)
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
            err_stack_underflow!(self.trap)
        }
    }
    
    pub fn peek_stack_value(&mut self)->ScriptValue{
        if let Some(value) = self.stack.last(){
            return *value
        }
        else{
            err_stack_underflow!(self.trap)
        }
    }
    
    pub fn peek_stack_value_at(&mut self, offset: usize)->ScriptValue{
        let len = self.stack.len();
        if offset < len {
            return self.stack[len - 1 - offset]
        }
        else{
            err_stack_underflow!(self.trap)
        }
    }
    
    pub fn pop_stack_value(&mut self)->ScriptValue{
        if let Some(value) = self.stack.pop(){
            return value
        }
        else{
            err_stack_underflow!(self.trap)
        }
    }
    
    pub fn push_stack_value(&mut self, value:ScriptValue){
        if self.stack.len() > self.stack_limit{
            err_stack_overflow!(self.trap);
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
        heap.scope_value(*self.scopes.last().unwrap(), id.into(), self.trap.pass())
    }
    
    pub fn set_scope_value(&mut self, heap:&mut ScriptHeap, id: LiveId, value:ScriptValue)->ScriptValue{
        heap.set_scope_value(*self.scopes.last().unwrap(), id.into(),value,self.trap.pass())
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
        
        let err = heap.push_all_fn_args(scope, args, self.trap.pass());
        if err.is_err(){
            return err
        }
        
        heap.set_object_deep(scope);
        heap.set_object_storage_auto(scope);
                
        if let Some(fnptr) = heap.parent_as_fn(scope){
            match fnptr{
                ScriptFnPtr::Native(ni)=>{
                    self.trap.in_rust = true;
                    return (*code.native.borrow().functions[ni.index as usize])(&mut ScriptVm{
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
            return err_not_fn!(self.trap)
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
                if self.trap.err.borrow_mut().len()>0{
                    if self.call_has_try(){
                        // pop all errors
                        self.trap.err.borrow_mut().clear();
                        let try_frame = self.tries.pop().unwrap();
                        self.truncate_bases(try_frame.bases, heap);
                        if try_frame.push_nil{
                            self.push_stack_unchecked(NIL)
                        }
                        self.trap.goto(try_frame.start_ip + try_frame.jump);
                        //self.last_err = err.value;
                    }
                    else{
                        while let Some(err) = self.trap.err.borrow_mut().pop(){
                            if let Some(ptr) = err.value.as_err(){
                                if let Some(loc2) = code.ip_to_loc(ptr.ip){
                                    let in_rust = if err.in_rust{"(in rust)"}else{""};
                                    log_with_level(&loc2.file, loc2.line, loc2.col, loc2.line, loc2.col, format!("{}{in_rust} {} ({}:{})", err.value, err.message, err.origin_file, err.origin_line), LogLevel::Error);
                                }
                            }
                        }
                    }
                }
                if let Some(trap) = self.trap.on.take(){
                    match trap{
                        
                        ScriptTrapOn::Pause=>{
                            return NIL
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
        //heap.print(*scope, true);
        //print!("Global:");
        //heap.print(global, true);
        //println!("");                                
        //self.heap.free_object(scope);
        value
    }
}
