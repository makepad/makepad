//! Thread-local reload parse measurements; concurrent lazy compilers never contaminate them.
use std::cell::RefCell;
use std::time::Instant;

/// Only the immediately preceding version's successful expansion results. Output tokens
/// have expansion-relative positions; native derive text has no AST arena references.
#[derive(Default, Clone)]
pub struct Reuse {
    expansions: crate::fxhash::HashMap<Vec<u8>, std::sync::Arc<crate::builtins::Expansion>>,
    rules: crate::fxhash::HashMap<Vec<u8>, std::sync::Arc<Rules>>,
}
/// transcription text, fragment-token origins, definition-body-token origins, opaque fragment
/// groups (text offsets of the first token's start and the last token's end)
/// a macro_rules expansion: its text, the tokens over it, token origins, body positions, token-tree groups
pub type Rules = (String, Vec<crate::lexer::Tok>, Vec<(u32, u32, u32)>, Vec<(u32, u32, u32)>, Vec<(u32, u32)>);
struct Reusing {
    previous: std::sync::Arc<Reuse>,
    next: Reuse,
    hits: usize,
}
crate::gen_local! {
    static REUSE: RefCell<Option<Reusing>>;
}
pub struct Session;
impl Session {
    pub fn new(previous: std::sync::Arc<Reuse>) -> Self {
        REUSE.with(|s| {
            assert!(s.borrow().is_none(), "nested reload parse");
            *s.borrow_mut() = Some(Reusing { previous, next: Reuse::default(), hits: 0 });
        });
        Self
    }
    pub fn finish(self) -> std::sync::Arc<Reuse> {
        let s = REUSE.with(|s| s.borrow_mut().take().unwrap());
        if std::env::var_os("MAKEPAD_RUN_RELOAD_TRACE_DETAIL").is_some() {
            eprintln!("reload-parse: expansion reuse {} hits, {} entries", s.hits, s.next.expansions.len() + s.next.rules.len());
        }
        std::sync::Arc::new(s.next)
    }
}
impl Drop for Session {
    fn drop(&mut self) { REUSE.with(|s| *s.borrow_mut() = None); }
}
/// Exact source bytes (including trivia), not a hash or item identity. Context-sensitive
/// expansions additionally include their source position and line/column, module, cfg and
/// file context. A moved hosted input is deliberately a miss: macros can observe spans.
/// External-input builtins (env/include) are never cached.
pub fn key(p: &crate::parser::Parser, kind: &str, lo: u32, hi: u32, contextual: bool) -> Option<Vec<u8>> {
    if crate::macro_reg::declaring() || !REUSE.with(|s| s.borrow().is_some()) { return None; }
    if hi < lo { return None; }
    let base = p.src.len() as u32;
    let lowered = p.token_input && lo >= base && hi as usize <= base as usize + p.ext.len();
    if hi > base && !lowered { return None; } // other generated inputs carry hygiene
    let mut key = format!("{kind}\0{}\0", p.edition).into_bytes();
    if let Some(c) = &p.bctx { key.extend_from_slice(format!("{:?}\0", c.cfg.set).as_bytes()); }
    if contextual {
        let c = p.bctx.as_ref()?;
        let (line, col) = c.byte_line_col(p.src, lo);
        let at = if lowered { 0 } else { lo };
        key.extend_from_slice(format!("{at}:{line}:{col}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}\0",
            c.krate, c.path, c.module_path, p.mod_stack, c.cfg.set,
            c.manifest_dir, c.pkg_name, c.pkg_version).as_bytes());
    }
    key.extend_from_slice(p.bytes(lo, hi));
    if lowered {
        key.push(0);
        for o in p.token_origins.iter().filter(|o| o.lo < hi && o.hi > lo) {
            for v in [o.lo.wrapping_sub(lo), o.hi.wrapping_sub(lo), o.source.file, o.source.lo, o.source.hi] { key.extend_from_slice(&v.to_le_bytes()); }
        }
    }
    Some(key)
}
pub fn key_tokens(p: &crate::parser::Parser, kind: &str, lo: u32, hi: u32, toks: std::ops::Range<usize>, location: (usize, usize)) -> Option<Vec<u8>> {
    if !p.token_input { return key(p, kind, lo, hi, true); }
    if crate::macro_reg::declaring() || !REUSE.with(|s| s.borrow().is_some()) || hi < lo { return None; }
    let c = p.bctx.as_ref()?;
    let sources = p.sources.as_ref()?;
    let own = sources.source_id(&c.path);
    let mut key = format!("{kind}\0{}\0{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{:?}:{location:?}\0", p.edition,
        c.krate, c.path, c.module_path, p.mod_stack, c.cfg.set, c.manifest_dir, c.pkg_name, c.pkg_version).into_bytes();
    // An expansion observes a span only as its source, offsets and line/column
    // (pm_tokens::Span; Span::source_text is None), so those -- not the hash of the whole
    // edited file -- identify it: the same input gives the same output, whose spans read the
    // new snapshot either way. An edit elsewhere in the file that moves none of the item's
    // spans keeps its key.
    let mut file: Option<(u32, std::sync::Arc<crate::source::SourceFile>)> = None;
    let mut span = |key: &mut Vec<u8>, at: u32| -> Option<()> {
        let mut s = p.source_span(at);
        if s.file == 0 { s.file = own; }
        if file.as_ref().is_none_or(|f| f.0 != s.file) {
            key.extend_from_slice(&s.file.to_le_bytes());
            file = Some((s.file, sources.get(s.file).ok()?));
        }
        let src = &file.as_ref()?.1;
        for at in [s.lo, s.hi] {
            let (line, col) = src.line_col(at);
            key.extend_from_slice(&at.to_le_bytes());
            key.extend_from_slice(&(line as u32).to_le_bytes());
            key.extend_from_slice(&(col as u32).to_le_bytes());
        }
        Some(())
    };
    span(&mut key, lo)?;
    span(&mut key, hi.saturating_sub(1).max(lo))?;
    for t in crate::parser::lazy_bodies::TokSeq::slice(&p.toks, toks.start, toks.end)?.iter() {
        span(&mut key, t.lo)?;
        span(&mut key, t.hi.saturating_sub(1).max(t.lo))?;
    }
    key.push(0);
    key.extend_from_slice(p.bytes(lo, hi));
    Some(key)
}
pub fn get(key: &Option<Vec<u8>>) -> Option<crate::builtins::Expansion> {
    let key = key.as_ref()?;
    REUSE.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut()?;
        let x = s.previous.expansions.get(key)?.clone();
        s.hits += 1;
        s.next.expansions.insert(key.clone(), x.clone());
        Some((*x).clone())
    })
}
pub fn put(key: Option<Vec<u8>>, x: &crate::builtins::Expansion) {
    if let Some(key) = key {
        REUSE.with(|s| if let Some(s) = s.borrow_mut().as_mut() {
            s.next.expansions.insert(key, std::sync::Arc::new(x.clone()));
        });
    }
}

