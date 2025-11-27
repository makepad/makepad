use crate::makepad_live_id::*;
use crate::makepad_error_log::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::function::*;
use crate::vm::*;
use crate::thread::*;
use crate::trap::*;
use crate::pod::*;
use std::any::Any;

    
impl ScriptThread{
    
    pub fn opcode(&mut self,opcode: Opcode, opargs:OpcodeArgs, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any){
        
        match opcode{
            
// ARITHMETIC            
            Opcode::NOT=>{
                let value = self.pop_stack_resolved(heap);
                if let Some(v) = value.as_f64(){
                    self.push_stack_unchecked(ScriptValue::from_f64(!(v as u64) as f64));
                    self.trap.goto_next();
                }
                else{
                    let v = heap.cast_to_bool(value);
                    self.push_stack_unchecked(ScriptValue::from_bool(!v));
                }
            },
            Opcode::NEG=>{
                let v = heap.cast_to_f64(self.pop_stack_resolved(heap), self.trap.ip);
                self.push_stack_unchecked(ScriptValue::from_f64(-v));
                self.trap.goto_next();
            },
            
            Opcode::MUL=>self.handle_f64_op(heap, opargs, |a,b| a*b),
            Opcode::DIV=>self.handle_f64_op(heap, opargs, |a,b| a/b),
            Opcode::MOD=>self.handle_f64_op(heap, opargs, |a,b| a%b),
            Opcode::ADD=>{
                let b = if opargs.is_u32(){
                    (opargs.to_u32()).into()
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let a = self.pop_stack_resolved(heap);
                if a.is_string_like() || b.is_string_like(){
                    let ptr = heap.new_string_with(|heap, out|{
                        heap.cast_to_string(a, out);
                        heap.cast_to_string(b, out);
                    });
                    self.push_stack_unchecked(ptr.into());
                }
                else{
                    let fa = heap.cast_to_f64(a, self.trap.ip);
                    let fb = heap.cast_to_f64(b, self.trap.ip);
                    self.push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa + fb, self.trap.ip));
                }
                self.trap.goto_next();
            }
                        
            Opcode::SUB=>self.handle_f64_op(heap, opargs, |a,b| a-b),
            Opcode::SHL=>self.handle_fu64_op(heap, opargs, |a,b| a>>b),
            Opcode::SHR=>self.handle_fu64_op(heap, opargs, |a,b| a<<b),
            Opcode::AND=>self.handle_fu64_op(heap, opargs, |a,b| a&b),
            Opcode::OR=>self.handle_fu64_op(heap, opargs, |a,b| a|b),
            Opcode::XOR=>self.handle_fu64_op(heap, opargs, |a,b| a^b),
            
// ASSIGN
            Opcode::ASSIGN=>{
                let value = self.pop_stack_resolved(heap);
                let id = self.pop_stack_value();
                if let Some(id) = id.as_id(){
                    let value = self.set_scope_value(heap, id, value);
                    self.push_stack_unchecked(value);
                }
                else{
                    let value = self.trap.err_not_assignable();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
            
            Opcode::ASSIGN_ADD=>{
                
                let value = self.pop_stack_resolved(heap);
                let id = self.pop_stack_value();
                if let Some(id) = id.as_id(){
                    let old_value = self.scope_value(heap, id);
                    if old_value.is_err(){
                        self.push_stack_unchecked(old_value);
                    }
                    else if old_value.is_string_like() || value.is_string_like(){
                        let str = heap.new_string_with(|heap, out|{
                            heap.cast_to_string(old_value, out);
                            heap.cast_to_string(value, out);
                        });
                        self.set_scope_value(heap, id, str.into());
                        self.push_stack_unchecked(NIL);
                    }
                    else{
                        let fa = heap.cast_to_f64(old_value, self.trap.ip);
                        let fb = heap.cast_to_f64(value,self.trap.ip);
                        let value = self.set_scope_value(heap, id, ScriptValue::from_f64_traced_nan(fa + fb, self.trap.ip));
                        self.push_stack_unchecked(value);
                    }
                }
                else{
                    let value = self.trap.err_not_assignable();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
            
                        
            Opcode::ASSIGN_SUB=>self.handle_f64_scope_assign_op(heap, |a,b| a-b),
            Opcode::ASSIGN_MUL=>self.handle_f64_scope_assign_op(heap, |a,b| a*b),
            Opcode::ASSIGN_DIV=>self.handle_f64_scope_assign_op(heap, |a,b| a/b),
            Opcode::ASSIGN_MOD=>self.handle_f64_scope_assign_op(heap, |a,b| a%b),
            Opcode::ASSIGN_AND=>self.handle_fu64_scope_assign_op(heap, |a,b| a&b),
            Opcode::ASSIGN_OR=>self.handle_fu64_scope_assign_op(heap, |a,b| a|b),
            Opcode::ASSIGN_XOR=>self.handle_fu64_scope_assign_op(heap, |a,b| a^b),
            Opcode::ASSIGN_SHL=>self.handle_fu64_scope_assign_op(heap, |a,b| a<<b),
            Opcode::ASSIGN_SHR=>self.handle_fu64_scope_assign_op(heap, |a,b| a>>b),

            Opcode::ASSIGN_IFNIL=>{
                let value = self.pop_stack_resolved(heap);
                let id = self.pop_stack_value();
                if let Some(id) = id.as_id(){
                    let va = self.scope_value(heap, id);
                    if va.is_err() || va.is_nil(){
                        let value = self.set_scope_value(heap, id, value);
                        self.push_stack_unchecked(value);
                    }
                    else{
                        self.push_stack_unchecked(NIL);
                    }
                }
                else{
                    let value = self.trap.err_not_assignable();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
// ASSIGN FIELD                       
            Opcode::ASSIGN_FIELD=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let value = heap.set_value(obj, field, value, &self.trap);
                    self.push_stack_unchecked(value);
                }
                else{
                    let value = self.trap.err_not_object();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
            Opcode::ASSIGN_FIELD_ADD=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let old_value = heap.value(obj, field, &self.trap);
                    if old_value.is_string_like() || value.is_string_like(){
                        let str = heap.new_string_with(|heap, out|{
                            heap.cast_to_string(old_value, out);
                            heap.cast_to_string(value, out);
                        });
                        let value = heap.set_value(obj, field, str.into(), &self.trap);
                        self.push_stack_unchecked(value);
                    }
                    else{
                        let fa = heap.cast_to_f64(old_value, self.trap.ip);
                        let fb = heap.cast_to_f64(value, self.trap.ip);
                        let value = heap.set_value(obj, field, ScriptValue::from_f64_traced_nan(fa + fb, self.trap.ip), &mut self.trap);
                        self.push_stack_unchecked(value);
                    }
                }
                else{
                    let value = self.trap.err_not_assignable();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }            
            Opcode::ASSIGN_FIELD_SUB=>self.handle_f64_field_assign_op(heap, |a,b| a-b),
            Opcode::ASSIGN_FIELD_MUL=>self.handle_f64_field_assign_op(heap, |a,b| a*b),
            Opcode::ASSIGN_FIELD_DIV=>self.handle_f64_field_assign_op(heap, |a,b| a/b),
            Opcode::ASSIGN_FIELD_MOD=>self.handle_f64_field_assign_op(heap, |a,b| a%b),
            Opcode::ASSIGN_FIELD_AND=>self.handle_fu64_field_assign_op(heap, |a,b| a&b),
            Opcode::ASSIGN_FIELD_OR=>self.handle_fu64_field_assign_op(heap, |a,b| a|b),
            Opcode::ASSIGN_FIELD_XOR=>self.handle_fu64_field_assign_op(heap, |a,b| a^b),
            Opcode::ASSIGN_FIELD_SHL=>self.handle_fu64_field_assign_op(heap, |a,b| a<<b),
            Opcode::ASSIGN_FIELD_SHR=>self.handle_fu64_field_assign_op(heap, |a,b| a>>b),
            Opcode::ASSIGN_FIELD_IFNIL=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let old_value = heap.value(obj, field, &self.trap);
                    if old_value.is_err() || old_value.is_nil(){
                        let value = heap.set_value(obj, field, value, &self.trap);
                        self.push_stack_unchecked(value);
                    }
                    else{
                        self.push_stack_unchecked(NIL);
                    }
                }
                else{
                    let value = self.trap.err_not_object();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
                        
            Opcode::ASSIGN_INDEX=>{
                let value = self.pop_stack_resolved(heap);
                let index = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let value = heap.set_value(obj, index, value, &self.trap);
                    self.push_stack_unchecked(value);
                }
                else if let Some(arr) = object.as_array(){
                    let value = heap.array_index(arr, index.as_index(), &self.trap);
                    self.push_stack_unchecked(value);
                }
                else{
                    let value = self.trap.err_not_object();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
// ASSIGN INDEX
            Opcode::ASSIGN_INDEX_ADD=>{
                let value = self.pop_stack_resolved(heap);
                let index = self.pop_stack_resolved(heap);
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let old_value = heap.value(obj, index, &self.trap);
                    if old_value.is_string_like() || value.is_string_like(){
                        let str = heap.new_string_with(|heap, out|{
                            heap.cast_to_string(old_value, out);
                            heap.cast_to_string(value, out);
                        });
                        let value = heap.set_value(obj, index, str.into(), &self.trap);
                        self.push_stack_unchecked(value);
                    }
                    else{
                        let fa = heap.cast_to_f64(old_value, self.trap.ip);
                        let fb = heap.cast_to_f64(value, self.trap.ip);
                        let value = heap.set_value(obj, index, ScriptValue::from_f64_traced_nan(fa + fb, self.trap.ip), &self.trap);
                        self.push_stack_unchecked(value);
                    }
                }
                else if let Some(arr) = object.as_array(){
                    let index = index.as_index();
                    let old_value = heap.array_index(arr, index, &self.trap);
                    if old_value.is_string_like() || value.is_string_like(){
                        let str = heap.new_string_with(|heap, out|{
                            heap.cast_to_string(old_value, out);
                            heap.cast_to_string(value, out);
                        });
                        let value = heap.set_array_index(arr, index, str.into(), &self.trap);
                        self.push_stack_unchecked(value);
                    }
                    else{
                        let fa = heap.cast_to_f64(old_value, self.trap.ip);
                        let fb = heap.cast_to_f64(value, self.trap.ip);
                        let value = heap.set_array_index(arr, index, ScriptValue::from_f64_traced_nan(fa + fb, self.trap.ip), &self.trap);
                        self.push_stack_unchecked(value);
                    }
                }
                else{
                    let value = self.trap.err_not_assignable();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            },
            Opcode::ASSIGN_INDEX_SUB=>self.handle_f64_index_assign_op(heap, |a,b| a-b),
            Opcode::ASSIGN_INDEX_MUL=>self.handle_f64_index_assign_op(heap, |a,b| a*b),
            Opcode::ASSIGN_INDEX_DIV=>self.handle_f64_index_assign_op(heap, |a,b| a/b),
            Opcode::ASSIGN_INDEX_MOD=>self.handle_f64_index_assign_op(heap, |a,b| a%b),
            Opcode::ASSIGN_INDEX_AND=>self.handle_fu64_index_assign_op(heap, |a,b| a&b),
            Opcode::ASSIGN_INDEX_OR=>self.handle_fu64_index_assign_op(heap, |a,b| a|b),
            Opcode::ASSIGN_INDEX_XOR=>self.handle_fu64_index_assign_op(heap, |a,b| a^b),
            Opcode::ASSIGN_INDEX_SHL=>self.handle_fu64_index_assign_op(heap, |a,b| a<<b),
            Opcode::ASSIGN_INDEX_SHR=>self.handle_fu64_index_assign_op(heap, |a,b| a>>b),
            Opcode::ASSIGN_INDEX_IFNIL=>{
                let value = self.pop_stack_resolved(heap);
                let index = self.pop_stack_resolved(heap);
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let old_value = heap.value(obj, index, &self.trap);
                    if old_value.is_err() || old_value.is_nil(){
                        let value = heap.set_value(obj, index, value, &self.trap);
                        self.push_stack_unchecked(value);
                    }
                    else{
                        self.push_stack_unchecked(NIL);
                    }
                }
                else if let Some(arr) = object.as_array(){
                    let index = index.as_index();
                    let old_value = heap.array_index(arr, index, &self.trap);
                    if old_value.is_err() || old_value.is_nil(){
                        let value = heap.set_array_index(arr, index, value, &self.trap);
                        self.push_stack_unchecked(value);
                    }
                    else{
                        self.push_stack_unchecked(NIL);
                    }
                }
                else{
                    let value = self.trap.err_not_object();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
// ASSIGN ME            
            Opcode::ASSIGN_ME=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                if self.call_has_me(){
                    let me = self.mes.last().unwrap();
                    match me{
                        ScriptMe::Call{args,..}=>{
                            heap.named_fn_arg(*args, field, value, &self.trap);
                        }
                        ScriptMe::Object(obj)=>{
                            if field.is_string_like(){
                                heap.set_string_keys(*obj);
                            }
                            heap.set_value(*obj, field, value, &self.trap);
                        }
                        ScriptMe::Pod{pod,..}=>{
                            heap.set_pod_field(*pod, field, value, &self.trap);
                        }
                        ScriptMe::Array(_arr)=>{
                            self.trap.err_not_allowed_in_array();
                        }
                    }
                }
                self.trap.goto_next();
            }
                        
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let value = match self.mes.last().unwrap(){
                    ScriptMe::Call{..} | ScriptMe::Pod{..}=>{
                        self.trap.err_not_allowed_in_arguments()
                    }
                    ScriptMe::Object(obj)=>{
                        heap.vec_insert_value_at(*obj, field, value, opcode == Opcode::ASSIGN_ME_BEFORE, &self.trap)
                    }
                    ScriptMe::Array(_arr)=>{
                        self.trap.err_not_allowed_in_array()
                    }
                };
                self.push_stack_unchecked(value);
                self.trap.goto_next();
            }
                        
            Opcode::ASSIGN_ME_BEGIN=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                let value = match self.mes.last().unwrap(){
                    ScriptMe::Call{..} | ScriptMe::Pod{..}=>{
                        self.trap.err_not_allowed_in_arguments()
                    }
                    ScriptMe::Object(obj)=>{
                        heap.vec_insert_value_begin(*obj, field, value, &self.trap)
                    }
                    ScriptMe::Array(_arr)=>{
                        self.trap.err_not_allowed_in_array()
                    }
                };
                self.push_stack_unchecked(value);
                self.trap.goto_next();
            }
            
            
// CONCAT  
            Opcode::CONCAT=>{
                let op1 = self.pop_stack_resolved(heap);
                let op2 = self.pop_stack_resolved(heap);
                let ptr = heap.new_string_with(|heap, out|{
                    heap.cast_to_string(op1, out);
                    heap.cast_to_string(op2, out);
                });
                self.push_stack_unchecked(ptr.into());
                self.trap.goto_next();
            }
// EQUALITY
            Opcode::EQ=> {
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_unchecked(heap.deep_eq(a, b).into());
                self.trap.goto_next();
            }
            Opcode::NEQ=> {
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_unchecked((!heap.deep_eq(a, b)).into());
                self.trap.goto_next();
            }
            
            Opcode::LT=>self.handle_f64_cmp_op(heap, opargs, |a,b| a<b),
            Opcode::GT=>self.handle_f64_cmp_op(heap, opargs, |a,b| a>b),
            Opcode::LEQ=>self.handle_f64_cmp_op(heap, opargs, |a,b| a<=b),
            Opcode::GEQ=>self.handle_f64_cmp_op(heap, opargs, |a,b| a>=b),
            
            Opcode::LOGIC_AND => self.handle_bool_op(heap, |a,b| a&&b),
            Opcode::LOGIC_OR => self.handle_bool_op(heap, |a,b| a||b),
            Opcode::NIL_OR => {
                let op1 = self.pop_stack_resolved(heap);
                let op2 = self.pop_stack_resolved(heap);
                if op1.is_nil(){
                    self.push_stack_unchecked(op2);
                }
                else{
                    self.push_stack_unchecked(op1);
                }
                self.trap.goto_next();
            }
            Opcode::SHALLOW_EQ =>{
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_value((a ==  b).into());
                self.trap.goto_next();
            }
            Opcode::SHALLOW_NEQ=>{
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_unchecked((a != b).into());
                self.trap.goto_next();
            }
// Object/Array begin
            Opcode::BEGIN_PROTO=>{
                let proto = self.pop_stack_resolved(heap);
                let me = heap.new_with_proto_checked(proto, &self.trap);
                self.mes.push(ScriptMe::Object(me));
                self.trap.goto_next();
            }
            Opcode::BEGIN_PROTO_ME=>{
                let field = self.peek_stack_value();
                let me = self.mes.last().unwrap();
                let proto = if let ScriptMe::Object(object) = me{
                    heap.value(*object, field, &self.trap)
                }
                else{
                    NIL
                };
                let me = heap.new_with_proto(proto);
                self.mes.push(ScriptMe::Object(me));
                self.trap.goto_next();
            }
            Opcode::END_PROTO=>{
                // see if we need to transform to a pod type
                let me = self.mes.pop().unwrap();
                if let ScriptMe::Object(me) = me{
                    heap.finalize_maybe_pod_type(me, &code.builtins.pod, &self.trap);
                }
                self.push_stack_unchecked(me.into());
                self.trap.goto_next();
            }
            Opcode::BEGIN_BARE=>{ // bare object
                let me = heap.new_object();
                self.mes.push(ScriptMe::Object(me));
                self.trap.goto_next();
            }
            Opcode::END_BARE=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_unchecked(me.into());
                self.trap.goto_next();
            }
            Opcode::BEGIN_ARRAY=>{
                let me = heap.new_array();
                self.mes.push(ScriptMe::Array(me));
                self.trap.goto_next();
            }
            Opcode::END_ARRAY=>{
                let me = self.mes.pop().unwrap();
                self.push_stack_unchecked(me.into());
                self.trap.goto_next();
            }
// Calling
            Opcode::CALL_ARGS=>{
                // alright we're calling a 'type'
                let fnobj = self.pop_stack_resolved(heap);
                // check if we are a POD 
                if let Some(ty) = heap.pod_type(fnobj){
                    // lets construct a new pod
                    let pod = heap.new_pod(ty);
                    self.mes.push(ScriptMe::Pod{pod, offset:ScriptPodOffset::default()});
                }
                else{
                    let scope = heap.new_with_proto(fnobj);
                    heap.clear_object_deep(scope);
                    self.mes.push(ScriptMe::Call{args:scope, this:None});
                }
                self.trap.goto_next();
            }
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC=>{
                //self.call_exec(heap, code, scope);
                // ok so now we have all our args on 'mes'
                let me = self.mes.pop().unwrap();
                
                let (args, this) = match me{
                    ScriptMe::Call{args, this}=>(args,this),
                    ScriptMe::Pod{pod,offset}=>{
                        // alright finalize the pod
                        heap.pod_check_arg_total(pod, offset, &self.trap);
                        self.push_stack_unchecked(pod.into());
                        self.trap.goto_next();
                        if opargs.is_pop_to_me(){
                            self.pop_to_me(heap, code);
                        }
                        return
                    }
                    _=>panic!()
                };
                
                if let Some(this) = this{
                    heap.force_value_in_map(args, id!(this).into(), this);
                }
                // set the scope back to 'deep' so values can be written again
                heap.set_object_deep(args);
                heap.set_object_storage_auto(args);
                                
                if let Some(fnptr) = heap.parent_as_fn(args){
                    match fnptr{
                        ScriptFnPtr::Native(ni)=>{
                            let ip = self.trap.ip;
                            self.trap.in_rust = true;
                            let ret = (*code.native.borrow().functions[ni.index as usize])(&mut ScriptVm{
                                host,
                                heap,
                                thread:self,
                                code
                            }, args);
                            
                            // if we trapped on 'pause' we need to reexecute this function
                            if self.is_paused{
                                self.mes.push(me);
                                return
                            }
                            
                            self.trap.in_rust = false;
                            self.trap.ip = ip;
                            self.push_stack_value(ret);
                            heap.free_object_if_unreffed(args);
                            self.trap.goto_next();
                        }
                        ScriptFnPtr::Script(sip)=>{
                            let call = CallFrame{
                                bases: self.new_bases(),
                                args: opargs,
                                return_ip: Some(ScriptIp{index: self.trap.ip.index + 1, body:self.trap.ip.body})
                            };
                            self.scopes.push(args);
                            self.calls.push(call);
                            self.trap.ip = sip;
                            if opargs.is_pop_to_me(){ // skip this
                                return
                            }
                        }
                    }
                }
                else{
                    let value = self.trap.err_not_fn();
                    self.push_stack_unchecked(value);
                    self.trap.goto_next();
                }
                
            }
            Opcode::METHOD_CALL_ARGS=>{
                
                let method =  self.pop_stack_value();
                let this = self.pop_stack_resolved(heap);
                let fnobj = if let Some(obj) = this.as_object(){
                    heap.object_method(obj, method, &mut Default::default())
                }
                else if let Some(pod) = this.as_pod(){ // we're calling a method on some other thing
                    heap.pod_method(pod, method, &mut Default::default())
                }
                else{
                    NIL
                };
                                
                let args = if fnobj.is_err() || fnobj == NIL{
                    let method = method.as_id().unwrap_or(id!());
                    let type_index = this.value_type().to_redux();
                    let type_entry = &code.native.borrow().type_table[type_index.to_index()];
                    if let Some(method_ptr) = type_entry.get(&method){
                        let args = heap.new_with_proto((*method_ptr).into());
                        args
                    }
                    else{ 
                        self.trap.err_not_found();
                        heap.new_with_proto(id!(undefined_function).into())
                    }
                }
                else{
                    if let Some(ty) = heap.pod_type(fnobj){
                        let pod = heap.new_pod(ty);
                        self.mes.push(ScriptMe::Pod{pod, offset:ScriptPodOffset::default()});
                        self.trap.goto_next();
                        return
                    }
                    heap.new_with_proto(fnobj)
                };
                //heap.set_object_map(scope);
                // set the args object to not write into the prototype
                heap.clear_object_deep(args);
                
                self.mes.push(ScriptMe::Call{args, this:Some(this)});
                self.trap.goto_next();
            }
// Fn def
            Opcode::FN_ARGS=>{
                let scope = *self.scopes.last_mut().unwrap();
                let me = heap.new_with_proto(scope.into());
                                
                // set it to a vec type to ensure ordered inserts
                heap.set_object_storage_vec2(me);
                heap.clear_object_deep(me);
                
                self.mes.push(ScriptMe::Object(me));
                self.trap.goto_next();
            }
            Opcode::FN_LET_ARGS=>{
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                let scope = *self.scopes.last_mut().unwrap();
                let me = heap.new_with_proto(scope.into());
                                                
                // set it to a vec type to ensure ordered inserts
                heap.set_object_storage_vec2(me);
                heap.clear_object_deep(me);
                self.mes.push(ScriptMe::Object(me));
                self.def_scope_value(heap, id, me.into());
                self.trap.goto_next();
            }   
            Opcode::FN_ARG_DYN=>{
                let value = if opargs.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                
                match self.mes.last().unwrap(){
                    ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..}=>{
                        self.trap.err_unexpected();
                    }
                    ScriptMe::Object(obj)=>{
                        heap.set_value(*obj, id.into(), value, &mut self.trap);
                    }
                };
                self.trap.goto_next();                
            }
            Opcode::FN_ARG_TYPED=>{
                let value = if opargs.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let _ty = self.pop_stack_value().as_id().unwrap_or(id!());
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                match self.mes.last().unwrap(){
                    ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..}=>{
                        self.trap.err_unexpected();
                    }
                    ScriptMe::Object(obj)=>{
                        heap.set_value(*obj, id.into(), value, &mut self.trap);
                    }
                };
                self.trap.goto_next();
            }
            Opcode::FN_BODY=>{ // alright we have all the args now we get an expression
                let jump_over_fn = opargs.to_u32();
                if let Some(me) = self.mes.pop(){
                    match me{
                        ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..}=>{
                            self.trap.err_unexpected();
                            self.push_stack_unchecked(NIL);
                        }
                        ScriptMe::Object(obj)=>{
                            heap.set_fn(obj, ScriptFnPtr::Script(
                                ScriptIp{body: self.trap.ip.body, index:(self.trap.ip() + 1)}
                            ));
                            self.push_stack_unchecked(obj.into());
                        }
                    };
                    self.trap.goto_rel(jump_over_fn);
                }
                else{
                    self.trap.err_unexpected();
                    self.push_stack_unchecked(NIL);
                    self.trap.goto_next();
                }
            }
            Opcode::RETURN=>{
                let value = if opargs.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let call = self.calls.pop().unwrap();
                self.truncate_bases(call.bases, heap);
                
                if let Some(ret) = call.return_ip{
                    self.trap.ip = ret;
                    self.push_stack_unchecked(value);
                    if call.args.is_pop_to_me(){
                        self.pop_to_me(heap, code);
                    }
                }
                else{
                    self.trap.on.set(Some(ScriptTrapOn::Return(value)));
                }
            }
            Opcode::RETURN_IF_ERR=>{
                let value = self.peek_stack_resolved(heap);
                if value.is_err(){
                    let call = self.calls.pop().unwrap();
                    self.truncate_bases(call.bases, heap);
                    if let Some(ret) = call.return_ip{
                        self.trap.ip = ret;
                        self.push_stack_unchecked(value);
                        if call.args.is_pop_to_me(){
                            self.pop_to_me(heap, code);
                        }
                    }
                    else{
                        self.trap.on.set(Some(ScriptTrapOn::Return(value)));
                    }
                }
                else{
                    self.trap.goto_next()
                }
            }
// IF            
            Opcode::IF_TEST=>{
                let test = self.pop_stack_resolved(heap);
                let test = heap.cast_to_bool(test);
                if test {
                    // continue
                    self.trap.goto_next()
                }
                else{ // jump to else
                    if opargs.is_need_nil(){ // no else coming
                        self.push_stack_unchecked(NIL);
                    }
                    self.trap.goto_rel(opargs.to_u32());
                }
            }
            
            Opcode::IF_ELSE =>{ // we are running into an else jump over it
                // we have to chuck our scope stack if we made any
                // also pop our ifelse stack
                self.trap.goto_rel(opargs.to_u32());
            }
// Use            
            Opcode::USE=>{
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    if field.as_id() == Some(id!(*)) {
                        let mut items = Vec::new();
                        if let Some(obj_data) = heap.objects.get(obj.index as usize) {
                            for (k, v) in obj_data.map.iter() {
                                items.push((*k, v.value));
                            }
                            for item in &obj_data.vec {
                                items.push((item.key, item.value));
                            }
                        }
                        for (k, v) in items {
                            if let Some(id) = k.as_id() {
                                self.def_scope_value(heap, id, v);
                            }
                        }
                    }
                    else{
                        let value = heap.value(obj, field, &self.trap);
                        if !value.is_nil(){
                            if let Some(field) = field.as_id(){
                                self.def_scope_value(heap, field, value);
                            }
                        }
                    }
                }
                self.trap.goto_next();
                return
            }
// Field            
            Opcode::FIELD=>{
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let value = heap.value(obj, field, &self.trap);
                    self.push_stack_unchecked(value);
                }
                else if let Some(pod) = object.as_pod(){
                    let value = heap.pod_read_field(pod, field, &code.builtins.pod, &self.trap);
                    self.push_stack_unchecked(value);
                }
                else {
                    let field = field.as_id().unwrap_or(id!());
                    let type_index = object.value_type().to_redux();
                    let getter = &code.native.borrow().getters[type_index.to_index()];
                    let ret = (*getter)(&mut ScriptVm{
                        host,
                        heap,
                        thread:self,
                        code
                    }, object, field);
                    self.push_stack_unchecked(ret);
                }
                self.trap.goto_next();
            }
            Opcode::FIELD_NIL=>{
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let value = heap.value(obj, field, &self.trap);
                    self.push_stack_unchecked(value);
                }
                else{
                    self.push_stack_unchecked(NIL);
                }
                self.trap.goto_next();
            }
            Opcode::ME_FIELD=>{
                let field = self.pop_stack_value();
                let value = match self.mes.last().unwrap(){
                    ScriptMe::Array(_)=>{
                        self.trap.err_not_allowed_in_array()
                    }
                    ScriptMe::Call{args,..}=>{
                        heap.value(*args, field, &self.trap)
                    }
                    ScriptMe::Pod{pod,..}=>{
                        heap.pod_read_field(*pod, field,  &code.builtins.pod, &self.trap)
                    }
                    ScriptMe::Object(obj)=>{
                        heap.value(*obj, field, &self.trap)
                    }
                };
                self.push_stack_value(value);
                self.trap.goto_next();
            }
            Opcode::PROTO_FIELD=>{ // implement proto field!
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let value = heap.value(obj, field, &self.trap);
                    self.push_stack_unchecked(value)
                }
                else{
                    let value = self.trap.err_not_object();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
            
            Opcode::POP_TO_ME=>{
                self.pop_to_me(heap, code);
                self.trap.goto_next();
            }
// Array index            
            Opcode::ARRAY_INDEX=>{
                let index = self.pop_stack_resolved(heap);
                let object = self.pop_stack_resolved(heap);
                
                if let Some(obj) = object.as_object(){
                    let value = heap.value(obj, index, &self.trap);
                    self.push_stack_unchecked(value)
                }
                else if let Some(arr) = object.as_array(){
                    let index = index.as_index();
                    let value = heap.array_index(arr, index, &self.trap);
                    self.push_stack_unchecked(value)
                }
                else{
                    let value = self.trap.err_not_object();
                    self.push_stack_unchecked(value);
                }
                self.trap.goto_next();
            }
// Let                   
            Opcode::LET_DYN=>{
                let value = if opargs.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value();
                let id = id.as_id().unwrap_or(id!());
                self.def_scope_value(heap, id, value);
                self.trap.goto_next();
            }
            Opcode::LET_TYPED=>{
                let value = if opargs.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let _ty = self.pop_stack_value();
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                self.def_scope_value(heap, id, value);
                self.trap.goto_next();
            }
            Opcode::VAR_DYN=>{
                let value = if opargs.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value();
                let id = id.as_id().unwrap_or(id!());
                self.def_scope_value(heap, id, value);
                self.trap.goto_next();
            }
            Opcode::VAR_TYPED=>{
                let value = if opargs.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let _ty = self.pop_stack_value();
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                self.def_scope_value(heap, id, value);
                self.trap.goto_next();
            } 
// Tree search            
            Opcode::SEARCH_TREE=>{
                self.trap.goto_next();
            }
// Log            
            Opcode::LOG=>{
                if let Some(loc) = code.ip_to_loc(self.trap.ip){
                    let value = self.peek_stack_resolved(heap);
                    if value != NIL{
                        if let Some(err) = value.as_err(){
                            if let Some(loc2) = code.ip_to_loc(err.ip){
                                log_with_level(&loc.file, loc.line, loc.col, loc.line, loc.col, format!("{} {}", value, loc2), LogLevel::Log);
                            }
                        }
                        if let Some(nanip) = value.as_f64_traced_nan(){
                            if let Some(loc2) = code.ip_to_loc(nanip){
                                log_with_level(&loc.file, loc.line, loc.col, loc.line, loc.col, format!("{} NaN Traced to {}", value, loc2), LogLevel::Log);
                            }
                        }
                        else{
                            let mut out = String::new();
                            let mut recur = Vec::new();
                            heap.to_debug_string(value, &mut recur, &mut out);
                            log_with_level(&loc.file, loc.line, loc.col, loc.line, loc.col, format!("{:?}:{out}", value.value_type()), LogLevel::Log);
                            //heap.print(value);
                            //println!("");
                        }
                    }
                    else{
                        log_with_level(&loc.file, loc.line, loc.col, loc.line, loc.col, format!("nil"), LogLevel::Log);
                    }
                }
                self.trap.goto_next();
            }
// Me/Scope
            Opcode::ME=>{
                if self.call_has_me(){
                    match self.mes.last().unwrap(){
                        ScriptMe::Array(arr)=>{
                            self.push_stack_value((*arr).into());
                        }
                        ScriptMe::Call{args,..}=>{
                            self.push_stack_value((*args).into());
                        }
                        ScriptMe::Pod{pod,..}=>{
                            self.push_stack_value((*pod).into());
                        }
                        ScriptMe::Object(obj)=>{
                            self.push_stack_value((*obj).into());
                        }
                    }
                }
                else{
                    self.push_stack_value(NIL);
                }
                self.trap.goto_next();
            }
            
            Opcode::SCOPE=>{
                let scope = *self.scopes.last_mut().unwrap();
                self.push_stack_value(scope.into());
                self.trap.goto_next();
            }
// For            
            Opcode::FOR_1 =>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, code, opargs.to_u32() as _, source, value_id, None, None);
            }
            Opcode::FOR_2 =>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                let index_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, code, opargs.to_u32() as _, source, value_id,Some(index_id), None);
            }
            Opcode::FOR_3=>{
                let source = self.pop_stack_resolved(heap);
                let value_id = self.pop_stack_value().as_id().unwrap();
                let index_id = self.pop_stack_value().as_id().unwrap();
                let key_id = self.pop_stack_value().as_id().unwrap();
                self.begin_for_loop(heap, code, opargs.to_u32() as _, source, value_id, Some(index_id), Some(key_id));
            }
            Opcode::LOOP=>{
                self.begin_loop(heap, opargs.to_u32() as _);
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
                    self.trap.goto_next();
                }
            }
            Opcode::CONTINUE=>{
                self.end_for_loop(heap, code);
            }
