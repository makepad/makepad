use makepad_script_derive::*;
use crate::id::*;
use crate::parser::ScriptParser;
use crate::value::*;
use crate::heap::*;
use crate::opcode::*;
use crate::object::*;
use crate::interop::*;

pub struct CallFrame{
    pub scope: ObjectPtr,
    pub stack_base: usize,
    pub mes_base: usize,
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

pub struct ScriptThread{
    stack_limit: usize,
    stack: Vec<Value>,
    calls: Vec<CallFrame>,
    mes: Vec<ScriptMe>,
    pub ip: usize
}

pub struct Script{
    pub sys_fns: SystemFns,
    pub parser: ScriptParser,
    pub threads: Vec<ScriptThread>,
    pub heap: ScriptHeap,
    pub global: ObjectPtr,
}

impl Script{
    pub fn new()->Self{
        let mut heap = ScriptHeap::new();
        let mut sys_fns = SystemFns::default();
        crate::sys_fns::build_sys_fns(&mut sys_fns, &mut heap);
        Self{
            sys_fns ,
            parser: Default::default(),
            threads: vec![ScriptThread::new()],
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
        self.threads[0].run(&self.parser, &mut self.heap, self.global, &self.sys_fns)
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
    
    pub fn new()->Self{
        Self{
            stack_limit: 1_000_000,
            stack: vec![],
            calls: vec![],
            mes: vec![],
            ip: 0
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
        self.mes.len() > self.calls.last().unwrap().mes_base
    }
    
    // lets resolve an id to a Value
    pub fn resolve(&self, id: Id, heap:&ScriptHeap)->Value{
        if let Some(call) = self.calls.last(){
            return heap.object_value(call.scope, id.into())
        }
        Value::NIL
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
                if let Some(me) = self.mes.last(){
                    heap.set_object_value(me.object, field, value);
                }
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                if let Some(me) = self.mes.last(){
                    heap.insert_object_value_at(me.object, field, value, opcode == Opcode::ASSIGN_ME_BEFORE);
                }
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
            
            Opcode::ASSIGN_ME_BEGIN=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                if let Some(me) = self.mes.last(){
                    heap.insert_object_value_begin(me.object, field, value);
                }
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
                let me = self.mes.last().unwrap();
                let proto = heap.object_value(me.object, field);
                let me = heap.new_object_with_proto(proto);
                self.mes.push(ScriptMe::object(me));
                self.ip += 1;
            }
            Opcode::END_PROTO=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value(me.object.into());
                self.ip += 1;
            }
            Opcode::BEGIN_BARE=>{ // bare object
                let me = heap.new_object(0);
                self.mes.push(ScriptMe::object(me));
                self.ip += 1;
            }
            Opcode::END_BARE=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value(me.object.into());
                self.ip += 1;
            }
            Opcode::BEGIN_ARRAY=>{
                let me = heap.new_object(0);
                self.mes.push(ScriptMe::array(me));
                self.ip += 1;
            }
            Opcode::END_ARRAY=>{
                let me = self.mes.pop().unwrap();
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
                let me = self.mes.pop().unwrap();
                let scope = me.object;
                // set the scope back to 'deep' so values can be written again
                heap.set_object_deep(scope);
                heap.set_object_type(scope, ObjectType::AUTO);
                                
                if let Some((jump_to, _is_system)) = heap.parent_object_as_fn(scope){
                    let call = CallFrame{
                        scope,
                        mes_base: self.mes.len(),
                        stack_base: self.stack.len(),
                        return_ip: self.ip + 1,
                    };
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
                let me = self.mes.pop().unwrap();
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
                            scope,
                            mes_base: self.mes.len(),
                            stack_base: self.stack.len() - 1,
                            return_ip: self.ip + 1,
                        };
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
                let call = self.calls.last_mut().unwrap();
                let me = heap.new_object_with_proto(call.scope.into());
                                
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
                if let Some(me) = self.mes.last(){
                    heap.set_object_value(me.object, id.into(), value);
                }
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
                if let Some(me) = self.mes.last(){
                    heap.set_object_value(me.object, id.into(), value);
                }
                self.ip += 1;
            }
            Opcode::FN_BODY=>{ // alright we have all the args now we get an expression
                let jump_over_fn = args.to_u32();
                let me = self.mes.pop().unwrap();
                                
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
                let call = self.calls.pop().unwrap();
                // we need to check if the scope was hooked by a closure
                
                heap.free_object_if_unreffed(call.scope);
                self.stack.truncate(call.stack_base);
                self.mes.truncate(call.mes_base);
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
                if let Some(me) = self.mes.last(){
                    self.push_stack_value(heap.object_value(me.object, field))
                }
                else{
                    self.push_stack_value(Value::NIL);
                }
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
                if let Some(me) = self.mes.last(){
                    if self.call_has_me(){
                        
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
                let call = self.calls.last_mut().unwrap();
                heap.set_object_value(call.scope, id.into(), value);
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
                let call = self.calls.last_mut().unwrap();
                heap.set_object_value(call.scope, id.into(), value);
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
                let call = self.calls.last_mut().unwrap();
                let scope = call.scope;
                self.push_stack_value(heap.fn_this(scope));
                self.ip += 1;
            }
            
            Opcode::ME=>{
                if self.call_has_me(){
                    let me = self.mes.last().unwrap();
                    self.push_stack_value(heap.fn_this(me.object));
                }
                else{
                    self.push_stack_value(Value::NIL);
                }
                self.ip += 1;
            }
                             
            opcode=>{
                println!("UNDEFINED OPCODE {}", opcode);
                self.ip += 1;
                // unknown instruction
            }
        }
        None
    }
      
    pub fn run(&mut self, parser: &ScriptParser, heap:&mut ScriptHeap, global:ObjectPtr, sys_fns:&SystemFns){
        let scope = heap.new_object(ObjectTag::DEEP);
        //heap.set_object_type(scope, ObjectType::VEC2);
        let call = CallFrame{
            scope,
            mes_base: 0,
            stack_base: 0,
            return_ip: 0,
        };
        self.mes.push(ScriptMe::object(global));
        self.calls.push(call);
        self.ip = 0;
        //let mut profile: std::collections::BTreeMap<Opcode, f64> = Default::default();
        while self.ip < parser.code.len(){
            let code = parser.code[self.ip];
            if let Some((opcode, args)) = code.as_opcode(){
                //let dt = std::time::Instant::now();
                if let Some(rust_call) = self.opcode(opcode, args, parser, heap, sys_fns){
                    match rust_call{
                        ScriptHook::SysCall(_sys_id)=>{
                        }
                        ScriptHook::RustCall=>{
                        }
                    }
                    self.stack.push(Value::NIL)
                }
                //if let Some(t) = profile.get(&opcode){
                 //   profile.insert(opcode, t + dt.elapsed().as_secs_f64());
                //}
                //else{
                //    profile.insert(opcode, dt.elapsed().as_secs_f64());
                //}
            }
            else{ // its a direct value-to-stack?
                self.push_stack_value(code);
                self.ip += 1;
            }
        }
        //println!("{:?}", profile);
        // lets have a look at our scope
        let call = self.calls.pop().unwrap();
        print!("Scope:");
        heap.print_object(call.scope, true);
        self.mes.pop();
        print!("\nGlobal:");
        heap.print_object(global, true);
        println!("");                                
        //self.heap.free_object(scope);
    }
}