//! The lint `meta_variable_misuse` (rustc_expand mbe/macro_check.rs): the meta-variables of a
//! `macro_rules!` rule are used consistently. A variable the matcher does not bind is unknown; one
//! used at a smaller repetition depth than it is bound at is "still repeating"; one used under a
//! different Kleene operator than it is bound under is a wrong-operator use; a nested macro
//! definition in a transcriber is checked with the variables of the outer rule in scope, and its
//! own matcher may not bind a variable twice. The check works on the definition's own tokens, so
//! it runs for the macros of the checked crates (macros loaded from an image have no tokens).
use crate::lexer::T;
use crate::macros::{close_of, is_open, rep_at, MTok, MacroDef};
use crate::fxhash::HashMap;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Kleene { op: u8 }

#[derive(Clone, Debug)]
enum Tt {
    Token(MTok),
    /// `$name` (in a matcher of a nested macro, `$name:frag` is a meta-variable, a colon, a token)
    MetaVar(u32, String),
    /// `$name:frag` in a top-level matcher
    MetaVarDecl(String),
    Delimited(MTok, Vec<Tt>),
    Sequence(Vec<Tt>, Kleene),
}

/// One finding: where, and the lint's message.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MetaVarLint {
    pub pos: u32,
    pub msg: String,
}

fn ident_like(t: &MTok) -> bool {
    matches!(t.kind, T::Ident | T::RawIdent) || (!t.text.is_empty() && t.text.chars().all(|c| c.is_alphanumeric() || c == '_') && t.kind != T::KwCrate)
}

/// `mbe::quoted::parse`: the token trees of `toks`; `lhs` reads `$x:frag` as a declaration.
fn parse_tts(toks: &[MTok], lhs: bool) -> Vec<Tt> {
    let mut out = Vec::new();
    let mut i = 0;
    while i < toks.len() {
        let t = &toks[i];
        if t.kind == T::Dollar && i + 1 < toks.len() {
            let n = &toks[i + 1];
            if n.kind == T::OpenParen {
                if let Some((lo, hi, _sep, op, next)) = rep_at(toks, i) {
                    out.push(Tt::Sequence(parse_tts(&toks[lo..hi], lhs), Kleene { op }));
                    i = next;
                    continue;
                }
            } else if ident_like(n) {
                if lhs && i + 3 < toks.len() && toks[i + 2].kind == T::Colon && ident_like(&toks[i + 3]) {
                    out.push(Tt::MetaVarDecl(n.text.clone()));
                    i += 4;
                } else {
                    out.push(Tt::MetaVar(t.pos, n.text.clone()));
                    i += 2;
                }
                continue;
            }
        }
        if is_open(t.kind) {
            let c = close_of(toks, i);
            out.push(Tt::Delimited(t.clone(), parse_tts(&toks[i + 1..c], lhs)));
            i = c + 1;
            continue;
        }
        out.push(Tt::Token(t.clone()));
        i += 1;
    }
    out
}

#[derive(Clone)]
struct BinderInfo {
    /// the Kleene operators it is bound under, outermost first
    ops: Vec<Kleene>,
}

type Binders = HashMap<String, BinderInfo>;

/// The state at which a macro definition was entered in the RHS of another.
#[derive(Clone)]
struct MacroState {
    binders: Binders,
    ops: Vec<Kleene>,
}

struct Ck<'a> {
    out: &'a mut Vec<MetaVarLint>,
}

