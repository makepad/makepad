// ported from N:rustc_borrowck/src/type_check/relate_tys.rs @8d1a764
// with the walk it drives: N:rustc_type_ir/src/relate.rs (structurally_relate_tys, FnSig,
// existential predicates, args) and N:rustc_type_ir/src/relate/combine.rs
// (super_combine_tys, combine_ty_args) @8d1a764, over Relax's TyKind. Relax runs the next
// solver only: unnormalized aliases and inference variables on the right are bugs, and
// opaque relations belong to N01's opaque storage (`BorrowckTypeOps::relate_opaque`).
use super::*;
use crate::types::fold::TypeFolder;
use crate::types::variance::xform;
use crate::types::RegionArgsId;

/// rustc_middle::traits::query::NoSolution: the types cannot be related.
pub use crate::next_solver::NoSolution;

impl<'a> TypeChecker<'a> {
    /// Adds constraints so that `a R b` where R is `v`: Co `a <: b`, Inv `a == b`, Contra `a :> b`.
    /// `a` may contain unresolved inference variables, `b` may not.
    pub(super) fn relate_types(&mut self, a: TyId, v: V, b: TyId, locations: Locations, category: ConstraintCategory) -> Result<(), NoSolution> {
        NllTypeRelating::new(self, locations, category, UniverseInfo::RelateTys { expected: a, found: b }, v).relate(a, b)
    }
    /// Add constraints so that `a == b` argument-wise.
    pub(super) fn eq_args(&mut self, a: &[GenericArg], b: &[GenericArg], locations: Locations, category: ConstraintCategory) -> Result<(), NoSolution> {
        let mut relating = NllTypeRelating::new(self, locations, category, UniverseInfo::Other, V::Inv);
        for (&a, &b) in a.iter().zip(b) { relating.arg(a, b)?; }
        Ok(())
    }
}

/// Values under one binder: a fn signature's inputs and output, a dyn principal's
/// arguments and lifetime arguments, or an associated-type binding's term.
#[derive(Clone, Debug)]
struct Bound { tys: Vec<TyId>, regions: Vec<RegionId> }

struct NllTypeRelating<'r, 'a> {
    type_checker: &'r mut TypeChecker<'a>,
    /// Where (and why) is this relation taking place?
    locations: Locations,
    /// The category of the resulting `'a: 'b` relationships.
    category: ConstraintCategory,
    /// What is being related, for bound region errors.
    universe_info: UniverseInfo,
    /// Covariant `a <: b`, Contravariant `b <: a`, Invariant `a == b`, Bivariant: no relation.
    ambient_variance: V,
}

impl<'r, 'a> NllTypeRelating<'r, 'a> {
    fn new(type_checker: &'r mut TypeChecker<'a>, locations: Locations, category: ConstraintCategory,
        universe_info: UniverseInfo, ambient_variance: V) -> Self {
        Self { type_checker, locations, category, universe_info, ambient_variance }
    }
    fn tcx(&self) -> &'a Tcx { self.type_checker.tcx }
    fn span(&self) -> Span { self.locations.span(self.type_checker.body) }
    fn ambient_covariance(&self) -> bool { matches!(self.ambient_variance, V::Co | V::Inv) }
    fn ambient_contravariance(&self) -> bool { matches!(self.ambient_variance, V::Contra | V::Inv) }

    fn no_bound_vars(&self, value: &Bound) -> bool {
        let tys = &self.tcx().tys;
        !value.tys.iter().any(|&t| tys.has_vars_bound_at_or_above(t, 0))
            && !value.regions.iter().any(|&r| matches!(tys.region_kind(r), RegionKind::ReBound(0, _)))
    }

    /// Instantiate the binder with placeholders in a new universe, created lazily on
    /// the first bound region.
    fn enter_forall(&mut self, binder: &Bound) -> Bound {
        if self.no_bound_vars(binder) { return binder.clone(); }
        let tcx = self.tcx();
        let mut lazy_universe = None;
        let mut replacer = BoundVarReplacer::new(|var| {
            let universe = match lazy_universe {
                Some(u) => u,
                None => { let u = self.create_next_universe(); lazy_universe = Some(u); u }
            };
            let placeholder = tcx.tys.intern_region(RegionKind::RePlaceholder { universe, var });
            self.next_placeholder_region(placeholder)
        }, |_| unreachable!("we only replace regions in nll_relate, not types"));
        Bound { tys: binder.tys.iter().map(|&t| replacer.instantiate(&tcx.tys, t)).collect(),
            regions: binder.regions.iter().map(|&r| replacer.fold_region(&tcx.tys, r, 0)).collect() }
    }

