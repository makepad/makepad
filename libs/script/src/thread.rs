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
    pub tries: usize,
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

pub struct ScriptMe{
    pub(crate) ty: u32,
    pub(crate) object: Object,
}
 
impl ScriptMe{
    pub const ARRAY: u32 = 1;
    pub const CALL: u32 = 2;
    pub const OBJ: u32 = 3;
    pub fn object(object:Object)->Self{Self{object, ty: Self::OBJ}}
    pub fn array(object:Object)->Self{Self{object, ty: Self::ARRAY}}
    pub fn call(object:Object)->Self{Self{object, ty: Self::CALL}}
}

pub struct ScriptThreadId(pub usize);

#[derive(Debug, Clone, Copy)]
pub enum ScriptTrapOn{
    Error(Value),
    Return(Value),
}

pub struct ScriptThread{
    pub(crate) stack_limit: usize,
    pub(crate) tries: Vec<TryFrame>,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) scopes: Vec<Object>,
    pub(crate) stack: Vec<Value>,
    pub(crate) calls: Vec<CallFrame>,
    pub(crate) mes: Vec<ScriptMe>,
    pub trap: ScriptTrap,
    pub(crate) last_err: Value,
}
use std::cell::Cell;
#[derive(Default, Debug)]
pub struct ScriptTrap{
    pub(crate) on: Cell<Option<ScriptTrapOn>>,
    pub ip: ScriptIp,
}

impl ScriptTrap{
    pub fn ip(&self)->u32{
        self.ip.index
    }
    pub fn goto(&mut self, wh:u32){
        self.ip.index = wh;
    }
    pub fn goto_rel(&mut self, wh:u32){
        self.ip.index += wh;
    }
    pub fn goto_next(&mut self){
        self.ip.index += 1;
    }
}

impl ScriptTrap{
    pub fn err(&self, err:Value)->Value{
        self.on.set(Some(ScriptTrapOn::Error(err)));
        err
    }
    
    pub fn err_notfound(&self)->Value{self.err(Value::err_notfound(self.ip))}
    pub fn err_notfn(&self)->Value{self.err(Value::err_notfn(self.ip))}
    pub fn err_notindex(&self)->Value{self.err(Value::err_notindex(self.ip))}
    pub fn err_notobject(&self)->Value{self.err(Value::err_notobject(self.ip))}
    pub fn err_stackunderflow(&self)->Value{self.err(Value::err_stackunderflow(self.ip))}
    pub fn err_stackoverflow(&self)->Value{self.err(Value::err_stackoverflow(self.ip))}
    pub fn err_invalidargs(&self)->Value{self.err(Value::err_invalidargs(self.ip))}
    pub fn err_notassignable(&self)->Value{self.err(Value::err_notassignable(self.ip))}
    pub fn err_unexpected(&self)->Value{self.err(Value::err_unexpected(self.ip))}
    pub fn err_assertfail(&self)->Value{self.err(Value::err_assertfail(self.ip))}
    pub fn err_notimpl(&self)->Value{self.err(Value::err_notimpl(self.ip))}
    pub fn err_frozen(&self)->Value{self.err(Value::err_frozen(self.ip))}
    pub fn err_vecfrozen(&self)->Value{self.err(Value::err_vecfrozen(self.ip))}
    pub fn err_invalidproptype(&self)->Value{self.err(Value::err_invalidproptype(self.ip))}
    pub fn err_invalidpropname(&self)->Value{self.err(Value::err_invalidpropname(self.ip))}
    pub fn err_keyalreadyexists(&self)->Value{self.err(Value::err_keyalreadyexists(self.ip))}
    pub fn err_invalidkeytype(&self)->Value{self.err(Value::err_invalidkeytype(self.ip))}
    pub fn err_vecbound(&self)->Value{self.err(Value::err_vecbound(self.ip))}
    pub fn err_invalidargtype(&self)->Value{self.err(Value::err_invalidargtype(self.ip))}
    pub fn err_invalidargname(&self)->Value{self.err(Value::err_invalidargname(self.ip))}
    pub fn err_invalidvarname(&self)->Value{self.err(Value::err_invalidvarname(self.ip))}
    pub fn err_user(&self)->Value{self.err(Value::err_user(self.ip))}
}

