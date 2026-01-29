//! Opcode function and method call operations
//!
//! This module contains handle functions for function calls, method calls,
//! function definitions, and related operations.

use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::function::*;
use crate::vm::*;
use crate::thread::*;
use crate::pod::*;
use crate::trap::*;
use std::any::Any;
use crate::*;

impl ScriptThread {
    // Calling handlers
    
    pub(crate) fn handle_call_args(&mut self, heap: &mut ScriptHeap) {
        let fnobj = self.pop_stack_resolved(heap);
        if let Some(ty) = heap.pod_type(fnobj){
            let pod = heap.new_pod(ty);
            self.mes.push(ScriptMe::Pod{pod, offset: ScriptPodOffset::default()});
        }
        else{
            let scope = heap.new_with_proto(fnobj);
            heap.clear_object_deep(scope);
            self.mes.push(ScriptMe::Call{args: scope, sself: None});
        }
        self.trap.goto_next();
    }
    
    // Returns true if caller should call pop_to_me, false if it should be skipped
    pub(crate) fn handle_call_exec(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, host: &mut dyn Any, opargs: OpcodeArgs) -> bool {
        let me = self.mes.pop().unwrap();
        
        let (args, sself) = match me{
            ScriptMe::Call{args, sself} => (args, sself),
            ScriptMe::Pod{pod, offset} => {
                heap.pod_check_arg_total(pod, offset, self.trap.pass());
                self.push_stack_unchecked(pod.into());
                self.trap.goto_next();
                return true // Pod: caller should handle pop_to_me
            }
            _ => panic!()
        };
        
        if let Some(sself) = sself{
            heap.force_value_in_map(args, id!(self).into(), sself);
        }
        heap.set_object_deep(args);
        heap.set_object_storage_auto(args);
                        
        if let Some(fnptr) = heap.parent_as_fn(args){
            match fnptr{
                ScriptFnPtr::Native(ni) => {
                    let ip = self.trap.ip;
                    self.trap.in_rust = true;
                    let ret = (*code.native.borrow().functions[ni.index as usize])(&mut ScriptVm{
                        host,
                        heap,
                        thread: self,
                        code
                    }, args);
                    
                    if self.is_paused{
                        self.mes.push(me);
                        return false // Paused: skip pop_to_me, function not complete
                    }
                    
                    self.trap.in_rust = false;
                    self.trap.ip = ip;
                    self.push_stack_value(ret);
                    heap.free_object_if_unreffed(args);
                    self.trap.goto_next();
                    return true // Native complete: caller should handle pop_to_me
                }
                ScriptFnPtr::Script(sip) => {
                    let call = CallFrame{
                        bases: self.new_bases(),
                        args: opargs,
                        return_ip: Some(ScriptIp{index: self.trap.ip.index + 1, body: self.trap.ip.body})
                    };
                    self.scopes.push(args);
                    self.calls.push(call);
                    self.trap.ip = sip;
                    return false // Script: skip pop_to_me, RETURN will handle it via call.args
                }
            }
        }
        else{
            let value = script_err_not_fn!(self.trap, "call target is not a function (got {:?})", heap.proto(args).value_type());
            self.push_stack_unchecked(value);
            self.trap.goto_next();
            return true // Error: caller should handle pop_to_me
        }
    }
    
    pub(crate) fn handle_method_call_args(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) -> bool {
        let method = self.pop_stack_value();
        let sself = self.pop_stack_resolved(heap);
        let fnobj = if let Some(obj) = sself.as_object(){
            heap.object_method(obj, method, NoTrap)
        }
        else if let Some(pod) = sself.as_pod(){
            heap.pod_method(pod, method, NoTrap)
        }
        else{
            NIL
        };
                        
        let args = if fnobj.is_err() || fnobj == NIL{
            let method = method.as_id().unwrap_or(id!());
            let type_index = sself.value_type().to_redux();
            let type_entry = &code.native.borrow().type_table[type_index.to_index()];
            if let Some(method_ptr) = type_entry.get(&method){
                heap.new_with_proto((*method_ptr).into())
            }
            else{ 
                script_err_not_found!(self.trap, "method {:?} not found on type {:?}", method, type_index);
                heap.new_with_proto(id!(undefined_function).into())
            }
        }
        else{
            if let Some(ty) = heap.pod_type(fnobj){
                let pod = heap.new_pod(ty);
                self.mes.push(ScriptMe::Pod{pod, offset: ScriptPodOffset::default()});
                self.trap.goto_next();
                return true // Pod: caller should return early, skip pop_to_me
            }
            heap.new_with_proto(fnobj)
        };
        heap.clear_object_deep(args);
        
        self.mes.push(ScriptMe::Call{args, sself: Some(sself)});
        self.trap.goto_next();
        false
    }

