// ported from N:rustc_next_trait_solver/src/canonical/canonicalizer.rs @8d1a764
// ported from N:rustc_type_ir/src/canonical.rs @8d1a764
// ported from N:rustc_type_ir/src/infer_ctxt.rs @8d1a764
// ported from N:rustc_infer/src/infer/canonical/mod.rs @8d1a764
use super::*;
use super::fold::fold_predicate;
use crate::types::fold::super_fold_const;
use crate::consts::{BoundConst, ConstKind, InferConst, PlaceholderConst};
use crate::infer::InferCtxt;
use crate::infer::type_variable::TypeVariableValue;
use crate::infer::unify_key::ConstVariableValue;
use crate::infer::opaque_types::OpaqueTypeKey;
use crate::tcx::{TypingMode, TypingModeEqWrapper};
use crate::types::*;
use crate::types::fold::{TypeFolder, super_fold_ty};
use std::collections::HashMap;
pub mod response;
pub mod method_response;
pub mod user_type;

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CanonicalVarKind {
    Ty { ui: UniverseIndex, sub_root: BoundVar },
    Int,
    Float,
    Region(UniverseIndex),
    Const(UniverseIndex),
    PlaceholderTy(PlaceholderType),
    PlaceholderRegion { universe: UniverseIndex, var: BoundVar },
    PlaceholderConst(PlaceholderConst),
}
impl CanonicalVarKind {
    fn universe(self) -> UniverseIndex {
        match self {
            Self::Int | Self::Float => UniverseIndex::ROOT,
            Self::Ty { ui, .. } | Self::Region(ui) | Self::Const(ui) => ui,
            Self::PlaceholderTy(p) => p.universe,
            Self::PlaceholderRegion { universe, .. } => universe,
            Self::PlaceholderConst(p) => p.universe,
        }
    }
    fn with_updated_universe(self, ui: UniverseIndex) -> Self {
        match self {
            Self::Int | Self::Float => { assert_eq!(ui, UniverseIndex::ROOT); self }
            Self::Ty { sub_root, .. } => Self::Ty { ui, sub_root },
            Self::Region(_) => Self::Region(ui),
            Self::Const(_) => Self::Const(ui),
            Self::PlaceholderTy(p) => Self::PlaceholderTy(PlaceholderType { universe: ui, ..p }),
            Self::PlaceholderRegion { var, .. } => Self::PlaceholderRegion { universe: ui, var },
            Self::PlaceholderConst(p) => Self::PlaceholderConst(PlaceholderConst { universe: ui, ..p }),
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Canonical<T> { pub value: T, pub max_universe: UniverseIndex, pub var_kinds: Vec<CanonicalVarKind> }
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct QueryInput { pub goal: Goal, pub predefined_opaques_in_body: Vec<(OpaqueTypeKey, TyId)> }
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
// Program contains several crates; coherence_crate identifies the local crate
// that rustc_next_trait_solver/src/coherence.rs obtains from its single-crate Tcx.
pub struct CanonicalQueryInput { pub canonical: Canonical<QueryInput>, pub typing_mode: TypingModeEqWrapper, pub coherence_crate: Option<u32> }
#[derive(Clone, Copy)]
pub enum CanonicalizeMode {
    Input { in_param_env: bool },
    Response { max_input_universe: UniverseIndex },
    /// rustc_infer canonicalizer.rs:217-248 (CanonicalizeUserTypeAnnotation): written lifetimes
    /// stay rigid, every region variable becomes a root-universe variable; universes are not kept.
    UserType,
}

#[derive(Default)]
pub struct CanonicalizerState {
    variables: Vec<GenericArg>,
    var_kinds: Vec<CanonicalVarKind>,
    variable_lookup_table: HashMap<GenericArg, usize>,
    sub_root_lookup_table: HashMap<u32, usize>,
    cache: HashMap<(TyId, DebruijnIndex), TyId>,
}
impl CanonicalizerState {
    fn clear(&mut self) {
        self.variables.clear(); self.var_kinds.clear(); self.variable_lookup_table.clear();
        self.sub_root_lookup_table.clear(); self.cache.clear();
    }
}
#[derive(Clone)]
pub(crate) struct CanonicalParamEnvCacheEntry {
    param_env: ParamEnvId,
    variables: Vec<GenericArg>,
    var_kinds: Vec<CanonicalVarKind>,
    variable_lookup_table: HashMap<GenericArg, usize>,
}

struct DeepVariableResolver<'a, 'tcx> { infcx: &'a InferCtxt<'tcx> }
impl TypeFolder for DeepVariableResolver<'_, '_> {
    fn fold_flags(&self) -> Option<TypeFlags> { Some(TypeFlags::HAS_ANY_INFER) }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: u32) -> Const {
        if tys.const_flags(ct).0 & TypeFlags::HAS_ANY_INFER.bits() == 0 { return ct; }
        super_fold_const(tys, self.infcx.shallow_resolve_const(ct), self, depth)
    }

    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: u32) -> TyId {
        if !tys.flags(ty).intersects(TypeFlags::HAS_ANY_INFER) { return ty; }
        super_fold_ty(tys, self.infcx.shallow_resolve(ty), self, depth)
    }
    fn fold_region(&mut self, tys: &Types, region: RegionId, _depth: u32) -> RegionId {
        if let RegionKind::ReVar(vid) = tys.region_kind(region) {
            self.infcx.inner.borrow_mut().unwrap_region_constraints().shallow_resolve_region_var(tys, vid)
        } else { region }
    }
}

struct Canonicalizer<'a, 'tcx> { delegate: &'a InferCtxt<'tcx>, mode: CanonicalizeMode, state: CanonicalizerState, infer_query: bool }
impl<'a, 'tcx> Canonicalizer<'a, 'tcx> {
    fn new(delegate: &'a InferCtxt<'tcx>, mode: CanonicalizeMode) -> Self {
        let state = std::mem::take(&mut *delegate.canonicalizer_state.borrow_mut());
        Self { delegate, mode, state, infer_query: false }
    }
    fn get_or_insert_bound_var(&mut self, arg: GenericArg, kind: CanonicalVarKind) -> BoundVar {
        let index = if self.state.variables.len() > 16 {
            if self.state.variable_lookup_table.is_empty() {
                for (index, &arg) in self.state.variables.iter().enumerate() { self.state.variable_lookup_table.insert(arg, index); }
            }
            if let Some(&index) = self.state.variable_lookup_table.get(&arg) { index }
            else {
                let index = self.state.variables.len();
                self.state.variables.push(arg); self.state.var_kinds.push(kind);
                self.state.variable_lookup_table.insert(arg, index);
                index
            }
        } else if let Some(index) = self.state.variables.iter().position(|&v| v == arg) { index }
        else {
            let index = self.state.variables.len();
            self.state.variables.push(arg); self.state.var_kinds.push(kind); index
        };
        BoundVar(index as u32)
    }
    fn universe(&self, ui: UniverseIndex) -> UniverseIndex {
        match self.mode { CanonicalizeMode::Input { .. } | CanonicalizeMode::UserType => UniverseIndex::ROOT, CanonicalizeMode::Response { .. } => ui }
    }
    fn placeholder_index(&self, original: BoundVar) -> BoundVar {
        match self.mode { CanonicalizeMode::Input { .. } => BoundVar(self.state.variables.len() as u32), CanonicalizeMode::Response { .. } | CanonicalizeMode::UserType => original }
    }
    fn finalize(mut self) -> (UniverseIndex, Vec<GenericArg>, Vec<CanonicalVarKind>) {
        let mut max = UniverseIndex::ROOT;
        if let CanonicalizeMode::Response { max_input_universe } = self.mode {
            for kind in &mut self.state.var_kinds {
                let ui = UniverseIndex(kind.universe().0.saturating_sub(max_input_universe.0));
                *kind = kind.with_updated_universe(ui);
                max = max.max(ui);
            }
        }
        let values = self.state.variables.clone();
        let kinds = self.state.var_kinds.clone();
        self.state.clear();
        *self.delegate.canonicalizer_state.borrow_mut() = self.state;
        (max, values, kinds)
    }
}
impl TypeFolder for Canonicalizer<'_, '_> {
    fn fold_flags(&self) -> Option<TypeFlags> { Some(TypeFlags::NEEDS_CANONICAL) }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: DebruijnIndex) -> Const {
        if tys.const_flags(ct).0 & TypeFlags::NEEDS_CANONICAL.bits() == 0 { return ct; }
        let ct = self.delegate.shallow_resolve_const(ct);
        let kind = match tys.const_kind(ct) {
            ConstKind::Infer(InferConst::Var(vid)) => {
                let mut inner = self.delegate.inner.borrow_mut();
                let mut table = inner.const_unification_table();
                let root = table.find(vid.0);
                let ConstVariableValue::Unknown { universe } = table.probe_value(root) else { unreachable!() };
                drop(inner);
                let ct = tys.intern_const(ConstKind::Infer(InferConst::Var(crate::consts::ConstVid(root))));
                let var = self.get_or_insert_bound_var(GenericArg::Const(ct), CanonicalVarKind::Const(self.universe(universe)));
                return tys.intern_const(ConstKind::Bound(depth, BoundConst { var }));
            }
            ConstKind::Placeholder(p) => CanonicalVarKind::PlaceholderConst(PlaceholderConst { universe: self.universe(p.universe), bound: BoundConst { var: self.placeholder_index(p.bound.var) } }),
            ConstKind::Param(_) if self.infer_query => return ct,
            ConstKind::Param(_) => {
                assert!(matches!(self.mode, CanonicalizeMode::Input { .. }), "param const in response");
                CanonicalVarKind::PlaceholderConst(PlaceholderConst { universe: UniverseIndex::ROOT, bound: BoundConst { var: BoundVar(self.state.variables.len() as u32) } })
            }
            _ => return super_fold_const(tys, ct, self, depth),
        };
        let var = self.get_or_insert_bound_var(GenericArg::Const(ct), kind);
        tys.intern_const(ConstKind::Bound(depth, BoundConst { var }))
    }

    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: DebruijnIndex) -> TyId {
        if !tys.flags(ty).intersects(TypeFlags::NEEDS_CANONICAL) {
            crate::solver_work::count(crate::solver_work::CANON_TY_SKIPPED);
            return ty;
        }
        let ty = self.delegate.shallow_resolve(ty);
        if let Some(&value) = self.state.cache.get(&(ty, depth)) { return value; }
        let kind = match *tys.kind(ty) {
            TyKind::Infer(vid) => {
                let mut inner = self.delegate.inner.borrow_mut();
                let mut table = inner.type_variables();
                let root = table.root_var(vid);
                let sub_root = table.sub_unification_table_root_var(root);
                let index = *self.state.sub_root_lookup_table.entry(sub_root).or_insert(self.state.variables.len());
                let TypeVariableValue::Unknown { universe } = table.probe(root) else { unreachable!() };
                let kind = CanonicalVarKind::Ty { ui: self.universe(universe), sub_root: BoundVar(index as u32) };
                drop(inner);
                let var = self.get_or_insert_bound_var(GenericArg::Type(tys.intern(TyKind::Infer(root))), kind);
                return tys.intern(TyKind::Bound(depth, BoundTy { var, kind: BoundTyKind::Anon }));
            }
            TyKind::IntVar(_) => Some(CanonicalVarKind::Int),
            TyKind::FloatVar(_) => Some(CanonicalVarKind::Float),
            TyKind::Placeholder(p) => Some(CanonicalVarKind::PlaceholderTy(PlaceholderType { universe: self.universe(p.universe), bound: self.placeholder_index(p.bound) })),
            TyKind::Param(_) if self.infer_query => return ty,
            TyKind::Param(_) => {
                assert!(matches!(self.mode, CanonicalizeMode::Input { .. }), "param in response");
                Some(CanonicalVarKind::PlaceholderTy(PlaceholderType { universe: UniverseIndex::ROOT, bound: BoundVar(self.state.variables.len() as u32) }))
            }
            _ => None,
        };
        let result = if let Some(kind) = kind {
            let var = self.get_or_insert_bound_var(GenericArg::Type(ty), kind);
            tys.intern(TyKind::Bound(depth, BoundTy { var, kind: BoundTyKind::Anon }))
        } else { super_fold_ty(tys, ty, self, depth) };
        self.state.cache.insert((ty, depth), result);
        result
    }
    fn fold_region(&mut self, tys: &Types, mut region: RegionId, depth: DebruijnIndex) -> RegionId {
        if matches!(self.mode, CanonicalizeMode::UserType) {
            // canonicalizer.rs:231-246: no opportunistic resolution, so distinct written holes stay
            // independent variables even if type checking has already related their regions.
            return match tys.region_kind(region) {
                RegionKind::ReBound(..) => region,
                RegionKind::ReVar(_) => {
                    let var = self.get_or_insert_bound_var(GenericArg::Lifetime(region), CanonicalVarKind::Region(UniverseIndex::ROOT));
                    tys.intern_region(RegionKind::ReBound(depth, var))
                }
                RegionKind::RePlaceholder { .. } => panic!("placeholder region in a user type annotation"),
                _ => region,
            };
        }
        if let RegionKind::ReVar(vid) = tys.region_kind(region) {
            region = self.delegate.inner.borrow_mut().unwrap_region_constraints().shallow_resolve_region_var(tys, vid);
        }
        // rustc_infer canonicalizer.rs:323: erroneous regions are never
        // canonical variables in method queries.
        if self.infer_query && matches!(tys.region_kind(region), RegionKind::ReError) { return region; }
        let kind = match (tys.region_kind(region), self.mode) {
            (RegionKind::ReBound(..), _) => return region,
            (RegionKind::ReStatic, CanonicalizeMode::Input { in_param_env: true } | CanonicalizeMode::Response { .. }) => return region,
            (RegionKind::ReErased | RegionKind::ReError, CanonicalizeMode::Response { .. }) => return region,
            (_, CanonicalizeMode::Input { .. }) if self.infer_query => CanonicalVarKind::Region(UniverseIndex::ROOT),
            (RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..), CanonicalizeMode::Response { .. }) if self.infer_query => return region,
            (_, CanonicalizeMode::Input { .. }) => CanonicalVarKind::PlaceholderRegion { universe: UniverseIndex::ROOT, var: BoundVar(self.state.variables.len() as u32) },
            (RegionKind::RePlaceholder { universe, var }, CanonicalizeMode::Response { max_input_universe }) => {
                assert!(universe == UniverseIndex::ROOT || !max_input_universe.can_name(universe));
                CanonicalVarKind::PlaceholderRegion { universe, var }
            }
            (RegionKind::ReVar(_), CanonicalizeMode::Response { .. }) => CanonicalVarKind::Region(self.delegate.inner.borrow_mut().unwrap_region_constraints().universe(tys, region)),
            (RegionKind::ReEarlyParam(_) | RegionKind::ReLateParam(..), CanonicalizeMode::Response { .. }) => panic!("param region in response"),
            (_, CanonicalizeMode::UserType) => unreachable!(),
        };
        let var = self.get_or_insert_bound_var(GenericArg::Lifetime(region), kind);
        tys.intern_region(RegionKind::ReBound(depth, var))
    }
}


