// ported from N:rustc_borrowck/src/region_infer/opaque_types/mod.rs @8d1a764
use crate::types::AliasTy;
mod member_constraints;
mod region_ctxt;
use region_ctxt::RegionCtxt;
use super::{NllRegionVariableOrigin, RegionDefinition};
use crate::borrowck::type_check::free_region_relations::UniversalRegionRelations;
use crate::borrowck::universal_regions::RegionClassification;
use crate::types::{RegionId, RegionKind, TyId, TyKind};
use crate::tcx::Tcx;
use crate::program::{DefId, Program};

use crate::consts::GenericArg;
use crate::infer::opaque_types::{OpaqueTypeKey, OpaqueTypeStorageEntries, ProvisionalHiddenType};
use crate::borrowck::type_check::{BorrowckTypeOps, MirTypeckRegionConstraints};

/// A use of an opaque type whose arguments are all universal regions, types and consts.
#[derive(Debug)]
pub struct DefiningUse {
    /// The opaque type using non-NLL regions: the actual free regions and placeholders, as the
    /// code outside of borrowck sees them.
    pub key: OpaqueTypeKey,
    /// 'static, then the NLL variables of the key's captured lifetime arguments.
    pub arg_regions: Vec<usize>,
    pub hidden_type: TyId,
    /// Where the use was registered (source range of the location being type-checked).
    pub span: (u32, u32),
}

/// Why a use of an opaque type in its defining scope does not define it
/// (rustc_trait_selection/src/opaque_types.rs NonDefiningUseReason).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NonDefiningUseReason {
    Tainted,
    NotAParam { opaque_type_key: OpaqueTypeKey, param_index: usize },
    DuplicateParam { opaque_type_key: OpaqueTypeKey, param_indices: Vec<usize> },
}

/// Deferred from `handle_opaque_type_uses` and only reported if there are no region errors:
/// with region errors, these are likely caused by them.
#[derive(Debug, PartialEq, Eq)]
pub enum DeferredOpaqueTypeError {
    InvalidOpaqueTypeArgs(NonDefiningUseReason, (u32, u32)),
    LifetimeMismatchOpaqueParam { arg: GenericArg, prev: GenericArg, span: (u32, u32), prev_span: (u32, u32) },
    UnexpectedHiddenRegion { opaque_type_key: OpaqueTypeKey, hidden_type: TyId, span: (u32, u32), member_region: usize },
    NonDefiningUseInDefiningScope { opaque_type_key: OpaqueTypeKey, span: (u32, u32) },
}

/// This contains fully resolved hidden types (in the opaque's own generic parameters) or the
/// error type, per defining opaque. Insertion-ordered.
pub type HiddenTypes = Vec<(DefId, TyId)>;

/// Record info needed to report the same name error later.
#[derive(Debug, Clone)]
pub struct UnexpectedHiddenRegion {
    /// The body where this error occurs: region variables only make sense in their own inference context.
    pub body: usize,
    pub opaque_type_key: OpaqueTypeKey,
    pub hidden_type: TyId,
    pub span: (u32, u32),
    pub member_region: usize,
}

/// The operations on the body's inference context the opaque handling needs.
pub trait OpaqueOps: BorrowckTypeOps {
    /// Whether the opaque type storage has any entry (`has_opaque_types_in_storage`).
    fn has_opaque_types_in_storage(&mut self) -> bool;
    fn opaque_storage_entries(&mut self) -> OpaqueTypeStorageEntries;
    /// `clone_opaque_types`: the lookup table, then the duplicate entries.
    fn clone_opaque_types(&mut self) -> Vec<(OpaqueTypeKey, ProvisionalHiddenType)>;
    fn opaque_types_added_since(&mut self, entries: OpaqueTypeStorageEntries) -> Vec<(OpaqueTypeKey, ProvisionalHiddenType)>;
    /// `resolve_vars_if_possible`: type, and const, inference variables only.
    fn resolve_vars_if_possible(&mut self, ty: TyId) -> TyId;
    /// `opaque_type_has_defining_use_args(MirBorrowck)`.
    fn opaque_type_has_defining_use_args(&mut self, key: &OpaqueTypeKey) -> Result<(), NonDefiningUseReason>;
    /// `remap_generic_params_to_declaration_params(MirBorrowck)`: the type in terms of the opaque's own parameters.
    fn remap_generic_params_to_declaration_params(&mut self, key: &OpaqueTypeKey, ty: TyId) -> TyId;
    /// Equates the normalized types in the body's environment; its region constraints hold everywhere
    /// (`fully_perform_op_raw`, `Locations::All`, `OpaqueType`).
    fn equate_hidden_types(&mut self, out: &mut MirTypeckRegionConstraints, actual: TyId, expected: TyId, span: (u32, u32)) -> Result<(), ()>;
}

/// Maps an NLL var to a deterministically chosen equal universal region. This ignores changes to
/// the region values due to member constraints: applying member constraints does not impact the
/// result of this function.
fn nll_var_to_universal_region(rcx: &RegionCtxt<'_>, r: usize) -> Option<RegionId> {
    // The SCC representative, not `r` itself ("strict lifetime equality").
    let vid = rcx.representative(r).rvid();
    match rcx.definitions[vid].origin {
        // Iterate over all universal regions in a consistent order and find the *first* equal
        // region: equal lifetimes get the same name. Closure restrictions: external regions are skipped.
        NllRegionVariableOrigin::FreeRegion => rcx.universal_region_relations.universal_regions.universal_regions_iter()
            .filter(|&ur| rcx.universal_region_relations.universal_regions.region_classification(ur) != Some(RegionClassification::External))
            .find(|&ur| rcx.universal_region_relations.equal(vid,ur))
            .and_then(|ur| rcx.definitions[ur].external_name),
        NllRegionVariableOrigin::Placeholder(p) => Some(p),
        // Were `r` equal to any universal region, its SCC representative would be a free region.
        NllRegionVariableOrigin::Existential => None,
    }
}

