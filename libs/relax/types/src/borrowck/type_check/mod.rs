// ported from N:rustc_borrowck/src/type_check/mod.rs @8d1a764
//! MIR type check: relates the types of every assignment, call, cast, aggregate,
//! annotation and constant, producing C0a outlives constraints with their
//! locations, categories and file-qualified spans. Inference, normalization,
//! trait proving and region allocation are N01's (`BorrowckTypeOps`); each such
//! operation converts its own region constraints into the sink it is given.
pub mod free_region_relations;
pub mod canonical;
pub(crate) mod inhabitedness;
pub mod constraint_conversion;
mod input_output;
pub mod liveness;
mod relate_tys;

use super::borrowck_errors::{BorrowckError, ErrorKind};
use super::constraints::{AnnotationSource, ConstraintCategory, OutlivesConstraint, OutlivesConstraintSet,
    RegionVid, ReturnConstraint};
use super::mir_nll::LocationMap;
use super::region_infer::{NllRegionVariableOrigin, TypeTest};
use super::region_infer::values::{LivenessValues, PlaceholderIndices};
use crate::lang_items::LangItem;
use crate::consts::GenericArg;
use crate::mir::visit::{MutatingUseContext, NonMutatingUseContext, NonUseContext, PlaceContext, Visitor};
use crate::mir::*;
use crate::program::{DefId, Program};
use crate::tcx::Tcx;
use crate::types::fold::BoundVarReplacer;
use crate::types::variance::V;
use crate::types::{FnHeader, RegionId, RegionKind, Safety, TyId, TyKind, UniverseIndex};
pub use relate_tys::NoSolution;

/// rustc_middle FnSig with its binder skipped: inputs/output may contain
/// `ReBound(0, _)` for the late-bound regions of a `PolyFnSig`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FnSig { pub inputs: Vec<TyId>, pub output: TyId, pub header: FnHeader }
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PolyFnSig { pub sig: FnSig, pub bound_regions: u32 }

/// The user type of an `ascribe_user_type` operation (rustc UserType).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UserType {
    /// A canonical annotation of the body, instantiated by N01's `instantiate_canonical`.
    Annotation(UserTypeId),
    /// `UserTypeKind::TypeOf(def, UserArgs { args, user_self_ty: None })`.
    TypeOf(DefId, Vec<GenericArg>),
}

/// The definitions whose clauses `normalize_and_prove_instantiated_clauses` instantiates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClauseOwner { Def(DefId), Closure(ClosureId), Coroutine(ClosureId) }

/// `crate::DeferredClosureRequirements` entry: the nested body is checked later
/// and its requirements applied at these locations.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeferredClosureRequirement { pub def: ClauseOwner, pub args: Vec<GenericArg>, pub locations: Locations }

/// N:diagnostics/bound_region_errors.rs UniverseInfo: why a universe was created.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum UniverseInfo { RelateTys { expected: TyId, found: TyId }, TypeOp, Other }

/// A `span_mirbug!`: the MIR is ill-typed. It taints the body, never accepts it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MirBug { pub span: Span, pub message: String }

/// The operations N01 supplies: N:lib.rs:651-736 (BorrowckInferCtxt), rustc_infer's
/// region/type variables and generalization, N:type_check/canonical.rs (type ops run
/// through `fully_perform_op`, whose region constraints are converted into `out` by
/// N:type_check/constraint_conversion.rs at the given locations and category), and the
/// typed queries of rustc_middle that need region-preserving native answers.
pub trait BorrowckTypeOps {
    fn universe(&self) -> UniverseIndex;
    fn create_next_universe(&mut self) -> UniverseIndex;
    /// `next_nll_region_var_in_universe`; the result is a `ReVar` region.
    fn next_nll_region_var(&mut self, origin: NllRegionVariableOrigin, universe: UniverseIndex) -> RegionId;
    /// UniversalRegions::to_region_vid: `ReVar(v)` is `v`; universal regions map to their vids.
    fn to_region_vid(&self, r: RegionId) -> RegionVid;
    fn encountered_re_error(&self) -> bool;
    fn shallow_resolve(&mut self, ty: TyId) -> TyId;
    fn shallow_resolve_const(&mut self, ct: crate::consts::Const) -> crate::consts::Const;
    fn next_ty_var(&mut self, span: Span) -> TyId;
    /// rustc_infer generalize.rs:124 instantiate_var up to `union_var_term`: generalizes
    /// `source` for `variance`, binds `vid`, returns the generalized type for the caller to relate.
    fn generalize_ty_var(&mut self, vid: u32, variance: V, source: TyId, span: Span) -> Result<TyId, NoSolution>;
    /// Opaque relations (N:type_check/relate_tys.rs relate_opaques, next-solver alias-relate),
    /// with the opaque storage of the root's inference context.
    fn relate_opaque(&mut self, out: &mut MirTypeckRegionConstraints, a: TyId, variance: V, b: TyId,
        locations: Locations, category: ConstraintCategory) -> Result<(), NoSolution>;
    fn prove_trait_ref(&mut self, out: &mut MirTypeckRegionConstraints, trait_: LangItem, args: &[GenericArg],
        locations: Locations, category: ConstraintCategory);
    /// `prove_clause(ClauseKind::WellFormed(ty))`.
    fn prove_wf(&mut self, out: &mut MirTypeckRegionConstraints, ty: TyId, locations: Locations, category: ConstraintCategory);
    fn normalize_and_prove_instantiated_clauses(&mut self, out: &mut MirTypeckRegionConstraints, owner: ClauseOwner,
        args: &[GenericArg], locations: Locations);
    fn normalize(&mut self, out: &mut MirTypeckRegionConstraints, ty: TyId, locations: Locations) -> TyId;
    /// Err is an ErrorGuaranteed already reported by the operation.
    fn deeply_normalize_sig(&mut self, out: &mut MirTypeckRegionConstraints, sig: &FnSig, location: Location) -> Result<FnSig, ()>;
    fn struct_tail(&mut self, out: &mut MirTypeckRegionConstraints, ty: TyId, location: Location) -> TyId;
    fn ascribe_user_type(&mut self, out: &mut MirTypeckRegionConstraints, inferred: TyId, user: UserType, span: Span);
    fn ascribe_user_type_skip_wf(&mut self, out: &mut MirTypeckRegionConstraints, inferred: TyId, user: TyId, span: Span);
    /// `CanonicalUserTypeAnnotation { span, inferred_ty }` of an annotation.
    fn user_type_annotation(&self, id: UserTypeId) -> (Span, TyId);
    /// `closure_user_provided_sig` of a closure-like body, canonical vars and binder
    /// instantiated with fresh variables (input_output.rs:48-56); None for other bodies.
    fn closure_user_provided_sig(&mut self, body: &Body) -> Option<FnSig>;
    /// `fn_sig` of a FnDef with its early-bound (type and lifetime) arguments instantiated.
    fn fn_def_sig(&self, def: DefId, args: &[TyId], regions: crate::types::RegionArgsId) -> PolyFnSig;
    /// The declared (unnormalized, region-preserving) type of field `field` of `base`.
    fn field_ty(&self, base: PlaceTy, field: FieldIdx) -> Option<TyId>;
    /// Field types of an ADT/closure/coroutine aggregate; Err is the field count.
    fn aggregate_field_ty(&mut self, out: &mut MirTypeckRegionConstraints, kind: &AggregateKind, field: FieldIdx,
        location: Location) -> Result<TyId, usize>;
    /// Rvalue::ty for forms typed by queries: ThreadLocalRef, Discriminant, PtrMetadata, Cmp,
    /// ShallowInitBox, Repeat with a non-literal length and ADT/closure/coroutine aggregates.
    fn rvalue_query_ty(&self, body: &Body, rvalue: &Rvalue) -> TyId;
    /// The type of an unevaluated constant (`type_of(def)` instantiated with `args`).
    fn unevaluated_const_ty(&self, def: DefId, args: &[GenericArg]) -> TyId;
    /// `ConstOperand::check_static_ptr`: the static this constant points to.
    fn static_ptr(&self, constant: &ConstOperand) -> Option<DefId>;
    /// `type_of(static)` instantiated and normalized.
    fn static_ty(&mut self, out: &mut MirTypeckRegionConstraints, def: DefId, locations: Locations) -> TyId;
    fn type_is_sized_modulo_regions(&mut self, ty: TyId) -> bool;
    fn is_privately_uninhabited(&mut self, ty: TyId) -> bool;
    /// N:path_utils.rs is_upvar_field_projection (needs MIR capture metadata).
    fn is_upvar_field_projection(&self, body: &Body, place: &Place) -> Option<FieldIdx>;
    /// `(argument index, name)` of simd_shuffle/simd_insert/simd_extract.
    fn simd_const_arg(&self, def: DefId) -> Option<(usize, &'static str)>;
    /// (identity args of the typeck root, the closure's parent args) for #98589.
    fn closure_parent_args(&self, def: ClauseOwner, args: &[GenericArg]) -> (Vec<GenericArg>, Vec<GenericArg>);
    fn unsized_fn_params(&self) -> bool;
    /// `def_kind == AnonConst && anon_const_kind == NonTypeSystemInline`.
    fn is_inline_const(&self, def: DefId) -> bool;
}

/// Where region constraints are required to hold (mod.rs:346).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Locations {
    /// Everywhere, e.g. user annotations and the signature; the span is the annotation's.
    All(Span),
    /// At a single location, where references flow from one spot to another.
    Single(Location),
}
impl Locations {
    pub fn from_location(&self) -> Option<Location> {
        match self { Locations::All(_) => None, Locations::Single(l) => Some(*l) }
    }
    pub fn span(&self, body: &Body) -> Span {
        match self { Locations::All(span) => *span, Locations::Single(l) => source_info(body, *l).span }
    }
}
pub fn source_info(body: &Body, l: Location) -> &SourceInfo {
    let block = &body.basic_blocks[l.block.0 as usize];
    match block.statements.get(l.statement_index) {
        Some(s) => &s.source_info,
        None => &block.terminator.as_ref().expect("location past an unfinished block").source_info,
    }
}
fn to_locations(l: Location) -> Locations { Locations::Single(l) }