pub fn canonicalize_goal(infcx: &InferCtxt<'_>, goal: &Goal, opaque_types: &[(OpaqueTypeKey, TyId)], typing_mode: TypingMode) -> (Vec<GenericArg>, CanonicalQueryInput) {
    crate::solver_work::count(crate::solver_work::CANON_GOALS);
    let tcx = infcx.tcx;
    let tys = &tcx.tys;
    let mut c = Canonicalizer::new(infcx, CanonicalizeMode::Input { in_param_env: true });
    let mut resolved = goal.param_env;
    if tcx.param_env_flags(resolved) & TypeFlags::HAS_ANY_INFER.bits() != 0 {
        crate::solver_work::add(crate::solver_work::ENV_RESOLVE_CLAUSES, tcx.param_env_by_id(resolved).clauses.len() as u64);
        resolved = super::fold::fold_param_env(tcx, resolved, &mut DeepVariableResolver { infcx });
    }
    let flags = tcx.param_env_flags(resolved);
    let param_env = if flags & TypeFlags::NEEDS_CANONICAL.bits() == 0 {
        crate::solver_work::count(crate::solver_work::CANON_ENV_SKIPPED);
        resolved
    } else if flags & TypeFlags::HAS_NON_REGION_INFER.bits() == 0 {
        // rustc canonicalizer.rs: only type/const inference depends on the caller's
        // universes. Resolved region variables can use the environment's cell.
        let cell = tcx.canonical_param_env(resolved);
        if cell.get().is_none() {
            crate::solver_work::add(crate::solver_work::ENV_CANON_CLAUSES, tcx.param_env_by_id(resolved).clauses.len() as u64);
            let param_env = super::fold::fold_param_env(tcx, resolved, &mut c);
            let entry = CanonicalParamEnvCacheEntry {
                param_env,
                variables: std::mem::take(&mut c.state.variables),
                var_kinds: std::mem::take(&mut c.state.var_kinds),
                variable_lookup_table: std::mem::take(&mut c.state.variable_lookup_table),
            };
            // Compute without holding a lock. Races produce the same deterministic entry.
            let _ = cell.set(entry);
        }
        let entry = cell.get().unwrap();
        c.state.variables.extend_from_slice(&entry.variables);
        c.state.var_kinds.extend_from_slice(&entry.var_kinds);
        c.state.variable_lookup_table.extend(entry.variable_lookup_table.iter().map(|(&k, &v)| (k, v)));
        entry.param_env
    } else {
        crate::solver_work::add(crate::solver_work::ENV_CANON_CLAUSES, tcx.param_env_by_id(resolved).clauses.len() as u64);
        super::fold::fold_param_env(tcx, resolved, &mut c)
    };
    c.state.cache.clear();
    c.mode = CanonicalizeMode::Input { in_param_env: false };
    let predicate = if goal.predicate.flags(tys) & TypeFlags::NEEDS_CANONICAL.bits() == 0 {
        crate::solver_work::count(crate::solver_work::CANON_PRED_SKIPPED);
        goal.predicate.clone()
    } else { fold_predicate(tys, &goal.predicate, &mut c) };
    let mut predefined_opaques_in_body = Vec::with_capacity(opaque_types.len());
    for (key, ty) in opaque_types {
        let mut args = Vec::with_capacity(key.args.len());
        for &arg in &*key.args { args.push(super::fold::fold_arg(tys, arg, &mut c, 0)); }
        predefined_opaques_in_body.push((OpaqueTypeKey { def_id: key.def_id, args: args.into() }, c.fold_ty(tys, *ty, 0)));
    }
    let value = QueryInput { goal: Goal { param_env, predicate }, predefined_opaques_in_body };
    let (max_universe, originals, var_kinds) = c.finalize();
    (originals, CanonicalQueryInput { canonical: Canonical { value, max_universe, var_kinds }, typing_mode: TypingModeEqWrapper(typing_mode), coherence_crate: infcx.coherence_crate })
}