/// opaque_types/mod.rs:57 `clone_and_resolve_opaque_types`: we eagerly map all regions to NLL
/// variables, as we need to introduce NLL variables for all used placeholders. Inference
/// variables are resolved, as MIR type check may still meet some (e.g. checking user types).
pub fn clone_and_resolve_opaque_types(tcx: &Tcx, infcx: &mut dyn OpaqueOps, relations: &UniversalRegionRelations,
    constraints: &mut MirTypeckRegionConstraints) -> (OpaqueTypeStorageEntries, Vec<(OpaqueTypeKey, ProvisionalHiddenType)>) {
    let opaque_types = infcx.clone_opaque_types();
    let entries = infcx.opaque_storage_entries();
    let tys = &tcx.tys;
    let universals = &relations.universal_regions;
    let regions = std::cell::RefCell::new((infcx, constraints));
    let to_var = |r: RegionId| {
        let mut guard = regions.borrow_mut();
        let (infcx, constraints) = &mut *guard;
        let vid = if matches!(tys.region_kind(r), RegionKind::RePlaceholder { .. }) {
            let region = constraints.placeholder_region(&mut **infcx, tcx, r);
            infcx.to_region_vid(region)
        } else { universals.to_region_vid(tys, r) };
        tys.intern_region(RegionKind::ReVar(vid as u32))
    };
    let fold = |ty: TyId, infcx_resolve: &dyn Fn(TyId) -> TyId| tys.map_regions(infcx_resolve(ty), &|r, depth| {
        if matches!(tys.region_kind(r), RegionKind::ReBound(d, _) if d < depth) { r } else { to_var(r) }
    }, 0);
    let mut result = vec![];
    for (key, hidden) in opaque_types {
        let resolve = |ty: TyId| { let mut guard = regions.borrow_mut(); guard.0.resolve_vars_if_possible(ty) };
        let args: Vec<GenericArg> = key.args.iter().map(|&arg| match arg {
            GenericArg::Lifetime(r) => GenericArg::Lifetime(to_var(r)),
            GenericArg::Type(t) => GenericArg::Type(fold(t, &resolve)),
            GenericArg::Const(c) => GenericArg::Const(tys.expect_const(fold(tys.intern(TyKind::Const(c)), &resolve))),
        }).collect();
        result.push((OpaqueTypeKey { def_id: key.def_id, args: args.into() }, ProvisionalHiddenType { ty: fold(hidden.ty, &resolve), span: hidden.span }));
    }
    (entries, result)
}

/// `add_hidden_type`: collects the defining uses inside of this typeck root. The hidden type is
/// mapped to the definition parameters of the opaque, and distinct hidden types are an error.
pub fn add_hidden_type(tcx: &Tcx, hidden_types: &mut HiddenTypes, def_id: DefId, hidden_ty: TyId) {
    // Sometimes two opaque types are the same only after we remap the generic parameters back to
    // the opaque type definition: `Opaque<X, Y>` mapped to `(X, Y)` and `Opaque<Y, X>` to `(Y, X)`.
    match hidden_types.iter_mut().find(|(d, _)| *d == def_id) {
        Some((_, prev)) if *prev == hidden_ty => {}
        Some((_, prev)) => {
            if !has_error(tcx, *prev) && !has_error(tcx, hidden_ty) {
                tcx.dcx.report(crate::next_solver::fulfill::ObligationCause { span: 0, body_id: def_id },
                    "E0308: concrete type differs from previous defining opaque type use".into());
            }
            *prev = tcx.tys.error;
        }
        None => hidden_types.push((def_id, hidden_ty)),
    }
}

/// opaque_types/mod.rs:190 `compute_definition_site_hidden_types`: the actual hidden types of the
/// opaque types, mapped to their definition sites. Outside of registering the computed hidden
/// types this does not mutate the borrowck state. It may fail to infer a hidden type and return
/// errors, but the computed hidden type is always applied to all opaque type uses to check them,
/// which also supports non-defining uses of opaques in their defining scope.
#[allow(clippy::too_many_arguments)]
pub fn compute_definition_site_hidden_types(tcx: &Tcx, prog: &Program, body: usize, infcx: &mut dyn OpaqueOps,
    definitions: Vec<RegionDefinition>, relations: &UniversalRegionRelations, constraints: &MirTypeckRegionConstraints,
    hidden_types: &mut HiddenTypes, unconstrained_hidden_type_errors: &mut Vec<UnexpectedHiddenRegion>,
    opaque_types: &[(OpaqueTypeKey, ProvisionalHiddenType)]) -> Vec<DeferredOpaqueTypeError> {
    let mut errors = vec![];
    // Member constraints are tracked in a region context of their own: the graph used by
    // `compute_regions` is not mutated, and this information is discarded at the end.
    let mut rcx = RegionCtxt::new(definitions, relations, constraints.outlives_constraints.clone());
    // Each use of an opaque type during type check whose generic arguments are fully universal is a defining use.
    let defining_uses = collect_defining_uses(tcx, prog, infcx, &mut rcx, opaque_types, &mut errors);
    // Compute and apply member constraints for all regions in the hidden types of each defining use.
    member_constraints::apply_member_constraints(&mut rcx, tcx, prog, &defining_uses);
    // All member regions are now equal to one of their choice regions: compute the opaque's hidden type.
    compute_definition_site_hidden_types_from_defining_uses(tcx, prog, infcx, body, &rcx, hidden_types, unconstrained_hidden_type_errors, &defining_uses, &mut errors);
    errors
}

