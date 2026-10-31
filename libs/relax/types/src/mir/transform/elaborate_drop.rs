//! Shared synchronous DropElaborator: rustc_mir_transform/src/elaborate_drop.rs:
//! 19,91,486,571,669,718,737,775,834,853,973,1032,1123,1231,1284.
use super::{DropError, DropProvider};
use super::patch::MirPatch;
use crate::mir::*;
use crate::types::TyKind;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DropStyle { Dead, Static, Conditional, Open }
#[derive(Clone, Copy)]
pub(crate) enum DropFlagMode { Shallow, Deep }
#[derive(Clone, Copy)]
pub(crate) enum Unwind { To(BasicBlock), InCleanup }
impl Unwind {
    pub fn cleanup(self) -> bool { matches!(self, Self::InCleanup) }
    pub fn action(self) -> UnwindAction {
        match self { Self::To(bb) => UnwindAction::Cleanup(bb), Self::InCleanup => UnwindAction::Terminate(UnwindTerminateReason::InCleanup) }
    }
}

/// Concrete arena indices replace rustc's associated Path type. Shim paths use 0.
/// The policy owns initialization facts; the same engine owns all generated MIR.
pub(crate) trait DropElaborator {
    fn drop_style(&self, path: usize, mode: DropFlagMode) -> DropStyle;
    fn drop_flag(&self, path: usize) -> Option<Local>;
    fn clear_drop_flag(&self, patch: &mut MirPatch, loc: Location, path: usize, mode: DropFlagMode);
    fn subpath(&self, path: usize, projection: &PlaceElem) -> Option<usize>;
}
#[derive(Clone)]
pub(crate) struct DropSite {
    pub place: Place,
    pub path: usize,
    pub succ: BasicBlock,
    pub unwind: Unwind,
    pub source: SourceInfo,
}
pub(crate) struct DropCtxt<'a> {
    pub body: &'a Body,
    pub provider: &'a dyn DropProvider,
    pub elaborator: &'a dyn DropElaborator,
    pub patch: &'a mut MirPatch,
}
pub(crate) fn project(place: &Place, elem: PlaceElem) -> Place {
    let mut place = place.clone(); place.projection.push(elem); place
}
pub(crate) fn integer(ty: TyId, bits: u128, size: u8, span: Span) -> Operand {
    Operand::Constant(Box::new(ConstOperand { span, user_ty: None,
        const_: Const::Val(ConstValue::Scalar(Scalar::Int { bits, size }), ty) }))
}
pub(crate) fn function(provider: &dyn DropProvider, def: DefId, args: Vec<TyId>, span: Span) -> Operand {
    Operand::Constant(Box::new(ConstOperand { span, user_ty: None,
        const_: Const::Val(ConstValue::ZeroSized, provider.tcx().tys.intern(TyKind::FnDef(def, provider.tcx().tys.mk_list(&args), crate::types::RegionArgsId::EMPTY))) }))
}
impl DropCtxt<'_> {
    fn ty(&self, place: &Place) -> Result<TyId, DropError> {
        let mut ty = PlaceTy::from_ty(self.patch.local_ty(self.body, place.local));
        for elem in &place.projection {
            ty = ty.projection_ty(self.provider.tcx(), elem).ok_or(DropError::InvalidMir("invalid drop place"))?;
        }
        Ok(ty.ty)
    }
    fn block(&mut self, site: &DropSite, unwind: Unwind, statements: Vec<Statement>, term: TerminatorKind) -> BasicBlock {
        self.patch.block(site.source, unwind.cleanup(), statements, term)
    }
    fn assign(&self, site: &DropSite, place: Place, value: Rvalue) -> Statement {
        Statement { source_info: site.source, kind: StatementKind::Assign(place, value) }
    }
    fn temp(&mut self, site: &DropSite, ty: TyId) -> Place { Place::local(self.patch.temp(ty, site.source)) }
    fn goto(&mut self, site: &DropSite, target: BasicBlock, unwind: Unwind) -> BasicBlock {
        self.block(site, unwind, Vec::new(), TerminatorKind::Goto { target })
    }
    fn drop_block(&mut self, site: &DropSite, target: BasicBlock, unwind: Unwind) -> BasicBlock {
        self.block(site, unwind, Vec::new(), TerminatorKind::Drop {
            place: site.place.clone(), target, unwind: unwind.action(), replace: false })
    }
    fn flag_test(&mut self, site: &DropSite, on_set: BasicBlock, on_unset: BasicBlock, unwind: Unwind) -> BasicBlock {
        match self.elaborator.drop_style(site.path, DropFlagMode::Shallow) {
            DropStyle::Dead => on_unset,
            DropStyle::Static => on_set,
            DropStyle::Conditional | DropStyle::Open => {
                let flag = self.elaborator.drop_flag(site.path).expect("conditional drop without flag");
                self.block(site, unwind, Vec::new(), TerminatorKind::SwitchInt {
                    discr: Operand::Copy(Place::local(flag)),
                    targets: SwitchTargets { values: vec![0], targets: vec![on_unset], otherwise: on_set } })
            }
        }
    }
    fn reset(&mut self, site: &DropSite, mode: DropFlagMode, succ: BasicBlock, unwind: Unwind) -> BasicBlock {
        if unwind.cleanup() { return succ; }
        let bb = self.goto(site, succ, unwind);
        self.elaborator.clear_drop_flag(self.patch, Location { block: bb, statement_index: 0 }, site.path, mode);
        bb
    }
    fn complete(&mut self, site: &DropSite) -> BasicBlock {
        let drop = self.drop_block(site, site.succ, site.unwind);
        self.flag_test(site, drop, site.succ, site.unwind)
    }
    pub fn elaborate(&mut self, site: &DropSite, bb: BasicBlock) -> Result<(), DropError> {
        let kind = match self.elaborator.drop_style(site.path, DropFlagMode::Deep) {
            DropStyle::Dead => TerminatorKind::Goto { target: site.succ },
            DropStyle::Static => TerminatorKind::Drop { place: site.place.clone(), target: site.succ,
                unwind: site.unwind.action(), replace: false },
            DropStyle::Conditional => TerminatorKind::Goto { target: self.complete(site) },
            DropStyle::Open => TerminatorKind::Goto { target: self.open(site)? },
        };
        self.patch.terminator(bb, kind);
        Ok(())
    }
    fn subdrop(&mut self, site: &DropSite, place: &Place, path: Option<usize>, succ: BasicBlock, unwind: Unwind) -> Result<BasicBlock, DropError> {
        let sub = DropSite { place: place.clone(), path: path.unwrap_or(site.path), succ, unwind, source: site.source };
        if path.is_some() {
            let bb = self.block(site, unwind, Vec::new(), TerminatorKind::Unreachable);
            self.elaborate(&sub, bb)?;
            Ok(bb)
        } else { Ok(self.complete(&sub)) }
    }
    fn half_ladder(&mut self, site: &DropSite, fields: &[(Place, Option<usize>)], mut succ: BasicBlock, unwinds: &[Unwind]) -> Result<Vec<BasicBlock>, DropError> {
        let mut blocks = vec![succ];
        for i in 0..fields.len() {
            let (place, path) = &fields[fields.len() - i - 1];
            succ = self.subdrop(site, place, *path, succ, unwinds[i])?;
            blocks.push(succ);
        }
        Ok(blocks)
    }
    fn ladder(&mut self, site: &DropSite, fields: Vec<(Place, Option<usize>)>, succ: BasicBlock, unwind: Unwind) -> Result<(BasicBlock, Unwind), DropError> {
        let mut live = Vec::new();
        for field in fields { if self.provider.needs_drop(self.ty(&field.0)?)? { live.push(field); } }
        let mut unwinds = vec![Unwind::InCleanup; live.len() + 1];
        if let Unwind::To(bb) = unwind {
            let cleanup = self.half_ladder(site, &live, bb, &unwinds)?;
            for (i, bb) in cleanup.into_iter().enumerate() { unwinds[i] = Unwind::To(bb); }
        }
        let normal = self.half_ladder(site, &live, succ, &unwinds)?;
        Ok((*normal.last().unwrap(), *unwinds.last().unwrap()))
    }
    fn fields(&self, ty: TyId, variant: Option<VariantIdx>, base: &Place, path: usize, count: usize) -> Result<Vec<(Place, Option<usize>)>, DropError> {
        let mut fields = Vec::new();
        for i in 0..count {
            let field = FieldIdx(i as u32);
            let ty = self.provider.field_ty(ty, variant, field)?;
            let elem = PlaceElem::Field(field, ty);
            fields.push((project(base, elem.clone()), self.elaborator.subpath(path, &elem)));
        }
        Ok(fields)
    }
    fn tuple(&mut self, site: &DropSite, ty: TyId, count: usize) -> Result<BasicBlock, DropError> {
        let fields = self.fields(ty, None, &site.place, site.path, count)?;
        let succ = self.reset(site, DropFlagMode::Shallow, site.succ, site.unwind);
        Ok(self.ladder(site, fields, succ, site.unwind)?.0)
    }
    fn destructor(&mut self, site: &DropSite, succ: BasicBlock, unwind: Unwind) -> Result<BasicBlock, DropError> {
        let ty = self.ty(&site.place)?;
        let TyKind::Adt(def, _, _) = self.provider.tcx().tys.kind(ty) else { return Err(DropError::InvalidMir("destructor on non-ADT")); };
        self.provider.adt_destructor(*def).ok_or(DropError::InvalidMir("missing destructor"))?;
        // Call the trait method with Self, just as rustc does. Direct impl args
        // need not have the ADT parameter order; instance resolution is M6a.
        let drop_trait = self.provider.tcx().lang_items.get(crate::lang_items::LangItem::Drop)
            .ok_or(DropError::InvalidMir("missing Drop lang item"))?;
        let prog = self.provider.program();
        let method = *prog.traits[prog.def(drop_trait).sub as usize].first()
            .ok_or(DropError::InvalidMir("missing Drop method"))?;
        let func = function(self.provider, method, vec![ty], site.source.span);
        let ref_ty = self.provider.tcx().tys.intern(TyKind::Ref(true, ty, crate::types::RegionId::ERASED));
        let receiver = self.temp(site, ref_ty);
        let unit = self.temp(site, self.provider.tcx().tys.unit);
        let stmt = self.assign(site, receiver.clone(), Rvalue::Ref(Region::ERASED,
            BorrowKind::Mut { kind: MutBorrowKind::Default }, site.place.clone()));
        Ok(self.block(site, unwind, vec![stmt], TerminatorKind::Call { func, args: vec![Operand::Move(receiver)],
            destination: unit, target: Some(succ), unwind: unwind.action(), call_source: CallSource::Misc,
            fn_span: site.source.span, arg_spans: vec![site.source.span] }))
    }
    /// Relax's OwnedBox contract is field 0 = NonNull<T>, without rustc's Unique
    /// wrapper. This is a fixed library ABI, never inferred by walking field shapes.
    /// rustc elaborate_drop.rs:737. The transmute preserves unsized metadata.
    fn box_contents(&mut self, site: &DropSite, ty: TyId, inner: TyId, succ: BasicBlock, unwind: Unwind) -> Result<BasicBlock, DropError> {
        let nonnull = self.provider.field_ty(ty, None, FieldIdx(0))?;
        let storage = project(&site.place, PlaceElem::Field(FieldIdx(0), nonnull));
        let ptr_ty = self.provider.tcx().tys.intern(TyKind::Ptr(false, inner));
        let ptr = self.temp(site, ptr_ty);
        let interior = project(&ptr, PlaceElem::Deref);
        let path = self.elaborator.subpath(site.path, &PlaceElem::Deref);
        let drop = if self.provider.needs_drop(inner)? { self.subdrop(site, &interior, path, succ, unwind)? } else { succ };
        let stmt = self.assign(site, ptr, Rvalue::Cast(CastKind::Transmute, Operand::Copy(storage), ptr_ty));
        Ok(self.block(site, unwind, vec![stmt], TerminatorKind::Goto { target: drop }))
    }
    fn adt(&mut self, site: &DropSite, ty: TyId, def: DefId, args: &[TyId]) -> Result<BasicBlock, DropError> {
        let adt = &self.provider.tcx().adts[&def];
        if adt.variants.is_empty() { return Ok(self.block(site, site.unwind, Vec::new(), TerminatorKind::Unreachable)); }
        let (succ, unwind) = if adt.is_union() || adt.is_manually_drop() { (site.succ, site.unwind) } else {
            let succ = self.reset(site, DropFlagMode::Shallow, site.succ, site.unwind);
            if adt.is_enum() { self.variants(site, ty, def, succ)? } else {
                let fields = self.fields(ty, None, &site.place, site.path, adt.variants[0].fields.len())?;
                self.ladder(site, fields, succ, site.unwind)?
            }
        };
        if self.provider.adt_destructor(def).is_none() { return Ok(succ); }
        let drop = if self.provider.is_box(def) {
            let normal = self.destructor(site, succ, unwind)?;
            let cleanup = match unwind {
                Unwind::To(bb) => Unwind::To(self.destructor(site, bb, Unwind::InCleanup)?),
                Unwind::InCleanup => Unwind::InCleanup,
            };
            self.box_contents(site, ty, *args.first().ok_or(DropError::InvalidMir("Box without pointee"))?, normal, cleanup)?
        } else { self.destructor(site, succ, unwind)? };
        Ok(self.flag_test(site, drop, succ, unwind))
    }
    fn variants(&mut self, site: &DropSite, ty: TyId, def: DefId, succ: BasicBlock) -> Result<(BasicBlock, Unwind), DropError> {
        let adt = &self.provider.tcx().adts[&def];
        let mut values = Vec::new(); let mut normal = Vec::new(); let mut cleanup = Vec::new();
        let mut otherwise = false; let mut otherwise_drop = false;
        for (i, variant) in adt.variants.iter().enumerate() {
            let v = VariantIdx(i as u32);
            let elem = PlaceElem::Downcast(None, v);
            if let Some(path) = self.elaborator.subpath(site.path, &elem) {
                let base = project(&site.place, elem);
                let fields = self.fields(ty, Some(v), &base, path, variant.fields.len())?;
                values.push(adt.discriminant_for_variant(self.provider.tcx(), self.provider.program(), i as u32).ok_or(DropError::InvalidMir("invalid drop variant"))?.val);
                if let Unwind::To(bb) = site.unwind {
                    // Separate half-ladders preserve the MSVC funclet successor invariant.
                    let unwinds = vec![Unwind::InCleanup; fields.len() + 1];
                    let ladder = self.half_ladder(site, &fields, bb, &unwinds)?;
                    cleanup.push(*ladder.last().unwrap());
                }
                normal.push(self.ladder(site, fields, succ, site.unwind)?.0);
            } else {
                otherwise = true;
                for j in 0..variant.fields.len() {
                    let field = self.provider.field_ty(ty, Some(v), FieldIdx(j as u32))?;
                    otherwise_drop |= self.provider.needs_drop(field)?;
                }
            }
        }
        if !otherwise { values.pop(); } else {
            normal.push(if otherwise_drop { self.drop_block(site, succ, site.unwind) } else { self.goto(site, succ, site.unwind) });
            if let Unwind::To(bb) = site.unwind {
                cleanup.push(if otherwise_drop { self.drop_block(site, bb, Unwind::InCleanup) } else { self.goto(site, bb, Unwind::InCleanup) });
            }
        }
        let discr_ty = adt.repr_int.map(|p| self.provider.tcx().tys.intern(TyKind::Int(p))).unwrap_or(self.provider.tcx().tys.isize_);
        let entry = self.variant_switch(site, discr_ty, &values, normal, succ, site.unwind);
        let unwind = match site.unwind {
            Unwind::To(bb) => Unwind::To(self.variant_switch(site, discr_ty, &values, cleanup, bb, Unwind::InCleanup)),
            Unwind::InCleanup => Unwind::InCleanup,
        };
        Ok((entry, unwind))
    }
    fn variant_switch(&mut self, site: &DropSite, ty: TyId, values: &[u128], mut blocks: Vec<BasicBlock>, succ: BasicBlock, unwind: Unwind) -> BasicBlock {
        let discr = self.temp(site, ty);
        let stmt = self.assign(site, discr.clone(), Rvalue::Discriminant(site.place.clone()));
        let otherwise = blocks.pop().unwrap();
        let switch = self.block(site, unwind, vec![stmt], TerminatorKind::SwitchInt {
            discr: Operand::Move(discr), targets: SwitchTargets { values: values.to_vec(), targets: blocks, otherwise } });
        self.flag_test(site, switch, succ, unwind)
    }
    /// Save the element pointer and advance before Drop: cleanup starts at the
    /// next element even if this element's destructor unwinds (rustc:1032).
    fn drop_loop(&mut self, site: &DropSite, elem_ty: TyId, cur: Local, len: Local, succ: BasicBlock, unwind: Unwind) -> BasicBlock {
        let tcx = self.provider.tcx();
        let ptr_ty = tcx.tys.intern(TyKind::Ptr(true, elem_ty));
        let ptr = self.temp(site, ptr_ty);
        let done = self.temp(site, tcx.tys.bool_);
        let cur_place = Place::local(cur);
        let one = integer(tcx.tys.usize_, 1, 8, site.source.span);
        let statements = vec![
            self.assign(site, ptr.clone(), Rvalue::RawPtr(RawPtrKind::Mut, project(&site.place, PlaceElem::Index(cur)))),
            self.assign(site, cur_place.clone(), Rvalue::BinaryOp(BinOp::Add, Box::new((Operand::Move(cur_place.clone()), one)))),
        ];
        let drop = self.block(site, unwind, statements, TerminatorKind::Unreachable);
        let compare = self.assign(site, done.clone(), Rvalue::BinaryOp(BinOp::Eq,
            Box::new((Operand::Copy(cur_place), Operand::Copy(Place::local(len))))));
        let head = self.block(site, unwind, vec![compare], TerminatorKind::SwitchInt {
            discr: Operand::Move(done), targets: SwitchTargets { values: vec![0], targets: vec![drop], otherwise: succ } });
        self.patch.terminator(drop, TerminatorKind::Drop { place: project(&ptr, PlaceElem::Deref), target: head,
            unwind: unwind.action(), replace: false });
        head
    }
    fn slice(&mut self, site: &DropSite, elem: TyId) -> Result<BasicBlock, DropError> {
        if site.place.projection != [PlaceElem::Deref] { return Err(DropError::InvalidMir("slice shim dropee must be a pointer dereference")); }
        let usize_ty = self.provider.tcx().tys.usize_;
        let len = self.patch.temp(usize_ty, site.source);
        let cur = self.patch.temp(usize_ty, site.source);
        let unwind = match site.unwind {
            Unwind::To(bb) => Unwind::To(self.drop_loop(site, elem, cur, len, bb, Unwind::InCleanup)),
            Unwind::InCleanup => Unwind::InCleanup,
        };
        let head = self.drop_loop(site, elem, cur, len, site.succ, unwind);
        let statements = vec![
            self.assign(site, Place::local(len), Rvalue::UnaryOp(UnOp::PtrMetadata, Operand::Copy(Place::local(site.place.local)))),
            self.assign(site, Place::local(cur), Rvalue::Use(integer(usize_ty, 0, 8, site.source.span))),
        ];
        let entry = self.block(site, unwind, statements, TerminatorKind::Goto { target: head });
        let reset = self.reset(site, DropFlagMode::Deep, entry, unwind);
        Ok(self.flag_test(site, reset, site.succ, unwind))
    }
    fn array(&mut self, site: &DropSite, ty: TyId, elem: TyId, len: crate::consts::Const) -> Result<BasicBlock, DropError> {
        let size = len.try_to_target_usize(&self.provider.tcx().tys);
        if let Some(size) = size {
            let mut fields = Vec::new(); let mut start = 0; let mut found = false;
            for offset in 0..size {
                let projection = PlaceElem::ConstantIndex { offset, min_length: size, from_end: false };
                if let Some(path) = self.elaborator.subpath(site.path, &projection) {
                    if start < offset { fields.push((project(&site.place, PlaceElem::Subslice { from: start, to: offset, from_end: false }), None)); }
                    fields.push((project(&site.place, projection), Some(path)));
                    start = offset + 1; found = true;
                }
            }
            if found {
                if start < size { fields.push((project(&site.place, PlaceElem::Subslice { from: start, to: size, from_end: false }), None)); }
                // rustc:1167 reverses these partitions before building the ladder.
                fields.reverse();
                let succ = self.reset(site, DropFlagMode::Shallow, site.succ, site.unwind);
                return Ok(self.ladder(site, fields, succ, site.unwind)?.0);
            }
        }
        let array_ptr_ty = self.provider.tcx().tys.intern(TyKind::Ptr(true, ty));
        let slice_ty = self.provider.tcx().tys.intern(TyKind::Slice(elem));
        let slice_ptr_ty = self.provider.tcx().tys.intern(TyKind::Ptr(true, slice_ty));
        let array_ptr = self.temp(site, array_ptr_ty); let slice_ptr = self.temp(site, slice_ptr_ty);
        let sub = DropSite { place: project(&slice_ptr, PlaceElem::Deref), ..site.clone() };
        let head = self.slice(&sub, elem)?;
        let statements = vec![
            self.assign(site, array_ptr.clone(), Rvalue::RawPtr(RawPtrKind::Mut, site.place.clone())),
            self.assign(site, slice_ptr, Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::Unsize, CoercionSource::Implicit), Operand::Move(array_ptr), slice_ptr_ty)),
        ];
        Ok(self.block(site, site.unwind, statements, TerminatorKind::Goto { target: head }))
    }
    fn open(&mut self, site: &DropSite) -> Result<BasicBlock, DropError> {
        let ty = self.ty(&site.place)?;
        match self.provider.tcx().tys.kind(ty) {
            TyKind::Tuple(fields) => self.tuple(site, ty, self.provider.tcx().tys.list(*fields).len()),
            TyKind::Closure(file, expr, crate::types::ClosureArgs { tupled_upvars_ty: captures, .. }) => {
                if self.provider.tcx().coroutines.contains_key(&(*file, *expr)) { return Err(DropError::Unsupported("coroutine drop")); }
                let TyKind::Tuple(fields) = self.provider.tcx().tys.kind(*captures) else { return Err(DropError::InvalidMir("closure capture storage")); };
                self.tuple(site, ty, self.provider.tcx().tys.list(*fields).len())
            }
            TyKind::Adt(def, args, _) => self.adt(site, ty, *def, self.provider.tcx().tys.list(*args)),
            // Dyn Drop remains a virtual glue call; M6a resolves it via the vtable,
            // carrying the same wide pointer. rustc elaborate_drop.rs:1309;
            // rustc_codegen_ssa/src/mir/block.rs:594,625 (optional vtable slot).
            TyKind::Dyn(..) => Ok(self.complete(site)),
            TyKind::Array(elem, len) => self.array(site, ty, *elem, *len),
            TyKind::Slice(elem) => self.slice(site, *elem),
            _ => Err(DropError::InvalidMir("open drop of non-aggregate")),
        }
    }
}
