use crate::shaderast::*;
use crate::env::{Env, Sym, VarKind};
use crate::ident::{Ident,IdentPath};
use crate::lit::{Lit, TyLit};
use crate::span::Span;
use crate::ty::Ty;
use crate::livetypes::LiveId;
use std::cell::Cell;

#[derive(Clone, Debug)]
pub struct DepAnalyser<'a,'b> {
    pub shader: &'a ShaderAst,
    pub decl: &'a FnDecl,
    pub env: &'a Env<'b>,
}

impl<'a,'b> DepAnalyser<'a,'b> {
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
            ExprKind::MethodCall {
                span,
                ident,
                ref arg_exprs,
            } => self.dep_analyse_method_call_expr(span, ident, arg_exprs),
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
                ident_path,
                ref arg_exprs,
            } => self.dep_analyse_call_expr(span, ident_path, arg_exprs),
            ExprKind::MacroCall {
                span,
                ref analysis,
                ident,
                ref arg_exprs,
            } => self.dep_analyse_macro_call_expr(span, analysis, ident, arg_exprs),
            ExprKind::ConsCall {
                span,
                ty_lit,
                ref arg_exprs,
            } => self.dep_analyse_cons_call_expr(span, ty_lit, arg_exprs),
            ExprKind::Var {
                span,
                ref kind,
                ident_path,
            } => self.dep_analyse_var_expr(span, kind, ident_path),
            ExprKind::Lit { span, lit } => self.dep_analyse_lit_expr(span, lit),
        }
    }

    fn dep_analyse_cond_expr(
        &mut self,
        _span: Span,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr,
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
        right_expr: &Expr,
    ) {
        self.dep_analyse_expr(left_expr);
        self.dep_analyse_expr(right_expr);
    }

    fn dep_analyse_un_expr(&mut self, _span: Span, _op: UnOp, expr: &Expr) {
        self.dep_analyse_expr(expr);
    }

    fn dep_analyse_method_call_expr(
        &mut self,
        span: Span,
        method_ident: Ident,
        arg_exprs: &[Expr],
    ) {
        match arg_exprs[0].ty.borrow().as_ref().unwrap() {
            Ty::Struct { ident } => {
                self.dep_analyse_call_expr(
                    span,
                    IdentPath::from_two(*ident, method_ident),
                    arg_exprs,
                );
            }
            _ => panic!(),
        }
    }

    fn dep_analyse_field_expr(&mut self, _span: Span, expr: &Expr, _field_ident: Ident) {
        self.dep_analyse_expr(expr);
    }

    fn dep_analyse_index_expr(&mut self, _span: Span, expr: &Expr, index_expr: &Expr) {
        self.dep_analyse_expr(expr);
        self.dep_analyse_expr(index_expr);
    }

    fn dep_analyse_call_expr(&mut self, span: Span, ident_path: IdentPath, arg_exprs: &[Expr]) {
        //let ident = ident_path.get_single().expect("IMPL");
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        match self.env.find_sym(ident_path, span).unwrap() {
            Sym::Builtin => {
                self.decl
                    .builtin_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(ident_path.get_single().expect("Builtin cant use ::"));
            }
            Sym::Fn => {
                self.decl
                    .callees
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(ident_path);
            }
            _ => panic!(),
        }
    }

    fn dep_analyse_macro_call_expr(
        &mut self,
        _span: Span,
        _analysis: &Cell<Option<MacroCallAnalysis>>,
        _ident: Ident,
        _arg_exprs: &[Expr],
    ) {
    }

    fn dep_analyse_cons_call_expr(&mut self, _span: Span, ty_lit: TyLit, arg_exprs: &[Expr]) {
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        self.decl
            .cons_fn_deps
            .borrow_mut()
            .as_mut()
            .unwrap()
            .insert((
                ty_lit,
                arg_exprs
                    .iter()
                    .map(|arg_expr| arg_expr.ty.borrow().as_ref().unwrap().clone())
                    .collect::<Vec<_>>(),
            ));
    }

    fn dep_analyse_var_expr(&mut self, _span: Span, kind: &Cell<Option<VarKind>>, ident_path: IdentPath) {
        match kind.get().unwrap() {
            VarKind::Geometry => {
                self.decl
                    .geometry_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(ident_path.get_single().expect("unexpected"));
            }
            VarKind::Instance => {
                self.decl
                    .instance_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(ident_path.get_single().expect("unexpected"));
            }
            VarKind::Texture => {
                self.decl.has_texture_deps.set(Some(true));
            }
            VarKind::Uniform => {
                self.decl
                    .uniform_block_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(
                        self.shader
                            .find_uniform_decl(ident_path.get_single().expect("unexpected"))
                            .unwrap()
                            .block_ident
                            .unwrap_or(Ident::new("default")),
                    );
            }
            VarKind::Varying => {
                self.decl.has_varying_deps.set(Some(true));
            }
            VarKind::LiveStyle =>{
                self.decl
                    .uniform_block_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(Ident::new("live"));

            }
            _ => {}
        }
    }

    fn dep_analyse_live_id_expr(&mut self, _span: Span, _kind: &Cell<Option<VarKind>>, _id:LiveId, _ident: Ident) {

    }

    fn dep_analyse_lit_expr(&mut self, _span: Span, _lit: Lit) {}
}
