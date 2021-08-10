use crate::shaderast::*;
use crate::const_eval::ConstEvaluator;
use crate::const_gather::ConstGatherer;
use crate::dep_analyse::DepAnalyser;
use crate::shaderast::Scopes;
use crate::shaderast::ScopeSymKind;
use crate::shaderast::Ident;
use crate::shaderast::{Ty, TyExpr};
use crate::ty_check::TyChecker;
use crate::shaderregistry::ShaderRegistry;

use makepad_live_parser::LiveError;
use makepad_live_parser::LiveErrorOrigin;
use makepad_live_parser::live_error_origin;
use makepad_live_parser::id;
use makepad_live_parser::Id;
use makepad_live_parser::Span;

use std::cell::RefCell;
use std::cell::Cell;
use std::collections::HashMap;

#[derive(Debug, Clone, Copy)]
pub struct ShaderAnalyseOptions {
    pub no_const_collapse: bool
}

#[derive(Debug)]
pub struct StructAnalyser<'a> {
    pub struct_decl: &'a StructDecl,
    pub scopes: &'a mut Scopes,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> StructAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            shader_registry: self.shader_registry,
            scopes: self.scopes,
        }
    }
    
    pub fn analyse_struct(&mut self) -> Result<(), LiveError> {
        self.scopes.push_scope();
        self.struct_decl.init_analysis();
        // we need to analyse the fields and put them somewhere.
        for field in &self.struct_decl.fields {
            self.analyse_field_decl(field) ?;
        }
        // first analyse decls
        for fn_node_ptr in &self.struct_decl.methods {
            let fn_decl = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            self.analyse_method_decl(fn_decl) ?;
        }
        // now analyse the functions
        for fn_node_ptr in &self.struct_decl.methods {
            let decl = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            FnDefAnalyser {
                decl,
                scopes: &mut self.scopes,
                shader_registry: self.shader_registry,
                options: self.options,
                is_inside_loop: false,
            }
            .analyse_fn_def() ?;
        }
        self.scopes.pop_scope();
        Ok(())
    }
    
    fn analyse_field_decl(&mut self, decl: &FieldDecl) -> Result<(), LiveError> {
        self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
        // ok so. if this thing depends on structs, lets store them.
        match decl.ty_expr.ty.borrow().as_ref().unwrap() {
            Ty::Struct(struct_ptr) => {
                self.struct_decl.struct_refs.borrow_mut().as_mut().unwrap().insert(*struct_ptr);
            }
            Ty::Array {..} => {
                todo!();
            }
            _ => ()
        }
        Ok(())
    }
    
    fn analyse_method_decl(&mut self, decl: &FnDecl) -> Result<(), LiveError> {
        for param in &decl.params {
            self.ty_checker().ty_check_ty_expr(&param.ty_expr) ?;
        }
        let return_ty = decl
            .return_ty_expr
            .as_ref()
            .map( | return_ty_expr | self.ty_checker().ty_check_ty_expr(return_ty_expr))
            .transpose() ?
        .unwrap_or(Ty::Void);
        *decl.return_ty.borrow_mut() = Some(return_ty);
        Ok(())
    }
    
}


