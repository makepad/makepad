//! Opcode for loop helper functions
//!
//! This module contains the implementation of for loop iteration,
//! including begin_for_loop, end_for_loop, break_for_loop, and related helpers.

use crate::makepad_live_id::*;
use crate::value::*;
use crate::vm::ScriptVm;
use crate::thread::*;

impl<'a> ScriptVm<'a> {
    pub fn begin_for_loop_inner(&mut self, jump: u32, source: ScriptValue, value_id: LiveId, index_id: Option<LiveId>, key_id: Option<LiveId>, first_value: ScriptValue, first_index: f64, first_key: ScriptValue) {    
        self.bx.threads.cur().trap.goto_next();
        let bases = self.bx.threads.cur_ref().new_bases();
        let start_ip = self.bx.threads.cur_ref().trap.ip();
        self.bx.threads.cur().loops.push(LoopFrame{
            bases,
            start_ip,
            values: Some(LoopValues{
                value_id,
                key_id,
                index_id,
                source,
                index: first_index,
            }),
            jump,
        });
        let scope = *self.bx.threads.cur_ref().scopes.last().unwrap();
        let new_scope = self.bx.heap.new_with_proto(scope.into());

        self.bx.threads.cur().scopes.push(new_scope);
        self.bx.heap.set_value_def(new_scope, value_id.into(), first_value);
        if let Some(key_id) = key_id{
            self.bx.heap.set_value_def(new_scope, key_id.into(), first_key);
        }
        if let Some(index_id) = index_id{
            self.bx.heap.set_value_def(new_scope, index_id.into(), first_index.into());
        }
    }
    
    pub fn begin_loop(&mut self, jump: u32) {   
        self.bx.threads.cur().trap.goto_next();
        let bases = self.bx.threads.cur_ref().new_bases();
        let start_ip = self.bx.threads.cur_ref().trap.ip.index;
        self.bx.threads.cur().loops.push(LoopFrame{
            bases,
            start_ip,
            values: None,
            jump,
        });
        let scope = *self.bx.threads.cur_ref().scopes.last().unwrap();
        let new_scope = self.bx.heap.new_with_proto(scope.into());
        self.bx.threads.cur().scopes.push(new_scope);
    }
                
    pub fn begin_for_loop(&mut self, jump: u32, source: ScriptValue, value_id: LiveId, index_id: Option<LiveId>, key_id: Option<LiveId>) {
        let v0 = ScriptValue::from_f64(0.0);
        if let Some(s) = source.as_number(){
            if s >= 1.0{
                self.begin_for_loop_inner(jump, source, value_id, key_id, index_id, v0, 0.0, v0);
                return
            }
        }
        else if let Some(obj) = source.as_object(){
            if self.bx.heap.has_proto(obj, self.bx.code.builtins.range.into()){ // range object
                let start = self.bx.heap.value(obj, id!(start).into(), self.bx.threads.cur().trap.pass()).as_f64().unwrap_or(0.0);
                let end = self.bx.heap.value(obj, id!(end).into(), self.bx.threads.cur().trap.pass()).as_f64().unwrap_or(0.0);
                let v = start.into();
                if (start - end).abs() >= 1.0{
                    self.begin_for_loop_inner(jump, source, value_id, index_id, key_id, v, start, v);
                    return
                }
            }
            else{
                if self.bx.heap.vec_len(obj) > 0{
                    let kv = self.bx.heap.vec_key_value(obj, 0, self.bx.threads.cur().trap.pass());
                    self.begin_for_loop_inner(jump, source, value_id, index_id, key_id, kv.value, 0.0, kv.key);
                    return
                }
            }
        }
        else if let Some(arr) = source.as_array(){
            if self.bx.heap.array_len(arr) > 0{
                let value = self.bx.heap.array_index(arr, 0, self.bx.threads.cur().trap.pass());
                self.begin_for_loop_inner(jump, source, value_id, index_id, key_id, value, 0.0, NIL);
                return
            }
        }
        self.bx.threads.cur().trap.goto_rel(jump);
    }
             
