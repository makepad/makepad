//! Opcode arithmetic and comparison operations
//!
//! This module contains handle functions for arithmetic operations (+, -, *, /, etc.),
//! comparison operations (==, !=, <, >, etc.), and logical operations (&&, ||).

use crate::numeric::NumericValue;
use crate::opcode::*;
use crate::pod::{ScriptPodTy, ScriptPodVec};
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

    /// A host's meaning for `a op b` on its objects (see
    /// `ScriptVmBase::object_op_hook`); `None` when no hook, no object
    /// operand, or the host declines.
    fn object_op(&mut self, op: crate::vm::ScriptBinOp, a: ScriptValue, b: ScriptValue) -> Option<ScriptValue> {
        let hook = self.bx.object_op_hook?;
        if a.as_object().is_none() && b.as_object().is_none() {
            return None;
        }
        let ip = self.bx.threads.cur_ref().trap.ip;
        let mut out = NIL;
        unsafe { (hook.call)(hook.data, &mut self.bx.heap, &op, a, b, ip, &mut out) }.then_some(out)
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
            if self.bx.heap.allocation_error_pending { return; }
            self.bx.threads.cur().push_stack_unchecked(ptr.into());
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        if let Some(v) = self.object_op(crate::vm::ScriptBinOp::Add, a, b) {
            self.bx.threads.cur().push_stack_unchecked(v);
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
        if self.bx.heap.allocation_error_pending { return; }
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
        let hard_deadline = self.bx.run_budget.as_ref()
            .map(|budget| budget.hard_deadline)
            .filter(|deadline| deadline.is_finite());
        let thread = self.bx.threads.cur();
        // A soft slice paces execution between opcodes. Only a hard bound caps comparison work.
        let real_limit = thread.instruction_limit_depth != 0;
        let maximum_work = if real_limit
            || hard_deadline.is_some()
            || self.bx.heap.has_allocation_budget()
        {
            crate::equality::MAX_EQUALITY_WORK
        } else {
            usize::MAX
        };
        let mut limit = crate::Limit::EqualityWork;
        let mut units_until_clock = DEADLINE_SAMPLE_UNITS;
        let result = self
            .bx
            .heap
            .deep_eq_bounded(a, b, maximum_work, || {
                if let Some(remaining) = thread.instruction_limit_remaining.as_mut() {
                    if *remaining == 0 && real_limit {
                        limit = crate::Limit::Fuel;
                        return false;
                    }
                    *remaining = remaining.wrapping_sub(1);
                }
                let Some(deadline) = hard_deadline else {
                    return true;
                };
                units_until_clock -= 1;
                if units_until_clock > 0 {
                    return true;
                }
                units_until_clock = DEADLINE_SAMPLE_UNITS;
                if crate::clock::monotonic_now() >= deadline {
                    limit = crate::Limit::Time;
                    return false;
                }
                true
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
                self.bx.poison_limit(limit);
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

        if let Some(v) = self.object_op(crate::vm::ScriptBinOp::Mul, a, b) {
            self.bx.threads.cur().push_stack_unchecked(v);
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

    /// Integer `/` and `%` on typed integer scalars and `ivec`/`uvec` pods,
    /// truncating toward zero like GLSL, HLSL, MSL and WGSL. `None` when the
    /// operands are not integer-typed, so the float path handles them. A zero
    /// divisor gives 0 (the GPU result is undefined; the VM must not panic),
    /// as the float path's `zip_f32` division does.
    pub(crate) fn int_div_rem(&mut self, a: ScriptValue, b: ScriptValue, rem: bool) -> Option<ScriptValue> {
        struct Op { lanes: [u32; 4], signed: bool, vec: Option<ScriptPodVec>, typed: bool }
        let read = |heap: &ScriptHeap, v: ScriptValue| -> Option<Op> {
            if let Some(i) = v.as_i32() {
                return Some(Op { lanes: [i as u32, 0, 0, 0], signed: true, vec: None, typed: true });
            }
            if let Some(u) = v.as_u32() {
                return Some(Op { lanes: [u, 0, 0, 0], signed: false, vec: None, typed: true });
            }
            if let Some(pod) = v.as_pod() {
                let data = &heap.pods[pod];
                if let ScriptPodTy::Vec(vt) = &heap.pod_types[data.ty.index as usize].ty {
                    let signed = match vt {
                        ScriptPodVec::Vec2i | ScriptPodVec::Vec3i | ScriptPodVec::Vec4i => true,
                        ScriptPodVec::Vec2u | ScriptPodVec::Vec3u | ScriptPodVec::Vec4u => false,
                        _ => return None,
                    };
                    let mut lanes = [0u32; 4];
                    lanes[..vt.dims()].copy_from_slice(&data.data[..vt.dims()]);
                    return Some(Op { lanes, signed, vec: Some(*vt), typed: true });
                }
                return None;
            }
            // An untyped whole number counts as an integer beside a typed one.
            let f = v.as_number()?;
            if f.fract() != 0.0 || f.abs() > 2147483647.0 {
                return None;
            }
            Some(Op { lanes: [f as i32 as u32, 0, 0, 0], signed: true, vec: None, typed: false })
        };
        let (oa, ob) = (read(&self.bx.heap, a)?, read(&self.bx.heap, b)?);
        if !oa.typed && !ob.typed {
            return None;
        }
        let n = match (oa.vec, ob.vec) {
            (Some(x), Some(y)) => {
                if x.dims() != y.dims() || oa.signed != ob.signed {
                    return None;
                }
                x.dims()
            }
            (Some(x), None) => x.dims(),
            (None, Some(y)) => y.dims(),
            (None, None) => 1,
        };
        // A bare number takes the signedness of the typed operand.
        let signed = if oa.typed { oa.signed } else { ob.signed };
        let lane = |o: &Op, i: usize| if o.vec.is_some() { o.lanes[i] } else { o.lanes[0] };
        let calc = |x: u32, y: u32| -> u32 {
            if y == 0 {
                0
            } else if signed {
                let (x, y) = (x as i32, y as i32);
                (if rem { x.wrapping_rem(y) } else { x.wrapping_div(y) }) as u32
            } else if rem {
                x % y
            } else {
                x / y
            }
        };
        let mut out = [0u32; 4];
        for i in 0..n {
            out[i] = calc(lane(&oa, i), lane(&ob, i));
        }
        match oa.vec.or(ob.vec) {
            None => Some(if signed { ScriptValue::from_i32(out[0] as i32) } else { ScriptValue::from_u32(out[0]) }),
            Some(vt) => {
                let pod = self.bx.heap.new_pod(vt.builtin(&self.bx.code.builtins.pod));
                self.bx.heap.pods[pod].data[..n].copy_from_slice(&out[..n]);
                Some(pod.into())
            }
        }
    }

    pub(crate) fn handle_mod(&mut self, args: OpcodeArgs) {
        let b = if args.is_u32() {
            ScriptValue::from_f64(args.to_u32() as f64)
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(v) = self.int_div_rem(a, b, true) {
            self.bx.threads.cur().push_stack_unchecked(v);
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        let ip = self.bx.threads.cur_ref().trap.ip;
        let (fa, fb) = (self.bx.heap.cast_to_f64(a, ip), self.bx.heap.cast_to_f64(b, ip));
        self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa % fb, ip));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_div(&mut self, args: OpcodeArgs) {
        let b = if args.is_u32() {
            ScriptValue::from_f64(args.to_u32() as f64)
        } else {
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);

        if let Some(v) = self.int_div_rem(a, b, false) {
            self.bx.threads.cur().push_stack_unchecked(v);
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        if let (Some(fa), Some(fb)) = (a.as_number(), b.as_number()) {
            let ip = self.bx.threads.cur_ref().trap.ip;
            self.bx
                .threads
                .cur()
                .push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa / fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }

        if let Some(v) = self.object_op(crate::vm::ScriptBinOp::Div, a, b) {
            self.bx.threads.cur().push_stack_unchecked(v);
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

        if let Some(v) = self.object_op(crate::vm::ScriptBinOp::Sub, a, b) {
            self.bx.threads.cur().push_stack_unchecked(v);
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
