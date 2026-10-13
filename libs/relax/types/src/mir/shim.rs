//! Drop glue bodies: rustc_mir_transform/src/shim.rs:299,332,433,447.
//! Call shims and lowering are M6a. No consumer uses these bodies yet.
use super::*;
use super::transform::{DropError, DropProvider};
use super::transform::patch::MirPatch;
use super::transform::elaborate_drop::*;
use crate::types::TyKind;

struct DropShimElaborator;
impl DropElaborator for DropShimElaborator {
    fn drop_style(&self, _: usize, mode: DropFlagMode) -> DropStyle {
        match mode { DropFlagMode::Shallow => DropStyle::Static, DropFlagMode::Deep => DropStyle::Open }
    }
    fn drop_flag(&self, _: usize) -> Option<Local> { None }
    fn clear_drop_flag(&self, _: &mut MirPatch, _: Location, _: usize, _: DropFlagMode) {}
    fn subpath(&self, _: usize, elem: &PlaceElem) -> Option<usize> {
        if matches!(elem, PlaceElem::Downcast(..)) { Some(0) } else { None }
    }
}

/// `def` is the semantic drop-glue function identity supplied by instance_mir.
/// The argument is *mut T, so data and DST metadata remain one typed MIR value.
/// False needs_drop produces the empty glue instance; dyn retains a virtual Drop
/// terminator (rustc elaborate_drop.rs:1309, rustc_codegen_ssa/src/mir/block.rs:625).
/// This is the layout-free build_drop_shim stage, before make_shim's packed-drop
/// moves, ABI unwind adjustment and optimization pipeline (rustc shim.rs:232).
pub fn build_drop_shim(provider: &dyn DropProvider, def: DefId, ty: TyId) -> Result<Body, DropError> {
    let source = SourceInfo::default();
    let tcx = provider.tcx();
    if tcx.is_coroutine(ty) { return Err(DropError::Unsupported("coroutine drop")); }
    let ptr_ty = tcx.tys.intern(TyKind::Ptr(true, ty));
    let mut body = Body {
        phase: MirPhase::Runtime(RuntimePhase::Initial), source: MirSource { owner: def, kind: BodyKind::Shim(ShimKind::DropGlue(ty)) },
        basic_blocks: Vec::new(), local_decls: Vec::new(),
        source_scopes: vec![SourceScopeData { span: source.span, parent_scope: None }],
        user_type_annotations: Vec::new(), var_debug_info: Vec::new(), arg_count: 1,
        spread_arg: None, promoted: Vec::new(), coroutine: None,
    };
    for (ty, kind) in [(tcx.tys.unit, LocalKind::Return), (ptr_ty, LocalKind::Arg)] {
        body.local_decls.push(LocalDecl { ty, kind, source_info: source, mutability: Mutability::Mut,
            user_ty: Vec::new(), local_info: LocalInfo::Boring });
    }
    body.basic_blocks.push(BasicBlockData { statements: Vec::new(), is_cleanup: false,
        terminator: Some(Terminator { source_info: source, kind: TerminatorKind::Return }) });
    if !provider.needs_drop(ty)? { return Ok(body); }
    body.basic_blocks[0].terminator.as_mut().unwrap().kind = TerminatorKind::Goto { target: BasicBlock(1) };
    body.basic_blocks.push(BasicBlockData { statements: Vec::new(), is_cleanup: false,
        terminator: Some(Terminator { source_info: source, kind: TerminatorKind::Return }) });
    let mut patch = MirPatch::new(&body);
    if let TyKind::Array(elem, _) = tcx.tys.kind(ty) {
        let slice = tcx.tys.intern(TyKind::Slice(*elem));
        let slice_ref = tcx.tys.intern(TyKind::Ref(true, slice, crate::types::RegionId::ERASED));
        let erased = Place::local(patch.temp(slice_ref, source));
        patch.statement(Location::default(), StatementKind::Assign(erased.clone(), Rvalue::Cast(
            CastKind::PointerCoercion(PointerCoercion::Unsize, CoercionSource::Implicit), Operand::Move(Place::local(Local(1))), slice_ref)));
        patch.terminator(BasicBlock(0), TerminatorKind::Call { func: function(provider, def, vec![slice], source.span),
            args: vec![Operand::Move(erased)], destination: Place::local(Local(0)), target: Some(BasicBlock(1)),
            unwind: UnwindAction::Continue, call_source: CallSource::Misc, fn_span: source.span, arg_spans: vec![source.span] });
    } else {
        let resume = patch.exit(UnwindAction::Continue, source);
        let site = DropSite { place: project(&Place::local(Local(1)), PlaceElem::Deref), path: 0,
            succ: BasicBlock(1), unwind: Unwind::To(resume), source };
        DropCtxt { body: &body, provider, elaborator: &DropShimElaborator, patch: &mut patch }.elaborate(&site, BasicBlock(0))?;
    }
    patch.apply(&mut body);
    Ok(body)
}
