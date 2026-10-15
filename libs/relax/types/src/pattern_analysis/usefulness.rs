//! The usefulness algorithm (rustc_pattern_analysis usefulness.rs): one pass over the matrix
//! of a match's arms computes the values no arm matches (the non-exhaustiveness witnesses)
//! and, for every arm and or-pattern alternative, whether it matches a value no earlier arm
//! matches (reachability). See rustc's module documentation for the theory; this is a
//! line-by-line port with the patterns held in an arena (`PIx`) instead of references.

use super::constructor::{ConstructorSet, Ctor};
use super::{Cx, PIx};
use crate::types::TyId;
use std::collections::HashMap;

/// A small growable bit set over row indices.
#[derive(Clone, Default)]
pub(crate) struct Bits(Vec<u64>);

impl Bits {
    fn insert(&mut self, i: usize) {
        let w = i / 64;
        if self.0.len() <= w { self.0.resize(w + 1, 0); }
        self.0[w] |= 1 << (i % 64);
    }
    fn insert_range(&mut self, n: usize) {
        for i in 0..n { self.insert(i); }
    }
    fn iter(&self) -> impl Iterator<Item = usize> + '_ {
        self.0.iter().enumerate().flat_map(|(wi, w)| (0..64).filter(move |b| w & (1 << b) != 0).map(move |b| wi * 64 + b))
    }
}

/// The reachability of one branch (an arm, an or-pattern alternative).
#[derive(Default)]
struct BranchPatUsefulness {
    useful: bool,
    /// the patterns of earlier arms that intersect this one (the redundancy's explanation)
    covered_by: Vec<PIx>,
}

impl BranchPatUsefulness {
    fn update(&mut self, row: &MatrixRow, matrix: &Matrix) {
        self.useful |= row.useful;
        for row_id in row.intersects_at_least.iter() {
            let row = &matrix.rows[row_id];
            if row.useful && !row.is_under_guard {
                if let Some(p) = row.head() {
                    if !self.covered_by.contains(&p) { self.covered_by.push(p); }
                }
            }
        }
    }
    fn is_redundant(&self) -> Option<Vec<PIx>> {
        if self.useful {
            None
        } else {
            let mut v = self.covered_by.clone();
            v.sort_unstable();
            Some(v)
        }
    }
}

/// Whether a place is known to hold a valid value (empty types may then be omitted).
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum PlaceValidity {
    ValidOnly,
    MaybeInvalid,
}

impl PlaceValidity {
    fn specialize(self, ctor: &Ctor) -> Self {
        // behind a reference or a union field, the data may not be valid
        if matches!(ctor, Ctor::Ref | Ctor::UnionField) { PlaceValidity::MaybeInvalid } else { self }
    }
}

#[derive(Clone)]
struct PlaceInfo {
    ty: TyId,
    private_uninhabited: bool,
    validity: PlaceValidity,
    is_scrutinee: bool,
}

impl PlaceInfo {
    fn specialize(&self, cx: &Cx, ctor: &Ctor) -> Vec<PlaceInfo> {
        let validity = self.validity.specialize(ctor);
        cx.ctor_sub_tys(ctor, self.ty).into_iter().map(|(ty, private_uninhabited)| PlaceInfo { ty, private_uninhabited, validity, is_scrutinee: false }).collect()
    }