fn collect_defining_uses(tcx: &Tcx, prog: &Program, infcx: &mut dyn OpaqueOps, rcx: &mut RegionCtxt<'_>,
    opaque_types: &[(OpaqueTypeKey, ProvisionalHiddenType)], errors: &mut Vec<DeferredOpaqueTypeError>) -> Vec<DefiningUse> {
    let tys = &tcx.tys;
    let mut defining_uses = vec![];
    for (key, hidden_type) in opaque_types {
        let variances = tcx.variances_of(prog, key.def_id);
        let non_nll_key = fold_captured_lifetime_args(&variances, key, |r| {
            match tys.region_kind(r) {
                RegionKind::ReVar(v) => nll_var_to_universal_region(rcx, v as usize).unwrap_or(r),
                _ => r,
            }
        });
        if let Err(err) = infcx.opaque_type_has_defining_use_args(&non_nll_key) {
            // A non-defining use is a hard error on stable (`use_typing_mode_borrowck()` is false; it is ignored
            // with the next solver's typing mode).
            // (Relax's typeck, unlike rustc's, resolves the type of a call to a function of the defining scope to
            // the hidden type: a use at an inference region is then no use the program wrote down, and is ignored
            // as the next solver does.)
            let inferred_lifetime = matches!(&err, NonDefiningUseReason::NotAParam { opaque_type_key, param_index }
                if matches!(opaque_type_key.args[*param_index], GenericArg::Lifetime(r) if matches!(tys.region_kind(r), RegionKind::ReVar(_))));
            if !inferred_lifetime { errors.push(DeferredOpaqueTypeError::InvalidOpaqueTypeArgs(err, hidden_type.span)); }
            continue;
        }
        // The original key, with its NLL variables, supplies the arg regions.
        let arg_regions = std::iter::once(rcx.universal_region_relations.universal_regions.fr_static).chain(
            iter_captured_args(&variances, key).filter_map(|(_, arg)| match arg {
                GenericArg::Lifetime(r) => match tys.region_kind(r) { RegionKind::ReVar(v) => Some(v as usize), _ => panic!("opaque argument was not renumbered") },
                _ => None,
            })).collect();
        defining_uses.push(DefiningUse { key: non_nll_key, arg_regions, hidden_type: hidden_type.ty, span: hidden_type.span });
    }
    defining_uses
}

#[allow(clippy::too_many_arguments)]
fn compute_definition_site_hidden_types_from_defining_uses(tcx: &Tcx, prog: &Program, infcx: &mut dyn OpaqueOps, body: usize, rcx: &RegionCtxt<'_>,
    hidden_types: &mut HiddenTypes, unconstrained_hidden_type_errors: &mut Vec<UnexpectedHiddenRegion>,
    defining_uses: &[DefiningUse], errors: &mut Vec<DeferredOpaqueTypeError>) {
    // Distinct region arguments of two opaques equal modulo regions: (key modulo regions) -> key.
    let mut decls_modulo_regions: Vec<(OpaqueTypeKey, (OpaqueTypeKey, (u32, u32)))> = vec![];
    for DefiningUse { key, arg_regions, hidden_type, span } in defining_uses {
        // Map all regions in the hidden type to the `arg_regions` of this defining use. A region that
        // ended up not being equal to any is an error: with the next solver the unconstrained region
        // may be resolved by a fully defining use in another body, so the error is deferred.
        let folded = ToArgRegionsFolder { tcx, prog, rcx, arg_regions, erase_unknown_regions: false }.fold_ty(*hidden_type);
        let hidden_type = match folded {
            Ok(ty) => ty,
            Err(member_region) => {
                unconstrained_hidden_type_errors.push(UnexpectedHiddenRegion { body, opaque_type_key: key.clone(), hidden_type: *hidden_type, span: *span, member_region });
                continue;
            }
        };
        // Now that member regions have their final value, map the key's arguments back to the
        // parameters of the opaque type definition.
        let hidden_type = infcx.remap_generic_params_to_declaration_params(key, hidden_type);
        // Two opaques with the same non-region parameters must have the same region parameters: the
        // new solver does various query operations modulo regions and could select unsound impls.
        let modulo = erase_key_regions(tcx, key);
        if let Some(pos) = decls_modulo_regions.iter().position(|(m, _)| *m == modulo) {
            let (prev, prev_span) = std::mem::replace(&mut decls_modulo_regions[pos].1, (key.clone(), *span));
            let variances = tcx.variances_of(prog, key.def_id);
            let captured: Vec<_> = iter_captured_args(&variances, &prev).map(|(_, a)| a)
                .zip(iter_captured_args(&variances, key).map(|(_, a)| a)).collect();
            if let Some((arg1, arg2)) = captured.into_iter().find(|(a, b)| a != b) {
                errors.push(DeferredOpaqueTypeError::LifetimeMismatchOpaqueParam { arg: arg1, prev: arg2, span: prev_span, prev_span: *span });
            }
        } else { decls_modulo_regions.push((modulo, (key.clone(), *span))); }
        add_hidden_type(tcx, hidden_types, key.def_id, hidden_type);
    }
}

fn erase_key_regions(tcx: &Tcx, key: &OpaqueTypeKey) -> OpaqueTypeKey {
    let tys = &tcx.tys;
    let args: Vec<GenericArg> = key.args.iter().map(|&arg| match arg {
        GenericArg::Lifetime(_) => GenericArg::Lifetime(RegionId::ERASED),
        GenericArg::Type(t) => GenericArg::Type(tys.erase_all_regions(t)),
        GenericArg::Const(c) => GenericArg::Const(tys.expect_const(tys.erase_all_regions(tys.intern(TyKind::Const(c))))),
    }).collect();
    OpaqueTypeKey { def_id: key.def_id, args: args.into() }
}

fn has_error(tcx: &Tcx, ty: TyId) -> bool { tcx.tys.flags(ty).intersects(crate::types::TypeFlags::HAS_NON_REGION_ERROR) }

