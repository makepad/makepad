use{
    std::{
        cell::{Cell, RefCell},
        fmt,
        fmt::Write,
        collections::BTreeSet,
    },
    crate::{
        makepad_live_compiler::{
            id,
            makepad_math::PrettyPrintedF32,
            LiveId,
            TokenSpan
        },
        shader_ast::*,
        shader_registry::ShaderRegistry
    }
};


struct VoidWrap();
impl VoidWrap{
    pub fn unwrap(&self){}
}

macro_rules! write {
    ($dst:expr, $($arg:tt)*) => ({let _ =$dst.write_fmt(std::format_args!($($arg)*));VoidWrap()})
}

macro_rules! writeln {
    ($dst:expr $(,)?) => (
        write!($dst, "\n")
    );
    ($dst:expr, $($arg:tt)*) => (
        {let _ = $dst.write_fmt(std::format_args!($($arg)*));VoidWrap()}
    );
}

#[derive(Clone)]
pub struct ClosureSiteInfo<'a> {
    pub site_index: usize,
    pub closure_site: &'a ClosureSite,
    pub call_ptr: FnPtr
}

pub trait BackendWriter {
    fn needs_cstyle_struct_cons(&self)->bool;
    fn needs_mul_fn_for_matrix_multiplication(&self) -> bool;
    fn needs_unpack_for_matrix_multiplication(&self) -> bool;
    fn const_table_is_vec4(&self) -> bool;

    fn use_cons_fn(&self, what: &str) -> bool;

    fn write_var_decl(
        &self,
        string: &mut String,
        sep: &'static str,
        is_inout: bool,
        is_packed: bool,
        ident: &dyn fmt::Display,
        ty: &Ty,
    ) -> bool;
    
    fn write_call_expr_hidden_args(&self, string: &mut String, hidden_args:&BTreeSet<HiddenArgKind >, sep: &str);
    fn write_fn_def_hidden_params(&self, string: &mut String, hidden_args:&BTreeSet<HiddenArgKind >, sep: &str);
    
    fn generate_live_value_prefix(&self, string: &mut String);
    fn generate_draw_shader_field_expr(&self, string: &mut String, field_ident: Ident, ty:&Ty);
    
    fn write_ty_lit(&self, string: &mut String, ty_lit: TyLit);
    fn write_builtin_call_ident(&self, string: &mut String, ident: Ident, arg_exprs: &[Expr]);
    
}

pub struct BlockGenerator<'a> {
    pub fn_def: &'a FnDef,
    pub closure_site_info: Option<ClosureSiteInfo<'a >>,
    // pub env: &'a Env,
    pub shader_registry: &'a ShaderRegistry,
    pub backend_writer: &'a dyn BackendWriter,
    pub const_table_offset: Option<usize>,
    //pub use_generated_cons_fns: bool,
    pub indent_level: usize,
    pub string: &'a mut String,
}

    
pub fn generate_cons_fn(backend_writer:&dyn BackendWriter, string: &mut String, ty_lit: TyLit, param_tys: &[Ty]) {
    let mut cons_name = format!("consfn_{}", ty_lit);
    for param_ty in param_tys {
        write!(cons_name, "_{}", param_ty).unwrap();
    }
    if !backend_writer.use_cons_fn(&cons_name) {
        return
    }
    
    backend_writer.write_ty_lit(string, ty_lit);
    write!(string, " {}(", cons_name).unwrap();
    let mut sep = "";
    if param_tys.len() == 1 {
        backend_writer.write_var_decl(string, sep, false, false, &Ident(id!(x)), &param_tys[0]);
    } else {
        for (index, param_ty) in param_tys.iter().enumerate() {
            write!(string, "{}", sep).unwrap();
            backend_writer.write_var_decl(string, sep, false, false,&DisplaConstructorArg(index), param_ty);
            sep = ", ";
        }
    }
    writeln!(string, ") {{").unwrap();
    write!(string, "    return ").unwrap();
    backend_writer.write_ty_lit(string, ty_lit);
    write!(string, "(").unwrap();
    let ty = ty_lit.to_ty();
    if param_tys.len() == 1 {
        let param_ty = &param_tys[0];
        match param_ty {
            Ty::Bool | Ty::Int | Ty::Float => {
                let mut sep = "";
                for _ in 0..ty.slots() {
                    write!(string, "{}x", sep).unwrap();
                    sep = ", ";
                }
            }
            Ty::Mat2 | Ty::Mat3 | Ty::Mat4 => {
                let dst_size = match ty {
                    Ty::Mat2 => 2,
                    Ty::Mat3 => 3,
                    Ty::Mat4 => 4,
                    _ => panic!(),
                };
                let src_size = match param_ty {
                    Ty::Mat2 => 2,
                    Ty::Mat3 => 3,
                    Ty::Mat4 => 4,
                    _ => panic!(),
                };
                let mut sep = "";
                for col_index in 0..dst_size {
                    for row_index in 0..dst_size {
                        if row_index < src_size && col_index < src_size {
                            write!(string, "{}x[{}][{}]", sep, col_index, row_index)
                                .unwrap();
                        } else {
                            write!(
                                string,
                                "{}{}",
                                sep,
                                if col_index == row_index {1.0} else {0.0}
                            )
                                .unwrap();
                        }
                        sep = ", ";
                    }
                }
            }
            _ => panic!(),
        }
    } else {
        let mut sep = "";
        for (index_0, param_ty) in param_tys.iter().enumerate() {
            if param_ty.slots() == 1 {
                write!(string, "{}x{}", sep, index_0).unwrap();
                sep = ", ";
            } else {
                for index_1 in 0..param_ty.slots() {
                    write!(string, "{}x{}[{}]", sep, index_0, index_1).unwrap();
                    sep = ", ";
                }
            }
        }
    }
    writeln!(string, ");").unwrap();
    writeln!(string, "}}").unwrap();
}

