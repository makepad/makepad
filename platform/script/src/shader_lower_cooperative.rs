//! Subgroup matrix types and operations, available only in compute kernels.
//! `shader.coopmat(component, rows, cols, usage)` describes a type. Components
//! are f16/f32/i32/u32 or shader.i8/shader.u8; usage is shader.matrix_a,
//! shader.matrix_b or shader.matrix_accumulator. These are opaque locals.
//! All participating subgroup lanes must execute each matrix operation with
//! the same pointer, stride and layout. Load/store strides count elements of
//! the pointed-to array (including packed u32/vec4u elements), not bytes.
//! `coopmat_fill(Type, scalar)` constructs a local. Load takes
//! `(Type, array[offset], stride, layout)`; store takes the same arguments
//! with a matrix value in place of Type. `coopmat_mul_add(a, b, c)` returns
//! the accumulator, `coopmat_scale(matrix, scalar)` scales its components,
//! and `coopmat_convert(Type, matrix)` changes their scalar type.
use crate::shader::{ShaderMode, ShaderType};
use crate::shader_ir::*;
use crate::shader_lower::IrFnCompiler;
use crate::shader_output::ShaderOutput;
use crate::*;
use makepad_live_id::*;

impl IrFnCompiler {
    pub(crate) fn push_cooperative_type(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, desc: ScriptObject) {
        let result = (|| {
            if output.mode != ShaderMode::Compute {
                return Err("cooperative matrices exist only in compute kernels");
            }
            let component = vm.bx.heap.proto(desc);
            let p = &vm.bx.code.builtins.pod;
            let scalar = match component.as_pod_type().or_else(|| vm.bx.heap.pod_type(component)) {
                Some(t) if t == p.pod_f16 => CooperativeScalar::F16,
                Some(t) if t == p.pod_f32 => CooperativeScalar::F32,
                Some(t) if t == p.pod_i32 => CooperativeScalar::I32,
                Some(t) if t == p.pod_u32 => CooperativeScalar::U32,
                _ => match component.as_number() {
                    Some(0.0) => CooperativeScalar::I8,
                    Some(1.0) => CooperativeScalar::U8,
                    _ => return Err("matrix component must be f16, f32, i32, u32, shader.i8 or shader.u8"),
                },
            };
            let number = |name| vm.bx.heap.value(desc, name, NoTrap).as_number();
            let dim = |name| match number(name) {
                Some(n) if n >= 1.0 && n <= u32::MAX as f64 && n.fract() == 0.0 => Ok(n as u32),
                _ => Err("matrix dimensions must be positive constant u32 values"),
            };
            let usage = match number(id!(usage).into()) {
                Some(0.0) => CooperativeUse::A,
                Some(1.0) => CooperativeUse::B,
                Some(2.0) => CooperativeUse::Accumulator,
                _ => return Err("matrix use must be matrix_a, matrix_b or matrix_accumulator"),
            };
            Ok(CooperativeMatrix { scalar, rows: dim(id!(rows).into())?, cols: dim(id!(cols).into())?, usage })
        })();
        match result {
            Ok(matrix) => {
                let ty = output.compiled.ir.ty(IrTy::CooperativeMatrix(matrix));
                self.push(ShaderType::CooperativeMatrixType(ty), NO_EXPR);
            }
            Err(message) => {
                output.push_error(message.into());
                script_err_shader!(self.trap, "{}", message);
                self.push(ShaderType::Error(NIL), NO_EXPR);
            }
        }
    }

