use crate::ast::*;
use crate::builtin::Builtin;
use crate::env::{Env, Sym, VarKind};
use crate::ident::Ident;
use crate::lit::{Lit, TyLit};
use crate::swizzle::Swizzle;
use crate::ty::Ty;
use crate::util::CommaSep;
use std::cell::Cell;
use std::collections::HashMap;
use std::error::Error;
use std::fmt::Write;
use std::rc::Rc;

#[derive(Clone, Debug)]
pub struct TyChecker<'a> {
    pub builtins: &'a HashMap<Ident, Builtin>,
    pub shader: &'a Shader,
    pub env: &'a Env,
    pub is_lvalue: bool,
}

impl<'a> TyChecker<'a> {
    pub fn ty_check_ty_expr(&mut self, ty_expr: &TyExpr) -> Result<Ty, Box<dyn Error>> {
        let ty = match ty_expr.kind {
            TyExprKind::Array {
                ref elem_ty_expr,
                len,
            } => self.ty_check_array_ty_expr(elem_ty_expr, len),
            TyExprKind::Struct { ident } => self.ty_check_struct_ty_expr(ident),
            TyExprKind::Lit { ty_lit } => self.ty_check_lit_ty_expr(ty_lit),
        }?;
        *ty_expr.ty.borrow_mut() = Some(ty.clone());
        Ok(ty)
    }

    fn ty_check_array_ty_expr(
        &mut self,
        elem_ty_expr: &TyExpr,
        len: u32,
    ) -> Result<Ty, Box<dyn Error>> {
        let elem_ty = Rc::new(self.ty_check_ty_expr(elem_ty_expr)?);
        let len = len as usize;
        Ok(Ty::Array { elem_ty, len })
    }

    fn ty_check_struct_ty_expr(&mut self, ident: Ident) -> Result<Ty, Box<dyn Error>> {
        match self
            .env
            .find_sym(ident)
            .ok_or_else(|| format!("`{}` is not defined in this scope", ident))?
        {
            Sym::Struct => Ok(Ty::Struct { ident }),
            _ => Err(format!("`{}` is not a type variable", ident).into()),
        }
    }

    fn ty_check_lit_ty_expr(&mut self, ty_lit: TyLit) -> Result<Ty, Box<dyn Error>> {
        Ok(ty_lit.to_ty())
    }

    pub fn ty_check_expr_with_expected_ty(
        &mut self,
        expr: &Expr,
        expected_ty: &Ty,
    ) -> Result<Ty, Box<dyn Error>> {
        let actual_ty = self.ty_check_expr(expr)?;
        if &actual_ty != expected_ty {
            return Err(format!(
                "can't match expected type `{}` with actual type `{}",
                expected_ty, actual_ty
            )
            .into());
        }
        Ok(actual_ty)
    }

    pub fn ty_check_expr(&mut self, expr: &Expr) -> Result<Ty, Box<dyn Error>> {
        let ty = match expr.kind {
            ExprKind::Cond {
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
            } => self.ty_check_cond_expr(expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                op,
                ref left_expr,
                ref right_expr,
            } => self.ty_check_bin_expr(op, left_expr, right_expr),
            ExprKind::Un { op, ref expr } => self.ty_check_un_expr(op, expr),
            ExprKind::Field {
                ref expr,
                field_ident,
            } => self.ty_check_field_expr(expr, field_ident),
            ExprKind::Index {
                ref expr,
                ref index_expr,
            } => self.ty_check_index_expr(expr, index_expr),
            ExprKind::Call {
                ident,
                ref arg_exprs,
            } => self.ty_check_call_expr(ident, arg_exprs),
            ExprKind::ConsCall {
                ty_lit,
                ref arg_exprs,
            } => self.ty_check_cons_call_expr(ty_lit, arg_exprs),
            ExprKind::Var {
                ref is_lvalue,
                ref kind,
                ident,
            } => self.ty_check_var_expr(is_lvalue, kind, ident),
            ExprKind::Lit { lit } => self.ty_check_lit_expr(lit),
        }?;
        *expr.ty.borrow_mut() = Some(ty.clone());
        Ok(ty)
    }