impl<'a> BlockGenerator<'a> {
    pub fn generate_block(&mut self, block: &Block) {
        write!(self.string, "{{\n").unwrap();
        self.write_indent();
        if !block.stmts.is_empty() {
            self.indent_level += 1;
            for stmt in &block.stmts {
                self.generate_stmt(stmt);
                writeln!(self.string).unwrap();
            }
            self.indent_level -= 1;
            self.write_indent();
        }
        write!(self.string, "}}").unwrap();
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
            Stmt::Match {
                span,
                ref expr,
                ref matches,
            } => self.generate_match_stmt(span, expr, matches),
            Stmt::Let {
                span,
                ref ty,
                ident,
                ref ty_expr,
                ref expr,
                ref shadow
            } => self.generate_let_stmt(span, ty, ident, ty_expr, expr, shadow),
            Stmt::Return {span, ref expr} => self.generate_return_stmt(span, expr),
            Stmt::Block {span, ref block} => self.generate_block_stmt(span, block),
            Stmt::Expr {span, ref expr} => self.generate_expr_stmt(span, expr),
        }
    }
    
    fn generate_break_stmt(&mut self, _span: TokenSpan) {
        writeln!(self.string, "break;").unwrap();
    }
    
    fn generate_continue_stmt(&mut self, _span: TokenSpan) {
        writeln!(self.string, "continue;").unwrap();
    }
    
    fn generate_for_stmt(
        &mut self,
        _span: TokenSpan,
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
        _span: TokenSpan,
        expr: &Expr,
        block_if_true: &Block,
        block_if_false: &Option<Box<Block >>,
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
    
    fn generate_match_stmt(
        &mut self,
        _span: TokenSpan,
        expr: &Expr,
        matches: &Vec<Match>,
    ) {
        for (index,match_item) in matches.iter().enumerate(){
            if index != 0{
                write!(self.string, "else  ").unwrap();
            }

            write!(self.string, "if(").unwrap();
            self.generate_expr(expr);
            write!(self.string, " == {})", match_item.enum_value.get().unwrap()).unwrap();
            
            self.generate_block(&match_item.block);
        }
    }
    
    fn generate_let_stmt(
        &mut self,
        _span: TokenSpan,
        ty: &RefCell<Option<Ty >>,
        ident: Ident,
        _ty_expr: &Option<TyExpr>,
        expr: &Option<Expr>,
        shadow: &Cell<Option<ScopeSymShadow >>
    ) {
        self.backend_writer.write_var_decl(
            &mut self.string,
            "",
            false, 
            false,
            &DisplayVarName(ident, shadow.get().unwrap()),
            ty.borrow().as_ref().unwrap()
        );
        if let Some(expr) = expr {
            write!(self.string, " = ").unwrap();
            self.generate_expr(expr);
        }
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_return_stmt(&mut self, _span: TokenSpan, expr: &Option<Expr>) {
        write!(self.string, "return").unwrap();
        if let Some(expr) = expr {
            write!(self.string, " ").unwrap();
            self.generate_expr(expr);
        }
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_block_stmt(&mut self, _span: TokenSpan, block: &Block) {
        self.generate_block(block);
        writeln!(self.string).unwrap();
    }
    
    fn generate_expr_stmt(&mut self, _span: TokenSpan, expr: &Expr) {
        self.generate_expr(expr);
        writeln!(self.string, ";").unwrap();
    }
    
    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            closure_site_info: self.closure_site_info.clone(),
            fn_def: Some(self.fn_def),
            shader_registry: self.shader_registry,
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
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
    
    //fn write_var_decl(&mut self, is_inout: bool, is_packed: bool, ident: Ident, ty: &Ty) {
    //    self.backend_writer
    //       .write_var_decl(&mut self.string, is_inout, is_packed, &ident, ty);
    // }
}

pub struct ExprGenerator<'a> {
    pub fn_def: Option<&'a FnDef>,
    pub closure_site_info: Option<ClosureSiteInfo<'a >>,
    // pub env: &'a Env,
    pub shader_registry: &'a ShaderRegistry,
    pub backend_writer: &'a dyn BackendWriter,
    pub const_table_offset: Option<usize>,
    //pub use_hidden_params2: bool,
    //pub use_generated_cons_fns: bool,
    pub string: &'a mut String,
}

impl<'a> ExprGenerator<'a> {
    pub fn generate_expr(&mut self, in_expr: &Expr) {
        fn const_table_index_to_vec4(string: &mut String, index: usize) {
            let base = index >> 2;
            let sub = index - (base << 2);
            match sub {
                0 => write!(string, "[{}].x", base).unwrap(),
                1 => write!(string, "[{}].y", base).unwrap(),
                2 => write!(string, "[{}].z", base).unwrap(),
                _ => write!(string, "[{}].w", base).unwrap(),
            };
        }
        match (in_expr.const_val.borrow().as_ref(), in_expr.const_index.get()) {
            (Some(Some(Val::Vec4(_))), Some(mut index)) if self.const_table_offset.is_some() => {
                let const_table_offset = self.const_table_offset.unwrap();
                self.write_ty_lit(TyLit::Vec4);
                write!(self.string, "(").unwrap();
                let mut sep = "";
                for _ in 0..4 {
                    write!(self.string, "{}const_table", sep).unwrap();
                    if self.backend_writer.const_table_is_vec4() {
                        const_table_index_to_vec4(self.string, index + const_table_offset);
                    }
                    else {
                        write!(self.string, "[{}]", index + const_table_offset).unwrap();
                    }
                    sep = ", ";
                    index += 1;
                }
                write!(self.string, ")").unwrap();
            },
            (Some(Some(Val::Float(_))), Some(index)) if self.const_table_offset.is_some() => {
                let const_table_offset = self.const_table_offset.unwrap();
                write!(self.string, "const_table").unwrap();
                if self.backend_writer.const_table_is_vec4() {
                    const_table_index_to_vec4(self.string, index + const_table_offset);
                }
                else {
                    write!(self.string, "[{}]", index + const_table_offset).unwrap();
                }
            }
            // TODO: Extract the next three cases into a write_val function
            (Some(Some(Val::Vec4(val))), _) => {
                self.write_ty_lit(TyLit::Vec4);
                write!(
                    self.string,
                    "({}, {}, {}, {})",
                    PrettyPrintedF32(val.x),
                    PrettyPrintedF32(val.y),
                    PrettyPrintedF32(val.z),
                    PrettyPrintedF32(val.w),
                ).unwrap();
            }
            (Some(Some(Val::Float(val))), _) => {
                write!(self.string, "{}f", PrettyPrintedF32(*val)).unwrap();
            },
            (Some(Some(val)), _) => {
                write!(self.string, "{}", val).unwrap();
            },
            _ => match in_expr.kind {
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
                ExprKind::Field {
                    span,
                    ref expr,
                    field_ident,
                } => self.generate_field_expr(span, expr, field_ident, in_expr.ty.borrow().as_ref().unwrap()),
                ExprKind::Index {
                    span,
                    ref expr,
                    ref index_expr,
                } => self.generate_index_expr(span, expr, index_expr),
                ExprKind::MethodCall {
                    span,
                    ident,
                    ref arg_exprs,
                    ref closure_site_index,
                } => self.generate_method_call_expr(span, ident, arg_exprs, closure_site_index),
                ExprKind::PlainCall {
                    span,
                    fn_ptr,
                    ident,
                    ref arg_exprs,
                    ref closure_site_index,
                    ref param_index,
                } => self.generate_plain_call_expr(span, ident, fn_ptr, arg_exprs, closure_site_index, param_index),
                ExprKind::BuiltinCall {
                    span,
                    ident,
                    ref arg_exprs,
                } => self.generate_builtin_call_expr(span, ident, arg_exprs),
                /*ExprKind::ClosureCall {
                    span,
                    ident,
                    ref arg_exprs,
                    ref param_index,
                } => self.generate_closure_call_expr(span, arg_exprs, param_index),*/
                ExprKind::ClosureDef(_) => (),
                ExprKind::ConsCall {
                    span,
                    ty_lit,
                    ref arg_exprs,
                } => self.generate_cons_call_expr(span, ty_lit, arg_exprs),
                ExprKind::StructCons {
                    struct_ptr,
                    span,
                    ref args
                } => self.generate_struct_cons(struct_ptr, span, args),
                ExprKind::Var {
                    span,
                    ref kind,
                    ..
                } => self.generate_var_expr(span, kind, &in_expr.ty.borrow()),
                ExprKind::Lit {span, lit} => self.generate_lit_expr(span, lit),
            },
        }
    }
    
    fn generate_cond_expr(
        &mut self,
        _span: TokenSpan,
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
    
    fn generate_bin_expr(&mut self, _span: TokenSpan, op: BinOp, left_expr: &Expr, right_expr: &Expr) {
        
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
    
    fn generate_un_expr(&mut self, _span: TokenSpan, op: UnOp, expr: &Expr) {
        write!(self.string, "{}", op).unwrap();
        self.generate_expr(expr);
    }
    
    fn generate_method_call_expr(&mut self, _span: TokenSpan, ident: Ident, arg_exprs: &[Expr], closure_site_index: &Cell<Option<usize >>) {
        // alright so. what if we have
        // lets check if this is a call with closure args
        
        // ok so. what is expr
        match arg_exprs[0].ty.borrow().as_ref().unwrap() {
            Ty::Struct(struct_ptr) => {
                let fn_def = self.shader_registry.struct_method_decl_from_ident(
                    self.shader_registry.structs.get(struct_ptr).unwrap(),
                    ident
                ).unwrap();
                
                self.generate_call_body(_span, fn_def, arg_exprs, closure_site_index);
            }
            Ty::DrawShader(shader_ptr) => {
                let fn_def = self.shader_registry.draw_shader_method_decl_from_ident(
                    self.shader_registry.draw_shader_defs.get(shader_ptr).unwrap(),
                    ident
                ).unwrap();
                
                if fn_def.has_closure_args() {
                    // ok so..
                    
                }
                self.generate_call_body(_span, fn_def, &arg_exprs[1..], closure_site_index);
            }
            _ => panic!(),
        }
    }
    
    
    fn generate_call_body(&mut self, _span: TokenSpan, fn_def: &FnDef, arg_exprs: &[Expr], closure_site_index: &Cell<Option<usize >>) {
        // lets create a fn name for this thing.
        if let Some(closure_site_index) = closure_site_index.get() {
            // ok so.. we have closure args. this means we have a callsite
            let call_def = self.fn_def.unwrap();
            let closure_sites = &call_def.closure_sites.borrow();

            let closure_site = &closure_sites.as_ref().unwrap()[closure_site_index];
            // ok our function name is different now:
            
            // and then our args
            write!(self.string, "{} (", DisplayFnNameWithClosureArgs(
                closure_site_index,
                call_def.fn_ptr,
                fn_def.ident
            )).unwrap();
            
            let mut sep = "";
            for arg_expr in arg_exprs {
                // check if the args is a closure, ifso skip it
                match arg_expr.ty.borrow().as_ref().unwrap(){
                    Ty::ClosureDef(_)=>{
                        continue;
                    },
                    _=>()
                }
                
                write!(self.string, "{}", sep).unwrap();
                self.generate_expr(arg_expr);
                sep = ", ";
            }
            // and now the closed over values
            for sym in &closure_site.all_closed_over {
                match sym.ty{
                    Ty::DrawShader(_)=>{
                        continue;
                    }
                    Ty::ClosureDef(_) | Ty::ClosureDecl=>panic!(),
                    _=>()
                }
                write!(self.string, "{}", sep).unwrap();
                write!(self.string, "{}", DisplayVarName(sym.ident, sym.shadow)).unwrap();
                sep = ", ";
            }

            let mut merged_hidden_args = BTreeSet::new();
            merged_hidden_args.extend(fn_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
            merged_hidden_args.extend(call_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
            self.backend_writer.write_call_expr_hidden_args(self.string, &merged_hidden_args, sep);

            write!(self.string, ")").unwrap();
        }
        else {
            write!(self.string, "{}_{} (", fn_def.fn_ptr, fn_def.ident).unwrap();
            let mut sep = "";
            for arg_expr in arg_exprs {
                write!(self.string, "{}", sep).unwrap();
                self.generate_expr(arg_expr);
                sep = ", ";
            }

            self.backend_writer.write_call_expr_hidden_args(self.string, fn_def.hidden_args.borrow().as_ref().unwrap(), sep);

            write!(self.string, ")").unwrap();
        }
    }
    
    fn generate_field_expr(&mut self, _span: TokenSpan, expr: &Expr, field_ident: Ident, ty:&Ty) {
        match expr.ty.borrow().as_ref() {
            Some(Ty::DrawShader(_)) => {
                self.backend_writer.generate_draw_shader_field_expr(&mut self.string, field_ident, ty);
                //write!(self.string, "{}", &DisplayDsIdent(field_ident)).unwrap();
            }
            Some(Ty::Struct{..})=>{
                self.generate_expr(expr);
                write!(self.string, ".{}", &DisplayStructField(field_ident)).unwrap();
            }
            _=>{
                self.generate_expr(expr);
                write!(self.string, ".{}", field_ident).unwrap();
            }
        }
    }
    
    fn generate_struct_cons(
        &mut self,
        struct_ptr: StructPtr,
        _span: TokenSpan,
        args: &Vec<(Ident, Expr)>,
    ) {
        let struct_decl = self.shader_registry.structs.get(&struct_ptr).unwrap();
        let (sep1, sep2) = if self.backend_writer.needs_cstyle_struct_cons() { ("(",")")}else{("{","}")};

        write!(self.string, "{}{}", struct_ptr, sep1).unwrap();
        for (index, field) in struct_decl.fields.iter().enumerate() {
            if index != 0 {
                write!(self.string, ",").unwrap();
            }
            let arg = args.iter().find( | (ident, _) | field.ident == *ident).unwrap();
            self.generate_expr(&arg.1);
        }
        write!(self.string, "{}", sep2).unwrap();
    }
    
    fn generate_index_expr(&mut self, _span: TokenSpan, expr: &Expr, index_expr: &Expr) {
        self.generate_expr(expr);
        write!(self.string, "[").unwrap();
        self.generate_expr(index_expr);
        write!(self.string, "]").unwrap();
    }
    
    
    fn generate_builtin_call_expr(&mut self, _span: TokenSpan, ident: Ident, arg_exprs: &[Expr]) {
        // lets create a fn name for this thing.
        
        self.backend_writer.write_builtin_call_ident(&mut self.string, ident, arg_exprs);
        
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for arg_expr in arg_exprs {
            write!(self.string, "{}", sep).unwrap();
            
            self.generate_expr(arg_expr);
            
            sep = ", ";
        }
        
        write!(self.string, ")").unwrap();
    }
    
    
    fn generate_plain_call_expr(&mut self, _span: TokenSpan, _ident: Option<Ident>, fn_ptr: Option<FnPtr>, arg_exprs: &[Expr], closure_site_index: &Cell<Option<usize >>, param_index: &Cell<Option<usize >>) {
        // lets create a fn name for this thing.
        if param_index.get().is_some(){ // its a closure
            self.generate_closure_call_expr(_span, arg_exprs, param_index);
        }
        else{
            let fn_def = self.shader_registry.all_fns.get(&fn_ptr.unwrap()).unwrap();
            self.generate_call_body(_span, fn_def, arg_exprs, closure_site_index);
        }
    }
    
    
    fn generate_closure_call_expr(&mut self, _span: TokenSpan, arg_exprs: &[Expr], param_index: &Cell<Option<usize >>) {
        
        let param_index = param_index.get().unwrap();
        
        let closure_site_info = self.closure_site_info.as_ref().unwrap();
        // find our closure def
        let closure_def_index = closure_site_info.closure_site.closure_args.iter().find( | arg | arg.param_index == param_index).unwrap().closure_def_index;
        let call_def = self.shader_registry.all_fns.get(&closure_site_info.call_ptr).unwrap();
        let closure_def = &call_def.closure_defs[closure_def_index.0];
        
        write!(self.string, "{}", DisplayClosureName(closure_site_info.call_ptr, closure_def_index)).unwrap();
        
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for arg_expr in arg_exprs {
            write!(self.string, "{}", sep).unwrap();
            self.generate_expr(arg_expr);
            sep = ", ";
        }
        // alright now we have to pass in the closed over syms IN order
        for sym in closure_def.closed_over_syms.borrow().as_ref().unwrap() {
            if let Ty::DrawShader(_) = sym.ty {
                continue;
            }
            write!(self.string, "{}", sep).unwrap();
            write!(self.string, "{}", DisplayClosedOverArg(sym.ident, sym.shadow)).unwrap();
            sep = ", ";
        }
        
        // we need to merge both the fn_def as well as the call_ptr
        let mut merged_hidden_args = BTreeSet::new();
        merged_hidden_args.extend(self.fn_def.unwrap().hidden_args.borrow().as_ref().unwrap().iter().cloned());
        merged_hidden_args.extend(call_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
        self.backend_writer.write_call_expr_hidden_args(self.string, &merged_hidden_args, sep);
        
        write!(self.string, ")").unwrap();
        
    }
    
    fn generate_macro_call_expr(
        &mut self,
        _analysis: &Cell<Option<MacroCallAnalysis >>,
        _span: TokenSpan,
        _ident: Ident,
        _arg_exprs: &[Expr],
    ) {
        
    }
    
    fn generate_cons_call_expr(&mut self, _span: TokenSpan, ty_lit: TyLit, arg_exprs: &[Expr]) {
        // lets build the constructor name
        let mut cons_name = format!("consfn_{}", ty_lit);
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
    
    fn generate_var_expr(&mut self, _span: TokenSpan, kind: &Cell<Option<VarKind >>, _ty: &Option<Ty>) {
        // ok so we have a few varkinds
        match kind.get().unwrap() {
            VarKind::Local {ident, shadow} => {
                write!(self.string, "{}", DisplayVarName(ident, shadow)).unwrap();
            }
            VarKind::MutLocal {ident, shadow} => {
                write!(self.string, "{}", DisplayVarName(ident, shadow)).unwrap();
            }
            VarKind::LiveValue(value_node_ptr) => {
                // this is a live value.. also prefix needed
                self.backend_writer.generate_live_value_prefix(self.string);
                write!(self.string, "{}", value_node_ptr).unwrap();
            }
        }
    }
    
    fn generate_lit_expr(&mut self, _span: TokenSpan, lit: Lit) {
        write!(self.string, "{}", lit).unwrap();
    }
    
    //fn write_ident(&mut self, ident: Ident) {
    //    self.backend_writer.write_ident(&mut self.string, ident);
    //}
    
    fn write_ty_lit(&mut self, ty_lit: TyLit) {
        self.backend_writer.write_ty_lit(&mut self.string, ty_lit);
    }
}

pub struct FnDefGenerator<'a> {
    pub fn_def: &'a FnDef,
    pub shader_registry: &'a ShaderRegistry,
    pub const_table_offset: Option<usize>,
    pub string: &'a mut String,
    pub backend_writer: &'a dyn BackendWriter
}

impl<'a> FnDefGenerator<'a> {
    pub fn generate_fn_def(&mut self) {
        
        self.backend_writer.write_var_decl(
            &mut self.string,
            "",
            false,
            false,
            &DisplayFnName(self.fn_def.fn_ptr, self.fn_def.ident), // here we must expand IdentPath to something
            self.fn_def.return_ty.borrow().as_ref().unwrap()
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &self.fn_def.params {
            if !param.shadow.get().is_none() {
                if self.backend_writer.write_var_decl(
                    &mut self.string,
                    sep,
                    param.is_inout,
                    false,
                    &DisplayVarName(param.ident, param.shadow.get().unwrap()),
                    param.ty_expr.ty.borrow().as_ref().unwrap(),
                ) {
                    sep = ", ";
                }
            }
        }
        self.backend_writer.write_fn_def_hidden_params(self.string, self.fn_def.hidden_args.borrow().as_ref().unwrap(), sep);
        write!(self.string, ") ").unwrap();
        self.generate_block(&self.fn_def.block);
        writeln!(self.string).unwrap();
        //self.visited.insert(self.decl.ident_path);
    }
    
    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: None,
            //env: self.env,
            fn_def: self.fn_def,
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
}

pub struct FnDefWithClosureArgsGenerator<'a> {
    pub closure_site_info: ClosureSiteInfo<'a>,
    pub fn_def: &'a FnDef,
    pub call_def: &'a FnDef,
    pub shader_registry: &'a ShaderRegistry,
    pub const_table_offset: Option<usize>,
    pub string: &'a mut String,
    pub backend_writer: &'a dyn BackendWriter
}


impl<'a> FnDefWithClosureArgsGenerator<'a> {
    pub fn generate_fn_def_with_all_closures(
        string:&mut String,
        shader_registry: &ShaderRegistry,
        fn_def: &FnDef,
        call_def: &FnDef,
        backend_writer: &dyn BackendWriter,
        const_table_offset: Option<usize>
    ) {
        // so first we are collecting the closures in defs that are actually used
        for (closure_def_index, closure_def) in call_def.closure_defs.iter().enumerate() {
            let closure_def_index = ClosureDefIndex(closure_def_index);
            for site in call_def.closure_sites.borrow().as_ref().unwrap() {
                if site.call_to == fn_def.fn_ptr { // alright this site calls the fn_def
                    for closure_site_arg in &site.closure_args {
                        if closure_site_arg.closure_def_index == closure_def_index {
                            // alright lets generate this closure
                            ClosureDefGenerator {
                                closure_site_arg: *closure_site_arg,
                                closure_def,
                                fn_def,
                                call_def,
                                shader_registry: shader_registry,
                                //env:self.env,
                                const_table_offset,
                                backend_writer,
                                string: string,
                            }
                            .generate_fn_def()
                        }
                    }
                }
            }
        }
        
        for (site_index, closure_site) in call_def.closure_sites.borrow().as_ref().unwrap().iter().enumerate() {
            // for each site
            if closure_site.call_to == fn_def.fn_ptr { // alright this site calls the fn_def
                // alright we need a fn def for this site_index
                FnDefWithClosureArgsGenerator {
                    closure_site_info: ClosureSiteInfo {
                        site_index,
                        closure_site,
                        call_ptr: call_def.fn_ptr,
                    },
                    fn_def,
                    call_def,
                    shader_registry,
                    //env:self.env,
                    const_table_offset,
                    backend_writer,
                    string,
                }
                .generate_fn_def_with_closure_args()
            }
        }
    }
    
    pub fn generate_fn_def_with_closure_args(&mut self) {
        
        self.backend_writer.write_var_decl(
            &mut self.string,
            "",
            false,
            false,
            &DisplayFnNameWithClosureArgs(
                self.closure_site_info.site_index,
                self.call_def.fn_ptr,
                self.fn_def.ident
            ), // here we must expand IdentPath to something
            self.fn_def.return_ty.borrow().as_ref().unwrap()
        );
        write!(self.string, "(").unwrap();
        let mut sep = "";
        for param in &self.fn_def.params {
            if !param.shadow.get().is_none() {
                if self.backend_writer.write_var_decl(
                    &mut self.string,
                    sep,
                    param.is_inout,
                    false,
                    &DisplayVarName(param.ident, param.shadow.get().unwrap()),
                    param.ty_expr.ty.borrow().as_ref().unwrap(),
                ) {
                    sep = ", ";
                }
            }
        }
        // now we iterate over the closures in our site,
        // and we need to merge the set of closed over args.
        for sym in &self.closure_site_info.closure_site.all_closed_over {
            if self.backend_writer.write_var_decl(
                &mut self.string,
                sep,
                false,
                false,
                &DisplayClosedOverArg(sym.ident, sym.shadow),
                &sym.ty,
            ) {
                sep = ", ";
            }
        }
        // we need the union of the call def and the fn def
        let mut merged_hidden_args = BTreeSet::new();
        merged_hidden_args.extend(self.fn_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
        merged_hidden_args.extend(self.call_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
        self.backend_writer.write_fn_def_hidden_params(self.string, &merged_hidden_args, sep);
        
        write!(self.string, ") ").unwrap();
        // alright so here the block is generated.. however
        // we need to know the names and the closed-over-args passthrough
        self.generate_block(&self.fn_def.block);
        
        
        writeln!(self.string).unwrap();
        //self.visited.insert(self.decl.ident_path);
    }
    
    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: Some(self.closure_site_info.clone()),
            //env: self.env,
            fn_def: self.fn_def,
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
}

pub struct ClosureDefGenerator<'a> {
    pub closure_def: &'a ClosureDef,
    pub closure_site_arg: ClosureSiteArg,
    pub fn_def: &'a FnDef,
    pub call_def: &'a FnDef,
    pub shader_registry: &'a ShaderRegistry,
    pub const_table_offset: Option<usize>,
    pub string: &'a mut String,
    pub backend_writer: &'a dyn BackendWriter
}

impl<'a> ClosureDefGenerator<'a> {
    pub fn generate_fn_def(&mut self) {
        
        let fn_param = &self.fn_def.params[self.closure_site_arg.param_index];
        
        let mut sep = "";
        
        if let TyExprKind::ClosureDecl {params, return_ty, ..} = &fn_param.ty_expr.kind {
            
            self.backend_writer.write_var_decl(
                &mut self.string,
                "",
                false,
                false,
                &DisplayClosureName(self.call_def.fn_ptr, self.closure_site_arg.closure_def_index), // here we must expand IdentPath to something
                return_ty.borrow().as_ref().unwrap(),
            );
            write!(self.string, "(").unwrap();
            
            // ok we have now params and names
            for (param_index, param) in params.iter().enumerate() {
                // lets fetch the name of this thing
                let closure_param = &self.closure_def.params[param_index];
                let shadow = closure_param.shadow.get().unwrap();
                if self.backend_writer.write_var_decl(
                    &mut self.string,
                    sep,
                    param.is_inout,
                    false,
                    &DisplayVarName(closure_param.ident, shadow),
                    param.ty_expr.ty.borrow().as_ref().unwrap(),
                ) {
                    sep = ", ";
                }
            }
        }
        else {
            panic!()
        }
        
        for sym in self.closure_def.closed_over_syms.borrow().as_ref().unwrap() {
            if self.backend_writer.write_var_decl(
                &mut self.string,
                sep,
                false,
                false,
                &DisplayVarName(sym.ident, sym.shadow),
                &sym.ty,
            ) {
                sep = ", ";
            }
        } 

        let mut merged_hidden_args = BTreeSet::new();
        merged_hidden_args.extend(self.fn_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
        merged_hidden_args.extend(self.call_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
        self.backend_writer.write_fn_def_hidden_params(self.string, &merged_hidden_args, sep);
        
        writeln!(self.string, ") {{").unwrap();
        
        match &self.closure_def.kind {
            ClosureDefKind::Expr(expr) => {
                write!(self.string, "    return ").unwrap();
                self.generate_expr(expr);
                writeln!(self.string, ";").unwrap();
                writeln!(self.string, "}}").unwrap();
            }
            ClosureDefKind::Block(block) => {
                self.generate_block(block);
                writeln!(self.string).unwrap();
            }
        }
        //self.visited.insert(self.decl.ident_path);
    }
    
    fn generate_block(&mut self, block: &Block) {
        BlockGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: None,
            //env: self.env,
            fn_def: self.fn_def,
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            indent_level: 0,
            string: self.string,
        }
        .generate_block(block)
    }
    
    
    fn generate_expr(&mut self, expr: &Expr) {
        ExprGenerator {
            shader_registry: self.shader_registry,
            closure_site_info: None,
            //env: self.env,
            fn_def: Some(self.fn_def),
            backend_writer: self.backend_writer,
            const_table_offset: self.const_table_offset,
            string: self.string,
        }
        .generate_expr(expr)
    }
}

pub struct DisplayDsIdent(pub Ident);
impl fmt::Display for DisplayDsIdent {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "ds_{}", self.0);
        fmt::Result::Ok(())
    }
}

pub struct DisplayPadding(pub usize);
impl fmt::Display for DisplayPadding {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "pad_{}", self.0);
        fmt::Result::Ok(())
    }
}

pub struct DisplayStructField(pub Ident);
impl fmt::Display for DisplayStructField {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "f_{}", self.0);
        fmt::Result::Ok(())
    }
}

pub struct DisplayFnName(pub FnPtr, pub Ident);
impl fmt::Display for DisplayFnName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "{}_{}", self.0, self.1);
        fmt::Result::Ok(())
    }
}

pub struct DisplayFnNameWithClosureArgs(pub usize, pub FnPtr, pub Ident);
impl fmt::Display for DisplayFnNameWithClosureArgs {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "site_{}_of_{}_{}", self.0, self.1, self.2);
        fmt::Result::Ok(())
    }
}

pub struct DisplayClosureName(pub FnPtr, pub ClosureDefIndex);
impl fmt::Display for DisplayClosureName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "closure_{}_in_{}", self.1.0, self.0);
        fmt::Result::Ok(())
    }
}

pub struct DisplayVarName(pub Ident, pub ScopeSymShadow);
impl fmt::Display for DisplayVarName {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "var_{}_{}", self.0, self.1.0);
        fmt::Result::Ok(())
    }
}

pub struct DisplayClosedOverArg(pub Ident, pub ScopeSymShadow);
impl fmt::Display for DisplayClosedOverArg {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "pass_{}_{}", self.0, self.1.0);
        fmt::Result::Ok(())
    }
}

pub struct DisplaConstructorArg(pub usize);
impl fmt::Display for DisplaConstructorArg {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        write!(f, "x{}", self.0);
        fmt::Result::Ok(())
    }
}
