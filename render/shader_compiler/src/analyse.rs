use crate::shaderast::*;
use crate::const_eval::ConstEvaluator;
use crate::const_gather::ConstGatherer;
use crate::dep_analyse::DepAnalyser;
use crate::env::Env;
use crate::env::LocalSym;
use crate::shaderast::Ident;
use crate::shaderast::{Ty, TyExpr};
use crate::ty_check::TyChecker;
use crate::shaderregistry::ShaderRegistry;

use makepad_live_parser::LiveError;
use makepad_live_parser::LiveErrorOrigin;
use makepad_live_parser::live_error_origin;
use makepad_live_parser::id;
use makepad_live_parser::Id;
use makepad_live_parser::FileId;
use makepad_live_parser::Span;

use std::cell::RefCell;
/*
pub fn analyse(shader: &ShaderAst, base_props:&[PropDef], sub_props: &[&PropDef], gather_all: bool) -> Result<(), Error> {
    let builtins = builtin::generate_builtins();
    let mut env = Env::new();
    env.push_scope();

    for &ident in builtins.keys() {
        env.insert_sym(Span::default(), ident, Sym::Builtin)?;
    }
       

    for prop in sub_props {
        env.insert_sym(
            Span::default(),
            Ident::new(&prop.ident),
            Sym::TyVar {
                ty: prop.prop_id.shader_ty(),
            },
        )?;
    }
    env.push_scope();
    ShaderAnalyser {
        builtins,
        shader,
        env,
        gather_all,
    }
    .analyse_shader()
}*/

#[derive(Debug, Clone, Copy)]
pub struct ShaderAnalyseOptions {
    pub no_const_collapse: bool
}

#[derive(Debug, Clone, Copy)]
pub struct ShaderGenerateOptions {
    pub const_file_id: Option<FileId>
}

#[derive(Debug)]
pub struct StructAnalyser<'a> {
    pub struct_decl: &'a StructDecl,
    pub env: &'a mut Env,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> StructAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            shader_registry: self.shader_registry,
            env: self.env,
        }
    }
    /*
    fn const_gatherer(&self) -> ConstGatherer {
        ConstGatherer {
            env: self.env,
        }
    }*/
    
    pub fn analyse_struct(&mut self) -> Result<(), LiveError> {
        self.env.push_scope();
        self.struct_decl.init_analysis();
        // we need to analyse the fields and put them somewhere.
        for field in &self.struct_decl.fields {
            self.analyse_field_decl(field) ?;
        }
        // first analyse decls
        for decl in &self.struct_decl.methods {
            self.analyse_method_decl(decl) ?;
        }
        // now analyse the functions
        for decl in &self.struct_decl.methods {
            FnDefAnalyser {
                decl,
                env: &mut self.env,
                shader_registry: self.shader_registry,
                options: self.options,
                is_inside_loop: false,
            }
            .analyse_fn_def() ?;
        }
        self.env.pop_scope();
        Ok(())
    }
    /*
    fn analyse_const_decl(&mut self, decl: &ConstDecl) -> Result<(), LiveError> {
        let expected_ty = self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
        let actual_ty = self.ty_checker().ty_check_expr_with_expected_ty(
            decl.span,
            &decl.expr,
            &expected_ty,
        ) ?;
        self.const_evaluator().const_eval_expr(&decl.expr) ?;
        self.env.insert_sym(
            decl.span,
            decl.ident.to_ident_path(),
            Sym::Var {
                is_mut: false,
                ty: actual_ty,
                kind: VarKind::Const,
            },
        )
    }*/
    
    fn analyse_field_decl(&mut self, decl: &FieldDecl) -> Result<(), LiveError> {
        self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
        // ok so. if this thing depends on structs, lets store them.
        match decl.ty_expr.ty.borrow().as_ref().unwrap(){
            Ty::Struct(struct_ptr)=>{
                self.struct_decl.struct_refs.borrow_mut().as_mut().unwrap().insert(*struct_ptr);
            }
            Ty::Array{..}=>{
                todo!();
            }
            _=>()
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
        //self.env.insert_sym(decl.span, decl.ident, Sym::Fn).ok();
        Ok(())
    }
    
}