    pub(crate) fn lower_cooperative_call(&mut self, vm: &ScriptVm, output: &mut ShaderOutput, name: LiveId, args: &[(ShaderType, ExprId)]) -> Result<(ShaderType, ExprId), String> {
        if output.mode != ShaderMode::Compute {
            return Err("cooperative matrices exist only in compute kernels".into());
        }
        let count = match name {
            id!(coopmat_load) | id!(coopmat_store) => 4,
            id!(coopmat_mul_add) => 3,
            _ => 2,
        };
        if args.len() != count { return Err(format!("expected {count} arguments")); }
        let type_arg = matches!(name, id!(coopmat_fill) | id!(coopmat_load) | id!(coopmat_convert));
        let ty = match args[0].0 {
            ShaderType::CooperativeMatrixType(t) if type_arg => t,
            ShaderType::CooperativeMatrix(t) if !type_arg => t,
            _ => return Err(if type_arg { "first argument must be a shader.coopmat type" } else { "first argument must be a cooperative matrix" }.into()),
        };
        let IrTy::CooperativeMatrix(matrix) = *output.compiled.ir.get(ty) else { unreachable!() };
        let matrix_arg = |i: usize| -> Result<CooperativeMatrix, String> {
            if let ShaderType::CooperativeMatrix(t) = args[i].0 {
                if let IrTy::CooperativeMatrix(m) = *output.compiled.ir.get(t) { return Ok(m); }
            }
            Err(format!("argument {} must be a cooperative matrix", i + 1))
        };
        let mut result_ty = ty;
        let kind = match name {
            id!(coopmat_mul_add) => {
                let b = matrix_arg(1)?;
                let c = matrix_arg(2)?;
                if matrix.usage != CooperativeUse::A || b.usage != CooperativeUse::B || c.usage != CooperativeUse::Accumulator
                    || matrix.cols != b.rows || matrix.rows != c.rows || b.cols != c.cols
                    || matrix.scalar.is_float() != b.scalar.is_float() || matrix.scalar.is_float() != c.scalar.is_float() {
                    return Err("mul_add requires compatible A[M,K], B[K,N], accumulator[M,N] matrices".into());
                }
                result_ty = self.f.exprs[args[2].1 as usize].ty;
                ExprKind::Builtin(Builtin::CooperativeMulAdd, args.iter().map(|a| a.1).collect())
            }
            id!(coopmat_convert) => {
                let src = matrix_arg(1)?;
                if src.rows != matrix.rows || src.cols != matrix.cols || src.usage != matrix.usage {
                    return Err("convert preserves matrix dimensions and use".into());
                }
                ExprKind::Convert(args[1].1)
            }
            id!(coopmat_load) | id!(coopmat_store) => {
                let place = args[1].1;
                if place == NO_EXPR { return Err("memory operand must be a buffer or workgroup element".into()); }
                let ExprKind::Index(base, _) = self.f.exprs[place as usize].kind else {
                    return Err("memory operand must be a buffer or workgroup element".into());
                };
                let ExprKind::Global(g) = self.f.exprs[base as usize].kind else {
                    return Err("matrix memory must be a device buffer or workgroup array".into());
                };
                let storage = output.compiled.ir.globals[g as usize].kind;
                if !matches!(storage, IrGlobalKind::Io(IrIo::Storage(_) | IrIo::Workgroup)) {
                    return Err("matrix memory must be a device buffer or workgroup array".into());
                }
                if name == id!(coopmat_store) && storage == IrGlobalKind::Io(IrIo::Storage(false)) {
                    return Err("cannot store into a read-only buffer".into());
                }
                if !matches!(output.compiled.ir.get(self.f.exprs[place as usize].ty), IrTy::Scalar(s) | IrTy::Vec(_, s) if *s != IrScalar::Bool) {
                    return Err("matrix memory elements must be numeric scalars or vectors".into());
                }
                for i in 2..4 {
                    let e = args[i].1;
                    if e == NO_EXPR { return Err("stride and layout must be integers".into()); }
                    self.concretize(e, IrScalar::U32);
                    if !matches!(self.f.exprs[e as usize].ty, TY_U32 | TY_I32) { return Err("stride and layout must be integers".into()); }
                }
                if !matches!(self.f.exprs[args[3].1 as usize].kind, ExprKind::Lit(IrLit::U32(0 | 1) | IrLit::I32(0 | 1))) {
                    return Err("layout must be constant 0 (row major) or 1 (column major)".into());
                }
                if name == id!(coopmat_load) {
                    ExprKind::Builtin(Builtin::CooperativeLoad, vec![place, args[2].1, args[3].1])
                } else {
                    result_ty = TY_VOID;
                    ExprKind::Builtin(Builtin::CooperativeStore, vec![place, args[0].1, args[2].1, args[3].1])
                }
            }
            _ => {
                let e = args[1].1;
                if e == NO_EXPR { return Err("fill/scale requires a numeric scalar".into()); }
                self.concretize(e, if matrix.scalar.is_float() { IrScalar::F32 } else { IrScalar::I32 });
                if !matches!(output.compiled.ir.get(self.f.exprs[e as usize].ty), IrTy::Scalar(s) if *s != IrScalar::Bool) {
                    return Err("fill/scale requires a numeric scalar".into());
                }
                if name == id!(coopmat_fill) { ExprKind::Construct(vec![e]) }
                else { ExprKind::Builtin(Builtin::CooperativeScale, vec![args[0].1, e]) }
            }
        };
        let e = self.expr(kind, result_ty);
        Ok((if result_ty == TY_VOID { ShaderType::Pod(vm.bx.code.builtins.pod.pod_void) } else { ShaderType::CooperativeMatrix(result_ty) }, e))
    }
}