/// MirTypeckRegionConstraints (mod.rs:274).
#[derive(Clone, Debug)]
pub struct MirTypeckRegionConstraints {
    pub placeholder_indices: PlaceholderIndices,
    /// The representative `ReVar` of each placeholder index.
    pub placeholder_index_to_region: Vec<RegionId>,
    /// Liveness of regions the type checker creates (late-bound call-site regions etc.).
    pub liveness_constraints: LivenessValues,
    pub outlives_constraints: OutlivesConstraintSet,
    /// Insertion-ordered (FxIndexMap in nightly).
    pub universe_causes: Vec<(UniverseIndex, UniverseInfo)>,
    pub type_tests: Vec<TypeTest>,
}
impl MirTypeckRegionConstraints {
    pub fn new(num_points: usize) -> Self {
        Self { placeholder_indices: Default::default(), placeholder_index_to_region: vec![],
            liveness_constraints: LivenessValues::new(0, num_points), outlives_constraints: Default::default(),
            universe_causes: vec![], type_tests: vec![] }
    }
    /// The region for a placeholder (an interned `RePlaceholder`), created once.
    pub fn placeholder_region(&mut self, infcx: &mut dyn BorrowckTypeOps, tcx: &Tcx, placeholder: RegionId) -> RegionId {
        let index = self.placeholder_indices.insert(placeholder);
        if let Some(&r) = self.placeholder_index_to_region.get(index) { return r; }
        let RegionKind::RePlaceholder { universe, .. } = tcx.tys.region_kind(placeholder) else { panic!("not a placeholder") };
        let region = infcx.next_nll_region_var(NllRegionVariableOrigin::Placeholder(placeholder), universe);
        self.placeholder_index_to_region.push(region);
        region
    }
}

/// MirTypeckResults (mod.rs:262). Region-bound pairs, known type-outlives and the
/// universal relations stay with N01's `free_region_relations::create` caller.
pub struct MirTypeckResults {
    pub constraints: MirTypeckRegionConstraints,
    pub deferred_closure_requirements: Vec<DeferredClosureRequirement>,
    pub errors: Vec<BorrowckError>,
    pub mir_bugs: Vec<MirBug>,
    pub tainted_by_errors: bool,
}

/// `type_check` (mod.rs:97): `normalized_inputs_and_output` and the universal
/// yield/resume types come from N01's `free_region_relations::create`. Liveness
/// (`liveness::generate`) runs on the region input the caller assembles from the result.
pub fn type_check(tcx: &Tcx, prog: &Program, infcx: &mut dyn BorrowckTypeOps, body: &Body, normalized_inputs_and_output: &[TyId],
    universal_yield_resume: Option<(TyId, TyId)>) -> MirTypeckResults {
    let location_map = LocationMap::new(body);
    let mut typeck = TypeChecker {
        tcx, prog, infcx, last_span: Span::default(), body, promoted: &body.promoted, defining: body.source.clone(),
        location_map: location_map.clone(), reported_errors: vec![],
        constraints: MirTypeckRegionConstraints::new(location_map.num_points()),
        deferred_closure_requirements: vec![], errors: vec![], mir_bugs: vec![], tainted_by_errors: false,
    };
    typeck.check_user_type_annotations();
    typeck.visit_body(body);
    typeck.equate_inputs_and_outputs(normalized_inputs_and_output, universal_yield_resume);
    typeck.check_signature_annotation();
    let mut tainted_by_errors = typeck.tainted_by_errors || !typeck.mir_bugs.is_empty() || !typeck.errors.is_empty();
    // A region error from type check clears the constraints that would only repeat it.
    if typeck.infcx.encountered_re_error() {
        typeck.constraints.outlives_constraints = Default::default();
        typeck.constraints.type_tests = Default::default();
        tainted_by_errors = true;
    }
    MirTypeckResults { constraints: typeck.constraints, deferred_closure_requirements: typeck.deferred_closure_requirements,
        errors: typeck.errors, mir_bugs: typeck.mir_bugs, tainted_by_errors }
}

/// The MIR type checker (mod.rs:224).
pub struct TypeChecker<'a> {
    tcx: &'a Tcx,
    prog: &'a Program,
    infcx: &'a mut dyn BorrowckTypeOps,
    last_span: Span,
    body: &'a Body,
    /// The root's promoted bodies (user type annotations are shared with them).
    promoted: &'a [Body],
    /// The defining body (the root, also while a promoted is checked).
    defining: MirSource,
    location_map: LocationMap,
    reported_errors: Vec<(TyId, Span)>,
    constraints: MirTypeckRegionConstraints,
    deferred_closure_requirements: Vec<DeferredClosureRequirement>,
    errors: Vec<BorrowckError>,
    mir_bugs: Vec<MirBug>,
    /// An error type was related (an earlier error was reported).
    tainted_by_errors: bool,
}

impl<'a> TypeChecker<'a> {
    fn mirbug(&mut self, message: String) {
        self.mir_bugs.push(MirBug { span: self.last_span, message });
    }
    fn is_user_variable(&self, l: Local) -> bool {
        let decl = &self.body.local_decls[l.0 as usize];
        decl.kind == LocalKind::UserVar || matches!(decl.local_info, LocalInfo::User { .. })
    }
    /// The category of a write to the return place of the defining body.
    fn return_category(&self) -> ConstraintCategory {
        match self.defining.kind {
            BodyKind::Static => ConstraintCategory::UseAsStatic,
            BodyKind::Const | BodyKind::InlineConst | BodyKind::AnonConst => ConstraintCategory::UseAsConst,
            _ => ConstraintCategory::Return(ReturnConstraint::Normal),
        }
    }

    /// Equate the inferred type and the annotated type for user type annotations.
    fn check_user_type_annotations(&mut self) {
        let body = self.body;
        for &id in &body.user_type_annotations {
            let (span, inferred_ty) = self.infcx.user_type_annotation(id);
            self.infcx.ascribe_user_type(&mut self.constraints, inferred_ty, UserType::Annotation(id), span);
        }
    }

    /// Try to relate `sub <: sup`.
    fn sub_types(&mut self, sub: TyId, sup: TyId, locations: Locations, category: ConstraintCategory) -> Result<(), NoSolution> {
        // The sup type is usually the "expected" type in diagnostics.
        self.relate_types(sup, V::Contra, sub, locations, category)
    }
    fn eq_types(&mut self, expected: TyId, found: TyId, locations: Locations, category: ConstraintCategory) -> Result<(), NoSolution> {
        self.relate_types(expected, V::Inv, found, locations, category)
    }

    fn relate_type_and_user_type(&mut self, a: TyId, v: V, user_ty: &UserTypeProjection, locations: Locations,
        category: ConstraintCategory) -> Result<(), NoSolution> {
        let id = self.body.user_type_annotations[user_ty.base.0 as usize];
        let (_, annotated_type) = self.infcx.user_type_annotation(id);
        let mut curr_projected_ty = PlaceTy::from_ty(annotated_type);
        for proj in &user_ty.projection {
            curr_projected_ty = match proj {
                PlaceElem::Field(field, _) => {
                    let Some(ty) = self.infcx.field_ty(curr_projected_ty, *field) else { return Err(NoSolution) };
                    PlaceTy::from_ty(self.infcx.normalize(&mut self.constraints, ty, locations))
                }
                elem => curr_projected_ty.projection_ty(self.tcx, elem).ok_or(NoSolution)?,
            };
        }
        let ty = curr_projected_ty.ty;
        self.relate_types(ty, crate::types::variance::xform(v, V::Contra), a, locations, category)
    }

