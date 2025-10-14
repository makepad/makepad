use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value::opcode::*;
use crate::makepad_value_derive::*;
use crate::object::*;
use crate::script::*;
use crate::thread::*;

macro_rules! f64_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op2 = $obj.pop_stack_resolved($heap);
        let op1 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1);
        let v2 = $heap.cast_to_f64(op2);
        $obj.stack.push(Value::from_f64(v1 $op v2));
        $obj.ip.index += 1;
    }}
}

macro_rules! f64_cmp_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op2 = $obj.pop_stack_resolved($heap);
        let op1 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1);
        let v2 = $heap.cast_to_f64(op2);
        $obj.stack.push(Value::from_bool(v1 $op v2));
        $obj.ip.index += 1;
    }}
}


macro_rules! fu64_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op2 = $obj.pop_stack_resolved($heap);
        let op1 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_f64(op1) as u64;
        let v2 = $heap.cast_to_f64(op2) as u64;
        $obj.stack.push(Value::from_f64((v1 $op v2) as f64));
        $obj.ip.index += 1;
    }}
} 

macro_rules! bool_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let op2 = $obj.pop_stack_resolved($heap);
        let op1 = $obj.pop_stack_resolved($heap);
        let v1 = $heap.cast_to_bool(op1);
        let v2 = $heap.cast_to_bool(op2);
        $obj.stack.push(Value::from_bool((v1 $op v2)));
        $obj.ip.index += 1;
    }}
} 

impl ScriptThread{
    