pub fn rules_key(p: &crate::parser::Parser, def: &crate::macros::MacroDef, lo: u32, hi: u32) -> Option<Vec<u8>> {
    // Non-token fragments parse into the caller's AST while matching (match_frag).
    // Skipping that work changes arena IDs and can skip nested macro side effects. Only
    // token-only matchers can safely reuse transcription without replaying the match.
    fn token_only(nodes: &[crate::macros::MNode]) -> bool {
        use crate::macros::{MNode, Frag};
        nodes.iter().all(|n| match n {
            MNode::Var(_, f) => matches!(f, Frag::Ident | Frag::Lifetime | Frag::Literal | Frag::Tt | Frag::Block | Frag::Vis),
            MNode::Rep(n, _, _) | MNode::Group(_, _, n) => token_only(n),
            MNode::Tok(_) => true,
        })
    }
    // Token-input transcription keeps buffer-relative provenance (tok_origin/body_origin),
    // which differs between an outline expansion and the loader parse after any body macro.
    if p.token_input || !def.arms.iter().all(|(m, _)| token_only(m)) { return None; }
    let mut k = key(p, "macro_rules", lo, hi, false)?;
    // Transcription observes byte origins, not line numbers or the caller's file name.
    // Nested file!/line! calls still expand afresh when the cached text is parsed.
    let here = p.bctx.as_ref().map_or("", |c| c.path.as_str());
    let local = !def.def_file.is_empty() && def.def_file == here;
    k.extend_from_slice(format!("{lo}:{:?}:{local}", p.krate).as_bytes());
    // Resolve the macro BEFORE looking up its expansion. Definition text AND definition
    // positions participate: an edit or a moved definition invalidates hygiene origins too.
    k.extend_from_slice(format!("{:?}:{:?}:{:?}:{}:{:?}", def.name, def.krate, if local { "" } else { &def.def_file }, def.def_pos, def.arms).as_bytes());
    Some(k)
}
pub fn get_rules(key: &Option<Vec<u8>>) -> Option<Rules> {
    let key = key.as_ref()?;
    REUSE.with(|s| {
        let mut s = s.borrow_mut();
        let s = s.as_mut()?;
        let x = s.previous.rules.get(key)?.clone();
        s.hits += 1;
        s.next.rules.insert(key.clone(), x.clone());
        Some((*x).clone())
    })
}
pub fn put_rules(key: Option<Vec<u8>>, x: &Rules) {
    if let Some(key) = key {
        REUSE.with(|s| if let Some(s) = s.borrow_mut().as_mut() {
            s.next.rules.insert(key, std::sync::Arc::new(x.clone()));
        });
    }
}