#[cfg(test)]
pub fn canonicalize_response(infcx: &InferCtxt<'_>, max_input_universe: UniverseIndex, values: &[GenericArg]) -> Canonical<Vec<GenericArg>> {
    let mut c = Canonicalizer::new(infcx, CanonicalizeMode::Response { max_input_universe });
    let mut value = Vec::with_capacity(values.len());
    for &arg in values { value.push(super::fold::fold_arg(&infcx.tcx.tys, arg, &mut c, 0)); }
    let (max_universe, _, var_kinds) = c.finalize();
    Canonical { value, max_universe, var_kinds }
}

pub fn instantiate_canonical_vars(infcx: &InferCtxt<'_>, max_universe: UniverseIndex, kinds: &[CanonicalVarKind]) -> Vec<GenericArg> {
    let mut universes = vec![infcx.universe()];
    for _ in 0..max_universe.0 { universes.push(infcx.create_next_universe()); }
    let tys = &infcx.tcx.tys;
    let mut values = Vec::with_capacity(kinds.len());
    for &kind in kinds {
        let universe = universes[kind.universe().0 as usize];
        values.push(match kind {
            CanonicalVarKind::Int => GenericArg::Type(infcx.next_int_var()),
            CanonicalVarKind::Float => GenericArg::Type(infcx.next_float_var()),
            CanonicalVarKind::Ty { .. } => GenericArg::Type(infcx.next_ty_var_in_universe(universe)),
            CanonicalVarKind::Region(_) => GenericArg::Lifetime(infcx.next_region_var_in_universe(universe)),
            CanonicalVarKind::Const(_) => GenericArg::Const(infcx.next_const_var_in_universe(universe)),
            CanonicalVarKind::PlaceholderTy(p) => GenericArg::Type(tys.intern(TyKind::Placeholder(PlaceholderType { universe, ..p }))),
            CanonicalVarKind::PlaceholderRegion { var, .. } => GenericArg::Lifetime(tys.intern_region(RegionKind::RePlaceholder { universe, var })),
            CanonicalVarKind::PlaceholderConst(p) => GenericArg::Const(tys.intern_const(ConstKind::Placeholder(PlaceholderConst { universe, ..p }))),
        });
    }
    for (index, kind) in kinds.iter().enumerate() {
        if let CanonicalVarKind::Ty { sub_root, .. } = kind {
            let GenericArg::Type(a) = values[index] else { unreachable!() };
            let GenericArg::Type(b) = values[sub_root.0 as usize] else { panic!("invalid sub-root") };
            let (TyKind::Infer(a), TyKind::Infer(b)) = (tys.kind(a), tys.kind(b)) else { unreachable!() };
            infcx.inner.borrow_mut().type_variables().sub_unify(*a, *b);
        }
    }
    values
}

