//! Opcode function and method call operations
//!
//! This module contains handle functions for function calls, method calls,
//! function definitions, and related operations.

use crate::makepad_live_id::*;
use crate::value::*;
use crate::opcode::*;
use crate::function::*;
use crate::vm::*;
use crate::thread::*;
use crate::trap::*;
use crate::pod::ScriptPodOffset;
use crate::*;

impl<'a> ScriptVm<'a> {
    // Calling handlers
    
    pub(crate) fn handle_call_args(&mut self) {
        let fnobj = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(ty) = self.bx.heap.pod_type(fnobj){
            let pod = self.bx.heap.new_pod(ty);
            self.bx.threads.cur().mes.push(ScriptMe::Pod{pod, offset: ScriptPodOffset::default()});
        }
        else{
            let scope = self.bx.heap.new_with_proto(fnobj);
            self.bx.heap.clear_object_deep(scope);
            self.bx.threads.cur().mes.push(ScriptMe::Call{args: scope, sself: None});
        }
        self.bx.threads.cur().trap.goto_next();
    }
    
    // Returns true if caller should call pop_to_me, false if it should be skipped
    pub(crate) fn handle_call_exec(&mut self, opargs: OpcodeArgs) -> bool {
        let me = self.bx.threads.cur().mes.pop().unwrap();
        
        let (args, sself) = match me{
            ScriptMe::Call{args, sself} => (args, sself),
            ScriptMe::Pod{pod, offset} => {
                self.bx.heap.pod_check_arg_total(pod, offset, self.bx.threads.cur().trap.pass());
                self.bx.threads.cur().push_stack_unchecked(pod.into());
                self.bx.threads.cur().trap.goto_next();
                return true // Pod: caller should handle pop_to_me
            }
            _ => panic!()
        };
        
        if let Some(sself) = sself{
            self.bx.heap.force_value_in_map(args, id!(self).into(), sself);
        }
        self.bx.heap.set_object_deep(args);
        self.bx.heap.set_object_storage_auto(args);
                        
        if let Some(fnptr) = self.bx.heap.parent_as_fn(args){
            match fnptr{
                ScriptFnPtr::Native(ni) => {
                    let ip = self.bx.threads.cur_ref().trap.ip;
                    // Get the function pointer and drop the borrow before calling
                    let func_ptr: *const dyn Fn(&mut ScriptVm, ScriptObject)->ScriptValue = {
                        let native = self.bx.code.native.borrow();
                        &*native.functions[ni.index as usize] as *const _
                    };
                    // Pause thread before native call so re-entrant calls get a different thread
                    self.bx.threads.cur().is_paused = true;
                    // SAFETY: The function pointer is valid as long as native functions aren't removed during execution
                    let ret = unsafe { (*func_ptr)(self, args) };
                    
                    // Check if native explicitly paused (via pause() which sets trap.on to Pause)
                    if matches!(self.bx.threads.cur().trap.on.get(), Some(ScriptTrapOn::Pause)) {
                        // Native explicitly paused, leave is_paused = true
                        self.bx.threads.cur().mes.push(me);
                        return false // Paused: skip pop_to_me, function not complete
                    }
                    
                    // Native didn't explicitly pause, unpause the thread
                    self.bx.threads.cur().is_paused = false;
                    
                    self.bx.threads.cur().trap.ip = ip;
                    self.bx.threads.cur().push_stack_value(ret); self.bx.heap.free_object_if_unreffed(args); // DISABLED: investigating RootObject already freed
                    self.bx.threads.cur().trap.goto_next();
                    return true // Native complete: caller should handle pop_to_me
                }
                ScriptFnPtr::Script(sip) => {
                    let call = CallFrame{
                        bases: self.bx.threads.cur_ref().new_bases(),
                        args: opargs,
                        return_ip: Some(ScriptIp{index: self.bx.threads.cur_ref().trap.ip.index + 1, body: self.bx.threads.cur_ref().trap.ip.body})
                    };
                    self.bx.threads.cur().scopes.push(args);
                    self.bx.threads.cur().calls.push(call);
                    self.bx.threads.cur().trap.ip = sip;
                    return false // Script: skip pop_to_me, RETURN will handle it via call.args
                }
            }
        }
        else{
            let value = script_err_wrong_value!(self.bx.threads.cur_ref().trap, "call target is not a function (got {:?})", self.bx.heap.proto(args).value_type());
            self.bx.threads.cur().push_stack_unchecked(value);
            self.bx.threads.cur().trap.goto_next();
            return true // Error: caller should handle pop_to_me
        }
    }
    
    pub(crate) fn handle_method_call_args(&mut self) -> bool {
        let method = self.bx.threads.cur().pop_stack_value();
        let sself = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let fnobj = if let Some(obj) = sself.as_object(){
            self.bx.heap.object_method(obj, method, NoTrap)
        }
        else if let Some(pod) = sself.as_pod(){
            self.bx.heap.pod_method(pod, method, NoTrap)
        }
        else{
            NIL
        };
                        
        let args = if fnobj.is_err() || fnobj == NIL{
            let method = method.as_id().unwrap_or(id!());
            let type_index = sself.value_type().to_redux();
            let type_entry = &self.bx.code.native.borrow().type_table[type_index.to_index()];
            if let Some(method_ptr) = type_entry.get(&method){
                self.bx.heap.new_with_proto((*method_ptr).into())
            }
            else{ 
                script_err_not_found!(self.bx.threads.cur_ref().trap, "method {:?} not found on type {:?}", method, type_index);
                self.bx.heap.new_with_proto(id!(undefined_function).into())
            }
        }
        else{
            if let Some(ty) = self.bx.heap.pod_type(fnobj){
                let pod = self.bx.heap.new_pod(ty);
                self.bx.threads.cur().mes.push(ScriptMe::Pod{pod, offset: ScriptPodOffset::default()});
                self.bx.threads.cur().trap.goto_next();
                return true // Pod: caller should return early, skip pop_to_me
            }
            self.bx.heap.new_with_proto(fnobj)
        };
        self.bx.heap.clear_object_deep(args);
        
        self.bx.threads.cur().mes.push(ScriptMe::Call{args, sself: Some(sself)});
        self.bx.threads.cur().trap.goto_next();
        false
    }

