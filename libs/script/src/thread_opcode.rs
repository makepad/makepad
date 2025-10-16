use crate::makepad_value::id::*;
use crate::heap::*;
use crate::makepad_value::value::*;
use crate::makepad_value::opcode::*;
use crate::makepad_value_derive::*;
use crate::object::*;
use crate::script::*;
use crate::thread::*;
use std::any::Any;

macro_rules! f64_scope_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let id = $obj.pop_stack_value();
        if let Some(id) = id.as_id(){
            let va = $obj.scope_value($heap, id);
            if va.is_err(){
                $obj.push_stack_value_nc(va);
            }
            else{
                let fa = $heap.cast_to_f64(va, $obj.ip);
                let fb = $heap.cast_to_f64(value, $obj.ip);
                $obj.set_scope_value($heap, id, Value::from_f64_traced_nan((fa $op fb), $obj.ip));
                $obj.push_stack_value_nc(NIL);
            }
        }
        else{
            $obj.push_stack_value_nc(Value::err_notassignable($obj.ip));
        }
        $obj.ip.index += 1;
    }}
}

macro_rules! fu64_scope_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let id = $obj.pop_stack_value();
        if let Some(id) = id.as_id(){
            let va = $obj.scope_value($heap, id);
            if va.is_err(){
                $obj.push_stack_value_nc(va);
            }
            else{
                let ua = $heap.cast_to_f64(va, $obj.ip) as u64;
                let ub = $heap.cast_to_f64(value, $obj.ip) as u64;
                $obj.set_scope_value($heap, id, Value::from_f64_traced_nan((ua $op ub) as f64, $obj.ip));
                $obj.push_stack_value_nc(NIL);
            }
        }
        else{
            $obj.push_stack_value_nc(Value::err_notassignable($obj.ip));
        }
        $obj.ip.index += 1;
    }}
}

macro_rules! f64_field_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let field = $obj.pop_stack_value();
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, field, Value::err_notfield($obj.ip));
            let fa = $heap.cast_to_f64(old_value, $obj.ip);
            let fb = $heap.cast_to_f64(value, $obj.ip);
            
            $heap.set_value(obj, field, Value::from_f64_traced_nan(fa $op fb, $obj.ip));
            
            $obj.push_stack_value_nc(NIL);
        }
        else{
            $obj.push_stack_value_nc(Value::err_notassignable($obj.ip));
        }
        $obj.ip.index += 1;
    }}
}

macro_rules! fu64_field_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let field = $obj.pop_stack_value();
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, field, Value::err_notfield($obj.ip));
            let fa = $heap.cast_to_f64(old_value, $obj.ip) as u64;
            let fb = $heap.cast_to_f64(value, $obj.ip) as u64;
                        
            $heap.set_value(obj, field, Value::from_f64_traced_nan((fa $op fb) as f64, $obj.ip));
                        
            $obj.push_stack_value_nc(NIL);
        }
        else{
            $obj.push_stack_value_nc(Value::err_notassignable($obj.ip));
        }
        $obj.ip.index += 1;
    }}
}

macro_rules! f64_index_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let index = $obj.pop_stack_resolved($heap);
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, index, Value::err_notindex($obj.ip));
            let fa = $heap.cast_to_f64(old_value, $obj.ip);
            let fb = $heap.cast_to_f64(value, $obj.ip);
                        
            $heap.set_value(obj, index, Value::from_f64_traced_nan(fa $op fb, $obj.ip));
                        
            $obj.push_stack_value_nc(NIL);
        }
        else{
            $obj.push_stack_value_nc(Value::err_notassignable($obj.ip));
        }
        $obj.ip.index += 1;
    }}
}

