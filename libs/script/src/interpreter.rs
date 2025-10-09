use makepad_script_derive::*;
use crate::id::*;
use crate::parser::ScriptParser;
use crate::value::*;
use crate::heap::*;
use crate::opcode::*;
use crate::object::*;
use crate::interop::*;

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
    ty: u32,
    object: ObjectPtr,
}
 
impl ScriptMe{
    const ARRAY: u32 = 1;
    const CALL: u32 = 2;
    const OBJ: u32 = 3;
    fn object(object:ObjectPtr)->Self{Self{object, ty: Self::OBJ}}
    fn array(object:ObjectPtr)->Self{Self{object, ty: Self::ARRAY}}
    fn call(object:ObjectPtr)->Self{Self{object, ty: Self::CALL}}
}

pub struct ScriptThreadId(pub usize);

struct LastVec<T>{
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
        self.vec.truncate(len);
    }
}

pub struct ScriptThread{
    stack_limit: usize,
    loops: Vec<LoopFrame>,
    scopes: LastVec<ObjectPtr>,
    stack: Vec<Value>,
    calls: LastVec<CallFrame>,
    mes: LastVec<ScriptMe>,
    pub ip: usize
}

pub struct Script{
    pub sys_fns: SystemFns,
    pub parser: ScriptParser,
    pub threads: Vec<ScriptThread>,
    pub heap: ScriptHeap,
    pub global: ObjectPtr,
    pub scope: ObjectPtr,
}

impl Script{
    pub fn new()->Self{
        let mut heap = ScriptHeap::new();
        let mut sys_fns = SystemFns::default();
        crate::sys_fns::build_sys_fns(&mut sys_fns, &mut heap);
        let scope = heap.new_object(0);
        let global = heap.new_object(0);
        Self{
            sys_fns ,
            parser: Default::default(),
            threads: vec![ScriptThread::new(scope, global)],
            scope,
            global: heap.new_object(0),
            heap: heap,
        }
    }
    
    pub fn parse(&mut self, code:&str){
        self.parser.parse(code, &mut self.heap);
        self.parser.tok.dump_tokens(&self.heap);
    }
    
    pub fn run(&mut self, code: &str){
        self.parse(code);
        self.threads[0].run(&self.parser, &mut self.heap, &self.sys_fns)
    }
}

macro_rules! f64_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op2 = $obj.pop_stack_resolved($heap);
        let op1 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1);
        let v2 = $heap.cast_to_f64(op2);
        $obj.stack.push(Value::from_f64(v1 $op v2));
        $obj.ip += 1;
    }}
}

macro_rules! f64_cmp_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op2 = $obj.pop_stack_resolved($heap);
        let op1 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1);
        let v2 = $heap.cast_to_f64(op2);
        $obj.stack.push(Value::from_bool(v1 $op v2));
        $obj.ip += 1;
    }}
}


macro_rules! fu64_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op2 = $obj.pop_stack_resolved($heap);
        let op1 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1) as u64;
        let v2 = $heap.cast_to_f64(op2) as u64;
        $obj.stack.push(Value::from_f64((v1 $op v2) as f64));
        $obj.ip += 1;
    }}
} 

pub enum ScriptHook{
    SysCall(usize),
    RustCall
}

impl ScriptThread{
    
