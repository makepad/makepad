#![allow(unused_variables)]
use crate::shaderast::*;
use crate::shaderast::Scopes;
use makepad_live_parser::LiveError;
use makepad_live_parser::LiveErrorOrigin;
use makepad_live_parser::live_error_origin;
use crate::shaderast::Ident;
use crate::lhs_check::LhsChecker;
use crate::shaderast::Lit;
use makepad_live_parser::Span;
use crate::swizzle::Swizzle;
use crate::shaderast::{Ty, TyLit, TyExprKind, TyExpr};
use crate::util::CommaSep;
use std::cell::Cell;
use std::fmt::Write;
use std::rc::Rc;
use crate::shaderregistry::ShaderRegistry;
use crate::shaderast::Sym;

#[derive(Clone, Debug)]
pub struct TyChecker<'a> {
    pub scopes: &'a Scopes,
    pub shader_registry: &'a ShaderRegistry
}

impl<'a> TyChecker<'a> {
    fn lhs_checker(&self) -> LhsChecker {
        LhsChecker {scopes: self.scopes, shader_registry: self.shader_registry,}
    }
    
    pub fn ty_check_ty_expr(&mut self, ty_expr: &TyExpr) -> Result<Ty, LiveError> {
        let ty = match &ty_expr.kind {
            TyExprKind::Array {
                ref elem_ty_expr,
                len,
            } => self.ty_check_array_ty_expr(ty_expr.span, elem_ty_expr, *len),
            TyExprKind::Lit {ty_lit} => self.ty_check_lit_ty_expr(ty_expr.span, *ty_lit),
            TyExprKind::Struct(struct_ptr) => Ok(Ty::Struct(*struct_ptr)),
            TyExprKind::DrawShader(shader_ptr) => Ok(Ty::DrawShader(*shader_ptr)),
            TyExprKind::ClosureDecl {return_ty_expr, params, return_ty} => {
                // check the closure
                let checked_return_ty = self.ty_check_ty_expr(return_ty_expr.as_ref().as_ref().unwrap()).unwrap_or(Ty::Void);
                *return_ty.borrow_mut() = Some(checked_return_ty);
                for param in params {
                    self.ty_check_ty_expr(&param.ty_expr) ?;
                }
                Ok(Ty::ClosureDecl)
            }
        } ?;
        *ty_expr.ty.borrow_mut() = Some(ty.clone());
        Ok(ty)
    }
    
    fn ty_check_array_ty_expr(
        &mut self,
        _span: Span,
        elem_ty_expr: &TyExpr,
        len: u32,
    ) -> Result<Ty, LiveError> {
        let elem_ty = Rc::new(self.ty_check_ty_expr(elem_ty_expr) ?);
        let len = len as usize;
        Ok(Ty::Array {elem_ty, len})
    }
    
    fn ty_check_lit_ty_expr(&mut self, _span: Span, ty_lit: TyLit) -> Result<Ty, LiveError> {
        Ok(ty_lit.to_ty())
    }
    
