use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::object::*;
use crate::vm::*;
use crate::thread::*;
use crate::trap::*;
use std::any::Any;

macro_rules! f64_scope_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let id = $obj.pop_stack_value();
        if let Some(id) = id.as_id(){
            let va = $obj.scope_value($heap, id);
            if va.is_err(){
                $obj.push_stack_unchecked(va);
            }
            else{
                let fa = $heap.cast_to_f64(va, $obj.trap.ip);
                let fb = $heap.cast_to_f64(value, $obj.trap.ip);
                let value = $obj.set_scope_value($heap, id, ScriptValue::from_f64_traced_nan((fa $op fb), $obj.trap.ip));
                $obj.push_stack_unchecked(value);
            }
        }
        else{
            let value = $obj.trap.err_not_assignable();
            $obj.push_stack_unchecked(value);
        }
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! fu64_scope_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let id = $obj.pop_stack_value();
        if let Some(id) = id.as_id(){
            let va = $obj.scope_value($heap, id);
            if va.is_err(){
                $obj.push_stack_unchecked(va);
            }
            else{
                let ua = $heap.cast_to_f64(va, $obj.trap.ip) as u64;
                let ub = $heap.cast_to_f64(value, $obj.trap.ip) as u64;
                let value = $obj.set_scope_value($heap, id, ScriptValue::from_f64_traced_nan((ua $op ub) as f64, $obj.trap.ip));
                $obj.push_stack_unchecked(value);
            }
        }
        else{
            let value = $obj.trap.err_not_assignable();
            $obj.push_stack_unchecked(value);
        }
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! f64_field_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let field = $obj.pop_stack_value();
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, field, &$obj.trap);
            let fa = $heap.cast_to_f64(old_value, $obj.trap.ip);
            let fb = $heap.cast_to_f64(value, $obj.trap.ip);
            let value = $heap.set_value(obj, field, ScriptValue::from_f64_traced_nan(fa $op fb, $obj.trap.ip), &mut $obj.trap);
            $obj.push_stack_unchecked(value);
        }
        else{
            let value = $obj.trap.err_not_assignable();
            $obj.push_stack_unchecked(value);
        }
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! fu64_field_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let field = $obj.pop_stack_value();
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, field, &$obj.trap);
            let fa = $heap.cast_to_f64(old_value, $obj.trap.ip) as u64;
            let fb = $heap.cast_to_f64(value, $obj.trap.ip) as u64;
            
            let value = $heap.set_value(obj, field, ScriptValue::from_f64_traced_nan((fa $op fb) as f64, $obj.trap.ip), &mut $obj.trap);
            $obj.push_stack_unchecked(value);
        }
        else{
            let value = $obj.trap.err_not_assignable();
            $obj.push_stack_unchecked(value);
        }
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! f64_index_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let index = $obj.pop_stack_resolved($heap);
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, index, &$obj.trap);
            let fa = $heap.cast_to_f64(old_value, $obj.trap.ip);
            let fb = $heap.cast_to_f64(value, $obj.trap.ip);
            let value = $heap.set_value(obj, index, ScriptValue::from_f64_traced_nan(fa $op fb, $obj.trap.ip), &$obj.trap);
            $obj.push_stack_unchecked(value);
        }
        else if let Some(arr) = object.as_array(){
            let index = index.as_index();
            let old_value = $heap.array_index(arr, index, &$obj.trap);
            let fa = $heap.cast_to_f64(old_value, $obj.trap.ip);
            let fb = $heap.cast_to_f64(value, $obj.trap.ip);
            let value = $heap.set_array_index(arr, index, ScriptValue::from_f64_traced_nan(fa $op fb, $obj.trap.ip), &$obj.trap);
            $obj.push_stack_unchecked(value);
        }
        else{
            let value = $obj.trap.err_not_assignable();
            $obj.push_stack_unchecked(value);
        }
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! fu64_index_assign_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let value = $obj.pop_stack_resolved($heap);
        let index = $obj.pop_stack_resolved($heap);
        let object = $obj.pop_stack_resolved($heap);
        if let Some(obj) = object.as_object(){
            let old_value = $heap.value(obj, index, &$obj.trap);
            let fa = $heap.cast_to_f64(old_value, $obj.trap.ip) as u64;
            let fb = $heap.cast_to_f64(value, $obj.trap.ip) as u64;
            let value = $heap.set_value(obj, index, ScriptValue::from_f64_traced_nan((fa $op fb) as f64, $obj.trap.ip), &mut $obj.trap);
            $obj.push_stack_unchecked(value);
        }
        else if let Some(arr) = object.as_array(){
            let index = index.as_index();
            let old_value = $heap.array_index(arr, index, &$obj.trap);
            let fa = $heap.cast_to_f64(old_value, $obj.trap.ip) as u64;
            let fb = $heap.cast_to_f64(value, $obj.trap.ip) as u64;
            let value = $heap.set_array_index(arr, index, ScriptValue::from_f64_traced_nan((fa $op fb) as f64, $obj.trap.ip), &$obj.trap);
            $obj.push_stack_unchecked(value);
        }
        else{
            let value = $obj.trap.err_not_assignable();
            $obj.push_stack_unchecked(value);
        }
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! f64_op_impl{
    ($obj:ident, $heap:ident, $args:ident, $op:tt)=>{{
        let fb = if $args.is_u32(){
            $args.to_u32() as f64
        }
        else{
            let b = $obj.pop_stack_resolved($heap);
            $heap.cast_to_f64(b, $obj.trap.ip)
        };
        let a = $obj.pop_stack_resolved($heap);
        let fa = $heap.cast_to_f64(a, $obj.trap.ip);
        $obj.push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa $op fb, $obj.trap.ip));
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! fu64_op_impl{
    ($obj:ident, $heap:ident, $args:ident, $op:tt)=>{{
        let ub = if $args.is_u32(){
            $args.to_u32() as u64
        }
        else{
            let b = $obj.pop_stack_resolved($heap);
            $heap.cast_to_f64(b, $obj.trap.ip) as u64
        };
        let a = $obj.pop_stack_resolved($heap);
        let ua = $heap.cast_to_f64(a, $obj.trap.ip) as u64;
        $obj.push_stack_unchecked(ScriptValue::from_f64_traced_nan((ua $op ub) as f64, $obj.trap.ip));
        $obj.trap.ip.index += 1;
    }}
} 

macro_rules! f64_cmp_impl{
    ($obj:ident, $heap:ident, $args:ident, $op:tt)=>{{
        let fb = if $args.is_u32(){
            $args.to_u32() as f64
        }
        else{
            let b = $obj.pop_stack_resolved($heap);
            $heap.cast_to_f64(b, $obj.trap.ip)
        };
        let a = $obj.pop_stack_resolved($heap);
        let fa = $heap.cast_to_f64(a, $obj.trap.ip);
        //let fb = $heap.cast_to_f64(b, $obj.ip);
        $obj.push_stack_unchecked(ScriptValue::from_bool(fa $op fb));
        $obj.trap.ip.index += 1;
    }}
}

macro_rules! bool_op_impl{
    ($obj:ident, $heap:ident, $op:tt)=>{{
        let b = $obj.pop_stack_resolved($heap);
        let a = $obj.pop_stack_resolved($heap);
        let ba = $heap.cast_to_bool(a);
        let bb = $heap.cast_to_bool(b);
        $obj.push_stack_unchecked(ScriptValue::from_bool((ba $op bb)));
        $obj.trap.ip.index += 1;
    }}
} 

impl ScriptThread{
    
    pub fn opcode(&mut self,opcode: Opcode, args:OpcodeArgs, heap:&mut ScriptHeap, code:&ScriptCode, host:&mut dyn Any){
        
        match opcode{
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
                self.push_stack_unchecked(ptr.into());
                self.trap.goto_next();
            }
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
                self.push_stack_value(heap.shallow_eq(a, b).into());
                self.trap.goto_next();
            }
            Opcode::SHALLOW_NEQ=>{
                let b = self.pop_stack_resolved(heap);
                let a = self.pop_stack_resolved(heap);
                self.push_stack_unchecked((!heap.shallow_eq(a, b)).into());
                self.trap.goto_next();
            }
            
            Opcode::ASSIGN_ME=>{
                let value = self.pop_stack_resolved(heap);
                let field = self.pop_stack_value();
                if self.call_has_me(){
                    let me = self.mes.last().unwrap();
                    match me{
                        ScriptMe::Call(obj)=>{
                            heap.named_fn_arg(*obj, field, value, &self.trap);
                        }
                        ScriptMe::Object(obj)=>{
                            heap.set_value(*obj, field, value, &self.trap);
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
                    ScriptMe::Call(_obj)=>{
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
                    ScriptMe::Call(_obj)=>{
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
            
            Opcode::BEGIN_PROTO=>{
                let proto = self.pop_stack_resolved(heap);
                let me = heap.new_with_proto_check(proto, &self.trap);
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
                let me = self.mes.pop().unwrap();
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
            
            Opcode::CALL_ARGS=>{
                let fnobj = self.pop_stack_resolved(heap);
                let scope = heap.new_with_proto(fnobj);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                self.mes.push(ScriptMe::Call(scope));
                self.trap.goto_next();
            }
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC=>{
                //self.call_exec(heap, code, scope);
                // ok so now we have all our args on 'mes'
                let me = self.mes.pop().unwrap();
                let scope = if let ScriptMe::Call(scope) = me{scope}else{panic!()};
                // set the scope back to 'deep' so values can be written again
                heap.set_object_deep(scope);
                heap.set_object_storage_type(scope, ScriptObjectStorageType::AUTO);
                                
                if let Some(fnptr) = heap.parent_as_fn(scope){
                    match fnptr{
                        ScriptFnPtr::Native(ni)=>{
                            let ip = self.trap.ip;
                            self.trap.in_rust = true;
                            let ret = (*code.native.borrow().fn_table[ni.index as usize].fn_ptr)(&mut ScriptVm{
                                host,
                                heap,
                                thread:self,
                                code
                            }, scope);
                            self.trap.in_rust = false;
                            self.trap.ip = ip;
                            self.push_stack_value(ret);
                            heap.free_object_if_unreffed(scope);
                            self.trap.goto_next();
                        }
                        ScriptFnPtr::Script(sip)=>{
                            let call = CallFrame{
                                bases: self.new_bases(),
                                args: args,
                                return_ip: Some(ScriptIp{index: self.trap.ip.index + 1, body:self.trap.ip.body})
                            };
                            self.scopes.push(scope);
                            self.calls.push(call);
                            self.trap.ip = sip;
                            if args.is_pop_to_me(){ // skip this
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
                else{ // we're calling a method on some other thing
                    NIL
                };
                let scope = if fnobj.is_err() || fnobj == NIL{
                    // lets take the type
                    let type_index = this.value_type().to_redux();
                    let method = method.as_id().unwrap_or(id!());
                    let type_entry = &code.type_methods.type_table[type_index];
                    
                    if let Some(method_ptr) = type_entry.get(&method){
                        let scope = heap.new_with_proto((*method_ptr).into());
                        scope
                    }
                    else{ 
                        self.trap.err_not_found();
                        heap.new_with_proto(id!(undefined_function).into())
                    }
                }
                else{
                    heap.new_with_proto(fnobj)
                };
                //heap.set_object_map(scope);
                // set the args object to not write into the prototype
                heap.clear_object_deep(scope);
                heap.force_value_in_map(scope, id!(this).into(), this.into());
                self.mes.push(ScriptMe::Call(scope));
                self.trap.goto_next();
            }
            
            Opcode::FN_ARGS=>{
                let scope = *self.scopes.last_mut().unwrap();
                let me = heap.new_with_proto(scope.into());
                                
                // set it to a vec type to ensure ordered inserts
                heap.set_object_storage_type(me, ScriptObjectStorageType::VEC2);
                heap.clear_object_deep(me);
                                                
                self.mes.push(ScriptMe::Object(me));
                self.trap.goto_next();
            }
                                    
            Opcode::FN_ARG_DYN=>{
                let value = if args.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                
                match self.mes.last().unwrap(){
                    ScriptMe::Call(_) | ScriptMe::Array(_)=>{
                        self.trap.err_unexpected();
                    }
                    ScriptMe::Object(obj)=>{
                        heap.set_value(*obj, id.into(), value, &mut self.trap);
                    }
                };
                self.trap.goto_next();                
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
                match self.mes.last().unwrap(){
                    ScriptMe::Call(_) | ScriptMe::Array(_)=>{
                        self.trap.err_unexpected();
                    }
                    ScriptMe::Object(obj)=>{
                        heap.set_value(*obj, id.into(), value, &mut self.trap);
                    }
                };
                self.trap.goto_next();
            }
            Opcode::FN_BODY=>{ // alright we have all the args now we get an expression
                let jump_over_fn = args.to_u32();
                let me = self.mes.pop().unwrap();
                match me{
                    ScriptMe::Call(_) | ScriptMe::Array(_)=>{
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
                    self.trap.ip = ret;
                    self.push_stack_unchecked(value);
                    if call.args.is_pop_to_me(){
                        self.pop_to_me(heap);
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
                            self.pop_to_me(heap);
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
            Opcode::IF_TEST=>{
                let test = self.pop_stack_resolved(heap);
                let test = heap.cast_to_bool(test);
                if test {
                    // continue
                    self.trap.goto_next()
                }
                else{ // jump to else
                    self.trap.goto_rel(args.to_u32());
                }
            }
            
            Opcode::IF_ELSE =>{ // we are running into an else jump over it
                // we have to chuck our scope stack if we made any
                // also pop our ifelse stack
                self.trap.goto_rel(args.to_u32());
            }   
            
            Opcode::FIELD=>{
                let field = self.pop_stack_value();
                let object = self.pop_stack_resolved(heap);
                if let Some(obj) = object.as_object(){
                    let value = heap.value(obj, field, &self.trap);
                    self.push_stack_unchecked(value);
                }
                else{
                    let value = self.trap.err_not_object();
                    self.push_stack_unchecked(value);
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
                    ScriptMe::Call(obj) | ScriptMe::Object(obj)=>{
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
                self.pop_to_me(heap);
                self.trap.goto_next();
            }
            
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
                   
            Opcode::LET_DYN=>{
                let value = if args.is_nil(){
                    NIL
                }
                else{
                    self.pop_stack_resolved(heap)
                };
                let id = self.pop_stack_value().as_id().unwrap_or(id!());
                self.def_scope_value(heap, id, value);
                /*
                let scope = *self.scopes.last_mut().unwrap();
                let value =heap.set_value_ip(scope, id.into(), value, self.ip);
                if value.is_err(){
                    self.trap = Some(ScriptTrap::Error(value));
                }
                */
                self.trap.goto_next();
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
                self.def_scope_value(heap, id, value);
/*                
                let scope = *self.scopes.last_mut().unwrap();
                let value = heap.set_value_ip(scope, id.into(), value, self.ip);
                if value.is_err(){
                    self.trap = Some(ScriptTrap::Error(value));
                }*/
                self.trap.goto_next();
            } 
            
            Opcode::SEARCH_TREE=>{
                self.trap.goto_next();
            }
                                  
            Opcode::LOG=>{
                
                if let Some(loc) = code.ip_to_loc(self.trap.ip){
                    let value = self.peek_stack_resolved(heap);
                    if value != NIL{
                        
                        if let Some(err) = value.as_err(){
                            if let Some(loc2) = code.ip_to_loc(err.ip){
                                println!("{} {} {}", loc, value, loc2);
                            }
                        }
                        if let Some(nanip) = value.as_f64_traced_nan(){
                            if let Some(loc2) = code.ip_to_loc(nanip){
                                println!("{} NaN Traced to {}", loc, loc2);
                            }
                        }
                        else{
                            print!("{} {:?}: ", loc, value.value_type());
                            heap.print(value);
                            println!("");
                        }
                    }
                    else{
                        println!("{} nil", loc);
                    }
                }
                self.trap.goto_next();
            }
            
            Opcode::ME=>{
                if self.call_has_me(){
                    match self.mes.last().unwrap(){
                        ScriptMe::Array(arr)=>{
                            self.push_stack_value((*arr).into());
                        }
                        ScriptMe::Call(obj) | ScriptMe::Object(obj)=>{
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
                    self.trap.goto_next();
                }
            }
            Opcode::CONTINUE=>{
                self.end_for_loop(heap, code);
            }
            Opcode::RANGE=>{
                let end = self.pop_stack_resolved(heap);
                let start = self.pop_stack_resolved(heap);
                let range = heap.new_with_proto(code.builtins.range.into());
                heap.set_value_def(range, id!(start).into(), start);
                heap.set_value_def(range, id!(end).into(), end);
                self.push_stack_unchecked(range.into());
                self.trap.goto_next();
            }
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
            Opcode::TRY_TEST=>{
                // make a try stack item
                self.last_err = NIL;
                self.tries.push(TryFrame{
                    start_ip: self.trap.ip(),
                    jump: args.to_u32() + 1,
                    bases: self.new_bases()
                });
                self.trap.goto_next();
            }
            Opcode::TRY_ERR=>{ // we hit err, meaning we dont have errors, pop try frame
                self.tries.pop().unwrap();
                self.trap.goto_rel(args.to_u32() + 1);
            }
            Opcode::TRY_OK=>{ // we hit ok, jump over it
                self.trap.goto_rel(args.to_u32());
            }
            opcode=>{
                println!("UNDEFINED OPCODE {}", opcode);
                self.trap.goto_next();
                // unknown instruction
            }
        }
        if args.is_pop_to_me(){
            self.pop_to_me(heap);
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
}