    pub fn new(scope:ObjectPtr, global:ObjectPtr)->Self{
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
            mes: LastVec::new(ScriptMe::object(global)),
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
            let scope = self.scopes.pop();
            heap.print_object(scope, true);
            heap.free_object_if_unreffed(scope);
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
    
    pub fn break_for_loop(&mut self, heap:&mut ScriptHeap){
        let lp = self.loops.pop().unwrap();
        self.truncate_bases(lp.bases, heap);
        self.ip = lp.start_ip + lp.jump - 1;
    }
    
    pub fn begin_for_loop_inner(&mut self, heap:&mut ScriptHeap, jump:usize, source:Value, value_id:Id, key_id:Option<Id>, index_id:Option<Id>, first_value:Value, first_key:Value, index:f64){    
                               
        self.ip += 1;
        self.loops.push(LoopFrame{
            bases: self.new_bases(),
            start_ip: self.ip,
            value_id,
            key_id,
            jump,
            index_id,
            source,
            index,
        });
        // lets make a new scope object and set our first value
        let scope = *self.scopes.last();
        let new_scope = heap.new_object_with_proto(scope.into());
        self.scopes.push(new_scope);
        // lets write our first value onto the scope
        heap.set_object_value(new_scope, value_id.into(), first_value);
        if let Some(key_id) = key_id{
            heap.set_object_value(new_scope, key_id.into(), first_key);
        }
        if let Some(index_id) = index_id{
            heap.set_object_value(new_scope, index_id.into(), first_key);
        }
    }
        
    pub fn begin_for_loop(&mut self, heap:&mut ScriptHeap, jump:usize, source:Value, value_id:Id, key_id:Option<Id>, index_id:Option<Id>){
        let v0 = Value::from_f64(0.0);
        if let Some(s) = source.as_f64(){
            if s >= 1.0{
                self.begin_for_loop_inner(heap, jump, source, value_id, key_id, index_id, v0, v0, 0.0);
                return
            }
        }
        else if let Some(obj) = source.as_object(){
            let proto = heap.object_prototype(obj);
            if let Some(id!(range)) = proto.as_id(){ // range object
                let start = heap.object_value(obj, id!(start).into()).as_f64().unwrap_or(0.0);
                let end = heap.object_value(obj, id!(end).into()).as_f64().unwrap_or(0.0);
                let v = start.into();
                if (start-end).abs() >= 1.0{
                    self.begin_for_loop_inner(heap, jump, source, value_id, key_id, index_id, v, v, start);
                    return
                }
            }
            else{
                let object = heap.object(obj);
                if object.tag.get_type().uses_vec2() && object.vec.len() > 1{
                    self.begin_for_loop_inner(heap, jump, source, value_id, key_id, index_id, object.vec[1], object.vec[0], 0.0);
                    return 
                }
                else if object.tag.get_type().is_vec1() && object.vec.len() > 0{
                    self.begin_for_loop_inner(heap, jump, source, value_id, key_id, index_id, object.vec[0], Value::NIL, 0.0);                  
                    return 
                }
            }
        }
        // jump over it and bail
        self.ip += jump as usize;
    }
    
    pub fn end_for_loop(&mut self, heap:&mut ScriptHeap){
        // alright lets take a look at our top loop thing
        let lf = self.loops.last_mut().unwrap();
        if let Some(end) = lf.source.as_f64(){
            lf.index += 1.0;
            if lf.index >= end{ // terminate
                self.break_for_loop(heap);
                return
            }
            self.ip = lf.start_ip;
            let scope = self.scopes.last();
            heap.set_object_value(*scope, lf.value_id.into(), lf.index.into());
        }
        else if let Some(obj) = lf.source.as_object(){
            let proto = heap.object_prototype(obj);
            if let Some(id!(range)) = proto.as_id(){
                let scope = self.scopes.last();
                let end = heap.object_value(obj, id!(end).into()).as_f64().unwrap_or(0.0);
                let step = heap.object_value(obj, id!(step).into()).as_f64().unwrap_or(1.0);
                lf.index += step;
                if lf.index >= end{
                    self.break_for_loop(heap);
                    return
                } 
                heap.set_object_value(*scope, lf.value_id.into(), lf.index.into());
                self.ip = lf.start_ip;
            }
            else{
                let object = heap.object(obj);
                if object.tag.get_type().uses_vec2() && object.vec.len() > 1{
                }
                else if object.tag.get_type().is_vec1() && object.vec.len() > 0{
                }
            }
        }
        else{ // unknown state
            println!("For end unknown state");
            self.ip += 1;
        }
    }
        
    pub fn opcode(&mut self,opcode: Opcode, args:OpcodeArgs, _parser: &ScriptParser, heap:&mut ScriptHeap, sys_fns:&SystemFns)->Option<ScriptHook>{
        match opcode{
            
            Opcode::NOT=>{
                let value = self.pop_stack_resolved(heap);
                if let Some(v) = value.as_f64(){
                    self.push_stack_value(Value::from_f64(!(v as u64) as f64));
                    self.ip += 1;
                }
                else{
                    let v = heap.cast_to_bool(value);
                    self.push_stack_value(Value::from_bool(!v));
                }
            },
            Opcode::NEG=>{
                let v = heap.cast_to_f64(self.pop_stack_resolved(heap));
                self.push_stack_value(Value::from_f64(-v));
                self.ip += 1;
            },
            
            Opcode::MUL=>f64_op_impl!(self, heap, *),
            Opcode::DIV=>f64_op_impl!(self, heap, /),
            Opcode::MOD=>f64_op_impl!(self, heap, %),
            Opcode::ADD=>f64_op_impl!(self, heap, +),
            Opcode::SUB=>f64_op_impl!(self, heap, -),
            Opcode::SHL=>fu64_op_impl!(self, heap, >>),
            Opcode::SHR=>fu64_op_impl!(self, heap, <<),
            Opcode::AND=>fu64_op_impl!(self, heap,&),
            Opcode::OR=>fu64_op_impl!(self, heap, |),
            Opcode::XOR=>fu64_op_impl!(self, heap, ^),
            
            Opcode::EQ=>f64_cmp_impl!(self, heap, ==),
            Opcode::NEQ=>f64_cmp_impl!(self, heap, !=),
            Opcode::LT=>f64_cmp_impl!(self, heap, <),
            Opcode::GT=>f64_cmp_impl!(self, heap, >),
            Opcode::LEQ=>f64_cmp_impl!(self, heap, <=),
            Opcode::GEQ=>f64_cmp_impl!(self, heap, >=),
            
            Opcode::CONCAT=>{
                let op1 = self.pop_stack_resolved(heap);
                let op2 = self.pop_stack_resolved(heap);
                let ptr = heap.new_string_with(|heap, out|{
                    heap.cast_to_string(op1, out);
                    heap.cast_to_string(op2, out);
                });
                self.push_stack_value(ptr.into());
                self.ip += 1;
            }
            
            Opcode::ASSIGN_ME=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.set_object_value(self.mes.last().object, field, value);
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.insert_object_value_at(self.mes.last().object, field, value, opcode == Opcode::ASSIGN_ME_BEFORE);
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::ASSIGN_ME_BEGIN=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.insert_object_value_begin(self.mes.last().object, field, value);
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::ASSIGN_INDEX=>{
                let value = self.pop_stack_resolved(heap);
                let index = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    heap.set_object_value(obj, index, value);
                }
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::ASSIGN_FIELD=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    heap.set_object_value(obj, field, value);
                }
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::BEGIN_PROTO=>{
                let proto = self.pop_stack_resolved(heap);
                let me = heap.new_object_with_proto(proto);
                self.mes.push(ScriptMe::object(me));
                self.ip += 1;
            }
            Opcode::BEGIN_PROTO_ME=>{
                let field = self.peek_stack_value();
                let me = self.mes.last();
                let proto = heap.object_value(me.object, field);
                let me = heap.new_object_with_proto(proto);
                self.mes.push(ScriptMe::object(me));
                self.ip += 1;
            }
            Opcode::END_PROTO=>{
                let me = self.mes.pop();
                self.push_stack_value(me.object.into());
                self.ip += 1;
            }
            Opcode::BEGIN_BARE=>{ // bare object
                let me = heap.new_object(0);
                self.mes.push(ScriptMe::object(me));
                self.ip += 1;
            }
            Opcode::END_BARE=>{
                let me = self.mes.pop();
                self.push_stack_value(me.object.into());
                self.ip += 1;
            }
            Opcode::BEGIN_ARRAY=>{
                let me = heap.new_object(0);
                self.mes.push(ScriptMe::array(me));
                self.ip += 1;
            }
            Opcode::END_ARRAY=>{
                let me = self.mes.pop();
                self.push_stack_value(me.object.into());
                self.ip += 1;
            }
            
            Opcode::CALL_ARGS=>{
                let fnobj = self.pop_stack_resolved(heap);
                let scope = heap.new_object_with_proto(fnobj);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                self.mes.push(ScriptMe::call(scope));
                self.ip += 1;
            }
            Opcode::CALL_EXEC=>{
                // ok so now we have all our args on 'mes'
                let me = self.mes.pop();
                let scope = me.object;
                // set the scope back to 'deep' so values can be written again
                heap.set_object_deep(scope);
                heap.set_object_type(scope, ObjectType::AUTO);
                                
                if let Some((jump_to, _is_system)) = heap.parent_object_as_fn(scope){
                    let call = CallFrame{
                        bases: self.new_bases(),
                        return_ip: self.ip + 1,
                    };
                    self.scopes.push(scope);
                    self.calls.push(call);
                    self.ip = jump_to as _;
                }
                else{
                    self.stack.push(Value::NIL);
                    self.ip += 1;
                }
            }
            Opcode::METHOD_CALL_ARGS=>{
                let method =  self.pop_stack_value();
                let this = self.pop_stack_resolved(heap);
                let fnobj = if let Some(obj) = this.as_object(){
                    heap.object_method(obj, method)
                }
                else{ // we're calling a method on some other thing
                    Value::NIL
                };
                let scope = if fnobj == Value::NIL{
                    // lets take the type
                    let type_index = this.value_type().to_index();
                    let method = method.as_id().unwrap_or(id!());
                    let sys_fn = &sys_fns.type_table[type_index];
                    
                    if let Some(sys_fn) = sys_fn.get(&method){
                        let scope = heap.new_object_with_proto(sys_fn.arg_obj);
                        scope
                    }
                    else{ // fn not found
                        println!("Method not found on object: {}()", method);
                        heap.new_object(0)
                    }
                }
                else{
                    heap.new_object_with_proto(fnobj)
                };
                //heap.set_object_map(scope);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                heap.set_fn_this(scope, this);
                self.mes.push(ScriptMe::call(scope));
                self.ip += 1;
            }
            Opcode::METHOD_CALL_EXEC=>{
                let me = self.mes.pop();
                let scope = me.object;
                //let this = self.peek_stack_value();
                // set the scope back to 'deep' so values can be written again
                heap.set_object_deep(scope);
                // set the heap back to a hashmap
                heap.set_object_type(scope, ObjectType::AUTO);
                
                if let Some((jump_to, is_system)) = heap.parent_object_as_fn(scope){
                    if is_system{
                        let ret = match &sys_fns.fn_table[jump_to as usize]{
                            SystemFnEntry::Inline{fn_ptr}=>{
                                fn_ptr(heap, scope)
                            }
                        };
                        self.stack.push(ret);
                        self.ip += 1;
                    }
                    else{
                        let call = CallFrame{
                            bases: self.new_bases(),
                            return_ip: self.ip + 1,
                        };
                        self.scopes.push(scope);
                        self.calls.push(call);
                        self.ip = jump_to as _;
                    }
                }
                else{
                    self.stack.push(Value::NIL);
                    self.ip += 1;
                }
            }
                        
            Opcode::FN_ARGS=>{
                let scope = *self.scopes.last_mut();
                let me = heap.new_object_with_proto(scope.into());
                                
                // set it to a vec type to ensure ordered inserts
                heap.set_object_type(me, ObjectType::VEC2);
                heap.clear_object_deep(me);
                                                
                self.mes.push(ScriptMe::object(me));
                self.ip += 1;
            }
                                    
            Opcode::FN_ARG_DYN=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                heap.set_object_value(self.mes.last().object, id.into(), value);
                self.ip += 1;                
            }
            Opcode::FN_ARG_TYPED=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let _ty = self.pop_stack_value().as_id().unwrap_or(id!());
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                heap.set_object_value(self.mes.last().object, id.into(), value);
                self.ip += 1;
            }
            Opcode::FN_BODY=>{ // alright we have all the args now we get an expression
                let jump_over_fn = args.to_u32();
                let me = self.mes.pop();
                                
                heap.set_object_is_fn(me.object, (self.ip + 1) as u32);
                self.ip += jump_over_fn as usize;
                self.stack.push(me.object.into());
            }
            Opcode::RETURN=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let call = self.calls.pop();
                self.free_unreffed_scopes(&call.bases, heap);
                self.truncate_bases(call.bases, heap);
                