macro_rules! fu64_index_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let index = $obj.pop_stack_resolved($heap);
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, index, Value::err_notindex($obj.ip));
            let fa = $heap.cast_to_f64(old_value, $obj.ip) as u64;
            let fb = $heap.cast_to_f64(value, $obj.ip) as u64;
                                    
            $heap.set_value(obj, index, Value::from_f64_traced_nan((fa $op fb) as f64, $obj.ip));
                                    
            $obj.push_stack_value_nc(NIL);
        }
        else{
            $obj.push_stack_value_nc(Value::err_notassignable($obj.ip));
        }
        $obj.ip.index += 1;
    }}
}

macro_rules! f64_op_impl{
    ($obj:ident, $heap:ident, $args:ident, $op:tt)=>{{
        let fb = if $args.is_u32(){
            $args.to_u32() as f64
        }
        else{
            let b = $obj.pop_stack_resolved($heap);
            $heap.cast_to_f64(b, $obj.ip)
        };
        let a = $obj.pop_stack_resolved($heap);
        let fa = $heap.cast_to_f64(a, $obj.ip);
        $obj.push_stack_value_nc(Value::from_f64_traced_nan(fa $op fb, $obj.ip));
        $obj.ip.index += 1;
    }}
}

macro_rules! fu64_op_impl{
    ($obj:ident, $heap:ident, $args:ident, $op:tt)=>{{
        let ub = if $args.is_u32(){
            $args.to_u32() as u64
        }
        else{
            let b = $obj.pop_stack_resolved($heap);
            $heap.cast_to_f64(b, $obj.ip) as u64
        };
        let a = $obj.pop_stack_resolved($heap);
        let ua = $heap.cast_to_f64(a, $obj.ip) as u64;
        $obj.push_stack_value_nc(Value::from_f64_traced_nan((ua $op ub) as f64, $obj.ip));
        $obj.ip.index += 1;
    }}
} 

macro_rules! f64_cmp_impl{
    ($obj:ident, $heap:ident, $args:ident, $op:tt)=>{{
        let fb = if $args.is_u32(){
            $args.to_u32() as f64
        }
        else{
            let b = $obj.pop_stack_resolved($heap);
            $heap.cast_to_f64(b, $obj.ip)
        };
        let a = $obj.pop_stack_resolved($heap);
        let fa = $heap.cast_to_f64(a, $obj.ip);
        //let fb = $heap.cast_to_f64(b, $obj.ip);
        $obj.push_stack_value_nc(Value::from_bool(fa $op fb));
        $obj.ip.index += 1;
    }}
}

macro_rules! bool_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let b = $obj.pop_stack_resolved($heap);
        let a = $obj.pop_stack_resolved($heap);
        let ba = $heap.cast_to_bool(a);
        let bb = $heap.cast_to_bool(b);
        $obj.push_stack_value_nc(Value::from_bool((ba $op bb)));
        $obj.ip.index += 1;
    }}
} 

impl ScriptThread{
    
