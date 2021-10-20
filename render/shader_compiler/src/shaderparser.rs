use std::iter::Cloned;
use std::slice::Iter;
use std::cell::Cell;
use std::cell::RefCell;
use makepad_live_parser::*;
use crate::shaderast::*;
use crate::shaderregistry::ShaderRegistry;
use crate::shaderregistry::LiveNodeFindResult;

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Ord, PartialOrd)]
pub enum ShaderParserDep {
    Const(ConstNodePtr),
    Struct(StructNodePtr),
    Function(Option<StructNodePtr>, FnNodePtr)
}

pub struct ShaderParser<'a> {
    pub token_index: usize,
    pub file_id: FileId,
    pub tokens_with_span: Cloned<Iter<'a, TokenWithSpan >>,
    pub live_scope: &'a [LiveScopeItem],
    pub shader_registry: &'a ShaderRegistry,
    pub type_deps: &'a mut Vec<ShaderParserDep>,
    pub closure_defs: Vec<ClosureDef>,
    pub token_with_span: TokenWithSpan,
    pub self_kind: Option<FnSelfKind>,
    pub end: usize,
}

impl<'a> ShaderParser<'a> {
    pub fn new(
        shader_registry: &'a ShaderRegistry,
        tokens: &'a [TokenWithSpan],
        live_scope: &'a [LiveScopeItem],
        type_deps: &'a mut Vec<ShaderParserDep>,
        self_kind: Option<FnSelfKind>
        
    ) -> Self {
        let mut tokens_with_span = tokens.iter().cloned();
        let token_with_span = tokens_with_span.next().unwrap();
        ShaderParser {
            closure_defs:Vec::new(),
            shader_registry,
            file_id: FileId::default(),
            live_scope,
            type_deps,
            tokens_with_span,
            token_with_span,
            token_index: 0,
            end: 0,
            self_kind
        }
    }
}

impl<'a> ShaderParser<'a> {
    
    fn peek_span(&self) -> Span {
        self.token_with_span.span
    }
    
    fn peek_token(&self) -> Token {
        self.token_with_span.token
    }
    
    fn eat_token(&mut self) -> Token {
        let token = self.peek_token();
        self.skip_token();
        token
    }
    
    fn skip_token(&mut self) {
        self.end = self.token_with_span.span.end() as usize;
        self.token_with_span = self.tokens_with_span.next().unwrap();
        self.token_index += 1;
    }
    
    fn error(&mut self, origin: LiveErrorOrigin, message: String) -> LiveError {
        LiveError {
            origin,
            span: self.token_with_span.span,
            message,
        }
    }
    
    fn end(&self) -> usize {
        self.end
    }
    
    fn token_end(&self) -> usize {
        self.token_with_span.span.end()
    }
    
    fn accept_ident(&mut self) -> Option<Ident> {
        if let Token::Ident(id) = self.peek_token() {
            self.skip_token();
            Some(Ident(id))
        }
        else {
            None
        }
    }
    
    //fn expect_decl(&mut self) -> Result<Decl, LiveError> {
    // }
    
    fn expect_ident_path(&mut self) -> Result<IdentPath, LiveError> {
        let mut ident_path = IdentPath::default();
        let span = self.begin_span();
        match self.peek_token() {
            
            Token::Ident(ident) => {
                self.skip_token();
                ident_path.push(Ident(ident));
            },
            token => {
                return Err(span.error(self, live_error_origin!(), format!("expected ident_path, unexpected token `{}`", token).into()));
            }
        };
        
        loop {
            if !self.accept_token(Token::Punct(id!(::))) {
                return Ok(ident_path);
            }
            match self.peek_token() {
                Token::Ident(ident) => {
                    self.skip_token();
                    if !ident_path.push(Ident(ident)) {
                        return Err(span.error(self, live_error_origin!(), format!("identifier too long `{}`", ident_path).into()));
                    }
                },
                _ => {
                    return Ok(ident_path);
                }
            }
        }
    }
    /*
    fn accept_id(&mut self, what_id: Id) -> bool {
        match self.peek_token() {
            Token::Ident(id) if id == what_id => {
                self.skip_token();
                true
            }
            Token::Punct(id) if id == what_id => {
                self.skip_token();
                true
            }
            _ => false
        }
    }
    
    fn expect_id(&mut self, what_id: Id) -> Result<(), LiveError> {
        let token = self.peek_token();
        match token {
            Token::Ident(id) if id == what_id => {
                self.skip_token();
                Ok(())
            }
            Token::Punct(id) if id == what_id => {
                self.skip_token();
                Ok(())
            }
            _ => Err(self.error(format!("expected id {}, unexpected token `{}`", what_id, token)))
        }
    }*/
    
    fn accept_token(&mut self, token: Token) -> bool {
        if self.peek_token() != token {
            return false;
        }
        self.skip_token();
        true
    }
    
    fn expect_ident(&mut self) -> Result<Ident, LiveError> {
        match self.peek_token() {
            Token::Ident(id) => {
                self.skip_token();
                Ok(Ident(id))
            }
            token => Err(self.error(live_error_origin!(), format!("expected ident, unexpected token `{}`", token))),
        }
    }
    