struct CanonicalInstantiator<'a> { values: &'a [GenericArg] }
impl TypeFolder for CanonicalInstantiator<'_> {
    fn fold_flags(&self) -> Option<TypeFlags> { Some(TypeFlags::HAS_BOUND_VARS) }
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: DebruijnIndex) -> Const {
        match tys.const_kind(ct) {
            ConstKind::Bound(d, bound) if d == depth => {
                let GenericArg::Const(ct) = self.values[bound.var.0 as usize] else { panic!("canonical const kind") };
                Shift { amount: depth }.fold_const(tys, ct, 0)
            }
            ConstKind::Bound(d, bound) if d > depth => tys.intern_const(ConstKind::Bound(d - 1, bound)),
            _ => super_fold_const(tys, ct, self, depth),
        }
    }

    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: DebruijnIndex) -> TyId {
        if !tys.has_vars_bound_at_or_above(ty, depth) { return ty; }
        match *tys.kind(ty) {
            TyKind::Bound(d, bound) if d == depth => {
                let GenericArg::Type(ty) = self.values[bound.var.0 as usize] else { panic!("canonical type kind") };
                tys.shift_in(ty, depth)
            }
            TyKind::Bound(d, bound) if d > depth => tys.intern(TyKind::Bound(d - 1, bound)),
            _ => super_fold_ty(tys, ty, self, depth),
        }
    }
    fn fold_region(&mut self, tys: &Types, region: RegionId, depth: DebruijnIndex) -> RegionId {
        match tys.region_kind(region) {
            RegionKind::ReBound(d, var) if d == depth => {
                let GenericArg::Lifetime(region) = self.values[var.0 as usize] else { panic!("canonical region kind") };
                if let RegionKind::ReBound(d, var) = tys.region_kind(region) { tys.intern_region(RegionKind::ReBound(d + depth, var)) } else { region }
            }
            RegionKind::ReBound(d, var) if d > depth => tys.intern_region(RegionKind::ReBound(d - 1, var)),
            _ => region,
        }
    }
}