    pub fn opcode(&mut self,opcode: Opcode, args:OpcodeArgs, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any){
        
        match opcode{
            Opcode::NOT=>{
                let value = self.pop_stack_resolved(heap);
                if let Some(v) = value.as_f64(){
                    self.push_stack_value_nc(Value::from_f64(!(v as u64) as f64));
                    self.ip.index += 1;
                }
                else{
                    let v = heap.cast_to_bool(value);
                    self.push_stack_value_nc(Value::from_bool(!v));
                }
            },
            Opcode::NEG=>{
                let v = heap.cast_to_f64(self.pop_stack_resolved(heap), self.ip);
                self.push_stack_value_nc(Value::from_f64(-v));
                self.ip.index += 1;
            },
            
            Opcode::MUL=>f64_op_impl!(self, heap, args, *),
            Opcode::DIV=>f64_op_impl!(self, heap, args, /),
            Opcode::MOD=>f64_op_impl!(self, heap, args, %),
            Opcode::ADD=>f64_op_impl!(self, heap, args, +),
            Opcode::SUB=>f64_op_impl!(self, heap, args, -),
            Opcode::SHL=>fu64_op_impl!(self, heap, args,>>),
            Opcode::SHR=>fu64_op_impl!(self, heap, args,<<),
            Opcode::AND=>fu64_op_impl!(self, heap,args,&),
            Opcode::OR=>fu64_op_impl!(self, heap, args,|),
            Opcode::XOR=>fu64_op_impl!(self, heap, args,^),
                                
            Opcode::CONCAT=>{
                let op1 = self.pop_stack_resolved(heap);
                let op2 = self.pop_stack_resolved(heap);
                let ptr = heap.new_string_with(|heap, out|{
                    heap.cast_to_string(op1, out);
                    heap.cast_to_string(op2, out);
                });
                self.push_stack_value_nc(ptr.into());
                self.ip.index += 1;
            }
            Opcode::EQ=> {
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_value_nc(heap.deep_eq(a, b).into());
                self.ip.index += 1;
            }
            Opcode::NEQ=> {
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_value_nc((!heap.deep_eq(a, b)).into());
                self.ip.index += 1;
            }
            
            Opcode::LT=>f64_cmp_impl!(self, heap, args, <),
            Opcode::GT=>f64_cmp_impl!(self, heap, args, >),
            Opcode::LEQ=>f64_cmp_impl!(self, heap, args, <=),
            Opcode::GEQ=>f64_cmp_impl!(self, heap, args, >=),
            
            Opcode::LOGIC_AND => bool_op_impl!(self, heap, &&),
            Opcode::LOGIC_OR => bool_op_impl!(self, heap, ||),
            Opcode::NIL_OR => {
                let op1 = self.pop_stack_resolved(heap);
                let op2 = self.pop_stack_resolved(heap);
                if op1.is_nil(){
                    self.push_stack_value_nc(op2);
                }
                else{
                    self.push_stack_value_nc(op1);
                }
                self.ip.index += 1;
            }
            Opcode::SHALLOW_EQ =>{
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_value(heap.shallow_eq(a, b).into());
                self.ip.index += 1;
            }
            Opcode::SHALLOW_NEQ=>{
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_value_nc((!heap.shallow_eq(a, b)).into());
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_ME=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.set_value(self.mes.last().unwrap().object, field, value);
                if !args.is_statement(){
                    self.push_stack_value_nc(NIL);
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.insert_value_at(self.mes.last().unwrap().object, field, value, opcode == Opcode::ASSIGN_ME_BEFORE);
                if !args.is_statement(){
                    self.push_stack_value_nc(NIL);
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_ME_BEGIN=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                heap.insert_value_begin(self.mes.last().unwrap().object, field, value);
                if !args.is_statement(){
                    self.push_stack_value_nc(NIL);
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN=>{
            }
            
            Opcode::ASSIGN_ADD=>f64_scope_assign_op_impl!(self, heap, +),
            Opcode::ASSIGN_SUB=>f64_scope_assign_op_impl!(self, heap, -),
            Opcode::ASSIGN_MUL=>f64_scope_assign_op_impl!(self, heap, *),
            Opcode::ASSIGN_DIV=>f64_scope_assign_op_impl!(self, heap, /),
            Opcode::ASSIGN_MOD=>f64_scope_assign_op_impl!(self, heap, %),
            Opcode::ASSIGN_AND=>fu64_scope_assign_op_impl!(self, heap, &),
            Opcode::ASSIGN_OR=>fu64_scope_assign_op_impl!(self, heap, |),
            Opcode::ASSIGN_XOR=>fu64_scope_assign_op_impl!(self, heap, ^),
            Opcode::ASSIGN_SHL=>fu64_scope_assign_op_impl!(self, heap, <<),
            Opcode::ASSIGN_SHR=>fu64_scope_assign_op_impl!(self, heap, >>),
            Opcode::ASSIGN_IFNIL=>{
                let value = self.pop_stack_resolved(heap);
                let id = self.pop_stack_value();
                if let Some(id) = id.as_id(){
                    let va = self.scope_value(heap, id);
                    if va.is_err() || va.is_nil(){
                        self.set_scope_value(heap, id, value);
                    }
                    self.push_stack_value_nc(NIL);
                }
                else{
                    self.push_stack_value_nc(Value::err_notassignable(self.ip));
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_FIELD=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    heap.set_value(obj, field, value);
                    self.push_stack_value_nc(value);
                }
                else{
                    self.push_stack_value_nc(Value::err_notobject(self.ip));
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_FIELD_ADD=>f64_field_assign_op_impl!(self, heap, +),
            Opcode::ASSIGN_FIELD_SUB=>f64_field_assign_op_impl!(self, heap, -),
            Opcode::ASSIGN_FIELD_MUL=>f64_field_assign_op_impl!(self, heap, *),
            Opcode::ASSIGN_FIELD_DIV=>f64_field_assign_op_impl!(self, heap, /),
            Opcode::ASSIGN_FIELD_MOD=>f64_field_assign_op_impl!(self, heap, %),
            Opcode::ASSIGN_FIELD_AND=>fu64_field_assign_op_impl!(self, heap, &),
            Opcode::ASSIGN_FIELD_OR=>fu64_field_assign_op_impl!(self, heap, |),
            Opcode::ASSIGN_FIELD_XOR=>fu64_field_assign_op_impl!(self, heap, ^),
            Opcode::ASSIGN_FIELD_SHL=>fu64_field_assign_op_impl!(self, heap, <<),
            Opcode::ASSIGN_FIELD_SHR=>fu64_field_assign_op_impl!(self, heap, >>),
            Opcode::ASSIGN_FIELD_IFNIL=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let old_value = heap.value(obj, field, Value::err_notfield(self.ip));
                    if old_value.is_err() || old_value.is_nil(){
                        heap.set_value(obj, field, value);
                    }
                    self.push_stack_value_nc(NIL);
                }
                else{
                    self.push_stack_value_nc(Value::err_notobject(self.ip));
                }
                self.ip.index += 1;
            }
            
            Opcode::ASSIGN_INDEX=>{
                let value = self.pop_stack_resolved(heap);
                let index = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    heap.set_value(obj, index, value);
                    self.push_stack_value_nc(value);
                }
                else{
                    self.push_stack_value_nc(Value::err_notobject(self.ip));
                }
                self.ip.index += 1;
            }
            Opcode::ASSIGN_INDEX_ADD=>f64_index_assign_op_impl!(self, heap, +),
            Opcode::ASSIGN_INDEX_SUB=>f64_index_assign_op_impl!(self, heap, -),
            Opcode::ASSIGN_INDEX_MUL=>f64_index_assign_op_impl!(self, heap, *),
            Opcode::ASSIGN_INDEX_DIV=>f64_index_assign_op_impl!(self, heap, /),
            Opcode::ASSIGN_INDEX_MOD=>f64_index_assign_op_impl!(self, heap, %),
            Opcode::ASSIGN_INDEX_AND=>fu64_index_assign_op_impl!(self, heap, &),
            Opcode::ASSIGN_INDEX_OR=>fu64_index_assign_op_impl!(self, heap, |),
            Opcode::ASSIGN_INDEX_XOR=>fu64_index_assign_op_impl!(self, heap, ^),
            Opcode::ASSIGN_INDEX_SHL=>fu64_index_assign_op_impl!(self, heap, <<),
            Opcode::ASSIGN_INDEX_SHR=>fu64_index_assign_op_impl!(self, heap, >>),
            Opcode::ASSIGN_INDEX_IFNIL=>{
                let value = self.pop_stack_resolved(heap);
                let index = self.pop_stack_resolved(heap);
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let old_value = heap.value(obj, index, Value::err_notindex(self.ip));
                    if old_value.is_err() || old_value.is_nil(){
                        heap.set_value(obj, index, value);
                    }
                    self.push_stack_value_nc(NIL);
                }
                else{
                    self.push_stack_value_nc(Value::err_notobject(self.ip));
                }
                self.ip.index += 1;
            }
            
            Opcode::BEGIN_PROTO=>{
                let proto = self.pop_stack_resolved(heap);
                let me = heap.new_with_proto(proto);
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
            Opcode::BEGIN_PROTO_ME=>{
                let field = self.peek_stack_value();
                let me = self.mes.last().unwrap();
                let proto = heap.value(me.object, field, NIL);
                let me = heap.new_with_proto(proto);
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
            Opcode::END_PROTO=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value_nc(me.object.into());
                self.ip.index += 1;
            }
            Opcode::BEGIN_BARE=>{ // bare object
                let me = heap.new(0);
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
            Opcode::END_BARE=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value_nc(me.object.into());
                self.ip.index += 1;
            }
            Opcode::BEGIN_ARRAY=>{
                let me = heap.new(0);
                self.mes.push(ScriptMe::array(me));
                self.ip.index += 1;
            }
            Opcode::END_ARRAY=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_value_nc(me.object.into());
                self.ip.index += 1;
            }
            
            Opcode::CALL_ARGS=>{
                let fnobj = self.pop_stack_resolved(heap);
                let scope = heap.new_with_proto(fnobj);
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
                                
                if let Some(fnptr) = heap.parent_as_fn(scope){
                    match fnptr{
                        ScriptFnPtr::Native(ni)=>{
                            let ip = self.ip;
                            let ret = (*code.native.fn_table[ni.index as usize].fn_ptr)(&mut ScriptCtx{
                                host,
                                heap,
                                thread:self,
                                code
                            }, scope);
                            self.ip = ip;
                            self.push_stack_value(ret);
                            heap.free_object_if_unreffed(scope);
                            self.ip.index += 1;
                        }
                        ScriptFnPtr::Script(sip)=>{
                            let call = CallFrame{
                                bases: self.new_bases(),
                                args: args,
                                return_ip: Some(ScriptIp{index: self.ip.index + 1, body:self.ip.body})
                            };
                            self.scopes.push(scope);
                            self.calls.push(call);
                            self.ip = sip;
                        }
                    }
                }
                else{
                    self.push_stack_value_nc(Value::err_notfn(self.ip));
                    self.ip.index += 1;
                }
            }
            Opcode::METHOD_CALL_ARGS=>{
                let method =  self.pop_stack_value();
                let this = self.pop_stack_resolved(heap);
                let fnobj = if let Some(obj) = this.as_object(){
                    heap.object_method(obj, method, NIL)
                }
                else{ // we're calling a method on some other thing
                    NIL
                };
                let scope = if fnobj == NIL{
                    // lets take the type
                    let type_index = this.value_type().to_redux();
                    let method = method.as_id().unwrap_or(id!());
                    let type_entry = &code.type_methods.type_table[type_index];
                    
                    if let Some(method_ptr) = type_entry.get(&method){
                        let scope = heap.new_with_proto((*method_ptr).into());
                        scope
                    }
                    else{ 
                        heap.new_with_proto(id!(undefined_function).into())
                    }
                }
                else{
                    heap.new_with_proto(fnobj)
                };
                //heap.set_object_map(scope);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                heap.set_value_in_map(scope, id!(this).into(), this.into());
                self.mes.push(ScriptMe::call(scope));
                self.ip.index += 1;
            }
            
            Opcode::FN_ARGS=>{
                let scope = *self.scopes.last_mut().unwrap();
                let me = heap.new_with_proto(scope.into());
                                
                // set it to a vec type to ensure ordered inserts
                heap.set_object_type(me, ObjectType::VEC2);
                heap.clear_object_deep(me);
                                                
                self.mes.push(ScriptMe::object(me));
                self.ip.index += 1;
            }
                                    
            Opcode::FN_ARG_DYN=>{
                let value = if args.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                heap.set_value(self.mes.last().unwrap().object, id.into(), value);
                self.ip.index += 1;                
            }
            Opcode::FN_ARG_TYPED=>{
                let value = if args.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let _ty = self.pop_stack_value().as_id().unwrap_or(id!());
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                heap.set_value(self.mes.last().unwrap().object, id.into(), value);
                self.ip.index += 1;
            }
            Opcode::FN_BODY=>{ // alright we have all the args now we get an expression
                let jump_over_fn = args.to_u32();
                let me = self.mes.pop().unwrap();
                                
                heap.set_fn(me.object, ScriptFnPtr::Script(
                    ScriptIp{body: self.ip.body, index:(self.ip.index + 1)}
                ));
                self.ip.index += jump_over_fn;
                self.push_stack_value_nc(me.object.into());
            }
            Opcode::RETURN=>{
                let value = if args.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let call = self.calls.pop().unwrap();
                self.truncate_bases(call.bases, heap);
                
                if let Some(ret) = call.return_ip{
                    self.ip = ret;
                    self.push_stack_value_nc(value);
                    if call.args.is_pop_to_me(){
                        self.pop_to_me(heap);
                    }
                }
                else{
                    self.trap = Some(ScriptTrap::Return(value));
                }
            }
            Opcode::RETURN_IF_ERR=>{
                let value = self.peek_stack_resolved(heap);
                if value.is_err(){
                    let call = self.calls.pop().unwrap();
                    self.truncate_bases(call.bases, heap);
                    if let Some(ret) = call.return_ip{
                        self.ip = ret;
                        self.push_stack_value_nc(value);
                        if call.args.is_pop_to_me(){
                            self.pop_to_me(heap);
                        }
                    }
                    else{
                        self.trap = Some(ScriptTrap::Return(value));
                    }
                }
                else{
                    self.ip.index += 1
                }
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
                    self.push_stack_value_nc(heap.value(obj, field, Value::err_notfield(self.ip)))
                }
                else{
                    self.push_stack_value_nc(Value::err_notobject(self.ip));
                }
                self.ip.index += 1;
            }
            Opcode::FIELD_NIL=>{
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value_nc(heap.value(obj, field, NIL))
                }
                else{
                    self.push_stack_value_nc(NIL);
                }
                self.ip.index += 1;
            }
            Opcode::ME_FIELD=>{
                let field = self.pop_stack_value();
                self.push_stack_value(heap.value(self.mes.last().unwrap().object, field,NIL));
                self.ip.index += 1;
            }
            Opcode::PROTO_FIELD=>{ // implement proto field!
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value_nc(heap.value(obj, field, Value::err_notfield(self.ip)))
                }
                else{
                    self.push_stack_value_nc(Value::err_notobject(self.ip));
                }
                self.ip.index += 1;
            }
            
            Opcode::POP_TO_ME=>{
                self.pop_to_me(heap);
                self.ip.index += 1;
            }
            
            Opcode::ARRAY_INDEX=>{
                let index = self.pop_stack_resolved(heap);
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    self.push_stack_value_nc(heap.value(obj, index,Value::err_notindex(self.ip)))
                }
                else{
                    self.push_stack_value_nc(Value::err_notobject(self.ip));
                }
                self.ip.index += 1;
            }
                   
