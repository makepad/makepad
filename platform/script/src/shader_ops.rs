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

/// GLSL ES has no implicit int -> uint or int -> float conversion (WGSL
/// concretizes literals but not typed ints): an integer constant next to, or
/// assigned to, a uint or float operand is written `5u` / `5.0` (or wrapped in
/// a conversion when it is an expression).
pub(crate) fn unsigned_literal(
    backend: &ShaderBackend,
    lit_ty: &ShaderType,
    lit: &str,
    other: &ShaderType,
    builtins: &crate::mod_pod::ScriptPodBuiltins,
) -> String {
    if !matches!(backend, ShaderBackend::Glsl | ShaderBackend::Wgsl)
        || !matches!(lit_ty, ShaderType::AbstractInt)
        || lit.is_empty()
    {
        return lit.to_string();
    }
    let is_uint = |x: &ScriptPodType| {
        *x == builtins.pod_u32
            || *x == builtins.pod_vec2u
            || *x == builtins.pod_vec3u
            || *x == builtins.pod_vec4u
    };
    let is_float = |x: &ScriptPodType| {
        *x == builtins.pod_f32
            || *x == builtins.pod_f16
            || *x == builtins.pod_vec2f
            || *x == builtins.pod_vec3f
            || *x == builtins.pod_vec4f
            || *x == builtins.pod_vec2h
            || *x == builtins.pod_vec3h
            || *x == builtins.pod_vec4h
    };
    let digits = lit.strip_prefix('-').unwrap_or(lit);
    let simple = !digits.is_empty() && digits.chars().all(|c| c.is_ascii_digit());
    let wgsl = matches!(backend, ShaderBackend::Wgsl);
    match other {
        ShaderType::Pod(pt) if is_uint(pt) && !lit.starts_with('-') => {
            if simple {
                format!("{}u", lit)
            } else {
                format!("{}({})", if wgsl { "u32" } else { "uint" }, lit)
            }
        }
        ShaderType::Pod(pt) if is_float(pt) => {
            if simple {
                format!("{}.0", lit)
            } else {
                format!("{}({})", if wgsl { "f32" } else { "float" }, lit)
            }
        }
        ShaderType::AbstractFloat => {
            if simple {
                format!("{}.0", lit)
            } else {
                format!("{}({})", if wgsl { "f32" } else { "float" }, lit)
            }
        }
        _ => lit.to_string(),
    }
}

/// The type an `unsigned_literal` result has, given the operand it was
/// converted against.
pub(crate) fn int_literal_type(
    lit_ty: &ShaderType,
    before: &str,
    after: &str,
    builtins: &crate::mod_pod::ScriptPodBuiltins,
) -> ShaderType {
    if matches!(lit_ty, ShaderType::AbstractInt) && before != after {
        if after.ends_with('u') || after.starts_with("uint(") || after.starts_with("u32(") {
            ShaderType::Pod(builtins.pod_u32)
        } else {
            ShaderType::AbstractFloat
        }
    } else {
        lit_ty.clone()
    }
}