    // Fn def handlers
    
    pub(crate) fn handle_fn_args(&mut self) {
        let scope = *self.bx.threads.cur().scopes.last_mut().unwrap();
        let me = self.bx.heap.new_with_proto(scope.into());
        self.bx.heap.set_object_storage_vec2(me);
        self.bx.heap.clear_object_deep(me);
        self.bx.threads.cur().mes.push(ScriptMe::Object(me));
        self.bx.threads.cur().trap.goto_next();
    }
    
    pub(crate) fn handle_fn_let_args(&mut self) {
        let id = self.bx.threads.cur().pop_stack_value().as_id().unwrap_or(id!());
        let scope = *self.bx.threads.cur().scopes.last_mut().unwrap();
        let me = self.bx.heap.new_with_proto(scope.into());
        self.bx.heap.set_object_storage_vec2(me);
        self.bx.heap.clear_object_deep(me);
        self.bx.threads.cur().mes.push(ScriptMe::Object(me));
        self.bx.threads.cur().def_scope_value(&mut self.bx.heap, id, me.into());
        self.bx.threads.cur().trap.goto_next();
    }
    
    pub(crate) fn handle_fn_arg_dyn(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil(){
            NIL
        }
        else{
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let id = self.bx.threads.cur().pop_stack_value().as_id().unwrap_or(id!());
        
        match self.bx.threads.cur_ref().mes.last().unwrap(){
            ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                script_err_unexpected!(self.bx.threads.cur_ref().trap, "FN_ARG_DYN for {:?}: expected Object context on stack", id);
            }
            ScriptMe::Object(obj) => {
                if id == id!(self) && self.bx.heap.vec_len(*obj) == 0 {
                    // ignore self as first argument
                }
                else {
                    self.bx.heap.set_value(*obj, id.into(), value, NoTrap);
                }
            }
        };
        self.bx.threads.cur().trap.goto_next();
    }
    
    pub(crate) fn handle_fn_arg_typed(&mut self, opargs: OpcodeArgs) {
        let _value = if opargs.is_nil(){
            NIL
        }
        else{
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let ty = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let id = self.bx.threads.cur().pop_stack_value().as_id().unwrap_or(id!());
        match self.bx.threads.cur_ref().mes.last().unwrap(){
            ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                script_err_unexpected!(self.bx.threads.cur_ref().trap, "FN_ARG_TYPED for {:?}: expected Object context on stack", id);
            }
            ScriptMe::Object(obj) => {
                self.bx.heap.set_value(*obj, id.into(), ty, NoTrap);
            }
        };
        self.bx.threads.cur().trap.goto_next();
    }
    
    pub(crate) fn handle_fn_body_dyn(&mut self, opargs: OpcodeArgs) {
        let jump_over_fn = opargs.to_u32();
        if let Some(me) = self.bx.threads.cur().mes.pop(){
            match me{
                ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                    script_err_unexpected!(self.bx.threads.cur_ref().trap, "FN_BODY_DYN: expected Object context for function body, got {:?}", me);
                    self.bx.threads.cur().push_stack_unchecked(NIL);
                }
                ScriptMe::Object(obj) => {
                    self.bx.heap.set_fn(obj, ScriptFnPtr::Script(
                        ScriptIp{body: self.bx.threads.cur_ref().trap.ip.body, index: (self.bx.threads.cur_ref().trap.ip() + 1)}
                    ));
                    self.bx.threads.cur().push_stack_unchecked(obj.into());
                }
            };
            self.bx.threads.cur().trap.goto_rel(jump_over_fn);
        }
        else{
            script_err_unexpected!(self.bx.threads.cur_ref().trap, "FN_BODY_DYN: me stack is empty (function definition without arguments block)");
            self.bx.threads.cur().push_stack_unchecked(NIL);
            self.bx.threads.cur().trap.goto_next();
        }
    }
    
    pub(crate) fn handle_fn_body_typed(&mut self, opargs: OpcodeArgs) {
        let jump_over_fn = opargs.to_u32();
        let _return_type = self.bx.threads.cur().pop_stack_value();
        if let Some(me) = self.bx.threads.cur().mes.pop(){
            match me{
                ScriptMe::Call{..} | ScriptMe::Array(_) | ScriptMe::Pod{..} => {
                    script_err_unexpected!(self.bx.threads.cur_ref().trap, "FN_BODY_TYPED: expected Object context for typed function body, got {:?}", me);
                    self.bx.threads.cur().push_stack_unchecked(NIL);
                }
                ScriptMe::Object(obj) => {
                    self.bx.heap.set_fn(obj, ScriptFnPtr::Script(
                        ScriptIp{body: self.bx.threads.cur_ref().trap.ip.body, index: (self.bx.threads.cur_ref().trap.ip() + 1)}
                    ));
                    self.bx.threads.cur().push_stack_unchecked(obj.into());
                }
            };
            self.bx.threads.cur().trap.goto_rel(jump_over_fn);
        }
        else{
            script_err_unexpected!(self.bx.threads.cur_ref().trap, "FN_BODY_TYPED: me stack is empty (typed function definition without arguments block)");
            self.bx.threads.cur().push_stack_unchecked(NIL);
            self.bx.threads.cur().trap.goto_next();
        }
    }
}