struct Shift { amount: u32 }
impl TypeFolder for Shift {
    fn fold_const(&mut self, tys: &Types, ct: Const, depth: DebruijnIndex) -> Const {
        match tys.const_kind(ct) {
            ConstKind::Bound(d, bound) if d >= depth => tys.intern_const(ConstKind::Bound(d + self.amount, bound)),
            _ => super_fold_const(tys, ct, self, depth),
        }
    }

    fn fold_ty(&mut self, tys: &Types, ty: TyId, depth: DebruijnIndex) -> TyId {
        match *tys.kind(ty) {
            TyKind::Bound(d, bound) if d >= depth => tys.intern(TyKind::Bound(d + self.amount, bound)),
            _ => super_fold_ty(tys, ty, self, depth),
        }
    }
    fn fold_region(&mut self, tys: &Types, region: RegionId, depth: DebruijnIndex) -> RegionId {
        match tys.region_kind(region) {
            RegionKind::ReBound(d, var) if d >= depth => tys.intern_region(RegionKind::ReBound(d + self.amount, var)),
            _ => region,
        }
    }
}


pub fn instantiate_query_input(tcx: &crate::tcx::Tcx, input: &QueryInput, values: &[GenericArg]) -> QueryInput {
    let tys = &tcx.tys;
    let mut f = CanonicalInstantiator { values };
    let param_env = super::fold::fold_param_env(tcx, input.goal.param_env, &mut f);
    let predicate = fold_predicate(tys, &input.goal.predicate, &mut f);
    let mut predefined_opaques_in_body = Vec::with_capacity(input.predefined_opaques_in_body.len());
    for (key, ty) in &input.predefined_opaques_in_body {
        let mut args = Vec::with_capacity(key.args.len());
        for &arg in &*key.args { args.push(super::fold::fold_arg(tys, arg, &mut f, 0)); }
        predefined_opaques_in_body.push((OpaqueTypeKey { def_id: key.def_id, args: args.into() }, f.fold_ty(tys, *ty, 0)));
    }
    QueryInput { goal: Goal { param_env, predicate }, predefined_opaques_in_body }
}

