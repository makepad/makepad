//! Lowering of arithmetic, comparison and logic operators. Literal
//! operands are folded or given their partner's type here, so the IR never
//! carries an abstract number past its use.

use crate::opcode::*;
use crate::shader::ShaderType;
use crate::shader_ir::*;
use crate::shader_lower::*;
use crate::shader_output::*;
use crate::shader_tables::*;
use crate::suggest::*;
use crate::value::*;
use crate::vm::*;
use crate::*;

fn is_uint_pod(b: &crate::mod_pod::ScriptPodBuiltins, t: ScriptPodType) -> bool {
    t == b.pod_u32 || t == b.pod_vec2u || t == b.pod_vec3u || t == b.pod_vec4u
}

fn is_float_pod(b: &crate::mod_pod::ScriptPodBuiltins, t: ScriptPodType) -> bool {
    t == b.pod_f32
        || t == b.pod_f16
        || t == b.pod_vec2f
        || t == b.pod_vec3f
        || t == b.pod_vec4f
        || t == b.pod_vec2h
        || t == b.pod_vec3h
        || t == b.pod_vec4h
}

/// The type an abstract int literal takes next to `other` (the text
/// compiler's `unsigned_literal` + `int_literal_type`: a uint partner makes
/// it a uint, a float partner a float).
pub(crate) fn literal_adapt(lit: &ShaderType, other: &ShaderType, b: &crate::mod_pod::ScriptPodBuiltins) -> ShaderType {
    if !matches!(lit, ShaderType::AbstractInt) {
        return lit.clone();
    }
    match other {
        ShaderType::Pod(t) if is_uint_pod(b, *t) => ShaderType::Pod(b.pod_u32),
        ShaderType::Pod(t) if is_float_pod(b, *t) => ShaderType::AbstractFloat,
        ShaderType::AbstractFloat => ShaderType::AbstractFloat,
        _ => lit.clone(),
    }
}

impl IrFnCompiler {
    /// A lifted literal (a table-constant read) meeting a half or integer
    /// operand is cast to that operand's element type, as the folded
    /// literal would have adapted.
    pub(crate) fn adapt_lifted(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, ty: ShaderType, e: ExprId, other: &ShaderType) -> (ShaderType, ExprId) {
        if output.table_consts.is_empty() || e == NO_EXPR {
            return (ty, e);
        }
        let ExprKind::Global(g) = self.f.exprs[e as usize].kind else {
            return (ty, e);
        };
        let name = output.ir.globals[g as usize].name;
        let mut lifted = false;
        for tc in &output.table_consts {
            if tc.shader_name == name {
                lifted = true;
            }
        }
        if !lifted {
            return (ty, e);
        }
        let p = &vm.bx.code.builtins.pod;
        let ShaderType::Pod(o) = other else {
            return (ty, e);
        };
        let o = *o;
        let colour = ty == ShaderType::Pod(p.pod_vec4f);
        let target = if o == p.pod_f16 || o == p.pod_vec2h || o == p.pod_vec3h {
            (!colour).then_some(p.pod_f16)
        } else if o == p.pod_vec4h {
            Some(if colour { p.pod_vec4h } else { p.pod_f16 })
        } else if o == p.pod_i32 || o == p.pod_vec2i || o == p.pod_vec3i || o == p.pod_vec4i {
            (!colour).then_some(p.pod_i32)
        } else if o == p.pod_u32 || o == p.pod_vec2u || o == p.pod_vec3u || o == p.pod_vec4u {
            (!colour).then_some(p.pod_u32)
        } else {
            None
        };
        match target {
            Some(pod) => {
                let t = Self::ir_ty(vm, output, pod);
                let c = self.expr(ExprKind::Convert(e), t);
                (ShaderType::Pod(pod), c)
            }
            None => (ty, e),
        }
    }

    fn lit_of(&self, e: ExprId) -> Option<IrLit> {
        if e == NO_EXPR {
            return None;
        }
        match self.f.exprs[e as usize].kind {
            ExprKind::Lit(l) => Some(l),
            _ => None,
        }
    }

