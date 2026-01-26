//! Opcode variable and field operations
//!
//! This module contains handle functions for variable declarations (let, var),
//! field access, use statements, object/array construction, logging, and related operations.

use crate::makepad_live_id::*;
use crate::makepad_error_log::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::vm::*;
use crate::thread::*;
use std::any::Any;

impl ScriptThread {
    // Object/Array begin handlers
    
    pub(crate) fn handle_begin_proto(&mut self, heap: &mut ScriptHeap) {
        let proto = self.pop_stack_resolved(heap);
        let me = heap.new_with_proto_checked(proto, &self.trap);
        self.mes.push(ScriptMe::Object(me));
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_begin_proto_me(&mut self, heap: &mut ScriptHeap) {
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
    
    pub(crate) fn handle_end_proto(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        let me = self.mes.pop().unwrap();
        if let ScriptMe::Object(me) = me{
            heap.finalize_maybe_pod_type(me, &code.builtins.pod, &self.trap);
        }
        self.push_stack_unchecked(me.into());
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_begin_bare(&mut self, heap: &mut ScriptHeap) {
        let me = heap.new_object();
        self.mes.push(ScriptMe::Object(me));
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_end_bare(&mut self) {
        let me = self.mes.pop().unwrap();
        self.push_stack_unchecked(me.into());
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_begin_array(&mut self, heap: &mut ScriptHeap) {
        let me = heap.new_array();
        self.mes.push(ScriptMe::Array(me));
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_end_array(&mut self) {
        let me = self.mes.pop().unwrap();
        self.push_stack_unchecked(me.into());
        self.trap.goto_next();
    }

    // Use handler
    
    pub(crate) fn handle_use(&mut self, heap: &mut ScriptHeap) {
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
    }

    // Field handlers
    
    pub(crate) fn handle_field(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, host: &mut dyn Any) {
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
                thread: self,
                code
            }, object, field);
            self.push_stack_unchecked(ret);
        }
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_field_nil(&mut self, heap: &mut ScriptHeap) {
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
    
    pub(crate) fn handle_me_field(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        let field = self.pop_stack_value();
        let value = match self.mes.last().unwrap(){
            ScriptMe::Array(_) => {
                self.trap.err_not_allowed_in_array()
            }
            ScriptMe::Call{args, ..} => {
                heap.value(*args, field, &self.trap)
            }
            ScriptMe::Pod{pod, ..} => {
                heap.pod_read_field(*pod, field, &code.builtins.pod, &self.trap)
            }
            ScriptMe::Object(obj) => {
                heap.value(*obj, field, &self.trap)
            }
        };
        self.push_stack_value(value);
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_proto_field(&mut self, heap: &mut ScriptHeap) {
        let field = self.pop_stack_value();
        let object = self.pop_stack_resolved(heap);
        println!("PROTO FIELD {}", field);
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
    
    pub(crate) fn handle_pop_to_me(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        self.pop_to_me(heap, code);
        self.trap.goto_next();
    }

    // Array index handler
    
    pub(crate) fn handle_array_index(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
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
        else if let Some(pod) = object.as_pod(){
            let index = index.as_index();
            let value = heap.pod_array_index(pod, index, &code.builtins.pod, &self.trap);
            self.push_stack_unchecked(value)
        }
        else{
            let value = self.trap.err_not_object();
            self.push_stack_unchecked(value);
        }
        self.trap.goto_next();
    }

    // Let handlers
    
    pub(crate) fn handle_let_dyn(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
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
    
    pub(crate) fn handle_let_typed(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
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
    
    pub(crate) fn handle_var_dyn(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
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
    
    pub(crate) fn handle_var_typed(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
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

    // Tree search handler
    
    pub(crate) fn handle_search_tree(&mut self) {
        self.trap.goto_next();
    }

    // Log handler
    
    pub(crate) fn handle_log(&mut self, heap: &ScriptHeap, code: &ScriptCode) {
        let value = self.peek_stack_resolved(heap);
        self.log(heap, code, value);
        self.trap.goto_next();
    }

    // Me/Scope handlers
    
    pub(crate) fn handle_me(&mut self) {
        if self.call_has_me(){
            match self.mes.last().unwrap(){
                ScriptMe::Array(arr) => {
                    self.push_stack_value((*arr).into());
                }
                ScriptMe::Call{args, ..} => {
                    self.push_stack_value((*args).into());
                }
                ScriptMe::Pod{pod, ..} => {
                    self.push_stack_value((*pod).into());
                }
                ScriptMe::Object(obj) => {
                    self.push_stack_value((*obj).into());
                }
            }
        }
        else{
            self.push_stack_value(NIL);
        }
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_scope(&mut self) {
        let scope = *self.scopes.last_mut().unwrap();
        self.push_stack_value(scope.into());
        self.trap.goto_next();
    }

    // Log implementation
    
    pub fn log(&self, heap: &ScriptHeap, code: &ScriptCode, value: ScriptValue){
        if let Some(loc) = code.ip_to_loc(self.trap.ip){
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
                    heap.to_debug_string(value, &mut recur, &mut out, true, 0);
                    log_with_level(&loc.file, loc.line, loc.col, loc.line, loc.col, format!("{:?}:{out}", value.value_type()), LogLevel::Log);
                }
            }
            else{
                log_with_level(&loc.file, loc.line, loc.col, loc.line, loc.col, format!("nil"), LogLevel::Log);
            }
        }
    }
}