#[cfg(test)]
mod tests;

// canonical/mod.rs:556-613: proof states retain all query-local variable identities.
fn fold_state(tcx: &crate::tcx::Tcx, state: &super::inspect::State, folder: &mut dyn TypeFolder) -> super::inspect::State {
    let tys = &tcx.tys;
    use super::inspect::{State, StateData};
    let mut var_values = Vec::with_capacity(state.var_values.len());
    for &arg in &state.var_values { var_values.push(super::fold::fold_arg(tys, arg, folder, 0)); }
    let data = match &state.data {
        StateData::Unit => StateData::Unit,
        StateData::Goal(g) => StateData::Goal(super::fold::fold_goal(tcx, g, folder)),
        StateData::Args(args) => {
            let mut values = Vec::with_capacity(args.len());
            for &arg in args { values.push(super::fold::fold_arg(tys, arg, folder, 0)); }
            StateData::Args(values)
        }
    };
    State { var_values, data }
}
pub fn make_canonical_state(infcx: &InferCtxt<'_>, values: &[GenericArg], universe: UniverseIndex, data: super::inspect::StateData) -> super::inspect::CanonicalState {
    let state = super::inspect::State { var_values: values.to_vec(), data };
    let state = fold_state(infcx.tcx, &state, &mut DeepVariableResolver { infcx });
    let mut c = Canonicalizer::new(infcx, CanonicalizeMode::Response { max_input_universe: universe });
    let value = fold_state(infcx.tcx, &state, &mut c);
    let (max_universe, _, var_kinds) = c.finalize(); Canonical { value, max_universe, var_kinds }
}
pub fn instantiate_canonical_state(infcx: &InferCtxt<'_>, prog: &crate::program::Program, previous: UniverseIndex, original: &mut Vec<GenericArg>, state: &super::inspect::CanonicalState) -> super::inspect::StateData {
    let universe = UniverseIndex(previous.0 + state.max_universe.0);
    while infcx.universe() < universe { infcx.create_next_universe(); }
    for arg in &state.value.var_values[original.len()..] {
        original.push(match arg {
            GenericArg::Type(_) => GenericArg::Type(infcx.next_ty_var_in_universe(universe)),
            GenericArg::Lifetime(_) => GenericArg::Lifetime(infcx.next_region_var_in_universe(universe)),
            GenericArg::Const(_) => GenericArg::Const(infcx.next_const_var_in_universe(universe)),
        });
    }
    let response = Canonical { value: Response { var_values: state.value.var_values.clone(), certainty: Certainty::Yes, external_constraints: Default::default() }, max_universe: state.max_universe, var_kinds: state.var_kinds.clone() };
    let values = response::compute_query_response_instantiation_values_in_universe(infcx, original, &response, previous);
    let state = fold_state(infcx.tcx, &state.value, &mut CanonicalInstantiator { values: &values });
    crate::infer::relate::relate_args(&mut response::ResponseRelating { infcx, prog }, original, &state.var_values).expect("invalid canonical state");
    state.data
}