    /// Check a promoted body in this context and transfer its constraints to `location`.
    fn check_promoted(&mut self, promoted_body: &'a Body, location: Location) {
        let parent_body = std::mem::replace(&mut self.body, promoted_body);
        let parent_map = std::mem::replace(&mut self.location_map, LocationMap::new(promoted_body));
        let mut constraints = OutlivesConstraintSet::default();
        let mut liveness_constraints = LivenessValues::new(0, self.location_map.num_points());
        let mut deferred = vec![];
        let swap = |this: &mut Self, c: &mut OutlivesConstraintSet, l: &mut LivenessValues, d: &mut Vec<_>| {
            std::mem::swap(&mut this.constraints.outlives_constraints, c);
            std::mem::swap(&mut this.constraints.liveness_constraints, l);
            std::mem::swap(&mut this.deferred_closure_requirements, d);
        };
        swap(self, &mut constraints, &mut liveness_constraints, &mut deferred);
        self.visit_body(promoted_body);
        self.body = parent_body;
        self.location_map = parent_map;
        swap(self, &mut constraints, &mut liveness_constraints, &mut deferred);
        let locations = to_locations(location);
        let point = self.location_map.point(location);
        for mut constraint in constraints.outlives {
            constraint.location = Some(point);
            if let ConstraintCategory::Return(_) | ConstraintCategory::UseAsConst | ConstraintCategory::UseAsStatic = constraint.category {
                // "Returning" from a promoted is an assignment to a temporary.
                constraint.category = ConstraintCategory::Boring;
            }
            self.constraints.outlives_constraints.push(constraint);
        }
        // Nested bodies of the promoted are instantiated at this location; their returns stay returns.
        for mut requirement in deferred {
            requirement.locations = locations;
            self.deferred_closure_requirements.push(requirement);
        }
        for region in liveness_constraints.live_regions() {
            self.constraints.liveness_constraints.add_point(region, point);
        }
    }

    fn operand_ty(&self, op: &Operand) -> TyId {
        match op {
            Operand::Copy(place) | Operand::Move(place) => self.place_ty(place).ty,
            Operand::Constant(c) => self.const_ty(c),
        }
    }
    fn const_ty(&self, c: &ConstOperand) -> TyId {
        match &c.const_ {
            Const::Val(_, ty) => *ty,
            Const::Unevaluated { promoted: Some(p), .. } => self.promoted[p.0 as usize].local_decls[0].ty,
            Const::Unevaluated { def, args, promoted: None } => self.infcx.unevaluated_const_ty(*def, args),
        }
    }
    fn place_ty(&self, place: &Place) -> PlaceTy {
        place.ty(self.body, self.tcx).expect("ill-formed MIR place")
    }
    /// rustc_middle mir/tcx.rs Rvalue::ty.
    fn rvalue_ty(&self, rv: &Rvalue) -> TyId {
        let tys = &self.tcx.tys;
        match rv {
            Rvalue::Use(op) | Rvalue::UnaryOp(UnOp::Not | UnOp::Neg, op) => self.operand_ty(op),
            Rvalue::Repeat(op, len) => match try_to_target_usize(len) {
                Some(n) => tys.intern(TyKind::Array(self.operand_ty(op), tys.const_usize(n))),
                None => self.infcx.rvalue_query_ty(self.body, rv),
            },
            Rvalue::Ref(region, kind, place) => {
                let r = tys.intern_region(RegionKind::ReVar(region.0));
                tys.intern(TyKind::Ref(matches!(kind, BorrowKind::Mut { .. }), self.place_ty(place).ty, r))
            }
            Rvalue::RawPtr(kind, place) => tys.intern(TyKind::Ptr(*kind == RawPtrKind::Mut, self.place_ty(place).ty)),
            Rvalue::Len(_) | Rvalue::NullaryOp(NullOp::SizeOf | NullOp::AlignOf | NullOp::OffsetOf(_), _) => tys.usize_,
            Rvalue::NullaryOp(NullOp::UbChecks | NullOp::ContractChecks, _) => tys.bool_,
            Rvalue::Cast(_, _, ty) => *ty,
            Rvalue::BinaryOp(op, ops) => {
                let lhs = self.operand_ty(&ops.0);
                match op {
                    BinOp::Eq | BinOp::Lt | BinOp::Le | BinOp::Ne | BinOp::Ge | BinOp::Gt => tys.bool_,
                    BinOp::AddWithOverflow | BinOp::SubWithOverflow | BinOp::MulWithOverflow => tys.intern(TyKind::Tuple(tys.mk_list(&[lhs, tys.bool_]))),
                    BinOp::Cmp => self.infcx.rvalue_query_ty(self.body, rv),
                    _ => lhs,
                }
            }
            Rvalue::CopyForDeref(place) => self.place_ty(place).ty,
            Rvalue::Aggregate(kind, ops) => match &**kind {
                AggregateKind::Array(ty) => tys.intern(TyKind::Array(*ty, tys.const_usize(ops.len() as u64))),
                AggregateKind::Tuple => tys.intern(TyKind::Tuple(tys.mk_list(&ops.iter().map(|op| self.operand_ty(op)).collect::<Vec<_>>()))),
                _ => self.infcx.rvalue_query_ty(self.body, rv),
            },
            Rvalue::ThreadLocalRef(_) | Rvalue::Discriminant(_) | Rvalue::UnaryOp(UnOp::PtrMetadata, _)
            | Rvalue::ShallowInitBox(..) => self.infcx.rvalue_query_ty(self.body, rv),
        }
    }

    fn check_call_dest(&mut self, sig: &FnSig, destination: &Place, is_diverging: bool, term_location: Location) {
        if is_diverging {
            if !self.infcx.is_privately_uninhabited(self.tcx.tys.erase_regions(sig.output)) {
                self.mirbug(format!("call to non-diverging function {sig:?} w/o dest"));
            }
        } else {
            let dest_ty = self.place_ty(destination).ty;
            let dest_ty = self.infcx.normalize(&mut self.constraints, dest_ty, to_locations(term_location));
            let category = match as_local(destination) {
                Some(Local(0)) => self.return_category(),
                Some(l) if !self.is_user_variable(l) => ConstraintCategory::Boring,
                // The return type of a call is interesting for diagnostics.
                _ => ConstraintCategory::Assignment,
            };
            if let Err(terr) = self.sub_types(sig.output, dest_ty, to_locations(term_location), category) {
                self.mirbug(format!("call dest mismatch ({dest_ty:?} <- {:?}): {terr:?}", sig.output));
            }
            // Without unsized_fn_params this is visit_local_decl's check.
            if self.infcx.unsized_fn_params() {
                let span = source_info(self.body, term_location).span;
                self.ensure_place_sized(dest_ty, span);
            }
        }
    }

    fn check_call_inputs(&mut self, func: &Operand, sig: &FnSig, args: &[Operand], term_location: Location, call_source: CallSource) {
        if args.len() < sig.inputs.len() || (args.len() > sig.inputs.len() && !sig.header.c_variadic) {
            self.mirbug(format!("call to {sig:?} with wrong # of args"));
        }
        let func_ty = self.operand_ty(func);
        if let TyKind::FnDef(def, ..) = *self.tcx.tys.kind(func_ty) {
            // Some SIMD intrinsics need a particular argument to be a constant.
            if let Some((idx, _name)) = self.infcx.simd_const_arg(def) {
                if !matches!(args.get(idx), Some(Operand::Constant(_))) {
                    let span = source_info(self.body, term_location).span;
                    self.errors.push(BorrowckError { kind: ErrorKind::SimdIntrinsicArgConst, span });
                }
            }
        }
        for (n, (&fn_arg, op_arg)) in sig.inputs.iter().zip(args).enumerate() {
            let op_arg_ty = self.operand_ty(op_arg);
            let op_arg_ty = self.infcx.normalize(&mut self.constraints, op_arg_ty, to_locations(term_location));
            let category = if from_hir_call(call_source) {
                ConstraintCategory::CallArgument(Some(self.tcx.tys.erase_regions(func_ty)))
            } else {
                ConstraintCategory::Boring
            };
            if let Err(terr) = self.sub_types(op_arg_ty, fn_arg, to_locations(term_location), category) {
                self.mirbug(format!("bad arg #{n:?} ({fn_arg:?} <- {op_arg_ty:?}): {terr:?}"));
            }
        }
    }

