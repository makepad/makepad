use std::iter::Cloned;
use std::slice::Iter;
use std::cell::Cell;
use std::cell::RefCell;
use makepad_live_parser::FileId;
use makepad_live_parser::TokenWithSpan;
use makepad_live_parser::Token;
use makepad_live_parser::Span;
use makepad_live_parser::LiveError;
use makepad_live_parser::LiveScopeItem;
use makepad_live_parser::Id;
use makepad_live_parser::token_punct;
use makepad_live_parser::token_ident;
use makepad_live_parser::id;
use makepad_live_parser::FullNodePtr;
use makepad_live_parser::LiveScopeTarget;
use crate::ident::Ident;
use crate::ty::TyLit;
use crate::ident::IdentPath;
use crate::shaderast::TyExprKind;
use crate::shaderast::TyExpr;
use crate::shaderast::FnDecl;
use crate::shaderast::Block;
use crate::shaderast::Expr;
use crate::shaderast::ExprKind;
use crate::shaderast::Param;
use crate::shaderast::Stmt;
use crate::shaderast::BinOp;
use crate::shaderast::UnOp;
//use crate::shaderast::Decl;
use crate::lit::Lit;

pub struct ShaderParser<'a> {
    pub token_index: usize,
    pub file_id: FileId,
    pub tokens_with_span: Cloned<Iter<'a, TokenWithSpan >>,
    pub live_scope: &'a [LiveScopeItem],
    pub type_deps: &'a mut Vec<FullNodePtr>,
    pub token_with_span: TokenWithSpan,
    pub end: usize,
}

