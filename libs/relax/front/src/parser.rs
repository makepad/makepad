//! Recursive-descent parser (Pratt for expressions) over the token list of one
//! file. Builds an `Ast`. Errors stop the file: the base stack must parse with
//! zero errors, so there is no recovery.

use crate::ast::*;
use crate::lexer::{T, Tok};
use crate::source::{SourceSpan, TokenOrigin, SourceRegistry};
use crate::origin_table::{Table, OSeg, OSegRef, Regions, Maps, Origins};

#[path = "fmtmac.rs"]
mod fmtmac;
#[path = "derive.rs"]
pub mod derive;
#[path = "lazy_bodies.rs"]
pub mod lazy_bodies;
#[path = "rs_expand.rs"]
pub(crate) mod rs_expand;

pub struct PErr {
    pub pos: u32,
    pub msg: String,
}

pub type PResult<X> = Result<X, PErr>;

const NO_STRUCT: u8 = 1;

#[derive(Clone, Copy)]
enum BodyOutline {
    Deferred(TokRange),
    Eager(u8),
    Mismatched(u32),
}

pub struct Parser<'a> {
    pub session: &'a std::sync::Arc<crate::Session>,
    pub src: &'a [u8],
    pub toks: lazy_bodies::Toks<'a>,
    pub pos: usize,
    pub edition: u16,
    pub ast: Ast,
    item_start: Vec<[u32; 2]>,
    /// skip fn bodies (brace match over tokens); parse them on first compile
    pub lazy_bodies: bool,
    /// skip the fn bodies a later parse can do alone (body_outline): ParsedFile::part parses
    /// them on the first need
    pub lazy_auto: bool,
    /// parsing a file's skipped bodies (ParsedFile::part): macros resolve to built-ins only
    lazy_part: bool,
    /// inline `mod` stacks of skipped bodies inside inline modules: (body's first token, names)
    lazy_mods: Vec<(u32, Vec<String>)>,
    /// `macro_rules!` definitions visible to this file (None: no expansion, e.g. discovery)
    pub macros: Option<crate::macros::MacroScope>,
    /// this file's crate name (`$crate`)
    pub krate: String,
    /// text of macro expansions, addressed by positions >= src.len()
    pub ext: lazy_bodies::Seg<'a, u8>,
    /// expansion text ranges -> call-site position: (lo, hi, call lo)
    pub exp_map: OSeg<'a, Regions>,
    /// macro_rules expansion tokens substituted from a fragment -> the fragment token's own
    /// position (rustc: substituted tokens keep their spans), ascending by expansion position:
    /// (expansion position, origin position, length)
    pub tok_origin: OSeg<'a, Maps>,
    /// Complete token provenance, in spelling-buffer order (independent of Rust grammar).
    pub token_origins: OSeg<'a, Origins>,
    pub token_input: bool,
    pub sources: Option<std::sync::Arc<SourceRegistry>>,
    exp_depth: u32,
    /// macro_rules expansions: (text lo, text hi, the macro's definition position in this
    /// file; 0 when defined elsewhere) -- the hygiene marks (ParsedFile::hyg_ctxt)
    pub rules_exp: OSeg<'a, Regions>,
    /// macro_rules body tokens -> their position in the definition: (expansion position,
    /// definition position, length), ascending
    pub body_origin: OSeg<'a, Maps>,
    /// tokens split by `eat_gt` (`>>` -> `>`), with their old value
    pub tok_undo: Vec<(u32, Tok)>,
    /// file context for built-in macros (file!, module_path!, env!, cfg of derive inputs)
    pub bctx: Option<std::rc::Rc<crate::builtins::FileCtx>>,
    /// names of the inline `mod x { }` blocks being parsed (module_path!)
    pub mod_stack: Vec<String>,
    /// token index -> expression of a `T::Interp` token
    interp: crate::fxhash::HashMap<u32, ExprId>,
    /// items generated after the last parsed item (derive expansions), for its container
    extra_items: Vec<ItemId>,
    /// Anonymous lengths using Self keep the enclosing associated-item owner.
    extra_assoc_items: Vec<ItemId>,
    /// inner attributes of an impl/trait body, for the item being finished
    pending_inner: Vec<Attr>,
    /// parsing the items of an impl / trait / extern block: generated items (anonymous consts)
    /// belong to the enclosing module, not to the impl
    in_assoc: u32,
    foreign_abi: Option<Option<Ident>>,
    /// inside a cfg'd-out item (parse_cfg_out): nothing is generated
    cfg_out: u32,
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
    pub fn new(sess: &'a std::sync::Arc<crate::Session>, src: &'a [u8], toks: impl Into<lazy_bodies::Toks<'a>>, edition: u16) -> Self {
        Parser {
            session: sess,
            src,
            toks: toks.into(),
            pos: 0,
            edition,
            ast: Ast::default(),
            item_start: Vec::new(),
            lazy_bodies: false,
            lazy_auto: false,
            lazy_part: false,
            lazy_mods: Vec::new(),
            macros: None,
            krate: String::new(),
            ext: Default::default(),
            exp_map: Default::default(),
            tok_origin: Default::default(),
            token_origins: Default::default(),
            token_input: false,
            sources: None,
            exp_depth: 0,
            rules_exp: Default::default(),
            body_origin: Default::default(),
            tok_undo: Vec::new(),
            bctx: None,
            mod_stack: Vec::new(),
            interp: Default::default(),
            extra_items: Vec::new(),
            extra_assoc_items: Vec::new(),
            pending_inner: Vec::new(),
            in_assoc: 0,
            foreign_abi: None,
            cfg_out: 0,
        }
    }

    // ------------------------------------------------------------ token helpers

    #[inline]
    fn kind(&self) -> T {
        self.toks.at(self.pos).kind
    }
    #[inline]
    fn nth(&self, n: usize) -> T {
        let i = self.pos + n;
        if i < self.toks.len() {
            self.toks.at(i).kind
        } else {
            T::Eof
        }
    }
    #[inline]
    fn tok(&self) -> Tok {
        self.toks.at(self.pos)
    }
    #[inline]
    fn lo(&self) -> u32 {
        self.toks.at(self.pos).lo
    }
    #[inline]
    fn prev_hi(&self) -> u32 {
        if self.pos == 0 {
            0
        } else {
            self.toks.at(self.pos - 1).hi
        }
    }
    #[inline]
    fn bump(&mut self) -> Tok {
        let t = self.toks.at(self.pos);
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
        let text = String::from_utf8_lossy(self.tok_text(t)).into_owned();
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
    fn text_of(&self, t: Tok) -> &[u8] {
        self.tok_text(t)
    }
    /// Text of a token: the file's source, or an expansion's text past its end.
    #[inline]
    pub fn tok_text(&self, t: Tok) -> &[u8] {
        self.bytes(t.lo, t.hi)
    }
    #[inline]
    pub fn bytes(&self, lo: u32, hi: u32) -> &[u8] {
        let base = self.src.len() as u32;
        if lo >= base {
            &self.ext[(lo - base) as usize..(hi - base) as usize]
        } else {
            &self.src[lo as usize..hi as usize]
        }
    }
    #[inline]
    fn is_word(&self, n: usize, w: &[u8]) -> bool {
        let i = self.pos + n;
        i < self.toks.len() && self.toks.at(i).kind == T::Ident && self.text_of(self.toks.at(i)) == w
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
        if matches!(self.kind(), T::Shr | T::Ge | T::ShrEq) {
            // a speculative parse (macro fragment matching) restores the split tokens
            self.tok_undo.push((self.pos as u32, self.toks.at(self.pos)));
        }
        match self.kind() {
            T::Gt => {
                self.pos += 1;
                true
            }
            T::Shr => {
                self.toks.edit(self.pos, |t| { t.kind = T::Gt; t.lo += 1; });
                true
            }
            T::Ge => {
                self.toks.edit(self.pos, |t| { t.kind = T::Eq; t.lo += 1; });
                true
            }
            T::ShrEq => {
                self.toks.edit(self.pos, |t| { t.kind = T::Ge; t.lo += 1; });
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
                self.toks.edit(self.pos, |t| { t.kind = T::Lt; t.lo += 1; });
                true
            }
            T::Le => {
                self.toks.edit(self.pos, |t| { t.kind = T::Eq; t.lo += 1; });
                true
            }
            T::ShlEq => {
                self.toks.edit(self.pos, |t| { t.kind = T::Le; t.lo += 1; });
                true
            }
            // rustc_parse break_and_eat: `<-` splits into `<` `-` (`f::<-1>()`)
            T::LArrow => {
                self.toks.edit(self.pos, |t| { t.kind = T::Minus; t.lo += 1; });
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
                self.toks.edit(self.pos, |t| { t.kind = T::And; t.lo += 1; });
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
                self.toks.edit(self.pos, |t| { t.kind = T::Or; t.lo += 1; });
                true
            }
            _ => false,
        }
    }

    // ------------------------------------------------------------ arenas

    fn mk_expr(&mut self, kind: ExprKind, lo: u32) -> ExprId {
        let hi = self.prev_hi();
        self.ast.exprs.push(Expr { kind, lo, hi });
        ExprId(self.ast.base[0] + self.ast.exprs.len() as u32 - 1)
    }
    fn mk_pat(&mut self, p: Pat) -> PatId {
        self.ast.pats.push(p);
        PatId(self.ast.base[1] + self.ast.pats.len() as u32 - 1)
    }
    fn mk_ty(&mut self, t: Ty) -> TyId {
        self.ast.tys.push(t);
        TyId(self.ast.base[2] + self.ast.tys.len() as u32 - 1)
    }

    // ------------------------------------------------------------ token trees

    /// A fn body at `pos` (`{`) that a later parse can do alone (ParsedFile::part),
    /// holding no item (nothing a lazy parse adds to the
    /// item list: item keywords, item-level macros, array lengths that become constant items)
    /// and calling only built-in fn-like macros that this scope does not shadow with a
    /// `macro_rules!` (the lazy parse resolves macros by built-in name; it has no textual scope).
    /// Scans tokens only.
    /// Return the validated inner token range, or why it must stay eager. Reuse
    /// this outline to advance the declaration parser: do not scan a deferred
    /// body a second time just to find its closing brace.
    fn body_outline(&self) -> BodyOutline {
        let profile = self.session.runtime.profile;
        let _profile = crate::load_profile::span(profile, "parse-body-outline", self.bctx.as_ref().map_or("", |c| c.path.as_str()));
        if self.macros.is_none() {
            return BodyOutline::Eager(7);
        }
        // a body in macro output (derives): fine unless the lazy parse is off for it
        if self.toks.at(self.pos).lo as usize >= self.src.len() && !self.session.options.lazy_macro_output {
            return BodyOutline::Eager(1);
        }
        let module = std::cell::OnceCell::new();
        let mut stack = vec![T::OpenBrace];
        let mut mismatched = None;
        let mut i = self.pos + 1;
        while i < self.toks.len() {
            let k = self.toks.at(i).kind;
            match k {
                T::OpenParen | T::OpenBracket | T::OpenBrace => stack.push(k),
                T::CloseParen | T::CloseBracket | T::CloseBrace => {
                    if !matches!((stack.pop(), k),
                        (Some(T::OpenParen), T::CloseParen) | (Some(T::OpenBracket), T::CloseBracket) | (Some(T::OpenBrace), T::CloseBrace)) {
                        mismatched.get_or_insert(self.toks.at(i).lo);
                    }
                    if stack.is_empty() {
                        // Eligibility is decided before reporting delimiter errors,
                        // just as when delim_tt scanned a second time after it.
                        return match mismatched {
                            Some(pos) => BodyOutline::Mismatched(pos),
                            None => BodyOutline::Deferred(TokRange { lo: self.pos as u32 + 1, hi: i as u32 }),
                        };
                    }
                }
                T::KwFn | T::KwImpl | T::KwStruct | T::KwEnum | T::KwTrait | T::KwMod | T::KwUse | T::KwStatic | T::KwExtern | T::KwType | T::Interp | T::Pound if k != T::Pound || self.toks.get(i + 1).map(|t| t.kind) == Some(T::Not) => return BodyOutline::Eager(if k == T::Interp { 5 } else { 2 }),
                T::KwConst if i + 2 < self.toks.len() && matches!(self.toks.at(i + 1).kind, T::Ident | T::RawIdent) && self.toks.at(i + 2).kind == T::Colon => return BodyOutline::Eager(2),
                T::Not if i > 0 && matches!(self.toks.at(i - 1).kind, T::Ident | T::RawIdent) && i + 1 < self.toks.len() && matches!(self.toks.at(i + 1).kind, T::OpenParen | T::OpenBracket | T::OpenBrace) => {
                    let t = self.toks.at(i - 1);
                    let name = std::str::from_utf8(self.bytes(t.lo, t.hi)).unwrap_or("").trim_start_matches("r#");
                    // `std::x!` / `core::x!` / `alloc::x!` or a plain name
                    if i >= 2 && self.toks.at(i - 2).kind == T::PathSep {
                        let q = if i >= 3 { self.toks.at(i - 3) } else { return BodyOutline::Eager(6) };
                        let qn = self.bytes(q.lo, q.hi);
                        if !(matches!(qn, b"std" | b"core" | b"alloc") && (i < 4 || self.toks.at(i - 4).kind != T::PathSep)) {
                            return BodyOutline::Eager(6);
                        }
                    }
                    let m = self.macros.as_ref().unwrap();
                    let module = module.get_or_init(|| self.cur_module());
                    if m.lookup(name, &self.krate, &[], module).is_some() {
                        return BodyOutline::Eager(3);
                    }
                    match crate::builtins::macro_def(name) {
                        Some(d) if d.builtin < crate::builtins::HOSTED && !crate::builtins::is_item_level(d.builtin) => {}
                        // the parser's own built-ins (format family, assert*, vec!, matches!, ..:
                        // arguments parsed in place, MacArgs; Relax's dialect rule R2)
                        None if crate::program::BUILTIN_MACROS.contains(&name) && !matches!(name, "include" | "thread_local" | "global_asm" | "naked_asm" | "test" | "bench" | "test_case" | "derive" | "cfg_accessible" | "cfg_eval" | "global_allocator" | "alloc_error_handler" | "derive_const" | "compile_error") => {}
                        d => {
                            if lazy_stats_on() {
                                let k = format!("{}{}", name, match &d { None => " (unknown)", Some(d) if d.builtin >= crate::builtins::HOSTED => " (hosted)", _ => " (item-level)" });
                                *LAZY_MACROS.lock().unwrap().entry(k).or_insert(0) += 1;
                            }
                            return BodyOutline::Eager(6);
                        }
                    }
                }
                T::Eof => return BodyOutline::Eager(7),
                _ => {}
            }
            i += 1;
        }
        BodyOutline::Eager(7)
    }

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
        // the open delimiters inside, innermost last: a closing one must match (rustc's lexer
        // builds token trees, `foo! { a )` is "mismatched closing delimiter")
        let mut open_inner: Vec<T> = Vec::new();
        loop {
            match self.kind() {
                k @ (T::OpenParen | T::OpenBracket | T::OpenBrace) => open_inner.push(k),
                k @ (T::CloseParen | T::CloseBracket | T::CloseBrace) => {
                    let inner = open_inner.pop();
                    let opener = inner.unwrap_or(open);
                    if !matches!((opener, k), (T::OpenParen, T::CloseParen) | (T::OpenBracket, T::CloseBracket) | (T::OpenBrace, T::CloseBrace)) {
                        return self.err("mismatched closing delimiter");
                    }
                    if inner.is_none() {
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
        let name = match self.ast.segs(path.segs).last() {
            Some(s) => self.bytes(s.name.lo, s.name.hi).to_vec(),
            None => return NO_MAC_ARGS,
        };
        let known = matches!(
            name.as_slice(),
            b"format" | b"print" | b"println" | b"eprint" | b"eprintln" | b"write" | b"writeln" | b"panic" | b"format_args"
                | b"vec" | b"assert" | b"assert_eq" | b"assert_ne" | b"debug_assert" | b"debug_assert_eq" | b"debug_assert_ne"
                | b"matches" | b"unreachable" | b"todo" | b"unimplemented" | b"dbg" | b"asm"
        );
        if !known {
            return NO_MAC_ARGS;
        }
        let is_matches = name == b"matches";
        let is_vec = name == b"vec";
        let save = self.pos;
        self.pos = toks.lo as usize;
        let r = if name == b"asm" { self.asm_args(toks.hi as usize) } else { self.mac_args_inner(is_matches, is_vec, toks.hi as usize) };
        self.pos = save;
        match r {
            Ok(a) => {
                self.ast.mac_args.push(a);
                self.ast.base[4] + self.ast.mac_args.len() as u32 - 1
            }
            Err(_) => NO_MAC_ARGS,
        }
    }

    fn mac_args_inner(&mut self, is_matches: bool, is_vec: bool, hi: usize) -> PResult<MacArgs> {
        let mut a = MacArgs { exprs: List::EMPTY, repeat: false, pat: None, guard: None, asm: None };
        let mut exprs = Vec::new();
        if is_matches {
            exprs.push(self.expr()?);
            self.expect(T::Comma)?;
            a.pat = Some(self.pat_top()?);
            if self.eat(T::KwIf) {
                a.guard = Some(self.expr()?);
            }
            self.eat(T::Comma);
        } else {
            while self.pos < hi {
                exprs.push(self.expr()?);
                if is_vec && exprs.len() == 1 && self.eat(T::Semi) {
                    exprs.push(self.expr()?);
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
        a.exprs = self.ast.push_list(exprs);
        Ok(a)
    }

    /// `asm!(templates.., operands.., options(..), clobber_abi(..))` (Reference
    /// inline-assembly.md [asm.syntax]); template strings become the leading `exprs`.
    fn asm_args(&mut self, hi: usize) -> PResult<MacArgs> {
        let mut a = MacArgs { exprs: List::EMPTY, repeat: false, pat: None, guard: None, asm: None };
        let mut spec = AsmArgs { templates: 0, ops: List::EMPTY, options: 0 };
        let mut exprs = Vec::new();
        let mut ops = Vec::new();
        while self.pos < hi && matches!(self.kind(), T::Str | T::RawStr) {
            exprs.push(self.expr()?);
            spec.templates += 1;
            if !self.eat(T::Comma) {
                break;
            }
        }
        if spec.templates == 0 {
            return self.err("asm! needs a template string literal first");
        }
        while self.pos < hi {
            if self.is_word(0, b"options") && self.nth(1) == T::OpenParen {
                self.pos += 2;
                while self.kind() != T::CloseParen {
                    let t = self.bump();
                    match self.text_of(t) {
                        b"noreturn" => spec.options |= ASM_NORETURN,
                        b"raw" => spec.options |= ASM_RAW,
                        b"att_syntax" => spec.options |= ASM_ATT_SYNTAX,
                        b"pure" | b"nomem" | b"readonly" | b"preserves_flags" | b"nostack" | b"may_unwind" | b"," => {}
                        _ => return self.err("unknown asm! option"),
                    }
                }
                self.expect(T::CloseParen)?;
            } else if self.is_word(0, b"clobber_abi") && self.nth(1) == T::OpenParen {
                // the stub is a C call: every caller-saved register is clobbered already
                self.pos += 2;
                while self.kind() != T::CloseParen && self.kind() != T::Eof {
                    self.pos += 1;
                }
                self.expect(T::CloseParen)?;
            } else {
                let name = if matches!(self.kind(), T::Ident | T::RawIdent) && self.nth(1) == T::Eq {
                    let n = self.ident_tok()?;
                    self.pos += 1;
                    Some(n)
                } else {
                    None
                };
                let dir = match self.kind() {
                    T::KwIn => AsmDir::In,
                    T::KwConst => AsmDir::Const,
                    T::Ident => match self.text_of(self.tok()) {
                        b"out" => AsmDir::Out,
                        b"lateout" => AsmDir::LateOut,
                        b"inout" => AsmDir::InOut,
                        b"inlateout" => AsmDir::InLateOut,
                        b"sym" => AsmDir::Sym,
                        _ => return self.err("expected an asm! operand"),
                    },
                    _ => return self.err("expected an asm! operand"),
                };
                self.pos += 1;
                let mut op = AsmOp { name, dir, reg: Ident { lo: 0, hi: 0 }, explicit: false, input: None, output: None };
                match dir {
                    AsmDir::Const | AsmDir::Sym => op.input = Some(self.expr()?),
                    _ => {
                        self.expect(T::OpenParen)?;
                        op.explicit = self.kind() == T::Str;
                        let t = self.bump();
                        op.reg = Ident { lo: t.lo, hi: t.hi };
                        self.expect(T::CloseParen)?;
                        let underscore = |p: &Self| p.is_word(0, b"_") && matches!(p.nth(1), T::Comma | T::CloseParen | T::Eof) ;
                        match dir {
                            AsmDir::In => op.input = Some(self.expr()?),
                            AsmDir::Out | AsmDir::LateOut => {
                                if underscore(self) || (self.is_word(0, b"_") && self.pos + 1 >= hi) {
                                    self.pos += 1;
                                } else {
                                    op.output = Some(self.expr()?);
                                }
                            }
                            _ => {
                                let x = self.expr()?;
                                if self.eat(T::FatArrow) {
                                    op.input = Some(x);
                                    if self.is_word(0, b"_") && (self.pos + 1 >= hi || matches!(self.nth(1), T::Comma)) {
                                        self.pos += 1;
                                    } else {
                                        op.output = Some(self.expr()?);
                                    }
                                } else {
                                    op.input = Some(x);
                                    op.output = Some(x);
                                }
                            }
                        }
                    }
                }
                if let Some(x) = op.input {
                    exprs.push(x);
                }
                if let Some(x) = op.output {
                    if Some(x) != op.input {
                        exprs.push(x);
                    }
                }
                ops.push(op);
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        if self.pos != hi {
            return self.err("unexpected tokens in asm! arguments");
        }
        a.exprs = self.ast.push_list(exprs);
        spec.ops = self.ast.push_list(ops);
        a.asm = Some(spec);
        Ok(a)
    }

    // ------------------------------------------------------------ attributes

    fn outer_attrs(&mut self) -> PResult<Vec<Attr>> {
        let mut v = Vec::new();
        while self.kind() == T::Pound && self.nth(1) == T::OpenBracket {
            self.pos += 1;
            let (_, toks) = self.delim_tt()?;
            self.unraw_attr(toks);
            v.push(Attr { inner: false, toks });
        }
        Ok(self.expand_cfg_attrs(v))
    }

    /// Raw identifiers in an attribute are its plain names (`#[r#repr(r#C, r#packed)]` is
    /// `#[repr(C, packed)]`): every reader compares attribute names by text.
    fn unraw_attr(&mut self, toks: TokRange) {
        for i in toks.lo as usize..(toks.hi as usize).min(self.toks.len()) {
            if self.toks.at(i).kind == T::RawIdent && self.tok_text(self.toks.at(i)).starts_with(b"r#") {
                self.toks.edit(i, |t| { t.kind = T::Ident; t.lo += 2; });
            }
        }
    }
    fn inner_attrs(&mut self, v: &mut Vec<Attr>) -> PResult<()> {
        let mut new = Vec::new();
        while self.kind() == T::Pound && self.nth(1) == T::Not && self.nth(2) == T::OpenBracket {
            self.pos += 2;
            let (_, toks) = self.delim_tt()?;
            self.unraw_attr(toks);
            new.push(Attr { inner: true, toks });
        }
        v.extend(self.expand_cfg_attrs(new));
        Ok(())
    }

    /// `#[cfg_attr(pred, a, b)]` -> `#[a] #[b]` or nothing, as the attributes are parsed (rustc
    /// expands cfg_attr before anything reads attributes: rustc_expand config.rs), so derives,
    /// `path`, `link_name`, `repr` .. never see a cfg_attr. Needs the crate's cfg (file context).
    fn expand_cfg_attrs(&self, v: Vec<Attr>) -> Vec<Attr> {
        let ctx = match &self.bctx {
            Some(c) => c.clone(),
            None => return v,
        };
        let view = lazy_bodies::TokView::new(self.src, &self.toks, &self.ext);
        let p = view.src();
        if !v.iter().any(|a| a.toks.hi > a.toks.lo && crate::cfg::tok_str(p, a.toks.lo as usize) == b"cfg_attr") {
            return v;
        }
        crate::cfg::expand_cfg_attr_list(p, &v, &ctx.cfg)
    }

    // ------------------------------------------------------------ file

    pub fn parse_file(&mut self) -> PResult<()> {
        let mut attrs = Vec::new();
        self.inner_attrs(&mut attrs)?;
        // a module file whose inner #![cfg] is false is removed with its module (rustc
        // config.rs): nothing in it is parsed or expanded
        if !self.cfg_active(&attrs) {
            self.ast.root_attrs = attrs.into();
            return Ok(());
        }
        self.ast.root_attrs = attrs.into();
        let items = self.items_until(T::Eof)?;
        self.ast.root_items = items.into();
        Ok(())
    }

    // ------------------------------------------------------------ macro_rules!

    /// Registers a `macro_rules! name { .. }` definition (tokens in `toks`).
    fn define_rules(&mut self, name: Ident, toks: TokRange, exported: bool, local_inner: bool) -> PResult<()> {
        if self.macros.is_none() {
            return Ok(());
        }
        // a raw identifier names the same macro (`macro_rules! r#try` is `try!`)
        let n = String::from_utf8_lossy(self.bytes(name.lo, name.hi)).trim_start_matches("r#").to_string();
        let mt = crate::macros::mtoks(self, &self.toks.range(toks.lo as usize, toks.hi as usize));
        let file = self.bctx.as_ref().map_or("", |c| c.path.as_str());
        self.macros.as_mut().unwrap().define_tokens(&n, &self.krate, file, name.lo, exported, local_inner, &mt)
            .map_err(|msg| PErr { pos: name.lo, msg })
    }

    /// A `use` item's bindings for macro lookups in this parse (MacroScope::add_use).
    fn record_use(&self, global: bool, tree: &UseTree) {
        let Some(m) = self.macros.as_ref() else { return };
        let module = self.cur_module();
        let mut prefix: Vec<String> = Vec::new();
        if global {
            prefix.push(String::new());
        }
        self.record_use_tree(m, &module, &mut prefix, tree);
    }

    fn seg_text(&self, sg: &PathSeg) -> String {
        match sg.kind() {
            SegKind::Crate => "crate".to_string(),
            SegKind::Super => "super".to_string(),
            SegKind::SelfValue => "self".to_string(),
            _ => String::from_utf8_lossy(self.bytes(sg.name.lo, sg.name.hi)).trim_start_matches("r#").to_string(),
        }
    }

    fn record_use_tree(&self, m: &crate::macros::MacroScope, module: &str, prefix: &mut Vec<String>, tree: &UseTree) {
        match tree {
            UseTree::Path(sg, rest) => {
                prefix.push(self.seg_text(sg));
                self.record_use_tree(m, module, prefix, self.ast.use_tree(*rest));
                prefix.pop();
            }
            UseTree::Name(sg, rename) => {
                let name = self.seg_text(sg);
                let bind = match rename {
                    Some(r) => String::from_utf8_lossy(self.bytes(r.lo, r.hi)).trim_start_matches("r#").to_string(),
                    None => name.clone(),
                };
                if bind == "_" || name == "self" {
                    return;
                }
                let mut p = prefix.clone();
                p.push(name);
                m.add_use(module, bind, p);
            }
            UseTree::Glob => m.add_glob_use(module, prefix.clone()),
            UseTree::Group(v) => {
                for t in self.ast.list(*v) {
                    self.record_use_tree(m, module, prefix, t);
                }
            }
        }
    }

    /// The module the parser is in (`module_path!` form: the file's module and inline `mod`s).
    pub fn cur_module(&self) -> String {
        let mut m = match &self.bctx {
            Some(c) => c.module_path.clone(),
            None => self.krate.clone(),
        };
        for s in &self.mod_stack {
            m.push_str("::");
            m.push_str(s);
        }
        m
    }

    /// The `macro_rules!` macro a call path names, if any.
    fn rules_macro(&self, path: &Path) -> Option<crate::macros::MacroRef> {
        let m = self.macros.as_ref()?;
        let seg = self.ast.segs(path.segs).last()?;
        let name = String::from_utf8_lossy(self.bytes(seg.name.lo, seg.name.hi)).trim_start_matches("r#").to_string();
        // a skipped body calls only built-ins (body_outline checked each call where it was)
        if self.lazy_part {
            return crate::builtins::macro_def(&name);
        }
        // `dep::m!` / `crate::m!` / `$crate::m!` (expanded as `::dep::m!` or `crate::m!`): the
        // module part, resolved like an item path
        let mut mpath: Vec<String> = Vec::new();
        if path.global() {
            mpath.push(String::new());
        }
        for f in &self.ast.segs(path.segs)[..path.segs.len() - 1] {
            mpath.push(match f.kind() {
                SegKind::Crate => "crate".to_string(),
                SegKind::Super => "super".to_string(),
                SegKind::SelfValue => "self".to_string(),
                _ => String::from_utf8_lossy(self.bytes(f.name.lo, f.name.hi)).trim_start_matches("r#").to_string(),
            });
        }
        // `::__relax_crate_x::m!` is `$crate::m!` of crate x
        if mpath.len() >= 2 && mpath[0].is_empty() && mpath[1].starts_with("__relax_crate_") {
            mpath.remove(0);
        }
        let module = self.cur_module();
        let qual = mpath.iter().find(|x| !x.is_empty()).cloned();
        let qual = qual.map(|q| q.strip_prefix("__relax_crate_").map(|x| x.to_string()).unwrap_or(q));
        let r = m.lookup(&name, &self.krate, &mpath, &module);
        // Relax's built-in macros (builtins.rs) when no macro_rules! of that name is visible
        match r {
            Some(d) => Some(crate::builtins::override_rules(&self.session.options, d)),
            None => crate::builtins::macro_def(&name).or_else(|| {
                // a macro this crate imported under another name (`use std::{eprintln as log_error}`)
                let own = matches!(qual.as_deref(), None | Some("crate") | Some("self") | Some("super"));
                let (q, target) = if own { m.table.alias(&self.krate, &name)?.clone() } else { return None };
                let q2 = match q.as_str() {
                    "std" | "core" | "alloc" | "" => None,
                    "crate" | "self" | "super" => Some("crate"),
                    other => Some(other),
                };
                // the dep's own re-export under that name (`pub use crate::{__trace as trace}`)
                let dep_alias = || -> Option<crate::macros::MacroRef> {
                    let dep = q2.filter(|q| *q != "crate")?;
                    let (dq, dt) = m.table.alias(dep, &target)?.clone();
                    let dq2 = match dq.as_str() {
                        "crate" | "self" | "super" | "" => dep.to_string(),
                        other => other.to_string(),
                    };
                    m.lookup(&dt, &self.krate, &[dq2.clone()], &module)
                };
                let q2v: Vec<String> = q2.map(|q| vec![q.to_string()]).unwrap_or_default();
                match m.lookup(&target, &self.krate, &q2v, &module).or_else(dep_alias) {
                    Some(d) => Some(crate::builtins::override_rules(&self.session.options, d)),
                    // a name imported as it is (`use dep::{trace}`) that is no macro there
                    None if target == name => None,
                    None => match crate::builtins::macro_def(&target) {
                        Some(d) => Some(d),
                        // a std macro Relax handles by its name (format family, assert, ..):
                        // the alias forwards its tokens to it, as `alias!(..)` => `target!(..)`
                        None => m.forwarding_def(&name, &self.krate, &target),
                    },
                }
            }),
        }
    }

    /// `concat!(args)` with every argument that is a `macro_rules!` call replaced by its expansion
    /// text, or None when no argument is one (the built-in then reads the arguments as they are).
    fn concat_with_rules_expanded(&mut self, toks: TokRange) -> PResult<Option<String>> {
        let (lo, hi) = (toks.lo as usize, toks.hi as usize);
        // top-level arguments
        let mut args = Vec::new();
        let mut depth = 0i32;
        let mut s = lo;
        for i in lo..hi {
            match self.toks.at(i).kind {
                T::OpenParen | T::OpenBracket | T::OpenBrace => depth += 1,
                T::CloseParen | T::CloseBracket | T::CloseBrace => depth -= 1,
                T::Comma if depth == 0 => {
                    args.push((s, i));
                    s = i + 1;
                }
                _ => {}
            }
        }
        if hi > s {
            args.push((s, hi));
        }
        let mut any = false;
        let mut out = Vec::new();
        for (a, z) in args {
            // `path ! (..)`: idents and `::` up to the `!`, then one delimited group to the end
            let mut i = a;
            while i < z && matches!(self.toks.at(i).kind, T::Ident | T::PathSep | T::KwCrate | T::Dollar) {
                i += 1;
            }
            let call = i > a && i + 2 < z + 1 && self.toks.at(i).kind == T::Not && matches!(self.toks.at(i + 1).kind, T::OpenParen | T::OpenBracket | T::OpenBrace) && matches!(self.toks.at(z - 1).kind, T::CloseParen | T::CloseBracket | T::CloseBrace);
            let def = if call {
                let name = String::from_utf8_lossy(self.tok_text(self.toks.at(i - 1))).trim_start_matches("r#").to_string();
                let first = String::from_utf8_lossy(self.tok_text(self.toks.at(a))).into_owned();
                let _ = first;
                // the path's module part (idents, `crate`, `$crate` spelt `__relax_crate_x`)
                let mut mpath: Vec<String> = Vec::new();
                if self.toks.at(a).kind == T::PathSep {
                    mpath.push(String::new());
                }
                for k in a..i.saturating_sub(1) {
                    if matches!(self.toks.at(k).kind, T::Ident | T::KwCrate) {
                        mpath.push(String::from_utf8_lossy(self.tok_text(self.toks.at(k))).trim_start_matches("r#").to_string());
                    }
                }
                if mpath.len() >= 2 && mpath[0].is_empty() && mpath[1].starts_with("__relax_crate_") {
                    mpath.remove(0);
                }
                let module = self.cur_module();
                match &self.macros {
                    Some(m) => m.lookup(&name, &self.krate, &mpath, &module).filter(|d| d.builtin == 0),
                    None => None,
                }
            } else {
                None
            };
            // a nested `concat!(..)` with macro_rules calls inside: the same, recursively
            let nested = if call && def.is_none() && self.tok_text(self.toks.at(i - 1)) == b"concat" {
                self.concat_with_rules_expanded(TokRange { lo: (i + 2) as u32, hi: (z - 1) as u32 })?
            } else {
                None
            };
            match def {
                Some(d) => {
                    let (text, _, _) = self.expand_rules(&d, i + 2, z - 1).map_err(|m| PErr { pos: self.toks.at(a).lo, msg: m })?;
                    // its own output may hold more calls: concat! re-expands it as an argument
                    out.push(text);
                    any = true;
                }
                None if nested.is_some() => {
                    out.push(nested.unwrap());
                    any = true;
                }
                None => {
                    let lo = self.toks.at(a).lo as usize;
                    let hi = self.toks.at(z - 1).hi as usize;
                    out.push(String::from_utf8_lossy(self.bytes(lo as u32, hi as u32)).into_owned());
                }
            }
        }
        Ok(if any { Some(format!("concat!({})", out.join(", "))) } else { None })
    }

    /// Expands a call: appends the expansion's tokens (ending in Eof) and returns their start.
    fn expand_call(&mut self, def: &crate::macros::MacroDef, toks: TokRange, call_lo: u32) -> PResult<usize> {
        self.expand_call_inner(def, toks, call_lo)
    }

    fn expand_call_inner(&mut self, def: &crate::macros::MacroDef, toks: TokRange, call_lo: u32) -> PResult<usize> {
        if self.exp_depth > 256 {
            return Err(PErr { pos: call_lo, msg: format!("`{}!` expands more than 256 levels deep -- add a base case", def.name) });
        }
        let trace = crate::preload::relax_trace().map_or(false, |t| t.split(',').any(|p| p.strip_prefix("expand:").map_or(false, |n| def.name.contains(n))));
        if def.builtin == crate::builtins::Builtin::Concat as u8 {
            // concat!'s arguments are expanded first (rustc expands macro calls among them
            // eagerly: `concat!("{", asset_ref!("x"), "}")` with a user macro_rules)
            if let Some(text) = self.concat_with_rules_expanded(toks)? {
                return self.expand_text("concat", &text, call_lo);
            }
        }
        if def.builtin != 0 {
            // built-in / hosted proc macro: tokens (its output is not re-lexed)
            let call_hi = self.toks.at(toks.hi as usize).hi;
            let b = def.builtin;
            let x = match crate::preload::timed_named(4, &def.name, || crate::builtins::expand(self, b, toks.lo as usize, toks.hi as usize, call_lo, call_hi)) {
                Ok(x) => x,
                // the error stays at the call (`compile_error!`): an expression's with the fn
                // that contains it, an item's with the module; the file still loads
                Err(m) => {
                    let semi = if crate::builtins::is_item_level(b) { ";" } else { "" };
                    crate::builtins::Expansion::lexed(format!("compile_error!({:?}){}", format!("`{}!`: {}", def.name, m), semi)).map_err(|m| PErr { pos: call_lo, msg: m })?
                }
            };
            // MAKEPAD_RUN_TRACE=expand:<macro name> prints expansions (debugger-free macro debugging)
            if trace {
                eprintln!("expand {}! => {}", def.name, x.text);
            }
            // output identifiers keep their input tokens' positions (hygiene contexts: a
            // fragment forwarded into a proc macro by a macro_rules stays the caller's)
            self.append_origins(&x, call_lo);
            return Ok(self.expand_toks(&x.text, &x.toks, call_lo));
        }
        let _profile = crate::load_profile::span(self.session.runtime.profile, "macro-rules-output", &def.name);
        let key = crate::parse_work::rules_key(self, def, call_lo, self.toks.at(toks.hi as usize).hi);
        let (text, origins, bodies) = if let Some(x) = crate::parse_work::get_rules(&key) { x } else {
            let x = crate::preload::timed_named(2, &def.name, || self.expand_rules(def, toks.lo as usize, toks.hi as usize)).map_err(|m| PErr { pos: call_lo, msg: m })?;
            crate::parse_work::put_rules(key, &x);
            x
        };
        if trace {
            eprintln!("expand {}! => {}", def.name, text);
        }
        let base = (self.src.len() + self.ext.len()) as u32;
        for (off, o, n) in origins {
            self.tok_origin.push((base + off, o, n));
        }
        let here = self.bctx.as_ref().map(|c| c.path.as_str()).unwrap_or("");
        let same_file = !def.def_file.is_empty() && def.def_file == here;
        let def_pos = if same_file { def.def_pos } else { u32::MAX };
        self.rules_exp.push((base, base + text.len() as u32, def_pos));
        // a body token's position in the definition (same file only): its own context is the
        // parent of the expansion's mark (macros defined by macros)
        if same_file {
            for (off, bp, n) in bodies {
                self.body_origin.push((base + off, bp, n));
            }
        }
        self.expand_text(&def.name, &text, call_lo)
    }

    /// Appends an expansion given as tokens over `text` (positions relative to it; a proc
    /// macro's output, no lexing) and returns the index of its first token.
    pub fn expand_toks(&mut self, text: &str, toks: &[Tok], call_lo: u32) -> usize {
        let base = (self.src.len() + self.ext.len()) as u32;
        self.ext.extend_from_slice(text.as_bytes());
        let start = self.toks.len();
        for t in toks {
            self.toks.push(Tok { kind: t.kind, lo: t.lo + base, hi: t.hi + base });
        }
        let e = base + text.len() as u32;
        self.toks.push(Tok { kind: T::Eof, lo: e, hi: e });
        self.exp_map.push((base, e, call_lo));
        start
    }

    /// Appends expansion text (lexed; ends in Eof) and returns the index of its first token.
    pub fn expand_text(&mut self, name: &str, text: &str, call_lo: u32) -> PResult<usize> {
        let base = (self.src.len() + self.ext.len()) as u32;
        self.ext.extend_from_slice(text.as_bytes());
        let mut nt = Vec::new();
        crate::lexer::lex(text.as_bytes(), self.edition, &mut nt).map_err(|e| PErr { pos: call_lo, msg: format!("`{}!` expands to text that does not lex: {}", name, e.msg) })?;
        let start = self.toks.len();
        for mut t in nt {
            t.lo += base;
            t.hi += base;
            self.toks.push(t);
        }
        if self.toks.last().map_or(true, |t| t.kind != T::Eof) {
            let e = base + text.len() as u32;
            self.toks.push(Tok { kind: T::Eof, lo: e, hi: e });
        }
        self.exp_map.push((base, base + text.len() as u32, call_lo));
        Ok(start)
    }

    /// Parses an expansion that starts at token `start`; it must consume it all.
    fn in_expansion<X>(&mut self, start: usize, f: impl FnOnce(&mut Self) -> PResult<X>) -> PResult<X> {
        let rules = self.session.runtime.profile && self.rules_exp.last().map_or(false, |(lo, hi, _)| lo <= self.toks.at(start).lo && self.toks.at(start).lo < hi);
        let _profile = crate::load_profile::span(rules, "macro-rules-parse", self.bctx.as_ref().map_or("", |c| c.path.as_str()));
        let save = self.pos;
        self.pos = start;
        self.exp_depth += 1;
        let r = f(self);
        self.exp_depth -= 1;
        let at_end = self.kind() == T::Eof;
        let rest = self.lo();
        self.pos = save;
        let x = r?;
        if !at_end {
            return Err(PErr { pos: rest, msg: "macro expansion has tokens left over -- check the macro body".to_string() });
        }
        Ok(x)
    }

    /// At an item / statement start: a `macro_rules!` call (`path!(..)` / `path! { .. }` ending
    /// the item or statement) expanded and parsed with `f`.
    fn try_expand_rules<X>(&mut self, f: impl FnOnce(&mut Self) -> PResult<X>) -> PResult<Option<X>> {
        if self.macros.is_none() || !self.mac_ahead() {
            return Ok(None);
        }
        let save = self.pos;
        let lo = self.lo();
        let path = self.path(true)?;
        let def = match self.rules_macro(&path) {
            Some(d) => d,
            None => {
                self.pos = save;
                return Ok(None);
            }
        };
        self.expect(T::Not)?;
        let (delim, toks) = self.delim_tt()?;
        // `m!(..).method()` etc. continue as an expression: not a statement macro
        if delim != b'{' && !matches!(self.kind(), T::Semi | T::CloseBrace | T::Eof) {
            self.pos = save;
            return Ok(None);
        }
        self.eat(T::Semi);
        let start = self.expand_call(&def, toks, lo)?;
        Ok(Some(self.in_expansion(start, f)?))
    }

    /// One statement of a block, or all statements of a statement-position macro call.
    fn block_stmts_step(&mut self, stmts: &mut Vec<Stmt>) -> PResult<()> {
        if self.macros.is_some() && self.mac_ahead() {
            let save = self.pos;
            if self.stmt_macro(stmts)? {
                return Ok(());
            }
            self.pos = save;
        }
        // `#[attrs] m! { .. }` (cfg_if's expansions are `#[cfg(..)] $crate::cfg_if! { .. }`):
        // the call is cfg-stripped before it expands (rustc_expand config.rs: a statement whose
        // #[cfg] is false is removed with its tokens), otherwise it is a statement macro
        if self.macros.is_some() && self.kind() == T::Pound {
            let save = self.pos;
            let attrs = self.outer_attrs()?;
            if self.mac_ahead() {
                let at = self.pos;
                let path = self.path(true)?;
                if self.rules_macro(&path).is_some() {
                    if !self.cfg_active(&attrs) {
                        self.expect(T::Not)?;
                        self.delim_tt()?;
                        self.eat(T::Semi);
                        return Ok(());
                    }
                    self.pos = at;
                    if self.stmt_macro(stmts)? {
                        return Ok(());
                    }
                }
            }
            self.pos = save;
        }
        let s = self.stmt()?;
        // generated items go before the statement (item order does not matter in a block; a
        // tail expression must stay last)
        for id in std::mem::take(&mut self.extra_items) {
            stmts.push(Stmt::Item(id));
        }
        stmts.push(s);
        Ok(())
    }

    /// A statement-position `macro_rules!` call at the cursor: its statements splice into the
    /// block (a `;` after the call ends a tail expression). False (cursor unspecified) when it
    /// is not one.
    fn stmt_macro(&mut self, stmts: &mut Vec<Stmt>) -> PResult<bool> {
        let r = self.try_expand_rules(|p| {
            let mut v = Vec::new();
            while p.kind() != T::Eof {
                // statements of the expansion may be macro calls themselves
                p.block_stmts_step(&mut v)?;
            }
            Ok(v)
        })?;
        match r {
            Some(mut v) => {
                let had_semi = self.pos > 0 && self.toks.at(self.pos - 1).kind == T::Semi;
                if had_semi {
                    if let Some(Stmt::Expr(_, semi)) = v.last_mut() {
                        *semi = true;
                    }
                }
                stmts.extend(v);
                Ok(true)
            }
            None => Ok(false),
        }
    }

    /// A `$s:stmt` fragment (no trailing `;` required).
    pub fn stmt_fragment(&mut self) -> PResult<()> {
        if self.kind() == T::KwLet {
            self.pos += 1;
            self.pat_top()?;
            if self.eat(T::Colon) {
                self.ty()?;
            }
            if self.eat(T::Eq) {
                self.expr()?;
            }
            return Ok(());
        }
        if self.at_item_start() {
            let attrs = self.outer_attrs()?;
            self.item(attrs)?;
            self.extra_items.clear();
            return Ok(());
        }
        self.expr().map(|_| ())
    }

    /// A `$i:item` fragment.
    pub fn item_fragment(&mut self) -> PResult<()> {
        let attrs = self.outer_attrs()?;
        let r = self.item(attrs)?;
        // a fragment match only checks the syntax: drop what it generated
        self.extra_items.clear();
        match r {
            Some(_) => Ok(()),
            None => self.err("expected item"),
        }
    }

    /// A `$m:meta` fragment: path, then `(..)` or `= expr` optionally.
    pub fn meta_fragment(&mut self) -> PResult<()> {
        // an expression-style path: `doc(..)` is the meta's arguments, not `Fn(..)` sugar
        self.path(true)?;
        if matches!(self.kind(), T::OpenParen | T::OpenBracket | T::OpenBrace) {
            self.delim_tt()?;
        } else if self.eat(T::Eq) {
            self.expr()?;
        }
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
            let item_tok = self.pos;
            let attrs = self.outer_attrs()?;
            if self.kind() == end && !attrs.is_empty() {
                // trailing attrs (e.g. cfg'd out tail) are not valid Rust; be strict
                return self.err("attributes without item");
            }
            // an item whose #[cfg] is false is removed before anything expands in it (rustc
            // cfg-strips before macro expansion: rustc_expand config.rs), but it is parsed first
            // (rustc parses the whole file: a syntax error in a cfg'd-out item is an error). It is
            // parsed with no expansion, derives or macro definitions, and then dropped.
            if !attrs.is_empty() && !self.cfg_active(&attrs) {
                self.parse_cfg_out(attrs)?;
                continue;
            }
            if let Some(v) = self.try_expand_rules(|p| p.items_until(T::Eof))? {
                items.extend(v);
                continue;
            }
            // an attribute macro (`#[proc_macro_attribute]` of the graph) replaces the item with
            // its output: the item is parsed for its extent only (no expansion), then dropped
            if !attrs.is_empty() && self.macros.is_some() && self.cfg_out == 0 {
                if let Some(k) = crate::builtins::attr_macro_at(self, &attrs) {
                    let attr = attrs[k].clone();
                    self.parse_cfg_out(attrs)?;
                    let x = crate::builtins::expand_attr_macro(self, &attr, item_tok, self.pos).map_err(|(pos, msg)| PErr { pos, msg })?;
                    self.append_origins(&x, self.toks.at(item_tok).lo);
                    let start = self.expand_toks(&x.text, &x.toks, self.toks.at(item_tok).lo);
                    let v = self.in_expansion(start, |p| p.items_until(T::Eof))?;
                    items.extend(v);
                    continue;
                }
            }
            let dattrs = if attrs.is_empty() || self.macros.is_none() { Vec::new() } else { attrs.clone() };
            match self.item(attrs)? {
                Some(id) => {
                    items.push(id);
                    if self.in_assoc == 0 {
                        items.append(&mut self.extra_items);
                    } else { items.append(&mut self.extra_assoc_items); }
                }
                None => return self.err("expected item"),
            }
            items.extend(self.hosted_derives(&dattrs, item_tok)?);
        }
        Ok(items)
    }

    /// Expand derives for module and block items alike, before item/impl collection.
    /// The proc-macro host selects native or JIT execution.
    fn hosted_derives(&mut self, attrs: &[Attr], item_tok: usize) -> PResult<Vec<ItemId>> {
        if attrs.is_empty() || self.macros.is_none() || self.cfg_out != 0 {
            return Ok(Vec::new());
        }
        let r = crate::builtins::derive_items(self, attrs, item_tok, self.pos).map_err(|(pos, msg)| PErr { pos, msg })?;
        let Some(x) = r else { return Ok(Vec::new()) };
        let _profile = crate::load_profile::span(self.session.runtime.profile, "derive-output-parse", self.bctx.as_ref().map_or("", |c| c.path.as_str()));
        let tp = std::time::Instant::now();
        let origins_profile = crate::load_profile::span(self.session.runtime.profile, "derive-output-origins", self.bctx.as_ref().map_or("", |c| c.path.as_str()));
        self.append_origins(&x, self.toks.at(item_tok).lo);
        let start = self.expand_toks(&x.text, &x.toks, self.toks.at(item_tok).lo);
        drop(origins_profile);
        // Pending built-in derive impls belong to the caller's item list, not to any
        // module or associated-item list introduced by the hosted expansion.
        let outer_items = std::mem::take(&mut self.extra_items);
        let items = self.in_expansion(start, |p| p.items_until(T::Eof));
        self.extra_items = outer_items;
        crate::macro_reg::add_stats(0, tp.elapsed().as_nanos() as u64, 0);
        items
    }

    /// Do the item's `#[cfg]`s hold for this file's crate? (No file context: yes.)
    fn cfg_active(&self, attrs: &[Attr]) -> bool {
        let Some(ctx) = &self.bctx else { return true };
        if !attrs.iter().any(|a| a.toks.hi > a.toks.lo && self.tok_text(self.toks.at(a.toks.lo as usize)) == b"cfg") {
            return true;
        }
        let view = lazy_bodies::TokView::new(self.src, &self.toks, &self.ext);
        let p = view.src();
        crate::cfg::active(p, attrs, &ctx.cfg)
    }

    /// A cfg'd-out item: parsed for its syntax only (no macro expansion, no derives, no
    /// macro_rules definitions, no generated items), then removed from the tree.
    fn parse_cfg_out(&mut self, attrs: Vec<Attr>) -> PResult<()> {
        let lens = (self.ast.exprs.len(), self.ast.pats.len(), self.ast.tys.len(), self.ast.items.len(), self.ast.blocks.len(), self.ast.mac_args.len(), self.extra_items.len());
        let ends = self.ast.ends();
        let macros = self.macros.take();
        self.cfg_out += 1;
        let r = self.item(attrs);
        self.cfg_out -= 1;
        self.macros = macros;
        self.ast.exprs.truncate(lens.0);
        self.ast.pats.truncate(lens.1);
        self.ast.tys.truncate(lens.2);
        self.ast.items.truncate(lens.3);
        self.ast.blocks.truncate(lens.4);
        self.ast.mac_args.truncate(lens.5);
        self.ast.fn_sigs.truncate((ends[24] - self.ast.base[24]) as usize);
        self.ast.params.truncate((ends[25] - self.ast.base[25]) as usize);
        self.ast.field_defs.truncate((ends[26] - self.ast.base[26]) as usize);
        self.ast.variants.truncate((ends[27] - self.ast.base[27]) as usize);
        self.ast.use_trees.truncate((ends[28] - self.ast.base[28]) as usize);
        self.ast.item_lists.truncate((ends[29] - self.ast.base[29]) as usize);
        self.ast.where_preds.truncate((ends[30] - self.ast.base[30]) as usize);
        self.ast.method_calls.truncate((ends[31] - self.ast.base[31]) as usize);
        self.ast.for_exprs.truncate((ends[32] - self.ast.base[32]) as usize);
        self.ast.closure_exprs.truncate((ends[33] - self.ast.base[33]) as usize);
        self.ast.struct_exprs.truncate((ends[34] - self.ast.base[34]) as usize);
        self.ast.mac_calls.truncate((ends[35] - self.ast.base[35]) as usize);
        self.ast.while_exprs.truncate((ends[36] - self.ast.base[36]) as usize);
        self.ast.use_items.truncate((ends[39] - self.ast.base[39]) as usize);
        self.ast.struct_items.truncate((ends[40] - self.ast.base[40]) as usize);
        self.ast.union_items.truncate((ends[41] - self.ast.base[41]) as usize);
        self.ast.enum_items.truncate((ends[42] - self.ast.base[42]) as usize);
        self.ast.alias_items.truncate((ends[43] - self.ast.base[43]) as usize);
        self.ast.trait_items.truncate((ends[44] - self.ast.base[44]) as usize);
        self.ast.trait_alias_items.truncate((ends[45] - self.ast.base[45]) as usize);
        self.ast.impl_items.truncate((ends[46] - self.ast.base[46]) as usize);
        self.ast.foreign_mods.truncate((ends[47] - self.ast.base[47]) as usize);
        self.ast.vis_paths.truncate((ends[48] - self.ast.base[48]) as usize);
        // let_stmts / attrs_stmts stay: the statement lists (never truncated here) keep the
        // cfg'd-out bodies' statements, which point at them
        self.extra_items.truncate(lens.6);
        match r? {
            Some(_) => Ok(()),
            None => self.err("expected item"),
        }
    }

    // ------------------------------------------------------------ visibility

    fn vis(&mut self) -> PResult<Vis> {
        if self.kind() == T::KwCrate && self.nth(1) != T::PathSep {
            // `crate` visibility (old nightly); treat as pub
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
                    return Ok(Vis::In(self.ast.push_row(p)));
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
        let tok_lo = self.pos;
        self.item_start.push([self.ast.exprs.len() as u32, self.ast.pats.len() as u32]);
        let r = self.item_inner(attrs, lo);
        self.item_start.pop();
        if let Ok(Some(id)) = &r {
            if !self.ast.item(*id).attrs.is_empty() {
                self.derive_after(*id, tok_lo)?;
            }
            if self.cfg_out == 0 && matches!(self.ast.item(*id).kind, ItemKind::Struct(..) | ItemKind::Enum(..)) {
                self.default_consts(*id)?;
            }
        }
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
                let use_lo = self.lo();
                self.pos += 1;
                let global = self.eat(T::PathSep);
                let tree = self.use_tree()?;
                self.expect(T::Semi)?;
                // imports a macro expansion wrote (the file's own are in the prescan's tree)
                if self.macros.is_some() && use_lo as usize >= self.src.len() {
                    self.record_use(global, &tree);
                }
                ItemKind::Use(self.ast.push_row(UseItem { global, tree }))
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
                let items = self.foreign_items(abi)?;
                ItemKind::ForeignMod(self.ast.push_row(ForeignModItem { abi, items }))
            }
            T::KwUnsafe if self.nth(1) == T::KwExtern && (self.nth(2) == T::OpenBrace || (is_str(self.nth(2)) && self.nth(3) == T::OpenBrace)) => {
                self.pos += 2;
                let abi = self.abi();
                let items = self.foreign_items(abi)?;
                ItemKind::ForeignMod(self.ast.push_row(ForeignModItem { abi, items }))
            }
            T::KwFn | T::KwConst | T::KwAsync | T::KwUnsafe | T::KwExtern
                if self.fn_ahead() =>
            {
                let (n, mut sig, body) = self.fn_item()?;
                if let Some(abi) = self.foreign_abi {
                    sig.abi = Some(abi);
                    sig.unsafe_ = true;
                }
                name = n;
                ItemKind::Fn(self.ast.push_fn_sig(sig), body, default_)
            }
            T::Ident if (self.is_word(0, b"safe")) && self.fn_ahead_from(1) => {
                self.pos += 1;
                let (n, mut sig, body) = self.fn_item()?;
                if let Some(abi) = self.foreign_abi { sig.abi = Some(abi); }
                name = n;
                ItemKind::Fn(self.ast.push_fn_sig(sig), body, default_)
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
                ItemKind::Struct(self.ast.push_row(StructItem { generics: g, data }))
            }
            T::Ident if self.is_word(0, b"union") && matches!(self.nth(1), T::Ident | T::RawIdent) => {
                self.pos += 1;
                name = self.ident_tok()?;
                let mut g = self.generics()?;
                self.where_clause(&mut g)?;
                let fields = self.named_fields()?;
                ItemKind::Union(self.ast.push_row(UnionItem { generics: g, fields }))
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
                    vars.push(Variant { attrs: self.ast.push_list(attrs), name: vname, data, disc });
                    if !self.eat(T::Comma) {
                        break;
                    }
                }
                self.expect(T::CloseBrace)?;
                let variants = self.ast.push_list(vars);
                ItemKind::Enum(self.ast.push_row(EnumItem { generics: g, variants }))
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
                let bounds = if self.eat(T::Colon) { self.bounds()? } else { List::EMPTY };
                self.where_clause(&mut generics)?;
                let ty = if self.eat(T::Eq) { Some(self.ty_plus()?) } else { None };
                self.where_clause(&mut generics)?;
                self.expect(T::Semi)?;
                ItemKind::TypeAlias(self.ast.push_row(TypeAliasItem { generics, bounds, ty }))
            }
            T::KwConst => {
                self.pos += 1;
                name = self.ident_or_underscore()?;
                // generic consts (unstable) not supported
                let ty = if self.eat(T::Colon) { Some(self.ty()?) } else { None };
                let e = if self.eat(T::Eq) { Some(self.expr()?) } else { None };
                self.expect(T::Semi)?;
                ItemKind::Const(OptTyId::from(ty), OptExprId::from(e))
            }
            T::KwStatic => {
                self.pos += 1;
                let m = self.eat(T::KwMut);
                name = self.ident_tok()?;
                self.expect(T::Colon)?;
                let ty = self.ty()?;
                let e = if self.eat(T::Eq) { Some(self.expr()?) } else { None };
                self.expect(T::Semi)?;
                ItemKind::Static(m, ty, OptExprId::from(e))
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
                    let active = self.cfg_active(&inner);
                    let macros = if !active {
                        self.cfg_out += 1;
                        self.macros.take()
                    } else {
                        None
                    };
                    let mname = String::from_utf8_lossy(self.bytes(name.lo, name.hi)).trim_start_matches("r#").to_string();
                    self.mod_stack.push(mname);
                    let items = self.items_until(T::CloseBrace);
                    self.mod_stack.pop();
                    if !active {
                        self.cfg_out -= 1;
                        self.macros = macros;
                    }
                    let items = items?;
                    self.expect(T::CloseBrace)?;
                    let mut attrs = attrs;
                    attrs.extend(inner);
                    let attrs = self.ast.push_list(attrs);
                    let items = self.ast.push_list(items);
                    return Ok(Some(self.push_item(Item { ranges: [0; 4], default_, attrs, vis, name, kind: ItemKind::Mod(Some(items)), lo, hi: self.prev_hi() })));
                }
            }
            T::Ident if self.is_word(0, b"macro_rules") && self.nth(1) == T::Not => {
                self.pos += 2;
                name = self.ident_tok()?;
                let (d, toks) = self.delim_tt()?;
                if d != b'{' {
                    self.eat(T::Semi);
                }
                let exported = attrs.iter().any(|a| a.toks.hi > a.toks.lo && self.tok_text(self.toks.at(a.toks.lo as usize)) == b"macro_export");
                let local_inner = attrs.iter().any(|a| a.toks.hi > a.toks.lo && self.tok_text(self.toks.at(a.toks.lo as usize)) == b"macro_export" && (a.toks.lo..a.toks.hi).any(|k| self.tok_text(self.toks.at(k as usize)) == b"local_inner_macros"));
                self.define_rules(name, toks, exported, local_inner)?;
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
                ItemKind::Mac(self.ast.push_row(MacCall { path, delim, toks, args }))
            }
            _ => {
                if matches!(vis, Vis::Private) && attrs.is_empty() && !default_ {
                    return Ok(None);
                }
                return self.err("expected item after attributes/visibility");
            }
        };
        let hi = self.prev_hi();
        let mut attrs = attrs;
        attrs.extend(std::mem::take(&mut self.pending_inner));
        let attrs = self.ast.push_list(attrs);
        Ok(Some(self.push_item(Item { ranges: [0; 4], default_, attrs, vis, name, kind, lo, hi })))
    }

    fn push_item(&mut self, mut it: Item) -> ItemId {
        let st = self.item_start[self.item_start.len() - 1];
        it.ranges = [st[0], self.ast.exprs.len() as u32, st[1], self.ast.pats.len() as u32];
        self.ast.items.push(it);
        ItemId(self.ast.base[23] + self.ast.items.len() as u32 - 1)
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
        if self.toks.at(i).kind == T::PathSep {
            i += 1;
        }
        loop {
            match self.toks.at(i).kind {
                T::Ident | T::RawIdent | T::KwSelfValue | T::KwSuper | T::KwCrate => i += 1,
                _ => return false,
            }
            match self.toks.at(i).kind {
                T::PathSep => i += 1,
                T::Not => {
                    return matches!(self.toks.at(i + 1).kind, T::OpenParen | T::OpenBracket | T::OpenBrace);
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
            let outline = (self.lazy_auto && self.kind() == T::OpenBrace).then(|| self.body_outline());
            if let Some(BodyOutline::Mismatched(pos)) = outline {
                return Err(PErr { pos, msg: "mismatched closing delimiter".into() });
            }
            if let Some(BodyOutline::Deferred(r)) = outline {
                self.pos = r.hi as usize + 1;
                if !self.mod_stack.is_empty() {
                    self.lazy_mods.push((r.lo, self.mod_stack.clone()));
                }
                lazy_body = Some(r);
                None
            } else {
                let stats = lazy_stats_on() && self.kind() == T::OpenBrace;
                let profile = self.session.runtime.profile;
                let why = if stats || profile {
                    match outline.unwrap_or_else(|| self.body_outline()) {
                        BodyOutline::Eager(why) => why,
                        _ => 0,
                    }
                } else { 0 };
                let phase = match why {
                    0 => "parse-body-skippable",
                    1 => "parse-body-macro-output",
                    2 => "parse-body-items",
                    3 => "parse-body-macro-rules",
                    4 => "parse-body-array-length",
                    5 => "parse-body-fragment",
                    6 => "parse-body-other-macro",
                    _ => "parse-body-no-scope",
                };
                let _profile = crate::load_profile::span(profile, phase, self.bctx.as_ref().map_or("", |c| c.path.as_str()));
                if stats {
                    let mps = crate::memstat::peak_start();
                    let b = crate::preload::timed(0, || self.block())?;
                    let bytes = crate::memstat::peak_end(mps);
                    LAZY_STATS[why as usize * 2].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    LAZY_STATS[why as usize * 2 + 1].fetch_add(bytes.max(0) as u64, std::sync::atomic::Ordering::Relaxed);
                    Some(b)
                } else {
                    Some(crate::preload::timed(0, || self.block())?)
                }
            }
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
                params: self.ast.push_list(params),
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
            params.push(Param { attrs: self.ast.push_list(attrs), pat, ty });
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
                let (lt, n) = if self.nth(1) == T::Lifetime { (Some(self.toks.at(self.pos + 1)), 2) } else { (None, 1) };
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

    fn tuple_fields(&mut self) -> PResult<List<FieldDef>> {
        self.expect(T::OpenParen)?;
        let mut v = Vec::new();
        while self.kind() != T::CloseParen {
            let attrs = self.outer_attrs()?;
            let vis = self.vis()?;
            let ty_lo = self.lo();
            let ty = self.ty_plus()?;
            let ty_span = (ty_lo, self.prev_hi());
            v.push(FieldDef { attrs: self.ast.push_list(attrs), vis, name: None, ty, default: None, ty_span });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseParen)?;
        Ok(self.ast.push_list(v))
    }

    fn named_fields(&mut self) -> PResult<List<FieldDef>> {
        self.expect(T::OpenBrace)?;
        let mut v = Vec::new();
        while self.kind() != T::CloseBrace {
            let attrs = self.outer_attrs()?;
            let vis = self.vis()?;
            let name = self.ident_tok()?;
            self.expect(T::Colon)?;
            let ty_lo = self.lo();
            let ty = self.ty_plus()?;
            let ty_span = (ty_lo, self.prev_hi());
            let default = if self.eat(T::Eq) { Some(self.expr()?) } else { None };
            v.push(FieldDef { attrs: self.ast.push_list(attrs), vis, name: Some(name), ty, default, ty_span });
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseBrace)?;
        Ok(self.ast.push_list(v))
    }

    fn trait_item(&mut self, name: &mut Ident, unsafe_: bool, auto: bool) -> PResult<ItemKind> {
        *name = self.ident_tok()?;
        let mut generics = self.generics()?;
        if self.eat(T::Eq) {
            let b = self.bounds()?;
            self.where_clause(&mut generics)?;
            self.expect(T::Semi)?;
            return Ok(ItemKind::TraitAlias(self.ast.push_row(TraitAliasItem { generics, bounds: b })));
        }
        let mut supers = if self.eat(T::Colon) { self.bounds()? } else { List::EMPTY };
        self.where_clause(&mut generics)?;
        // `trait F where Self: Copy + Neg<Output = Self>` is `trait F: Copy + Neg<..>` (rustc:
        // where-clauses on Self are the trait's supertraits, super_predicates_of)
        let mut more = Vec::new();
        for w in self.ast.list(generics.where_) {
            if let WherePred::Bound { ty, bounds, hrtb } = w {
                if hrtb.is_empty() && matches!(self.ast.ty(*ty), Ty::Path(p) if p.no_qself() && matches!(self.ast.segs(p.segs), [s] if s.kind() == SegKind::SelfType && s.args().is_none())) {
                    more.extend_from_slice(self.ast.list(*bounds));
                }
            }
        }
        if !more.is_empty() {
            let mut v = self.ast.list(supers).to_vec();
            v.extend(more);
            supers = self.ast.push_list(v);
        }
        let items = self.assoc_items()?;
        Ok(ItemKind::Trait(self.ast.push_row(TraitItem { unsafe_, auto, generics, supers, items })))
    }

    fn assoc_items(&mut self) -> PResult<List<ItemId>> {
        self.expect(T::OpenBrace)?;
        // inner attributes (`impl X { #![cfg(..)] .. }`) apply to the enclosing item
        let mut inner = Vec::new();
        self.inner_attrs(&mut inner)?;
        self.in_assoc += 1;
        let items = self.items_until(T::CloseBrace);
        self.in_assoc -= 1;
        let items = items?;
        self.expect(T::CloseBrace)?;
        // (after the member items, which are items of their own)
        self.pending_inner.extend(inner);
        Ok(self.ast.push_list(items))
    }

    // rustc_ast_lowering/src/item.rs:795: foreign functions inherit the block ABI
    // and default to unsafe; an explicit `safe fn` overrides only the safety.
    fn foreign_items(&mut self, abi: Option<Ident>) -> PResult<List<ItemId>> {
        let saved = self.foreign_abi;
        self.foreign_abi = Some(abi);
        let items = self.assoc_items();
        self.foreign_abi = saved;
        items
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
            let p = match self.ast.ty(first) {
                Ty::Path(p) => p.clone(),
                _ => return self.err("expected trait path in impl"),
            };
            (Some(p), self.ty_impl_header()?)
        } else {
            (None, first)
        };
        self.where_clause(&mut generics)?;
        let items = self.assoc_items()?;
        Ok(ItemKind::Impl(self.ast.push_row(ImplItem { unsafe_, default_, generics, negative, trait_, self_ty, items })))
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
                Ok(UseTree::Group(self.ast.push_list(v)))
            }
            _ => {
                let seg = self.path_seg_name()?;
                if self.eat(T::PathSep) {
                    let rest = self.use_tree()?;
                    Ok(UseTree::Path(seg, self.ast.push_use_tree(rest)))
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
        Ok(PathSeg::new(kind, Ident { lo: t.lo, hi: t.hi }, GenericArgsId::NONE))
    }

    // ------------------------------------------------------------ generics

    fn generics(&mut self) -> PResult<Generics> {
        let mut g = Generics::default();
        if !self.eat_lt() {
            return Ok(g);
        }
        let mut params = Vec::new();
        loop {
            if self.eat_gt() {
                break;
            }
            let attrs = self.outer_attrs()?;
            let attrs = self.ast.push_list(attrs);
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
                let bounds = self.ast.push_list(bounds);
                params.push(GenericParam { attrs, name: Ident { lo: t.lo, hi: t.hi }, kind: GenericParamKind::Lifetime(bounds) });
            } else if self.eat(T::KwConst) {
                let name = self.ident_tok()?;
                self.expect(T::Colon)?;
                let ty = self.ty()?;
                let def = if self.eat(T::Eq) { Some(self.const_arg()?) } else { None };
                params.push(GenericParam { attrs, name, kind: GenericParamKind::Const(ty, def) });
            } else {
                let name = self.ident_tok()?;
                let bounds = if self.eat(T::Colon) { self.bounds()? } else { List::EMPTY };
                let def = if self.eat(T::Eq) { Some(self.ty_plus()?) } else { None };
                params.push(GenericParam { attrs, name, kind: GenericParamKind::Type(bounds, def) });
            }
            if !self.eat(T::Comma) {
                self.expect_gt()?;
                break;
            }
        }
        g.params = self.ast.push_list(params);
        Ok(g)
    }

    fn hrtb(&mut self) -> PResult<List<GenericParam>> {
        if self.kind() == T::KwFor && self.nth(1) == T::Lt {
            self.pos += 1;
            Ok(self.generics()?.params)
        } else {
            Ok(List::EMPTY)
        }
    }

    fn where_clause(&mut self, g: &mut Generics) -> PResult<()> {
        if !self.eat(T::KwWhere) {
            return Ok(());
        }
        let mut preds = self.ast.list(g.where_).to_vec();
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
                    let bs = self.ast.push_list(bs);
                    preds.push(WherePred::Lifetime(Ident { lo: t.lo, hi: t.hi }, bs));
                }
                _ => {
                    let hrtb = self.hrtb()?;
                    let ty = self.ty()?;
                    self.expect(T::Colon)?;
                    let bounds = self.bounds()?;
                    preds.push(WherePred::Bound { hrtb, ty, bounds });
                }
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        g.where_ = self.ast.push_list(preds);
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

    fn bounds(&mut self) -> PResult<List<Bound>> {
        let mut v = Vec::new();
        while self.at_bound_start() {
            v.push(self.bound()?);
            if !self.eat(T::Plus) {
                break;
            }
        }
        Ok(self.ast.push_list(v))
    }

    fn bound(&mut self) -> PResult<Bound> {
        if self.kind() == T::Lifetime {
            let t = self.bump();
            return Ok(Bound::Lifetime(Ident { lo: t.lo, hi: t.hi }));
        }
        if self.kind() == T::KwUse && self.nth(1) == T::Lt {
            self.pos += 1;
            let args = self.generic_args_angle()?;
            return Ok(Bound::Use(args));
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
        let mut qself = QSelfId::NONE;
        let mut global = false;
        let mut segs = Vec::new();
        if self.kind() == T::Lt || self.kind() == T::Shl {
            self.eat_lt();
            let ty = self.ty()?;
            let trait_path = if self.eat(T::KwAs) { Some(self.path(false)?) } else { None };
            self.expect_gt()?;
            qself = QSelfId(self.ast.base[14] + self.ast.qselfs.len() as u32);
            self.ast.qselfs.push(QSelf { ty, trait_path });
            self.expect(T::PathSep)?;
        } else if self.eat(T::PathSep) {
            global = true;
        }
        loop {
            let mut seg = self.path_seg_name()?;
            // generic args
            if expr {
                if self.kind() == T::PathSep && matches!(self.nth(1), T::Lt | T::Shl | T::LArrow) {
                    self.pos += 1;
                    seg.set_args(self.generic_args_angle()?);
                }
            } else {
                if self.kind() == T::PathSep && matches!(self.nth(1), T::Lt | T::Shl | T::LArrow) {
                    self.pos += 1;
                }
                if matches!(self.kind(), T::Lt | T::Shl | T::LArrow) {
                    seg.set_args(self.generic_args_angle()?);
                } else if self.kind() == T::OpenParen && seg.kind() == SegKind::Ident {
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
                    let tys = self.ast.push_list(tys);
                    seg.set_args(self.ast.push_gargs(GenericArgs::Paren(tys, ret)));
                }
            }
            segs.push(seg);
            if self.kind() == T::PathSep && matches!(self.nth(1), T::Ident | T::RawIdent | T::KwSelfValue | T::KwSelfType | T::KwSuper | T::KwCrate) {
                self.pos += 1;
                continue;
            }
            break;
        }
        let segs = self.mk_segs_q(segs, global, qself);
        Ok(Path { segs, lo })
    }

    /// Stores a path's segments in the tree's segment array.
    pub fn mk_segs(&mut self, v: Vec<PathSeg>, global: bool) -> SegList {
        self.mk_segs_q(v, global, QSelfId::NONE)
    }

    /// Stores a path's segments; a qualified path's QSelf id goes in a placeholder segment
    /// just before them (Ast::qself).
    pub fn mk_segs_q(&mut self, v: Vec<PathSeg>, global: bool, qself: QSelfId) -> SegList {
        if qself.is_some() {
            self.ast.path_segs.push(PathSeg::new(SegKind::Ident, Ident::default(), GenericArgsId(qself.0)));
        }
        let lo = self.ast.base[5] + self.ast.path_segs.len() as u32;
        let len = v.len() as u32;
        self.ast.path_segs.extend(v);
        let segs = SegList::new(lo, len, global);
        if qself.is_some() { segs.with_qself() } else { segs }
    }

    fn generic_args_angle(&mut self) -> PResult<GenericArgsId> {
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
                    GenericArg::Binding(name, GenericArgsId::NONE, self.ty_plus()?)
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
                    GenericArg::Binding(name, ga, self.ty_plus()?)
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
        let v = self.ast.push_list(v);
        Ok(self.ast.push_gargs(GenericArgs::Angle(v)))
    }

    fn gat_binding_ahead(&self) -> bool {
        // Ident < ... > =
        let mut i = self.pos + 1;
        let mut depth = 0i32;
        loop {
            match self.toks.at(i).kind {
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
                return self.toks.at(i).kind == T::Eq;
            }
        }
    }

    fn const_arg(&mut self) -> PResult<ExprId> {
        let lo = self.lo();
        match self.kind() {
            T::OpenBrace => {
                let b = self.block()?;
                Ok(self.mk_expr(ExprKind::Block(b, OptIdent::NONE), lo))
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
                    Ty::Tuple(List::EMPTY)
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
                        Ty::Tuple(self.ast.push_list(v))
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
            T::KwFn | T::KwUnsafe | T::KwExtern => self.fn_ptr_ty(List::EMPTY)?,
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
                    Ty::DynTrait(self.ast.push_list(b), false)
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
                    let mlo = self.lo();
                    let path = self.path(true)?;
                    self.expect(T::Not)?;
                    let (delim, toks) = self.delim_tt()?;
                    if let Some(def) = self.rules_macro(&path) {
                        let start = self.expand_call(&def, toks, mlo)?;
                        return self.in_expansion(start, |p| p.ty());
                    }
                    let args = self.mac_args(&path, toks);
                    { let m = self.ast.push_row(MacCall { path, delim, toks, args }); Ty::Mac(m) }
                } else {
                    let path = self.path(false)?;
                    if plus && self.kind() == T::Plus && matches!(self.nth(1), T::Lifetime | T::Ident | T::PathSep | T::Question) {
                        // bare trait object `Trait + Send` (pre-2021)
                        let mut b = vec![Bound::Trait { hrtb: List::EMPTY, maybe: false, konst: false, path }];
                        while self.eat(T::Plus) {
                            if !self.at_bound_start() {
                                break;
                            }
                            b.push(self.bound()?);
                        }
                        Ty::DynTrait(self.ast.push_list(b), false)
                    } else {
                        Ty::Path(path)
                    }
                }
            }
            _ => return self.err("expected type"),
        };
        Ok(self.mk_ty(t))
    }

    fn fn_ptr_ty(&mut self, hrtb: List<GenericParam>) -> PResult<Ty> {
        let unsafe_ = self.eat(T::KwUnsafe);
        let abi = if self.eat(T::KwExtern) { Some(self.abi()) } else { None };
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
        let params = self.ast.push_list(params);
        Ok(Ty::Fn(self.ast.push_fn_ptr(FnPtr { hrtb, unsafe_, abi, params, ret, variadic })))
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
        let v = self.ast.push_list(v);
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
                    Pat::Tuple(self.ast.push_list(v))
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
                Pat::Slice(self.ast.push_list(v))
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
                    let mlo = self.lo();
                    let path = self.path(true)?;
                    self.expect(T::Not)?;
                    let (delim, toks) = self.delim_tt()?;
                    if let Some(def) = self.rules_macro(&path) {
                        let start = self.expand_call(&def, toks, mlo)?;
                        return self.in_expansion(start, |p| p.pat_top());
                    }
                    let args = self.mac_args(&path, toks);
                    { let m = self.ast.push_row(MacCall { path, delim, toks, args }); Pat::Mac(m) }
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
                                Pat::TupleStruct(path, self.ast.push_list(v))
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
            let attrs = self.ast.push_list(attrs);
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
        let fields = self.ast.push_list(fields);
        Ok(Pat::Struct(path, fields, rest))
    }

    // ------------------------------------------------------------ blocks & statements

    pub fn block(&mut self) -> PResult<BlockId> {
        let lo = self.lo();
        self.expect(T::OpenBrace)?;
        // items generated before the block (a signature's anonymous array-length consts) belong
        // to the enclosing item list, not to the block's statements
        let outer_items = std::mem::take(&mut self.extra_items);
        let outer_assoc = std::mem::replace(&mut self.in_assoc, 0);
        let mut attrs = Vec::new();
        self.inner_attrs(&mut attrs)?;
        let mut stmts = Vec::new();
        while self.kind() != T::CloseBrace {
            if self.kind() == T::Eof {
                return self.err("unexpected end of file in block");
            }
            self.block_stmts_step(&mut stmts)?;
        }
        for id in std::mem::take(&mut self.extra_items) {
            stmts.insert(0, Stmt::Item(id));
        }
        self.extra_items = outer_items;
        self.in_assoc = outer_assoc;
        self.pos += 1;
        let stmts = self.ast.push_list(stmts);
        let attrs = self.ast.push_list(attrs);
        self.ast.blocks.push(Block { attrs, stmts, lo, hi: self.prev_hi() });
        Ok(BlockId(self.ast.base[3] + self.ast.blocks.len() as u32 - 1))
    }

    fn stmt(&mut self) -> PResult<Stmt> {
        if self.eat(T::Semi) {
            return Ok(Stmt::Empty);
        }
        let item_tok = self.pos;
        let attrs = self.outer_attrs()?;
        if self.at_item_start() && !attrs.is_empty() && !self.cfg_active(&attrs) {
            self.parse_cfg_out(attrs)?;
            return Ok(Stmt::Empty);
        }
        let s = self.stmt_after_attrs(attrs.clone())?;
        if matches!(s, Stmt::Item(_)) {
            let items = self.hosted_derives(&attrs, item_tok)?;
            self.extra_items.extend(items);
        }
        if let Stmt::Expr(e, semi) = s {
            if !attrs.is_empty() {
                // kept for cfg stripping at load
                let attrs = self.ast.push_list(attrs);
                let r = self.ast.push_row(AttrsStmt { attrs, expr: e, semi });
                return Ok(Stmt::Attrs(r));
            }
        }
        Ok(s)
    }

    fn stmt_after_attrs(&mut self, attrs: Vec<Attr>) -> PResult<Stmt> {
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
            let attrs = self.ast.push_list(attrs);
            let r = self.ast.push_row(LetStmt { attrs, pat, ty, init, else_ });
            return Ok(Stmt::Let(r));
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
                let e = match self.builtin_mac_expr(&path, toks, lo)? {
                    Some(e) => e,
                    None => {
                        let args = self.mac_args(&path, toks);
                        { let m = self.ast.push_row(MacCall { path, delim, toks, args }); self.mk_expr(ExprKind::Mac(m), lo) }
                    }
                };
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
                if !semi && !matches!(self.kind(), T::CloseBrace | T::Eof) {
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
        if matches!(self.kind(), T::CloseBrace | T::Eof) {
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
            self.mk_expr(ExprKind::Range(OptExprId::NONE, OptExprId::from(rhs), incl), lo)
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
                lhs = self.mk_expr(ExprKind::Range(OptExprId::from(Some(lhs)), OptExprId::from(rhs), incl), lo);
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
                            let turbofish = if self.kind() == T::PathSep && matches!(self.nth(1), T::Lt | T::Shl | T::LArrow) {
                                self.pos += 1;
                                self.generic_args_angle()?
                            } else {
                                GenericArgsId::NONE
                            };
                            if self.kind() == T::OpenParen {
                                let args = self.call_args()?;
                                let args = self.ast.push_list(args);
                                let row = self.ast.push_row(MethodCallRow { name, turbofish, args });
                                e = self.mk_expr(ExprKind::MethodCall(e, row), lo);
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
                            let s = self.text_of(t).to_vec();
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
                    let args = self.ast.push_list(args);
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
            if let Some(e) = self.list_elem()? {
                v.push(e);
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseParen)?;
        Ok(v)
    }

    /// An element of an array / tuple / call-argument list: None when a `#[cfg(..)]` on it is false
    /// (the element is removed; Reference expressions.md [expr.attr.restriction] and
    /// conditional-compilation.md [cfg.attr]; rustc_expand config.rs configure_expr).
    fn list_elem(&mut self) -> PResult<Option<ExprId>> {
        let attrs = if self.kind() == T::Pound && self.nth(1) == T::OpenBracket { self.outer_attrs()? } else { Vec::new() };
        let e = self.expr()?;
        Ok(if attrs.is_empty() || self.cfg_present(&attrs) { Some(e) } else { None })
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
            T::Interp => {
                let e = self.interp.get(&(self.pos as u32)).copied();
                self.pos += 1;
                match e {
                    Some(e) => Ok(e),
                    None => self.err("internal error: spliced expression missing"),
                }
            }
            T::Int | T::Float | T::Str | T::ByteStr | T::CStr | T::RawStr | T::Char | T::Byte | T::KwTrue | T::KwFalse => {
                self.lit_expr()
            }
            T::OpenParen => {
                self.pos += 1;
                if self.eat(T::CloseParen) {
                    return Ok(self.mk_expr(ExprKind::Tuple(List::EMPTY), lo));
                }
                let first = self.list_elem()?;
                if let Some(f) = first {
                    if self.eat(T::CloseParen) {
                        return Ok(self.mk_expr(ExprKind::Paren(f), lo));
                    }
                }
                let mut v: Vec<ExprId> = first.into_iter().collect();
                while self.eat(T::Comma) {
                    if self.kind() == T::CloseParen {
                        break;
                    }
                    if let Some(e) = self.list_elem()? {
                        v.push(e);
                    }
                }
                self.expect(T::CloseParen)?;
                let v = self.ast.push_list(v);
                Ok(self.mk_expr(ExprKind::Tuple(v), lo))
            }
            T::OpenBracket => {
                self.pos += 1;
                if self.eat(T::CloseBracket) {
                    return Ok(self.mk_expr(ExprKind::Array(List::EMPTY), lo));
                }
                let first = self.list_elem()?;
                if let Some(f) = first {
                    if self.eat(T::Semi) {
                        let n = self.expr()?;
                        self.expect(T::CloseBracket)?;
                        return Ok(self.mk_expr(ExprKind::Repeat(f, n), lo));
                    }
                }
                let mut v: Vec<ExprId> = first.into_iter().collect();
                while self.eat(T::Comma) {
                    if self.kind() == T::CloseBracket {
                        break;
                    }
                    if let Some(e) = self.list_elem()? {
                        v.push(e);
                    }
                }
                self.expect(T::CloseBracket)?;
                let v = self.ast.push_list(v);
                Ok(self.mk_expr(ExprKind::Array(v), lo))
            }
            T::OpenBrace => {
                let b = self.block()?;
                Ok(self.mk_expr(ExprKind::Block(b, OptIdent::NONE), lo))
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
                        Ok(self.mk_expr(ExprKind::Block(b, OptIdent::from(label)), lo))
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
                    let attrs = self.ast.push_list(attrs);
                    arms.push(Arm { attrs, pat, guard, body });
                }
                self.pos += 1;
                let arms = self.ast.push_list(arms);
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
                Ok(self.mk_expr(ExprKind::Return(OptExprId::from(e)), lo))
            }
            T::KwYield => {
                self.pos += 1;
                let e = if self.can_begin_expr(r) { Some(self.expr_bp(0, r)?) } else { None };
                Ok(self.mk_expr(ExprKind::Yield(OptExprId::from(e)), lo))
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
                Ok(self.mk_expr(ExprKind::Break(OptIdent::from(label), OptExprId::from(e)), lo))
            }
            T::KwContinue => {
                self.pos += 1;
                let label = if self.kind() == T::Lifetime {
                    let t = self.bump();
                    Some(Ident { lo: t.lo, hi: t.hi })
                } else {
                    None
                };
                Ok(self.mk_expr(ExprKind::Continue(OptIdent::from(label)), lo))
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
                    return Ok(self.mk_expr(ExprKind::Async(crate::ast::ASYNC_PLAIN, b), lo));
                }
                if self.kind() == T::KwMove && self.nth(1) == T::OpenBrace {
                    self.pos += 1;
                    let b = self.block()?;
                    return Ok(self.mk_expr(ExprKind::Async(crate::ast::ASYNC_MOVE, b), lo));
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
                    if let Some(def) = self.rules_macro(&path) {
                        let start = self.expand_call(&def, toks, lo)?;
                        return self.in_expansion(start, |p| {
                            let e = p.expr()?;
                            p.eat(T::Semi);
                            Ok(e)
                        });
                    }
                    if let Some(e) = self.builtin_mac_expr(&path, toks, lo)? {
                        return Ok(e);
                    }
                    let args = self.mac_args(&path, toks);
                    return Ok({ let m = self.ast.push_row(MacCall { path, delim, toks, args }); self.mk_expr(ExprKind::Mac(m), lo) });
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
        let mut rest = false;
        while self.kind() != T::CloseBrace {
            let attrs = self.outer_attrs()?;
            let attrs = self.ast.push_list(attrs);
            if self.eat(T::DotDot) {
                if self.kind() != T::CloseBrace {
                    base = Some(self.expr()?);
                } else {
                    rest = true;
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
                let segs = self.mk_segs(vec![PathSeg::new(SegKind::Ident, name, GenericArgsId::NONE)], false);
                let p = Path { segs, lo: t.lo };
                let e = self.mk_expr(ExprKind::Path(p), t.lo);
                fields.push(FieldExpr { attrs, name, expr: e, shorthand: true });
            }
            if !self.eat(T::Comma) {
                break;
            }
        }
        self.expect(T::CloseBrace)?;
        let fields = self.ast.push_list(fields);
        let row = self.ast.push_row(StructRow { path, fields, base: OptExprId::from(base), rest });
        Ok(self.mk_expr(ExprKind::Struct(row), lo))
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
                let attrs = self.ast.push_list(attrs);
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
            (Some(ty), self.mk_expr(ExprKind::Block(b, OptIdent::NONE), blo))
        } else {
            (None, self.expr_bp(0, r)?)
        };
        if is_async {
            // `async |args| -> T { body }`: a closure returning its body as a coroutine that
            // owns the arguments and uses the closure's upvars (rustc async closures); the
            // declared return type is the coroutine's output (`{ let async: T = body; async }`,
            // the `async` token's text naming the binding: no user name can clash)
            let name = Ident { lo, hi: lo + 5 };
            let blo = self.ast.expr(body).lo;
            let stmts = match ret {
                Some(t) => {
                    let pat = self.mk_pat(Pat::Ident { by_ref: false, mutbl: false, name, sub: None });
                    let segs = self.mk_segs(vec![PathSeg::new(SegKind::Ident, name, GenericArgsId::NONE)], false);
                    let path = Path { segs, lo: blo };
                    let tail = self.mk_expr(ExprKind::Path(path), blo);
                    let r = self.ast.push_row(LetStmt { attrs: List::EMPTY, pat, ty: Some(t), init: Some(body), else_: None });
                    vec![Stmt::Let(r), Stmt::Expr(tail, false)]
                }
                None => vec![Stmt::Expr(body, false)],
            };
            let stmts = self.ast.push_list(stmts);
            self.ast.blocks.push(Block { attrs: List::EMPTY, stmts, lo: blo, hi: self.prev_hi() });
            let b = BlockId(self.ast.base[3] + self.ast.blocks.len() as u32 - 1);
            let fut = self.mk_expr(ExprKind::Async(crate::ast::ASYNC_CLOSURE_BODY, b), blo);
            let params = self.ast.push_list(params);
            let row = self.ast.push_row(ClosureRow { is_move, is_async, params, ret: None, body: fut });
            return Ok(self.mk_expr(ExprKind::Closure(row), lo));
        }
        let params = self.ast.push_list(params);
        let row = self.ast.push_row(ClosureRow { is_move, is_async, params, ret, body });
        Ok(self.mk_expr(ExprKind::Closure(row), lo))
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
                Some(self.mk_expr(ExprKind::Block(b, OptIdent::NONE), elo))
            }
        } else {
            None
        };
        Ok(self.mk_expr(ExprKind::If(cond, then, OptExprId::from(els)), lo))
    }

    fn loop_expr(&mut self, label: Option<Ident>, lo: u32) -> PResult<ExprId> {
        self.expect(T::KwLoop)?;
        let b = self.block()?;
        Ok(self.mk_expr(ExprKind::Loop(b, OptIdent::from(label)), lo))
    }
    fn while_expr(&mut self, label: Option<Ident>, lo: u32) -> PResult<ExprId> {
        self.expect(T::KwWhile)?;
        let c = self.expr_ns()?;
        let b = self.block()?;
        let row = self.ast.push_row(WhileRow { cond: c, body: b, label });
        Ok(self.mk_expr(ExprKind::While(row), lo))
    }
    fn for_expr(&mut self, label: Option<Ident>, lo: u32) -> PResult<ExprId> {
        self.expect(T::KwFor)?;
        let p = self.pat_top()?;
        self.expect(T::KwIn)?;
        let it = self.expr_ns()?;
        let b = self.block()?;
        let row = self.ast.push_row(ForRow { pat: p, iter: it, body: b, label });
        Ok(self.mk_expr(ExprKind::For(row), lo))
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
pub fn parse_source<'a>(sess: &'a std::sync::Arc<crate::Session>, src: &'a [u8], edition: u16) -> Result<Parser<'a>, (u32, String)> {
    parse_source_mode(sess, src, edition, false)
}

pub fn parse_source_mode<'a>(sess: &'a std::sync::Arc<crate::Session>, src: &'a [u8], edition: u16, lazy: bool) -> Result<Parser<'a>, (u32, String)> {
    let mut toks = Vec::with_capacity(src.len() / 4);
    crate::lexer::lex(src, edition, &mut toks).map_err(|e| (e.pos, e.msg.to_string()))?;
    let mut p = Parser::new(sess, src, toks, edition);
    p.lazy_bodies = lazy;
    p.parse_file().map_err(|e| (e.pos, e.msg))?;
    Ok(p)
}

/// The origin of a substituted fragment token at `pos` (tok_origin is ascending).
fn origin_of<'t>(tok_origin: impl Into<OSegRef<'t, Maps>>, pos: u32) -> Option<u32> {
    match tok_origin.into().floor_exact(pos) {
        Some((_, (t, o, n))) if pos < t + n => Some(o + (pos - t)),
        _ => None,
    }
}

/// The call site of the expansion whose text contains `pos` (regions ascending, disjoint).
fn region_of<'t>(exp_map: impl Into<OSegRef<'t, Regions>>, pos: u32) -> Option<u32> {
    let (_, r) = exp_map.into().floor(pos)?;
    if pos <= r.1 {
        Some(r.2)
    } else {
        None
    }
}

/// Resolve macro positions before interpreting a source span. A full origin is terminal:
/// its offsets belong to authored bytes, never to another expansion's spelling buffer.
fn source_origin<'a>(mut pos: u32, exact: bool, src_len: u32,
    tables: impl Fn(u32) -> (OSegRef<'a, Maps>, OSegRef<'a, Regions>, OSegRef<'a, Origins>),
) -> Result<SourceSpan, u32> {
    for _ in 0..64 {
        let (to, em, origins) = tables(pos);
        if exact {
            // AST ends (for example Block.hi) are exclusive. If no next token starts here,
            // that boundary still belongs to the preceding token's authored construct.
            let span = origins.floor(pos).map(|(_, o)| o).filter(|o| pos <= o.hi).map(|o| o.source);
            if let Some(span) = span { return Ok(span); }
        }
        if pos < src_len { break; }
        let next = if exact { origin_of(to, pos).or_else(|| region_of(em, pos)) } else { region_of(em, pos) };
        match next { Some(p) if p != pos => pos = p, _ => break }
    }
    Err(pos.min(src_len))
}

impl Parser<'_> {
    pub fn source_span(&self, pos: u32) -> SourceSpan {
        match source_origin(pos, true, self.src.len() as u32, |_| (self.tok_origin.view(), self.exp_map.view(), self.token_origins.view())) {
            Ok(span) => span,
            Err(lo) => {
                let i = self.toks.partition_point(|t| t.lo <= lo);
                let hi = i.checked_sub(1).and_then(|i| self.toks.get(i)).filter(|t| lo < t.hi && t.hi <= self.src.len() as u32).map_or(lo, |t| t.hi);
                SourceSpan { file: 0, lo, hi }
            }
        }
    }

    /// Source spans for nested proc-macro inputs. Ordinary Rust retains its parser offsets
    /// for hygiene; token-file inputs use explicit source IDs throughout the macro bridge.
    pub fn input_span(&self, lo: u32, hi: u32) -> Option<crate::pm_tokens::Span> {
        if !self.token_input && lo < self.src.len() as u32 { return None; }
        let mut span = self.source_span(lo).join(self.source_span(hi.saturating_sub(1).max(lo)));
        // Ordinary Rust keeps parser offsets for hygiene, except tokens authored in
        // an included file: those offsets belong to that file's immutable snapshot.
        if !self.token_input && span.file == 0 { return None; }
        let sources = self.sources.as_ref()?;
        if span.file == 0 { span.file = sources.source_id(&self.bctx.as_ref()?.path); }
        Some(sources.get(span.file).expect("validated token source").token_span(span))
    }

    fn append_origins(&mut self, expansion: &crate::builtins::Expansion, call: u32) {
        let base = (self.src.len() + self.ext.len()) as u32;
        if expansion.origins.is_empty() {
            let source = self.source_span(call);
            self.token_origins.extend(expansion.toks.iter().map(|t| TokenOrigin { lo: base + t.lo, hi: base + t.hi, source }));
            return;
        }
        let mut token = 0;
        for origin in &expansion.origins {
            while token < expansion.toks.len() && expansion.toks[token].hi <= origin.lo { token += 1; }
            let mut source = origin.source;
            if !self.token_input && source.file == 0 {
                // Preserve Rust's identifier hygiene independently of the diagnostic arena.
                if source.hi > source.lo {
                    if let Some(t) = expansion.toks.get(token).filter(|t| matches!(t.kind, T::Ident | T::RawIdent) || (T::KwAs as u8..=T::KwTry as u8).contains(&(t.kind as u8))) {
                        let name_lo = t.lo + if t.kind == T::RawIdent { 2 } else { 0 };
                        self.tok_origin.push((base + name_lo, source.lo, t.hi - name_lo));
                    }
                }
                if source.lo >= self.src.len() as u32 {
                    source = self.source_span(source.lo).join(self.source_span(source.hi.saturating_sub(1).max(source.lo)));
                }
            }
            self.token_origins.push(TokenOrigin { source, ..origin.shifted(base) });
        }
    }
}

/// A local's hygiene context (rustc SyntaxContext for macro_rules): the marks applied, outermost
/// first; mark m is macro_rules expansion m of the file (empty: the root context).
pub type HygCtxt = Vec<u32>;

/// ParsedFile::expansion_kind.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpnKind { Root, LocalRules, ExternalRules, Builtin }

/// A parsed file that owns its source, tokens and tree.
pub struct ParsedFile {
    /// the file's source followed by the text of its macro expansions
    pub src: crate::arena::Arena<u8>,
    pub toks: crate::packed::PackedToks,
    pub ast: Ast,
    pub edition: u16,
    /// expansion text ranges -> call-site position
    pub exp_map: Table<Regions>,
    /// byte offset of each line start (built on the first position query)
    pub line_starts: std::sync::OnceLock<Vec<u32>>,
    /// length of the file's own source (expansion text follows it)
    pub src_len: u32,
    /// Cached path/source identity, shared by file metadata and front-image lookups.
    pub source_hash: std::sync::OnceLock<(String, u128)>,
    /// substituted fragment tokens -> their origin (Parser::tok_origin)
    pub tok_origin: Table<Maps>,
    pub token_origins: Table<Origins>,
    /// Whole-file TokenStream input: diagnostics use authored origins.
    pub token_input: bool,
    pub sources: Option<std::sync::Arc<SourceRegistry>>,
    /// macro_rules expansion regions with their macro's definition position (Parser::rules_exp;
    /// u32::MAX: defined in another file)
    pub rules_exp: Table<Regions>,
    /// macro_rules body tokens -> definition positions (Parser::body_origin)
    pub body_origin: Table<Maps>,
    /// the fn bodies this parse skipped (FnSig::lazy_body) and what parsing them later needs
    pub lazy: FileLazy,
    /// Immutable body segments, published when their owners are demanded.
    pub part: std::sync::OnceLock<Box<FilePart>>,
    /// per item: (an impl's header, how many earlier impls of the file have that header)
    pub impl_heads: std::sync::OnceLock<Box<[(Box<[u8]>, u32)]>>,
    /// Where the arrays above are mapped from (None: they are on the heap).
    pub image: Option<ImageLoc>,
    /// Writes this file's lazily parsed part as an image (ImageSink::part) before it is published.
    pub sink: Option<std::sync::Arc<dyn ImageSink>>,
}

/// A written front image: the compiler's image store (its file name) and the entry's key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ImageLoc {
    pub store: std::sync::Arc<str>,
    pub key: u128,
}

/// Writes final front-end arrays as images and makes them slices of the mapping (the compiler's
/// image store, cache/front_run.rs): a file at its publication (Program, before it is shared), a
/// lazily parsed part before ParsedFile::part publishes it. Both are no-ops when the arrays do not
/// qualify or the write fails (they stay on the heap).
pub trait ImageSink: Send + Sync {
    /// true: `pf`'s arrays are mapped now (`pf.image` set)
    fn file(&self, path: &str, pf: &mut ParsedFile) -> bool;
    /// `part`/`ast`: the file's first lazily parsed part, not yet published
    fn part(&self, pf: &ParsedFile, part: &mut FilePart, ast: &mut Ast);
}

/// A file's LazyCtx (None: the load skipped no body). A restored collected image keeps it
/// encoded until a body parse asks for it; whether there is one needs no decoding.
pub struct FileLazy {
    bytes: Option<&'static [u8]>,
    value: std::sync::OnceLock<Option<Box<LazyCtx>>>,
}
impl Default for FileLazy {
    fn default() -> Self { None.into() }
}
impl From<Option<Box<LazyCtx>>> for FileLazy {
    fn from(v: Option<Box<LazyCtx>>) -> Self { Self { bytes: None, value: std::sync::OnceLock::from(v) } }
}
impl FileLazy {
    /// An encoded `Option<Box<LazyCtx>>` in a checksummed image.
    pub fn mapped(bytes: &'static [u8]) -> Self { Self { bytes: Some(bytes), value: std::sync::OnceLock::new() } }
    pub fn as_ref(&self) -> Option<&LazyCtx> {
        self.value.get_or_init(|| crate::crate_image::decode(self.bytes.expect("mapped LazyCtx"))
            .expect("checksummed collected LazyCtx")).as_deref()
    }
    pub fn is_some(&self) -> bool {
        match (self.value.get(), self.bytes) {
            (Some(v), _) => v.is_some(),
            // Option's encoding: a 1 byte, then the value
            (None, Some(b)) => b.first() == Some(&1),
            (None, None) => false,
        }
    }
    pub fn is_none(&self) -> bool { !self.is_some() }
    pub fn take(&mut self) -> Self { std::mem::take(self) }
    /// The undecoded image bytes, if any (Codec copies them).
    pub fn image(&self) -> Option<&'static [u8]> { self.bytes.filter(|_| self.value.get().is_none()) }
}

/// What parsing a file's skipped fn bodies needs besides the file (ParsedFile::part).
pub struct LazyCtx {
    pub krate: String,
    pub path: String,
    pub module_path: String,
    /// inline `mod` stacks of bodies inside inline modules: (body's first token, names)
    pub mods: Vec<(u32, Vec<String>)>,
    /// the macro table's S2 formatting flag at the load (fmtmac: format family expansion)
    pub s2_fmt: bool,
}

/// A file's lazily parsed fn bodies: their nodes continue the file's arena ids (in
/// `ast.part`), their macro expansions its positions (from `src.len()`) and token indices
/// (from `toks.len()`). The default policy batches ordinary bodies in item order;
/// Rust demand mode and generated bodies append one segment per requested body.
/// Each version owns its IDs; reload matches closures by ordinal within their owner.
pub struct FilePart {
    pub src_base: u32,
    pub tok_base: u32,
    pub rules_base: u32,
    pub rules_exp: Table<Regions>,
    pub body_origin: Table<Maps>,
    pub next: std::sync::OnceLock<Box<FilePart>>,
    pub ext: crate::arena::Arena<u8>,
    pub toks: crate::packed::PackedToks,
    pub exp_map: Table<Regions>,
    pub tok_origin: Table<Maps>,
    pub token_origins: Table<Origins>,
    /// per skipped fn item, in item order: its body block and the body's arena ranges
    /// [expr lo, expr hi, pat lo, pat hi), or the body's parse error
    pub bodies: PartBodies,
    /// Where the part's arrays (and its AST part's) are mapped from (None: on the heap).
    pub image: Option<ImageLoc>,
}

/// FilePart::bodies: a parse's list, or a restored part image's rows
/// [item, block, expr lo, expr hi, pat lo, pat hi] (an image records only parsed bodies).
pub enum PartBodies {
    Parsed(Vec<(u32, BodyResult)>),
    Mapped(&'static [u32]),
}
impl PartBodies {
    pub fn len(&self) -> usize { match self { Self::Parsed(v) => v.len(), Self::Mapped(rows) => rows.len() / 6 } }
    pub fn is_empty(&self) -> bool { self.len() == 0 }
    pub fn get(&self, item: u32) -> Option<BodyResult> {
        match self {
            Self::Parsed(v) => Some(v[v.binary_search_by_key(&item, |b| b.0).ok()?].1.clone()),
            Self::Mapped(rows) => {
                // rows are in item order, like a parse's list
                let (mut lo, mut hi) = (0, rows.len() / 6);
                while lo < hi {
                    let mid = (lo + hi) / 2;
                    let c = &rows[mid * 6..mid * 6 + 6];
                    match c[0].cmp(&item) {
                        std::cmp::Ordering::Less => lo = mid + 1,
                        std::cmp::Ordering::Greater => hi = mid,
                        std::cmp::Ordering::Equal => return Some(Ok((BlockId(c[1]), [c[2], c[3], c[4], c[5]]))),
                    }
                }
                None
            }
        }
    }
    pub fn iter(&self) -> Box<dyn Iterator<Item = (u32, BodyResult)> + '_> {
        match self {
            Self::Parsed(v) => Box::new(v.iter().cloned()),
            Self::Mapped(rows) => Box::new(rows.chunks_exact(6).map(|c| (c[0], Ok((BlockId(c[1]), [c[2], c[3], c[4], c[5]]))))),
        }
    }
}
impl std::fmt::Debug for PartBodies {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result { f.debug_list().entries(self.iter()).finish() }
}

pub type BodyResult = Result<(BlockId, [u32; 4]), String>;

impl ParsedFile {
    /// One file-aware diagnostic lookup. Rust retains macro invocation blame; token-file
    /// input follows copied fragments and full spans, regardless of the file extension.
    pub fn source_span(&self, pos: u32) -> SourceSpan {
        if self.token_input {
            let resolved = source_origin(pos, true, self.src_len, |pos| {
                let (to, em) = self.tables(pos);
                let origins = if pos as usize >= self.src.len() {
                    self.part_at(pos).map_or(&self.token_origins, |p| &p.token_origins)
                } else { &self.token_origins };
                (to.into(), em.into(), origins.into())
            });
            if let Ok(span) = resolved { return span; }
        }
        let (lo, hi) = self.legacy_diagnostic_span(pos);
        SourceSpan { file: 0, lo, hi }
    }

    /// Warm images use private source IDs until publication into their Program's registry.
    pub fn bind_sources(&mut self, path: &str, sources: &std::sync::Arc<SourceRegistry>) -> Result<(), String> {
        let Some(old) = &self.sources else { return Ok(()); };
        if std::sync::Arc::ptr_eq(old, sources) { return Ok(()); }
        let mut ids = crate::fxhash::HashMap::default();
        for origin in self.token_origins.iter().chain(self.parts().flat_map(|p| p.token_origins.iter())) {
            let id = origin.source.file;
            if id == 0 || ids.contains_key(&id) { continue; }
            let src = old.get(id)?;
            ids.insert(id, match &src.src {
                crate::source::SourceBytes::Shared(bytes) => sources.register(&src.path, bytes.clone())?,
                crate::source::SourceBytes::Mapped(bytes) => sources.register_mapped(&src.path, bytes)?,
            });
        }
        self.token_origins = self.token_origins.remap_files(|f| ids[&f]);
        let mut part = self.part.get_mut();
        while let Some(p) = part {
            p.token_origins = p.token_origins.remap_files(|f| ids[&f]);
            part = p.next.get_mut();
        }
        // Include the current source for nested macros in bodies parsed after restoration.
        if self.token_input { sources.register(path, self.src[..self.src_len as usize].into())?; }
        self.sources = Some(sources.clone());
        Ok(())
    }

    pub fn source_hash(&self, path: &str) -> u128 {
        let (cached_path, hash) = self.source_hash.get_or_init(|| (path.to_string(), crate::crate_image::file_hash(path, &self.src[..self.src_len as usize])));
        if cached_path == path { *hash } else { crate::crate_image::file_hash(path, &self.src[..self.src_len as usize]) }
    }

    /// An impl item's header text (up to its first item, whitespace dropped) and its ordinal
    /// among the file's earlier impls with the same header (twins: two `impl Foo` blocks, a
    /// macro's impls). A position-free identity for the impl (stable::def_ident). None: not
    /// an impl of this file.
    pub fn impl_head(&self, item: ItemId) -> Option<(&[u8], u32)> {
        let v = self.impl_heads.get_or_init(|| {
            let mut seen: std::collections::HashMap<Vec<u8>, u32> = std::collections::HashMap::new();
            self.ast.items.iter().map(|it| {
                let ItemKind::Impl(row) = &it.kind else { return (Box::default(), 0) };
                let end = self.ast.list(self.ast.row(*row).items).first().map_or(it.hi, |&c| self.ast.item(c).lo.min(it.hi));
                let head: Vec<u8> = self.bytes(it.lo, end.max(it.lo)).iter().copied().filter(|c| !c.is_ascii_whitespace()).collect();
                let n = seen.entry(head.clone()).or_insert(0);
                *n += 1;
                (head.into_boxed_slice(), *n - 1)
            }).collect()
        });
        let (h, n) = v.get(item.0 as usize)?;
        (!h.is_empty()).then_some((&h[..], *n))
    }

    /// The position in the file's own text a position comes from: macro expansion positions
    /// followed back to their call sites (a position in the text is its own).
    pub fn call_site(&self, mut pos: u32) -> u32 {
        for _ in 0..64 {
            match region_of(self.tables(pos).1, pos) {
                Some(p) if p != pos => pos = p,
                _ => break,
            }
        }
        pos
    }

    /// Parse a file's skipped ordinary bodies together, on the first need of any of them, in
    /// source order: their expression/pattern IDs are a function of the file alone, so IDs that
    /// outlive a run (closure slot keys in the code cache) mean the same expression in the next
    /// one. (Per-item parsing in demand order numbered them differently in every run.)
    /// A file with an image gets its part written and mapped (ImageSink::part) before the part
    /// and its AST are published: readers never hold heap part arrays that later go away.
    pub fn part(&self, cfg: &crate::cfg::CfgSet) -> Option<&FilePart> {
        self.lazy.as_ref()?;
        Some(self.part.get_or_init(|| {
            let (mut fp, mut ast) = parse_part(self, cfg);
            if let (Some(sink), Some(_)) = (&self.sink, &self.image) {
                sink.part(self, &mut fp, &mut ast);
            }
            let mut tail = &self.ast;
            while let Some(next) = tail.part.get() { tail = next; }
            assert!(tail.part.set(Box::new(ast)).is_ok());
            Box::new(fp)
        }))
    }

    /// The body a fn item's load skipped (`cfg`: the file's crate's): its block and its arena
    /// ranges [expr lo, expr hi, pat lo, pat hi). Rust demand mode parses just this item.
    /// None: the item has no skipped body.
    pub fn lazy_body(&self, item: ItemId, cfg: &crate::cfg::CfgSet) -> Option<Result<(BlockId, [u32; 4]), String>> {
        match &self.ast.item(item).kind {
            ItemKind::Fn(sig, None, _) if self.ast.fn_sig(*sig).lazy_body.is_some() => {}
            _ => return None,
        }

        let part = self.part(cfg)?;
        part.bodies.get(item.0)
    }

    /// The text at positions [lo, hi): the file's source and expansions, then its lazily parsed
    /// bodies' expansions.
    pub fn bytes(&self, lo: u32, hi: u32) -> &[u8] {
        let n = self.src.len() as u32;
        if lo >= n {
            if let Some(p) = self.part_at(lo) {
                let (a, b) = ((lo - p.src_base) as usize, (hi - p.src_base) as usize);
                return &p.ext[a.min(p.ext.len())..b.min(p.ext.len())];
            }
        }
        &self.src[(lo as usize).min(self.src.len())..(hi as usize).min(self.src.len())]
    }

    /// Token `i`: the file's, then its lazily parsed bodies' expansion tokens.
    pub fn tok(&self, i: u32) -> Tok {
        match self.toks.get(i as usize) {
            Some(t) => t,
            None => self.parts().find_map(|p| p.toks.get(i.wrapping_sub(p.tok_base) as usize)).unwrap_or(Tok { kind: T::Eof, lo: 0, hi: 0 }),
        }
    }

    /// The source view cfg evaluation reads (cfg::Src), lazily parsed bodies included.
    pub fn src_view(&self) -> crate::cfg::Src<'_> {
        crate::cfg::Src { src: &self.src, plain: &[], ext: &[], parts: Some(self) }
    }

    /// The expansion tables that hold position `pos` (the lazily parsed bodies' own past the
    /// file's text).
    #[inline]
    fn tables(&self, pos: u32) -> (&Table<Maps>, &Table<Regions>) {
        if pos as usize >= self.src.len() {
            if let Some(p) = self.part_at(pos) {
                return (&p.tok_origin, &p.exp_map);
            }
        }
        (&self.tok_origin, &self.exp_map)
    }

    /// Drops the growth slack of every array (a parsed file is never appended to again; the
    /// arrays grew by doubling, ~30% of their capacity was unused).
    pub fn shrink(&mut self) {
        self.src.shrink_to_fit();
        self.ast.shrink();
    }

    /// The hygiene context of an identifier at `pos` (rustc: a token of a macro_rules body gets
    /// the expansion's mark, a substituted fragment keeps its own context, a built-in macro's
    /// output the context of its call). Marks of macros defined by expansions are not chained.
    pub fn hyg_ctxt(&self, pos: u32) -> HygCtxt {
        let mut out = Vec::new();
        self.hyg_ctxt_into(pos, &mut out, 0);
        out
    }

    fn hyg_ctxt_into(&self, mut pos: u32, out: &mut Vec<u32>, depth: u32) {
        if depth > 32 {
            return;
        }
        for _ in 0..64 {
            if pos < self.src_len {
                return;
            }
            let (to, em) = self.tables(pos);
            if let Some(o) = origin_of(to, pos) {
                pos = o;
                continue;
            }
            let part = self.part_at(pos);
            let (rules, bodies, base) = part.map_or((&self.rules_exp, &self.body_origin, 0), |p| (&p.rules_exp, &p.body_origin, p.rules_base));
            if let Some((k, r)) = OSegRef::from(rules).floor(pos) {
                if pos < r.1 {
                    // the token's context in the definition (a macro made by a macro), then
                    // this expansion's mark
                    if let Some(d) = origin_of(bodies, pos) {
                        self.hyg_ctxt_into(d, out, depth + 1);
                    }
                    out.push(base + k as u32);
                    return;
                }
            }
            match region_of(em, pos) {
                Some(c) => pos = c,
                None => return,
            }
        }
    }

    /// Whether a binding (local or label) made at `bpos` is the one a use at `upos` with the
    /// same name sees (rustc hygiene for macro_rules: the binding's context must be the use's
    /// with marks removed only for macros defined, in this file, after the binding).
    pub fn hyg_visible(&self, bpos: u32, upos: u32) -> bool {
        let c = self.hyg_ctxt(upos);
        let bc = self.hyg_ctxt(bpos);
        c.len() >= bc.len()
            && c[..bc.len()] == bc[..]
            && c[bc.len()..].iter().all(|m| {
                let d = self.hyg_def_pos(*m);
                d != u32::MAX && self.pos_before(bpos, d)
            })
    }

    /// Where the token at `pos` was written (rustc: the outer expansion of its span's context):
    /// the file's own text or a fragment substituted from it, the body of a `macro_rules!`
    /// defined in this file, of one defined elsewhere (another crate's: lints are not reported
    /// in it), or the output of a built-in / derive / proc macro (compiler-generated).
    pub fn expansion_kind(&self, mut pos: u32) -> ExpnKind {
        for _ in 0..64 {
            if pos < self.src_len {
                return ExpnKind::Root;
            }
            let (to, _) = self.tables(pos);
            if let Some(o) = origin_of(to, pos) {
                pos = o;
                continue;
            }
            let rules = self.part_at(pos).map_or(&self.rules_exp, |p| &p.rules_exp);
            if let Some((_, r)) = OSegRef::from(rules).floor(pos) {
                if pos < r.1 {
                    return if r.2 != u32::MAX { ExpnKind::LocalRules } else { ExpnKind::ExternalRules };
                }
            }
            return ExpnKind::Builtin;
        }
        ExpnKind::Root
    }

    /// The definition position of mark `m`'s macro (u32::MAX: another file).
    pub fn hyg_def_pos(&self, m: u32) -> u32 {
        self.rules_exp.get(m as usize).or_else(|| self.parts().find_map(|p| p.rules_exp.get(m.wrapping_sub(p.rules_base) as usize))).map_or(u32::MAX, |r| r.2)
    }

    /// Whether position `a` comes before `b` in the program text: positions in the same
    /// expansion compare directly, else the source positions they stand for.
    pub fn pos_before(&self, a: u32, b: u32) -> bool {
        let reg = |p: u32| -> Option<(usize, bool)> {
            if p < self.src_len {
                return None;
            }
            let em = self.tables(p).1;
            OSegRef::from(em).floor(p).filter(|(_, r)| p <= r.1).map(|(k, _)| (k, p as usize >= self.src.len()))
        };
        if reg(a) == reg(b) {
            return a < b;
        }
        self.src_pos(a) < self.src_pos(b)
    }

    /// The source position `pos` stands for (a fragment's origin, else the call site).
    pub fn src_pos(&self, mut pos: u32) -> u32 {
        for _ in 0..64 {
            if pos < self.src_len {
                return pos;
            }
            let (to, em) = self.tables(pos);
            match origin_of(to, pos) {
                Some(o) => pos = o,
                None => match region_of(em, pos) {
                    Some(c) => pos = c,
                    None => return pos,
                },
            }
        }
        pos
    }
    pub fn diagnostic_pos(&self, mut pos: u32) -> u32 {
        if self.token_input { return self.source_span(pos).lo; }
        for _ in 0..64 {
            if pos < self.src_len { break; }
            match region_of(self.tables(pos).1, pos) {
                Some(p) if p != pos => pos = p,
                _ => break,
            }
        }
        pos.min(self.src_len)
    }

    pub fn diagnostic_span(&self, pos: u32) -> (u32, u32) {
        let span = self.source_span(pos);
        (span.lo, span.hi)
    }

    fn legacy_diagnostic_span(&self, pos: u32) -> (u32, u32) {
        let lo = if self.token_input { pos.min(self.src_len) } else { self.diagnostic_pos(pos) };
        // the token holding lo: the last one starting at or before it (tokens are in position
        // order and do not overlap; a scan of every token per lookup made each warm closure
        // identity check linear in its file)
        let i = self.toks.partition_point(|t| t.lo <= lo);
        match i.checked_sub(1).and_then(|i| self.toks.get(i)).filter(|t| lo < t.hi && t.hi <= self.src_len) {
            Some(t) => (lo, t.hi), None => (lo, lo),
        }
    }

    /// Line and column of a position; positions inside macro expansions map to the call site.
    pub fn line_col(&self, pos: u32) -> (usize, usize) {
        if self.token_input {
            let span = self.source_span(pos);
            if span.file != 0 {
                return self.sources.as_ref().expect("token file sources").get(span.file).expect("validated source").line_col(span.lo);
            }
        }
        let pos = self.diagnostic_pos(pos);
        // binary search over line starts (errors are reported by the thousand on a cold graph)
        let starts = self.line_starts.get_or_init(|| {
            let mut v = vec![0u32];
            for (i, &c) in self.src[..self.src_len as usize].iter().enumerate() {
                if c == b'\n' {
                    v.push(i as u32 + 1);
                }
            }
            v
        });
        let pos = pos.min(self.src_len);
        let line = match starts.binary_search(&pos) {
            Ok(i) => i,
            Err(i) => i - 1,
        };
        (line + 1, (pos - starts[line]) as usize + 1)
    }

    #[inline]
    /// An identifier's name: a raw identifier without its `r#` (`r#in` names `in`, as in rustc).
    pub fn text(&self, id: Ident) -> &str {
        let s = std::str::from_utf8(self.bytes(id.lo, id.hi)).unwrap_or("?");
        s.strip_prefix("r#").unwrap_or(s)
    }
    pub fn tok_text(&self, i: u32) -> &str {
        let t = self.tok(i);
        std::str::from_utf8(self.bytes(t.lo, t.hi)).unwrap_or("?")
    }
}

pub fn parse_owned(sess: &std::sync::Arc<crate::Session>, src: Vec<u8>, edition: u16) -> Result<ParsedFile, (u32, String)> {
    parse_owned_ctx(sess, src, None, edition, None, "", None, false).map(|r| r.0)
}

/// Parses `src` (with its tokens when lexed already). With a macro scope, `macro_rules!` and
/// the built-in macros expand (the file context `bctx` is what built-ins need, builtins.rs);
/// the scope's definitions and lookups come back with the file.
pub fn parse_owned_ctx(sess: &std::sync::Arc<crate::Session>, src: Vec<u8>, pre: Option<Vec<Tok>>, edition: u16, macros: Option<crate::macros::MacroScope>, krate: &str, bctx: Option<std::rc::Rc<crate::builtins::FileCtx>>, lazy: bool) -> Result<(ParsedFile, Option<crate::macros::MacroUse>), (u32, String)> {
    parse_owned_input(sess, src, pre, edition, macros, krate, bctx, lazy, None, None).map_err(|(s, m)| (s.lo, m))
}

/// Parse a complete Rust module from tokens, without re-lexing generated Rust text. The
/// registry supplies the original source snapshot for `path` and every nonzero Span.file.
/// Span.file zero refers to `path`. The path's suffix never enables dialect grammar here.
/// Errors already carry authored source spans; Program::parse_diagnostic formats them.
pub fn parse_token_stream(stream: crate::pm_tokens::TokenStream, path: &str, edition: u16,
    macros: Option<crate::macros::MacroScope>, ctx: std::rc::Rc<crate::builtins::FileCtx>,
    sources: std::sync::Arc<SourceRegistry>, lazy: bool,
) -> Result<(ParsedFile, Option<crate::macros::MacroUse>), (SourceSpan, String)> {
    let input = crate::builtins::Expansion::from_stream(&stream, edition).map_err(|m| (SourceSpan::default(), m))?;
    parse_token_input(input, path, edition, macros, ctx, sources, lazy)
}

/// The same entry with the flattened token input retained by the preload scan.
pub fn parse_token_input(input: crate::builtins::Expansion, path: &str, edition: u16,
    macros: Option<crate::macros::MacroScope>, ctx: std::rc::Rc<crate::builtins::FileCtx>,
    sources: std::sync::Arc<SourceRegistry>, lazy: bool,
) -> Result<(ParsedFile, Option<crate::macros::MacroUse>), (SourceSpan, String)> {
    let own = sources.get(sources.source_id(path)).map_err(|m| (SourceSpan::default(), m))?;
    if ctx.path != path { return Err((SourceSpan::default(), "token file context path differs from source path".into())); }
    // Validate once per source, not once per output token. Invalid IDs must not silently
    // become offsets in the current file, including when tokens come from another worker.
    let mut snapshots = crate::fxhash::HashMap::default();
    snapshots.insert(0, own.clone());
    for origin in &input.origins {
        let span = origin.source;
        let source = match snapshots.entry(span.file) {
            std::collections::hash_map::Entry::Occupied(e) => e.into_mut(),
            std::collections::hash_map::Entry::Vacant(e) => e.insert(sources.get(span.file).map_err(|m| (span, m))?),
        };
        if span.lo > span.hi || span.hi as usize > source.src.len() {
            return Err((span, format!("token span outside source snapshot: {}", source.path)));
        }
    }
    parse_owned_input(&ctx.cfg.session, own.src.to_vec(), None, edition, macros, &ctx.krate, Some(ctx.clone()), lazy, Some(input), Some(sources))
}

pub(crate) fn parse_owned_input(sess: &std::sync::Arc<crate::Session>, mut src: Vec<u8>, pre: Option<Vec<Tok>>, edition: u16, macros: Option<crate::macros::MacroScope>, krate: &str, bctx: Option<std::rc::Rc<crate::builtins::FileCtx>>, lazy: bool,
    input: Option<crate::builtins::Expansion>, sources: Option<std::sync::Arc<SourceRegistry>>,
) -> Result<(ParsedFile, Option<crate::macros::MacroUse>), (SourceSpan, String)> {
    let sources = Some(sources.unwrap_or_default());
    let src_len = src.len() as u32;
    let token_input = input.is_some();
    let (toks, ast, ext, exp_map, tok_origin, token_origins, rules_exp, body_origin, used, lazy) = {
        let toks = if token_input { Vec::new() } else { match pre {
            Some(t) => t,
            None => {
                let _timing = crate::parse_work::time(0);
                let mut toks = Vec::with_capacity(src.len() / 4);
                crate::lexer::lex(&src, edition, &mut toks).map_err(|e| (SourceSpan { file: 0, lo: e.pos, hi: e.pos }, e.msg.to_string()))?;
                toks
            }
        }};
        let mut p = Parser::new(sess, &src, toks, edition);
        if let Some(input) = input {
            p.ext = input.text.into_bytes().into();
            p.toks = input.toks.into_iter().map(|t| Tok { kind: t.kind, lo: t.lo + src_len, hi: t.hi + src_len }).collect::<Vec<_>>().into();
            let end = src_len + p.ext.len() as u32;
            p.toks.push(Tok { kind: T::Eof, lo: end, hi: end });
            p.token_origins = input.origins.into_iter().map(|o| o.shifted(src_len)).collect::<Vec<_>>().into();
            // EOF has no spelling, but syntax errors at EOF retain the final authored span.
            let source = p.token_origins.last().map_or(SourceSpan::default(), |o| o.source);
            p.token_origins.push(TokenOrigin { lo: end, hi: end, source });
        }
        p.token_input = token_input;
        p.sources = sources.clone();
        p.macros = macros;
        p.krate = krate.to_string();
        p.lazy_auto = lazy && bctx.is_some() && p.macros.is_some();
        let lazy_ctx = bctx.as_ref().map(|c| (c.krate.clone(), c.path.clone(), c.module_path.clone()));
        p.bctx = bctx;
        let parsed = { let _timing = crate::parse_work::time(1); p.parse_file() };
        if let Err(e) = parsed {
            if token_input { return Err((p.source_span(e.pos), e.msg)); }
            // an error inside an expansion is reported at its call site
            let mut pos = e.pos;
            for _ in 0..64 {
                if pos < src_len { break; }
                let next = region_of(p.exp_map.view(),pos);
                match next { Some(at) if at != pos => pos = at, _ => break }
            }
            return Err((SourceSpan { file: 0, lo: pos, hi: pos }, e.msg));
        }
        let s2_fmt = p.macros.as_ref().map_or(false, |m| m.table.s2_fmt);
        let skipped = p.ast.items.iter().any(|it| matches!(&it.kind, ItemKind::Fn(sig, None, _) if p.ast.fn_sig(*sig).lazy_body.is_some()));
        let lazy = match lazy_ctx {
            Some((krate, path, module_path)) if (p.lazy_auto && skipped) => Some(Box::new(LazyCtx { krate, path, module_path, mods: std::mem::take(&mut p.lazy_mods), s2_fmt })),
            _ => None,
        };
        (p.toks.into_own(), p.ast, p.ext.into_own(), p.exp_map.into_own(), p.tok_origin.into_own(), p.token_origins.into_own(), p.rules_exp.into_own(), p.body_origin.into_own(), p.macros.map(|m| m.finish()), lazy)
    };
    src.reserve_exact(ext.len());
    src.extend_from_slice(&ext);
    let mut pf = ParsedFile { src: src.into(), toks: toks.into(), ast, edition, exp_map: Table::from_rows(&exp_map), line_starts: std::sync::OnceLock::new(), src_len, source_hash: Default::default(), tok_origin: Table::from_rows(&tok_origin), token_origins: Table::from_rows(&token_origins), token_input, sources, rules_exp: Table::from_rows(&rules_exp), body_origin: Table::from_rows(&body_origin), lazy: lazy.into(), part: std::sync::OnceLock::new(), impl_heads: std::sync::OnceLock::new(), image: None, sink: None };
    pf.shrink();
    Ok((pf, used))
}

/// MAKEPAD_RUN_LAZY_STATS=1 (measurement): per fn body parsed, whether a lazy parse could skip it;
/// [n, arena bytes, nested bytes] for skippable bodies, then for the others
pub static LAZY_STATS: [std::sync::atomic::AtomicU64; 16] = [const { std::sync::atomic::AtomicU64::new(0) }; 16];
/// MAKEPAD_RUN_LAZY_STATS: the macros that kept bodies eager (`other macro`), by name
pub static LAZY_MACROS: relax_session::sync::Mutex<std::collections::BTreeMap<String, u64>> = relax_session::sync::Mutex::new(std::collections::BTreeMap::new());
/// MAKEPAD_RUN_LAZY_STATS: what made a file's skipped bodies parse (type-check origin, thread)
pub static LAZY_WHY_PARSED: relax_session::sync::Mutex<std::collections::BTreeMap<String, u64>> = relax_session::sync::Mutex::new(std::collections::BTreeMap::new());
/// why a body is parsed at load (Parser::body_outline)
pub const LAZY_WHY: [&str; 8] = ["skippable", "macro output", "item inside", "macro_rules call", "array length", "fragment", "other macro", "no macro scope"];

fn lazy_stats_on() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var_os("MAKEPAD_RUN_LAZY_STATS").is_some())
}

/// MAKEPAD_RUN_LAZY_BODIES (default on for `makepad run`; 0: off): fn bodies a later parse can do
/// alone are skipped at load and parsed on the first need (ParsedFile::part). Set by the run
/// driver before loading.
pub static LAZY_AUTO: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

pub fn lazy_auto() -> bool {
    LAZY_AUTO.load(std::sync::atomic::Ordering::Relaxed)
}

/// Lazily parsed bodies: files whose skipped bodies were parsed, and those bodies
pub static LAZY_PARSED: [std::sync::atomic::AtomicU64; 2] = [const { std::sync::atomic::AtomicU64::new(0) }; 2];

/// Parses every fn body `pf`'s load skipped (in item order) as the load would have: the same
/// file context, built-in macros only (see Parser::body_outline), then cfg-strips them.
/// The part and its AST (continuing the file's last AST part's ids); the caller publishes both.
pub fn parse_part(pf: &ParsedFile, cfg: &crate::cfg::CfgSet) -> (FilePart, Ast) {
    let sess = &cfg.session;
    let _mem = crate::memstat::tag(crate::memstat::Tag::ParseLazy);
    let ctx = pf.lazy.as_ref().expect("parse_part: the file skipped no body");
    // the file's only part: it continues the file's own arrays
    assert!(pf.part.get().is_none(), "parse_part: the file's bodies are already parsed");
    // as at the load: the file's own text, its expansions as the parser's `ext` (line!/column!
    // and spans of macro output map through them to call sites), its tokens and expansion
    // tables -- all read in place; the parse keeps only what its own expansions append
    let n_src = pf.src_len as usize;
    let mut p = Parser::new(sess, &pf.src[..n_src], &pf.toks, pf.edition);
    p.ext = lazy_bodies::Seg::on(&pf.src[n_src..]);
    p.exp_map = OSeg::on(&pf.exp_map);
    p.tok_origin = OSeg::on(&pf.tok_origin);
    p.token_origins = OSeg::on(&pf.token_origins);
    p.rules_exp = OSeg::on(&pf.rules_exp);
    p.body_origin = OSeg::on(&pf.body_origin);
    let mut table = crate::macros::MacroTable::default();
    table.s2_fmt = ctx.s2_fmt;
    p.macros = Some(crate::macros::MacroScope::new(std::sync::Arc::new(table)));
    p.lazy_part = true;
    // the load's expansion tables (built-ins read call sites through them: line!() in macro
    // output); the part keeps only what its own expansions add
    p.token_input = pf.token_input;
    p.sources = pf.sources.clone();
    let mut ast_tail = &pf.ast;
    while let Some(next) = ast_tail.part.get() { ast_tail = next; }
    p.ast.base = ast_tail.ends().into();
    let (n_ext, n_toks, n_rules) = (pf.src.len() - n_src, pf.toks.len(), pf.rules_exp.len());
    p.krate = ctx.krate.clone();
    p.bctx = Some(std::rc::Rc::new(crate::builtins::FileCtx::new(&ctx.krate, &ctx.path, &ctx.module_path, cfg)));
    let mut bodies = Vec::new();
    let mut roots = Vec::new();
    for (ii, it) in pf.ast.items.iter().enumerate() {
        let r = match &it.kind {
            ItemKind::Fn(sig, None, _) => match pf.ast.fn_sig(*sig).lazy_body {
                Some(r) => r,
                None => continue,
            },
            _ => continue,
        };
        if bodies.is_empty() { crate::why::event("parse-bodies", &ctx.path, 0, "", None); }
        if crate::why::enabled() {
            let name = String::from_utf8_lossy(pf.bytes(it.name.lo, it.name.hi));
            crate::why::event("parse-body", &ctx.path, pf.line_col(it.lo).0, &format!("{name} [item:{ii}]"), None);
        }
        p.mod_stack = ctx.mods.iter().find(|m| m.0 == r.lo).map(|m| m.1.clone()).unwrap_or_default();
        p.pos = r.lo as usize - 1;
        let e0 = p.ast.ends();
        let res = match p.block() {
            Ok(b) => {
                let e1 = p.ast.ends();
                roots.push(b);
                Ok((b, [e0[0], e1[0], e0[1], e1[1]]))
            }
            Err(e) => {
                if p.token_input {
                    let span = p.source_span(e.pos);
                    let sources = p.sources.as_ref().expect("token file sources");
                    let id = if span.file == 0 { sources.source_id(&ctx.path) } else { span.file };
                    let source = sources.get(id).expect("validated source");
                    let (l, c) = source.line_col(span.lo);
                    Err(format!("{}:{}:{}: {}", source.path, l, c, e.msg))
                } else {
                    let (l, c) = pf.line_col(e.pos.min(pf.src_len.saturating_sub(1)));
                    Err(format!("{}:{}:{}: {}", ctx.path, l, c, e.msg))
                }
            }
        };
        bodies.push((ii as u32, res));
    }
    if !p.ast.items.is_empty() {
        // body_outline let an item through: every body of the file fails loudly instead of
        // compiling against an item list that misses it
        let msg = format!("{}: a lazily parsed fn body defined an item (makepad run bug; MAKEPAD_RUN_LAZY_BODIES=0 works around it)", ctx.path);
        for b in bodies.iter_mut() {
            b.1 = Err(msg.clone());
        }
    }
    {
        let view = lazy_bodies::TokView::new(p.src, &p.toks, &p.ext);
        let src = view.src();
        crate::program::strip_cfg_nodes(&mut p.ast, src, cfg);
        for b in roots {
            crate::program::strip_body_block(&mut p.ast, src, cfg, b);
        }
    }
    let mut ast = std::mem::take(&mut p.ast);
    // what the part appended, exactly sized (it is never appended to again)
    fn exact<T>(mut v: Vec<T>) -> Vec<T> { v.shrink_to_fit(); v }
    let (toks, ext) = (exact(p.toks.into_own()), exact(p.ext.into_own()));
    let (exp_map, tok_origin, token_origins, rules_exp, body_origin) = (Table::from_rows(p.exp_map.own()), Table::from_rows(p.tok_origin.own()),
        Table::from_rows(p.token_origins.own()), Table::from_rows(p.rules_exp.own()), Table::from_rows(p.body_origin.own()));
    ast.shrink();
    if lazy_stats_on() {
        let why = crate::clock::TYS_ORIGIN.with(|o| o.get());
        let th = std::thread::current().name().unwrap_or("?").trim_end_matches(char::is_numeric).to_string();
        *LAZY_WHY_PARSED.lock().unwrap().entry(format!("{} on {}", why, th)).or_insert(0) += 1;
    }
    if !bodies.is_empty() && !pf.parts().any(|p| !p.bodies.is_empty()) {
        LAZY_PARSED[0].fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }
    LAZY_PARSED[1].fetch_add(bodies.len() as u64, std::sync::atomic::Ordering::Relaxed);
    (FilePart { src_base: (n_src + n_ext) as u32, tok_base: n_toks as u32, rules_base: n_rules as u32,
        rules_exp, body_origin,
        next: Default::default(), ext: ext.into(), toks: toks.into(), exp_map, tok_origin, token_origins, bodies: PartBodies::Parsed(bodies), image: None }, ast)
}

/// MAKEPAD_RUN_LAZY_MACRO_OUTPUT=0: fn bodies in macro output (derives) stay eager.


#[cfg(test)]
mod diagnostic_tests {
    use super::*;

    #[test]
    fn token_file_parse_errors_keep_authored_file_and_entire_span() {
        use crate::pm_tokens::{TokenStream, TokenTree, Ident, Punct, Spacing, Span};
        let sess = std::sync::Arc::new(crate::Session::default());
        let p = crate::program::Program::new(&sess);
        let path = "/fake/syntax.rs";
        let other = "/fake/other.rs";
        p.sources.register(path, std::sync::Arc::from(&b"authored syntax"[..])).unwrap();
        let id = p.sources.register(other, std::sync::Arc::from(&b"header\n  authored_type\n"[..])).unwrap();
        let span = Span { file: id, lo: 9, hi: 23, ..Span::DUMMY };
        let ctx = std::rc::Rc::new(crate::builtins::FileCtx::new("test", path, "test", &crate::cfg::CfgSet::host(&sess, &[])));
        let stream = TokenStream { trees: vec![
            TokenTree::Ident(Ident::new("struct", Span { lo: 0, hi: 15, ..Span::DUMMY })),
            TokenTree::Punct(Punct { ch: ';', spacing: Spacing::Alone, span }),
        ] };
        let (at, msg) = match parse_token_stream(stream, path, 2021, None, ctx, p.sources.clone(), false) {
            Err(e) => e, Ok(_) => panic!("expected an error"),
        };
        assert_eq!(at, span.into());
        let d = p.parse_diagnostic(path, at, msg).unwrap();
        assert_eq!((d.file.as_str(), d.line, d.col), (other, 2, 3));
        assert_eq!((d.end_line, d.end_col), (3, 1));
    }
    #[test]
    fn nested_macro_parse_error_uses_outer_call() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        let source = b"macro_rules! inner { () => { fn broken() { let = ; } } }\nmacro_rules! outer { () => { inner!(); } }\nouter!();\n".to_vec();
        let call = source.windows(9).rposition(|s| s == b"outer!();").unwrap() as u32;
        let result = parse_owned_ctx(sess, source,None,2021,Some(crate::macros::MacroScope::new(std::sync::Arc::new(crate::macros::MacroTable::default()))),"test",None,false);
        match result { Err((pos,_)) => assert_eq!(pos,call), Ok(_) => panic!("expected a parse error") }
    }
}