#[derive(Debug)]
pub struct DrawShaderAnalyser<'a> {
    pub draw_shader_decl: &'a DrawShaderDecl,
    pub scopes: &'a mut Scopes,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> DrawShaderAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            scopes: self.scopes,
            shader_registry: self.shader_registry,
        }
    }
    
    pub fn analyse_shader(&mut self) -> Result<(), LiveError> {
        self.scopes.push_scope();
        
        for decl in &self.draw_shader_decl.fields {
            self.analyse_field_decl(decl) ?;
        }
        // first analyse decls
        for fn_node_ptr in &self.draw_shader_decl.methods {
            let fn_decl = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            self.analyse_method_decl(fn_decl) ?;
        }
        
        // now analyse the methods
        for fn_node_ptr in &self.draw_shader_decl.methods {
            let decl = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            FnDefAnalyser {
                shader_registry: self.shader_registry,
                decl,
                scopes: &mut self.scopes,
                options: self.options,
                is_inside_loop: false,
            }
            .analyse_fn_def() ?;
        }
        
        self.scopes.pop_scope();
        
        let mut all_fns = Vec::new();
        let mut vertex_fns = Vec::new();
        // we should insert our vertex call
        
        self.analyse_call_tree(
            &mut Vec::new(),
            self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_decl, Ident(id!(vertex))).unwrap(),
            &mut vertex_fns,
            &mut all_fns,
        ) ?;
        
        let mut pixel_fns = Vec::new();
        self.analyse_call_tree(
            &mut Vec::new(),
            self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_decl, Ident(id!(pixel))).unwrap(),
            &mut pixel_fns,
            &mut all_fns,
        ) ?;
        
        // mark all the draw_shader_refs we reference in pixelshaders.
        for pixel_fn in &pixel_fns {
            // if we run into a DrawShaderMethod mark it as
            if let Some(fn_decl) = self.shader_registry.all_fns.get(pixel_fn) {
                if let Some(FnSelfKind::DrawShader(_)) = fn_decl.self_kind {
                    // lets iterate all
                    for dsr in fn_decl.draw_shader_refs.borrow().as_ref().unwrap() {
                        // ok we have a draw shader ident we use, now mark it on our draw_shader_decl.
                        for field in &self.draw_shader_decl.fields {
                            if field.ident == *dsr { // we found it
                                match &field.kind {
                                    DrawShaderFieldKind::Geometry {ref is_used_in_pixel_shader, ..} => {
                                        is_used_in_pixel_shader.set(true);
                                    }
                                    DrawShaderFieldKind::Instance {ref is_used_in_pixel_shader, ..} => {
                                        is_used_in_pixel_shader.set(true);
                                    }
                                    _ => ()
                                }
                            }
                        }
                    }
                }
            }
        }
        
        let mut all_structs = Vec::new();
        let mut pixel_structs = Vec::new();
        let mut vertex_structs = Vec::new();
        
        for pixel_fn in &pixel_fns {
            let fn_decl = self.shader_registry.all_fns.get(pixel_fn).unwrap();
            // lets collect all structs
            for struct_ptr in fn_decl.struct_refs.borrow().as_ref().unwrap().iter() {
                let struct_decl = self.shader_registry.structs.get(struct_ptr).unwrap();
                self.analyse_struct_tree(&mut Vec::new(), *struct_ptr, struct_decl, &mut pixel_structs, &mut all_structs) ?;
            }
        }
        for vertex_fn in &vertex_fns {
            let fn_decl = self.shader_registry.all_fns.get(vertex_fn).unwrap();
            // lets collect all structs
            for struct_ptr in fn_decl.struct_refs.borrow().as_ref().unwrap().iter() {
                let struct_decl = self.shader_registry.structs.get(struct_ptr).unwrap();
                self.analyse_struct_tree(&mut Vec::new(), *struct_ptr, struct_decl, &mut vertex_structs, &mut all_structs) ?;
            }
        }
        
        *self.draw_shader_decl.all_fns.borrow_mut() = all_fns;
        *self.draw_shader_decl.vertex_fns.borrow_mut() = vertex_fns;
        *self.draw_shader_decl.pixel_fns.borrow_mut() = pixel_fns;
        
        *self.draw_shader_decl.all_structs.borrow_mut() = all_structs;
        *self.draw_shader_decl.vertex_structs.borrow_mut() = vertex_structs;
        *self.draw_shader_decl.pixel_structs.borrow_mut() = pixel_structs;
        
        Ok(())
    }
    
    fn analyse_field_decl(&mut self, decl: &DrawShaderFieldDecl) -> Result<(), LiveError> {
        let ty = match decl.kind {
            DrawShaderFieldKind::Geometry {..} => {
                let ty = self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
                match ty {
                    Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 | Ty::Mat4 => {}
                    _ => {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: decl.span,
                            message: String::from(
                                "attribute must be either a floating-point scalar or vector or mat4",
                            ),
                        })
                    }
                }
                ty
            }
            DrawShaderFieldKind::Instance {..} => {
                let ty = self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
                match ty {
                    Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 | Ty::Mat4 => {}
                    _ => {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: decl.span,
                            message: String::from(
                                "attribute must be either a floating-point scalar or vector or mat4",
                            ),
                        })
                    }
                }
                ty
            }
            DrawShaderFieldKind::Texture {..} => {
                let ty = self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
                match ty {
                    Ty::Texture2D => {}
                    _ => {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: decl.span,
                            message: String::from("texture must be a texture2D"),
                        })
                    }
                }
                ty
            }
            DrawShaderFieldKind::Uniform {..} => {
                let ty = self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
                ty
            },
            DrawShaderFieldKind::Varying {..} => {
                let ty = self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
                match ty {
                    Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 => {}
                    _ => {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: decl.span,
                            message: String::from(
                                "varying must be either a floating-point scalar or vector",
                            ),
                        })
                    }
                }
                ty
            }
        };
        *decl.ty_expr.ty.borrow_mut() = Some(ty);
        Ok(())
    }
    
    fn analyse_method_decl(&mut self, decl: &FnDecl) -> Result<(), LiveError> {
        for param in &decl.params {
            self.ty_checker().ty_check_ty_expr(&param.ty_expr) ?;
        }
        let return_ty = decl
            .return_ty_expr
            .as_ref()
            .map( | return_ty_expr | self.ty_checker().ty_check_ty_expr(return_ty_expr))
            .transpose() ?
        .unwrap_or(Ty::Void);
        
        if decl.ident == Ident(id!(vertex)) {
            match return_ty {
                Ty::Vec4 => {}
                _ => {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: decl.span,
                        message: String::from(
                            "function `vertex` must return a value of type `vec4`",
                        ),
                    })
                }
            }
        } else if decl.ident == Ident(id!(pixel)) {
            match return_ty {
                Ty::Vec4 => {}
                _ => {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: decl.span,
                        message: String::from(
                            "function `fragment` must return a value of type `vec4`",
                        ),
                    })
                }
            }
        } else {
            match return_ty {
                Ty::Array {..} => {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: decl.span,
                        message: String::from("functions can't return arrays"),
                    })
                }
                _ => {}
            }
        }
        *decl.return_ty.borrow_mut() = Some(return_ty);
        //self.env.insert_sym(decl.span, decl.ident, Sym::Fn).ok();
        Ok(())
    }
    
    fn analyse_struct_tree(
        &self,
        call_stack: &mut Vec<StructNodePtr>,
        struct_ptr: StructNodePtr,
        struct_decl: &StructDecl,
        deps: &mut Vec<StructNodePtr>,
        all_deps: &mut Vec<StructNodePtr>,
    ) -> Result<(), LiveError> {
        // lets see if callee is already in the vec, ifso remove it
        if let Some(index) = deps.iter().position( | v | v == &struct_ptr) {
            deps.remove(index);
        }
        deps.push(struct_ptr);
        
        if let Some(index) = all_deps.iter().position( | v | v == &struct_ptr) {
            all_deps.remove(index);
        }
        all_deps.push(struct_ptr);
        
        call_stack.push(struct_ptr);
        
        for sub_ptr in struct_decl.struct_refs.borrow().as_ref().unwrap().iter() {
            // ok now we need a fn decl for this callee
            let sub_decl = self.shader_registry.structs.get(sub_ptr).unwrap();
            if call_stack.contains(&sub_ptr) {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span: sub_decl.span,
                    message: format!("Struct has recursively dependency"),
                });
            }
            
            self.analyse_struct_tree(call_stack, *sub_ptr, sub_decl, deps, all_deps) ?;
        }
        call_stack.pop();
        Ok(())
    }
    
    
    fn analyse_call_tree(
        &self,
        call_stack: &mut Vec<FnNodePtr>,
        decl: &FnDecl,
        //callee: Callee,
        deps: &mut Vec<FnNodePtr>,
        all_deps: &mut Vec<FnNodePtr>,
    ) -> Result<(), LiveError> {
        // lets see if callee is already in the vec, ifso remove it
        if let Some(index) = deps.iter().position( | v | v == &decl.fn_node_ptr) {
            deps.remove(index);
        }
        deps.push(decl.fn_node_ptr);
        
        if let Some(index) = all_deps.iter().position( | v | v == &decl.fn_node_ptr) {
            all_deps.remove(index);
        }
        all_deps.push(decl.fn_node_ptr);
        
        call_stack.push(decl.fn_node_ptr);
        for callee in decl.callees.borrow().as_ref().unwrap().iter() {
            // ok now we need a fn decl for this callee
            let callee_decl = self.shader_registry.all_fns.get(&callee).unwrap();
            if call_stack.contains(&callee_decl.fn_node_ptr) {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span: decl.span,
                    message: format!("function `{}` recursively calls `{}`", decl.ident, callee_decl.ident),
                });
            }
            
            self.analyse_call_tree(call_stack, callee_decl, deps, all_deps) ?;
        }
        call_stack.pop();
        
        Ok(())
    }
}



