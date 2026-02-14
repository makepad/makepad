//! Shader arithmetic and comparison operations
//!
//! This module contains handle functions for arithmetic operations (+, -, *, /, etc.),
//! comparison operations (==, !=, <, >, etc.), and logical operations (&&, ||).

use crate::opcode::*;
use crate::shader::*;
use crate::shader_backend::ShaderBackend;
use crate::shader_tables::*;
use crate::suggest::*;
use crate::vm::*;
use crate::*;
use std::fmt::Write;

impl ShaderFnCompiler {
    pub(crate) fn handle_neg(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        _opargs: OpcodeArgs,
        op: &str,
    ) {
        let (t1, s1) = self.pop_resolved(vm, output);
        let mut s = self.stack.new_string();
        write!(s, "({}{})", op, s1).ok();
        let ty = type_table_neg(&t1, self.trap.pass(), &vm.bx.code.builtins.pod);
        self.stack.push(self.trap.pass(), ty, s);
    }

    pub(crate) fn handle_eq(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        op: &str,
    ) {
        let (t2, s2) = if opargs.is_u32() {
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        } else {
            self.pop_resolved(vm, output)
        };
        let (t1, s1) = self.pop_resolved(vm, output);
        let mut s = self.stack.new_string();
        write!(s, "({} {} {})", s1, op, s2).ok();
        let ty = type_table_eq(&t1, &t2, self.trap.pass(), &vm.bx.code.builtins.pod);
        self.stack.push(self.trap.pass(), ty, s);
    }

    /// Handle LOGIC_AND_TEST / LOGIC_OR_TEST for shaders
    /// These opcodes have short-circuit semantics in the interpreter, but in shaders
    /// we evaluate both operands and combine them with the operator.
    pub(crate) fn handle_logic_test(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        op: &'static str,
    ) {
        // Pop the first operand (already evaluated and on the stack)
        let (first_type, first_operand) = self.pop_resolved(vm, output);

        // Calculate the target IP (where the jump would land in the interpreter)
        let target_ip = self.trap.ip.index + opargs.to_u32();

        // Push a LogicOp marker - we'll combine when we reach target_ip
        self.mes.push(ShaderMe::LogicOp {
            target_ip,
            op,
            first_operand,
            first_type,
        });
    }

    /// Check if we've reached a logic operation's target IP and combine the operands
    pub(crate) fn handle_logic_phi(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        // Loop to handle nested logic ops that may complete at the same IP
        loop {
            let should_handle = if let Some(ShaderMe::LogicOp { target_ip, .. }) = self.mes.last() {
                self.trap.ip.index >= *target_ip
            } else {
                false
            };

            if !should_handle {
                break;
            }

            // Pop the LogicOp and combine with the second operand on the stack
            if let Some(ShaderMe::LogicOp {
                op,
                first_operand,
                first_type,
                ..
            }) = self.mes.pop()
            {
                // Pop the second operand (result of evaluating the RHS) - must resolve Id types
                let (second_type, second_operand) = self.pop_resolved(vm, output);

                // Combine them
                let mut s = self.stack.new_string();
                write!(s, "({} {} {})", first_operand, op, second_operand).ok();

                // Determine result type
                let ty = type_table_logic(
                    &first_type,
                    &second_type,
                    self.trap.pass(),
                    &vm.bx.code.builtins.pod,
                );

                // Push the combined result
                self.stack.push(self.trap.pass(), ty, s);

                // Free the operand strings
                self.stack.free_string(first_operand);
                self.stack.free_string(second_operand);
            }
        }
    }

    pub(crate) fn handle_arithmetic(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        op: &str,
        is_int: bool,
    ) {
        let (t2, s2) = if opargs.is_u32() {
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        } else {
            self.pop_resolved(vm, output)
        };
        let (t1, s1) = self.pop_resolved(vm, output);
        let mut s = self.stack.new_string();
        let is_hlsl_matrix_mul = if op == "*" && matches!(output.backend, ShaderBackend::Hlsl) {
            let is_matrix = |ty: &ShaderType| match ty {
                ShaderType::Pod(pod_ty) => matches!(
                    vm.bx.heap.pod_types[pod_ty.index as usize].ty,
                    crate::pod::ScriptPodTy::Mat(_)
                ),
                _ => false,
            };
            is_matrix(&t1) || is_matrix(&t2)
        } else {
            false
        };

        if is_hlsl_matrix_mul {
            write!(s, "mul({}, {})", s1, s2).ok();
        } else {
            write!(s, "({} {} {})", s1, op, s2).ok();
        }
        let ty = if is_int {
            type_table_int_arithmetic(&t1, &t2, self.trap.pass(), &vm.bx.code.builtins.pod)
        } else {
            type_table_float_arithmetic(&t1, &t2, self.trap.pass(), &vm.bx.code.builtins.pod)
        };
        self.stack.push(self.trap.pass(), ty, s);
    }

