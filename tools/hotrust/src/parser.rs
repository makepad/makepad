//! Recursive-descent parser (Pratt for expressions) over the token list of one
//! file. Builds an `Ast`. Errors stop the file: the base stack must parse with
//! zero errors, so there is no recovery.

use crate::ast::*;
use crate::lexer::{T, Tok};

pub struct PErr {
    pub pos: u32,
    pub msg: String,
}

pub type PResult<X> = Result<X, PErr>;

const NO_STRUCT: u8 = 1;

pub struct Parser<'a> {
    pub src: &'a [u8],
    pub toks: Vec<Tok>,
    pos: usize,
    pub edition: u16,
    pub ast: Ast,
    item_start: Vec<[u32; 2]>,
    /// skip fn bodies (brace match over tokens); parse them on first compile
    pub lazy_bodies: bool,
}

// precedence levels
const P_ASSIGN: u8 = 1;
const P_RANGE: u8 = 2;
const P_OROR: u8 = 3;
const P_ANDAND: u8 = 4;
const P_CMP: u8 = 5;
const P_BOR: u8 = 6;
const P_BXOR: u8 = 7;
const P_BAND: u8 = 8;
const P_SHIFT: u8 = 9;
const P_ADD: u8 = 10;
const P_MUL: u8 = 11;
const P_AS: u8 = 12;

impl<'a> Parser<'a> {
    pub fn new(src: &'a [u8], toks: Vec<Tok>, edition: u16) -> Self {
        Parser {
            src,
            toks,
            pos: 0,
            edition,
            ast: Ast::default(),
            item_start: Vec::new(),
            lazy_bodies: false,
        }
    }

    // ------------------------------------------------------------ token helpers

