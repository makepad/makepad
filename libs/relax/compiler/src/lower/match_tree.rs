//! Decision trees for `match` (rustc_mir_build/src/builder/matches: the candidates of the arms are
//! sorted by one test at a time; a test is made once and every arm it rules out is left behind).
//!
//! `lower_match` tests the arms one after the other (`test_pat` per arm, each re-reading and
//! re-decoding the scrutinee); for a match of several arms over integers, `char`s, `bool`s and
//! enum variants this module builds the dispatch instead. A row is an arm (or one alternative of
//! an arm's or-pattern) with the obligations its pattern puts on parts of the scrutinee; the tree
//! picks the first obligation of the first row, reads that part once, and sends each outcome (a
//! range of the integer, a variant of the enum) to the rows that can still match. The tests are
//! emitted as chains of range compares of the one value; switch lowering (codegen/src/switch.rs)
//! turns those into jump tables, bit tests or a binary search.
//!
//! Only the dispatch is new: an arm still binds its names, runs its guard and its body through the
//! code `lower_match` always emitted. Patterns the tree cannot decide (constants, strings, slices,
//! or-patterns that bind) mark their row opaque: the tree sends it to the arm's full entry, which
//! tests the whole pattern as before. A failed guard continues with the arms after it, tested one
//! by one as before; so do the opaque rows.

use super::*;
use std::collections::VecDeque;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(super) enum PKey {
    Mem(u32, i32),
    Regs(Vec<u32>),
}

fn pkey(p: &Place) -> PKey {
    match p {
        Place::Mem(b, o) => PKey::Mem(b.0, *o),
        Place::Regs(r) => PKey::Regs(r.iter().map(|v| v.0).collect()),
    }
}

/// A part of the scrutinee a pattern still has to be matched against.
#[derive(Clone)]
struct Ob {
    place: Place,
    ty: TyId,
    pat: PatId,
}

/// What a pattern asks of one place that a test decides.
#[derive(Clone)]
enum Atom {
    /// the integer lies in one of these inclusive ranges, in increasing order
    Ints(Vec<(i128, i128)>),
    /// the enum value is variant `vi`; `pat` is the pattern (for the fields' obligations)
    Variant { vi: u32, pat: PatId },
}

#[derive(Clone)]
struct DRow {
    id: u32,
    arm: usize,
    /// the arm's own entry must test its whole pattern
    opaque: bool,
    /// the variant payload of this row's pattern, to become obligations when the row is normalized
    expand: Option<(Place, TyId, u32, PatId)>,
    pending: VecDeque<Ob>,
    atoms: Vec<(PKey, Place, TyId, Atom)>,
}

/// The state of one match's tree: the arms' entry blocks and what the tree reaches.
pub(super) struct Tree {
    /// per live arm: its full entry (tests the whole pattern, then goes to `post`)
    pub e: Vec<u32>,
    /// per live arm: the code after its pattern tests (bindings, guard, body)
    pub post: Vec<u32>,
    pub used_e: Vec<bool>,
    pub used_post: Vec<bool>,
    next_id: u32,
    nodes: u32,
    failed: bool,
    /// places reached through references, per normalization (see `normalize`)
    derefs: HashMap<(PKey, u32), (Place, TyId)>,
}

const MAX_NODES: u32 = 600;
const MAX_ROWS: usize = 400;

impl Tree {
    pub(super) fn new(e: Vec<u32>, post: Vec<u32>) -> Tree {
        let n = e.len();
        Tree { e, post, used_e: vec![false; n], used_post: vec![false; n], next_id: 0, nodes: 0, failed: false, derefs: HashMap::default() }
    }
}

impl<'a> Lcx<'a> {
    /// Builds the dispatch of a match over `place: pt` with `n` live arms into a fresh block.
    /// `forced[k]`: arm k is entered at its full entry only. None when the tree is not worth it
    /// or cannot be built (the caller emits the arms in order).
    pub(super) fn match_tree(&mut self, pats: &[PatId], forced: &[bool], tree: &mut Tree, place: &Place, pt: TyId) -> Option<u32> {
        let mut rows = Vec::new();
        for (k, p) in pats.iter().enumerate() {
            let id = tree.next_id;
            tree.next_id += 1;
            let mut pending = VecDeque::new();
            pending.push_back(Ob { place: place.clone(), ty: pt, pat: *p });
            rows.push(DRow { id, arm: k, opaque: forced[k], expand: None, pending, atoms: Vec::new() });
        }
        let root = self.f.block();
        let saved = self.cur;
        self.node(tree, root, rows);
        self.switch(saved);
        if tree.failed {
            return None;
        }
        Some(root)
    }

