// ported from N:rustc_borrowck/src/region_infer/region_context.rs @8d1a764
use crate::types::{AliasTy, AliasTyKind};
use super::{NllRegionVariableOrigin as Origin, RegionDefinition, RegionErrorKind, TypeTest, VerifyBound, ConstraintSccs};
use super::values::{LivenessValues, PlaceholderIndices, RegionElement, RegionValues};
use crate::borrowck::constraints::{ConstraintCategory as Category, OutlivesConstraint, OutlivesConstraintSet, ReturnConstraint};
use crate::borrowck::constraints::graph::ConstraintGraph;
use crate::borrowck::handle_placeholders::{LoweredConstraints, RegionTracker};
use crate::borrowck::type_check::free_region_relations::UniversalRegionRelations;
use std::collections::VecDeque;
use crate::types::{TyId, TyKind, RegionId, RegionKind};
use crate::tcx::Tcx;
use crate::program::Program;

pub struct RegionInferenceContext {
    pub definitions: Vec<RegionDefinition>,
    pub constraint_sccs: ConstraintSccs,
    pub scc_values: RegionValues,
    pub universal_region_relations: UniversalRegionRelations,
    pub errors: Vec<RegionErrorKind>,
    pub propagated_outlives_requirements: Vec<OutlivesConstraint>,
    pub propagated_type_requirements: Vec<super::ClosureTypeRequirement>,
    constraints: OutlivesConstraintSet,
    constraint_graph: ConstraintGraph,
    scc_annotations: Vec<RegionTracker>,
    liveness_constraints: LivenessValues,
}
impl RegionInferenceContext {
    pub fn solve(tcx: &Tcx, prog: &Program, lowered: LoweredConstraints, universal_region_relations: UniversalRegionRelations,
        liveness_constraints: LivenessValues, num_points: usize, type_tests: &[TypeTest], nested: bool) -> Self {
        let LoweredConstraints { definitions, constraint_sccs, scc_annotations, outlives_constraints } = lowered;
        let mut placeholders = PlaceholderIndices::default();
        for d in &definitions { if let Origin::Placeholder(p) = d.origin { placeholders.insert(p); } }
        let mut scc_values = RegionValues::new(constraint_sccs.num_sccs(), num_points, definitions.iter().enumerate().filter(|(_,d)| d.origin == Origin::FreeRegion).map(|(r,_)|r).collect(), placeholders);
        for (r, d) in definitions.iter().enumerate() {
            let scc = constraint_sccs.scc(r);
            match d.origin {
                Origin::FreeRegion => scc_values.add_free_region(scc, r),
                Origin::Placeholder(p) => scc_values.add_placeholder(scc, p),
                Origin::Existential => (),
            }
            if let Some(live) = liveness_constraints.point_liveness(r) { scc_values.merge_liveness(scc, live); }
        }
        let constraint_graph = outlives_constraints.graph(definitions.len());
        let mut this = Self { definitions, constraint_sccs, scc_values, universal_region_relations,
            errors: vec![], propagated_outlives_requirements: vec![], propagated_type_requirements: vec![], constraints: outlives_constraints,
            constraint_graph, scc_annotations, liveness_constraints };
        this.propagate_constraints();
        this.check_type_tests(tcx, prog, type_tests, nested);
        this.check_universal_regions(tcx, nested);
        this
    }
    fn propagate_constraints(&mut self) {
        for a in self.constraint_sccs.all_sccs() {
            crate::preempt::point();
            for &b in self.constraint_sccs.successors(a) { self.scc_values.add_region(a,b); }
        }
    }
    pub fn liveness_constraints(&self) -> &LivenessValues { &self.liveness_constraints }
    pub fn outlives_constraints(&self) -> &OutlivesConstraintSet { &self.constraints }
    pub fn with_liveness(&self, tcx: &Tcx, prog: &Program, liveness: LivenessValues, num_points: usize, tests: &[TypeTest], nested: bool) -> Self {
        let lowered = LoweredConstraints { definitions: self.definitions.clone(), constraint_sccs: self.constraint_sccs.clone(),
            scc_annotations: self.scc_annotations.clone(), outlives_constraints: self.constraints.clone() };
        Self::solve(tcx, prog, lowered, self.universal_region_relations.clone(), liveness, num_points, tests, nested)
    }
    pub fn region_contains_point(&self, r: usize, point: usize) -> bool { self.scc_values.contains_point(self.constraint_sccs.scc(r), point) }
    pub fn universal_regions_outlived_by(&self, r: usize) -> impl Iterator<Item=usize> + '_ {
        self.scc_values.universal_regions_outlived_by(self.constraint_sccs.scc(r))
    }
    pub fn eval_equal(&self, a: usize, b: usize) -> bool { self.eval_outlives(a,b) && self.eval_outlives(b,a) }
    pub fn eval_outlives(&self, sup: usize, sub: usize) -> bool {
        let n = self.scc_annotations.len().max(0);
        let _ = n;
        let a = self.constraint_sccs.scc(sup);
        let b = self.constraint_sccs.scc(sub);
        if a == b { return true; }
        let relations = &self.universal_region_relations;
        let fr_static = relations.universal_regions.fr_static;
        if sub != fr_static && !self.scc_annotations[a].can_name_all_placeholders(self.scc_annotations[b]) {
            return self.eval_outlives(sup, fr_static);
        }
        if !self.scc_values.universal_regions_outlived_by(b).all(|r1| {
            self.scc_values.universal_regions_outlived_by(a).any(|r2| relations.outlives(r2,r1))
        }) { return false; }
        relations.universal_regions.is_universal_region(sup) || self.scc_values.contains_points(a,b)
    }
    fn to_region_vid(&self, tys: &crate::types::Types, r: RegionId) -> usize {
        match tys.region_kind(r) {
            RegionKind::ReVar(v) => v as usize,
            RegionKind::ReStatic => self.universal_region_relations.universal_regions.fr_static,
            _ => panic!("type-test region was not renumbered"),
        }
    }
    fn eval_verify_bound(&self, tcx: &Tcx, prog: &Program, generic_ty: TyId, lower: usize, bound: &VerifyBound) -> bool {
        match bound {
            VerifyBound::IfEq { ty, bound } => {
                let generic_ty = self.normalize_to_scc_representatives(&tcx.tys,generic_ty);
                let ty = self.normalize_to_scc_representatives(&tcx.tys,*ty);
                let bound = match tcx.tys.region_kind(*bound) {
                    RegionKind::ReBound(..) => *bound,
                    _ => tcx.tys.intern_region(RegionKind::ReVar(self.scc_annotations[self.constraint_sccs.scc(self.to_region_vid(&tcx.tys,*bound))].representative.rvid() as u32)),
                };
                extract_verify_if_eq(tcx,prog,ty,bound,generic_ty).is_some_and(|r| {
                    let (sup, nvars) = (self.to_region_vid(&tcx.tys,r), self.constraint_sccs.num_vars());
                    assert!(sup < nvars && lower < nvars, "IfEq verify bound: sup {sup} ({:?}) lower {lower} of {nvars} region variables; generic_ty {generic_ty:?} ty {ty:?} bound {bound:?}", tcx.tys.region_kind(r));
                    self.eval_outlives(sup,lower)
                })
            }
            VerifyBound::IsEmpty => self.scc_values.elements_contained_in(self.constraint_sccs.scc(lower)).next().is_none(),
            VerifyBound::OutlivedBy(r) => {
                let nvars = self.constraint_sccs.num_vars();
                assert!(*r < nvars && lower < nvars, "OutlivedBy verify bound: bound region {r} lower {lower} of {nvars} region variables; generic_ty {generic_ty:?}");
                self.eval_outlives(*r, lower)
            }
            VerifyBound::AnyBound(bs) => bs.iter().any(|b| self.eval_verify_bound(tcx,prog,generic_ty,lower,b)),
            VerifyBound::AllBounds(bs) => bs.iter().all(|b| self.eval_verify_bound(tcx,prog,generic_ty,lower,b)),
        }
    }
    fn normalize_to_scc_representatives(&self, tys: &crate::types::Types, ty: TyId) -> TyId {
        tys.map_regions(ty,&|r,_| match tys.region_kind(r) {
            RegionKind::ReBound(..) => r,
            _ => tys.intern_region(RegionKind::ReVar(self.scc_annotations[self.constraint_sccs.scc(self.to_region_vid(tys,r))].representative.rvid() as u32)),
        },0)
    }
    fn check_type_tests(&mut self, tcx: &Tcx, prog: &Program, tests: &[TypeTest], nested: bool) {
        let mut failed = vec![];
        for (test,t) in tests.iter().enumerate() {
            if self.eval_verify_bound(tcx,prog,t.generic_kind,t.lower_bound,&t.verify_bound) { continue; }
            if nested && self.try_promote_type_test(tcx,t) { continue; }
            failed.push((tcx.tys.erase_all_regions(t.generic_kind),test));
        }
        let static_scc = self.constraint_sccs.scc(self.universal_region_relations.universal_regions.fr_static);
        let mut reported = crate::fxhash::HashSet::default();
        for &(ty,test) in &failed {
            let t = &tests[test];
            if self.constraint_sccs.scc(t.lower_bound) != static_scc && failed.iter().any(|&(other,i)|
                ty == other && tests[i].span == t.span && self.constraint_sccs.scc(tests[i].lower_bound) == static_scc) { continue; }
            if reported.insert((ty,t.lower_bound,t.span)) { self.errors.push(RegionErrorKind::TypeTest { test }); }
        }
    }
    fn try_promote_type_test(&mut self, tcx: &Tcx, test: &TypeTest) -> bool {
        let Some(subject) = self.try_promote_type_test_subject(tcx,test.generic_kind) else { return false; };
        let scc = self.constraint_sccs.scc(test.lower_bound);
        let universal_regions = &self.universal_region_relations.universal_regions;
        // A placeholder from a higher universe in the lower bound requires `'static`.
        if self.scc_values.elements_contained_in(scc).any(|e| matches!(e,RegionElement::PlaceholderRegion(_))) {
            self.propagated_type_requirements.push(super::ClosureTypeRequirement { subject, outlived_free_region: universal_regions.fr_static, blame_span: test.span });
            return true;
        }
        // For each region outlived by the lower bound, the non-local upper bounds of it; (rustc
        // asserts only that some universal region is outlived, a region may have no non-local bound).
        let mut found_outlived_universal_region = false;
        let mut bounds = Vec::new();
        for ur in self.scc_values.universal_regions_outlived_by(scc) {
            found_outlived_universal_region = true;
            bounds.extend(self.universal_region_relations.non_local_upper_bounds(ur));
        }
        assert!(found_outlived_universal_region, "promotable type test without a universal lower bound");
        self.propagated_type_requirements.extend(bounds.into_iter().map(|outlived_free_region|
            super::ClosureTypeRequirement { subject,outlived_free_region,blame_span:test.span }));
        true
    }
    fn try_promote_type_test_subject(&self, tcx: &Tcx, ty: TyId) -> Option<TyId> {
        let failed = std::cell::Cell::new(false);
        let ty = tcx.tys.map_regions(ty,&|r,_| {
            if matches!(tcx.tys.region_kind(r),RegionKind::ReBound(..)) { return r; }
            let vid = self.to_region_vid(&tcx.tys,r);
            self.universal_regions_outlived_by(vid)
                .filter(|&ur| !self.universal_region_relations.universal_regions.is_local_free_region(ur))
                .find(|&ur| self.eval_equal(ur,vid))
                .map(|ur| tcx.tys.intern_region(RegionKind::ReVar(ur as u32)))
                .unwrap_or_else(|| { failed.set(true); r })
        },0);
        if failed.get() { None } else { Some(ty) }
    }
    fn check_universal_regions(&mut self, tcx: &Tcx, nested: bool) {
        for fr in 0..self.definitions.len() {
            match self.definitions[fr].origin {
                Origin::FreeRegion => self.check_universal_region(tcx, fr, nested),
                Origin::Placeholder(p) => self.check_bound_universal_region(fr, p),
                Origin::Existential => (),
            }
        }
    }
    fn check_universal_region(&mut self, tcx: &Tcx, longer_fr: usize, nested: bool) {
        let scc = self.constraint_sccs.scc(longer_fr);
        let representative = self.scc_annotations[scc].representative.rvid();
        let shorter_regions: Vec<_> = if representative != longer_fr { vec![representative] }
            else { self.scc_values.universal_regions_outlived_by(scc).collect() };
        let mut error_reported = false;
        for shorter_fr in shorter_regions {
            if self.universal_region_relations.outlives(longer_fr, shorter_fr) { continue; }
            if nested && self.try_propagate_universal_region_error(tcx, longer_fr, shorter_fr) { continue; }
            self.errors.push(RegionErrorKind::RegionError { longer_fr, shorter_fr, is_reported: !error_reported });
            error_reported = true;
        }
    }
    fn try_propagate_universal_region_error(&mut self, tcx: &Tcx, longer: usize, shorter: usize) -> bool {
        let minus = self.universal_region_relations.non_local_lower_bounds(longer);
        if minus.is_empty() { return false; }
        let plus = self.universal_region_relations.non_local_upper_bounds(shorter);
        let mut constraints = vec![];
        for a in minus { for &b in &plus { constraints.push((a,b)); } }
        let subset: Vec<_> = constraints.iter().copied().filter(|&(a,b)| self.eval_outlives(a,b)).collect();
        let propagated = if subset.is_empty() { constraints } else { subset };
        assert!(!propagated.is_empty());
        let blame = self.best_blame_constraint(tcx,longer,Origin::FreeRegion,shorter).expect("propagated region relation has no constraint path");
        self.propagated_outlives_requirements.extend(propagated.into_iter().map(|(sup,sub)| OutlivesConstraint { sup,sub,..blame }));
        true
    }
    fn check_bound_universal_region(&mut self, longer_fr: usize, placeholder: crate::types::RegionId) {
        let scc = self.constraint_sccs.scc(longer_fr);
        if let Some(element) = self.scc_values.elements_contained_in(scc).find(|e| *e != RegionElement::PlaceholderRegion(placeholder)) {
            let illegally_outlived_r = match element {
                RegionElement::RootUniversalRegion(r) => r,
                RegionElement::PlaceholderRegion(p) => self.definitions.iter().position(|d| d.origin == Origin::Placeholder(p)).unwrap(),
                RegionElement::Location(p) => self.constraint_path_to(longer_fr, |r| self.liveness_constraints.is_live_at_point_eagerly(r,p), true).unwrap().1,
            };
            self.errors.push(RegionErrorKind::PlaceholderOutlivesIllegalRegion { longer_fr, illegally_outlived_r });
        }
    }
    pub fn constraint_path_to(&self, from: usize, target: impl Fn(usize) -> bool, include_placeholder_static: bool)
        -> Option<(Vec<OutlivesConstraint>, usize)> {
        self.find_constraint_path_between_regions_inner(true, from, &target, include_placeholder_static)
            .or_else(|| self.find_constraint_path_between_regions_inner(false, from, &target, include_placeholder_static))
    }
    fn find_constraint_path_between_regions_inner(&self, ignore_opaque: bool, from: usize,
        target: &impl Fn(usize) -> bool, include_placeholder_static: bool) -> Option<(Vec<OutlivesConstraint>,usize)> {
        let mut context = vec![None; self.definitions.len()];
        let mut visited = vec![false; self.definitions.len()];
        visited[from] = true;
        let mut deque = VecDeque::from([from]);
        let fr_static = self.universal_region_relations.universal_regions.fr_static;
        while let Some(r) = deque.pop_front() {
            crate::preempt::point();
            if target(r) {
                let mut path = vec![];
                let mut p = r;
                while p != from {
                    let c: OutlivesConstraint = context[p].unwrap(); p = c.sup; path.push(c);
                }
                path.reverse(); return Some((path,r));
            }
            let mut handle = |c: OutlivesConstraint| {
                if !visited[c.sub] { visited[c.sub] = true; context[c.sub] = Some(c); deque.push_back(c.sub); }
            };
            if r == fr_static {
                for sub in 0..self.definitions.len() { handle(OutlivesConstraint { sup: r, sub, location: None, span: Default::default(), category: Category::Internal, from_closure: false }); }
            } else {
                for &c in self.constraint_graph.outgoing_edges_from_graph(r, &self.constraints) {
                    if (!include_placeholder_static && matches!(c.category, Category::OutlivesUnnameablePlaceholder(_)))
                        || (ignore_opaque && c.category == Category::OpaqueType) { continue; }
                    handle(c);
                }
            }
        }
        None
    }
    pub fn best_blame_constraint(&self, tcx: &Tcx, from: usize, from_origin: Origin, to: usize) -> Option<OutlivesConstraint> {
        let mut path = self.constraint_path_to(from, |r| r == to, true)?.0;
        if let Some(unnameable) = path.iter().find_map(|c| match c.category { Category::OutlivesUnnameablePlaceholder(r) => Some(r), _ => None }) {
            if unnameable != from { path = self.constraint_path_to(from, |r| r == unnameable, false)?.0; }
        }
        let blame_source = from_origin == Origin::FreeRegion;
        // N:region_infer/region_context.rs:711-727: an implicit unsizing coercion to an ADT
        // over a trait object ranks just after returns when blaming `'static`, unless a
        // type annotation is on the path.
        let fr_static = self.universal_region_relations.universal_regions.fr_static;
        let annotated = path.iter().any(|c| matches!(c.category, Category::TypeAnnotation(_)));
        let interest = |c: &OutlivesConstraint| match c.category {
            Category::Cast { unsize_to: Some(unsize_ty), is_implicit_coercion: true, .. }
                if to == fr_static
                    && matches!(tcx.tys.kind(unsize_ty), TyKind::Adt(_, args, _) if tcx.tys.list(*args).iter().any(|&a| matches!(tcx.tys.kind(a), TyKind::Dyn(..))))
                    && !annotated => 1,
            category => category.interest(),
        };
        let best = if blame_source { path.iter().enumerate().rev().min_by_key(|(_,c)| interest(c)) }
            else { path.iter().enumerate().min_by_key(|(_,c)| interest(c)) }?;
        if matches!(best.1.category, Category::Return(_)) && path.get(best.0 + 1).is_some_and(|c| c.category == Category::OpaqueType) { return Some(path[best.0 + 1]); }
        let mut blamed = path[best.0];
        if blamed.category == Category::Return(ReturnConstraint::Normal) {
            if let Some(field) = path.iter().find_map(|c| if let Category::ClosureUpvar(f) = c.category { Some(f) } else { None }) {
                blamed.category = Category::Return(ReturnConstraint::ClosureUpvar(field));
            }
        }
        Some(blamed)
    }
}