    fn check_iscleanup(&mut self, block_data: &BasicBlockData) {
        let is_cleanup = block_data.is_cleanup;
        let Some(term) = &block_data.terminator else { return };
        match &term.kind {
            TerminatorKind::Goto { target } => self.assert_iscleanup(*target, is_cleanup),
            TerminatorKind::SwitchInt { targets, .. } => {
                for &target in targets.targets.iter().chain([&targets.otherwise]) { self.assert_iscleanup(target, is_cleanup); }
            }
            TerminatorKind::UnwindResume => if !is_cleanup { self.mirbug("resume on non-cleanup block!".into()) },
            TerminatorKind::UnwindTerminate(_) => if !is_cleanup { self.mirbug("terminate on non-cleanup block!".into()) },
            TerminatorKind::Return => if is_cleanup { self.mirbug("return on cleanup block".into()) },
            TerminatorKind::TailCall { .. } => if is_cleanup { self.mirbug("tailcall on cleanup block".into()) },
            TerminatorKind::CoroutineDrop => if is_cleanup { self.mirbug("coroutine_drop in cleanup block".into()) },
            TerminatorKind::Yield { resume, drop, .. } => {
                if is_cleanup { self.mirbug("yield in cleanup block".into()) }
                self.assert_iscleanup(*resume, is_cleanup);
                if let Some(drop) = drop { self.assert_iscleanup(*drop, is_cleanup); }
            }
            TerminatorKind::Unreachable => {}
            TerminatorKind::Drop { target, unwind, .. } | TerminatorKind::Assert { target, unwind, .. } => {
                self.assert_iscleanup(*target, is_cleanup);
                self.assert_iscleanup_unwind(*unwind, is_cleanup);
            }
            TerminatorKind::Call { target, unwind, .. } => {
                if let Some(target) = target { self.assert_iscleanup(*target, is_cleanup); }
                self.assert_iscleanup_unwind(*unwind, is_cleanup);
            }
            TerminatorKind::FalseEdge { real_target, imaginary_target } => {
                self.assert_iscleanup(*real_target, is_cleanup);
                self.assert_iscleanup(*imaginary_target, is_cleanup);
            }
            TerminatorKind::FalseUnwind { real_target, unwind } => {
                self.assert_iscleanup(*real_target, is_cleanup);
                self.assert_iscleanup_unwind(*unwind, is_cleanup);
            }
            // Its targets are not represented; visit_terminator already reported it.
            TerminatorKind::InlineAsm(_) => {}
        }
    }
    fn assert_iscleanup(&mut self, bb: BasicBlock, iscleanuppad: bool) {
        if self.body.basic_blocks[bb.0 as usize].is_cleanup != iscleanuppad {
            self.mirbug(format!("cleanuppad mismatch: {bb:?} should be {iscleanuppad:?}"));
        }
    }
    fn assert_iscleanup_unwind(&mut self, unwind: UnwindAction, is_cleanup: bool) {
        match unwind {
            UnwindAction::Cleanup(unwind) => {
                if is_cleanup { self.mirbug("unwind on cleanup block".into()) }
                self.assert_iscleanup(unwind, true);
            }
            UnwindAction::Continue => if is_cleanup { self.mirbug("unwind on cleanup block".into()) },
            UnwindAction::Unreachable | UnwindAction::Terminate(_) => {}
        }
    }

    fn ensure_place_sized(&mut self, ty: TyId, span: Span) {
        // Sized does not depend on regions.
        let erased_ty = self.tcx.tys.erase_regions(ty);
        if !self.infcx.type_is_sized_modulo_regions(erased_ty) && !self.reported_errors.contains(&(ty, span)) {
            // Every unsized rvalue evaluates through a temp, return slot or local.
            self.reported_errors.push((ty, span));
            self.errors.push(BorrowckError { kind: ErrorKind::UnsizedMove, span });
        }
    }

    /// If this rvalue supports a user-given type annotation, return it.
    fn rvalue_user_ty(&self, rvalue: &Rvalue) -> Option<UserTypeAnnotationIndex> {
        match rvalue {
            Rvalue::Aggregate(kind, _) => match &**kind {
                AggregateKind::Adt(_, _, _, user_ty, _) => *user_ty,
                _ => None,
            },
            _ => None,
        }
    }

    fn check_aggregate_rvalue(&mut self, kind: &AggregateKind, operands: &[Operand], location: Location) {
        self.prove_aggregate_predicates(kind, location);
        match kind {
            // A tuple's field types are its operands' types.
            AggregateKind::Tuple => return,
            AggregateKind::RawPtr(..) => {
                self.mirbug("RawPtr should only be in runtime MIR".into());
                return;
            }
            _ => {}
        }
        for (i, operand) in operands.iter().enumerate() {
            let field_ty = match kind {
                AggregateKind::Array(ty) => Ok(*ty),
                _ => self.infcx.aggregate_field_ty(&mut self.constraints, kind, FieldIdx(i as u32), location),
            };
            let field_ty = match field_ty {
                Ok(ty) => ty,
                Err(field_count) => {
                    self.mirbug(format!("accessed field #{i} but variant only has {field_count}"));
                    continue;
                }
            };
            let operand_ty = self.operand_ty(operand);
            let operand_ty = self.infcx.normalize(&mut self.constraints, operand_ty, to_locations(location));
            if let Err(terr) = self.sub_types(operand_ty, field_ty, to_locations(location), ConstraintCategory::Boring) {
                self.mirbug(format!("{operand_ty:?} is not a subtype of {field_ty:?}: {terr:?}"));
            }
        }
    }

    /// Constraints of a borrow `&'a P` at `location`: reborrowing through a
    /// reference requires its region to outlive the borrow's.
    fn add_reborrow_constraint(&mut self, location: Location, borrow_region: Region, borrowed_place: &Place) {
        let category = match self.infcx.is_upvar_field_projection(self.body, borrowed_place) {
            Some(field) => ConstraintCategory::ClosureUpvar(field),
            None => ConstraintCategory::Boring,
        };
        let point = self.location_map.point(location);
        let span = source_info(self.body, location).span;
        for i in (0..borrowed_place.projection.len()).rev() {
            if borrowed_place.projection[i] != PlaceElem::Deref { continue; }
            let base = Place { local: borrowed_place.local, projection: borrowed_place.projection[..i].to_vec() };
            let base_ty = self.place_ty(&base).ty;
            match *self.tcx.tys.kind(base_ty) {
                TyKind::Ref(mutbl, _, ref_region) => {
                    let sup = self.infcx.to_region_vid(ref_region);
                    self.constraints.outlives_constraints.push(OutlivesConstraint {
                        sup, sub: borrow_region.0 as RegionVid, location: Some(point), span, category, from_closure: false,
                    });
                    // A shared reference need not stay valid for the whole borrow;
                    // a mutable one must, or `&mut *x` could clone `&mut T` out of `&&mut T`.
                    if !mutbl { break; }
                }
                // Deref of a raw pointer is guaranteed valid.
                TyKind::Ptr(..) => break,
                // Deref of Box needs the base to be valid: propagate.
                TyKind::Adt(def, ..) if self.tcx.adts.get(&def).is_some_and(|a| a.is_box()) => {}
                _ => self.mirbug(format!("unexpected deref ty {base_ty:?} in {borrowed_place:?}")),
            }
        }
    }

    fn prove_aggregate_predicates(&mut self, kind: &AggregateKind, location: Location) {
        let (owner, args) = match kind {
            AggregateKind::Adt(def, _, args, _, _) => (ClauseOwner::Def(*def), args.clone()),
            // Closures carry requirements on the free regions of their creator, like where
            // clauses on the closure struct; they are checked here where it is instantiated.
            AggregateKind::Closure(id, args) => (ClauseOwner::Closure(*id), self.prove_closure_bounds(ClauseOwner::Closure(*id), args, location)),
            AggregateKind::Coroutine(id, args) => (ClauseOwner::Coroutine(*id), self.prove_closure_bounds(ClauseOwner::Coroutine(*id), args, location)),
            AggregateKind::Array(_) | AggregateKind::Tuple | AggregateKind::RawPtr(..) => return,
        };
        self.infcx.normalize_and_prove_instantiated_clauses(&mut self.constraints, owner, &args, to_locations(location));
    }

    fn prove_closure_bounds(&mut self, def: ClauseOwner, args: &[GenericArg], location: Location) -> Vec<GenericArg> {
        // Propagated requirements are applied once the nested body has been checked.
        self.deferred_closure_requirements.push(DeferredClosureRequirement { def, args: args.to_vec(), locations: to_locations(location) });
        // Equate closure args to regions inherited from the typeck root (#98589).
        let (typeck_root_args, parent_args) = self.infcx.closure_parent_args(def, args);
        assert_eq!(typeck_root_args.len(), parent_args.len());
        if self.eq_args(&typeck_root_args, &parent_args, to_locations(location), ConstraintCategory::BoringNoLocation).is_err() {
            self.mirbug(format!("could not relate closure to parent {typeck_root_args:?} != {parent_args:?}"));
        }
        args.to_vec()
    }