pub enum ScriptHook{
    SysCall(usize),
    RustCall
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
            self.trap.err_stackunderflow()
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
            self.trap.err_stackunderflow()
        }
    }
    
    pub fn peek_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.last(){
            return *value
        }
        else{
            self.trap.err_stackunderflow()
        }
    }
    
    pub fn pop_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.pop(){
            return value
        }
        else{
            self.trap.err_stackunderflow()
        }
    }
    
    pub fn push_stack_value(&mut self, value:Value){
        if self.stack.len() > self.stack_limit{
            self.trap.err_stackoverflow();
        }
        else{
            self.stack.push(value);
        }
    }
    
    pub fn push_stack_value_nc(&mut self, value:Value){
        self.stack.push(value);
    }
    
    pub fn call_has_me(&self)->bool{
        self.mes.len() > self.calls.last().unwrap().bases.mes
    }
    
    pub fn call_has_try(&self)->bool{
        self.tries.len() > self.calls.last().unwrap().bases.tries
    }
    
    // lets resolve an id to a Value
    pub fn scope_value(&mut  self, heap:&ScriptHeap, id: Id)->Value{
        let val = heap.value(*self.scopes.last().unwrap(), id.into(),&mut self.trap);
        val
    }
    
    pub fn set_scope_value(&mut self, heap:&mut ScriptHeap, id: Id, value:Value)->Value{
        heap.set_scope_value(*self.scopes.last().unwrap(), id.into(),value,&mut self.trap)
    }
    
    pub fn def_scope_value(&mut self, heap:&mut ScriptHeap, id: Id, value:Value){
        // alright if we are shadowing a value, we need to make a new scope
        if let Some(new_scope) = heap.def_scope_value(*self.scopes.last().unwrap(), id, value){
            self.scopes.push(new_scope);
        }
    }
    
    pub fn call(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any, fnobj:Value, args:&[Value])->Value{
        let scope = heap.new_with_proto(fnobj);
        
        heap.clear_object_deep(scope);
        
        let err = heap.push_all_fn_args(scope, args, &mut self.trap);
        if err.is_err(){
            return err
        }
        
        heap.set_object_deep(scope);
        heap.set_object_type(scope, ObjectType::AUTO);
        
        if let Some(fnptr) = heap.parent_as_fn(scope){
            match fnptr{
                ScriptFnPtr::Native(ni)=>{
                    return (*code.native.borrow().fn_table[ni.index as usize].fn_ptr)(&mut Vm{
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
                    self.trap.ip = sip;
                    return self.run_core(heap, code, host);
                }
            }
        }
        else{
            return self.trap.err_notfn()
        }
    }
    
    pub fn run_core(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any)->Value{
        let mut body = &code.bodies[self.trap.ip.body as usize];
        while (self.trap.ip.index as usize) < body.parser.opcodes.len(){
            let opcode = body.parser.opcodes[self.trap.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                self.opcode(opcode, args, heap, code, host);
                // if exception tracing
                if let Some(trap) = self.trap.on.take(){
                    match trap{
                        ScriptTrapOn::Error(value)=>{
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
                                        println!("{} {}", value, loc2);
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
            body = &code.bodies[self.trap.ip.body as usize];
        }
        NIL
    }
    
    pub fn run_root(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any, body_id: u16){
        
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
                
        self.scopes.push(code.bodies[body_id as usize].scope);
        self.mes.push(ScriptMe::object(code.bodies[body_id as usize].me));
                
        self.trap.ip.body = body_id;
        self.trap.ip.index = 0;
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
