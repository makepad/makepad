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
        swizzle::Swizzle,
        ty::Ty,
    },
    std::{
        cell::{
            Cell,
            RefCell,
        },
        fmt::Write,
    }
};

pub fn generate_vertex_shader(shader: &ShaderAst) -> String {
    let mut string = String::new();
    ShaderGenerator {
        shader,
        string: &mut string,
    }
    .generate_vertex_shader();
    string
}

pub fn generate_fragment_shader(shader: &ShaderAst) -> String {
    let mut string = String::new();
    ShaderGenerator {
        shader,
        string: &mut string,
    }
    .generate_fragment_shader();
    string
}

struct ShaderGenerator<'a> {
    shader: &'a ShaderAst,
    string: &'a mut String,
}

impl<'a> ShaderGenerator<'a> {
    fn generate_vertex_shader(&mut self) {
        let packed_attributes_size = self.compute_packed_attributes_size();
        let packed_varyings_size = self.compute_packed_varyings_size();
        self.generate_declarations(Some(packed_attributes_size), packed_varyings_size);
        self.generate_fn_decl(self.shader.find_fn_decl(Ident::new("vertex")).unwrap());
        writeln!(self.string, "void main() {{").unwrap();
        let mut attribute_unpacker = VarUnpacker::new(
            "_m_packed_attribute",
            packed_attributes_size,
            &mut self.string
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) => {
                    attribute_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                Decl::Instance(decl) => {
                    attribute_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                }
                _ => {}
            }
        }
        let mut varying_packer = VarPacker::new(
            "_m_packed_varying",
            packed_varyings_size,
            &mut self.string
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_packer.pack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_packer.pack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                }
                Decl::Varying(decl) => {
                    varying_packer.pack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                _ => {}
            }
        }
        write!(self.string, "}}").unwrap();
    }

    fn generate_fragment_shader(&mut self) {
        let packed_varyings_size = self.compute_packed_varyings_size();
        self.generate_declarations(None, packed_varyings_size);
        self.generate_fn_decl(self.shader.find_fn_decl(Ident::new("pixel")).unwrap());
        writeln!(self.string, "void main() {{").unwrap();
        let mut varying_unpacker = VarUnpacker::new(
            "_m_packed_varying",
            packed_varyings_size,
            &mut self.string
        );
        for decl in &self.shader.decls {
            match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    varying_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                }
                Decl::Varying(decl) => {
                    varying_unpacker.unpack_var(
                        decl.ident,
                        decl.ty_expr.ty.borrow().as_ref().unwrap(),
                    );
                },
                _ => {}
            }
        }
        write!(self.string, "}}").unwrap();
    }

    fn generate_declarations(
        &mut self,
        packed_attributes_size: Option<usize>,
        packed_varyings_size: usize
    ) {
        for decl in &self.shader.decls {
            match decl {
                Decl::Struct(decl) => self.generate_struct_decl(decl),
                _ => {}
            }
        }

        for decl in &self.shader.decls {
            match decl {
                Decl::Const(decl) => self.generate_const_decl(decl),
                _ => {}
            }
        }

        for decl in &self.shader.decls {
            match decl {
                Decl::Uniform(decl) => self.generate_uniform_decl(decl),
                _ => {}
            }
        }

        if let Some(packed_attributes_size) = packed_attributes_size {
            self.generate_packed_attribute_declarations(packed_attributes_size);
        }

        self.generate_packed_varying_declarations(packed_varyings_size);
    }

    fn generate_struct_decl(&mut self, decl: &StructDecl) {
        write!(self.string, "struct {} {{", decl.ident).unwrap();
        if !decl.fields.is_empty() {
            writeln!(self.string).unwrap();
            for field in &decl.fields {
                write!(self.string, "    ").unwrap();
                write_ident_and_ty(
                    &mut self.string,
                    field.ident,
                    field.ty_expr.ty.borrow().as_ref().unwrap(),
                );
                writeln!(self.string, ";").unwrap();
            }
        }
        writeln!(self.string, "}};").unwrap();
    }

    fn generate_const_decl(&mut self, decl: &ConstDecl) {
        write!(self.string, "const ").unwrap();
        write_ident_and_ty(
            &mut self.string,
            decl.ident,
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, " = ").unwrap();
        self.generate_expr(&decl.expr);
        writeln!(self.string, ";").unwrap();
    }

    fn generate_uniform_decl(&mut self, decl: &UniformDecl) {
        write!(self.string, "uniform ").unwrap();
        write_ident_and_ty(
            self.string,
            decl.ident,
            decl.ty_expr.ty.borrow().as_ref().unwrap(),
        );
        writeln!(self.string, ";").unwrap();
    }

    fn compute_packed_attributes_size(&self) -> usize {
        let mut packed_attributes_size = 0;
        for decl in &self.shader.decls {
            packed_attributes_size += match decl {
                Decl::Attribute(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
                Decl::Instance(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
                _ => 0,
            }
        }
        packed_attributes_size
    }

    fn generate_packed_attribute_declarations(
        &mut self,
        mut packed_attributes_size: usize
    ) {
        let mut packed_attribute_index = 0;
        loop {
            let packed_attribute_size = packed_attributes_size.min(4);
            writeln!(
                self.string,
                "attribute {} _m_packed_attribute_{};",
                match packed_attribute_size {
                    0 => break,
                    1 => "float",
                    2 => "vec2",
                    3 => "vec3",
                    4 => "vec4",
                    _ => panic!(),
                },
                packed_attribute_index,
            )
            .unwrap();
            packed_attributes_size -= packed_attribute_size;
            packed_attribute_index += 1;
        }
    }

    fn compute_packed_varyings_size(&self) -> usize {
        let mut packed_varyings_size = 0;
        for decl in &self.shader.decls {
            packed_varyings_size += match decl {
                Decl::Attribute(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    decl.ty_expr.ty.borrow().as_ref().unwrap().size()
                },
                Decl::Instance(decl) if decl.is_used_in_fragment_shader.get().unwrap() => {
                    decl.ty_expr.ty.borrow().as_ref().unwrap().size()
                }
                Decl::Varying(decl) => decl.ty_expr.ty.borrow().as_ref().unwrap().size(),
                _ => 0,
            }
        }
        packed_varyings_size
    }

    fn generate_packed_varying_declarations(
        &mut self,
        mut packed_varyings_size: usize
    ) {
        let mut packed_varying_index = 0;
        loop {
            let packed_varying_size = packed_varyings_size.min(4);
            writeln!(
                self.string,
                "varying {} _m_packed_varying_{};",
                match packed_varying_size {
                    0 => break,
                    1 => "float",
                    2 => "vec2",
                    3 => "vec3",
                    4 => "vec4",
                    _ => panic!(),
                },
                packed_varying_index,
            )
            .unwrap();
            packed_varyings_size -= packed_varying_size;
            packed_varying_index += 1;
        }
    }

    fn generate_fn_decl(&mut self, decl: &FnDecl) {
        for &callee in decl.callees.borrow().as_ref().unwrap().iter() {
            self.generate_fn_decl(self.shader.find_fn_decl(callee).unwrap());
        }
        write_ident_and_ty(
            &mut self.string,
            decl.ident,
            decl.return_ty.borrow().as_ref().unwrap(),
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &decl.params {
            write!(self.string, "{}", sep).unwrap();
            write_ident_and_ty(
                &mut self.string,
                param.ident,
                param.ty_expr.ty.borrow().as_ref().unwrap(),
            );
            sep = ", ";
        }
        write!(self.string, ") ").unwrap();
        self.generate_block(&decl.block);
        writeln!(self.string).unwrap();
    }

    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            indent_level: 0,
            string: self.string
        }
        .generate_block(block)
    }

    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            string: self.string,
        }
        .generate_expr(expr)
    }
}

