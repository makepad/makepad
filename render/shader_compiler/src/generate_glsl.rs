use {
    crate::{
        ast::*,
        env::VarKind,
        ident::Ident,
        lit::{
            Lit,
            TyLit
        },
        span::Span,
        ty::Ty,
    },
    std::{
        cell::Cell,
        fmt::Write,
    }
};

struct ExprGenerator<'a> {
    string: &'a mut String,
}

impl<'a> ExprGenerator<'a> {
    fn generate_expr(&mut self, expr: &Expr) {
        match expr.kind {
            ExprKind::Cond {
                span,
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
            } => self.generate_cond_expr(span, expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                span,
                op,
                ref left_expr,
                ref right_expr,
            } => self.generate_bin_expr(span, op, left_expr, right_expr),
            ExprKind::Un {
                span,
                op,
                ref expr
            } => self.generate_un_expr(span, op, expr),
            ExprKind::MethodCall {
                span,
                ident,
                ref arg_exprs
            } => self.generate_method_call_expr(span, ident, arg_exprs),
            ExprKind::Field {
                span,
                ref expr,
                field_ident,
            } => self.generate_field_expr(span, expr, field_ident),
            ExprKind::Index {
                span,
                ref expr,
                ref index_expr,
            } => self.generate_index_expr(span, expr, index_expr),
            ExprKind::Call {
                span,
                ident,
                ref arg_exprs,
            } => self.generate_call_expr(span, ident, arg_exprs),
            ExprKind::MacroCall {
                ref analysis,
                span,
                ident,
                ref arg_exprs,
                ..
            } => self.generate_macro_call_expr(analysis, span, ident, arg_exprs),
            ExprKind::ConsCall {
                span,
                ty_lit,
                ref arg_exprs,
            } => self.generate_cons_call_expr(span, ty_lit, arg_exprs),
            ExprKind::Var {
                span,
                ref is_lvalue,
                ref kind,
                ident,
            } => self.generate_var_expr(span, is_lvalue, kind, ident),
            ExprKind::Lit {
                span,
                lit
            } => self.generate_lit_expr(span, lit),
        }
    }

    fn generate_cond_expr(
        &mut self,
        _span: Span,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr
    ) {
        write!(self.string, "(").unwrap();
        self.generate_expr(expr);
        write!(self.string, " ? ").unwrap();
        self.generate_expr(expr_if_true);
        write!(self.string, " : ").unwrap();
        self.generate_expr(expr_if_false);
        write!(self.string, ")").unwrap();
    }

    fn generate_bin_expr(
        &mut self,
        _span: Span,
        op: BinOp,
        left_expr: &Expr,
        right_expr: &Expr
    ) {
        write!(self.string, "(").unwrap();
        self.generate_expr(left_expr);
        write!(self.string, " {} ", op).unwrap();
        self.generate_expr(right_expr);
        write!(self.string, ")").unwrap();
    }

    fn generate_un_expr(
        &mut self,
        _span: Span,
        op: UnOp,
        expr: &Expr
    ) {
        write!(self.string, "{}", op).unwrap();
        self.generate_expr(expr);
    }

    fn generate_method_call_expr(
        &mut self,
        span: Span,
        ident: Ident,
        arg_exprs: &[Expr]
    ) {
        match arg_exprs[0].ty.borrow().as_ref().unwrap() {
            Ty::Struct { ident: struct_ident } => {
                self.generate_call_expr(
                    span,
                    Ident::new(format!("{}::{}", struct_ident, ident)),
                    arg_exprs
                );
            },
            _ => panic!()
        }
    }

    fn generate_field_expr(
        &mut self,
        _span: Span,
        expr: &Expr,
        field_ident: Ident
    ) {
        self.generate_expr(expr);
        write!(self.string, ".{}", field_ident).unwrap();
    }

    fn generate_index_expr(
        &mut self,
        _span: Span,
        expr: &Expr,
        index_expr: &Expr
    ) {
        self.generate_expr(expr);
        write!(self.string, "[").unwrap();
        self.generate_expr(index_expr);
        write!(self.string, "]").unwrap();
    }

    fn generate_call_expr(
        &mut self,
        _span: Span,
        ident: Ident,
        arg_exprs: &[Expr],
    ) {
        write!(self.string, "{}(", ident).unwrap();
        let mut sep = "";
        for arg_expr in arg_exprs {
            write!(self.string, "{}", sep).unwrap();
            self.generate_expr(arg_expr);
            sep = ", ";
        }
        write!(self.string, ")").unwrap();
    }

    fn generate_macro_call_expr(
        &mut self,
        analysis: &Cell<Option<MacroCallAnalysis>>,
        _span: Span,
        _ident: Ident,
        _arg_exprs: &[Expr],
    ) {
        match analysis.get().unwrap() {
            MacroCallAnalysis::Color { r, g, b, a } => {
                write!(self.string, "vec4({}, {}, {}, {})", r, g, b, a).unwrap();
            }
        }
    }

    fn generate_cons_call_expr(
        &mut self,
        _span: Span,
        ty_lit: TyLit,
        arg_exprs: &[Expr]
    ) {
        write!(self.string, "_m_{}", ty_lit).unwrap();
        for arg_expr in arg_exprs {
            write!(self.string, "_{}", arg_expr.ty.borrow().as_ref().unwrap()).unwrap();
        }
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for arg_expr in arg_exprs {
            write!(self.string, "{}", sep).unwrap();
            self.generate_expr(arg_expr);
            sep = ", ";
        }
        write!(self.string, ")").unwrap();
    }

    fn generate_var_expr(
        &mut self,
        _span: Span,
        _is_lvalue: &Cell<Option<bool>>,
        _kind: &Cell<Option<VarKind>>,
        ident: Ident,
    ) {
        write!(self.string, "{}", ident).unwrap()
    }

    fn generate_lit_expr(
        &mut self,
        _span: Span,
        lit: Lit
    ) {
        write!(self.string, "{}", lit).unwrap();
    }
}