/// opaque_types/mod.rs:`apply_definition_site_hidden_types`: this is what applies member
/// constraints to the borrowck state, and checks all uses of the opaques in their defining
/// scope by equating the hidden type of each use with the instantiated final hidden type.
pub fn apply_definition_site_hidden_types(tcx: &Tcx, prog: &Program, infcx: &mut dyn OpaqueOps, constraints: &mut MirTypeckRegionConstraints,
    hidden_types: &mut HiddenTypes, opaque_types: &[(OpaqueTypeKey, ProvisionalHiddenType)]) -> Vec<DeferredOpaqueTypeError> {
    let tys = &tcx.tys;
    let errors = vec![];
    for (key, hidden_type) in opaque_types {
        let Some(&(_, expected)) = hidden_types.iter().find(|(d, _)| *d == key.def_id) else {
            // No defining use: every use was reported as an invalid use (`collect_defining_uses`).
            continue;
        };
        // We erase all non-member regions of the opaque and treat them as existentials.
        let expected = instantiate_hidden_type(tcx, prog, key, expected);
        let fresh = std::cell::RefCell::new(&mut *infcx);
        let expected = tys.map_regions(expected, &|r, depth| match tys.region_kind(r) {
            RegionKind::ReErased => fresh.borrow_mut().next_nll_region_var(NllRegionVariableOrigin::Existential, crate::types::UniverseIndex::ROOT),
            RegionKind::ReBound(d, _) if d < depth => r,
            _ => r,
        }, 0);
        drop(fresh);
        // We now simply equate the expected with the actual hidden type.
        if infcx.equate_hidden_types(constraints, hidden_type.ty, expected, hidden_type.span).is_err() {
            add_hidden_type(tcx, hidden_types, key.def_id, tys.error);
        }
    }
    errors
}

/// `DefinitionSiteHiddenType::ty.instantiate(tcx, key.args)`: the declaration's parameters take the key's arguments.
fn instantiate_hidden_type(tcx: &Tcx, prog: &Program, key: &OpaqueTypeKey, ty: TyId) -> TyId {
    tcx.instantiate_ty(prog, key.def_id, &key.args, ty)
}

/// We handle `UnexpectedHiddenRegion` errors lazily with the next solver: there may be a fully
/// defining use in another body. If none exists, the error is registered with the body it
/// originates from.
pub fn handle_unconstrained_hidden_type_errors(tcx: &Tcx, hidden_types: &HiddenTypes, unconstrained: &mut Vec<UnexpectedHiddenRegion>,
    deferred_errors: &mut [&mut Vec<DeferredOpaqueTypeError>]) {
    let mut errors = std::mem::take(unconstrained);
    errors.retain(|e| !hidden_types.iter().any(|(d, _)| *d == e.opaque_type_key.def_id));
    for e in errors {
        tcx.dcx.delayed_bug(crate::next_solver::fulfill::ObligationCause { span: 0, body_id: e.opaque_type_key.def_id },
            "opaque type with non-universal region args".into());
        deferred_errors[e.body].push(DeferredOpaqueTypeError::UnexpectedHiddenRegion {
            opaque_type_key: e.opaque_type_key, hidden_type: e.hidden_type, span: e.span, member_region: e.member_region });
    }
}

/// In theory `apply_definition_site_hidden_types` could introduce new uses of opaque types. We do
/// not check these new uses so this could be unsound: any is a delayed bug.
pub fn detect_opaque_types_added_while_handling_opaque_types(tcx: &Tcx, prog: &Program, infcx: &mut dyn OpaqueOps, entries: OpaqueTypeStorageEntries) {
    for (key, _) in infcx.opaque_types_added_since(entries) {
        tcx.dcx.delayed_bug(crate::next_solver::fulfill::ObligationCause { span: 0, body_id: key.def_id },
            format!("unexpected cyclic definition of `{}`", prog.def_path(key.def_id)));
    }
}

/// `OpaqueTypeKey::iter_captured_args`: the arguments of the opaque that its hidden type may
/// mention; an uncaptured lifetime is bivariant.
pub fn iter_captured_args<'a>(variances: &'a [crate::types::variance::V], key: &'a OpaqueTypeKey) -> impl Iterator<Item = (usize, GenericArg)> + 'a {
    key.args.iter().copied().zip(variances.iter()).enumerate().filter_map(|(i, (arg, v))| match (arg, v) {
        (_, crate::types::variance::V::Inv) => Some((i, arg)),
        (GenericArg::Lifetime(_), crate::types::variance::V::Bi) => None,
        _ => panic!("unexpected opaque type arg variance"),
    })
}

/// `OpaqueTypeKey::fold_captured_lifetime_args`.
pub fn fold_captured_lifetime_args(variances: &[crate::types::variance::V], key: &OpaqueTypeKey, mut f: impl FnMut(RegionId) -> RegionId) -> OpaqueTypeKey {
    let args: Vec<GenericArg> = key.args.iter().copied().zip(variances.iter()).map(|(arg, v)| match (arg, v) {
        (GenericArg::Lifetime(_), crate::types::variance::V::Bi) => arg,
        (GenericArg::Lifetime(r), _) => GenericArg::Lifetime(f(r)),
        _ => arg,
    }).collect();
    OpaqueTypeKey { def_id: key.def_id, args: args.into() }
}

