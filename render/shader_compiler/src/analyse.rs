use{
    std::{
        cell::{RefCell, Cell},
        collections::{BTreeMap, BTreeSet, HashMap}
    },
    crate::{
        makepad_live_compiler::{
            live_error_origin,
            id,
            LiveRegistry,
            LiveError,
            LiveErrorOrigin,
            LiveId,
            TokenSpan
        },
        shader_ast::*,
        const_eval::ConstEvaluator,
        const_gather::ConstGatherer,
        dep_analyse::DepAnalyser,
        ty_check::TyChecker,
        shader_registry::ShaderRegistry
    }
};


#[derive(Clone, Copy)]
pub struct ShaderAnalyseOptions {
    pub no_const_collapse: bool
}

pub struct StructAnalyser<'a> {
    pub struct_def: &'a StructDef,
    pub scopes: &'a mut Scopes,
    pub live_registry: &'a LiveRegistry,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> StructAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            live_registry: self.live_registry,
            shader_registry: self.shader_registry,
            scopes: self.scopes,
        }
    }
    
    pub fn analyse_struct(&mut self) -> Result<(), LiveError> {
        self.scopes.push_scope();
        self.struct_def.init_analysis();
        // we need to analyse the fields and put them somewhere.
        for field in &self.struct_def.fields {
            self.analyse_field_def(field) ?;
        }
        // first analyse decls
        for fn_node_ptr in &self.struct_def.methods {
            let fn_decl = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            self.analyse_method_decl(fn_decl) ?;
        }
        // now analyse the functions
        for fn_node_ptr in &self.struct_def.methods {
            let fn_def = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            FnDefAnalyser {
                live_registry: self.live_registry,
                shader_registry: self.shader_registry,
                closure_return_ty: None,
                fn_def,
                scopes: &mut self.scopes,
                options: self.options,
                is_inside_loop: false,
            }
            .analyse_fn_def() ?;
        }
        self.scopes.pop_scope();
        Ok(())
    }
    
    fn analyse_field_def(&mut self, field_def: &StructFieldDef) -> Result<(), LiveError> {
        self.ty_checker().ty_check_ty_expr(&field_def.ty_expr) ?;
        // ok so. if this thing depends on structs, lets store them.
        match field_def.ty_expr.ty.borrow().as_ref().unwrap() {
            Ty::Struct(struct_ptr) => {
                self.struct_def.struct_refs.borrow_mut().as_mut().unwrap().insert(*struct_ptr);
            }
            Ty::Array {..} => {
                todo!();
            }
            _ => ()
        }
        Ok(())
    }
    
    fn analyse_method_decl(&mut self, decl: &FnDef) -> Result<(), LiveError> {
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
        *decl.hidden_args.borrow_mut() = Some(BTreeSet::new());
        Ok(())
    }
    
}

