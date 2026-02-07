//! Opcode assignment operations
//!
//! This module contains handle functions for assignment operations:
//! ASSIGN, ASSIGN_FIELD, ASSIGN_INDEX, and ASSIGN_ME variants.

use crate::opcode::*;
use crate::thread::*;
use crate::value::*;
use crate::vm::ScriptVm;
use crate::*;

impl<'a> ScriptVm<'a> {
    // ASSIGN handlers

    pub(crate) fn handle_assign(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let id = self.bx.threads.cur().pop_stack_value();
        if let Some(id) = id.as_id() {
            let value = self
                .bx
                .threads
                .cur()
                .set_scope_value(&mut self.bx.heap, id, value);
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "assign target is not an identifier, got {:?}",
                id.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_assign_add(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let id = self.bx.threads.cur().pop_stack_value();
        if let Some(id) = id.as_id() {
            let old_value = self.bx.threads.cur().scope_value(&self.bx.heap, id);
            if old_value.is_err() {
                self.bx.threads.cur().push_stack_unchecked(old_value);
            } else if old_value.is_string_like() || value.is_string_like() {
                let str = self.bx.heap.new_string_with(|heap, out| {
                    heap.cast_to_string(old_value, out);
                    heap.cast_to_string(value, out);
                });
                self.bx
                    .threads
                    .cur()
                    .set_scope_value(&mut self.bx.heap, id, str.into());
                self.bx.threads.cur().push_stack_unchecked(NIL);
            } else {
                let ip = self.bx.threads.cur_ref().trap.ip;
                let fa = self.bx.heap.cast_to_f64(old_value, ip);
                let fb = self.bx.heap.cast_to_f64(value, ip);
                let value = self.bx.threads.cur().set_scope_value(
                    &mut self.bx.heap,
                    id,
                    ScriptValue::from_f64_traced_nan(fa + fb, ip),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            }
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "+= target is not an identifier, got {:?}",
                id.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    /// ASSIGN_IFNIL - lazy evaluation: checks if scope var is nil
    /// Stack: [id] -> [id] (continue) or [] (jump)
    /// If scope[id] is NOT nil: pop id, push nil, jump to skip RHS
    /// If scope[id] IS nil: leave id on stack, continue to evaluate RHS then ASSIGN
    pub(crate) fn handle_assign_ifnil(&mut self, opargs: OpcodeArgs) {
        let id = self.bx.threads.cur().peek_stack_value();
        if let Some(id_val) = id.as_id() {
            let va = self.bx.threads.cur().scope_value(&self.bx.heap, id_val);
            if !va.is_err() && !va.is_nil() {
                // Value is NOT nil - skip the RHS evaluation
                // Pop the id from stack and push nil as result
                self.bx.threads.cur().pop_stack_value();
                self.bx.threads.cur().push_stack_unchecked(NIL);
                // Jump past the RHS and ASSIGN
                self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
            } else {
                // Value IS nil - continue to evaluate RHS
                // Leave id on stack for ASSIGN to use later
                self.bx.threads.cur().trap.goto_next();
            }
        } else {
            // Not an identifier - error
            self.bx.threads.cur().pop_stack_value();
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "?= target is not an identifier, got {:?}",
                id.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
            self.bx.threads.cur().trap.goto_next();
        }
    }

    // ASSIGN FIELD handlers

    pub(crate) fn handle_assign_field(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let value =
                self.bx
                    .heap
                    .set_value(obj, field, value, self.bx.threads.cur().trap.pass());
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let value = script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "cannot assign field {:?} on {:?} (not an object)",
                field,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_assign_field_add(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, field, self.bx.threads.cur().trap.pass());
            if old_value.is_string_like() || value.is_string_like() {
                let str = self.bx.heap.new_string_with(|heap, out| {
                    heap.cast_to_string(old_value, out);
                    heap.cast_to_string(value, out);
                });
                let value = self.bx.heap.set_value(
                    obj,
                    field,
                    str.into(),
                    self.bx.threads.cur().trap.pass(),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            } else {
                let ip = self.bx.threads.cur_ref().trap.ip;
                let fa = self.bx.heap.cast_to_f64(old_value, ip);
                let fb = self.bx.heap.cast_to_f64(value, ip);
                let value = self.bx.heap.set_value(
                    obj,
                    field,
                    ScriptValue::from_f64_traced_nan(fa + fb, ip),
                    self.bx.threads.cur().trap.pass(),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            }
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "cannot += field {:?} on {:?} (not an object)",
                field,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_assign_field_ifnil(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, field, self.bx.threads.cur().trap.pass());
            if old_value.is_err() || old_value.is_nil() {
                let value =
                    self.bx
                        .heap
                        .set_value(obj, field, value, self.bx.threads.cur().trap.pass());
                self.bx.threads.cur().push_stack_unchecked(value);
            } else {
                self.bx.threads.cur().push_stack_unchecked(NIL);
            }
        } else {
            let value = script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "cannot ?= field {:?} on {:?} (not an object)",
                field,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    // ASSIGN INDEX handlers

    pub(crate) fn handle_assign_index(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let index = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let value =
                self.bx
                    .heap
                    .set_value(obj, index, value, self.bx.threads.cur().trap.pass());
            self.bx.threads.cur().push_stack_unchecked(value);
        } else if let Some(arr) = object.as_array() {
            let value =
                self.bx
                    .heap
                    .array_index(arr, index.as_index(), self.bx.threads.cur().trap.pass());
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let value = script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "cannot assign index {:?} on {:?} (not an object/array)",
                index,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_assign_index_add(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let index = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, index, self.bx.threads.cur().trap.pass());
            if old_value.is_string_like() || value.is_string_like() {
                let str = self.bx.heap.new_string_with(|heap, out| {
                    heap.cast_to_string(old_value, out);
                    heap.cast_to_string(value, out);
                });
                let value = self.bx.heap.set_value(
                    obj,
                    index,
                    str.into(),
                    self.bx.threads.cur().trap.pass(),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            } else {
                let ip = self.bx.threads.cur_ref().trap.ip;
                let fa = self.bx.heap.cast_to_f64(old_value, ip);
                let fb = self.bx.heap.cast_to_f64(value, ip);
                let value = self.bx.heap.set_value(
                    obj,
                    index,
                    ScriptValue::from_f64_traced_nan(fa + fb, ip),
                    self.bx.threads.cur().trap.pass(),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            }
        } else if let Some(arr) = object.as_array() {
            let index = index.as_index();
            let old_value = self
                .bx
                .heap
                .array_index(arr, index, self.bx.threads.cur().trap.pass());
            if old_value.is_string_like() || value.is_string_like() {
                let str = self.bx.heap.new_string_with(|heap, out| {
                    heap.cast_to_string(old_value, out);
                    heap.cast_to_string(value, out);
                });
                let value = self.bx.heap.set_array_index(
                    arr,
                    index,
                    str.into(),
                    self.bx.threads.cur().trap.pass(),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            } else {
                let ip = self.bx.threads.cur_ref().trap.ip;
                let fa = self.bx.heap.cast_to_f64(old_value, ip);
                let fb = self.bx.heap.cast_to_f64(value, ip);
                let value = self.bx.heap.set_array_index(
                    arr,
                    index,
                    ScriptValue::from_f64_traced_nan(fa + fb, ip),
                    self.bx.threads.cur().trap.pass(),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            }
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "cannot += index {:?} on {:?} (not an object/array)",
                index,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_assign_index_ifnil(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let index = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, index, self.bx.threads.cur().trap.pass());
            if old_value.is_err() || old_value.is_nil() {
                let value =
                    self.bx
                        .heap
                        .set_value(obj, index, value, self.bx.threads.cur().trap.pass());
                self.bx.threads.cur().push_stack_unchecked(value);
            } else {
                self.bx.threads.cur().push_stack_unchecked(NIL);
            }
        } else if let Some(arr) = object.as_array() {
            let index = index.as_index();
            let old_value = self
                .bx
                .heap
                .array_index(arr, index, self.bx.threads.cur().trap.pass());
            if old_value.is_err() || old_value.is_nil() {
                let value = self.bx.heap.set_array_index(
                    arr,
                    index,
                    value,
                    self.bx.threads.cur().trap.pass(),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            } else {
                self.bx.threads.cur().push_stack_unchecked(NIL);
            }
        } else {
            let value = script_err_wrong_value!(
                self.bx.threads.cur_ref().trap,
                "cannot ?= index {:?} on {:?} (not an object/array)",
                index,
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    // ASSIGN ME handlers

    pub(crate) fn handle_assign_me(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        if self.bx.threads.cur_ref().call_has_me() {
            let Some(me) = self.bx.threads.cur_ref().mes.last() else {
                self.bail("mes empty in handle_assign_me");
                return;
            };
            match me {
                ScriptMe::Call { args, .. } => {
                    let args = *args;
                    self.bx.heap.named_fn_arg(
                        args,
                        field,
                        value,
                        self.bx.threads.cur().trap.pass(),
                    );
                }
                ScriptMe::Object(obj) => {
                    let obj = *obj;
                    if field.is_string_like() {
                        self.bx.heap.set_string_keys(obj);
                    }
                    self.bx
                        .heap
                        .set_value(obj, field, value, self.bx.threads.cur().trap.pass());
                }
                ScriptMe::Pod { pod, .. } => {
                    let pod = *pod;
                    self.bx.heap.set_pod_field(
                        pod,
                        field,
                        value,
                        self.bx.threads.cur().trap.pass(),
                    );
                }
                ScriptMe::Array(_arr) => {
                    script_err_not_allowed!(
                        self.bx.threads.cur_ref().trap,
                        "named assign {:?} not allowed in array literal context",
                        field
                    );
                }
            }
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_assign_me_before_after(&mut self, opcode: Opcode) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let Some(me) = self.bx.threads.cur_ref().mes.last() else {
            self.bail("mes empty in handle_assign_me_before_after");
            return;
        };
        let value = match me {
            ScriptMe::Call { .. } | ScriptMe::Pod { .. } => {
                script_err_not_allowed!(
                    self.bx.threads.cur_ref().trap,
                    "before/after {:?} not allowed in function call arguments",
                    field
                )
            }
            ScriptMe::Object(obj) => {
                let obj = *obj;
                self.bx.heap.vec_insert_value_at(
                    obj,
                    field,
                    value,
                    opcode == Opcode::ASSIGN_ME_BEFORE,
                    self.bx.threads.cur().trap.pass(),
                )
            }
            ScriptMe::Array(_arr) => {
                script_err_not_allowed!(
                    self.bx.threads.cur_ref().trap,
                    "before/after {:?} not allowed in array literal context",
                    field
                )
            }
        };
        self.bx.threads.cur().push_stack_unchecked(value);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_assign_me_begin(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let Some(me) = self.bx.threads.cur_ref().mes.last() else {
            self.bail("mes empty in handle_assign_me_begin");
            return;
        };
        let value = match me {
            ScriptMe::Call { .. } | ScriptMe::Pod { .. } => {
                script_err_not_allowed!(
                    self.bx.threads.cur_ref().trap,
                    "begin {:?} not allowed in function call arguments",
                    field
                )
            }
            ScriptMe::Object(obj) => {
                let obj = *obj;
                self.bx.heap.vec_insert_value_begin(
                    obj,
                    field,
                    value,
                    self.bx.threads.cur().trap.pass(),
                )
            }
            ScriptMe::Array(_arr) => {
                script_err_not_allowed!(
                    self.bx.threads.cur_ref().trap,
                    "begin {:?} not allowed in array literal context",
                    field
                )
            }
        };
        self.bx.threads.cur().push_stack_unchecked(value);
        self.bx.threads.cur().trap.goto_next();
    }

    // Generic assignment operation handlers

    pub fn handle_f64_scope_assign_op<F>(&mut self, f: F)
    where
        F: FnOnce(f64, f64) -> f64,
    {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let id = self.bx.threads.cur().pop_stack_value();
        if let Some(id) = id.as_id() {
            let va = self.bx.threads.cur().scope_value(&self.bx.heap, id);
            if va.is_err() {
                self.bx.threads.cur().push_stack_unchecked(va);
            } else {
                let ip = self.bx.threads.cur_ref().trap.ip;
                let fa = self.bx.heap.cast_to_f64(va, ip);
                let fb = self.bx.heap.cast_to_f64(value, ip);
                let value = self.bx.threads.cur().set_scope_value(
                    &mut self.bx.heap,
                    id,
                    ScriptValue::from_f64_traced_nan(f(fa, fb), ip),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            }
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "compound assignment target is not an identifier, got {:?}",
                id.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_fu64_scope_assign_op<F>(&mut self, f: F)
    where
        F: FnOnce(u64, u64) -> u64,
    {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let id = self.bx.threads.cur().pop_stack_value();
        if let Some(id) = id.as_id() {
            let old_value = self.bx.threads.cur().scope_value(&self.bx.heap, id);
            if old_value.is_err() {
                self.bx.threads.cur().push_stack_unchecked(old_value);
            } else {
                let ip = self.bx.threads.cur_ref().trap.ip;
                let ua = self.bx.heap.cast_to_f64(old_value, ip) as u64;
                let ub = self.bx.heap.cast_to_f64(value, ip) as u64;
                let value = self.bx.threads.cur().set_scope_value(
                    &mut self.bx.heap,
                    id,
                    ScriptValue::from_f64_traced_nan(f(ua, ub) as f64, ip),
                );
                self.bx.threads.cur().push_stack_unchecked(value);
            }
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "bitwise compound assignment target is not an identifier, got {:?}",
                id.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_f64_field_assign_op<F>(&mut self, f: F)
    where
        F: FnOnce(f64, f64) -> f64,
    {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, field, self.bx.threads.cur().trap.pass());
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old_value, ip);
            let fb = self.bx.heap.cast_to_f64(value, ip);
            let value = self.bx.heap.set_value(
                obj,
                field,
                ScriptValue::from_f64_traced_nan(f(fa, fb), ip),
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "field compound assignment on {:?} (not an object)",
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_fu64_field_assign_op<F>(&mut self, f: F)
    where
        F: FnOnce(u64, u64) -> u64,
    {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let field = self.bx.threads.cur().pop_stack_value();
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, field, self.bx.threads.cur().trap.pass());
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old_value, ip) as u64;
            let fb = self.bx.heap.cast_to_f64(value, ip) as u64;
            let value = self.bx.heap.set_value(
                obj,
                field,
                ScriptValue::from_f64_traced_nan(f(fa, fb) as f64, ip),
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "field bitwise compound assignment on {:?} (not an object)",
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_f64_index_assign_op<F>(&mut self, f: F)
    where
        F: FnOnce(f64, f64) -> f64,
    {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let index = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, index, self.bx.threads.cur().trap.pass());
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old_value, ip);
            let fb = self.bx.heap.cast_to_f64(value, ip);
            let value = self.bx.heap.set_value(
                obj,
                index,
                ScriptValue::from_f64_traced_nan(f(fa, fb), ip),
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        } else if let Some(arr) = object.as_array() {
            let index = index.as_index();
            let old_value = self
                .bx
                .heap
                .array_index(arr, index, self.bx.threads.cur().trap.pass());
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old_value, ip);
            let fb = self.bx.heap.cast_to_f64(value, ip);
            let value = self.bx.heap.set_array_index(
                arr,
                index,
                ScriptValue::from_f64_traced_nan(f(fa, fb), ip),
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "index compound assignment on {:?} (not an object/array)",
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_fu64_index_assign_op<F>(&mut self, f: F)
    where
        F: FnOnce(u64, u64) -> u64,
    {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let index = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let object = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(obj) = object.as_object() {
            let old_value = self
                .bx
                .heap
                .value(obj, index, self.bx.threads.cur().trap.pass());
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old_value, ip) as u64;
            let fb = self.bx.heap.cast_to_f64(value, ip) as u64;
            let value = self.bx.heap.set_value(
                obj,
                index,
                ScriptValue::from_f64_traced_nan(f(fa, fb) as f64, ip),
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        } else if let Some(arr) = object.as_array() {
            let index = index.as_index();
            let old_value = self
                .bx
                .heap
                .array_index(arr, index, self.bx.threads.cur().trap.pass());
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old_value, ip) as u64;
            let fb = self.bx.heap.cast_to_f64(value, ip) as u64;
            let value = self.bx.heap.set_array_index(
                arr,
                index,
                ScriptValue::from_f64_traced_nan(f(fa, fb) as f64, ip),
                self.bx.threads.cur().trap.pass(),
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        } else {
            let value = script_err_immutable!(
                self.bx.threads.cur_ref().trap,
                "index bitwise compound assignment on {:?} (not an object/array)",
                object.value_type()
            );
            self.bx.threads.cur().push_stack_unchecked(value);
        }
        self.bx.threads.cur().trap.goto_next();
    }
}
