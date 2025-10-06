use makepad_script_derive::*;
use crate::id::*;
use crate::parser::ScriptParser;
use crate::value::*;
use crate::heap::*;
use crate::opcode::*;

pub struct CallFrame{
    pub scope: ObjectPtr,
    pub stack_base: usize,
    pub mes_base: usize,
    pub return_ip: usize,
}

pub struct ScriptMe{
    ty: u32,
    argc: u32,
    object: ObjectPtr,
}
 
impl ScriptMe{
    const ARRAY: u32 = 1;
    const CALL: u32 = 2;
    const OBJ: u32 = 3;
    fn object(object:ObjectPtr)->Self{Self{object, ty: Self::OBJ, argc:0}}
    fn array(object:ObjectPtr)->Self{Self{object, ty: Self::ARRAY, argc:0}}
    fn call(object:ObjectPtr)->Self{Self{object, ty: Self::CALL, argc:0}}
}

pub struct ScriptThread{
    stack_limit: usize,
    stack: Vec<Value>,
    calls: Vec<CallFrame>,
    mes: Vec<ScriptMe>,
    pub ip: usize
}
    
pub struct ScriptInterpreter{
    pub threads: Vec<ScriptThread>,
    pub heap: ScriptHeap,
    pub global: ObjectPtr,
}