pub struct DrawShaderAnalyser<'a> {
    pub draw_shader_def: &'a DrawShaderDef,
    pub scopes: &'a mut Scopes,
    pub live_registry: &'a LiveRegistry,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> DrawShaderAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            live_registry: self.live_registry,
            scopes: self.scopes,
            shader_registry: self.shader_registry,
        }
    }
    
    pub fn analyse_shader(&mut self) -> Result<(), LiveError> {
        self.scopes.push_scope();
        
        //let mut var_inputs = DrawShaderVarInputs::default();
        for field in &self.draw_shader_def.fields {
            self.analyse_field_decl(field) ?;
        }
        
        // first analyse decls
        for fn_node_ptr in &self.draw_shader_def.methods {
            let fn_def = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            self.analyse_method_def(fn_def) ?;
        }
        
        // now analyse the methods
        for fn_node_ptr in &self.draw_shader_def.methods {
            let fn_def = self.shader_registry.all_fns.get(fn_node_ptr).unwrap();
            FnDefAnalyser {
                live_registry: self.live_registry,
                shader_registry: self.shader_registry,
                closure_return_ty: None,
                fn_def,
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
            self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(vertex))).unwrap(),
            &mut vertex_fns,
            &mut all_fns,
        ) ?;
        
        let mut pixel_fns = Vec::new();
        self.analyse_call_tree(
            &mut Vec::new(),
            self.shader_registry.draw_shader_method_decl_from_ident(self.draw_shader_def, Ident(id!(pixel))).unwrap(),
            &mut pixel_fns,
            &mut all_fns,
        ) ?;
        
        // mark all the draw_shader_refs we reference in pixelshaders.
        for pixel_fn in &pixel_fns {
            // if we run into a DrawShaderMethod mark it as
            if let Some(fn_def) = self.shader_registry.all_fns.get(pixel_fn) {
                if let Some(FnSelfKind::DrawShader(_)) = fn_def.self_kind {
                    // lets iterate all
                    for dsr in fn_def.draw_shader_refs.borrow().as_ref().unwrap() {
                        // ok we have a draw shader ident we use, now mark it on our draw_shader_decl.
                        for field in &self.draw_shader_def.fields {
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
        let mut all_live_refs = BTreeMap::new();

        for pixel_fn in &pixel_fns {
            let fn_decl = self.shader_registry.all_fns.get(pixel_fn).unwrap();
            // lets collect all structs
            for struct_ptr in fn_decl.struct_refs.borrow().as_ref().unwrap().iter() {
                let struct_def = self.shader_registry.structs.get(struct_ptr).unwrap();
                self.analyse_struct_tree(&mut Vec::new(), *struct_ptr, struct_def, &mut pixel_structs, &mut all_structs) ?;
            }
        }
        for vertex_fn in &vertex_fns {
            let fn_decl = self.shader_registry.all_fns.get(vertex_fn).unwrap();
            // lets collect all structs
            for struct_ptr in fn_decl.struct_refs.borrow().as_ref().unwrap().iter() {
                let struct_def = self.shader_registry.structs.get(struct_ptr).unwrap();
                self.analyse_struct_tree(&mut Vec::new(), *struct_ptr, struct_def, &mut vertex_structs, &mut all_structs) ?;
            }
        }
        
        for any_fn in all_fns.iter().rev() {
            let fn_def = self.shader_registry.all_fns.get(any_fn).unwrap();
            all_live_refs.extend(fn_def.live_refs.borrow().as_ref().cloned().unwrap());
            // fill in fns where hidden args is none
           // if fn_def.hidden_args.borrow().is_none() {
                self.analyse_hidden_args(fn_def);
           // }
        } 
        
        *self.draw_shader_def.all_live_refs.borrow_mut() = all_live_refs;
        
        *self.draw_shader_def.all_fns.borrow_mut() = all_fns;
        *self.draw_shader_def.vertex_fns.borrow_mut() = vertex_fns;
        *self.draw_shader_def.pixel_fns.borrow_mut() = pixel_fns;
        
        *self.draw_shader_def.all_structs.borrow_mut() = all_structs;
        *self.draw_shader_def.vertex_structs.borrow_mut() = vertex_structs;
        *self.draw_shader_def.pixel_structs.borrow_mut() = pixel_structs;
        Ok(())
    }
    
    fn analyse_hidden_args(&mut self, fn_def: &FnDef) {
        // ok so.. lets build it up
        let mut hidden_args = BTreeSet::new();
        for ident in fn_def.draw_shader_refs.borrow().as_ref().unwrap() {
            let field_def = self.draw_shader_def.fields.iter().find( | field | field.ident == *ident).unwrap();
            match &field_def.kind {
                DrawShaderFieldKind::Geometry {is_used_in_pixel_shader, ..} => {
                    if is_used_in_pixel_shader.get() {
                        hidden_args.insert(HiddenArgKind::Varyings);
                    }
                    else {
                        hidden_args.insert(HiddenArgKind::Geometries);
                    }
                }
                DrawShaderFieldKind::Instance {is_used_in_pixel_shader, ..} => {
                    if is_used_in_pixel_shader.get() {
                        hidden_args.insert(HiddenArgKind::Varyings);
                    }
                    else {
                        hidden_args.insert(HiddenArgKind::Instances);
                    }
                }
                DrawShaderFieldKind::Texture {..} => {
                    hidden_args.insert(HiddenArgKind::Textures);
                }
                DrawShaderFieldKind::Uniform {block_ident, ..} => {
                    hidden_args.insert(HiddenArgKind::Uniform(*block_ident));
                }
                DrawShaderFieldKind::Varying {..} => {
                    hidden_args.insert(HiddenArgKind::Varyings);
                }
            }
        }
        if fn_def.live_refs.borrow().as_ref().unwrap().len() > 0 {
            hidden_args.insert(HiddenArgKind::LiveUniforms);
        }
        // merge in the others
        for callee in fn_def.callees.borrow().as_ref().unwrap().iter() {
            let other_fn_def = self.shader_registry.all_fns.get(callee).unwrap();
            
            hidden_args.extend(other_fn_def.hidden_args.borrow().as_ref().unwrap().iter().cloned());
        }
        *fn_def.hidden_args.borrow_mut() = Some(hidden_args);
    }
    
    
    fn analyse_field_decl(&mut self, decl: &DrawShaderFieldDef) -> Result<(), LiveError> {
        let ty = match decl.kind {
            DrawShaderFieldKind::Geometry {..} => {
                let ty = self.ty_checker().ty_check_ty_expr(&decl.ty_expr) ?;
                match ty {
                    Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 | Ty::Mat4 => {}
                    _ => {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: decl.span.into(),
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
                    Ty::Float | Ty::Vec2 | Ty::Vec3 | Ty::Vec4 | Ty::Mat2 | Ty::Mat3 | Ty::Mat4 | Ty::Enum(_) => {}
                    _ => {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: decl.span.into(),
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
                            span: decl.span.into(),
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
                            span: decl.span.into(),
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
    
    fn analyse_method_def(&mut self, def: &FnDef) -> Result<(), LiveError> {
        for param in &def.params {
            self.ty_checker().ty_check_ty_expr(&param.ty_expr) ?;
        }
        let return_ty = def
            .return_ty_expr
            .as_ref()
            .map( | return_ty_expr | self.ty_checker().ty_check_ty_expr(return_ty_expr))
            .transpose() ?
        .unwrap_or(Ty::Void);
        
        if def.ident == Ident(id!(vertex)) {
            match return_ty {
                Ty::Vec4 => {}
                _ => {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: def.span.into(),
                        message: String::from(
                            "function `vertex` must return a value of type `vec4`",
                        ),
                    })
                }
            }
        } else if def.ident == Ident(id!(pixel)) {
            match return_ty {
                Ty::Vec4 => {}
                _ => {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: def.span.into(),
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
                        span: def.span.into(),
                        message: String::from("functions can't return arrays"),
                    })
                }
                _ => {}
            }
        }
        *def.return_ty.borrow_mut() = Some(return_ty);
        //self.env.insert_sym(decl.span, decl.ident, Sym::Fn).ok();
        Ok(())
    }
    
    fn analyse_struct_tree(
        &self,
        call_stack: &mut Vec<StructPtr>,
        struct_ptr: StructPtr,
        struct_def: &StructDef,
        deps: &mut Vec<StructPtr>,
        all_deps: &mut Vec<StructPtr>,
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
        
        for sub_ptr in struct_def.struct_refs.borrow().as_ref().unwrap().iter() {
            // ok now we need a fn decl for this callee
            let sub_decl = self.shader_registry.structs.get(sub_ptr).unwrap();
            if call_stack.contains(&sub_ptr) {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span: sub_decl.span.into(),
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
        call_stack: &mut Vec<FnPtr>,
        def: &FnDef,
        //callee: Callee,
        deps: &mut Vec<FnPtr>,
        all_deps: &mut Vec<FnPtr>,
    ) -> Result<(), LiveError> {
        // lets see if callee is already in the vec, ifso remove it
        if let Some(index) = deps.iter().position( | v | v == &def.fn_ptr) {
            deps.remove(index);
        }
        deps.push(def.fn_ptr);
        
        if let Some(index) = all_deps.iter().position( | v | v == &def.fn_ptr) {
            all_deps.remove(index);
        }
        all_deps.push(def.fn_ptr);
        
        call_stack.push(def.fn_ptr);
        for callee in def.callees.borrow().as_ref().unwrap().iter() {
            // ok now we need a fn decl for this callee
            let callee_decl = self.shader_registry.all_fns.get(&callee).unwrap();
            if call_stack.contains(&callee_decl.fn_ptr) {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span: def.span.into(),
                    message: format!("function `{}` recursively calls `{}`", def.ident, callee_decl.ident),
                });
            }
            
            self.analyse_call_tree(call_stack, callee_decl, deps, all_deps) ?;
        }
        call_stack.pop();
        
        Ok(())
    }
}

pub struct ConstAnalyser<'a> {
    pub const_def: &'a ConstDef,
    pub scopes: &'a mut Scopes,
    pub live_registry: &'a LiveRegistry,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
}

impl<'a> ConstAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            live_registry: self.live_registry,
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
        let expected_ty = self.ty_checker().ty_check_ty_expr(&self.const_def.ty_expr) ?;
        let actual_ty = self.ty_checker().ty_check_expr_with_expected_ty(
            self.const_def.span,
            &self.const_def.expr,
            &expected_ty,
        ) ?;
        if expected_ty != actual_ty {
            return Err(LiveError {
                origin: live_error_origin!(),
                span: self.const_def.span.into(),
                message: String::from("Declared type and inferred type not the same"),
            } .into());
        }
        self.const_evaluator().const_eval_expr(&self.const_def.expr) ?;
        Ok(())
    }
}

pub struct FnDefAnalyser<'a> {
    pub fn_def: &'a FnDef,
    pub closure_return_ty: Option<&'a RefCell<Option<Ty >> >,
    pub scopes: &'a mut Scopes,
    pub live_registry: &'a LiveRegistry,
    pub shader_registry: &'a ShaderRegistry,
    pub options: ShaderAnalyseOptions,
    pub is_inside_loop: bool,
}

impl<'a> FnDefAnalyser<'a> {
    fn ty_checker(&self) -> TyChecker {
        TyChecker {
            live_registry: self.live_registry,
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
            fn_def: self.fn_def
        }
    }
    
    fn dep_analyser(&self) -> DepAnalyser {
        DepAnalyser {
            shader_registry: self.shader_registry,
            fn_def: self.fn_def,
            scopes: &self.scopes,
        }
    }
    
    pub fn analyse_fn_decl(&mut self) -> Result<(), LiveError> {
        for param in &self.fn_def.params {
            self.ty_checker().ty_check_ty_expr(&param.ty_expr) ?;
        }
        let return_ty = self.fn_def
            .return_ty_expr
            .as_ref()
            .map( | return_ty_expr | self.ty_checker().ty_check_ty_expr(return_ty_expr))
            .transpose() ?
        .unwrap_or(Ty::Void);
        *self.fn_def.return_ty.borrow_mut() = Some(return_ty);
        Ok(())
    }
    
    pub fn analyse_fn_def(&mut self) -> Result<(), LiveError> {
        self.scopes.push_scope();
        for (param_index, param) in self.fn_def.params.iter().enumerate() {
            match &param.ty_expr.kind {
                TyExprKind::ClosureDecl {return_ty, params, ..} => {
                    self.scopes.insert_sym(
                        param.span,
                        param.ident,
                        Ty::ClosureDecl,
                        ScopeSymKind::Closure {
                            param_index,
                            return_ty: return_ty.borrow().clone().unwrap(),
                            params: params.clone()
                        },
                    );
                }
                _ => {
                    let shadow = self.scopes.insert_sym(
                        param.span,
                        param.ident,
                        param.ty_expr.ty.borrow().as_ref().unwrap().clone(),
                        ScopeSymKind::MutLocal,
                    );
                    param.shadow.set(Some(shadow));
                }
            }
            
        }
        *self.fn_def.return_ty.borrow_mut() = Some(
            self.fn_def
                .return_ty_expr
                .as_ref()
                .map( | return_ty_expr | return_ty_expr.ty.borrow().as_ref().unwrap().clone())
                .unwrap_or(Ty::Void),
        );
        self.fn_def.init_analysis();
        self.analyse_block(&self.fn_def.block) ?;
        self.scopes.pop_scope();
        // alright we have closures to analyse
        // let closure_isntances = self.
        // lets move the closures from env to
        // then analyse it
        self.analyse_closures() ?;
        
        // lets build up our fn_args_hidden and combine it
        // with our callees
        
        if let Some(ty_expr) = &self.fn_def.return_ty_expr {
            if let Ty::Void = ty_expr.ty.borrow().as_ref().unwrap() {
            }
            else {
                if !self.fn_def.has_return.get() {
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span: self.fn_def.span.into(),
                        message: format!(
                            "Function has no return",
                        ),
                    });
                }
            }
        }
        
        Ok(())
    }
    
    fn analyse_closures(&mut self) -> Result<(), LiveError> {
        
        let mut closure_sites = self.scopes.closure_sites.replace(Vec::new());
        let mut closure_scopes = self.scopes.closure_scopes.replace(HashMap::new());
        
        for closure_site in &mut closure_sites {
            let fn_decl = self.shader_registry.all_fns.get(&closure_site.call_to).unwrap();
            
            // lets start the closure
            for closure_arg in &closure_site.closure_args {
                
                let mut scopes = closure_scopes.get_mut(&closure_arg.closure_def_index).unwrap();
                // lets swap our scopes for the closure scopes
                std::mem::swap(&mut self.scopes.scopes, &mut scopes);
                
                // ok now we analyse the closure
                // lets fetch the fn_decl
                let closure_def = &self.fn_def.closure_defs[closure_arg.closure_def_index.0];
                let fn_param = &fn_decl.params[closure_arg.param_index];
                
                if let TyExprKind::ClosureDecl {params, return_ty, ..} = &fn_param.ty_expr.kind {
                    self.scopes.clear_referenced_syms();
                    self.scopes.push_scope();
                    // alright we have a fn_decl and a closure_def
                    // lets get the closure-decl
                    if closure_def.params.len() != params.len() {
                        return Err(LiveError {
                            origin: live_error_origin!(),
                            span: closure_def.span.into(),
                            message: format!(
                                "Closure does not have the same number of arguments as function decl: {} expected: {}",
                                closure_def.params.len(),
                                params.len()
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
                            decl_param.ty_expr.ty.borrow().as_ref().unwrap().clone(),
                            ScopeSymKind::MutLocal,
                        );
                        def_param.shadow.set(Some(shadow));
                    }
                    // ok and now we go analyse the body.
                    match &closure_def.kind {
                        ClosureDefKind::Expr(expr) => {
                            self.analyse_expr_stmt(closure_def.span, expr) ?;
                            // check the expr ty vs return ty
                            if expr.ty.borrow().as_ref() != return_ty.borrow().as_ref() {
                                return Err(LiveError {
                                    origin: live_error_origin!(),
                                    span: closure_def.span.into(),
                                    message: format!(
                                        "Closure return type not correct: {} expected: {}",
                                        expr.ty.borrow().as_ref().unwrap(),
                                        return_ty.borrow().as_ref().unwrap()
                                    ),
                                });
                            }
                        }
                        ClosureDefKind::Block(block) => {
                            self.closure_return_ty = Some(return_ty);
                            // ohdear. the return should be checked against the closure
                            // not the fndef.
                            self.analyse_block(block) ?;
                            self.closure_return_ty = None;
                        }
                    }
                    // TODO CHECK THE RETURN TYPE
                    
                    self.scopes.pop_scope();
                    // ok we also have something else.
                    // ok we have to store the variables we have accessed on frpm scope
                    let all_syms = self.scopes.all_referenced_syms();
                    for sym in &all_syms {
                        closure_site.all_closed_over.insert(sym.clone());
                    }
                    *closure_def.closed_over_syms.borrow_mut() = Some(all_syms);
                }
                else {
                    panic!()
                }
                // lets figure out what the
                std::mem::swap(&mut self.scopes.scopes, &mut scopes);
            }
            // ok now we declare the inputs of the closure on the scope stack
        }
        // move or extend
        let mut closure_sites_out = self.fn_def.closure_sites.borrow_mut();
        if closure_sites_out.is_some() {
            closure_sites_out.as_mut().unwrap().extend(closure_sites);
        }
        else {
            *closure_sites_out = Some(closure_sites);
        }
        // recur
        if self.scopes.closure_sites.borrow().len()>0 {
            return Err(LiveError {
                origin: live_error_origin!(),
                span: self.fn_def.span.into(),
                message: format!("Nesting closures is not supported at the moment"),
            });
            
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
            Stmt::Match {
                span,
                ref expr,
                ref matches,
            } => self.analyse_match_stmt(span, expr, matches),
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
    
    fn analyse_break_stmt(&self, span: TokenSpan) -> Result<(), LiveError> {
        if !self.is_inside_loop {
            return Err(LiveError {
                origin: live_error_origin!(),
                span:span.into(),
                message: String::from("break outside loop"),
            } .into());
        }
        Ok(())
    }
    
    fn analyse_continue_stmt(&self, span: TokenSpan) -> Result<(), LiveError> {
        if !self.is_inside_loop {
            return Err(LiveError {
                origin: live_error_origin!(),
                span:span.into(),
                message: String::from("continue outside loop"),
            } .into());
        }
        Ok(())
    }
    
    fn analyse_for_stmt(
        &mut self,
        span: TokenSpan,
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
                    span:span.into(),
                    message: String::from("step must not be zero"),
                } .into());
            }
            if from < to && step < 0 {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span:span.into(),
                    message: String::from("step must not be positive"),
                } .into());
            }
            if from > to && step > 0 {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span:span.into(),
                    message: String::from("step must not be negative"),
                } .into());
            }
            self.dep_analyser().dep_analyse_expr(step_expr);
        }
        self.scopes.push_scope();
        self.scopes.insert_sym(
            span,
            ident,
            Ty::Int,
            ScopeSymKind::Local,
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
        span: TokenSpan,
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
    
    fn analyse_match_stmt(
        &mut self,
        span: TokenSpan,
        expr: &Expr,
        matches: &Vec<Match>,
    ) -> Result<(), LiveError> {
        let ty = self.ty_checker()
            .ty_check_expr(expr) ?;
        // ok so the ty MUST be an Enum
        if let Ty::Enum(live_type) = ty {
            self.const_evaluator().try_const_eval_expr(expr);
            self.const_gatherer().const_gather_expr(expr);
            self.dep_analyser().dep_analyse_expr(expr);
            
            for match_item in matches {
                // lets fetch our Enum + Variant and see if its the same live_type
                let shader_enum = self.shader_registry.enums.get(&live_type).unwrap();
                // ok so.. our match_item
                if match_item.enum_name.0 != shader_enum.enum_name{
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span:span.into(),
                        message: format!("Enum name mismatched, expected {} got {}", shader_enum.enum_name, match_item.enum_name.0),
                    } .into())
                } 
                
                if let Some(pos) = shader_enum.variants.iter().position( | id | *id == match_item.enum_variant.0) {
                    match_item.enum_value.set(Some(pos));
                }
                else{
                    return Err(LiveError {
                        origin: live_error_origin!(),
                        span:span.into(),
                        message: format!("Variant not found on enum {}::{}", match_item.enum_name.0, match_item.enum_variant.0),
                    } .into())
                }
                // lets see if we have the right name
                self.scopes.push_scope();
                self.analyse_block(&match_item.block) ?;
                self.scopes.pop_scope();
            }
            Ok(())
        }
        else {
            Err(LiveError {
                origin: live_error_origin!(),
                span:span.into(),
                message: String::from("Can only match on enum types"),
            } .into())
        }
    }
    
    fn analyse_let_stmt(
        &mut self,
        span: TokenSpan,
        ty: &RefCell<Option<Ty >>,
        ident: Ident,
        ty_expr: &Option<TyExpr>,
        expr: &Option<Expr>,
        shadow: &Cell<Option<ScopeSymShadow >>,
    ) -> Result<(), LiveError> {
        *ty.borrow_mut() = Some(if let Some(ty_expr) = ty_expr {
            if expr.is_none() {
                return Err(LiveError {
                    origin: live_error_origin!(),
                    span:span.into(),
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
                    span:span.into(),
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
                span:span.into(),
                message: format!("can't infer type of variable `{}`", ident),
            });
        });
        let new_shadow = self.scopes.insert_sym(
            span,
            ident,
            ty.borrow().as_ref().unwrap().clone(),
            ScopeSymKind::MutLocal,
        );
        shadow.set(Some(new_shadow));
        Ok(())
    }
    
    fn analyse_return_stmt(&mut self, span: TokenSpan, expr: &Option<Expr>) -> Result<(), LiveError> {
        
        self.fn_def.has_return.set(true);
        if let Some(expr) = expr {
            if let Some(ty) = self.closure_return_ty {
                self.ty_checker().ty_check_expr_with_expected_ty(
                    span,
                    expr,
                    ty.borrow().as_ref().unwrap()
                ) ?;
            }
            else {
                self.ty_checker().ty_check_expr_with_expected_ty(
                    span,
                    expr,
                    self.fn_def.return_ty.borrow().as_ref().unwrap()
                ) ?;
            }
            
            self.const_evaluator().try_const_eval_expr(expr);
            self.const_gatherer().const_gather_expr(expr);
            self.dep_analyser().dep_analyse_expr(expr);
        } else if self.fn_def.return_ty.borrow().as_ref().unwrap() != &Ty::Void {
            return Err(LiveError {
                origin: live_error_origin!(),
                span:span.into(),
                message: String::from("missing return expression"),
            } .into());
        }
        Ok(())
    }
    
    fn analyse_block_stmt(&mut self, _span: TokenSpan, block: &Block) -> Result<(), LiveError> {
        self.scopes.push_scope();
        self.analyse_block(block) ?;
        self.scopes.pop_scope();
        Ok(())
    }
    
    fn analyse_expr_stmt(&mut self, _span: TokenSpan, expr: &Expr) -> Result<(), LiveError> {
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