struct VarPacker<'a> {
    packed_var_prefix: &'a str,
    packed_vars_size: usize,
    packed_var_index: usize,
    packed_var_size: usize,
    packed_var_offset: usize,
    string: &'a mut String
}

impl<'a> VarPacker<'a> {
    fn new(
        packed_var_prefix: &'a str,
        packed_vars_size: usize,
        string: &'a mut String
    ) -> VarPacker<'a> {
        VarPacker {
            packed_var_prefix,
            packed_vars_size,
            packed_var_index: 0,
            packed_var_size: packed_vars_size.min(4),
            packed_var_offset: 0,
            string,
        }
    }

    fn pack_var(&mut self, ident: Ident, ty: &Ty) {
        let var_size = ty.size();
        let mut var_offset = 0;
        while var_offset < var_size {
            let count = var_size - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = count.min(packed_count);
            write!(self.string, "    {}_{}", self.packed_var_prefix, self.packed_var_index).unwrap();
            if self.packed_var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        self.packed_var_offset,
                        self.packed_var_offset + min_count
                    )
                )
                .unwrap();
            }
            write!(self.string, " = {}", ident).unwrap();
            if var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        var_offset,
                        var_offset + min_count
                    )
                )
                .unwrap();
            }
            writeln!(self.string, ";").unwrap();
            self.packed_var_offset += min_count;
            if self.packed_var_offset == self.packed_var_size {
                self.packed_vars_size -= self.packed_var_size;
                self.packed_var_index += 1;
                self.packed_var_size = self.packed_var_size.min(4);
                self.packed_var_offset = 0;
            }
            var_offset += min_count; 
        }
    }
}

struct VarUnpacker<'a> {
    packed_var_prefix: &'a str,
    packed_vars_size: usize,
    packed_var_index: usize,
    packed_var_size: usize,
    packed_var_offset: usize,
    string: &'a mut String
}