    /// Folds an operator over two abstract literals.
    fn fold(&mut self, op: BinOp, a: IrLit, b: IrLit) -> Option<(ShaderType, ExprId)> {
        match (a, b) {
            (IrLit::AbstractInt(x), IrLit::AbstractInt(y)) => {
                let v = match op {
                    BinOp::Add => x.wrapping_add(y),
                    BinOp::Sub => x.wrapping_sub(y),
                    BinOp::Mul => x.wrapping_mul(y),
                    BinOp::Div => {
                        if y == 0 {
                            script_err_shader!(self.trap, "integer division by zero in a constant");
                            0
                        } else {
                            x / y
                        }
                    }
                    BinOp::Rem => {
                        if y == 0 {
                            script_err_shader!(self.trap, "integer division by zero in a constant");
                            0
                        } else {
                            x % y
                        }
                    }
                    BinOp::Shl => x.wrapping_shl(y as u32),
                    BinOp::Shr => x.wrapping_shr(y as u32),
                    BinOp::BitAnd => x & y,
                    BinOp::BitOr => x | y,
                    BinOp::BitXor => x ^ y,
                    _ => {
                        let c = match op {
                            BinOp::Eq => x == y,
                            BinOp::Ne => x != y,
                            BinOp::Lt => x < y,
                            BinOp::Le => x <= y,
                            BinOp::Gt => x > y,
                            BinOp::Ge => x >= y,
                            _ => return None,
                        };
                        let e = self.expr(ExprKind::Lit(IrLit::Bool(c)), TY_BOOL);
                        return Some((ShaderType::None, e));
                    }
                };
                let e = self.expr(ExprKind::Lit(IrLit::AbstractInt(v)), TY_AINT);
                Some((ShaderType::AbstractInt, e))
            }
            (IrLit::AbstractInt(_) | IrLit::AbstractFloat(_), IrLit::AbstractInt(_) | IrLit::AbstractFloat(_)) => {
                let x = match a {
                    IrLit::AbstractInt(i) => i as f64,
                    IrLit::AbstractFloat(f) => f,
                    _ => 0.0,
                };
                let y = match b {
                    IrLit::AbstractInt(i) => i as f64,
                    IrLit::AbstractFloat(f) => f,
                    _ => 0.0,
                };
                let v = match op {
                    BinOp::Add => x + y,
                    BinOp::Sub => x - y,
                    BinOp::Mul => x * y,
                    BinOp::Div => x / y,
                    BinOp::Rem => x % y,
                    BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => {
                        let c = match op {
                            BinOp::Eq => x == y,
                            BinOp::Ne => x != y,
                            BinOp::Lt => x < y,
                            BinOp::Le => x <= y,
                            BinOp::Gt => x > y,
                            _ => x >= y,
                        };
                        let e = self.expr(ExprKind::Lit(IrLit::Bool(c)), TY_BOOL);
                        return Some((ShaderType::None, e));
                    }
                    _ => return None,
                };
                let e = self.expr(ExprKind::Lit(IrLit::AbstractFloat(v)), TY_AFLOAT);
                Some((ShaderType::AbstractFloat, e))
            }
            _ => None,
        }
    }

    /// Gives an abstract operand its partner's scalar.
    fn settle_operand(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, e: ExprId, other: &ShaderType, op: BinOp) {
        let et = if e == NO_EXPR { TY_VOID } else { self.f.exprs[e as usize].ty };
        if et != TY_AINT && et != TY_AFLOAT {
            return;
        }
        if matches!(op, BinOp::Shl | BinOp::Shr) {
            self.concretize(e, IrScalar::U32);
            return;
        }
        if let ShaderType::Pod(t) = other {
            let it = Self::ir_ty(vm, output, *t);
            if let Some(s) = output.ir.scalar_of(it) {
                self.concretize(e, s);
                return;
            }
        }
        self.concretize_default(e);
    }