    /// Normalizes `rows` at the current block: every pending obligation becomes an atom, is dropped
    /// (matches anything), makes the row opaque, or turns into more obligations; or-patterns that
    /// do not bind split their row.
    fn dt_normalize(&mut self, tree: &mut Tree, rows: Vec<DRow>) -> Vec<DRow> {
        tree.derefs.clear();
        let mut out = Vec::new();
        let mut work: Vec<DRow> = rows.into_iter().rev().collect();
        while let Some(mut row) = work.pop() {
            if out.len() + work.len() > MAX_ROWS {
                tree.failed = true;
                return out;
            }
            if let Some((place, ty, vi, pat)) = row.expand.take() {
                let obs = self.dt_variant_fields(&place, ty, vi, pat);
                for o in obs.into_iter().rev() {
                    row.pending.push_front(o);
                }
            }
            let mut dead = false;
            let mut split: Option<(Place, TyId, Vec<PatId>)> = None;
            while let Some(ob) = row.pending.pop_front() {
                match self.dt_classify(tree, &ob) {
                    Cl::Drop => {}
                    Cl::Opaque => row.opaque = true,
                    Cl::Expand(obs) => {
                        for o in obs.into_iter().rev() {
                            row.pending.push_front(o);
                        }
                    }
                    Cl::Atom(place, ty, atom) => {
                        if let Atom::Ints(r) = &atom {
                            if r.is_empty() {
                                dead = true;
                                break;
                            }
                        }
                        row.atoms.push((pkey(&place), place, ty, atom));
                    }
                    Cl::Alts(place, ty, alts) => {
                        split = Some((place, ty, alts));
                        break;
                    }
                }
            }
            if dead {
                continue;
            }
            if let Some((place, ty, alts)) = split {
                // one row per alternative, in order, each holding the rest of the row
                let mut rs = Vec::new();
                for a in alts {
                    let id = tree.next_id;
                    tree.next_id += 1;
                    let mut r = row.clone();
                    r.id = id;
                    r.pending.push_front(Ob { place: place.clone(), ty, pat: a });
                    rs.push(r);
                }
                for r in rs.into_iter().rev() {
                    work.push(r);
                }
                continue;
            }
            out.push(row);
        }
        out
    }

