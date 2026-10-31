//! rustc_mir_build/src/builder/matches/util.rs: false edges, fake borrows of the matched places.
use super::{Binding, Candidate, FlatPat, MatchPairTree, SubpatternBindings, TestableCase};
use crate::fxhash::HashMap;
use crate::mir::build::{BuildResult, Builder, PlaceBase};
use crate::mir::{BasicBlock, BorrowKind, FakeBorrowKind, Local, LocalInfo, LocalKind, MutBorrowKind, Mutability, Place, PlaceElem, SourceInfo, Span, TerminatorKind};

/// An insertion-ordered map with a handful of keys (rustc `FxIndexMap` here).
pub(super) struct OrderedMap<K, V> {
    index: HashMap<K, usize>,
    entries: Vec<(K, V)>,
}

impl<K: std::hash::Hash + Eq + Clone, V> OrderedMap<K, V> {
    pub(super) fn new() -> Self {
        Self { index: HashMap::default(), entries: Vec::new() }
    }
    pub(super) fn get(&self, key: &K) -> Option<&V> {
        self.index.get(key).map(|&i| &self.entries[i].1)
    }
    pub(super) fn entry_or_default(&mut self, key: K) -> &mut V
    where
        V: Default,
    {
        let i = match self.index.get(&key) {
            Some(&i) => i,
            None => {
                self.entries.push((key.clone(), V::default()));
                self.index.insert(key, self.entries.len() - 1);
                self.entries.len() - 1
            }
        };
        &mut self.entries[i].1
    }
    pub(super) fn insert(&mut self, key: K, value: V) {
        match self.index.get(&key) {
            Some(&i) => self.entries[i].1 = value,
            None => {
                self.entries.push((key.clone(), value));
                self.index.insert(key, self.entries.len() - 1);
            }
        }
    }
    pub(super) fn keys(&self) -> impl Iterator<Item = &K> {
        self.entries.iter().map(|(k, _)| k)
    }
    pub(super) fn iter(&self) -> impl Iterator<Item = (&K, &V)> {
        self.entries.iter().map(|(k, v)| (k, v))
    }
    pub(super) fn into_entries(self) -> Vec<(K, V)> {
        self.entries
    }
}

/// `FakeBorrowKind` is ordered `Shallow < Deep`.
fn borrow_rank(kind: FakeBorrowKind) -> u8 {
    match kind {
        FakeBorrowKind::Shallow => 0,
        FakeBorrowKind::Deep => 1,
    }
}

impl Builder<'_> {
    /// Creates a false edge to `imaginary_target` and a real edge to `real_target`. If
    /// `imaginary_target` is the same as the real target, a Goto is generated instead.
    pub(super) fn false_edges(&mut self, from_block: BasicBlock, real_target: BasicBlock, imaginary_target: BasicBlock, source_info: SourceInfo) {
        if imaginary_target != real_target {
            self.cfg.terminate(from_block, source_info, TerminatorKind::FalseEdge { real_target, imaginary_target });
        } else {
            self.cfg.goto(from_block, source_info, real_target)
        }
    }
}

/// Determine the set of places that have to be stable across match guards.
///
/// Returns a list of places that need a fake borrow along with a local to store it.
///
/// Match exhaustiveness checking is not able to handle the case where the place being matched on is
/// mutated in the guards. We add "fake borrows" to the guards that prevent any mutation of the
/// place being matched. There are a some subtleties:
///
/// 1. Borrowing `*x` doesn't prevent assigning to `x`. If `x` is a shared reference, the borrow
///    isn't even tracked. As such we have to add fake borrows of any prefixes of a place.
/// 2. We don't want `match x { (Some(_), _) => (), .. }` to conflict with mutable borrows of `x.1`,
///    so we only add fake borrows for places which are bound or tested by the match.
/// 3. We don't want `match x { Some(_) => (), .. }` to conflict with mutable borrows of
///    `(x as Some).0`, so the borrows are a special shallow borrow that only affects the place and
///    not its projections.
/// 4. The fake borrows may be of places in inactive variants, e.g. here we need to fake borrow `x`
///    and `(x as Some).0`, but when we reach the guard `x` may not be `Some`. So it would be UB to
///    generate code for the fake borrows. They therefore have to be removed by a MIR pass run after
///    borrow checking.
pub(super) fn collect_fake_borrows(
    cx: &mut Builder<'_>,
    candidates: &[Candidate],
    temp_span: Span,
    scrutinee_base: PlaceBase,
) -> BuildResult<Vec<(Place, Local, FakeBorrowKind)>> {
    if candidates.iter().all(|candidate| !candidate.has_guard) {
        // Fake borrows are only used when there is a guard.
        return Ok(Vec::new());
    }
    let mut collector = FakeBorrowCollector { scrutinee_base, fake_borrows: OrderedMap::new() };
    for candidate in candidates.iter() {
        collector.visit_candidate(candidate);
    }
    let mut out = Vec::new();
    for (matched_place, borrow_kind) in collector.fake_borrows.into_entries() {
        let fake_borrow_deref_ty = cx.place_ty(&matched_place)?.ty;
        let fake_borrow_ty = cx.ref_ty(false, fake_borrow_deref_ty);
        let fake_borrow_temp = cx.new_local(fake_borrow_ty, Mutability::Mut, LocalKind::Temp, LocalInfo::FakeBorrow, temp_span);
        out.push((matched_place, fake_borrow_temp, borrow_kind));
    }
    Ok(out)
}