/// rustc_next_trait_solver/src/canonical/mod.rs:611-629 and
/// rustc_type_ir/src/canonical.rs:284-304: identity without inference allocation.
pub fn response_no_constraints_raw(tys: &Types, max_universe: UniverseIndex, var_kinds: &[CanonicalVarKind], certainty: Certainty) -> Canonical<Response> {
    let mut var_values = Vec::with_capacity(var_kinds.len());
    for (i, kind) in var_kinds.iter().enumerate() {
        let var = BoundVar(i as u32);
        var_values.push(match kind {
            CanonicalVarKind::Ty { .. } | CanonicalVarKind::Int | CanonicalVarKind::Float | CanonicalVarKind::PlaceholderTy(_) => GenericArg::Type(tys.intern(TyKind::Bound(0, BoundTy { var, kind: BoundTyKind::Anon }))),
            CanonicalVarKind::Region(_) | CanonicalVarKind::PlaceholderRegion { .. } => GenericArg::Lifetime(tys.intern_region(RegionKind::ReBound(0, var))),
            CanonicalVarKind::Const(_) | CanonicalVarKind::PlaceholderConst(_) => GenericArg::Const(tys.intern_const(ConstKind::Bound(0, BoundConst { var }))),
        });
    }
    Canonical { max_universe, var_kinds: var_kinds.to_vec(), value: Response { certainty, var_values, external_constraints: Default::default() } }
}