    fn expect_token(&mut self, expected: Token) -> Result<(), LiveError> {
        let actual = self.peek_token();
        if actual != expected {
            return Err(self.error(live_error_origin!(), format!("expected {} unexpected token `{}`", expected, actual)));
        }
        self.skip_token();
        Ok(())
    }
    
    fn begin_span(&self) -> SpanTracker {
        SpanTracker {
            file_id: self.token_with_span.span.file_id(),
            start: self.token_with_span.span.start(),
        }
    }
    
    // lets parse a function.
    pub fn expect_self_decl(&mut self, ident: Ident, decl_node_ptr: LivePtr) -> Result<Option<DrawShaderFieldDef>, LiveError> {
        let span = self.begin_span();
        let decl_ty = self.expect_ident() ?;
        let decl_name = self.expect_ident() ?;
        if decl_name != ident {
            panic!()
        }
        self.expect_token(Token::Punct(id!(:))) ?;
        // now we expect a type
        let ty_expr = self.expect_ty_expr() ?;
        match decl_ty {
            Ident(id!(geometry)) => {
                return span.end(self, | span | Ok(Some(DrawShaderFieldDef {
                    kind: DrawShaderFieldKind::Geometry {
                        is_used_in_pixel_shader: Cell::new(false),
                        var_def_node_ptr: Some(VarDefNodePtr(decl_node_ptr)),
                    },
                    span,
                    ident,
                    ty_expr
                })))
            }
            Ident(id!(instance)) => {
                return span.end(self, | span | Ok(Some(DrawShaderFieldDef {
                    kind: DrawShaderFieldKind::Instance {
                        is_used_in_pixel_shader: Cell::new(false),
                        var_def_node_ptr: Some(VarDefNodePtr(decl_node_ptr)),
                        //input_type: DrawShaderInputType::VarDef(decl_node_ptr),
                    },
                    span,
                    ident,
                    ty_expr
                })))
            }
            Ident(id!(uniform)) => {
                let block_ident = if self.accept_token(Token::Ident(id!(in))) {
                    self.expect_ident() ?
                }
                else {
                    Ident(id!(default))
                };
                return span.end(self, | span | Ok(Some(DrawShaderFieldDef {
                    kind: DrawShaderFieldKind::Uniform {
                        var_def_node_ptr: Some(VarDefNodePtr(decl_node_ptr)),
                        //input_type: DrawShaderInputType::VarDef(decl_node_ptr),
                        block_ident,
                    },
                    span,
                    ident,
                    ty_expr
                })))
            }
            Ident(id!(varying)) => {
                return span.end(self, | span | Ok(Some(DrawShaderFieldDef {
                    kind: DrawShaderFieldKind::Varying {
                        var_def_node_ptr: VarDefNodePtr(decl_node_ptr),
                    },
                    span,
                    ident,
                    ty_expr
                })))
            }
            Ident(id!(texture)) => {
                return span.end(self, | span | Ok(Some(DrawShaderFieldDef {
                    kind: DrawShaderFieldKind::Texture {
                        var_def_node_ptr: Some(VarDefNodePtr(decl_node_ptr)),
                        //input_type: DrawShaderInputType::VarDef(decl_node_ptr),
                    },
                    span,
                    ident,
                    ty_expr
                })))
            }
            Ident(id!(const)) => {
                return Ok(None)
            }
            _ => {
                return Err(span.error(self, live_error_origin!(), format!("unexpected decl type `{}`", decl_ty).into()))
            }
        }
    }
    
    // lets parse a function.
    pub fn expect_const_def(&mut self, ident: Ident) -> Result<ConstDef, LiveError> {
        let span = self.begin_span();
        let decl_ty = self.expect_ident() ?;
        let decl_name = self.expect_ident() ?;
        if decl_name != ident {
            panic!()
        }
        self.expect_token(Token::Punct(id!(:))) ?;
        // now we expect a type
        let ty_expr = self.expect_ty_expr() ?;
        self.expect_token(Token::Punct(id!( =))) ?;
        
        let expr = self.expect_expr() ?;
        
        if decl_ty != Ident(id!(const)) {
            panic!()
        }
        
        // ok lets parse the value
        return span.end(self, | span | Ok(ConstDef {
            span,
            ident,
            ty_expr,
            expr
        }))
    }
    
    // lets parse a function.
    pub fn expect_field(&mut self, ident: Ident, var_def_node_ptr: VarDefNodePtr) -> Result<Option<StructFieldDef>, LiveError> {
        let span = self.begin_span();
        let decl_ty = self.expect_ident() ?;
        let decl_name = self.expect_ident() ?;
        if decl_name != ident {
            panic!()
        }
        self.expect_token(Token::Punct(id!(:))) ?;
        // now we expect a type
        let ty_expr = self.expect_ty_expr() ?;
        match decl_ty {
            Ident(id!(field)) => {
                return span.end(self, | span | Ok(Some(StructFieldDef {
                    var_def_node_ptr,
                    span,
                    ident,
                    ty_expr
                })))
            }
            Ident(id!(const)) => {
                return Ok(None)
            },
            _ => {
                return Err(span.error(self, live_error_origin!(), format!("unexpected decl type in struct `{}`", decl_ty).into()))
            }
        }
    }
    
