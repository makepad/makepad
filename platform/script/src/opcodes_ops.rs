//! Opcode arithmetic and comparison operations
//!
//! This module contains handle functions for arithmetic operations (+, -, *, /, etc.),
//! comparison operations (==, !=, <, >, etc.), and logical operations (&&, ||).

use crate::numeric::NumericValue;
use crate::opcode::*;
use crate::value::*;
use crate::trap::ScriptTrapOn;
use crate::vm::ScriptVm;
use crate::*;

impl<'a> ScriptVm<'a> {
    // ARITHMETIC handlers

    pub(crate) fn handle_not(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        // `!` always negates truth conversion. The old f64 path did a bitwise
        // NOT (`!1` was 18446744073709551614, not false), so the result
        // depended on whether a number was stored as f64 or as u40.
        let v = self.bx.heap.cast_to_bool(value);
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(ScriptValue::from_bool(!v));
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_neg(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(f) = value.as_number() {
            self.bx
                .threads
                .cur()
                .push_stack_unchecked(ScriptValue::from_f64(-f));
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        let ip = self.bx.threads.cur_ref().trap.ip;
        let num = NumericValue::from_script_value_heap(&self.bx.heap, value, ip);
        let result = num.zip_f32(NumericValue::F64(-1.0), |a, b| a * b);
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_add(&mut self, opargs: OpcodeArgs) {
        let b = if opargs.is_u32() {
            (opargs.to_u32()).into()
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        // number + number is the hot case; string-like and number type tags
        // are disjoint so checking numbers first is order-neutral
        if let (Some(fa), Some(fb)) = (a.as_number(), b.as_number()) {
            let ip = self.bx.threads.cur_ref().trap.ip;
            self.bx
                .threads
                .cur()
                .push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa + fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        if a.is_string_like() || b.is_string_like() {
            let ptr = self.bx.heap.new_string_concat(a, b);
            self.bx.threads.cur().push_stack_unchecked(ptr.into());
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.zip_f32(nb, |x, y| x + y);
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }

    // CONCAT handler

    pub(crate) fn handle_concat(&mut self) {
        let op1 = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let op2 = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let ptr = self.bx.heap.new_string_concat(op1, op2);
        self.bx.threads.cur().push_stack_unchecked(ptr.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // EQUALITY handlers

    pub(crate) fn handle_eq(&mut self) {
        self.handle_structural_equality(false);
    }

    pub(crate) fn handle_neq(&mut self) {
        self.handle_structural_equality(true);
    }

    /// Structural `==` / `!=`. Every unit of native comparison work charges
    /// one instruction of VM fuel, the hard deadline is sampled while the
    /// comparison runs, and `MAX_EQUALITY_WORK` caps a single comparison of a
    /// bounded evaluation.
    /// Exhaustion is an uncatchable bail, exactly like the instruction limit.
    fn handle_structural_equality(&mut self, negate: bool) {
        /// Work units between clock reads. A trivial comparison never
        /// touches the clock; a long one checks the hard deadline often.
        const DEADLINE_SAMPLE_UNITS: u32 = 256;
        let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let deadline = self.bx.run_budget.as_ref().map(|budget| budget.hard_deadline);
        let thread = self.bx.threads.cur();
        // The work ceiling belongs to bounded evaluations. With no instruction
        // limit, run budget or allocation budget the host asked for no bound,
        // and comparing two large arrays is legitimate in trusted code.
        let maximum_work = if thread.instruction_limit_remaining.is_some()
            || deadline.is_some()
            || self.bx.heap.has_allocation_budget()
        {
            crate::equality::MAX_EQUALITY_WORK
        } else {
            usize::MAX
        };
        let mut units_until_clock = DEADLINE_SAMPLE_UNITS;
        let result = self
            .bx
            .heap
            .deep_eq_bounded(a, b, maximum_work, || {
                if let Some(remaining) = thread.instruction_limit_remaining.as_mut() {
                    if *remaining == 0 {
                        return false;
                    }
                    *remaining -= 1;
                }
                let Some(deadline) = deadline else {
                    return true;
                };
                units_until_clock -= 1;
                if units_until_clock > 0 {
                    return true;
                }
                units_until_clock = DEADLINE_SAMPLE_UNITS;
                crate::clock::monotonic_now() < deadline
            });
        match result {
            Some(equal) => {
                self.bx
                    .threads
                    .cur()
                    .push_stack_unchecked((equal != negate).into());
                self.bx.threads.cur().trap.goto_next();
            }
            None => {
                let error = script_err_limit!(
                    self.bx.threads.cur_ref().trap,
                    "script equality work, instruction, or time limit exceeded"
                );
                // Native work exhaustion is uncatchable just like VM fuel.
                self.drain_errors();
                self.bx
                    .threads
                    .cur()
                    .trap
                    .set_on(Some(ScriptTrapOn::Bail(error)));
            }
        }
    }

    pub(crate) fn handle_shallow_eq(&mut self) {
        let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        self.bx.threads.cur().push_stack_value((a == b).into());
        self.bx.threads.cur().trap.goto_next();
    }

    pub(crate) fn handle_shallow_neq(&mut self) {
        let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        self.bx.threads.cur().push_stack_unchecked((a != b).into());
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_f64_op<F>(&mut self, args: OpcodeArgs, f: F)
    where
        F: FnOnce(f64, f64) -> f64,
    {
        let ip = self.bx.threads.cur_ref().trap.ip;
        let fb = if args.is_u32() {
            args.to_u32() as f64
        } else {
            let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
            self.bx.heap.cast_to_f64(b, ip)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let fa = self.bx.heap.cast_to_f64(a, ip);
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(fa, fb), ip));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_fu64_op<F>(&mut self, args: OpcodeArgs, f: F)
    where
        F: FnOnce(u64, u64) -> u64,
    {
        let ip = self.bx.threads.cur_ref().trap.ip;
        let ub = if args.is_u32() {
            args.to_u32() as u64
        } else {
            let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
            self.bx.heap.cast_to_f64(b, ip) as u64
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let ua = self.bx.heap.cast_to_f64(a, ip) as u64;
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(ua, ub) as f64, ip));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_f64_cmp_op<F>(&mut self, args: OpcodeArgs, f: F)
    where
        F: FnOnce(f64, f64) -> bool,
    {
        let ip = self.bx.threads.cur_ref().trap.ip;
        let fb = if args.is_u32() {
            args.to_u32() as f64
        } else {
            let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
            self.bx.heap.cast_to_f64(b, ip)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let fa = self.bx.heap.cast_to_f64(a, ip);
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(ScriptValue::from_bool(f(fa, fb)));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_mul(&mut self, args: OpcodeArgs) {
        let b = if args.is_u32() {
            ScriptValue::from_f64(args.to_u32() as f64)
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        if let (Some(fa), Some(fb)) = (a.as_number(), b.as_number()) {
            let ip = self.bx.threads.cur_ref().trap.ip;
            self.bx
                .threads
                .cur()
                .push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa * fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.multiply(nb);
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_div(&mut self, args: OpcodeArgs) {
        let b = if args.is_u32() {
            ScriptValue::from_f64(args.to_u32() as f64)
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        if let (Some(fa), Some(fb)) = (a.as_number(), b.as_number()) {
            let ip = self.bx.threads.cur_ref().trap.ip;
            self.bx
                .threads
                .cur()
                .push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa / fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.zip_f32(nb, |x, y| if y != 0.0 { x / y } else { 0.0 });
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_sub(&mut self, args: OpcodeArgs) {
        let b = if args.is_u32() {
            ScriptValue::from_f64(args.to_u32() as f64)
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        if let (Some(fa), Some(fb)) = (a.as_number(), b.as_number()) {
            let ip = self.bx.threads.cur_ref().trap.ip;
            self.bx
                .threads
                .cur()
                .push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa - fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.zip_f32(nb, |x, y| x - y);
        self.bx
            .threads
            .cur()
            .push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }
}
