use crate::ast::*;
use crate::ident::Ident;
use crate::lit::Lit;
use crate::token::Token;
use std::error;
use std::fmt;

#[derive(Clone, Debug)]
pub struct Parser<T> {
    token: Token,
    tokens: T,
}

impl<T> Parser<T>
where
    T: Iterator<Item = Token>,
{
    pub fn new(mut tokens: T) -> Parser<T> {
        let token = tokens.next().unwrap();
        Parser { token, tokens }
    }
}

impl<T> Parser<T>
where
    T: Iterator<Item = Token>,
{
    fn accept_token(&mut self, token: Token) -> bool {
        if self.token != token {
            return false;
        }
        self.skip_token();
        true
    }

    fn expect_token(&mut self, token: Token) -> Result<(), Error> {
        if self.token != token {
            return Err(Error::UnexpectedToken(self.token));
        }
        self.skip_token();
        Ok(())
    }

    fn skip_token(&mut self) {
        self.token = self.tokens.next().unwrap();
    }
}

impl ParsedShader {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<ParsedShader, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut decls = Vec::new();
        while parser.token != Token::Eof {
            decls.push(Decl::parse(parser)?)
        }
        Ok(ParsedShader { decls })
    }
}

impl Decl {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<Decl, Error>
    where
        T: Iterator<Item = Token>,
    {
        match parser.token {
            Token::Attribute => Ok(Decl::Attribute(AttributeDecl::parse(parser)?)),
            Token::Fn => Ok(Decl::Fn(FnDecl::parse(parser)?)),
            Token::Struct => Ok(Decl::Struct(StructDecl::parse(parser)?)),
            Token::Uniform => Ok(Decl::Uniform(UniformDecl::parse(parser)?)),
            Token::Varying => Ok(Decl::Varying(VaryingDecl::parse(parser)?)),
            _ => Err(Error::UnexpectedToken(parser.token)),
        }
    }
}

impl AttributeDecl {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<AttributeDecl, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Attribute)?;
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::Colon)?;
        let ty_expr = TyExpr::parse(parser)?;
        parser.expect_token(Token::Semi)?;
        Ok(AttributeDecl { ident, ty_expr })
    }
}

impl FnDecl {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<FnDecl, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Fn)?;
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::LeftParen)?;
        let mut params = Vec::new();
        if !parser.accept_token(Token::RightParen) {
            loop {
                params.push(Param::parse(parser)?);
                if !parser.accept_token(Token::Comma) {
                    break;
                }
            }
            parser.expect_token(Token::RightParen)?;
        }
        let return_ty_expr = if parser.accept_token(Token::Arrow) {
            Some(TyExpr::parse(parser)?)
        } else {
            None
        };
        let block = Block::parse(parser)?;
        Ok(FnDecl {
            ident,
            params,
            return_ty_expr,
            block,
        })
    }
}

impl Param {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<Param, Error>
    where
        T: Iterator<Item = Token>,
    {
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::Colon)?;
        let ty_expr = TyExpr::parse(parser)?;
        Ok(Param { ident, ty_expr })
    }
}

impl StructDecl {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<StructDecl, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Struct)?;
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::LeftBrace)?;
        let mut members = Vec::new();
        if !parser.accept_token(Token::RightBrace) {
            loop {
                members.push(Member::parse(parser)?);
                if !parser.accept_token(Token::Comma) {
                    break;
                }
            }
            parser.expect_token(Token::RightBrace)?;
        }
        Ok(StructDecl { ident, members })
    }
}

impl UniformDecl {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<UniformDecl, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Uniform)?;
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::Colon)?;
        let ty_expr = TyExpr::parse(parser)?;
        let block_ident = if parser.accept_token(Token::Block) {
            Some(Ident::parse(parser)?)
        } else {
            None
        };
        parser.expect_token(Token::Semi)?;
        Ok(UniformDecl {
            ident,
            ty_expr,
            block_ident,
        })
    }
}

impl VaryingDecl {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<VaryingDecl, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Varying)?;
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::Colon)?;
        let ty_expr = TyExpr::parse(parser)?;
        parser.expect_token(Token::Semi)?;
        Ok(VaryingDecl { ident, ty_expr })
    }
}

impl Member {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<Member, Error>
    where
        T: Iterator<Item = Token>,
    {
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::Colon)?;
        let ty_expr = TyExpr::parse(parser)?;
        Ok(Member { ident, ty_expr })
    }
}

impl Block {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<Block, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::LeftBrace)?;
        let mut stmts = Vec::new();
        while !parser.accept_token(Token::RightBrace) {
            stmts.push(Stmt::parse(parser)?);
        }
        Ok(Block { stmts })
    }
}

impl Stmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<Stmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        Ok(match parser.token {
            Token::Break => Stmt::Break(BreakStmt::parse(parser)?),
            Token::Continue => Stmt::Continue(ContinueStmt::parse(parser)?),
            Token::For => Stmt::For(ForStmt::parse(parser)?),
            Token::If => Stmt::If(IfStmt::parse(parser)?),
            Token::Let => Stmt::Let(LetStmt::parse(parser)?),
            Token::Return => Stmt::Return(ReturnStmt::parse(parser)?),
            Token::LeftBrace => Stmt::Block(BlockStmt::parse(parser)?),
            _ => Stmt::Expr(ExprStmt::parse(parser)?),
        })
    }
}