    // lets parse a function.
    pub fn expect_method_def(mut self, fn_node_ptr: FnNodePtr, ident: Ident) -> Result<Option<FnDef>, LiveError> {
        let span = self.begin_span();
        
        self.expect_token(Token::OpenParen) ?;
        let mut params = Vec::new();
        if !self.accept_token(Token::CloseParen) {
            
            let span = self.begin_span();
            let is_inout = self.accept_token(Token::Ident(id!(inout)));
            
            if self.peek_token() != Token::Ident(id!(self)){
                return Ok(None)
            }
            self.skip_token();
            //self.expect_token(token_ident!(self)) ?;
            
            let kind = self.self_kind.unwrap().to_ty_expr_kind();
            params.push(span.end(&mut self, | span | Param {
                span,
                is_inout,
                ident: Ident(id!(self)),
                shadow: Cell::new(None),
                ty_expr: TyExpr {
                    span,
                    ty: RefCell::new(None),
                    kind
                },
            }));
            
            while self.accept_token(Token::Punct(id!(,))) {
                params.push(self.expect_param() ?);
            }
            self.expect_token(Token::CloseParen) ?;
        }
        let return_ty_expr = if self.accept_token(Token::Punct(id!( ->))) {
            Some(self.expect_ty_expr() ?)
        } else {
            None
        };
        let block = self.expect_block() ?;
        let self_kind = self.self_kind.clone();
        let span = span.end(&mut self, | span | span);
        Ok(Some(FnDef::new(
            fn_node_ptr,
            span,
            ident,
            self_kind,
            params,
            return_ty_expr,
            block,
            self.closure_defs
        )))
    }
    
    // lets parse a function.
    pub fn expect_plain_fn_def(mut self, fn_node_ptr: FnNodePtr, ident: Ident) -> Result<FnDef, LiveError> {
        let span = self.begin_span();
        
        self.expect_token(Token::OpenParen) ?;
        let mut params = Vec::new();
        if !self.accept_token(Token::CloseParen) {
            if self.peek_token() == Token::Ident(id!(self)){
                return Err(span.error(&mut self, live_error_origin!(), format!("use of self not allowed in plain function").into()))
            }
            let param = self.expect_param() ?;
            
            params.push(param);
            while self.accept_token(Token::Punct(id!(,))) {
                params.push(self.expect_param() ?);
            }
            self.expect_token(Token::CloseParen) ?;
        }
        let return_ty_expr = if self.accept_token(Token::Punct(id!( ->))) {
            Some(self.expect_ty_expr() ?)
        } else {
            None
        };
        let block = self.expect_block() ?;
        let self_kind = self.self_kind.clone();
        let span = span.end(&mut self, | span | span);
        Ok(FnDef::new(
            fn_node_ptr,
            span,
            ident,
            self_kind,
            params,
            return_ty_expr,
            block,
            self.closure_defs
        ))
    }
    