    /// The constructors to specialize the column by, and the missing ones the witnesses
    /// report. None: a type the analysis cannot list (an error type).
    fn split_column_ctors<'a>(&self, cx: &Cx, ctors: impl Iterator<Item = &'a Ctor> + Clone) -> Option<(Vec<Ctor>, Vec<Ctor>)> {
        if self.private_uninhabited {
            // skip the whole column
            return Some((vec![Ctor::PrivateUninhabited], vec![]));
        }
        if ctors.clone().any(|c| matches!(c, Ctor::Or)) {
            // only on the first column of the expanded or-pattern rows
            return Some((vec![Ctor::Or], vec![]));
        }
        let ctors_for_ty = cx.ctors_for_ty(self.ty)?;
        // `match x {}` on an empty type x is allowed (the toplevel exception); the
        // `exhaustive_patterns` feature is never on
        let is_toplevel_exception = self.is_scrutinee && matches!(ctors_for_ty, ConstructorSet::NoConstructors);
        let empty_arms_are_unreachable = self.validity == PlaceValidity::ValidOnly && is_toplevel_exception;
        let can_omit_empty_arms = self.validity == PlaceValidity::ValidOnly || is_toplevel_exception;

        let split_set = ctors_for_ty.split(ctors);
        let all_missing = split_set.present.is_empty();
        let mut split_ctors = split_set.present;
        if !(split_set.missing.is_empty() && (split_set.missing_empty.is_empty() || empty_arms_are_unreachable)) {
            split_ctors.push(Ctor::Missing);
        }
        let mut missing_ctors = split_set.missing;
        if !can_omit_empty_arms {
            missing_ctors.extend(split_set.missing_empty);
        }
        // `_` instead of listing every missing constructor when none was mentioned (except at
        // the top: `match x {}` lists the variants)
        let report_individual_missing_ctors = self.is_scrutinee || !all_missing;
        if !missing_ctors.is_empty() && !report_individual_missing_ctors {
            missing_ctors = vec![Ctor::Wildcard];
        } else if missing_ctors.iter().any(|c| c.is_non_exhaustive()) {
            missing_ctors = vec![Ctor::NonExhaustive];
        }
        Some((split_ctors, missing_ctors))
    }
}

/// A row of patterns: `None` is a wildcard.
#[derive(Clone)]
struct PatStack {
    pats: Vec<Option<PIx>>,
    /// whether this row is relevant for usefulness (a wildcard row under a specialized
    /// constructor that is missing anyway is not)
    relevant: bool,
}

impl PatStack {
    fn head(&self) -> Option<PIx> {
        self.pats[0]
    }

    fn expand_or_pat(&self, cx: &Cx) -> Vec<PatStack> {
        match self.head() {
            Some(p) if cx.pat(p).ctor == Ctor::Or => cx.pat(p).fields.iter().map(|(_, f)| {
                let mut new = self.clone();
                new.pats[0] = Some(*f);
                new
            }).collect(),
            _ => vec![self.clone()],
        }
    }

    fn pop_head_constructor(&self, cx: &Cx, ctor: &Ctor, ctor_arity: usize, ctor_is_relevant: bool) -> Option<PatStack> {
        let head = self.head();
        if head.is_some_and(|p| cx.pat(p).arity > ctor_arity) {
            return None;
        }
        let mut new_pats = specialize(cx, head, ctor, ctor_arity);
        new_pats.extend_from_slice(&self.pats[1..]);
        let head_is_wild = head.map_or(true, |p| cx.pat(p).ctor == Ctor::Wildcard);
        let ctor_is_relevant = !head_is_wild || ctor_is_relevant;
        Some(PatStack { pats: new_pats, relevant: self.relevant && ctor_is_relevant })
    }
}

/// The fields of `pat` under `other_ctor` (pat.rs DeconstructedPat::specialize).
fn specialize(cx: &Cx, pat: Option<PIx>, other_ctor: &Ctor, other_ctor_arity: usize) -> Vec<Option<PIx>> {
    let Some(p) = pat else { return vec![None; other_ctor_arity] };
    if matches!(other_ctor, Ctor::PrivateUninhabited) {
        return Vec::new();
    }
    let p = cx.pat(p);
    let mut fields = vec![None; other_ctor_arity];
    match p.ctor {
        Ctor::Slice(super::constructor::Slice { kind: super::constructor::SliceKind::VarLen(prefix, _), .. }) if p.arity != other_ctor_arity => {
            // the suffix moves to the end of the longer slice
            for (idx, f) in &p.fields {
                let new_idx = if *idx < prefix { *idx } else { *idx + other_ctor_arity - p.arity };
                fields[new_idx] = Some(*f);
            }
        }
        _ => {
            for (idx, f) in &p.fields {
                if let Some(slot) = fields.get_mut(*idx) { *slot = Some(*f); }
            }
        }
    }
    fields
}

