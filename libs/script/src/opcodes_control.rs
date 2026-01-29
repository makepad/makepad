//! Opcode control flow operations
//!
//! This module contains handle functions for control flow: if/else statements,
//! for loops, return statements, range, is, and try/ok operations.

use crate::makepad_live_id::*;
use crate::heap::*;
use crate::value::*;
use crate::opcode::*;
use crate::vm::ScriptCode;
use crate::thread::*;
use crate::trap::*;
use crate::*;

impl ScriptThread {
    // IF handlers
    
    pub(crate) fn handle_if_test(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
        let test = self.pop_stack_resolved(heap);
        let test = heap.cast_to_bool(test);
        if test {
            self.trap.goto_next()
        }
        else{
            if opargs.is_need_nil(){
                self.push_stack_unchecked(NIL);
            }
            self.trap.goto_rel(opargs.to_u32());
        }
    }
    
    pub(crate) fn handle_if_else(&mut self, opargs: OpcodeArgs) {
        self.trap.goto_rel(opargs.to_u32());
    }

    // RETURN handlers
    
    pub(crate) fn handle_return(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, opargs: OpcodeArgs) {
        let value = if opargs.is_nil(){
            NIL
        }
        else{
            self.pop_stack_resolved(heap)
        };
        let call = self.calls.pop().unwrap();
        self.truncate_bases(call.bases, heap);
        
        if let Some(ret) = call.return_ip{
            self.trap.ip = ret;
            self.push_stack_unchecked(value);
            if call.args.is_pop_to_me(){
                self.pop_to_me(heap, code);
            }
        }
        else{
            self.trap.on.set(Some(ScriptTrapOn::Return(value)));
        }
    }
    
    pub(crate) fn handle_return_if_err(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, _opargs: OpcodeArgs) -> bool {
        let value = self.peek_stack_resolved(heap);
        if value.is_err(){
            let call = self.calls.pop().unwrap();
            self.truncate_bases(call.bases, heap);
            if let Some(ret) = call.return_ip{
                self.trap.ip = ret;
                self.push_stack_unchecked(value);
                if call.args.is_pop_to_me(){
                    self.pop_to_me(heap, code);
                }
            }
            else{
                self.trap.on.set(Some(ScriptTrapOn::Return(value)));
            }
            true
        }
        else{
            self.trap.goto_next();
            false
        }
    }

    // For loop handlers
    
    pub(crate) fn handle_for_1(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, opargs: OpcodeArgs) {
        let source = self.pop_stack_resolved(heap);
        let value_id = self.pop_stack_value().as_id().unwrap();
        self.begin_for_loop(heap, code, opargs.to_u32() as _, source, value_id, None, None);
    }
    
    pub(crate) fn handle_for_2(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, opargs: OpcodeArgs) {
        let source = self.pop_stack_resolved(heap);
        let value_id = self.pop_stack_value().as_id().unwrap();
        let index_id = self.pop_stack_value().as_id().unwrap();
        self.begin_for_loop(heap, code, opargs.to_u32() as _, source, value_id, Some(index_id), None);
    }
    
    pub(crate) fn handle_for_3(&mut self, heap: &mut ScriptHeap, code: &ScriptCode, opargs: OpcodeArgs) {
        let source = self.pop_stack_resolved(heap);
        let value_id = self.pop_stack_value().as_id().unwrap();
        let index_id = self.pop_stack_value().as_id().unwrap();
        let key_id = self.pop_stack_value().as_id().unwrap();
        self.begin_for_loop(heap, code, opargs.to_u32() as _, source, value_id, Some(index_id), Some(key_id));
    }
    
    pub(crate) fn handle_loop(&mut self, heap: &mut ScriptHeap, opargs: OpcodeArgs) {
        self.begin_loop(heap, opargs.to_u32() as _);
    }
    
    pub(crate) fn handle_for_end(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        self.end_for_loop(heap, code);
    }
    
