use crate::makepad_id::id::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::vm::*;
use crate::object::*;
use std::any::Any;

#[derive(Debug, Default)]
pub struct StackBases{
    pub loops: usize,
    pub stack: usize,
    pub scope: usize,
    pub mes: usize,
}

#[derive(Debug)]
pub struct LoopValues{
    pub value_id: Id,
    pub key_id: Option<Id>,
    pub index_id: Option<Id>,
    pub source: Value,
    pub index: f64,
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

pub struct ScriptMe{
    pub(crate) ty: u32,
    pub(crate) object: ObjectPtr,
}
 
impl ScriptMe{
    pub const ARRAY: u32 = 1;
    pub const CALL: u32 = 2;
    pub const OBJ: u32 = 3;
    pub fn object(object:ObjectPtr)->Self{Self{object, ty: Self::OBJ}}
    pub fn array(object:ObjectPtr)->Self{Self{object, ty: Self::ARRAY}}
    pub fn call(object:ObjectPtr)->Self{Self{object, ty: Self::CALL}}
}

pub struct ScriptThreadId(pub usize);

pub enum ScriptTrap{
    Error(Value),
    Return(Value),
}

pub struct ScriptThread{
    pub(crate) stack_limit: usize,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) scopes: Vec<ObjectPtr>,
    pub(crate) stack: Vec<Value>,
    pub(crate) calls: Vec<CallFrame>,
    pub(crate) mes: Vec<ScriptMe>,
    pub(crate) trap: Option<ScriptTrap>,
    pub ip: ScriptIp,
}

pub enum ScriptHook{
    SysCall(usize),
    RustCall
}

impl ScriptThread{
    
    pub fn new()->Self{
        Self{
            scopes: vec![],
            stack_limit: 1_000_000,
            loops: vec![],
            stack: vec![],
            calls: vec![],
            mes: vec![],
            ip: ScriptIp::default(),
            trap: None,
        }
    }
    
    pub fn new_bases(&self)->StackBases{
        StackBases{
            loops: self.loops.len(),
            stack: self.stack.len(),
            scope: self.scopes.len(),
            mes: self.mes.len()
        }
    }
    
    pub fn truncate_bases(&mut self, bases:StackBases, heap:&mut ScriptHeap){
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
    
    pub fn pop_stack_resolved(&mut self, heap:&ScriptHeap)->Value{
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
            let value = Value::err_stackunderflow(self.ip);
            self.trap = Some(ScriptTrap::Error(value));
            value
        }
    }
    
    pub fn peek_stack_resolved(&mut self, heap:&ScriptHeap)->Value{
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
            let value = Value::err_stackunderflow(self.ip);
            self.trap = Some(ScriptTrap::Error(value));
            value
        }
    }
    
    pub fn peek_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.last(){
            return *value
        }
        else{
            let value = Value::err_stackunderflow(self.ip);
            self.trap = Some(ScriptTrap::Error(value));
            value
        }
    }
    
    pub fn pop_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.pop(){
            return value
        }
        else{
            let value = Value::err_stackunderflow(self.ip);
            self.trap = Some(ScriptTrap::Error(value));
            value
        }
    }
    
    pub fn push_stack_value(&mut self, value:Value){
        if self.stack.len() > self.stack_limit{
            self.trap = Some(ScriptTrap::Error(Value::err_stackoverflow(self.ip)));
        }
        else{
            if value.is_err(){
                self.trap = Some(ScriptTrap::Error(value));
            }
            self.stack.push(value);
        }
    }
    
    pub fn push_stack_value_nc(&mut self, value:Value){
        if value.is_err(){
            self.trap = Some(ScriptTrap::Error(value));
        }
        self.stack.push(value);
    }
    
    pub fn call_has_me(&self)->bool{
        self.mes.len() > self.calls.last().unwrap().bases.mes
    }
    
    // lets resolve an id to a Value
    pub fn scope_value(&self, heap:&ScriptHeap, id: Id)->Value{
        return heap.value(*self.scopes.last().unwrap(), id.into(),Value::err_notfound(self.ip));
    }
    
    pub fn set_scope_value(&self, heap:&mut ScriptHeap, id: Id, value:Value){
        heap.set_value(*self.scopes.last().unwrap(), id.into(),value);
    }
    
    pub fn call(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any, fnobj:Value, args:&[Value])->Value{
        let scope = heap.new_with_proto(fnobj);
        
        heap.clear_object_deep(scope);
        heap.push_all_fn_args(scope, args);
        heap.set_object_deep(scope);
        heap.set_object_type(scope, ObjectType::AUTO);
        
        if let Some(fnptr) = heap.parent_as_fn(scope){
            match fnptr{
                ScriptFnPtr::Native(ni)=>{
                    return (*code.native.fn_table[ni.index as usize].fn_ptr)(&mut ScriptVmRef{
                        host,
                        heap,
                        thread:self,
                        code
                    }, scope);
                }
                ScriptFnPtr::Script(sip)=>{
                    let call = CallFrame{
                        bases: self.new_bases(),
                        args: OpcodeArgs::default(),
                        return_ip: None
                    };
                    self.scopes.push(scope);
                    self.calls.push(call);
                    self.ip = sip;
                    return self.run_core(heap, code, host);
                }
            }
        }
        else{
            return Value::err_notfn(self.ip)
        }
    }
    
    pub fn run_core(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any)->Value{
        let mut body = &code.bodies[self.ip.body as usize];
        while (self.ip.index as usize) < body.parser.opcodes.len(){
            let opcode = body.parser.opcodes[self.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(opcode, args, heap, code, host);
                // if exception tracing
                if let Some(trap) = self.trap.take(){
                    match trap{
                        ScriptTrap::Error(value)=>{
                            if let Some(ptr) = value.as_err(){
                                if let Some(loc2) = code.ip_to_loc(ptr.ip){
                                    println!("{} {}", value, loc2);
                                }
                            }
                        }
                        ScriptTrap::Return(value)=>{
                            return value
                        }
                    }
                }
            }
            else{ // its a direct value-to-stack?
                self.push_stack_value(opcode);
                self.ip.index += 1;
            }
            body = &code.bodies[self.ip.body as usize];
        }
        NIL
    }
    
    pub fn run_root(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any, body_id: u16){
        
        self.calls.push(CallFrame{
            bases: StackBases{
                loops: 0,
                stack: 0,
                scope: 0,
                mes: 0,
            },
            args: Default::default(),
            return_ip: None,
        });
                
        self.scopes.push(code.bodies[body_id as usize].scope);
        self.mes.push(ScriptMe::object(code.bodies[body_id as usize].me));
                
        self.ip.body = body_id;
        self.ip.index = 0;
        //let mut profile: std::collections::BTreeMap<Opcode, f64> = Default::default();
        
        // the main interpreter loop
        self.run_core(heap, code, host);
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
    }
}