    pub fn end_for_loop(&mut self) {
        let lf = self.bx.threads.cur().loops.last_mut().unwrap();
        if let Some(values) = &mut lf.values{
            if let Some(end) = values.source.as_number(){
                values.index += 1.0;
                if values.index >= end{
                    self.break_for_loop();
                    return
                }
                let start_ip = lf.start_ip;
                let bases_scope = lf.bases.scope;
                let value_id = values.value_id;
                let index = values.index;
                self.bx.threads.cur().trap.goto(start_ip);
                while self.bx.threads.cur_ref().scopes.len() > bases_scope{
                    let scope = self.bx.threads.cur().scopes.pop().unwrap();
                    self.bx.heap.free_object_if_unreffed(scope);
                }
                let scope = self.bx.heap.new_with_proto((*self.bx.threads.cur_ref().scopes.last().unwrap()).into());
                self.bx.threads.cur().scopes.push(scope);
                self.bx.heap.set_value_def(scope, value_id.into(), index.into());
                return
            }
            else if let Some(obj) = values.source.as_object(){
                if self.bx.heap.has_proto(obj, self.bx.code.builtins.range.into()){
                    let end = self.bx.heap.value(obj, id!(end).into(), self.bx.threads.cur().trap.pass()).as_f64().unwrap_or(0.0);
                    let step = self.bx.heap.value(obj, id!(step).into(), self.bx.threads.cur().trap.pass()).as_f64().unwrap_or(1.0);
                    let lf = self.bx.threads.cur().loops.last_mut().unwrap();
                    let values = lf.values.as_mut().unwrap();
                    values.index += step;
                    if values.index >= end{
                        self.break_for_loop();
                        return
                    }
                    let start_ip = lf.start_ip;
                    let bases_scope = lf.bases.scope;
                    let value_id = values.value_id;
                    let index = values.index;
                    while self.bx.threads.cur_ref().scopes.len() > bases_scope{
                        let scope = self.bx.threads.cur().scopes.pop().unwrap();
                        self.bx.heap.free_object_if_unreffed(scope);
                    }
                    let scope = self.bx.heap.new_with_proto((*self.bx.threads.cur_ref().scopes.last().unwrap()).into());
                    self.bx.threads.cur().scopes.push(scope);
                    self.bx.heap.set_value_def(scope, value_id.into(), index.into());
                    self.bx.threads.cur().trap.goto(start_ip);
                    return
                }
                else{
                    let lf = self.bx.threads.cur().loops.last_mut().unwrap();
                    let values = lf.values.as_mut().unwrap();
                    values.index += 1.0;
                    let index = values.index;
                    let source = values.source;
                    let obj = source.as_object().unwrap();
                    if index >= self.bx.heap.vec_len(obj) as f64{
                        self.break_for_loop();
                        return
                    }
                    let kv = self.bx.heap.vec_key_value(obj, index as usize, self.bx.threads.cur().trap.pass());
                    
                    let lf = self.bx.threads.cur().loops.last_mut().unwrap();
                    let values = lf.values.as_ref().unwrap();
                    let start_ip = lf.start_ip;
                    let bases_scope = lf.bases.scope;
                    let value_id = values.value_id;
                    let index_id = values.index_id;
                    let key_id = values.key_id;
                    let index = values.index;
                    
                    while self.bx.threads.cur_ref().scopes.len() > bases_scope{
                        let scope = self.bx.threads.cur().scopes.pop().unwrap();
                        self.bx.heap.free_object_if_unreffed(scope);
                    }
                    let scope = self.bx.heap.new_with_proto((*self.bx.threads.cur_ref().scopes.last().unwrap()).into());
                    self.bx.threads.cur().scopes.push(scope);
                    self.bx.heap.set_value_def(scope, value_id.into(), kv.value.into());
                    if let Some(index_id) = index_id{
                        self.bx.heap.set_value_def(scope, index_id.into(), index.into());
                    }
                    if let Some(key_id) = key_id{
                        self.bx.heap.set_value_def(scope, key_id.into(), kv.key);
                    }
                    
                    self.bx.threads.cur().trap.goto(start_ip);
                    return
                }
            }
            else if let Some(arr) = values.source.as_array(){
                values.index += 1.0;
                let index = values.index;
                if index >= self.bx.heap.array_len(arr) as f64{
                    self.break_for_loop();
                    return
                }
                let value = self.bx.heap.array_index(arr, index as usize, self.bx.threads.cur().trap.pass());
                                    
                let lf = self.bx.threads.cur().loops.last_mut().unwrap();
                let values = lf.values.as_ref().unwrap();
                let start_ip = lf.start_ip;
                let bases_scope = lf.bases.scope;
                let value_id = values.value_id;
                let index_id = values.index_id;
                let index = values.index;
                
                while self.bx.threads.cur_ref().scopes.len() > bases_scope{
                    let scope = self.bx.threads.cur().scopes.pop().unwrap();
                    self.bx.heap.free_object_if_unreffed(scope);
                }
                let scope = self.bx.heap.new_with_proto((*self.bx.threads.cur_ref().scopes.last().unwrap()).into());
                self.bx.threads.cur().scopes.push(scope);
                
                self.bx.heap.set_value_def(scope, value_id.into(), value.into());
                if let Some(index_id) = index_id{
                    self.bx.heap.set_value_def(scope, index_id.into(), index.into());
                }
                                    
                self.bx.threads.cur().trap.goto(start_ip);
                return
            }
        }
        else{
            let start_ip = self.bx.threads.cur_ref().loops.last().unwrap().start_ip;
            self.bx.threads.cur().trap.goto(start_ip);
            return
        }
        println!("For end unknown state");
        self.bx.threads.cur().trap.goto_next();
    }
                    