impl Ck<'_> {
    fn lint(&mut self, pos: u32, msg: String) {
        if !self.out.iter().any(|l| l.pos == pos && l.msg == msg) { self.out.push(MetaVarLint { pos, msg }); }
    }

    /// `macros`: the outer macros, innermost last.
    fn check_binders(&mut self, lhs: &Tt, macros: &[MacroState], binders: &mut Binders, ops: &[Kleene]) {
        match lhs {
            Tt::Token(_) => {}
            // only in a nested macro's matcher (the RHS of the outer macro)
            Tt::MetaVar(pos, name) => {
                if macros.is_empty() { return; }
                if let Some(prev) = binders.get(name) {
                    let _ = prev;
                    self.lint(*pos, "duplicate matcher binding".to_string());
                } else if get_binder_info(macros, binders, name).is_none() {
                    binders.insert(name.clone(), BinderInfo { ops: ops.to_vec() });
                } else {
                    self.check_occurrences(lhs, macros, binders, ops);
                }
            }
            // only in a top-level matcher
            Tt::MetaVarDecl(name) => {
                if !macros.is_empty() { return; }
                // (a duplicate there is a hard error, reported by the rule check)
                if get_binder_info(macros, binders, name).is_none() {
                    binders.insert(name.clone(), BinderInfo { ops: ops.to_vec() });
                }
            }
            Tt::Delimited(_, tts) => for tt in tts { self.check_binders(tt, macros, binders, ops); },
            Tt::Sequence(tts, k) => {
                let mut ops = ops.to_vec();
                ops.push(*k);
                for tt in tts { self.check_binders(tt, macros, binders, &ops); }
            }
        }
    }

    fn check_occurrences(&mut self, rhs: &Tt, macros: &[MacroState], binders: &Binders, ops: &[Kleene]) {
        match rhs {
            Tt::Token(_) | Tt::MetaVarDecl(_) => {}
            Tt::MetaVar(pos, name) => self.check_ops_is_prefix(macros, binders, ops, *pos, name),
            Tt::Delimited(_, tts) => self.check_nested_occurrences(tts, macros, binders, ops),
            Tt::Sequence(tts, k) => {
                let mut ops = ops.to_vec();
                ops.push(*k);
                self.check_nested_occurrences(tts, macros, binders, &ops);
            }
        }
    }

    fn check_nested_occurrences(&mut self, tts: &[Tt], macros: &[MacroState], binders: &Binders, ops: &[Kleene]) {
        #[derive(Clone, Copy, PartialEq, Eq)]
        enum S { Empty, MacroRules, MacroRulesBang, MacroRulesBangName, Macro, MacroName, MacroNameParen }
        let mut state = S::Empty;
        let mut nested_macros: Vec<MacroState> = macros.to_vec();
        nested_macros.push(MacroState { binders: binders.clone(), ops: ops.to_vec() });
        let mut nested_binders = Binders::default();
        let is_ident = |tt: &Tt| matches!(tt, Tt::Token(t) if matches!(t.kind, T::Ident) || (t.kind != T::RawIdent && ident_like(t) && !t.text.chars().next().is_some_and(|c| c.is_ascii_digit())));
        for tt in tts {
            match (state, tt) {
                (S::Empty, Tt::Token(t)) if t.kind != T::RawIdent && (t.text == "macro_rules" || t.text == "macro") => {
                    state = if t.text == "macro_rules" { S::MacroRules } else { S::Macro };
                }
                (S::MacroRules, Tt::Token(t)) if t.kind == T::Not => state = S::MacroRulesBang,
                (S::MacroRulesBang, tt) if is_ident(tt) => state = S::MacroRulesBangName,
                (S::MacroRulesBang, Tt::MetaVar(..)) => {
                    state = S::MacroRulesBangName;
                    self.check_occurrences(tt, macros, binders, ops);
                }
                (S::MacroRulesBangName, Tt::Delimited(open, inner)) | (S::MacroName, Tt::Delimited(open, inner)) if open.kind == T::OpenBrace => {
                    let macro_rules = state == S::MacroRulesBangName;
                    state = S::Empty;
                    let rest = self.check_nested_macro(macro_rules, inner, &nested_macros);
                    self.check_nested_occurrences(&inner[rest..], macros, binders, ops);
                }
                (S::Macro, tt) if is_ident(tt) => state = S::MacroName,
                (S::Macro, Tt::MetaVar(..)) => {
                    state = S::MacroName;
                    self.check_occurrences(tt, macros, binders, ops);
                }
                (S::MacroName, Tt::Delimited(open, _)) if open.kind == T::OpenParen => {
                    state = S::MacroNameParen;
                    nested_binders = Binders::default();
                    self.check_binders(tt, &nested_macros, &mut nested_binders, &[]);
                }
                (S::MacroNameParen, Tt::Delimited(open, _)) if open.kind == T::OpenBrace => {
                    state = S::Empty;
                    self.check_occurrences(tt, &nested_macros, &nested_binders, &[]);
                }
                (_, tt) => {
                    state = S::Empty;
                    self.check_occurrences(tt, macros, binders, ops);
                }
            }
        }
    }

    /// The body of a nested macro, as long as it looks like `(LHS) => {RHS}` rules; where the check stopped.
    fn check_nested_macro(&mut self, macro_rules: bool, tts: &[Tt], macros: &[MacroState]) -> usize {
        let n = tts.len();
        let mut i = 0;
        let separator = if macro_rules { T::Semi } else { T::Comma };
        loop {
            if i + 2 >= n || !matches!(tts[i], Tt::Delimited(..)) || !matches!(&tts[i + 1], Tt::Token(t) if t.kind == T::FatArrow) || !matches!(tts[i + 2], Tt::Delimited(..)) { break; }
            let (lhs, rhs) = (&tts[i], &tts[i + 2]);
            let mut binders = Binders::default();
            self.check_binders(lhs, macros, &mut binders, &[]);
            self.check_occurrences(rhs, macros, &binders, &[]);
            i += 3;
            if i == n || !matches!(&tts[i], Tt::Token(t) if t.kind == separator) { break; }
            i += 1;
        }
        i
    }

    fn check_ops_is_prefix(&mut self, macros: &[MacroState], binders: &Binders, ops: &[Kleene], pos: u32, name: &str) {
        let mut states: Vec<MacroState> = macros.to_vec();
        states.push(MacroState { binders: binders.clone(), ops: ops.to_vec() });
        // the operator stacks from the innermost state outwards, until the variable's binder
        let mut acc: Vec<&Vec<Kleene>> = Vec::new();
        for state in states.iter().rev() {
            acc.push(&state.ops);
            if let Some(binder) = state.binders.get(name) {
                let mut occurrence: Vec<Kleene> = Vec::new();
                for ops in acc.iter().rev() { occurrence.extend_from_slice(ops); }
                self.ops_is_prefix(pos, name, &binder.ops, &occurrence);
                return;
            }
        }
        self.lint(pos, format!("unknown macro variable `{name}`"));
    }

    fn ops_is_prefix(&mut self, pos: u32, name: &str, binder_ops: &[Kleene], occurrence_ops: &[Kleene]) {
        for (i, binder) in binder_ops.iter().enumerate() {
            if i >= occurrence_ops.len() {
                self.lint(pos, format!("variable `{name}` is still repeating at this depth"));
                return;
            }
            if occurrence_ops[i].op != binder.op {
                self.lint(pos, "meta-variable repeats with different Kleene operator".to_string());
                return;
            }
        }
    }
}