type AliasVariances = Option<(crate::types::variance::GenericIndices, Vec<crate::types::variance::V>)>;
fn bivariant(variance: &AliasVariances, region: bool, index: usize) -> bool {
    variance.as_ref().is_some_and(|(indices, vs)| {
        let indices = if region { &indices.lifetimes } else { &indices.types_consts };
        indices.get(index).is_some_and(|&i| vs.get(i as usize) == Some(&crate::types::variance::V::Bi))
    })
}
struct ToArgRegionsFolder<'a,'b> {
    tcx: &'a Tcx, prog: &'a Program, rcx: &'a RegionCtxt<'b>, arg_regions: &'a [usize], erase_unknown_regions: bool,
}
impl ToArgRegionsFolder<'_, '_> {
    fn fold_region(&self, r: RegionId) -> Result<RegionId, usize> {
        let vid = match self.tcx.tys.region_kind(r) {
            RegionKind::ReBound(..) => return Ok(r),
            RegionKind::ReVar(v) => v as usize,
            _ => panic!("opaque hidden type was not renumbered"),
        };
        if let Some(region) = self.arg_regions.iter().copied().find(|&a| self.rcx.eval_equal(vid,a))
            .and_then(|a| nll_var_to_universal_region(self.rcx,a)) { Ok(region) }
        else if self.erase_unknown_regions { Ok(RegionId::ERASED) }
        else { Err(vid) }
    }
    fn list(&mut self, ts: &[TyId]) -> Result<Vec<TyId>, usize> {
        ts.iter().map(|&t| self.fold_ty(t)).collect()
    }
    fn args(&mut self, def: DefId, ts: &[TyId], rs: crate::types::RegionArgsId, variance: AliasVariances)
        -> Result<(Vec<TyId>, crate::types::RegionArgsId), usize> {
        let tys = &self.tcx.tys;
        let original_regions = tys.region_args(rs);
        let indices = variance.as_ref().map(|p| p.0.clone())
            .or_else(|| self.tcx.adts.get(&def).map(|d| d.generic_indices.clone()))
            .or_else(|| self.tcx.fn_item_sig(self.prog, def).map(|d| d.generic_indices.clone()))
            .or_else(|| self.tcx.nominal_region_contracts.get(&def).map(|c| {
                let lifetimes: Vec<_> = c.regions.iter().filter_map(|p| match tys.region_kind(p.1) {
                    RegionKind::ReEarlyParam(i) => Some(i), _ => None,
                }).collect();
                let mut full = 0;
                let types_consts = (0..ts.len()).map(|_| {
                    while lifetimes.contains(&full) { full += 1; }
                    let i = full; full += 1; i
                }).collect();
                crate::types::variance::GenericIndices { lifetimes,types_consts,consts:vec![] }
            }));
        let mut order = vec![];
        for i in 0..ts.len() {
            let full = indices.as_ref().and_then(|g| g.types_consts.get(i)).copied().unwrap_or(i as u32);
            order.push((full,false,i));
        }
        for i in 0..original_regions.len() {
            let full = indices.as_ref().and_then(|g| g.lifetimes.get(i)).copied().unwrap_or((ts.len()+i) as u32);
            order.push((full,true,i));
        }
        order.sort_by_key(|p| p.0);
        let mut types = ts.to_vec(); let mut regions = original_regions.to_vec();
        for (_,region,i) in order {
            let previous = self.erase_unknown_regions;
            self.erase_unknown_regions |= bivariant(&variance,region,i);
            let result = if region {
                self.fold_region(regions[i]).map(|r| { regions[i] = r; })
            } else {
                self.fold_ty(types[i]).map(|t| { types[i] = t; })
            };
            self.erase_unknown_regions = previous;
            result?;
        }
        Ok((types,self.tcx.tys.intern_region_args(&regions)))
    }
    // Fallible traversal, including nightly's region-before-referent ordering.
    // Every recursive operation uses ?, so later arguments are never visited
    // after an unknown member region has failed substitution.
    fn fold_ty(&mut self, ty: TyId) -> Result<TyId, usize> {
        let tys = &self.tcx.tys;
        if !tys.flags(ty).intersects(crate::types::TypeFlags::HAS_FREE_REGIONS) { return Ok(ty); }
        let kind = match tys.kind(ty) {
            TyKind::Ref(m,t,r) => { let r = self.fold_region(*r)?; TyKind::Ref(*m,self.fold_ty(*t)?,r) }
            TyKind::Ptr(m,t) => TyKind::Ptr(*m,self.fold_ty(*t)?),
            TyKind::Slice(t) => TyKind::Slice(self.fold_ty(*t)?),
            TyKind::Array(t,n) => {
                let t = self.fold_ty(*t)?;
                let n = self.fold_ty(tys.intern(TyKind::Const(*n)))?;
                return Ok(tys.mk_array(t,tys.expect_const(n)));
            }
            TyKind::Const(ct) => {
                use crate::consts::{ConstKind, GenericArg, Value, ValTree};
                let kind = match tys.const_kind(*ct) {
                    ConstKind::Alias(rigid, mut alias) => {
                        let mut args = Vec::new();
                        for arg in &*alias.args {
                            args.push(match *arg {
                                GenericArg::Lifetime(r) => GenericArg::Lifetime(self.fold_region(r)?),
                                GenericArg::Type(t) => GenericArg::Type(self.fold_ty(t)?),
                                GenericArg::Const(c) => GenericArg::Const(tys.expect_const(self.fold_ty(tys.intern(TyKind::Const(c)))?)),
                            });
                        }
                        alias.args = args.into(); ConstKind::Alias(rigid,alias)
                    }
                    ConstKind::Value(v) => {
                        let ty = self.fold_ty(v.ty)?;
                        let valtree = if let Some(branches) = v.valtree.try_to_branch() {
                            let mut folded = Vec::new();
                            for &c in branches { folded.push(tys.expect_const(self.fold_ty(tys.intern(TyKind::Const(c)))?)); }
                            ValTree::from_branches(tys,&folded)
                        } else { v.valtree };
                        ConstKind::Value(Value { ty, valtree })
                    }
                    _ => return Ok(ty),
                };
                TyKind::Const(tys.intern_const(kind))
            }
            TyKind::Tuple(ts) => TyKind::Tuple(tys.mk_list(&self.list(tys.list(*ts))?)),
            TyKind::Adt(d,ts,rs) | TyKind::FnDef(d,ts,rs) => {
                let (ts,rs) = self.args(*d,tys.list(*ts),*rs,None)?;
                match tys.kind(ty) {
                    TyKind::Adt(..) => TyKind::Adt(*d,tys.mk_list(&ts),rs),
                    TyKind::FnDef(..) => TyKind::FnDef(*d,tys.mk_list(&ts),rs),
                    _ => unreachable!(),
                }
            }
            TyKind::Alias(rigid, alias) => {
                let d = alias.kind.def_id();
                // mod.rs:502-514: opt_alias_variances (opaques and RPITITs).
                let variance = self.tcx.alias_declared_variances(self.prog, alias.kind);
                let (args,regions) = self.args(d,tys.list(alias.args),alias.regions,variance)?;
                TyKind::Alias(*rigid, AliasTy { kind: alias.kind, args: tys.mk_list(&args), regions })
            }
            TyKind::FnPtr(ts,r,h,n) => TyKind::FnPtr(tys.mk_list(&self.list(tys.list(*ts))?),self.fold_ty(*r)?,*h,*n),
            TyKind::Dyn(d,ts,bs,rs,r,n,autos) => {
                let (ts,rs) = self.args(*d,tys.list(*ts),*rs,None)?;
                let bs: Vec<_> = tys.list(*bs).iter().map(|&(d,t,n)| Ok((d,self.fold_ty(t)?,n))).collect::<Result<_,usize>>()?;
                TyKind::Dyn(*d,tys.mk_list(&ts),tys.mk_list(&bs),rs,self.fold_region(*r)?,*n,*autos)
            }
            TyKind::Closure(f, e, crate::types::ClosureArgs { parent_args: o, closure_kind_ty, closure_sig_as_fn_ptr_ty: s, tupled_upvars_ty: u }) => {
                let previous = self.erase_unknown_regions;
                self.erase_unknown_regions = true;
                let owner = self.fold_ty(*o);
                self.erase_unknown_regions = previous;
                let owner = owner?;
                TyKind::Closure(*f, *e, crate::types::ClosureArgs { parent_args: owner, closure_kind_ty: self.fold_ty(*closure_kind_ty)?, closure_sig_as_fn_ptr_ty: self.fold_ty(*s)?, tupled_upvars_ty: self.fold_ty(*u)? })
            }
            _ => return Ok(ty),
        };
        Ok(tys.intern(kind))
    }
}

