use {
    crate::{
        ast::*,
        env::VarKind,
        ident::Ident,
        lit::{Lit, TyLit},
        span::Span,
        ty::Ty,
    },
    std::{
        cell::{Cell, RefCell},
        fmt::Write,
    },
};

pub trait BackendWriter {
    fn write_var_decl(
        &self,
        string: &mut String,
        is_inout: bool,
        is_packed: bool,
        ident: Ident,
        ty: &Ty,
    );

    fn write_ident(&self, string: &mut String, ident: Ident);

    fn write_ty_lit(&self, string: &mut String, ty_lit: TyLit);
}

pub struct BlockGenerator<'a> {
    pub shader: &'a ShaderAst,
    pub decl: &'a FnDecl,
    pub backend_writer: &'a dyn BackendWriter,
    pub use_hidden_params: bool,
    pub use_generated_cons_fns: bool,
    pub indent_level: usize,
    pub string: &'a mut String,
}

impl<'a> BlockGenerator<'a> {
    pub fn generate_block(&mut self, block: &Block) {
        write!(self.string, "{{").unwrap();
        if !block.stmts.is_empty() {
            writeln!(self.string).unwrap();
            self.indent_level += 1;
            for stmt in &block.stmts {
                self.generate_stmt(stmt);
            }
            self.indent_level -= 1;
            self.write_indent();
        }
        write!(self.string, "}}").unwrap()
    }

    fn generate_stmt(&mut self, stmt: &Stmt) {
        self.write_indent();
        match *stmt {
            Stmt::Break { span } => self.generate_break_stmt(span),
            Stmt::Continue { span } => self.generate_continue_stmt(span),
            Stmt::For {
                span,
                ident,
                ref from_expr,
                ref to_expr,
                ref step_expr,
                ref block,
            } => self.generate_for_stmt(span, ident, from_expr, to_expr, step_expr, block),
            Stmt::If {
                span,
                ref expr,
                ref block_if_true,
                ref block_if_false,
            } => self.generate_if_stmt(span, expr, block_if_true, block_if_false),
            Stmt::Let {
                span,
                ref ty,
                ident,
                ref ty_expr,
                ref expr,
            } => self.generate_let_stmt(span, ty, ident, ty_expr, expr),
            Stmt::Return { span, ref expr } => self.generate_return_stmt(span, expr),
            Stmt::Block { span, ref block } => self.generate_block_stmt(span, block),
            Stmt::Expr { span, ref expr } => self.generate_expr_stmt(span, expr),
        }
    }

    fn generate_break_stmt(&mut self, _span: Span) {
        writeln!(self.string, "break;").unwrap();
    }

    fn generate_continue_stmt(&mut self, _span: Span) {
        writeln!(self.string, "continue;").unwrap();
    }

    fn generate_for_stmt(
        &mut self,
        _span: Span,
        ident: Ident,
        from_expr: &Expr,
        to_expr: &Expr,
        step_expr: &Option<Expr>,
        block: &Block,
    ) {
        let from = from_expr
            .const_val
            .borrow()
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .to_int()
            .unwrap();
        let to = to_expr
            .const_val
            .borrow()
            .as_ref()
            .unwrap()
            .as_ref()
            .unwrap()
            .to_int()
            .unwrap();
        let step = if let Some(step_expr) = step_expr {
            step_expr
                .const_val
                .borrow()
                .as_ref()
                .unwrap()
                .as_ref()
                .unwrap()
                .to_int()
                .unwrap()
        } else if from < to {
            1
        } else {
            -1
        };
        write!(
            self.string,
            "for (int {0} = {1}; {0} {2} {3}; {0} {4} {5}) ",
            ident,
            if from <= to { from } else { from - 1 },
            if from <= to { "<" } else { ">=" },
            to,
            if step > 0 { "+=" } else { "-=" },
            step.abs()
        )
        .unwrap();
        self.generate_block(block);
        writeln!(self.string).unwrap();
    }

    fn generate_if_stmt(
        &mut self,
        _span: Span,
        expr: &Expr,
        block_if_true: &Block,
        block_if_false: &Option<Box<Block>>,
    ) {
        write!(self.string, "if").unwrap();
        self.generate_expr(expr);
        write!(self.string, " ").unwrap();
        self.generate_block(block_if_true);
        if let Some(block_if_false) = block_if_false {
            write!(self.string, "else").unwrap();
            self.generate_block(block_if_false);
        }
        writeln!(self.string).unwrap();
    }

    fn generate_let_stmt(
        &mut self,
        _span: Span,
        ty: &RefCell<Option<Ty>>,
        ident: Ident,
        _ty_expr: &Option<TyExpr>,
        expr: &Option<Expr>,
    ) {
        self.write_var_decl(false, false, ident, ty.borrow().as_ref().unwrap());
        if let Some(expr) = expr {
            write!(self.string, " = ").unwrap();
            self.generate_expr(expr);
        }
        writeln!(self.string, ";").unwrap();
    }