#[derive(Debug)]
pub struct ConstAnalyser<'a> {
    pub decl: &'a ConstDecl,
    pub scopes: &'a mut Scopes,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> ConstAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            shader_registry: self.shader_registry,
            scopes: self.scopes,
        }
    }
    
    fn const_evaluator(&self) -> ConstEvaluator {
        ConstEvaluator {
            options: self.options
        }
    }
    
    pub fn analyse_const_decl(&mut self) -> Result<(), LiveError> {
        let expected_ty = self.ty_checker().ty_check_ty_expr(&self.decl.ty_expr) ?;
        let actual_ty = self.ty_checker().ty_check_expr_with_expected_ty(
            self.decl.span,
            &self.decl.expr,
            &expected_ty,
        ) ?;
        if expected_ty != actual_ty {
            return Err(LiveError {
                origin: live_error_origin!(),
                span: self.decl.span,
                message: String::from("Declared type and inferred type not the same"),
            } .into());
        }
        self.const_evaluator().const_eval_expr(&self.decl.expr) ?;
        Ok(())
    }
}


#[derive(Debug)]
pub struct FnDefAnalyser<'a> {
    pub decl: &'a FnDecl,
    pub scopes: &'a mut Scopes,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
    pub is_inside_loop: bool,
}

impl<'a> FnDefAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            shader_registry: self.shader_registry,
            scopes: self.scopes,
        }
    }
    
    fn const_evaluator(&self) -> ConstEvaluator {
        ConstEvaluator {
            options: self.options
        }
    }
    
    fn const_gatherer(&self) -> ConstGatherer {
        ConstGatherer {
            decl: self.decl
        }
    }
    
    fn dep_analyser(&self) -> DepAnalyser {
        DepAnalyser {
            shader_registry: self.shader_registry,
            decl: self.decl,
            scopes: &self.scopes,
        }
    }
    
    pub fn analyse_fn_decl(&mut self) -> Result<(), LiveError> {
        for param in &self.decl.params {
            self.ty_checker().ty_check_ty_expr(&param.ty_expr) ?;
        }
        let return_ty = self.decl
            .return_ty_expr
            .as_ref()
            .map( | return_ty_expr | self.ty_checker().ty_check_ty_expr(return_ty_expr))
            .transpose() ?
        .unwrap_or(Ty::Void);
        *self.decl.return_ty.borrow_mut() = Some(return_ty);
        Ok(())
    }
    
    pub fn analyse_fn_def(&mut self) -> Result<(), LiveError> {
        self.scopes.push_scope();
        for param in &self.decl.params {
            match &param.ty_expr.kind {
                TyExprKind::ClosureDecl {return_ty, params, ..} => {
                    self.scopes.insert_sym(
                        param.span,
                        param.ident,
                        ScopeSymKind::Closure {
                            return_ty: return_ty.borrow().clone().unwrap(),
                            params: params.clone()
                        },
                    );
                }
                _ => {
                    let shadow = self.scopes.insert_sym(
                        param.span,
                        param.ident,
                        ScopeSymKind::MutLocal(param.ty_expr.ty.borrow().as_ref().unwrap().clone()),
                    );
                    param.shadow.set(Some(shadow));
                }
            }
            
        }
        *self.decl.return_ty.borrow_mut() = Some(
            self.decl
                .return_ty_expr
                .as_ref()
                .map( | return_ty_expr | return_ty_expr.ty.borrow().as_ref().unwrap().clone())
                .unwrap_or(Ty::Void),
        );
        self.decl.init_analysis();
        self.analyse_block(&self.decl.block) ?;
        self.scopes.pop_scope();
        // alright we have closures to analyse
        // let closure_isntances = self.
        // lets move the closures from env to
        // then analyse it
        self.analyse_closures() ?;
        
        
        Ok(())
    }
    
    fn analyse_closures(&mut self) -> Result<(), LiveError> {
        let closure_instances = self.scopes.closure_instances.replace(Vec::new());
        let mut closure_scopes = self.scopes.closure_scopes.replace(HashMap::new());
        for ci in closure_instances {
            let fn_decl = self.shader_registry.all_fns.get(&ci.fn_node_ptr).unwrap();
            
            // lets start the closure
            for ci_arg in ci.closure_args {
                
                let mut scopes = closure_scopes.get_mut(&ci_arg.def_index).unwrap();
                // lets swap our scopes for the closure scopes
                std::mem::swap(&mut self.scopes.scopes, &mut scopes);
                
                // ok now we analyse the closure
                // lets fetch the fn_decl
                self.scopes.push_scope();
                let closure_def = &self.decl.closure_defs[ci_arg.def_index.0];
                let fn_param = &fn_decl.params[ci_arg.arg_index];
                
                if let TyExprKind::ClosureDecl {params, ..} = &fn_param.ty_expr.kind {
                    // alright we have a fn_decl and a closure_def
                    // lets get the closure-decl
                    if closure_def.params.len() != params.len() {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: closure_def.span,
                            message: format!(
                                "Closure does not have the same number of arguments as function decl: {} expected: {}",
                                closure_def.params.len(), params.len()
                            ),
                        });
                    }
                    // lets now push the argument idents on the scope
                    for i in 0..closure_def.params.len() {
                        let def_param = &closure_def.params[i];
                        let decl_param = &params[i];
                        // the decl params should already be analysed
                        // the def params not.
                        let shadow = self.scopes.insert_sym(
                            def_param.span,
                            def_param.ident,
                            ScopeSymKind::MutLocal(decl_param.ty_expr.ty.borrow().as_ref().unwrap().clone()),
                        );
                        def_param.shadow.set(Some(shadow));
                    }
                    // ok and now we go analyse the body.
                    match &closure_def.kind{
                        ClosureDefKind::Expr(expr)=>{
                            self.analyse_expr_stmt(closure_def.span, expr)?;
                        }
                        ClosureDefKind::Block(block)=>{
                            self.analyse_block(block)?;
                        }
                    }
                    
                }
                else {
                    panic!()
                }
                // lets figure out what the
                
                self.scopes.pop_scope();
                
                std::mem::swap(&mut self.scopes.scopes, &mut scopes);
            }
            // ok now we declare the inputs of the closure on the scope stack
            
        }
        Ok(())
    }
    
    fn analyse_block(&mut self, block: &Block) -> Result<(), LiveError> {
        for stmt in &block.stmts {
            self.analyse_stmt(stmt) ?;
        }
        Ok(())
    }
    
    fn analyse_stmt(&mut self, stmt: &Stmt) -> Result<(), LiveError> {
        match *stmt {
            Stmt::Break {span} => self.analyse_break_stmt(span),
            Stmt::Continue {span} => self.analyse_continue_stmt(span),
            Stmt::For {
                span,
                ident,
                ref from_expr,
                ref to_expr,
                ref step_expr,
                ref block,
            } => self.analyse_for_stmt(span, ident, from_expr, to_expr, step_expr, block),
            Stmt::If {
                span,
                ref expr,
                ref block_if_true,
                ref block_if_false,
            } => self.analyse_if_stmt(span, expr, block_if_true, block_if_false),
            Stmt::Let {
                span,
                ref ty,
                ident,
                ref shadow,
                ref ty_expr,
                ref expr,
            } => self.analyse_let_stmt(span, ty, ident, ty_expr, expr, shadow),
            Stmt::Return {span, ref expr} => self.analyse_return_stmt(span, expr),
            Stmt::Block {span, ref block} => self.analyse_block_stmt(span, block),
            Stmt::Expr {span, ref expr} => self.analyse_expr_stmt(span, expr),
        }
    }
    
    fn analyse_break_stmt(&self, span: Span) -> Result<(), LiveError> {
        if !self.is_inside_loop {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: String::from("break outside loop"),
            } .into());
        }
        Ok(())
    }
    
    fn analyse_continue_stmt(&self, span: Span) -> Result<(), LiveError> {
        if !self.is_inside_loop {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: String::from("continue outside loop"),
            } .into());
        }
        Ok(())
    }
    
    fn analyse_for_stmt(
        &mut self,
        span: Span,
        ident: Ident,
        from_expr: &Expr,
        to_expr: &Expr,
        step_expr: &Option<Expr>,
        block: &Block,
    ) -> Result<(), LiveError> {
        self.ty_checker()
            .ty_check_expr_with_expected_ty(span, from_expr, &Ty::Int) ?;
        let from = self
        .const_evaluator()
            .const_eval_expr(from_expr) ?
        .to_int()
            .unwrap();
        self.dep_analyser().dep_analyse_expr(from_expr);
        self.ty_checker()
            .ty_check_expr_with_expected_ty(span, to_expr, &Ty::Int) ?;
        let to = self
        .const_evaluator()
            .const_eval_expr(to_expr) ?
        .to_int()
            .unwrap();
        self.dep_analyser().dep_analyse_expr(to_expr);
        if let Some(step_expr) = step_expr {
            self.ty_checker()
                .ty_check_expr_with_expected_ty(span, step_expr, &Ty::Int) ?;
            let step = self
            .const_evaluator()
                .const_eval_expr(step_expr) ?
            .to_int()
                .unwrap();
            if step == 0 {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: String::from("step must not be zero"),
                } .into());
            }
            if from < to && step < 0 {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: String::from("step must not be positive"),
                } .into());
            }
            if from > to && step > 0 {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: String::from("step must not be negative"),
                } .into());
            }
            self.dep_analyser().dep_analyse_expr(step_expr);
        }
        self.scopes.push_scope();
        self.scopes.insert_sym(
            span,
            ident,
            ScopeSymKind::Local(Ty::Int),
        );
        let was_inside_loop = self.is_inside_loop;
        self.is_inside_loop = true;
        self.analyse_block(block) ?;
        self.is_inside_loop = was_inside_loop;
        self.scopes.pop_scope();
        Ok(())
    }
    
    fn analyse_if_stmt(
        &mut self,
        span: Span,
        expr: &Expr,
        block_if_true: &Block,
        block_if_false: &Option<Box<Block >>,
    ) -> Result<(), LiveError> {
        self.ty_checker()
            .ty_check_expr_with_expected_ty(span, expr, &Ty::Bool) ?;
        self.const_evaluator().try_const_eval_expr(expr);
        self.const_gatherer().const_gather_expr(expr);
        self.dep_analyser().dep_analyse_expr(expr);
        self.scopes.push_scope();
        self.analyse_block(block_if_true) ?;
        self.scopes.pop_scope();
        if let Some(block_if_false) = block_if_false {
            self.scopes.push_scope();
            self.analyse_block(block_if_false) ?;
            self.scopes.pop_scope();
        }
        Ok(())
    }
    
    fn analyse_let_stmt(
        &mut self,
        span: Span,
        ty: &RefCell<Option<Ty >>,
        ident: Ident,
        ty_expr: &Option<TyExpr>,
        expr: &Option<Expr>,
        shadow: &Cell<Option<ScopeSymShadow>>,
    ) -> Result<(), LiveError> {
        *ty.borrow_mut() = Some(if let Some(ty_expr) = ty_expr {
            if expr.is_none() {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: format!("cannot define an uninitialised variable `{}`", ident),
                });
            }
            let expected_ty = self.ty_checker().ty_check_ty_expr(ty_expr) ?;
            if let Some(expr) = expr {
                let actual_ty =
                self.ty_checker()
                    .ty_check_expr_with_expected_ty(span, expr, &expected_ty) ?;
                self.dep_analyser().dep_analyse_expr(expr);
                actual_ty
            } else {
                expected_ty
            }
            
        } else if let Some(expr) = expr {
            let ty = self.ty_checker().ty_check_expr(expr) ?;
            if ty == Ty::Void {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span,
                    message: String::from("init expression cannot be void")
                });
            }
            self.const_evaluator().try_const_eval_expr(expr);
            self.const_gatherer().const_gather_expr(expr);
            self.dep_analyser().dep_analyse_expr(expr);
            ty
        } else {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: format!("can't infer type of variable `{}`", ident),
            });
        });
        let new_shadow = self.scopes.insert_sym(
            span,
            ident,
            ScopeSymKind::MutLocal(ty.borrow().as_ref().unwrap().clone()),
        );
        shadow.set(Some(new_shadow));
        Ok(())
    }
    
    fn analyse_return_stmt(&mut self, span: Span, expr: &Option<Expr>) -> Result<(), LiveError> {
        
        
        if let Some(expr) = expr {
            self.ty_checker().ty_check_expr_with_expected_ty(
                span,
                expr,
                self.decl.return_ty.borrow().as_ref().unwrap(),
            ) ?;
            
            self.const_evaluator().try_const_eval_expr(expr);
            self.const_gatherer().const_gather_expr(expr);
            self.dep_analyser().dep_analyse_expr(expr);
        } else if self.decl.return_ty.borrow().as_ref().unwrap() != &Ty::Void {
            return Err(LiveError {
                origin: live_error_origin!(),
                span,
                message: String::from("missing return expression"),
            } .into());
        }
        Ok(())
    }
    
    fn analyse_block_stmt(&mut self, _span: Span, block: &Block) -> Result<(), LiveError> {
        self.scopes.push_scope();
        self.analyse_block(block) ?;
        self.scopes.pop_scope();
        Ok(())
    }
    
    fn analyse_expr_stmt(&mut self, _span: Span, expr: &Expr) -> Result<(), LiveError> {
        self.ty_checker().ty_check_expr(expr) ?;
        self.const_evaluator().try_const_eval_expr(expr);
        self.const_gatherer().const_gather_expr(expr);
        self.dep_analyser().dep_analyse_expr(expr);
        Ok(())
    }
}

#[derive(Clone, Copy, Debug)]
enum ShaderKind {
    Vertex,
    Fragment,
}