impl BreakStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<BreakStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Break)?;
        parser.expect_token(Token::Semi)?;
        Ok(BreakStmt)
    }
}

impl ContinueStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<ContinueStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Continue)?;
        parser.expect_token(Token::Semi)?;
        Ok(ContinueStmt)
    }
}

impl ForStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<ForStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::For)?;
        let ident = Ident::parse(parser)?;
        parser.expect_token(Token::From)?;
        let from_expr = Expr::parse(parser)?;
        parser.expect_token(Token::To)?;
        let to_expr = Expr::parse(parser)?;
        let step_expr = if parser.accept_token(Token::Step) {
            Some(Expr::parse(parser)?)
        } else {
            None
        };
        let block = Block::parse(parser)?;
        Ok(ForStmt {
            ident,
            from_expr,
            to_expr,
            step_expr,
            block,
        })
    }
}

impl IfStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<IfStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::If)?;
        let expr = Expr::parse(parser)?;
        let block_if_true = Block::parse(parser)?;
        let block_if_false = if parser.accept_token(Token::Else) {
            Some(if parser.token == Token::If {
                Block {
                    stmts: vec![Stmt::If(IfStmt::parse(parser)?)],
                }
            } else {
                Block::parse(parser)?
            })
        } else {
            None
        };
        Ok(IfStmt {
            expr,
            block_if_true,
            block_if_false,
        })
    }
}

impl LetStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<LetStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Let)?;
        let ident = Ident::parse(parser)?;
        let ty_expr = if parser.accept_token(Token::Colon) {
            Some(TyExpr::parse(parser)?)
        } else {
            None
        };
        let expr = if parser.accept_token(Token::Eq) {
            Some(Expr::parse(parser)?)
        } else {
            None
        };
        parser.expect_token(Token::Semi)?;
        Ok(LetStmt {
            ident,
            ty_expr,
            expr,
        })
    }
}

impl ReturnStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<ReturnStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        parser.expect_token(Token::Return)?;
        let expr = if !parser.accept_token(Token::Semi) {
            let expr = Expr::parse(parser)?;
            parser.expect_token(Token::Semi)?;
            Some(expr)
        } else {
            None
        };
        Ok(ReturnStmt { expr })
    }
}

impl BlockStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<BlockStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        let block = Block::parse(parser)?;
        Ok(BlockStmt { block })
    }
}

impl ExprStmt {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<ExprStmt, Error>
    where
        T: Iterator<Item = Token>,
    {
        let expr = Expr::parse(parser)?;
        parser.expect_token(Token::Semi)?;
        Ok(ExprStmt { expr })
    }
}

impl TyExpr {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<TyExpr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let ty_expr = TyExpr::parse_prim(parser)?;
        Ok(if parser.accept_token(Token::LeftBracket) {
            let elem_ty_expr = Box::new(ty_expr);
            let len = match parser.token {
                Token::Lit(Lit::Int(lit)) => {
                    parser.skip_token();
                    lit
                }
                _ => return Err(Error::UnexpectedToken(parser.token)),
            };
            parser.expect_token(Token::RightBracket)?;
            TyExpr::Array(ArrayTyExpr { elem_ty_expr, len })
        } else {
            ty_expr
        })
    }

    pub fn parse_prim<T>(parser: &mut Parser<T>) -> Result<TyExpr, Error>
    where
        T: Iterator<Item = Token>,
    {
        match parser.token {
            Token::Ident(ident) => {
                parser.skip_token();
                Ok(TyExpr::Struct(StructTyExpr { ident }))
            }
            Token::TyLit(ty_lit) => {
                parser.skip_token();
                Ok(TyExpr::TyLit(ty_lit))
            }
            _ => Err(Error::UnexpectedToken(parser.token)),
        }
    }
}