    #[inline]
    fn kind(&self) -> T {
        self.toks[self.pos].kind
    }
    #[inline]
    fn nth(&self, n: usize) -> T {
        let i = self.pos + n;
        if i < self.toks.len() {
            self.toks[i].kind
        } else {
            T::Eof
        }
    }
    #[inline]
    fn tok(&self) -> Tok {
        self.toks[self.pos]
    }
    #[inline]
    fn lo(&self) -> u32 {
        self.toks[self.pos].lo
    }
    #[inline]
    fn prev_hi(&self) -> u32 {
        if self.pos == 0 {
            0
        } else {
            self.toks[self.pos - 1].hi
        }
    }
    #[inline]
    fn bump(&mut self) -> Tok {
        let t = self.toks[self.pos];
        if t.kind != T::Eof {
            self.pos += 1;
        }
        t
    }
    #[inline]
    fn eat(&mut self, k: T) -> bool {
        if self.kind() == k {
            self.pos += 1;
            true
        } else {
            false
        }
    }
    fn err<X>(&self, msg: &str) -> PResult<X> {
        let t = self.tok();
        let text = String::from_utf8_lossy(&self.src[t.lo as usize..t.hi as usize]).into_owned();
        Err(PErr {
            pos: t.lo,
            msg: format!("{} (found {:?} `{}`)", msg, t.kind, text),
        })
    }
    fn expect(&mut self, k: T) -> PResult<Tok> {
        if self.kind() == k {
            Ok(self.bump())
        } else {
            self.err(&format!("expected {:?}", k))
        }
    }
    #[inline]
    fn text_of(&self, t: Tok) -> &'a [u8] {
        &self.src[t.lo as usize..t.hi as usize]
    }
    #[inline]
    fn is_word(&self, n: usize, w: &[u8]) -> bool {
        let i = self.pos + n;
        i < self.toks.len() && self.toks[i].kind == T::Ident && self.text_of(self.toks[i]) == w
    }
    fn ident_tok(&mut self) -> PResult<Ident> {
        match self.kind() {
            T::Ident | T::RawIdent => {
                let t = self.bump();
                Ok(Ident { lo: t.lo, hi: t.hi })
            }
            // edition 2015: async/await/dyn/try are idents; they lex as idents there.
            _ => self.err("expected identifier"),
        }
    }
    fn is_ident(&self) -> bool {
        matches!(self.kind(), T::Ident | T::RawIdent)
    }

    /// Split a compound token so its first char is consumed: `>>` -> `>` etc.
    fn eat_gt(&mut self) -> bool {
        match self.kind() {
            T::Gt => {
                self.pos += 1;
                true
            }
            T::Shr => {
                self.toks[self.pos].kind = T::Gt;
                self.toks[self.pos].lo += 1;
                true
            }
            T::Ge => {
                self.toks[self.pos].kind = T::Eq;
                self.toks[self.pos].lo += 1;
                true
            }
            T::ShrEq => {
                self.toks[self.pos].kind = T::Ge;
                self.toks[self.pos].lo += 1;
                true
            }
            _ => false,
        }
    }
    fn expect_gt(&mut self) -> PResult<()> {
        if self.eat_gt() {
            Ok(())
        } else {
            self.err("expected `>`")
        }
    }
    fn eat_lt(&mut self) -> bool {
        match self.kind() {
            T::Lt => {
                self.pos += 1;
                true
            }
            T::Shl => {
                self.toks[self.pos].kind = T::Lt;
                self.toks[self.pos].lo += 1;
                true
            }
            T::Le => {
                self.toks[self.pos].kind = T::Eq;
                self.toks[self.pos].lo += 1;
                true
            }
            T::ShlEq => {
                self.toks[self.pos].kind = T::Le;
                self.toks[self.pos].lo += 1;
                true
            }
            _ => false,
        }
    }
    fn eat_amp(&mut self) -> bool {
        match self.kind() {
            T::And => {
                self.pos += 1;
                true
            }
            T::AndAnd => {
                self.toks[self.pos].kind = T::And;
                self.toks[self.pos].lo += 1;
                true
            }
            _ => false,
        }
    }
    fn eat_or(&mut self) -> bool {
        match self.kind() {
            T::Or => {
                self.pos += 1;
                true
            }
            T::OrOr => {
                self.toks[self.pos].kind = T::Or;
                self.toks[self.pos].lo += 1;
                true
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------ arenas

    fn mk_expr(&mut self, kind: ExprKind, lo: u32) -> ExprId {
        let hi = self.prev_hi();
        self.ast.exprs.push(Expr { kind, lo, hi });
        ExprId(self.ast.exprs.len() as u32 - 1)
    }
    fn mk_pat(&mut self, p: Pat) -> PatId {
        self.ast.pats.push(p);
        PatId(self.ast.pats.len() as u32 - 1)
    }
    fn mk_ty(&mut self, t: Ty) -> TyId {
        self.ast.tys.push(t);
        TyId(self.ast.tys.len() as u32 - 1)
    }

    // ------------------------------------------------------------ token trees

    /// At an open delimiter: skips the balanced group; returns the inner range.
    fn delim_tt(&mut self) -> PResult<(u8, TokRange)> {
        let open = self.kind();
        let d = match open {
            T::OpenParen => b'(',
            T::OpenBracket => b'[',
            T::OpenBrace => b'{',
            _ => return self.err("expected delimiter"),
        };
        self.pos += 1;
        let lo = self.pos as u32;
        let mut depth = 1u32;
        loop {
            match self.kind() {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => {
                    depth -= 1;
                    if depth == 0 {
                        let hi = self.pos as u32;
                        self.pos += 1;
                        return Ok((d, TokRange { lo, hi }));
                    }
                }
                T::Eof => return self.err("unbalanced delimiters"),
                _ => {}
            }
            self.pos += 1;
        }
    }

    // ------------------------------------------------------------ built-in macro arguments

    fn mac_args(&mut self, path: &Path, toks: TokRange) -> u32 {
        let name = match path.segs.last() {
            Some(s) => &self.src[s.name.lo as usize..s.name.hi as usize],
            None => return NO_MAC_ARGS,
        };
        let known = matches!(
            name,
            b"format" | b"print" | b"println" | b"eprint" | b"eprintln" | b"write" | b"writeln" | b"panic" | b"format_args"
                | b"vec" | b"assert" | b"assert_eq" | b"assert_ne" | b"debug_assert" | b"debug_assert_eq" | b"debug_assert_ne"
                | b"matches" | b"unreachable" | b"todo" | b"unimplemented" | b"dbg"
        );
        if !known {
            return NO_MAC_ARGS;
        }
        let is_matches = name == b"matches";
        let is_vec = name == b"vec";
        let save = self.pos;
        self.pos = toks.lo as usize;
        let r = self.mac_args_inner(is_matches, is_vec, toks.hi as usize);
        self.pos = save;
        match r {
            Ok(a) => {
                self.ast.mac_args.push(a);
                self.ast.mac_args.len() as u32 - 1
            }
            Err(_) => NO_MAC_ARGS,
        }
    }

    fn mac_args_inner(&mut self, is_matches: bool, is_vec: bool, hi: usize) -> PResult<MacArgs> {
        let mut a = MacArgs { exprs: Vec::new(), repeat: false, pat: None, guard: None };
        if is_matches {
            a.exprs.push(self.expr()?);
            self.expect(T::Comma)?;
            a.pat = Some(self.pat_top()?);
            if self.eat(T::KwIf) {
                a.guard = Some(self.expr()?);
            }
            self.eat(T::Comma);
        } else {
            while self.pos < hi {
                a.exprs.push(self.expr()?);
                if is_vec && a.exprs.len() == 1 && self.eat(T::Semi) {
                    a.exprs.push(self.expr()?);
                    a.repeat = true;
                    break;
                }
                if !self.eat(T::Comma) {
                    break;
                }
            }
        }
        if self.pos != hi {
            return self.err("unexpected tokens in macro arguments");
        }
        Ok(a)
    }

    // ------------------------------------------------------------ attributes

    fn outer_attrs(&mut self) -> PResult<Vec<Attr>> {
        let mut v = Vec::new();
        while self.kind() == T::Pound && self.nth(1) == T::OpenBracket {
            self.pos += 1;
            let (_, toks) = self.delim_tt()?;
            v.push(Attr { inner: false, toks });
        }
        Ok(v)
    }
    fn inner_attrs(&mut self, v: &mut Vec<Attr>) -> PResult<()> {
        while self.kind() == T::Pound && self.nth(1) == T::Not && self.nth(2) == T::OpenBracket {
            self.pos += 2;
            let (_, toks) = self.delim_tt()?;
            v.push(Attr { inner: true, toks });
        }
        Ok(())
    }

    // ------------------------------------------------------------ file

    pub fn parse_file(&mut self) -> PResult<()> {
        let mut attrs = Vec::new();
        self.inner_attrs(&mut attrs)?;
        self.ast.root_attrs = attrs;
        let items = self.items_until(T::Eof)?;
        self.ast.root_items = items;
        Ok(())
    }

    fn items_until(&mut self, end: T) -> PResult<Vec<ItemId>> {
        let mut items = Vec::new();
        loop {
            if self.kind() == end {
                break;
            }
            if self.eat(T::Semi) {
                continue;
            }
            let attrs = self.outer_attrs()?;
            if self.kind() == end && !attrs.is_empty() {
                // trailing attrs (e.g. cfg'd out tail) are not valid Rust; be strict
                return self.err("attributes without item");
            }
            match self.item(attrs)? {
                Some(id) => items.push(id),
                None => return self.err("expected item"),
            }
        }
        Ok(items)
    }

    // ------------------------------------------------------------ visibility

    fn vis(&mut self) -> PResult<Vis> {
        if self.kind() == T::KwCrate && self.nth(1) != T::PathSep {
            // `crate` visibility (old nightly); treat as pub(crate)
            if matches!(self.nth(1), T::KwFn | T::KwStruct | T::KwEnum | T::KwMod | T::KwUse) {
                self.pos += 1;
                return Ok(Vis::Crate);
            }
        }
        if !self.eat(T::KwPub) {
            return Ok(Vis::Private);
        }
        if self.kind() == T::OpenParen {
            match (self.nth(1), self.nth(2)) {
                (T::KwCrate, T::CloseParen) => {
                    self.pos += 3;
                    return Ok(Vis::Crate);
                }
                (T::KwSuper, T::CloseParen) => {
                    self.pos += 3;
                    return Ok(Vis::Super);
                }
                (T::KwSelfValue, T::CloseParen) => {
                    self.pos += 3;
                    return Ok(Vis::SelfMod);
                }
                (T::KwIn, _) => {
                    self.pos += 2;
                    let p = self.path(false)?;
                    self.expect(T::CloseParen)?;
                    return Ok(Vis::In(p));
                }
                _ => {}
            }
        }
        Ok(Vis::Pub)
    }

    // ------------------------------------------------------------ items

    /// Is the current token the start of an item (used in statement position)?
    fn at_item_start(&self) -> bool {
        match self.kind() {
            T::KwPub | T::KwFn | T::KwStruct | T::KwEnum | T::KwTrait | T::KwType | T::KwMod | T::KwUse
            | T::KwImpl => true,
            T::KwExtern => true,
            T::KwStatic => matches!(self.nth(1), T::Ident | T::RawIdent | T::KwMut),
            T::KwConst => match self.nth(1) {
                T::Ident | T::RawIdent => self.nth(2) == T::Colon,
                T::KwFn | T::KwUnsafe | T::KwAsync | T::KwExtern => true,
                _ => false,
            },
            T::KwUnsafe => matches!(self.nth(1), T::KwFn | T::KwImpl | T::KwTrait | T::KwExtern)
                || self.is_word(1, b"auto"),
            T::KwAsync => matches!(self.nth(1), T::KwFn | T::KwUnsafe),
            T::Ident => {
                (self.is_word(0, b"union") && matches!(self.nth(1), T::Ident | T::RawIdent))
                    || (self.is_word(0, b"macro_rules") && self.nth(1) == T::Not)
                    || (self.is_word(0, b"auto") && self.nth(1) == T::KwTrait)
            }
            _ => false,
        }
    }

    fn item(&mut self, attrs: Vec<Attr>) -> PResult<Option<ItemId>> {
        let lo = self.lo();
        self.item_start.push([self.ast.exprs.len() as u32, self.ast.pats.len() as u32]);
        let r = self.item_inner(attrs, lo);
        self.item_start.pop();
        r
    }

    fn item_inner(&mut self, attrs: Vec<Attr>, lo: u32) -> PResult<Option<ItemId>> {
        let vis = self.vis()?;
        let mut name = Ident::default();
        // `default` (specialization) in impls
        let mut default_ = false;
        if self.is_word(0, b"default")
            && matches!(self.nth(1), T::KwFn | T::KwUnsafe | T::KwConst | T::KwType | T::KwImpl | T::KwAsync | T::KwExtern | T::KwPub)
        {
            self.pos += 1;
            default_ = true;
        }
        let kind = match self.kind() {
            T::KwUse => {
                self.pos += 1;
                let global = self.eat(T::PathSep);
                let tree = self.use_tree()?;
                self.expect(T::Semi)?;
                ItemKind::Use { global, tree }
            }
            T::KwExtern if self.nth(1) == T::KwCrate => {
                self.pos += 2;
                name = if self.eat(T::KwSelfValue) {
                    Ident { lo: self.prev_hi() - 4, hi: self.prev_hi() }
                } else {
                    self.ident_tok()?
                };
                let rename = if self.eat(T::KwAs) { Some(self.ident_or_underscore()?) } else { None };
                self.expect(T::Semi)?;
                ItemKind::ExternCrate(rename)
            }
            T::KwExtern if matches!(self.nth(1), T::OpenBrace) || (self.nth(2) == T::OpenBrace && is_str(self.nth(1))) => {
                self.pos += 1;
                let abi = self.abi();
                let items = self.foreign_items()?;
                ItemKind::ForeignMod(abi, items)
            }
            T::KwUnsafe if self.nth(1) == T::KwExtern && (self.nth(2) == T::OpenBrace || (is_str(self.nth(2)) && self.nth(3) == T::OpenBrace)) => {
                self.pos += 2;
                let abi = self.abi();
                let items = self.foreign_items()?;
                ItemKind::ForeignMod(abi, items)
            }
            T::KwFn | T::KwConst | T::KwAsync | T::KwUnsafe | T::KwExtern
                if self.fn_ahead() =>
            {
                let (n, sig, body) = self.fn_item()?;
                name = n;
                ItemKind::Fn(Box::new(sig), body, default_)
            }
            T::Ident if (self.is_word(0, b"safe")) && self.fn_ahead_from(1) => {
                self.pos += 1;
                let (n, sig, body) = self.fn_item()?;
                name = n;
                ItemKind::Fn(Box::new(sig), body, default_)
            }
            T::KwStruct => {
                self.pos += 1;
                name = self.ident_tok()?;
                let mut g = self.generics()?;
                let data = if self.kind() == T::OpenParen {
                    let f = self.tuple_fields()?;
                    self.where_clause(&mut g)?;
                    self.expect(T::Semi)?;
                    VariantData::Tuple(f)
                } else {
                    self.where_clause(&mut g)?;
                    if self.eat(T::Semi) {
                        VariantData::Unit
                    } else {
                        VariantData::Struct(self.named_fields()?)
                    }
                };
                ItemKind::Struct(g, data)
            }
            T::Ident if self.is_word(0, b"union") && matches!(self.nth(1), T::Ident | T::RawIdent) => {
                self.pos += 1;
                name = self.ident_tok()?;
                let mut g = self.generics()?;
                self.where_clause(&mut g)?;
                ItemKind::Union(g, self.named_fields()?)
            }
            T::KwEnum => {
                self.pos += 1;
                name = self.ident_tok()?;
                let mut g = self.generics()?;
                self.where_clause(&mut g)?;
                self.expect(T::OpenBrace)?;
                let mut vars = Vec::new();
                while self.kind() != T::CloseBrace {
                    let attrs = self.outer_attrs()?;
                    let _ = self.vis()?;
                    let vname = self.ident_tok()?;
                    let data = match self.kind() {
                        T::OpenParen => VariantData::Tuple(self.tuple_fields()?),
                        T::OpenBrace => VariantData::Struct(self.named_fields()?),
                        _ => VariantData::Unit,
                    };
                    let disc = if self.eat(T::Eq) { Some(self.expr()?) } else { None };
                    vars.push(Variant { attrs, name: vname, data, disc });
                    if !self.eat(T::Comma) {
                        break;
                    }
                }
                self.expect(T::CloseBrace)?;
                ItemKind::Enum(g, vars)
            }
            T::KwTrait => {
                self.pos += 1;
                self.trait_item(&mut name, false, false)?
            }
            T::KwUnsafe if self.nth(1) == T::KwTrait => {
                self.pos += 2;
                self.trait_item(&mut name, true, false)?
            }
            T::Ident if self.is_word(0, b"auto") && self.nth(1) == T::KwTrait => {
                self.pos += 2;
                self.trait_item(&mut name, false, true)?
            }
            T::KwUnsafe if self.is_word(1, b"auto") => {
                self.pos += 3;
                self.trait_item(&mut name, true, true)?
            }
            T::KwImpl => {
                self.pos += 1;
                self.impl_item(false, default_)?
            }
            T::KwUnsafe if self.nth(1) == T::KwImpl => {
                self.pos += 2;
                self.impl_item(true, default_)?
            }
            T::KwType => {
                self.pos += 1;
                name = self.ident_tok()?;
                let mut generics = self.generics()?;
                let bounds = if self.eat(T::Colon) { self.bounds()? } else { Vec::new() };
                self.where_clause(&mut generics)?;
                let ty = if self.eat(T::Eq) { Some(self.ty_plus()?) } else { None };
                self.where_clause(&mut generics)?;
                self.expect(T::Semi)?;
                ItemKind::TypeAlias { generics, bounds, ty }
            }
            T::KwConst => {
                self.pos += 1;
                name = self.ident_or_underscore()?;
                // generic consts (unstable) not supported
                let ty = if self.eat(T::Colon) { Some(self.ty()?) } else { None };
                let e = if self.eat(T::Eq) { Some(self.expr()?) } else { None };
                self.expect(T::Semi)?;
                ItemKind::Const(ty, e)
            }
            T::KwStatic => {
                self.pos += 1;
                let m = self.eat(T::KwMut);
                name = self.ident_tok()?;
                self.expect(T::Colon)?;
                let ty = self.ty()?;
                let e = if self.eat(T::Eq) { Some(self.expr()?) } else { None };
                self.expect(T::Semi)?;
                ItemKind::Static(m, ty, e)
            }
            T::KwMod => {
                self.pos += 1;
                name = self.ident_tok()?;
                if self.eat(T::Semi) {
                    ItemKind::Mod(None)
                } else {
                    self.expect(T::OpenBrace)?;
                    let mut inner = Vec::new();
                    self.inner_attrs(&mut inner)?;
                    let items = self.items_until(T::CloseBrace)?;
                    self.expect(T::CloseBrace)?;
                    let mut attrs = attrs;
                    attrs.extend(inner);
                    return Ok(Some(self.push_item(Item { ranges: [0; 4], attrs, vis, name, kind: ItemKind::Mod(Some(items)), lo, hi: self.prev_hi() })));
                }
            }
            T::Ident if self.is_word(0, b"macro_rules") && self.nth(1) == T::Not => {
                self.pos += 2;
                name = self.ident_tok()?;
                let (d, toks) = self.delim_tt()?;
                if d != b'{' {
                    self.eat(T::Semi);
                }
                ItemKind::MacroRules(toks)
            }
            T::Ident | T::PathSep | T::KwSelfValue | T::KwSuper | T::KwCrate
                if self.mac_ahead() =>
            {
                let path = self.path(true)?;
                self.expect(T::Not)?;
                // `name! ident { }` (macro 2.0 style) not supported
                let (delim, toks) = self.delim_tt()?;
                if delim != b'{' {
                    self.eat(T::Semi);
                }
                let args = self.mac_args(&path, toks);
                ItemKind::Mac(MacCall { path, delim, toks, args })
            }
            _ => {
                if matches!(vis, Vis::Private) && attrs.is_empty() && !default_ {
                    return Ok(None);
                }
                return self.err("expected item after attributes/visibility");
            }
        };
        let hi = self.prev_hi();
        Ok(Some(self.push_item(Item { ranges: [0; 4], attrs, vis, name, kind, lo, hi })))
    }

    fn push_item(&mut self, mut it: Item) -> ItemId {
        let st = self.item_start[self.item_start.len() - 1];
        it.ranges = [st[0], self.ast.exprs.len() as u32, st[1], self.ast.pats.len() as u32];
        self.ast.items.push(it);
        ItemId(self.ast.items.len() as u32 - 1)
    }

    fn ident_or_underscore(&mut self) -> PResult<Ident> {
        self.ident_tok()
    }

    fn abi(&mut self) -> Option<Ident> {
        if is_str(self.kind()) {
            let t = self.bump();
            Some(Ident { lo: t.lo, hi: t.hi })
        } else {
            None
        }
    }

    /// path `!` delimiter (item- or statement-level macro call)
    fn mac_ahead(&self) -> bool {
        let mut i = self.pos;
        if self.toks[i].kind == T::PathSep {
            i += 1;
        }
        loop {
            match self.toks[i].kind {
                T::Ident | T::RawIdent | T::KwSelfValue | T::KwSuper | T::KwCrate => i += 1,
                _ => return false,
            }
            match self.toks[i].kind {
                T::PathSep => i += 1,
                T::Not => {
                    return matches!(self.toks[i + 1].kind, T::OpenParen | T::OpenBracket | T::OpenBrace);
                }
                _ => return false,
            }
        }
    }

    fn fn_ahead(&self) -> bool {
        self.fn_ahead_from(0)
    }
    fn fn_ahead_from(&self, mut n: usize) -> bool {
        loop {
            match self.nth(n) {
                T::KwFn => return true,
                T::KwConst | T::KwAsync | T::KwUnsafe => n += 1,
                T::KwExtern => {
                    n += 1;
                    if is_str(self.nth(n)) {
                        n += 1;
                    }
                }
                T::Ident if self.is_word(n, b"safe") => n += 1,
                _ => return false,
            }
        }
    }

    fn fn_item(&mut self) -> PResult<(Ident, FnSig, Option<BlockId>)> {
        let mut konst = false;
        let mut asyncness = false;
        let mut unsafe_ = false;
        let mut abi = None;
        loop {
            match self.kind() {
                T::KwConst => konst = true,
                T::KwAsync => asyncness = true,
                T::KwUnsafe => unsafe_ = true,
                T::Ident if self.is_word(0, b"safe") => {}
                T::KwExtern => {
                    self.pos += 1;
                    abi = Some(self.abi());
                    continue;
                }
                T::KwFn => break,
                _ => return self.err("expected fn"),
            }
            self.pos += 1;
        }
        self.expect(T::KwFn)?;
        let name = self.ident_tok()?;
        let mut generics = self.generics()?;
        let (self_param, params, variadic) = self.fn_params()?;
        let ret = if self.eat(T::RArrow) { Some(self.ty_noplus_ret()?) } else { None };
        self.where_clause(&mut generics)?;
        let mut lazy_body = None;
        let body = if self.eat(T::Semi) {
            None
        } else if self.lazy_bodies && self.kind() == T::OpenBrace {
            let (_, r) = self.delim_tt()?;
            lazy_body = Some(r);
            None
        } else {
            Some(self.block()?)
        };
        Ok((
            name,
            FnSig {
                konst,
                asyncness,
                unsafe_,
                abi,
                generics,
                self_param,
                params,
                variadic,
                ret,
                lazy_body,
            },
            body,
        ))
    }

    fn ty_noplus_ret(&mut self) -> PResult<TyId> {
        self.ty()
    }

    fn fn_params(&mut self) -> PResult<(Option<SelfParam>, Vec<Param>, bool)> {
        self.expect(T::OpenParen)?;
        let mut self_param = None;
        let mut params = Vec::new();
        let mut variadic = false;
        let mut first = true;
        while self.kind() != T::CloseParen {
            let attrs = self.outer_attrs()?;
            if first {
                if let Some(sp) = self.self_param()? {
                    self_param = Some(sp);
                    first = false;
                    if !self.eat(T::Comma) {
                        break;
                    }
                    continue;
                }
            }
            first = false;
            if self.eat(T::DotDotDot) {
                variadic = true;
                self.eat(T::Comma);
                break;
            }
            // `name: ...` variadic
            if self.is_ident() && self.nth(1) == T::Colon && self.nth(2) == T::DotDotDot {
                self.pos += 3;
                variadic = true;
                self.eat(T::Comma);
                break;
            }
            let save = (self.pos,);
            let res = self.named_param();
            let (pat, ty) = match res {
                Ok(x) => x,
                Err(e) => {
                    if self.edition >= 2018 {
                        return Err(e);
                    }
                    // 2015 anonymous parameter
                    self.pos = save.0;
                    let ty = self.ty()?;
                    let pat = self.mk_pat(Pat::Wild);
                    (pat, ty)
                }
            };
            params.push(Param { attrs, pat, ty });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseParen)?;
        Ok((self_param, params, variadic))
    }

    fn named_param(&mut self) -> PResult<(PatId, TyId)> {
        let pat = self.pat_no_or()?;
        self.expect(T::Colon)?;
        let ty = self.ty()?;
        Ok((pat, ty))
    }

    fn self_param(&mut self) -> PResult<Option<SelfParam>> {
        let end = |k: T| matches!(k, T::Comma | T::CloseParen | T::Colon);
        match self.kind() {
            T::KwSelfValue if end(self.nth(1)) => {
                self.pos += 1;
                if self.eat(T::Colon) {
                    let ty = self.ty()?;
                    return Ok(Some(SelfParam::Typed(false, ty)));
                }
                Ok(Some(SelfParam::Value(false)))
            }
            T::KwMut if self.nth(1) == T::KwSelfValue && end(self.nth(2)) => {
                self.pos += 2;
                if self.eat(T::Colon) {
                    let ty = self.ty()?;
                    return Ok(Some(SelfParam::Typed(true, ty)));
                }
                Ok(Some(SelfParam::Value(true)))
            }
            T::And => {
                let (lt, n) = if self.nth(1) == T::Lifetime { (Some(self.toks[self.pos + 1]), 2) } else { (None, 1) };
                let (m, n2) = if self.nth(n) == T::KwMut { (true, n + 1) } else { (false, n) };
                if self.nth(n2) == T::KwSelfValue && matches!(self.nth(n2 + 1), T::Comma | T::CloseParen) {
                    self.pos += n2 + 1;
                    let lt = match lt {
                        Some(t) => Some(Ident { lo: t.lo, hi: t.hi }),
                        None => None,
                    };
                    Ok(Some(SelfParam::Ref(lt, m)))
                } else {
                    Ok(None)
                }
            }
            _ => Ok(None),
        }
    }

    fn tuple_fields(&mut self) -> PResult<Vec<FieldDef>> {
        self.expect(T::OpenParen)?;
        let mut v = Vec::new();
        while self.kind() != T::CloseParen {
            let attrs = self.outer_attrs()?;
            let vis = self.vis()?;
            let ty = self.ty_plus()?;
            v.push(FieldDef { attrs, vis, name: None, ty });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseParen)?;
        Ok(v)
    }

    fn named_fields(&mut self) -> PResult<Vec<FieldDef>> {
        self.expect(T::OpenBrace)?;
        let mut v = Vec::new();
        while self.kind() != T::CloseBrace {
            let attrs = self.outer_attrs()?;
            let vis = self.vis()?;
            let name = self.ident_tok()?;
            self.expect(T::Colon)?;
            let ty = self.ty_plus()?;
            v.push(FieldDef { attrs, vis, name: Some(name), ty });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseBrace)?;
        Ok(v)
    }

    fn trait_item(&mut self, name: &mut Ident, unsafe_: bool, auto: bool) -> PResult<ItemKind> {
        *name = self.ident_tok()?;
        let mut generics = self.generics()?;
        if self.eat(T::Eq) {
            let b = self.bounds()?;
            self.where_clause(&mut generics)?;
            self.expect(T::Semi)?;
            return Ok(ItemKind::TraitAlias(generics, b));
        }
        let supers = if self.eat(T::Colon) { self.bounds()? } else { Vec::new() };
        self.where_clause(&mut generics)?;
        let items = self.assoc_items()?;
        Ok(ItemKind::Trait { unsafe_, auto, generics, supers, items })
    }

    fn assoc_items(&mut self) -> PResult<Vec<ItemId>> {
        self.expect(T::OpenBrace)?;
        let mut _inner = Vec::new();
        self.inner_attrs(&mut _inner)?;
        let items = self.items_until(T::CloseBrace)?;
        self.expect(T::CloseBrace)?;
        Ok(items)
    }

    fn foreign_items(&mut self) -> PResult<Vec<ItemId>> {
        self.assoc_items()
    }

    fn impl_generics_ahead(&self) -> bool {
        if self.kind() != T::Lt {
            return false;
        }
        match self.nth(1) {
            T::Lifetime | T::Gt | T::KwConst | T::Pound => true,
            T::Ident | T::RawIdent => matches!(self.nth(2), T::Gt | T::Comma | T::Colon | T::Eq),
            _ => false,
        }
    }

    fn impl_item(&mut self, unsafe_: bool, default_: bool) -> PResult<ItemKind> {
        let mut generics = if self.impl_generics_ahead() { self.generics()? } else { Generics::default() };
        // `impl const Trait`
        if self.kind() == T::KwConst {
            self.pos += 1;
        }
        let negative = self.eat(T::Not);
        let first = self.ty_impl_header()?;
        let (trait_, self_ty) = if self.eat(T::KwFor) {
            let p = match &self.ast.tys[first.0 as usize] {
                Ty::Path(p) => p.clone(),
                _ => return self.err("expected trait path in impl"),
            };
            (Some(p), self.ty_impl_header()?)
        } else {
            (None, first)
        };
        self.where_clause(&mut generics)?;
        let items = self.assoc_items()?;
        Ok(ItemKind::Impl { unsafe_, default_, generics, negative, trait_, self_ty, items })
    }

    fn ty_impl_header(&mut self) -> PResult<TyId> {
        // bare trait path in 2015 may be `impl Trait + Send`; rare. dyn handled by ty().
        self.ty()
    }

    // ------------------------------------------------------------ use trees

    fn use_tree(&mut self) -> PResult<UseTree> {
        match self.kind() {
            T::Star => {
                self.pos += 1;
                Ok(UseTree::Glob)
            }
            T::OpenBrace => {
                self.pos += 1;
                let mut v = Vec::new();
                while self.kind() != T::CloseBrace {
                    self.eat(T::PathSep);
                    v.push(self.use_tree()?);
                    if !self.eat(T::Comma) {
                        break;
                    }
                }
                self.expect(T::CloseBrace)?;
                Ok(UseTree::Group(v))
            }
            _ => {
                let seg = self.path_seg_name()?;
                if self.eat(T::PathSep) {
                    let rest = self.use_tree()?;
                    Ok(UseTree::Path(seg, Box::new(rest)))
                } else {
                    let rename = if self.eat(T::KwAs) { Some(self.ident_or_underscore()?) } else { None };
                    Ok(UseTree::Name(seg, rename))
                }
            }
        }
    }

    fn path_seg_name(&mut self) -> PResult<PathSeg> {
        let t = self.tok();
        let kind = match t.kind {
            T::Ident | T::RawIdent => SegKind::Ident,
            T::KwSelfValue => SegKind::SelfValue,
            T::KwSelfType => SegKind::SelfType,
            T::KwSuper => SegKind::Super,
            T::KwCrate => SegKind::Crate,
            _ => return self.err("expected path segment"),
        };
        self.pos += 1;
        Ok(PathSeg { kind, name: Ident { lo: t.lo, hi: t.hi }, args: None })
    }

    // ------------------------------------------------------------ generics

    fn generics(&mut self) -> PResult<Generics> {
        let mut g = Generics::default();
        if !self.eat_lt() {
            return Ok(g);
        }
        loop {
            if self.eat_gt() {
                break;
            }
            let attrs = self.outer_attrs()?;
            if self.kind() == T::Lifetime {
                let t = self.bump();
                let mut bounds = Vec::new();
                if self.eat(T::Colon) {
                    while self.kind() == T::Lifetime {
                        let b = self.bump();
                        bounds.push(Ident { lo: b.lo, hi: b.hi });
                        if !self.eat(T::Plus) {
                            break;
                        }
                    }
                }
                g.params.push(GenericParam { attrs, name: Ident { lo: t.lo, hi: t.hi }, kind: GenericParamKind::Lifetime(bounds) });
            } else if self.eat(T::KwConst) {
                let name = self.ident_tok()?;
                self.expect(T::Colon)?;
                let ty = self.ty()?;
                let def = if self.eat(T::Eq) { Some(self.const_arg()?) } else { None };
                g.params.push(GenericParam { attrs, name, kind: GenericParamKind::Const(ty, def) });
            } else {
                let name = self.ident_tok()?;
                let bounds = if self.eat(T::Colon) { self.bounds()? } else { Vec::new() };
                let def = if self.eat(T::Eq) { Some(self.ty_plus()?) } else { None };
                g.params.push(GenericParam { attrs, name, kind: GenericParamKind::Type(bounds, def) });
            }
            if !self.eat(T::Comma) {
                self.expect_gt()?;
                break;
            }
        }
        Ok(g)
    }

    fn hrtb(&mut self) -> PResult<Vec<GenericParam>> {
        if self.kind() == T::KwFor && self.nth(1) == T::Lt {
            self.pos += 1;
            Ok(self.generics()?.params)
        } else {
            Ok(Vec::new())
        }
    }

    fn where_clause(&mut self, g: &mut Generics) -> PResult<()> {
        if !self.eat(T::KwWhere) {
            return Ok(());
        }
        loop {
            match self.kind() {
                T::OpenBrace | T::Semi | T::Eq | T::Eof => break,
                T::Lifetime => {
                    let t = self.bump();
                    self.expect(T::Colon)?;
                    let mut bs = Vec::new();
                    while self.kind() == T::Lifetime {
                        let b = self.bump();
                        bs.push(Ident { lo: b.lo, hi: b.hi });
                        if !self.eat(T::Plus) {
                            break;
                        }
                    }
                    g.where_.push(WherePred::Lifetime(Ident { lo: t.lo, hi: t.hi }, bs));
                }
                _ => {
                    let hrtb = self.hrtb()?;
                    let ty = self.ty()?;
                    self.expect(T::Colon)?;
                    let bounds = self.bounds()?;
                    g.where_.push(WherePred::Bound { hrtb, ty, bounds });
                }
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        Ok(())
    }

    fn at_bound_start(&self) -> bool {
        match self.kind() {
            T::Lifetime | T::Question | T::KwFor | T::Ident | T::RawIdent | T::PathSep | T::KwSelfType | T::KwSuper
            | T::KwCrate | T::KwSelfValue | T::Tilde | T::OpenParen | T::KwConst | T::KwAsync => true,
            T::KwUse => true,
            _ => false,
        }
    }

    fn bounds(&mut self) -> PResult<Vec<Bound>> {
        let mut v = Vec::new();
        while self.at_bound_start() {
            v.push(self.bound()?);
            if !self.eat(T::Plus) {
                break;
            }
        }
        Ok(v)
    }

    fn bound(&mut self) -> PResult<Bound> {
        if self.kind() == T::Lifetime {
            let t = self.bump();
            return Ok(Bound::Lifetime(Ident { lo: t.lo, hi: t.hi }));
        }
        if self.kind() == T::KwUse && self.nth(1) == T::Lt {
            self.pos += 1;
            let _ = self.generic_args_angle()?;
            return Ok(Bound::Use);
        }
        let paren = self.eat(T::OpenParen);
        let mut konst = false;
        if self.kind() == T::Tilde && self.nth(1) == T::KwConst {
            self.pos += 2;
            konst = true;
        } else if self.kind() == T::KwConst {
            self.pos += 1;
            konst = true;
        }
        self.eat(T::KwAsync);
        let maybe = self.eat(T::Question);
        let hrtb = self.hrtb()?;
        let path = self.path(false)?;
        if paren {
            self.expect(T::CloseParen)?;
        }
        Ok(Bound::Trait { hrtb, maybe, konst, path })
    }

    // ------------------------------------------------------------ paths

    /// Path in expression mode (`expr == true`: generics need `::<`) or type mode.
    pub fn path(&mut self, expr: bool) -> PResult<Path> {
        let lo = self.lo();
        let mut qself = None;
        let mut global = false;
        let mut segs = Vec::new();
        if self.kind() == T::Lt || self.kind() == T::Shl {
            self.eat_lt();
            let ty = self.ty()?;
            let trait_path = if self.eat(T::KwAs) { Some(self.path(false)?) } else { None };
            self.expect_gt()?;
            qself = Some(Box::new(QSelf { ty, trait_path }));
            self.expect(T::PathSep)?;
        } else if self.eat(T::PathSep) {
            global = true;
        }
        loop {
            let mut seg = self.path_seg_name()?;
            // generic args
            if expr {
                if self.kind() == T::PathSep && matches!(self.nth(1), T::Lt | T::Shl) {
                    self.pos += 1;
                    seg.args = Some(Box::new(self.generic_args_angle()?));
                }
            } else {
                if self.kind() == T::PathSep && matches!(self.nth(1), T::Lt | T::Shl) {
                    self.pos += 1;
                }
                if matches!(self.kind(), T::Lt | T::Shl) {
                    seg.args = Some(Box::new(self.generic_args_angle()?));
                } else if self.kind() == T::OpenParen && seg.kind == SegKind::Ident {
                    // Fn(A, B) -> C sugar
                    self.pos += 1;
                    let mut tys = Vec::new();
                    while self.kind() != T::CloseParen {
                        tys.push(self.ty_plus()?);
                        if !self.eat(T::Comma) {
                            break;
                        }
                    }
                    self.expect(T::CloseParen)?;
                    let ret = if self.eat(T::RArrow) { Some(self.ty()?) } else { None };
                    seg.args = Some(Box::new(GenericArgs::Paren(tys, ret)));
                }
            }
            segs.push(seg);
            if self.kind() == T::PathSep && matches!(self.nth(1), T::Ident | T::RawIdent | T::KwSelfValue | T::KwSelfType | T::KwSuper | T::KwCrate) {
                self.pos += 1;
                continue;
            }
            break;
        }
        Ok(Path { global, qself, segs, lo })
    }

    fn generic_args_angle(&mut self) -> PResult<GenericArgs> {
        if !self.eat_lt() {
            return self.err("expected `<`");
        }
        let mut v = Vec::new();
        loop {
            if self.eat_gt() {
                break;
            }
            let arg = match self.kind() {
                T::Lifetime => {
                    let t = self.bump();
                    GenericArg::Lifetime(Ident { lo: t.lo, hi: t.hi })
                }
                T::Ident | T::RawIdent if self.nth(1) == T::Eq => {
                    let name = self.ident_tok()?;
                    self.pos += 1;
                    GenericArg::Binding(name, None, self.ty_plus()?)
                }
                T::Ident | T::RawIdent if self.nth(1) == T::Colon => {
                    let name = self.ident_tok()?;
                    self.pos += 1;
                    GenericArg::Constraint(name, self.bounds()?)
                }
                T::Ident | T::RawIdent if self.nth(1) == T::Lt && self.gat_binding_ahead() => {
                    let name = self.ident_tok()?;
                    let ga = self.generic_args_angle()?;
                    self.expect(T::Eq)?;
                    GenericArg::Binding(name, Some(Box::new(ga)), self.ty_plus()?)
                }
                T::OpenBrace | T::Int | T::Float | T::Str | T::Char | T::Byte | T::KwTrue | T::KwFalse | T::Minus => {
                    GenericArg::Const(self.const_arg()?)
                }
                _ => GenericArg::Type(self.ty_plus()?),
            };
            v.push(arg);
            if !self.eat(T::Comma) {
                self.expect_gt()?;
                break;
            }
        }
        Ok(GenericArgs::Angle(v))
    }

    fn gat_binding_ahead(&self) -> bool {
        // Ident < ... > =
        let mut i = self.pos + 1;
        let mut depth = 0i32;
        loop {
            match self.toks[i].kind {
                T::Lt => depth += 1,
                T::Shl => depth += 2,
                T::Gt => depth -= 1,
                T::Shr => depth -= 2,
                T::Eof | T::Semi | T::OpenBrace => return false,
                _ => {}
            }
            i += 1;
            if depth < 0 {
                return false;
            }
            if depth == 0 {
                return self.toks[i].kind == T::Eq;
            }
        }
    }

    fn const_arg(&mut self) -> PResult<ExprId> {
        let lo = self.lo();
        match self.kind() {
            T::OpenBrace => {
                let b = self.block()?;
                Ok(self.mk_expr(ExprKind::Block(b, None), lo))
            }
            T::Minus => {
                self.pos += 1;
                let e = self.lit_expr()?;
                Ok(self.mk_expr(ExprKind::Unary(UnOp::Neg, e), lo))
            }
            T::Ident | T::RawIdent => {
                let p = self.path(true)?;
                Ok(self.mk_expr(ExprKind::Path(p), lo))
            }
            _ => self.lit_expr(),
        }
    }

    fn lit_expr(&mut self) -> PResult<ExprId> {
        let lo = self.lo();
        let k = match self.kind() {
            T::Int => LitKind::Int,
            T::Float => LitKind::Float,
            T::Str => LitKind::Str,
            T::ByteStr => LitKind::ByteStr,
            T::CStr => LitKind::CStr,
            T::RawStr => LitKind::RawStr,
            T::Char => LitKind::Char,
            T::Byte => LitKind::Byte,
            T::KwTrue => LitKind::Bool(true),
            T::KwFalse => LitKind::Bool(false),
            _ => return self.err("expected literal"),
        };
        self.pos += 1;
        Ok(self.mk_expr(ExprKind::Lit(k), lo))
    }

    // ------------------------------------------------------------ types

    pub fn ty(&mut self) -> PResult<TyId> {
        self.ty_inner(false)
    }
    fn ty_plus(&mut self) -> PResult<TyId> {
        self.ty_inner(true)
    }

    fn ty_inner(&mut self, plus: bool) -> PResult<TyId> {
        let t = match self.kind() {
            T::OpenParen => {
                self.pos += 1;
                if self.eat(T::CloseParen) {
                    Ty::Tuple(Vec::new())
                } else {
                    let first = self.ty_plus()?;
                    if self.eat(T::CloseParen) {
                        Ty::Paren(first)
                    } else {
                        let mut v = vec![first];
                        while self.eat(T::Comma) {
                            if self.kind() == T::CloseParen {
                                break;
                            }
                            v.push(self.ty_plus()?);
                        }
                        self.expect(T::CloseParen)?;
                        Ty::Tuple(v)
                    }
                }
            }
            T::Not => {
                self.pos += 1;
                Ty::Never
            }
            T::And | T::AndAnd => {
                self.eat_amp();
                let lt = if self.kind() == T::Lifetime {
                    let t = self.bump();
                    Some(Ident { lo: t.lo, hi: t.hi })
                } else {
                    None
                };
                let m = self.eat(T::KwMut);
                let inner = self.ty_inner(false)?;
                Ty::Ref(lt, m, inner)
            }
            T::Star => {
                self.pos += 1;
                let m = if self.eat(T::KwMut) {
                    true
                } else {
                    self.expect(T::KwConst)?;
                    false
                };
                let inner = self.ty_inner(false)?;
                Ty::Ptr(m, inner)
            }
            T::OpenBracket => {
                self.pos += 1;
                let elem = self.ty_plus()?;
                if self.eat(T::Semi) {
                    let n = self.expr()?;
                    self.expect(T::CloseBracket)?;
                    Ty::Array(elem, n)
                } else {
                    self.expect(T::CloseBracket)?;
                    Ty::Slice(elem)
                }
            }
            T::KwFn | T::KwUnsafe | T::KwExtern => self.fn_ptr_ty(Vec::new())?,
            T::KwFor => {
                let hrtb = self.hrtb()?;
                if matches!(self.kind(), T::KwFn | T::KwUnsafe | T::KwExtern) {
                    self.fn_ptr_ty(hrtb)?
                } else {
                    let path = self.path(false)?;
                    let mut b = vec![Bound::Trait { hrtb, maybe: false, konst: false, path }];
                    while self.eat(T::Plus) {
                        if !self.at_bound_start() {
                            break;
                        }
                        b.push(self.bound()?);
                    }
                    Ty::DynTrait(b, false)
                }
            }
            T::KwImpl => {
                self.pos += 1;
                Ty::ImplTrait(self.bounds()?)
            }
            T::KwDyn => {
                self.pos += 1;
                Ty::DynTrait(self.bounds()?, true)
            }
            T::Ident if self.edition < 2018 && self.is_word(0, b"dyn") && matches!(self.nth(1), T::Ident | T::PathSep | T::KwFor | T::Question | T::Lifetime | T::OpenParen) => {
                self.pos += 1;
                Ty::DynTrait(self.bounds()?, true)
            }
            T::Ident if self.is_word(0, b"_") => {
                self.pos += 1;
                Ty::Infer
            }
            T::DotDotDot => {
                self.pos += 1;
                Ty::CVarArgs
            }
            T::Question => {
                // `?Sized` as a bare trait object (2015) in bounds-like position
                let b = self.bounds()?;
                Ty::DynTrait(b, false)
            }
            T::Ident | T::RawIdent | T::PathSep | T::Lt | T::Shl | T::KwSelfType | T::KwSelfValue | T::KwSuper | T::KwCrate => {
                if self.mac_ahead() {
                    let path = self.path(true)?;
                    self.expect(T::Not)?;
                    let (delim, toks) = self.delim_tt()?;
                    let args = self.mac_args(&path, toks);
                    Ty::Mac(MacCall { path, delim, toks, args })
                } else {
                    let path = self.path(false)?;
                    if plus && self.kind() == T::Plus && matches!(self.nth(1), T::Lifetime | T::Ident | T::PathSep | T::Question) {
                        // bare trait object `Trait + Send` (pre-2021)
                        let mut b = vec![Bound::Trait { hrtb: Vec::new(), maybe: false, konst: false, path }];
                        while self.eat(T::Plus) {
                            if !self.at_bound_start() {
                                break;
                            }
                            b.push(self.bound()?);
                        }
                        Ty::DynTrait(b, false)
                    } else {
                        Ty::Path(path)
                    }
                }
            }
            _ => return self.err("expected type"),
        };
        Ok(self.mk_ty(t))
    }

    fn fn_ptr_ty(&mut self, hrtb: Vec<GenericParam>) -> PResult<Ty> {
        let unsafe_ = self.eat(T::KwUnsafe);
        let abi = if self.eat(T::KwExtern) { self.abi() } else { None };
        self.expect(T::KwFn)?;
        self.expect(T::OpenParen)?;
        let mut params = Vec::new();
        let mut variadic = false;
        while self.kind() != T::CloseParen {
            let _ = self.outer_attrs()?;
            if self.eat(T::DotDotDot) {
                variadic = true;
                break;
            }
            // optional name: `x: T` or `_: T`
            if matches!(self.kind(), T::Ident | T::RawIdent) && self.nth(1) == T::Colon {
                self.pos += 2;
            }
            params.push(self.ty_plus()?);
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseParen)?;
        let ret = if self.eat(T::RArrow) { Some(self.ty()?) } else { None };
        Ok(Ty::Fn(Box::new(FnPtr { hrtb, unsafe_, abi, params, ret, variadic })))
    }

    // ------------------------------------------------------------ patterns

    pub fn pat_top(&mut self) -> PResult<PatId> {
        self.eat_or();
        let first = self.pat_no_or()?;
        if self.kind() != T::Or {
            return Ok(first);
        }
        let mut v = vec![first];
        while self.kind() == T::Or {
            self.pos += 1;
            v.push(self.pat_no_or()?);
        }
        Ok(self.mk_pat(Pat::Or(v)))
    }

    fn pat_range_end_start(&self) -> bool {
        matches!(
            self.kind(),
            T::Int | T::Float | T::Char | T::Byte | T::Minus | T::Ident | T::RawIdent | T::PathSep | T::KwSelfType | T::Lt | T::KwCrate | T::KwSuper | T::KwSelfValue
        )
    }

    fn pat_range_end(&mut self) -> PResult<ExprId> {
        let lo = self.lo();
        if self.eat(T::Minus) {
            let e = self.lit_expr()?;
            return Ok(self.mk_expr(ExprKind::Unary(UnOp::Neg, e), lo));
        }
        if matches!(self.kind(), T::Int | T::Float | T::Char | T::Byte) {
            return self.lit_expr();
        }
        let p = self.path(true)?;
        Ok(self.mk_expr(ExprKind::Path(p), lo))
    }

    fn maybe_range_pat(&mut self, start: ExprId) -> PResult<Option<Pat>> {
        match self.kind() {
            T::DotDotEq | T::DotDotDot => {
                self.pos += 1;
                let end = self.pat_range_end()?;
                Ok(Some(Pat::Range(Some(start), Some(end), true)))
            }
            T::DotDot => {
                self.pos += 1;
                if self.pat_range_end_start() {
                    let end = self.pat_range_end()?;
                    Ok(Some(Pat::Range(Some(start), Some(end), false)))
                } else {
                    Ok(Some(Pat::Range(Some(start), None, false)))
                }
            }
            _ => Ok(None),
        }
    }

    pub fn pat_no_or(&mut self) -> PResult<PatId> {
        let p = match self.kind() {
            T::Ident if self.is_word(0, b"_") => {
                self.pos += 1;
                Pat::Wild
            }
            T::DotDot => {
                self.pos += 1;
                Pat::Rest
            }
            T::DotDotEq => {
                self.pos += 1;
                let end = self.pat_range_end()?;
                Pat::Range(None, Some(end), true)
            }
            T::And | T::AndAnd => {
                self.eat_amp();
                let m = self.eat(T::KwMut);
                let inner = self.pat_no_or()?;
                Pat::Ref(m, inner)
            }
            T::OpenParen => {
                self.pos += 1;
                let mut v = Vec::new();
                let mut trailing = false;
                while self.kind() != T::CloseParen {
                    v.push(self.pat_top()?);
                    trailing = false;
                    if !self.eat(T::Comma) {
                        break;
                    }
                    trailing = true;
                }
                self.expect(T::CloseParen)?;
                if v.len() == 1 && !trailing {
                    Pat::Paren(v[0])
                } else {
                    Pat::Tuple(v)
                }
            }
            T::OpenBracket => {
                self.pos += 1;
                let mut v = Vec::new();
                while self.kind() != T::CloseBracket {
                    v.push(self.pat_top()?);
                    if !self.eat(T::Comma) {
                        break;
                    }
                }
                self.expect(T::CloseBracket)?;
                Pat::Slice(v)
            }
            T::KwRef | T::KwMut => {
                let by_ref = self.eat(T::KwRef);
                let mutbl = self.eat(T::KwMut);
                let name = self.ident_tok_or_self()?;
                let sub = if self.eat(T::At) { Some(self.pat_no_or()?) } else { None };
                Pat::Ident { by_ref, mutbl, name, sub }
            }
            T::KwBox => {
                self.pos += 1;
                Pat::Box(self.pat_no_or()?)
            }
            T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte | T::KwTrue | T::KwFalse | T::Minus => {
                let lo = self.lo();
                let e = if self.eat(T::Minus) {
                    let l = self.lit_expr()?;
                    self.mk_expr(ExprKind::Unary(UnOp::Neg, l), lo)
                } else {
                    self.lit_expr()?
                };
                match self.maybe_range_pat(e)? {
                    Some(p) => p,
                    None => Pat::Lit(e),
                }
            }
            T::Ident | T::RawIdent | T::PathSep | T::Lt | T::Shl | T::KwSelfType | T::KwSelfValue | T::KwSuper | T::KwCrate => {
                if self.mac_ahead() {
                    let path = self.path(true)?;
                    self.expect(T::Not)?;
                    let (delim, toks) = self.delim_tt()?;
                    let args = self.mac_args(&path, toks);
                    Pat::Mac(MacCall { path, delim, toks, args })
                } else {
                    let lo = self.lo();
                    let simple = matches!(self.kind(), T::Ident | T::RawIdent) && !matches!(self.nth(1), T::PathSep | T::OpenParen | T::OpenBrace);
                    if simple && !matches!(self.nth(1), T::DotDot | T::DotDotEq | T::DotDotDot) {
                        let name = self.ident_tok()?;
                        let sub = if self.eat(T::At) { Some(self.pat_no_or()?) } else { None };
                        Pat::Ident { by_ref: false, mutbl: false, name, sub }
                    } else {
                        let path = self.path(true)?;
                        match self.kind() {
                            T::OpenParen => {
                                self.pos += 1;
                                let mut v = Vec::new();
                                while self.kind() != T::CloseParen {
                                    v.push(self.pat_top()?);
                                    if !self.eat(T::Comma) {
                                        break;
                                    }
                                }
                                self.expect(T::CloseParen)?;
                                Pat::TupleStruct(path, v)
                            }
                            T::OpenBrace => self.struct_pat(path)?,
                            T::DotDot | T::DotDotEq | T::DotDotDot => {
                                let e = self.mk_expr(ExprKind::Path(path), lo);
                                self.maybe_range_pat(e)?.unwrap()
                            }
                            _ => Pat::Path(path),
                        }
                    }
                }
            }
            _ => return self.err("expected pattern"),
        };
        Ok(self.mk_pat(p))
    }

    fn ident_tok_or_self(&mut self) -> PResult<Ident> {
        if self.kind() == T::KwSelfValue {
            let t = self.bump();
            return Ok(Ident { lo: t.lo, hi: t.hi });
        }
        self.ident_tok()
    }

    fn struct_pat(&mut self, path: Path) -> PResult<Pat> {
        self.expect(T::OpenBrace)?;
        let mut fields = Vec::new();
        let mut rest = false;
        while self.kind() != T::CloseBrace {
            let attrs = self.outer_attrs()?;
            if self.eat(T::DotDot) {
                rest = true;
                break;
            }
            if (self.is_ident() || self.kind() == T::Int) && self.nth(1) == T::Colon {
                let t = self.bump();
                self.pos += 1;
                let pat = self.pat_top()?;
                fields.push(FieldPat { attrs, name: Ident { lo: t.lo, hi: t.hi }, pat, shorthand: false });
            } else {
                let is_box = self.eat(T::KwBox);
                let by_ref = self.eat(T::KwRef);
                let mutbl = self.eat(T::KwMut);
                let name = self.ident_tok()?;
                let mut pat = self.mk_pat(Pat::Ident { by_ref, mutbl, name, sub: None });
                if is_box {
                    pat = self.mk_pat(Pat::Box(pat));
                }
                fields.push(FieldPat { attrs, name, pat, shorthand: true });
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseBrace)?;
        Ok(Pat::Struct(path, fields, rest))
    }

    // ------------------------------------------------------------ blocks & statements

    pub fn block(&mut self) -> PResult<BlockId> {
        let lo = self.lo();
        self.expect(T::OpenBrace)?;
        let mut attrs = Vec::new();
        self.inner_attrs(&mut attrs)?;
        let mut stmts = Vec::new();
        while self.kind() != T::CloseBrace {
            if self.kind() == T::Eof {
                return self.err("unexpected end of file in block");
            }
            let s = self.stmt()?;
            stmts.push(s);
        }
        self.pos += 1;
        self.ast.blocks.push(Block { attrs, stmts, lo, hi: self.prev_hi() });
        Ok(BlockId(self.ast.blocks.len() as u32 - 1))
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        if self.eat(T::Semi) {
            return Ok(Stmt::Empty);
        }
        let attrs = self.outer_attrs()?;
        if self.kind() == T::KwLet {
            self.pos += 1;
            let pat = self.pat_top()?;
            let ty = if self.eat(T::Colon) { Some(self.ty()?) } else { None };
            let (init, else_) = if self.eat(T::Eq) {
                let e = self.expr()?;
                let el = if self.eat(T::KwElse) { Some(self.block()?) } else { None };
                (Some(e), el)
            } else {
                (None, None)
            };
            self.expect(T::Semi)?;
            return Ok(Stmt::Let { attrs, pat, ty, init, else_ });
        }
        if self.at_item_start() {
            if let Some(id) = self.item(attrs.clone())? {
                return Ok(Stmt::Item(id));
            }
        }
        if self.kind() == T::CloseBrace && !attrs.is_empty() {
            return self.err("attributes without statement");
        }
        let lo = self.lo();
        // statement-level brace macro: `m! { }` ends the statement
        if self.mac_ahead() {
            let save = self.pos;
            let path = self.path(true)?;
            self.expect(T::Not)?;
            if self.kind() == T::OpenBrace {
                let (delim, toks) = self.delim_tt()?;
                let args = self.mac_args(&path, toks);
                let e = self.mk_expr(ExprKind::Mac(MacCall { path, delim, toks, args }), lo);
                if matches!(self.kind(), T::Dot | T::Question) {
                    let e = self.postfix(e, lo)?;
                    let e = self.expr_continue(e, 0, 0, lo)?;
                    let semi = self.eat(T::Semi);
                    return Ok(Stmt::Expr(e, semi));
                }
                let semi = self.eat(T::Semi);
                return Ok(Stmt::Expr(e, semi));
            }
            self.pos = save;
        }
        if self.block_like_start() {
            let e = self.primary(0)?;
            if matches!(self.kind(), T::Dot | T::Question) {
                let e = self.postfix(e, lo)?;
                let e = self.expr_continue(e, 0, 0, lo)?;
                let semi = self.eat(T::Semi);
                if !semi && self.kind() != T::CloseBrace {
                    return self.err("expected `;`");
                }
                return Ok(Stmt::Expr(e, semi));
            }
            let semi = self.eat(T::Semi);
            return Ok(Stmt::Expr(e, semi));
        }
        let e = self.expr()?;
        if self.eat(T::Semi) {
            return Ok(Stmt::Expr(e, true));
        }
        if self.kind() == T::CloseBrace {
            return Ok(Stmt::Expr(e, false));
        }
        self.err("expected `;` or `}` after expression")
    }

    fn block_like_start(&self) -> bool {
        match self.kind() {
            T::OpenBrace | T::KwIf | T::KwMatch | T::KwLoop | T::KwWhile | T::KwFor => true,
            T::KwUnsafe => self.nth(1) == T::OpenBrace,
            T::KwConst => self.nth(1) == T::OpenBrace,
            T::KwAsync => self.nth(1) == T::OpenBrace || (self.nth(1) == T::KwMove && self.nth(2) == T::OpenBrace),
            T::Lifetime => self.nth(1) == T::Colon,
            _ => false,
        }
    }

    // ------------------------------------------------------------ expressions

    pub fn expr(&mut self) -> PResult<ExprId> {
        self.expr_bp(0, 0)
    }
    fn expr_ns(&mut self) -> PResult<ExprId> {
        self.expr_bp(0, NO_STRUCT)
    }

    fn can_begin_expr(&self, r: u8) -> bool {
        match self.kind() {
            T::OpenBrace => r & NO_STRUCT == 0,
            T::Ident | T::RawIdent | T::Lifetime | T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char
            | T::Byte | T::OpenParen | T::OpenBracket | T::Or | T::OrOr | T::Not | T::Minus | T::Star | T::And
            | T::AndAnd | T::DotDot | T::DotDotEq | T::Lt | T::Shl | T::PathSep | T::Pound | T::KwIf | T::KwMatch
            | T::KwLoop | T::KwWhile | T::KwFor | T::KwUnsafe | T::KwMove | T::KwAsync | T::KwReturn | T::KwBreak
            | T::KwContinue | T::KwLet | T::KwConst | T::KwSelfValue | T::KwSelfType | T::KwSuper | T::KwCrate
            | T::KwTrue | T::KwFalse | T::KwBox | T::KwYield | T::KwStatic => true,
            _ => false,
        }
    }

    fn expr_bp(&mut self, min: u8, r: u8) -> PResult<ExprId> {
        let lo = self.lo();
        // attributes on expressions
        if self.kind() == T::Pound {
            let _ = self.outer_attrs()?;
        }
        // prefix range
        let lhs = if matches!(self.kind(), T::DotDot | T::DotDotEq) {
            let incl = self.bump().kind == T::DotDotEq;
            let rhs = if self.can_begin_expr(r) { Some(self.expr_bp(P_RANGE + 1, r)?) } else { None };
            self.mk_expr(ExprKind::Range(None, rhs, incl), lo)
        } else {
            self.unary(r)?
        };
        self.expr_continue(lhs, min, r, lo)
    }

    fn expr_continue(&mut self, mut lhs: ExprId, min: u8, r: u8, lo: u32) -> PResult<ExprId> {
        loop {
            let k = self.kind();
            let (prec, op) = match k {
                T::Eq => (P_ASSIGN, None),
                T::PlusEq => (P_ASSIGN, Some(BinOp::Add)),
                T::MinusEq => (P_ASSIGN, Some(BinOp::Sub)),
                T::StarEq => (P_ASSIGN, Some(BinOp::Mul)),
                T::SlashEq => (P_ASSIGN, Some(BinOp::Div)),
                T::PercentEq => (P_ASSIGN, Some(BinOp::Rem)),
                T::CaretEq => (P_ASSIGN, Some(BinOp::BitXor)),
                T::AndEq => (P_ASSIGN, Some(BinOp::BitAnd)),
                T::OrEq => (P_ASSIGN, Some(BinOp::BitOr)),
                T::ShlEq => (P_ASSIGN, Some(BinOp::Shl)),
                T::ShrEq => (P_ASSIGN, Some(BinOp::Shr)),
                T::DotDot | T::DotDotEq => (P_RANGE, None),
                T::OrOr => (P_OROR, Some(BinOp::Or)),
                T::AndAnd => (P_ANDAND, Some(BinOp::And)),
                T::EqEq => (P_CMP, Some(BinOp::Eq)),
                T::Ne => (P_CMP, Some(BinOp::Ne)),
                T::Lt => (P_CMP, Some(BinOp::Lt)),
                T::Le => (P_CMP, Some(BinOp::Le)),
                T::Gt => (P_CMP, Some(BinOp::Gt)),
                T::Ge => (P_CMP, Some(BinOp::Ge)),
                T::Or => (P_BOR, Some(BinOp::BitOr)),
                T::Caret => (P_BXOR, Some(BinOp::BitXor)),
                T::And => (P_BAND, Some(BinOp::BitAnd)),
                T::Shl => (P_SHIFT, Some(BinOp::Shl)),
                T::Shr => (P_SHIFT, Some(BinOp::Shr)),
                T::Plus => (P_ADD, Some(BinOp::Add)),
                T::Minus => (P_ADD, Some(BinOp::Sub)),
                T::Star => (P_MUL, Some(BinOp::Mul)),
                T::Slash => (P_MUL, Some(BinOp::Div)),
                T::Percent => (P_MUL, Some(BinOp::Rem)),
                T::KwAs => (P_AS, None),
                _ => break,
            };
            if prec < min {
                break;
            }
            self.pos += 1;
            if prec == P_ASSIGN {
                let rhs = self.expr_bp(P_ASSIGN, r)?;
                lhs = match op {
                    None => self.mk_expr(ExprKind::Assign(lhs, rhs), lo),
                    Some(o) => self.mk_expr(ExprKind::AssignOp(o, lhs, rhs), lo),
                };
            } else if prec == P_RANGE {
                let incl = k == T::DotDotEq;
                let rhs = if self.can_begin_expr(r) { Some(self.expr_bp(P_RANGE + 1, r)?) } else { None };
                lhs = self.mk_expr(ExprKind::Range(Some(lhs), rhs, incl), lo);
            } else if prec == P_AS {
                let ty = self.ty()?;
                lhs = self.mk_expr(ExprKind::Cast(lhs, ty), lo);
            } else {
                let rhs = self.expr_bp(prec + 1, r)?;
                lhs = self.mk_expr(ExprKind::Binary(op.unwrap(), lhs, rhs), lo);
            }
        }
        Ok(lhs)
    }

    fn unary(&mut self, r: u8) -> PResult<ExprId> {
        let lo = self.lo();
        match self.kind() {
            T::Minus => {
                self.pos += 1;
                let e = self.unary(r)?;
                Ok(self.mk_expr(ExprKind::Unary(UnOp::Neg, e), lo))
            }
            T::Not => {
                self.pos += 1;
                let e = self.unary(r)?;
                Ok(self.mk_expr(ExprKind::Unary(UnOp::Not, e), lo))
            }
            T::Star => {
                self.pos += 1;
                let e = self.unary(r)?;
                Ok(self.mk_expr(ExprKind::Unary(UnOp::Deref, e), lo))
            }
            T::And | T::AndAnd => {
                self.eat_amp();
                let mut raw = false;
                let m;
                if self.is_word(0, b"raw") && matches!(self.nth(1), T::KwConst | T::KwMut) {
                    self.pos += 1;
                    raw = true;
                    m = self.bump().kind == T::KwMut;
                } else {
                    m = self.eat(T::KwMut);
                }
                let e = self.unary(r)?;
                Ok(self.mk_expr(ExprKind::AddrOf(raw, m, e), lo))
            }
            T::KwBox => {
                self.pos += 1;
                let e = self.unary(r)?;
                Ok(self.mk_expr(ExprKind::Box(e), lo))
            }
            _ => {
                let e = self.primary(r)?;
                self.postfix(e, lo)
            }
        }
    }

    fn postfix(&mut self, mut e: ExprId, lo: u32) -> PResult<ExprId> {
        loop {
            match self.kind() {
                T::Question => {
                    self.pos += 1;
                    e = self.mk_expr(ExprKind::Try(e), lo);
                }
                T::Dot => {
                    self.pos += 1;
                    match self.kind() {
                        T::KwAwait => {
                            self.pos += 1;
                            e = self.mk_expr(ExprKind::Await(e), lo);
                        }
                        T::Ident | T::RawIdent => {
                            let name = self.ident_tok()?;
                            let turbofish = if self.kind() == T::PathSep && matches!(self.nth(1), T::Lt | T::Shl) {
                                self.pos += 1;
                                Some(Box::new(self.generic_args_angle()?))
                            } else {
                                None
                            };
                            if self.kind() == T::OpenParen {
                                let args = self.call_args()?;
                                e = self.mk_expr(ExprKind::MethodCall { recv: e, name, turbofish, args }, lo);
                            } else {
                                e = self.mk_expr(ExprKind::Field(e, name), lo);
                            }
                        }
                        T::Int => {
                            let t = self.bump();
                            let n = parse_index(self.text_of(t));
                            e = self.mk_expr(ExprKind::TupleField(e, n), lo);
                        }
                        T::Float => {
                            // `x.0.1`
                            let t = self.bump();
                            let s = self.text_of(t);
                            let mut dot = s.len();
                            for i in 0..s.len() {
                                if s[i] == b'.' {
                                    dot = i;
                                    break;
                                }
                            }
                            let a = parse_index(&s[..dot]);
                            e = self.mk_expr(ExprKind::TupleField(e, a), lo);
                            if dot + 1 < s.len() {
                                let b = parse_index(&s[dot + 1..]);
                                e = self.mk_expr(ExprKind::TupleField(e, b), lo);
                            }
                        }
                        _ => return self.err("expected field or method"),
                    }
                }
                T::OpenParen => {
                    let args = self.call_args()?;
                    e = self.mk_expr(ExprKind::Call(e, args), lo);
                }
                T::OpenBracket => {
                    self.pos += 1;
                    let i = self.expr()?;
                    self.expect(T::CloseBracket)?;
                    e = self.mk_expr(ExprKind::Index(e, i), lo);
                }
                _ => break,
            }
        }
        Ok(e)
    }

    fn call_args(&mut self) -> PResult<Vec<ExprId>> {
        self.expect(T::OpenParen)?;
        let mut v = Vec::new();
        while self.kind() != T::CloseParen {
            v.push(self.expr()?);
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseParen)?;
        Ok(v)
    }

    fn label(&mut self) -> Option<Ident> {
        if self.kind() == T::Lifetime && self.nth(1) == T::Colon {
            let t = self.bump();
            self.pos += 1;
            Some(Ident { lo: t.lo, hi: t.hi })
        } else {
            None
        }
    }

    fn primary(&mut self, r: u8) -> PResult<ExprId> {
        let lo = self.lo();
        match self.kind() {
            T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte | T::KwTrue | T::KwFalse => {
                self.lit_expr()
            }
            T::OpenParen => {
                self.pos += 1;
                if self.eat(T::CloseParen) {
                    return Ok(self.mk_expr(ExprKind::Tuple(Vec::new()), lo));
                }
                let first = self.expr()?;
                if self.eat(T::CloseParen) {
                    return Ok(self.mk_expr(ExprKind::Paren(first), lo));
                }
                let mut v = vec![first];
                while self.eat(T::Comma) {
                    if self.kind() == T::CloseParen {
                        break;
                    }
                    v.push(self.expr()?);
                }
                self.expect(T::CloseParen)?;
                Ok(self.mk_expr(ExprKind::Tuple(v), lo))
            }
            T::OpenBracket => {
                self.pos += 1;
                if self.eat(T::CloseBracket) {
                    return Ok(self.mk_expr(ExprKind::Array(Vec::new()), lo));
                }
                let first = self.expr()?;
                if self.eat(T::Semi) {
                    let n = self.expr()?;
                    self.expect(T::CloseBracket)?;
                    return Ok(self.mk_expr(ExprKind::Repeat(first, n), lo));
                }
                let mut v = vec![first];
                while self.eat(T::Comma) {
                    if self.kind() == T::CloseBracket {
                        break;
                    }
                    v.push(self.expr()?);
                }
                self.expect(T::CloseBracket)?;
                Ok(self.mk_expr(ExprKind::Array(v), lo))
            }
            T::OpenBrace => {
                let b = self.block()?;
                Ok(self.mk_expr(ExprKind::Block(b, None), lo))
            }
            T::KwUnsafe => {
                self.pos += 1;
                let b = self.block()?;
                Ok(self.mk_expr(ExprKind::Unsafe(b), lo))
            }
            T::KwConst if self.nth(1) == T::OpenBrace => {
                self.pos += 1;
                let b = self.block()?;
                Ok(self.mk_expr(ExprKind::ConstBlock(b), lo))
            }
            T::Lifetime => {
                let label = self.label();
                if label.is_none() {
                    return self.err("unexpected lifetime");
                }
                match self.kind() {
                    T::KwLoop => self.loop_expr(label, lo),
                    T::KwWhile => self.while_expr(label, lo),
                    T::KwFor => self.for_expr(label, lo),
                    T::OpenBrace => {
                        let b = self.block()?;
                        Ok(self.mk_expr(ExprKind::Block(b, label), lo))
                    }
                    _ => self.err("expected loop or block after label"),
                }
            }
            T::KwLoop => self.loop_expr(None, lo),
            T::KwWhile => self.while_expr(None, lo),
            T::KwFor => self.for_expr(None, lo),
            T::KwIf => self.if_expr(lo),
            T::KwMatch => {
                self.pos += 1;
                let scrut = self.expr_ns()?;
                self.expect(T::OpenBrace)?;
                let mut _inner = Vec::new();
                self.inner_attrs(&mut _inner)?;
                let mut arms = Vec::new();
                while self.kind() != T::CloseBrace {
                    let attrs = self.outer_attrs()?;
                    let pat = self.pat_top()?;
                    let guard = if self.eat(T::KwIf) { Some(self.expr()?) } else { None };
                    self.expect(T::FatArrow)?;
                    let blo = self.lo();
                    let body;
                    if self.block_like_start() {
                        let e = self.primary(0)?;
                        if matches!(self.kind(), T::Dot | T::Question) {
                            let e = self.postfix(e, blo)?;
                            body = self.expr_continue(e, 0, 0, blo)?;
                            if !self.eat(T::Comma) && self.kind() != T::CloseBrace {
                                return self.err("expected `,` after match arm");
                            }
                        } else {
                            body = e;
                            self.eat(T::Comma);
                        }
                    } else {
                        body = self.expr()?;
                        if !self.eat(T::Comma) && self.kind() != T::CloseBrace {
                            return self.err("expected `,` after match arm");
                        }
                    }
                    arms.push(Arm { attrs, pat, guard, body });
                }
                self.pos += 1;
                Ok(self.mk_expr(ExprKind::Match(scrut, arms), lo))
            }
            T::KwLet => {
                self.pos += 1;
                let pat = self.pat_top()?;
                self.expect(T::Eq)?;
                let e = self.expr_bp(P_CMP, r | NO_STRUCT & r)?;
                Ok(self.mk_expr(ExprKind::Let(pat, e), lo))
            }
            T::KwReturn => {
                self.pos += 1;
                let e = if self.can_begin_expr(r) { Some(self.expr_bp(0, r)?) } else { None };
                Ok(self.mk_expr(ExprKind::Return(e), lo))
            }
            T::KwYield => {
                self.pos += 1;
                let e = if self.can_begin_expr(r) { Some(self.expr_bp(0, r)?) } else { None };
                Ok(self.mk_expr(ExprKind::Yield(e), lo))
            }
            T::KwBreak => {
                self.pos += 1;
                let label = if self.kind() == T::Lifetime {
                    let t = self.bump();
                    Some(Ident { lo: t.lo, hi: t.hi })
                } else {
                    None
                };
                let e = if self.can_begin_expr(r) && !(self.kind() == T::Lifetime) { Some(self.expr_bp(0, r)?) } else { None };
                Ok(self.mk_expr(ExprKind::Break(label, e), lo))
            }
            T::KwContinue => {
                self.pos += 1;
                let label = if self.kind() == T::Lifetime {
                    let t = self.bump();
                    Some(Ident { lo: t.lo, hi: t.hi })
                } else {
                    None
                };
                Ok(self.mk_expr(ExprKind::Continue(label), lo))
            }
            T::Or | T::OrOr | T::KwMove => self.closure(false, r, lo),
            T::KwStatic if matches!(self.nth(1), T::Or | T::OrOr | T::KwMove) => {
                self.pos += 1;
                self.closure(false, r, lo)
            }
            T::KwAsync => {
                self.pos += 1;
                if self.kind() == T::OpenBrace {
                    let b = self.block()?;
                    return Ok(self.mk_expr(ExprKind::Async(false, b), lo));
                }
                if self.kind() == T::KwMove && self.nth(1) == T::OpenBrace {
                    self.pos += 1;
                    let b = self.block()?;
                    return Ok(self.mk_expr(ExprKind::Async(true, b), lo));
                }
                self.closure(true, r, lo)
            }
            T::Ident if self.is_word(0, b"_") => {
                self.pos += 1;
                Ok(self.mk_expr(ExprKind::Underscore, lo))
            }
            T::Ident | T::RawIdent | T::PathSep | T::Lt | T::Shl | T::KwSelfValue | T::KwSelfType | T::KwSuper | T::KwCrate => {
                let path = self.path(true)?;
                if self.kind() == T::Not && matches!(self.nth(1), T::OpenParen | T::OpenBracket | T::OpenBrace) {
                    self.pos += 1;
                    let (delim, toks) = self.delim_tt()?;
                    let args = self.mac_args(&path, toks);
                    return Ok(self.mk_expr(ExprKind::Mac(MacCall { path, delim, toks, args }), lo));
                }
                if self.kind() == T::OpenBrace && r & NO_STRUCT == 0 {
                    return self.struct_expr(path, lo);
                }
                Ok(self.mk_expr(ExprKind::Path(path), lo))
            }
            _ => self.err("expected expression"),
        }
    }

    fn struct_expr(&mut self, path: Path, lo: u32) -> PResult<ExprId> {
        self.expect(T::OpenBrace)?;
        let mut fields = Vec::new();
        let mut base = None;
        while self.kind() != T::CloseBrace {
            let attrs = self.outer_attrs()?;
            if self.eat(T::DotDot) {
                if self.kind() != T::CloseBrace {
                    base = Some(self.expr()?);
                }
                break;
            }
            let t = self.tok();
            if !matches!(t.kind, T::Ident | T::RawIdent | T::Int) {
                return self.err("expected field name");
            }
            self.pos += 1;
            let name = Ident { lo: t.lo, hi: t.hi };
            if self.eat(T::Colon) {
                let e = self.expr()?;
                fields.push(FieldExpr { attrs, name, expr: e, shorthand: false });
            } else {
                let p = Path { global: false, qself: None, segs: vec![PathSeg { kind: SegKind::Ident, name, args: None }], lo: t.lo };
                let e = self.mk_expr(ExprKind::Path(p), t.lo);
                fields.push(FieldExpr { attrs, name, expr: e, shorthand: true });
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseBrace)?;
        Ok(self.mk_expr(ExprKind::Struct(path, fields, base), lo))
    }

    fn closure(&mut self, is_async: bool, r: u8, lo: u32) -> PResult<ExprId> {
        let is_move = self.eat(T::KwMove);
        let mut params = Vec::new();
        if !self.eat(T::OrOr) {
            if !self.eat_or() {
                return self.err("expected closure");
            }
            while self.kind() != T::Or {
                let attrs = self.outer_attrs()?;
                let pat = self.pat_no_or()?;
                let ty = if self.eat(T::Colon) { Some(self.ty()?) } else { None };
                params.push(ClosureParam { attrs, pat, ty });
                if !self.eat(T::Comma) {
                    break;
                }
            }
            if !self.eat_or() {
                return self.err("expected `|`");
            }
        }
        let (ret, body) = if self.eat(T::RArrow) {
            let ty = self.ty()?;
            let blo = self.lo();
            let b = self.block()?;
            (Some(ty), self.mk_expr(ExprKind::Block(b, None), blo))
        } else {
            (None, self.expr_bp(0, r)?)
        };
        Ok(self.mk_expr(ExprKind::Closure { is_move, is_async, params, ret, body }, lo))
    }

    fn if_expr(&mut self, lo: u32) -> PResult<ExprId> {
        self.expect(T::KwIf)?;
        let cond = self.expr_ns()?;
        let then = self.block()?;
        let els = if self.eat(T::KwElse) {
            let elo = self.lo();
            if self.kind() == T::KwIf {
                Some(self.if_expr(elo)?)
            } else {
                let b = self.block()?;
                Some(self.mk_expr(ExprKind::Block(b, None), elo))
            }
        } else {
            None
        };
        Ok(self.mk_expr(ExprKind::If(cond, then, els), lo))
    }

    fn loop_expr(&mut self, label: Option<Ident>, lo: u32) -> PResult<ExprId> {
        self.expect(T::KwLoop)?;
        let b = self.block()?;
        Ok(self.mk_expr(ExprKind::Loop(b, label), lo))
    }
    fn while_expr(&mut self, label: Option<Ident>, lo: u32) -> PResult<ExprId> {
        self.expect(T::KwWhile)?;
        let c = self.expr_ns()?;
        let b = self.block()?;
        Ok(self.mk_expr(ExprKind::While(c, b, label), lo))
    }
    fn for_expr(&mut self, label: Option<Ident>, lo: u32) -> PResult<ExprId> {
        self.expect(T::KwFor)?;
        let p = self.pat_top()?;
        self.expect(T::KwIn)?;
        let it = self.expr_ns()?;
        let b = self.block()?;
        Ok(self.mk_expr(ExprKind::For(p, it, b, label), lo))
    }
}

#[inline]
fn is_str(k: T) -> bool {
    matches!(k, T::Str | T::RawStr)
}

fn parse_index(s: &[u8]) -> u32 {
    let mut n = 0u32;
    for &c in s {
        if c.is_ascii_digit() {
            n = n * 10 + (c - b'0') as u32;
        } else if c != b'_' {
            break;
        }
    }
    n
}

/// Lex + parse one file.
pub fn parse_source(src: &[u8], edition: u16) -> Result<Parser<'_>, (u32, String)> {
    parse_source_mode(src, edition, false)
}

pub fn parse_source_mode(src: &[u8], edition: u16, lazy: bool) -> Result<Parser<'_>, (u32, String)> {
    let mut toks = Vec::with_capacity(src.len() / 4);
    crate::lexer::lex(src, edition, &mut toks).map_err(|e| (e.pos, e.msg.to_string()))?;
    let mut p = Parser::new(src, toks, edition);
    p.lazy_bodies = lazy;
    p.parse_file().map_err(|e| (e.pos, e.msg))?;
    Ok(p)
}

/// A parsed file that owns its source, tokens and tree.
pub struct ParsedFile {
    pub src: Vec<u8>,
    pub toks: Vec<Tok>,
    pub ast: Ast,
    pub edition: u16,
}

impl ParsedFile {
    #[inline]
    pub fn text(&self, id: Ident) -> &str {
        std::str::from_utf8(&self.src[id.lo as usize..id.hi as usize]).unwrap_or("?")
    }
    pub fn tok_text(&self, i: u32) -> &str {
        let t = self.toks[i as usize];
        std::str::from_utf8(&self.src[t.lo as usize..t.hi as usize]).unwrap_or("?")
    }
}

pub fn parse_owned(src: Vec<u8>, edition: u16) -> Result<ParsedFile, (u32, String)> {
    let (toks, ast) = {
        let p = parse_source(&src, edition)?;
        (p.toks, p.ast)
    };
    Ok(ParsedFile { src, toks, ast, edition })
}
