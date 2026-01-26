//! Shader control flow operations
//!
//! This module contains handle functions for control flow: if/else statements,
//! for loops, ranges, and return statements.

use std::fmt::Write;
use makepad_live_id::*;
use crate::vm::*;
use crate::opcode::*;
use crate::shader::*;
use crate::shader_tables::*;

impl ShaderFnCompiler {
    /// Check if we're currently in unreachable code (after a return in the current branch)
    pub(crate) fn is_unreachable(&self) -> bool {
        // Check if ANY IfBody in the scope chain has returned (making subsequent code unreachable)
        // or if the FnBody is fully escaped
        for me in self.mes.iter().rev() {
            match me {
                ShaderMe::IfBody { has_return, .. } => {
                    if *has_return {
                        return true;
                    }
                    // Continue checking parent scopes
                }
                ShaderMe::FnBody { escaped, .. } => {
                    return *escaped;
                }
                _ => {}
            }
        }
        false
    }

    /// Check if the PARENT scope is unreachable (skipping the innermost IfBody)
    /// Used for IF_ELSE to determine if the else branch should generate code
    pub(crate) fn is_parent_scope_unreachable(&self) -> bool {
        let mut skipped_first_if = false;
        for me in self.mes.iter().rev() {
            match me {
                ShaderMe::IfBody { has_return, .. } => {
                    if !skipped_first_if {
                        // Skip the innermost IfBody (the one we're transitioning out of)
                        skipped_first_if = true;
                        continue;
                    }
                    if *has_return {
                        return true;
                    }
                }
                ShaderMe::FnBody { escaped, .. } => {
                    return *escaped;
                }
                _ => {}
            }
        }
        false
    }

