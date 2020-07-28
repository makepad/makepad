use crate::ast::*;
use crate::error::Error;
use crate::ident::Ident;
use crate::lit::Lit;
use crate::span::Span;
use crate::token::{Token, TokenWithSpan};
use std::cell::{Cell, RefCell};
use std::fmt::Write;
use std::iter::Cloned;
use std::slice::Iter;

pub fn parse(tokens_with_span: &[TokenWithSpan], shader: &mut ShaderAst) -> Result<(), Error> {
    let mut tokens_with_span = tokens_with_span.iter().cloned();
    let token_with_span = tokens_with_span.next().unwrap();
    Parser {tokens_with_span, token_with_span, end: 0, shader}.parse()
}

struct Parser<'a> {
    tokens_with_span: Cloned<Iter<'a, TokenWithSpan>>,
    token_with_span: TokenWithSpan,
    end: usize,
    shader: &'a mut ShaderAst,
}

impl<'a> Parser<'a> {
    fn parse(&mut self) -> Result<(), Error> {
        while self.peek_token() != Token::Eof {
            let span = self.begin_span();
            match self.peek_token() {
                Token::Geometry => {
                    let decl = self.parse_geometry_decl() ?;
                    self.shader.decls.push(Decl::Geometry(decl));
                },
                Token::Const => {
                    let decl = self.parse_const_decl() ?;
                    self.shader.decls.push(Decl::Const(decl));
                }
                Token::Fn => {
                    let decl = self.parse_fn_decl(None) ?;
                    self.shader.decls.push(Decl::Fn(decl));
                }
                Token::Impl => {
                    self.expect_token(Token::Impl) ?;
                    let prefix = self.parse_ident() ?;
                    self.expect_token(Token::LeftBrace) ?;
                    while !self.accept_token(Token::RightBrace) {
                        let decl = self.parse_fn_decl(Some(prefix)) ?;
                        self.shader.decls.push(Decl::Fn(decl));
                    }
                }
                Token::Instance => {
                    let decl = self.parse_instance_decl() ?;
                    self.shader.decls.push(Decl::Instance(decl));
                }
                Token::Struct => {
                    let decl = self.parse_struct_decl() ?;
                    self.shader.decls.push(Decl::Struct(decl));
                }
                Token::Texture => {
                    let decl = self.parse_texture_decl() ?;
                    self.shader.decls.push(Decl::Texture(decl));
                },
                Token::Uniform => {
                    let decl = self.parse_uniform_decl() ?;
                    self.shader.decls.push(Decl::Uniform(decl));
                },
                Token::Varying => {
                    let decl = self.parse_varying_decl() ?;
                    self.shader.decls.push(Decl::Varying(decl));
                },
                token => {
                    return Err(span.error(self, format!("unexpected token `{}`", token).into()))
                },
            }
        }
        Ok(())
    }
    