#[derive(Clone)]
struct MatrixRow {
    pats: PatStack,
    is_under_guard: bool,
    /// the row of the parent matrix this one came from (the arm, at the top)
    parent_row: usize,
    useful: bool,
    /// earlier rows of the same matrix this row intersects
    intersects_at_least: Bits,
    /// whether the head pattern is a branch (an arm, or an or-pattern alternative)
    head_is_branch: bool,
}

impl MatrixRow {
    fn head(&self) -> Option<PIx> {
        self.pats.head()
    }
}

#[derive(Clone)]
struct Matrix {
    rows: Vec<MatrixRow>,
    place_info: Vec<PlaceInfo>,
    wildcard_row_is_relevant: bool,
}

impl Matrix {
    fn push(&mut self, mut row: MatrixRow) {
        row.intersects_at_least = Bits::default();
        self.rows.push(row);
    }

    fn specialize_constructor(&self, cx: &Cx, ctor: &Ctor, ctor_is_relevant: bool) -> Option<Matrix> {
        if matches!(ctor, Ctor::Or) {
            let mut matrix = Matrix { rows: Vec::new(), place_info: self.place_info.clone(), wildcard_row_is_relevant: self.wildcard_row_is_relevant };
            for (i, row) in self.rows.iter().enumerate() {
                let is_or_pat = row.head().is_some_and(|p| cx.pat(p).ctor == Ctor::Or);
                for patstack in row.pats.expand_or_pat(cx) {
                    matrix.push(MatrixRow { pats: patstack, parent_row: i, is_under_guard: row.is_under_guard, useful: false, intersects_at_least: Bits::default(), head_is_branch: is_or_pat });
                }
            }
            Some(matrix)
        } else {
            let sub = self.place_info[0].specialize(cx, ctor);
            let arity = sub.len();
            let place_info = sub.into_iter().chain(self.place_info[1..].iter().cloned()).collect();
            let mut matrix = Matrix { rows: Vec::new(), place_info, wildcard_row_is_relevant: self.wildcard_row_is_relevant && ctor_is_relevant };
            for (i, row) in self.rows.iter().enumerate() {
                let head_ctor = row.head().map_or(&Ctor::Wildcard, |p| &cx.pat(p).ctor);
                if ctor.is_covered_by(head_ctor)? {
                    let pats = row.pats.pop_head_constructor(cx, ctor, arity, ctor_is_relevant)?;
                    matrix.push(MatrixRow { pats, parent_row: i, is_under_guard: row.is_under_guard, useful: false, intersects_at_least: Bits::default(), head_is_branch: false });
                }
            }
            Some(matrix)
        }
    }

    /// Recovers the parent rows' usefulness and intersections from a specialized matrix.
    fn unspecialize(&mut self, specialized: Matrix) {
        for child_row in specialized.rows.iter() {
            let parent_row_id = child_row.parent_row;
            let parent_row = &mut self.rows[parent_row_id];
            parent_row.useful |= child_row.useful;
            for child_intersection in child_row.intersects_at_least.iter() {
                let parent_intersection = specialized.rows[child_intersection].parent_row;
                if parent_intersection != parent_row_id {
                    parent_row.intersects_at_least.insert(parent_intersection);
                }
            }
        }
    }
}

/// A pattern of a witness: a value no arm matches.
#[derive(Clone, Debug)]
pub struct WitnessPat {
    pub ctor: Ctor,
    pub fields: Vec<WitnessPat>,
    pub ty: TyId,
}

impl WitnessPat {
    pub fn wildcard(cx: &Cx, ty: TyId) -> Self {
        let is_empty = cx.ctors_for_ty(ty).is_some_and(|c| c.all_empty());
        WitnessPat { ctor: if is_empty { Ctor::Never } else { Ctor::Wildcard }, fields: Vec::new(), ty }
    }

    pub fn wild_from_ctor(cx: &Cx, ctor: Ctor, ty: TyId) -> Self {
        if matches!(ctor, Ctor::Wildcard) {
            return Self::wildcard(cx, ty);
        }
        let fields = cx.ctor_sub_tys(&ctor, ty).into_iter().filter(|(_, skip)| !skip).map(|(t, _)| Self::wildcard(cx, t)).collect();
        WitnessPat { ctor, fields, ty }
    }
}

