// ported from N:rustc_infer/src/infer/mod.rs @8d1a764
pub mod diagnostics;
pub mod free_regions;
pub mod lexical_region_resolve;
pub mod outlives;
pub mod fresh_args;
pub mod type_variable;
pub mod relate;
pub mod at;
pub mod resolve;
pub mod typing_mode;
pub mod region_constraints;
pub mod snapshot;
pub mod opaque_types;
pub mod unify_key;
use crate::tcx::{Tcx, TypingMode};
use crate::types::{RegionId, TyId, TyKind, UniverseIndex};
use std::cell::{Cell, RefCell};

#[derive(Default)]
pub struct InferCtxtInner {
    type_variable_storage: type_variable::TypeVariableStorage,
    region_constraint_storage: Option<region_constraints::RegionConstraintStorage>,
    region_obligations: Vec<(TyId, RegionId, u32)>,
    opaque_type_storage: opaque_types::OpaqueTypeStorage,
    const_unification_storage: unify_key::ConstUnificationStorage,
    int_unification_storage: unify_key::IntUnificationStorage,
    float_unification_storage: unify_key::FloatUnificationStorage,
    undo_log: snapshot::undo_log::InferCtxtUndoLogs,
}

impl InferCtxtInner {
    pub fn region_obligations(&self) -> &[(TyId, RegionId, u32)] { &self.region_obligations }
    pub fn const_unification_table(&mut self) -> unify_key::ConstUnificationTable<'_> { self.const_unification_storage.with_log(&mut self.undo_log) }
    pub fn int_unification_table(&mut self) -> unify_key::IntUnificationTable<'_> { self.int_unification_storage.with_log(&mut self.undo_log) }
    pub fn float_unification_table(&mut self) -> unify_key::FloatUnificationTable<'_> { self.float_unification_storage.with_log(&mut self.undo_log) }
    pub fn type_variables(&mut self) -> type_variable::TypeVariableTable<'_> { self.type_variable_storage.with_log(&mut self.undo_log) }
    pub fn unwrap_region_constraints(&mut self) -> region_constraints::RegionConstraintCollector<'_> { self.region_constraint_storage.as_mut().expect("regions already resolved").with_log(&mut self.undo_log) }
    pub fn opaque_types(&mut self) -> opaque_types::OpaqueTypeTable<'_> { self.opaque_type_storage.with_log(&mut self.undo_log) }
}

pub struct InferCtxt<'a> {
    pub tcx: &'a Tcx,
    pub typing_mode: Cell<TypingMode>,
    pub recursion_limit: crate::tcx::RecursionLimit,
    pub skip_leak_check: bool,
    pub cause: crate::next_solver::fulfill::ObligationCause,
    /// The span of the root obligation being proven (the solver's `origin_span`): the origin of the region obligations it registers.
    pub origin_span: Cell<u32>,
    pub errors: diagnostics::DiagCtxt,
    pub(crate) diagnostic_checkpoint: diagnostics::DiagnosticCheckpoint,
    tainted: Cell<bool>,
    pub(crate) coherence_crate: Option<u32>,
    pub inner: RefCell<InferCtxtInner>,
    universe: Cell<UniverseIndex>,
    pub(crate) lexical_region_resolutions: RefCell<Option<lexical_region_resolve::LexicalRegionResolutions>>,
    pub(crate) canonicalizer_state: RefCell<crate::next_solver::canonical::CanonicalizerState>,
}