    pub(crate) fn handle_not(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (t1, e1) = self.pop_resolved(vm, output);
        if let Some(IrLit::AbstractInt(v)) = self.lit_of(e1) {
            let e = self.expr(ExprKind::Lit(IrLit::AbstractInt(!v)), TY_AINT);
            return self.push(ShaderType::AbstractInt, e);
        }
        let pods = vm.bx.code.builtins.pod.clone();
        let concrete = t1.make_concrete(&pods);
        match concrete {
            Some(pod_ty) if pod_ty == pods.pod_bool => {
                let e = self.expr(ExprKind::Unary(UnOp::Not, e1), TY_BOOL);
                self.push(ShaderType::Pod(pod_ty), e);
            }
            Some(pod_ty) if pod_ty == pods.pod_i32 || pod_ty == pods.pod_u32 => {
                let t = Self::ir_ty(vm, output, pod_ty);
                let e = self.expr(ExprKind::Unary(UnOp::BitNot, e1), t);
                self.push(ShaderType::Pod(pod_ty), e);
            }
            Some(pod_ty) => {
                script_err_shader!(
                    self.trap,
                    "`!` in shaders only supports bool, i32, or u32; got {}",
                    format_pod_type_name(&vm.bx.heap, pod_ty)
                );
                self.push(ShaderType::Error(NIL), NO_EXPR);
            }
            None => {
                script_err_shader!(self.trap, "`!` in shaders requires a concrete type");
                self.push(ShaderType::Error(NIL), NO_EXPR);
            }
        }
    }

    pub(crate) fn handle_neg(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        let (t1, e1) = self.pop_resolved(vm, output);
        match self.lit_of(e1) {
            Some(IrLit::AbstractInt(v)) => {
                let e = self.expr(ExprKind::Lit(IrLit::AbstractInt(-v)), TY_AINT);
                return self.push(ShaderType::AbstractInt, e);
            }
            Some(IrLit::AbstractFloat(v)) => {
                let e = self.expr(ExprKind::Lit(IrLit::AbstractFloat(-v)), TY_AFLOAT);
                return self.push(ShaderType::AbstractFloat, e);
            }
            _ => {}
        }
        let ty = type_table_neg(&t1, self.trap.pass(), &vm.bx.code.builtins.pod);
        let it = match ty.make_concrete(&vm.bx.code.builtins.pod) {
            Some(p) => Self::ir_ty(vm, output, p),
            None => TY_VOID,
        };
        let e = self.expr(ExprKind::Unary(UnOp::Neg, e1), it);
        self.push(ty, e);
    }

    pub(crate) fn handle_eq(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, op: BinOp) {
        let (t2, e2) = if opargs.is_u32() {
            self.packed_operand(vm, output, opargs)
        } else {
            self.pop_resolved(vm, output)
        };
        let (t1, e1) = self.pop_resolved(vm, output);
        let (t1, e1) = self.adapt_lifted(vm, output, t1, e1, &t2);
        let (t2, e2) = self.adapt_lifted(vm, output, t2, e2, &t1);
        if let (Some(a), Some(b)) = (self.lit_of(e1), self.lit_of(e2)) {
            if let Some((_, e)) = self.fold(op, a, b) {
                return self.push(ShaderType::Pod(vm.bx.code.builtins.pod.pod_bool), e);
            }
        }
        let pods = vm.bx.code.builtins.pod.clone();
        let a1 = literal_adapt(&t1, &t2, &pods);
        let a2 = literal_adapt(&t2, &t1, &pods);
        self.settle_operand(vm, output, e1, &t2, op);
        self.settle_operand(vm, output, e2, &t1, op);
        let ty = type_table_eq(&a1, &a2, self.trap.pass(), &pods);
        let (e1, e2) = self.unify_int_float(vm, output, e1, e2);
        let it = match ty.make_concrete(&pods) {
            Some(p) => Self::ir_ty(vm, output, p),
            None => TY_BOOL,
        };
        let e = self.expr(ExprKind::Binary(op, e1, e2), it);
        self.push(ty, e);
    }

    /// An integer operand next to a float of the same shape is converted
    /// (GLSL ES and WGSL have no implicit int-to-float conversion).
    fn unify_int_float(&mut self, _vm: &ScriptVm, output: &mut ShaderOutput, e1: ExprId, e2: ExprId) -> (ExprId, ExprId) {
        if e1 == NO_EXPR || e2 == NO_EXPR {
            return (e1, e2);
        }
        let (t1, t2) = (self.f.exprs[e1 as usize].ty, self.f.exprs[e2 as usize].ty);
        let (s1, s2) = (output.ir.scalar_of(t1), output.ir.scalar_of(t2));
        let (l1, l2) = (output.ir.lanes(t1), output.ir.lanes(t2));
        match (s1, s2) {
            (Some(a), Some(b)) if a.is_int() && b.is_float() && l1 == l2 && l1 > 0 => {
                let c = self.expr(ExprKind::Convert(e1), t2);
                (c, e2)
            }
            (Some(a), Some(b)) if a.is_float() && b.is_int() && l1 == l2 && l1 > 0 => {
                let c = self.expr(ExprKind::Convert(e2), t1);
                (e1, c)
            }
            _ => (e1, e2),
        }
    }

