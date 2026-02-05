//! Opcode variable and field operations
//!
//! This module contains handle functions for variable declarations (let, var),
//! field access, use statements, object/array construction, logging, and related operations.

use crate::makepad_error_log::*;
use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::thread::*;
use crate::value::*;
use crate::vm::*;
use crate::*;

impl<'a> ScriptVm<'a> {
    // Object/Array begin handlers

    pub(crate) fn handle_begin_proto(&mut self) {
        let proto = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let me = self
            .bx
            .heap
            .new_with_proto_checked(proto, self.bx.threads.cur().trap.pass());
        self.bx.threads.cur().mes.push(ScriptMe::Object(me));
        self.bx.threads.cur().trap.goto_next();
    }

    /// Part 1 of proto-inherit (+:) operator.
    pub(crate) fn handle_proto_inherit_read(&mut self) {
        let field = self.bx.threads.cur().peek_stack_value();
        let me = self.bx.threads.cur_ref().mes.last().unwrap();
        let proto = if let ScriptMe::Object(object) = me {
            let object = *object;
            let value = self.bx.heap.proto_field_from_value(
                object,
                field,
                self.bx.threads.cur().trap.pass(),
            );
            if value.is_nil() || value.is_err() {
                self.bx.threads.cur().trap.err.take();
                if let Some(field_id) = field.as_id() {
                    self.bx.heap.proto_field_from_type_check(
                        object,
                        field_id,
                        self.bx.threads.cur().trap.pass(),
                    )
                } else {
                    NIL
                }
            } else {
                value
            }
        } else {
            NIL
        };
        self.bx.threads.cur().push_stack_unchecked(proto);
        self.bx.threads.cur().trap.goto_next();
    }