    pub(crate) fn handle_arithmetic_assign(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        op: &str,
        is_int: bool,
    ) {
        let (t2, s2) = if opargs.is_u32() {
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        } else {
            self.pop_resolved(vm, output)
        };
        let (id_ty, id_s) = self.stack.pop(self.trap.pass());
        if let ShaderType::Id(id) = id_ty {
            if let Some((var, shadow)) = self.shader_scope.find_var(id) {
                if !matches!(var, ShaderScopeItem::Var { .. }) {
                    script_err_immutable!(
                        self.trap,
                        "shader: cannot assign to let-bound variable {:?}",
                        id
                    );
                }
                let t1 = ShaderType::Pod(var.ty());
                let _ty = if is_int {
                    type_table_int_arithmetic(&t1, &t2, self.trap.pass(), &vm.bx.code.builtins.pod)
                } else {
                    type_table_float_arithmetic(
                        &t1,
                        &t2,
                        self.trap.pass(),
                        &vm.bx.code.builtins.pod,
                    )
                };

                let mut s = self.stack.new_string();
                if shadow > 0 {
                    write!(s, "_s{}{}", shadow, id).ok();
                } else {
                    write!(s, "{}", id).ok();
                }
                write!(s, " {} {}", op, s2).ok();
                self.stack.push(
                    self.trap.pass(),
                    ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                    s,
                );
            } else {
                script_err_not_found!(self.trap, "shader: variable {} not found in scope", id);
                self.stack.push(
                    self.trap.pass(),
                    ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                    String::new(),
                );
            }
        } else {
            script_err_immutable!(
                self.trap,
                "shader: compound assign target must be identifier, got {:?}",
                id_ty
            );
            self.stack.push(
                self.trap.pass(),
                ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                String::new(),
            );
        }
        self.stack.free_string(s2);
        self.stack.free_string(id_s);
    }

