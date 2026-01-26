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
    
    /// Part 1 of proto-inherit (+:) operator.
    /// Reads the field from current me (following prototype chain or type-check),
    /// leaves field on stack, and pushes the proto value on stack for BEGIN_PROTO.
    pub(crate) fn handle_proto_inherit_read(&mut self, heap: &mut ScriptHeap) {
        let field = self.peek_stack_value();
        let me = self.mes.last().unwrap();
        let proto = if let ScriptMe::Object(object) = me {
            // First try to get value from prototype chain (handles inheritance)
            let value = heap.proto_field_from_value(*object, field, &self.trap);
            
            // If value not found, try to create from type-check structure
            if value.is_nil() || value.is_err() {
                self.trap.err.take(); // Clear any error from value lookup
                if let Some(field_id) = field.as_id() {
                    heap.proto_field_from_type_check(*object, field_id, &self.trap)
                } else {
                    NIL
                }
            } else {
                value
            }
        } else {
            NIL
        };
        self.push_stack_unchecked(proto);
        self.trap.goto_next();
    }
    
    /// Part 2 of proto-inherit (+:) operator.
    /// Pops the constructed object and field from stack, writes object to current me[field].
    /// Pushes NIL to satisfy POP_TO_ME (the assignment has no result value).
    pub(crate) fn handle_proto_inherit_write(&mut self, heap: &mut ScriptHeap) {
        let object = self.pop_stack_resolved(heap);
        let field = self.pop_stack_value();
        if let Some(me) = self.mes.last() {
            if let ScriptMe::Object(me_obj) = me {
                if field.is_string_like() {
                    heap.set_string_keys(*me_obj);
                }
                heap.set_value(*me_obj, field, object, &self.trap);
            }
        }
        // Push NIL as result so POP_TO_ME has something to pop
        self.push_stack_unchecked(NIL);
        self.trap.goto_next();
    }
    
    /// Part 1 of scope-inherit (value += {}) operator.
    /// Peeks the identifier from stack, reads scope variable value, pushes proto value on stack.
    pub(crate) fn handle_scope_inherit_read(&mut self, heap: &mut ScriptHeap) {
        let id = self.peek_stack_value();
        let proto = if let Some(id) = id.as_id() {
            let value = self.scope_value(heap, id);
            // If not found or error, clear error and use NIL (will create bare object)
            if value.is_nil() || value.is_err() {
                self.trap.err.take();
                NIL
            } else {
                value
            }
        } else {
            NIL
        };
        self.push_stack_unchecked(proto);
        self.trap.goto_next();
    }
    
    /// Part 2 of scope-inherit (value += {}) operator.
    /// Pops the constructed object and identifier from stack, assigns to scope variable.
    /// Pushes NIL to satisfy POP_TO_ME (the assignment has no result value).
    pub(crate) fn handle_scope_inherit_write(&mut self, heap: &mut ScriptHeap) {
        let object = self.pop_stack_resolved(heap);
        let id = self.pop_stack_value();
        if let Some(id) = id.as_id() {
            self.set_scope_value(heap, id, object);
        }
        // Push NIL as result so POP_TO_ME has something to pop
        self.push_stack_unchecked(NIL);
        self.trap.goto_next();
    }
    
    /// Part 1 of field-inherit (obj.field += {}) operator.
    /// Stack has [object, field]. Peeks both, reads object.field, pushes proto value.
    pub(crate) fn handle_field_inherit_read(&mut self, heap: &ScriptHeap) {
        let field = self.peek_stack_value();
        let object = self.peek_stack_value_at(1);
        // Resolve if it's an identifier
        let object = if let Some(id) = object.as_id() {
            if !object.is_escaped_id() {
                self.scope_value(heap, id)
            } else {
                object
            }
        } else {
            object
        };
        let proto = if let Some(obj) = object.as_object() {
            let value = heap.value(obj, field, &self.trap);
            if value.is_nil() || value.is_err() {
                self.trap.err.take();
                NIL
            } else {
                value
            }
        } else {
            NIL
        };
        self.push_stack_unchecked(proto);
        self.trap.goto_next();
    }
    
    /// Part 2 of field-inherit (obj.field += {}) operator.
    /// Pops built_object, field, object from stack. Writes built_object to object.field.
    /// Pushes NIL to satisfy POP_TO_ME.
    pub(crate) fn handle_field_inherit_write(&mut self, heap: &mut ScriptHeap) {
        let built_object = self.pop_stack_resolved(heap);
        let field = self.pop_stack_value();
        let object = self.pop_stack_resolved(heap);
        if let Some(obj) = object.as_object() {
            if field.is_string_like() {
                heap.set_string_keys(obj);
            }
            heap.set_value(obj, field, built_object, &self.trap);
        }
        // Push NIL as result so POP_TO_ME has something to pop
        self.push_stack_unchecked(NIL);
        self.trap.goto_next();
    }
    
    /// Part 1 of index-inherit (obj[index] += {}) operator.
    /// Stack has [object, index]. Peeks both, reads object[index], pushes proto value.
    pub(crate) fn handle_index_inherit_read(&mut self, heap: &ScriptHeap) {
        let index = self.peek_stack_value();
        let object = self.peek_stack_value_at(1);
        // Resolve if it's an identifier
        let object = if let Some(id) = object.as_id() {
            if !object.is_escaped_id() {
                self.scope_value(heap, id)
            } else {
                object
            }
        } else {
            object
        };
        let proto = if let Some(obj) = object.as_object() {
            let value = heap.value(obj, index, &self.trap);
            if value.is_nil() || value.is_err() {
                self.trap.err.take();
                NIL
            } else {
                value
            }
        } else if let Some(arr) = object.as_array() {
            let idx = index.as_index();
            let value = heap.array_index(arr, idx, &self.trap);
            if value.is_nil() || value.is_err() {
                self.trap.err.take();
                NIL
            } else {
                value
            }
        } else {
            NIL
        };
        self.push_stack_unchecked(proto);
        self.trap.goto_next();
    }
    
    /// Part 2 of index-inherit (obj[index] += {}) operator.
    /// Pops built_object, index, object from stack. Writes built_object to object[index].
    /// Pushes NIL to satisfy POP_TO_ME.
    pub(crate) fn handle_index_inherit_write(&mut self, heap: &mut ScriptHeap) {
        let built_object = self.pop_stack_resolved(heap);
        let index = self.pop_stack_value();
        let object = self.pop_stack_resolved(heap);
        if let Some(obj) = object.as_object() {
            heap.set_value(obj, index, built_object, &self.trap);
        } else if let Some(arr) = object.as_array() {
            let idx = index.as_index();
            heap.set_array_index(arr, idx, built_object, &self.trap);
        }
        // Push NIL as result so POP_TO_ME has something to pop
        self.push_stack_unchecked(NIL);
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
        if let Some(obj) = object.as_object(){
            // First try to get value from prototype chain (handles inheritance)
            let value = heap.proto_field_from_value(obj, field, &self.trap);
            
            // If value not found, try to create from type-check structure
            if value.is_nil() || value.is_err() {
                self.trap.err.take(); // Clear any error from value lookup
                if let Some(field_id) = field.as_id() {
                    let value = heap.proto_field_from_type_check(obj, field_id, &self.trap);
                    self.push_stack_unchecked(value);
                } else {
                    let value = self.trap.err_not_found();
                    self.push_stack_unchecked(value);
                }
            } else {
                self.push_stack_unchecked(value)
            }
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