            Opcode::LET_DYN=>{
                let value = if args.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                let scope = *self.scopes.last_mut().unwrap();
                heap.set_value(scope, id.into(), value);
                self.ip.index += 1;
            }
            Opcode::LET_TYPED=>{
                let value = if args.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let _ty = self.pop_stack_value();
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                let scope = *self.scopes.last_mut().unwrap();
                heap.set_value(scope, id.into(), value);
                self.ip.index += 1;
            } 
            
            Opcode::SEARCH_TREE=>{
                self.ip.index += 1;
            }
                                  
            Opcode::LOG=>{
                
                if let Some(loc) = code.ip_to_loc(self.ip){
                    let value = self.peek_stack_resolved(heap);
                    if value != NIL{
                        if let Some(err) = value.as_err(){
                            if let Some(loc2) = code.ip_to_loc(err.ip){
                                println!("{} {} {}", loc, value, loc2);
                            }
                        }
                        else if let Some(obj) = value.as_object(){
                            print!("{} ", loc);
                            heap.print(obj, true);
                            println!("");
                        }
                        else if let Some(nanip) = value.as_f64_traced_nan(){
                            if let Some(loc2) = code.ip_to_loc(nanip){
                                println!("{} NaN Traced to {}", loc, loc2);
                            }
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
                    self.push_stack_value(NIL);
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
            Opcode::LOOP=>{
                self.begin_loop(heap, args.to_u32() as _);
            }
            Opcode::FOR_END=>{
                self.end_for_loop(heap, code);
            }
            Opcode::BREAK=>{
                self.break_for_loop(heap);
            }
            Opcode::BREAKIFNOT=>{
                let value = self.pop_stack_resolved(heap);
                if !heap.cast_to_bool(value){
                    self.break_for_loop(heap);
                }
                else{
                    self.ip.index += 1;
                }
            }
            Opcode::CONTINUE=>{
                self.end_for_loop(heap, code);
            }
            Opcode::RANGE=>{
                let end = self.pop_stack_resolved(heap);
                let start = self.pop_stack_resolved(heap);
                let range = heap.new_with_proto(code.builtins.range.into());
                heap.set_value(range, id!(start).into(), start);
                heap.set_value(range, id!(end).into(), end);
                self.push_stack_value_nc(range.into());
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
                                if let Some(rhs) = self.scope_value(heap,id).as_object(){
                                    if let Some(obj) = lhs.as_object(){
                                        heap.has_proto(obj, rhs.into())
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
                    heap.has_proto(obj, rhs)
                }
                else{
                    false
                };
                self.push_stack_value_nc(cmp.into());
                self.ip.index += 1;
            }
            
            opcode=>{
                println!("UNDEFINED OPCODE {}", opcode);
                self.ip.index += 1;
                // unknown instruction
            }
        }
        if args.is_pop_to_me(){
            self.pop_to_me(heap);
        }
    }
    
    pub fn pop_to_me(&mut self, heap:&mut ScriptHeap){
        let value = self.pop_stack_value();
        if self.call_has_me(){
            let me = self.mes.last().unwrap();
            let (key, value) = if let Some(id) = value.as_id(){
                if value.is_escaped_id(){ (NIL, value) }
                else{(value, self.scope_value(heap, id))}
            }else{(NIL,value)};
            if me.ty == ScriptMe::CALL{
                heap.push_fn_arg(me.object, value);       
            }
            else if me.ty == ScriptMe::OBJ{
                if !value.is_nil() && !value.is_err(){
                    heap.push_value(me.object, key, value);       
                }
            }
            else{
                heap.push_value(me.object, NIL, value);       
            }
        }
    }
    
    pub fn begin_for_loop_inner(&mut self, heap:&mut ScriptHeap, jump:u32, source:Value, value_id:Id, index_id:Option<Id>, key_id:Option<Id>, first_value:Value, first_index:f64, first_key:Value){    
                                               
        self.ip.index += 1;
        self.loops.push(LoopFrame{
            bases: self.new_bases(),
            start_ip: self.ip.index,
            values: Some(LoopValues{
                value_id,
                key_id,
                index_id,
                source,
                index: first_index,
            }),
            jump,
        });
        // lets make a new scope object and set our first value
        let scope = *self.scopes.last().unwrap();
        let new_scope = heap.new_with_proto(scope.into());

        self.scopes.push(new_scope);
        // lets write our first value onto the scope
        heap.set_value(new_scope, value_id.into(), first_value);
        if let Some(key_id) = key_id{
            heap.set_value(new_scope, key_id.into(), first_key);
        }
        if let Some(index_id) = index_id{
            heap.set_value(new_scope, index_id.into(), first_index.into());
        }
    }
    
    pub fn begin_loop(&mut self, heap:&mut ScriptHeap, jump:u32){   
        self.ip.index += 1;
        self.loops.push(LoopFrame{
            bases: self.new_bases(),
            start_ip: self.ip.index,
            values: None,
            jump,
        });
        // lets make a new scope object and set our first value
        let scope = *self.scopes.last().unwrap();
        let new_scope = heap.new_with_proto(scope.into());
        self.scopes.push(new_scope);
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
            if heap.has_proto(obj, code.builtins.range.into()){ // range object
                let start = heap.value(obj, id!(start).into(),NIL).as_f64().unwrap_or(0.0);
                let end = heap.value(obj, id!(end).into(),NIL).as_f64().unwrap_or(0.0);
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
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, object.vec[0], 0.0, NIL);                  
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
        if let Some(values) = &mut lf.values{
            if let Some(end) = values.source.as_f64(){
                values.index += 1.0;
                if values.index >= end{ // terminate
                    self.break_for_loop(heap);
                    return
                }
                self.ip.index = lf.start_ip;
                let scope = self.scopes.last().unwrap();
                heap.set_value(*scope, values.value_id.into(), values.index.into());
                return
            }
            else if let Some(obj) = values.source.as_object(){
                if heap.has_proto(obj, code.builtins.range.into()){ // range object
                    let scope = self.scopes.last().unwrap();
                    let end = heap.value(obj, id!(end).into(),NIL).as_f64().unwrap_or(0.0);
                    let step = heap.value(obj, id!(step).into(),NIL).as_f64().unwrap_or(1.0);
                    values.index += step;
                    if values.index >= end{
                        self.break_for_loop(heap);
                        return
                    } 
                    heap.set_value(*scope, values.value_id.into(), values.index.into());
                    self.ip.index = lf.start_ip;
                    return
                }
                else{
                    let object = heap.object(obj);
                    if object.tag.get_type().uses_vec2(){
                        let len = object.vec.len() >> 1;
                        values.index += 1.0;
                        if values.index >= len as f64{
                            self.break_for_loop(heap);
                            return
                        }
                        let scope = self.scopes.pop().unwrap();
                        let value = object.vec[values.index as usize * 2 + 1];
                        let key = if values.key_id.is_some(){
                            object.vec[values.index as usize * 2]
                        }else{NIL};
                        
                        let scope = heap.new_if_reffed(scope);
                        heap.set_value(scope, values.value_id.into(), value.into());
                        if let Some(index_id) = values.index_id{
                            heap.set_value(scope, index_id.into(), values.index.into());
                        }
                        if let Some(key_id) = values.key_id{
                            heap.set_value(scope, key_id.into(), key);
                        }
                        self.scopes.push(scope);
                        
                        self.ip.index = lf.start_ip;
                        return                    
                    }
                    else if object.tag.get_type().is_vec1() && object.vec.len() > 0{
                        let len = object.vec.len();
                        values.index += 1.0;
                        if values.index >= len as f64{
                            self.break_for_loop(heap);
                            return
                        }
                        self.ip.index = lf.start_ip;
                        let scope = self.scopes.pop().unwrap();
                        let value = object.vec[values.index as usize];
                        let scope = heap.new_if_reffed(scope);
                        heap.set_value(scope, values.value_id.into(), value.into());
                        self.scopes.push(scope);
                        return                    
                    }
                }
            }
        }
        else{ // we are a loop
            self.ip.index = lf.start_ip;
            return
        }
        println!("For end unknown state");
        self.ip.index += 1;
    }
                    
    pub fn break_for_loop(&mut self, heap:&mut ScriptHeap){
        let lp = self.loops.pop().unwrap();
        if let Some(values) = lp.values{
            if let Some(obj) = values.source.as_object(){
                heap.free_object_if_unreffed(obj);
            }
        }            
        self.truncate_bases(lp.bases, heap);
        self.ip.index = lp.start_ip + lp.jump - 1;
    }
}