                self.ip = call.return_ip;
                self.stack.push(value);
            }
            
            Opcode::IF_TEST=>{
                let test = self.pop_stack_resolved(heap);
                let test = heap.cast_to_bool(test);
                if test {
                    // continue
                    self.ip += 1
                }
                else{ // jump to else
                    self.ip += args.to_u32() as usize;
                }
            }
            
            Opcode::IF_ELSE =>{ // we are running into an else jump over it
                self.ip += args.to_u32() as usize;
            }   
            
            Opcode::FIELD=>{
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value(heap.object_value(obj, field))
                }
                else{
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            Opcode::ME_FIELD=>{
                let field = self.pop_stack_value();
                self.push_stack_value(heap.object_value(self.mes.last().object, field));
                self.ip += 1;
            }
            Opcode::PROTO_FIELD=>{ // implement proto field!
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value(heap.object_value(obj, field))
                }
                else{
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::POP_TO_ME=>{
                let value = self.stack.pop().unwrap();
                if self.call_has_me(){
                    let me = self.mes.last();
                    let (key, value) = if let Some(id) = value.as_id(){
                        if value.is_escaped_id(){ (Value::NIL, value) }
                        else{(value, self.resolve(id, heap))}
                    }else{(Value::NIL,value)};
                        
                    if !value.is_nil() || me.ty != ScriptMe::OBJ{
                        if me.ty == ScriptMe::CALL{
                            heap.push_fn_arg(me.object, value);       
                        }
                        else if me.ty == ScriptMe::OBJ{
                            heap.object_push_value(me.object, key, value);       
                        }
                        else{
                            heap.object_push_value(me.object, Value::NIL, value);       
                        }
                    }
                }
                self.ip += 1;
            }
            
            Opcode::ARRAY_INDEX=>{
                let index = self.pop_stack_resolved(heap);
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value(heap.object_value(obj, index))
                }
                else{
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
                   
            Opcode::LET_DYN=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                let scope = *self.scopes.last_mut();
                heap.set_object_value(scope, id.into(), value);
                self.ip += 1;
            }
            Opcode::LET_TYPED=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let _ty = self.pop_stack_value();
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                let scope = *self.scopes.last_mut();
                heap.set_object_value(scope, id.into(), value);
                self.ip += 1;
            } 
            
            Opcode::SEARCH_TREE=>{
                self.ip += 1;
            }
                                  
            Opcode::LOG=>{
                let value = self.peek_stack_resolved(heap);
                if value != Value::NIL{
                    if let Some(obj) = value.as_object(){
                        print!("Log OBJECT:");
                        heap.print_object(obj, true);
                        println!("");
                    }
                    else{
                        println!("Log {:?}: {:?}", value.value_type(), value);
                    }
                }
                else{
                    println!("Log: NIL");
                }
                self.ip += 1;
            }
            
            Opcode::THIS=>{
                // look up this on the scope
                let scope = *self.scopes.last_mut();
                self.push_stack_value(heap.fn_this(scope));
                self.ip += 1;
            }
            
            Opcode::ME=>{
                if self.call_has_me(){
                    let me = self.mes.last();
                    self.push_stack_value(heap.fn_this(me.object));
                }
                else{
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::SCOPE=>{
                let scope = *self.scopes.last_mut();
                self.push_stack_value(scope.into());
                self.ip += 1;
            }
            
            Opcode::FOR_1 =>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, args.to_u32() as _, source, value_id, None, None);
            }
            Opcode::FOR_2 =>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                let key_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, args.to_u32() as _, source, value_id,Some(key_id), None);
            }
            Opcode::FOR_3=>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                let key_id = self.pop_stack_value().as_id().unwrap();
                let index_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, args.to_u32() as _, source, value_id, Some(key_id), Some(index_id));
            }
            Opcode::FOR_END=>{
                self.end_for_loop(heap);
            }
            opcode=>{
                println!("UNDEFINED OPCODE {}", opcode);
                self.ip += 1;
                // unknown instruction
            }
        }
        None
    }
      
    pub fn run(&mut self, parser: &ScriptParser, heap:&mut ScriptHeap,sys_fns:&SystemFns){
        
        self.ip = 0;
        //let mut profile: std::collections::BTreeMap<Opcode, f64> = Default::default();
        let mut counter = 0;
        
        while self.ip < parser.code.len(){
            let code = parser.code[self.ip];
            if let Some((opcode, args)) = code.as_opcode(){
                if let Some(rust_call) = self.opcode(opcode, args, parser, heap, sys_fns){
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
        let scope = self.scopes.last();
        
        print!("Instructions {counter}\nScope:");
        heap.print_object(*scope, true);
        print!("\nGlobal:");
        //heap.print_object(global, true);
        println!("");                                
        //self.heap.free_object(scope);
    }
}