    fn generate_return_stmt(&mut self, _span: Span, expr: &Option<Expr>) {
        write!(self.string, "return").unwrap();
        if let Some(expr) = expr {
            write!(self.string, " ").unwrap();
            self.generate_expr(expr);
        }
        writeln!(self.string, ";").unwrap();
    }

    fn generate_block_stmt(&mut self, _span: Span, block: &Block) {
        self.generate_block(block);
        writeln!(self.string).unwrap();
    }

    fn generate_expr_stmt(&mut self, _span: Span, expr: &Expr) {
        self.generate_expr(expr);
        writeln!(self.string, ";").unwrap();
    }

    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            shader: self.shader,
            decl: Some(self.decl),
            backend_writer: self.backend_writer,
            use_hidden_params: self.use_hidden_params,
            use_generated_cons_fns: self.use_generated_cons_fns,
            string: self.string,
        }
        .generate_expr(expr)
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent_level {
            write!(self.string, "    ").unwrap();
        }
    }

    fn write_var_decl(&mut self, is_inout: bool, is_packed: bool, ident: Ident, ty: &Ty) {
        self.backend_writer
            .write_var_decl(&mut self.string, is_inout, is_packed, ident, ty);
    }
}

pub struct ExprGenerator<'a> {
    pub shader: &'a ShaderAst,
    pub decl: Option<&'a FnDecl>,
    pub backend_writer: &'a dyn BackendWriter,
    pub use_hidden_params: bool,
    pub use_generated_cons_fns: bool,
    pub string: &'a mut String,
}