// ported from N:rustc_infer/src/infer/outlives/test_type_match.rs @8d1a764
pub(super) fn extract_verify_if_eq(tcx: &Tcx, prog: &Program, pattern: TyId, bound: RegionId, value: TyId) -> Option<RegionId> {
    let mut matcher = MatchAgainstHigherRankedOutlives { tcx, prog, map: Default::default() };
    if !matcher.tys(pattern,value,0) { return None; }
    Some(match tcx.tys.region_kind(bound) {
        RegionKind::ReBound(0,b) => matcher.map.get(&b).copied().unwrap_or_else(|| tcx.tys.intern_region(RegionKind::ReStatic)),
        RegionKind::ReBound(..) => panic!("escaping verify-if-eq bound"),
        _ => bound,
    })
}
struct MatchAgainstHigherRankedOutlives<'a> {
    tcx: &'a Tcx,
    prog: &'a Program,
    map: crate::fxhash::HashMap<crate::types::BoundVar,RegionId>,
}
impl MatchAgainstHigherRankedOutlives<'_> {
    fn regions(&mut self, pattern: RegionId, value: RegionId, depth: u32) -> bool {
        match self.tcx.tys.region_kind(pattern) {
            RegionKind::ReBound(d,b) if d == depth => *self.map.entry(b).or_insert(value) == value,
            _ => pattern == value,
        }
    }
    fn list(&mut self, a: &[TyId], b: &[TyId], depth: u32) -> bool {
        a.len() == b.len() && a.iter().zip(b).all(|(&a,&b)| self.tys(a,b,depth))
    }
    fn args(&mut self, d: crate::program::DefId, a: &[TyId], b: &[TyId], ar: crate::types::RegionArgsId, br: crate::types::RegionArgsId, depth: u32, variances: bool) -> bool {
        let tcx = self.tcx;
        let indices = if variances { tcx.adts.get(&d).map(|a| &a.generic_indices)
            .or_else(|| tcx.fn_sig(self.prog, d).map(|s| &s.generic_indices)) } else { None };
        let bi = |i| tcx.variances_of(self.prog, d).get(i) == Some(&crate::types::variance::V::Bi);
        if a.len() != b.len() || !a.iter().zip(b).enumerate().all(|(i,(&a,&b))|
            indices.and_then(|g| g.types_consts.get(i)).is_some_and(|&i| bi(i as usize)) || self.tys(a,b,depth)) { return false; }
        let ar = tcx.tys.region_args(ar); let br = tcx.tys.region_args(br);
        ar.len() == br.len() && ar.iter().zip(br.iter()).enumerate().all(|(i,(&a,&b))|
            indices.and_then(|g| g.lifetimes.get(i)).is_some_and(|&i| bi(i as usize)) || self.regions(a,b,depth))
    }
    // Native const arguments participate in the same higher-ranked matching.
    // rustc_infer/src/infer/outlives/test_type_match.rs:207-216 @8d1a764.
    fn consts(&mut self, a: crate::consts::Const, b: crate::consts::Const, depth: u32) -> bool {
        use crate::consts::{ConstKind, GenericArg, ValTreeKind};
        if a == b { return true; }
        let tys = &self.tcx.tys;
        match (tys.const_kind(a),tys.const_kind(b)) {
            (ConstKind::Infer(_),_) | (_,ConstKind::Infer(_)) => panic!("unresolved const inference"),
            (ConstKind::Error,_) | (_,ConstKind::Error) => true,
            (ConstKind::Alias(ar,a),ConstKind::Alias(br,b)) => {
                debug_assert_eq!(ar,br);
                if a.kind != b.kind || a.args.len() != b.args.len() { return false; }
                for (&a,&b) in a.args.iter().zip(b.args.iter()) {
                    let matched = match (a,b) {
                        (GenericArg::Type(a),GenericArg::Type(b)) => self.tys(a,b,depth),
                        (GenericArg::Lifetime(a),GenericArg::Lifetime(b)) => self.regions(a,b,depth),
                        (GenericArg::Const(a),GenericArg::Const(b)) => self.consts(a,b,depth),
                        _ => false,
                    };
                    if !matched { return false; }
                }
                true
            }
            (ConstKind::Value(a),ConstKind::Value(b)) => match (a.valtree.kind(),b.valtree.kind()) {
                (ValTreeKind::Leaf(a),ValTreeKind::Leaf(b)) => a == b,
                (ValTreeKind::Branch(a),ValTreeKind::Branch(b)) => {
                    if a.len() != b.len() { return false; }
                    for i in 0..a.len() { if !self.consts(a[i],b[i],depth) { return false; } }
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }
    fn tys(&mut self, a: TyId, b: TyId, depth: u32) -> bool {
        let tys = &self.tcx.tys;
        if matches!(tys.kind(a),TyKind::Error | TyKind::Bound(..)) { return false; }
        if a == b { return true; }
        match (tys.kind(a),tys.kind(b)) {
            (TyKind::Ref(am,a,ar),TyKind::Ref(bm,b,br)) => am == bm && self.regions(*ar,*br,depth) && self.tys(*a,*b,depth),
            (TyKind::Ptr(am,a),TyKind::Ptr(bm,b)) => am == bm && self.tys(*a,*b,depth),
            (TyKind::Slice(a),TyKind::Slice(b)) => self.tys(*a,*b,depth),
            (TyKind::Tuple(a),TyKind::Tuple(b)) => self.list(tys.list(*a),tys.list(*b),depth),
            (TyKind::Const(a),TyKind::Const(b)) => self.consts(*a,*b,depth),
            (TyKind::Array(a,an),TyKind::Array(b,bn)) => self.tys(*a,*b,depth) && self.consts(*an,*bn,depth),
            (TyKind::Adt(ad,a,ar),TyKind::Adt(bd,b,br)) | (TyKind::FnDef(ad,a,ar),TyKind::FnDef(bd,b,br)) =>
                ad == bd && self.args(*ad,tys.list(*a),tys.list(*b),*ar,*br,depth,true),
            (TyKind::Alias(_, AliasTy { kind: ak, args: a, regions: ar }),TyKind::Alias(_, AliasTy { kind: bk, args: b, regions: br }))
                if matches!(ak, AliasTyKind::Projection { .. } | AliasTyKind::Opaque { .. }) => ak == bk && {
                    // Relate<AliasTy> (rustc_type_ir/src/relate.rs:224-240): opaques and RPITIT
                    // projections relate through their declared variances (opt_alias_variances,
                    // rustc_middle/src/ty/util.rs:954-974; bivariant arguments are skipped), other
                    // projections invariantly.
                    let declared = self.tcx.alias_declared_variances(self.prog, *ak);
                    let bi = |region: bool, i: usize| declared.as_ref().is_some_and(|(indices, vs)| {
                        let indices = if region { &indices.lifetimes } else { &indices.types_consts };
                        indices.get(i).is_some_and(|&i| vs.get(i as usize) == Some(&crate::types::variance::V::Bi))
                    });
                    let ar = tys.region_args(*ar); let br = tys.region_args(*br);
                    tys.list(*a).len() == tys.list(*b).len() && ar.len() == br.len()
                        && tys.list(*a).iter().zip(tys.list(*b)).enumerate().all(|(i,(&a,&b))| bi(false,i) || self.tys(a,b,depth))
                        && ar.iter().zip(br.iter()).enumerate().all(|(i,(&a,&b))| bi(true,i) || self.regions(a,b,depth))
                },
            (TyKind::FnPtr(a,ao,ah,_),TyKind::FnPtr(b,bo,bh,_)) => ah == bh && self.list(tys.list(*a),tys.list(*b),depth+1) && self.tys(*ao,*bo,depth+1),
            (TyKind::Closure(af, ae, crate::types::ClosureArgs { parent_args: ao, closure_kind_ty: ak, closure_sig_as_fn_ptr_ty: a, tupled_upvars_ty: au }),TyKind::Closure(bf, be, crate::types::ClosureArgs { parent_args: bo, closure_kind_ty: bk, closure_sig_as_fn_ptr_ty: b, tupled_upvars_ty: bu })) => af == bf && ae == be
                && self.tys(*ak,*bk,depth) && self.tys(*a,*b,depth) && self.tys(*au,*bu,depth) && self.tys(*ao,*bo,depth),
            (TyKind::Dyn(ad,a,ab,ar,al,_,aa),TyKind::Dyn(bd,b,bb,br,bl,_,ba)) => ad == bd && aa == ba
                && self.args(*ad,tys.list(*a),tys.list(*b),*ar,*br,depth+1,false) && self.regions(*al,*bl,depth)
                && tys.list(*ab).len() == tys.list(*bb).len() && tys.list(*ab).iter().zip(tys.list(*bb)).all(|((ad,a,_),(bd,b,_))| ad == bd && self.tys(*a,*b,depth+1)),
            _ => false,
        }
    }
}
