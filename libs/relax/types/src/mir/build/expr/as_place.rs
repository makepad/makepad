//! rustc_mir_build/src/builder/expr/as_place.rs. Owner: lane B. (`PlaceBuilder` lives in `place.rs`.)
//! Deviations: no unsafe-binder projections (unstable).
use crate::mir::build::builder::{Builder, ForGuard, TempLifetime};
use crate::mir::build::cfg::{BlockAnd, BlockAndExtension};
use crate::mir::build::expr::category::Category;
use crate::mir::build::place::{PlaceBase, PlaceBuilder};
use crate::mir::build::thir::{ExprId, ExprKind};
use crate::mir::build::BuildResult;
use crate::mir::{
    AssertKind, BasicBlock, BinOp, BorrowKind, CanonicalUserTypeAnnotation, FakeBorrowKind, FakeReadCause, Local, Mutability, Operand,
    Place, PlaceElem, Rvalue, SourceInfo, Span, Statement, StatementKind, UserTypeProjection, Variance, VariantIdx,
};
use crate::types::TyKind;

impl Builder<'_> {
    /// The temporary lifetime of `expr` (rustc `region_scope_tree.temporary_scope(expr.temp_scope_id)`).
    pub fn expr_temp_lifetime(&self, expr: ExprId) -> TempLifetime {
        TempLifetime { temp_lifetime: self.thir.thir[expr].temp_scope, backwards_incompatible: self.thir.thir[expr].bid }
    }

    /// Compile `expr`, yielding a place that we can move from etc.
    ///
    /// WARNING: Any user code might:
    /// - Invalidate any slice bounds checks performed.
    /// - Change the address that this `Place` refers to.
    /// - Modify the memory that this place refers to.
    /// - Invalidate the memory that this place refers to, this will be caught by borrow checking.
    ///
    /// Extra care is needed if any user code is allowed to run between calling this method and using it, as is the
    /// case for `match` and index expressions.
    pub fn as_place(&mut self, mut block: BasicBlock, expr_id: ExprId) -> BuildResult<BlockAnd<Place>> {
        let place_builder = unpack!(block = self.as_place_builder(block, expr_id));
        Ok(block.and(place_builder.to_place(self)))
    }

    /// This is used when constructing a compound `Place`, so that we can avoid creating intermediate `Place` values
    /// until we know the full set of projections.
    pub fn as_place_builder(&mut self, block: BasicBlock, expr_id: ExprId) -> BuildResult<BlockAnd<PlaceBuilder>> {
        self.expr_as_place(block, expr_id, Mutability::Mut, None)
    }

    /// Compile `expr`, yielding a place that we can move from etc. Mutability note: The caller of this method
    /// promises only to read from the resulting place, but the place itself may or may not be mutable.
    pub fn as_read_only_place(&mut self, mut block: BasicBlock, expr_id: ExprId) -> BuildResult<BlockAnd<Place>> {
        let place_builder = unpack!(block = self.as_read_only_place_builder(block, expr_id));
        Ok(block.and(place_builder.to_place(self)))
    }

    /// This is used when constructing a compound `Place`, so that we can avoid creating intermediate `Place` values
    /// until we know the full set of projections. Mutability note: The caller of this method promises only to read
    /// from the resulting place, but the place itself may or may not be mutable.
    pub fn as_read_only_place_builder(&mut self, block: BasicBlock, expr_id: ExprId) -> BuildResult<BlockAnd<PlaceBuilder>> {
        self.expr_as_place(block, expr_id, Mutability::Not, None)
    }

    fn expr_as_place(
        &mut self,
        mut block: BasicBlock,
        expr_id: ExprId,
        mutability: Mutability,
        fake_borrow_temps: Option<&mut Vec<Local>>,
    ) -> BuildResult<BlockAnd<PlaceBuilder>> {
        let thir = self.thir;
        let expr = &thir.thir[expr_id];
        let expr_span = expr.span;
        let source_info = self.source_info(expr_span);
        match &expr.kind {
            ExprKind::Scope { region_scope, value, .. } => {
                let (region_scope, value) = (*region_scope, *value);
                self.in_scope((region_scope, source_info), |this| this.expr_as_place(block, value, mutability, fake_borrow_temps))
            }
            ExprKind::Field { lhs, variant_index, name } => {
                let lhs_expr = &thir.thir[*lhs];
                let mut place_builder = unpack!(block = self.expr_as_place(block, *lhs, mutability, fake_borrow_temps));
                if let TyKind::Adt(def, ..) = self.root.cx.tcx.tys.kind(lhs_expr.ty) {
                    if self.root.cx.tcx.adts[def].is_enum() {
                        place_builder = place_builder.downcast(Some(self.variant_name(*def, *variant_index)), *variant_index);
                    }
                }
                Ok(block.and(place_builder.field(*name, expr.ty)))
            }
            ExprKind::Deref { arg } => {
                let place_builder = unpack!(block = self.expr_as_place(block, *arg, mutability, fake_borrow_temps));
                Ok(block.and(place_builder.deref()))
            }
            ExprKind::Index { lhs, index } => self.lower_index_expression(block, *lhs, *index, mutability, fake_borrow_temps, expr_span, source_info),
            ExprKind::UpvarRef { var } => Ok(block.and(PlaceBuilder::from(PlaceBase::Upvar { var: *var }))),
            ExprKind::VarRef { id } => {
                let place_builder = if self.is_bound_var_in_guard(*id) {
                    let index = self.var_local_id(*id, ForGuard::RefWithinGuard);
                    PlaceBuilder::from(index).deref()
                } else {
                    let index = self.var_local_id(*id, ForGuard::OutsideGuard);
                    PlaceBuilder::from(index)
                };
                Ok(block.and(place_builder))
            }
            ExprKind::PlaceTypeAscription { source, user_ty, user_ty_span } => {
                let place_builder = unpack!(block = self.expr_as_place(block, *source, mutability, fake_borrow_temps));
                if let Some(user_ty) = user_ty {
                    let ty_source_info = self.source_info(*user_ty_span);
                    let annotation_index = self.push_annotation(CanonicalUserTypeAnnotation { span: *user_ty_span, user_ty: user_ty.clone(), inferred_ty: expr.ty });
                    let place = place_builder.to_place(self);
                    self.cfg.push(
                        block,
                        Statement {
                            source_info: ty_source_info,
                            kind: StatementKind::AscribeUserType(place, UserTypeProjection { base: annotation_index, projection: vec![] }, Variance::Invariant),
                        },
                    );
                }
                Ok(block.and(place_builder))
            }
            ExprKind::ValueTypeAscription { source, user_ty, user_ty_span } => {
                let temp_lifetime = self.expr_temp_lifetime(*source);
                let temp = unpack!(block = self.as_temp(block, temp_lifetime, *source, mutability));
                if let Some(user_ty) = user_ty {
                    let ty_source_info = self.source_info(*user_ty_span);
                    let annotation_index = self.push_annotation(CanonicalUserTypeAnnotation { span: *user_ty_span, user_ty: user_ty.clone(), inferred_ty: expr.ty });
                    self.cfg.push(
                        block,
                        Statement {
                            source_info: ty_source_info,
                            kind: StatementKind::AscribeUserType(Place::local(temp), UserTypeProjection { base: annotation_index, projection: vec![] }, Variance::Invariant),
                        },
                    );
                }
                Ok(block.and(PlaceBuilder::from(temp)))
            }
            ExprKind::Array { .. }
            | ExprKind::Tuple { .. }
            | ExprKind::Adt { .. }
            | ExprKind::Closure { .. }
            | ExprKind::Unary { .. }
            | ExprKind::Binary { .. }
            | ExprKind::LogicalOp { .. }
            | ExprKind::Cast { .. }
            | ExprKind::Use { .. }
            | ExprKind::NeverToAny { .. }
            | ExprKind::PointerCoercion { .. }
            | ExprKind::Repeat { .. }
            | ExprKind::Borrow { .. }
            | ExprKind::RawBorrow { .. }
            | ExprKind::Match { .. }
            | ExprKind::If { .. }
            | ExprKind::Loop { .. }
            | ExprKind::Block { .. }
            | ExprKind::Let { .. }
            | ExprKind::Assign { .. }
            | ExprKind::AssignOp { .. }
            | ExprKind::Break { .. }
            | ExprKind::Continue { .. }
            | ExprKind::Return { .. }
            | ExprKind::Literal { .. }
            | ExprKind::NamedConst { .. }
            | ExprKind::NonHirLiteral { .. }
            | ExprKind::ZstLiteral { .. }
            | ExprKind::ConstParam { .. }
            | ExprKind::ConstBlock { .. }
            | ExprKind::StaticRef { .. }
            | ExprKind::Yield { .. }
            | ExprKind::Unreachable
            | ExprKind::ThreadLocalRef(_)
            | ExprKind::Call { .. }
            | ExprKind::ByUse { .. } => {
                debug_assert!(!matches!(Category::of(&expr.kind), Some(Category::Place)));
                let temp_lifetime = self.expr_temp_lifetime(expr_id);
                let temp = unpack!(block = self.as_temp(block, temp_lifetime, expr_id, mutability));
                Ok(block.and(PlaceBuilder::from(temp)))
            }
        }
    }

    /// The name of a variant, for `Downcast` projections.
    fn variant_name(&self, def: crate::program::DefId, variant: VariantIdx) -> crate::program::Sym {
        self.root.cx.tcx.adts[&def].variants[variant.0 as usize].name
    }

    fn push_annotation(&mut self, annotation: CanonicalUserTypeAnnotation) -> crate::mir::UserTypeAnnotationIndex {
        self.canonical_user_type_annotations.push(annotation);
        crate::mir::UserTypeAnnotationIndex(self.canonical_user_type_annotations.len() as u32 - 1)
    }

    fn lower_index_expression(
        &mut self,
        mut block: BasicBlock,
        base: ExprId,
        index: ExprId,
        mutability: Mutability,
        fake_borrow_temps: Option<&mut Vec<Local>>,
        expr_span: Span,
        source_info: SourceInfo,
    ) -> BuildResult<BlockAnd<PlaceBuilder>> {
        let base_fake_borrow_temps = &mut Vec::new();
        let is_outermost_index = fake_borrow_temps.is_none();
        let fake_borrow_temps = fake_borrow_temps.unwrap_or(base_fake_borrow_temps);

        let base_place = unpack!(block = self.expr_as_place(block, base, mutability, Some(fake_borrow_temps)));

        // Making this a *fresh* temporary means we do not have to worry about the index changing later: Nothing
        // will ever change this temporary. The "retagging" transformation (for Stacked Borrows) relies on this.
        // THIR contract: a builtin `Index` has a `usize` index (everything else is an overloaded `Index::index`
        // call); a body that breaks it is unchecked, not a source of false MIR bugs.
        if self.thir.thir[index].ty != self.root.cx.tcx.tys.usize_ {
            return Err(self.error(expr_span, crate::mir::build::UnsupportedReason::UnresolvedFact("builtin index with a non-usize index")));
        }
        let index_lifetime = self.expr_temp_lifetime(index);
        let idx = unpack!(block = self.as_temp(block, index_lifetime, index, Mutability::Not));

        block = self.bounds_check(block, &base_place, idx, expr_span, source_info)?;

        if is_outermost_index {
            self.read_fake_borrows(block, fake_borrow_temps, source_info);
        } else {
            let base = base_place.to_place(self);
            self.add_fake_borrows_of_base(base, block, fake_borrow_temps, expr_span, source_info)?;
        }

        Ok(block.and(base_place.index(idx)))
    }

    fn bounds_check(&mut self, block: BasicBlock, slice: &PlaceBuilder, index: Local, expr_span: Span, source_info: SourceInfo) -> BuildResult<BasicBlock> {
        let slice = slice.to_place(self);

        // len = len(slice)
        let len = self.len_of_slice_or_array(block, slice, expr_span, source_info)?;

        // lt = idx < len
        let bool_ty = self.root.cx.tcx.tys.bool_;
        let lt = self.temp(bool_ty, expr_span);
        let len_copy = match &len {
            Operand::Move(place) => Operand::Copy(place.clone()),
            other => other.clone(),
        };
        self.cfg.push_assign(block, source_info, lt.clone(), Rvalue::BinaryOp(BinOp::Lt, Box::new((Operand::Copy(Place::local(index)), len_copy))));
        let msg = AssertKind::BoundsCheck { len, index: Operand::Copy(Place::local(index)) };

        // assert!(lt, "...")
        self.assert(block, Operand::Move(lt), true, msg, expr_span)
    }

    fn add_fake_borrows_of_base(&mut self, base_place: Place, block: BasicBlock, fake_borrow_temps: &mut Vec<Local>, expr_span: Span, source_info: SourceInfo) -> BuildResult<()> {
        let tcx = self.root.cx.tcx;
        let place_ty = self.place_ty(&base_place)?;
        if let TyKind::Slice(_) = tcx.tys.kind(place_ty.ty) {
            // We need to create fake borrows to ensure that the bounds check that we just did stays valid. Since we
            // can't assign to unsized values, we only need to ensure that none of the pointers up to the base place
            // change.
            for i in (0..base_place.projection.len()).rev() {
                let prefix = Place { local: base_place.local, projection: base_place.projection[..i].to_vec() };
                match &base_place.projection[i] {
                    PlaceElem::Deref => {
                        let fake_borrow_deref_ty = self.place_ty(&prefix)?.ty;
                        let fake_borrow_ty = tcx.tys.intern(TyKind::Ref(false, fake_borrow_deref_ty, crate::types::RegionId::ERASED));
                        let fake_borrow_temp = self.new_local(fake_borrow_ty, Mutability::Mut, crate::mir::LocalKind::Temp, crate::mir::LocalInfo::Boring, expr_span);
                        self.cfg.push_assign(
                            block,
                            source_info,
                            Place::local(fake_borrow_temp),
                            Rvalue::Ref(crate::mir::Region::ERASED, BorrowKind::Fake(FakeBorrowKind::Shallow), prefix),
                        );
                        fake_borrow_temps.push(fake_borrow_temp);
                    }
                    PlaceElem::Index(_) => {
                        let index_ty = self.place_ty(&prefix)?;
                        match tcx.tys.kind(index_ty.ty) {
                            // The previous index expression has already done the bounds check, so we only need to
                            // fake-borrow up to the first slice index.
                            TyKind::Slice(_) => break,
                            TyKind::Array(..) => (),
                            _ => return Err(self.error(expr_span, crate::mir::build::UnsupportedReason::Internal("unexpected index base"))),
                        }
                    }
                    PlaceElem::Field(..) | PlaceElem::Downcast(..) | PlaceElem::OpaqueCast(..) | PlaceElem::Subtype(..) | PlaceElem::ConstantIndex { .. } | PlaceElem::Subslice { .. } => (),
                }
            }
        }
        Ok(())
    }

    fn read_fake_borrows(&mut self, bb: BasicBlock, fake_borrow_temps: &mut Vec<Local>, source_info: SourceInfo) {
        // All indexes have been evaluated now, read all of the fake borrows so that they are live across those index expressions.
        for temp in fake_borrow_temps.iter() {
            self.cfg.push_fake_read(bb, source_info, FakeReadCause::ForIndex, Place::local(*temp));
        }
    }
}