    pub(crate) fn handle_break(&mut self, heap: &mut ScriptHeap) {
        self.break_for_loop(heap);
    }
    
    pub(crate) fn handle_breakifnot(&mut self, heap: &mut ScriptHeap) {
        let value = self.pop_stack_resolved(heap);
        if !heap.cast_to_bool(value){
            self.break_for_loop(heap);
        }
        else{
            self.trap.goto_next();
        }
    }
    
    pub(crate) fn handle_continue(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        self.end_for_loop(heap, code);
    }

    // Range handler
    
    pub(crate) fn handle_range(&mut self, heap: &mut ScriptHeap, code: &ScriptCode) {
        let end = self.pop_stack_resolved(heap);
        let start = self.pop_stack_resolved(heap);
        // Validate that both operands are numbers
        if !start.is_number() {
            self.push_stack_unchecked(script_err_type_mismatch!(self.trap, "range start must be a number"));
            return;
        }
        if !end.is_number() {
            self.push_stack_unchecked(script_err_type_mismatch!(self.trap, "range end must be a number"));
            return;
        }
        let range = heap.new_with_proto(code.builtins.range.into());
        heap.set_value_def(range, id!(start).into(), start);
        heap.set_value_def(range, id!(end).into(), end);
        self.push_stack_unchecked(range.into());
        self.trap.goto_next();
    }

    // Is handler
    
    pub(crate) fn handle_is(&mut self, heap: &mut ScriptHeap) {
        let rhs = self.pop_stack_value();
        let lhs = self.pop_stack_resolved(heap);
        let cmp = if let Some(id) = rhs.as_id(){
            match lhs.value_type().to_redux(){
                ScriptValueType::REDUX_NUMBER => id == id!(number).into(),
                ScriptValueType::REDUX_NAN => id == id!(number).into() || id == id!(nan).into(),
                ScriptValueType::REDUX_BOOL => id == id!(bool).into(),
                ScriptValueType::REDUX_NIL => id == id!(nan).into(),
                ScriptValueType::REDUX_COLOR => id == id!(color).into(),
                ScriptValueType::REDUX_STRING => id == id!(string).into(),
                ScriptValueType::REDUX_OBJECT => {
                    id == id!(object).into() || {
                        if let Some(rhs) = self.scope_value(heap, id).as_object(){
                            if let Some(obj) = lhs.as_object(){
                                heap.has_proto(obj, rhs.into())
                            }
                            else{
                                false
                            }
                        }
                        else{
                            false
                        }
                    }
                },
                ScriptValueType::REDUX_ID => id == id!(id).into(),
                _ => false
            }
        }
        else if let Some(obj) = lhs.as_object(){
            heap.has_proto(obj, rhs)
        }
        else{
            false
        };
        self.push_stack_unchecked(cmp.into());
        self.trap.goto_next();
    }

    // Try / OK handlers
    
    pub(crate) fn handle_ok_test(&mut self, opargs: OpcodeArgs) {
        //self.last_err = NIL;
        self.tries.push(TryFrame{
            push_nil: true,
            start_ip: self.trap.ip(),
            jump: opargs.to_u32() + 1,
            bases: self.new_bases()
        });
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_ok_end(&mut self) {
        self.tries.pop();
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_try_test(&mut self, opargs: OpcodeArgs) {
        //self.last_err = NIL;
        self.tries.push(TryFrame{
            push_nil: false,
            start_ip: self.trap.ip(),
            jump: opargs.to_u32() + 1,
            bases: self.new_bases()
        });
        self.trap.goto_next();
    }
    
    pub(crate) fn handle_try_err(&mut self, opargs: OpcodeArgs) {
        self.tries.pop().unwrap();
        self.trap.goto_rel(opargs.to_u32() + 1);
    }
    
    pub(crate) fn handle_try_ok(&mut self, opargs: OpcodeArgs) {
        self.trap.goto_rel(opargs.to_u32());
    }
}
