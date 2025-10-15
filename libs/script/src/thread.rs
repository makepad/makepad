use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value::opcode::*;
use crate::script::*;

#[derive(Debug, Default)]
pub struct StackBases{
    pub loops: usize,
    pub stack: usize,
    pub scope: usize,
    pub mes: usize,
}

#[derive(Debug)]
pub struct LoopFrame{
    pub value_id: Id,
    pub key_id: Option<Id>,
    pub index_id: Option<Id>,
    pub source: Value,
    pub start_ip: u32,
    pub jump: u32,
    pub index: f64,
    pub bases: StackBases,
}

pub struct CallFrame{
    pub bases: StackBases,
    pub args: OpcodeArgs,
    pub return_ip: ScriptIp,
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
        Value::from_err_stackunderflow(self.ip)
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
        Value::from_err_stackunderflow(self.ip)
    }
    
    pub fn peek_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.last(){
            return *value
        }
        else{
            Value::from_err_stackunderflow(self.ip)
        }
    }
    
    pub fn pop_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.pop(){
            return value
        }
        else{
            println!("STACK UNDERFLOW");
            Value::from_err_stackunderflow(self.ip)
        }
    }
    
    pub fn push_stack_value(&mut self, value:Value){
        if self.stack.len() > self.stack_limit{
           println!("STACK OVERFLOW")
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
        return heap.object_value(*self.scopes.last().unwrap(), id.into(),Value::from_err_notfound(self.ip));
    }
    
    pub fn set_scope_value(&self, heap:&mut ScriptHeap, id: Id, value:Value){
        heap.set_object_value(*self.scopes.last().unwrap(), id.into(),value);
    }
    
    pub fn call(&mut self, _heap:&mut ScriptHeap, _code:&ScriptCode, _scope:Value){
        
    }
    
    pub fn run(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, body_id: u16){
        
        self.calls.push(CallFrame{
            bases: StackBases{
                loops: 0,
                stack: 0,
                scope: 0,
                mes: 0,
            },
            args: Default::default(),
            return_ip: ScriptIp::default(),
        });
                
        self.scopes.push(code.bodies[body_id as usize].scope);
        self.mes.push(ScriptMe::object(code.bodies[body_id as usize].me));
                
        self.ip.body = body_id;
        self.ip.index = 0;
        //let mut profile: std::collections::BTreeMap<Opcode, f64> = Default::default();
        let mut counter = 0;
        #[derive(Copy,Clone,Debug)]
        struct Count{
            index: usize,
            count: usize
        }
        // let mut opcodes = [Count{index:0,count:0};128];
        // for i in 0..128{opcodes[i].index = i}
        let mut body = &code.bodies[self.ip.body as usize];
        while (self.ip.index as usize) < body.parser.opcodes.len(){
            let opcode = body.parser.opcodes[self.ip.index as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                //opcodes[opcode.0 as usize].count += 1;
                self.opcode(opcode, args, heap, code);
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
                    }
                }
                    
            }
            else{ // its a direct value-to-stack?
                self.push_stack_value(opcode);
                self.ip.index += 1;
            }
            body = &code.bodies[self.ip.body as usize];
            counter += 1;
        }
        //println!("{:?}", profile);
        // lets have a look at our scope
        let _call = self.calls.last();
        let _scope = self.scopes.last();
        //opcodes.sort_by(|a,b| a.count.cmp(&b.count));
        //println!("{:?}", opcodes);
        println!("Instructions {counter} Allocated objects:{:?}", heap.objects.len());
        //heap.print_object(*scope, true);
        //print!("Global:");
        //heap.print_object(global, true);
        //println!("");                                
        //self.heap.free_object(scope);
    }
}