    pub fn break_for_loop(&mut self) {
        let lp = self.bx.threads.cur().loops.pop().unwrap();
        self.bx.threads.cur().truncate_bases(lp.bases, &mut self.bx.heap);
        self.bx.threads.cur().trap.goto(lp.start_ip + lp.jump - 1);
    }
    
    pub fn pop_to_me(&mut self) {
        let value = self.bx.threads.cur().pop_stack_value();
        if self.bx.threads.cur_ref().call_has_me(){
            let (key, value) = if let Some(id) = value.as_id(){
                if value.is_escaped_id(){ (NIL, value) }
                else{(value, self.bx.threads.cur().scope_value(&self.bx.heap, id))}
            }else{(NIL, value)};
                        
            match self.bx.threads.cur_ref().mes.last().unwrap(){
                ScriptMe::Call{args, ..} => {
                    let args = *args;
                    self.bx.heap.unnamed_fn_arg(args, value, self.bx.threads.cur().trap.pass());
                }
                ScriptMe::Object(obj) => {
                    let obj = *obj;
                    if !value.is_nil() && !value.is_err(){
                        self.bx.heap.vec_push(obj, key, value, self.bx.threads.cur().trap.pass());
                    }
                }
                ScriptMe::Pod{pod, offset} => {
                    let pod = *pod;
                    let mut offset_copy = *offset;
                    self.bx.heap.pod_pop_to_me(pod, &mut offset_copy, key, value, &self.bx.code.builtins.pod, self.bx.threads.cur().trap.pass());
                    // Write the updated offset back
                    if let Some(ScriptMe::Pod{offset, ..}) = self.bx.threads.cur().mes.last_mut() {
                        *offset = offset_copy;
                    }
                }
                ScriptMe::Array(arr) => {
                    let arr = *arr;
                    self.bx.heap.array_push(arr, value, self.bx.threads.cur().trap.pass())
                }
            }
        }
    }
}