impl ShaderFnCompiler {
    pub(crate) fn handle_not(
        &mut self,
        vm: &mut ScriptVm,
        output: &mut ShaderOutput,
        _opargs: OpcodeArgs,
    ) {
        let (t1, s1) = self.pop_resolved(vm, output);
        let concrete = t1.make_concrete(&vm.bx.code.builtins.pod);
        let mut s = self.stack.new_string();
        let ty = match concrete {
            Some(pod_ty) if pod_ty == vm.bx.code.builtins.pod.pod_bool => {
                write!(s, "(!{})", s1).ok();
                ShaderType::Pod(pod_ty)
            }
            Some(pod_ty)
                if pod_ty == vm.bx.code.builtins.pod.pod_i32
                    || pod_ty == vm.bx.code.builtins.pod.pod_u32 =>
            {
                write!(s, "(~{})", s1).ok();
                ShaderType::Pod(pod_ty)
            }
            Some(pod_ty) => {
                script_err_shader!(
                    self.trap,
                    "`!` in shaders only supports bool, i32, or u32; got {}",
                    format_pod_type_name(&vm.bx.heap, pod_ty)
                );
                ShaderType::Error(NIL)
            }
            None => {
                script_err_shader!(self.trap, "`!` in shaders requires a concrete type");
                ShaderType::Error(NIL)
            }
        };
        self.stack.push(self.trap.pass(), ty, s);
    }

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
            self.packed_operand(vm, output, opargs)
        } else {
            self.pop_resolved(vm, output)
        };
        let (t1, s1) = self.pop_resolved(vm, output);
        let mut s = self.stack.new_string();

        let is_simple_int_literal = |value: &str| {
            !value.is_empty()
                && value
                    .chars()
                    .all(|c| c.is_ascii_digit() || c == '-' || c == '+')
        };
        let is_simple_uint_literal =
            |value: &str| value.ends_with('u') && is_simple_int_literal(&value[..value.len() - 1]);
        let to_float_literal = |value: &str| {
            if is_simple_int_literal(value) {
                Some(format!("{}.0", value))
            } else if is_simple_uint_literal(value) {
                Some(format!("{}.0", &value[..value.len() - 1]))
            } else {
                None
            }
        };
        let is_float_like = |ty: &ShaderType| match ty {
            ShaderType::Pod(pt) => vm.bx.heap.pod_types[pt.index as usize].ty.is_float_type(),
            ShaderType::AbstractFloat => true,
            _ => false,
        };
        let is_int_like = |ty: &ShaderType| match ty {
            ShaderType::AbstractInt => true,
            ShaderType::Pod(pt)
                if *pt == vm.bx.code.builtins.pod.pod_i32
                    || *pt == vm.bx.code.builtins.pod.pod_u32 =>
            {
                true
            }
            _ => false,
        };

        let pods = &vm.bx.code.builtins.pod;
        let n1 = unsigned_literal(&output.backend, &t1, &s1, &t2, pods);
        let n2 = unsigned_literal(&output.backend, &t2, &s2, &t1, pods);
        let (t1, t2) = (
            int_literal_type(&t1, &s1, &n1, pods),
            int_literal_type(&t2, &s2, &n2, pods),
        );
        let (s1, s2) = (n1, n2);
        let lhs = if matches!(output.backend, ShaderBackend::Glsl | ShaderBackend::Rust)
            && is_int_like(&t1)
            && is_float_like(&t2)
        {
            if let Some(v) = to_float_literal(&s1) {
                v
            } else {
                format!("({} as f32)", s1)
            }
        } else {
            s1.to_string()
        };
        let rhs = if matches!(output.backend, ShaderBackend::Glsl | ShaderBackend::Rust)
            && is_int_like(&t2)
            && is_float_like(&t1)
        {
            if let Some(v) = to_float_literal(&s2) {
                v
            } else {
                format!("({} as f32)", s2)
            }
        } else {
            s2.to_string()
        };

        write!(s, "({} {} {})", lhs, op, rhs).ok();
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
            let ready_logic_index =
                self.mes
                    .iter()
                    .enumerate()
                    .rev()
                    .find_map(|(index, me)| match me {
                        ShaderMe::FnBody { .. }
                        | ShaderMe::ForLoop { .. }
                        | ShaderMe::LoopBody { .. }
                        | ShaderMe::IfBody { .. } => None,
                        ShaderMe::LogicOp { target_ip, .. } if self.trap.ip.index >= *target_ip => {
                            Some(index)
                        }
                        ShaderMe::LogicOp { .. }
                        | ShaderMe::BuiltinCall { .. }
                        | ShaderMe::PodBuiltinMethod { .. }
                        | ShaderMe::ScriptCall { .. }
                        | ShaderMe::Pod { .. }
                        | ShaderMe::ArrayConstruct { .. }
                        | ShaderMe::TextureBuiltin { .. } => Some(usize::MAX),
                    });

            let Some(index) = ready_logic_index else {
                break;
            };
            if index == usize::MAX {
                break;
            }

            // Pop the LogicOp and combine with the second operand on the stack
            if let ShaderMe::LogicOp {
                op,
                first_operand,
                first_type,
                ..
            } = self.mes.remove(index)
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
            self.packed_operand(vm, output, opargs)
        } else {
            self.pop_resolved(vm, output)
        };
        let (t1, s1) = self.pop_resolved(vm, output);
        let mut s = self.stack.new_string();

        let is_simple_int_literal = |value: &str| {
            value
                .chars()
                .all(|c| c.is_ascii_digit() || c == '-' || c == '+')
        };
        let is_float_like = |ty: &ShaderType| match ty {
            ShaderType::Pod(pt) => vm.bx.heap.pod_types[pt.index as usize].ty.is_float_type(),
            ShaderType::AbstractFloat => true,
            _ => false,
        };

        let pods = &vm.bx.code.builtins.pod;
        let n1 = unsigned_literal(&output.backend, &t1, &s1, &t2, pods);
        let n2 = unsigned_literal(&output.backend, &t2, &s2, &t1, pods);
        let (t1, t2) = (
            int_literal_type(&t1, &s1, &n1, pods),
            int_literal_type(&t2, &s2, &n2, pods),
        );
        let (s1, s2) = (n1, n2);
        let lhs = if matches!(output.backend, ShaderBackend::Glsl | ShaderBackend::Rust)
            && !is_int
            && matches!(t1, ShaderType::AbstractInt)
            && is_float_like(&t2)
            && is_simple_int_literal(&s1)
        {
            format!("{}.0", s1)
        } else {
            s1.to_string()
        };
        let rhs = if matches!(output.backend, ShaderBackend::Glsl | ShaderBackend::Rust)
            && !is_int
            && matches!(t2, ShaderType::AbstractInt)
            && is_float_like(&t1)
            && is_simple_int_literal(&s2)
        {
            format!("{}.0", s2)
        } else {
            s2.to_string()
        };

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
            write!(s, "mul({}, {})", lhs, rhs).ok();
        } else {
            write!(s, "({} {} {})", lhs, op, rhs).ok();
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
            self.packed_operand(vm, output, opargs)
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

                let s2 = unsigned_literal(&output.backend, &t2, &s2, &t1, &vm.bx.code.builtins.pod);
                let mut s = self.stack.new_string();
                let var_name = if matches!(var, ShaderScopeItem::Param { .. }) {
                    output.backend.map_param_name(id, shadow)
                } else {
                    output.backend.map_local_name(id, shadow)
                };
                write!(s, "{}", var_name).ok();
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
            self.packed_operand(vm, output, opargs)
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
                    let s2 = unsigned_literal(&output.backend, &t2, &s2, &t1, &vm.bx.code.builtins.pod);
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
                    let field_name = output.backend.map_field_name(field_id);
                    write!(s, "{0}.{1} {2} {3}", instance_s, field_name, op, s2).ok();
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
                    let s2 = unsigned_literal(&output.backend, &t2, &s2, &t1, &vm.bx.code.builtins.pod);
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
                    let field_name = output.backend.map_field_name(field_id);
                    write!(s, "{0}->{1} {2} {3}", instance_s, field_name, op, s2).ok();
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
            self.packed_operand(vm, output, opargs)
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

                let index_s2 = self.bounded_index_expr(vm, output, pod_ty, &index_ty, &index_s);
                let mut s = self.stack.new_string();
                write!(s, "{}[{}] {} {}", instance_s, index_s2, op, s2).ok();
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
