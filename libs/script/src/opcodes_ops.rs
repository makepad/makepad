//! Opcode arithmetic and comparison operations
//!
//! This module contains handle functions for arithmetic operations (+, -, *, /, etc.),
//! comparison operations (==, !=, <, >, etc.), and logical operations (&&, ||).

use crate::value::*;
use crate::opcode::*;
use crate::numeric::NumericValue;
use crate::vm::ScriptVm;

impl<'a> ScriptVm<'a> {
    // ARITHMETIC handlers
    
    pub(crate) fn handle_not(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(v) = value.as_f64(){
            self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64(!(v as u64) as f64));
            self.bx.threads.cur().trap.goto_next();
        }
        else{
            let v = self.bx.heap.cast_to_bool(value);
            self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_bool(!v));
        }
    }
    
    pub(crate) fn handle_neg(&mut self) {
        let value = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if let Some(f) = value.as_number() {
            self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64(-f));
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        let ip = self.bx.threads.cur_ref().trap.ip;
        let num = NumericValue::from_script_value_heap(&self.bx.heap, value, ip);
        let result = num.zip_f32(NumericValue::F64(-1.0), |a, b| a * b);
        self.bx.threads.cur().push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }
    
    pub(crate) fn handle_add(&mut self, opargs: OpcodeArgs) {
        let b = if opargs.is_u32(){
            (opargs.to_u32()).into()
        }
        else{
            self.bx.threads.cur().pop_stack_resolved(&self.bx.heap)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        
        if a.is_string_like() || b.is_string_like(){
            let ptr = self.bx.heap.new_string_with(|heap, out|{
                heap.cast_to_string(a, out);
                heap.cast_to_string(b, out);
            });
            self.bx.threads.cur().push_stack_unchecked(ptr.into());
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        
        if let (Some(fa), Some(fb)) = (a.as_number(), b.as_number()) {
            let ip = self.bx.threads.cur_ref().trap.ip;
            self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa + fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        
        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.zip_f32(nb, |x, y| x + y);
        self.bx.threads.cur().push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }

    // CONCAT handler
    
    pub(crate) fn handle_concat(&mut self) {
        let op1 = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let op2 = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let ptr = self.bx.heap.new_string_with(|heap, out|{
            heap.cast_to_string(op1, out);
            heap.cast_to_string(op2, out);
        });
        self.bx.threads.cur().push_stack_unchecked(ptr.into());
        self.bx.threads.cur().trap.goto_next();
    }

    // EQUALITY handlers
    
    pub(crate) fn handle_eq(&mut self) {
        let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        self.bx.threads.cur().push_stack_unchecked(self.bx.heap.deep_eq(a, b).into());
        self.bx.threads.cur().trap.goto_next();
    }
    
    pub(crate) fn handle_neq(&mut self) {
        let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        self.bx.threads.cur().push_stack_unchecked((!self.bx.heap.deep_eq(a, b)).into());
        self.bx.threads.cur().trap.goto_next();
    }
    
    pub(crate) fn handle_nil_or(&mut self) {
        let op1 = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let op2 = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        if op1.is_nil(){
            self.bx.threads.cur().push_stack_unchecked(op2);
        }
        else{
            self.bx.threads.cur().push_stack_unchecked(op1);
        }
        self.bx.threads.cur().trap.goto_next();
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
    where F: FnOnce(f64, f64) -> f64
    {
        let ip = self.bx.threads.cur_ref().trap.ip;
        let fb = if args.is_u32(){
            args.to_u32() as f64
        }
        else{
            let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
            self.bx.heap.cast_to_f64(b, ip)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let fa = self.bx.heap.cast_to_f64(a, ip);
        self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(fa, fb), ip));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_fu64_op<F>(&mut self, args: OpcodeArgs, f: F)
    where F: FnOnce(u64, u64) -> u64
    {
        let ip = self.bx.threads.cur_ref().trap.ip;
        let ub = if args.is_u32(){
            args.to_u32() as u64
        }
        else{
            let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
            self.bx.heap.cast_to_f64(b, ip) as u64
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let ua = self.bx.heap.cast_to_f64(a, ip) as u64;
        self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(ua, ub) as f64, ip));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_f64_cmp_op<F>(&mut self, args: OpcodeArgs, f: F)
    where F: FnOnce(f64, f64) -> bool
    {
        let ip = self.bx.threads.cur_ref().trap.ip;
        let fb = if args.is_u32(){
            args.to_u32() as f64
        }
        else{
            let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
            self.bx.heap.cast_to_f64(b, ip)
        };
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let fa = self.bx.heap.cast_to_f64(a, ip);
        self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_bool(f(fa, fb)));
        self.bx.threads.cur().trap.goto_next();
    }

    pub fn handle_bool_op<F>(&mut self, f: F)
    where F: FnOnce(bool, bool) -> bool
    {
        let b = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let a = self.bx.threads.cur().pop_stack_resolved(&self.bx.heap);
        let ba = self.bx.heap.cast_to_bool(a);
        let bb = self.bx.heap.cast_to_bool(b);
        self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_bool(f(ba, bb)));
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
            self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa * fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        
        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.multiply(nb);
        self.bx.threads.cur().push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
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
            self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa / fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        
        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.zip_f32(nb, |x, y| if y != 0.0 { x / y } else { 0.0 });
        self.bx.threads.cur().push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
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
            self.bx.threads.cur().push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa - fb, ip));
            self.bx.threads.cur().trap.goto_next();
            return;
        }
        
        let ip = self.bx.threads.cur_ref().trap.ip;
        let na = NumericValue::from_script_value_heap(&self.bx.heap, a, ip);
        let nb = NumericValue::from_script_value_heap(&self.bx.heap, b, ip);
        let result = na.zip_f32(nb, |x, y| x - y);
        self.bx.threads.cur().push_stack_unchecked(result.to_script_value_heap(&mut self.bx.heap, &self.bx.code));
        self.bx.threads.cur().trap.goto_next();
    }
}
