use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
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
    pub return_ip: u32,
    pub return_body: u16
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

pub struct ScriptThread{
    pub(crate) stack_limit: usize,
    pub(crate) loops: Vec<LoopFrame>,
    pub(crate) scopes: Vec<ObjectPtr>,
    pub(crate) stack: Vec<Value>,
    pub(crate) calls: Vec<CallFrame>,
    pub(crate) mes: Vec<ScriptMe>,
    pub ip: u32,
    pub body: u16,
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
            ip: 0,
            body: 0
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
                return self.resolve(id, heap)
            }
            return val    
        }
        //else{ this slows down execution by 10%? weird
        //    println!("STACK UNDERFLOW");
            Value::NIL
       // }
    }
    
    pub fn peek_stack_resolved(&mut self, heap:&ScriptHeap)->Value{
        if let Some(val) = self.stack.last(){
            if let Some(id) = val.as_id(){
                if val.is_escaped_id(){
                    return *val
                }
                return self.resolve(id, heap)
            }
            return *val    
        }
        else{
           println!("STACK UNDERFLOW");
            Value::NIL
        }
    }
    
    pub fn peek_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.last(){
            return *value
        }
        else{
            println!("STACK UNDERFLOW");
            Value::NIL
        }
    }
    
    pub fn pop_stack_value(&mut self)->Value{
        if let Some(value) = self.stack.pop(){
            return value
        }
        else{
            println!("STACK UNDERFLOW");
            Value::NIL
        }
    }
    
    pub fn push_stack_value(&mut self, value:Value){
        if self.stack.len() > self.stack_limit{
            println!("STACK OVERFLOW")
        }
        else{
            self.stack.push(value);
        }
    }
    
    pub fn call_has_me(&self)->bool{
        self.mes.len() > self.calls.last().unwrap().bases.mes
    }
    
    // lets resolve an id to a Value
    pub fn resolve(&self, id: Id, heap:&ScriptHeap)->Value{
        return heap.object_value(*self.scopes.last().unwrap(), id.into());
    }
    
    pub fn run(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, body_id: u16){
        
        self.calls.push(CallFrame{
            bases: StackBases{
                loops: 0,
                stack: 0,
                scope: 0,
                mes: 0,
            },
            return_ip: 0,
            return_body: 0,
        });
                
        self.scopes.push(code.bodies[body_id as usize].scope);
        self.mes.push(ScriptMe::object(code.bodies[body_id as usize].me));
                
        self.body = body_id;
        self.ip = 0;
        //let mut profile: std::collections::BTreeMap<Opcode, f64> = Default::default();
        let mut counter = 0;
        
        let mut body = &code.bodies[self.body as usize];
        while (self.ip as usize) < body.parser.opcodes.len(){
            let opcode = body.parser.opcodes[self.ip as usize];
            if let Some((opcode, args)) = opcode.as_opcode(){
                if let Some(rust_call) = self.opcode(opcode, args, heap, code){
                    match rust_call{
                        ScriptHook::SysCall(_sys_id)=>{
                        }
                        ScriptHook::RustCall=>{
                        }
                    }
                    self.stack.push(Value::NIL)
                }
            }
            else{ // its a direct value-to-stack?
                self.push_stack_value(opcode);
                self.ip += 1;
            }
            body = &code.bodies[self.body as usize];
            counter += 1;
        }
        //println!("{:?}", profile);
        // lets have a look at our scope
        let _call = self.calls.last();
        let _scope = self.scopes.last();
        
        println!("Instructions {counter} Allocated objects:{:?}", heap.objects.len());
        //heap.print_object(*scope, true);
        //print!("Global:");
        //heap.print_object(global, true);
        //println!("");                                
        //self.heap.free_object(scope);
    }
}
