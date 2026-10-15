// ported from N:rustc_borrowck/src/type_check/constraint_conversion.rs @8d1a764
use super::{Locations, MirTypeckRegionConstraints};
use crate::mir::Span;
use crate::borrowck::constraints::{ConstraintCategory, OutlivesConstraint, RegionVid};
use crate::borrowck::infer_ctxt::BorrowckInferCtxt;
use crate::borrowck::region_infer::{TypeTest, VerifyBound};
use crate::borrowck::universal_regions::UniversalRegions;
use crate::infer::outlives::obligations::{TypeOutlives, TypeOutlivesDelegate};
use crate::infer::outlives::verify;
use crate::next_solver::type_op::QueryRegionConstraints;
use crate::next_solver::{ClauseKind, Predicate, PredicateKind};
use crate::tcx::outlives::OutlivesPredicate;
use crate::types::{RegionId, RegionKind, TyId, TypeFlags};

pub struct ConstraintConversion<'a, 'tcx> {
    infcx: &'a BorrowckInferCtxt<'tcx>,
    universal_regions: &'a UniversalRegions,
    /// Each RBP `GK: 'a` is assumed to be true. These encode relationships like `T: 'a`
    /// that are added via implicit bounds or the `param_env`. The regions are the
    /// "original" ones (keys of the universal indices), not their region variables.
    region_bound_pairs: &'a [(TyId, RegionId)],
    known_type_outlives_obligations: &'a [Predicate],
    locations: Locations,
    span: Span,
    category: ConstraintCategory,
    constraints: &'a mut MirTypeckRegionConstraints,
}

impl<'a, 'tcx> ConstraintConversion<'a, 'tcx> {
    #[allow(clippy::too_many_arguments)]
    pub fn new(
        infcx: &'a BorrowckInferCtxt<'tcx>,
        universal_regions: &'a UniversalRegions,
        region_bound_pairs: &'a [(TyId, RegionId)],
        known_type_outlives_obligations: &'a [Predicate],
        locations: Locations,
        span: Span,
        category: ConstraintCategory,
        constraints: &'a mut MirTypeckRegionConstraints,
    ) -> Self {
        Self { infcx, universal_regions, region_bound_pairs, known_type_outlives_obligations, locations, span, category, constraints }
    }

    pub fn convert_all(&mut self, query_constraints: &QueryRegionConstraints) {
        for c in &query_constraints.constraints {
            for predicate in c.constraint.iter_outlives() { self.convert(predicate, c.category); }
        }
    }

    fn convert(&mut self, clause: OutlivesPredicate, constraint_category: ConstraintCategory) {
        match clause {
            OutlivesPredicate::Region(r1, r2) => {
                let r1_vid = self.to_region_vid(r1);
                let r2_vid = self.to_region_vid(r2);
                self.add_outlives(r1_vid, r2_vid, constraint_category, self.span);
            }
            OutlivesPredicate::Type(t1, r2) => {
                // Scraped constraints may have had inference vars.
                let t1 = self.infcx.deeply_resolve_ignoring_regions(t1);
                let tcx = self.infcx.tcx;
                let implicit_region_bound = self.universal_regions.implicit_region_bound()
                    .map(|fr| tcx.tys.intern_region(RegionKind::ReVar(fr as u32)));
                let (prog, region_bound_pairs, known) = (self.infcx.prog, self.region_bound_pairs, self.known_type_outlives_obligations);
                TypeOutlives::new(self, tcx, prog, region_bound_pairs, implicit_region_bound, known)
                    .type_must_outlive(t1, r2, constraint_category);
            }
        }
    }

    /// Placeholder regions need to be converted eagerly because it may create new region
    /// variables, which we must not do when verifying our region bounds.
    fn replace_placeholders_with_nll(&mut self, value: TyId) -> TyId {
        let tys = &self.infcx.tcx.tys;
        if !tys.flags(value).intersects(TypeFlags::HAS_RE_PLACEHOLDER) { return value; }
        let mut placeholders = Vec::new();
        collect_placeholders(tys, value, &mut placeholders);
        let replaced: Vec<_> = placeholders.iter().map(|&p| (p, self.placeholder_region(p))).collect();
        tys.map_regions(value, &|r, _| replaced.iter().find(|(p, _)| *p == r).map_or(r, |(_, v)| *v), 0)
    }