    fn check_cast(&mut self, cast_kind: CastKind, op: &Operand, ty: TyId, location: Location) {
        let tcx = self.tcx;
        let tys = &tcx.tys;
        let cast = |implicit: bool| ConstraintCategory::Cast { is_raw_ptr_dyn_type_cast: false, is_implicit_coercion: implicit, unsize_to: None };
        let locations = to_locations(location);
        let ty_from = self.operand_ty(op);
        match cast_kind {
            CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer, source) => {
                let implicit = source == CoercionSource::Implicit;
                let Some(mut src_sig) = self.fn_sig(ty_from) else { return self.mirbug(format!("reify of non-function {ty_from:?}")) };
                // Target safety: a safe fn item may be reified to an unsafe fn pointer.
                let TyKind::FnPtr(_, _, target_hdr, target_bound) = *tys.kind(ty) else { return self.mirbug(format!("reify to {ty:?}")) };
                // type_check/mod.rs:1047-1057: a safe #[target_feature] fn item is safe where
                // the body has its features
                if let TyKind::FnDef(def_id, _, _) = *tys.kind(ty_from) {
                    if tcx.fn_item_sig(self.prog, def_id).is_some_and(|s| s.safe_target_features)
                        && target_hdr.safety == Safety::Safe
                        && tcx.adjust_target_feature_sig(self.prog, def_id, self.body.source.owner) {
                        src_sig.sig.header.safety = Safety::Safe;
                    }
                }
                if src_sig.sig.header.safety == Safety::Safe && target_hdr.safety == Safety::Unsafe {
                    src_sig.sig.header.safety = Safety::Unsafe;
                }
                // Casting a higher-ranked fn item to a non-higher-ranked fn pointer throws away
                // implied bounds, so prove the instantiated signature WF first.
                let target_has_bound = target_bound != 0 && self.fn_ptr_has_bound_vars(ty);
                if src_sig.bound_regions != 0 && self.poly_sig_has_bound_vars(&src_sig) && !target_has_bound {
                    let sig = self.instantiate_with_fresh_vars(&src_sig);
                    let src_ty = mk_fn_ptr(self.tcx, &sig, 0);
                    self.infcx.prove_wf(&mut self.constraints, src_ty, locations, cast(implicit));
                    let src_ty = self.infcx.normalize(&mut self.constraints, src_ty, locations);
                    if let Err(terr) = self.sub_types(src_ty, ty, locations, cast(implicit)) {
                        self.mirbug(format!("equating {ty:?} with {src_ty:?} yields {terr:?}"));
                    }
                }
                let src_ty = mk_fn_ptr(self.tcx, &src_sig.sig, src_sig.bound_regions);
                // The fn item's signature is not WF-checked through the FnDef type.
                self.infcx.prove_wf(&mut self.constraints, src_ty, locations, cast(implicit));
                let src_ty = self.infcx.normalize(&mut self.constraints, src_ty, locations);
                if let Err(terr) = self.sub_types(src_ty, ty, locations, cast(implicit)) {
                    self.mirbug(format!("equating {src_ty:?} with {ty:?} yields {terr:?}"));
                }
            }
            CastKind::PointerCoercion(PointerCoercion::ClosureFnPointer, source) => {
                // Relax closure types carry their un-tupled signature as a FnPtr.
                let TyKind::Closure(_, _, crate::types::ClosureArgs { closure_sig_as_fn_ptr_ty: sig, .. }) = *tys.kind(ty_from) else { return self.mirbug(format!("closure cast of {ty_from:?}")) };
                let TyKind::FnPtr(inputs, output, header, n) = tys.kind(sig).clone() else { return self.mirbug(format!("closure signature {sig:?}")) };
                // signature_unclosure: the target pointer's safety (nightly's payload) is the target type's.
                let safety = match tys.kind(ty) { TyKind::FnPtr(_, _, h, _) => h.safety, _ => header.safety };
                let ty_fn_ptr_from = tys.intern(TyKind::FnPtr(inputs, output, FnHeader { safety, ..FnHeader::RUST }, n));
                if let Err(terr) = self.sub_types(ty_fn_ptr_from, ty, locations, cast(source == CoercionSource::Implicit)) {
                    self.mirbug(format!("equating {ty_fn_ptr_from:?} with {ty:?} yields {terr:?}"));
                }
            }
            CastKind::PointerCoercion(PointerCoercion::UnsafeFnPointer, source) => {
                let Some(sig) = self.fn_sig(ty_from) else { return self.mirbug(format!("unsafe fn cast of {ty_from:?}")) };
                // The fn_sig query may hold unnormalized types.
                let fn_ptr = self.infcx.normalize(&mut self.constraints, mk_fn_ptr(self.tcx, &sig.sig, sig.bound_regions), locations);
                let TyKind::FnPtr(inputs, output, header, n) = tys.kind(fn_ptr).clone() else { return self.mirbug(format!("normalized signature {fn_ptr:?}")) };
                let ty_fn_ptr_from = tys.intern(TyKind::FnPtr(inputs, output, FnHeader { safety: Safety::Unsafe, ..header }, n));
                if let Err(terr) = self.sub_types(ty_fn_ptr_from, ty, locations, cast(source == CoercionSource::Implicit)) {
                    self.mirbug(format!("equating {ty_fn_ptr_from:?} with {ty:?} yields {terr:?}"));
                }
            }
            CastKind::PointerCoercion(PointerCoercion::Unsize, source) => {
                let unsize_to = erase_region_vars(self.tcx, ty);
                self.infcx.prove_trait_ref(&mut self.constraints, LangItem::CoerceUnsized, &[GenericArg::Type(ty_from), GenericArg::Type(ty)], locations,
                    ConstraintCategory::Cast { is_raw_ptr_dyn_type_cast: false, is_implicit_coercion: source == CoercionSource::Implicit, unsize_to: Some(unsize_to) });
            }
            CastKind::PointerCoercion(PointerCoercion::MutToConstPointer, source) => {
                let TyKind::Ptr(true, from) = *tys.kind(ty_from) else { return self.mirbug(format!("unexpected base type for cast {ty:?}")) };
                let TyKind::Ptr(false, to) = *tys.kind(ty) else { return self.mirbug(format!("unexpected target type for cast {ty:?}")) };
                if let Err(terr) = self.sub_types(from, to, locations, cast(source == CoercionSource::Implicit)) {
                    self.mirbug(format!("relating {from:?} with {to:?} yields {terr:?}"));
                }
            }
            CastKind::PointerCoercion(PointerCoercion::ArrayToPointer, source) => {
                let Some((elem, from_mut)) = (match *tys.kind(ty_from) {
                    TyKind::Ptr(m, array) => match *tys.kind(array) { TyKind::Array(elem, _) => Some((elem, m)), _ => None },
                    _ => None,
                }) else { return self.mirbug(format!("ArrayToPointer cast from unexpected type {ty_from:?}")) };
                let TyKind::Ptr(to_mut, to) = *tys.kind(ty) else { return self.mirbug(format!("ArrayToPointer cast to unexpected type {ty:?}")) };
                if to_mut && !from_mut { return self.mirbug(format!("ArrayToPointer cast from const {ty:?} to mut {to:?}")); }
                if let Err(terr) = self.sub_types(elem, to, locations, cast(source == CoercionSource::Implicit)) {
                    self.mirbug(format!("relating {elem:?} with {to:?} yields {terr:?}"));
                }
            }
            CastKind::PointerExposeProvenance => if !matches!((self.cast_ty(ty_from), self.cast_ty(ty)), (Some(CastTy::Ptr(_) | CastTy::FnPtr), Some(CastTy::Int(IntLike::Int)))) {
                self.mirbug(format!("Invalid PointerExposeProvenance cast {ty_from:?} -> {ty:?}"));
            },
            CastKind::PointerWithExposedProvenance => if !matches!((self.cast_ty(ty_from), self.cast_ty(ty)), (Some(CastTy::Int(IntLike::Int)), Some(CastTy::Ptr(_)))) {
                self.mirbug(format!("Invalid PointerWithExposedProvenance cast {ty_from:?} -> {ty:?}"));
            },
            CastKind::IntToInt => if !matches!((self.cast_ty(ty_from), self.cast_ty(ty)), (Some(CastTy::Int(_)), Some(CastTy::Int(_)))) {
                self.mirbug(format!("Invalid IntToInt cast {ty_from:?} -> {ty:?}"));
            },
            CastKind::IntToFloat => if !matches!((self.cast_ty(ty_from), self.cast_ty(ty)), (Some(CastTy::Int(IntLike::Int)), Some(CastTy::Float))) {
                self.mirbug(format!("Invalid IntToFloat cast {ty_from:?} -> {ty:?}"));
            },
            CastKind::FloatToInt => if !matches!((self.cast_ty(ty_from), self.cast_ty(ty)), (Some(CastTy::Float), Some(CastTy::Int(IntLike::Int)))) {
                self.mirbug(format!("Invalid FloatToInt cast {ty_from:?} -> {ty:?}"));
            },
            CastKind::FloatToFloat => if !matches!((self.cast_ty(ty_from), self.cast_ty(ty)), (Some(CastTy::Float), Some(CastTy::Float))) {
                self.mirbug(format!("Invalid FloatToFloat cast {ty_from:?} -> {ty:?}"));
            },
            CastKind::FnPtrToPtr => if !matches!((self.cast_ty(ty_from), self.cast_ty(ty)), (Some(CastTy::FnPtr), Some(CastTy::Ptr(_)))) {
                self.mirbug(format!("Invalid FnPtrToPtr cast {ty_from:?} -> {ty:?}"));
            },
            CastKind::PtrToPtr => {
                let (Some(CastTy::Ptr(src)), Some(CastTy::Ptr(dst))) = (self.cast_ty(ty_from), self.cast_ty(ty)) else {
                    return self.mirbug(format!("Invalid PtrToPtr cast {ty_from:?} -> {ty:?}"));
                };
                if self.infcx.type_is_sized_modulo_regions(dst) {
                    // Wide to thin pointer cast; may occur in an environment with impossible clauses.
                    self.infcx.prove_trait_ref(&mut self.constraints, LangItem::Sized, &[GenericArg::Type(dst)], locations, cast(true));
                } else {
                    let src_tail = self.infcx.struct_tail(&mut self.constraints, src, location);
                    let dst_tail = self.infcx.struct_tail(&mut self.constraints, dst, location);
                    if let (TyKind::Dyn(..), TyKind::Dyn(..)) = (tys.kind(src_tail), tys.kind(dst_tail)) {
                        // Relax trait objects always have a principal (tcx rejects `dyn Send`), so this
                        // is nightly's (Some, Some) arm: trait arguments are invariant, only the
                        // object lifetime is subtyped (#136702). Auto traits are hir typeck's.
                        let src_obj = without_auto_traits(self.tcx, src_tail);
                        let dst_obj = without_auto_traits(self.tcx, dst_tail);
                        let category = ConstraintCategory::Cast { is_raw_ptr_dyn_type_cast: true, is_implicit_coercion: false, unsize_to: None };
                        if let Err(terr) = self.sub_types(src_obj, dst_obj, locations, category) {
                            self.mirbug(format!("dyn pointer cast {src_obj:?} -> {dst_obj:?}: {terr:?}"));
                        }
                    }
                }
            }
            // Pattern types are not represented, so no Transmute is permitted in analysis MIR.
            CastKind::Transmute => self.mirbug(format!("Unexpected CastKind::Transmute {ty_from:?} -> {ty:?}, which is not permitted in Analysis MIR")),
        }
    }

    fn cast_ty(&self, t: TyId) -> Option<CastTy> {
        Some(match self.tcx.tys.kind(t) {
            TyKind::Int(_) => CastTy::Int(IntLike::Int),
            TyKind::Bool | TyKind::Char => CastTy::Int(IntLike::Other),
            TyKind::Adt(def, ..) if self.tcx.adts.get(def).is_some_and(|a| a.is_enum && a.variants.iter().all(|v| v.fields.is_empty())) => CastTy::Int(IntLike::Other),
            TyKind::Float(_) => CastTy::Float,
            TyKind::FnPtr(..) => CastTy::FnPtr,
            TyKind::Ptr(_, t) => CastTy::Ptr(*t),
            _ => return None,
        })
    }

    /// `func_ty.fn_sig(tcx)` for FnDef and FnPtr.
    fn fn_sig(&self, func_ty: TyId) -> Option<PolyFnSig> {
        match self.tcx.tys.kind(func_ty) {
            TyKind::FnPtr(inputs, output, header, n) => Some(PolyFnSig { sig: FnSig { inputs: self.tcx.tys.list(*inputs).to_vec(), output: *output, header: *header }, bound_regions: *n }),
            TyKind::FnDef(def, args, regions) => Some(self.infcx.fn_def_sig(*def, self.tcx.tys.list(*args), *regions)),
            _ => None,
        }
    }
    fn poly_sig_has_bound_vars(&self, sig: &PolyFnSig) -> bool {
        sig.sig.inputs.iter().chain([&sig.sig.output]).any(|&t| self.tcx.tys.has_vars_bound_at_or_above(t, 0))
    }
    fn fn_ptr_has_bound_vars(&self, ty: TyId) -> bool {
        match self.fn_sig(ty) { Some(sig) => self.poly_sig_has_bound_vars(&sig), None => false }
    }
    /// `instantiate_binder_with_fresh_vars(HigherRankedType)`.
    fn instantiate_with_fresh_vars(&mut self, sig: &PolyFnSig) -> FnSig {
        let tcx = self.tcx;
        let tys = &tcx.tys;
        let infcx = &mut *self.infcx;
        let universe = infcx.universe();
        let mut replacer = BoundVarReplacer::new(|_| infcx.next_nll_region_var(NllRegionVariableOrigin::Existential, universe),
            |b| panic!("unexpected bound type {b:?} in a fn signature"));
        FnSig { inputs: sig.sig.inputs.iter().map(|&t| replacer.instantiate(tys, t)).collect(),
            output: replacer.instantiate(tys, sig.sig.output), header: sig.sig.header }
    }
}