impl<'a> ExprGenerator<'a> {
    pub fn generate_expr(&mut self, expr: &Expr) {
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
            ExprKind::Un { span, op, ref expr } => self.generate_un_expr(span, op, expr),
            ExprKind::MethodCall {
                span,
                ident,
                ref arg_exprs,
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
                ref kind,
                ident,
            } => self.generate_var_expr(span, kind, ident),
            ExprKind::Lit { span, lit } => self.generate_lit_expr(span, lit),
        }
    }

    fn generate_cond_expr(
        &mut self,
        _span: Span,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr,
    ) {
        write!(self.string, "(").unwrap();
        self.generate_expr(expr);
        write!(self.string, " ? ").unwrap();
        self.generate_expr(expr_if_true);
        write!(self.string, " : ").unwrap();
        self.generate_expr(expr_if_false);
        write!(self.string, ")").unwrap();
    }

    fn generate_bin_expr(&mut self, _span: Span, op: BinOp, left_expr: &Expr, right_expr: &Expr) {
        write!(self.string, "(").unwrap();
        self.generate_expr(left_expr);
        write!(self.string, " {} ", op).unwrap();
        self.generate_expr(right_expr);
        write!(self.string, ")").unwrap();
    }

    fn generate_un_expr(&mut self, _span: Span, op: UnOp, expr: &Expr) {
        write!(self.string, "{}", op).unwrap();
        self.generate_expr(expr);
    }

    fn generate_method_call_expr(&mut self, span: Span, ident: Ident, arg_exprs: &[Expr]) {
        match arg_exprs[0].ty.borrow().as_ref().unwrap() {
            Ty::Struct {
                ident: struct_ident,
            } => {
                self.generate_call_expr(
                    span,
                    Ident::new(format!("mpsc_{}_{}", struct_ident, ident)),
                    arg_exprs,
                );
            }
            _ => panic!(),
        }
    }

    fn generate_field_expr(&mut self, _span: Span, expr: &Expr, field_ident: Ident) {
        self.generate_expr(expr);
        write!(self.string, ".{}", field_ident).unwrap();
    }

    fn generate_index_expr(&mut self, _span: Span, expr: &Expr, index_expr: &Expr) {
        self.generate_expr(expr);
        write!(self.string, "[").unwrap();
        self.generate_expr(index_expr);
        write!(self.string, "]").unwrap();
    }

    fn generate_call_expr(&mut self, _span: Span, ident: Ident, arg_exprs: &[Expr]) {
        self.write_ident(ident);
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for arg_expr in arg_exprs {
            write!(self.string, "{}", sep).unwrap();
            self.generate_expr(arg_expr);
            sep = ", ";
        }
        if self.use_hidden_params {
            if let Some(decl) = self.shader.find_fn_decl(ident) {
                for &ident in decl.uniform_block_deps.borrow().as_ref().unwrap() {
                    write!(self.string, "{}mpsc_{}_uniforms", sep, ident).unwrap();
                    sep = ", ";
                }
                if decl.has_texture_deps.get().unwrap() {
                    write!(self.string, "{}mpsc_textures", sep).unwrap();
                    sep = ", ";
                }
                if decl.is_used_in_vertex_shader.get().unwrap() {
                    if !decl.geometry_deps.borrow().as_ref().unwrap().is_empty() {
                        write!(self.string, "{}mpsc_attributes", sep).unwrap();
                        sep = ", ";
                    }
                    if !decl.instance_deps.borrow().as_ref().unwrap().is_empty() {
                        write!(self.string, "{}mpsc_instances", sep).unwrap();
                        sep = ", ";
                    }
                    if decl.has_varying_deps.get().unwrap() {
                        write!(self.string, "{}mpsc_varyings", sep).unwrap();
                    }
                } else {
                    assert!(decl.is_used_in_fragment_shader.get().unwrap());
                    if !decl.geometry_deps.borrow().as_ref().unwrap().is_empty()
                        || !decl.instance_deps.borrow().as_ref().unwrap().is_empty()
                        || decl.has_varying_deps.get().unwrap()
                    {
                        write!(self.string, "{}mpsc_varyings", sep).unwrap();
                    }
                }
            }
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
        fn float_to_string(v: f32) -> String {
            if v.abs().fract() < 0.00000001 {
                format!("{}.0", v)
            } else {
                format!("{}", v)
            }
        }
        match analysis.get().unwrap() {
            MacroCallAnalysis::Color { r, g, b, a } => {
                self.backend_writer.write_ty_lit(self.string, TyLit::Vec4);
                write!(
                    self.string,
                    "({}, {}, {}, {})",
                    float_to_string(r),
                    float_to_string(g),
                    float_to_string(b),
                    float_to_string(a)
                )
                .unwrap();
            }
        }
    }

    fn generate_cons_call_expr(&mut self, _span: Span, ty_lit: TyLit, arg_exprs: &[Expr]) {
        if self.use_generated_cons_fns {
            write!(self.string, "mpsc_").unwrap();
            write!(self.string, "{}", ty_lit).unwrap();
            //self.write_ty_lit(ty_lit);
            for arg_expr in arg_exprs {
                write!(self.string, "_{}", arg_expr.ty.borrow().as_ref().unwrap()).unwrap();
            }
        } else {
            self.write_ty_lit(ty_lit);
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

    fn generate_var_expr(&mut self, _span: Span, kind: &Cell<Option<VarKind>>, ident: Ident) {
        if self.use_hidden_params {
            if let Some(decl) = self.decl {
                let is_used_in_vertex_shader = decl.is_used_in_vertex_shader.get().unwrap();
                let is_used_in_fragment_shader = decl.is_used_in_fragment_shader.get().unwrap();
                if is_used_in_vertex_shader {
                    match kind.get().unwrap() {
                        VarKind::Geometry => write!(self.string, "mpsc_geometries.").unwrap(),
                        VarKind::Instance => write!(self.string, "mpsc_instances.").unwrap(),
                        VarKind::Uniform => {
                            write!(
                                self.string,
                                "mpsc_{}_uniforms.",
                                self.shader
                                    .find_uniform_decl(ident)
                                    .unwrap()
                                    .block_ident
                                    .unwrap_or(Ident::new("default")),
                            )
                            .unwrap();
                        }
                        VarKind::Texture => write!(self.string, "mpsc_textures.").unwrap(),
                        VarKind::Varying => write!(self.string, "mpsc_varyings.").unwrap(),
                        _ => {}
                    }
                }
                if is_used_in_fragment_shader {
                    match kind.get().unwrap() {
                        VarKind::Uniform => {
                            write!(
                                self.string,
                                "mpsc_{}_uniforms.",
                                self.shader
                                    .find_uniform_decl(ident)
                                    .unwrap()
                                    .block_ident
                                    .unwrap_or(Ident::new("default")),
                            )
                            .unwrap();
                        }
                        VarKind::Texture => write!(self.string, "mpsc_textures.").unwrap(),
                        VarKind::Geometry | VarKind::Instance | VarKind::Varying => {
                            write!(self.string, "mpsc_varyings.").unwrap()
                        }
                        _ => {}
                    }
                }
            }
        }
        write!(self.string, "{}", ident).unwrap()
    }

    fn generate_lit_expr(&mut self, _span: Span, lit: Lit) {
        write!(self.string, "{}", lit).unwrap();
    }

    fn write_ident(&mut self, ident: Ident) {
        self.backend_writer.write_ident(&mut self.string, ident);
    }

    fn write_ty_lit(&mut self, ty_lit: TyLit) {
        self.backend_writer.write_ty_lit(&mut self.string, ty_lit);
    }
}