/// Partial witnesses: the last pattern is the head column's.
#[derive(Clone)]
struct WitnessStack(Vec<WitnessPat>);

impl WitnessStack {
    fn apply_constructor(mut self, cx: &Cx, ty: TyId, ctor: &Ctor) -> Vec<WitnessStack> {
        let len = self.0.len();
        let arity = cx.ctor_arity(ctor, ty);
        let fields: Vec<WitnessPat> = self.0.drain((len - arity)..).rev().collect();
        if matches!(ctor, Ctor::UnionField) && fields.iter().filter(|p| !matches!(p.ctor, Ctor::Wildcard)).count() >= 2 {
            // a union value has one field: one witness per non-wildcard field
            self.0.push(WitnessPat::wild_from_ctor(cx, ctor.clone(), ty));
            fields.into_iter().enumerate().filter(|(_, p)| !matches!(p.ctor, Ctor::Wildcard)).map(|(i, p)| {
                let mut ret = self.clone();
                ret.0.last_mut().unwrap().fields[i] = p;
                ret
            }).collect()
        } else {
            self.0.push(WitnessPat { ctor: ctor.clone(), fields, ty });
            vec![self]
        }
    }
}

/// A set of partial witnesses, each a stack of the matrix's columns (reversed).
#[derive(Clone)]
struct WitnessMatrix(Vec<WitnessStack>);

impl WitnessMatrix {
    fn empty() -> Self {
        WitnessMatrix(Vec::new())
    }
    fn unit_witness() -> Self {
        WitnessMatrix(vec![WitnessStack(Vec::new())])
    }
    fn is_empty(&self) -> bool {
        self.0.is_empty()
    }
    fn push_pattern(&mut self, pat: WitnessPat) {
        for w in self.0.iter_mut() {
            w.0.push(pat.clone());
        }
    }
    fn apply_constructor(&mut self, cx: &Cx, ty: TyId, missing_ctors: &[Ctor], ctor: &Ctor) {
        if self.is_empty() || matches!(ctor, Ctor::Or) {
            return;
        }
        if matches!(ctor, Ctor::Missing) {
            // a witness for each missing constructor
            let mut ret = Self::empty();
            for c in missing_ctors {
                let pat = WitnessPat::wild_from_ctor(cx, c.clone(), ty);
                let mut m = self.clone();
                m.push_pattern(pat);
                ret.0.extend(m.0);
            }
            *self = ret;
        } else {
            for w in std::mem::take(&mut self.0) {
                self.0.extend(w.apply_constructor(cx, ty, ctor));
            }
        }
    }
}

struct UsefulnessCtxt<'a, 'c> {
    cx: &'a Cx<'c>,
    branch_usefulness: HashMap<PIx, BranchPatUsefulness>,
    complexity_limit: usize,
    complexity_level: usize,
}