impl<'a> TypeChecker<'a> {
    fn visit_body_checked(&mut self, body: &'a Body) {
        for (i, decl) in body.local_decls.iter().enumerate() {
            self.check_local_decl(Local(i as u32), decl);
        }
        for (b, block_data) in body.basic_blocks.iter().enumerate() {
            let mut location = Location { block: BasicBlock(b as u32), statement_index: 0 };
            for stmt in &block_data.statements {
                self.check_statement(stmt, location);
                location.statement_index += 1;
            }
            if let Some(term) = &block_data.terminator { self.check_terminator(term, location); }
            self.check_iscleanup(block_data);
        }
    }

    fn check_statement(&mut self, stmt: &Statement, location: Location) {
        self.super_statement(stmt, location);
        match &stmt.kind {
            StatementKind::Assign(place, rv) => {
                // Assignments to temporaries are lowering artifacts, not user-caused.
                let category = match as_local(place) {
                    Some(Local(0)) => self.return_category(),
                    Some(l) if matches!(self.body.local_decls[l.0 as usize].local_info, LocalInfo::AggregateTemp) => ConstraintCategory::Usage,
                    Some(l) if !self.is_user_variable(l) => ConstraintCategory::Boring,
                    _ => ConstraintCategory::Assignment,
                };
                let place_ty = self.place_ty(place).ty;
                let place_ty = self.infcx.normalize(&mut self.constraints, place_ty, to_locations(location));
                let rv_ty = self.rvalue_ty(rv);
                let rv_ty = self.infcx.normalize(&mut self.constraints, rv_ty, to_locations(location));
                if let Err(terr) = self.sub_types(rv_ty, place_ty, to_locations(location), category) {
                    self.mirbug(format!("bad assignment ({place_ty:?} = {rv_ty:?}): {terr:?}"));
                }
                if let Some(index) = self.rvalue_user_ty(rv) {
                    if let Err(terr) = self.relate_type_and_user_type(rv_ty, V::Inv, &UserTypeProjection { base: index, projection: vec![] },
                        to_locations(location), ConstraintCategory::TypeAnnotation(AnnotationSource::GenericArg)) {
                        self.mirbug(format!("bad user type on rvalue ({index:?} = {rv_ty:?}): {terr:?}"));
                    }
                }
                if !self.infcx.unsized_fn_params() {
                    self.infcx.prove_trait_ref(&mut self.constraints, LangItem::Sized, &[GenericArg::Type(place_ty)], to_locations(location), ConstraintCategory::SizedBound);
                }
            }
            StatementKind::AscribeUserType(place, projection, variance) => {
                let place_ty = self.place_ty(place).ty;
                if let Err(terr) = self.relate_type_and_user_type(place_ty, mir_variance(*variance), projection,
                    Locations::All(stmt.source_info.span), ConstraintCategory::TypeAnnotation(AnnotationSource::Ascription)) {
                    self.mirbug(format!("bad type assert ({place_ty:?} <: {:?} with projections {:?}): {terr:?}", projection.base, projection.projection));
                }
            }
            StatementKind::Intrinsic(NonDivergingIntrinsic::Assume(..)) | StatementKind::FakeRead(..) | StatementKind::StorageLive(..)
            | StatementKind::StorageDead(..) | StatementKind::ConstEvalCounter | StatementKind::PlaceMention(..) | StatementKind::Nop => {}
            StatementKind::Intrinsic(NonDivergingIntrinsic::CopyNonOverlapping { .. }) | StatementKind::SetDiscriminant { .. } => {
                self.mirbug("Statement not allowed in this MIR phase".into());
            }
        }
    }

