use {
    crate::{
        shaderast::*,
        env::VarKind,
        ident::Ident,
        ident::IdentPath,
        lit::{Lit, TyLit},
        span::Span,
        ty::Ty,
        util::PrettyPrintedFloat,
        val::Val,
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
    
    fn write_call_expr_hidden_args(&self, string: &mut String, use_const_table: bool, ident_path: IdentPath, shader: &ShaderAst, sep: &str);
    
    fn generate_var_expr(&self, string: &mut String, span:Span, ident_path: IdentPath, kind: &Cell<Option<VarKind>>, shader: &ShaderAst, decl: &FnDecl, ty:&Option<Ty>);
    
    fn write_ident(&self, string: &mut String, ident: Ident);
    
    fn write_ty_lit(&self, string: &mut String, ty_lit: TyLit);
    
    fn write_call_ident(&self, string: &mut String, ident: Ident, arg_exprs: &[Expr]);
    
    fn needs_mul_fn_for_matrix_multiplication(&self) -> bool;
    
    fn needs_unpack_for_matrix_multiplication(&self) -> bool;
    
    fn const_table_is_vec4(&self) -> bool;
    
    fn use_cons_fn(&self, what: &str) -> bool;
}

pub struct BlockGenerator<'a> {
    pub shader: &'a ShaderAst,
    pub decl: &'a FnDecl,
    pub backend_writer: &'a dyn BackendWriter,
    pub create_const_table: bool,
    //pub use_generated_cons_fns: bool,
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
            Stmt::Break {span} => self.generate_break_stmt(span),
            Stmt::Continue {span} => self.generate_continue_stmt(span),
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
            Stmt::Return {span, ref expr} => self.generate_return_stmt(span, expr),
            Stmt::Block {span, ref block} => self.generate_block_stmt(span, block),
            Stmt::Expr {span, ref expr} => self.generate_expr_stmt(span, expr),
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
            if from <= to {from} else {from - 1},
            if from <= to {"<"} else {">="},
            to,
            if step > 0 {"+="} else {"-="},
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
            create_const_table: self.create_const_table,
            //use_hidden_params: self.use_hidden_params,
            //use_generated_cons_fns: self.use_generated_cons_fns,
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
    pub create_const_table: bool,
    //pub use_hidden_params2: bool,
    //pub use_generated_cons_fns: bool,
    pub string: &'a mut String,
}

impl<'a> ExprGenerator<'a> {
    pub fn generate_expr(&mut self, expr: &Expr) {
        fn const_table_index_to_vec4(string: &mut String, index: usize) {
            let base = index>>2;
            let sub = index - (base << 2);
            match sub {
                0 => write!(string, "[{}].x", base).unwrap(),
                1 => write!(string, "[{}].y", base).unwrap(),
                2 => write!(string, "[{}].z", base).unwrap(),
                _ => write!(string, "[{}].w", base).unwrap(),
            }
        }
        match (expr.const_val.borrow().as_ref(), expr.const_index.get()) {
            (Some(Some(Val::Vec4(_))), Some(mut index)) if self.create_const_table => {
                self.write_ty_lit(TyLit::Vec4);
                write!(self.string, "(").unwrap();
                let mut sep = "";
                for _ in 0..4 {
                    write!(self.string, "{}mpsc_const_table", sep).unwrap();
                    if self.backend_writer.const_table_is_vec4() {
                        const_table_index_to_vec4(self.string, index);
                    }
                    else {
                        write!(self.string, "[{}]", index).unwrap();
                    }
                    sep = ", ";
                    index += 1;
                }
                write!(self.string, ")").unwrap();
            },
            (Some(Some(Val::Float(_))), Some(index)) if self.create_const_table => {
                write!(self.string, "mpsc_const_table").unwrap();
                if self.backend_writer.const_table_is_vec4() {
                    const_table_index_to_vec4(self.string, index);
                }
                else {
                    write!(self.string, "[{}]", index).unwrap();
                }
            }
            // TODO: Extract the next three cases into a write_val function
            (Some(Some(Val::Vec4(val))), _) => {
                self.write_ty_lit(TyLit::Vec4);
                write!(
                    self.string,
                    "({}, {}, {}, {})",
                    PrettyPrintedFloat(val.x),
                    PrettyPrintedFloat(val.y),
                    PrettyPrintedFloat(val.z),
                    PrettyPrintedFloat(val.w),
                ).unwrap();
            }
            (Some(Some(Val::Float(val))), _) => {
                write!(self.string, "{}", PrettyPrintedFloat(*val)).unwrap();
            },
            (Some(Some(val)), _) => {
                write!(self.string, "{}", val).unwrap();
            },
            _ => match expr.kind {
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
                ExprKind::Un {span, op, ref expr} => self.generate_un_expr(span, op, expr),
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
                    ident_path,
                    ref arg_exprs,
                } => self.generate_call_expr(span, ident_path, arg_exprs),
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
                    ident_path,
                } => self.generate_var_expr(span, kind, ident_path, &expr.ty.borrow()),
                ExprKind::Lit {span, lit} => self.generate_lit_expr(span, lit),
            },
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
        
