//! Patterns: tests (reads of discriminants, lengths, values) and bindings; `match`.

use super::bir::*;
use super::build::{strip_paren, Build};
use crate::ast::*;
use crate::program::{DefKind, Prim};
use crate::typeck::Res;
use crate::types::{TyId, TyKind};

impl<'a> Build<'a> {
    fn pat_deref(&self, p: PatId, mut place: Place, mut t: TyId) -> (Place, TyId) {
        if let Some(&n) = self.b.pat_derefs.get(&p.0) {
            for _ in 0..n {
                match self.tx.kind(t) {
                    TyKind::Ref(_, i, _) => {
                        place = place.deref();
                        t = i;
                    }
                    _ => break,
                }
            }
        }
        (place, t)
    }

    fn variant_of_pat(&self, p: PatId) -> u32 {
        match self.b.pat_res.get(&p.0) {
            Some(Res::Def(d)) if self.tx.prog.def(*d).kind == DefKind::Variant => self.tx.prog.def(*d).sub,
            _ => 0,
        }
    }

    fn n_variants(&self, t: TyId) -> usize {
        match self.tx.kind(t) {
            TyKind::Adt(d, _, _) => self.tx.tcx.adts.get(&d).map_or(1, |a| if a.is_enum { a.variants.len() } else { 1 }),
            _ => 1,
        }
    }

    fn read_discr(&mut self, place: Place, fail: Bb) {
        let it = self.tx.tcx.tys.prim(Prim::Isize);
        let d = self.new_local(it, LocalKind::Temp, true, None);
        self.push(StmtKind::StorageLive(d));
        self.assign(Place::local(d), Rvalue::Discriminant(place));
        let ok = self.new_block();
        self.term(TermKind::Switch(Operand::Copy(Place::local(d)), vec![ok, fail]));
        self.switch_to(ok);
        self.push(StmtKind::StorageDead(d));
    }

    fn read_value(&mut self, place: Place, fail: Bb) {
        let bt = self.tx.tcx.tys.bool_;
        let d = self.new_local(bt, LocalKind::Temp, true, None);
        self.push(StmtKind::StorageLive(d));
        self.assign(Place::local(d), Rvalue::Op(Operation::PatternTest, vec![Operand::Copy(place)]));
        let ok = self.new_block();
        self.term(TermKind::Switch(Operand::Copy(Place::local(d)), vec![ok, fail]));
        self.switch_to(ok);
        self.push(StmtKind::StorageDead(d));
    }

    /// The element `i` of a slice pattern with `n` sub-patterns: arrays index from the start
    /// (rustc's move paths of arrays use start offsets), slices from the end after a rest.
    fn slice_elem_proj(&self, t: TyId, rest: Option<usize>, n: usize, i: usize) -> Proj {
        let alen = match self.tx.kind(t) {
            TyKind::Array(_, l) => l.try_to_target_usize(&self.tx.tcx.tys).map(|n| n as u32),
            _ => None,
        };
        match rest {
            Some(r) if i > r => match alen {
                Some(l) => Proj::ConstIndex(l - (n - i) as u32, false, l),
                None => Proj::ConstIndex((n - i) as u32, true, (n - 1) as u32),
            },
            _ => Proj::ConstIndex(i as u32, false, match (alen, rest) {
                (Some(l), _) => l,
                (None, Some(_)) => (n - 1) as u32,
                (None, None) => n as u32,
            }),
        }
    }

    fn seq_indices(&self, v: &[PatId], n: usize) -> Vec<(usize, u32)> {
        let rest = v.iter().position(|p| matches!(self.ast.pat(*p), Pat::Rest));
        let mut out = Vec::new();
        match rest {
            None => {
                for i in 0..v.len() {
                    out.push((i, i as u32));
                }
            }
            Some(r) => {
                for i in 0..r {
                    out.push((i, i as u32));
                }
                let after = v.len() - r - 1;
                for j in 0..after {
                    out.push((r + 1 + j, (n.saturating_sub(after) + j) as u32));
                }
            }
        }
        out
    }