    fn check_terminator(&mut self, term: &Terminator, term_location: Location) {
        self.super_terminator(term, term_location);
        let tcx = self.tcx;
        let tys = &tcx.tys;
        match &term.kind {
            TerminatorKind::Goto { .. } | TerminatorKind::UnwindResume | TerminatorKind::UnwindTerminate(_)
            | TerminatorKind::Return | TerminatorKind::CoroutineDrop | TerminatorKind::Unreachable
            | TerminatorKind::Drop { .. } | TerminatorKind::FalseEdge { .. } | TerminatorKind::FalseUnwind { .. } => {}
            // Operands are not represented; nothing may be accepted unchecked.
            TerminatorKind::InlineAsm(_) => self.mirbug("inline assembly operands are not represented in MIR".into()),
            TerminatorKind::SwitchInt { discr, .. } => {
                let switch_ty = self.operand_ty(discr);
                if !matches!(tys.kind(switch_ty), TyKind::Int(_) | TyKind::Char | TyKind::Bool) {
                    self.mirbug(format!("bad SwitchInt discr ty {switch_ty:?}"));
                }
            }
            TerminatorKind::Call { func, args, .. } | TerminatorKind::TailCall { func, args, .. } => {
                let (call_source, destination, is_diverging) = match &term.kind {
                    TerminatorKind::Call { call_source, destination, target, .. } => (*call_source, destination.clone(), target.is_none()),
                    _ => (CallSource::Normal, Place::local(Local(0)), false),
                };
                let func_ty = self.operand_ty(func);
                let Some(sig) = self.fn_sig(func_ty) else { return self.mirbug(format!("call to non-function {func_ty:?}")) };
                // Late-bound regions become fresh region variables, live at the call.
                let mut late_bound = vec![];
                let unnormalized_sig = {
                    let infcx = &mut *self.infcx;
                    let universe = infcx.universe();
                    let mut replacer = BoundVarReplacer::new(|_| {
                        let r = infcx.next_nll_region_var(NllRegionVariableOrigin::Existential, universe);
                        late_bound.push(r);
                        r
                    }, |b| panic!("unexpected bound type {b:?} in a fn signature"));
                    FnSig { inputs: sig.sig.inputs.iter().map(|&t| replacer.instantiate(tys, t)).collect(),
                        output: replacer.instantiate(tys, sig.sig.output), header: sig.sig.header }
                };
                // WF of the signature before normalization: `<&'a &'b () as Trait>::Assoc`
                // normalizes away its `'b: 'a` bound (#91068).
                for &ty in unnormalized_sig.inputs.iter().chain([&unnormalized_sig.output]) {
                    self.infcx.prove_wf(&mut self.constraints, ty, to_locations(term_location), ConstraintCategory::Boring);
                }
                let Ok(sig) = self.infcx.deeply_normalize_sig(&mut self.constraints, &unnormalized_sig, term_location) else { return };
                // WF(sig) does not imply WF(normalized(sig)) with built-in Fn impls (#114936).
                if sig != unnormalized_sig {
                    for &ty in sig.inputs.iter().chain([&sig.output]) {
                        self.infcx.prove_wf(&mut self.constraints, ty, to_locations(term_location), ConstraintCategory::Boring);
                    }
                }
                self.check_call_dest(&sig, &destination, is_diverging, term_location);
                // The late-bound regions instantiated at the call must be live there.
                let point = self.location_map.point(term_location);
                for r in late_bound {
                    let vid = self.infcx.to_region_vid(r);
                    self.constraints.liveness_constraints.add_point(vid, point);
                }
                self.check_call_inputs(func, &sig, args, term_location, call_source);
            }
            TerminatorKind::Assert { cond, msg, .. } => {
                let cond_ty = self.operand_ty(cond);
                if cond_ty != tys.bool_ { self.mirbug(format!("bad Assert ({cond_ty:?}, not bool")); }
                if let AssertKind::BoundsCheck { len, index } = msg {
                    if self.operand_ty(len) != tys.usize_ { self.mirbug(format!("bounds-check length non-usize {len:?}")); }
                    if self.operand_ty(index) != tys.usize_ { self.mirbug(format!("bounds-check index non-usize {index:?}")); }
                }
            }
            TerminatorKind::Yield { value, resume_arg, .. } => {
                match self.body.coroutine.as_ref().map(|c| (c.yield_ty, c.resume_ty)) {
                    None => self.mirbug("yield in non-coroutine".into()),
                    Some((yield_ty, resume_ty)) => {
                        let value_ty = self.operand_ty(value);
                        if let Err(terr) = self.sub_types(value_ty, yield_ty, to_locations(term_location), ConstraintCategory::Yield) {
                            self.mirbug(format!("type of yield value is {value_ty:?}, but the yield type is {yield_ty:?}: {terr:?}"));
                        }
                        let resume_place_ty = self.place_ty(resume_arg).ty;
                        if let Err(terr) = self.sub_types(resume_ty, resume_place_ty, to_locations(term_location), ConstraintCategory::Yield) {
                            self.mirbug(format!("type of resume place is {resume_place_ty:?}, but the resume type is {resume_ty:?}: {terr:?}"));
                        }
                    }
                }
            }
        }
    }

    fn check_local_decl(&mut self, local: Local, decl: &LocalDecl) {
        self.visit_local_decl(decl);
        for user_ty in &decl.user_ty {
            let id = self.body.user_type_annotations[user_ty.base.0 as usize];
            let (span, _) = self.infcx.user_type_annotation(id);
            let ty = if matches!(decl.local_info, LocalInfo::User { is_ref_binding: false, .. }) {
                decl.ty
            } else if let TyKind::Ref(_, rty, _) = *self.tcx.tys.kind(decl.ty) {
                // `let ref x: T = ..`: the annotation is on the referent.
                rty
            } else {
                self.mirbug(format!("{local:?} with ref binding has wrong type {:?}", decl.ty));
                continue;
            };
            if let Err(terr) = self.relate_type_and_user_type(ty, V::Inv, user_ty, Locations::All(span),
                ConstraintCategory::TypeAnnotation(AnnotationSource::Declaration)) {
                self.mirbug(format!("bad user type on variable {local:?}: {:?} != {:?} ({terr:?})", decl.ty, user_ty.base));
            }
        }
        // With unsized_fn_params this is check_call_dest's; arguments and return
        // values (ADT constructors lack `Self: Sized`) are not required Sized here.
        if !self.infcx.unsized_fn_params() && matches!(self.local_kind(local), LocalKind::Temp | LocalKind::UserVar) {
            self.ensure_place_sized(decl.ty, decl.source_info.span);
        }
    }
    fn local_kind(&self, local: Local) -> LocalKind {
        if local.0 == 0 { LocalKind::Return } else if local.0 <= self.body.arg_count { LocalKind::Arg } else { LocalKind::Temp }
    }

    fn check_rvalue(&mut self, rvalue: &Rvalue, location: Location) {
        let tcx = self.tcx;
        let tys = &tcx.tys;
        let locations = to_locations(location);
        match rvalue {
            Rvalue::Aggregate(kind, ops) => self.check_aggregate_rvalue(kind, ops, location),
            Rvalue::Repeat(operand, len) => {
                let array_ty = self.rvalue_ty(rvalue);
                self.infcx.prove_wf(&mut self.constraints, array_ty, locations, ConstraintCategory::Boring);
                // A length that may exceed one copies the element.
                if try_to_target_usize(len).is_none_or(|len| len > 1) {
                    if let Operand::Move(place) = operand {
                        let ty = self.place_ty(place).ty;
                        self.infcx.prove_trait_ref(&mut self.constraints, LangItem::Copy, &[GenericArg::Type(ty)], locations, ConstraintCategory::CopyBound);
                    }
                }
            }
            Rvalue::Cast(kind, op, ty) => self.check_cast(*kind, op, *ty, location),
            Rvalue::Ref(region, _, borrowed_place) => self.add_reborrow_constraint(location, *region, borrowed_place),
            Rvalue::BinaryOp(BinOp::Eq | BinOp::Ne | BinOp::Lt | BinOp::Le | BinOp::Gt | BinOp::Ge, ops) => {
                let ty_left = self.operand_ty(&ops.0);
                let ty_right = self.operand_ty(&ops.1);
                match tys.kind(ty_left) {
                    // Types with regions are comparable if they have a common supertype.
                    TyKind::Ptr(..) | TyKind::FnPtr(..) => {
                        let common_ty = self.infcx.next_ty_var(source_info(self.body, location).span);
                        if let Err(err) = self.sub_types(ty_left, common_ty, locations, ConstraintCategory::CallArgument(None)) {
                            self.mirbug(format!("Could not equate type variable with {ty_left:?}: {err:?}"));
                        }
                        if let Err(terr) = self.sub_types(ty_right, common_ty, locations, ConstraintCategory::CallArgument(None)) {
                            self.mirbug(format!("unexpected comparison types {ty_left:?} and {ty_right:?} yields {terr:?}"));
                        }
                    }
                    TyKind::Int(_) | TyKind::Bool | TyKind::Char | TyKind::Float(_) if ty_left == ty_right => {}
                    // Other types are compared by trait methods.
                    _ => self.mirbug(format!("unexpected comparison types {ty_left:?} and {ty_right:?}")),
                }
            }
            Rvalue::Use(_) | Rvalue::UnaryOp(..) | Rvalue::CopyForDeref(_) | Rvalue::BinaryOp(..) | Rvalue::RawPtr(..)
            | Rvalue::ThreadLocalRef(..) | Rvalue::Discriminant(..) | Rvalue::Len(_) | Rvalue::NullaryOp(..)
            | Rvalue::ShallowInitBox(..) => {}
        }
    }

    fn check_operand(&mut self, op: &Operand, location: Location) {
        let Operand::Constant(constant) = op else { return };
        // mod.rs:1707: super_operand visits the constant (visit_const_operand) before the
        // inline-const bounds are proven.
        self.check_const_operand(constant, location);
        if let Const::Unevaluated { def, args, promoted: None } = &constant.const_ {
            // Inline consts are closures of their parent: prove their bounds here.
            if self.infcx.is_inline_const(*def) {
                let args = self.prove_closure_bounds(ClauseOwner::Def(*def), args, location);
                self.infcx.normalize_and_prove_instantiated_clauses(&mut self.constraints, ClauseOwner::Def(*def), &args, to_locations(location));
            }
        }
    }