// ---------------------------------------------------------------------------------------------
// The operations on N01's inference context. Opaque storage and equality are the context's own;
// the defining-use validation and the remapping to declaration parameters are
// rustc_trait_selection/src/opaque_types.rs and rustc_middle/src/ty/{opaque_types,mod}.rs.
// ---------------------------------------------------------------------------------------------
use crate::borrowck::infer_ctxt::BorrowckInferCtxt;

impl OpaqueOps for BorrowckInferCtxt<'_> {
    fn has_opaque_types_in_storage(&mut self) -> bool { !self.infcx.inner.borrow_mut().opaque_types().is_empty() }
    fn opaque_storage_entries(&mut self) -> OpaqueTypeStorageEntries { self.infcx.inner.borrow_mut().opaque_types().num_entries() }
    fn clone_opaque_types(&mut self) -> Vec<(OpaqueTypeKey, ProvisionalHiddenType)> {
        let mut inner = self.infcx.inner.borrow_mut();
        let storage = inner.opaque_types();
        let mut values = storage.iter_lookup_table().to_vec();
        values.extend_from_slice(storage.iter_duplicate_entries());
        values
    }
    fn opaque_types_added_since(&mut self, entries: OpaqueTypeStorageEntries) -> Vec<(OpaqueTypeKey, ProvisionalHiddenType)> {
        self.infcx.inner.borrow_mut().opaque_types().opaque_types_added_since(entries)
    }
    fn resolve_vars_if_possible(&mut self, ty: TyId) -> TyId { self.infcx.deeply_resolve_ignoring_regions(ty) }

    /// rustc_trait_selection/src/opaque_types.rs `opaque_type_has_defining_use_args`: the opaque
    /// type parameter validity check. With the new solver, uses which fail it are simply
    /// non-defining, and an error is only emitted if no defining use exists. (The
    /// "already malformed in HIR typeck" taint check is typeck's own result and not repeated.)
    fn opaque_type_has_defining_use_args(&mut self, key: &OpaqueTypeKey) -> Result<(), NonDefiningUseReason> {
        let (tcx, prog) = (self.tcx, self.prog);
        let tys = &tcx.tys;
        let variances = tcx.variances_of(prog, key.def_id);
        // Computed lazily: most uses never need the environment.
        struct Lazy<'a> { tcx: &'a Tcx, prog: &'a Program, def: DefId, args: Option<Vec<GenericArg>> }
        impl Lazy<'_> { fn get(&mut self) -> &Vec<GenericArg> { self.args.get_or_insert_with(|| canonical_opaque_args(self.tcx, self.prog, self.def)) } }
        let mut canonical = Lazy { tcx, prog, def: key.def_id, args: None };
        let mut seen_params: Vec<(GenericArg, Vec<usize>)> = vec![];
        for (i, arg) in iter_captured_args(&variances, key) {
            let arg_is_param = match arg {
                GenericArg::Lifetime(lt) => matches!(tys.region_kind(lt), RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..))
                    || (tys.region_kind(lt) == RegionKind::ReStatic && matches!(canonical.get()[i], GenericArg::Lifetime(r) if tys.region_kind(r) == RegionKind::ReStatic)),
                GenericArg::Type(t) => matches!(tys.kind(t), TyKind::Param(_)),
                GenericArg::Const(c) => matches!(tys.const_kind(c), crate::consts::ConstKind::Param(_)),
            };
            if arg_is_param {
                // The same lifetime may appear multiple times in the generic args, unless the opaque
                // type requires the lifetimes to be equal ("an exception to the uniqueness rule").
                let at = match seen_params.iter().position(|(a, _)| *a == arg) { Some(at) => at, None => { seen_params.push((arg, vec![])); seen_params.len() - 1 } };
                let first = seen_params[at].1.first().copied();
                if !first.is_some_and(|prev| { let c = canonical.get(); c[i] == c[prev] }) { seen_params[at].1.push(i); }
            } else {
                // Prevent `fn foo() -> Foo<u32>` from being defining.
                if matches!(canonical.get()[i], GenericArg::Lifetime(r) if tys.region_kind(r) == RegionKind::ReError) { return Err(NonDefiningUseReason::Tainted); }
                return Err(NonDefiningUseReason::NotAParam { opaque_type_key: key.clone(), param_index: i });
            }
        }
        for (_, param_indices) in seen_params {
            if param_indices.len() > 1 { return Err(NonDefiningUseReason::DuplicateParam { opaque_type_key: key.clone(), param_indices }); }
        }
        Ok(())
    }

    /// rustc_middle/src/ty/mod.rs `remap_generic_params_to_declaration_params` (MirBorrowck): the
    /// type outside the function, in the context of the opaque.
    fn remap_generic_params_to_declaration_params(&mut self, key: &OpaqueTypeKey, ty: TyId) -> TyId {
        let (tcx, prog) = (self.tcx, self.prog);
        // Use the key to build a reverse map from regions to their identity mappings: `impl Trait`
        // lifetimes are computed by replacing existing lifetimes with 'static and remapping only those
        // used in the return type, so the parameters shift. The same lifetime may be paired with several
        // identity lifetimes; the last one is the one introduced by the desugaring.
        let identity = tcx.generic_indices_of(prog, key.def_id).identity_args(&tcx.tys);
        let mut map: Vec<(GenericArg, GenericArg)> = vec![];
        for (from, to) in key.args.iter().copied().zip(identity) {
            match map.iter_mut().find(|(f, _)| *f == from) { Some(entry) => entry.1 = to, None => map.push((from, to)) }
        }
        let mut mapper = ReverseMapper { tcx, map, do_not_error: false, def: key.def_id };
        crate::types::fold::TypeFolder::fold_ty(&mut mapper, &tcx.tys, ty, 0)
    }

    fn equate_hidden_types(&mut self, out: &mut MirTypeckRegionConstraints, actual: TyId, expected: TyId, at: (u32, u32)) -> Result<(), ()> {
        use crate::next_solver::type_op::CustomTypeOp;
        let env = self.param_env_id();
        let span = crate::mir::Span { file: self.prog.def(self.root_def_id).file, lo: at.0, hi: at.1 };
        let universals = self.universal_regions.as_ref().expect("opaque handling follows the type check");
        // The types are normalized before they are equated.
        let op = CustomTypeOp::new(|ocx: &mut crate::next_solver::fulfill::ObligationCtxt<'_, '_>| {
            let actual = ocx.normalize_ty(&env, actual);
            let expected = ocx.normalize_ty(&env, expected);
            ocx.eq(&env, actual, expected)
        });
        crate::borrowck::type_check::canonical::fully_perform_op_raw(self, universals, &self.region_bound_pairs, &self.param_env.clauses, out,
            crate::borrowck::type_check::Locations::All(span), span, crate::borrowck::constraints::ConstraintCategory::OpaqueType, op)
            .map_err(|_| self.tainted_by_errors.set(true))
    }
}