    fn instantiate_binder_with_existentials(&mut self, binder: &Bound) -> Bound {
        if self.no_bound_vars(binder) { return binder.clone(); }
        let tcx = self.tcx();
        // The replacer creates one existential per bound region.
        let mut replacer = BoundVarReplacer::new(|_| self.next_existential_region_var(),
            |_| unreachable!("we only replace regions in nll_relate, not types"));
        Bound { tys: binder.tys.iter().map(|&t| replacer.instantiate(&tcx.tys, t)).collect(),
            regions: binder.regions.iter().map(|&r| replacer.fold_region(&tcx.tys, r, 0)).collect() }
    }

    fn create_next_universe(&mut self) -> UniverseIndex {
        let universe = self.type_checker.infcx.create_next_universe();
        self.type_checker.constraints.universe_causes.push((universe, self.universe_info.clone()));
        universe
    }
    fn next_existential_region_var(&mut self) -> RegionId {
        let universe = self.type_checker.infcx.universe();
        self.type_checker.infcx.next_nll_region_var(NllRegionVariableOrigin::Existential, universe)
    }
    fn next_placeholder_region(&mut self, placeholder: RegionId) -> RegionId {
        let tc = &mut *self.type_checker;
        tc.constraints.placeholder_region(&mut *tc.infcx, tc.tcx, placeholder)
    }

    fn push_outlives(&mut self, sup: RegionId, sub: RegionId) {
        let tc = &mut *self.type_checker;
        let sub = tc.infcx.to_region_vid(sub);
        let sup = tc.infcx.to_region_vid(sup);
        let location = self.locations.from_location().map(|l| tc.location_map.point(l));
        let span = self.locations.span(tc.body);
        tc.constraints.outlives_constraints.push(OutlivesConstraint { sup, sub, location, span, category: self.category, from_closure: false });
    }

    /// PredicateEmittingRelation::register_predicates of WellFormed clauses.
    fn register_wf(&mut self, ty: TyId) {
        let tc = &mut *self.type_checker;
        tc.infcx.prove_wf(&mut tc.constraints, ty, self.locations, self.category);
    }

    fn relate(&mut self, a: TyId, b: TyId) -> Result<(), NoSolution> { self.tys(a, b) }

    fn relate_with_variance(&mut self, variance: V, a: TyId, b: TyId) -> Result<(), NoSolution> {
        let old_ambient_variance = self.ambient_variance;
        self.ambient_variance = xform(self.ambient_variance, variance);
        // In a bivariant context this always succeeds.
        let r = if self.ambient_variance == V::Bi { Ok(()) } else { self.relate(a, b) };
        self.ambient_variance = old_ambient_variance;
        r
    }
    fn relate_regions_with_variance(&mut self, variance: V, a: RegionId, b: RegionId) {
        let old_ambient_variance = self.ambient_variance;
        self.ambient_variance = xform(self.ambient_variance, variance);
        if self.ambient_variance != V::Bi { self.regions(a, b); }
        self.ambient_variance = old_ambient_variance;
    }

    /// relate_ty_args: declared variances outside invariant contexts (combine_ty_args).
    fn relate_ty_args(&mut self, a_ty: TyId, b_ty: TyId, def: DefId, a_args: (&[TyId], RegionArgsId), b_args: (&[TyId], RegionArgsId))
        -> Result<(), NoSolution> {
        let tcx = self.tcx();
        let (a_regions, b_regions) = (tcx.tys.region_args(a_args.1), tcx.tys.region_args(b_args.1));
        if a_args.0.len() != b_args.0.len() || a_regions.len() != b_regions.len() { return Err(NoSolution); }
        // Relax keeps type/const and lifetime arguments in two lists; nightly relates the
        // one GenericArgs list in declaration order (relate_args_invariantly and
        // combine_ty_args), so the arguments are related in that merged order. An
        // argument without a declaration index keeps its list position after the others.
        let indices = match tcx.tys.kind(a_ty) {
            TyKind::Adt(..) => tcx.adts.get(&def).map(|a| &a.generic_indices),
            _ => tcx.fn_item_sig(self.type_checker.prog, def).map(|s| &s.generic_indices),
        };
        let mut order: Vec<(u32, bool, usize)> = (0..a_args.0.len())
            .map(|i| (indices.and_then(|x| x.types_consts.get(i)).copied().unwrap_or(u32::MAX), false, i))
            .chain((0..a_regions.len()).map(|i| (indices.and_then(|x| x.lifetimes.get(i)).copied().unwrap_or(u32::MAX), true, i)))
            .collect();
        order.sort_by_key(|&(full, _, _)| full);
        if self.ambient_variance == V::Inv {
            for &(_, region, i) in &order {
                if region { self.relate_regions_with_variance(V::Inv, a_regions[i], b_regions[i]); }
                else { self.relate_with_variance(V::Inv, a_args.0[i], b_args.0[i])?; }
            }
            return Ok(());
        }
        // Variances are in declaration order. A missing declaration is invariant, as in variance.rs.
        let variances = tcx.variances_of(self.type_checker.prog, def);
        let variance = |full: u32| variances.get(full as usize).copied().unwrap_or(V::Inv);
        let mut has_unconstrained_bivariant_arg = false;
        for &(full, region, i) in &order {
            let v = variance(full);
            if region {
                self.relate_regions_with_variance(v, a_regions[i], b_regions[i]);
                continue;
            }
            let (a, b) = (a_args.0[i], b_args.0[i]);
            if v == V::Bi && (self.has_non_region_infer(a) || self.has_non_region_infer(b)) {
                has_unconstrained_bivariant_arg = true;
            }
            self.relate_with_variance(v, a, b)?;
        }
        // A generalized bivariant argument may stay unconstrained (#41677); WF of both
        // types constrains it through the where-clause that makes it bivariant.
        if has_unconstrained_bivariant_arg {
            self.register_wf(a_ty);
            self.register_wf(b_ty);
        }
        Ok(())
    }
    fn has_non_region_infer(&mut self, t: TyId) -> bool {
        let tys = &self.tcx().tys;
        tys.has_infer(t) && { let r = self.type_checker.infcx.shallow_resolve(t); tys.has_infer(r) }
    }