    /// The region-inference representation of a verify bound: Relax's region solver
    /// evaluates bounds over region variables, so the "original" regions of the bound
    /// (and of its `IfEq` types) are mapped here, after placeholder replacement.
    fn verify_bound_to_nll(&mut self, bound: verify::VerifyBound) -> VerifyBound {
        match bound {
            verify::VerifyBound::OutlivedBy(r) => VerifyBound::OutlivedBy(self.to_region_vid(r)),
            verify::VerifyBound::IsEmpty => VerifyBound::IsEmpty,
            verify::VerifyBound::AnyBound(bs) => VerifyBound::AnyBound(bs.into_iter().map(|b| self.verify_bound_to_nll(b)).collect()),
            verify::VerifyBound::AllBounds(bs) => VerifyBound::AllBounds(bs.into_iter().map(|b| self.verify_bound_to_nll(b)).collect()),
            verify::VerifyBound::IfEq(p) => {
                let PredicateKind::Clause(ClauseKind::TypeOutlives(ty, bound)) = p.kind else { unreachable!() };
                let ty = self.replace_placeholders_with_nll(ty);
                let ty = self.fold_to_region_vids(ty);
                let bound = match self.infcx.tcx.tys.region_kind(bound) {
                    RegionKind::ReBound(..) => bound,
                    _ => { let vid = self.to_region_vid(bound); self.infcx.tcx.tys.intern_region(RegionKind::ReVar(vid as u32)) }
                };
                VerifyBound::IfEq { ty, bound }
            }
        }
    }

    fn fold_to_region_vids(&self, ty: TyId) -> TyId {
        let tys = &self.infcx.tcx.tys;
        let universal_regions = self.universal_regions;
        tys.map_regions(ty, &|r, depth| match tys.region_kind(r) {
            RegionKind::ReBound(debruijn, _) if debruijn < depth => r,
            _ => tys.intern_region(RegionKind::ReVar(universal_regions.to_region_vid(tys, r) as u32)),
        }, 0)
    }

    fn verify_to_type_test(&mut self, generic_kind: TyId, region: RegionId, verify_bound: VerifyBound) -> TypeTest {
        let lower_bound = self.to_region_vid(region);
        TypeTest { generic_kind, lower_bound, span: crate::borrowck::constraints::Span { lo: self.span.lo, hi: self.span.hi }, verify_bound }
    }

    fn placeholder_region(&mut self, placeholder: RegionId) -> RegionId {
        let index = self.constraints.placeholder_indices.insert(placeholder);
        if let Some(&r) = self.constraints.placeholder_index_to_region.get(index) { return r; }
        let RegionKind::RePlaceholder { universe, .. } = self.infcx.tcx.tys.region_kind(placeholder) else { unreachable!() };
        let region = self.infcx.next_nll_region_var_in_universe(crate::borrowck::region_infer::NllRegionVariableOrigin::Placeholder(placeholder), universe);
        self.constraints.placeholder_index_to_region.push(region);
        region
    }

    fn to_region_vid(&mut self, r: RegionId) -> RegionVid {
        let tys = &self.infcx.tcx.tys;
        if let RegionKind::RePlaceholder { .. } = tys.region_kind(r) {
            let RegionKind::ReVar(vid) = tys.region_kind(self.placeholder_region(r)) else { unreachable!() };
            vid as RegionVid
        } else {
            self.universal_regions.to_region_vid(tys, r)
        }
    }

    fn add_outlives(&mut self, sup: RegionVid, sub: RegionVid, category: ConstraintCategory, span: Span) {
        let category = match self.category {
            ConstraintCategory::Boring | ConstraintCategory::BoringNoLocation => category,
            _ => self.category,
        };
        let location = match self.locations { Locations::All(_) => None, Locations::Single(location) => Some(self.infcx.location_point(location)) };
        self.constraints.outlives_constraints.push(OutlivesConstraint { sup, sub, location, span, category, from_closure: false });
    }

    fn add_type_test(&mut self, type_test: TypeTest) {
        self.constraints.type_tests.push(type_test);
    }
}

fn collect_placeholders(tys: &crate::types::Types, ty: TyId, out: &mut Vec<RegionId>) {
    let found = std::cell::RefCell::new(Vec::new());
    tys.map_regions(ty, &|r, _| {
        if matches!(tys.region_kind(r), RegionKind::RePlaceholder { .. }) && !found.borrow().contains(&r) { found.borrow_mut().push(r); }
        r
    }, 0);
    out.extend(found.into_inner());
}

impl TypeOutlivesDelegate for ConstraintConversion<'_, '_> {
    fn delayed_bug(&mut self, message: String) { self.infcx.errors.delayed_bug(self.infcx.cause, message); }
    fn push_sub_region_constraint(&mut self, a: RegionId, b: RegionId, constraint_category: ConstraintCategory) {
        let b = self.to_region_vid(b);
        let a = self.to_region_vid(a);
        self.add_outlives(b, a, constraint_category, self.span);
    }

    fn push_verify(&mut self, kind: TyId, a: RegionId, bound: verify::VerifyBound) {
        let kind = self.replace_placeholders_with_nll(kind);
        let kind = self.fold_to_region_vids(kind);
        let bound = self.verify_bound_to_nll(bound);
        let type_test = self.verify_to_type_test(kind, a, bound);
        self.add_type_test(type_test);
    }
}