    /// The fields of variant `vi` of the pattern `pat` (a tuple-struct or struct pattern) as obligations.
    fn dt_variant_fields(&mut self, place: &Place, ty: TyId, vi: u32, pat: PatId) -> Vec<Ob> {
        let ast = self.ast();
        let mut out = Vec::new();
        match ast.pat(pat) {
            Pat::TupleStruct(_, v) => {
                let n = self.variant_len(ty, vi);
                let idx = seq_indices(ast, ast.list(*v), n);
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(place, ty, vi, fi);
                    out.push(Ob { place: fp, ty: ft, pat: ast.list(*v)[pi] });
                }
            }
            Pat::Struct(_, fields, _) => {
                for fp in ast.list(*fields) {
                    let fname = {
                        let tx = self.ident_text(fp.name);
                        match crate::ast::tuple_index(tx).ok_or(()) {
                            Ok(n) => n | 0x8000_0000,
                            Err(_) => self.u.prog.syms.get(tx).unwrap_or(u32::MAX),
                        }
                    };
                    if let Some(fi) = self.field_index(ty, vi, fname) {
                        let (pl, ft) = self.project(place, ty, vi, fi);
                        out.push(Ob { place: pl, ty: ft, pat: fp.pat });
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// An integer-like type the tree switches on: its (min, max, signed).
    fn dt_domain(&self, ty: TyId) -> Option<(i128, i128, bool)> {
        match self.kind(ty) {
            TyKind::Bool => Some((0, 1, false)),
            TyKind::Char => Some((0, 0x10_ffff, false)),
            TyKind::Int(p) if !matches!(p, Prim::I128 | Prim::U128) => {
                let it = self.int_ty(ty);
                let bits = it.bits as u32;
                if it.signed {
                    Some((-(1i128 << (bits - 1)), (1i128 << (bits - 1)) - 1, true))
                } else {
                    Some((0, (1i128 << bits) - 1, false))
                }
            }
            _ => None,
        }
    }

    /// The value of a literal / constant pattern expression of integer-like type `ty`.
    fn dt_const_int(&mut self, e: ExprId, ty: TyId) -> Option<i128> {
        if let ExprKind::Unary(UnOp::Neg, inner) = self.ast().expr(e).kind {
            return self.dt_const_int(inner, ty).map(|v| -v);
        }
        let v = self.lower_expr_coerced(e, ty);
        let Val::L(l) = v else { return None };
        if l.len() != 1 {
            return None;
        }
        let k = match self.f.blocks[self.cur as usize].insts.last() {
            Some(Inst::Iconst(d, k)) if *d == l[0] => *k,
            _ => return None,
        };
        let (_, _, signed) = self.dt_domain(ty)?;
        Some(if signed { k as i128 } else { (k as u64) as i128 })
    }

    /// The place a pattern with `n` recorded reference peels sees, reading each pointer once per
    /// normalization (rows of one block share it: they test the same place).
    fn dt_deref(&mut self, tree: &mut Tree, p: PatId, place: &Place, ty: TyId) -> (Place, TyId) {
        let n = self.b.pat_derefs.get(&p.0).copied().unwrap_or(0);
        if n == 0 {
            return (place.clone(), ty);
        }
        let key = (pkey(place), n as u32);
        if let Some(r) = tree.derefs.get(&key) {
            return r.clone();
        }
        let r = self.pat_deref(p, place, ty);
        tree.derefs.insert(key, r.clone());
        r
    }

    fn dt_classify(&mut self, tree: &mut Tree, ob: &Ob) -> Cl {
        let ast = self.ast();
        let (place, ty) = self.dt_deref(tree, ob.pat, &ob.place, ob.ty);
        let p = ob.pat;
        match ast.pat(p) {
            Pat::Wild | Pat::Rest => Cl::Drop,
            Pat::Paren(x) => Cl::Expand(vec![Ob { place, ty, pat: *x }]),
            Pat::Ident { sub, .. } => match self.b.pat_res.get(&p.0).copied() {
                Some(Res::Def(d)) => self.dt_def(&place, ty, p, d),
                Some(_) => Cl::Opaque,
                None => match sub {
                    Some(s) => Cl::Expand(vec![Ob { place, ty, pat: *s }]),
                    None => Cl::Drop,
                },
            },
            Pat::Path(_) => match self.b.pat_res.get(&p.0).copied() {
                Some(Res::Def(d)) => self.dt_def(&place, ty, p, d),
                _ => Cl::Opaque,
            },
            Pat::Lit(e) => {
                let Some((lo, hi, _)) = self.dt_domain(ty) else { return Cl::Opaque };
                let Some(v) = self.dt_const_int(*e, ty) else { return Cl::Opaque };
                if v < lo || v > hi {
                    return Cl::Opaque;
                }
                Cl::Atom(place, ty, Atom::Ints(vec![(v, v)]))
            }
            Pat::Range(a, b, incl) => {
                let Some((dmin, dmax, _)) = self.dt_domain(ty) else { return Cl::Opaque };
                let lo = match a {
                    Some(a) => match self.dt_const_int(*a, ty) {
                        Some(v) => v,
                        None => return Cl::Opaque,
                    },
                    None => dmin,
                };
                let hi = match b {
                    Some(b) => match self.dt_const_int(*b, ty) {
                        Some(v) => {
                            if *incl {
                                v
                            } else {
                                v - 1
                            }
                        }
                        None => return Cl::Opaque,
                    },
                    None => dmax,
                };
                if lo < dmin || hi > dmax {
                    return Cl::Opaque;
                }
                Cl::Atom(place, ty, Atom::Ints(if lo <= hi { vec![(lo, hi)] } else { Vec::new() }))
            }
            Pat::Tuple(v) => {
                let n = match self.kind(ty) {
                    TyKind::Tuple(x) => self.u.tcx.tys.list(x).len(),
                    _ => return Cl::Opaque,
                };
                let idx = seq_indices(ast, ast.list(*v), n);
                let mut obs = Vec::new();
                for (pi, fi) in idx {
                    let (fp, ft) = self.project(&place, ty, 0, fi);
                    obs.push(Ob { place: fp, ty: ft, pat: ast.list(*v)[pi] });
                }
                Cl::Expand(obs)
            }
            Pat::TupleStruct(..) | Pat::Struct(..) => {
                let TyKind::Adt(d, _, _) = self.kind(ty) else { return Cl::Opaque };
                let Some(adt) = self.u.tcx.adts.get(&d) else { return Cl::Opaque };
                if adt.is_union {
                    return Cl::Opaque;
                }
                let vi = self.variant_of_pat(p);
                if adt.variants.len() <= 1 {
                    Cl::Expand(self.dt_variant_fields(&place, ty, vi, p))
                } else if self.dt_tag_readable(ty) {
                    Cl::Atom(place, ty, Atom::Variant { vi, pat: p })
                } else {
                    Cl::Opaque
                }
            }
            Pat::Or(alts) => {
                let mut bound = Vec::new();
                self.pat_binding_locals(p, &mut bound);
                if !bound.is_empty() {
                    return Cl::Opaque;
                }
                // alternatives that are all integer sets of this place merge into one set
                let mut merged: Vec<(i128, i128)> = Vec::new();
                let mut all_ints = true;
                for a in ast.list(*alts) {
                    let sub = Ob { place: place.clone(), ty, pat: *a };
                    match self.dt_classify(tree, &sub) {
                        Cl::Atom(pl, _, Atom::Ints(r)) if pkey(&pl) == pkey(&place) => merged.extend(r),
                        Cl::Drop => {
                            // an alternative that matches anything: the or-pattern does
                            return Cl::Drop;
                        }
                        _ => {
                            all_ints = false;
                            break;
                        }
                    }
                }
                if all_ints {
                    merged.sort();
                    let mut m: Vec<(i128, i128)> = Vec::new();
                    for (lo, hi) in merged {
                        match m.last_mut() {
                            Some(last) if lo <= last.1 + 1 => last.1 = last.1.max(hi),
                            _ => m.push((lo, hi)),
                        }
                    }
                    Cl::Atom(place, ty, Atom::Ints(m))
                } else {
                    Cl::Alts(place, ty, ast.list(*alts).to_vec())
                }
            }
            Pat::Ref(_, x) => {
                let inner = match self.kind(ty) {
                    TyKind::Ref(_, i, _) => i,
                    _ => return Cl::Opaque,
                };
                let key = (pkey(&place), u32::MAX);
                let (pl, it) = match tree.derefs.get(&key) {
                    Some(r) => r.clone(),
                    None => {
                        let ptr = self.read_place(&place, ty);
                        let ptr = self.to_leaves(ptr, ty);
                        if ptr.is_empty() {
                            return Cl::Opaque;
                        }
                        let pl = if self.unsized_ty(inner) { Place::Regs(ptr) } else { Place::Mem(ptr[0], 0) };
                        tree.derefs.insert(key, (pl.clone(), inner));
                        (pl, inner)
                    }
                };
                Cl::Expand(vec![Ob { place: pl, ty: it, pat: *x }])
            }
            Pat::Box(_) | Pat::Slice(_) | Pat::Mac(_) => Cl::Opaque,
        }
    }

    /// A path pattern that names a definition.
    fn dt_def(&mut self, place: &Place, ty: TyId, p: PatId, d: DefId) -> Cl {
        match self.u.prog.def(d).kind {
            DefKind::Variant => {
                let vi = self.u.prog.def(d).sub;
                match self.kind(ty) {
                    TyKind::Adt(ad, _, _) => match self.u.tcx.adts.get(&ad) {
                        Some(adt) if adt.variants.len() <= 1 => Cl::Drop,
                        Some(_) if self.dt_tag_readable(ty) => Cl::Atom(place.clone(), ty, Atom::Variant { vi, pat: p }),
                        _ => Cl::Opaque,
                    },
                    _ => Cl::Opaque,
                }
            }
            DefKind::Const | DefKind::AssocConst => Cl::Opaque,
            // a unit struct: nothing to test
            _ => Cl::Drop,
        }
    }

    /// Does an enum of type `ty` have a discriminant `read_tag` can produce?
    fn dt_tag_readable(&mut self, ty: TyId) -> bool {
        let l = self.layout(ty);
        let TyKind::Adt(d, _, _) = self.kind(ty) else { return false };
        let Some(adt) = self.u.tcx.adts.get(&d) else { return false };
        adt.is_enum && (l.tag.is_some() || matches!(l.enc, crate::layout::TagEnc::Single(_)))
    }

    /// Emits the node for `rows` into the empty block `blk`.
    fn node(&mut self, tree: &mut Tree, blk: u32, rows: Vec<DRow>) {
        tree.nodes += 1;
        if tree.nodes > MAX_NODES {
            tree.failed = true;
            return;
        }
        self.switch(blk);
        let rows = self.dt_normalize(tree, rows);
        if tree.failed {
            return;
        }
        let Some(r0) = rows.first() else {
            self.term(Term::Unreachable);
            return;
        };
        if r0.atoms.is_empty() {
            // the first row matches: its arm
            let k = r0.arm;
            if r0.opaque {
                tree.used_e[k] = true;
                self.goto(tree.e[k]);
            } else {
                tree.used_post[k] = true;
                self.goto(tree.post[k]);
            }
            return;
        }
        let (key, place, ty, first) = r0.atoms[0].clone();
        match first {
            Atom::Ints(_) => self.dt_ints_node(tree, rows, key, place, ty),
            Atom::Variant { .. } => self.dt_variants_node(tree, rows, key, place, ty),
        }
    }

    /// `x <cond> k` as a flag vreg: an immediate where the back ends encode `k` (a signed 32-bit
    /// immediate), else against a constant register.
    fn dt_cmp(&mut self, cond: Cond, signed: bool, x: VReg, k: i64) -> VReg {
        let c = self.f.vreg(Cls::I);
        if k as i32 as i64 == k {
            self.emit(Inst::ICmpI(cond, signed, c, x, k));
        } else {
            let kr = self.iconst(k);
            self.emit(Inst::ICmp(cond, signed, c, x, kr));
        }
        c
    }

    /// The row without its atom on `key`, if it has one (and the atom).
    fn dt_take(row: &DRow, key: &PKey) -> (DRow, Option<Atom>) {
        let mut r = row.clone();
        let mut got = None;
        if let Some(i) = r.atoms.iter().position(|a| &a.0 == key) {
            got = Some(r.atoms.remove(i).3);
        }
        (r, got)
    }

    fn dt_ints_node(&mut self, tree: &mut Tree, rows: Vec<DRow>, key: PKey, place: Place, ty: TyId) {
        let (dmin, dmax, signed) = self.dt_domain(ty).unwrap();
        // the points where the set of rows that match changes
        let mut points: Vec<i128> = vec![dmin];
        for r in &rows {
            if let Some((_, _, _, Atom::Ints(rs))) = r.atoms.iter().find(|a| a.0 == key) {
                for &(lo, hi) in rs {
                    points.push(lo);
                    if hi < dmax {
                        points.push(hi + 1);
                    }
                }
            }
        }
        points.sort();
        points.dedup();
        // cells: [points[i], points[i+1] - 1], the last up to dmax; rows per cell
        let mut kids: Vec<(Vec<u32>, u32, Vec<(i128, i128)>, Vec<DRow>)> = Vec::new(); // (row ids, block, ranges, rows)
        for (i, &lo) in points.iter().enumerate() {
            let hi = if i + 1 < points.len() { points[i + 1] - 1 } else { dmax };
            let mut sub: Vec<DRow> = Vec::new();
            for r in &rows {
                let (r2, atom) = Self::dt_take(r, &key);
                let hit = match atom {
                    None => true,
                    Some(Atom::Ints(rs)) => rs.iter().any(|&(a, b)| a <= lo && lo <= b),
                    Some(_) => false,
                };
                if hit {
                    sub.push(r2);
                }
            }
            let ids: Vec<u32> = sub.iter().map(|r| r.id).collect();
            match kids.iter_mut().find(|k| k.0 == ids) {
                Some(k) => match k.2.last_mut() {
                    Some(last) if last.1 + 1 == lo => last.1 = hi,
                    _ => k.2.push((lo, hi)),
                },
                None => {
                    let b = self.f.block();
                    kids.push((ids, b, vec![(lo, hi)], sub));
                }
            }
        }
        // the child covering the most values is the final else
        let width = |k: &(Vec<u32>, u32, Vec<(i128, i128)>, Vec<DRow>)| -> i128 { k.2.iter().map(|r| r.1 - r.0 + 1).sum() };
        let di = (0..kids.len()).max_by_key(|i| width(&kids[*i])).unwrap();
        let default = kids[di].1;
        // read the value once, test the ranges of the other children in order
        let v = self.read_place(&place, ty);
        let v = self.to_leaves(v, ty);
        let v = v[0];
        let mut tests: Vec<(i128, i128, u32)> = Vec::new();
        for (i, k) in kids.iter().enumerate() {
            if i != di {
                for &(lo, hi) in &k.2 {
                    tests.push((lo, hi, k.1));
                }
            }
        }
        tests.sort();
        let kc = |x: i128| x as i64;
        for (i, &(lo, hi, to)) in tests.iter().enumerate() {
            let next = if i + 1 == tests.len() { default } else { self.f.block() };
            if lo == hi {
                let c = self.dt_cmp(Cond::Eq, false, v, kc(lo));
                self.term(Term::Branch(c, to, next));
            } else if lo == dmin {
                let c = self.dt_cmp(Cond::Le, signed, v, kc(hi));
                self.term(Term::Branch(c, to, next));
            } else if hi == dmax {
                let c = self.dt_cmp(Cond::Ge, signed, v, kc(lo));
                self.term(Term::Branch(c, to, next));
            } else {
                let mid = self.f.block();
                let c = self.dt_cmp(Cond::Ge, signed, v, kc(lo));
                self.term(Term::Branch(c, mid, next));
                self.switch(mid);
                let c2 = self.dt_cmp(Cond::Le, signed, v, kc(hi));
                self.term(Term::Branch(c2, to, next));
            }
            if next != default {
                self.switch(next);
            }
        }
        if tests.is_empty() {
            self.goto(default);
        }
        for (_, b, _, sub) in kids {
            self.node(tree, b, sub);
            if tree.failed {
                return;
            }
        }
    }

    fn dt_variants_node(&mut self, tree: &mut Tree, rows: Vec<DRow>, key: PKey, place: Place, ty: TyId) {
        let TyKind::Adt(d, _, _) = self.kind(ty) else {
            tree.failed = true;
            return;
        };
        let Some(adt) = self.u.tcx.adts.get(&d) else {
            tree.failed = true;
            return;
        };
        // the variants some row tests, in order of first appearance
        let mut vis: Vec<u32> = Vec::new();
        for r in &rows {
            if let Some((_, _, _, Atom::Variant { vi, .. })) = r.atoms.iter().find(|a| a.0 == key) {
                if !vis.contains(vi) {
                    vis.push(*vi);
                }
            }
        }
        // rows per variant: the rows that test it (their fields become obligations) and the
        // rows that do not test this place (they match every variant)
        let mut kids: Vec<(Option<u32>, u32, Vec<DRow>)> = Vec::new();
        for &vi in &vis {
            let mut sub = Vec::new();
            for r in &rows {
                let (mut r2, atom) = Self::dt_take(r, &key);
                match atom {
                    None => sub.push(r2),
                    Some(Atom::Variant { vi: v2, pat }) if v2 == vi => {
                        let id = tree.next_id;
                        tree.next_id += 1;
                        r2.id = id;
                        r2.expand = Some((place.clone(), ty, vi, pat));
                        sub.push(r2);
                    }
                    Some(_) => {}
                }
            }
            kids.push((Some(vi), self.f.block(), sub));
        }
        let mut other: Vec<DRow> = Vec::new();
        for r in &rows {
            let (r2, atom) = Self::dt_take(r, &key);
            if atom.is_none() {
                other.push(r2);
            }
        }
        let all_variants = vis.len() == adt.variants.len();
        // every variant is tested and no row is left for the others: the last one is the else
        let default = if all_variants && other.is_empty() {
            kids.last().unwrap().1
        } else {
            let b = self.f.block();
            kids.push((None, b, other));
            b
        };
        let Some(tag) = self.read_tag(&place, ty) else {
            tree.failed = true;
            return;
        };
        if tag.len() != 1 {
            tree.failed = true;
            return;
        }
        let tests: Vec<(u32, u32)> = kids.iter().filter(|k| k.0.is_some() && k.1 != default).map(|k| (k.0.unwrap(), k.1)).collect();
        for (i, &(vi, to)) in tests.iter().enumerate() {
            let Some(discr) = adt.discriminant_for_variant(&self.u.tcx, &self.u.prog, vi) else {
                tree.failed = true;
                return;
            };
            let next = if i + 1 == tests.len() { default } else { self.f.block() };
            let c = self.dt_cmp(Cond::Eq, true, tag[0], discr.signed_value() as i64);
            self.term(Term::Branch(c, to, next));
            if next != default {
                self.switch(next);
            }
        }
        if tests.is_empty() {
            self.goto(default);
        }
        for (_, b, sub) in kids {
            self.node(tree, b, sub);
            if tree.failed {
                return;
            }
        }
    }
}

enum Cl {
    Drop,
    Opaque,
    Expand(Vec<Ob>),
    Atom(Place, TyId, Atom),
    /// an or-pattern whose alternatives need rows of their own
    Alts(Place, TyId, Vec<PatId>),
}
