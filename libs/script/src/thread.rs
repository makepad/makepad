use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value_derive::*;
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
    pub start_ip: usize,
    pub jump: usize,
    pub index: f64,
    pub bases: StackBases,
}

pub struct CallFrame{
    pub bases: StackBases,
    pub return_ip: usize,
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
    pub(crate) scopes: LastVec<ObjectPtr>,
    pub(crate) stack: Vec<Value>,
    pub(crate) calls: LastVec<CallFrame>,
    pub(crate) mes: LastVec<ScriptMe>,
    pub ip: usize
}

pub enum ScriptHook{
    SysCall(usize),
    RustCall
}

impl ScriptThread{
    
    pub fn new(heap: &mut ScriptHeap, scope: ObjectPtr)->Self{
        let root = heap.new_object_with_proto(id!(root).into());
        Self{
            scopes:  LastVec::new(scope),
            stack_limit: 1_000_000,
            loops: vec![],
            stack: vec![],
            calls: LastVec::new(CallFrame{
                bases: StackBases{
                    loops: 0,
                    stack: 0,
                    scope: 1,
                    mes: 1,
                },
                return_ip: 0,
            }),
            mes: LastVec::new(ScriptMe::object(root)),
            ip: 0
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
            heap.free_object_if_unreffed(self.scopes.pop());
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
        self.mes.len() > self.calls.last().bases.mes
    }
    
    // lets resolve an id to a Value
    pub fn resolve(&self, id: Id, heap:&ScriptHeap)->Value{
        return heap.object_value(*self.scopes.last(), id.into());
    }
    
    pub fn run(&mut self, heap:&mut ScriptHeap, ctx:&ScriptCtx){
        
        self.ip = 0;
        //let mut profile: std::collections::BTreeMap<Opcode, f64> = Default::default();
        let mut counter = 0;
        
        while self.ip < ctx.parser.code.len(){
            let code = ctx.parser.code[self.ip];
            if let Some((opcode, args)) = code.as_opcode(){
                if let Some(rust_call) = self.opcode(opcode, args, heap, ctx){
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
                self.push_stack_value(code);
                self.ip += 1;
            }
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

pub struct LastVec<T>{
    vec: Vec<T>,
}

impl<T> LastVec<T>{
    pub fn new(t:T)->Self{
        Self{
            vec:vec![t],
        }
    }
        
    pub fn last(&self)->&T{
        let idx = self.vec.len()-1;
        unsafe{self.vec.get_unchecked(idx)}
    }
        
    pub fn last_mut(&mut self)->&T{
        let idx = self.vec.len()-1;
        unsafe{self.vec.get_unchecked_mut(idx)}
    }
        
    pub fn push(&mut self, t:T){
        self.vec.push(t);
    }
        
    pub fn pop(&mut self)->T{
        let r = self.vec.pop().unwrap();
        if self.vec.len()==0{panic!()}
        r
    }
        
    pub fn len(&self)->usize{
        self.vec.len()
    }
        
    pub fn truncate(&mut self, len:usize){
        self.vec.truncate(len.max(1));
    }
}