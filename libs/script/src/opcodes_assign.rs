//! Opcode assignment operations
//!
//! This module contains handle functions for assignment operations:
//! ASSIGN, ASSIGN_FIELD, ASSIGN_INDEX, and ASSIGN_ME variants.

use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::thread::*;

impl ScriptThread {
    // ASSIGN handlers
    
    pub(crate) fn handle_assign(&mut self, heap: &mut ScriptHeap) {
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
    
    pub(crate) fn handle_assign_add(&mut self, heap: &mut ScriptHeap) {
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
                let fb = heap.cast_to_f64(value, self.trap.ip);
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
    
    pub(crate) fn handle_assign_ifnil(&mut self, heap: &mut ScriptHeap) {
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

    // ASSIGN FIELD handlers
    
    pub(crate) fn handle_assign_field(&mut self, heap: &mut ScriptHeap) {
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
    
    pub(crate) fn handle_assign_field_add(&mut self, heap: &mut ScriptHeap) {
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
    
    pub(crate) fn handle_assign_field_ifnil(&mut self, heap: &mut ScriptHeap) {
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

    // ASSIGN INDEX handlers
    
    pub(crate) fn handle_assign_index(&mut self, heap: &mut ScriptHeap) {
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
    
    pub(crate) fn handle_assign_index_add(&mut self, heap: &mut ScriptHeap) {
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
    }
    
    pub(crate) fn handle_assign_index_ifnil(&mut self, heap: &mut ScriptHeap) {
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

    // ASSIGN ME handlers
    
    pub(crate) fn handle_assign_me(&mut self, heap: &mut ScriptHeap) {
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
    
    pub(crate) fn handle_assign_me_before_after(&mut self, heap: &mut ScriptHeap, opcode: Opcode) {
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
    
    pub(crate) fn handle_assign_me_begin(&mut self, heap: &mut ScriptHeap) {
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

    // Generic assignment operation handlers

    pub fn handle_f64_scope_assign_op<F>(&mut self, heap: &mut ScriptHeap, f: F)
    where F: FnOnce(f64, f64) -> f64
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
                let value = self.set_scope_value(heap, id, ScriptValue::from_f64_traced_nan(f(fa, fb), self.trap.ip));
                self.push_stack_unchecked(value);
            }
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_fu64_scope_assign_op<F>(&mut self, heap: &mut ScriptHeap, f: F)
    where F: FnOnce(u64, u64) -> u64
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
                let value = self.set_scope_value(heap, id, ScriptValue::from_f64_traced_nan(f(ua, ub) as f64, self.trap.ip));
                self.push_stack_unchecked(value);
            }
        }
        else{
            let value = self.trap.err_not_assignable();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    pub fn handle_f64_field_assign_op<F>(&mut self, heap: &mut ScriptHeap, f: F)
    where F: FnOnce(f64, f64) -> f64
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

    pub fn handle_fu64_field_assign_op<F>(&mut self, heap: &mut ScriptHeap, f: F)
    where F: FnOnce(u64, u64) -> u64
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

    pub fn handle_f64_index_assign_op<F>(&mut self, heap: &mut ScriptHeap, f: F)
    where F: FnOnce(f64, f64) -> f64
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

    pub fn handle_fu64_index_assign_op<F>(&mut self, heap: &mut ScriptHeap, f: F)
    where F: FnOnce(u64, u64) -> u64
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
}