        // if left_expr or right_expr is a matrix, HLSL needs to use mul()
        let left_is_mat = match left_expr.ty.borrow().as_ref().unwrap() {
            Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => true,
            _ => false
        };
        let right_is_mat = match right_expr.ty.borrow().as_ref().unwrap() {
            Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => true,
            _ => false
        };
        
        if self.backend_writer.needs_mul_fn_for_matrix_multiplication() {
            if left_is_mat || right_is_mat {
                write!(self.string, "mul(").unwrap();
                self.generate_expr(left_expr);
                write!(self.string, ", ").unwrap();
                self.generate_expr(right_expr);
                write!(self.string, ")").unwrap();
                return
            }
        }
        else if self.backend_writer.needs_unpack_for_matrix_multiplication() {
            if left_is_mat && !right_is_mat {
                match right_expr.ty.borrow().as_ref().unwrap() {
                    Ty::Vec4 => {
                        write!(self.string, "(").unwrap();
                        self.generate_expr(left_expr);
                        write!(self.string, " {} ", op).unwrap();
                        self.backend_writer.write_ty_lit(self.string, TyLit::Vec4);
                        write!(self.string, "(").unwrap();
                        self.generate_expr(right_expr);
                        write!(self.string, "))").unwrap();
                        return
                    },
                    Ty::Vec3 => {
                        write!(self.string, "(").unwrap();
                        self.generate_expr(left_expr);
                        write!(self.string, " {} ", op).unwrap();
                        self.backend_writer.write_ty_lit(self.string, TyLit::Vec3);
                        write!(self.string, "(").unwrap();
                        self.generate_expr(right_expr);
                        write!(self.string, "))").unwrap();
                        return
                    },
                    _ => ()
                };
            }
            else if !left_is_mat && right_is_mat {
                match left_expr.ty.borrow().as_ref().unwrap() {
                    Ty::Vec4 => {
                        write!(self.string, "(").unwrap();
                        self.backend_writer.write_ty_lit(self.string, TyLit::Vec4);
                        write!(self.string, "(").unwrap();
                        self.generate_expr(left_expr);
                        write!(self.string, ") {} ", op).unwrap();
                        self.generate_expr(right_expr);
                        write!(self.string, ")").unwrap();
                        return
                    },
                    Ty::Vec3 => {
                        write!(self.string, "(").unwrap();
                        self.backend_writer.write_ty_lit(self.string, TyLit::Vec3);
                        write!(self.string, "(").unwrap();
                        self.generate_expr(left_expr);
                        write!(self.string, ") {} ", op).unwrap();
                        self.generate_expr(right_expr);
                        write!(self.string, ")").unwrap();
                        return
                    },
                    _ => ()
                };
            }
        }
        
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
                    IdentPath::from_two(*struct_ident, ident),
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
    
    fn generate_call_expr(&mut self, _span: Span, ident_path: IdentPath, arg_exprs: &[Expr]) {
        let ident = ident_path.to_struct_fn_ident();

        //TODO add built-in check
        self.backend_writer.write_call_ident(&mut self.string, ident, arg_exprs);
        
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for arg_expr in arg_exprs {
            write!(self.string, "{}", sep).unwrap();
            
            self.generate_expr(arg_expr);
            
            sep = ", ";
        }
        
        self.backend_writer.write_call_expr_hidden_args(&mut self.string, self.create_const_table, ident_path, &self.shader, &sep);
        
        write!(self.string, ")").unwrap();
    }
    
    fn generate_macro_call_expr(
        &mut self,
        _analysis: &Cell<Option<MacroCallAnalysis>>,
        _span: Span,
        _ident: Ident,
        _arg_exprs: &[Expr],
    ) {

    }
    
    fn generate_cons_call_expr(&mut self, _span: Span, ty_lit: TyLit, arg_exprs: &[Expr]) {
        // lets build the constructor name
        let mut cons_name = format!("mpsc_{}", ty_lit);
        for arg_expr in arg_exprs {
            write!(cons_name, "_{}", arg_expr.ty.borrow().as_ref().unwrap()).unwrap();
        }
        if self.backend_writer.use_cons_fn(&cons_name) {
            write!(self.string, "{}", cons_name).unwrap();
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
    
    fn generate_var_expr(&mut self, span: Span, kind: &Cell<Option<VarKind>>, ident_path: IdentPath, ty:&Option<Ty>) {
        //self.backend_write.generate_var_expr(&mut self.string, span, kind, &self.shader, decl)
        if let Some(decl) = self.decl {
            self.backend_writer.generate_var_expr(&mut self.string, span, ident_path, kind, &self.shader, decl, ty)
        }
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