    /// Falls through on match; jumps to `fail` otherwise.
    pub fn test_pat(&mut self, p: PatId, place: Place, t: TyId, fail: Bb) {
        let (place, t) = self.pat_deref(p, place, t);
        let ast = self.ast;
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident { sub, .. } => {
                if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
                    self.test_def(d, place, t, fail);
                    return;
                }
                if let Some(s) = sub {
                    self.test_pat(*s, place, t, fail);
                }
            }
            Pat::Lit(_) => self.read_value(place, fail),
            Pat::Range(a, b, _) => {
                // the bounds are consts (`A::<'a>::X..=A::<'static>::X`): their user types hold
                for e in [a, b].into_iter().flatten() {
                    let e = strip_paren(ast, *e);
                    if let (ExprKind::Path(path), Some(Res::Def(d))) = (&ast.expr(e).kind, self.b.res.get(&e.0).copied()) {
                        if self.tx.prog.def(d).kind == DefKind::AssocConst {
                            let et = self.ty(e);
                            let old = self.span;
                            self.span = self.espan(e);
                            self.const_use(path, e.0, et, d, None);
                            self.span = old;
                        }
                    }
                }
                self.read_value(place, fail)
            }
            Pat::Path(path) => match self.b.pat_res.get(&p.0).copied() {
                Some(Res::Def(d)) => {
                    if self.tx.prog.def(d).kind == DefKind::AssocConst {
                        // a const pattern is compared with the matched place (PartialEq::eq at
                        // one type): its user type's regions meet the place's
                        let old = self.span;
                        if let Some(l) = ast.segs(path.segs).last() {
                            self.span = Span { lo: path.lo, hi: l.name.hi };
                        }
                        self.const_use(path, p.0 | crate::typeck::PAT_KEY, t, d, Some(place.clone()));
                        self.span = old;
                    }
                    self.test_def(d, place, t, fail)
                }
                _ => self.read_value(place, fail),
            },
            Pat::TupleStruct(_, v) => {
                let vi = self.variant_of_pat(p);
                let multi = self.n_variants(t) > 1;
                if multi {
                    self.read_discr(place.clone(), fail);
                }
                let base = if self.tx.is_enum(t) { place.project(Proj::Downcast(vi)) } else { place };
                let n = self.tx.n_fields(t, vi);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                    self.test_pat(ast.list(*v)[pi], base.project(Proj::Field(fi, ft)), ft, fail);
                }
            }
            Pat::Struct(_, fields, _) => {
                let vi = self.variant_of_pat(p);
                let multi = self.n_variants(t) > 1;
                if multi {
                    self.read_discr(place.clone(), fail);
                }
                let base = if self.tx.is_enum(t) { place.project(Proj::Downcast(vi)) } else { place };
                for fp in ast.list(*fields) {
                    let name = { let tx = self.pf.text(fp.name); crate::ast::tuple_index(tx).ok_or(()).map(|n| n | 0x8000_0000).unwrap_or_else(|_| self.tx.prog.syms.get(tx).unwrap_or(u32::MAX)) };
                    if let Some(fi) = self.field_index(t, vi, name) {
                        let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                        self.test_pat(fp.pat, base.project(Proj::Field(fi, ft)), ft, fail);
                    }
                }
            }
            Pat::Tuple(v) => {
                let n = self.tx.n_fields(t, 0);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, 0, fi).unwrap_or(t);
                    self.test_pat(ast.list(*v)[pi], place.project(Proj::Field(fi, ft)), ft, fail);
                }
            }
            Pat::Slice(v) => {
                let (et, is_arr) = match self.tx.kind(t) {
                    TyKind::Array(e, _) => (e, true),
                    TyKind::Slice(e) => (e, false),
                    _ => (t, true),
                };
                let rest = ast.list(*v).iter().position(|x| crate::typeck::is_rest_pat(ast, *x));
                let only_rest = v.len() == 1 && rest.is_some();
                if !is_arr && !only_rest {
                    let ut = self.tx.tcx.tys.usize_;
                    let l = self.new_local(ut, LocalKind::Temp, true, None);
                    self.push(StmtKind::StorageLive(l));
                    self.assign(Place::local(l), Rvalue::Len(place.clone()));
                    let ok = self.new_block();
                    self.term(TermKind::Switch(Operand::Copy(Place::local(l)), vec![ok, fail]));
                    self.switch_to(ok);
                    self.push(StmtKind::StorageDead(l));
                }
                for (i, x) in ast.list(*v).iter().enumerate() {
                    if Some(i) == rest {
                        continue;
                    }
                    let pr = self.slice_elem_proj(t, rest, v.len(), i);
                    self.test_pat(*x, place.project(pr), et, fail);
                }
            }
            Pat::Ref(_, x) | Pat::Box(x) => {
                let inner = self.tx.builtin_deref(t).unwrap_or(t);
                self.test_pat(*x, place.deref(), inner, fail);
            }
            Pat::Paren(x) => self.test_pat(*x, place, t, fail),
            Pat::Or(alts) => {
                let ok = self.new_block();
                let n = alts.len();
                for (i, a) in ast.list(*alts).iter().enumerate() {
                    if i + 1 == n {
                        self.test_pat(*a, place.clone(), t, fail);
                        self.goto(ok);
                    } else {
                        let next = self.new_block();
                        self.test_pat(*a, place.clone(), t, next);
                        self.goto(ok);
                        self.switch_to(next);
                    }
                }
                self.switch_to(ok);
            }
            Pat::Mac(_) => self.unsupported("macro pattern"),
        }
    }

    /// The places a pattern tests (refutable reads) and the sources of its bindings, without
    /// emitting code (rustc FakeBorrowCollector's inputs).
    fn tested_places(&self, p: PatId, place: Place, t: TyId, tested: &mut Vec<Place>, bound: &mut Vec<Place>) {
        self.pattern_places(p, place, t, tested, bound);
    }

    // Visit every alternative to collect the places read by a guard test.
    fn pattern_places(&self, p: PatId, place: Place, t: TyId, tested: &mut Vec<Place>, bound: &mut Vec<Place>) {
        let (place, t) = self.pat_deref(p, place, t);
        let ast = self.ast;
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => {}
            Pat::Ident { sub, .. } => {
                if let Some(Res::Def(d)) = self.b.pat_res.get(&p.0).copied() {
                    match self.tx.prog.def(d).kind {
                        DefKind::Variant if self.n_variants(t) > 1 => tested.push(place),
                        DefKind::Variant | DefKind::Struct => {}
                        _ => tested.push(place),
                    }
                    return;
                }
                bound.push(place.clone());
                if let Some(s) = sub {
                    self.pattern_places(*s, place, t, tested, bound);
                }
            }
            Pat::Lit(_) | Pat::Range(..) => tested.push(place),
            Pat::Path(_) => match self.b.pat_res.get(&p.0).copied() {
                Some(Res::Def(d)) => match self.tx.prog.def(d).kind {
                    DefKind::Variant if self.n_variants(t) > 1 => tested.push(place),
                    DefKind::Variant | DefKind::Struct => {}
                    _ => tested.push(place),
                },
                _ => tested.push(place),
            },
            Pat::TupleStruct(_, v) => {
                let vi = self.variant_of_pat(p);
                if self.n_variants(t) > 1 {
                    tested.push(place.clone());
                }
                let base = if self.tx.is_enum(t) { place.project(Proj::Downcast(vi)) } else { place };
                let n = self.tx.n_fields(t, vi);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                    self.pattern_places(ast.list(*v)[pi], base.project(Proj::Field(fi, ft)), ft, tested, bound);
                }
            }
            Pat::Struct(_, fields, _) => {
                let vi = self.variant_of_pat(p);
                if self.n_variants(t) > 1 {
                    tested.push(place.clone());
                }
                let base = if self.tx.is_enum(t) { place.project(Proj::Downcast(vi)) } else { place };
                for fp in ast.list(*fields) {
                    let name = { let tx = self.pf.text(fp.name); crate::ast::tuple_index(tx).ok_or(()).map(|n| n | 0x8000_0000).unwrap_or_else(|_| self.tx.prog.syms.get(tx).unwrap_or(u32::MAX)) };
                    if let Some(fi) = self.field_index(t, vi, name) {
                        let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                        self.pattern_places(fp.pat, base.project(Proj::Field(fi, ft)), ft, tested, bound);
                    }
                }
            }
            Pat::Tuple(v) => {
                let n = self.tx.n_fields(t, 0);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, 0, fi).unwrap_or(t);
                    self.pattern_places(ast.list(*v)[pi], place.project(Proj::Field(fi, ft)), ft, tested, bound);
                }
            }
            Pat::Slice(v) => {
                let (et, is_arr) = match self.tx.kind(t) {
                    TyKind::Array(e, _) => (e, true),
                    TyKind::Slice(e) => (e, false),
                    _ => (t, true),
                };
                let rest = ast.list(*v).iter().position(|x| crate::typeck::is_rest_pat(ast, *x));
                let only_rest = v.len() == 1 && rest.is_some();
                if !is_arr && !only_rest {
                    tested.push(place.clone());
                }
                for (i, x) in ast.list(*v).iter().enumerate() {
                    if Some(i) == rest {
                        continue;
                    }
                    let pr = self.slice_elem_proj(t, rest, v.len(), i);
                    self.pattern_places(*x, place.project(pr), et, tested, bound);
                }
            }
            Pat::Ref(_, x) | Pat::Box(x) => {
                let inner = self.tx.builtin_deref(t).unwrap_or(t);
                self.pattern_places(*x, place.deref(), inner, tested, bound);
            }
            Pat::Paren(x) => self.pattern_places(*x, place, t, tested, bound),
            Pat::Or(alts) => {
                for a in ast.list(*alts) {
                    self.pattern_places(*a, place.clone(), t, tested, bound);
                }
            }
            Pat::Mac(_) => {}
        }
    }

    /// The bindings of `let PAT: T` get the annotation projected to them (rustc
    /// PatternTypeProjections / LocalDecl.user_ty, mir_build matches/mod.rs visit_primary_bindings:
    /// `.deref()` / `.variant()` / `.leaf()` while descending the pattern): each binding is
    /// exactly the projected user type, a `ref` binding's pointee is.
    pub(super) fn ascribe_pat_bindings(&mut self, pat: PatId, at: crate::ast::TyId, t: TyId) {
        let ann = self.user_ty_index(UserAnn::Ty(at), t);
        let mut path = Vec::new();
        self.pat_user_projs(pat, t, ann, &mut path, 0);
    }

    fn pat_user_projs(&mut self, p: PatId, t: TyId, ann: u32, path: &mut Vec<Proj>, depth: u32) {
        if depth > 24 {
            return;
        }
        let ast = self.ast;
        // match ergonomics: the pattern sees through the references it peels
        let mark = path.len();
        let mut t = t;
        if let Some(&n) = self.b.pat_derefs.get(&p.0) {
            for _ in 0..n {
                match self.tx.kind(t) {
                    TyKind::Ref(_, i, _) => {
                        path.push(Proj::Deref);
                        t = i;
                    }
                    _ => break,
                }
            }
        }
        match ast.pat(p) {
            Pat::Ident { by_ref, sub, .. } => {
                if self.b.pat_res.contains_key(&p.0) {
                    path.truncate(mark);
                    return;
                }
                if let Some(s) = sub {
                    self.pat_user_projs(*s, t, ann, path, depth + 1);
                }
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    if let Some(d) = self.locals.get(&li).cloned() {
                        if d.proj.is_empty() {
                            let by_ref = *by_ref || self.b.pat_bind_ref.contains_key(&p.0);
                            self.out.local_user_tys.push(LocalUserTy { local: d.local, ann, projs: path.clone(), by_ref });
                        }
                    }
                }
            }
            Pat::TupleStruct(_, v) => {
                let vi = self.variant_of_pat(p);
                if self.tx.is_enum(t) {
                    path.push(Proj::Downcast(vi));
                }
                let n = self.tx.n_fields(t, vi);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                    path.push(Proj::Field(fi, ft));
                    self.pat_user_projs(ast.list(*v)[pi], ft, ann, path, depth + 1);
                    path.pop();
                }
            }
            Pat::Struct(_, fields, _) => {
                let vi = self.variant_of_pat(p);
                if self.tx.is_enum(t) {
                    path.push(Proj::Downcast(vi));
                }
                for fp in ast.list(*fields) {
                    let name = { let tx = self.pf.text(fp.name); crate::ast::tuple_index(tx).ok_or(()).map(|n| n | 0x8000_0000).unwrap_or_else(|_| self.tx.prog.syms.get(tx).unwrap_or(u32::MAX)) };
                    if let Some(fi) = self.field_index(t, vi, name) {
                        let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                        path.push(Proj::Field(fi, ft));
                        self.pat_user_projs(fp.pat, ft, ann, path, depth + 1);
                        path.pop();
                    }
                }
            }
            Pat::Tuple(v) => {
                let n = self.tx.n_fields(t, 0);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, 0, fi).unwrap_or(t);
                    path.push(Proj::Field(fi, ft));
                    self.pat_user_projs(ast.list(*v)[pi], ft, ann, path, depth + 1);
                    path.pop();
                }
            }
            Pat::Ref(_, x) | Pat::Box(x) => {
                let inner = self.tx.builtin_deref(t).unwrap_or(t);
                path.push(Proj::Deref);
                self.pat_user_projs(*x, inner, ann, path, depth + 1);
                path.pop();
            }
            Pat::Paren(x) => self.pat_user_projs(*x, t, ann, path, depth + 1),
            // (every alternative binds the same locals; slices have no user type projection here)
            Pat::Or(alts) => {
                if let Some(a) = ast.list(*alts).first() {
                    self.pat_user_projs(*a, t, ann, path, depth + 1);
                }
            }
            _ => {}
        }
        path.truncate(mark);
    }

    fn note_pat_annot(&mut self, path: &Path, place: &Place) {
        let v = Self::path_args(self.ast, path);
        if !v.is_empty() {
            let ty = self.place_ty(place);
            let ann = self.user_ty_index(UserAnn::Path(v), ty);
            self.out.ascriptions.push(Ascription { place: place.clone(), ann, projs: Vec::new() });
        }
    }

    fn test_def(&mut self, d: crate::program::DefId, place: Place, t: TyId, fail: Bb) {
        match self.tx.prog.def(d).kind {
            DefKind::Variant => {
                if self.n_variants(t) > 1 {
                    self.read_discr(place, fail);
                }
            }
            DefKind::Struct => {}
            _ => self.read_value(place, fail),
        }
    }

    /// Binds the pattern's bindings (already declared) from `place`. `guard`: by-value
    /// bindings become shared references (the guard sees bindings by reference, rustc).
    pub fn bind_pat(&mut self, p: PatId, place: Place, t: TyId, guard: bool) {
        self.bind_pat_inner(p, place, t, guard);
    }

    fn bind_pat_inner(&mut self, p: PatId, place: Place, t: TyId, guard: bool) {
        let (place, t) = self.pat_deref(p, place, t);
        let ast = self.ast;
        match ast.pat(p) {
            Pat::Wild | Pat::Rest | Pat::Lit(_) | Pat::Range(..) | Pat::Path(_) | Pat::Mac(_) => {}
            Pat::Ident { by_ref, mutbl, sub, .. } => {
                if self.b.pat_res.contains_key(&p.0) {
                    return;
                }
                // `x @ sub`: the subpattern's bindings first (rustc binds them before the `@`
                // binding, so copies out of a place happen before it is moved)
                if let Some(s) = sub {
                    self.bind_pat_inner(*s, place.clone(), t, guard);
                }
                if let Some(&li) = self.b.pat_local.get(&p.0) {
                    let dflt = self.b.pat_bind_ref.get(&p.0).copied();
                    let by_ref_k = if *by_ref {
                        Some(if *mutbl { 2 } else { 1 })
                    } else {
                        dflt
                    };
                    let target = self.local_place(li);
                    let old = self.span;
                    if let Pat::Ident { name, .. } = ast.pat(p) {
                        self.span = Span { lo: name.lo, hi: name.hi };
                    }
                    match by_ref_k {
                        Some(k) if guard => {
                            // the guard sees the reference binding through a guard copy (rustc:
                            // ref bindings in guards are separate locals); the real binding
                            // after the guard (or-pattern alternatives share the guard copy)
                            let r = self.rv();
                            let bk = if k == 2 { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                            let g = match self.guard_local(li) {
                                Some(g) => g,
                                None => {
                                    let lt = self.b.locals[li as usize].ty;
                                    let g = self.new_local(lt, LocalKind::Temp, true, None);
                                    self.push(StmtKind::StorageLive(g));
                                    self.guard_alias(li, Place::local(g));
                                    let nm = self.tx.prog.syms.str(self.b.locals[li as usize].name).to_string();
                                    self.out.guard_locals.push((g, nm));
                                    g
                                }
                            };
                            self.assign(Place::local(g), Rvalue::Ref(r, bk, place.clone()));
                        }
                        Some(k) => {
                            let r = self.rv();
                            let bk = if k == 2 { BorrowKind::Mut { two_phase: false } } else { BorrowKind::Shared };
                            self.assign(target, Rvalue::Ref(r, bk, place.clone()));
                        }
                        None if guard => {
                            // a guard binding: `&place`, the real binding after the guard
                            let r = self.rv();
                            let g = match self.guard_local(li) {
                                Some(g) => g,
                                None => {
                                    let rt = self.tx.ref_ty(false, t);
                                    let g = self.new_local(rt, LocalKind::Temp, true, None);
                                    self.push(StmtKind::StorageLive(g));
                                    self.guard_alias(li, Place::local(g).deref());
                                    let nm = self.tx.prog.syms.str(self.b.locals[li as usize].name).to_string();
                                    self.out.guard_locals.push((g, nm));
                                    g
                                }
                            };
                            self.assign(Place::local(g), Rvalue::Ref(r, BorrowKind::Shared, place.clone()));
                        }
                        None => {
                            let op = self.operand_of(place.clone());
                            let is_move = matches!(op, Operand::Move(_));
                            self.assign(target, Rvalue::Use(op));
                            if let (true, Some(sp)) = (is_move, self.scrut_span) {
                                let si = self.out.blocks[self.cur as usize].stmts.len().saturating_sub(1) as u32;
                                self.out.move_sites.push(((self.cur, si), sp));
                            }
                        }
                    }
                    self.span = old;
                }
            }
            Pat::TupleStruct(path, v) => {
                if !guard {
                    self.note_pat_annot(path, &place);
                }
                let vi = self.variant_of_pat(p);
                let base = if self.tx.is_enum(t) { place.project(Proj::Downcast(vi)) } else { place };
                let n = self.tx.n_fields(t, vi);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                    self.bind_pat_inner(ast.list(*v)[pi], base.project(Proj::Field(fi, ft)), ft, guard);
                }
            }
            Pat::Struct(path, fields, _) => {
                if !guard {
                    self.note_pat_annot(path, &place);
                }
                let vi = self.variant_of_pat(p);
                let base = if self.tx.is_enum(t) { place.project(Proj::Downcast(vi)) } else { place };
                for fp in ast.list(*fields) {
                    let name = { let tx = self.pf.text(fp.name); crate::ast::tuple_index(tx).ok_or(()).map(|n| n | 0x8000_0000).unwrap_or_else(|_| self.tx.prog.syms.get(tx).unwrap_or(u32::MAX)) };
                    if let Some(fi) = self.field_index(t, vi, name) {
                        let ft = self.tx.field_ty(t, vi, fi).unwrap_or(t);
                        self.bind_pat_inner(fp.pat, base.project(Proj::Field(fi, ft)), ft, guard);
                    }
                }
            }
            Pat::Tuple(v) => {
                let n = self.tx.n_fields(t, 0);
                for (pi, fi) in self.seq_indices(ast.list(*v), n) {
                    let ft = self.tx.field_ty(t, 0, fi).unwrap_or(t);
                    self.bind_pat_inner(ast.list(*v)[pi], place.project(Proj::Field(fi, ft)), ft, guard);
                }
            }
            Pat::Slice(v) => {
                let et = match self.tx.kind(t) {
                    TyKind::Array(e, _) | TyKind::Slice(e) => e,
                    _ => t,
                };
                let rest = ast.list(*v).iter().position(|x| crate::typeck::is_rest_pat(ast, *x));
                for (i, x) in ast.list(*v).iter().enumerate() {
                    if Some(i) == rest {
                        // `rest @ ..`
                        if let Pat::Ident { sub: Some(_), .. } | Pat::Ident { .. } = ast.pat(*x) {
                            let after = v.len() - i - 1;
                            let sub = match self.tx.kind(t) {
                                TyKind::Array(_, l) if l.try_to_target_usize(&self.tx.tcx.tys).is_some() => Proj::Subslice(i as u32, (l.try_to_target_usize(&self.tx.tcx.tys).unwrap() as u32).saturating_sub(after as u32), false),
                                _ => Proj::Subslice(i as u32, after as u32, true),
                            };
                            let sp = place.project(sub);
                            let st = self.b.pty(*x);
                            self.bind_pat_inner(*x, sp, st, guard);
                        }
                        continue;
                    }
                    let pr = self.slice_elem_proj(t, rest, v.len(), i);
                    self.bind_pat_inner(*x, place.project(pr), et, guard);
                }
            }
            Pat::Ref(_, x) | Pat::Box(x) => {
                let inner = self.tx.builtin_deref(t).unwrap_or(t);
                self.bind_pat_inner(*x, place.deref(), inner, guard);
            }
            Pat::Paren(x) => self.bind_pat_inner(*x, place, t, guard),
            Pat::Or(alts) => {
                // the bindings of whichever alternative matched: each alternative binds the same
                // locals; bind through the first that tests (re-test to pick)
                let n = alts.len();
                if n == 1 {
                    self.bind_pat_inner(ast.list(*alts)[0], place, t, guard);
                    return;
                }
                let mut bs = Vec::new();
                self.pat_bindings(p, &mut bs);
                if bs.is_empty() {
                    return;
                }
                let join = self.new_block();
                for (i, a) in ast.list(*alts).iter().enumerate() {
                    if i + 1 == n {
                        self.bind_pat_inner(*a, place.clone(), t, guard);
                        self.goto(join);
                    } else {
                        let next = self.new_block();
                        self.test_pat(*a, place.clone(), t, next);
                        // rustc's false edge between alternatives: every alternative's bindings
                        // are considered, also after an irrefutable one
                        let real = self.new_block();
                        self.term(TermKind::FalseEdge(real, next));
                        self.switch_to(real);
                        self.bind_pat_inner(*a, place.clone(), t, guard);
                        self.goto(join);
                        self.switch_to(next);
                    }
                }
                self.switch_to(join);
            }
        }
    }

    pub fn match_expr(&mut self, dest: Place, x: ExprId, arms: &[Arm]) {
        let t = self.b.ty(x);
        let ast = self.ast;
        let any_mut = arms.iter().any(|a| self.pat_binds_mut_ref(a.pat));
        let place = if self.is_place_expr(x) && !self.b.coerce.contains_key(&x.0) {
            self.as_place(x, any_mut)
        } else {
            let tmp = self.temp_for(t, Some(strip_paren(ast, x)));
            self.into(Place::local(tmp), x);
            Place::local(tmp)
        };
        self.push(StmtKind::PlaceMention(place.clone()));
        let join = self.new_block();
        let dt = self.place_ty(&dest);
        let n = arms.len();
        if n == 0 {
            self.push(StmtKind::FakeRead(FakeReadCause::ForMatchedPlace, place.clone()));
            self.term(TermKind::Unreachable);
            self.dead();
            return;
        }
        // Relax's format_args! expansion binds its argument tuple in a match whose arm builds
        // the Arguments; rustc's lowering extends those temporaries to the enclosing statement
        // (super let), so the arm here is not a temporary scope
        // fake borrows across the guards (rustc collect_fake_borrows): shallow borrows of every
        // place a pattern tests and of the reference prefixes of tested and bound places, taken
        // once before the tests and read at the end of each guard
        let mut fakes: Vec<Local> = Vec::new();
        if arms.iter().any(|a| a.guard.is_some()) {
            let mut tested = Vec::new();
            let mut bound = Vec::new();
            for a in arms {
                self.tested_places(a.pat, place.clone(), t, &mut tested, &mut bound);
            }
            let mut fb: Vec<Place> = Vec::new();
            let add_deref_prefixes = |q: &Place, fb: &mut Vec<Place>| {
                for (i, pr) in q.proj.iter().enumerate() {
                    if *pr == Proj::Deref {
                        let pre = Place { local: q.local, proj: q.proj[..i].to_vec() };
                        if !fb.contains(&pre) {
                            fb.push(pre);
                        }
                    }
                }
            };
            for q in &tested {
                if !fb.contains(q) {
                    fb.push(q.clone());
                }
                add_deref_prefixes(q, &mut fb);
            }
            for q in &bound {
                add_deref_prefixes(q, &mut fb);
            }
            for q in fb {
                let qt = self.place_ty(&q);
                let fr = self.rv();
                let rt = self.tx.ref_ty(false, qt);
                let l = self.new_local(rt, LocalKind::Temp, true, None);
                self.push(StmtKind::StorageLive(l));
                self.assign(Place::local(l), Rvalue::Ref(fr, BorrowKind::Shallow, q));
                fakes.push(l);
            }
        }
        let fmt_expansion = arms.len() == 1 && {
            let mut bs = Vec::new();
            self.pat_bindings(arms[0].pat, &mut bs);
            bs.iter().any(|(_, li)| self.tx.prog.syms.str(self.b.locals[*li as usize].name).starts_with("__relax_a"))
        };
        for (i, arm) in arms.iter().enumerate() {
            let next = self.new_block();
            let sp = self.set_span(arm.body);
            let depth = self.scopes.len();
            // the arm is a terminating scope for its bindings and temporaries (rustc resolve_arm)
            self.push_scope(!fmt_expansion);
            // the tests read the scrutinee: rustc reports conflicts there
            let sv = self.set_span(x);
            self.test_pat(arm.pat, place.clone(), t, next);
            self.span = sv;
            if i + 1 < n {
                // rustc's false edge from a candidate to the next one: the later arms stay
                // reachable for borrowck even when this pattern is irrefutable
                let real = self.new_block();
                self.term(TermKind::FalseEdge(real, next));
                self.switch_to(real);
            }
            self.declare_bindings(arm.pat);
            if let Some(g) = arm.guard {
                let gm = self.guard_mark();
                self.bind_pat(arm.pat, place.clone(), t, true);
                let ok = self.new_block();
                let gfail = self.new_block();
                self.cond(g, ok, gfail, false);
                let glocals = self.guard_locals_since(gm);
                self.switch_to(gfail);
                self.clear_guard_aliases_to(gm);
                self.exit_to(depth);
                self.goto(next);
                self.switch_to(ok);
                // the guard's bindings are read at its end (rustc FakeRead ForGuardBinding)
                for g in glocals {
                    self.push(StmtKind::FakeRead(FakeReadCause::ForGuardBinding, Place::local(g)));
                }
                self.clear_guard_aliases_to(gm);
                for f in &fakes {
                    self.push(StmtKind::FakeRead(FakeReadCause::ForGuardBinding, Place::local(*f)));
                }
            }
            let saved_ss = self.scrut_span.replace({ let xe = self.ast.expr(x); Span { lo: xe.lo, hi: xe.hi } });
            self.bind_pat(arm.pat, place.clone(), t, false);
            self.scrut_span = saved_ss;
            self.into_coerced_rb(dest.clone(), arm.body, dt, true);
            self.pop_scope();
            self.goto(join);
            self.switch_to(next);
            if i + 1 == n {
                self.term(TermKind::Unreachable);
            }
            self.span = sp;
        }
        self.switch_to(join);
    }
}