fn get_binder_info<'a>(macros: &'a [MacroState], binders: &'a Binders, name: &str) -> Option<&'a BinderInfo> {
    binders.get(name).or_else(|| macros.iter().rev().find_map(|s| s.binders.get(name)))
}

/// The findings of `meta_variable_misuse` for the rules of `d` (None: no token input).
pub fn check_meta_variables(d: &MacroDef) -> Option<Vec<MetaVarLint>> {
    let toks = d.input_tokens()?;
    let mut out = Vec::new();
    let mut ck = Ck { out: &mut out };
    let mut i = 0;
    while i < toks.len() {
        if toks[i].kind == T::Semi { i += 1; continue; }
        if !is_open(toks[i].kind) { break; }
        let mc = close_of(toks, i);
        let lhs = Tt::Delimited(toks[i].clone(), parse_tts(&toks[i + 1..mc], true));
        i = mc + 1;
        if i >= toks.len() || toks[i].kind != T::FatArrow { break; }
        i += 1;
        if i >= toks.len() || !is_open(toks[i].kind) { break; }
        let bc = close_of(toks, i);
        let rhs = Tt::Delimited(toks[i].clone(), parse_tts(&toks[i + 1..bc], false));
        i = bc + 1;
        let mut binders = Binders::default();
        ck.check_binders(&lhs, &[], &mut binders, &[]);
        ck.check_occurrences(&rhs, &[], &binders, &[]);
    }
    Some(out)
}