    fn expect_ty_expr(&mut self) -> Result<TyExpr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_prim_ty_expr() ?;
        if self.accept_token(Token::OpenBracket) {
            let elem_ty_expr = Box::new(acc);
            let token = self.peek_token();
            if let Some(Lit::Int(len)) = Lit::from_token(token) {
                self.skip_token();
                self.expect_token(Token::CloseBracket) ?;
                acc = span.end(self, | span | TyExpr {
                    ty: RefCell::new(None),
                    span,
                    kind: TyExprKind::Array {
                        elem_ty_expr,
                        len: len as u32,
                    },
                });
            }
            else {
                return Err(span.error(self, live_error_origin!(), format!("unexpected token `{}`", token).into()))
            }
        }
        Ok(acc)
    }
    
    fn scan_scope_for_live_ptr(&mut self, id: Id) -> Option<LivePtr> {
        for item in self.live_scope.iter().rev() {
            if item.id == id {
                let full_ptr = match item.target {
                    LiveScopeTarget::LivePtr(live_ptr) => live_ptr,
                    LiveScopeTarget::LocalPtr(local_ptr) => LivePtr {
                        file_id: self.file_id,
                        local_ptr
                    }
                };
                return Some(full_ptr)
            }
        }
        return None
    }
    
    
    fn scan_scope_for_struct(&mut self, id: Id) -> bool {
        if let Some(full_node_ptr) = self.scan_scope_for_live_ptr(id) {
            let ptr = ShaderParserDep::Struct(StructNodePtr(full_node_ptr));
            
            if !self.type_deps.contains(&ptr) {
                self.type_deps.push(ptr);
            }
            return true
        }
        return false
    }
    
    fn expect_prim_ty_expr(&mut self) -> Result<TyExpr, LiveError> {
        let span = self.begin_span();
        match self.peek_token() {
            Token::Ident(id) => {
                // properly parse type here
                if id == id!(fn) {
                    self.skip_token();
                    // ok now we parse (ident:ty, ty)
                    self.expect_token(Token::OpenParen) ?;
                    let mut params = Vec::new();
                    if !self.accept_token(Token::CloseParen) {
                        params.push(self.expect_param() ?);
                        while self.accept_token(Token::Punct(id!(,))) {
                            params.push(self.expect_param() ?);
                        }
                        self.expect_token(Token::CloseParen) ?;
                    }
                    let return_ty_expr = if self.accept_token(Token::Punct(id!( ->))) {
                        Some(self.expect_ty_expr() ?)
                    } else {
                        None
                    };
                    Ok(span.end(self, | span | TyExpr {
                        ty: RefCell::new(None),
                        span,
                        kind: TyExprKind::ClosureDecl {
                            params,
                            return_ty: RefCell::new(None),
                            return_ty_expr:Box::new(return_ty_expr)
                        },
                    }))
                }
                else
                if let Some(ty_lit) = TyLit::from_id(id) {
                    self.skip_token();
                    Ok(span.end(self, | span | TyExpr {
                        ty: RefCell::new(None),
                        span,
                        kind: TyExprKind::Lit {ty_lit: ty_lit},
                    }))
                }
                else {
                    if id == id!(Self) {
                        self.skip_token();
                        if let Some(FnSelfKind::Struct(struct_node_ptr)) = self.self_kind {
                            
                            return Ok(span.end(self, | span | TyExpr {
                                span,
                                ty: RefCell::new(None),
                                kind: TyExprKind::Struct(struct_node_ptr),
                            }))
                        }
                        return Err(span.error(self, live_error_origin!(), format!("Use of Self not allowed here").into()));
                    }
                    // ok lets tget the ident path
                    
                    let ident_path = self.expect_ident_path() ?;
                    
                    if let Some(ptr) = self.scan_scope_for_live_ptr(ident_path.segs[0]) {
                        match self.shader_registry.find_live_node_by_path(ptr, &ident_path.segs[1..ident_path.len()]) {
                            LiveNodeFindResult::NotFound => {
                                return Err(span.error(self, live_error_origin!(), format!("Struct not found `{}`", ident_path).into()))
                            }
                            LiveNodeFindResult::Function(_)
                                | LiveNodeFindResult::Const(_)
                                | LiveNodeFindResult::Component(_)
                                | LiveNodeFindResult::LiveValue(_, _)
                                | LiveNodeFindResult::PossibleStatic(_, _) => {
                                return Err(span.error(self, live_error_origin!(), format!("Not a Struct type `{}`", ident_path).into()))
                            }
                            LiveNodeFindResult::Struct(struct_ptr) => {
                                //yay .. lets make a struct typedep
                                self.type_deps.push(ShaderParserDep::Struct(struct_ptr));
                                
                                return Ok(span.end(self, | span | TyExpr {
                                    span,
                                    ty: RefCell::new(None),
                                    kind: TyExprKind::Struct(struct_ptr),
                                }))
                            }
                        }
                    }
                    else {
                        return Err(span.error(self, live_error_origin!(), format!("Cannot find type `{}`", ident_path).into()));
                    }
                }
            }
            token => Err(span.error(self, live_error_origin!(), format!("unexpected token `{}`", token).into())),
        }
    }
    
    fn expect_param(&mut self) -> Result<Param, LiveError> {
        let span = self.begin_span();
        let is_inout = self.accept_token(Token::Ident(id!(inout)));
        let ident = self.expect_ident() ?;
        self.expect_token(Token::Punct(id!(:))) ?;
        let ty_expr = self.expect_ty_expr() ?;
        Ok(span.end(self, | span | Param {
            shadow: Cell::new(None),
            span,
            is_inout,
            ident,
            ty_expr,
        }))
    }
    
    fn expect_block(&mut self) -> Result<Block, LiveError> {
        self.expect_token(Token::OpenBrace) ?;
        let mut stmts = Vec::new();
        while !self.accept_token(Token::CloseBrace) {
            stmts.push(self.expect_stmt() ?);
        }
        Ok(Block {stmts})
    }
    
    
    fn expect_stmt(&mut self) -> Result<Stmt, LiveError> {
        match self.peek_token() {
            Token::Ident(id!(break)) => self.expect_break_stmt(),
            Token::Ident(id!(continue)) => self.expect_continue_stmt(),
            Token::Ident(id!(for)) => self.expect_for_stmt(),
            Token::Ident(id!(if)) => self.expect_if_stmt(),
            Token::Ident(id!(let)) => self.expect_let_stmt(),
            Token::Ident(id!(return)) => self.expect_return_stmt(),
            _ => self.expect_expr_stmt(),
        }
    }
    
    fn expect_break_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(Token::Ident(id!(break))) ?;
        self.expect_token(Token::Punct(id!(;))) ?;
        Ok(span.end(self, | span | Stmt::Break {span}))
    }
    
    fn expect_continue_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(Token::Ident(id!(continue))) ?;
        self.expect_token(Token::Punct(id!(;))) ?;
        Ok(span.end(self, | span | Stmt::Continue {span}))
    }
    
    fn expect_for_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(Token::Ident(id!(for))) ?;
        let ident = self.expect_ident() ?;
        self.expect_token(Token::Ident(id!(from))) ?;
        let from_expr = self.expect_expr() ?;
        self.expect_token(Token::Ident(id!(to))) ?;
        let to_expr = self.expect_expr() ?;
        let step_expr = if self.accept_token(Token::Ident(id!(step))) {
            Some(self.expect_expr() ?)
        } else {
            None
        };
        let block = Box::new(self.expect_block() ?);
        Ok(span.end(self, | span | Stmt::For {
            span,
            ident,
            from_expr,
            to_expr,
            step_expr,
            block,
        }))
    }
    
    
    fn expect_if_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(Token::Ident(id!(if))) ?;
        let expr = self.expect_expr() ?;
        let block_if_true = Box::new(self.expect_block() ?);
        let block_if_false = if self.accept_token(Token::Ident(id!(else))) {
            if self.peek_token() == Token::Ident(id!(if)) {
                Some(Box::new(Block {
                    stmts: vec![self.expect_if_stmt() ?],
                }))
            } else {
                Some(Box::new(self.expect_block() ?))
            }
        } else {
            None
        };
        Ok(span.end(self, | span | Stmt::If {
            span,
            expr,
            block_if_true,
            block_if_false,
        }))
    }
    
    
    fn expect_let_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(Token::Ident(id!(let))) ?;
        let ident = self.expect_ident() ?;
        let ty_expr = if self.accept_token(Token::Punct(id!(:))) {
            Some(self.expect_ty_expr() ?)
        } else {
            None
        };
        let expr = if self.accept_token(Token::Punct(id!( =))) {
            Some(self.expect_expr() ?)
        } else {
            None
        };
        self.expect_token(Token::Punct(id!(;))) ?;
        Ok(span.end(self, | span | Stmt::Let {
            span,
            shadow: Cell::new(None),
            ty: RefCell::new(None),
            ident,
            ty_expr,
            expr,
        }))
    }
    
    fn expect_return_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(Token::Ident(id!(return))) ?;
        let expr = if !self.accept_token(Token::Punct(id!(;))) {
            let expr = self.expect_expr() ?;
            self.expect_token(Token::Punct(id!(;))) ?;
            Some(expr)
        } else {
            None
        };
        Ok(span.end(self, | span | Stmt::Return {span, expr}))
    }
    
    fn expect_expr_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        let expr = self.expect_expr() ?;
        self.expect_token(Token::Punct(id!(;))) ?;
        Ok(span.end(self, | span | Stmt::Expr {span, expr}))
    }
    
    fn expect_expr(&mut self) -> Result<Expr, LiveError> {
        self.expect_assign_expr()
    }
    
    fn expect_assign_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let expr = self.expect_cond_expr() ?;
        Ok(if let Some(op) = BinOp::from_assign_op(self.peek_token()) {
            self.skip_token();
            let left_expr = Box::new(expr);
            let right_expr = Box::new(self.expect_assign_expr() ?);
            span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Bin {
                    span,
                    op,
                    left_expr,
                    right_expr,
                },
            })
        } else {
            expr
        })
    }
    
    
    fn expect_cond_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let expr = self.expect_or_expr() ?;
        Ok(if self.accept_token(Token::Punct(id!( ?))) {
            let expr = Box::new(expr);
            let expr_if_true = Box::new(self.expect_expr() ?);
            self.expect_token(Token::Punct(id!(:))) ?;
            let expr_if_false = Box::new(self.expect_cond_expr() ?);
            span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Cond {
                    span,
                    expr,
                    expr_if_true,
                    expr_if_false,
                },
            })
        } else {
            expr
        })
    }
    
    
    fn expect_or_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_and_expr() ?;
        while let Some(op) = BinOp::from_or_op(self.peek_token()) {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_and_expr() ?);
            acc = span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Bin {
                    span,
                    op,
                    left_expr,
                    right_expr,
                },
            });
        }
        Ok(acc)
    }
    
    
    fn expect_and_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_eq_expr() ?;
        while let Some(op) = BinOp::from_and_op(self.peek_token()) {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_eq_expr() ?);
            acc = span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Bin {
                    span,
                    op,
                    left_expr,
                    right_expr,
                },
            });
        }
        Ok(acc)
    }
    
    
    fn expect_eq_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_rel_expr() ?;
        while let Some(op) = BinOp::from_eq_op(self.peek_token()) {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_rel_expr() ?);
            acc = span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Bin {
                    span,
                    op,
                    left_expr,
                    right_expr,
                },
            });
        }
        Ok(acc)
    }
    
    fn expect_rel_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_add_expr() ?;
        while let Some(op) = BinOp::from_rel_op(self.peek_token()) {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_add_expr() ?);
            acc = span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Bin {
                    span,
                    op,
                    left_expr,
                    right_expr,
                },
            });
        }
        Ok(acc)
    }
    
    fn expect_add_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_mul_expr() ?;
        while let Some(op) = BinOp::from_add_op(self.peek_token()) {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_mul_expr() ?);
            acc = span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Bin {
                    span,
                    op,
                    left_expr,
                    right_expr,
                },
            });
        }
        Ok(acc)
    }
    
    fn expect_mul_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_un_expr() ?;
        while let Some(op) = BinOp::from_mul_op(self.peek_token()) {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.expect_un_expr() ?);
            acc = span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Bin {
                    span,
                    op,
                    left_expr,
                    right_expr,
                },
            });
        }
        Ok(acc)
    }
    
    fn expect_un_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        Ok(if let Some(op) = UnOp::from_un_op(self.peek_token()) {
            self.skip_token();
            let expr = Box::new(self.expect_un_expr() ?);
            span.end(self, | span | Expr {
                span,
                ty: RefCell::new(None),
                const_val: RefCell::new(None),
                const_index: Cell::new(None),
                kind: ExprKind::Un {span, op, expr},
            })
        } else {
            self.expect_postfix_expr() ?
        })
    }
    
    fn expect_postfix_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        let mut acc = self.expect_prim_expr() ?;
        loop {
            match self.peek_token() {
                Token::Punct(id!(.)) => {
                    self.skip_token();
                    let ident = self.expect_ident() ?;
                    acc = if self.accept_token(Token::OpenParen) {
                        let mut arg_exprs = vec![acc];
                        if !self.accept_token(Token::CloseParen) {
                            loop {
                                arg_exprs.push(self.expect_expr() ?);
                                if !self.accept_token(Token::Punct(id!(,))) {
                                    break;
                                }
                            }
                            self.expect_token(Token::CloseParen) ?;
                        }
                        span.end(self, | span | Expr {
                            span,
                            ty: RefCell::new(None),
                            const_val: RefCell::new(None),
                            const_index: Cell::new(None),
                            kind: ExprKind::MethodCall {
                                span,
                                ident,
                                arg_exprs,
                                closure_site_index:Cell::new(None)
                            },
                        })
                    } else {
                        let expr = Box::new(acc);
                        span.end(self, | span | Expr {
                            span,
                            ty: RefCell::new(None),
                            const_val: RefCell::new(None),
                            const_index: Cell::new(None),
                            kind: ExprKind::Field {
                                span,
                                expr,
                                field_ident: ident,
                            },
                        })
                    }
                }
                Token::CloseBracket => {
                    self.skip_token();
                    let expr = Box::new(acc);
                    let index_expr = Box::new(self.expect_expr() ?);
                    self.expect_token(Token::CloseBracket) ?;
                    acc = span.end(self, | span | Expr {
                        span,
                        ty: RefCell::new(None),
                        const_val: RefCell::new(None),
                        const_index: Cell::new(None),
                        kind: ExprKind::Index {
                            span,
                            expr,
                            index_expr,
                        },
                    });
                }
                _ => break,
            }
        }
        Ok(acc)
    }
    
    
    fn expect_prim_expr(&mut self) -> Result<Expr, LiveError> {
        let span = self.begin_span();
        match self.peek_token() {
            Token::Ident(ident) => {
                if let Some(ty_lit) = TyLit::from_id(ident) {
                    self.skip_token();
                    self.expect_token(Token::OpenParen) ?;
                    let mut arg_exprs = Vec::new();
                    if !self.accept_token(Token::CloseParen) {
                        loop {
                            arg_exprs.push(self.expect_expr() ?);
                            if !self.accept_token(Token::Punct(id!(,))) {
                                break;
                            }
                        }
                        self.expect_token(Token::CloseParen) ?;
                    }
                    return Ok(span.end(self, | span | Expr {
                        span,
                        ty: RefCell::new(None),
                        const_val: RefCell::new(None),
                        const_index: Cell::new(None),
                        kind: ExprKind::ConsCall {
                            span,
                            ty_lit,
                            arg_exprs,
                        },
                    }))
                }
                else {
                    let ident_path = self.expect_ident_path() ?;
                    match self.peek_token() {
                        Token::OpenBrace => { // its a struct constructor call
                            
                            let struct_node_ptr = if ident_path.len() == 1 && ident_path.segs[0] == id!(Self) {
                                if let Some(FnSelfKind::Struct(struct_node_ptr)) = self.self_kind {
                                    struct_node_ptr
                                }
                                else {
                                    return Err(span.error(self, live_error_origin!(), format!("Use of Self not allowed here").into()));
                                }
                            }
                            else if let Some(ptr) = self.scan_scope_for_live_ptr(ident_path.segs[0]) {
                                match self.shader_registry.find_live_node_by_path(ptr, &ident_path.segs[1..ident_path.len()]) {
                                    LiveNodeFindResult::Struct(struct_ptr) => {
                                        self.type_deps.push(ShaderParserDep::Struct(struct_ptr));
                                        struct_ptr
                                    }
                                    LiveNodeFindResult::NotFound => {
                                        return Err(span.error(self, live_error_origin!(), format!("Struct not found `{}`", ident_path).into()))
                                    }
                                    LiveNodeFindResult::PossibleStatic(_, _)
                                        | LiveNodeFindResult::Function(_)
                                        | LiveNodeFindResult::Component(_)
                                        | LiveNodeFindResult::Const(_)
                                        | LiveNodeFindResult::LiveValue(_, _) => {
                                        return Err(span.error(self, live_error_origin!(), format!("Not a struct `{}`", ident_path).into()))
                                    }
                                }
                            }
                            else {
                                return Err(span.error(self, live_error_origin!(), format!("Struct not found `{}`", ident_path).into()))
                            };
                            
                            self.skip_token();
                            let mut args = Vec::new();
                            loop {
                                let name = self.expect_ident() ?;
                                self.expect_token(Token::Punct(id!(:))) ?;
                                let expr = self.expect_expr() ?;
                                self.accept_token(Token::Punct(id!(,)));
                                args.push((name, expr));
                                // see if we have a } or a ,
                                match self.peek_token() {
                                    Token::Eof => {
                                        return Err(span.error(self, live_error_origin!(), format!("Unexpected Eof").into()))
                                    }
                                    Token::CloseBrace => {
                                        self.skip_token();
                                        return Ok(span.end(self, | span | Expr {
                                            span,
                                            ty: RefCell::new(None),
                                            const_val: RefCell::new(None),
                                            const_index: Cell::new(None),
                                            kind: ExprKind::StructCons {
                                                struct_node_ptr,
                                                span,
                                                args
                                            },
                                        }))
                                    }
                                    _ => ()
                                }
                                
                            }
                        }
                        Token::OpenParen => {
                            let arg_exprs = self.expect_arg_exprs() ?;
                            if ident_path.len() == 1 && self.shader_registry.builtins.get(&Ident(ident_path.segs[0])).is_some() {
                                Ok(span.end(self, | span | Expr {
                                    span,
                                    ty: RefCell::new(None),
                                    const_val: RefCell::new(None),
                                    const_index: Cell::new(None),
                                    kind: ExprKind::BuiltinCall {
                                        span,
                                        ident: Ident(ident_path.segs[0]),
                                        arg_exprs,
                                    },
                                }))
                            }
                            else if let Some(ptr) = self.scan_scope_for_live_ptr(ident_path.segs[0]) {
                                match self.shader_registry.find_live_node_by_path(ptr, &ident_path.segs[1..ident_path.len()]) {
                                    LiveNodeFindResult::NotFound => {
                                        Err(span.error(self, live_error_origin!(), format!("Function not found `{}`", ident_path).into()))
                                    }
                                    LiveNodeFindResult::Component(_)
                                        | LiveNodeFindResult::Struct(_)
                                        | LiveNodeFindResult::Const(_)
                                        | LiveNodeFindResult::LiveValue(_, _) => {
                                        Err(span.error(self, live_error_origin!(), format!("Not a function `{}`", ident_path).into()))
                                    }
                                    LiveNodeFindResult::Function(fn_node_ptr) => {
                                        self.type_deps.push(ShaderParserDep::Function(None, fn_node_ptr));
                                        Ok(span.end(self, | span | Expr {
                                            span,
                                            ty: RefCell::new(None),
                                            const_val: RefCell::new(None),
                                            const_index: Cell::new(None),
                                            kind: ExprKind::PlainCall {
                                                span,
                                                fn_node_ptr: Some(fn_node_ptr),
                                                ident: if ident_path.len()==1{Some(Ident(ident_path.segs[0]))}else{None},
                                                arg_exprs,
                                                param_index:Cell::new(None),
                                                closure_site_index:Cell::new(None),
                                            },
                                        }))
                                        //Err(span.error(self, live_error_origin!(), format!("Cannot call a struct `{}`", ident_path).into()))
                                    }
                                    LiveNodeFindResult::PossibleStatic(struct_node_ptr, fn_node_ptr) => {
                                        // we need to register struct_node_ptr as a dep to compile
                                        self.type_deps.push(ShaderParserDep::Struct(struct_node_ptr));
                                        self.type_deps.push(ShaderParserDep::Function(Some(struct_node_ptr), fn_node_ptr));
                                        Ok(span.end(self, | span | Expr {
                                            span,
                                            ty: RefCell::new(None),
                                            const_val: RefCell::new(None),
                                            const_index: Cell::new(None),
                                            kind: ExprKind::PlainCall {
                                                span,
                                                ident: if ident_path.len()==1{Some(Ident(ident_path.segs[0]))}else{None},
                                                fn_node_ptr:Some(fn_node_ptr),
                                                arg_exprs,
                                                param_index:Cell::new(None),
                                                closure_site_index:Cell::new(None),
                                            },
                                        }))
                                        //Err(span.error(self, live_error_origin!(), format!("Cannot call a struct `{}`", ident_path).into()))
                                    }
                                    
                                }
                            }
                            else if ident_path.len() == 1{
                                // it must be a closure call, even though we don't know if its really there.
                                Ok(span.end(self, | span | Expr {
                                    span,
                                    ty: RefCell::new(None),
                                    const_val: RefCell::new(None),
                                    const_index: Cell::new(None),
                                    kind:  ExprKind::PlainCall {
                                        span,
                                        ident: Some(Ident(ident_path.segs[0])),
                                        fn_node_ptr:None,
                                        arg_exprs,
                                        param_index:Cell::new(None),
                                        closure_site_index:Cell::new(None),
                                    },
                                }))
                            }
                            else{
                                Err(span.error(self, live_error_origin!(), format!("Call not found `{}`", ident_path).into()))
                            }
                        }
                        _ => {
                            // ok we wanna resolve, however if its multi-segment and not resolved it fails.
                            
                            let mut var_resolve = VarResolve::NotFound;
                            if let Some(ptr) = self.scan_scope_for_live_ptr(ident_path.segs[0]) {
                                match self.shader_registry.find_live_node_by_path(ptr, &ident_path.segs[1..ident_path.len()]) {
                                    LiveNodeFindResult::LiveValue(value_ptr, ty) => {
                                        var_resolve = VarResolve::LiveValue(value_ptr, ty);
                                    }
                                    LiveNodeFindResult::Const(const_ptr) => {
                                        self.type_deps.push(ShaderParserDep::Const(const_ptr));
                                        var_resolve = VarResolve::Const(const_ptr);
                                    }
                                    LiveNodeFindResult::Function(fn_ptr) => {
                                        self.type_deps.push(ShaderParserDep::Function(None, fn_ptr));
                                        var_resolve = VarResolve::Function(fn_ptr);
                                    }
                                    _ => ()
                                }
                            }
                            if let VarResolve::NotFound = var_resolve{
                                if ident_path.len()>1{
                                    return  Err(span.error(self, live_error_origin!(), format!("Identifier not found `{}`", ident_path).into()))
                                }
                            }
                            
                            Ok(span.end(self, | span | Expr {
                                span,
                                ty: RefCell::new(None),
                                const_val: RefCell::new(None),
                                const_index: Cell::new(None),
                                kind: ExprKind::Var {
                                    ident: if ident_path.len()>1{None} else {Some(Ident(ident_path.segs[0]))},
                                    span,
                                    var_resolve,
                                    kind: Cell::new(None),
                                },
                            }))
                        },
                    }
                }
            }
            Token::Bool(v) => {
                self.skip_token();
                Ok(span.end(self, | span | Expr {
                    span,
                    ty: RefCell::new(None),
                    const_val: RefCell::new(None),
                    const_index: Cell::new(None),
                    kind: ExprKind::Lit {span, lit: Lit::Bool(v)},
                }))
            }
            Token::Int(v) => {
                self.skip_token();
                Ok(span.end(self, | span | Expr {
                    span,
                    ty: RefCell::new(None),
                    const_val: RefCell::new(None),
                    const_index: Cell::new(None),
                    kind: ExprKind::Lit {span, lit: Lit::Int(v as i32)},
                }))
            }
            Token::Float(v) => {
                self.skip_token();
                Ok(span.end(self, | span | Expr {
                    span,
                    ty: RefCell::new(None),
                    const_val: RefCell::new(None),
                    const_index: Cell::new(None),
                    kind: ExprKind::Lit {span, lit: Lit::Float(v as f32)},
                }))
            }
            Token::Color(v) => {
                self.skip_token();
                Ok(span.end(self, | span | Expr {
                    span,
                    ty: RefCell::new(None),
                    const_val: RefCell::new(None),
                    const_index: Cell::new(None),
                    kind: ExprKind::Lit {span, lit: Lit::Color(v)},
                }))
            }
            Token::Punct(id!(|))=>{
                // closure def
                self.skip_token();
                let mut params = Vec::new();
                if !self.accept_token(Token::Punct(id!(|))) {
                    loop {
                        let span = self.begin_span();
                        params.push(ClosureParam{
                            ident: self.expect_ident()?,
                            span: span.end(self, |span| span),
                            shadow: Cell::new(None)
                        });
                        if !self.accept_token(Token::Punct(id!(,))) {
                            break;
                        }
                    }
                    self.expect_token(Token::Punct(id!(|))) ?;
                }
                if self.peek_token() == Token::OpenBrace{
                    let block = self.expect_block()?;
                    let span = span.end(self, |span| span);
                    let closure_def_index = ClosureDefIndex(self.closure_defs.len());
                    self.closure_defs.push(ClosureDef {
                        span,
                        params,
                        closed_over_syms: RefCell::new(None),
                        kind: ClosureDefKind::Block(block)
                    });
                    Ok(Expr {
                        span,
                        ty: RefCell::new(None),
                        const_val: RefCell::new(None),
                        const_index: Cell::new(None),
                        kind: ExprKind::ClosureDef(closure_def_index)
                    })
                }
                else{
                    let expr = self.expect_expr()?;
                    let span = span.end(self, |span| span);
                    let closure_def_index = ClosureDefIndex(self.closure_defs.len());
                    self.closure_defs.push(ClosureDef {
                        span,
                        params,
                        closed_over_syms: RefCell::new(None),
                        kind: ClosureDefKind::Expr(expr)
                    });
                    Ok(Expr {
                        span,
                        ty: RefCell::new(None),
                        const_val: RefCell::new(None),
                        const_index: Cell::new(None),
                        kind: ExprKind::ClosureDef(closure_def_index)
                    })
                }
            }
            Token::OpenParen => {
                self.skip_token();
                let expr = self.expect_expr() ?;
                self.expect_token(Token::CloseParen) ?;
                Ok(expr)
            }
            token => Err(span.error(self, live_error_origin!(), format!("unexpected token `{}`", token).into())),
        }
    }
    
    fn expect_arg_exprs(&mut self) -> Result<Vec<Expr>, LiveError> {
        self.expect_token(Token::OpenParen) ?;
        let mut arg_exprs = Vec::new();
        if !self.accept_token(Token::CloseParen) {
            loop {
                arg_exprs.push(self.expect_expr() ?);
                if !self.accept_token(Token::Punct(id!(,))) {
                    break;
                }
            }
            self.expect_token(Token::CloseParen) ?;
        }
        Ok(arg_exprs)
    }
}

pub struct SpanTracker {
    pub file_id: FileId,
    pub start: usize,
}

impl SpanTracker {
    pub fn end<F, R>(&self, parser: &mut ShaderParser, f: F) -> R
    where
    F: FnOnce(Span) -> R,
    {
        f(Span::new(
            self.file_id,
            self.start,
            parser.token_end(),
        ))
    }
    
    pub fn error(&self, parser: &mut ShaderParser, origin: LiveErrorOrigin, message: String) -> LiveError {
        LiveError {
            origin,
            span: Span::new(
                self.file_id,
                self.start,
                parser.token_end(),
            ),
            message,
        }
    }
}