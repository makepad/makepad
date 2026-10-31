// ported from N:rustc_resolve/src/late.rs (check_consistent_bindings, fresh_binding)
//! The bindings of one pattern: a name bound twice in it (E0416), a name an or-pattern's
//! alternatives do not all bind (E0408) or bind in different modes (E0409).
use super::*;

/// A binding a pattern makes: its name, where, and the mode (`ref`, `mut`).
#[derive(Clone)]
struct Bound { name: Sym, lo: u32, by_ref: bool, mutbl: bool, hyg: Vec<u32> }

impl Bound { fn same(&self, other: &Bound) -> bool { self.name == other.name && self.hyg == other.hyg } }

impl Fcx<'_> {
    /// Checks the pattern `p` (a whole pattern, not a sub-pattern) once.
    pub(super) fn check_pattern_bindings(&mut self, p: PatId) {
        let mut seen = Vec::new();
        self.collect_bindings(p, &mut seen, true);
    }

    /// Appends the bindings of `p` to `out`; `dup`: report a name `out` already has.
    fn collect_bindings(&mut self, p: PatId, out: &mut Vec<Bound>, dup: bool) {
        let ast = self.ast();
        match ast.pat(p) {
            Pat::Ident { by_ref, mutbl, name, sub } => {
                let is_path = sub.is_none() && !*by_ref && !*mutbl && self.ident_pat_is_path(p);
                if !is_path {
                    let s = self.sym(*name);
                    let hyg = self.prog.files[self.file as usize].hyg_ctxt(name.lo);
                    let this = Bound { name: s, lo: name.lo, by_ref: *by_ref, mutbl: *mutbl, hyg };
                    if dup && out.iter().any(|b| b.same(&this)) {
                        self.err(name.lo, format!("E0416: identifier `{}` is bound more than once in the same pattern", self.text(*name)));
                    }
                    out.push(this);
                }
                if let Some(sp) = sub { self.collect_bindings(*sp, out, dup); }
            }
            Pat::TupleStruct(_, v) | Pat::Tuple(v) | Pat::Slice(v) => for x in ast.list(*v) { self.collect_bindings(*x, out, dup); },
            Pat::Struct(_, fields, _) => for f in ast.list(*fields) { self.collect_bindings(f.pat, out, dup); },
            Pat::Ref(_, x) | Pat::Paren(x) | Pat::Box(x) => self.collect_bindings(*x, out, dup),
            Pat::Or(alts) => {
                let alts: Vec<PatId> = ast.list(*alts).to_vec();
                let mut sets: Vec<Vec<Bound>> = Vec::new();
                for a in &alts {
                    let mut set = Vec::new();
                    self.collect_bindings(*a, &mut set, true);
                    sets.push(set);
                }
                // every alternative binds the names of all of them, in one mode
                let mut union: Vec<Bound> = Vec::new();
                for set in &sets { for b in set { if !union.iter().any(|u| u.same(b)) { union.push(b.clone()); } } }
                for u in &union {
                    for set in &sets {
                        match set.iter().find(|b| b.same(u)) {
                            None => {
                                self.err(u.lo, format!("E0408: variable `{}` is not bound in all patterns", self.prog.syms.str(u.name)));
                                break;
                            }
                            Some(b) if (b.by_ref, b.mutbl) != (u.by_ref, u.mutbl) => {
                                self.err(b.lo, format!("E0409: variable `{}` is bound inconsistently across alternatives separated by `|`", self.prog.syms.str(u.name)));
                                break;
                            }
                            _ => {}
                        }
                    }
                }
                for u in union {
                    if dup && out.iter().any(|b| b.same(&u)) {
                        self.err(u.lo, format!("E0416: identifier `{}` is bound more than once in the same pattern", self.prog.syms.str(u.name)));
                    }
                    out.push(u);
                }
            }
            _ => {}
        }
    }
}