    fn ty_check_cond_expr(
        &mut self,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr,
    ) -> Result<Ty, Box<dyn Error>> {
        if self.is_lvalue {
            return Err("invalid lvalue expression".into());
        }
        self.ty_check_expr_with_expected_ty(expr, &Ty::Bool)?;
        let ty_if_true = self.ty_check_expr(expr_if_true)?;
        self.ty_check_expr_with_expected_ty(expr_if_false, &ty_if_true)?;
        Ok(ty_if_true)
    }

    fn ty_check_bin_expr(
        &mut self,
        op: BinOp,
        left_expr: &Expr,
        right_expr: &Expr,
    ) -> Result<Ty, Box<dyn Error>> {
        if self.is_lvalue {
            return Err("invalid lvalue expression".into());
        }
        let was_lvalue = self.is_lvalue;
        match op {
            BinOp::Assign | BinOp::AddAssign | BinOp::SubAssign | BinOp::DivAssign => {
                self.is_lvalue = true
            }
            _ => {}
        }
        let left_ty = self.ty_check_expr(left_expr)?;
        self.is_lvalue = was_lvalue;
        let right_ty = self.ty_check_expr(right_expr)?;
        match op {
            BinOp::Assign => {
                if left_ty == right_ty {
                    Some(left_ty.clone())
                } else {
                    None
                }
            }
            BinOp::AddAssign | BinOp::SubAssign | BinOp::DivAssign => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
            BinOp::MulAssign => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Mat2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Mat3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Mat4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
            BinOp::Or | BinOp::And => match (&left_ty, &right_ty) {
                (Ty::Bool, Ty::Bool) => Some(Ty::Bool),
                _ => None,
            },
            BinOp::Eq | BinOp::Ne => match (&left_ty, &right_ty) {
                (Ty::Bool, Ty::Bool) => Some(Ty::Bool),
                (Ty::Int, Ty::Int) => Some(Ty::Bool),
                (Ty::Float, Ty::Float) => Some(Ty::Bool),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Bool),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Bool),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Bool),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Bool),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Bool),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Bool),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Bool),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Bool),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Bool),
                _ => None,
            },
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Bool),
                (Ty::Float, Ty::Float) => Some(Ty::Bool),
                _ => None,
            },
            BinOp::Add | BinOp::Sub | BinOp::Div => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Float, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Float, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Float, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Float, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Float, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Float, Ty::Mat4) => Some(Ty::Mat4),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
            BinOp::Mul => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Float, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Float, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Float, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Float, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Float, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Float, Ty::Mat4) => Some(Ty::Mat4),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Mat2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Mat3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Mat4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
        }
        .ok_or_else(|| {
            format!(
                "can't apply binary operator `{}` to operands of type `{}` and `{}",
                op, left_ty, right_ty
            )
            .into()
        })
    }

    fn ty_check_un_expr(&mut self, op: UnOp, expr: &Expr) -> Result<Ty, Box<dyn Error>> {
        if self.is_lvalue {
            return Err("invalid lvalue expression".into());
        }
        let ty = self.ty_check_expr(expr)?;
        match op {
            UnOp::Not => match ty {
                Ty::Bool => Some(Ty::Bool),
                _ => None,
            },
            UnOp::Neg => match ty {
                Ty::Int => Some(Ty::Int),
                Ty::Float => Some(Ty::Float),
                _ => None,
            },
        }
        .ok_or_else(|| {
            format!(
                "can't apply unary operator `{}` to operand of type `{}`",
                op, ty
            )
            .into()
        })
    }

    fn ty_check_field_expr(
        &mut self,
        expr: &Expr,
        field_ident: Ident,
    ) -> Result<Ty, Box<dyn Error>> {
        if self.is_lvalue {
            return Err("invalid lvalue expression".into());
        }
        let ty = self.ty_check_expr(expr)?;
        match ty {
            ref ty if ty.is_vector() => {
                let swizzle = Swizzle::parse(field_ident)
                    .filter(|swizzle| {
                        if swizzle.len() > 4 {
                            return false;
                        }
                        let size = ty.size();
                        for &index in swizzle {
                            if index > size {
                                return false;
                            }
                        }
                        true
                    })
                    .ok_or_else(|| {
                        format!("field `{}` is not defined on type `{}`", field_ident, ty)
                    })?;
                Ok(match ty {
                    Ty::Bvec2 | Ty::Bvec3 | Ty::Bvec4 => match swizzle.len() {
                        1 => Ty::Bool,
                        2 => Ty::Bvec2,
                        3 => Ty::Bvec3,
                        4 => Ty::Bvec4,
                        _ => panic!(),
                    },
                    Ty::Ivec2 | Ty::Ivec3 | Ty::Ivec4 => match swizzle.len() {
                        1 => Ty::Int,
                        2 => Ty::Ivec2,
                        3 => Ty::Ivec3,
                        4 => Ty::Ivec4,
                        _ => panic!(),
                    },
                    Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => match swizzle.len() {
                        1 => Ty::Float,
                        2 => Ty::Vec2,
                        3 => Ty::Vec3,
                        4 => Ty::Vec4,
                        _ => panic!(),
                    },
                    _ => panic!(),
                })
            }
            Ty::Struct { ident } => Ok(self
                .shader
                .find_struct_decl(ident)
                .unwrap()
                .find_field(field_ident)
                .ok_or(format!(
                    "field `{}` is not defined on type `{}`",
                    field_ident, ident
                ))?
                .ty_expr
                .ty
                .borrow()
                .as_ref()
                .unwrap()
                .clone()),
            _ => Err(format!("can't access field on value of type `{}`", ty).into()),
        }
    }

    fn ty_check_index_expr(
        &mut self,
        expr: &Expr,
        index_expr: &Expr,
    ) -> Result<Ty, Box<dyn Error>> {
        if self.is_lvalue {
            return Err("invalid lvalue expression".into());
        }
        let ty = self.ty_check_expr(expr)?;
        let was_lvalue = self.is_lvalue;
        self.is_lvalue = false;
        let index_ty = self.ty_check_expr(index_expr)?;
        self.is_lvalue = was_lvalue;
        let elem_ty = match ty {
            Ty::Bvec2 | Ty::Bvec3 | Ty::Bvec4 => Ty::Bool,
            Ty::Ivec2 | Ty::Ivec3 | Ty::Ivec4 => Ty::Int,
            Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => Ty::Float,
            Ty::Mat2 => Ty::Vec2,
            Ty::Mat3 => Ty::Vec3,
            Ty::Mat4 => Ty::Vec4,
            _ => return Err(format!("can't index into value of type `{}`", ty).into()),
        };
        if index_ty != Ty::Int {
            return Err("index is not an integer".into());
        }
        Ok(elem_ty)
    }

    fn ty_check_call_expr(
        &mut self,
        ident: Ident,
        arg_exprs: &[Expr],
    ) -> Result<Ty, Box<dyn Error>> {
        if self.is_lvalue {
            return Err("invalid lvalue expression".into());
        }
        for arg_expr in arg_exprs {
            self.ty_check_expr(arg_expr)?;
        }
        match self
            .env
            .find_sym(ident)
            .ok_or_else(|| format!("`{}` is not defined", ident))?
        {
            Sym::Builtin => {
                let builtin = self.builtins.get(&ident).unwrap();
                let arg_tys = arg_exprs
                    .iter()
                    .map(|arg_expr| arg_expr.ty.borrow().as_ref().unwrap().clone())
                    .collect::<Vec<_>>();
                Ok(builtin
                    .return_tys
                    .get(&arg_tys)
                    .ok_or({
                        let mut string = String::new();
                        write!(
                            string,
                            "can't apply builtin `{}` to arguments of types ",
                            ident
                        )
                        .unwrap();
                        let mut sep = "";
                        for arg_ty in arg_tys {
                            write!(string, "{}{}", sep, arg_ty).unwrap();
                            sep = ", ";
                        }
                        string
                    })?
                    .clone())
            }
            Sym::Fn => {
                let fn_decl = self.shader.find_fn_decl(ident).unwrap();
                if arg_exprs.len() < fn_decl.params.len() {
                    return Err(format!(
                        "not enough arguments for call to function `{}`: expected {}, got {}",
                        ident,
                        fn_decl.params.len(),
                        arg_exprs.len(),
                    )
                    .into());
                }
                if arg_exprs.len() < fn_decl.params.len() {
                    return Err(format!(
                        "too many arguments for call to function `{}`: expected {}, got {}",
                        ident,
                        fn_decl.params.len(),
                        arg_exprs.len()
                    )
                    .into());
                }
                for (index, (arg_expr, param)) in
                    arg_exprs.iter().zip(fn_decl.params.iter()).enumerate()
                {
                    let arg_ty = arg_expr.ty.borrow();
                    let arg_ty = arg_ty.as_ref().unwrap();
                    let param_ty = param.ty_expr.ty.borrow();
                    let param_ty = param_ty.as_ref().unwrap();
                    if arg_ty != param_ty {
                        return Err(format!(
                            "wrong type for argument {} in call to function `{}`: expected `{}`, got `{}`",
                            index + 1,
                            ident,
                            param_ty,
                            arg_ty,
                        ).into());
                    }
                }
                Ok(fn_decl.return_ty.borrow().as_ref().unwrap().clone())
            }
            _ => Err(format!("`{}` is not a function", ident).into()),
        }
    }

    #[allow(clippy::redundant_closure_call)]
    fn ty_check_cons_call_expr(
        &mut self,
        ty_lit: TyLit,
        arg_exprs: &[Expr],
    ) -> Result<Ty, Box<dyn Error>> {
        if self.is_lvalue {
            return Err("invalid lvalue expression".into());
        }
        let ty = ty_lit.to_ty();
        let arg_tys = arg_exprs
            .iter()
            .map(|arg_expr| self.ty_check_expr(arg_expr))
            .collect::<Result<Vec<_>, _>>()?;
        match (&ty, arg_tys.as_slice()) {
            (ty, [arg_ty]) if ty.is_scalar() && arg_ty.is_scalar() => Ok(ty.clone()),
            (ty, [arg_ty]) if ty.is_vector() && arg_ty.is_scalar() => Ok(ty.clone()),
            (ty, arg_tys)
                if ty.is_vector()
                    && (|| {
                        arg_tys.iter().all(|arg_ty| {
                            arg_ty.is_scalar() || arg_ty.is_vector() || arg_ty.is_matrix()
                        })
                    })() =>
            {
                let expected_size = ty.size();
                let actual_size = arg_tys.iter().map(|arg_ty| arg_ty.size()).sum::<usize>();
                if actual_size < expected_size {
                    return Err(format!(
                        "not enough components for call to constructor `{}`: expected {}, got {}",
                        ty_lit, actual_size, expected_size,
                    )
                    .into());
                }
                if actual_size > expected_size {
                    return Err(format!(
                        "too many components for call to constructor `{}`: expected {}, got {}",
                        ty_lit, actual_size, expected_size,
                    )
                    .into());
                }
                Ok(ty.clone())
            }
            (ty, [arg_ty]) if ty.is_matrix() && arg_ty.is_scalar() || arg_ty.is_matrix() => {
                Ok(ty.clone())
            }
            _ => Err(format!(
                "can't construct value of type `{}` with arguments of types `{}`",
                ty,
                CommaSep(&arg_tys)
            )
            .into()),
        }
    }

    fn ty_check_var_expr(
        &mut self,
        is_lvalue: &Cell<Option<bool>>,
        kind: &Cell<Option<VarKind>>,
        ident: Ident,
    ) -> Result<Ty, Box<dyn Error>> {
        match *self
            .env
            .find_sym(ident)
            .ok_or_else(|| format!("`{}` is not defined in this scope", ident))?
        {
            Sym::Var {
                is_mut,
                ref ty,
                kind: new_kind,
            } => {
                if self.is_lvalue && !is_mut {
                    return Err(format!("can't assign to variable `{}`", ident).into());
                }
                is_lvalue.set(Some(self.is_lvalue));
                kind.set(Some(new_kind));
                Ok(ty.clone())
            }
            _ => Err(format!("`{}` is not a variable", ident).into()),
        }
    }

    fn ty_check_lit_expr(&mut self, lit: Lit) -> Result<Ty, Box<dyn Error>> {
        Ok(lit.to_ty())
    }
}