    pub(crate) fn handle_logic_test(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, op: BinOp) {
        let (first_type, first) = self.pop_resolved(vm, output);
        let target_ip = self.trap.ip.index + opargs.to_u32();
        self.mes.push(IrMe::LogicOp { target_ip, op, first, first_type });
    }

    pub(crate) fn handle_logic_phi(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput) {
        loop {
            let mut ready = None;
            let mut i = self.mes.len();
            while i > 0 {
                i -= 1;
                match &self.mes[i] {
                    IrMe::FnBody { .. } | IrMe::ForLoop { .. } | IrMe::LoopBody { .. } | IrMe::IfBody { .. } => continue,
                    IrMe::LogicOp { target_ip, .. } if self.trap.ip.index >= *target_ip => {
                        ready = Some(i);
                        break;
                    }
                    _ => break,
                }
            }
            let Some(index) = ready else {
                break;
            };
            if let IrMe::LogicOp { op, first, first_type, .. } = self.mes.remove(index) {
                let (second_type, second) = self.pop_resolved(vm, output);
                let ty = type_table_logic(&first_type, &second_type, self.trap.pass(), &vm.bx.code.builtins.pod);
                let e = self.expr(ExprKind::Binary(op, first, second), TY_BOOL);
                self.push(ty, e);
            }
        }
    }

    pub(crate) fn handle_arithmetic(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, op: BinOp, is_int: bool) {
        let (t2, e2) = if opargs.is_u32() {
            self.packed_operand(vm, output, opargs)
        } else {
            self.pop_resolved(vm, output)
        };
        let (t1, e1) = self.pop_resolved(vm, output);
        let (t1, e1) = self.adapt_lifted(vm, output, t1, e1, &t2);
        let (t2, e2) = self.adapt_lifted(vm, output, t2, e2, &t1);
        if let (Some(a), Some(b)) = (self.lit_of(e1), self.lit_of(e2)) {
            if let Some((ty, e)) = self.fold(op, a, b) {
                return self.push(ty, e);
            }
        }
        let pods = vm.bx.code.builtins.pod.clone();
        let a1 = literal_adapt(&t1, &t2, &pods);
        let a2 = literal_adapt(&t2, &t1, &pods);
        let ty = if is_int {
            type_table_int_arithmetic(&a1, &a2, self.trap.pass(), &pods)
        } else {
            type_table_float_arithmetic(&a1, &a2, self.trap.pass(), &pods)
        };
        self.settle_operand(vm, output, e1, &t2, op);
        self.settle_operand(vm, output, e2, &t1, op);
        let it = match ty.make_concrete(&pods) {
            Some(p) => Self::ir_ty(vm, output, p),
            None => TY_VOID,
        };
        let e = self.expr(ExprKind::Binary(op, e1, e2), it);
        self.push(ty, e);
    }

    /// `place op= value`, checked as `place = place op value`.
    fn compound_assign(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, place: ExprId, target: ScriptPodType, t2: ShaderType, e2: ExprId, op: BinOp, is_int: bool, what: &str) {
        let t1 = ShaderType::Pod(target);
        let (t2, e2) = self.adapt_lifted(vm, output, t2, e2, &t1);
        let pods = vm.bx.code.builtins.pod.clone();
        let a2 = literal_adapt(&t2, &t1, &pods);
        let res = if is_int {
            type_table_int_arithmetic(&t1, &a2, self.trap.pass(), &pods)
        } else {
            type_table_float_arithmetic(&t1, &a2, self.trap.pass(), &pods)
        };
        let val_ty = res.make_concrete(&pods).unwrap_or(pods.pod_void);
        if val_ty != target {
            script_err_pod!(
                self.trap,
                "shader: {} compound assign type mismatch: expected {}, got {}",
                what,
                format_pod_type_name(&vm.bx.heap, target),
                format_pod_type_name(&vm.bx.heap, val_ty)
            );
        }
        self.settle_operand(vm, output, e2, &t1, op);
        self.emit(Stmt::CompoundAssign(place, op, e2));
        let n = self.nop();
        self.push(ShaderType::Pod(pods.pod_void), n);
    }

