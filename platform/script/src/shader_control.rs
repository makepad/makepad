//! Shader control flow operations
//!
//! This module contains handle functions for control flow: if/else statements,
//! for loops, ranges, and return statements.

use crate::opcode::*;
use crate::shader::*;
use crate::shader_backend::ShaderBackend;
use crate::shader_tables::*;
use crate::suggest::format_pod_type_name;
use crate::vm::*;
use crate::*;
use makepad_live_id::*;
use std::fmt::Write;

/// Hard ceiling on iterations for any loop whose bound this compiler cannot
/// prove at compile time.
///
/// Shader source reaching this compiler may come from a downloaded game or
/// from an AI, and a loop that never terminates does not merely misbehave —
/// it hangs the GPU and most drivers respond by resetting the device, which
/// takes the whole app with it (and on a headset, that is about as bad as a
/// failure gets). Since the loop bound is an arbitrary expression string,
/// rather than trying to prove it safe, the cap is *emitted* into the
/// generated code: legitimate shaders never approach it, pathological ones
/// terminate. 65536 is far above any real per-pixel loop (the built-ins use
/// single- or double-digit counts) while still bounding a runaway to a
/// blink rather than a hang.
pub const LOOP_GUARD_MAX_ITERS: u32 = 65536;

/// Parse an emitted bound expression as a plain integer literal.
///
/// Bounds arrive as already-lowered target-language strings, so this
/// deliberately recognises only the simple literal forms (`7`, `7u`, `16u32`,
/// and the casts the backends emit). Anything else — a uniform, an
/// arithmetic expression, a function call — is treated as unprovable and
/// gets the emitted guard. Being conservative here costs one comparison per
/// iteration in rare loops; being permissive would cost a device reset.
pub(crate) fn literal_bound(expr: &str) -> Option<u64> {
    let s = expr.trim();
    // Unwrap a single layer of backend casts: `uint(12)`, `u32(12)`.
    let s = s
        .strip_prefix("uint(")
        .or_else(|| s.strip_prefix("u32("))
        .and_then(|r| r.strip_suffix(')'))
        .map(str::trim)
        .unwrap_or(s);
    let digits = s
        .trim_end_matches("u32")
        .trim_end_matches("u")
        .trim_end_matches("U");
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    digits.parse::<u64>().ok()
}

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

    /// Find an outer IfBody's phi variable (skipping the innermost IfBody) and mark it as assigned.
    /// Used when an inner if-block (like in match/else-if chains) has a value but no phi,
    /// and we need to assign to an outer phi instead.
    /// Returns the phi name if found, and sets `phi_assigned_by_inner` on the outer IfBody.
    pub(crate) fn find_and_mark_outer_phi(&mut self) -> Option<String> {
        let mut skipped_first_if = false;
        for me in self.mes.iter_mut().rev() {
            if let ShaderMe::IfBody {
                phi,
                phi_assigned_by_inner,
                ..
            } = me
            {
                if !skipped_first_if {
                    // Skip the innermost IfBody (the one we're closing)
                    skipped_first_if = true;
                    continue;
                }
                // Found an outer IfBody - return its phi if present and mark it
                if let Some(phi) = phi {
                    *phi_assigned_by_inner = true;
                    return Some(phi.clone());
                }
            }
        }
        None
    }

    pub(crate) fn handle_if_else_phi(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        // Loop to handle ALL IfBodies whose target_ip has been reached.
        // This is important for match/else-if chains where multiple ifs end at the same position.
        loop {
            // Check if the last me is an IfBody that needs closing
            let should_handle = if let Some(ShaderMe::IfBody { target_ip, .. }) = self.mes.last() {
                self.trap.ip.index >= *target_ip
            } else {
                false
            };

            if !should_handle {
                break;
            }

            // Now extract the fields we need (we know it's an IfBody that needs handling)
            if let Some(ShaderMe::IfBody {
                target_ip: _,
                phi,
                start_pos,
                stack_depth,
                phi_type,
                has_return,
                if_branch_returned,
                phi_assigned_by_inner,
                created_unreachable: _,
            }) = self.mes.last()
            {
                // Check if both branches returned (escape analysis)
                let both_returned = *if_branch_returned && *has_return;

                // Clone/copy what we need before any mutable operations
                let phi = phi.clone();
                let start_pos = *start_pos;
                let stack_depth = *stack_depth;
                let phi_type = phi_type.clone();
                let has_return = *has_return;
                let phi_assigned_by_inner = *phi_assigned_by_inner;

                if self.stack.types.len() > stack_depth {
                    // Else branch has a value on the stack
                    let (ty, val) = self.pop_resolved(vm, output);

                    // Check if the else value is void
                    let else_concrete = ty.make_concrete(&vm.bx.code.builtins.pod);
                    let else_is_void = else_concrete
                        .map(|t| t == vm.bx.code.builtins.pod.pod_void)
                        .unwrap_or(false);

                    if else_is_void {
                        // Emit void value as statement
                        if !val.is_empty() {
                            self.out.push_str(&val);
                            self.out.push_str(";\n");
                        }
                    } else if let Some(ref phi) = phi {
                        if let Some(ref phi_type) = phi_type {
                            // declare the phi at start
                            let ty = type_table_if_else(
                                phi_type,
                                &ty,
                                self.trap.pass(),
                                &vm.bx.code.builtins.pod,
                            );
                            let ty = ty
                                .make_concrete(&vm.bx.code.builtins.pod)
                                .unwrap_or(vm.bx.code.builtins.pod.pod_void);

                            // Skip phi handling if type is void
                            if ty != vm.bx.code.builtins.pod.pod_void {
                                self.out.push_str(&format!("{} = {};\n", phi, val));
                                let ty_name = if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                                    output.backend.map_pod_name(name)
                                } else {
                                    id!(unknown)
                                };
                                // Generate backend-appropriate variable declaration with zero init
                                let mut s = self.stack.new_string();
                                output
                                    .backend
                                    .write_var_decl_zero_init(&mut s, ty_name, phi);
                                self.out.insert_str(start_pos, &s);
                                self.stack.free_string(s);
                                let mut s = self.stack.new_string();
                                write!(s, "{}", phi).ok();
                                self.stack.push(self.trap.pass(), ShaderType::Pod(ty), s);
                            }
                        }
                    } else {
                        // No phi for this IfBody, but we have a non-void value.
                        // This happens in match/else-if chains where inner if has no else.
                        // Look for an outer IfBody's phi to assign to.
                        let outer_phi = self.find_and_mark_outer_phi();
                        if let Some(outer_phi) = outer_phi {
                            // Assign to the outer phi (flag is already set by find_and_mark_outer_phi)
                            self.out.push_str(&format!("{} = {};\n", outer_phi, val));
                        }
                        // If no outer phi, the value is discarded (this shouldn't happen in well-formed code)
                    }
                    self.stack.free_string(val);
                } else if let Some(ref phi) = phi {
                    // If branch had a value (created phi) but else branch has no value on stack.
                    // This can happen in two cases:
                    // 1. Only if-branch has value (else is statement-only) - don't push result
                    // 2. Inner if assigned to our phi (match/else-if) - push result
                    if let Some(ref phi_type) = phi_type {
                        let ty = phi_type
                            .make_concrete(&vm.bx.code.builtins.pod)
                            .unwrap_or(vm.bx.code.builtins.pod.pod_void);

                        // Skip phi handling if type is void
                        if ty != vm.bx.code.builtins.pod.pod_void {
                            let ty_name = if let Some(name) = vm.bx.heap.pod_type_name(ty) {
                                output.backend.map_pod_name(name)
                            } else {
                                id!(unknown)
                            };
                            // Generate backend-appropriate variable declaration with zero init
                            let mut s = self.stack.new_string();
                            output
                                .backend
                                .write_var_decl_zero_init(&mut s, ty_name, phi);
                            self.out.insert_str(start_pos, &s);
                            self.stack.free_string(s);

                            // If inner code assigned to our phi, push the result onto the stack
                            if phi_assigned_by_inner {
                                let mut s = self.stack.new_string();
                                write!(s, "{}", phi).ok();
                                self.stack.push(self.trap.pass(), ShaderType::Pod(ty), s);
                            }
                        }
                    }
                } else if has_return {
                    // If branch had a return with no else branch and no phi value.
                    // The following POP_TO_ME opcode expects a value but there isn't one.
                    // Skip it to avoid stack underflow.
                    self.skip_next_pop_to_me = true;
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
            } else {
                // Shouldn't happen since we checked above
                break;
            }
        }
    }

    pub(crate) fn handle_if_test(&mut self, opargs: OpcodeArgs) {
        let (_ty, val) = self.stack.pop(self.trap.pass());
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
            phi_assigned_by_inner: false,
            created_unreachable: false,
        });
    }

    /// Handle IF_TEST when in unreachable code - don't generate code or pop stack,
    /// but track the control structure so we can properly close it
    pub(crate) fn handle_if_test_unreachable(&mut self, opargs: OpcodeArgs) {
        // Don't pop from stack or generate code - just track the structure
        // Mark has_return: true since we're already in unreachable code
        // Mark created_unreachable: true so we don't emit closing } later
        self.mes.push(ShaderMe::IfBody {
            target_ip: self.trap.ip.index + opargs.to_u32(),
            start_pos: self.out.len(),
            stack_depth: self.stack.types.len(),
            phi: None,
            phi_type: None,
            has_return: true, // Already unreachable, so this branch is "returned"
            if_branch_returned: false,
            phi_assigned_by_inner: false,
            created_unreachable: true, // No code emitted for this if block
        });
    }

    pub(crate) fn handle_if_else(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
    ) {
        let popped = if let Some(ShaderMe::IfBody { stack_depth, .. }) = self.mes.last() {
            if self.stack.types.len() > *stack_depth {
                Some(self.pop_resolved(vm, output))
            } else {
                None
            }
        } else {
            None
        };

        if let Some(ShaderMe::IfBody {
            target_ip,
            start_pos,
            stack_depth: _,
            phi,
            phi_type,
            has_return,
            if_branch_returned,
            phi_assigned_by_inner: _,
            created_unreachable: _,
        }) = self.mes.last_mut()
        {
            if let Some((ty, val)) = popped {
                // Check if the type is void - if so, don't create a phi, just emit as statement
                let concrete_ty = ty.make_concrete(&vm.bx.code.builtins.pod);
                let is_void = concrete_ty
                    .map(|t| t == vm.bx.code.builtins.pod.pod_void)
                    .unwrap_or(false);

                if is_void {
                    // Emit as statement without phi assignment
                    if !val.is_empty() {
                        self.out.push_str(&val);
                        self.out.push_str(";\n");
                    }
                } else {
                    *phi_type = Some(ty);
                    let phi_name = if let Some(p) = phi {
                        p.clone()
                    } else {
                        let s = format!("_phi_{}", start_pos);
                        *phi = Some(s.clone());
                        s
                    };
                    self.out.push_str(&format!("{} = {};\n", phi_name, val));
                }
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
            script_err_unexpected!(self.trap, "unexpected in shader control");
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

    /// Handle if/else phi when in unreachable code - close the structure properly
    pub(crate) fn handle_if_else_phi_unreachable(&mut self) {
        if let Some(ShaderMe::IfBody {
            target_ip,
            has_return,
            if_branch_returned,
            created_unreachable,
            ..
        }) = self.mes.last()
        {
            if self.trap.ip.index >= *target_ip {
                let both_returned = *if_branch_returned && *has_return;
                let was_created_unreachable = *created_unreachable;

                // Only emit closing brace if code was actually generated for this if block
                // If created_unreachable is true, no `if(...){` was emitted, so no `}` needed
                if !was_created_unreachable {
                    self.out.push_str("}\n");
                    self.shader_scope.exit_scope();
                }
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

    pub(crate) fn handle_return(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
    ) {
        // Check if we're already escaped (all code paths have returned)
        let already_escaped = self
            .mes
            .iter()
            .rev()
            .find_map(|me| match me {
                ShaderMe::FnBody { escaped, .. } => Some(*escaped),
                _ => None,
            })
            .unwrap_or(false);

        if already_escaped {
            // Still need to consume the stack value if present
            if !opargs.is_nil() {
                let (_ty, s) = self.stack.pop(self.trap.pass());
                self.stack.free_string(s);
            }
            return;
        }

        // Check if we're inside an IfBody before taking mutable borrow
        let inside_if = self
            .mes
            .iter()
            .any(|me| matches!(me, ShaderMe::IfBody { .. }));

        // Pop and resolve the return value BEFORE borrowing self.mes mutably
        // Use pop_resolved to resolve Id types (like variable names) to their actual Pod types
        let (ty, s) = if opargs.is_nil() {
            (vm.bx.code.builtins.pod.pod_void, self.stack.new_string())
        } else {
            let (ty, s) = self.pop_resolved(vm, output);
            let ty = ty
                .make_concrete(&vm.bx.code.builtins.pod)
                .unwrap_or(vm.bx.code.builtins.pod.pod_void);
            (ty, s)
        };

        // Find our FnBody to record return type
        if let Some(me) = self
            .mes
            .iter_mut()
            .rev()
            .find(|v| matches!(v, ShaderMe::FnBody { .. }))
        {
            if let ShaderMe::FnBody { ret, escaped, .. } = me {
                if let Some(ret) = ret {
                    if ty != *ret {
                        script_err_inconsistent!(self.trap, "return type changed");
                    }
                }
                *ret = Some(ty);

                if ty == vm.bx.code.builtins.pod.pod_void {
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
        if let Some(me) = self
            .mes
            .iter_mut()
            .rev()
            .find(|v| matches!(v, ShaderMe::IfBody { .. }))
        {
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

    pub(crate) fn handle_for_1(
        &mut self,
        vm: &mut ScriptVm,
        _output: &mut ShaderOutput,
        backend: &ShaderBackend,
    ) {
        let (source, _) = self.stack.pop(self.trap.pass());
        let (val_id, _) = self.stack.pop(self.trap.pass());
        if let ShaderType::Range { start, end, mut ty } = source {
            if let ShaderType::Id(id) = val_id {
                // Shader for loops only support u32 for now.
                // If the range is abstract int or i32, we cast it to u32.
                if ty == vm.bx.code.builtins.pod.pod_i32 {
                    ty = vm.bx.code.builtins.pod.pod_u32;
                }

                if ty != vm.bx.code.builtins.pod.pod_u32 {
                    script_err_type_mismatch!(
                        self.trap,
                        "shader for loop only supports u32 range, got {}",
                        format_pod_type_name(&vm.bx.heap, ty)
                    );
                }
                self.shader_scope.enter_scope();
                let shadow = self.shader_scope.define_var(id, ty);
                let loop_var_name = backend.map_local_name(id, shadow);
                let ty_name = backend.map_pod_name(id!(u32));
                // `end` is an arbitrary expression string, so a bound like
                // `0..some_uniform` is unbounded at compile time. A literal
                // small enough to trust is emitted as-is (the overwhelmingly
                // common case, and it keeps generated code readable); anything
                // else gets a hard iteration cap so a hostile or merely buggy
                // shader terminates instead of hanging the GPU.
                let bounded = literal_bound(&end).is_some_and(|n| n <= LOOP_GUARD_MAX_ITERS as u64);
                match backend {
                    ShaderBackend::Wgsl => {
                        write!(
                            self.out,
                            "for(var {0}: {3} = {1}; {0} < {2}; {0}++){{\n",
                            loop_var_name, start, end, ty_name
                        )
                        .ok();
                    }
                    ShaderBackend::Rust => {
                        write!(
                            self.out,
                            "for {0} in {1}..{2} {{\n",
                            loop_var_name, start, end
                        )
                        .ok();
                    }
                    ShaderBackend::Glsl => {
                        // GLSL ES 3.0 disallows implicit casts from int to uint,
                        // so we must explicitly cast the bounds to the loop variable type.
                        write!(
                            self.out,
                            "for({3} {0} = {3}({1}); {0} < {3}({2}); {0}++){{\n",
                            loop_var_name, start, end, ty_name
                        )
                        .ok();
                    }
                    _ => {
                        write!(
                            self.out,
                            "for({3} {0} = {1}; {0} < {2}; {0}++){{\n",
                            loop_var_name, start, end, ty_name
                        )
                        .ok();
                    }
                }
                if !bounded {
                    // The loop variable itself is the counter, so no extra
                    // local is needed: bail once it has advanced further from
                    // its start than any legitimate shader loop would.
                    match backend {
                        ShaderBackend::Rust => {
                            write!(
                                self.out,
                                "if {loop_var_name} - ({start}) >= {LOOP_GUARD_MAX_ITERS} {{ break; }}\n"
                            )
                            .ok();
                        }
                        _ => {
                            write!(
                                self.out,
                                "if({loop_var_name} - ({start}) >= {LOOP_GUARD_MAX_ITERS}u){{break;}}\n"
                            )
                            .ok();
                        }
                    }
                }
                self.mes.push(ShaderMe::ForLoop {
                    var_id: id,
                    stack_depth: self.stack.types.len(),
                });
            } else {
                script_err_unexpected!(self.trap, "unexpected in shader control");
            }
        } else {
            script_err_unexpected!(self.trap, "unexpected in shader control");
        }
    }

    pub(crate) fn handle_for_end(&mut self) {
        if let Some(me) = self.mes.pop() {
            match me {
                ShaderMe::ForLoop { .. } | ShaderMe::LoopBody { .. } => {
                    self.out.push_str("}\n");
                    self.shader_scope.exit_scope();
                }
                _ => {
                    script_err_unexpected!(self.trap, "unexpected in shader control");
                }
            }
        } else {
            script_err_unexpected!(self.trap, "unexpected in shader control");
        }
    }

    pub(crate) fn handle_loop(&mut self, output: &mut ShaderOutput, backend: &ShaderBackend) {
        self.shader_scope.enter_scope();
        let guard = output.next_loop_guard();
        // `loop{}` breaks on a runtime condition, so nothing in the source
        // bounds it. A shader that never exits hangs the GPU and, on most
        // drivers, takes the whole app down with a device reset — so the
        // bound is emitted rather than trusted. See LOOP_GUARD_MAX_ITERS.
        match backend {
            ShaderBackend::Wgsl => {
                write!(self.out, "var {guard}: u32 = 0u;\nloop{{\n").ok();
                write!(
                    self.out,
                    "{guard} = {guard} + 1u; if({guard} > {LOOP_GUARD_MAX_ITERS}u){{break;}}\n"
                )
                .ok();
            }
            ShaderBackend::Rust => {
                write!(self.out, "let mut {guard}: u32 = 0;\nwhile true {{\n").ok();
                write!(
                    self.out,
                    "{guard} += 1; if {guard} > {LOOP_GUARD_MAX_ITERS} {{ break; }}\n"
                )
                .ok();
            }
            _ => {
                write!(self.out, "uint {guard} = 0u;\nwhile(true){{\n").ok();
                write!(
                    self.out,
                    "{guard}++; if({guard} > {LOOP_GUARD_MAX_ITERS}u){{break;}}\n"
                )
                .ok();
            }
        }
        self.mes.push(ShaderMe::LoopBody {
            stack_depth: self.stack.types.len(),
        });
    }

    pub(crate) fn handle_break(&mut self) {
        self.out.push_str("break;\n");
    }

    pub(crate) fn handle_breakifnot(&mut self) {
        let (_ty, cond_s) = self.stack.pop(self.trap.pass());
        write!(self.out, "if(!({cond_s})){{break;}}\n").ok();
    }

    pub(crate) fn handle_continue(&mut self) {
        self.out.push_str("continue;\n");
    }

    pub(crate) fn handle_range(&mut self, vm: &mut ScriptVm) {
        let (end_ty, end_s) = self.stack.pop(self.trap.pass());
        let (start_ty, start_s) = self.stack.pop(self.trap.pass());
        // Validate that both operands can be made into concrete numeric types
        let start_concrete = start_ty.make_concrete(&vm.bx.code.builtins.pod);
        let end_concrete = end_ty.make_concrete(&vm.bx.code.builtins.pod);
        if let (Some(start_pod), Some(end_pod)) = (start_concrete, end_concrete) {
            // Check that both are numeric types
            let start_is_number = vm.bx.heap.pod_type_ref(start_pod).ty.is_number();
            let end_is_number = vm.bx.heap.pod_type_ref(end_pod).ty.is_number();
            if !start_is_number || !end_is_number {
                self.stack.free_string(start_s);
                self.stack.free_string(end_s);
                script_err_type_mismatch!(self.trap, "range requires numbers");
                return;
            }
            self.stack.push(
                self.trap.pass(),
                ShaderType::Range {
                    start: start_s,
                    end: end_s,
                    ty: start_pod,
                },
                String::new(),
            );
        } else {
            self.stack.free_string(start_s);
            self.stack.free_string(end_s);
            script_err_type_mismatch!(self.trap, "range requires numbers");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // The guard is only skipped when the bound is provably a small literal.
    // Everything else must fall through to the emitted iteration cap — these
    // cases are the difference between a bounded loop and a device reset.
    #[test]
    fn literal_bounds_are_recognised() {
        assert_eq!(literal_bound("7"), Some(7));
        assert_eq!(literal_bound("7u"), Some(7));
        assert_eq!(literal_bound("16u32"), Some(16));
        assert_eq!(literal_bound(" 42 "), Some(42));
        assert_eq!(literal_bound("uint(12)"), Some(12));
        assert_eq!(literal_bound("u32(12)"), Some(12));
    }

    #[test]
    fn non_literal_bounds_are_treated_as_unprovable() {
        // A uniform, a varying, arithmetic, a call, or anything else the
        // compiler cannot fold: all must be guarded.
        for expr in [
            "count",
            "self.count",
            "n + 1",
            "uint(self.rect_size.x)",
            "textureSize(tex)",
            "",
            "0x10",
            "1e9",
            "-1",
        ] {
            assert_eq!(literal_bound(expr), None, "expr {expr:?} must not be trusted");
        }
    }

    #[test]
    fn absurd_literal_bounds_still_get_guarded() {
        // Parsed fine, but far above LOOP_GUARD_MAX_ITERS — handle_for_1
        // compares against the cap, so this still receives the break.
        let n = literal_bound("4000000000").unwrap();
        assert!(n > LOOP_GUARD_MAX_ITERS as u64);
    }

    #[test]
    fn guard_cap_is_generous_versus_real_shader_loops() {
        // Built-in shaders use single/double-digit loop counts; the cap must
        // sit far above them so no legitimate shader is ever clipped.
        assert!(LOOP_GUARD_MAX_ITERS >= 4096);
    }
}