// Range            
            Opcode::RANGE=>{
                let end = self.pop_stack_resolved(heap);
                let start = self.pop_stack_resolved(heap);
                let range = heap.new_with_proto(code.builtins.range.into());
                heap.set_value_def(range, id!(start).into(), start);
                heap.set_value_def(range, id!(end).into(), end);
                self.push_stack_unchecked(range.into());
                self.trap.goto_next();
            }
// Is            
            Opcode::IS=>{
                let rhs = self.pop_stack_value();
                let lhs = self.pop_stack_resolved(heap);
                let cmp = if let Some(id) = rhs.as_id(){
                    match lhs.value_type().to_redux(){
                        ScriptValueType::REDUX_NUMBER=>id == id!(number).into(),
                        ScriptValueType::REDUX_NAN=>id == id!(number).into() || id == id!(nan).into(),
                        ScriptValueType::REDUX_BOOL=>id == id!(bool).into(),
                        ScriptValueType::REDUX_NIL=>id == id!(nan).into(),
                        ScriptValueType::REDUX_COLOR=>id == id!(color).into(),
                        ScriptValueType::REDUX_STRING=>id == id!(string).into(),
                        ScriptValueType::REDUX_OBJECT=>{
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
                        ScriptValueType::REDUX_ID=>id == id!(id).into(),
                        _=>false
                    }
                }
                else if let Some(obj) = lhs.as_object(){
                    heap.has_proto(obj, rhs)
                }
                else{
                    false
                };
                self.push_stack_unchecked(cmp.into());
                self.trap.goto_next();
            }
// Try / OK            
            Opcode::OK_TEST=>{
                // make a try stack item
                self.last_err = NIL;
                self.tries.push(TryFrame{
                    push_nil: true,
                    start_ip: self.trap.ip(),
                    jump: opargs.to_u32() + 1,
                    bases: self.new_bases()
                });
                self.trap.goto_next();
            }
            Opcode::OK_END=>{
                self.tries.pop();
                self.trap.goto_next();
            }
            Opcode::TRY_TEST=>{
                // make a try stack item
                self.last_err = NIL;
                self.tries.push(TryFrame{
                    push_nil: false,
                    start_ip: self.trap.ip(),
                    jump: opargs.to_u32() + 1,
                    bases: self.new_bases()
                });
                self.trap.goto_next();
            }
            Opcode::TRY_ERR=>{ // we hit err, meaning we dont have errors, pop try frame
                self.tries.pop().unwrap();
                self.trap.goto_rel(opargs.to_u32() + 1);
            }
            Opcode::TRY_OK=>{ // we hit ok, jump over it
                self.trap.goto_rel(opargs.to_u32());
            }
            opcode=>{
                println!("UNDEFINED OPCODE {}", opcode);
                self.trap.goto_next();
                // unknown instruction
            }
        }
        if opargs.is_pop_to_me(){
            self.pop_to_me(heap, code);
        }
    }
    
    pub fn begin_for_loop_inner(&mut self, heap:&mut ScriptHeap, jump:u32, source:ScriptValue, value_id:LiveId, index_id:Option<LiveId>, key_id:Option<LiveId>, first_value:ScriptValue, first_index:f64, first_key:ScriptValue){    
                                               
        self.trap.goto_next();
        self.loops.push(LoopFrame{
            bases: self.new_bases(),
            start_ip: self.trap.ip(),
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
        heap.set_value_def(new_scope, value_id.into(), first_value);
        if let Some(key_id) = key_id{
            heap.set_value_def(new_scope, key_id.into(), first_key);
        }
        if let Some(index_id) = index_id{
            heap.set_value_def(new_scope, index_id.into(), first_index.into());
        }
    }
    
    pub fn begin_loop(&mut self, heap:&mut ScriptHeap, jump:u32){   
        self.trap.goto_next();
        self.loops.push(LoopFrame{
            bases: self.new_bases(),
            start_ip: self.trap.ip.index,
            values: None,
            jump,
        });
        // lets make a new scope object and set our first value
        let scope = *self.scopes.last().unwrap();
        let new_scope = heap.new_with_proto(scope.into());
        self.scopes.push(new_scope);
    }
                
    pub fn begin_for_loop(&mut self, heap:&mut ScriptHeap, code:&ScriptCode, jump:u32, source:ScriptValue, value_id:LiveId, index_id:Option<LiveId>, key_id:Option<LiveId>){
        let v0 = ScriptValue::from_f64(0.0);
        if let Some(s) = source.as_f64(){
            if s >= 1.0{
                self.begin_for_loop_inner(heap, jump, source, value_id, key_id, index_id, v0, 0.0, v0);
                return
            }
        }
        else if let Some(obj) = source.as_object(){
            if heap.has_proto(obj, code.builtins.range.into()){ // range object
                let start = heap.value(obj, id!(start).into(),&self.trap).as_f64().unwrap_or(0.0);
                let end = heap.value(obj, id!(end).into(),&self.trap).as_f64().unwrap_or(0.0);
                let v = start.into();
                if (start-end).abs() >= 1.0{
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, v, start, v);
                    return
                }
            }
            else{
                if heap.vec_len(obj)>0{
                    let kv = heap.vec_key_value(obj, 0,&self.trap);
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, kv.value, 0.0, kv.key);
                    return
                }
            }
        }
        else if let Some(arr) = source.as_array(){
            if heap.array_len(arr)>0{
                let value = heap.array_index(arr, 0, &self.trap);
                self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, value, 0.0, NIL);
                return
            }
        }
        // jump over it and bail
        self.trap.goto_rel(jump);
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
                self.trap.goto(lf.start_ip);
                while self.scopes.len() > lf.bases.scope{
                    heap.free_object_if_unreffed(self.scopes.pop().unwrap());
                }
                let scope = heap.new_with_proto((*self.scopes.last().unwrap()).into());
                self.scopes.push(scope);
                heap.set_value_def(scope, values.value_id.into(), values.index.into());
                return
            }
            else if let Some(obj) = values.source.as_object(){
                if heap.has_proto(obj, code.builtins.range.into()){ // range object
                    let end = heap.value(obj, id!(end).into(),&self.trap).as_f64().unwrap_or(0.0);
                    let step = heap.value(obj, id!(step).into(),&self.trap).as_f64().unwrap_or(1.0);
                    values.index += step;
                    if values.index >= end{
                        self.break_for_loop(heap);
                        return
                    } 
                    while self.scopes.len() > lf.bases.scope{
                        heap.free_object_if_unreffed(self.scopes.pop().unwrap());
                    }
                    let scope = heap.new_with_proto((*self.scopes.last().unwrap()).into());
                    self.scopes.push(scope);
                    heap.set_value_def(scope, values.value_id.into(), values.index.into());
                    self.trap.goto(lf.start_ip);
                    return
                }
                else{
                    values.index += 1.0;
                    if values.index >= heap.vec_len(obj) as f64{
                        self.break_for_loop(heap);
                        return
                    }
                    let kv = heap.vec_key_value(obj, values.index as usize,&self.trap);
                    
                    while self.scopes.len() > lf.bases.scope{
                        heap.free_object_if_unreffed(self.scopes.pop().unwrap());
                    }
                    let scope = heap.new_with_proto((*self.scopes.last().unwrap()).into());
                    self.scopes.push(scope);
                    heap.set_value_def(scope, values.value_id.into(), kv.value.into());
                    if let Some(index_id) = values.index_id{
                        heap.set_value_def(scope, index_id.into(), values.index.into());
                    }
                    if let Some(key_id) = values.key_id{
                        heap.set_value_def(scope, key_id.into(), kv.key);
                    }
                    
                    self.trap.goto(lf.start_ip);
                    return
                }
            }
            else if let Some(arr) = values.source.as_array(){
                values.index += 1.0;
                if values.index >= heap.array_len(arr) as f64{
                    self.break_for_loop(heap);
                    return
                }
                let value = heap.array_index(arr, values.index as usize,&self.trap);
                                    
                while self.scopes.len() > lf.bases.scope{
                    heap.free_object_if_unreffed(self.scopes.pop().unwrap());
                }
                let scope = heap.new_with_proto((*self.scopes.last().unwrap()).into());
                self.scopes.push(scope);
                
                heap.set_value_def(scope, values.value_id.into(), value.into());
                if let Some(index_id) = values.index_id{
                    heap.set_value_def(scope, index_id.into(), values.index.into());
                }
                                    
                self.trap.goto(lf.start_ip);
                return
            }
        }
        else{ // we are a loop
            self.trap.goto(lf.start_ip);
            return
        }
        println!("For end unknown state");
        self.trap.goto_next();
    }
                    
    pub fn break_for_loop(&mut self, heap:&mut ScriptHeap){
        let lp = self.loops.pop().unwrap();
        self.truncate_bases(lp.bases, heap);
        self.trap.goto(lp.start_ip + lp.jump - 1);
    }
    
    pub fn handle_f64_scope_assign_op<F>(&mut self, heap:&mut ScriptHeap, f:F)
    where F: FnOnce(f64, f64)->f64
    {
        let value = self.pop_stack_resolved(heap);
        let id = self.pop_stack_value();
        if let Some(id) = id.as_id(){
            let va = self.scope_value(heap, id);
            if va.is_err(){
                self.push_stack_unchecked(va);
            }
            else{
                let fa = heap.cast_to_f64(va, self.trap.ip);
                let fb = heap.cast_to_f64(value, self.trap.ip);
                let value = self.set_scope_value(heap, id, ScriptValue::from_f64_traced_nan(f(fa,fb), self.trap.ip));
                self.push_stack_unchecked(value);
            }
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_fu64_scope_assign_op<F>(&mut self, heap:&mut ScriptHeap, f:F)
    where F: FnOnce(u64, u64)->u64
    {
        let value = self.pop_stack_resolved(heap);
        let id = self.pop_stack_value();
        if let Some(id) = id.as_id(){
            let old_value = self.scope_value(heap, id);
            if old_value.is_err(){
                self.push_stack_unchecked(old_value);
            }
            else{
                let ua = heap.cast_to_f64(old_value, self.trap.ip) as u64;
                let ub = heap.cast_to_f64(value, self.trap.ip) as u64;
                let value = self.set_scope_value(heap, id, ScriptValue::from_f64_traced_nan(f(ua,ub) as f64, self.trap.ip));
                self.push_stack_unchecked(value);
            }
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_f64_field_assign_op<F>(&mut self, heap:&mut ScriptHeap, f:F)
    where F: FnOnce(f64, f64)->f64
    {
        let value = self.pop_stack_resolved(heap);
        let field = self.pop_stack_value();
        let object = self.pop_stack_resolved(heap);
        if let Some(obj) = object.as_object(){
            let old_value = heap.value(obj, field, &self.trap);
            let fa = heap.cast_to_f64(old_value, self.trap.ip);
            let fb = heap.cast_to_f64(value, self.trap.ip);
            let value = heap.set_value(obj, field, ScriptValue::from_f64_traced_nan(f(fa, fb), self.trap.ip), &mut self.trap);
            self.push_stack_unchecked(value);
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_fu64_field_assign_op<F>(&mut self, heap:&mut ScriptHeap, f:F)
    where F: FnOnce(u64, u64)->u64
    {
        let value = self.pop_stack_resolved(heap);
        let field = self.pop_stack_value();
        let object = self.pop_stack_resolved(heap);
        if let Some(obj) = object.as_object(){
            let old_value = heap.value(obj, field, &self.trap);
            let fa = heap.cast_to_f64(old_value, self.trap.ip) as u64;
            let fb = heap.cast_to_f64(value, self.trap.ip) as u64;
            
            let value = heap.set_value(obj, field, ScriptValue::from_f64_traced_nan(f(fa, fb) as f64, self.trap.ip), &mut self.trap);
            self.push_stack_unchecked(value);
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_f64_index_assign_op<F>(&mut self, heap:&mut ScriptHeap, f:F)
    where F: FnOnce(f64, f64)->f64
    {
        let value = self.pop_stack_resolved(heap);
        let index = self.pop_stack_resolved(heap);
        let object = self.pop_stack_resolved(heap);
        if let Some(obj) = object.as_object(){
            let old_value = heap.value(obj, index, &self.trap);
            let fa = heap.cast_to_f64(old_value, self.trap.ip);
            let fb = heap.cast_to_f64(value, self.trap.ip);
            let value = heap.set_value(obj, index, ScriptValue::from_f64_traced_nan(f(fa, fb), self.trap.ip), &self.trap);
            self.push_stack_unchecked(value);
        }
        else if let Some(arr) = object.as_array(){
            let index = index.as_index();
            let old_value = heap.array_index(arr, index, &self.trap);
            let fa = heap.cast_to_f64(old_value, self.trap.ip);
            let fb = heap.cast_to_f64(value, self.trap.ip);
            let value = heap.set_array_index(arr, index, ScriptValue::from_f64_traced_nan(f(fa, fb), self.trap.ip), &self.trap);
            self.push_stack_unchecked(value);
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_fu64_index_assign_op<F>(&mut self, heap:&mut ScriptHeap, f:F)
    where F: FnOnce(u64, u64)->u64
    {
        let value = self.pop_stack_resolved(heap);
        let index = self.pop_stack_resolved(heap);
        let object = self.pop_stack_resolved(heap);
        if let Some(obj) = object.as_object(){
            let old_value = heap.value(obj, index, &self.trap);
            let fa = heap.cast_to_f64(old_value, self.trap.ip) as u64;
            let fb = heap.cast_to_f64(value, self.trap.ip) as u64;
            let value = heap.set_value(obj, index, ScriptValue::from_f64_traced_nan(f(fa, fb) as f64, self.trap.ip), &mut self.trap);
            self.push_stack_unchecked(value);
        }
        else if let Some(arr) = object.as_array(){
            let index = index.as_index();
            let old_value = heap.array_index(arr, index, &self.trap);
            let fa = heap.cast_to_f64(old_value, self.trap.ip) as u64;
            let fb = heap.cast_to_f64(value, self.trap.ip) as u64;
            let value = heap.set_array_index(arr, index, ScriptValue::from_f64_traced_nan(f(fa, fb) as f64, self.trap.ip), &self.trap);
            self.push_stack_unchecked(value);
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_f64_op<F>(&mut self, heap:&mut ScriptHeap, args:OpcodeArgs, f:F)
    where F: FnOnce(f64, f64)->f64
    {
        let fb = if args.is_u32(){
            args.to_u32() as f64
        }
        else{
            let b = self.pop_stack_resolved(heap);
            heap.cast_to_f64(b, self.trap.ip)
        };
        let a = self.pop_stack_resolved(heap);
        let fa = heap.cast_to_f64(a, self.trap.ip);
        self.push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(fa, fb), self.trap.ip));
        self.trap.goto_next();
    }

    pub fn handle_fu64_op<F>(&mut self, heap:&mut ScriptHeap, args:OpcodeArgs, f:F)
    where F: FnOnce(u64, u64)->u64
    {
        let ub = if args.is_u32(){
            args.to_u32() as u64
        }
        else{
            let b = self.pop_stack_resolved(heap);
            heap.cast_to_f64(b, self.trap.ip) as u64
        };
        let a = self.pop_stack_resolved(heap);
        let ua = heap.cast_to_f64(a, self.trap.ip) as u64;
        self.push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(ua, ub) as f64, self.trap.ip));
        self.trap.goto_next();
    }

    pub fn handle_f64_cmp_op<F>(&mut self, heap:&mut ScriptHeap, args:OpcodeArgs, f:F)
    where F: FnOnce(f64, f64)->bool
    {
        let fb = if args.is_u32(){
            args.to_u32() as f64
        }
        else{
            let b = self.pop_stack_resolved(heap);
            heap.cast_to_f64(b, self.trap.ip)
        };
        let a = self.pop_stack_resolved(heap);
        let fa = heap.cast_to_f64(a, self.trap.ip);
        self.push_stack_unchecked(ScriptValue::from_bool(f(fa, fb)));
        self.trap.goto_next();
    }

    pub fn handle_bool_op<F>(&mut self, heap:&mut ScriptHeap, f:F)
    where F: FnOnce(bool, bool)->bool
    {
        let b = self.pop_stack_resolved(heap);
        let a = self.pop_stack_resolved(heap);
        let ba = heap.cast_to_bool(a);
        let bb = heap.cast_to_bool(b);
        self.push_stack_unchecked(ScriptValue::from_bool(f(ba, bb)));
        self.trap.goto_next();
    }
}