pub(super) struct FakeBorrowCollector {
    /// Base of the scrutinee place. Used to distinguish bindings inside the scrutinee place from
    /// bindings inside deref patterns.
    scrutinee_base: PlaceBase,
    /// Store for each place the kind of borrow to take. In case of conflicts, we take the strongest
    /// borrow (i.e. Deep > Shallow).
    /// Invariant: for any place in `fake_borrows`, all the prefixes of this place that are
    /// dereferences are also borrowed with the same of stronger borrow kind.
    fake_borrows: OrderedMap<Place, FakeBorrowKind>,
}

impl FakeBorrowCollector {
    fn covers(&self, place: &Place, kind: FakeBorrowKind) -> bool {
        self.fake_borrows.get(place).is_some_and(|k| borrow_rank(*k) >= borrow_rank(kind))
    }

    // Fake borrow this place and its dereference prefixes.
    fn fake_borrow(&mut self, place: Place, kind: FakeBorrowKind) {
        if self.covers(&place, kind) {
            return;
        }
        self.fake_borrows.insert(place.clone(), kind);
        // Also fake borrow the prefixes of any fake borrow.
        self.fake_borrow_deref_prefixes(&place, kind);
    }

    // Fake borrow the prefixes of this place that are dereferences.
    fn fake_borrow_deref_prefixes(&mut self, place: &Place, kind: FakeBorrowKind) {
        for i in (0..place.projection.len()).rev() {
            if let PlaceElem::Deref = place.projection[i] {
                // Insert a shallow borrow after a deref. For other projections the borrow of
                // `place_ref` will conflict with any mutation of `place.base`.
                let prefix = Place { local: place.local, projection: place.projection[..i].to_vec() };
                if self.covers(&prefix, kind) {
                    return;
                }
                self.fake_borrows.insert(prefix, kind);
            }
        }
    }

    fn visit_candidate(&mut self, candidate: &Candidate) {
        for binding in &candidate.extra_data.bindings {
            if let SubpatternBindings::One(binding) = binding {
                self.visit_binding(binding);
            }
        }
        for match_pair in &candidate.match_pairs {
            self.visit_match_pair(match_pair);
        }
    }

    fn visit_flat_pat(&mut self, flat_pat: &FlatPat) {
        for binding in &flat_pat.extra_data.bindings {
            if let SubpatternBindings::One(binding) = binding {
                self.visit_binding(binding);
            }
        }
        for match_pair in &flat_pat.match_pairs {
            self.visit_match_pair(match_pair);
        }
    }

    fn visit_match_pair(&mut self, match_pair: &MatchPairTree) {
        if let TestableCase::Or { pats, .. } = &match_pair.testable_case {
            for flat_pat in pats.iter() {
                self.visit_flat_pat(flat_pat)
            }
        } else if matches!(match_pair.testable_case, TestableCase::Deref { .. }) {
            // The subpairs of a deref pattern are all places relative to the deref temporary, so we
            // don't fake borrow them. Problem is, if we only shallowly fake-borrowed
            // `match_pair.place`, this would allow:
            // ```
            // let mut b = Box::new(false);
            // match b {
            //     deref!(true) => {} // not reached because `*b == false`
            //     _ if { *b = true; false } => {} // not reached because the guard is `false`
            //     deref!(false) => {} // not reached because the guard changed it
            //     // UB because we reached the unreachable.
            // }
            // ```
            // Hence we fake borrow using a deep borrow.
            if let Some(place) = &match_pair.place {
                self.fake_borrow(place.clone(), FakeBorrowKind::Deep);
            }
        } else {
            // Insert a Shallow borrow of any place that is switched on.
            if let Some(place) = &match_pair.place {
                self.fake_borrow(place.clone(), FakeBorrowKind::Shallow);
            }
            for subpair in &match_pair.subpairs {
                self.visit_match_pair(subpair);
            }
        }
    }

    fn visit_binding(&mut self, Binding { source, .. }: &Binding) {
        if let PlaceBase::Local(l) = self.scrutinee_base {
            if l != source.local {
                // The base of this place is a temporary created for deref patterns. We don't emit
                // fake borrows for these as they are not initialized in all branches.
                return;
            }
        }
        // Insert a borrows of prefixes of places that are bound and are behind a dereference
        // projection.
        //
        // These borrows are taken to avoid situations like the following:
        //
        // match x[10] {
        //     _ if { x = &[0]; false } => (),
        //     y => (), // Out of bounds array access!
        // }
        //
        // match *x {
        //     // y is bound by reference in the guard and then by copy in the
        //     // arm, so y is 2 in the arm!
        //     y if { y == 1 && (x = &2) == () } => y,
        //     _ => 3,
        // }
        //
        // We don't just fake borrow the whole place because this is allowed:
        // match u {
        //     _ if { u = true; false } => (),
        //     x => (),
        // }
        self.fake_borrow_deref_prefixes(source, FakeBorrowKind::Shallow);
    }
}

#[must_use]
pub(super) fn ref_pat_borrow_kind(ref_mutability: Mutability) -> BorrowKind {
    match ref_mutability {
        Mutability::Mut => BorrowKind::Mut { kind: MutBorrowKind::Default },
        Mutability::Not => BorrowKind::Shared,
    }
}
