// ported from N:rustc_const_eval/src/interpret/{step.rs,call.rs,operator.rs,cast.rs,discriminant.rs,intrinsics.rs} @8d1a764
//! Statements, rvalues and terminators.
use super::*;
use crate::mir::{AggregateKind, AssertKind, BinOp, CastKind, PointerCoercion, UnOp};

impl Interp<'_> {
    pub(super) fn step(&mut self) -> R<()> {
        self.steps += 1;
        if self.steps > STEP_LIMIT { return unsupported("step limit"); }
        let (body, block, stmt) = { let f = self.frames.last().unwrap(); (f.body.clone(), f.block, f.stmt) };
        let bb = &body.basic_blocks[block.0 as usize];
        if stmt < bb.statements.len() {
            self.statement(&bb.statements[stmt].kind)?;
            self.frames.last_mut().unwrap().stmt += 1;
            return Ok(());
        }
        let Some(t) = &bb.terminator else { return unsupported("unterminated block") };
        self.terminator(&t.kind)
    }

    fn in_std_fn(&self) -> bool {
        let f = self.frames.last().unwrap();
        if !matches!(f.body.source.kind, mir::BodyKind::Fn) { return false; }
        let krate = self.prog.def(f.body.source.owner).krate;
        matches!(self.prog.syms.str(self.prog.crates[krate as usize].name), "core" | "alloc" | "std")
    }

    fn goto(&mut self, b: BasicBlock) { let f = self.frames.last_mut().unwrap(); f.block = b; f.stmt = 0; }

    fn statement(&mut self, s: &StatementKind) -> R<()> {
        match s {
            StatementKind::Assign(place, rv) => {
                let dest = self.eval_place(place)?;
                self.rvalue(dest, rv)
            }
            StatementKind::SetDiscriminant { place, variant_index } => { let p = self.eval_place(place)?; self.write_discriminant(p, variant_index.0) }
            StatementKind::StorageLive(l) => {
                let ty = self.frames.last().unwrap().body.local_decls[l.0 as usize].ty;
                let (size, align) = self.size_align(ty)?;
                if let Some(old) = self.frames.last().unwrap().locals[l.0 as usize] { self.allocs[old as usize].live = false; }
                let a = self.alloc(Alloc::new(size, align, Kind::Stack));
                self.frames.last_mut().unwrap().locals[l.0 as usize] = Some(a);
                Ok(())
            }
            StatementKind::StorageDead(l) => {
                if let Some(a) = self.frames.last_mut().unwrap().locals[l.0 as usize].take() { self.allocs[a as usize].live = false; }
                Ok(())
            }
            StatementKind::Intrinsic(i) => match i {
                mir::NonDivergingIntrinsic::Assume(op) => { let (v, _) = self.eval_operand(op)?; if !self.bool_of(&v)? { return ub("`assume` called with `false`"); } Ok(()) }
                mir::NonDivergingIntrinsic::CopyNonOverlapping { src, dst, count } => {
                    let (s, st) = self.eval_operand(src)?; let (d, _) = self.eval_operand(dst)?; let (n, _) = self.eval_operand(count)?;
                    let elem = match self.tcx.tys.kind(st) { TyKind::Ptr(_, t) => *t, _ => return unsupported("copy pointee") };
                    self.copy(self.ptr_of(&s, 0)?, self.ptr_of(&d, 0)?, elem, self.int_of(&n)? as u64, true)
                }
            },
            StatementKind::FakeRead(..) | StatementKind::PlaceMention(_) | StatementKind::AscribeUserType(..) | StatementKind::ConstEvalCounter
            | StatementKind::Nop | StatementKind::BackwardIncompatibleDropHint { .. } => Ok(()),
        }
    }

    /// intrinsics.rs copy / copy_nonoverlapping (memory.rs mem_copy: bytes, init mask and provenance)
    fn copy(&mut self, src: Ptr, dst: Ptr, elem: TyId, count: u64, nonoverlapping: bool) -> R<()> {
        let (size, align) = self.size_align(elem)?;
        let Some(len) = size.checked_mul(count) else { return ub("overflow computing total size of `copy`") };
        let v = self.read(src, len, align)?;
        if nonoverlapping {
            if let (Some(a), Some(b)) = (src.prov, dst.prov) {
                if a.alloc == b.alloc && src.off < dst.off + len && dst.off < src.off + len && len > 0 { return ub("`copy_nonoverlapping` called on overlapping ranges"); }
            }
        }
        self.write(dst, &v, align)
    }

    fn rvalue(&mut self, dest: MPlace, rv: &Rvalue) -> R<()> {
        let tys = &self.tcx.tys;
        let v = match rv {
            Rvalue::Use(op) | Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::MutToConstPointer | PointerCoercion::ArrayToPointer, _), op, _) => {
                let (v, ty) = self.eval_operand(op)?;
                if let Rvalue::Cast(CastKind::PointerCoercion(PointerCoercion::ArrayToPointer, _), ..) = rv { let _ = ty; }
                v
            }
            Rvalue::CopyForDeref(p) => { let p = self.eval_place(p)?; self.read_place(p)? }
            Rvalue::Repeat(op, n) => {
                let (v, _) = self.eval_operand(op)?;
                let n = match n { Const::Ty(_, c) => c.try_to_target_usize(tys), Const::Val(ConstValue::Scalar(mir::Scalar::Int { bits, .. }), _) => Some(*bits as u64), _ => None };
                let Some(n) = n else { return unsupported("repeat count") };
                if v.size() == 0 { Alloc::new(0, 1, Kind::Stack) } else {
                if v.size().saturating_mul(n) > 1 << 28 { return unsupported("huge repeat"); }
                let mut out = Alloc::new(v.size() * n, 1, Kind::Stack);
                for i in 0..n { out.put(i * v.size(), &v, self.ps); }
                out }
            }
            Rvalue::Ref(_, kind, place) => {
                let p = self.eval_place(place)?;
                let shared = matches!(kind, mir::BorrowKind::Shared) && self.is_freeze(p.ty);
                let ptr = Ptr { prov: p.ptr.prov.map(|pr| Prov { shared: pr.shared || shared, ..pr }), off: p.ptr.off };
                // (a reference must point to a live allocation; validity of the final value checks the rest)
                self.check_bounds(ptr, 0, 1)?;
                self.ptr_val(ptr, p.meta)
            }
            Rvalue::RawPtr(_, place) => { let p = self.eval_place(place)?; self.ptr_val(p.ptr, p.meta) }
            Rvalue::Cast(kind, op, to) => { let (v, from) = self.eval_operand(op)?; self.cast(*kind, v, from, *to)? }
            Rvalue::BinaryOp(op, ab) => {
                let (a, at) = self.eval_operand(&ab.0)?; let (b, bt) = self.eval_operand(&ab.1)?;
                self.binop(*op, &a, at, &b, bt, dest.ty)?
            }
            Rvalue::UnaryOp(op, o) => {
                let (v, t) = self.eval_operand(o)?;
                match op {
                    UnOp::Not if matches!(tys.kind(t), TyKind::Bool) => val_bytes(!self.bool_of(&v)? as u128, 1),
                    UnOp::Not => { let Some((s, _)) = self.int_ty(t) else { return unsupported("not") }; val_bytes(!self.int_of(&v)? & mask(s), s) }
                    UnOp::Neg => match tys.kind(t) {
                        TyKind::Float(Prim::F32) => val_bytes((f32::from_bits(self.int_of(&v)? as u32) * -1.0).to_bits() as u128, 4),
                        TyKind::Float(_) => val_bytes((-f64::from_bits(self.int_of(&v)? as u64)).to_bits() as u128, 8),
                        _ => { let Some((s, _)) = self.int_ty(t) else { return unsupported("neg") }; val_bytes(0u128.wrapping_sub(self.int_of(&v)?) & mask(s), s) }
                    },
                    UnOp::PtrMetadata => {
                        if v.size() == 2 * self.ps { v.slice(self.ps, self.ps, self.ps) } else { Alloc::new(0, 1, Kind::Stack) }
                    }
                }
            }
            Rvalue::Discriminant(place) => {
                let p = self.eval_place(place)?;
                let d = self.read_discriminant(p)?;
                let (size, _) = self.size_align(dest.ty)?;
                val_bytes(d & mask(size), size)
            }
            Rvalue::Aggregate(kind, ops) => return self.aggregate(dest, kind, ops),
            Rvalue::ThreadLocalRef(_) => return ub("cannot access thread local static"),
        };
        self.write_val(dest, &v)
    }

    /// The type has no UnsafeCell outside indirection (rustc Ty::is_freeze; Relax marks UnsafeCell
    /// with `#[rustc_layout_no_niche]`).
    pub(super) fn is_freeze(&self, ty: TyId) -> bool {
        let tys = &self.tcx.tys;
        match tys.kind(ty) {
            TyKind::Adt(d, args, _) => {
                let Some(adt) = self.tcx.adts.get(d) else { return false };
                if adt.no_niche { return false; }
                let args: Vec<TyId> = tys.list(*args).to_vec();
                adt.variants.iter().all(|v| v.fields.iter().all(|f| self.is_freeze(tys.subst_erasing_regions(f.ty, &args))))
            }
            TyKind::Tuple(l) => tys.list(*l).iter().all(|t| self.is_freeze(*t)),
            TyKind::Array(e, _) | TyKind::Slice(e) => self.is_freeze(*e),
            TyKind::Dyn(..) | TyKind::Param(_) | TyKind::Alias(..) | TyKind::Closure(..) => false,
            _ => true,
        }
    }

    fn aggregate(&mut self, dest: MPlace, kind: &AggregateKind, ops: &[Operand]) -> R<()> {
        match kind {
            AggregateKind::Array(e) => {
                let (size, _) = self.size_align(*e)?;
                for (i, op) in ops.iter().enumerate() {
                    let (v, _) = self.eval_operand(op)?;
                    self.write_val(MPlace { ptr: Ptr { off: dest.ptr.off + i as u64 * size, ..dest.ptr }, meta: None, ty: *e, variant: None, packed: dest.packed }, &v)?;
                }
                Ok(())
            }
            AggregateKind::Tuple | AggregateKind::Adt(..) => {
                let (variant, active) = match kind { AggregateKind::Adt(_, v, _, _, active) => (Some(v.0), *active), _ => (None, None) };
                let l = self.layout(dest.ty)?;
                let is_enum = matches!(self.tcx.tys.kind(dest.ty), TyKind::Adt(d, ..) if self.tcx.adts.get(d).is_some_and(|a| a.is_enum));
                for (i, op) in ops.iter().enumerate() {
                    let (v, t) = self.eval_operand(op)?;
                    let fi = active.map_or(i, |f| f.0 as usize);
                    let offs = if is_enum { &l.variant_fields[variant.unwrap() as usize] } else { &l.fields };
                    // (a union's fields all start at 0)
                    let o = if active.is_some() { 0 } else { match offs.get(fi) { Some(&o) if o != u32::MAX => o, _ => return unsupported("aggregate field offset") } };
                    let packed = match kind { AggregateKind::Adt(d, ..) => self.tcx.adts.get(d).map_or(0, |a| a.repr_packed as u64), _ => 0 };
                    let packed = if packed > 0 { Some(dest.packed.map_or(packed, |q| q.min(packed))) } else { dest.packed };
                    self.write_val(MPlace { ptr: Ptr { off: dest.ptr.off + o as u64, ..dest.ptr }, meta: None, ty: t, variant: None, packed }, &v)?;
                }
                if is_enum { self.write_discriminant(dest, variant.unwrap())?; }
                Ok(())
            }
            AggregateKind::RawPtr(_, _) => {
                let (p, _) = self.eval_operand(&ops[0])?; let (m, _) = self.eval_operand(&ops[1])?;
                let ptr = self.ptr_of(&p, 0)?;
                let meta = if m.size() == 0 { None } else { Some(self.int_of(&m)? as u64) };
                let v = self.ptr_val(ptr, meta);
                self.write_val(dest, &v)
            }
            _ => unsupported("closure aggregate"),
        }
    }

    // ---- discriminants (discriminant.rs)

    fn adt_discrs(&self, ty: TyId) -> R<Vec<u128>> {
        let TyKind::Adt(d, ..) = self.tcx.tys.kind(ty) else { return unsupported("discriminant of a non-enum") };
        let Some(adt) = self.tcx.adts.get(d) else { return unsupported("adt") };
        if !adt.is_enum { return Ok(vec![0]); }
        Ok(adt.discriminants(self.tcx, self.prog).into_iter().map(|(_, d)| d.val).collect())
    }

    fn write_discriminant(&mut self, p: MPlace, variant: u32) -> R<()> {
        let l = self.layout(p.ty)?;
        let discrs = self.adt_discrs(p.ty)?;
        let Some((off, mem)) = l.tag else { return Ok(()) };
        let size = mem_size(mem);
        let tag = match l.enc {
            crate::layout::TagEnc::Single(_) => return Ok(()),
            crate::layout::TagEnc::Direct => discrs[variant as usize],
            crate::layout::TagEnc::Niche { untagged, lo, hi, start } => {
                if variant == untagged || variant < lo || variant > hi { return Ok(()); }
                start.wrapping_add((variant - lo) as u128)
            }
        };
        self.write(Ptr { off: p.ptr.off + off as u64, ..p.ptr }, &val_bytes(tag & mask(size), size), 1)
    }

    /// The variant index of the enum value at `p` (discriminant.rs read_discriminant: "enum value has invalid tag").
    pub(super) fn read_variant(&mut self, p: MPlace) -> R<u32> {
        let l = self.layout(p.ty)?;
        if let crate::layout::TagEnc::Single(v) = l.enc { if l.uninhabited { return ub("enum value has uninhabited variant"); } return Ok(v); }
        let Some((off, mem)) = l.tag else { return Ok(0) };
        let size = mem_size(mem);
        let tv = self.read(Ptr { off: p.ptr.off + off as u64, ..p.ptr }, size, 1)?;
        // (discriminant.rs: a pointer in the tag of a niche whose only invalid value is 0 is the untagged variant)
        if !tv.ptrs.is_empty() { if let crate::layout::TagEnc::Niche { untagged, .. } = l.enc { return Ok(untagged); } }
        let tag = self.int_of(&tv)?;
        let discrs = self.adt_discrs(p.ty)?;
        match l.enc {
            crate::layout::TagEnc::Direct => discrs.iter().position(|d| d & mask(size) == tag).map(|v| v as u32).map_or_else(|| ub(format!("enum value has invalid tag: {tag:#x}")), Ok),
            crate::layout::TagEnc::Niche { untagged, lo, hi, start } => {
                let rel = tag.wrapping_sub(start) & mask(size);
                if rel <= (hi - lo) as u128 && lo + rel as u32 != untagged { Ok(lo + rel as u32) } else { Ok(untagged) }
            }
            crate::layout::TagEnc::Single(v) => Ok(v),
        }
    }

    fn read_discriminant(&mut self, p: MPlace) -> R<u128> {
        let v = self.read_variant(p)?;
        Ok(self.adt_discrs(p.ty)?[v as usize])
    }

    // ---- casts (cast.rs)

    fn cast(&mut self, kind: CastKind, v: Val, from: TyId, to: TyId) -> R<Val> {
        let tys = &self.tcx.tys;
        match kind {
            CastKind::IntToInt => {
                let (Some((fs, fsigned)), Some((ts, _))) = (self.int_ty(from), self.int_ty(to)) else { return unsupported("int cast") };
                // (cast.rs: the operand is read as its type: a bool or char must be valid)
                let mut x = match self.tcx.tys.kind(from) { TyKind::Bool => self.bool_of(&v)? as u128, _ => self.int_of(&v)? };
                if matches!(self.tcx.tys.kind(from), TyKind::Char) && char::from_u32(x as u32).is_none() { return ub(format!("interpreting an invalid 32-bit value as a char: {x:#010x}")); }
                if fsigned && fs < 16 && (x >> (fs * 8 - 1)) & 1 == 1 { x |= !mask(fs); }
                Ok(val_bytes(x & mask(ts), ts))
            }
            CastKind::PtrToPtr | CastKind::FnPtrToPtr => {
                let to_fat = match tys.kind(to) { TyKind::Ptr(_, t) | TyKind::Ref(_, t, _) => self.is_fat(*t)?, _ => false };
                if v.size() == 2 * self.ps && !to_fat { Ok(v.slice(0, self.ps, self.ps)) } else { Ok(v) }
            }
            CastKind::Transmute => {
                let (size, _) = self.size_align(to)?;
                if size != v.size() { return ub("transmuting between types of different sizes"); }
                Ok(v)
            }
            CastKind::PointerExposeProvenance => {
                let p = self.ptr_of(&v, 0)?;
                if p.prov.is_some() { return self.ptr_int_ub("exposing pointers is not possible at compile-time"); }
                let (size, _) = self.size_align(to)?;
                Ok(val_bytes(p.off as u128, size))
            }
            CastKind::PointerWithExposedProvenance => { let x = self.int_of(&v)?; Ok(self.ptr_val(Ptr { prov: None, off: x as u64 }, None)) }
            CastKind::PointerCoercion(PointerCoercion::Unsize, _) => {
                let (TyKind::Ref(_, a, _) | TyKind::Ptr(_, a), TyKind::Ref(_, b, _) | TyKind::Ptr(_, b)) = (tys.kind(from).clone(), tys.kind(to).clone()) else { return unsupported("unsize of a smart pointer") };
                match (tys.kind(a), tys.kind(b)) {
                    (TyKind::Array(_, n), TyKind::Slice(_)) => {
                        let Some(n) = n.try_to_target_usize(tys) else { return unsupported("array length") };
                        let p = self.ptr_of(&v, 0)?;
                        Ok(self.ptr_val(p, Some(n)))
                    }
                    _ => unsupported("unsize coercion"),
                }
            }
            CastKind::PointerCoercion(PointerCoercion::ReifyFnPointer(_), _) => {
                let TyKind::FnDef(d, ..) = tys.kind(from) else { return unsupported("reify") };
                let mut a = Alloc::new(0, 1, Kind::Function(*d)); a.mutable = false;
                let a = self.alloc(a);
                Ok(self.ptr_val(Ptr { prov: Some(Prov { alloc: a, shared: false }), off: 0 }, None))
            }
            CastKind::IntToFloat | CastKind::FloatToInt | CastKind::FloatToFloat => self.float_cast(kind, v, from, to),
            _ => unsupported("cast"),
        }
    }

    fn float_cast(&self, kind: CastKind, v: Val, from: TyId, to: TyId) -> R<Val> {
        let tys = &self.tcx.tys;
        let x = self.int_of(&v)?;
        let as_f64 = |t: TyId, bits: u128| -> f64 { if matches!(tys.kind(t), TyKind::Float(Prim::F32)) { f32::from_bits(bits as u32) as f64 } else { f64::from_bits(bits as u64) } };
        let to_float = |t: TyId, f: f64| -> Val { if matches!(tys.kind(t), TyKind::Float(Prim::F32)) { val_bytes((f as f32).to_bits() as u128, 4) } else { val_bytes(f.to_bits() as u128, 8) } };
        match kind {
            CastKind::FloatToFloat => Ok(to_float(to, as_f64(from, x))),
            CastKind::IntToFloat => {
                let Some((fs, signed)) = self.int_ty(from) else { return unsupported("int to float") };
                if fs > 8 { return unsupported("128-bit float cast"); }
                let f = if signed { ((x as u64) << (64 - fs * 8)) as i64 >> (64 - fs * 8) } else { 0 };
                Ok(to_float(to, if signed { f as f64 } else { x as u64 as f64 }))
            }
            _ => {
                let Some((ts, signed)) = self.int_ty(to) else { return unsupported("float to int") };
                if ts > 8 { return unsupported("128-bit float cast"); }
                let f = as_f64(from, x);
                // (Rust's `as` saturates; NaN is 0)
                let bits = if signed { let i = f as i64; let lo = -(1i64 << (ts * 8 - 1)).wrapping_sub(0); let hi = if ts == 8 { i64::MAX } else { (1i64 << (ts * 8 - 1)) - 1 }; (i.clamp(if ts == 8 { i64::MIN } else { lo }, hi) as u64) as u128 } else { let u = f as u64; (if ts == 8 { u } else { u.min((1u64 << (ts * 8)) - 1) }) as u128 };
                Ok(val_bytes(bits & mask(ts), ts))
            }
        }
    }

    // ---- binary operators (operator.rs)

    fn binop(&mut self, op: BinOp, a: &Val, at: TyId, b: &Val, bt: TyId, dest: TyId) -> R<Val> {
        let tys = &self.tcx.tys;
        if matches!(tys.kind(at), TyKind::Ptr(..) | TyKind::Ref(..) | TyKind::FnPtr(..)) {
            if op == BinOp::Offset {
                let TyKind::Ptr(_, pointee) = tys.kind(at) else { return unsupported("offset") };
                let (size, _) = self.size_align(*pointee)?;
                let p = self.ptr_of(a, 0)?;
                let n = self.int_of(b)? as u64 as i64;
                let q = self.offset_inbounds(p, n, size)?;
                return Ok(self.ptr_val(q, None));
            }
            return unsupported("pointer comparison or arithmetic");
        }
        if let TyKind::Float(p) = tys.kind(at) {
            let (x, y) = (self.int_of(a)?, self.int_of(b)?);
            let f32_ = *p == Prim::F32;
            let (x, y) = if f32_ { (f32::from_bits(x as u32) as f64, f32::from_bits(y as u32) as f64) } else { (f64::from_bits(x as u64), f64::from_bits(y as u64)) };
            let cmp = |c: bool| Ok(val_bytes(c as u128, 1));
            let r = match op {
                BinOp::Eq => return cmp(x == y), BinOp::Ne => return cmp(x != y), BinOp::Lt => return cmp(x < y),
                BinOp::Le => return cmp(x <= y), BinOp::Gt => return cmp(x > y), BinOp::Ge => return cmp(x >= y),
                BinOp::Add => x + y, BinOp::Sub => x - y, BinOp::Mul => x * y, BinOp::Div => x / y, BinOp::Rem => x % y,
                _ => return unsupported("float op"),
            };
            // (f32 arithmetic rounds once: an f64 result of two f32 operands is exact for + - * and correctly rounded for /)
            return Ok(if f32_ { val_bytes((r as f32).to_bits() as u128, 4) } else { val_bytes(r.to_bits() as u128, 8) });
        }
        let Some((s, signed)) = self.int_ty(at) else { return unsupported("binop operand") };
        let (x, y) = if matches!(tys.kind(at), TyKind::Bool) { (self.bool_of(a)? as u128, self.bool_of(b)? as u128) } else { (self.int_of(a)?, self.int_of(b)?) };
        let sx = |v: u128| -> i128 { if s >= 16 { v as i128 } else { ((v << (128 - s * 8)) as i128) >> (128 - s * 8) } };
        let bits = s * 8;
        let cmp = |c: bool| Ok(val_bytes(c as u128, 1));
        let lt = if signed { sx(x) < sx(y) } else { x < y };
        match op {
            BinOp::Eq => return cmp(x == y), BinOp::Ne => return cmp(x != y), BinOp::Lt => return cmp(lt),
            BinOp::Le => return cmp(lt || x == y), BinOp::Gt => return cmp(!lt && x != y), BinOp::Ge => return cmp(!lt),
            BinOp::Cmp => { let o: i8 = if x == y { 0 } else if lt { -1 } else { 1 }; return Ok(val_bytes(o as u8 as u128, 1)); }
            _ => {}
        }
        let shift = |y: u128| -> R<u32> {
            let sh = if matches!(self.int_ty(bt), Some((_, true))) && (y >> (self.int_ty(bt).unwrap().0 * 8 - 1)) & 1 == 1 { u128::MAX } else { y };
            Ok(sh.min(u32::MAX as u128) as u32)
        };
        // (value, overflowed)
        let (r, ov): (u128, bool) = match op {
            BinOp::Add | BinOp::AddWithOverflow | BinOp::AddUnchecked => {
                let r = x.wrapping_add(y) & mask(s);
                (r, if signed { sx(x).checked_add(sx(y)).is_none_or(|v| v != sx(r)) } else { x.checked_add(y).is_none_or(|v| v > mask(s)) })
            }
            BinOp::Sub | BinOp::SubWithOverflow | BinOp::SubUnchecked => {
                let r = x.wrapping_sub(y) & mask(s);
                (r, if signed { sx(x).checked_sub(sx(y)).is_none_or(|v| v != sx(r)) } else { y > x })
            }
            BinOp::Mul | BinOp::MulWithOverflow | BinOp::MulUnchecked => {
                let r = x.wrapping_mul(y) & mask(s);
                (r, if signed { sx(x).checked_mul(sx(y)).is_none_or(|v| v != sx(r)) } else { x.checked_mul(y).is_none_or(|v| v > mask(s)) })
            }
            BinOp::Div | BinOp::Rem => {
                if y == 0 { return ub(if op == BinOp::Div { "dividing by zero" } else { "calculating the remainder with a divisor of zero" }); }
                if signed {
                    let (a, c) = (sx(x), sx(y));
                    if c == -1 && a == sx(1u128 << (bits - 1)) { return ub(if op == BinOp::Div { "overflow in signed division (dividing MIN by -1)" } else { "overflow in signed remainder (dividing MIN by -1)" }); }
                    ((if op == BinOp::Div { a / c } else { a % c }) as u128 & mask(s), false)
                } else { (if op == BinOp::Div { x / y } else { x % y }, false) }
            }
            BinOp::BitAnd => (x & y, false), BinOp::BitOr => (x | y, false), BinOp::BitXor => (x ^ y, false),
            BinOp::Shl | BinOp::ShlUnchecked | BinOp::Shr | BinOp::ShrUnchecked => {
                let sh = shift(y)?;
                let ov = sh >= bits as u32;
                let sh = sh % bits as u32;
                let r = if matches!(op, BinOp::Shl | BinOp::ShlUnchecked) { (x << sh) & mask(s) } else if signed { (sx(x) >> sh) as u128 & mask(s) } else { x >> sh };
                if ov && matches!(op, BinOp::ShlUnchecked | BinOp::ShrUnchecked) { return ub(format!("overflowing shift by {} in `{}`", y, if op == BinOp::ShlUnchecked { "unchecked_shl" } else { "unchecked_shr" })); }
                (r, ov)
            }
            _ => return unsupported("binop"),
        };
        match op {
            BinOp::AddWithOverflow | BinOp::SubWithOverflow | BinOp::MulWithOverflow => {
                let l = self.layout(dest)?;
                let mut out = Alloc::new(l.size as u64, 1, Kind::Stack);
                out.put(l.fields[0] as u64, &val_bytes(r, s), self.ps);
                out.put(l.fields[1] as u64, &val_bytes(ov as u128, 1), self.ps);
                Ok(out)
            }
            BinOp::AddUnchecked | BinOp::SubUnchecked | BinOp::MulUnchecked if ov => {
                ub(format!("arithmetic overflow in `{}`", match op { BinOp::AddUnchecked => "unchecked_add", BinOp::SubUnchecked => "unchecked_sub", _ => "unchecked_mul" }))
            }
            _ => Ok(val_bytes(r, s)),
        }
    }

    /// intrinsics.rs ptr_offset_inbounds: "in-bounds pointer arithmetic failed".
    pub(super) fn offset_inbounds(&self, p: Ptr, count: i64, elem: u64) -> R<Ptr> {
        let Some(bytes) = (elem as i64).checked_mul(count) else { return ub("overflowing pointer arithmetic: the total offset in bytes does not fit in an `isize`") };
        if bytes == 0 { return Ok(p); }
        let sign = if bytes < 0 { "-" } else { "" };
        let n = bytes.unsigned_abs();
        let Some(pr) = p.prov else { return ub(format!("in-bounds pointer arithmetic failed: attempting to offset pointer by {sign}{n} byte{}, but got {} which is a dangling pointer (it has no provenance)", if n == 1 { "" } else { "s" }, self.ptr_str(p))) };
        let a = &self.allocs[pr.alloc as usize];
        if !a.live { return ub(format!("in-bounds pointer arithmetic failed: {} has been freed, so this pointer is dangling", self.alloc_name(pr.alloc))); }
        let new = p.off as i128 + bytes as i128;
        if new < 0 || new as u64 > a.size() {
            return ub(format!("in-bounds pointer arithmetic failed: attempting to offset pointer by {sign}{n} byte{}, but got {} which is only {} bytes from the end of the allocation", if n == 1 { "" } else { "s" }, self.ptr_str(p), a.size().saturating_sub(p.off)));
        }
        Ok(Ptr { prov: p.prov, off: new as u64 })
    }

    // ---- terminators (step.rs, call.rs)

    fn terminator(&mut self, t: &TerminatorKind) -> R<()> {
        match t {
            TerminatorKind::Goto { target } => { self.goto(*target); Ok(()) }
            TerminatorKind::FalseEdge { real_target, .. } | TerminatorKind::FalseUnwind { real_target, .. } => { self.goto(*real_target); Ok(()) }
            TerminatorKind::SwitchInt { discr, targets } => {
                let (v, t) = self.eval_operand(discr)?;
                let x = if matches!(self.tcx.tys.kind(t), TyKind::Bool) { self.bool_of(&v)? as u128 } else { self.int_of(&v)? };
                let to = targets.values.iter().position(|&c| c == x).map_or(targets.otherwise, |i| targets.targets[i]);
                self.goto(to);
                Ok(())
            }
            TerminatorKind::Return => self.pop_frame(),
            TerminatorKind::Unreachable => ub("entering unreachable code"),
            TerminatorKind::Assert { cond, expected, msg, target, .. } => {
                let (v, _) = self.eval_operand(cond)?;
                if self.bool_of(&v)? == *expected { self.goto(*target); return Ok(()); }
                // (rustc builds its std without overflow checks: an overflow assert of a Relax std fn
                // is not one rustc's MIR has)
                if matches!(msg, AssertKind::Overflow(..) | AssertKind::OverflowNeg(_)) && self.in_std_fn() { self.goto(*target); return Ok(()); }
                ub(self.assert_message(msg)?)
            }
            TerminatorKind::Drop { place, target, .. } => {
                let p = self.eval_place(place)?;
                let env = crate::tcx::TypingEnv::fully_monomorphized(self.prog);
                if self.tcx.needs_drop(self.prog, env, p.ty) { return unsupported("drop glue"); }
                self.goto(*target);
                Ok(())
            }
            TerminatorKind::Call { func, args, destination, target, .. } => self.call(func, args, destination, *target),
            _ => unsupported("terminator"),
        }
    }

    fn assert_message(&mut self, msg: &AssertKind) -> R<String> {
        let tys = &self.tcx.tys;
        let show = |it: &mut Self, op: &Operand| -> R<String> {
            let (v, t) = it.eval_operand(op)?;
            let x = it.int_of(&v)?;
            let (s, signed) = it.int_ty(t).unwrap_or((8, false));
            let name = crate::typeck::ty_to_string(false, it.prog, it.tcx, t);
            let n = if signed && s < 16 { (((x << (128 - s * 8)) as i128) >> (128 - s * 8)).to_string() } else { x.to_string() };
            Ok(format!("{n}_{name}"))
        };
        let _ = tys;
        Ok(match msg {
            AssertKind::Overflow(op, a, b) => {
                let (a, b) = (show(self, a)?, show(self, b)?);
                let sym = match op { BinOp::Add => "+", BinOp::Sub => "-", BinOp::Mul => "*", BinOp::Div => "/", BinOp::Rem => "%", BinOp::Shl => "<<", BinOp::Shr => ">>", _ => "?" };
                match op {
                    BinOp::Shl | BinOp::Shr => format!("attempt to shift {} by `{b}`, which would overflow", if *op == BinOp::Shl { "left" } else { "right" }),
                    BinOp::Div => format!("attempt to compute `{a} / {b}`, which would overflow"),
                    BinOp::Rem => format!("attempt to compute the remainder of `{a} % {b}`, which would overflow"),
                    _ => format!("attempt to compute `{a} {sym} {b}`, which would overflow"),
                }
            }
            AssertKind::OverflowNeg(a) => format!("attempt to negate `{}`, which would overflow", show(self, a)?),
            AssertKind::DivisionByZero(a) => format!("attempt to divide `{}` by zero", show(self, a)?),
            AssertKind::RemainderByZero(a) => format!("attempt to calculate the remainder of `{}` with a divisor of zero", show(self, a)?),
            AssertKind::BoundsCheck { len, index } => {
                let (l, _) = self.eval_operand(len)?; let (i, _) = self.eval_operand(index)?;
                format!("index out of bounds: the length is {} but the index is {}", self.int_of(&l)?, self.int_of(&i)?)
            }
            AssertKind::MisalignedPointerDereference { .. } => "misaligned pointer dereference".into(),
            AssertKind::NullPointerDereference => "null pointer dereference occurred".into(),
            _ => return unsupported("coroutine assert"),
        })
    }

    fn call(&mut self, func: &Operand, args: &[Operand], destination: &Place, target: Option<BasicBlock>) -> R<()> {
        let (_, fty) = self.eval_operand(func)?;
        let TyKind::FnDef(d, fargs, _) = self.tcx.tys.kind(fty).clone() else { return unsupported("call through a pointer") };
        let fargs: Vec<TyId> = self.tcx.tys.list(fargs).to_vec();
        let mut vals = Vec::new();
        let mut tys = Vec::new();
        for a in args { let (v, t) = self.eval_operand(a)?; vals.push(v); tys.push(t); }
        let dest = self.eval_place(destination)?;
        let path = self.prog.def_path(d);
        if path.contains("intrinsic_decls::") || path.starts_with("core::intrinsics::") {
            self.intrinsic(self.prog.name(d).to_string(), &fargs, &vals, &tys, dest)?;
            match target { Some(t) => { self.goto(t); return Ok(()); } None => return ub("entering unreachable code") }
        }
        // machine.rs hook_special_const_fn: a panic ends the evaluation with its message
        let lang = &self.tcx.lang_items;
        // (Relax lowers `panic!` to panic_at(args, location), panic_fmt's body)
        if lang.get(crate::lang_items::LangItem::PanicFmt) == Some(d) || path == "core::panicking::panic_at" { let m = self.const_panic_fmt(&vals[0], tys[0])?; return ub(format!("evaluation panicked: {m}")); }
        if lang.get(crate::lang_items::LangItem::PanicDisplay) == Some(d) { let m = self.panic_display(&vals[0], fargs.first().copied())?; return ub(format!("evaluation panicked: {m}")); }
        let env = crate::tcx::TypingEnv::fully_monomorphized(self.prog);
        let inst = match self.tcx.resolve_instance_raw(self.prog, env, d, &fargs) { Ok(i) => i, Err(_) => return unsupported("instance") };
        let crate::instance::InstanceKind::Item(callee) = inst.def else { return unsupported("shim call") };
        let cargs = inst.compact_args(self.tcx);
        if !matches!(self.prog.def(callee).kind, DefKind::Fn | DefKind::AssocFn) { return unsupported("call of a non-fn"); }
        let body = self.body_of(callee, &cargs)?;
        if body.arg_count as usize != vals.len() || body.spread_arg.is_some() { return unsupported("call arity"); }
        self.push_frame(body, vals, Some((dest, target)))
    }

    /// core's const_panic_fmt, which rustc's hook calls for panic_fmt: `fmt.as_str()` (Relax's std's
    /// own `Arguments::as_str`, interpreted) and its text; arguments that need formatting are
    /// unreachable there (rustc lowers a const `panic!("{}", s)` to panic_display).
    fn const_panic_fmt(&mut self, args: &Val, args_ty: TyId) -> R<String> {
        let key = crate::tcx::ty_key(&self.tcx.tys, args_ty);
        let as_str = self.tcx.inherent.get(&key).and_then(|imps| imps.iter().flat_map(|&imp| self.prog.impls[self.prog.def(imp).sub as usize].items.iter().copied()).find(|&it| self.prog.name(it) == "as_str"));
        let Some(as_str) = as_str else { return unsupported("Arguments::as_str") };
        let body = self.body_of(as_str, &[])?;
        let (size, align) = self.size_align(args_ty)?;
        let tmp = self.alloc(Alloc::new(size, align, Kind::Stack));
        let ps = self.ps; self.allocs[tmp as usize].put(0, args, ps);
        let this = self.ptr_val(Ptr { prov: Some(Prov { alloc: tmp, shared: true }), off: 0 }, None);
        let ret_ty = body.local_decls[0].ty;
        let ret = self.call_to_completion(body, vec![this])?;
        // Option<&'static str>
        let place = MPlace { ptr: Ptr { prov: Some(Prov { alloc: ret, shared: false }), off: 0 }, meta: None, ty: ret_ty, variant: None, packed: None };
        let v = self.read_variant(place)?;
        let TyKind::Adt(od, oargs, _) = self.tcx.tys.kind(ret_ty).clone() else { return unsupported("as_str result") };
        let Some(adt) = self.tcx.adts.get(&od) else { return unsupported("as_str result") };
        if self.prog.syms.str(adt.variants[v as usize].name) != "Some" { return unsupported("formatted const panic"); }
        let l = self.layout(ret_ty)?;
        let off = match l.variant_fields.get(v as usize).and_then(|f| f.first()) { Some(&o) => o as u64, None => l.fields.first().map_or(0, |&o| o as u64) };
        let sty = self.tcx.tys.subst_erasing_regions(adt.variants[v as usize].fields[0].ty, &self.tcx.tys.list(oargs).to_vec());
        let sv = self.read_place(MPlace { ptr: Ptr { off, ..place.ptr }, meta: None, ty: sty, variant: None, packed: None })?;
        self.read_str(&sv)
    }

    /// rustc's hook for panic_display: its argument is a `&&str`.
    fn panic_display(&mut self, arg: &Val, t: Option<TyId>) -> R<String> {
        let Some(t) = t else { return unsupported("panic_display") };
        if !matches!(self.tcx.tys.kind(t), TyKind::Ref(_, s, _) if matches!(self.tcx.tys.kind(*s), TyKind::Str)) { return unsupported("panic_display of a non-str"); }
        let p = self.ptr_of(arg, 0)?;
        let ps = self.ps;
        let sv = self.read(p, 2 * ps, ps)?;
        self.read_str(&sv)
    }

    /// intrinsics.rs caller_location: the `Location` of the innermost frame that is not
    /// #[track_caller] (find_closest_untracked_caller_location), at its current call, as global memory.
    fn caller_location(&mut self, ref_ty: TyId) -> R<u32> {
        let TyKind::Ref(_, loc_ty, _) = self.tcx.tys.kind(ref_ty).clone() else { return unsupported("caller_location type") };
        let span = self.frames.iter().rev().find_map(|f| {
            let tracked = matches!(f.body.source.kind, mir::BodyKind::Fn) && self.tcx.fn_sig(self.prog, f.body.source.owner).is_some_and(|s| s.track_caller);
            if tracked { return None; }
            let bb = &f.body.basic_blocks[f.block.0 as usize];
            Some(if f.stmt < bb.statements.len() { bb.statements[f.stmt].source_info.span } else { bb.terminator.as_ref()?.source_info.span })
        });
        let Some(span) = span else { return unsupported("caller_location span") };
        let (Some(file), Some(path)) = (self.prog.files.get(span.file as usize), self.prog.file_paths.get(span.file as usize)) else { return unsupported("caller_location file") };
        let (line, col) = file.line_col(span.lo);
        let mut name = path.as_bytes().to_vec();
        name.push(0);
        let n = name.len() as u64;
        let mut fa = Alloc::new(n, 1, Kind::Global);
        fa.bytes = name; fa.init = vec![true; n as usize]; fa.mutable = false; fa.global = true;
        let fa = self.alloc(fa);
        let TyKind::Adt(d, ..) = self.tcx.tys.kind(loc_ty).clone() else { return unsupported("Location") };
        let Some(adt) = self.tcx.adts.get(&d) else { return unsupported("Location") };
        let names: Vec<String> = adt.variants[0].fields.iter().map(|f| self.prog.syms.str(f.name).to_string()).collect();
        let l = self.layout(loc_ty)?;
        let mut v = Alloc::new(l.size as u64, l.align as u64, Kind::Global);
        let ps = self.ps;
        for (i, name) in names.iter().enumerate() {
            let Some(&o) = l.fields.get(i) else { return unsupported("Location field") };
            let fv = match name.as_str() {
                "file_bytes_with_nul" => self.ptr_val(Ptr { prov: Some(Prov { alloc: fa, shared: true }), off: 0 }, Some(n)),
                "line" => val_bytes(line as u128, 4),
                "col" => val_bytes(col as u128, 4),
                _ => return unsupported("Location field"),
            };
            v.put(o as u64, &fv, ps);
        }
        v.mutable = false; v.global = true;
        Ok(self.alloc(v))
    }

    /// The text a `&str` value points to.
    fn read_str(&mut self, v: &Val) -> R<String> {
        let p = self.ptr_of(v, 0)?;
        let n = self.int_of(&v.slice(self.ps, self.ps, self.ps))? as u64;
        let b = self.read(p, n, 1)?;
        if b.init.iter().any(|x| !x) { return Err(self.uninit_err(&b, "")); }
        Ok(String::from_utf8_lossy(&b.bytes).into_owned())
    }

    /// Calls `body` with `args` on a stack of its own and runs it to its return; the allocation of
    /// the return value.
    fn call_to_completion(&mut self, body: Rc<Body>, args: Vec<Val>) -> R<u32> {
        let ty = body.local_decls[0].ty;
        let (size, align) = self.size_align(ty)?;
        let ret = self.alloc(Alloc::new(size, align, Kind::Stack));
        let place = MPlace { ptr: Ptr { prov: Some(Prov { alloc: ret, shared: false }), off: 0 }, meta: None, ty, variant: None, packed: None };
        let depth = self.frames.len();
        self.push_frame(body, args, Some((place, None)))?;
        while self.frames.len() > depth { self.step()?; }
        Ok(ret)
    }

    /// intrinsics.rs emulate_intrinsic over Relax's intrinsic set.
    fn intrinsic(&mut self, name: String, fargs: &[TyId], vals: &[Val], tys: &[TyId], dest: MPlace) -> R<()> {
        let t0 = fargs.first().copied();
        let ps = self.ps;
        let v = match name.as_str() {
            "size_of" => { let (s, _) = self.size_align(t0.unwrap())?; val_bytes(s as u128, ps) }
            "align_of" | "min_align_of" | "pref_align_of" => { let (_, a) = self.size_align(t0.unwrap())?; val_bytes(a as u128, ps) }
            "transmute" | "transmute_unchecked" => {
                let (size, _) = self.size_align(dest.ty)?;
                if size != vals[0].size() { return ub("transmuting between types of different sizes"); }
                vals[0].clone()
            }
            "ptr_read" | "read_via_copy" => {
                let p = self.ptr_of(&vals[0], 0)?;
                let (size, align) = self.size_align(dest.ty)?;
                let v = self.read(p, size, align)?;
                // (a typed read of the program's memory, as rustc's read_via_copy)
                self.check_int_read(&v, dest.ty, true)?;
                v
            }
            "ptr_write" | "write_via_move" => {
                let p = self.ptr_of(&vals[0], 0)?;
                let (_, align) = self.size_align(tys[1])?;
                self.write(p, &vals[1], align)?;
                return Ok(());
            }
            "copy_nonoverlapping" | "copy" => {
                let elem = t0.unwrap();
                let (s, d) = (self.ptr_of(&vals[0], 0)?, self.ptr_of(&vals[1], 0)?);
                let n = self.int_of(&vals[2])? as u64;
                return self.copy(s, d, elem, n, name == "copy_nonoverlapping");
            }
            "offset" | "ptr_add" | "ptr_add_mut" | "arith_offset" => {
                let elem = match self.tcx.tys.kind(tys[0]) { TyKind::Ptr(_, t) => *t, _ => return unsupported("offset pointee") };
                let (size, _) = self.size_align(elem)?;
                let p = self.ptr_of(&vals[0], 0)?;
                let n = self.int_of(&vals[1])? as u64 as i64;
                if name == "arith_offset" { return unsupported("wrapping offset"); }
                let q = self.offset_inbounds(p, n, size)?;
                self.ptr_val(q, None)
            }
            "caller_location" => { let loc = self.caller_location(dest.ty)?; self.ptr_val(Ptr { prov: Some(Prov { alloc: loc, shared: true }), off: 0 }, None) }
            "unreachable" => return ub("entering unreachable code"),
            "abort" => return ub("the program aborted execution"),
            _ => return unsupported(format!("intrinsic {name} in {}", self.prog.def_path(self.frames.last().unwrap().body.source.owner))),
        };
        self.write_val(dest, &v)
    }
}

pub(super) fn mask(size: u64) -> u128 { if size >= 16 { u128::MAX } else { (1u128 << (size * 8)) - 1 } }
pub(super) fn mem_size(m: relax_backend::rir::Mem) -> u64 {
    match m { relax_backend::rir::Mem::Int(b, _) => b as u64, relax_backend::rir::Mem::F32 => 4, relax_backend::rir::Mem::F64 => 8 }
}