/// usefulness.rs compute_exhaustiveness_and_usefulness. None: an error type or the
/// complexity limit (the match is not reported).
fn compute_exhaustiveness_and_usefulness(mcx: &mut UsefulnessCtxt, matrix: &mut Matrix) -> Option<WitnessMatrix> {
    let cx = mcx.cx;
    if !matrix.wildcard_row_is_relevant && matrix.rows.iter().all(|r| !r.pats.relevant) {
        return Some(WitnessMatrix::empty());
    }
    let Some(place) = matrix.place_info.first().cloned() else {
        // no columns: the first row (and those after guarded rows) is useful
        mcx.complexity_level += matrix.rows.len();
        if mcx.complexity_level > mcx.complexity_limit { return None; }
        let mut useful = true;
        for (i, row) in matrix.rows.iter_mut().enumerate() {
            row.useful = useful;
            row.intersects_at_least.insert_range(i);
            useful &= row.is_under_guard;
        }
        return Some(if useful && matrix.wildcard_row_is_relevant { WitnessMatrix::unit_witness() } else { WitnessMatrix::empty() });
    };
    let heads: Vec<Ctor> = matrix.rows.iter().map(|r| r.head().map_or(Ctor::Wildcard, |p| cx.pat(p).ctor.clone())).collect();
    let (split_ctors, missing_ctors) = place.split_column_ctors(cx, heads.iter())?;
    let ty = place.ty;
    let mut ret = WitnessMatrix::empty();
    for ctor in split_ctors {
        // a wildcard row is only relevant under a constructor that is not missing anyway
        let ctor_is_relevant = matches!(ctor, Ctor::Missing) || missing_ctors.is_empty();
        let mut spec_matrix = matrix.specialize_constructor(cx, &ctor, ctor_is_relevant)?;
        let mut witnesses = compute_exhaustiveness_and_usefulness(mcx, &mut spec_matrix)?;
        witnesses.apply_constructor(cx, ty, &missing_ctors, &ctor);
        ret.0.extend(witnesses.0);
        matrix.unspecialize(spec_matrix);
    }
    for row in matrix.rows.iter() {
        if row.head_is_branch {
            if let Some(p) = row.head() {
                mcx.branch_usefulness.entry(p).or_default().update(row, matrix);
            }
        }
    }
    Some(ret)
}

pub struct MatchArm {
    pub pat: PIx,
    pub has_guard: bool,
}

pub enum Usefulness {
    /// useful, with its redundant sub-patterns (or-pattern alternatives) and what covers them
    Useful(Vec<(PIx, Vec<PIx>)>),
    /// redundant, and the earlier patterns covering it
    Redundant(Vec<PIx>),
}

pub struct UsefulnessReport {
    pub arm_usefulness: Vec<Usefulness>,
    pub non_exhaustiveness_witnesses: Vec<WitnessPat>,
}

pub fn compute_match_usefulness(cx: &Cx, arms: &[MatchArm], scrut_ty: TyId, scrut_validity: PlaceValidity) -> Option<UsefulnessReport> {
    let mut mcx = UsefulnessCtxt { cx, branch_usefulness: HashMap::new(), complexity_limit: 10_000_000, complexity_level: 0 };
    let place_info = PlaceInfo { ty: scrut_ty, private_uninhabited: false, validity: scrut_validity, is_scrutinee: true };
    let mut matrix = Matrix { rows: Vec::with_capacity(arms.len()), place_info: vec![place_info], wildcard_row_is_relevant: true };
    for (arm_id, arm) in arms.iter().enumerate() {
        matrix.push(MatrixRow { pats: PatStack { pats: vec![Some(arm.pat)], relevant: true }, parent_row: arm_id, is_under_guard: arm.has_guard, useful: false, intersects_at_least: Bits::default(), head_is_branch: true });
    }
    let witnesses = compute_exhaustiveness_and_usefulness(&mut mcx, &mut matrix)?;
    let non_exhaustiveness_witnesses = witnesses.0.into_iter().map(|w| {
        let mut w = w.0;
        w.pop().expect("one witness column")
    }).collect();
    let mut arm_usefulness = Vec::with_capacity(arms.len());
    for arm in arms {
        let u = mcx.branch_usefulness.get(&arm.pat)?;
        arm_usefulness.push(match u.is_redundant() {
            Some(covered_by) => Usefulness::Redundant(covered_by),
            None => {
                let mut redundant = Vec::new();
                walk_redundant(cx, &mcx.branch_usefulness, arm.pat, &mut redundant);
                Usefulness::Useful(redundant)
            }
        });
    }
    Some(UsefulnessReport { arm_usefulness, non_exhaustiveness_witnesses })
}

/// The redundant branches inside a useful pattern (stopping at the first redundant one).
fn walk_redundant(cx: &Cx, bu: &HashMap<PIx, BranchPatUsefulness>, p: PIx, out: &mut Vec<(PIx, Vec<PIx>)>) {
    if let Some(u) = bu.get(&p) {
        if let Some(covered_by) = u.is_redundant() {
            out.push((p, covered_by));
            return;
        }
    }
    for (_, f) in &cx.pat(p).fields {
        walk_redundant(cx, bu, *f, out);
    }
}