impl Expr {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        Expr::parse_assign(parser)
    }

    pub fn parse_assign<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_cond(parser)?;
        while let Some(op) = parser.token.to_assign_op() {
            parser.skip_token();
            let x = Box::new(acc);
            let y = Box::new(Expr::parse_cond(parser)?);
            acc = Expr::Bin(BinExpr { op, x, y });
        }
        Ok(acc)
    }

    pub fn parse_cond<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let expr = Expr::parse_or(parser)?;
        Ok(if parser.accept_token(Token::Question) {
            let x = Box::new(expr);
            let y = Box::new(Expr::parse(parser)?);
            parser.expect_token(Token::Colon)?;
            let z = Box::new(Expr::parse_cond(parser)?);
            Expr::Cond(CondExpr { x, y, z })
        } else {
            expr
        })
    }

    pub fn parse_or<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_and(parser)?;
        while let Some(op) = parser.token.to_or_op() {
            parser.skip_token();
            let x = Box::new(acc);
            let y = Box::new(Expr::parse_and(parser)?);
            acc = Expr::Bin(BinExpr { op, x, y });
        }
        Ok(acc)
    }

    pub fn parse_and<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_eq(parser)?;
        while let Some(op) = parser.token.to_and_op() {
            parser.skip_token();
            let x = Box::new(acc);
            let y = Box::new(Expr::parse_eq(parser)?);
            acc = Expr::Bin(BinExpr { op, x, y });
        }
        Ok(acc)
    }

    pub fn parse_eq<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_rel(parser)?;
        while let Some(op) = parser.token.to_eq_op() {
            parser.skip_token();
            let x = Box::new(acc);
            let y = Box::new(Expr::parse_rel(parser)?);
            acc = Expr::Bin(BinExpr { op, x, y });
        }
        Ok(acc)
    }

    pub fn parse_rel<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_add(parser)?;
        while let Some(op) = parser.token.to_rel_op() {
            parser.skip_token();
            let x = Box::new(acc);
            let y = Box::new(Expr::parse_add(parser)?);
            acc = Expr::Bin(BinExpr { op, x, y });
        }
        Ok(acc)
    }

    pub fn parse_add<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_mul(parser)?;
        while let Some(op) = parser.token.to_add_op() {
            parser.skip_token();
            let x = Box::new(acc);
            let y = Box::new(Expr::parse_mul(parser)?);
            acc = Expr::Bin(BinExpr { op, x, y });
        }
        Ok(acc)
    }

    pub fn parse_mul<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_postfix(parser)?;
        while let Some(op) = parser.token.to_mul_op() {
            parser.skip_token();
            let x = Box::new(acc);
            let y = Box::new(Expr::parse_postfix(parser)?);
            acc = Expr::Bin(BinExpr { op, x, y });
        }
        Ok(acc)
    }

    pub fn parse_postfix<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        let mut acc = Expr::parse_un(parser)?;
        loop {
            match parser.token {
                Token::LeftBracket => {
                    parser.skip_token();
                    let x = Box::new(acc);
                    let i = Box::new(Expr::parse(parser)?);
                    parser.expect_token(Token::RightBracket)?;
                    acc = Expr::Index(IndexExpr { x, i })
                }
                Token::Dot => {
                    parser.skip_token();
                    let x = Box::new(acc);
                    let ident = Ident::parse(parser)?;
                    acc = Expr::Member(MemberExpr { x, ident });
                }
                _ => break,
            }
        }
        Ok(acc)
    }

    pub fn parse_un<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        if let Some(op) = parser.token.to_un_op() {
            parser.skip_token();
            let x = Box::new(Expr::parse_un(parser)?);
            Ok(Expr::Un(UnExpr { op, x }))
        } else {
            Expr::parse_prim(parser)
        }
    }

    pub fn parse_prim<T>(parser: &mut Parser<T>) -> Result<Expr, Error>
    where
        T: Iterator<Item = Token>,
    {
        match parser.token {
            Token::Ident(ident) => {
                parser.skip_token();
                Ok(if parser.accept_token(Token::LeftParen) {
                    let mut xs = Vec::new();
                    if !parser.accept_token(Token::RightParen) {
                        loop {
                            xs.push(Expr::parse(parser)?);
                            if !parser.accept_token(Token::Comma) {
                                break;
                            }
                        }
                        parser.expect_token(Token::RightParen)?;
                    }
                    Expr::Call(CallExpr { ident, xs })
                } else {
                    Expr::Var(VarExpr { ident })
                })
            }
            Token::Lit(lit) => {
                parser.skip_token();
                Ok(Expr::Lit(lit))
            }
            Token::TyLit(ty_lit) => {
                parser.skip_token();
                parser.expect_token(Token::LeftParen)?;
                let mut xs = Vec::new();
                if !parser.accept_token(Token::RightParen) {
                    loop {
                        xs.push(Expr::parse(parser)?);
                        if !parser.accept_token(Token::Comma) {
                            break;
                        }
                    }
                    parser.expect_token(Token::RightParen)?;
                }
                Ok(Expr::ConsCall(ConsCallExpr { ty_lit, xs }))
            }
            Token::LeftParen => {
                parser.skip_token();
                let expr = Expr::parse(parser)?;
                parser.expect_token(Token::RightParen)?;
                Ok(expr)
            }
            _ => Err(Error::UnexpectedToken(parser.token)),
        }
    }
}

impl Ident {
    pub fn parse<T>(parser: &mut Parser<T>) -> Result<Ident, Error>
    where
        T: Iterator<Item = Token>,
    {
        match parser.token {
            Token::Ident(ident) => {
                parser.skip_token();
                Ok(ident)
            }
            _ => Err(Error::UnexpectedToken(parser.token)),
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
            Token::Lt => Some(BinOp::Lt),
            Token::LtEq => Some(BinOp::Le),
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

#[derive(Clone, Copy, Debug)]
pub enum Error {
    UnexpectedToken(Token),
}

impl error::Error for Error {}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Error::UnexpectedToken(token) => write!(f, "unexpected token {}", token),
        }
    }
}