    pub fn opcode(&mut self,opcode: Opcode, args:OpcodeArgs, heap:&mut ScriptHeap, code:&ScriptCode)->Option<ScriptHook>{
        match opcode{
            
            Opcode::NOT=>{
                let value = self.pop_stack_resolved(heap);
                if let Some(v) = value.as_f64(){
                    self.push_stack_value(Value::from_f64(!(v as u64) as f64));
                    self.ip.index += 1;
                }
                else{
                    let v = heap.cast_to_bool(value);
                    self.push_stack_value(Value::from_bool(!v));
                }
            },
            Opcode::NEG=>{
                let v = heap.cast_to_f64(self.pop_stack_resolved(heap));
                self.push_stack_value(Value::from_f64(-v));
                self.ip.index += 1;
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
            
            Opcode::LOGIC_AND => bool_op_impl!(self, heap, &&),
            Opcode::LOGIC_OR => bool_op_impl!(self, heap, ||),
            
            Opcode::CONCAT=>{
                let op1 = self.pop_stack_resolved(heap);
                let op2 = self.pop_stack_resolved(heap);
                let ptr = heap.new_string_with(|heap, out|{
                    heap.cast_to_string(op1, out);
                    heap.cast_to_string(op2, out);
                });
                self.push_stack_value(ptr.into());
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_ME=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.set_object_value(self.mes.last().unwrap().object, field, value);
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.insert_object_value_at(self.mes.last().unwrap().object, field, value, opcode == Opcode::ASSIGN_ME_BEFORE);
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_ME_BEGIN=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.insert_object_value_begin(self.mes.last().unwrap().object, field, value);
                if !args.is_statement(){
                    self.push_stack_value(Value::NIL);
                }
                self.ip.index += 1;
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
                self.ip.index += 1;
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
                self.ip.index += 1;
            }
            
            Opcode::BEGIN_PROTO=>{
                let proto = self.pop_stack_resolved(heap);
                let me = heap.new_object_with_proto(proto);
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
            Opcode::BEGIN_PROTO_ME=>{
                let field = self.peek_stack_value();
                let me = self.mes.last().unwrap();
                let proto = heap.object_value(me.object, field, Value::NIL);
                let me = heap.new_object_with_proto(proto);
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
            Opcode::END_PROTO=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value(me.object.into());
                self.ip.index += 1;
            }
            Opcode::BEGIN_BARE=>{ // bare object
                let me = heap.new_object(0);
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
            Opcode::END_BARE=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value(me.object.into());
                self.ip.index += 1;
            }
            Opcode::BEGIN_ARRAY=>{
                let me = heap.new_object(0);
                self.mes.push(ScriptMe::array(me));
                self.ip.index += 1;
            }
            Opcode::END_ARRAY=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value(me.object.into());
                self.ip.index += 1;
            }
            
            Opcode::CALL_ARGS=>{
                let fnobj = self.pop_stack_resolved(heap);
                let scope = heap.new_object_with_proto(fnobj);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                self.mes.push(ScriptMe::call(scope));
                self.ip.index += 1;
            }
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC=>{
                //self.call_exec(heap, code, scope);
                // ok so now we have all our args on 'mes'
                let me = self.mes.pop().unwrap();
                let scope = me.object;
                // set the scope back to 'deep' so values can be written again
                heap.set_object_deep(scope);
                heap.set_object_type(scope, ObjectType::AUTO);
                                
                if let Some(fnptr) = heap.parent_object_as_fn(scope){
                    match fnptr{
                        ScriptFnPtr::Native(ni)=>{
                            let ret = (*code.native.fn_table[ni.index as usize].fn_ptr)(&mut ScriptCtx{
                                heap,
                                thread:self,
                                code
                            }, scope);
                            self.stack.push(ret);
                            heap.free_object_if_unreffed(scope);
                            self.ip.index += 1;
                        }
                        ScriptFnPtr::Script(sip)=>{
                            let call = CallFrame{
                                bases: self.new_bases(),
                                return_ip: ScriptIp{index: self.ip.index + 1, body:self.ip.body}
                            };
                            self.scopes.push(scope);
                            self.calls.push(call);
                            self.ip = sip;
                        }
                    }
                }
                else{
                    self.stack.push(Value::from_exc_call(self.ip));
                    self.ip.index += 1;
                }
            }
            Opcode::METHOD_CALL_ARGS=>{
                let method =  self.pop_stack_value();
                let this = self.pop_stack_resolved(heap);
                let fnobj = if let Some(obj) = this.as_object(){
                    heap.object_method(obj, method, Value::NIL)
                }
                else{ // we're calling a method on some other thing
                    Value::NIL
                };
                let scope = if fnobj == Value::NIL{
                    // lets take the type
                    let type_index = this.value_type().to_redux();
                    let method = method.as_id().unwrap_or(id!());
                    let type_entry = &code.methods.type_table[type_index];
                    
                    if let Some(method_ptr) = type_entry.get(&method){
                        let scope = heap.new_object_with_proto(method_ptr.fn_obj);
                        scope
                    }
                    else{ 
                        heap.new_object_with_proto(id!(undefined_function).into())
                    }
                }
                else{
                    heap.new_object_with_proto(fnobj)
                };
                //heap.set_object_map(scope);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                heap.set_object_value_in_map(scope, id!(this).into(), this.into());
                self.mes.push(ScriptMe::call(scope));
                self.ip.index += 1;
            }
            
            Opcode::FN_ARGS=>{
                let scope = *self.scopes.last_mut().unwrap();
                let me = heap.new_object_with_proto(scope.into());
                                
                // set it to a vec type to ensure ordered inserts
                heap.set_object_type(me, ObjectType::VEC2);
                heap.clear_object_deep(me);
                                                
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
                                    
            Opcode::FN_ARG_DYN=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                heap.set_object_value(self.mes.last().unwrap().object, id.into(), value);
                self.ip.index += 1;                
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
                heap.set_object_value(self.mes.last().unwrap().object, id.into(), value);
                self.ip.index += 1;
            }
            Opcode::FN_BODY=>{ // alright we have all the args now we get an expression
                let jump_over_fn = args.to_u32();
                let me = self.mes.pop().unwrap();
                                
                heap.set_object_fn(me.object, ScriptFnPtr::Script(
                    ScriptIp{body: self.ip.body, index:(self.ip.index + 1)}
                ));
                self.ip.index += jump_over_fn;
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
                self.truncate_bases(call.bases, heap);
                
                self.ip = call.return_ip;
                self.stack.push(value);
            }
            
            Opcode::IF_TEST=>{
                let test = self.pop_stack_resolved(heap);
                let test = heap.cast_to_bool(test);
                if test {
                    // continue
                    self.ip.index += 1
                }
                else{ // jump to else
                    self.ip.index += args.to_u32();
                }
            }
            
            Opcode::IF_ELSE =>{ // we are running into an else jump over it
                self.ip.index += args.to_u32();
            }   
            
            Opcode::FIELD=>{
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value(heap.object_value(obj, field, Value::from_exc_read(self.ip)))
                }
                else{
                    self.push_stack_value(Value::from_exc_read(self.ip));
                }
                self.ip.index += 1;
            }
            Opcode::ME_FIELD=>{
                let field = self.pop_stack_value();
                self.push_stack_value(heap.object_value(self.mes.last().unwrap().object, field,Value::NIL));
                self.ip.index += 1;
            }
            Opcode::PROTO_FIELD=>{ // implement proto field!
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value(heap.object_value(obj, field, Value::from_exc_read(self.ip)))
                }
                else{
                    self.push_stack_value(Value::from_exc_read(self.ip));
                }
                self.ip.index += 1;
            }
            
            Opcode::POP_TO_ME=>{
                let value = self.pop_stack_value();
                if self.call_has_me(){
                    let me = self.mes.last().unwrap();
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
                self.ip.index += 1;
            }
            
            Opcode::ARRAY_INDEX=>{
                let index = self.pop_stack_resolved(heap);
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value(heap.object_value(obj, index,Value::from_exc_read(self.ip)))
                }
                else{
                    self.push_stack_value(Value::from_exc_read(self.ip));
                }
                self.ip.index += 1;
            }
                   
            Opcode::LET_DYN=>{
                let value = if args.is_nil(){
                    Value::NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                let scope = *self.scopes.last_mut().unwrap();
                heap.set_object_value(scope, id.into(), value);
                self.ip.index += 1;
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
                let scope = *self.scopes.last_mut().unwrap();
                heap.set_object_value(scope, id.into(), value);
                self.ip.index += 1;
            } 
            
            Opcode::SEARCH_TREE=>{
                self.ip.index += 1;
            }
                                  
            Opcode::LOG=>{
                
                if let Some(loc) = code.ip_to_loc(self.ip){
                    let value = self.peek_stack_resolved(heap);
                    if value != Value::NIL{
                        if let Some(ptr) = value.as_exc(){
                            if let Some(loc2) = code.ip_to_loc(ptr){
                                println!("{} {} {}", loc, value, loc2);
                            }
                        }
                        else if let Some(obj) = value.as_object(){
                            print!("{} ", loc);
                            heap.print_object(obj, true);
                            println!("");
                        }
                        else{
                            println!("{} {:?}: {:?}", loc, value.value_type(), value);
                        }
                    }
                    else{
                        println!("{} nil", loc);
                    }
                }
                self.ip.index += 1;
            }
            
            Opcode::ME=>{
                if self.call_has_me(){
                    let me = self.mes.last().unwrap();
                    self.push_stack_value(me.object.into());
                }
                else{
                    self.push_stack_value(Value::NIL);
                }
                self.ip.index += 1;
            }
            
            Opcode::SCOPE=>{
                let scope = *self.scopes.last_mut().unwrap();
                self.push_stack_value(scope.into());
                self.ip.index += 1;
            }
            
            Opcode::FOR_1 =>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, code, args.to_u32() as _, source, value_id, None, None);
            }
            Opcode::FOR_2 =>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                let index_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, code, args.to_u32() as _, source, value_id,Some(index_id), None);
            }
            Opcode::FOR_3=>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                let index_id = self.pop_stack_value().as_id().unwrap();
                let key_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, code, args.to_u32() as _, source, value_id, Some(index_id), Some(key_id));
            }
            Opcode::FOR_END=>{
                self.end_for_loop(heap, code);
            }
            Opcode::RANGE=>{
                let end = self.pop_stack_resolved(heap);
                let start = self.pop_stack_resolved(heap);
                let range = heap.new_object_with_proto(code.builtins.range.into());
                heap.set_object_value(range, id!(start).into(), start);
                heap.set_object_value(range, id!(end).into(), end);
                self.stack.push(range.into());
                self.ip.index += 1;
            }
            Opcode::IS=>{
                let rhs = self.pop_stack_value();
                let lhs = self.pop_stack_resolved(heap);
                let cmp = if let Some(id) = rhs.as_id(){
                    match lhs.value_type().to_redux(){
                        ValueType::REDUX_NUMBER=>id == id!(number).into(),
                        ValueType::REDUX_NAN=>id == id!(number).into() || id == id!(nan).into(),
                        ValueType::REDUX_BOOL=>id == id!(bool).into(),
                        ValueType::REDUX_NIL=>id == id!(nan).into(),
                        ValueType::REDUX_COLOR=>id == id!(color).into(),
                        ValueType::REDUX_STRING=>id == id!(string).into(),
                        ValueType::REDUX_OBJECT=>{
                            id == id!(object).into() || {
                                if let Some(rhs) = self.resolve(id, heap).as_object(){
                                    if let Some(obj) = lhs.as_object(){
                                        heap.object_has_proto(obj, rhs.into())
                                    }
                                    else{
                                        false
                                    }
                                }
                                else{
                                    false
                                }
                            }
                        },
                        ValueType::REDUX_ID=>id == id!(id).into(),
                        _=>false
                    }
                }
                else if let Some(obj) = lhs.as_object(){
                    heap.object_has_proto(obj, rhs)
                }
                else{
                    false
                };
                self.stack.push(cmp.into());
                self.ip.index += 1;
            }
            
            opcode=>{
                println!("UNDEFINED OPCODE {}", opcode);
                self.ip.index += 1;
                // unknown instruction
            }
        }
        None
    }
    
        
    pub fn begin_for_loop_inner(&mut self, heap:&mut ScriptHeap, jump:u32, source:Value, value_id:Id, index_id:Option<Id>, key_id:Option<Id>, first_value:Value, first_index:f64, first_key:Value){    
                                               
        self.ip.index += 1;
        self.loops.push(LoopFrame{
            bases: self.new_bases(),
            start_ip: self.ip.index,
            value_id,
            key_id,
            jump,
            index_id,
            source,
            index: first_index,
        });
        // lets make a new scope object and set our first value
        let scope = *self.scopes.last().unwrap();
        let new_scope = heap.new_object_with_proto(scope.into());
        self.scopes.push(new_scope);
        // lets write our first value onto the scope
        heap.set_object_value(new_scope, value_id.into(), first_value);
        if let Some(key_id) = key_id{
            heap.set_object_value(new_scope, key_id.into(), first_key);
        }
        if let Some(index_id) = index_id{
            heap.set_object_value(new_scope, index_id.into(), first_index.into());
        }
    }
                
    pub fn begin_for_loop(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, jump:u32, source:Value, value_id:Id, index_id:Option<Id>, key_id:Option<Id>){
        let v0 = Value::from_f64(0.0);
        if let Some(s) = source.as_f64(){
            if s >= 1.0{
                self.begin_for_loop_inner(heap, jump, source, value_id, key_id, index_id, v0, 0.0, v0);
                return
            }
        }
        else if let Some(obj) = source.as_object(){
            if heap.object_has_proto(obj, code.builtins.range.into()){ // range object
                let start = heap.object_value(obj, id!(start).into(),Value::NIL).as_f64().unwrap_or(0.0);
                let end = heap.object_value(obj, id!(end).into(),Value::NIL).as_f64().unwrap_or(0.0);
                let v = start.into();
                if (start-end).abs() >= 1.0{
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, v, start, v);
                    return
                }
            }
            else{
                let object = heap.object(obj);
                if object.tag.get_type().uses_vec2() && object.vec.len() > 1{
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, object.vec[1], 0.0, object.vec[0]);
                    return 
                }
                else if object.tag.get_type().is_vec1() && object.vec.len() > 0{
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, object.vec[0], 0.0, Value::NIL);                  
                    return 
                }
            }
        }
        // jump over it and bail
        self.ip.index += jump;
    }
            
    pub fn end_for_loop(&mut self, heap:&mut ScriptHeap, code:&ScriptCode){
        // alright lets take a look at our top loop thing
        let lf = self.loops.last_mut().unwrap();
        if let Some(end) = lf.source.as_f64(){
            lf.index += 1.0;
            if lf.index >= end{ // terminate
                self.break_for_loop(heap);
                return
            }
            self.ip.index = lf.start_ip;
            let scope = self.scopes.last().unwrap();
            heap.set_object_value(*scope, lf.value_id.into(), lf.index.into());
            return
        }
        else if let Some(obj) = lf.source.as_object(){
            if heap.object_has_proto(obj, code.builtins.range.into()){ // range object
                let scope = self.scopes.last().unwrap();
                let end = heap.object_value(obj, id!(end).into(),Value::NIL).as_f64().unwrap_or(0.0);
                let step = heap.object_value(obj, id!(step).into(),Value::NIL).as_f64().unwrap_or(1.0);
                lf.index += step;
                if lf.index >= end{
                    self.break_for_loop(heap);
                    return
                } 
                heap.set_object_value(*scope, lf.value_id.into(), lf.index.into());
                self.ip.index = lf.start_ip;
                return
            }
            else{
                let object = heap.object(obj);
                if object.tag.get_type().uses_vec2(){
                    let len = object.vec.len() >> 1;
                    lf.index += 1.0;
                    if lf.index >= len as f64{
                        self.break_for_loop(heap);
                        return
                    }
                    let scope = self.scopes.pop().unwrap();
                    let value = object.vec[lf.index as usize * 2 + 1];
                    let key = if lf.key_id.is_some(){
                        object.vec[lf.index as usize * 2]
                    }else{Value::NIL};
                    let scope = heap.new_object_if_reffed(scope);
                    heap.set_object_value(scope, lf.value_id.into(), value.into());
                    if let Some(index_id) = lf.index_id{
                        heap.set_object_value(scope, index_id.into(), lf.index.into());
                    }
                    if let Some(key_id) = lf.key_id{
                        heap.set_object_value(scope, key_id.into(), key);
                    }
                    self.scopes.push(scope);
                    self.ip.index = lf.start_ip;
                    return                    
                }
                else if object.tag.get_type().is_vec1() && object.vec.len() > 0{
                    let len = object.vec.len();
                    lf.index += 1.0;
                    if lf.index >= len as f64{
                        self.break_for_loop(heap);
                        return
                    }
                    self.ip.index = lf.start_ip;
                    let scope = self.scopes.pop().unwrap();
                    let value = object.vec[lf.index as usize];
                    let scope = heap.new_object_if_reffed(scope);
                    heap.set_object_value(scope, lf.value_id.into(), value.into());
                    self.scopes.push(scope);
                    return                    
                }
            }
        }
        println!("For end unknown state");
        self.ip.index += 1;
    }
                    
    pub fn break_for_loop(&mut self, heap:&mut ScriptHeap){
        let lp = self.loops.pop().unwrap();
        if let Some(obj) = lp.source.as_object(){
            heap.free_object_if_unreffed(obj);
        }
        self.truncate_bases(lp.bases, heap);
        self.ip.index = lp.start_ip + lp.jump - 1;
    }
}
