//! Opcode arithmetic and comparison operations
//!
//! This module contains handle functions for arithmetic operations (+, -, *, /, etc.),
//! comparison operations (==, !=, <, >, etc.), and logical operations (&&, ||).

use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::thread::*;

impl ScriptThread {
    // ARITHMETIC handlers
    
    pub(crate) fn handle_not(&mut self, heap: &mut ScriptHeap) {
        let value = self.pop_stack_resolved(heap);
        if let Some(v) = value.as_f64(){
            self.push_stack_unchecked(ScriptValue::from_f64(!(v as u64) as f64));
            self.trap.goto_next();
        }
        else{
            let v = heap.cast_to_bool(value);
            self.push_stack_unchecked(ScriptValue::from_bool(!v));
            // Note: original code did NOT have goto_next() here
        }
    }
    
    pub(crate) fn handle_neg(&mut self, heap: &mut ScriptHeap) {
        let v = heap.cast_to_f64(self.pop_stack_resolved(heap), self.trap.ip);
        self.push_stack_unchecked(ScriptValue::from_f64(-v));
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_add(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
        let b = if opargs.is_u32(){
            (opargs.to_u32()).into()
        }
        else{
            self.pop_stack_resolved(heap)
        };
        let a = self.pop_stack_resolved(heap);
        if a.is_string_like() || b.is_string_like(){
            let ptr = heap.new_string_with(|heap, out|{
                heap.cast_to_string(a, out);
                heap.cast_to_string(b, out);
            });
            self.push_stack_unchecked(ptr.into());
        }
        else{
            let fa = heap.cast_to_f64(a, self.trap.ip);
            let fb = heap.cast_to_f64(b, self.trap.ip);
            self.push_stack_unchecked(ScriptValue::from_f64_traced_nan(fa + fb, self.trap.ip));
        }
        self.trap.goto_next();
    }

    // CONCAT handler
    
    pub(crate) fn handle_concat(&mut self, heap: &mut ScriptHeap) {
        let op1 = self.pop_stack_resolved(heap);
        let op2 = self.pop_stack_resolved(heap);
        let ptr = heap.new_string_with(|heap, out|{
            heap.cast_to_string(op1, out);
            heap.cast_to_string(op2, out);
        });
        self.push_stack_unchecked(ptr.into());
        self.trap.goto_next();
    }

    // EQUALITY handlers
    
    pub(crate) fn handle_eq(&mut self, heap: &mut ScriptHeap) {
        let b = self.pop_stack_resolved(heap);
        let a = self.pop_stack_resolved(heap);
        self.push_stack_unchecked(heap.deep_eq(a, b).into());
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_neq(&mut self, heap: &mut ScriptHeap) {
        let b = self.pop_stack_resolved(heap);
        let a = self.pop_stack_resolved(heap);
        self.push_stack_unchecked((!heap.deep_eq(a, b)).into());
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_nil_or(&mut self, heap: &mut ScriptHeap) {
        let op1 = self.pop_stack_resolved(heap);
        let op2 = self.pop_stack_resolved(heap);
        if op1.is_nil(){
            self.push_stack_unchecked(op2);
        }
        else{
            self.push_stack_unchecked(op1);
        }
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_shallow_eq(&mut self, heap: &mut ScriptHeap) {
        let b = self.pop_stack_resolved(heap);
        let a = self.pop_stack_resolved(heap);
        self.push_stack_value((a == b).into());
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_shallow_neq(&mut self, heap: &mut ScriptHeap) {
        let b = self.pop_stack_resolved(heap);
        let a = self.pop_stack_resolved(heap);
        self.push_stack_unchecked((a != b).into());
        self.trap.goto_next();
    }

    // Generic arithmetic operation handlers

    pub fn handle_f64_op<F>(&mut self, heap: &mut ScriptHeap, args: OpcodeArgs, f: F)
    where F: FnOnce(f64, f64) -> f64
    {
        let fb = if args.is_u32(){
            args.to_u32() as f64
        }
        else{
            let b = self.pop_stack_resolved(heap);
            heap.cast_to_f64(b, self.trap.ip)
        };
        let a = self.pop_stack_resolved(heap);
        let fa = heap.cast_to_f64(a, self.trap.ip);
        self.push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(fa, fb), self.trap.ip));
        self.trap.goto_next();
    }

    pub fn handle_fu64_op<F>(&mut self, heap: &mut ScriptHeap, args: OpcodeArgs, f: F)
    where F: FnOnce(u64, u64) -> u64
    {
        let ub = if args.is_u32(){
            args.to_u32() as u64
        }
        else{
            let b = self.pop_stack_resolved(heap);
            heap.cast_to_f64(b, self.trap.ip) as u64
        };
        let a = self.pop_stack_resolved(heap);
        let ua = heap.cast_to_f64(a, self.trap.ip) as u64;
        self.push_stack_unchecked(ScriptValue::from_f64_traced_nan(f(ua, ub) as f64, self.trap.ip));
        self.trap.goto_next();
    }

    pub fn handle_f64_cmp_op<F>(&mut self, heap: &mut ScriptHeap, args: OpcodeArgs, f: F)
    where F: FnOnce(f64, f64) -> bool
    {
        let fb = if args.is_u32(){
            args.to_u32() as f64
        }
        else{
            let b = self.pop_stack_resolved(heap);
            heap.cast_to_f64(b, self.trap.ip)
        };
        let a = self.pop_stack_resolved(heap);
        let fa = heap.cast_to_f64(a, self.trap.ip);
        self.push_stack_unchecked(ScriptValue::from_bool(f(fa, fb)));
        self.trap.goto_next();
    }

    pub fn handle_bool_op<F>(&mut self, heap: &mut ScriptHeap, f: F)
    where F: FnOnce(bool, bool) -> bool
    {
        let b = self.pop_stack_resolved(heap);
        let a = self.pop_stack_resolved(heap);
        let ba = heap.cast_to_bool(a);
        let bb = heap.cast_to_bool(b);
        self.push_stack_unchecked(ScriptValue::from_bool(f(ba, bb)));
        self.trap.goto_next();
    }
}