    fn tys(&mut self, a: TyId, b: TyId) -> Result<(), NoSolution> {
        let tcx = self.tcx();
        let a = self.type_checker.infcx.shallow_resolve(a);
        if a == b { return Ok(()); }
        match (tcx.tys.kind(a), tcx.tys.kind(b)) {
            // Type variables on the right are a bug in MIR typeck.
            (_, TyKind::Infer(_)) => Err(NoSolution),
            (&TyKind::Infer(a_vid), _) => {
                // instantiate_ty_var(target_is_expected = true): relate generalized <rel> b.
                let span = self.span();
                let generalized = self.type_checker.infcx.generalize_ty_var(a_vid, self.ambient_variance, b, span)?;
                self.relate(generalized, b)
            }
            (TyKind::Alias(_, crate::types::AliasTy { kind: crate::types::AliasTyKind::Opaque { .. }, .. }), _) | (_, TyKind::Alias(_, crate::types::AliasTy { kind: crate::types::AliasTyKind::Opaque { .. }, .. })) => {
                let tc = &mut *self.type_checker;
                tc.infcx.relate_opaque(&mut tc.constraints, a, self.ambient_variance, b, self.locations, self.category)
            }
            _ => self.super_combine_tys(a, b),
        }
    }

    fn arg(&mut self, a: GenericArg, b: GenericArg) -> Result<(), NoSolution> {
        match (a, b) {
            (GenericArg::Type(a), GenericArg::Type(b)) => self.relate_with_variance(V::Inv, a, b),
            (GenericArg::Lifetime(a), GenericArg::Lifetime(b)) => { self.relate_regions_with_variance(V::Inv, a, b); Ok(()) }
            (GenericArg::Const(a), GenericArg::Const(b)) => self.consts(a, b),
            _ => Err(NoSolution),
        }
    }

    // N:relate_tys.rs:477-487 and rustc_type_ir/src/relate.rs:552-633.
    fn consts(&mut self, a: crate::consts::Const, b: crate::consts::Const) -> Result<(), NoSolution> {
        use crate::consts::{ConstKind, ValTreeKind};
        let tys = &self.tcx().tys;
        let a = self.type_checker.infcx.shallow_resolve_const(a);
        assert!(!a.has_non_region_infer(tys) && !b.has_non_region_infer(tys), "unexpected const inference in MIR typeck");
        if a == b { return Ok(()); }
        match (tys.const_kind(a), tys.const_kind(b)) {
            (ConstKind::Error, _) | (_, ConstKind::Error) => { self.type_checker.tainted_by_errors = true; Ok(()) }
            (ConstKind::Param(a), ConstKind::Param(b)) if a.index == b.index => Ok(()),
            (ConstKind::Placeholder(a), ConstKind::Placeholder(b)) if a == b => Ok(()),
            (ConstKind::Value(a), ConstKind::Value(b)) => match (a.valtree.kind(), b.valtree.kind()) {
                (ValTreeKind::Leaf(a), ValTreeKind::Leaf(b)) if a == b => Ok(()),
                (ValTreeKind::Branch(a), ValTreeKind::Branch(b)) if a.len() == b.len() => {
                    for (&a, &b) in a.iter().zip(b.iter()) { self.consts(a, b)?; }
                    Ok(())
                }
                _ => Err(NoSolution),
            },
            (ConstKind::Alias(ar, a), ConstKind::Alias(br, b)) => {
                debug_assert_eq!(ar, br);
                if a.kind != b.kind || a.args.len() != b.args.len() { return Err(NoSolution); }
                for (&a, &b) in a.args.iter().zip(b.args.iter()) { self.arg(a, b)?; }
                Ok(())
            }
            _ => Err(NoSolution),
        }
    }

