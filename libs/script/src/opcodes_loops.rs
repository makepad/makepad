//! Opcode for loop helper functions
//!
//! This module contains the implementation of for loop iteration,
//! including begin_for_loop, end_for_loop, break_for_loop, and related helpers.

use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::vm::ScriptCode;
use crate::thread::*;

impl ScriptThread {
    pub fn begin_for_loop_inner(&mut self, heap: &mut ScriptHeap, jump: u32, source: ScriptValue, value_id: LiveId, index_id: Option<LiveId>, key_id: Option<LiveId>, first_value: ScriptValue, first_index: f64, first_key: ScriptValue) {    
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
    
    pub fn begin_loop(&mut self, heap: &mut ScriptHeap, jump: u32) {   
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
                
    pub fn begin_for_loop(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, jump: u32, source: ScriptValue, value_id: LiveId, index_id: Option<LiveId>, key_id: Option<LiveId>) {
        let v0 = ScriptValue::from_f64(0.0);
        if let Some(s) = source.as_number(){
            if s >= 1.0{
                self.begin_for_loop_inner(heap, jump, source, value_id, key_id, index_id, v0, 0.0, v0);
                return
            }
        }
        else if let Some(obj) = source.as_object(){
            if heap.has_proto(obj, code.builtins.range.into()){ // range object
                let start = heap.value(obj, id!(start).into(), self.trap.pass()).as_f64().unwrap_or(0.0);
                let end = heap.value(obj, id!(end).into(), self.trap.pass()).as_f64().unwrap_or(0.0);
                let v = start.into();
                if (start - end).abs() >= 1.0{
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, v, start, v);
                    return
                }
            }
            else{
                if heap.vec_len(obj) > 0{
                    let kv = heap.vec_key_value(obj, 0, self.trap.pass());
                    self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, kv.value, 0.0, kv.key);
                    return
                }
            }
        }
        else if let Some(arr) = source.as_array(){
            if heap.array_len(arr) > 0{
                let value = heap.array_index(arr, 0, self.trap.pass());
                self.begin_for_loop_inner(heap, jump, source, value_id, index_id, key_id, value, 0.0, NIL);
                return
            }
        }
        // jump over it and bail
        self.trap.goto_rel(jump);
    }
             
    pub fn end_for_loop(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        // alright lets take a look at our top loop thing
        let lf = self.loops.last_mut().unwrap();
        if let Some(values) = &mut lf.values{
            if let Some(end) = values.source.as_number(){
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
                    let end = heap.value(obj, id!(end).into(), self.trap.pass()).as_f64().unwrap_or(0.0);
                    let step = heap.value(obj, id!(step).into(), self.trap.pass()).as_f64().unwrap_or(1.0);
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
                    let kv = heap.vec_key_value(obj, values.index as usize, self.trap.pass());
                    
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
                let value = heap.array_index(arr, values.index as usize, self.trap.pass());
                                    
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
                    
    pub fn break_for_loop(&mut self, heap: &mut ScriptHeap) {
        let lp = self.loops.pop().unwrap();
        self.truncate_bases(lp.bases, heap);
        self.trap.goto(lp.start_ip + lp.jump - 1);
    }
    
    pub fn pop_to_me(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        let value = self.pop_stack_value();
        if self.call_has_me(){
            let (key, value) = if let Some(id) = value.as_id(){
                if value.is_escaped_id(){ (NIL, value) }
                else{(value, self.scope_value(heap, id))}
            }else{(NIL, value)};
                        
            match self.mes.last_mut().unwrap(){
                ScriptMe::Call{args, ..} => {
                    heap.unnamed_fn_arg(*args, value, self.trap.pass());
                }
                ScriptMe::Object(obj) => {
                    if !value.is_nil() && !value.is_err(){
                        heap.vec_push(*obj, key, value, self.trap.pass());
                    }
                }
                ScriptMe::Pod{pod, offset} => {
                    heap.pod_pop_to_me(*pod, offset, key, value, &code.builtins.pod, self.trap.pass());
                }
                ScriptMe::Array(arr) => {
                    heap.array_push(*arr, value, self.trap.pass())
                }
            }
        }
    }
}