    pub(crate) fn handle_arithmetic_field_assign(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        op: &str,
        is_int: bool,
    ) {
        let (t2, s2) = if opargs.is_u32() {
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        } else {
            self.pop_resolved(vm, output)
        };

        let (field_ty, field_s) = self.stack.pop(self.trap.pass());
        let (instance_ty, instance_s) = self.pop_resolved(vm, output);

        if let ShaderType::Id(field_id) = field_ty {
            if let ShaderType::Pod(pod_ty) = instance_ty {
                if let Some(ret_ty) =
                    vm.bx
                        .heap
                        .pod_field_type(pod_ty, field_id, &vm.bx.code.builtins.pod)
                {
                    let t1 = ShaderType::Pod(ret_ty);
                    let op_res_ty = if is_int {
                        type_table_int_arithmetic(
                            &t1,
                            &t2,
                            self.trap.pass(),
                            &vm.bx.code.builtins.pod,
                        )
                    } else {
                        type_table_float_arithmetic(
                            &t1,
                            &t2,
                            self.trap.pass(),
                            &vm.bx.code.builtins.pod,
                        )
                    };

                    let val_ty = op_res_ty
                        .make_concrete(&vm.bx.code.builtins.pod)
                        .unwrap_or(vm.bx.code.builtins.pod.pod_void);
                    if val_ty != ret_ty {
                        script_err_pod!(
                            self.trap,
                            "shader: field {:?} compound assign type mismatch: expected {}, got {}",
                            field_id,
                            format_pod_type_name(&vm.bx.heap, ret_ty),
                            format_pod_type_name(&vm.bx.heap, val_ty)
                        );
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{0}.{1} {2} {3}", instance_s, field_id, op, s2).ok();
                    self.stack.push(
                        self.trap.pass(),
                        ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                        s,
                    );
                } else {
                    script_err_not_found!(
                        self.trap,
                        "shader: field {:?} not found in pod type{}",
                        field_id,
                        suggest_pod_field(&vm.bx.heap, pod_ty, field_id)
                    );
                    self.stack.push(
                        self.trap.pass(),
                        ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                        String::new(),
                    );
                }
            } else if let ShaderType::PodPtr(pod_ty) = instance_ty {
                // Pointer type (e.g., uniform buffer in Metal) - use -> for field access
                if let Some(ret_ty) =
                    vm.bx
                        .heap
                        .pod_field_type(pod_ty, field_id, &vm.bx.code.builtins.pod)
                {
                    let t1 = ShaderType::Pod(ret_ty);
                    let op_res_ty = if is_int {
                        type_table_int_arithmetic(
                            &t1,
                            &t2,
                            self.trap.pass(),
                            &vm.bx.code.builtins.pod,
                        )
                    } else {
                        type_table_float_arithmetic(
                            &t1,
                            &t2,
                            self.trap.pass(),
                            &vm.bx.code.builtins.pod,
                        )
                    };

                    let val_ty = op_res_ty
                        .make_concrete(&vm.bx.code.builtins.pod)
                        .unwrap_or(vm.bx.code.builtins.pod.pod_void);
                    if val_ty != ret_ty {
                        script_err_pod!(self.trap, "shader: ptr field {:?} compound assign type mismatch: expected {}, got {}", field_id, format_pod_type_name(&vm.bx.heap, ret_ty), format_pod_type_name(&vm.bx.heap, val_ty));
                    }

                    let mut s = self.stack.new_string();
                    write!(s, "{0}->{1} {2} {3}", instance_s, field_id, op, s2).ok();
                    self.stack.push(
                        self.trap.pass(),
                        ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                        s,
                    );
                } else {
                    script_err_not_found!(
                        self.trap,
                        "shader: ptr field {:?} not found in pod type{}",
                        field_id,
                        suggest_pod_field(&vm.bx.heap, pod_ty, field_id)
                    );
                    self.stack.push(
                        self.trap.pass(),
                        ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                        String::new(),
                    );
                }
            } else {
                script_err_shader!(
                    self.trap,
                    "shader: cannot do field compound assign on type {:?}",
                    instance_ty
                );
                self.stack.push(
                    self.trap.pass(),
                    ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                    String::new(),
                );
            }
        } else {
            script_err_unexpected!(
                self.trap,
                "shader: field compound assign requires identifier, got {:?}",
                field_ty
            );
            self.stack.push(
                self.trap.pass(),
                ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                String::new(),
            );
        }
        self.stack.free_string(s2);
        self.stack.free_string(field_s);
        self.stack.free_string(instance_s);
    }

    pub(crate) fn handle_arithmetic_index_assign(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        opargs: OpcodeArgs,
        op: &str,
        is_int: bool,
    ) {
        let (t2, s2) = if opargs.is_u32() {
            let mut s = self.stack.new_string();
            write!(s, "{}", opargs.to_u32()).ok();
            (ShaderType::AbstractInt, s)
        } else {
            self.pop_resolved(vm, output)
        };

        let (index_ty, index_s) = self.pop_resolved(vm, output);
        let (instance_ty, instance_s) = self.pop_resolved(vm, output);

        if let ShaderType::Pod(pod_ty) = instance_ty {
            let builtins = &vm.bx.code.builtins.pod;
            let elem_ty = type_table_elem_type(
                &vm.bx.heap.pod_types[pod_ty.index as usize].ty,
                self.trap.pass(),
                builtins,
            );

            if let Some(ret_ty) = elem_ty {
                match index_ty {
                    ShaderType::AbstractInt => {}
                    ShaderType::Pod(t) if t == builtins.pod_i32 || t == builtins.pod_u32 => {}
                    _ => {
                        let got_type = match index_ty {
                            ShaderType::Pod(t) => format_pod_type_name(&vm.bx.heap, t),
                            _ => format!("{:?}", index_ty),
                        };
                        script_err_pod!(
                            self.trap,
                            "shader: index must be integer, got {}",
                            got_type
                        );
                    }
                }

                let t1 = ShaderType::Pod(ret_ty);
                let op_res_ty = if is_int {
                    type_table_int_arithmetic(&t1, &t2, self.trap.pass(), builtins)
                } else {
                    type_table_float_arithmetic(&t1, &t2, self.trap.pass(), builtins)
                };

                let val_ty = op_res_ty
                    .make_concrete(builtins)
                    .unwrap_or(builtins.pod_void);
                if val_ty != ret_ty {
                    script_err_pod!(
                        self.trap,
                        "shader: index compound assign type mismatch: expected {}, got {}",
                        format_pod_type_name(&vm.bx.heap, ret_ty),
                        format_pod_type_name(&vm.bx.heap, val_ty)
                    );
                }

                let mut s = self.stack.new_string();
                write!(s, "{}[{}] {} {}", instance_s, index_s, op, s2).ok();
                self.stack
                    .push(self.trap.pass(), ShaderType::Pod(builtins.pod_void), s);
            } else {
                script_err_immutable!(
                    self.trap,
                    "shader: type is not indexable for compound assign"
                );
                self.stack.push(
                    self.trap.pass(),
                    ShaderType::Pod(builtins.pod_void),
                    String::new(),
                );
            }
        } else {
            script_err_shader!(
                self.trap,
                "shader: cannot do index compound assign on type {:?}",
                instance_ty
            );
            self.stack.push(
                self.trap.pass(),
                ShaderType::Pod(vm.bx.code.builtins.pod.pod_void),
                String::new(),
            );
        }
        self.stack.free_string(s2);
        self.stack.free_string(index_s);
        self.stack.free_string(instance_s);
    }
}
