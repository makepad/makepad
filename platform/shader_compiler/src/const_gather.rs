use{
    std::cell::Cell,
    crate::{
         makepad_live_compiler::{
            TokenSpan
        },
        shader_ast::*,
    }
};


#[derive(Clone)]
pub struct ConstGatherer<'a> {
    pub fn_def: &'a FnDef,
}

impl<'a> ConstGatherer<'a> {
    pub fn const_gather_expr(&self, expr: &Expr) {
        //let gather_span = if self.gather_all{Some(expr.span)}else{None};
        
        match expr.const_val.borrow().as_ref().unwrap() {
            Some(Val::Vec4(val)) => {
                expr.const_index.set(Some(
                    self.fn_def.const_table.borrow().as_ref().unwrap().len(),
                ));
                self.write_span(&expr.span, 4);
                self.write_f32(val.x);
                self.write_f32(val.y);
                self.write_f32(val.z);
                self.write_f32(val.w);
                return;
            }
            Some(Val::Float(val)) => {
                expr.const_index.set(Some(
                    self.fn_def.const_table.borrow().as_ref().unwrap().len(),
                ));
                self.write_span(&expr.span, 1);
                self.write_f32(*val);
                return;
            }
            _ => {},
        }

        match expr.kind {
            ExprKind::Cond {
                span,
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
            } => self.const_gather_cond_expr(span, expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                span,
                op,
                ref left_expr,
                ref right_expr,
            } => self.const_gather_bin_expr(span, op, left_expr, right_expr),
            ExprKind::Un { span, op, ref expr } => self.const_gather_un_expr(span, op, expr),
            ExprKind::MethodCall {
                ref arg_exprs,
                ..
            } => self.const_gather_all_call_expr(arg_exprs),
            ExprKind::PlainCall {
                ref arg_exprs,
                ..
            } => self.const_gather_all_call_expr(arg_exprs),
            ExprKind::BuiltinCall {
                ref arg_exprs,
                ..
            } => self.const_gather_all_call_expr(arg_exprs),
            /*ExprKind::ClosureCall {
                ref arg_exprs,
                ..
            } => self.const_gather_all_call_expr(arg_exprs),*/
            ExprKind::ClosureDef(_) => (),
            ExprKind::ConsCall {
                ref arg_exprs,
                ..
            } => self.const_gather_all_call_expr(arg_exprs),
            ExprKind::Field {
                span,
                ref expr,
                field_ident,
            } => self.const_gather_field_expr(span, expr, field_ident),
            ExprKind::Index {
                span,
                ref expr,
                ref index_expr,
            } => self.const_gather_index_expr(span, expr, index_expr),
            ExprKind::Var {
                span,
                ref kind,
                ..
            } => self.const_gather_var_expr(span, kind),
            ExprKind::StructCons{
                struct_ptr,
                span,
                ref args
            } => self.const_gather_struct_cons(struct_ptr, span, args),
            ExprKind::Lit { span, lit } => self.const_gather_lit_expr(span, lit),
        }
    }

    fn const_gather_cond_expr(
        &self,
        _span: TokenSpan,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr,
    ) {
        self.const_gather_expr(expr);
        self.const_gather_expr(expr_if_true);
        self.const_gather_expr(expr_if_false);
    }

    #[allow(clippy::float_cmp)]
    fn const_gather_bin_expr(&self, _span: TokenSpan, _op: BinOp, left_expr: &Expr, right_expr: &Expr) {
        self.const_gather_expr(left_expr);
        self.const_gather_expr(right_expr);
    }

    fn const_gather_un_expr(&self, _span: TokenSpan, _op: UnOp, expr: &Expr) {
        self.const_gather_expr(expr);
    }

    fn const_gather_field_expr(&self, _span: TokenSpan, expr: &Expr, _field_ident: Ident) {
        self.const_gather_expr(expr);
    }

    fn const_gather_index_expr(&self, _span: TokenSpan, expr: &Expr, _index_expr: &Expr) {
        self.const_gather_expr(expr);
    }

    fn const_gather_all_call_expr(&self, arg_exprs: &[Expr]) {
        for arg_expr in arg_exprs {
            self.const_gather_expr(arg_expr);
        }
    }

    fn const_gather_var_expr(&self, _span: TokenSpan, _kind: &Cell<Option<VarKind>>) {}

    fn const_gather_lit_expr(&self, _span: TokenSpan, _lit: Lit) {}

    fn const_gather_struct_cons(
        &self,
        _struct_ptr: StructPtr,
        _span: TokenSpan,
        args: &Vec<(Ident,Expr)>,
    ) {
        for arg in args{
            self.const_gather_expr(&arg.1);
        }
    }

    fn write_span(&self, span: &TokenSpan, slots:usize) {
        let index = self.fn_def.const_table.borrow().as_ref().unwrap().len();
        self.fn_def
            .const_table_spans
            .borrow_mut()
            .as_mut()
            .unwrap()
            .push(ConstTableSpan{
                token_id: span.token_id,
                offset: index,
                slots
            });            
    }

    fn write_f32(&self, val: f32) {
        self.fn_def
            .const_table
            .borrow_mut()
            .as_mut()
            .unwrap()
            .push(val);
    }
}