impl<'a> InferCtxt<'a> {
    /// rustc_infer/src/infer/mod.rs:1088: errors emitted through this context's taintable
    /// handle (`dcx()`, :724) or observed as error types (set_tainted_by_errors).
    pub fn tainted_by_errors(&self) -> bool { self.tainted.get() || self.errors.tainted_with_errors() }
    pub fn set_tainted_by_errors(&self) { self.tainted.set(true); }
    pub fn register_ty_outlives(&self, ty: TyId, region: RegionId) {
        let mut inner = self.inner.borrow_mut();
        inner.region_obligations.push((ty, region, self.origin_span.get()));
        inner.undo_log.push(snapshot::undo_log::UndoLog::PushRegionObligation);
    }
    pub fn new(tcx: &'a Tcx, typing_mode: TypingMode, recursion_limit: crate::tcx::RecursionLimit) -> Self {
        Self { tcx, typing_mode: Cell::new(typing_mode), recursion_limit, skip_leak_check: false, cause: Default::default(), origin_span: Cell::new(0), errors: tcx.dcx.into_taintable(), diagnostic_checkpoint: tcx.dcx.checkpoint(), tainted: Cell::new(false), coherence_crate: None, inner: RefCell::new(InferCtxtInner { region_constraint_storage: Some(Default::default()), ..Default::default() }), lexical_region_resolutions: RefCell::new(None), universe: Cell::new(UniverseIndex::ROOT), canonicalizer_state: RefCell::new(Default::default()) }
    }
    pub fn for_item(tcx: &'a Tcx, prog: &crate::program::Program, typing_mode: TypingMode, def: crate::program::DefId) -> Self {
        let mut infcx = Self::new(tcx, typing_mode, crate::tcx::RecursionLimit::for_item(prog, def));
        if def != crate::program::NO_DEF {
            let d = prog.def(def);
            infcx.cause = crate::next_solver::fulfill::ObligationCause { span: prog.files[d.file as usize].ast.item(d.item).lo, body_id: def };
        }
        infcx
    }
    pub fn recursion_limit(&self, prog: &crate::program::Program) -> usize {
        // The crate root's attributes: that crate's declaration input (lead decision 52 R1).
        if self.cause.body_id != crate::program::NO_DEF {
            self.tcx.queries.read_def_declarations(prog, self.cause.body_id).expect("collected program");
            let krate = prog.def(self.cause.body_id).krate;
            let root = &prog.mods[prog.crates[krate as usize].root_mod as usize];
            let file = &prog.files[root.file as usize];
            for attr in &file.ast.root_attrs {
                if attr.toks.hi > attr.toks.lo && file.tok_text(attr.toks.lo) == "recursion_limit" {
                    if attr.toks.hi == attr.toks.lo + 3 && file.tok_text(attr.toks.lo + 1) == "="
                        && file.tok_text(attr.toks.lo + 2).trim_matches('"').parse::<usize>().is_ok() { break; }
                    self.errors.report(self.cause, "invalid recursion_limit attribute".into());
                }
            }
        }
        self.recursion_limit.0
    }
    pub fn universe(&self) -> UniverseIndex { self.universe.get() }
    pub fn create_next_universe(&self) -> UniverseIndex {
        let universe = UniverseIndex(self.universe.get().0.checked_add(1).expect("universe overflow"));
        self.universe.set(universe);
        universe
    }
    pub fn next_ty_var(&self) -> TyId { self.next_ty_var_in_universe(self.universe()) }
    pub fn next_ty_var_in_universe(&self, universe: UniverseIndex) -> TyId {
        let vid = self.inner.borrow_mut().type_variables().new_var(universe);
        self.tcx.tys.intern(TyKind::Infer(vid))
    }
    pub fn next_const_var(&self) -> crate::consts::Const { self.next_const_var_in_universe(self.universe()) }
    pub fn next_const_var_in_universe(&self, universe: UniverseIndex) -> crate::consts::Const {
        use crate::consts::{ConstKind, ConstVid, InferConst};
        let vid = self.inner.borrow_mut().const_unification_table().new_key(unify_key::ConstVariableValue::Unknown { universe });
        self.tcx.tys.intern_const(ConstKind::Infer(InferConst::Var(ConstVid(vid))))
    }
    pub fn next_int_var(&self) -> TyId {
        let vid = self.inner.borrow_mut().int_unification_table().new_key(unify_key::IntVarValue::Unknown);
        self.tcx.tys.intern(TyKind::IntVar(vid))
    }
    pub fn next_float_var(&self) -> TyId {
        let vid = self.inner.borrow_mut().float_unification_table().new_key(unify_key::FloatVarValue::Unknown);
        self.tcx.tys.intern(TyKind::FloatVar(vid))
    }
    pub fn shallow_resolve(&self, ty: TyId) -> TyId {
        let mut inner = self.inner.borrow_mut();
        match *self.tcx.tys.kind(ty) {
            TyKind::Infer(vid) => {
                let mut table = inner.type_variables();
                let root = table.root_var(vid);
                let value = table.probe(root).known();
                drop(inner);
                match value { Some(ty) => self.shallow_resolve(ty), None => self.tcx.tys.intern(TyKind::Infer(root)) }
            }
            TyKind::IntVar(vid) => {
                use unify_key::IntVarValue::*;
                let mut table = inner.int_unification_table();
                let root = table.find(vid);
                self.tcx.tys.intern(match table.probe_value(root) {
                    Unknown => TyKind::IntVar(root), IntType(p) | UintType(p) => TyKind::Int(p),
                })
            }
            TyKind::FloatVar(vid) => {
                use unify_key::FloatVarValue::*;
                let mut table = inner.float_unification_table();
                let root = table.find(vid);
                self.tcx.tys.intern(match table.probe_value(root) {
                    Unknown => TyKind::FloatVar(root), Known(p) => TyKind::Float(p),
                })
            }
            _ => ty,
        }
    }
    pub fn shallow_resolve_const(&self, ct: crate::consts::Const) -> crate::consts::Const {
        use crate::consts::{ConstKind, InferConst};
        if let ConstKind::Infer(InferConst::Var(vid)) = self.tcx.tys.const_kind(ct) {
            {
                let mut inner = self.inner.borrow_mut();
                let mut table = inner.const_unification_table();
                let root = table.find(vid.0);
                table.probe_value(root).known().unwrap_or_else(|| self.tcx.tys.intern_const(ConstKind::Infer(InferConst::Var(crate::consts::ConstVid(root)))))
            }
        } else { ct }
    }
    pub fn next_region_var(&self) -> RegionId { self.next_region_var_in_universe(self.universe()) }
    /// mod.rs:1023-1035.
    pub fn next_nll_region_var(&self, origin: crate::borrowck::region_infer::NllRegionVariableOrigin) -> RegionId {
        self.next_nll_region_var_in_universe(origin, self.universe())
    }
    pub fn next_nll_region_var_in_universe(&self, origin: crate::borrowck::region_infer::NllRegionVariableOrigin, universe: UniverseIndex) -> RegionId {
        self.inner.borrow_mut().unwrap_region_constraints().new_region_var_with_origin(&self.tcx.tys, universe, region_constraints::RegionVariableOrigin::Nll(origin))
    }
    pub fn num_region_vars(&self) -> usize { self.inner.borrow_mut().unwrap_region_constraints().num_region_vars() }
    pub fn region_var_origin(&self, vid: u32) -> region_constraints::RegionVariableOrigin { self.inner.borrow_mut().unwrap_region_constraints().var_origin(vid) }
    /// outlives/obligations.rs:201.
    pub fn take_registered_region_obligations(&self) -> Vec<(TyId, RegionId)> {
        assert!(!self.in_snapshot(), "cannot take registered region obligations in a snapshot");
        std::mem::take(&mut self.inner.borrow_mut().region_obligations).into_iter().map(|(t, r, _)| (t, r)).collect()
    }
    /// mod.rs take_and_reset_region_constraints.
    pub fn take_and_reset_region_constraints(&self) -> region_constraints::RegionConstraintData {
        assert!(self.inner.borrow().region_obligations.is_empty(), "region_obligations not empty");
        self.inner.borrow_mut().unwrap_region_constraints().take_and_reset_data()
    }
    pub fn next_region_var_in_universe(&self, universe: UniverseIndex) -> RegionId {
        self.inner.borrow_mut().unwrap_region_constraints().new_region_var(&self.tcx.tys, universe)
    }
}

#[cfg(test)]
mod tests;

impl InferCtxt<'_> {
    pub fn require_lang_item(&self, item: crate::lang_items::LangItem) -> Option<crate::program::DefId> {
        match self.tcx.lang_items.require(item) {
            Ok(def) => Some(def),
            Err(error) => { self.errors.report(self.cause, error.to_string()); None }
        }
    }
}
