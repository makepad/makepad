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

use crate::opcode::*;
use crate::vm::ScriptVm;

impl<'a> ScriptVm<'a> {
    
    #[inline]
    pub fn opcode(&mut self, opcode: Opcode, opargs: OpcodeArgs) {
        
        match opcode {
// ARITHMETIC            
            Opcode::NOT => self.handle_not(),
            Opcode::NEG => self.handle_neg(),
            Opcode::MUL => self.handle_mul(opargs),
            Opcode::DIV => self.handle_div(opargs),
            Opcode::MOD => self.handle_f64_op(opargs, |a,b| a%b),
            Opcode::ADD => self.handle_add(opargs),
            Opcode::SUB => self.handle_sub(opargs),
            Opcode::SHL => self.handle_fu64_op(opargs, |a,b| a<<b),
            Opcode::SHR => self.handle_fu64_op(opargs, |a,b| a>>b),
            Opcode::AND => self.handle_fu64_op(opargs, |a,b| a&b),
            Opcode::OR => self.handle_fu64_op(opargs, |a,b| a|b),
            Opcode::XOR => self.handle_fu64_op(opargs, |a,b| a^b),
            
// ASSIGN
            Opcode::ASSIGN => self.handle_assign(),
            Opcode::ASSIGN_ADD => self.handle_assign_add(),
            Opcode::ASSIGN_SUB => self.handle_f64_scope_assign_op(|a,b| a-b),
            Opcode::ASSIGN_MUL => self.handle_f64_scope_assign_op(|a,b| a*b),
            Opcode::ASSIGN_DIV => self.handle_f64_scope_assign_op(|a,b| a/b),
            Opcode::ASSIGN_MOD => self.handle_f64_scope_assign_op(|a,b| a%b),
            Opcode::ASSIGN_AND => self.handle_fu64_scope_assign_op(|a,b| a&b),
            Opcode::ASSIGN_OR => self.handle_fu64_scope_assign_op(|a,b| a|b),
            Opcode::ASSIGN_XOR => self.handle_fu64_scope_assign_op(|a,b| a^b),
            Opcode::ASSIGN_SHL => self.handle_fu64_scope_assign_op(|a,b| a<<b),
            Opcode::ASSIGN_SHR => self.handle_fu64_scope_assign_op(|a,b| a>>b),
            Opcode::ASSIGN_IFNIL => self.handle_assign_ifnil(opargs),

// ASSIGN FIELD                       
            Opcode::ASSIGN_FIELD => self.handle_assign_field(),
            Opcode::ASSIGN_FIELD_ADD => self.handle_assign_field_add(),
            Opcode::ASSIGN_FIELD_SUB => self.handle_f64_field_assign_op(|a,b| a-b),
            Opcode::ASSIGN_FIELD_MUL => self.handle_f64_field_assign_op(|a,b| a*b),
            Opcode::ASSIGN_FIELD_DIV => self.handle_f64_field_assign_op(|a,b| a/b),
            Opcode::ASSIGN_FIELD_MOD => self.handle_f64_field_assign_op(|a,b| a%b),
            Opcode::ASSIGN_FIELD_AND => self.handle_fu64_field_assign_op(|a,b| a&b),
            Opcode::ASSIGN_FIELD_OR => self.handle_fu64_field_assign_op(|a,b| a|b),
            Opcode::ASSIGN_FIELD_XOR => self.handle_fu64_field_assign_op(|a,b| a^b),
            Opcode::ASSIGN_FIELD_SHL => self.handle_fu64_field_assign_op(|a,b| a<<b),
            Opcode::ASSIGN_FIELD_SHR => self.handle_fu64_field_assign_op(|a,b| a>>b),
            Opcode::ASSIGN_FIELD_IFNIL => self.handle_assign_field_ifnil(),
                        
            Opcode::ASSIGN_INDEX => self.handle_assign_index(),
            Opcode::ASSIGN_INDEX_ADD => self.handle_assign_index_add(),
            Opcode::ASSIGN_INDEX_SUB => self.handle_f64_index_assign_op(|a,b| a-b),
            Opcode::ASSIGN_INDEX_MUL => self.handle_f64_index_assign_op(|a,b| a*b),
            Opcode::ASSIGN_INDEX_DIV => self.handle_f64_index_assign_op(|a,b| a/b),
            Opcode::ASSIGN_INDEX_MOD => self.handle_f64_index_assign_op(|a,b| a%b),
            Opcode::ASSIGN_INDEX_AND => self.handle_fu64_index_assign_op(|a,b| a&b),
            Opcode::ASSIGN_INDEX_OR => self.handle_fu64_index_assign_op(|a,b| a|b),
            Opcode::ASSIGN_INDEX_XOR => self.handle_fu64_index_assign_op(|a,b| a^b),
            Opcode::ASSIGN_INDEX_SHL => self.handle_fu64_index_assign_op(|a,b| a<<b),
            Opcode::ASSIGN_INDEX_SHR => self.handle_fu64_index_assign_op(|a,b| a>>b),
            Opcode::ASSIGN_INDEX_IFNIL => self.handle_assign_index_ifnil(),

// ASSIGN ME            
            Opcode::ASSIGN_ME => self.handle_assign_me(),
            Opcode::ASSIGN_ME_BEFORE | Opcode::ASSIGN_ME_AFTER => self.handle_assign_me_before_after(opcode),
            Opcode::ASSIGN_ME_BEGIN => self.handle_assign_me_begin(),
            
// CONCAT  
            Opcode::CONCAT => self.handle_concat(),

// EQUALITY
            Opcode::EQ => self.handle_eq(),
            Opcode::NEQ => self.handle_neq(),
            Opcode::LT => self.handle_f64_cmp_op(opargs, |a,b| a<b),
            Opcode::GT => self.handle_f64_cmp_op(opargs, |a,b| a>b),
            Opcode::LEQ => self.handle_f64_cmp_op(opargs, |a,b| a<=b),
            Opcode::GEQ => self.handle_f64_cmp_op(opargs, |a,b| a>=b),
            
            Opcode::LOGIC_AND_TEST => self.handle_logic_and_test(opargs),
            Opcode::LOGIC_OR_TEST => self.handle_logic_or_test(opargs),
            Opcode::NIL_OR_TEST => self.handle_nil_or_test(opargs),
            Opcode::SHALLOW_EQ => self.handle_shallow_eq(),
            Opcode::SHALLOW_NEQ => self.handle_shallow_neq(),

// Object/Array begin
            Opcode::BEGIN_PROTO => self.handle_begin_proto(),
            Opcode::PROTO_INHERIT_READ => self.handle_proto_inherit_read(),
            Opcode::PROTO_INHERIT_WRITE => self.handle_proto_inherit_write(),
            Opcode::SCOPE_INHERIT_READ => self.handle_scope_inherit_read(),
            Opcode::SCOPE_INHERIT_WRITE => self.handle_scope_inherit_write(),
            Opcode::FIELD_INHERIT_READ => self.handle_field_inherit_read(),
            Opcode::FIELD_INHERIT_WRITE => self.handle_field_inherit_write(),
            Opcode::INDEX_INHERIT_READ => self.handle_index_inherit_read(),
            Opcode::INDEX_INHERIT_WRITE => self.handle_index_inherit_write(),
            Opcode::END_PROTO => self.handle_end_proto(),
            Opcode::BEGIN_BARE => self.handle_begin_bare(),
            Opcode::END_BARE => self.handle_end_bare(),
            Opcode::BEGIN_ARRAY => self.handle_begin_array(),
            Opcode::END_ARRAY => self.handle_end_array(),

// Calling
            Opcode::CALL_ARGS => self.handle_call_args(),
            Opcode::CALL_EXEC | Opcode::METHOD_CALL_EXEC => {
                let should_pop_to_me = self.handle_call_exec(opargs);
                if should_pop_to_me && opargs.is_pop_to_me(){
                    self.pop_to_me();
                }
                return
            }
            Opcode::METHOD_CALL_ARGS => {
                if self.handle_method_call_args() {
                    // Pod case: return early, skip pop_to_me (original returned before end)
                    return
                }
                // Normal case: falls through to end-of-function pop_to_me check
            }

// Fn def
            Opcode::FN_ARGS => self.handle_fn_args(),
            Opcode::FN_LET_ARGS => self.handle_fn_let_args(),
            Opcode::FN_ARG_DYN => self.handle_fn_arg_dyn(opargs),
            Opcode::FN_ARG_TYPED => self.handle_fn_arg_typed(opargs),
            Opcode::FN_BODY_DYN => self.handle_fn_body_dyn(opargs),
            Opcode::FN_BODY_TYPED => self.handle_fn_body_typed(opargs),
            Opcode::RETURN => {
                self.handle_return(opargs);
                if opargs.is_pop_to_me(){
                    self.pop_to_me();
                }
                return
            }
            Opcode::RETURN_IF_ERR => {
                if self.handle_return_if_err(opargs) {
                    // Error case: original fell through to end-of-function check
                    if opargs.is_pop_to_me(){
                        self.pop_to_me();
                    }
                    return
                }
                // Non-error case: falls through to end-of-function pop_to_me check
            }

// IF            
            Opcode::IF_TEST => self.handle_if_test(opargs),
            Opcode::IF_ELSE => self.handle_if_else(opargs),

// Use            
            Opcode::USE => {
                self.handle_use();
                // Original returned early, skipping pop_to_me
                return
            }

// Field            
            Opcode::FIELD => self.handle_field(),
            Opcode::FIELD_NIL => self.handle_field_nil(),
            Opcode::ME_FIELD => self.handle_me_field(),
            Opcode::PROTO_FIELD => self.handle_proto_field(),
            Opcode::POP_TO_ME => self.handle_pop_to_me(),
            Opcode::ME_SPLAT => self.handle_me_splat(),

// Array index            
            Opcode::ARRAY_INDEX => self.handle_array_index(),

// Let                   
            Opcode::LET_DYN => self.handle_let_dyn(opargs),
            Opcode::LET_TYPED => self.handle_let_typed(opargs),
            Opcode::VAR_DYN => self.handle_var_dyn(opargs),
            Opcode::VAR_TYPED => self.handle_var_typed(opargs),

// Tree search            
            Opcode::SEARCH_TREE => self.handle_search_tree(),

// Log            
            Opcode::LOG => self.handle_log(),

// Me/Scope
            Opcode::ME => self.handle_me(),
            Opcode::SCOPE => self.handle_scope(),

// For            
            Opcode::FOR_1 => self.handle_for_1(opargs),
            Opcode::FOR_2 => self.handle_for_2(opargs),
            Opcode::FOR_3 => self.handle_for_3(opargs),
            Opcode::LOOP => self.handle_loop(opargs),
            Opcode::FOR_END => self.handle_for_end(),
            Opcode::BREAK => self.handle_break(),
            Opcode::BREAKIFNOT => self.handle_breakifnot(),
            Opcode::CONTINUE => self.handle_continue(),

// Range            
            Opcode::RANGE => self.handle_range(),

// Is            
            Opcode::IS => self.handle_is(),

// Try / OK            
            Opcode::OK_TEST => self.handle_ok_test(opargs),
            Opcode::OK_END => self.handle_ok_end(),
            Opcode::TRY_TEST => self.handle_try_test(opargs),
            Opcode::TRY_ERR => self.handle_try_err(opargs),
            Opcode::TRY_OK => self.handle_try_ok(opargs),

// Destructuring
            Opcode::DUP => self.handle_dup(),
            Opcode::DROP => self.handle_drop(),
            Opcode::ARRAY_INDEX_NIL => self.handle_array_index_nil(),
            Opcode::LET_DESTRUCT_ARRAY_EL => self.handle_let_destruct_array_el(opargs),
            Opcode::LET_DESTRUCT_OBJECT_EL => self.handle_let_destruct_object_el(),

            opcode => {
                println!("UNDEFINED OPCODE {}", opcode);
                self.bx.threads.cur().trap.goto_next();
            }
        }
        if opargs.is_pop_to_me(){
            self.pop_to_me();
        }
    }
}