pub const SLOTS: usize = 10;
#[derive(Default, Clone, Copy, Debug)]
pub struct Times(pub [f64; SLOTS]);
crate::gen_local! {
    static STATE: RefCell<Option<(Times, [bool; SLOTS])>>;
}
pub fn begin() { STATE.with(|s| *s.borrow_mut() = Some((Times::default(), [false; SLOTS]))); }
pub fn finish() -> Times { STATE.with(|s| s.borrow_mut().take().map(|s| s.0).unwrap_or_default()) }
pub struct Timer(usize, Option<Instant>);
pub fn time(slot: usize) -> Timer {
    let active = STATE.with(|s| {
        let mut s = s.borrow_mut();
        if let Some((_, busy)) = s.as_mut() {
            if !busy[slot] { busy[slot] = true; return true; }
        }
        false
    });
    Timer(slot, active.then(Instant::now))
}
impl Drop for Timer {
    fn drop(&mut self) {
        if let Some(t) = self.1 {
            STATE.with(|s| if let Some((times, busy)) = s.borrow_mut().as_mut() {
                times.0[self.0] += t.elapsed().as_secs_f64() * 1000.;
                busy[self.0] = false;
            });
        }
    }
}
impl Times {
    pub fn print(&self, path: &str) {
        eprintln!("reload-parse: {} lex={:.3} parse_file={:.3} (derive={:.3} builtin/script={:.3} macro_rules={:.3}) prepare_file={:.3} classify={:.3} ms", path, self.0[0], self.0[1], self.0[2], self.0[3], self.0[4], self.0[5], self.0[6]);
        eprintln!("reload-parse: input_tokens={:.3} (indexes={:.3} stream={:.3}) ms", self.0[7], self.0[8], self.0[9]);
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn reuse_context_and_eviction() {
        let sess = &std::sync::Arc::new(crate::Session::default());
        fn send_sync<T: Send + Sync>() {}
        send_sync::<crate::program::ParseSnapshot>();
        let mut table = crate::macros::MacroTable::default();
        table.s2_fmt = true;
        let mut s = crate::program::ParseSnapshot {
            old: 0, path: "/tmp/relax-parse-reuse.rs".into(), edition: 2021,
            kname: "test".into(), mod_path: "test".into(),
            cfg: crate::cfg::CfgSet::host(sess, &[]), macros: std::sync::Arc::new(table),
            reuse: Default::default(), previous: None,
            context: crate::source_context::Context::new("/tmp/relax-parse-reuse.rs", Default::default(), sess),
        };
        let initial = "#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)] struct S { x: u32 }\nmacro_rules! m { ($x:literal) => { $x + 1 }; }\nfn f() -> u32 { m!(2) }\nfn g() -> u32 { 0 }";
        for text in [initial.to_string(), initial.replace("{ 0 }", "{ 1 }"),
            format!("\n\n{initial}"), initial.replace("Debug, Default", "Debug"),
            initial.replace("$x + 1", "$x + 9"),
            initial.replace("$x:literal", "$x:expr"), "fn only() {}".into()] {
            let v = s.parse(text.as_bytes().to_vec()).unwrap();
            let full = s.parse_full(text.as_bytes().to_vec()).unwrap();
            assert_eq!(format!("{:?}", v.pf.ast), format!("{:?}", full.pf.ast));
            assert_eq!(&*v.pf.src, &*full.pf.src);
            assert_eq!(v.pf.tok_origin, full.pf.tok_origin);
            assert_eq!(v.pf.token_origins, full.pf.token_origins);
            assert_eq!(v.pf.body_origin, full.pf.body_origin);
            s.reuse = v.reuse;
        }
        assert!(s.reuse.expansions.is_empty() && s.reuse.rules.is_empty());
        assert!(s.parse(b"fn broken(".to_vec()).is_err());
        // A failed parse must release its thread-local session.
        s.parse(b"fn recovered() {}".to_vec()).unwrap();
    }
}