    pub(crate) fn handle_if_else_phi(&mut self, vm: &ScriptVm, output: &ShaderOutput) {
        if let Some(ShaderMe::IfBody { target_ip, phi, start_pos, stack_depth, phi_type, has_return, if_branch_returned }) = self.mes.last() {
            if self.trap.ip.index >= *target_ip {
                // Check if both branches returned (escape analysis)
                let both_returned = *if_branch_returned && *has_return;

                if self.stack.types.len() > *stack_depth {
                    let (ty, val) = self.stack.pop(&self.trap);
                    if let Some(phi) = phi {
                        if let Some(phi_type) = phi_type {
                            self.out.push_str(&format!("{} = {};\n", phi, val));
                            // declare the phi at start
                            let ty = type_table_if_else(phi_type, &ty, &self.trap, &vm.code.builtins.pod);
                            let ty = ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
                            let ty_name = if let Some(name) = vm.heap.pod_type_name(ty) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(unknown)
                            };
                            let mut s = self.stack.new_string();
                            write!(s, "let {phi}:{ty_name};\n").ok();
                            self.out.insert_str(*start_pos, &s);
                            self.stack.free_string(s);
                            let mut s = self.stack.new_string();
                            write!(s, "{}", phi).ok();
                            self.stack.push(&self.trap, ShaderType::Pod(ty), s);
                        }
                    }
                    self.stack.free_string(val);
                }
                self.out.push_str("}\n");
                self.shader_scope.exit_scope();
                self.mes.pop();

                // If both branches returned, propagate escape status up
                if both_returned {
                    // Find the parent and mark it as having returned/escaped
                    if let Some(parent) = self.mes.last_mut() {
                        match parent {
                            ShaderMe::IfBody { has_return, .. } => {
                                *has_return = true;
                            }
                            ShaderMe::FnBody { escaped, .. } => {
                                *escaped = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn handle_if_test(&mut self, opargs: OpcodeArgs) {
        let (_ty, val) = self.stack.pop(&self.trap);
        let start_pos = self.out.len();
        self.out.push_str("if(");
        self.out.push_str(&val);
        self.out.push_str("){\n");
        self.shader_scope.enter_scope();
        self.stack.free_string(val);

        self.mes.push(ShaderMe::IfBody {
            target_ip: self.trap.ip.index + opargs.to_u32(),
            start_pos,
            stack_depth: self.stack.types.len(),
            phi: None,
            phi_type: None,
            has_return: false,
            if_branch_returned: false,
        });
    }

    /// Handle IF_TEST when in unreachable code - don't generate code or pop stack,
    /// but track the control structure so we can properly close it
    pub(crate) fn handle_if_test_unreachable(&mut self, opargs: OpcodeArgs) {
        // Don't pop from stack or generate code - just track the structure
        // Mark has_return: true since we're already in unreachable code
        self.mes.push(ShaderMe::IfBody {
            target_ip: self.trap.ip.index + opargs.to_u32(),
            start_pos: self.out.len(),
            stack_depth: self.stack.types.len(),
            phi: None,
            phi_type: None,
            has_return: true, // Already unreachable, so this branch is "returned"
            if_branch_returned: false,
        });
    }

    pub(crate) fn handle_if_else(&mut self, opargs: OpcodeArgs) {
        if let Some(ShaderMe::IfBody {
            target_ip,
            start_pos,
            stack_depth,
            phi,
            phi_type,
            has_return,
            if_branch_returned,
        }) = self.mes.last_mut()
        {
            if self.stack.types.len() > *stack_depth {
                let (ty, val) = self.stack.pop(&self.trap);
                *phi_type = Some(ty);
                let phi_name = if let Some(p) = phi {
                    p.clone()
                } else {
                    let s = format!("_phi_{}", start_pos);
                    *phi = Some(s.clone());
                    s
                };
                self.out.push_str(&format!("{} = {};\n", phi_name, val));
                self.stack.free_string(val);
            }
            self.out.push_str("}\nelse{\n");
            self.shader_scope.exit_scope();
            self.shader_scope.enter_scope();
            *target_ip = self.trap.ip.index + opargs.to_u32();
            // Save whether the if-branch returned, reset has_return for else branch
            *if_branch_returned = *has_return;
            *has_return = false;
        } else {
            self.trap.err_unexpected();
        }
    }

    /// Handle IF_ELSE when in unreachable code - just update structure, no code generation
    pub(crate) fn handle_if_else_unreachable(&mut self, opargs: OpcodeArgs) {
        if let Some(ShaderMe::IfBody {
            target_ip,
            has_return,
            if_branch_returned,
            ..
        }) = self.mes.last_mut()
        {
            *target_ip = self.trap.ip.index + opargs.to_u32();
            // Save whether the if-branch "returned", keep has_return true since we're unreachable
            *if_branch_returned = *has_return;
            // Keep has_return true since we're in unreachable code - else branch is also unreachable
            *has_return = true;
        }
    }

    /// Handle if/else phi when in unreachable code - just close the structure
    pub(crate) fn handle_if_else_phi_unreachable(&mut self) {
        if let Some(ShaderMe::IfBody { target_ip, has_return, if_branch_returned, .. }) = self.mes.last() {
            if self.trap.ip.index >= *target_ip {
                let both_returned = *if_branch_returned && *has_return;
                self.mes.pop();

                // If both branches returned, propagate up
                if both_returned {
                    if let Some(parent) = self.mes.last_mut() {
                        match parent {
                            ShaderMe::IfBody { has_return, .. } => {
                                *has_return = true;
                            }
                            ShaderMe::FnBody { escaped, .. } => {
                                *escaped = true;
                            }
                            _ => {}
                        }
                    }
                }
            }
        }
    }

    pub(crate) fn handle_return(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs) {
        // Check if we're already escaped (all code paths have returned)
        let already_escaped = self.mes.iter().rev()
            .find_map(|me| match me {
                ShaderMe::FnBody { escaped, .. } => Some(*escaped),
                _ => None,
            })
            .unwrap_or(false);

        if already_escaped {
            // Still need to consume the stack value if present
            if !opargs.is_nil() {
                let (_ty, s) = self.stack.pop(&self.trap);
                self.stack.free_string(s);
            }
            return;
        }

        // Check if we're inside an IfBody before taking mutable borrow
        let inside_if = self.mes.iter().any(|me| matches!(me, ShaderMe::IfBody { .. }));

        // Pop and resolve the return value BEFORE borrowing self.mes mutably
        // Use pop_resolved to resolve Id types (like variable names) to their actual Pod types
        let (ty, s) = if opargs.is_nil() {
            (vm.code.builtins.pod.pod_void, self.stack.new_string())
        } else {
            let (ty, s) = self.pop_resolved(vm, output);
            let ty = ty.make_concrete(&vm.code.builtins.pod).unwrap_or(vm.code.builtins.pod.pod_void);
            (ty, s)
        };

        // Find our FnBody to record return type
        if let Some(me) = self.mes.iter_mut().rev().find(|v| matches!(v, ShaderMe::FnBody { .. })) {
            if let ShaderMe::FnBody { ret, escaped } = me {
                if let Some(ret) = ret {
                    if ty != *ret {
                        self.trap.err_return_type_changed();
                    }
                }
                *ret = Some(ty);

                if ty == vm.code.builtins.pod.pod_void {
                    self.out.push_str(&s);
                    self.out.push_str(";\nreturn;\n");
                } else {
                    self.out.push_str("return ");
                    self.out.push_str(&s);
                    self.out.push_str(";\n");
                }

                // If not inside an IfBody (return at function level), mark function as escaped
                if !inside_if {
                    *escaped = true;
                }
            }
        }

        self.stack.free_string(s);

        // Mark the innermost IfBody as having a return
        if let Some(me) = self.mes.iter_mut().rev().find(|v| matches!(v, ShaderMe::IfBody { .. })) {
            if let ShaderMe::IfBody { has_return, .. } = me {
                *has_return = true;
            }
        }

        // NOTE: For a transpiler (unlike an interpreter), we do NOT set the trap here.
        // The interpreter sets ScriptTrapOn::Return to actually return control flow,
        // but the transpiler just generates code and must continue processing all opcodes
        // to properly close if/else blocks and other control structures.
        // The compile_fn loop uses fn_end_index (derived from FN_BODY_DYN's opargs) to know
        // when to stop, rather than relying on the Return trap.
    }

    pub(crate) fn handle_for_1(&mut self) {
        let (source, _) = self.stack.pop(&self.trap);
        let (val_id, _) = self.stack.pop(&self.trap);
        if let ShaderType::Range { start, end, ty } = source {
            if let ShaderType::Id(id) = val_id {
                self.shader_scope.enter_scope();
                self.shader_scope.define_var(id, ty);
                write!(self.out, "for(var {0} = {1}; {0} < {2}; {0}++){{\n", id, start, end).ok();
                self.mes.push(ShaderMe::ForLoop { var_id: id });
            } else {
                self.trap.err_unexpected();
            }
        } else {
            self.trap.err_unexpected();
        }
    }

    pub(crate) fn handle_for_end(&mut self) {
        if let Some(me) = self.mes.pop() {
            if let ShaderMe::ForLoop { .. } = me {
                self.out.push_str("}\n");
                self.shader_scope.exit_scope();
            } else {
                self.trap.err_unexpected();
            }
        } else {
            self.trap.err_unexpected();
        }
    }

    pub(crate) fn handle_range(&mut self, vm: &mut ScriptVm) {
        let (_end_ty, end_s) = self.stack.pop(&self.trap);
        let (start_ty, start_s) = self.stack.pop(&self.trap);
        if let Some(ty) = start_ty.make_concrete(&vm.code.builtins.pod) {
            self.stack.push(
                &self.trap,
                ShaderType::Range {
                    start: start_s,
                    end: end_s,
                    ty,
                },
                String::new(),
            );
        } else {
            self.trap.err_no_matching_shader_type();
        }
    }
}
