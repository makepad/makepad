use crate::ast::*;
use crate::env::{Env, Sym, VarKind};
use crate::ident::Ident;
use crate::lit::{Lit, TyLit};
use crate::span::Span;
use std::cell::Cell;

#[derive(Clone, Debug)]
pub struct DepAnalyser<'a> {
    pub shader: &'a Shader,
    pub decl: &'a FnDecl,
    pub env: &'a Env,
}

impl<'a> DepAnalyser<'a> {
    pub fn dep_analyse_expr(&mut self, expr: &Expr) {
        match expr.kind {
            ExprKind::Cond {
                span,
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
            } => self.dep_analyse_cond_expr(span, expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                span,
                op,
                ref left_expr,
                ref right_expr,
            } => self.dep_analyse_bin_expr(span, op, left_expr, right_expr),
            ExprKind::Un { span, op, ref expr } => self.dep_analyse_un_expr(span, op, expr),
            ExprKind::Field {
                span,
                ref expr,
                field_ident,
            } => self.dep_analyse_field_expr(span, expr, field_ident),
            ExprKind::Index {
                span,
                ref expr,
                ref index_expr,
            } => self.dep_analyse_index_expr(span, expr, index_expr),
            ExprKind::Call {
                span,
                ident,
                ref arg_exprs,
            } => self.dep_analyse_call_expr(span, ident, arg_exprs),
            ExprKind::ConsCall {
                span,
                ty_lit,
                ref arg_exprs,
            } => self.dep_analyse_cons_call_expr(span, ty_lit, arg_exprs),
            ExprKind::Var {
                span,
                ref is_lvalue,
                ref kind,
                ident,
                ..
            } => self.dep_analyse_var_expr(span, is_lvalue, kind, ident),
            ExprKind::Lit { span, lit } => self.dep_analyse_lit_expr(span, lit),
        }
    }

    fn dep_analyse_cond_expr(
        &mut self,
        _span: Span,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr
    ) {
        self.dep_analyse_expr(expr);
        self.dep_analyse_expr(expr_if_true);
        self.dep_analyse_expr(expr_if_false);
    }

    fn dep_analyse_bin_expr(
        &mut self,
        _span: Span,
        _op: BinOp,
        left_expr: &Expr,
        right_expr: &Expr
    ) {
        self.dep_analyse_expr(left_expr);
        self.dep_analyse_expr(right_expr);
    }

    fn dep_analyse_un_expr(&mut self, _span: Span, _op: UnOp, expr: &Expr) {
        self.dep_analyse_expr(expr);
    }

    fn dep_analyse_field_expr(&mut self, _span: Span, expr: &Expr, _field_ident: Ident) {
        self.dep_analyse_expr(expr);
    }

    fn dep_analyse_index_expr(&mut self, _span: Span, expr: &Expr, index_expr: &Expr) {
        self.dep_analyse_expr(expr);
        self.dep_analyse_expr(index_expr);
    }

    fn dep_analyse_call_expr(&mut self, _span: Span, ident: Ident, arg_exprs: &[Expr]) {
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        match self.env.find_sym(ident).unwrap() {
            Sym::Builtin => {
                self.decl
                    .builtin_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(ident);
            }
            Sym::Fn => {
                self.decl
                    .callees
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(ident);
            }
            _ => panic!(),
        }
    }

    fn dep_analyse_cons_call_expr(&mut self, _span: Span, ty_lit: TyLit, arg_exprs: &[Expr]) {
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        self.decl.cons_deps.borrow_mut().as_mut().unwrap().insert((
            ty_lit,
            arg_exprs
                .iter()
                .map(|arg_expr| arg_expr.ty.borrow().as_ref().unwrap().clone())
                .collect::<Vec<_>>(),
        ));
    }

    fn dep_analyse_var_expr(
        &mut self,
        _span: Span,
        is_lvalue: &Cell<Option<bool>>,
        _kind: &Cell<Option<VarKind>>,
        ident: Ident,
    ) {
        match self.env.find_sym(ident).unwrap() {
            Sym::Var { kind, .. } => match kind {
                VarKind::Attribute => {
                    self.decl
                        .attribute_deps
                        .borrow_mut()
                        .as_mut()
                        .unwrap()
                        .insert(ident);
                }
                VarKind::Uniform => {
                    self.decl
                        .uniform_block_deps
                        .borrow_mut()
                        .as_mut()
                        .unwrap()
                        .insert(
                            self.shader
                                .find_uniform_decl(ident)
                                .unwrap()
                                .block_ident
                                .unwrap_or(Ident::new("default")),
                        );
                }
                VarKind::Varying => {
                    if is_lvalue.get().unwrap() {
                        self.decl.has_out_varying_deps.set(Some(true));
                    } else {
                        self.decl.has_in_varying_deps.set(Some(true));
                    }
                }
                _ => {}
            },
            _ => panic!(),
        }
    }

    fn dep_analyse_lit_expr(&mut self, _span: Span, _lit: Lit) {}
}