impl<'a> ShaderParser<'a> {
    pub fn new(tokens: &'a [TokenWithSpan], live_scope: &'a [LiveScopeItem], type_deps: &'a mut Vec<FullNodePtr>) -> Self {
        let mut tokens_with_span = tokens.iter().cloned();
        let token_with_span = tokens_with_span.next().unwrap();
        ShaderParser {
            file_id: FileId::default(),
            live_scope,
            type_deps,
            tokens_with_span,
            token_with_span,
            token_index: 0,
            end: 0,
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
    
    fn error(&mut self, message: String) -> LiveError {
        LiveError {
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
                return Err(span.error(self, format!("expected ident_path, unexpected token `{}`", token).into()));
            }
        };
        
        loop {
            if !self.accept_token(token_punct!(::)) {
                return Ok(ident_path);
            }
            match self.peek_token() {
                Token::Ident(ident) => {
                    self.skip_token();
                    if !ident_path.push(Ident(ident)) {
                        return Err(span.error(self, format!("identifier too long `{}`", ident_path).into()));
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
            token => Err(self.error(format!("expected ident, unexpected token `{}`", token))),
        }
    }
    
    fn expect_token(&mut self, expected: Token) -> Result<(), LiveError> {
        let actual = self.peek_token();
        if actual != expected {
            return Err(self.error(format!("expected {} unexpected token `{}`", expected, actual)));
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
    pub fn expect_fn_decl(&mut self, prefix: Option<(Ident, FullNodePtr)>) -> Result<FnDecl, LiveError> {
        let span = self.begin_span();
        self.expect_token(token_ident!(fn)) ?;
        
        let ident_path = if let Some((prefix, _)) = prefix {
            IdentPath::from_two(prefix, self.expect_ident() ?)
        } else {
            IdentPath::from_ident(self.expect_ident() ?)
        };
        
        self.expect_token(Token::OpenParen) ?;
        let mut params = Vec::new();
        if !self.accept_token(Token::CloseParen) {
            
            if let Some((prefix, prefix_ptr)) = prefix {
                let span = self.begin_span();
                let is_inout = self.accept_token(token_ident!(inout));
                if self.accept_token(token_ident!(self)) {
                    params.push(span.end(self, | span | Param {
                        span,
                        is_inout,
                        ident: Ident(id!(self)),
                        ty_expr: TyExpr {
                            ty: RefCell::new(None),
                            kind: TyExprKind::Var {
                                span: Span::default(),
                                ident: prefix,
                                full_ptr: prefix_ptr,
                            },
                        },
                    }))
                } else {
                    let ident = self.expect_ident() ?;
                    self.expect_token(token_punct!(:)) ?;
                    let ty_expr = self.expect_ty_expr() ?;
                    params.push(span.end(self, | span | Param {
                        span,
                        is_inout,
                        ident,
                        ty_expr,
                    }));
                }
            } else {
                params.push(self.expect_param() ?);
            }
            while self.accept_token(token_punct!(,)) {
                params.push(self.expect_param() ?);
            }
            self.expect_token(Token::CloseParen) ?;
        }
        let return_ty_expr = if self.accept_token(token_punct!( ->)) {
            Some(self.expect_ty_expr() ?)
        } else {
            None
        };
        let block = self.expect_block() ?;
        
        Ok(span.end(self, | span | FnDecl {
            span,
            return_ty: RefCell::new(None),
            is_used_in_vertex_shader: Cell::new(None),
            is_used_in_fragment_shader: Cell::new(None),
            callees: RefCell::new(None),
            uniform_block_deps: RefCell::new(None),
            has_texture_deps: Cell::new(None),
            geometry_deps: RefCell::new(None),
            instance_deps: RefCell::new(None),
            has_varying_deps: Cell::new(None),
            builtin_deps: RefCell::new(None),
            cons_fn_deps: RefCell::new(None),
            ident_path,
            params,
            return_ty_expr,
            block,
        }))
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
                    kind: TyExprKind::Array {
                        span,
                        elem_ty_expr,
                        len: len as u32,
                    },
                });
            }
            else {
                return Err(span.error(self, format!("unexpected token `{}`", token).into()))
            }
        }
        Ok(acc)
    }
    
    fn expect_prim_ty_expr(&mut self) -> Result<TyExpr, LiveError> {
        let span = self.begin_span();
        match self.peek_token() {
            Token::Ident(id) => {
                self.skip_token();
                if let Some(ty_lit) = TyLit::from_id(id) {
                    Ok(span.end(self, | span | TyExpr {
                        ty: RefCell::new(None),
                        kind: TyExprKind::Lit {span, ty_lit: ty_lit},
                    }))
                }
                else {
                    // this is where we use a custom type
                    // lets resolve this right now against our scope
                    // and store a pointer to it.
                    for item in self.live_scope.iter().rev() {
                        if item.id == id {
                            let full_ptr = match item.target {
                                LiveScopeTarget::Full(full) => full,
                                LiveScopeTarget::Local(local) => FullNodePtr {
                                    file_id: self.file_id,
                                    local_ptr: local
                                }
                            };
                            self.type_deps.push(full_ptr);
                            return Ok(span.end(self, | span | TyExpr {
                                ty: RefCell::new(None),
                                kind: TyExprKind::Var {
                                    full_ptr,
                                    span,
                                    ident: Ident(id)
                                },
                            }))
                        }
                    }
                    return Err(span.error(self, format!("Cannot find type `{}` on scope", id).into()));
                }
            }
            token => Err(span.error(self, format!("unexpected token `{}`", token).into())),
        }
    }
    
    fn expect_param(&mut self) -> Result<Param, LiveError> {
        let span = self.begin_span();
        let is_inout = self.accept_token(token_ident!(inout));
        let ident = self.expect_ident() ?;
        self.expect_token(token_punct!(:)) ?;
        let ty_expr = self.expect_ty_expr() ?;
        Ok(span.end(self, | span | Param {
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
            token_ident!(break) => self.expect_break_stmt(),
            token_ident!(continue) => self.expect_continue_stmt(),
            token_ident!(for) => self.expect_for_stmt(),
            token_ident!(if) => self.expect_if_stmt(),
            token_ident!(let) => self.expect_let_stmt(),
            token_ident!(return) => self.expect_return_stmt(),
            _ => self.expect_expr_stmt(),
        }
    }
    
    fn expect_break_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(token_ident!(break)) ?;
        self.expect_token(token_punct!(;)) ?;
        Ok(span.end(self, | span | Stmt::Break {span}))
    }
    
    fn expect_continue_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(token_ident!(continue)) ?;
        self.expect_token(token_punct!(;)) ?;
        Ok(span.end(self, | span | Stmt::Continue {span}))
    }
    
    fn expect_for_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(token_ident!(for)) ?;
        let ident = self.expect_ident() ?;
        self.expect_token(token_ident!(from)) ?;
        let from_expr = self.expect_expr() ?;
        self.expect_token(token_ident!(to)) ?;
        let to_expr = self.expect_expr() ?;
        let step_expr = if self.accept_token(token_ident!(step)) {
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
        self.expect_token(token_ident!(if)) ?;
        let expr = self.expect_expr() ?;
        let block_if_true = Box::new(self.expect_block() ?);
        let block_if_false = if self.accept_token(token_ident!(else)) {
            if self.peek_token() == token_ident!(if) {
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
        self.expect_token(token_ident!(let)) ?;
        let ident = self.expect_ident() ?;
        let ty_expr = if self.accept_token(token_punct!(:)) {
            Some(self.expect_ty_expr() ?)
        } else {
            None
        };
        let expr = if self.accept_token(token_punct!( =)) {
            Some(self.expect_expr() ?)
        } else {
            None
        };
        self.expect_token(token_punct!(;)) ?;
        Ok(span.end(self, | span | Stmt::Let {
            span,
            ty: RefCell::new(None),
            ident,
            ty_expr,
            expr,
        }))
    }
    
    fn expect_return_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        self.expect_token(token_ident!(return)) ?;
        let expr = if !self.accept_token(token_punct!(;)) {
            let expr = self.expect_expr() ?;
            self.expect_token(token_punct!(;)) ?;
            Some(expr)
        } else {
            None
        };
        Ok(span.end(self, | span | Stmt::Return {span, expr}))
    }
    
    fn expect_expr_stmt(&mut self) -> Result<Stmt, LiveError> {
        let span = self.begin_span();
        let expr = self.expect_expr() ?;
        self.expect_token(token_punct!(;)) ?;
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
        Ok(if self.accept_token(token_punct!( ?)) {
            let expr = Box::new(expr);
            let expr_if_true = Box::new(self.expect_expr() ?);
            self.expect_token(token_punct!(:)) ?;
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
                token_punct!(.) => {
                    self.skip_token();
                    let ident = self.expect_ident() ?;
                    acc = if self.accept_token(Token::OpenParen) {
                        let mut arg_exprs = vec![acc];
                        if !self.accept_token(Token::CloseParen) {
                            loop {
                                arg_exprs.push(self.expect_expr() ?);
                                if !self.accept_token(token_punct!(,)) {
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
                            if !self.accept_token(token_punct!(,)) {
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
                        token_punct!(!) => {
                            let ident = if let Some(ident) = ident_path.get_single() {
                                ident
                            }
                            else {
                                return Err(span.error(self, format!("not a valid macro `{}`", ident_path).into()));
                            };
                            self.skip_token();
                            let arg_exprs = self.expect_arg_exprs() ?;
                            return Ok(span.end(self, | span | Expr {
                                span,
                                ty: RefCell::new(None),
                                const_val: RefCell::new(None),
                                const_index: Cell::new(None),
                                kind: ExprKind::MacroCall {
                                    span,
                                    analysis: Cell::new(None),
                                    ident,
                                    arg_exprs,
                                },
                            }))
                        }
                        Token::OpenParen => {
                            let arg_exprs = self.expect_arg_exprs() ?;
                            return Ok(span.end(self, | span | Expr {
                                span,
                                ty: RefCell::new(None),
                                const_val: RefCell::new(None),
                                const_index: Cell::new(None),
                                kind: ExprKind::Call {
                                    span,
                                    ident_path,
                                    arg_exprs,
                                },
                            }))
                        }
                        _ => {
                            Ok(span.end(self, | span | Expr {
                                span,
                                ty: RefCell::new(None),
                                const_val: RefCell::new(None),
                                const_index: Cell::new(None),
                                kind: ExprKind::Var {
                                    span,
                                    kind: Cell::new(None),
                                    ident_path,
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
            Token::OpenParen => {
                self.skip_token();
                let expr = self.expect_expr() ?;
                self.expect_token(Token::CloseParen) ?;
                Ok(expr)
            }
            token => Err(span.error(self, format!("unexpected token `{}`", token).into())),
        }
    }
    
    fn expect_arg_exprs(&mut self) -> Result<Vec<Expr>, LiveError> {
        self.expect_token(Token::OpenParen) ?;
        let mut arg_exprs = Vec::new();
        if !self.accept_token(Token::CloseParen) {
            loop {
                arg_exprs.push(self.expect_expr() ?);
                if !self.accept_token(token_punct!(,)) {
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
    
    pub fn error(&self, parser: &mut ShaderParser, message: String) -> LiveError {
        LiveError {
            span: Span::new(
                self.file_id,
                self.start,
                parser.token_end(),
            ),
            message,
        }
    }
}