    /// Part 2 of proto-inherit (+:) operator.
    pub(crate) fn handle_proto_inherit_write(&mut self) {
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        if let Some(me) = self.bx.threads.cur_ref().mes.last() {
            if let ScriptMe::Object(me_obj) = me {
                let me_obj = *me_obj;
                if field.is_string_like() {
                    self.bx.heap.set_string_keys(me_obj);
                }
                self.bx
                    .heap
                    .set_value(me_obj, field, object, self.bx.threads.cur().trap.pass());
            }
        }
        self.bx.threads.cur().push_stack_unchecked(NIL);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_scope_inherit_read(&mut self) {
        let id = self.bx.threads.cur().peek_stack_value();
        let proto = if let Some(id) = id.as_id() {
            let value = self.bx.threads.cur().scope_value(&self.bx.heap, id);
            if value.is_nil() || value.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                value
            }
        } else {
            NIL
        };
        self.bx.threads.cur().push_stack_unchecked(proto);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_scope_inherit_write(&mut self) {
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let id = self.bx.threads.cur().pop_stack_value();
        if let Some(id) = id.as_id() {
            self.bx
                .threads
                .cur()
                .set_scope_value(&mut self.bx.heap, id, object);
        }
        self.bx.threads.cur().push_stack_unchecked(NIL);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_field_inherit_read(&mut self) {
        let field = self.bx.threads.cur().peek_stack_value();
        let object = self.bx.threads.cur().peek_stack_value_at(1);
        let object = if let Some(id) = object.as_id() {
            if !object.is_escaped_id() {
                self.bx.threads.cur().scope_value(&self.bx.heap, id)
            } else {
                object
            }
        } else {
            object
        };
        let proto = if let Some(obj) = object.as_object() {
            let value = self
                .bx
                .heap
                .value(obj, field, self.bx.threads.cur().trap.pass());
            if value.is_nil() || value.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                value
            }
        } else {
            NIL
        };
        self.bx.threads.cur().push_stack_unchecked(proto);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_field_inherit_write(&mut self) {
        let built_object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            if field.is_string_like() {
                self.bx.heap.set_string_keys(obj);
            }
            self.bx
                .heap
                .set_value(obj, field, built_object, self.bx.threads.cur().trap.pass());
        }
        self.bx.threads.cur().push_stack_unchecked(NIL);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_index_inherit_read(&mut self) {
        let index = self.bx.threads.cur().peek_stack_value();
        let object = self.bx.threads.cur().peek_stack_value_at(1);
        let object = if let Some(id) = object.as_id() {
            if !object.is_escaped_id() {
                self.bx.threads.cur().scope_value(&self.bx.heap, id)
            } else {
                object
            }
        } else {
            object
        };
        let proto = if let Some(obj) = object.as_object() {
            let value = self
                .bx
                .heap
                .value(obj, index, self.bx.threads.cur().trap.pass());
            if value.is_nil() || value.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                value
            }
        } else if let Some(arr) = object.as_array() {
            let idx = index.as_index();
            let value = self
                .bx
                .heap
                .array_index(arr, idx, self.bx.threads.cur().trap.pass());
            if value.is_nil() || value.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                value
            }
        } else {
            NIL
        };
        self.bx.threads.cur().push_stack_unchecked(proto);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_index_inherit_write(&mut self) {
        let built_object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let index = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            self.bx
                .heap
                .set_value(obj, index, built_object, self.bx.threads.cur().trap.pass());
        } else if let Some(arr) = object.as_array() {
            let idx = index.as_index();
            self.bx
                .heap
                .set_array_index(arr, idx, built_object, self.bx.threads.cur().trap.pass());
        }
        self.bx.threads.cur().push_stack_unchecked(NIL);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_end_proto(&mut self) {
        let me = self.bx.threads.cur().mes.pop().unwrap();
        if let ScriptMe::Object(me) = me {
            self.bx.heap.finalize_maybe_pod_type(
                me,
                &self.bx.code.builtins.pod,
                self.bx.threads.cur().trap.pass(),
            );
        }
        self.bx.threads.cur().push_stack_unchecked(me.into());
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_begin_bare(&mut self) {
        let me = self.bx.heap.new_object();
        self.bx.threads.cur().mes.push(ScriptMe::Object(me));
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_end_bare(&mut self) {
        let me = self.bx.threads.cur().mes.pop().unwrap();
        self.bx.threads.cur().push_stack_unchecked(me.into());
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_begin_array(&mut self) {
        let me = self.bx.heap.new_array();
        self.bx.threads.cur().mes.push(ScriptMe::Array(me));
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_end_array(&mut self) {
        let me = self.bx.threads.cur().mes.pop().unwrap();
        self.bx.threads.cur().push_stack_unchecked(me.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // Use handler

    pub(crate) fn handle_use(&mut self) {
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            if field.as_id() == Some(id!(*)) {
                let mut items = Vec::new();
                let obj_data = &self.bx.heap.objects[obj];
                for (k, v) in obj_data.map.iter() {
                    items.push((*k, v.value));
                }
                for item in &obj_data.vec {
                    items.push((item.key, item.value));
                }
                for (k, v) in items {
                    if let Some(id) = k.as_id() {
                        self.bx
                            .threads
                            .cur()
                            .def_scope_value(&mut self.bx.heap, id, v);
                    }
                }
            } else {
                let value = self
                    .bx
                    .heap
                    .value(obj, field, self.bx.threads.cur().trap.pass());
                if !value.is_nil() {
                    if let Some(field) = field.as_id() {
                        self.bx
                            .threads
                            .cur()
                            .def_scope_value(&mut self.bx.heap, field, value);
                    }
                }
            }
        }
        self.bx.threads.cur().trap.goto_next();
    }

    // Field handlers

    pub(crate) fn handle_field(&mut self) {
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let value = self
                .bx
                .heap
                .value(obj, field, self.bx.threads.cur().trap.pass());
            self.bx.threads.cur().push_stack_unchecked(value);
        } else if let Some(pod) = object.as_pod() {
            let value = self.bx.heap.pod_read_field(
                pod,
                field,
                &self.bx.code.builtins.pod,
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let field = field.as_id().unwrap_or(id!());
            let type_index = object.value_type().to_redux();
            // Get the getter pointer and drop the borrow before calling
            let getter_ptr: *const dyn Fn(&mut ScriptVm, ScriptValue, LiveId) -> ScriptValue = {
                let native = self.bx.code.native.borrow();
                &*native.getters[type_index.to_index()] as *const _
            };
            // SAFETY: The getter pointer is valid as long as native getters aren't removed during execution
            let ret = unsafe { (*getter_ptr)(self, object, field) };
            self.bx.threads.cur().push_stack_unchecked(ret);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_field_nil(&mut self) {
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let value = self
                .bx
                .heap
                .value(obj, field, self.bx.threads.cur().trap.pass());
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            self.bx.threads.cur().push_stack_unchecked(NIL);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_me_field(&mut self) {
        let field = self.bx.threads.cur().pop_stack_value();
        let value = match self.bx.threads.cur_ref().mes.last().unwrap() {
            ScriptMe::Array(_) => {
                script_err_not_allowed!(
                    self.bx.threads.cur_ref().trap,
                    "field access {:?} not allowed in array literal context",
                    field
                )
            }
            ScriptMe::Call { args, .. } => {
                let args = *args;
                self.bx
                    .heap
                    .value(args, field, self.bx.threads.cur().trap.pass())
            }
            ScriptMe::Pod { pod, .. } => {
                let pod = *pod;
                self.bx.heap.pod_read_field(
                    pod,
                    field,
                    &self.bx.code.builtins.pod,
                    self.bx.threads.cur().trap.pass(),
                )
            }
            ScriptMe::Object(obj) => {
                let obj = *obj;
                self.bx
                    .heap
                    .value(obj, field, self.bx.threads.cur().trap.pass())
            }
        };
        self.bx.threads.cur().push_stack_value(value);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_proto_field(&mut self) {
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let value =
                self.bx
                    .heap
                    .proto_field_from_value(obj, field, self.bx.threads.cur().trap.pass());
            if value.is_nil() || value.is_err() {
                self.bx.threads.cur().trap.err.take();
                if let Some(field_id) = field.as_id() {
                    let value = self.bx.heap.proto_field_from_type_check(
                        obj,
                        field_id,
                        self.bx.threads.cur().trap.pass(),
                    );
                    self.bx.threads.cur().push_stack_unchecked(value);
                } else {
                    let value = script_err_not_found!(
                        self.bx.threads.cur_ref().trap,
                        "proto field lookup requires identifier, got {:?}",
                        field.value_type()
                    );
                    self.bx.threads.cur().push_stack_unchecked(value);
                }
            } else {
                self.bx.threads.cur().push_stack_unchecked(value)
            }
        } else {
            let value = script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "proto_field {:?} target is not an object (got {:?})",
                field,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_pop_to_me(&mut self) {
        self.pop_to_me();
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_me_splat(&mut self) {
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if !self.bx.threads.cur_ref().call_has_me() {
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        match self.bx.threads.cur_ref().mes.last().unwrap() {
            ScriptMe::Object(obj) => {
                let obj = *obj;
                if let Some(source_obj) = source.as_object() {
                    self.bx
                        .heap
                        .merge_object(obj, source_obj, self.bx.threads.cur().trap.pass());
                } else if let Some(source_arr) = source.as_array() {
                    let len = self.bx.heap.array_len(source_arr);
                    for i in 0..len {
                        let v = self.bx.heap.array_index(
                            source_arr,
                            i,
                            self.bx.threads.cur().trap.pass(),
                        );
                        self.bx
                            .heap
                            .vec_push(obj, NIL, v, self.bx.threads.cur().trap.pass());
                    }
                }
            }
            ScriptMe::Array(arr) => {
                let arr = *arr;
                if let Some(source_arr) = source.as_array() {
                    self.bx
                        .heap
                        .merge_array(arr, source_arr, self.bx.threads.cur().trap.pass());
                } else if let Some(source_obj) = source.as_object() {
                    self.bx
                        .heap
                        .array_push_vec(arr, source_obj, self.bx.threads.cur().trap.pass());
                }
            }
            ScriptMe::Call { args, .. } => {
                let args = *args;
                if let Some(source_obj) = source.as_object() {
                    let len = self.bx.heap.vec_len(source_obj);
                    for i in 0..len {
                        let kv = self.bx.heap.vec_key_value(
                            source_obj,
                            i,
                            self.bx.threads.cur().trap.pass(),
                        );
                        self.bx.heap.unnamed_fn_arg(
                            args,
                            kv.value,
                            self.bx.threads.cur().trap.pass(),
                        );
                    }
                } else if let Some(source_arr) = source.as_array() {
                    let len = self.bx.heap.array_len(source_arr);
                    for i in 0..len {
                        let v = self.bx.heap.array_index(
                            source_arr,
                            i,
                            self.bx.threads.cur().trap.pass(),
                        );
                        self.bx
                            .heap
                            .unnamed_fn_arg(args, v, self.bx.threads.cur().trap.pass());
                    }
                }
            }
            ScriptMe::Pod { .. } => {
                script_err_not_impl!(
                    self.bx.threads.cur_ref().trap,
                    "splat operator (..) not supported for pod types"
                );
            }
        }
        self.bx.threads.cur().trap.goto_next();
    }

    // Array index handler

    pub(crate) fn handle_array_index(&mut self) {
        let index = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        if let Some(obj) = object.as_object() {
            let value = self
                .bx
                .heap
                .value(obj, index, self.bx.threads.cur().trap.pass());
            self.bx.threads.cur().push_stack_unchecked(value)
        } else if let Some(arr) = object.as_array() {
            let index = index.as_index();
            let value = self
                .bx
                .heap
                .array_index(arr, index, self.bx.threads.cur().trap.pass());
            self.bx.threads.cur().push_stack_unchecked(value)
        } else if let Some(pod) = object.as_pod() {
            let index = index.as_index();
            let value = self.bx.heap.pod_array_index(
                pod,
                index,
                &self.bx.code.builtins.pod,
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value)
        } else {
            let value = script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "cannot index {:?} on {:?} (not an object/array/pod)",
                index,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    // Let handlers

    pub(crate) fn handle_let_dyn(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let id = self.bx.threads.cur().pop_stack_value();
        let id = id.as_id().unwrap_or(id!());
        self.bx
            .threads
            .cur()
            .def_scope_value(&mut self.bx.heap, id, value);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_let_typed(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let _ty = self.bx.threads.cur().pop_stack_value();
        let id = self
            .bx
            .threads
            .cur()
            .pop_stack_value()
            .as_id()
            .unwrap_or(id!());
        self.bx
            .threads
            .cur()
            .def_scope_value(&mut self.bx.heap, id, value);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_var_dyn(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let id = self.bx.threads.cur().pop_stack_value();
        let id = id.as_id().unwrap_or(id!());
        self.bx
            .threads
            .cur()
            .def_scope_value(&mut self.bx.heap, id, value);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_var_typed(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let _ty = self.bx.threads.cur().pop_stack_value();
        let id = self
            .bx
            .threads
            .cur()
            .pop_stack_value()
            .as_id()
            .unwrap_or(id!());
        self.bx
            .threads
            .cur()
            .def_scope_value(&mut self.bx.heap, id, value);
        self.bx.threads.cur().trap.goto_next();
    }

    // Tree search handler

    pub(crate) fn handle_search_tree(&mut self) {
        self.bx.threads.cur().trap.goto_next();
    }

    // Log handler

    pub(crate) fn handle_log(&mut self) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        self.log(value);
        self.bx.threads.cur().trap.goto_next();
    }

    // Me/Scope handlers

    pub(crate) fn handle_me(&mut self) {
        let value = if self.bx.threads.cur_ref().call_has_me() {
            match self.bx.threads.cur_ref().mes.last().unwrap() {
                ScriptMe::Array(arr) => (*arr).into(),
                ScriptMe::Call { args, .. } => (*args).into(),
                ScriptMe::Pod { pod, .. } => (*pod).into(),
                ScriptMe::Object(obj) => (*obj).into(),
            }
        } else {
            NIL
        };
        self.bx.threads.cur().push_stack_value(value);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_scope(&mut self) {
        let scope = *self.bx.threads.cur().scopes.last_mut().unwrap();
        self.bx.threads.cur().push_stack_value(scope.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // Log implementation

    pub fn log(&self, value: ScriptValue) {
        if let Some(loc) = self.bx.code.ip_to_loc(self.bx.threads.cur_ref().trap.ip) {
            if value != NIL {
                if let Some(err_ptr) = value.as_err() {
                    if let Some(loc2) = self.bx.code.ip_to_loc(err_ptr.ip) {
                        let err_queue = self.bx.threads.cur_ref().trap.err.borrow();
                        if let Some(err) = err_queue.iter().find(|e| e.value == value) {
                            log_with_level(
                                &loc.file,
                                loc.line,
                                loc.col,
                                loc.line,
                                loc.col,
                                format!(
                                    "{} ({}:{}) {}",
                                    err.message, err.origin_file, err.origin_line, loc2
                                ),
                                LogLevel::Log,
                            );
                        } else {
                            log_with_level(
                                &loc.file,
                                loc.line,
                                loc.col,
                                loc.line,
                                loc.col,
                                format!("{} {}", value, loc2),
                                LogLevel::Log,
                            );
                        }
                    }
                } else if let Some(nanip) = value.as_f64_traced_nan() {
                    if let Some(loc2) = self.bx.code.ip_to_loc(nanip) {
                        log_with_level(
                            &loc.file,
                            loc.line,
                            loc.col,
                            loc.line,
                            loc.col,
                            format!("{} NaN Traced to {}", value, loc2),
                            LogLevel::Log,
                        );
                    }
                } else {
                    let mut out = String::new();
                    let mut recur = Vec::new();
                    self.bx
                        .heap
                        .to_debug_string(value, &mut recur, &mut out, true, 0);
                    log_with_level(
                        &loc.file,
                        loc.line,
                        loc.col,
                        loc.line,
                        loc.col,
                        format!("{:?}:{out}", value.value_type()),
                        LogLevel::Log,
                    );
                }
            } else {
                log_with_level(
                    &loc.file,
                    loc.line,
                    loc.col,
                    loc.line,
                    loc.col,
                    format!("nil"),
                    LogLevel::Log,
                );
            }
        }
    }

    // Destructuring handlers

    /// DUP - duplicate top of stack
    pub(crate) fn handle_dup(&mut self) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        self.bx.threads.cur().push_stack_unchecked(value);
        self.bx.threads.cur().trap.goto_next();
    }

    /// DROP - discard top of stack
    pub(crate) fn handle_drop(&mut self) {
        self.bx.threads.cur().pop_stack_value();
        self.bx.threads.cur().trap.goto_next();
    }

    /// ARRAY_INDEX_NIL - like ARRAY_INDEX but returns nil instead of error
    /// Stack: [source, index] -> [value_or_nil]
    pub(crate) fn handle_array_index_nil(&mut self) {
        let index = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        let value = if let Some(arr) = source.as_array() {
            let idx = index.as_index();
            // Try to get, return NIL if out of bounds or error
            let result = self
                .bx
                .heap
                .array_index(arr, idx, self.bx.threads.cur().trap.pass());
            if result.is_err() {
                self.bx.threads.cur().trap.err.take(); // Clear the error
                NIL
            } else {
                result
            }
        } else if let Some(obj) = source.as_object() {
            let result = self
                .bx
                .heap
                .value(obj, index, self.bx.threads.cur().trap.pass());
            if result.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                result
            }
        } else {
            NIL
        };

        self.bx.threads.cur().push_stack_unchecked(value);
        self.bx.threads.cur().trap.goto_next();
    }

    /// LET_DESTRUCT_ARRAY_EL(index) - destructure array element
    /// Stack: [source, id] -> [source]
    /// Binds: id = source[index] (nil-safe extraction)
    pub(crate) fn handle_let_destruct_array_el(&mut self, opargs: OpcodeArgs) {
        let index = opargs.to_u32() as usize;
        let id = self.bx.threads.cur().pop_stack_value();
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        // Extract value from source array at index (nil-safe)
        let value = if let Some(arr) = source.as_array() {
            let result = self
                .bx
                .heap
                .array_index(arr, index, self.bx.threads.cur().trap.pass());
            if result.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                result
            }
        } else if let Some(obj) = source.as_object() {
            let result = self.bx.heap.value(
                obj,
                ScriptValue::from_u32(index as u32),
                self.bx.threads.cur().trap.pass(),
            );
            if result.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                result
            }
        } else {
            NIL
        };

        // Bind the value to the identifier
        if let Some(id) = id.as_id() {
            self.bx
                .threads
                .cur()
                .def_scope_value(&mut self.bx.heap, id, value);
        }

        // Push source back on stack for next element
        self.bx.threads.cur().push_stack_unchecked(source);
        self.bx.threads.cur().trap.goto_next();
    }

    /// LET_DESTRUCT_OBJECT_EL - destructure object element
    /// Stack: [source, id] -> [source]
    /// Binds: id = source[id] (nil-safe extraction)
    pub(crate) fn handle_let_destruct_object_el(&mut self) {
        let id = self.bx.threads.cur().pop_stack_value();
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        // Extract value from source object using id as key (nil-safe)
        let value = if let Some(obj) = source.as_object() {
            let result = self
                .bx
                .heap
                .value(obj, id, self.bx.threads.cur().trap.pass());
            if result.is_err() {
                self.bx.threads.cur().trap.err.take();
                NIL
            } else {
                result
            }
        } else {
            NIL
        };

        // Bind the value to the identifier
        if let Some(id) = id.as_id() {
            self.bx
                .threads
                .cur()
                .def_scope_value(&mut self.bx.heap, id, value);
        }

        // Push source back on stack for next element
        self.bx.threads.cur().push_stack_unchecked(source);
        self.bx.threads.cur().trap.goto_next();
    }
}