/// rustc_trait_selection/src/opaque_types.rs `LazyOpaqueTyEnv::get_canonical_args`: the opaque's
/// identity arguments where lifetimes that its explicit and implied bounds make equal (to each
/// other or to 'static) have the same name. Example: `type Opaque<'a: 'static, 'b: 'c, 'c: 'b> = impl Sized;`
/// has identity args `['a, 'b, 'c]` and canonical args `['static, 'b, 'b]`.
fn canonical_opaque_args(tcx: &Tcx, prog: &Program, opaque: DefId) -> Vec<GenericArg> {
    let tys = &tcx.tys;
    let parent = prog.opaque_owner(opaque);
    // The parent's identity arguments, extended with the parent lifetime each lifetime captured by
    // the opaque stands for.
    let mut args = tcx.generic_indices_of(prog, opaque).identity_args(tys);
    let generics = tcx.opaque_generics(prog, opaque);
    let captured = tcx.opaque_captured_parent_lifetimes(prog, opaque);
    let n_parent_lifetimes = generics.parent.lifetimes.len();
    let indices = tcx.generic_indices_of(prog, opaque);
    for (k, &index) in indices.lifetimes.iter().enumerate().skip(n_parent_lifetimes) {
        let position = indices.lifetimes.iter().position(|&i| i == index).unwrap();
        args[indices.lifetimes[position] as usize] = GenericArg::Lifetime(captured[k - n_parent_lifetimes]);
    }
    // The free region relations of the parent's environment: declared bounds and the bounds its
    // signature implies.
    let mut edges = vec![];
    if let Some(sig) = crate::borrowck::universal_regions::SignatureRegions::function(tcx, prog, parent) {
        let raw: Vec<TyId> = sig.inputs.iter().copied().chain([sig.output]).collect();
        let implied = crate::borrowck::implied_bounds::ImpliedBounds::create(tcx, prog, &sig.contract.predicates, &raw, &[], vec![]);
        // `(sup, sub)`: `sub <= sup`.
        edges.extend(implied.outlives.iter().map(|&(sup, sub)| (sub, sup)));
    }
    let free_regions = crate::infer::free_regions::FreeRegionMap::new(&edges);
    let mut seen = vec![tys.intern_region(RegionKind::ReStatic)];
    args.into_iter().map(|arg| match arg {
        GenericArg::Lifetime(r1) if tys.region_kind(r1) != RegionKind::ReError => {
            match seen.iter().copied().find(|&r2| free_regions.sub_free_regions(tys, r1, r2) && free_regions.sub_free_regions(tys, r2, r1)) {
                Some(r2) => GenericArg::Lifetime(r2),
                None => { seen.push(r1); arg }
            }
        }
        _ => arg,
    }).collect()
}