#[derive(Debug)]
pub struct DrawShaderAnalyser<'a> {
    //pub body: &'a ShaderBody,
    pub draw_shader_decl: &'a DrawShaderDecl,
    pub env: &'a mut Env,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> DrawShaderAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            env: self.env,
            shader_registry: self.shader_registry,
        }
    }
    /*
    fn const_evaluator(&self) -> ConstEvaluator {
        ConstEvaluator {
            env: self.env,
            no_const_collapse: self.options.no_const_collapse
        }
    }*/
    /*
    fn const_gatherer(&self) -> ConstGatherer {
        ConstGatherer {
            env: self.env,
        }
    }*/
    
    pub fn analyse_shader(&mut self, shader_node_ptr:DrawShaderNodePtr) -> Result<(), LiveError> {
        self.env.push_scope();

        for decl in &self.draw_shader_decl.fields {
            self.analyse_field_decl(decl) ?;
        }
        // first analyse decls
        for decl in &self.draw_shader_decl.methods {
            self.analyse_method_decl(decl) ?;
        }
        
        // now analyse the methods
        for decl in &self.draw_shader_decl.methods {
            FnDefAnalyser {
                shader_registry: self.shader_registry,
                decl,
                env: &mut self.env,
                options: self.options,
                is_inside_loop: false, 
            }
            .analyse_fn_def() ?;
        }

        self.env.pop_scope();
        
        let mut all_fns = Vec::new();
        let mut vertex_fns = Vec::new();
        // we should insert our vertex call
        self.analyse_call_tree(
            &mut Vec::new(),
            self.draw_shader_decl.find_method(Ident(id!(vertex))).unwrap(),
            Callee::DrawShaderMethod{shader_node_ptr, ident:Ident(id!(vertex))},
            &mut vertex_fns,
            &mut all_fns,
        ) ?;

        let mut pixel_fns = Vec::new();
        self.analyse_call_tree(
            &mut Vec::new(),
            self.draw_shader_decl.find_method(Ident(id!(pixel))).unwrap(),
            Callee::DrawShaderMethod{shader_node_ptr, ident:Ident(id!(pixel))},
            &mut pixel_fns,
            &mut all_fns,
        ) ?;

        // mark all the draw_shader_refs we reference in pixelshaders.
        for pixel_fn in &pixel_fns{
            // if we run into a DrawShaderMethod mark it as 
            if let Callee::DrawShaderMethod{shader_node_ptr, ident} = pixel_fn{
                if let Some(fn_decl) = self.shader_registry.draw_shader_method_from_ptr(*shader_node_ptr, *ident){
                    // lets iterate all 
                    for dsr in fn_decl.draw_shader_refs.borrow().as_ref().unwrap(){
                        // ok we have a draw shader ident we use, now mark it on our draw_shader_decl.
                        for field in &self.draw_shader_decl.fields{
                            if field.ident == *dsr{ // we found it
                                match &field.kind{
                                    DrawShaderFieldKind::Geometry{ref is_used_in_pixel_shader, ..}=>{
                                        is_used_in_pixel_shader.set(true);
                                    }
                                    DrawShaderFieldKind::Instance{ref is_used_in_pixel_shader, ..}=>{
                                        is_used_in_pixel_shader.set(true);
                                    }
                                    _=>()
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
        
        for pixel_fn in &pixel_fns{
            let fn_decl = self.shader_registry.fn_decl_from_callee(pixel_fn).unwrap();
            // lets collect all structs
            for struct_ptr in fn_decl.struct_refs.borrow().as_ref().unwrap().iter(){
                let struct_decl = self.shader_registry.structs.get(struct_ptr).unwrap();
                self.analyse_struct_tree(&mut Vec::new(), *struct_ptr, struct_decl, &mut pixel_structs, &mut all_structs)?;
            }
        }
        for vertex_fn in &vertex_fns{
            let fn_decl = self.shader_registry.fn_decl_from_callee(vertex_fn).unwrap();
            // lets collect all structs
            for struct_ptr in fn_decl.struct_refs.borrow().as_ref().unwrap().iter(){
                let struct_decl = self.shader_registry.structs.get(struct_ptr).unwrap();
                self.analyse_struct_tree(&mut Vec::new(), *struct_ptr, struct_decl, &mut vertex_structs, &mut all_structs)?;
            }
        }
        
        *self.draw_shader_decl.all_fns.borrow_mut() = all_fns;
        *self.draw_shader_decl.vertex_fns.borrow_mut() = vertex_fns;
        *self.draw_shader_decl.pixel_fns.borrow_mut() = pixel_fns;

        *self.draw_shader_decl.all_structs.borrow_mut() = all_structs;
        *self.draw_shader_decl.vertex_structs.borrow_mut() = vertex_structs;
        *self.draw_shader_decl.pixel_structs.borrow_mut() = pixel_structs;
        /*
        *self.draw_shader_decl.const_table.borrow_mut() = 
            Some(self.env.const_table.borrow_mut().take().unwrap());
        
        *self.draw_shader_decl.const_table_spans.borrow_mut() = 
            Some(self.env.const_table_spans.borrow_mut().take().unwrap());
        */
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
    
    /*
    fn analyse_struct_decl(&mut self, decl: &StructDecl) -> Result<(), LiveError> {
        for field in &decl.fields {
            self.ty_checker().ty_check_ty_expr(&field.ty_expr) ?;
        }
        self.env.insert_sym(
            decl.span,
            decl.ident.to_ident_path(),
            Sym::TyVar {
                ty: Ty::Struct {ident: decl.ident},
            },
        )
    }*/
    
    fn analyse_struct_tree(
        &self,
        call_stack: &mut Vec<StructNodePtr>,
        struct_ptr: StructNodePtr,
        struct_decl: &StructDecl,
        deps: &mut Vec<StructNodePtr>,
        all_deps: &mut Vec<StructNodePtr>,
    ) -> Result<(), LiveError> {
        // lets see if callee is already in the vec, ifso remove it
        if let Some(index) = deps.iter().position(|v| v == &struct_ptr){
            deps.remove(index);
        }
        deps.push(struct_ptr);

        if let Some(index) = all_deps.iter().position(|v| v == &struct_ptr){
            all_deps.remove(index);
        }
        all_deps.push(struct_ptr);

        call_stack.push(struct_ptr);
        
        for sub_ptr in struct_decl.struct_refs.borrow().as_ref().unwrap().iter() {
            // ok now we need a fn decl for this callee
            let sub_decl = self.shader_registry.structs.get(sub_ptr).unwrap();
            if call_stack.contains(&sub_ptr) {
                return Err(LiveError {
                    origin:live_error_origin!(),
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
        callee: Callee,
        deps: &mut Vec<Callee>,
        all_deps: &mut Vec<Callee>,
    ) -> Result<(), LiveError> {
        // lets see if callee is already in the vec, ifso remove it
        if let Some(index) = deps.iter().position(|v| v == &callee){
            deps.remove(index);
        }
        deps.push(callee.clone());

        if let Some(index) = all_deps.iter().position(|v| v == &callee){
            all_deps.remove(index);
        }
        all_deps.push(callee.clone());

        call_stack.push(decl.fn_node_ptr);
        for callee in decl.callees.borrow().as_ref().unwrap().iter() {
            // ok now we need a fn decl for this callee
            let callee_decl = self.shader_registry.fn_decl_from_callee(callee).unwrap();
            if call_stack.contains(&callee_decl.fn_node_ptr) {
                return Err(LiveError {
                    origin:live_error_origin!(),
                    span: decl.span,
                    message: format!("function `{}` recursively calls `{}`", decl.ident, callee_decl.ident),
                });
            }
           
            self.analyse_call_tree(call_stack, callee_decl, *callee, deps, all_deps) ?;
        }
        call_stack.pop();

        Ok(())
    }
    /*
    fn propagate_deps(&mut self, visited: &mut HashSet<IdentPath>, decl: &FnDecl) -> Result<(), LiveError> {
        if visited.contains(&decl.ident_path) {
            return Ok(());
        }
        for &callee in decl.callees.borrow().as_ref().unwrap().iter() {
            let callee_decl = self.shader.shader_body.find_fn_decl(callee).unwrap();
            self.propagate_deps(visited, callee_decl) ?;
            decl.uniform_block_deps
                .borrow_mut()
                .as_mut()
                .unwrap()
                .extend(callee_decl.uniform_block_deps.borrow().as_ref().unwrap());
            decl.has_texture_deps.set(Some(
                decl.has_texture_deps.get().unwrap() || callee_decl.has_texture_deps.get().unwrap(),
            ));
            decl.geometry_deps
                .borrow_mut()
                .as_mut()
                .unwrap()
                .extend(callee_decl.geometry_deps.borrow().as_ref().unwrap());
            decl.instance_deps
                .borrow_mut()
                .as_mut()
                .unwrap()
                .extend(callee_decl.instance_deps.borrow().as_ref().unwrap());
            decl.has_varying_deps.set(Some(
                decl.has_varying_deps.get().unwrap() || callee_decl.has_varying_deps.get().unwrap(),
            ));
            decl.builtin_deps.borrow_mut().as_mut().unwrap().extend(
                callee_decl
                    .builtin_deps
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .iter()
                    .cloned(),
            );
            decl.cons_fn_deps.borrow_mut().as_mut().unwrap().extend(
                callee_decl
                    .cons_fn_deps
                    .borrow()
                    .as_ref()
                    .unwrap()
                    .iter()
                    .cloned(),
            );
        }
        if decl.is_used_in_vertex_shader.get().unwrap()
            && decl.is_used_in_fragment_shader.get().unwrap()
        {
            if !decl.geometry_deps.borrow().as_ref().unwrap().is_empty() {
                return Err(LiveError {
                    origin:live_error_origin!(),
                    span: decl.span,
                    message: format!(
                        "function `{}` can't access any geometries, since it's used in both the vertex and fragment shader",
                        decl.ident_path
                    ),
                });
            }
            if !decl.instance_deps.borrow().as_ref().unwrap().is_empty() {
                return Err(LiveError {
                    origin:live_error_origin!(),
                    span: decl.span,
                    message: format!(
                        "function `{}` can't access any instances, since it's used in both the vertex and fragment shader",
                        decl.ident_path
                    ),
                });
            }
            if decl.has_varying_deps.get().unwrap() {
                return Err(LiveError {
                    origin:live_error_origin!(),
                    span: decl.span,
                    message: format!(
                        "function `{}` can't access any varyings, since it's used in both the vertex and fragment shader",
                        decl.ident_path
                    ),
                });
            }
        }
        visited.insert(decl.ident_path);
        Ok(())
    }*/
}



#[derive(Debug)]
pub struct ConstAnalyser<'a> {
    pub decl: &'a ConstDecl,
    pub env: &'a mut Env,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> ConstAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            shader_registry: self.shader_registry,
            env: self.env,
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
    pub env: &'a mut Env,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
    pub is_inside_loop: bool,
}



impl<'a> FnDefAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            shader_registry: self.shader_registry,
            env: self.env,
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
            env: &self.env,
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
        self.env.push_scope();
        for param in &self.decl.params {
            self.env.insert_sym(
                param.span,
                param.ident,
                LocalSym {
                    is_mut: true,
                    ty: param.ty_expr.ty.borrow().as_ref().unwrap().clone(),
                },
            ) ?;
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
        self.env.pop_scope();
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
                ref ty_expr,
                ref expr,
            } => self.analyse_let_stmt(span, ty, ident, ty_expr, expr),
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
        self.env.push_scope();
        self.env.insert_sym(
            span,
            ident,
            LocalSym {
                is_mut: false,
                ty: Ty::Int,
            },
        ) ?;
        let was_inside_loop = self.is_inside_loop;
        self.is_inside_loop = true;
        self.analyse_block(block) ?;
        self.is_inside_loop = was_inside_loop;
        self.env.pop_scope();
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
        self.env.push_scope();
        self.analyse_block(block_if_true) ?;
        self.env.pop_scope();
        if let Some(block_if_false) = block_if_false {
            self.env.push_scope();
            self.analyse_block(block_if_false) ?;
            self.env.pop_scope();
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
    ) -> Result<(), LiveError> {
        *ty.borrow_mut() = Some(if let Some(ty_expr) = ty_expr {
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
        self.env.insert_sym(
            span,
            ident,
            LocalSym {
                is_mut: true,
                ty: ty.borrow().as_ref().unwrap().clone(),
            },
        )
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
        self.env.push_scope();
        self.analyse_block(block) ?;
        self.env.pop_scope();
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
