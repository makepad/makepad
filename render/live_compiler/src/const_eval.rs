use crate::shaderast::*;
use crate::env::VarKind;
use crate::error::LiveError;
use crate::ident::{Ident,IdentPath};
use crate::lit::{Lit, TyLit};
use crate::span::Span;
use crate::ty::Ty;
use crate::val::Val;
use std::cell::Cell;

#[derive(Clone, Debug)]
pub struct ConstEvaluator<'a> {
    pub shader: &'a ShaderAst,
    pub no_const_collapse: bool
}

impl<'a> ConstEvaluator<'a> {
    pub fn const_eval_expr(&self, expr: &Expr) -> Result<Val, LiveError> {
        self.try_const_eval_expr(expr).ok_or_else(|| LiveError {
            span: expr.span,
            message: String::from("expression is not const"),
        })
    }

    pub fn try_const_eval_expr(&self, expr: &Expr) -> Option<Val> {
        let const_val = match expr.kind {
            ExprKind::Cond {
                span,
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
            } => self.try_const_eval_cond_expr(span, expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                span,
                op,
                ref left_expr,
                ref right_expr,
            } => self.try_const_eval_bin_expr(span, op, left_expr, right_expr),
            ExprKind::Un { span, op, ref expr } => self.try_const_eval_un_expr(span, op, expr),
            ExprKind::MethodCall {
                span,
                ident,
                ref arg_exprs,
            } => self.try_const_eval_method_call_expr(span, ident, arg_exprs),
            ExprKind::Field {
                span,
                ref expr,
                field_ident,
            } => self.try_const_eval_field_expr(span, expr, field_ident),
            ExprKind::Index {
                span,
                ref expr,
                ref index_expr,
            } => self.try_const_eval_index_expr(span, expr, index_expr),
            ExprKind::Call {
                span,
                ident_path,
                ref arg_exprs,
            } => self.try_const_eval_call_expr(span, ident_path, arg_exprs),
            ExprKind::MacroCall {
                span,
                ref analysis,
                ident,
                ref arg_exprs,
                ..
            } => self.try_const_eval_macro_call_expr(span, analysis, ident, arg_exprs),
            ExprKind::ConsCall {
                span,
                ty_lit,
                ref arg_exprs,
            } => self.try_const_eval_cons_call_expr(span, ty_lit, arg_exprs),
            ExprKind::Var {
                span,
                ref kind,
                ident_path,
            } => self.try_const_eval_var_expr(span, kind, ident_path),
            ExprKind::Lit { span, lit } => self.try_const_eval_lit_expr(span, lit),
        };
        *expr.const_val.borrow_mut() = Some(const_val.clone());
        expr.const_index.set(None);
        const_val
    }

    fn try_const_eval_cond_expr(
        &self,
        _span: Span,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr,
    ) -> Option<Val> {
        let val = self.try_const_eval_expr(expr)?;
        let val_if_true = self.try_const_eval_expr(expr_if_true)?;
        let val_if_false = self.try_const_eval_expr(expr_if_false)?;
        Some(if val.to_bool().unwrap() {
            val_if_true
        } else {
            val_if_false
        })
    }

    #[allow(clippy::float_cmp)]
    fn try_const_eval_bin_expr(
        &self,
        _span: Span,
        op: BinOp,
        left_expr: &Expr,
        right_expr: &Expr,
    ) -> Option<Val> {
        let left_val = self.try_const_eval_expr(left_expr);
        let right_val = self.try_const_eval_expr(right_expr);
        let left_val = left_val?;
        let right_val = right_val?;
        if self.no_const_collapse{
            return None
        }
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
    }

    fn try_const_eval_un_expr(&self, _span: Span, op: UnOp, expr: &Expr) -> Option<Val> {
        let val = self.try_const_eval_expr(expr);
        let val = val?;
        if self.no_const_collapse{
            return None
        }
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
    }

    fn try_const_eval_method_call_expr(
        &self,
        span: Span,
        ident: Ident,
        arg_exprs: &[Expr],
    ) -> Option<Val> {
        match arg_exprs[0].ty.borrow().as_ref().unwrap() {
            Ty::Struct {
                ident: struct_ident,
            } => self.try_const_eval_call_expr(
                span,
                IdentPath::from_two(*struct_ident, ident),
                arg_exprs,
            ),
            _ => panic!(),
        }
    }

    fn try_const_eval_field_expr(
        &self,
        _span: Span,
        expr: &Expr,
        _field_ident: Ident,
    ) -> Option<Val> {
        self.try_const_eval_expr(expr);
        None
    }

    fn try_const_eval_index_expr(
        &self,
        _span: Span,
        expr: &Expr,
        _index_expr: &Expr,
    ) -> Option<Val> {
        self.try_const_eval_expr(expr);
        None
    }

    fn try_const_eval_call_expr(
        &self,
        _span: Span,
        _ident_path: IdentPath,
        arg_exprs: &[Expr],
    ) -> Option<Val> {
        for arg_expr in arg_exprs {
            self.try_const_eval_expr(arg_expr);
        }
        None
    }

    fn try_const_eval_macro_call_expr(
        &self,
        _span: Span,
        _analysis: &Cell<Option<MacroCallAnalysis>>,
        _ident: Ident,
        _arg_exprs: &[Expr],
    ) -> Option<Val> {
        None
    }

    fn try_const_eval_cons_call_expr(
        &self,
        _span: Span,
        _ty_lit: TyLit,
        arg_exprs: &[Expr],
    ) -> Option<Val> {
        for arg_expr in arg_exprs {
            self.try_const_eval_expr(arg_expr);
        }
        None
    }


    fn try_const_eval_var_expr(
        &self,
        _span: Span,
        kind: &Cell<Option<VarKind>>,
        ident_path: IdentPath,
    ) -> Option<Val> {
        let ident = if let Some(ident) = ident_path.get_single(){
            ident
        }
        else{
            return None
        };
        
        match kind.get().unwrap() {
            VarKind::Const => Some(
                self.shader
                    .find_const_decl(ident)
                    .unwrap()
                    .expr
                    .const_val
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .as_ref()
                    .unwrap()
                    .clone(),
            ),
            _ => None,
        }
    }

    fn try_const_eval_lit_expr(&self, _span: Span, lit: Lit) -> Option<Val> {
        Some(lit.to_val())
    }
}