    fn regions(&mut self, a: RegionId, b: RegionId) {
        if self.ambient_covariance() {
            // Covariant: &'a u8 <: &'b u8. Hence, `'a: 'b`.
            self.push_outlives(a, b);
        }
        if self.ambient_contravariance() {
            // Contravariant: &'b u8 <: &'a u8. Hence, `'b: 'a`.
            self.push_outlives(b, a);
        }
    }

    /// `for<..> A <: for<..> B`: B's bound regions become placeholders (universe U1), then
    /// A's become existentials in U1; never the other way round.
    fn binders(&mut self, a: &Bound, b: &Bound, relate: &mut dyn FnMut(&mut Self, &Bound, &Bound) -> Result<(), NoSolution>)
        -> Result<(), NoSolution> {
        if self.no_bound_vars(a) && self.no_bound_vars(b) { return relate(self, a, b); }
        match self.ambient_variance {
            V::Co => {
                // Any instantiation of A against every instantiation of B. Placeholders
                // first, or the existentials get the wrong universe.
                let b = self.enter_forall(b);
                let a = self.instantiate_binder_with_existentials(a);
                relate(self, &a, &b)
            }
            V::Contra => {
                let a = self.enter_forall(a);
                let b = self.instantiate_binder_with_existentials(b);
                relate(self, &a, &b)
            }
            V::Inv => {
                // `exists<..> A == for<..> B` and `exists<..> B == for<..> A`.
                let b2 = self.enter_forall(b);
                let a2 = self.instantiate_binder_with_existentials(a);
                relate(self, &a2, &b2)?;
                let a3 = self.enter_forall(a);
                let b3 = self.instantiate_binder_with_existentials(b);
                relate(self, &a3, &b3)
            }
            V::Bi => Ok(()),
        }
    }

    /// Relate for FnSig: headers must agree; inputs are contravariant.
    fn fn_sigs(&mut self, a: &Bound, a_hdr: FnHeader, b: &Bound, b_hdr: FnHeader) -> Result<(), NoSolution> {
        if a_hdr.c_variadic != b_hdr.c_variadic || a_hdr.safety != b_hdr.safety || a_hdr.abi != b_hdr.abi { return Err(NoSolution); }
        if a.tys.len() != b.tys.len() { return Err(NoSolution); }
        let n = a.tys.len() - 1;
        for i in 0..n { self.relate_with_variance(V::Contra, a.tys[i], b.tys[i])?; }
        self.relate(a.tys[n], b.tys[n])
    }

    fn super_combine_tys(&mut self, a: TyId, b: TyId) -> Result<(), NoSolution> {
        let tys = &self.tcx().tys;
        match (tys.kind(a), tys.kind(b)) {
            (TyKind::Error, _) | (_, TyKind::Error) => { self.type_checker.tainted_by_errors = true; Ok(()) }
            // All other cases of inference are errors.
            (TyKind::Infer(_), _) | (_, TyKind::Infer(_)) => Err(NoSolution),
            _ => self.structurally_relate_tys(a, b),
        }
    }

