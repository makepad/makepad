//! rustc_mir_build/src/builder/misc.rs (and the `len_of_slice_or_array` helper of expr/as_place.rs).
use crate::mir::build::{Builder, BuildResult, UnsupportedReason};
use crate::mir::{
    BasicBlock, Const, ConstOperand, ConstValue, FakeReadCause, Operand, Place, PlaceElem, PlaceTy, RawPtrKind, Rvalue, Scalar,
    SourceInfo, Span, UnOp,
};
use crate::program::Prim;
use crate::types::{RegionId, TyId, TyKind, Types};

/// rustc `Ty::is_trivially_pure_clone_copy`.
pub(in crate::mir::build) fn is_trivially_pure_clone_copy(tys: &Types, ty: TyId) -> bool {
    match *tys.kind(ty) {
        TyKind::Bool | TyKind::Char | TyKind::Never | TyKind::IntVar(_) | TyKind::FloatVar(_) | TyKind::Int(_) | TyKind::Float(_) | TyKind::FnDef(..) => true,
        TyKind::Array(e, _) => is_trivially_pure_clone_copy(tys, e),
        TyKind::Tuple(l) => {
            let fields = tys.list(l);
            fields.len() <= 3 && fields.iter().all(|&f| is_trivially_pure_clone_copy(tys, f))
        }
        TyKind::Ref(false, ..) | TyKind::Ptr(..) => true,
        _ => false,
    }
}

impl Builder<'_> {
    pub(in crate::mir::build) fn literal_operand(&self, span: Span, const_: Const) -> Operand {
        Operand::Constant(Box::new(ConstOperand { span, user_ty: None, const_ }))
    }

    /// An integer-like scalar constant (`Const::from_bits`).
    pub(in crate::mir::build) fn scalar_const(&self, bits: u128, size: u8, ty: TyId) -> Const {
        Const::Val(ConstValue::Scalar(Scalar::Int { bits, size }), ty)
    }

    /// misc.rs:`push_usize`.
    pub(in crate::mir::build) fn push_usize(&mut self, block: BasicBlock, source_info: SourceInfo, value: u64) -> Place {
        let usize_ty = self.root.cx.tcx.tys.prim(Prim::Usize);
        let temp = self.temp(usize_ty, source_info.span);
        let const_ = self.scalar_const(value as u128, 8, usize_ty);
        let operand = self.literal_operand(source_info.span, const_);
        self.cfg.push_assign(block, source_info, temp.clone(), Rvalue::Use(operand));
        temp
    }

    /// The type of `place`: its local's type under the projection (fields carry their types).
    pub(crate) fn place_ty(&self, place: &Place) -> BuildResult<PlaceTy> {
        let tcx = self.root.cx.tcx;
        let mut ty = PlaceTy::from_ty(self.local_decls[place.local.0 as usize].ty);
        for elem in &place.projection {
            ty = match ty.projection_ty(tcx, elem) {
                Some(ty) => ty,
                None => return Err(self.error(Span::default(), UnsupportedReason::UnresolvedFact("ill-typed place projection"))),
            };
        }
        Ok(ty)
    }

    /// misc.rs:`consume_by_copy_or_move`.
    pub(in crate::mir::build) fn consume_by_copy_or_move(&self, place: Place) -> BuildResult<Operand> {
        let ty = self.place_ty(&place)?.ty;
        let cx = &self.root.cx;
        if cx.tcx.type_is_copy_modulo_regions(cx.prog, cx.eval, self.owner.owner, ty) {
            Ok(Operand::Copy(place))
        } else {
            Ok(Operand::Move(place))
        }
    }

    /// as_place.rs:`len_of_slice_or_array`.
    pub(crate) fn len_of_slice_or_array(&mut self, block: BasicBlock, place: Place, span: Span, source_info: SourceInfo) -> BuildResult<Operand> {
        let tcx = self.root.cx.tcx;
        let place_ty = self.place_ty(&place)?.ty;
        match *tcx.tys.kind(place_ty) {
            TyKind::Array(_, len) => {
                // The length is a constant, but one statement keeps the array "used" for borrowck.
                self.cfg.push_fake_read(block, source_info, FakeReadCause::ForIndex, place);
                let const_ = Const::Ty(tcx.tys.prim(Prim::Usize), len);
                Ok(Operand::Constant(Box::new(ConstOperand { span, user_ty: None, const_ })))
            }
            TyKind::Slice(_) => {
                let direct = matches!(place.projection[..], [PlaceElem::Deref]) && is_trivially_pure_clone_copy(&tcx.tys, self.local_decls[place.local.0 as usize].ty);
                let ptr_or_ref = if direct {
                    // directly passed to `PtrMetadata`: only for `Copy` types, `&mut [_]` must pass NLL
                    Operand::Copy(Place::local(place.local))
                } else {
                    let ptr_ty = tcx.tys.intern(TyKind::Ptr(false, place_ty));
                    let slice_ptr = self.temp(ptr_ty, span);
                    self.cfg.push_assign(block, source_info, slice_ptr.clone(), Rvalue::RawPtr(RawPtrKind::FakeForPtrMetadata, place));
                    Operand::Move(slice_ptr)
                };
                let len = self.temp(tcx.tys.prim(Prim::Usize), span);
                self.cfg.push_assign(block, source_info, len.clone(), Rvalue::UnaryOp(UnOp::PtrMetadata, ptr_or_ref));
                Ok(Operand::Move(len))
            }
            _ => Err(self.error(span, UnsupportedReason::UnresolvedFact("len of a non-sequence place"))),
        }
    }

    /// `Pin<T>`'s `T` (pinned bindings and `&pin` patterns are unstable: never produced).
    pub(in crate::mir::build) fn pinned_ty(&self, _ty: TyId) -> Option<TyId> {
        None
    }

    /// A reference type `&ty` / `&mut ty` with an erased region.
    pub(in crate::mir::build) fn ref_ty(&self, mutable: bool, ty: TyId) -> TyId {
        self.root.cx.tcx.tys.intern(TyKind::Ref(mutable, ty, RegionId::ERASED))
    }
}