impl ScriptInterpreter{
    pub fn new()->Self{
        let mut heap = ScriptHeap::new();
        Self{
            threads: vec![ScriptThread::new()],
            global: heap.new_object(ObjectTag::MAP),
            heap: heap,
        }
    }
    pub fn run(&mut self, parser: &ScriptParser){
        self.threads[0].run(parser, &mut self.heap, self.global)
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

impl ScriptThread{
    
    const SYSTEM_OBJECT_METHODS: [(Id,fn(t:&mut ScriptThread));1] = [
        // len
        (id!(len),|t|{
            
        })
    ];
    
    fn look_up_system_method()->Value{
        Value::NIL
    }
    
    
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
    
    pub fn peek_stack_pair(&mut self, heap:&ScriptHeap)->(Value,Value){
        if let Some(val) = self.stack.last(){
            if let Some(id) = val.as_id(){
                if val.is_escaped_id(){
                    return (*val,*val)
                }
                return (*val, self.resolve(id, heap))
            }
            return (*val,*val)    
        }
        else{
           println!("STACK UNDERFLOW");
            (Value::NIL, Value::NIL)
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
        if id == id!(me){
            if let Some(me) = self.mes.last(){
                if self.call_has_me(){
                    return (*me).object.into()
                }
            }
        }
        else if let Some(call) = self.calls.last(){
            if id == id!(scope){
                return (call.scope).into()
            }
            return heap.object_value(call.scope, id.into())
        }
        Value::NIL
    }
    
    pub fn opcode(&mut self,opcode: Opcode, args:OpcodeArgs, _parser: &ScriptParser, heap:&mut ScriptHeap){
        match opcode{
            Opcode::POP_TO_ME=>{
                let value = self.stack.pop().unwrap();
                if let Some(me) = self.mes.last(){
                    if self.call_has_me(){
                        let (key, value) = if let Some(id) = value.as_id(){
                            if value.is_escaped_id(){
                                (Value::NIL, value)
                            }
                            else if me.ty == ScriptMe::ARRAY{
                                (Value::NIL, self.resolve(id, heap))
                            }
                            else{
                                (value, self.resolve(id, heap))
                            }
                        }
                        else{
                            (Value::NIL, value)
                        };
                        if me.ty == ScriptMe::CALL{
                            heap.push_fn_arg(me.object, value);
                        }
                        else if !value.is_nil() || me.ty == ScriptMe::ARRAY{
                            heap.push_object_value(me.object, key, value);
                        }
                    }
                }
                self.ip += 1;
            }
            Opcode::LOG=>{
                let value = self.peek_stack_pair(heap);
                if value.1 != Value::NIL{
                    if let Some(obj) = value.1.as_object(){
                        print!("Log :");
                        heap.print_object(obj, true);
                        println!("");
                    }
                    else{
                        println!("Log: {:?}", value.1);
                    }
                }
                else{
                    println!("Log: {}:{}", value.0,value.1)
                }
                self.ip += 1;
            }
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
            Opcode::LET_DYN=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                let call = self.calls.last_mut().unwrap();
                heap.push_object_value(call.scope, id.into(), value);
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
                heap.push_object_value(call.scope, id.into(), value);
                self.ip += 1;
            }
            Opcode::SEARCH_TREE=>{
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
                    heap.set_object_value_top(me.object, id.into(), value);
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
                    heap.set_object_value_top(me.object, id.into(), value);
                }
                self.ip += 1;
            }
            Opcode::FN_ARGS=>{
                let call = self.calls.last_mut().unwrap();
                let me = heap.new_object_with_proto(call.scope.into());
                // we should set this as fields
                heap.clear_object_map(me);
                
                self.mes.push(ScriptMe::object(me));
                self.ip += 1;
            }
            Opcode::FN_BODY=>{ // alright we have all the args now we get an expression
                let jump_over_fn = args.to_u32();
                let me = self.mes.pop().unwrap();
                heap.set_object_is_fn(me.object, (self.ip + 1) as u32);
                self.ip += jump_over_fn as usize;
                self.stack.push(me.object.into());
            }
            Opcode::METHOD_CALL_ARGS=>{
                let method =  self.pop_stack_value();
                let this = self.pop_stack_resolved(heap);
                // alright so now we look up the method on this
                println!("LOOKING UP METHOD{}", method);
                let fnobj = if let Some(obj) = this.as_object(){
                    heap.object_method(obj, method)
                }
                else{ // we're calling a method on some other thing
                    Value::NIL
                };
                let scope = if fnobj == Value::NIL{ // ok look up the method on the system api
                    // lets check if our object has a foreign baseclass
                    
                    
                    let scope = heap.new_object(ObjectTag::MAP);
                    
                    //if this.is_object(){
                    //    SYSTEM_OBJECT_METHODS
                   //}
                    //for (name,_) in 
                    heap.set_object_is_system_fn(scope, 0);
                    heap.set_object_value(scope, id!(this).into(), this);
                    scope
                }
                else{
                    heap.new_object_with_proto(fnobj)
                };
                heap.set_object_map(scope);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                heap.set_object_value(scope, id!(this).into(), this);
                self.mes.push(ScriptMe::call(scope));
                self.ip += 1;
            }
            Opcode::METHOD_CALL_EXEC=>{
                let me = self.mes.pop().unwrap();
                let scope = me.object;
                // set the scope back to 'deep' so values can be written again
                heap.set_object_deep(scope);
                if let Some((jump_to, is_system)) = heap.get_parent_object_is_fn(scope){
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
                if let Some((jump_to, is_system)) = heap.get_parent_object_is_fn(scope){
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
            _=>{
                self.ip += 1;
                // unknown instruction
            }
        }
    }
      
    pub fn run(&mut self, parser: &ScriptParser, heap:&mut ScriptHeap, global:ObjectPtr){
        let scope = heap.new_object(ObjectTag::MAP|ObjectTag::DEEP);
                
        let call = CallFrame{
            scope,
            mes_base: 0,
            stack_base: 0,
            return_ip: 0,
        };
        self.mes.push(ScriptMe::object(global));
        self.calls.push(call);
        self.ip = 0;
        while self.ip < parser.code.len(){
            let code = parser.code[self.ip];
            if let Some((opcode, args)) = code.as_opcode(){
                self.opcode(opcode, args, parser, heap);
            }
            else{ // its a direct value-to-stack?
                self.push_stack_value(code);
                self.ip += 1;
            }
        }
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