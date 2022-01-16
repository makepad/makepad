use{
    std::cell::Cell,
    crate::{
        makepad_live_compiler::*,
        shader_ast::*,
        shader_registry::ShaderRegistry
    }
};

pub struct LhsChecker<'a> {
    pub scopes: &'a Scopes,
    pub shader_registry: &'a ShaderRegistry,
}

impl<'a> LhsChecker<'a> {
    pub fn lhs_check_expr(&mut self, expr: &Expr) -> Result<(), LiveError> {
        match expr.kind {
            ExprKind::Cond {
                span,
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
                ..
            } => self.lhs_check_cond_expr(span, expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                span,
                op,
                ref left_expr,
                ref right_expr,
                ..
            } => self.lhs_check_bin_expr(span, op, left_expr, right_expr),
            ExprKind::Un {span, op, ref expr} => self.lhs_check_un_expr(span, op, expr),
            ExprKind::Field {
                span,
                ref expr,
                field_ident,
            } => self.lhs_check_field_expr(span, expr, field_ident),
            ExprKind::Index {
                span,
                ref expr,
                ref index_expr,
            } => self.lhs_check_index_expr(span, expr, index_expr),
            ExprKind::MethodCall {
                span,
                ..
            } => self.lhs_check_all_call_expr(span),
            ExprKind::PlainCall {
                span,
                ..
            } => self.lhs_check_all_call_expr(span),
           /* ExprKind::ClosureCall {
                span,
                ..
            } => self.lhs_check_all_call_expr(span),*/
            ExprKind::ClosureDef(_) => self.lhs_check_closure(expr.span),
            ExprKind::BuiltinCall {
                span,
                ..
            } => self.lhs_check_all_call_expr(span),
            ExprKind::ConsCall {
                span,
                ..
            } => self.lhs_check_all_call_expr(span),
            ExprKind::StructCons {
                span,
                ..
            } => self.lhs_check_all_call_expr(span),
            ExprKind::Var {
                span,
                ref kind,
                ..
            } => self.lhs_check_var_expr(span, kind),
            ExprKind::Lit {span, lit} => self.lhs_check_lit_expr(span, lit),
        }
    }
    
    fn lhs_check_closure(
        &mut self,
        span: TokenSpan,
    ) -> Result<(), LiveError> {
        return Err(LiveError {
            origin: live_error_origin!(),
            span:span.into(),
            message: String::from("expression is not a valid left hand side"),
        });
    }
    
    fn lhs_check_cond_expr(
        &mut self,
        span: TokenSpan,
        _expr: &Expr,
        _expr_if_true: &Expr,
        _expr_if_false: &Expr,
    ) -> Result<(), LiveError> {
        return Err(LiveError {
            origin: live_error_origin!(),
            span:span.into(),
            message: String::from("expression is not a valid left hand side"),
        });
    }
    
    fn lhs_check_bin_expr(
        &mut self,
        span: TokenSpan,
        _op: BinOp,
        _left_expr: &Expr,
        _right_expr: &Expr,
    ) -> Result<(), LiveError> {
        return Err(LiveError {
            origin:live_error_origin!(),
            span:span.into(),
            message: String::from("expression is not a valid left hand side"),
        });
    }
    
    fn lhs_check_un_expr(&mut self, span: TokenSpan, _op: UnOp, _expr: &Expr) -> Result<(), LiveError> {
        return Err(LiveError {
            origin:live_error_origin!(),
            span:span.into(),
            message: String::from("expression is not a valid left hand side"),
        });
    }
    
    fn lhs_check_all_call_expr(
        &mut self,
        span: TokenSpan,
    ) -> Result<(), LiveError> {
        return Err(LiveError {
            origin:live_error_origin!(),
            span:span.into(),
            message: String::from("expression is not a valid left hand side"),
        });
    }
    
    fn lhs_check_field_expr(
        &mut self,
        span: TokenSpan,
        expr: &Expr,
        field_ident: Ident,
    ) -> Result<(), LiveError> {
        // lets grab the ty from expr
        match expr.ty.borrow().as_ref().unwrap(){
            Ty::DrawShader(shader_ptr)=>{
                let field_decl = self.shader_registry.draw_shader_defs.get(shader_ptr).unwrap().find_field(field_ident) .unwrap();
                match &field_decl.kind{
                    DrawShaderFieldKind::Varying{..}=>{
                        Ok(())
                    }
                    _=>{
                        Err(LiveError {
                            origin:live_error_origin!(),
                            span:span.into(),
                            message: String::from("Can only assign to varying values for shader self"),
                        })
                    }
                }
            }
            _=>{
                 self.lhs_check_expr(expr)
            }
        }
    }
    
    fn lhs_check_index_expr(
        &mut self,
        _span: TokenSpan,
        expr: &Expr,
        _index_expr: &Expr,
    ) -> Result<(), LiveError> {
        self.lhs_check_expr(expr)
    }
    
    fn lhs_check_call_expr(
        &mut self,
        span: TokenSpan,
        _ident_path: IdentPath,
        _arg_exprs: &[Expr],
    ) -> Result<(), LiveError> {
        return Err(LiveError {
            origin:live_error_origin!(),
            span:span.into(),
            message: String::from("expression is not a valid left hand side"),
        });
    }
    
    fn lhs_check_var_expr(
        &mut self,
        span: TokenSpan,
        kind: &Cell<Option<VarKind >>,
    ) -> Result<(), LiveError> {
        if let VarKind::MutLocal{..} = kind.get().unwrap(){
            Ok(())
        }
        else{
            Err(LiveError {
                origin:live_error_origin!(),
                span:span.into(),
                message: String::from("expression is not a valid left hand side"),
            })
        }
    }
    /*
    fn lhs_check_live_id_expr(
        &mut self,
        span: Span,
        _kind: &Cell<Option<VarKind>>,
        _id:LiveItemId,
        _ident: Ident,
    ) -> Result<(), LiveError> {
        return Err(LiveError {
            span,
            message: String::from("liveid is not a valid left hand side"),
        });
    }*/
    
    fn lhs_check_lit_expr(&mut self, _span: TokenSpan, _lit: Lit) -> Result<(), LiveError> {
        Ok(())
    }
}
