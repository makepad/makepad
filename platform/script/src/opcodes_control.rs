//! Opcode control flow operations
//!
//! This module contains handle functions for control flow: if/else statements,
//! for loops, return statements, range, is, and try/ok operations.

use crate::makepad_live_id::*;
use crate::opcode::*;
use crate::thread::*;
use crate::trap::*;
use crate::value::*;
use crate::vm::ScriptVm;
use crate::*;

impl<'a> ScriptVm<'a> {
    // IF handlers

    pub(crate) fn handle_if_test(&mut self, opargs: OpcodeArgs) {
        let test = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let test = self.bx.heap.cast_to_bool(test);
        if test {
            self.bx.threads.cur().trap.goto_next()
        } else {
            if opargs.is_need_nil() {
                self.bx.threads.cur().push_stack_unchecked(NIL);
            }
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        }
    }

    pub(crate) fn handle_if_else(&mut self, opargs: OpcodeArgs) {
        self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
    }

    // RETURN handlers

    pub(crate) fn handle_return(&mut self, opargs: OpcodeArgs) {
        let value = if opargs.is_nil() {
            NIL
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let Some(call) = self.bx.threads.cur().calls.pop() else {
            self.bail("calls empty in handle_return");
            return;
        };
        self.bx
            .threads
            .cur()
            .truncate_bases(call.bases, &mut self.bx.heap);

        if let Some(ret) = call.return_ip {
            self.bx.threads.cur().trap.ip = ret;
            self.bx.threads.cur().push_stack_unchecked(value);
            if call.args.is_pop_to_me() {
                self.pop_to_me();
            }
        } else {
            self.bx
                .threads
                .cur()
                .trap
                .on
                .set(Some(ScriptTrapOn::Return(value)));
        }
    }

    pub(crate) fn handle_return_if_err(&mut self, _opargs: OpcodeArgs) -> bool {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        if value.is_err() {
            let Some(call) = self.bx.threads.cur().calls.pop() else {
                self.bail("calls empty in handle_return_if_err");
                return true;
            };
            self.bx
                .threads
                .cur()
                .truncate_bases(call.bases, &mut self.bx.heap);
            if let Some(ret) = call.return_ip {
                self.bx.threads.cur().trap.ip = ret;
                self.bx.threads.cur().push_stack_unchecked(value);
                if call.args.is_pop_to_me() {
                    self.pop_to_me();
                }
            } else {
                self.bx
                    .threads
                    .cur()
                    .trap
                    .on
                    .set(Some(ScriptTrapOn::Return(value)));
            }
            true
        } else {
            self.bx.threads.cur().trap.goto_next();
            false
        }
    }

    // For loop handlers

    pub(crate) fn handle_for_1(&mut self, opargs: OpcodeArgs) {
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let Some(value_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_1 value_id not an id");
            return;
        };
        self.begin_for_loop(opargs.to_u32() as _, source, value_id, None, None);
    }

    pub(crate) fn handle_for_2(&mut self, opargs: OpcodeArgs) {
        // for k v in source: k = key (obj) or index (array/range), v = value
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let Some(value_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_2 value_id not an id");
            return;
        };
        let Some(first_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_2 first_id not an id");
            return;
        };
        // Pass first_id as key_id - for objects it gets key, for arrays/ranges it gets index
        self.begin_for_loop(opargs.to_u32() as _, source, value_id, None, Some(first_id));
    }

    pub(crate) fn handle_for_3(&mut self, opargs: OpcodeArgs) {
        // for i k v in source: i = index, k = key, v = value (objects only)
        let source = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let Some(value_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_3 value_id not an id");
            return;
        };
        let Some(key_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_3 key_id not an id");
            return;
        };
        let Some(index_id) = self.bx.threads.cur().pop_stack_value().as_id() else {
            self.bail("for_3 index_id not an id");
            return;
        };
        self.begin_for_loop(
            opargs.to_u32() as _,
            source,
            value_id,
            Some(index_id),
            Some(key_id),
        );
    }

    pub(crate) fn handle_loop(&mut self, opargs: OpcodeArgs) {
        self.begin_loop(opargs.to_u32() as _);
    }

    pub(crate) fn handle_for_end(&mut self) {
        self.end_for_loop();
    }

    pub(crate) fn handle_break(&mut self) {
        self.break_for_loop();
    }

    pub(crate) fn handle_breakifnot(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if !self.bx.heap.cast_to_bool(value) {
            self.break_for_loop();
        } else {
            self.bx.threads.cur().trap.goto_next();
        }
    }

    pub(crate) fn handle_continue(&mut self) {
        self.end_for_loop();
    }

    // Range handler

    pub(crate) fn handle_range(&mut self) {
        let end = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let start = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if !start.is_number() {
            let err = script_err_type_mismatch!(
                self.bx.threads.cur().trap,
                "range must start with a number, did you forget a , before a splat operator?"
            );
            self.bx.threads.cur().push_stack_unchecked(err);
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        if !end.is_number() {
            let err =
                script_err_type_mismatch!(self.bx.threads.cur().trap, "range end must be a number");
            self.bx.threads.cur().push_stack_unchecked(err);
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        let range = self
            .bx
            .heap
            .new_with_proto(self.bx.code.builtins.range.into());
        self.bx.heap.set_value_def(range, id!(start).into(), start);
        self.bx.heap.set_value_def(range, id!(end).into(), end);
        self.bx.threads.cur().push_stack_unchecked(range.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // Is handler

    pub(crate) fn handle_is(&mut self) {
        let rhs = self.bx.threads.cur().pop_stack_value();
        let lhs = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let cmp = if let Some(id) = rhs.as_id() {
            let ty = lhs.value_type();
            let is_number = id == id!(number).into();
            match ty {
                // All numeric types match "is number"
                ScriptValueType::F64
                | ScriptValueType::F32
                | ScriptValueType::F16
                | ScriptValueType::U32
                | ScriptValueType::I32
                | ScriptValueType::U40 => is_number,
                ScriptValueType::NAN => is_number || id == id!(nan).into(),
                ScriptValueType::BOOL => id == id!(bool).into(),
                ScriptValueType::NIL => id == id!(nil).into(),
                ScriptValueType::COLOR => id == id!(color).into(),
                ScriptValueType::OBJECT => {
                    id == id!(object).into() || {
                        if let Some(rhs) = self
                            .bx
                            .threads
                            .cur()
                            .scope_value(&self.bx.heap, id)
                            .as_object()
                        {
                            if let Some(obj) = lhs.as_object() {
                                self.bx.heap.has_proto(obj, rhs.into())
                            } else {
                                false
                            }
                        } else {
                            false
                        }
                    }
                }
                ScriptValueType::ARRAY => id == id!(array).into(),
                ScriptValueType::REGEX => id == id!(regex).into(),
                _ if ty.to_redux() == ScriptValueType::REDUX_STRING => id == id!(string).into(),
                _ if ty.to_redux() == ScriptValueType::REDUX_ID => id == id!(id).into(),
                _ => false,
            }
        } else if rhs.is_nil() {
            // `x is nil` where nil is the actual value
            lhs.is_nil()
        } else if let Some(obj) = lhs.as_object() {
            self.bx.heap.has_proto(obj, rhs)
        } else {
            false
        };
        self.bx.threads.cur().push_stack_unchecked(cmp.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // Short-circuit evaluation handlers

    /// || short-circuit: if first operand is truthy, skip second operand and keep value
    pub(crate) fn handle_logic_or_test(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        let test = self.bx.heap.cast_to_bool(value);
        if test {
            // First operand is truthy - skip second operand, keep value on stack
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        } else {
            // First operand is falsy - pop it and evaluate second operand
            self.bx.threads.cur().pop_stack_value();
            self.bx.threads.cur().trap.goto_next();
        }
    }

    /// && short-circuit: if first operand is falsy, skip second operand and keep value
    pub(crate) fn handle_logic_and_test(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        let test = self.bx.heap.cast_to_bool(value);
        if !test {
            // First operand is falsy - skip second operand, keep value on stack
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        } else {
            // First operand is truthy - pop it and evaluate second operand
            self.bx.threads.cur().pop_stack_value();
            self.bx.threads.cur().trap.goto_next();
        }
    }

    /// |? short-circuit: if first operand is not nil, skip second operand and keep value
    pub(crate) fn handle_nil_or_test(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().peek_stack_resolved(&self.bx.heap);
        if !value.is_nil() {
            // First operand is not nil - skip second operand, keep value on stack
            self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
        } else {
            // First operand is nil - pop it and evaluate second operand
            self.bx.threads.cur().pop_stack_value();
            self.bx.threads.cur().trap.goto_next();
        }
    }

    // Try / OK handlers

    pub(crate) fn handle_ok_test(&mut self, opargs: OpcodeArgs) {
        let ip = self.bx.threads.cur_ref().trap.ip();
        let bases = self.bx.threads.cur_ref().new_bases();
        self.bx.threads.cur().tries.push(TryFrame {
            push_nil: true,
            start_ip: ip,
            jump: opargs.to_u32() + 1,
            bases,
        });
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_ok_end(&mut self) {
        self.bx.threads.cur().tries.pop();
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_try_test(&mut self, opargs: OpcodeArgs) {
        let ip = self.bx.threads.cur_ref().trap.ip();
        let bases = self.bx.threads.cur_ref().new_bases();
        self.bx.threads.cur().tries.push(TryFrame {
            push_nil: false,
            start_ip: ip,
            jump: opargs.to_u32() + 1,
            bases,
        });
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_try_err(&mut self, opargs: OpcodeArgs) {
        if self.bx.threads.cur().tries.pop().is_none() {
            self.bail("tries empty in handle_try_err");
            return;
        }
        self.bx.threads.cur().trap.goto_rel(opargs.to_u32() + 1);
    }

    pub(crate) fn handle_try_ok(&mut self, opargs: OpcodeArgs) {
        self.bx.threads.cur().trap.goto_rel(opargs.to_u32());
    }
}
