#![allow(unused_variables)]
use crate::shaderast::*;
use crate::shaderast::Scopes;
use crate::shaderast::Ident;
use makepad_live_parser::Span;
//use makepad_live_parser::Id;
//use makepad_live_parser::id;
use crate::shaderast::{Ty, TyLit};
use std::cell::{Cell};
use crate::shaderregistry::ShaderRegistry;

#[derive(Clone, Debug)]
pub struct DepAnalyser<'a> {
    pub decl: &'a FnDecl,
    pub shader_registry: &'a ShaderRegistry,
    pub scopes: &'a Scopes,
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
            ExprKind::Un {span, op, ref expr} => self.dep_analyse_un_expr(span, op, expr),
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
            ExprKind::MethodCall {
                span,
                ident,
                ref arg_exprs,
            } => self.dep_analyse_method_call_expr(span, ident, arg_exprs),
            ExprKind::PlainCall {
                span,
                //dent,
                ref arg_exprs,
                fn_node_ptr,
            } => self.dep_analyse_plain_call_expr(span, arg_exprs, fn_node_ptr),
            ExprKind::BuiltinCall {
                span,
                ident,
                ref arg_exprs,
            } => self.dep_analyse_builtin_call_expr(span, ident, arg_exprs),
            ExprKind::ClosureCall {
                span,
                ident,
                ref arg_exprs,
            } => self.dep_analyse_closure_call_expr(span, ident, arg_exprs),
            ExprKind::ClosureDef(_) => (),
            ExprKind::ConsCall {
                span,
                ty_lit,
                ref arg_exprs,
            } => self.dep_analyse_cons_call_expr(span, ty_lit, arg_exprs),
            ExprKind::StructCons{
                struct_node_ptr,
                span,
                ref args
            } => self.dep_analyse_struct_cons(struct_node_ptr, span, args),
            ExprKind::Var {
                span,
                ref kind,
                ..
            } => self.dep_analyse_var_expr(span, expr.ty.borrow().as_ref(), kind),
            ExprKind::Lit {span, lit} => self.dep_analyse_lit_expr(span, lit),
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
            Ty::Struct(struct_ptr) => {
                // ok we have a struct ptr
                for arg_expr in arg_exprs {
                    self.dep_analyse_expr(arg_expr);
                }
                let mut set = self.decl.callees.borrow_mut();
                // ok we need to find the method FnPtr from the struct_ptr
                let struct_decl = self.shader_registry.structs.get(struct_ptr).unwrap();
                if let Some(fn_node_ptr) = self.shader_registry.struct_method_ptr_from_ident(struct_decl, method_ident){
                    set.as_mut().unwrap().insert(fn_node_ptr);
                }
                //panic!("IMPL")
            }
            Ty::DrawShader(shader_ptr)=>{
                // ok we have a struct ptr
                for arg_expr in arg_exprs {
                    self.dep_analyse_expr(arg_expr);
                }
                let mut set = self.decl.callees.borrow_mut();
                let draw_shader_decl = self.shader_registry.draw_shaders.get(shader_ptr).unwrap();
                if let Some(fn_node_ptr) = self.shader_registry.draw_shader_method_ptr_from_ident(draw_shader_decl, method_ident){
                    set.as_mut().unwrap().insert(fn_node_ptr);
                }
            }
            _ => panic!(),
        }
    }
    
    /*
    fn dep_analyse_static_call_expr(
        &mut self,
        span: Span,
        method_ident: Ident,
        arg_exprs: &[Expr],
        struct_node_ptr: StructNodePtr,
    ) {
        match arg_exprs[0].ty.borrow().as_ref().unwrap() {
            Ty::Struct(struct_ptr) => {
                // this is a method call.
                for arg_expr in arg_exprs {
                    self.dep_analyse_expr(arg_expr);
                }
                panic!("IMPL")
            }
            _ => panic!(),
        }
    }*/
    
    fn dep_analyse_closure_call_expr(
        &mut self,
        span: Span,
        ident: Ident,
        arg_exprs: &[Expr],
    ) {
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        // ok so this must be a closure call.
        
        self.decl
            .closure_deps
            .borrow_mut()
            .as_mut()
            .unwrap()
            .insert(ident);
    }
    
    fn dep_analyse_builtin_call_expr(
        &mut self,
        span: Span,
        ident: Ident,
        arg_exprs: &[Expr],
    ) {
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        self.decl
            .builtin_deps
            .borrow_mut()
            .as_mut()
            .unwrap()
            .insert(ident);
    }
    
    
    fn dep_analyse_plain_call_expr(
        &mut self,
        span: Span,
        //ident: Ident,
        arg_exprs: &[Expr],
        fn_node_ptr: FnNodePtr
    ) {
        //let ident = ident_path.get_single().expect("IMPL");
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        let mut set = self.decl.callees.borrow_mut();
        set.as_mut().unwrap().insert(fn_node_ptr);
        /*
        match self.env.find_sym(ident, span).unwrap() {
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
                    .insert(FnCall::Call{ident});
            }
            _ => panic!(),
        }*/
    }
    
    fn dep_analyse_field_expr(&mut self, _span: Span, expr: &Expr, field_ident: Ident) {
        // so we have to store which 'shader props' we use
        match expr.ty.borrow().as_ref().unwrap(){
            Ty::DrawShader(shader_ptr)=>{
                self.decl.draw_shader_refs.borrow_mut().as_mut().unwrap().insert(field_ident);
            }
            _=>{
                  self.dep_analyse_expr(expr)
            }
        }
       
    }
    
    fn dep_analyse_index_expr(&mut self, _span: Span, expr: &Expr, index_expr: &Expr) {
        self.dep_analyse_expr(expr);
        self.dep_analyse_expr(index_expr);
    }
    
    fn dep_analyse_cons_call_expr(&mut self, _span: Span, ty_lit: TyLit, arg_exprs: &[Expr]) {
        for arg_expr in arg_exprs {
            self.dep_analyse_expr(arg_expr);
        }
        self.decl
            .constructor_fn_deps
            .borrow_mut()
            .as_mut()
            .unwrap()
            .insert((
            ty_lit,
            arg_exprs
                .iter()
                .map( | arg_expr | arg_expr.ty.borrow().as_ref().unwrap().clone())
                .collect::<Vec<_ >> (),
        ));
    }
    
    fn dep_analyse_struct_cons(
        &mut self,
        struct_node_ptr: StructNodePtr,
        _span: Span,
        args: &Vec<(Ident,Expr)>,
    ) {
        // alright we have a struct constructor
        self.decl.struct_refs.borrow_mut().as_mut().unwrap().insert(struct_node_ptr);
        for arg in args{
            self.dep_analyse_expr(&arg.1);
        }
    }    
    
    fn dep_analyse_var_expr(&mut self, _span: Span, ty:Option<&Ty>, kind: &Cell<Option<VarKind >>) {
        // alright so. a var expr..
        match kind.get().unwrap() {
            VarKind::Const(const_ptr) =>{
                self.decl.const_refs.borrow_mut().as_mut().unwrap().insert(const_ptr);
            }
            VarKind::LiveValue(value_ptr)=>{
                self.decl.live_refs.borrow_mut().as_mut().unwrap().insert(value_ptr);
            }
            VarKind::Local(_) | VarKind::MutLocal(_)=>{ // we need to store the type
                match ty{
                    Some(Ty::Struct(struct_ptr))=>{
                        self.decl.struct_refs.borrow_mut().as_mut().unwrap().insert(*struct_ptr);
                    }
                    Some(Ty::Array{..})=>{
                        todo!();
                    }
                    _=>()
                }
            },
        };
        /*
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
            VarKind::Uniform(block) => {
                self.decl
                    .uniform_block_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(block);
            }
            VarKind::Varying => {
                self.decl.has_varying_deps.set(Some(true));
            }
            VarKind::Live(_) => {
                self.decl
                    .uniform_block_deps
                    .borrow_mut()
                    .as_mut()
                    .unwrap()
                    .insert(Ident(id!(live)));
                
            }
            _ => {}
        }*/
    }
    /*
    fn dep_analyse_live_id_expr(&mut self, _span: Span, _kind: &Cell<Option<VarKind>>, _id:LiveItemId, _ident: Ident) {

    }
*/
    fn dep_analyse_lit_expr(&mut self, _span: Span, _lit: Lit) {}
}