    // Fn def handlers
    
    pub(crate) fn handle_fn_args(&mut self, heap: &mut ScriptHeap) {
        let scope = *self.scopes.last_mut().unwrap();
        let me = heap.new_with_proto(scope.into());
        heap.set_object_storage_vec2(me);
        heap.clear_object_deep(me);
        self.mes.push(ScriptMe::Object(me));
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_fn_let_args(&mut self, heap: &mut ScriptHeap) {
        let id = self.pop_stack_value().as_id().unwrap_or(id!());
        let scope = *self.scopes.last_mut().unwrap();
        let me = heap.new_with_proto(scope.into());
        heap.set_object_storage_vec2(me);
        heap.clear_object_deep(me);
        self.mes.push(ScriptMe::Object(me));
        self.def_scope_value(heap, id, me.into());
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_fn_arg_dyn(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
        let value = if opargs.is_nil(){
            NIL
        }
        else{
            self.pop_stack_resolved(heap)
        };
        let id = self.pop_stack_value().as_id().unwrap_or(id!());
        
        match self.mes.last().unwrap(){
            ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                script_err_unexpected!(self.trap, "FN_ARG_DYN for {:?}: expected Object context on stack", id);
            }
            ScriptMe::Object(obj) => {
                if id == id!(self) && heap.vec_len(*obj) == 0 {
                    // ignore self as first argument
                }
                else {
                    heap.set_value(*obj, id.into(), value, NoTrap);
                }
            }
        };
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_fn_arg_typed(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
        let _value = if opargs.is_nil(){
            NIL
        }
        else{
            self.pop_stack_resolved(heap)
        };
        let ty = self.pop_stack_resolved(heap);
        let id = self.pop_stack_value().as_id().unwrap_or(id!());
        match self.mes.last().unwrap(){
            ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                script_err_unexpected!(self.trap, "FN_ARG_TYPED for {:?}: expected Object context on stack", id);
            }
            ScriptMe::Object(obj) => {
                heap.set_value(*obj, id.into(), ty, NoTrap);
            }
        };
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_fn_body_dyn(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
        let jump_over_fn = opargs.to_u32();
        if let Some(me) = self.mes.pop(){
            match me{
                ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                    script_err_unexpected!(self.trap, "FN_BODY_DYN: expected Object context for function body, got {:?}", me);
                    self.push_stack_unchecked(NIL);
                }
                ScriptMe::Object(obj) => {
                    heap.set_fn(obj, ScriptFnPtr::Script(
                        ScriptIp{body: self.trap.ip.body, index: (self.trap.ip() + 1)}
                    ));
                    self.push_stack_unchecked(obj.into());
                }
            };
            self.trap.goto_rel(jump_over_fn);
        }
        else{
            script_err_unexpected!(self.trap, "FN_BODY_DYN: me stack is empty (function definition without arguments block)");
            self.push_stack_unchecked(NIL);
            self.trap.goto_next();
        }
    }
    
    pub(crate) fn handle_fn_body_typed(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
        let jump_over_fn = opargs.to_u32();
        let _return_type = self.pop_stack_value();
        if let Some(me) = self.mes.pop(){
            match me{
                ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                    script_err_unexpected!(self.trap, "FN_BODY_TYPED: expected Object context for typed function body, got {:?}", me);
                    self.push_stack_unchecked(NIL);
                }
                ScriptMe::Object(obj) => {
                    heap.set_fn(obj, ScriptFnPtr::Script(
                        ScriptIp{body: self.trap.ip.body, index: (self.trap.ip() + 1)}
                    ));
                    self.push_stack_unchecked(obj.into());
                }
            };
            self.trap.goto_rel(jump_over_fn);
        }
        else{
            script_err_unexpected!(self.trap, "FN_BODY_TYPED: me stack is empty (typed function definition without arguments block)");
            self.push_stack_unchecked(NIL);
            self.trap.goto_next();
        }
    }
}