    fn structurally_relate_tys(&mut self, a: TyId, b: TyId) -> Result<(), NoSolution> {
        let tcx = self.tcx();
        let tys = &tcx.tys;
        match (tys.kind(a).clone(), tys.kind(b).clone()) {
            (TyKind::Bound(..), _) | (_, TyKind::Bound(..)) => Err(NoSolution),
            (TyKind::Adt(ad, ats, ars), TyKind::Adt(bd, bts, brs)) if ad == bd => self.relate_ty_args(a, b, ad, (tys.list(ats), ars), (tys.list(bts), brs)),
            (TyKind::FnDef(ad, ats, ars), TyKind::FnDef(bd, bts, brs)) if ad == bd => self.relate_ty_args(a, b, ad, (tys.list(ats), ars), (tys.list(bts), brs)),
            (TyKind::Dyn(ad, aargs, abind, ars, ar, _, aautos), TyKind::Dyn(bd, bargs, bbind, brs, br, _, bautos)) => {
                // Existential predicates: same principal, projections and auto traits, each under its binder.
                if ad != bd || tys.list(abind).len() != tys.list(bbind).len() || aautos != bautos || tys.list(aargs).len() != tys.list(bargs).len() { return Err(NoSolution); }
                let a_principal = Bound { tys: tys.list(aargs).to_vec(), regions: tys.region_args(ars).to_vec() };
                let b_principal = Bound { tys: tys.list(bargs).to_vec(), regions: tys.region_args(brs).to_vec() };
                self.binders(&a_principal, &b_principal, &mut |this, a, b| {
                    for (&a, &b) in a.tys.iter().zip(&b.tys) { this.relate_with_variance(V::Inv, a, b)?; }
                    for (&a, &b) in a.regions.iter().zip(&b.regions) { this.relate_regions_with_variance(V::Inv, a, b); }
                    Ok(())
                })?;
                for (&(a_item, a_term, _), &(b_item, b_term, _)) in tys.list(abind).iter().zip(tys.list(bbind)) {
                    if a_item != b_item { return Err(NoSolution); }
                    self.binders(&Bound { tys: vec![a_term], regions: vec![] }, &Bound { tys: vec![b_term], regions: vec![] },
                        &mut |this, a, b| this.relate_with_variance(V::Inv, a.tys[0], b.tys[0]))?;
                }
                self.regions(ar, br);
                Ok(())
            }
            // All closure types of one expression are the same closure: equate all their regions.
            (TyKind::Closure(af, ae, aa), TyKind::Closure(bf, be, ba)) if (af, ae) == (bf, be) => {
                // ClosureArgs order: parent args, kind, signature, then upvars.
                for (a, b) in [(aa.parent_args, ba.parent_args), (aa.closure_kind_ty, ba.closure_kind_ty), (aa.closure_sig_as_fn_ptr_ty, ba.closure_sig_as_fn_ptr_ty), (aa.tupled_upvars_ty, ba.tupled_upvars_ty)] { self.relate_with_variance(V::Inv, a, b)?; }
                Ok(())
            }
            (TyKind::Ptr(am, at), TyKind::Ptr(bm, bt)) => {
                if am != bm { return Err(NoSolution); }
                self.relate_with_variance(if am { V::Inv } else { V::Co }, at, bt)
            }
            (TyKind::Ref(am, at, ar), TyKind::Ref(bm, bt, br)) => {
                if am != bm { return Err(NoSolution); }
                self.regions(ar, br);
                self.relate_with_variance(if am { V::Inv } else { V::Co }, at, bt)
            }
            (TyKind::Array(at, alen), TyKind::Array(bt, blen)) => {
                self.relate(at, bt)?;
                self.consts(alen, blen)
            }
            (TyKind::Slice(at), TyKind::Slice(bt)) => self.relate(at, bt),
            (TyKind::Tuple(as_), TyKind::Tuple(bs)) => {
                if tys.list(as_).len() != tys.list(bs).len() { return Err(NoSolution); }
                for (&a, &b) in tys.list(as_).iter().zip(tys.list(bs)) { self.relate(a, b)?; }
                Ok(())
            }
            (TyKind::FnPtr(ai, ao, ah, _), TyKind::FnPtr(bi, bo, bh, _)) => {
                let a_sig = Bound { tys: tys.list(ai).iter().copied().chain([ao]).collect(), regions: vec![] };
                let b_sig = Bound { tys: tys.list(bi).iter().copied().chain([bo]).collect(), regions: vec![] };
                self.binders(&a_sig, &b_sig, &mut |this, a, b| this.fn_sigs(a, ah, b, bh))
            }
            // Rigid projections (normalization already failed to make progress).
            (TyKind::Const(a), TyKind::Const(b)) => self.consts(a, b),
            (TyKind::Alias(ar, aa), TyKind::Alias(br, ba)) if ar == br && aa.kind == ba.kind => {
                let (ats, ars, bts, brs) = (aa.args, aa.regions, ba.args, ba.regions);
                if tys.list(ats).len() != tys.list(bts).len() || tys.region_args(ars).len() != tys.region_args(brs).len() { return Err(NoSolution); }
                for (&a, &b) in tys.list(ats).iter().zip(tys.list(bts)) { self.relate_with_variance(V::Inv, a, b)?; }
                for (&a, &b) in tys.region_args(ars).iter().zip(tys.region_args(brs).iter()) { self.relate_regions_with_variance(V::Inv, a, b); }
                Ok(())
            }
            _ => Err(NoSolution),
        }
    }
}
