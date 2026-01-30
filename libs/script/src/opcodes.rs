//! Opcode execution for the script virtual machine
//!
//! This module contains the main opcode dispatch function and re-exports
//! the handler implementations from the split modules:
//! - `opcodes_ops` - Arithmetic and comparison operations
//! - `opcodes_assign` - Assignment operations
//! - `opcodes_calls` - Function and method calls
//! - `opcodes_control` - Control flow (if, for, return, try/ok)
//! - `opcodes_vars` - Variables, fields, and object operations
//! - `opcodes_loops` - For loop helper functions

use crate::heap::*;
use crate::opcode::*;
use crate::vm::ScriptCode;
use crate::thread::*;
use std::any::Any;

impl ScriptThread {
    
    pub fn opcode(&mut self, opcode: Opcode, opargs: OpcodeArgs, heap: &mut ScriptHeap, code: &ScriptCode, host: &mut dyn Any) {
        
        match opcode {
// ARITHMETIC            
            Opcode::NOT => self.handle_not(heap),
            Opcode::NEG => self.handle_neg(heap, code),
            Opcode::MUL => self.handle_mul(heap, code, opargs),
            Opcode::DIV => self.handle_div(heap, code, opargs),
            Opcode::MOD => self.handle_f64_op(heap, opargs, |a,b| a%b),
            Opcode::ADD => self.handle_add(heap, code, opargs),
            Opcode::SUB => self.handle_sub(heap, code, opargs),
            Opcode::SHL => self.handle_fu64_op(heap, opargs, |a,b| a>>b),
            Opcode::SHR => self.handle_fu64_op(heap, opargs, |a,b| a<<b),
            Opcode::AND => self.handle_fu64_op(heap, opargs, |a,b| a&b),
            Opcode::OR => self.handle_fu64_op(heap, opargs, |a,b| a|b),
            Opcode::XOR => self.handle_fu64_op(heap, opargs, |a,b| a^b),
            
// ASSIGN
            Opcode::ASSIGN => self.handle_assign(heap),
            Opcode::ASSIGN_ADD => self.handle_assign_add(heap),
            Opcode::ASSIGN_SUB => self.handle_f64_scope_assign_op(heap, |a,b| a-b),
            Opcode::ASSIGN_MUL => self.handle_f64_scope_assign_op(heap, |a,b| a*b),
            Opcode::ASSIGN_DIV => self.handle_f64_scope_assign_op(heap, |a,b| a/b),
            Opcode::ASSIGN_MOD => self.handle_f64_scope_assign_op(heap, |a,b| a%b),
            Opcode::ASSIGN_AND => self.handle_fu64_scope_assign_op(heap, |a,b| a&b),
            Opcode::ASSIGN_OR => self.handle_fu64_scope_assign_op(heap, |a,b| a|b),
            Opcode::ASSIGN_XOR => self.handle_fu64_scope_assign_op(heap, |a,b| a^b),
            Opcode::ASSIGN_SHL => self.handle_fu64_scope_assign_op(heap, |a,b| a<<b),
            Opcode::ASSIGN_SHR => self.handle_fu64_scope_assign_op(heap, |a,b| a>>b),
            Opcode::ASSIGN_IFNIL => self.handle_assign_ifnil(heap),

// ASSIGN FIELD                       
            Opcode::ASSIGN_FIELD => self.handle_assign_field(heap),
            Opcode::ASSIGN_FIELD_ADD => self.handle_assign_field_add(heap),
            Opcode::ASSIGN_FIELD_SUB => self.handle_f64_field_assign_op(heap, |a,b| a-b),
            Opcode::ASSIGN_FIELD_MUL => self.handle_f64_field_assign_op(heap, |a,b| a*b),
            Opcode::ASSIGN_FIELD_DIV => self.handle_f64_field_assign_op(heap, |a,b| a/b),
            Opcode::ASSIGN_FIELD_MOD => self.handle_f64_field_assign_op(heap, |a,b| a%b),
            Opcode::ASSIGN_FIELD_AND => self.handle_fu64_field_assign_op(heap, |a,b| a&b),
            Opcode::ASSIGN_FIELD_OR => self.handle_fu64_field_assign_op(heap, |a,b| a|b),
            Opcode::ASSIGN_FIELD_XOR => self.handle_fu64_field_assign_op(heap, |a,b| a^b),
            Opcode::ASSIGN_FIELD_SHL => self.handle_fu64_field_assign_op(heap, |a,b| a<<b),
            Opcode::ASSIGN_FIELD_SHR => self.handle_fu64_field_assign_op(heap, |a,b| a>>b),
            Opcode::ASSIGN_FIELD_IFNIL => self.handle_assign_field_ifnil(heap),
                        
            Opcode::ASSIGN_INDEX => self.handle_assign_index(heap),
            Opcode::ASSIGN_INDEX_ADD => self.handle_assign_index_add(heap),
            Opcode::ASSIGN_INDEX_SUB => self.handle_f64_index_assign_op(heap, |a,b| a-b),
            Opcode::ASSIGN_INDEX_MUL => self.handle_f64_index_assign_op(heap, |a,b| a*b),
            Opcode::ASSIGN_INDEX_DIV => self.handle_f64_index_assign_op(heap, |a,b| a/b),
            Opcode::ASSIGN_INDEX_MOD => self.handle_f64_index_assign_op(heap, |a,b| a%b),
            Opcode::ASSIGN_INDEX_AND => self.handle_fu64_index_assign_op(heap, |a,b| a&b),
            Opcode::ASSIGN_INDEX_OR => self.handle_fu64_index_assign_op(heap, |a,b| a|b),
            Opcode::ASSIGN_INDEX_XOR => self.handle_fu64_index_assign_op(heap, |a,b| a^b),
            Opcode::ASSIGN_INDEX_SHL => self.handle_fu64_index_assign_op(heap, |a,b| a<<b),
            Opcode::ASSIGN_INDEX_SHR => self.handle_fu64_index_assign_op(heap, |a,b| a>>b),
            Opcode::ASSIGN_INDEX_IFNIL => self.handle_assign_index_ifnil(heap),

// ASSIGN ME            
            Opcode::ASSIGN_ME => self.handle_assign_me(heap),
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER => self.handle_assign_me_before_after(heap, opcode),
            Opcode::ASSIGN_ME_BEGIN => self.handle_assign_me_begin(heap),
            
// CONCAT  
            Opcode::CONCAT => self.handle_concat(heap),

// EQUALITY
            Opcode::EQ => self.handle_eq(heap),
            Opcode::NEQ => self.handle_neq(heap),
            Opcode::LT => self.handle_f64_cmp_op(heap, opargs, |a,b| a<b),
            Opcode::GT => self.handle_f64_cmp_op(heap, opargs, |a,b| a>b),
            Opcode::LEQ => self.handle_f64_cmp_op(heap, opargs, |a,b| a<=b),
            Opcode::GEQ => self.handle_f64_cmp_op(heap, opargs, |a,b| a>=b),
            
            Opcode::LOGIC_AND => self.handle_bool_op(heap, |a,b| a&&b),
            Opcode::LOGIC_OR => self.handle_bool_op(heap, |a,b| a||b),
            Opcode::NIL_OR => self.handle_nil_or(heap),
            Opcode::SHALLOW_EQ => self.handle_shallow_eq(heap),
            Opcode::SHALLOW_NEQ => self.handle_shallow_neq(heap),

// Object/Array begin
            Opcode::BEGIN_PROTO => self.handle_begin_proto(heap),
            Opcode::PROTO_INHERIT_READ => self.handle_proto_inherit_read(heap),
            Opcode::PROTO_INHERIT_WRITE => self.handle_proto_inherit_write(heap),
            Opcode::SCOPE_INHERIT_READ => self.handle_scope_inherit_read(heap),
            Opcode::SCOPE_INHERIT_WRITE => self.handle_scope_inherit_write(heap),
            Opcode::FIELD_INHERIT_READ => self.handle_field_inherit_read(heap),
            Opcode::FIELD_INHERIT_WRITE => self.handle_field_inherit_write(heap),
            Opcode::INDEX_INHERIT_READ => self.handle_index_inherit_read(heap),
            Opcode::INDEX_INHERIT_WRITE => self.handle_index_inherit_write(heap),
            Opcode::END_PROTO => self.handle_end_proto(heap, code),
            Opcode::BEGIN_BARE => self.handle_begin_bare(heap),
            Opcode::END_BARE => self.handle_end_bare(),
            Opcode::BEGIN_ARRAY => self.handle_begin_array(heap),
            Opcode::END_ARRAY => self.handle_end_array(),

// Calling
            Opcode::CALL_ARGS => self.handle_call_args(heap),
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC => {
                let should_pop_to_me = self.handle_call_exec(heap, code, host, opargs);
                if should_pop_to_me && opargs.is_pop_to_me(){
                    self.pop_to_me(heap, code);
                }
                return
            }
            Opcode::METHOD_CALL_ARGS => {
                if self.handle_method_call_args(heap, code) {
                    // Pod case: return early, skip pop_to_me (original returned before end)
                    return
                }
                // Normal case: falls through to end-of-function pop_to_me check
            }

// Fn def
            Opcode::FN_ARGS => self.handle_fn_args(heap),
            Opcode::FN_LET_ARGS => self.handle_fn_let_args(heap),
            Opcode::FN_ARG_DYN => self.handle_fn_arg_dyn(heap, opargs),
            Opcode::FN_ARG_TYPED => self.handle_fn_arg_typed(heap, opargs),
            Opcode::FN_BODY_DYN => self.handle_fn_body_dyn(heap, opargs),
            Opcode::FN_BODY_TYPED => self.handle_fn_body_typed(heap, opargs),
            Opcode::RETURN => {
                self.handle_return(heap, code, opargs);
                if opargs.is_pop_to_me(){
                    self.pop_to_me(heap, code);
                }
                return
            }
            Opcode::RETURN_IF_ERR => {
                if self.handle_return_if_err(heap, code, opargs) {
                    // Error case: original fell through to end-of-function check
                    if opargs.is_pop_to_me(){
                        self.pop_to_me(heap, code);
                    }
                    return
                }
                // Non-error case: falls through to end-of-function pop_to_me check
            }

// IF            
            Opcode::IF_TEST => self.handle_if_test(heap, opargs),
            Opcode::IF_ELSE => self.handle_if_else(opargs),

// Use            
            Opcode::USE => {
                self.handle_use(heap);
                // Original returned early, skipping pop_to_me
                return
            }

// Field            
            Opcode::FIELD => self.handle_field(heap, code, host),
            Opcode::FIELD_NIL => self.handle_field_nil(heap),
            Opcode::ME_FIELD => self.handle_me_field(heap, code),
            Opcode::PROTO_FIELD => self.handle_proto_field(heap),
            Opcode::POP_TO_ME => self.handle_pop_to_me(heap, code),
            Opcode::ME_SPLAT => self.handle_me_splat(heap),

// Array index            
            Opcode::ARRAY_INDEX => self.handle_array_index(heap, code),

// Let                   
            Opcode::LET_DYN => self.handle_let_dyn(heap, opargs),
            Opcode::LET_TYPED => self.handle_let_typed(heap, opargs),
            Opcode::VAR_DYN => self.handle_var_dyn(heap, opargs),
            Opcode::VAR_TYPED => self.handle_var_typed(heap, opargs),

// Tree search            
            Opcode::SEARCH_TREE => self.handle_search_tree(),

// Log            
            Opcode::LOG => self.handle_log(heap, code),

// Me/Scope
            Opcode::ME => self.handle_me(),
            Opcode::SCOPE => self.handle_scope(),

// For            
            Opcode::FOR_1 => self.handle_for_1(heap, code, opargs),
            Opcode::FOR_2 => self.handle_for_2(heap, code, opargs),
            Opcode::FOR_3 => self.handle_for_3(heap, code, opargs),
            Opcode::LOOP => self.handle_loop(heap, opargs),
            Opcode::FOR_END => self.handle_for_end(heap, code),
            Opcode::BREAK => self.handle_break(heap),
            Opcode::BREAKIFNOT => self.handle_breakifnot(heap),
            Opcode::CONTINUE => self.handle_continue(heap, code),

// Range            
            Opcode::RANGE => self.handle_range(heap, code),

// Is            
            Opcode::IS => self.handle_is(heap),

// Try / OK            
            Opcode::OK_TEST => self.handle_ok_test(opargs),
            Opcode::OK_END => self.handle_ok_end(),
            Opcode::TRY_TEST => self.handle_try_test(opargs),
            Opcode::TRY_ERR => self.handle_try_err(opargs),
            Opcode::TRY_OK => self.handle_try_ok(opargs),

            opcode => {
                println!("UNDEFINED OPCODE {}", opcode);
                self.trap.goto_next();
            }
        }
        if opargs.is_pop_to_me(){
            self.pop_to_me(heap, code);
        }
    }
}
