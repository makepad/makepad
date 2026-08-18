//! Frame-slot opcode handlers.
//!
//! Slot-compiled fn bodies keep lexically resolved locals in the thread's
//! slot slab (`thread.slots`) instead of scope-object maps. Every handler
//! here mirrors the observable behavior of its dynamic counterpart exactly
//! (pop order, pushed results, escape barrier, string/number semantics of
//! compound assigns) — only the storage location differs. The `[id]` stream
//! slot of the dynamic forms is still present (shape parity for parser
//! patch-back) and is popped and discarded.

use crate::opcode::*;
use crate::value::*;
use crate::vm::ScriptVm;

impl<'a> ScriptVm<'a> {
    pub(crate) fn handle_slots_frame(&mut self, opargs: OpcodeArgs) {
        let n = opargs.to_u32() as usize;
        let thread = self.bx.threads.cur();
        thread.slot_base = thread.slots.len();
        let new_len = thread.slot_base + n;
        thread.slots.resize(new_len, NIL);
        thread.trap.goto_next();
    }

    /// Copy the declared args of the current call into slots 0..n, with the
    /// same lookup the dynamic path uses: bound value from the call scope's
    /// map, else the definition-time default from the fn object's vec.
    pub(crate) fn handle_args_to_slots(&mut self, opargs: OpcodeArgs) {
        let n = opargs.to_u32();
        let Some(&scope) = self.bx.threads.cur_ref().scopes.last() else {
            self.bail("scopes empty in args_to_slots");
            return;
        };
        let fnobj = self.bx.heap.proto(scope).as_object();
        for i in 0..n {
            let mut val = NIL;
            if let Some(fnobj) = fnobj {
                let (name, default) = {
                    let obj = &self.bx.heap.objects[fnobj];
                    if let Some(kv) = obj.vec.get(i as usize) {
                        (kv.key, kv.value)
                    } else {
                        (NIL, NIL)
                    }
                };
                if !name.is_nil() {
                    val = self.bx.heap.objects[scope]
                        .map
                        .get(&name)
                        .map(|v| v.value)
                        .unwrap_or(default);
                }
            }
            self.bx.threads.cur().set_slot(i, val);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_push_slot(&mut self, opargs: OpcodeArgs) {
        let thread = self.bx.threads.cur();
        let v = thread.slot(opargs.to_u32());
        thread.push_stack_value(v);
        thread.trap.goto_next();
    }

    pub(crate) fn handle_let_slot(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let _id = self.bx.threads.cur().pop_stack_value();
        // parity with def_scope_value: stored values pass the escape barrier
        self.bx.heap.escape_value(value);
        self.bx.threads.cur().set_slot(opargs.to_u32(), value);
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_store_slot(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let _id = self.bx.threads.cur().pop_stack_value();
        // parity with set_scope_value: escape barrier, NIL result pushed
        self.bx.heap.escape_value(value);
        self.bx.threads.cur().set_slot(opargs.to_u32(), value);
        self.bx.threads.cur().push_stack_unchecked(NIL);
        self.bx.threads.cur().trap.goto_next();
    }

    /// `+=` on a slot: mirrors handle_assign_add including the string-concat
    /// branch and the stored-error passthrough.
    pub(crate) fn handle_assign_slot_add(&mut self, opargs: OpcodeArgs) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let _id = self.bx.threads.cur().pop_stack_value();
        let slot = opargs.to_u32();
        let old = self.bx.threads.cur().slot(slot);
        if old.is_err() {
            self.bx.threads.cur().push_stack_unchecked(old);
        } else if old.is_string_like() || value.is_string_like() {
            let str = self.bx.heap.new_string_concat(old, value);
            self.bx.threads.cur().set_slot(slot, str);
            self.bx.threads.cur().push_stack_unchecked(NIL);
        } else {
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old, ip);
            let fb = self.bx.heap.cast_to_f64(value, ip);
            self.bx
                .threads
                .cur()
                .set_slot(slot, ScriptValue::from_f64_traced_nan(fa + fb, ip));
            self.bx.threads.cur().push_stack_unchecked(NIL);
        }
        self.bx.threads.cur().trap.goto_next();
    }

    /// Numeric compound assign on a slot (-=, *=, /=, %=): mirrors
    /// handle_f64_scope_assign_op / scope_rmw_numeric.
    pub(crate) fn handle_slot_num_assign_op<F>(&mut self, opargs: OpcodeArgs, f: F)
    where
        F: FnOnce(f64, f64) -> f64,
    {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let _id = self.bx.threads.cur().pop_stack_value();
        let slot = opargs.to_u32();
        let old = self.bx.threads.cur().slot(slot);
        if old.is_err() {
            self.bx.threads.cur().push_stack_unchecked(old);
        } else {
            let ip = self.bx.threads.cur_ref().trap.ip;
            let fa = self.bx.heap.cast_to_f64(old, ip);
            let fb = self.bx.heap.cast_to_f64(value, ip);
            self.bx
                .threads
                .cur()
                .set_slot(slot, ScriptValue::from_f64_traced_nan(f(fa, fb), ip));
            self.bx.threads.cur().push_stack_unchecked(NIL);
        }
        self.bx.threads.cur().trap.goto_next();
    }
}