    pub(crate) fn handle_arithmetic_assign(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, op: BinOp, is_int: bool) {
        let (t2, e2) = if opargs.is_u32() {
            self.packed_operand(vm, output, opargs)
        } else {
            self.pop_resolved(vm, output)
        };
        let (id_ty, _) = self.pop();
        let pods = vm.bx.code.builtins.pod.clone();
        let ShaderType::Id(id) = id_ty else {
            script_err_immutable!(self.trap, "shader: compound assign target must be identifier, got {:?}", id_ty);
            let n = self.nop();
            return self.push(ShaderType::Pod(pods.pod_void), n);
        };
        let Some(var) = self.scope.find(id) else {
            script_err_not_found!(self.trap, "shader: variable {} not found in scope", id);
            let n = self.nop();
            return self.push(ShaderType::Pod(pods.pod_void), n);
        };
        if !matches!(var, IrVar::Local(_, _, true)) {
            script_err_immutable!(self.trap, "shader: cannot assign to let-bound variable {:?}", id);
        }
        let place = self.var_expr(vm, output, var);
        self.compound_assign(vm, output, place, var.pod(), t2, e2, op, is_int, "variable");
    }

    pub(crate) fn handle_arithmetic_field_assign(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, op: BinOp, is_int: bool) {
        let (t2, e2) = if opargs.is_u32() {
            self.packed_operand(vm, output, opargs)
        } else {
            self.pop_resolved(vm, output)
        };
        let (field_ty, _) = self.pop();
        let (instance_ty, instance) = self.pop_resolved(vm, output);
        let pods = vm.bx.code.builtins.pod.clone();
        let ShaderType::Id(field_id) = field_ty else {
            script_err_unexpected!(self.trap, "shader: field compound assign requires identifier, got {:?}", field_ty);
            let n = self.nop();
            return self.push(ShaderType::Pod(pods.pod_void), n);
        };
        let pod_ty = match instance_ty {
            ShaderType::Pod(p) | ShaderType::PodPtr(p) => p,
            _ => {
                script_err_shader!(self.trap, "shader: cannot do field compound assign on type {:?}", instance_ty);
                let n = self.nop();
                return self.push(ShaderType::Pod(pods.pod_void), n);
            }
        };
        match vm.bx.heap.pod_field_type(pod_ty, field_id, &pods) {
            Some(ret_ty) => {
                let place = self.field_expr(vm, output, instance, pod_ty, field_id, ret_ty);
                self.compound_assign(vm, output, place, ret_ty, t2, e2, op, is_int, "field");
            }
            None => {
                script_err_not_found!(
                    self.trap,
                    "shader: field {:?} not found in pod type{}",
                    field_id,
                    suggest_pod_field(&vm.bx.heap, pod_ty, field_id)
                );
                let n = self.nop();
                self.push(ShaderType::Pod(pods.pod_void), n);
            }
        }
    }

    pub(crate) fn handle_arithmetic_index_assign(&mut self, vm: &mut ScriptVm, output: &mut ShaderOutput, opargs: OpcodeArgs, op: BinOp, is_int: bool) {
        let (t2, e2) = if opargs.is_u32() {
            self.packed_operand(vm, output, opargs)
        } else {
            self.pop_resolved(vm, output)
        };
        let (index_ty, index) = self.pop_resolved(vm, output);
        let (instance_ty, instance) = self.pop_resolved(vm, output);
        let pods = vm.bx.code.builtins.pod.clone();
        let ShaderType::Pod(pod_ty) = instance_ty else {
            script_err_shader!(self.trap, "shader: cannot do index compound assign on type {:?}", instance_ty);
            let n = self.nop();
            return self.push(ShaderType::Pod(pods.pod_void), n);
        };
        let elem = type_table_elem_type(&vm.bx.heap.pod_types[pod_ty.index as usize].ty, self.trap.pass(), &pods);
        let Some(ret_ty) = elem else {
            script_err_immutable!(self.trap, "shader: type is not indexable for compound assign");
            let n = self.nop();
            return self.push(ShaderType::Pod(pods.pod_void), n);
        };
        self.check_index_type(vm, &index_ty);
        let place = self.index_expr(vm, output, instance, pod_ty, &index_ty, index, ret_ty);
        self.compound_assign(vm, output, place, ret_ty, t2, e2, op, is_int, "index");
    }
}