    fn parse_geometry_decl(&mut self) -> Result<GeometryDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Geometry) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_path() ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | GeometryDecl {
            is_used_in_fragment_shader: Cell::new(None),
            span,
            ident,
            ty_expr
        }))
    }
    
    fn parse_const_decl(&mut self) -> Result<ConstDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Const) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_expr() ?;
        self.expect_token(Token::Eq) ?;
        let expr = self.parse_expr() ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | ConstDecl {
            span,
            ident,
            ty_expr,
            expr,
        }))
    }
    
    fn parse_fn_decl(&mut self, prefix: Option<Ident>) -> Result<FnDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Fn) ?;
        let ident = if let Some(prefix) = prefix {
            Ident::new(format!("{}::{}", prefix, self.parse_ident() ?))
        } else {
            self.parse_ident() ?
        };
        self.expect_token(Token::LeftParen) ?;
        let mut params = Vec::new();
        if !self.accept_token(Token::RightParen) {
            if let Some(prefix) = prefix {
                let span = self.begin_span();
                let is_inout = self.accept_token(Token::Inout);
                if self.accept_token(Token::Self_) {
                    params.push(span.end(self, |span| Param {
                        span,
                        is_inout,
                        ident: Ident::new("self"),
                        ty_expr: TyExpr {
                            ty: RefCell::new(None),
                            kind: TyExprKind::Var {
                                span: Span::default(),
                                ident: prefix,
                            },
                        }
                    }))
                } else {
                    let ident = self.parse_ident()?;
                    self.expect_token(Token::Colon)?;
                    let ty_expr = self.parse_ty_expr()?;
                    params.push(span.end(self, |span| Param {
                        span,
                        is_inout,
                        ident,
                        ty_expr,        
                    }));
                }
            } else {
                params.push(self.parse_param() ?);
            }
            while self.accept_token(Token::Comma) {
                params.push(self.parse_param() ?);
            }
            self.expect_token(Token::RightParen) ?;
        }
        let return_ty_expr = if self.accept_token(Token::Arrow) {
            Some(self.parse_ty_expr() ?)
        } else {
            None
        };
        let block = self.parse_block() ?;
        Ok(span.end(&self, | span | FnDecl {
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
            ident,
            params,
            return_ty_expr,
            block,
        }))
    }
    
    fn parse_instance_decl(&mut self) -> Result<InstanceDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Instance) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_path() ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | InstanceDecl {
            is_used_in_fragment_shader: Cell::new(None),
            span,
            ident,
            ty_expr
        }))
    }
    
    fn parse_struct_decl(&mut self) -> Result<StructDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Struct) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::LeftBrace) ?;
        let mut fields = Vec::new();
        if !self.accept_token(Token::RightBrace) {
            loop {
                fields.push(self.parse_field() ?);
                if !self.accept_token(Token::Comma) {
                    break;
                }
            }
            self.expect_token(Token::RightBrace) ?;
        }
        Ok(span.end(&self, | span | StructDecl {
            span,
            ident,
            fields
        }))
    }
    
    fn parse_texture_decl(&mut self) -> Result<TextureDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Texture) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_path() ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | TextureDecl {
            span,
            ident,
            ty_expr
        }))
    }
    
    fn parse_uniform_decl(&mut self) -> Result<UniformDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Uniform) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_path() ?;
        let block_ident = if self.accept_token(Token::In) {
            Some(self.parse_ident() ?)
        } else {
            None
        };
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | UniformDecl {
            span,
            ident,
            ty_expr,
            block_ident,
        }))
    }
    
    fn parse_varying_decl(&mut self) -> Result<VaryingDecl, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Varying) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_expr() ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | VaryingDecl {
            span,
            ident,
            ty_expr
        }))
    }
    
    fn parse_param(&mut self) -> Result<Param, Error> {
        let span = self.begin_span();
        let is_inout = self.accept_token(Token::Inout);
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_expr() ?;
        Ok(span.end(self, |span| Param {
            span,
            is_inout,
            ident,
            ty_expr
        }))
    }
    
    fn parse_field(&mut self) -> Result<Field, Error> {
        let ident = self.parse_ident() ?;
        self.expect_token(Token::Colon) ?;
        let ty_expr = self.parse_ty_expr() ?;
        Ok(Field {ident, ty_expr})
    }
    
    fn parse_block(&mut self) -> Result<Block, Error> {
        self.expect_token(Token::LeftBrace) ?;
        let mut stmts = Vec::new();
        while !self.accept_token(Token::RightBrace) {
            stmts.push(self.parse_stmt() ?);
        }
        Ok(Block {stmts})
    }
    
    fn parse_stmt(&mut self) -> Result<Stmt, Error> {
        match self.peek_token() {
            Token::Break => self.parse_break_stmt(),
            Token::Continue => self.parse_continue_stmt(),
            Token::For => self.parse_for_stmt(),
            Token::If => self.parse_if_stmt(),
            Token::Let => self.parse_let_stmt(),
            Token::Return => self.parse_return_stmt(),
            _ => self.parse_expr_stmt(),
        }
    }
    
    fn parse_break_stmt(&mut self) -> Result<Stmt, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Break) ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | Stmt::Break {span}))
    }
    
    fn parse_continue_stmt(&mut self) -> Result<Stmt, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Continue) ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | Stmt::Continue {span}))
    }
    
    fn parse_for_stmt(&mut self) -> Result<Stmt, Error> {
        let span = self.begin_span();
        self.expect_token(Token::For) ?;
        let ident = self.parse_ident() ?;
        self.expect_token(Token::From) ?;
        let from_expr = self.parse_expr() ?;
        self.expect_token(Token::To) ?;
        let to_expr = self.parse_expr() ?;
        let step_expr = if self.accept_token(Token::Ident(Ident::new("step"))) {
            Some(self.parse_expr() ?)
        } else {
            None
        };
        let block = Box::new(self.parse_block() ?);
        Ok(span.end(&self, | span | Stmt::For {
            span,
            ident,
            from_expr,
            to_expr,
            step_expr,
            block,
        }))
    }
    
    fn parse_if_stmt(&mut self) -> Result<Stmt, Error> {
        let span = self.begin_span();
        self.expect_token(Token::If) ?;
        let expr = self.parse_expr() ?;
        let block_if_true = Box::new(self.parse_block() ?);
        let block_if_false = if self.accept_token(Token::Else) {
            if self.peek_token() == Token::If {
                Some(Box::new(Block {stmts: vec![self.parse_if_stmt() ?]}))
            }
            else {
                Some(Box::new(self.parse_block() ?))
            }
        } else {
            None
        };
        Ok(span.end(&self, | span | Stmt::If {
            span,
            expr,
            block_if_true,
            block_if_false,
        }))
    }
    
    fn parse_let_stmt(&mut self) -> Result<Stmt, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Let) ?;
        let ident = self.parse_ident() ?;
        let ty_expr = if self.accept_token(Token::Colon) {
            Some(self.parse_ty_expr() ?)
        } else {
            None
        };
        let expr = if self.accept_token(Token::Eq) {
            Some(self.parse_expr() ?)
        } else {
            None
        };
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | Stmt::Let {
            span,
            ty: RefCell::new(None),
            ident,
            ty_expr,
            expr,
        }))
    }
    
    fn parse_return_stmt(&mut self) -> Result<Stmt, Error> {
        let span = self.begin_span();
        self.expect_token(Token::Return) ?;
        let expr = if !self.accept_token(Token::Semi) {
            let expr = self.parse_expr() ?;
            self.expect_token(Token::Semi) ?;
            Some(expr)
        } else {
            None
        };
        Ok(span.end(&self, | span | Stmt::Return {span, expr}))
    }
    
    fn parse_expr_stmt(&mut self) -> Result<Stmt, Error> {
        let span = self.begin_span();
        let expr = self.parse_expr() ?;
        self.expect_token(Token::Semi) ?;
        Ok(span.end(&self, | span | Stmt::Expr {span, expr}))
    }
    
    fn parse_ty_path(&mut self) -> Result<TyExpr, Error> {
        let span = self.begin_span();
        let mut string = String::new();
        write!(string, "{}", self.parse_ident() ?).unwrap();
        self.expect_token(Token::PathSep) ?;
        loop {
            write!(string, "::{}", self.parse_ident() ?).unwrap();
            if !self.accept_token(Token::PathSep) {
                break;
            }
        }
        self.expect_token(Token::LeftParen) ?;
        self.expect_token(Token::RightParen) ?;
        Ok(span.end(&self, | span | TyExpr {
            ty: RefCell::new(None),
            kind: TyExprKind::Var {
                span,
                ident: Ident::new(string)
            }
        }))
    }
    
    fn parse_ty_expr(&mut self) -> Result<TyExpr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_prim_ty_expr() ?;
        if self.accept_token(Token::LeftBracket) {
            let elem_ty_expr = Box::new(acc);
            match self.peek_token() {
                Token::Lit(Lit::Int(len)) => {
                    self.skip_token();
                    self.expect_token(Token::RightBracket) ?;
                    acc = span.end(&self, | span | TyExpr {
                        ty: RefCell::new(None),
                        kind: TyExprKind::Array {span, elem_ty_expr, len},
                    });
                }
                token => {
                    return Err(span.error(self, format!("unexpected token `{}`", token).into()))
                },
            }
        }
        Ok(acc)
    }
    
    fn parse_prim_ty_expr(&mut self) -> Result<TyExpr, Error> {
        let span = self.begin_span();
        match self.peek_token() {
            Token::Ident(ident) => {
                self.skip_token();
                Ok(span.end(&self, | span | TyExpr {
                    ty: RefCell::new(None),
                    kind: TyExprKind::Var {span, ident},
                }))
            }
            Token::TyLit(ty_lit) => {
                self.skip_token();
                Ok(span.end(&self, | span | TyExpr {
                    ty: RefCell::new(None),
                    kind: TyExprKind::Lit {span, ty_lit},
                }))
            }
            token => {
                Err(span.error(self, format!("unexpected token `{}`", token).into()))
            },
        }
    }
    
    fn parse_expr(&mut self) -> Result<Expr, Error> {
        self.parse_assign_expr()
    }
    
    fn parse_assign_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let expr = self.parse_cond_expr() ?;
        Ok(if let Some(op) = self.peek_token().to_assign_op() {
            self.skip_token();
            let left_expr = Box::new(expr);
            let right_expr = Box::new(self.parse_assign_expr() ?);
            span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_cond_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let expr = self.parse_or_expr() ?;
        Ok(if self.accept_token(Token::Question) {
            let expr = Box::new(expr);
            let expr_if_true = Box::new(self.parse_expr() ?);
            self.expect_token(Token::Colon) ?;
            let expr_if_false = Box::new(self.parse_cond_expr() ?);
            span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_or_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_and_expr() ?;
        while let Some(op) = self.peek_token().to_or_op() {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.parse_and_expr() ?);
            acc = span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_and_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_eq_expr() ?;
        while let Some(op) = self.peek_token().to_and_op() {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.parse_eq_expr() ?);
            acc = span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_eq_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_rel_expr() ?;
        while let Some(op) = self.peek_token().to_eq_op() {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.parse_rel_expr() ?);
            acc = span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_rel_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_add_expr() ?;
        while let Some(op) = self.peek_token().to_rel_op() {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.parse_add_expr() ?);
            acc = span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_add_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_mul_expr() ?;
        while let Some(op) = self.peek_token().to_add_op() {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.parse_mul_expr() ?);
            acc = span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_mul_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_un_expr() ?;
        while let Some(op) = self.peek_token().to_mul_op() {
            self.skip_token();
            let left_expr = Box::new(acc);
            let right_expr = Box::new(self.parse_postfix_expr() ?);
            acc = span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
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
    
    fn parse_un_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        Ok(if let Some(op) = self.peek_token().to_un_op() {
            self.skip_token();
            let expr = Box::new(self.parse_un_expr() ?);
            span.end(self, | span | Expr {
                ty: RefCell::new(None),
                val: RefCell::new(None),
                kind: ExprKind::Un {
                    span,
                    op,
                    expr
                },
            })
        } else {
            self.parse_postfix_expr() ?
        })
    }
    
    fn parse_postfix_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        let mut acc = self.parse_prim_expr() ?;
        loop {
            match self.peek_token() {
                Token::Dot => {
                    self.skip_token();
                    let ident = self.parse_ident() ?;
                    acc = Expr {
                        ty: RefCell::new(None),
                        val: RefCell::new(None),
                        kind: if self.accept_token(Token::LeftParen) {
                            let mut arg_exprs = vec![acc];
                            if !self.accept_token(Token::RightParen) {
                                loop {
                                    arg_exprs.push(self.parse_expr() ?);
                                    if !self.accept_token(Token::Comma) {
                                        break;
                                    }
                                }
                                self.expect_token(Token::RightParen) ?;
                            }
                            span.end(self, | span | ExprKind::MethodCall {
                                span,
                                ident,
                                arg_exprs,
                            })
                        } else {
                            let expr = Box::new(acc);
                            span.end(self, | span | ExprKind::Field {
                                span,
                                expr,
                                field_ident: ident,
                            })
                        }
                    };
                }
                Token::LeftBracket => {
                    self.skip_token();
                    let expr = Box::new(acc);
                    let index_expr = Box::new(self.parse_expr() ?);
                    self.expect_token(Token::RightBracket) ?;
                    acc = span.end(self, | span | Expr {
                        ty: RefCell::new(None),
                        val: RefCell::new(None),
                        kind: ExprKind::Index {
                            span,
                            expr,
                            index_expr
                        },
                    });
                }
                _ => break,
            }
        }
        Ok(acc)
    }
    
    fn parse_prim_expr(&mut self) -> Result<Expr, Error> {
        let span = self.begin_span();
        match self.peek_token() {
            Token::Self_ => {
                self.skip_token();
                Ok(Expr {
                    ty: RefCell::new(None),
                    val: RefCell::new(None),
                    kind: span.end(self, |span| ExprKind::Var {
                        span,
                        kind: Cell::new(None),
                        ident: Ident::new("self"),
                    })
                })
            }
            Token::Ident(mut ident) => {
                self.skip_token();
                Ok(Expr {
                    ty: RefCell::new(None),
                    val: RefCell::new(None),
                    kind: {
                        match self.peek_token() {
                            Token::Not => {
                                self.skip_token();
                                let arg_exprs = self.parse_arg_exprs()?;
                                span.end(self, |span| ExprKind::MacroCall {
                                    span,
                                    analysis: Cell::new(None),
                                    ident,
                                    arg_exprs
                                })
                            }
                            Token::PathSep => {
                                self.skip_token();
                                ident = Ident::new(
                                    format!("{}::{}", ident, self.parse_ident()?)
                                );
                                let arg_exprs = self.parse_arg_exprs()?;
                                span.end(self, |span| ExprKind::Call {
                                    span,
                                    ident,
                                    arg_exprs
                                })
                            }
                            Token::LeftParen => {
                                let arg_exprs = self.parse_arg_exprs()?;
                                span.end(self, |span| ExprKind::Call {
                                    span,
                                    ident,
                                    arg_exprs
                                })
                            }
                            _ => {
                                span.end(self, |span| ExprKind::Var {
                                    span,
                                    kind: Cell::new(None),
                                    ident,
                                })
                            }
                        }
                    }
                })
            }
            Token::Lit(lit) => {
                self.skip_token();
                Ok(span.end(self, | span | Expr {
                    ty: RefCell::new(None),
                    val: RefCell::new(None),
                    kind: ExprKind::Lit {
                        span,
                        lit
                    },
                }))
            }
            Token::TyLit(ty_lit) => {
                self.skip_token();
                self.expect_token(Token::LeftParen) ?;
                let mut arg_exprs = Vec::new();
                if !self.accept_token(Token::RightParen) {
                    loop {
                        arg_exprs.push(self.parse_expr() ?);
                        if !self.accept_token(Token::Comma) {
                            break;
                        }
                    }
                    self.expect_token(Token::RightParen) ?;
                }
                Ok(span.end(self, | span | Expr {
                    ty: RefCell::new(None),
                    val: RefCell::new(None),
                    kind: ExprKind::ConsCall {
                        span,
                        ty_lit,
                        arg_exprs
                    },
                }))
            }
            Token::LeftParen => {
                self.skip_token();
                let expr = self.parse_expr() ?;
                self.expect_token(Token::RightParen) ?;
                Ok(expr)
            }
            token => {
                Err(span.error(self, format!("unexpected token `{}`", token).into()))
            },
        }
    }

    fn parse_ident(&mut self) -> Result<Ident, Error> {
        let span = self.begin_span();
        match self.peek_token() {
            Token::Ident(ident) => {
                self.skip_token();
                Ok(ident)
            }
            token => {
                Err(span.error(self, format!("unexpected token `{}`", token).into()))
            },
        }
    }

    fn parse_arg_exprs(&mut self) -> Result<Vec<Expr>, Error> {
        self.expect_token(Token::LeftParen)?;
        let mut arg_exprs = Vec::new();
        if !self.accept_token(Token::RightParen) {
            loop {
                arg_exprs.push(self.parse_expr()?);
                if !self.accept_token(Token::Comma) {
                    break;
                }
            }
            self.expect_token(Token::RightParen)?;
        }
        Ok(arg_exprs)
    }
    
    fn accept_token(&mut self, token: Token) -> bool {
        if self.peek_token() != token {
            return false;
        }
        self.skip_token();
        true
    }
    
    fn expect_token(&mut self, expected: Token) -> Result<(), Error> {
        let span = self.begin_span();
        let actual = self.peek_token();
        if actual != expected {
            return Err(span.error(self, format!("unexpected token `{}`", actual).into()));
        }
        self.skip_token();
        Ok(())
    }
    
    fn peek_token(&self) -> Token {
        self.token_with_span.token
    }
    
    fn skip_token(&mut self) {
        self.end = self.token_with_span.span.end;
        self.token_with_span = self.tokens_with_span.next().unwrap();
    }
    
    fn begin_span(&self) -> SpanTracker {
        SpanTracker {
            loc_id: self.token_with_span.span.loc_id,
            start: self.token_with_span.span.start,
        }
    }
}

impl Token {
    fn to_assign_op(self) -> Option<BinOp> {
        match self {
            Token::Eq => Some(BinOp::Assign),
            Token::PlusEq => Some(BinOp::AddAssign),
            Token::MinusEq => Some(BinOp::SubAssign),
            Token::StarEq => Some(BinOp::MulAssign),
            Token::SlashEq => Some(BinOp::DivAssign),
            _ => None,
        }
    }
    
    fn to_or_op(self) -> Option<BinOp> {
        match self {
            Token::OrOr => Some(BinOp::Or),
            _ => None,
        }
    }
    
    fn to_and_op(self) -> Option<BinOp> {
        match self {
            Token::AndAnd => Some(BinOp::And),
            _ => None,
        }
    }
    
    fn to_eq_op(self) -> Option<BinOp> {
        match self {
            Token::EqEq => Some(BinOp::Eq),
            Token::NotEq => Some(BinOp::Ne),
            _ => None,
        }
    }
    
    fn to_rel_op(self) -> Option<BinOp> {
        match self {
            Token::Lt => Some(BinOp::Le),
            Token::LtEq => Some(BinOp::Lt),
            Token::Gt => Some(BinOp::Gt),
            Token::GtEq => Some(BinOp::Ge),
            _ => None,
        }
    }
    
    fn to_add_op(self) -> Option<BinOp> {
        match self {
            Token::Plus => Some(BinOp::Add),
            Token::Minus => Some(BinOp::Sub),
            _ => None,
        }
    }
    
    fn to_mul_op(self) -> Option<BinOp> {
        match self {
            Token::Star => Some(BinOp::Mul),
            Token::Slash => Some(BinOp::Div),
            _ => None,
        }
    }
    
    fn to_un_op(self) -> Option<UnOp> {
        match self {
            Token::Not => Some(UnOp::Not),
            Token::Minus => Some(UnOp::Neg),
            _ => None,
        }
    }
}

struct SpanTracker {
    loc_id: usize,
    start: usize,
}

impl SpanTracker {
    fn end<F, R>(&self, parser: &Parser, f: F) -> R
    where
    F: FnOnce(Span) -> R
    {
        f(Span {
            loc_id: self.loc_id,
            start: self.start,
            end: parser.end,
        })
    }
    
    fn error(&self, parser: &Parser, message: String) -> Error {
        Error {
            span: Span {
                loc_id: self.loc_id,
                start: self.start,
                end: parser.token_with_span.span.end,
            },
            message,
        }
    }
}