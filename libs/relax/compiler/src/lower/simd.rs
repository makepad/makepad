//! rustc's generic SIMD intrinsics (`core::intrinsics::simd`: simd_add .. simd_select) on
//! `#[repr(simd)]` values. A SIMD value lives in memory like any aggregate; each intrinsic is
//! lowered at its call site to the lane-typed vector IR, one 16-byte chunk at a time (`VLoad`,
//! `Inst::VX`, `VStore`). A value whose size is not a multiple of 16 goes through zeroed
//! 16-byte-padded copies (the padding lanes are computed and dropped). The back end encodes
//! each op natively where the target has an instruction sequence and legalises the rest to
//! per-lane scalar code (relax_backend::legal), so every lane type works on every target.
//! Store->load forwarding keeps chains of intrinsics in registers.

use super::*;

/// The lane type of a SIMD value.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(super) enum Lane {
    S(VW),
    U(VW),
    F(VW),
}

impl Lane {
    fn w(self) -> VW {
        match self {
            Lane::S(w) | Lane::U(w) | Lane::F(w) => w,
        }
    }
}

impl<'a> Lcx<'a> {
    /// (lane type, lane count) of a `#[repr(simd)]` struct: one array field `[E; N]` or N
    /// fields of one primitive type E (the older form); None for any other type.
    pub(super) fn simd_shape(&mut self, t: TyId) -> Option<(Lane, u32)> {
        let TyKind::Adt(d, args, _) = self.kind(t) else { return None };
        let nf = self.u.tcx.adts.get(&d).filter(|a| !a.is_enum && !a.is_union)?.variants[0].fields.len();
        let env = crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog);
        let fts: Vec<TyId> = (0..nf).map(|i| self.u.tcx.field_ty(&self.u.prog, env, d, 0, i as u32, self.u.tcx.tys.list(args))).collect();
        let lane = |k: &TyKind| -> Option<Lane> {
            Some(match k {
                TyKind::Int(p) | TyKind::Float(p) => match p {
                    Prim::I8 => Lane::S(VW::B),
                    Prim::I16 => Lane::S(VW::H),
                    Prim::I32 => Lane::S(VW::S),
                    Prim::I64 | Prim::Isize => Lane::S(VW::D),
                    Prim::U8 => Lane::U(VW::B),
                    Prim::U16 => Lane::U(VW::H),
                    Prim::U32 => Lane::U(VW::S),
                    Prim::U64 | Prim::Usize => Lane::U(VW::D),
                    Prim::F32 => Lane::F(VW::S),
                    Prim::F64 => Lane::F(VW::D),
                    _ => return None,
                },
                // thin pointers (gather/scatter addresses): 64-bit lanes
                TyKind::Ptr(..) => Lane::U(VW::D),
                _ => return None,
            })
        };
        match fts.as_slice() {
            [one] => match self.kind(*one) {
                TyKind::Array(e, n) => {
                    let n = n.try_to_target_usize(&self.u.tcx.tys)? as u32;
                    Some((lane(&self.kind(e))?, n))
                }
                k => Some((lane(&k)?, 1)),
            },
            [first, ..] => {
                let l = lane(&self.kind(*first))?;
                fts.iter().all(|f| *f == *first).then_some((l, nf as u32))
            }
            [] => None,
        }
    }

    /// An inline const block of an integer SIMD type, evaluated now (rustc evaluates every inline
    /// const at compile time): its lanes stored as constants, its bytes remembered for the
    /// intrinsics that take a constant vector (simd_shuffle's index). None: lowered as code.
    pub(super) fn simd_const_block(&mut self, bl: crate::ast::BlockId, t: TyId) -> Option<Val> {
        let (l, n) = self.simd_shape(t)?;
        if matches!(l, Lane::F(_)) || !matches!(self.kind(t), TyKind::Adt(..)) {
            return None;
        }
        let bytes = self.u.const_block_shared(self.b, bl, t)?;
        let w = (l.w().bits() / 8) as usize;
        if bytes.len() < n as usize * w {
            return None;
        }
        let out = self.new_slot_for(t);
        for k in 0..n as usize {
            let mut x = [0u8; 8];
            x[..w].copy_from_slice(&bytes[k * w..(k + 1) * w]);
            let r = self.iconst(i64::from_le_bytes(x));
            self.emit(Inst::Store(Mem::Int(w as u8, false), out, (k * w) as i32, r));
        }
        self.simd_consts.insert((out.0, 0), bytes);
        Some(Val::M(out, 0))
    }

    /// The element type of a `#[repr(simd)]` struct (see `simd_shape`).
    fn simd_elem_ty(&mut self, t: TyId) -> Option<TyId> {
        let TyKind::Adt(d, args, _) = self.kind(t) else { return None };
        let env = crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog);
        let f0 = self.u.tcx.field_ty(&self.u.prog, env, d, 0, 0, self.u.tcx.tys.list(args));
        Some(match self.kind(f0) {
            TyKind::Array(e, _) => e,
            _ => f0,
        })
    }

    /// The memory of an operand: (base, offset).
    fn simd_mem(&mut self, v: Val, t: TyId) -> (VReg, i32) {
        match v {
            Val::M(base, off) => (base, off),
            v => {
                let base = self.new_slot_for(t);
                self.store_val(base, 0, t, v);
                (base, 0)
            }
        }
    }

    /// A 16-byte-aligned slot of `chunks` * 16 bytes, its address.
    fn simd_scratch(&mut self, chunks: u32) -> VReg {
        let s = self.f.slot(16 * chunks, 16);
        self.f.slots[s as usize].born = self.coro.is_none();
        let d = self.f.vreg(Cls::I);
        self.emit(Inst::SlotAddr(d, s));
        d
    }

    /// An operand as `chunks` 16-byte pieces in memory: in place when its size is a whole number
    /// of chunks, else copied into a zeroed padded slot.
    fn simd_chunks(&mut self, v: Val, t: TyId, size: u32, chunks: u32) -> (VReg, i32) {
        let (base, off) = self.simd_mem(v, t);
        if size == 16 * chunks {
            return (base, off);
        }
        let pad = self.simd_scratch(chunks);
        let z = self.f.vreg(Cls::V128);
        self.emit(Inst::VConst(z, 0));
        for c in 0..chunks {
            self.emit(Inst::VStore(pad, 16 * c as i32, z));
        }
        let src = self.addr_add(base, off);
        self.emit(Inst::Copy(pad, src, size));
        (pad, 0)
    }

    /// The vector op of a lane-wise intrinsic on lanes `l`; None when the intrinsic is not
    /// lane-wise (or not defined for the lane type).
    fn simd_op(name: &str, l: Lane) -> Option<VX> {
        use Lane::*;
        let w = l.w();
        let h = NanBits::Host;
        let cmp = |c: Cond| match l {
            S(w) => VX::ICmp(c, w),
            U(w) => VX::UCmp(c, w),
            F(w) => VX::FCmp(c, w),
        };
        Some(match (name, l) {
            ("simd_add", F(_)) => VX::FAdd(w, h),
            ("simd_add", _) => VX::Add(w),
            ("simd_sub", F(_)) => VX::FSub(w, h),
            ("simd_sub", _) => VX::Sub(w),
            ("simd_mul", F(_)) => VX::FMul(w, h),
            ("simd_mul", _) => VX::MulLo(w),
            ("simd_div", F(_)) => VX::FDiv(w, h),
            ("simd_and", S(_) | U(_)) => VX::And,
            ("simd_or", S(_) | U(_)) => VX::Or,
            ("simd_xor", S(_) | U(_)) => VX::Xor,
            ("simd_shl", S(_) | U(_)) => VX::ShlV(w),
            ("simd_shr", S(_)) => VX::ShrSV(w),
            ("simd_shr", U(_)) => VX::ShrUV(w),
            ("simd_neg", F(_)) => VX::FNeg(w),
            ("simd_neg", S(_)) => VX::Neg(w),
            ("simd_fabs", F(_)) => VX::FAbs(w),
            ("simd_fsqrt", F(_)) => VX::FSqrt(w, h),
            ("simd_floor", F(_)) => VX::FRound(w, RoundTo::Floor),
            ("simd_ceil", F(_)) => VX::FRound(w, RoundTo::Ceil),
            ("simd_trunc", F(_)) => VX::FRound(w, RoundTo::Trunc),
            ("simd_round", F(_)) => VX::FRound(w, RoundTo::Away),
            ("simd_minimum_number_nsz", F(_)) => VX::FMinNum(w),
            ("simd_maximum_number_nsz", F(_)) => VX::FMaxNum(w),
            ("simd_saturating_add", S(_)) => VX::AddSatS(w),
            ("simd_saturating_add", U(_)) => VX::AddSatU(w),
            ("simd_saturating_sub", S(_)) => VX::SubSatS(w),
            ("simd_saturating_sub", U(_)) => VX::SubSatU(w),
            ("simd_eq", _) => cmp(Cond::Eq),
            ("simd_ne", _) => cmp(Cond::Ne),
            ("simd_lt", _) => cmp(Cond::Lt),
            ("simd_le", _) => cmp(Cond::Le),
            ("simd_gt", _) => cmp(Cond::Gt),
            ("simd_ge", _) => cmp(Cond::Ge),
            _ => return None,
        })
    }

    /// `core::intrinsics::simd::simd_*`; None when `name` is not one this lowering knows.
    pub(super) fn simd_intrinsic(&mut self, name: &str, vals: &[(Val, TyId)], t: TyId) -> Option<Val> {
        if let Some(v) = self.simd_per_lane(name, vals, t) {
            return Some(v);
        }
        let select = name == "simd_select";
        let any = [Lane::S(VW::S), Lane::U(VW::S), Lane::F(VW::S)];
        if !select && !any.iter().any(|&l| Self::simd_op(name, l).is_some()) {
            return None;
        }
        let arity = match name {
            "simd_select" => 3,
            "simd_neg" | "simd_fabs" | "simd_fsqrt" | "simd_floor" | "simd_ceil" | "simd_trunc" | "simd_round" => 1,
            _ => 2,
        };
        // the operand whose lanes the op reads (simd_select: if_true; a compare: its inputs)
        let vt = vals.get(if select { 1 } else { 0 })?.1;
        let Some((lane, n)) = self.simd_shape(vt) else {
            let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, vt);
            self.err(self.pos, format!("E0511: invalid monomorphization of `{}` intrinsic: expected SIMD input type, found non-SIMD `{}`", name, ts));
            return Some(self.dummy_val(t));
        };
        let op = if select { None } else { Self::simd_op(name, lane) };
        if !select && op.is_none() {
            let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, vt);
            self.err(self.pos, format!("E0511: invalid monomorphization of `{}` intrinsic: unsupported element type of `{}`", name, ts));
            return Some(self.dummy_val(t));
        }
        if vals.len() != arity {
            self.err(self.pos, format!("internal: `{}` takes {} arguments", name, arity));
            return Some(self.dummy_val(t));
        }
        // the result (and simd_select's mask) has the lane count and width of the input
        for ty in [Some(t), select.then_some(vals[0].1)].into_iter().flatten() {
            match self.simd_shape(ty) {
                Some((l2, n2)) if n2 == n && l2.w() == lane.w() => {}
                _ => {
                    let (ts, us) = (crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, ty), crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, vt));
                    self.err(self.pos, format!("E0511: invalid monomorphization of `{}` intrinsic: `{}` must have {} lanes of the width of `{}`", name, ts, n, us));
                    return Some(self.dummy_val(t));
                }
            }
        }
        let size = n * lane.w().bits() / 8;
        let chunks = size.div_ceil(16);
        let ops: Vec<(VReg, i32)> = vals.iter().map(|(v, ty)| self.simd_chunks(v.clone(), *ty, size, chunks)).collect();
        let out = if size == 16 * chunks { self.new_slot_for(t) } else { self.simd_scratch(chunks) };
        for c in 0..chunks as i32 {
            let regs: Vec<VReg> = ops
                .iter()
                .map(|&(base, off)| {
                    let r = self.f.vreg(Cls::V128);
                    self.emit(Inst::VLoad(r, base, off + 16 * c));
                    r
                })
                .collect();
            let d = self.f.vreg(Cls::V128);
            match op {
                None => self.emit(Inst::VSel(d, regs[0], regs[1], regs[2])),
                Some(op) => self.emit(Inst::VX(op, d, regs[0], if arity == 1 { regs[0] } else { regs[1] })),
            }
            self.emit(Inst::VStore(out, 16 * c, d));
        }
        if size == 16 * chunks {
            return Some(Val::M(out, 0));
        }
        let res = self.new_slot_for(t);
        self.emit(Inst::Copy(res, out, size));
        Some(Val::M(res, 0))
    }

    fn simd_err(&mut self, name: &str, what: &str, ty: TyId) -> Val {
        let ts = crate::typeck::ty_to_string(false, &self.u.prog, &self.u.tcx, ty);
        self.err(self.pos, format!("E0511: invalid monomorphization of `{}` intrinsic: {} `{}`", name, what, ts));
        self.dummy_val(ty)
    }

    fn lane_mem(l: Lane) -> Mem {
        let bytes = (l.w().bits() / 8) as u8;
        match l {
            Lane::S(_) => Mem::Int(bytes, true),
            Lane::U(_) => Mem::Int(bytes, false),
            Lane::F(VW::D) => Mem::F64,
            Lane::F(_) => Mem::F32,
        }
    }

    fn lane_cls(l: Lane) -> Cls {
        match l {
            Lane::F(VW::D) => Cls::F64,
            Lane::F(_) => Cls::F32,
            _ => Cls::I,
        }
    }

    fn lane_ity(l: Lane) -> IntTy {
        IntTy { bits: l.w().bits() as u8, signed: matches!(l, Lane::S(_)) }
    }

    /// lane `k` of the SIMD value at (base, off), extended by signedness (ints)
    fn lane_get(&mut self, base: VReg, off: i32, l: Lane, k: u32) -> VReg {
        let d = self.f.vreg(Self::lane_cls(l));
        self.emit(Inst::Load(Self::lane_mem(l), d, base, off + (k * l.w().bits() / 8) as i32));
        d
    }

    /// simd_shuffle with a known index vector over sources that are whole 16-byte chunks:
    /// per output chunk one VShuffle (pshufb / tbl) of the two source chunks its lanes come from.
    fn simd_shuffle_const(&mut self, idx: &Val, ops: &[(VReg, i32)], l: Lane, n: u32, m: u32, t: TyId) -> Option<Val> {
        let Val::M(ib, io) = *idx else { return None };
        let bytes = self.simd_consts.get(&(ib.0, io))?.clone();
        let lb = l.w().bits() / 8;
        let (src, dst) = (n * lb, m * lb);
        if src % 16 != 0 || dst % 16 != 0 || bytes.len() < 4 * m as usize {
            return None;
        }
        let lane = |j: u32| u32::from_le_bytes(bytes[4 * j as usize..4 * j as usize + 4].try_into().unwrap());
        if (0..m).any(|j| lane(j) >= 2 * n) {
            return None;
        }
        // per output chunk: its source chunks (of x ++ y) and byte indices
        let per = 16 / lb;
        let mut plan: Vec<([u32; 2], [u8; 16])> = Vec::new();
        for c in 0..dst / 16 {
            let mut chunks: Vec<u32> = Vec::new();
            let mut idx = [0u8; 16];
            for k in 0..per {
                let byte = lane(c * per + k) * lb;
                let sc = byte / 16;
                let at = match chunks.iter().position(|x| *x == sc) {
                    Some(p) => p,
                    None if chunks.len() < 2 => {
                        chunks.push(sc);
                        chunks.len() - 1
                    }
                    None => return None,
                };
                for b in 0..lb {
                    idx[(k * lb + b) as usize] = (16 * at as u32 + byte % 16 + b) as u8;
                }
            }
            let a = chunks[0];
            plan.push(([a, *chunks.get(1).unwrap_or(&a)], idx));
        }
        let half = src / 16;
        let load = |s: &mut Self, ch: u32| -> VReg {
            let (base, off) = if ch < half { ops[0] } else { ops[1] };
            let r = s.f.vreg(Cls::V128);
            s.emit(Inst::VLoad(r, base, off + 16 * (ch % half) as i32));
            r
        };
        let out = self.new_slot_for(t);
        for (c, (chunks, idx)) in plan.into_iter().enumerate() {
            let a = load(self, chunks[0]);
            let b = if chunks[1] == chunks[0] { a } else { load(self, chunks[1]) };
            let d = self.f.vreg(Cls::V128);
            self.emit(Inst::VShuffle(d, a, b, idx));
            self.emit(Inst::VStore(out, 16 * c as i32, d));
        }
        Some(Val::M(out, 0))
    }

    fn lane_put(&mut self, base: VReg, off: i32, l: Lane, k: u32, v: VReg) {
        self.emit(Inst::Store(Self::lane_mem(l), base, off + (k * l.w().bits() / 8) as i32, v));
    }

    /// The intrinsics computed lane by lane in scalar code: those without a vector op
    /// (integer div/rem, bit counts, byte/bit reversal, fused multiply-add, round-ties-even),
    /// lane moves (extract/insert/splat/shuffle), conversions, reductions and bit masks.
    /// None when `name` is not one of them.
    fn simd_per_lane(&mut self, name: &str, vals: &[(Val, TyId)], t: TyId) -> Option<Val> {
        let unary_bits = matches!(name, "simd_ctpop" | "simd_ctlz" | "simd_cttz" | "simd_bswap" | "simd_bitreverse" | "simd_round_ties_even");
        let known = unary_bits
            || matches!(name, "simd_div" | "simd_rem" | "simd_fma" | "simd_relaxed_fma" | "simd_cast" | "simd_as" | "simd_splat" | "simd_shuffle" | "simd_bitmask" | "simd_select_bitmask")
            || matches!(name, "simd_extract" | "simd_extract_dyn" | "simd_insert" | "simd_insert_dyn")
            || matches!(name, "simd_gather" | "simd_scatter" | "simd_masked_load" | "simd_masked_store" | "simd_arith_offset" | "simd_cast_ptr" | "simd_expose_provenance" | "simd_with_exposed_provenance")
            || name.starts_with("simd_reduce_");
        if !known || vals.is_empty() {
            return None;
        }
        // the SIMD operand that sets the lanes
        let vi = match name {
            "simd_splat" => None,
            "simd_select_bitmask" => Some(1),
            "simd_masked_load" | "simd_masked_store" => Some(2),
            _ => Some(0),
        };
        let shape_ty = match vi {
            Some(i) => vals[i].1,
            None => t,
        };
        let Some((l, n)) = self.simd_shape(shape_ty) else {
            return Some(self.simd_err(name, "expected SIMD type, found non-SIMD", shape_ty));
        };
        // integer div/rem by vector: rustc's float div is lane-wise (the vector path); float rem
        // has no scalar instruction here
        if name == "simd_div" && matches!(l, Lane::F(_)) {
            return None;
        }
        let float = matches!(l, Lane::F(_));
        let bits = l.w().bits() as i64;
        let ops: Vec<(VReg, i32)> = vals.iter().map(|(v, ty)| self.simd_mem(v.clone(), *ty)).collect();
        let scalar = |s: &mut Self, k: usize| s.to_leaves(vals[k].0.clone(), vals[k].1)[0];
        match name {
            "simd_div" | "simd_rem" | "simd_fma" | "simd_relaxed_fma" | "simd_ctpop" | "simd_ctlz" | "simd_cttz" | "simd_bswap" | "simd_bitreverse" | "simd_round_ties_even" => {
                let ok = match name {
                    "simd_div" | "simd_rem" | "simd_ctpop" | "simd_ctlz" | "simd_cttz" | "simd_bswap" | "simd_bitreverse" => !float,
                    _ => float,
                };
                if !ok {
                    return Some(self.simd_err(name, "unsupported element type of", shape_ty));
                }
                let out = self.new_slot_for(t);
                for k in 0..n {
                    let x = self.lane_get(ops[0].0, ops[0].1, l, k);
                    let r = match name {
                        "simd_div" | "simd_rem" => {
                            let y = self.lane_get(ops[1].0, ops[1].1, l, k);
                            let d = self.f.vreg(Cls::I);
                            self.emit(Inst::IBin(if name == "simd_div" { IOp::Div } else { IOp::Rem }, Self::lane_ity(l), d, x, y));
                            d
                        }
                        "simd_fma" | "simd_relaxed_fma" => {
                            let y = self.lane_get(ops[1].0, ops[1].1, l, k);
                            let z = self.lane_get(ops[2].0, ops[2].1, l, k);
                            let d = self.f.vreg(Self::lane_cls(l));
                            // relaxed_fma may round once or twice: rustc's x86-64 baseline (no FMA)
                            // multiplies and adds, AArch64 fuses; Relax gives the same bits as rustc
                            if name == "simd_relaxed_fma" && cfg!(target_arch = "x86_64") {
                                let p = self.f.vreg(Self::lane_cls(l));
                                self.emit(Inst::FBin(FOp::Mul, l.w() == VW::D, p, x, y));
                                self.emit(Inst::FBin(FOp::Add, l.w() == VW::D, d, p, z));
                            } else {
                                self.emit(Inst::FMulAdd(false, false, l.w() == VW::D, d, x, y, z));
                            }
                            d
                        }
                        "simd_round_ties_even" => {
                            let d = self.f.vreg(Self::lane_cls(l));
                            self.emit(Inst::FUnary(FUn::RoundEven, l.w() == VW::D, d, x));
                            d
                        }
                        _ => {
                            // 64-bit bit ops on the zero-extended lane, scaled to the lane width
                            let m = if bits == 64 { -1 } else { (1i64 << bits) - 1 };
                            let xz = self.and_k(x, m);
                            match name {
                                "simd_ctpop" => self.popcount64(xz),
                                "simd_ctlz" => {
                                    let c = self.clz64(xz);
                                    let k = self.iconst(64 - bits);
                                    self.ibin(IOp::Sub, false, c, k)
                                }
                                "simd_cttz" => {
                                    let top = self.iconst(if bits == 64 { 0 } else { 1i64 << bits });
                                    let y = self.ibin(IOp::Or, false, xz, top);
                                    self.ctz64(y)
                                }
                                "simd_bswap" => {
                                    let r = self.bswap64(xz);
                                    self.shr_k(r, 64 - bits)
                                }
                                _ => {
                                    let r = self.rev64(xz);
                                    self.shr_k(r, 64 - bits)
                                }
                            }
                        }
                    };
                    self.lane_put(out, 0, l, k, r);
                }
                Some(Val::M(out, 0))
            }
            "simd_cast" | "simd_as" | "simd_cast_ptr" | "simd_expose_provenance" | "simd_with_exposed_provenance" => {
                let Some((l2, n2)) = self.simd_shape(t) else {
                    return Some(self.simd_err(name, "expected SIMD return type, found non-SIMD", t));
                };
                if n2 != n {
                    return Some(self.simd_err(name, "the return type must have the lane count of the input, found", t));
                }
                let out = self.new_slot_for(t);
                for k in 0..n {
                    let x = self.lane_get(ops[0].0, ops[0].1, l, k);
                    let (fd, td) = (l.w() == VW::D, l2.w() == VW::D);
                    let conv = match (l, l2) {
                        (Lane::F(_), Lane::F(_)) if fd == td => None,
                        (Lane::F(_), Lane::F(_)) => Some(if fd { Conv::F64ToF32 } else { Conv::F32ToF64 }),
                        (Lane::F(_), _) => Some(Conv::FToInt(fd, Self::lane_ity(l2))),
                        (_, Lane::F(_)) => Some(Conv::IntToF(Self::lane_ity(l), td)),
                        _ => Some(Conv::IntToInt(Self::lane_ity(l2))),
                    };
                    let r = match conv {
                        None => x,
                        Some(c) => {
                            let d = self.f.vreg(Self::lane_cls(l2));
                            self.emit(Inst::Conv(c, d, x));
                            d
                        }
                    };
                    self.lane_put(out, 0, l2, k, r);
                }
                Some(Val::M(out, 0))
            }
            "simd_gather" | "simd_scatter" | "simd_masked_load" | "simd_masked_store" => {
                // lane k of the value <-> *addr[k] where mask lane k is set; a masked-off lane
                // reads its fallback / writes a dummy slot through the same access (no branch)
                let (mi, vi) = if name.starts_with("simd_masked") { (0, 2) } else { (2, 0) };
                let Some((ml, mn)) = self.simd_shape(vals[mi].1) else {
                    return Some(self.simd_err(name, "the mask must be a SIMD vector, found", vals[mi].1));
                };
                if mn != n || matches!(ml, Lane::F(_)) {
                    return Some(self.simd_err(name, "the mask must be an integer vector of the value's lane count, found", vals[mi].1));
                }
                let load = matches!(name, "simd_gather" | "simd_masked_load");
                let out = if load { Some(self.new_slot_for(t)) } else { None };
                let dummy = self.simd_scratch(1);
                let esize = l.w().bits() as i64 / 8;
                let base = if name.starts_with("simd_masked") { Some(scalar(self, 1)) } else { None };
                for k in 0..n {
                    let m = self.lane_get(ops[mi].0, ops[mi].1, ml, k);
                    let z = self.iconst(0);
                    let on = self.icmp(Cond::Ne, false, m, z);
                    let p = match base {
                        Some(b) => {
                            let o = self.iconst(k as i64 * esize);
                            self.ibin(IOp::Add, false, b, o)
                        }
                        None => self.lane_get(ops[1].0, ops[1].1, Lane::U(VW::D), k),
                    };
                    let off = self.addr_add(if load { ops[vi].0 } else { dummy }, if load { ops[vi].1 + (k as i64 * esize) as i32 } else { 0 });
                    let a = self.f.vreg(Cls::I);
                    self.emit(Inst::Sel(a, on, p, off));
                    if load {
                        let d = self.f.vreg(Self::lane_cls(l));
                        self.emit(Inst::Load(Self::lane_mem(l), d, a, 0));
                        self.lane_put(out.unwrap(), 0, l, k, d);
                    } else {
                        let x = self.lane_get(ops[vi].0, ops[vi].1, l, k);
                        self.emit(Inst::Store(Self::lane_mem(l), a, 0, x));
                    }
                }
                Some(match out {
                    Some(o) => Val::M(o, 0),
                    None => Val::L(vec![]),
                })
            }
            "simd_arith_offset" => {
                // lane k = ptr[k] + offset[k] * size_of pointee (wrapping)
                let pointee = match self.simd_elem_ty(vals[0].1).map(|e| self.kind(e)) {
                    Some(TyKind::Ptr(_, p)) => p,
                    _ => return Some(self.simd_err(name, "expected a SIMD vector of pointers, found", vals[0].1)),
                };
                let size = self.layout(pointee).size as i64;
                let out = self.new_slot_for(t);
                for k in 0..n {
                    let p = self.lane_get(ops[0].0, ops[0].1, Lane::U(VW::D), k);
                    let o = self.lane_get(ops[1].0, ops[1].1, Lane::S(VW::D), k);
                    let sz = self.iconst(size);
                    let o = self.ibin(IOp::Mul, false, o, sz);
                    let r = self.ibin(IOp::Add, false, p, o);
                    self.lane_put(out, 0, Lane::U(VW::D), k, r);
                }
                Some(Val::M(out, 0))
            }
            "simd_splat" => {
                let x = scalar(self, 0);
                let out = self.new_slot_for(t);
                for k in 0..n {
                    self.lane_put(out, 0, l, k, x);
                }
                Some(Val::M(out, 0))
            }
            "simd_extract" | "simd_extract_dyn" => {
                let i = scalar(self, 1);
                let a = self.simd_lane_addr(ops[0], l, i);
                let d = self.f.vreg(Self::lane_cls(l));
                self.emit(Inst::Load(Self::lane_mem(l), d, a, 0));
                Some(Val::L(vec![d]))
            }
            "simd_insert" | "simd_insert_dyn" => {
                let out = self.new_slot_for(t);
                let size = n * l.w().bits() / 8;
                let src = self.addr_add(ops[0].0, ops[0].1);
                self.emit(Inst::Copy(out, src, size));
                let i = scalar(self, 1);
                let x = scalar(self, 2);
                let a = self.simd_lane_addr((out, 0), l, i);
                self.emit(Inst::Store(Self::lane_mem(l), a, 0, x));
                Some(Val::M(out, 0))
            }
            "simd_shuffle" => {
                // out lane j = (x ++ y) lane idx[j]; idx is a SIMD value of u32
                let (idx_l, m) = match self.simd_shape(vals[2].1) {
                    Some((il @ Lane::U(VW::S), m)) => (il, m),
                    _ => return Some(self.simd_err(name, "the index must be a SIMD vector of u32, found", vals[2].1)),
                };
                match self.simd_shape(t) {
                    Some((l2, m2)) if l2 == l && m2 == m => {}
                    _ => return Some(self.simd_err(name, "the return type must have the input's element type and the index's lane count, found", t)),
                }
                let size = n * l.w().bits() / 8;
                // a constant index (an inline const): one byte shuffle of two source chunks per
                // 16-byte output chunk, when its bytes come from at most two chunks of x ++ y
                if let Some(v) = self.simd_shuffle_const(&vals[2].0, &ops, l, n, m, t) {
                    return Some(v);
                }
                let both = self.f.slot(2 * size, 16);
                self.f.slots[both as usize].born = self.coro.is_none();
                let xy = self.f.vreg(Cls::I);
                self.emit(Inst::SlotAddr(xy, both));
                let (sx, sy) = (self.addr_add(ops[0].0, ops[0].1), self.addr_add(ops[1].0, ops[1].1));
                self.emit(Inst::Copy(xy, sx, size));
                let hi = self.addr_add(xy, size as i32);
                self.emit(Inst::Copy(hi, sy, size));
                let out = self.new_slot_for(t);
                for j in 0..m {
                    let i = self.lane_get(ops[2].0, ops[2].1, idx_l, j);
                    let a = self.simd_lane_addr((xy, 0), l, i);
                    let d = self.f.vreg(Self::lane_cls(l));
                    self.emit(Inst::Load(Self::lane_mem(l), d, a, 0));
                    self.lane_put(out, 0, l, j, d);
                }
                Some(Val::M(out, 0))
            }
            "simd_bitmask" => {
                // bit k = lane k's top bit, in an integer or a byte array of the result's size
                let size = self.layout(t).size;
                if float || size > 8 || (size * 8) < n.min(64) {
                    return Some(self.simd_err(name, "cannot return a bit mask of", t));
                }
                let mut r = self.iconst(0);
                for k in 0..n {
                    let x = self.lane_get(ops[0].0, ops[0].1, l, k);
                    let b = self.shr_k(x, 63.min(bits - 1));
                    let b = self.and_k(b, 1);
                    let kk = self.iconst(k as i64);
                    let b = self.ibin(IOp::Shl, false, b, kk);
                    r = self.ibin(IOp::Or, false, r, b);
                }
                let out = self.new_slot_for(t);
                let tmp = self.simd_scratch(1);
                self.emit(Inst::Store(Mem::Int(8, false), tmp, 0, r));
                self.emit(Inst::Copy(out, tmp, size));
                Some(Val::M(out, 0))
            }
            "simd_select_bitmask" => {
                let msize = self.layout(vals[0].1).size;
                if msize > 8 {
                    return Some(self.simd_err(name, "cannot select by a bit mask of", vals[0].1));
                }
                let tmp = self.simd_scratch(1);
                let z = self.iconst(0);
                self.emit(Inst::Store(Mem::Int(8, false), tmp, 0, z));
                let ms = self.addr_add(ops[0].0, ops[0].1);
                self.emit(Inst::Copy(tmp, ms, msize));
                let mbits = self.f.vreg(Cls::I);
                self.emit(Inst::Load(Mem::Int(8, false), mbits, tmp, 0));
                let out = self.new_slot_for(t);
                for k in 0..n {
                    let b = self.shr_k(mbits, k as i64);
                    let b = self.and_k(b, 1);
                    let x = self.lane_get(ops[1].0, ops[1].1, l, k);
                    let y = self.lane_get(ops[2].0, ops[2].1, l, k);
                    let d = self.f.vreg(Self::lane_cls(l));
                    self.emit(Inst::Sel(d, b, x, y));
                    self.lane_put(out, 0, l, k, d);
                }
                Some(Val::M(out, 0))
            }
            _ => {
                // reductions
                let op = &name["simd_reduce_".len()..];
                let mut acc = match op {
                    "add_ordered" | "mul_ordered" => scalar(self, 1),
                    _ => self.lane_get(ops[0].0, ops[0].1, l, 0),
                };
                let from = if matches!(op, "add_ordered" | "mul_ordered") { 0 } else { 1 };
                let fw = l.w() == VW::D;
                if matches!(op, "any" | "all") {
                    // mask lanes are 0 or all ones: any = or of the lanes != 0, all = and
                    for k in 1..n {
                        let x = self.lane_get(ops[0].0, ops[0].1, l, k);
                        acc = self.ibin(if op == "any" { IOp::Or } else { IOp::And }, false, acc, x);
                    }
                    let z = self.iconst(0);
                    let r = self.icmp(Cond::Ne, false, acc, z);
                    return Some(Val::L(vec![r]));
                }
                for k in from..n {
                    let x = self.lane_get(ops[0].0, ops[0].1, l, k);
                    acc = match (op, float) {
                        // (rustc has no float min/max reductions: E0511 below)
                        ("add_ordered" | "add_unordered", true) | ("mul_ordered" | "mul_unordered", true) => {
                            let fop = if op.starts_with("add") { FOp::Add } else { FOp::Mul };
                            let d = self.f.vreg(Self::lane_cls(l));
                            self.emit(Inst::FBin(fop, fw, d, acc, x));
                            d
                        }
                        ("min" | "max", false) => {
                            let lt = self.icmp(Cond::Lt, matches!(l, Lane::S(_)), x, acc);
                            let d = self.f.vreg(Cls::I);
                            if op == "min" {
                                self.emit(Inst::Sel(d, lt, x, acc));
                            } else {
                                self.emit(Inst::Sel(d, lt, acc, x));
                            }
                            d
                        }
                        (_, false) => {
                            let iop = match op {
                                "add_ordered" | "add_unordered" => IOp::Add,
                                "mul_ordered" | "mul_unordered" => IOp::Mul,
                                "and" => IOp::And,
                                "or" => IOp::Or,
                                "xor" => IOp::Xor,
                                _ => return None,
                            };
                            let d = self.f.vreg(Cls::I);
                            self.emit(Inst::IBin(iop, Self::lane_ity(l), d, acc, x));
                            d
                        }
                        _ => return Some(self.simd_err(name, "unsupported element type of", shape_ty)),
                    };
                }
                Some(Val::L(vec![acc]))
            }
        }
    }

    /// The address of lane `i` (a runtime index) of the SIMD value at `at`.
    fn simd_lane_addr(&mut self, at: (VReg, i32), l: Lane, i: VReg) -> VReg {
        let base = self.addr_add(at.0, at.1);
        let sh = self.iconst((l.w().bits() / 8).trailing_zeros() as i64);
        let iz = self.and_k(i, 0xffff_ffff);
        let o = self.ibin(IOp::Shl, false, iz, sh);
        self.ibin(IOp::Add, false, base, o)
    }
}
