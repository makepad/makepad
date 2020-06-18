use crate::ast::*;
use crate::env::VarKind;
use crate::ident::Ident;
use crate::lit::{Lit, TyLit};
use crate::val::Val;
use std::cell::Cell;
use std::error::Error;

#[derive(Clone, Debug)]
pub struct ConstEvaluator<'a> {
    pub shader: &'a Shader,
}

impl<'a> ConstEvaluator<'a> {
    pub fn const_eval_expr(&self, expr: &Expr) -> Result<Val, Box<dyn Error>> {
        let val = match expr.kind {
            ExprKind::Cond {
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
            } => self.const_eval_cond_expr(expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                op,
                ref left_expr,
                ref right_expr,
            } => self.const_eval_bin_expr(op, left_expr, right_expr),
            ExprKind::Un { op, ref expr } => self.const_eval_un_expr(op, expr),
            ExprKind::Field {
                ref expr,
                field_ident,
            } => self.const_eval_field_expr(expr, field_ident),
            ExprKind::Index {
                ref expr,
                ref index_expr,
            } => self.const_eval_index_expr(expr, index_expr),
            ExprKind::Call {
                ident,
                ref arg_exprs,
            } => self.const_eval_call_expr(ident, arg_exprs),
            ExprKind::ConsCall {
                ty_lit,
                ref arg_exprs,
            } => self.const_eval_cons_call_expr(ty_lit, arg_exprs),
            ExprKind::Var {
                ref is_lvalue,
                ref kind,
                ident,
            } => self.const_eval_var_expr(is_lvalue, kind, ident),
            ExprKind::Lit { lit } => self.const_eval_lit_expr(lit),
        }?;
        *expr.val.borrow_mut() = Some(val.clone());
        Ok(val)
    }

    fn const_eval_cond_expr(
        &self,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr,
    ) -> Result<Val, Box<dyn Error>> {
        let val = self.const_eval_expr(expr)?;
        let val_if_true = self.const_eval_expr(expr_if_true)?;
        let val_if_false = self.const_eval_expr(expr_if_false)?;
        Ok(if val.to_bool().unwrap() {
            val_if_true
        } else {
            val_if_false
        })
    }

    #[allow(clippy::float_cmp)]
    fn const_eval_bin_expr(
        &self,
        op: BinOp,
        left_expr: &Expr,
        right_expr: &Expr,
    ) -> Result<Val, Box<dyn Error>> {
        let left_val = self.const_eval_expr(left_expr)?;
        let right_val = self.const_eval_expr(right_expr)?;
        match op {
            BinOp::Or => match (&left_val, &right_val) {
                (Val::Bool(x), Val::Bool(y)) => Some(Val::Bool(*x || *y)),
                _ => None,
            },
            BinOp::And => match (&left_val, &right_val) {
                (Val::Bool(x), Val::Bool(y)) => Some(Val::Bool(*x && *y)),
                _ => None,
            },
            BinOp::Eq => match (&left_val, &right_val) {
                (Val::Bool(x), Val::Bool(y)) => Some(Val::Bool(x == y)),
                (Val::Int(x), Val::Int(y)) => Some(Val::Bool(x == y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Bool(x == y)),
                _ => None,
            },
            BinOp::Ne => match (&left_val, &right_val) {
                (Val::Bool(x), Val::Bool(y)) => Some(Val::Bool(x != y)),
                (Val::Int(x), Val::Int(y)) => Some(Val::Bool(x != y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Bool(x != y)),
                _ => None,
            },
            BinOp::Lt => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Bool(x < y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Bool(x < y)),
                _ => None,
            },
            BinOp::Le => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Bool(x <= y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Bool(x <= y)),
                _ => None,
            },
            BinOp::Gt => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Bool(x > y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Bool(x > y)),
                _ => None,
            },
            BinOp::Ge => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Bool(x >= y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Bool(x >= y)),
                _ => None,
            },
            BinOp::Add => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Int(x + y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Float(x + y)),
                _ => None,
            },
            BinOp::Sub => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Int(x - y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Float(x - y)),
                _ => None,
            },
            BinOp::Mul => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Int(x * y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Float(x * y)),
                _ => None,
            },
            BinOp::Div => match (&left_val, &right_val) {
                (Val::Int(x), Val::Int(y)) => Some(Val::Int(x / y)),
                (Val::Float(x), Val::Float(y)) => Some(Val::Float(x / y)),
                _ => None,
            },
            _ => None,
        }
        .ok_or_else(|| "expression is not const".into())
    }

    fn const_eval_un_expr(&self, op: UnOp, expr: &Expr) -> Result<Val, Box<dyn Error>> {
        let val = self.const_eval_expr(expr)?;
        match op {
            UnOp::Not => match val {
                Val::Bool(x) => Some(Val::Bool(!x)),
                _ => None,
            },
            UnOp::Neg => match val {
                Val::Int(x) => Some(Val::Int(-x)),
                Val::Float(x) => Some(Val::Float(-x)),
                _ => None,
            },
        }
        .ok_or_else(|| "expression is not const".into())
    }

    fn const_eval_field_expr(
        &self,
        _expr: &Expr,
        _field_ident: Ident,
    ) -> Result<Val, Box<dyn Error>> {
        Err("expression is not const".into())
    }

    fn const_eval_index_expr(
        &self,
        _expr: &Expr,
        _index_expr: &Expr,
    ) -> Result<Val, Box<dyn Error>> {
        Err("expression is not const".into())
    }

    fn const_eval_call_expr(
        &self,
        _ident: Ident,
        _arg_exprs: &[Expr],
    ) -> Result<Val, Box<dyn Error>> {
        Err("expression is not const".into())
    }

    fn const_eval_cons_call_expr(
        &self,
        _ty_lit: TyLit,
        _arg_exprs: &[Expr],
    ) -> Result<Val, Box<dyn Error>> {
        Err("expression is not const".into())
    }

    fn const_eval_var_expr(
        &self,
        _is_lvalue: &Cell<Option<bool>>,
        kind: &Cell<Option<VarKind>>,
        ident: Ident,
    ) -> Result<Val, Box<dyn Error>> {
        match kind.get().unwrap() {
            VarKind::Const => Ok(self
                .shader
                .find_const_decl(ident)
                .unwrap()
                .expr
                .val
                .borrow()
                .as_ref()
                .unwrap()
                .clone()),
            _ => Err("expression is not const".into()),
        }
    }

    fn const_eval_lit_expr(&self, lit: Lit) -> Result<Val, Box<dyn Error>> {
        Ok(lit.to_val())
    }
}