/// rustc_middle/src/ty/opaque_types.rs `ReverseMapper`: converts the generic parameters of a type
/// from the defining item's generics to the opaque type's own.
struct ReverseMapper<'a> { tcx: &'a Tcx, map: Vec<(GenericArg, GenericArg)>, do_not_error: bool, def: DefId }
impl ReverseMapper<'_> {
    fn error(&self, message: String) {
        self.tcx.dcx.report(crate::next_solver::fulfill::ObligationCause { span: 0, body_id: self.def }, message);
    }
    /// A closure's types include all the lifetime parameters of the enclosing function, even unused ones;
    /// those in the parent arguments are not an error when missing from the reverse map.
    fn fold_closure_args(&mut self, tys: &crate::types::Types, args: &crate::types::ClosureArgs) -> crate::types::ClosureArgs {
        use crate::types::fold::TypeFolder;
        assert!(!self.do_not_error);
        self.do_not_error = true;
        let parent_args = self.fold_ty(tys, args.parent_args, 0);
        self.do_not_error = false;
        crate::types::ClosureArgs { parent_args, closure_kind_ty: self.fold_ty(tys, args.closure_kind_ty, 0),
            closure_sig_as_fn_ptr_ty: self.fold_ty(tys, args.closure_sig_as_fn_ptr_ty, 0), tupled_upvars_ty: self.fold_ty(tys, args.tupled_upvars_ty, 0) }
    }
}
impl crate::types::fold::TypeFolder for ReverseMapper<'_> {
    fn fold_region(&mut self, tys: &crate::types::Types, r: RegionId, depth: u32) -> RegionId {
        match tys.region_kind(r) {
            // Bound regions and 'static need no remapping: `'r` in `for<'r> fn(&'r u32)` is ignored. Erased
            // regions are not unerased, and error regions stay.
            RegionKind::ReBound(..) | RegionKind::ReStatic | RegionKind::ReErased | RegionKind::ReError => return r,
            // The regions expected from borrow checking.
            RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..) => {}
            // All of the regions should be erased by writeback or mapped back to named regions by borrowck.
            kind => panic!("unexpected region kind in opaque type: {kind:?}"),
        }
        let _ = depth;
        match self.map.iter().find(|(from, _)| *from == GenericArg::Lifetime(r)).map(|x| x.1) {
            Some(GenericArg::Lifetime(r1)) => r1,
            Some(other) => panic!("region mapped to unexpected kind: {other:?}"),
            None if self.do_not_error => tys.intern_region(RegionKind::ReStatic),
            None => {
                self.error(format!("non-defining opaque type use in defining scope: lifetime `{:?}` is part of concrete type but not used in parameter list of the `impl Trait` type alias", tys.region_kind(r)));
                tys.intern_region(RegionKind::ReError)
            }
        }
    }
    fn fold_ty(&mut self, tys: &crate::types::Types, ty: TyId, depth: u32) -> TyId {
        match tys.kind(ty).clone() {
            TyKind::Closure(file, expr, args) => { let args = self.fold_closure_args(tys, &args); tys.intern(TyKind::Closure(file, expr, args)) }
            TyKind::Param(_) => match self.map.iter().find(|(from, _)| *from == GenericArg::Type(ty)).map(|x| x.1) {
                Some(GenericArg::Type(t1)) => t1,
                Some(other) => panic!("type mapped to unexpected kind: {other:?}"),
                None => {
                    self.error("type parameter is part of concrete type but not used in parameter list for the `impl Trait` type alias".into());
                    tys.error
                }
            },
            _ => crate::types::fold::super_fold_ty(tys, ty, self, depth),
        }
    }
    fn fold_const(&mut self, tys: &crate::types::Types, ct: crate::consts::Const, _: u32) -> crate::consts::Const {
        if matches!(tys.const_kind(ct), crate::consts::ConstKind::Param(..)) {
            return match self.map.iter().find(|(from, _)| *from == GenericArg::Const(ct)).map(|x| x.1) {
                Some(GenericArg::Const(c1)) => c1,
                Some(other) => panic!("const mapped to unexpected kind: {other:?}"),
                None => {
                    self.error("const parameter is part of concrete type but not used in parameter list for the `impl Trait` type alias".into());
                    tys.intern_const(crate::consts::ConstKind::Error)
                }
            };
        }
        ct
    }
}

/// A key without arguments, for fixtures of prevalidated defining uses.
#[cfg(test)]
pub(super) fn test_key(def_id: DefId) -> OpaqueTypeKey { OpaqueTypeKey { def_id, args: Vec::new().into() } }
/// The hidden types of prevalidated defining uses (collection and validation are the producer's).
#[cfg(test)]
pub(super) fn hidden_types_from_uses(tcx: &Tcx, prog: &Program, infcx: &mut dyn OpaqueOps, definitions: Vec<RegionDefinition>,
    relations: &UniversalRegionRelations, constraints: crate::borrowck::constraints::OutlivesConstraintSet, uses: &[DefiningUse])
    -> (HiddenTypes, Vec<DeferredOpaqueTypeError>) {
    let mut rcx = RegionCtxt::new(definitions, relations, constraints);
    member_constraints::apply_member_constraints(&mut rcx, tcx, prog, uses);
    let (mut hidden, mut unconstrained, mut errors) = (vec![], vec![], vec![]);
    compute_definition_site_hidden_types_from_defining_uses(tcx, prog, infcx, 0, &rcx, &mut hidden, &mut unconstrained, uses, &mut errors);
    for e in unconstrained {
        errors.push(DeferredOpaqueTypeError::UnexpectedHiddenRegion { opaque_type_key: e.opaque_type_key, hidden_type: e.hidden_type, span: e.span, member_region: e.member_region });
    }
    (hidden, errors)
}

#[cfg(test)]
pub(super) fn test_fail_fast(tcx: &Tcx, ty: TyId) {
    use crate::types::UniverseIndex;
    use crate::borrowck::constraints::OutlivesConstraintSet;
    use crate::borrowck::universal_regions::UniversalRegions;
    let definitions = (0..3).map(|_| RegionDefinition { origin:NllRegionVariableOrigin::FreeRegion,
        universe:UniverseIndex::ROOT,external_name:None }).collect();
    let relations = UniversalRegionRelations::create(UniversalRegions::new(0,None,vec![
        (0,RegionClassification::Global),(1,RegionClassification::Local),(2,RegionClassification::Local)]),3,&[]);
    let rcx = RegionCtxt::new(definitions,&relations,OutlivesConstraintSet::default());
    let mut folder = ToArgRegionsFolder { tcx, prog:&crate::program::Program::new(&std::sync::Arc::new(crate::Session::default())),rcx:&rcx,arg_regions:&[0,1],erase_unknown_regions:false };
    assert_eq!(folder.fold_ty(ty),Err(2));
}