    pub fn ty_check_expr_with_expected_ty(
        &mut self,
        span: Span,
        expr: &Expr,
        expected_ty: &Ty,
    ) -> Result<Ty, LiveError> {
        let actual_ty = self.ty_check_expr(expr) ?;
        if &actual_ty != expected_ty {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: format!(
                    "can't match expected type `{}` with actual type `{}",
                    expected_ty,
                    actual_ty
                ),
            });
        }
        Ok(actual_ty)
    }
    
    pub fn ty_check_expr(&mut self, expr: &Expr) -> Result<Ty, LiveError> {
        let ty = match expr.kind {
            ExprKind::Cond {
                span,
                ref expr,
                ref expr_if_true,
                ref expr_if_false,
                ..
            } => self.ty_check_cond_expr(span, expr, expr_if_true, expr_if_false),
            ExprKind::Bin {
                span,
                op,
                ref left_expr,
                ref right_expr,
                ..
            } => self.ty_check_bin_expr(span, op, left_expr, right_expr),
            ExprKind::Un {span, op, ref expr} => self.ty_check_un_expr(span, op, expr),
            ExprKind::PlainCall {
                span,
                //ident,
                fn_node_ptr,
                ref arg_exprs,
            } => self.ty_check_plain_call_expr(span, arg_exprs, fn_node_ptr),
            ExprKind::MethodCall {
                span,
                ident,
                ref arg_exprs,
            } => self.ty_check_method_call_expr(span, ident, arg_exprs),
            ExprKind::BuiltinCall {
                span,
                ident,
                ref arg_exprs,
            } => self.ty_check_builtin_call_expr(span, ident, arg_exprs),
            ExprKind::ClosureCall {
                span,
                ident,
                ref arg_exprs,
            } => self.ty_check_closure_call_expr(span, ident, arg_exprs),
            ExprKind::ClosureDef(index) => self.ty_check_closure_def(index),
            ExprKind::ConsCall {
                span,
                ty_lit,
                ref arg_exprs,
            } => self.ty_check_cons_call_expr(span, ty_lit, arg_exprs),
            ExprKind::Field {
                span,
                ref expr,
                field_ident,
            } => self.ty_check_field_expr(span, expr, field_ident),
            ExprKind::Index {
                span,
                ref expr,
                ref index_expr,
            } => self.ty_check_index_expr(span, expr, index_expr),
            
            ExprKind::Var {
                span,
                ref kind,
                var_resolve,
            } => self.ty_check_var_expr(span, kind, var_resolve),
            ExprKind::StructCons {
                struct_node_ptr,
                span,
                ref args
            } => self.ty_check_struct_cons(struct_node_ptr, span, args),
            ExprKind::Lit {span, lit} => self.ty_check_lit_expr(span, lit),
        } ?;
        *expr.ty.borrow_mut() = Some(ty.clone());
        Ok(ty)
    }
    
    fn ty_check_closure_def(
        &mut self,
        index: ClosureDefIndex,
    ) -> Result<Ty, LiveError> {
        Ok(Ty::ClosureDef(index))
    }
    
    fn ty_check_cond_expr(
        &mut self,
        span: Span,
        expr: &Expr,
        expr_if_true: &Expr,
        expr_if_false: &Expr,
    ) -> Result<Ty, LiveError> {
        self.ty_check_expr_with_expected_ty(span, expr, &Ty::Bool) ?;
        let ty_if_true = self.ty_check_expr(expr_if_true) ?;
        self.ty_check_expr_with_expected_ty(span, expr_if_false, &ty_if_true) ?;
        Ok(ty_if_true)
    }
    
    fn ty_check_bin_expr(
        &mut self,
        span: Span,
        op: BinOp,
        left_expr: &Expr,
        right_expr: &Expr,
    ) -> Result<Ty, LiveError> {
        let left_ty = self.ty_check_expr(left_expr) ?;
        let right_ty = self.ty_check_expr(right_expr) ?;
        match op {
            BinOp::Assign
                | BinOp::AddAssign
                | BinOp::SubAssign
                | BinOp::MulAssign
                | BinOp::DivAssign => {
                self.lhs_checker().lhs_check_expr(left_expr) ?;
            }
            _ => {}
        }
        match op {
            BinOp::Assign => {
                if left_ty == right_ty {
                    Some(left_ty.clone())
                } else {
                    None
                }
            }
            BinOp::AddAssign | BinOp::SubAssign | BinOp::DivAssign => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
            BinOp::MulAssign => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Mat2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Mat3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Mat4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
            BinOp::Or | BinOp::And => match (&left_ty, &right_ty) {
                (Ty::Bool, Ty::Bool) => Some(Ty::Bool),
                _ => None,
            },
            BinOp::Eq | BinOp::Ne => match (&left_ty, &right_ty) {
                (Ty::Bool, Ty::Bool) => Some(Ty::Bool),
                (Ty::Int, Ty::Int) => Some(Ty::Bool),
                (Ty::Float, Ty::Float) => Some(Ty::Bool),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Bool),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Bool),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Bool),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Bool),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Bool),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Bool),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Bool),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Bool),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Bool),
                _ => None,
            },
            BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Bool),
                (Ty::Float, Ty::Float) => Some(Ty::Bool),
                _ => None,
            },
            BinOp::Add | BinOp::Sub | BinOp::Div => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Float, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Float, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Float, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Float, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Float, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Float, Ty::Mat4) => Some(Ty::Mat4),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
            BinOp::Mul => match (&left_ty, &right_ty) {
                (Ty::Int, Ty::Int) => Some(Ty::Int),
                (Ty::Float, Ty::Float) => Some(Ty::Float),
                (Ty::Float, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Float, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Float, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Float, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Float, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Float, Ty::Mat4) => Some(Ty::Mat4),
                (Ty::Ivec2, Ty::Int) => Some(Ty::Ivec2),
                (Ty::Ivec2, Ty::Ivec2) => Some(Ty::Ivec2),
                (Ty::Ivec3, Ty::Int) => Some(Ty::Ivec3),
                (Ty::Ivec3, Ty::Ivec3) => Some(Ty::Ivec3),
                (Ty::Ivec4, Ty::Int) => Some(Ty::Ivec4),
                (Ty::Ivec4, Ty::Ivec4) => Some(Ty::Ivec4),
                (Ty::Vec2, Ty::Float) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Vec2, Ty::Mat2) => Some(Ty::Vec2),
                (Ty::Vec3, Ty::Float) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Vec3, Ty::Mat3) => Some(Ty::Vec3),
                (Ty::Vec4, Ty::Float) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Vec4, Ty::Mat4) => Some(Ty::Vec4),
                (Ty::Mat2, Ty::Float) => Some(Ty::Mat2),
                (Ty::Mat2, Ty::Vec2) => Some(Ty::Vec2),
                (Ty::Mat2, Ty::Mat2) => Some(Ty::Mat2),
                (Ty::Mat3, Ty::Float) => Some(Ty::Mat3),
                (Ty::Mat3, Ty::Vec3) => Some(Ty::Vec3),
                (Ty::Mat3, Ty::Mat3) => Some(Ty::Mat3),
                (Ty::Mat4, Ty::Float) => Some(Ty::Mat4),
                (Ty::Mat4, Ty::Vec4) => Some(Ty::Vec4),
                (Ty::Mat4, Ty::Mat4) => Some(Ty::Mat4),
                _ => None,
            },
        }
        .ok_or_else( || LiveError {
            origin: live_error_origin!(),
            span,
            message: format!(
                "can't apply binary operator `{}` to operands of type `{}` and `{}",
                op,
                left_ty,
                right_ty
            )
                .into(),
        })
    }
    
    fn ty_check_un_expr(&mut self, span: Span, op: UnOp, expr: &Expr) -> Result<Ty, LiveError> {
        let ty = self.ty_check_expr(expr) ?;
        match op {
            UnOp::Not => match ty {
                Ty::Bool => Some(Ty::Bool),
                _ => None,
            },
            UnOp::Neg => match ty {
                Ty::Int => Some(Ty::Int),
                Ty::Float => Some(Ty::Float),
                Ty::Vec2 => Some(Ty::Vec2),
                Ty::Vec3 => Some(Ty::Vec3),
                Ty::Vec4 => Some(Ty::Vec4),
                _ => None,
            },
        }
        .ok_or_else( || LiveError {
            origin: live_error_origin!(),
            span,
            message: format!(
                "can't apply unary operator `{}` to operand of type `{}`",
                op,
                ty
            )
                .into(),
        })
    }
    
    fn ty_check_plain_call_expr(
        &mut self,
        span: Span,
        //ident: Ident,
        arg_exprs: &[Expr],
        fn_node_ptr: FnNodePtr,
    ) -> Result<Ty, LiveError> {

        for arg_expr in arg_exprs {
            self.ty_check_expr(arg_expr) ?;
        }

        // alright so.
        let fn_decl = self.shader_registry.all_fns.get(&fn_node_ptr).expect("fn ptr invalid");
        
        self.check_call_args(span, fn_node_ptr, arg_exprs, &fn_decl) ?;
        
        // lets return the right ty
        return Ok(fn_decl.return_ty.borrow().clone().unwrap())
    }
    
    fn ty_check_method_call_expr(
        &mut self,
        span: Span,
        ident: Ident,
        arg_exprs: &[Expr],
    ) -> Result<Ty, LiveError> {
        
        let ty = self.ty_check_expr(&arg_exprs[0]) ?;
        match ty {
            Ty::DrawShader(shader_ptr) => { // a shader method call
                
                if let Some(fn_decl) = self.shader_registry.draw_shader_method_decl_from_ident(
                    self.shader_registry.draw_shaders.get(&shader_ptr).unwrap(),
                    ident
                ) {

                    for arg_expr in arg_exprs {
                        self.ty_check_expr(arg_expr) ?;
                    }

                    self.check_call_args(span, fn_decl.fn_node_ptr, arg_exprs, fn_decl) ?;
                    
                    if let Some(return_ty) = fn_decl.return_ty.borrow().clone() {
                        return Ok(return_ty);
                    }
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span,
                        message: format!("shader method `{}` is not type checked `{}`", ident, ty),
                    });
                }
            },
            Ty::Struct(struct_ptr) => {
                //println!("GOT STRUCT {:?}", struct_ptr);
                // ok lets find 'ident' on struct_ptr
                if let Some(fn_decl) = self.shader_registry.struct_method_decl_from_ident(
                    self.shader_registry.structs.get(&struct_ptr).unwrap(),
                    ident
                ) {
                    for arg_expr in arg_exprs {
                        self.ty_check_expr(arg_expr) ?;
                    }

                    self.check_call_args(span, fn_decl.fn_node_ptr, arg_exprs, fn_decl) ?;
                    
                    if let Some(return_ty) = fn_decl.return_ty.borrow().clone() {
                        return Ok(return_ty);
                    }
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span,
                        message: format!("struct method `{}` is not type checked `{}`", ident, ty),
                    });
                }
            },
            _ => ()
        }
        Err(LiveError {
            origin: live_error_origin!(),
            span,
            message: format!("method `{}` is not defined on type `{}`", ident, ty),
        })
    }
    
    fn ty_check_closure_call_expr(
        &mut self,
        span: Span,
        ident: Ident,
        arg_exprs: &[Expr],
    ) -> Result<Ty, LiveError> {
        
        for arg_expr in arg_exprs {
            self.ty_check_expr(arg_expr) ?;
        }
        match self.scopes.find_sym_on_scopes(ident, span) {
            Some(Sym::Closure {return_ty, params}) => {
                let closure_args = self.check_params_against_args(span, &params, arg_exprs) ?;
                // error out
                if closure_args.len() > 0{
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span,
                        message: format!("Cannot pass closures to closures, please implement"),
                    })
                }
                return Ok(return_ty)
            }
            _ => ()
        }
        Err(LiveError {
            origin: live_error_origin!(),
            span,
            message: format!("Closure call `{}` is not defined on", ident),
        })
    }
    
    
    fn ty_check_builtin_call_expr(
        &mut self,
        span: Span,
        ident: Ident,
        arg_exprs: &[Expr],
    ) -> Result<Ty, LiveError> {
        for arg_expr in arg_exprs {
            self.ty_check_expr(arg_expr) ?;
        }
        
        let builtin = self.shader_registry.builtins.get(&ident).unwrap();
        let arg_tys = arg_exprs
            .iter()
            .map( | arg_expr | arg_expr.ty.borrow().as_ref().unwrap().clone())
            .collect::<Vec<_ >> ();
        Ok(builtin .return_tys .get(&arg_tys) .ok_or({
            let mut message = String::new();
            write!(
                message,
                "can't apply builtin `{}` to arguments of types ",
                ident
            )
                .unwrap();
            let mut sep = "";
            for arg_ty in arg_tys {
                write!(message, "{}{}", sep, arg_ty).unwrap();
                sep = ", ";
            }
            LiveError {origin: live_error_origin!(), span, message}
        }) ? .clone())
    }
    
    fn check_call_args(
        &mut self,
        span: Span,
        fn_node_ptr: FnNodePtr,
        arg_exprs: &[Expr],
        fn_decl: &FnDecl,
    ) -> Result<(), LiveError> {
        match self.check_params_against_args(span, &fn_decl.params, arg_exprs) {
           Err(err)=> Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: format!("function: `{}`: {}", self.shader_registry.fn_ident_from_ptr(fn_node_ptr), err.message)
            }),
            Ok(closure_args)=>{
                if closure_args.len()>0{
                    let mut ci = self.scopes.closure_instances.borrow_mut();
                    ci.push(ClosureInstance{
                        fn_node_ptr,
                        scope: self.scopes.scopes.last().cloned().unwrap(),
                        closure_args
                    })
                }
                Ok(())
            }
        }
    }
    
    fn check_params_against_args(
        &mut self,
        span: Span,
        params: &[Param],
        arg_exprs: &[Expr],
    ) -> Result<Vec<ClosureInstanceArg>, LiveError> {
        if arg_exprs.len() < params.len() {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: format!(
                    "not enough arguments expected {}, got {}",
                    params.len(),
                    arg_exprs.len(),
                )
                    .into(),
            });
        }
        if arg_exprs.len() > params.len() {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: format!(
                    "too many arguments for call expected {}, got {}",
                    params.len(),
                    arg_exprs.len()
                )
                    .into(),
            });
        }
        let mut closure_args= Vec::new();
        for (arg_index, (arg_expr, param)) in arg_exprs.iter().zip(params.iter()).enumerate()
        {
            let arg_ty = arg_expr.ty.borrow();
            let arg_ty = arg_ty.as_ref().unwrap();
            let param_ty = param.ty_expr.ty.borrow();
            let param_ty = param_ty.as_ref().unwrap();
            
            // if the thing is a closure def / decl we have to do a deep compare
            // we should have the closure def on our scopes
            // and the closure decl should be on our param.ty_expr
            if let Ty::ClosureDef(def_index) = arg_ty{
                if let Ty::ClosureDecl = param_ty{
                    closure_args.push(ClosureInstanceArg{
                        arg_index,
                        def_index:*def_index,
                    });
                    continue
                }
            }
            if arg_ty != param_ty {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!(
                        "wrong type for argument {} expected `{}`, got `{}`",
                        arg_index + 1,
                        param_ty,
                        arg_ty,
                    ).into()
                });
            }
            if param.is_inout {
                self.lhs_checker().lhs_check_expr(arg_expr) ?;
            }
        }
        if closure_args.len()>0{
            
        }
        Ok(closure_args)
    }
    
    fn ty_check_field_expr(
        &mut self,
        span: Span,
        expr: &Expr,
        field_ident: Ident,
    ) -> Result<Ty, LiveError> {
        let ty = self.ty_check_expr(expr) ?;
        match ty {
            ref ty if ty.is_vector() => {
                let swizzle = Swizzle::parse(field_ident)
                    .filter( | swizzle | {
                    if swizzle.len() > 4 {
                        return false;
                    }
                    let size = ty.size();
                    for &index in swizzle {
                        if index > size {
                            return false;
                        }
                    }
                    true
                })
                    .ok_or_else( || LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("field `{}` is not defined on type `{}`", field_ident, ty),
                }) ?;
                Ok(match ty {
                    Ty::Bvec2 | Ty::Bvec3 | Ty::Bvec4 => match swizzle.len() {
                        1 => Ty::Bool,
                        2 => Ty::Bvec2,
                        3 => Ty::Bvec3,
                        4 => Ty::Bvec4,
                        _ => panic!(),
                    },
                    Ty::Ivec2 | Ty::Ivec3 | Ty::Ivec4 => match swizzle.len() {
                        1 => Ty::Int,
                        2 => Ty::Ivec2,
                        3 => Ty::Ivec3,
                        4 => Ty::Ivec4,
                        _ => panic!(),
                    },
                    Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => match swizzle.len() {
                        1 => Ty::Float,
                        2 => Ty::Vec2,
                        3 => Ty::Vec3,
                        4 => Ty::Vec4,
                        _ => panic!(),
                    },
                    _ => panic!(),
                })
            }
            Ty::Struct(struct_ptr) => {
                Ok(self.shader_registry.structs.get(&struct_ptr) .unwrap() .find_field(field_ident) .ok_or(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("field `{}` is not defined on type `{:?}`", field_ident, struct_ptr),
                }) ? .ty_expr .ty .borrow() .as_ref() .unwrap() .clone())
            },
            Ty::DrawShader(shader_ptr) => {
                Ok(self.shader_registry.draw_shaders.get(&shader_ptr).unwrap().find_field(field_ident) .ok_or(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("field `{}` is not defined on shader `{:?}`", field_ident, shader_ptr),
                }) ? .ty_expr .ty .borrow() .as_ref() .unwrap() .clone())
            }
            _ => Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: format!("can't access field on value of type `{}`", ty).into(),
            }),
        }
    }
    
    fn ty_check_index_expr(
        &mut self,
        span: Span,
        expr: &Expr,
        index_expr: &Expr,
    ) -> Result<Ty, LiveError> {
        let ty = self.ty_check_expr(expr) ?;
        let index_ty = self.ty_check_expr(index_expr) ?;
        let elem_ty = match ty {
            Ty::Bvec2 | Ty::Bvec3 | Ty::Bvec4 => Ty::Bool,
            Ty::Ivec2 | Ty::Ivec3 | Ty::Ivec4 => Ty::Int,
            Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => Ty::Float,
            Ty::Mat2 => Ty::Vec2,
            Ty::Mat3 => Ty::Vec3,
            Ty::Mat4 => Ty::Vec4,
            _ => {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("can't index into value of type `{}`", ty).into(),
                })
            }
        };
        if index_ty != Ty::Int {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: "index is not an integer".into(),
            });
        }
        Ok(elem_ty)
    }
    
    #[allow(clippy::redundant_closure_call)]
    fn ty_check_cons_call_expr(
        &mut self,
        span: Span,
        ty_lit: TyLit,
        arg_exprs: &[Expr],
    ) -> Result<Ty, LiveError> {
        let ty = ty_lit.to_ty();
        let arg_tys = arg_exprs
            .iter()
            .map( | arg_expr | self.ty_check_expr(arg_expr))
            .collect::<Result<Vec<_>, _ >> () ?;
        match (&ty, arg_tys.as_slice()) {
            (ty, [arg_ty]) if ty.is_scalar() && arg_ty.is_scalar() => Ok(ty.clone()),
            (ty, [arg_ty]) if ty.is_vector() && arg_ty.is_scalar() => Ok(ty.clone()),
            (ty, [arg_ty]) if ty.is_matrix() && arg_ty.is_scalar() || arg_ty.is_matrix() => {
                Ok(ty.clone())
            }
            (ty, arg_tys)
            if ty.is_vector()
                && ( || {
                arg_tys.iter().all( | arg_ty | {
                    arg_ty.is_scalar() || arg_ty.is_vector() || arg_ty.is_matrix()
                })
            })()
                || ty.is_matrix()
                && ( || {
                arg_tys.iter().all( | arg_ty | {
                    arg_ty.is_scalar() || arg_ty.is_vector() || arg_ty.is_matrix()
                })
            })() =>
            {
                let expected_size = ty.size();
                let actual_size = arg_tys.iter().map( | arg_ty | arg_ty.size()).sum::<usize>();
                if actual_size < expected_size {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span,
                        message: format!(
                            "not enough components for call to constructor `{}`: expected {}, got {}",
                            ty_lit,
                            actual_size,
                            expected_size,
                        )
                            .into()
                    });
                }
                if actual_size > expected_size {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span,
                        message: format!(
                            "too many components for call to constructor `{}`: expected {}, got {}",
                            ty_lit,
                            expected_size,
                            actual_size,
                        )
                            .into(),
                    });
                }
                Ok(ty.clone())
            }
            _ => Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: format!(
                    "can't construct value of type `{}` with arguments of types `{}`",
                    ty,
                    CommaSep(&arg_tys)
                )
                    .into(),
            }),
        }
    }
    
    fn ty_check_var_expr(
        &mut self,
        span: Span,
        kind: &Cell<Option<VarKind >>,
        var_resolve: VarResolve
    ) -> Result<Ty, LiveError> {
        
        // use the suggestion
        match var_resolve {
            VarResolve::Const(const_node_ptr) => {
                // ok we have a const, and we can fetch it,
                // and it has a type
                let const_decl = self.shader_registry.consts.get(&const_node_ptr).unwrap();
                kind.set(Some(VarKind::Const(const_node_ptr)));
                return Ok(const_decl.ty_expr.ty.borrow().clone().unwrap());
                //kind.set(Some(VarKind::Const(value_ptr)));
                //return Ok(ty_lit.to_ty());
            },
            VarResolve::LiveValue(value_ptr, ty_lit) => {
                kind.set(Some(VarKind::LiveValue(value_ptr)));
                return Ok(ty_lit.to_ty());
            }
            VarResolve::NotFound(ident) => {
                match self.scopes.find_sym_on_scopes(ident, span) {
                    Some(Sym::Local {is_mut, ty}) => {
                        // ok its either a local var or something.
                        kind.set(Some(if is_mut {VarKind::MutLocal(ident)}else {VarKind::Local(ident)}));
                        return Ok(ty)
                    }
                    _ => ()
                }
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("`{}` is not defined in this scope", ident),
                })
            }
        }
    }
    
    fn ty_check_struct_cons(
        &mut self,
        struct_node_ptr: StructNodePtr,
        span: Span,
        args: &Vec<(Ident, Expr)>,
    ) -> Result<Ty, LiveError> {
        let struct_decl = self.shader_registry.structs.get(&struct_node_ptr).unwrap();
        for (ident, expr) in args {
            self.ty_check_expr(expr) ?;
            // ok so now we find ident, then check the type
            if let Some(field) = struct_decl.fields.iter().find( | field | field.ident == *ident) {
                // ok so the field has a TyExpr
                let field_ty = field.ty_expr.ty.borrow();
                let my_ty = expr.ty.borrow();
                if field_ty.as_ref() != my_ty.as_ref() {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span,
                        message: format!("field `{}` is the wrong type {} instead of {}", ident, my_ty.as_ref().unwrap(), field_ty.as_ref().unwrap()),
                    })
                }
            }
            else {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("`{}` is not a valid struct field", ident),
                })
            }
        }
        // if we are missing idents or have doubles, error
        for field in &struct_decl.fields {
            if args.iter().position( | (ident, expr) | ident == &field.ident).is_none() {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("`{}` field is missing", field.ident),
                })
            }
        }
        for i in 0..args.len() {
            for j in (i + 1)..args.len() {
                if args[i].0 == args[j].0 { // duplicate
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span,
                        message: format!("`{}` field is duplicated", args[i].0),
                    })
                }
            }
        }
        // its all ok.
        Ok(Ty::Struct(struct_node_ptr))
    }
    
    fn ty_check_lit_expr(&mut self, _span: Span, lit: Lit) -> Result<Ty, LiveError> {
        Ok(lit.to_ty())
    }
}