impl<'a> VarUnpacker<'a> {
    fn new(
        packed_var_prefix: &'a str,
        packed_vars_size: usize,
        string: &'a mut String
    ) -> VarUnpacker<'a> {
        VarUnpacker {
            packed_var_prefix,
            packed_vars_size,
            packed_var_index: 0,
            packed_var_size: packed_vars_size.min(4),
            packed_var_offset: 0,
            string,
        }
    }

    fn unpack_var(&mut self, ident: Ident, ty: &Ty) {
        let var_size = ty.size();
        let mut var_offset = 0;
        while var_offset < var_size {
            let count = var_size - var_offset;
            let packed_count = self.packed_var_size - self.packed_var_offset;
            let min_count = count.min(packed_count);
            write!(self.string, "    {}", ident).unwrap();
            if var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        var_offset,
                        var_offset + min_count
                    )
                )
                .unwrap();
            }
            write!(self.string, " = {}_{}", self.packed_var_prefix, self.packed_var_index).unwrap();
            if self.packed_var_size > 1 {
                write!(
                    self.string,
                    ".{}",
                    Swizzle::from_range(
                        self.packed_var_offset,
                        self.packed_var_offset + min_count
                    )
                )
                .unwrap();
            }
            writeln!(self.string, ";").unwrap();
            var_offset += min_count;
            self.packed_var_offset += min_count;
            if self.packed_var_offset == self.packed_var_size {
                self.packed_vars_size -= self.packed_var_size;
                self.packed_var_index += 1;
                self.packed_var_size = self.packed_var_size.min(4);
                self.packed_var_offset = 0;
            } 
        }
    }
}

struct BlockGenerator<'a> {
    indent_level: usize,
    string: &'a mut String,
}

impl<'a> BlockGenerator<'a> {
    fn generate_block(&mut self, block: &Block) {
        write!(self.string, "{{").unwrap();
        if !block.stmts.is_empty() {
            writeln!(self.string).unwrap();
            self.indent_level += 1;
            for stmt in &block.stmts {
                self.generate_stmt(stmt);
            }
            self.indent_level -= 1;
        }
        write!(self.string, "}}").unwrap()
    }

    fn generate_stmt(&mut self, stmt: &Stmt) {
        self.write_indent();
        match *stmt {
            Stmt::Break {
                span
            } => self.generate_break_stmt(span),
            Stmt::Continue {
                span
            } => self.generate_continue_stmt(span),
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
            Stmt::Return {
                span,
                ref expr
            } => self.generate_return_stmt(span, expr),
            Stmt::Block {
                span,
                ref block
            } => self.generate_block_stmt(span, block),
            Stmt::Expr {
                span,
                ref expr
            } => self.generate_expr_stmt(span, expr),
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
        let from = from_expr.val.borrow().as_ref().unwrap().to_int().unwrap();
        let to = to_expr.val.borrow().as_ref().unwrap().to_int().unwrap();
        let step = if let Some(step_expr) = step_expr {
            step_expr.val.borrow().as_ref().unwrap().to_int().unwrap()
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
        write!(self.string, "if (").unwrap();
        self.generate_expr(expr);
        write!(self.string, " ").unwrap();
        self.generate_block(block_if_true);
        if let Some(block_if_false) = block_if_false {
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
        write_ident_and_ty(&mut self.string, ident, ty.borrow().as_ref().unwrap());
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
            string: self.string,
        }
        .generate_expr(expr)
    }

    fn write_indent(&mut self) {
        for _ in 0..self.indent_level {
            write!(self.string, "    ").unwrap();
        }
    }
}

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

fn write_ident_and_ty(string: &mut String, ident: Ident, ty: &Ty) {
    match *ty {
        Ty::Void => write!(string, "void {}", ident).unwrap(),
        Ty::Bool => write!(string, "bool {}", ident).unwrap(),
        Ty::Int => write!(string, "int {}", ident).unwrap(),
        Ty::Float => write!(string, "float {}", ident).unwrap(),
        Ty::Bvec2 => write!(string, "bvec2 {}", ident).unwrap(),
        Ty::Bvec3 => write!(string, "bvec3 {}", ident).unwrap(),
        Ty::Bvec4 => write!(string, "bvec4 {}", ident).unwrap(),
        Ty::Ivec2 => write!(string, "ivec2 {}", ident).unwrap(),
        Ty::Ivec3 => write!(string, "ivec3 {}", ident).unwrap(),
        Ty::Ivec4 => write!(string, "ivec4 {}", ident).unwrap(),
        Ty::Vec2 => write!(string, "vec2 {}", ident).unwrap(),
        Ty::Vec3 => write!(string, "vec3 {}", ident).unwrap(),
        Ty::Vec4 => write!(string, "vec4 {}", ident).unwrap(),
        Ty::Mat2 => write!(string, "mat2 {}", ident).unwrap(),
        Ty::Mat3 => write!(string, "mat3 {}", ident).unwrap(),
        Ty::Mat4 => write!(string, "mat4 {}", ident).unwrap(),
        Ty::Texture2d => panic!(),
        Ty::Array { ref elem_ty, len } => {
            write_ident_and_ty(string, ident, elem_ty);
            write!(string, "[{}]", len).unwrap();
        }
        Ty::Struct {
            ident: struct_ident,
        } => write!(string, "{} {}", struct_ident, ident).unwrap(),
    }
}