    fn check_const_operand(&mut self, constant: &ConstOperand, location: Location) {
        let ty = self.const_ty(constant);
        let point = self.location_map.point(location);
        let mut live = vec![];
        for_each_free_region(self.tcx, ty, &mut |r| live.push(r));
        for r in live {
            let vid = self.infcx.to_region_vid(r);
            self.constraints.liveness_constraints.add_point(vid, point);
        }
        let locations = to_locations(location);
        if let Some(index) = constant.user_ty {
            if let Err(terr) = self.relate_type_and_user_type(ty, V::Inv, &UserTypeProjection { base: index, projection: vec![] },
                locations, ConstraintCategory::TypeAnnotation(AnnotationSource::GenericArg)) {
                self.mirbug(format!("bad constant user type {index:?} vs {ty:?}: {terr:?}"));
            }
        } else {
            match &constant.const_ {
                Const::Unevaluated { promoted: Some(promoted), .. } => {
                    let promoted_bodies: &'a [Body] = self.promoted;
                    let promoted_body = &promoted_bodies[promoted.0 as usize];
                    self.check_promoted(promoted_body, location);
                    let promoted_ty = promoted_body.local_decls[0].ty;
                    if let Err(terr) = self.eq_types(ty, promoted_ty, locations, ConstraintCategory::Boring) {
                        self.mirbug(format!("bad promoted type ({ty:?}: {promoted_ty:?}): {terr:?}"));
                    }
                }
                Const::Unevaluated { def, args, promoted: None } => {
                    let span = locations.span(self.body);
                    self.infcx.ascribe_user_type(&mut self.constraints, ty, UserType::TypeOf(*def, args.clone()), span);
                }
                Const::Val(..) => {
                    if let Some(static_def) = self.infcx.static_ptr(constant) {
                        let normalized_ty = self.infcx.static_ty(&mut self.constraints, static_def, locations);
                        let literal_ty = match *self.tcx.tys.kind(ty) {
                            TyKind::Ref(_, t, _) | TyKind::Ptr(_, t) => t,
                            _ => return self.mirbug(format!("static pointer constant of type {ty:?}")),
                        };
                        if let Err(terr) = self.eq_types(literal_ty, normalized_ty, locations, ConstraintCategory::Boring) {
                            self.mirbug(format!("bad static type {constant:?} ({terr:?})"));
                        }
                    }
                }
            }
            // Const generic parameters (`Const::Ty(Param)`) are ascribed TypeOf(param) by
            // nightly; Relax MIR has no const-parameter operand form (they are TyIds).
            if let TyKind::FnDef(def, args, regions) = self.tcx.tys.kind(ty).clone() {
                self.infcx.normalize_and_prove_instantiated_clauses(&mut self.constraints, ClauseOwner::Def(def), &self.tcx.complete_args(self.prog, def, self.tcx.tys.list(args), regions), locations);
                for t in self.tcx.tys.list(args).iter().copied() {
                    self.infcx.prove_wf(&mut self.constraints, t, locations, ConstraintCategory::Boring);
                }
            }
        }
    }

    fn check_place(&mut self, place: &Place, context: PlaceContext, location: Location) {
        let projected_context = if !place.projection.is_empty() && context.is_use() {
            if context.is_mutating_use() { PlaceContext::MutatingUse(MutatingUseContext::Projection) }
            else { PlaceContext::NonMutatingUse(NonMutatingUseContext::Projection) }
        } else { context };
        // super_projection: innermost base last.
        for i in (0..place.projection.len()).rev() {
            let base = Place { local: place.local, projection: place.projection[..i].to_vec() };
            self.check_projection_elem(&base, &place.projection[i], projected_context, location);
        }
        if context == PlaceContext::NonMutatingUse(NonMutatingUseContext::Copy) {
            // `T: Copy` is proven, not tested modulo regions: its impl's bounds apply (#29149).
            let ty = self.place_ty(place).ty;
            self.infcx.prove_trait_ref(&mut self.constraints, LangItem::Copy, &[GenericArg::Type(ty)], to_locations(location), ConstraintCategory::CopyBound);
        }
    }

    fn check_projection_elem(&mut self, base: &Place, elem: &PlaceElem, context: PlaceContext, location: Location) {
        let base_ty = self.place_ty(base);
        let locations = to_locations(location);
        match elem {
            // These add no constraints; the MIR validator checks their invariants.
            PlaceElem::Deref | PlaceElem::Index(_) | PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. }
            | PlaceElem::Downcast(..) => {}
            PlaceElem::Field(field, fty) => {
                let fty = self.infcx.normalize(&mut self.constraints, *fty, locations);
                let Some(ty) = self.infcx.field_ty(base_ty, *field) else {
                    return self.mirbug(format!("bad field access {field:?} of {:?}", base_ty.ty));
                };
                let ty = self.infcx.normalize(&mut self.constraints, ty, locations);
                if let Err(terr) = self.relate_types(ty, ambient_variance(context), fty, locations, ConstraintCategory::Boring) {
                    self.mirbug(format!("bad field access ({ty:?}: {fty:?}): {terr:?}"));
                }
            }
            PlaceElem::OpaqueCast(ty) => {
                let ty = self.infcx.normalize(&mut self.constraints, *ty, locations);
                if let Err(terr) = self.relate_types(ty, ambient_variance(context), base_ty.ty, locations,
                    ConstraintCategory::TypeAnnotation(AnnotationSource::OpaqueCast)) {
                    self.mirbug(format!("bad opaque cast ({ty:?}: {:?}): {terr:?}", base_ty.ty));
                }
            }
            PlaceElem::Subtype(_) => self.mirbug("Subtype projections are runtime MIR".into()),
        }
    }
}

impl<'a> Visitor for TypeChecker<'a> {
    fn visit_span(&mut self, span: &Span) {
        if *span != Span::default() { self.last_span = *span; }
    }
    fn visit_body(&mut self, body: &Body) {
        assert!(std::ptr::eq(self.body, body));
        let body: &'a Body = self.body;
        self.visit_body_checked(body);
    }
    fn visit_rvalue(&mut self, rv: &Rvalue, at: Location) {
        self.super_rvalue(rv, at);
        self.check_rvalue(rv, at);
    }
    fn visit_operand(&mut self, op: &Operand, at: Location) {
        self.super_operand(op, at);
        self.check_operand(op, at);
    }
    fn visit_place(&mut self, place: &Place, context: PlaceContext, at: Location) {
        self.super_place(place, context, at);
        self.check_place(place, context, at);
    }
}

/// PlaceContext::ambient_variance (rustc_middle mir/visit.rs:1461).
fn ambient_variance(context: PlaceContext) -> V {
    match context {
        PlaceContext::MutatingUse(_) => V::Inv,
        PlaceContext::NonUse(NonUseContext::StorageDead | NonUseContext::StorageLive | NonUseContext::VarDebugInfo) => V::Inv,
        PlaceContext::NonMutatingUse(_) => V::Co,
        PlaceContext::NonUse(NonUseContext::AscribeUserTy(variance)) => mir_variance(variance),
    }
}
fn mir_variance(v: Variance) -> V {
    match v { Variance::Covariant => V::Co, Variance::Contravariant => V::Contra, Variance::Invariant => V::Inv, Variance::Bivariant => V::Bi }
}
fn as_local(place: &Place) -> Option<Local> { place.projection.is_empty().then_some(place.local) }
/// CallSource::from_hir_call.
fn from_hir_call(source: CallSource) -> bool { matches!(source, CallSource::Normal) }
fn try_to_target_usize(c: &Const) -> Option<u64> {
    match c {
        Const::Val(ConstValue::Scalar(Scalar::Int { bits, .. }), _) => u64::try_from(*bits).ok(),
        Const::Val(..) | Const::Unevaluated { .. } => None,
    }
}
fn mk_fn_ptr(tcx: &Tcx, sig: &FnSig, bound_regions: u32) -> TyId {
    tcx.tys.intern(TyKind::FnPtr(tcx.tys.mk_list(&sig.inputs), sig.output, sig.header, bound_regions))
}
fn without_auto_traits(tcx: &Tcx, t: TyId) -> TyId {
    match tcx.tys.kind(t).clone() {
        TyKind::Dyn(d, args, bindings, rs, r, n, _) => tcx.tys.intern(TyKind::Dyn(d, args, bindings, rs, r, n, crate::types::List::EMPTY)),
        _ => t,
    }
}
/// `fold_regions(ty, |r| if ReVar { re_erased } else { r })`.
fn erase_region_vars(tcx: &Tcx, t: TyId) -> TyId {
    let tys = &tcx.tys;
    tys.map_regions(t, &|r, _| if matches!(tys.region_kind(r), RegionKind::ReVar(_)) { RegionId::ERASED } else { r }, 0)
}
/// `tcx.for_each_free_region`: regions not bound inside `t`.
fn for_each_free_region(tcx: &Tcx, t: TyId, f: &mut dyn FnMut(RegionId)) {
    let tys = &tcx.tys;
    let found = std::cell::RefCell::new(vec![]);
    tys.map_regions(t, &|r, _| {
        if !matches!(tys.region_kind(r), RegionKind::ReBound(..)) { found.borrow_mut().push(r); }
        r
    }, 0);
    for r in found.into_inner() { f(r); }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum IntLike { Int, Other }
/// rustc_middle ty/cast.rs CastTy (Int covers bool/char/C-like enums).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum CastTy { Int(IntLike), Float, FnPtr, Ptr(TyId) }
