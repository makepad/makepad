// ported from N:rustc_mir_build/src/builder/expr/as_rvalue.rs @8d1a764 (division/remainder checks)
//! Constant initializers as data. A const/static whose initializer is built from literals,
//! tuples, arrays, struct and enum constructors, references (promoted into data), other
//! constants, layout queries and calls of simple `const fn`s is evaluated here, on its type-checked body,
//! straight into the bytes of its layout -- no lowering, optimisation or code generation of
//! an initializer function that runs once. Lookup tables (unicode properties, OpenType
//! language tags) are the common case. Anything outside this subset returns None and the
//! initializer is compiled and run as before (jit.rs run_init), so the subset only decides
//! speed, never semantics. Pointers in the result address the unit's stable data, static
//! storage, or permanent function entries. Const fn locals may be updated, but references
//! to their storage and writes through references still require the JIT.

use crate::ast::{BinOp, ExprId, ExprKind, ItemKind, LitKind, Stmt, UnOp};
use crate::jit::Unit;
use crate::program::DefKind;
use crate::typeck::{Body, Coerce, Res};
use crate::types::{TyId, TyKind};

/// Steps (expressions evaluated) per initializer before giving up: tables are large but
/// straight-line; a runaway (deep recursion through const fns) falls back to the JIT.
const STEP_BUDGET: u32 = 4_000_000;
const CALL_DEPTH: u32 = 32;

crate::gen_local! {
    /// the constants whose data this thread's nested evaluations are reading (const_data)
    /// (compiler TLS: the settle swap forgets the buffer with its generation)
    static CONSTS_READING: std::cell::RefCell<Vec<(crate::program::DefId, Vec<TyId>)>>;
}

struct Ctx<'a> {
    u: &'a mut Unit,
    steps: u32,
    depth: u32,
    trace: bool,
    reason: Option<String>,
}

/// Evaluates const/static `d` (instance `args`) into `buf` (its layout's size). Returns false
/// (nothing reliable written) when the initializer is outside the subset.
pub fn eval_into(u: &mut Unit, d: crate::program::DefId, args: &[TyId], buf: u64) -> bool {
    let options = &u.prog.session.clone().options;
    let _mem = crate::memstat::tag(crate::memstat::Tag::Ctfe);
    if off(options) {
        return false;
    }
    let init = match relax_types::const_eval::const_body(&u.prog, d) { Some(e) => e, None => return false };
    let t = match u.tcx.const_ty(&u.prog, d) {
        Some(t) => u.tcx.tys.subst_erasing_regions(t, args),
        None => return false,
    };
    let epoch = u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst);
    let body = match check_quiet(u, d, args) {
        Some(b) => b,
        None => return false,
    };
    let epoch = Some(epoch);
    if let Some(f) = u.prog.exec_profile_file(body.file) { f.note(crate::exec_profile::CTFE); }
    let mut cx = Ctx { u, steps: 0, depth: 0, trace: std::env::var_os("MAKEPAD_RUN_CTFE_TRACE").is_some(), reason: None };
    let mut env: Vec<Option<Vec<u8>>> = Vec::new();
    let v = match cx.eval(&body, &mut env, init) {
        Some(v) => v,
        None => {
            if cx.trace {
                eprintln!("ctfe: fallback {}: {}", cx.u.prog.def_path(d), cx.reason.as_deref().unwrap_or("unsupported initializer"));
            }
            crate::jit::ctfe_fallback_body(cx.u, epoch, body);
            return false;
        }
    };
    let size = cx.size(t) as usize;
    if v.len() != size {
        if cx.trace { eprintln!("ctfe: fallback {}: value size {}, layout size {}", cx.u.prog.def_path(d), v.len(), size); }
        crate::jit::ctfe_fallback_body(cx.u, epoch, body);
        return false;
    }
    unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), buf as *mut u8, size) };
    true
}

/// Evaluates the inline const block `bl` of the (instantiated) body `body` to the bytes of its
/// value of type `t`; None outside the subset (lowering then runs the block).
pub fn eval_const_block(u: &mut Unit, body: &Body, bl: crate::ast::BlockId, t: TyId) -> Option<Vec<u8>> {
    if off(&u.prog.session.clone().options) {
        return None;
    }
    let mut cx = Ctx { u, steps: 0, depth: 0, trace: false, reason: None };
    let mut env: Vec<Option<Vec<u8>>> = Vec::new();
    let v = cx.block(body, &mut env, bl)?;
    (v.len() == cx.size(t) as usize).then_some(v)
}

/// A successful N01 body is complete: constant queries resolve synchronously or unwind.
fn check_quiet(u: &mut Unit, d: crate::program::DefId, args: &[TyId]) -> Option<std::sync::Arc<Body>> {
    let _o = crate::clock::origin("ctfe");
    let b = std::sync::Arc::new(crate::typeck::instantiate_body(
        &crate::tcx::EvalCx::new(&u.prog, &u.tcx, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), &crate::tcx::EvalState::new()),
        &u.prog, &u.tcx, d, args));
    if !b.errors.is_empty() || !b.deferred_misses.is_empty() { return None; }
    Some(b)
}

/// MAKEPAD_RUN_CTFE=check: evaluate here AND compile and run the initializer, compare the two values
/// by type (pointers by what they point to, padding ignored); a difference is reported.
pub fn check_mode(options: &crate::options::Options) -> bool {
    options.ctfe == crate::options::Ctfe::Check
}

/// Do the values at `a` and `b` of type `t` mean the same? (for check_mode)
pub fn same_value(u: &mut Unit, t: TyId, a: u64, b: u64, depth: u32) -> bool {
    if depth > 16 {
        return true;
    }
    // N:rustc_ty_utils/src/layout.rs:63-75: the inspected type is the
    // normalized layout type, not the unevaluated array-length alias.
    let t = u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), t);
    let l = u.layout_of(t);
    let bytes = |p: u64, n: usize| unsafe { std::slice::from_raw_parts(p as *const u8, n) };
    let rd = |p: u64| unsafe { (p as *const u64).read_unaligned() };
    match u.tcx.tys.kind(t).clone() {
        // A raw pointer can be dangling (MAP_FAILED, null, a function address). Never
        // dereference it just to compare constants.
        TyKind::Ptr(..) => bytes(a, l.size as usize) == bytes(b, l.size as usize),
        TyKind::Ref(_, inner, _) => {
            let (pa, pb) = (rd(a), rd(b));
            match u.tcx.tys.kind(inner).clone() {
                TyKind::Str => {
                    let (na, nb) = (rd(a + 8), rd(b + 8));
                    na == nb && (na == 0 || bytes(pa, na as usize) == bytes(pb, nb as usize))
                }
                TyKind::Slice(et) => {
                    let (na, nb) = (rd(a + 8), rd(b + 8));
                    if na != nb {
                        return false;
                    }
                    let es = u.layout_of(et).size as u64;
                    (0..na).all(|i| same_value(u, et, pa + i * es, pb + i * es, depth + 1))
                }
                TyKind::Dyn(..) => pa == pb && rd(a + 8) == rd(b + 8),
                _ => {
                    if pa == 0 || pb == 0 || u.lay.is_unsized(&crate::layout::LayoutCx::new(&u.prog), &mut u.tcx, inner) {
                        return pa == pb;
                    }
                    pa == pb || same_value(u, inner, pa, pb, depth + 1)
                }
            }
        }
        TyKind::Array(et, n) => {
            let n = n.try_to_target_usize(&u.tcx.tys).expect("normalized array length");
            let es = u.layout_of(et).size as u64;
            (0..n.min(1 << 24)).all(|i| same_value(u, et, a + i * es, b + i * es, depth + 1))
        }
        TyKind::Tuple(ts) => u.tcx.tys.list(ts).to_vec().into_iter().enumerate().all(|(i, ft)| {
            let off = l.fields.get(i).copied().unwrap_or(0) as u64;
            same_value(u, ft, a + off, b + off, depth + 1)
        }),
        TyKind::Adt(d, args, _) => {
            let adt = match u.tcx.adts.get(&d).cloned() {
                Some(x) => x,
                None => return bytes(a, l.size as usize) == bytes(b, l.size as usize),
            };
            if adt.is_union {
                return bytes(a, l.size as usize) == bytes(b, l.size as usize);
            }
            let vi = if adt.is_enum {
                // same tag bytes, then that variant's fields
                if let Some((toff, relax_backend::rir::Mem::Int(n, _))) = l.tag {
                    if bytes(a + toff as u64, n as usize) != bytes(b + toff as u64, n as usize) {
                        return false;
                    }
                    let mut raw = [0u8; 16];
                    raw[..n as usize].copy_from_slice(bytes(a + toff as u64, n as usize));
                    let Some(disc) = crate::lower::decode_tag_const(u, i128::from_le_bytes(raw), &l, &adt) else { return false; };
                    match adt.discriminants(&u.tcx, &u.prog).iter().position(|(_, v)| v.signed_value() == disc) {
                        Some(v) => v,
                        None => return true,
                    }
                } else {
                    0
                }
            } else {
                0
            };
            let fields = match adt.variants.get(vi) {
                Some(v) => v.fields.clone(),
                None => return true,
            };
            for fi in 0..fields.len() {
                let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args));
                let off = if adt.is_enum { l.variant_fields.get(vi).and_then(|v| v.get(fi)).copied() } else { l.fields.get(fi).copied() };
                let off = match off {
                    Some(o) => o as u64,
                    None => continue,
                };
                if !same_value(u, ft, a + off, b + off, depth + 1) {
                    return false;
                }
            }
            true
        }
        _ => bytes(a, l.size as usize) == bytes(b, l.size as usize),
    }
}

/// Marks the bytes a value of type `t` at `a` defines (mask[off..] = 1): scalars and pointers
/// whole, struct/tuple/array fields, an enum's tag and its ACTIVE variant's fields; padding and
/// inactive-variant bytes stay 0. Unions and unknown types count as fully defined.
pub fn defined_mask(u: &mut Unit, t: TyId, a: u64, mask: &mut [u8], off: usize, depth: u32) {
    // N:rustc_ty_utils/src/layout.rs:63-75, as in same_value above.
    let t = u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), t);
    let l = u.layout_of(t);
    let size = l.size as usize;
    let all = |mask: &mut [u8]| {
        for b in mask.iter_mut().skip(off).take(size) {
            *b = 1;
        }
    };
    if depth > 32 {
        return all(mask);
    }
    let bytes = |p: u64, n: usize| unsafe { std::slice::from_raw_parts(p as *const u8, n) };
    match u.tcx.tys.kind(t).clone() {
        TyKind::Array(et, n) => {
            let n = n.try_to_target_usize(&u.tcx.tys).expect("normalized array length");
            let es = u.layout_of(et).size as usize;
            for i in 0..(n as usize).min(1 << 20) {
                defined_mask(u, et, a + (i * es) as u64, mask, off + i * es, depth + 1);
            }
        }
        TyKind::Tuple(ts) => {
            for (i, ft) in u.tcx.tys.list(ts).to_vec().into_iter().enumerate() {
                let fo = l.fields.get(i).copied().unwrap_or(0) as usize;
                defined_mask(u, ft, a + fo as u64, mask, off + fo, depth + 1);
            }
        }
        TyKind::Adt(d, args, _) => {
            let adt = match u.tcx.adts.get(&d).cloned() {
                Some(x) if !x.is_union => x,
                _ => return all(mask),
            };
            let vi = if adt.is_enum {
                match l.tag {
                    Some((toff, relax_backend::rir::Mem::Int(n, _))) => {
                        for b in mask.iter_mut().skip(off + toff as usize).take(n as usize) {
                            *b = 1;
                        }
                        let mut raw = [0u8; 16];
                        raw[..n as usize].copy_from_slice(bytes(a + toff as u64, n as usize));
                        let Some(disc) = crate::lower::decode_tag_const(u, i128::from_le_bytes(raw), &l, &adt) else { return all(mask); };
                        match adt.discriminants(&u.tcx, &u.prog).iter().position(|(_, v)| v.signed_value() == disc) {
                            Some(v) => v,
                            None => return all(mask),
                        }
                    }
                    None => 0,
                    _ => return all(mask),
                }
            } else {
                0
            };
            let fields = match adt.variants.get(vi) {
                Some(v) => v.fields.clone(),
                None => return all(mask),
            };
            for fi in 0..fields.len() {
                let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, vi as u32, fi as u32, u.tcx.tys.list(args));
                let fo = if adt.is_enum { l.variant_fields.get(vi).and_then(|v| v.get(fi)).copied() } else { l.fields.get(fi).copied() };
                if let Some(fo) = fo {
                    defined_mask(u, ft, a + fo as u64, mask, off + fo as usize, depth + 1);
                }
            }
        }
        _ => all(mask),
    }
}

/// rustc_const_eval interpret/validity.rs, the scalar part: a constant's final value holds only
/// valid bools and chars (arrays, tuples and structs are walked). The message on the first invalid one.
pub fn invalid_scalar(u: &mut Unit, t: TyId, a: u64, depth: u32) -> Option<String> {
    if depth > 16 { return None; }
    let t = u.tcx.normalize_erasing_regions(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), t);
    let l = u.layout_of(t);
    match u.tcx.tys.kind(t).clone() {
        TyKind::Bool => {
            let v = unsafe { *(a as *const u8) };
            (v > 1).then(|| format!("constructing invalid value of type bool: encountered {:#04x}, but expected a boolean", v))
        }
        TyKind::Char => {
            let v = unsafe { (a as *const u32).read_unaligned() };
            (char::from_u32(v).is_none()).then(|| format!("constructing invalid value of type char: encountered {:#x}, but expected a valid unicode scalar value (in `0..=0x10FFFF` but not in `0xD800..=0xDFFF`)", v))
        }
        TyKind::Array(et, n) => {
            let n = n.try_to_target_usize(&u.tcx.tys)?;
            let es = u.layout_of(et).size as u64;
            for i in 0..n.min(1 << 16) {
                if let Some(m) = invalid_scalar(u, et, a + i * es, depth + 1) { return Some(m); }
            }
            None
        }
        TyKind::Tuple(ts) => {
            for (i, ft) in u.tcx.tys.list(ts).to_vec().into_iter().enumerate() {
                let fo = l.fields.get(i).copied().unwrap_or(0) as u64;
                if let Some(m) = invalid_scalar(u, ft, a + fo, depth + 1) { return Some(m); }
            }
            None
        }
        TyKind::Adt(d, args, _) => {
            let adt = u.tcx.adts.get(&d).cloned()?;
            if adt.is_enum || adt.is_union { return None; }
            for fi in 0..adt.variants[0].fields.len() {
                let ft = u.tcx.field_ty(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, 0, fi as u32, u.tcx.tys.list(args));
                let fo = l.fields.get(fi).copied()? as u64;
                if let Some(m) = invalid_scalar(u, ft, a + fo, depth + 1) { return Some(m); }
            }
            None
        }
        _ => None,
    }
}

/// Zeroes the bytes of the value of type `t` at `a` that the value does not define (padding,
/// inactive enum payloads), as rustc's constant allocations are emitted: a constant's bytes are
/// then the same whichever way its initializer ran (evaluated here or compiled and run, whose
/// copies carry whatever the stack held) -- the disk cache keys data objects by their bytes.
pub fn zero_undefined(u: &mut Unit, t: TyId, a: u64) {
    let size = u.layout_of(t).size as usize;
    if size == 0 {
        return;
    }
    let mut mask = vec![0u8; size];
    defined_mask(u, t, a, &mut mask, 0, 0);
    let p = a as *mut u8;
    for (i, m) in mask.iter().enumerate() {
        if *m == 0 {
            unsafe { *p.add(i) = 0 };
        }
    }
}

/// MAKEPAD_RUN_CTFE=0: every initializer is compiled and run (the reference behaviour).
fn off(options: &crate::options::Options) -> bool {
    options.ctfe == crate::options::Ctfe::Off
}

fn put(v: &mut [u8], off: usize, src: &[u8]) -> Option<()> {
    v.get_mut(off..off + src.len())?.copy_from_slice(src);
    Some(())
}

fn rd_u64(v: &[u8], off: usize) -> Option<u64> {
    Some(u64::from_le_bytes(v.get(off..off + 8)?.try_into().ok()?))
}

impl<'a> Ctx<'a> {
    fn kind(&self, t: TyId) -> TyKind {
        self.u.tcx.tys.kind(t).clone()
    }
    fn layout(&mut self, t: TyId) -> &'static crate::layout::Layout {
        let u = &mut *self.u;
        u.layout_of(t)
    }
    fn size(&mut self, t: TyId) -> u32 {
        self.layout(t).size
    }
    /// Integer width and signedness of a primitive (bool 1 unsigned, char 4 unsigned)
    fn int_of(&self, t: TyId) -> Option<(usize, bool)> {
        match self.u.tcx.tys.kind(t) {
            TyKind::Int(p) => {
                let (b, s) = crate::types::Types::int_info(*p);
                Some((b as usize, s))
            }
            TyKind::Bool => Some((1, false)),
            TyKind::Char => Some((4, false)),
            _ => None,
        }
    }

    /// The value of `e` after the coercion recorded on it.
    fn eval(&mut self, b: &Body, env: &mut Vec<Option<Vec<u8>>>, e: ExprId) -> Option<Vec<u8>> {
        let result = self.eval_coerced(b, env, e);
        if result.is_none() && self.trace && self.reason.is_none() {
            let file = &self.u.prog.files[b.file as usize];
            let ex = file.ast.expr(e);
            let source: String = String::from_utf8_lossy(file.bytes(ex.lo, ex.hi)).chars().take(200).collect();
            let call = match ex.kind {
                ExprKind::Call(f, _) => match self.kind(b.ty(f)) {
                    TyKind::FnDef(d, _, _) => Some(d),
                    _ => None,
                },
                ExprKind::MethodCall { .. } => b.methods.get(&e.0).map(|m| m.def),
                _ => None,
            };
            self.reason = Some(format!("{}: {:?}, call {:?}, coercion {:?}, source {:?}", self.u.prog.def_path(b.def), ex.kind, call.map(|d| self.u.prog.def_path(d)), b.coerce(e.0), source));
        }
        result
    }

    fn eval_coerced(&mut self, b: &Body, env: &mut Vec<Option<Vec<u8>>>, e: ExprId) -> Option<Vec<u8>> {
        self.steps += 1;
        if self.steps > STEP_BUDGET {
            if self.trace { self.reason = Some("expression step budget exhausted".into()); }
            return None;
        }
        let v = self.eval_raw(b, env, e)?;
        match b.coerce(e.0) {
            None | Some(Coerce::MutToConst | Coerce::UnsafeFn | Coerce::RefToRaw(_)) => Some(v),
            Some(Coerce::ReifyFn) => self.fn_pointer(b.ty(e)),
            Some(Coerce::ClosureFn) => {
                // the closure's FnOnce instance, as lowering reifies it (lower.rs Coerce::ClosureFn,
                // rustc_codegen_ssa/src/mir/rvalue.rs:536-550): its body takes the environment first
                let instance = crate::instance::Instance::resolve_closure(&self.u.tcx, &self.u.prog, b.ty(e), relax_types::upvar::ClosureKind::FnOnce);
                let id = self.u.instance_fn_id(instance);
                Some(self.u.entry(id).to_le_bytes().to_vec())
            }
            Some(Coerce::Unsize) => {
                // &[T; N] -> &[T]: the thin pointer gets N
                let n = match self.kind(b.ty(e)) {
                    TyKind::Ref(_, a, _) | TyKind::Ptr(_, a) => match self.kind(a) {
                        TyKind::Array(_, n) if n.try_to_target_usize(&self.u.tcx.tys).is_some() => n.try_to_target_usize(&self.u.tcx.tys).unwrap(),
                        _ => return None,
                    },
                    _ => return None,
                };
                if v.len() != 8 {
                    return None;
                }
                let mut f = v;
                f.extend_from_slice(&n.to_le_bytes());
                Some(f)
            }
            Some(_) => None,
        }
    }

    fn eval_raw(&mut self, b: &Body, env: &mut Vec<Option<Vec<u8>>>, e: ExprId) -> Option<Vec<u8>> {
        let t = b.ty(e);
        let file = b.file as usize;
        let kind = self.u.prog.files[file].ast.expr(e).kind.clone();
        match kind {
            ExprKind::Closure(_) => match self.kind(t) {
                TyKind::Closure(_, _, crate::types::ClosureArgs { tupled_upvars_ty: captures, .. }) if matches!(self.kind(captures), TyKind::Tuple(ref ts) if ts.is_empty()) => Some(Vec::new()),
                _ => None,
            },
            ExprKind::Paren(x) => self.eval(b, env, x),
            ExprKind::Lit(k) => self.lit(b, e, k, t),
            ExprKind::Unary(op, x) => {
                if b.binops.contains_key(&e.0) || b.methods.contains_key(&e.0) {
                    return None;
                }
                // Rust permits the minimum signed value as a negated literal, although
                // its positive magnitude does not fit the signed type.
                if op == UnOp::Neg {
                    if let Some((n, true)) = self.int_of(t) {
                        let literal = crate::lit::strip_paren(&self.u.prog.files[file].ast, x);
                        if matches!(self.u.prog.files[file].ast.expr(literal).kind, ExprKind::Lit(LitKind::Int)) {
                            let ex = self.u.prog.files[file].ast.expr(literal);
                            let text = std::str::from_utf8(self.u.prog.files[file].bytes(ex.lo, ex.hi)).ok()?;
                            let magnitude = crate::const_fold::parse_int_lit(text)?;
                            if magnitude > 1u128 << (n * 8 - 1) { return None; }
                            return Some(store_int(magnitude.wrapping_neg() as i128, n));
                        }
                    }
                }
                let v = self.eval(b, env, x)?;
                match op {
                    UnOp::Neg => match self.kind(t) {
                        TyKind::Int(_) => {
                            let (n, s) = self.int_of(t)?;
                            let x = load_int(&v, n, s);
                            let r = if s { (x as i128).checked_neg()? } else { return None };
                            if !fits(r, n, s) {
                                return None;
                            }
                            Some(store_int(r, n))
                        }
                        TyKind::Float(p) => {
                            if p == crate::program::Prim::F32 {
                                let x = f32::from_bits(u32::from_le_bytes(v.get(..4)?.try_into().ok()?));
                                Some((-x).to_bits().to_le_bytes().to_vec())
                            } else {
                                let x = f64::from_bits(u64::from_le_bytes(v.get(..8)?.try_into().ok()?));
                                Some((-x).to_bits().to_le_bytes().to_vec())
                            }
                        }
                        _ => None,
                    },
                    UnOp::Not => match self.kind(t) {
                        TyKind::Bool => Some(vec![(v.first()? ^ 1) & 1]),
                        TyKind::Int(_) => Some(v.iter().map(|x| !x).collect()),
                        _ => None,
                    },
                    UnOp::Deref => {
                        if !matches!(self.kind(b.ty(x)), TyKind::Ref(..)) { return None; }
                        // `*r` of a reference into data: the pointee's bytes
                        let n = self.size(t) as usize;
                        let p = rd_u64(&v, 0)?;
                        if p == 0 {
                            return None;
                        }
                        let mut out = vec![0u8; n];
                        unsafe { std::ptr::copy_nonoverlapping(p as *const u8, out.as_mut_ptr(), n) };
                        Some(out)
                    }
                }
            }
            ExprKind::Binary(op, x, y) => {
                if b.binops.contains_key(&e.0) {
                    return None;
                }
                self.binary(b, env, op, x, y)
            }
            ExprKind::Assign(x, y) | ExprKind::AssignOp(_, x, y) => {
                // Assignment (and primitive compound assignment) evaluates the RHS first.
                // Only local places: no writes through references or into statics.
                let mut v = self.eval(b, env, y)?;
                let (li, off) = self.local_place(b, env, x)?;
                let xt = b.ty(x);
                let size = self.size(xt) as usize;
                if self.u.tcx.needs_drop(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), xt) { return None; }
                if let ExprKind::AssignOp(op, _, _) = kind {
                    if b.binops.contains_key(&e.0) { return None; }
                    // A generic body's overloaded compound assignment retains its
                    // evaluation order after monomorphization. Leave it to lowering.
                    if !b.args.is_empty() { return None; }
                    let lhs = env.get(li)?.as_ref()?.get(off..off + size)?.to_vec();
                    v = self.binary_values(op, &lhs, xt, &v, b.ty(y))?;
                }
                if v.len() != size { return None; }
                if env.len() <= li { env.resize(li + 1, None); }
                if off == 0 && size == self.size(b.locals.get(li)?.ty) as usize {
                    env[li] = Some(v);
                } else {
                    put(env[li].as_mut()?, off, &v)?;
                }
                Some(Vec::new())
            }
            ExprKind::Cast(x, _) => {
                let v = self.eval(b, env, x)?;
                self.cast(&v, b.ty(x), t)
            }
            ExprKind::Tuple(xs) => {
                let l = self.layout(t);
                let mut out = vec![0u8; l.size as usize];
                for (i, x) in self.u.prog.files[file].ast.list(xs).to_vec().iter().enumerate() {
                    let v = self.eval(b, env, *x)?;
                    put(&mut out, *l.fields.get(i)? as usize, &v)?;
                }
                Some(out)
            }
            ExprKind::Array(xs) => {
                let (et, n) = match self.kind(t) {
                    TyKind::Array(et, n) if n.try_to_target_usize(&self.u.tcx.tys).is_some() => (et, n.try_to_target_usize(&self.u.tcx.tys).unwrap()),
                    _ => return None,
                };
                if n as usize != xs.len() {
                    return None;
                }
                let es = self.size(et) as usize;
                let mut out = Vec::with_capacity(es * xs.len());
                for x in &self.u.prog.files[file].ast.list(xs).to_vec() {
                    let v = self.eval(b, env, *x)?;
                    if v.len() != es {
                        return None;
                    }
                    out.extend_from_slice(&v);
                }
                Some(out)
            }
            ExprKind::Repeat(x, _) => {
                let n = match self.kind(t) {
                    TyKind::Array(_, n) if n.try_to_target_usize(&self.u.tcx.tys).is_some() => n.try_to_target_usize(&self.u.tcx.tys).unwrap() as usize,
                    _ => return None,
                };
                let v = self.eval(b, env, x)?;
                if v.len().checked_mul(n)? > 1 << 28 {
                    return None;
                }
                Some(v.repeat(n))
            }
            ExprKind::Struct(row) if !self.u.prog.files[file].ast.row(row).rest => {
                let crate::ast::StructRow { fields, base, .. } = *self.u.prog.files[file].ast.row(row);
                let d = match b.res.get(&e.0) {
                    Some(Res::Def(d)) => *d,
                    _ => return None,
                };
                let vi = if self.u.prog.def(d).kind == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
                let mut vals = Vec::new();
                for fe in &self.u.prog.files[file].ast.list(fields).to_vec() {
                    let txt = self.u.prog.files[file].text(fe.name).to_string();
                    let name = if txt.as_bytes().first()?.is_ascii_digit() {
                        txt.parse::<u32>().ok()? | 0x8000_0000
                    } else {
                        self.u.prog.syms.get(&txt)?
                    };
                    let fi = self.field_index(t, vi, name)?;
                    let v = self.eval(b, env, fe.expr)?;
                    vals.push((fi, v));
                }
                if let Some(base) = base.get() {
                    // Explicit fields are evaluated before the update base (Reference
                    // expressions/struct-expr); only omitted fields are copied from it.
                    if b.ty(base) != t { return None; }
                    let v = self.eval(b, env, base)?;
                    let (adt, args) = match self.kind(t) {
                        TyKind::Adt(d, args, _) => (d, args),
                        _ => return None,
                    };
                    let a = self.u.tcx.adts.get(&adt)?;
                    if a.is_enum || a.is_union { return None; }
                    let nf = a.variants.first()?.fields.len();
                    for fi in 0..nf as u32 {
                        if vals.iter().any(|(i, _)| *i == fi) { continue; }
                        let ft = self.u.tcx.field_ty(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), adt, 0, fi, self.u.tcx.tys.list(args));
                        let off = self.struct_field_off(t, fi)? as usize;
                        let size = self.size(ft) as usize;
                        vals.push((fi, v.get(off..off + size)?.to_vec()));
                    }
                }
                self.build_adt(t, vi, vals)
            }
            ExprKind::Path(_) => match b.res.get(&e.0).copied() {
                Some(Res::Local(li)) => env.get(li as usize)?.clone(),
                Some(Res::Def(d)) => {
                    let k = self.u.prog.def(d).kind;
                    match k {
                        DefKind::Const | DefKind::AssocConst => {
                            let args = b.res_args.get(&e.0).map_or_else(Vec::new, |&l| self.u.tcx.tys.list(l).to_vec());
                            let a = self.const_data(d, args)?;
                            let n = self.size(t) as usize;
                            let mut out = vec![0u8; n];
                            unsafe { std::ptr::copy_nonoverlapping(a as *const u8, out.as_mut_ptr(), n) };
                            Some(out)
                        }
                        DefKind::Static => {
                            // Mutable/interior-mutable static reads need a full interpreter.
                            let def = self.u.prog.def(d);
                            if !matches!(self.u.prog.files[def.file as usize].ast.item(def.item).kind, ItemKind::Static(false, _, e) if e.is_some())
                                || !self.u.tcx.is_freeze(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), t) { return None; }
                            let a = self.u.ctfe_static_addr(d)?;
                            // Only completed static initializers have a captured value.
                            // An address may already exist during a cyclic initialization.
                            let sym = self.u.static_sym_at(a)?;
                            let n = self.size(t) as usize;
                            self.u.syms.initial_data.lock().unwrap_or_else(|e| e.into_inner()).get(&sym.id)?.get(..n).map(<[u8]>::to_vec)
                        }
                        DefKind::Fn | DefKind::AssocFn | DefKind::ForeignFn if matches!(self.kind(t), TyKind::FnDef(..)) => Some(Vec::new()),
                        // a unit struct / fieldless variant
                        DefKind::Variant | DefKind::Struct if matches!(self.kind(t), TyKind::Adt(..)) => {
                            let vi = if k == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
                            self.build_adt(t, vi, Vec::new())
                        }
                        _ => None,
                    }
                }
                _ => None,
            },
            ExprKind::MethodCall(recv, row) => {
                let args = self.u.prog.files[file].ast.row(row).args;
                let m = b.methods.get(&e.0)?;
                let (mut rv, rt) = match (m.derefs, m.autoref) {
                    (0, 0) => (self.eval(b, env, recv)?, b.ty(recv)),
                    (0, 1) => (self.borrow(b, env, recv)?, b.ty(recv)),
                    (1, 1) if !b.ov_deref(recv.0, 0).is_some() => {
                        let TyKind::Ref(_, inner, _) = self.kind(b.ty(recv)) else { return None };
                        (self.eval(b, env, recv)?, inner)
                    }
                    _ => return None,
                };
                // Array receivers use slice methods through a shared unsizing borrow.
                if m.autoref == 1 {
                    if let TyKind::Array(_, n) = self.kind(rt) {
                        let sig = self.u.tcx.fn_sig(&self.u.prog, m.def)?;
                        let st = self.u.tcx.tys.subst_erasing_regions(*sig.params.first()?, self.u.tcx.tys.list(m.args));
                        if matches!(self.kind(st), TyKind::Ref(_, p, _) if matches!(self.kind(p), TyKind::Slice(_))) {
                            let n = n.try_to_target_usize(&self.u.tcx.tys)?;
                            if rv.len() != 8 { return None; }
                            rv.extend_from_slice(&n.to_le_bytes());
                        }
                    }
                }
                let mut vals = vec![rv];
                for a in self.u.prog.files[file].ast.list(args).to_vec() {
                    vals.push(self.eval(b, env, a)?);
                }
                self.call_const_fn(m.def, self.u.tcx.tys.list(m.args).to_vec(), vals, t)
            }
            ExprKind::Call(f, args) => {
                if b.legacy_args.contains_key(&e.0) || b.call_derefs(e.0).is_some() {
                    return None;
                }
                let fx = crate::lit::strip_paren(&self.u.prog.files[file].ast, f);
                if let Some(Res::Def(d)) = b.res.get(&fx.0).copied() {
                    let k = self.u.prog.def(d).kind;
                    if matches!(k, DefKind::Variant | DefKind::Struct) {
                        // tuple struct / tuple variant constructor
                        let vi = if k == DefKind::Variant { self.u.prog.def(d).sub } else { 0 };
                        let mut vals = Vec::new();
                        for (i, a) in self.u.prog.files[file].ast.list(args).to_vec().iter().enumerate() {
                            vals.push((i as u32, self.eval(b, env, *a)?));
                        }
                        return self.build_adt(t, vi, vals);
                    }
                }
                match self.kind(b.ty(f)) {
                    TyKind::FnDef(d0, gargs0, _) => {
                        let mut vals = Vec::new();
                        for a in &self.u.prog.files[file].ast.list(args).to_vec() {
                            vals.push(self.eval(b, env, *a)?);
                        }
                        let instance = self.u.tcx.resolve_instance_raw(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), d0, self.u.tcx.tys.list(gargs0)).expect("established instance dispatch");
                        let relax_types::instance::InstanceKind::Item(d) = instance.def else { return None };
                        if self.u.prog.def(d).kind == DefKind::Closure { return None; }
                        self.call_const_fn(d, instance.compact_args(&self.u.tcx), vals, t)
                    }
                    _ => None,
                }
            }
            ExprKind::AddrOf(false, false, x) => {
                let x = crate::lit::strip_paren(&self.u.prog.files[file].ast, x);
                if let Some(Res::Def(d)) = b.res.get(&x.0).copied() {
                    if self.u.prog.def(d).kind == DefKind::Static {
                        let a = self.u.ctfe_static_addr(d)?;
                        return Some(a.to_le_bytes().to_vec());
                    }
                }
                if self.owned_local_place(b, x) { return None; }
                let xt = b.ty(x);
                if self.u.tcx.is_freeze(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), xt) {
                    return self.borrow(b, env, x);
                }
                // a promoted rvalue (or a constant's place): its bytes in data, by address
                let l = self.layout(xt);
                let v = self.eval(b, env, x)?;
                if v.len() != l.size as usize || l.uninhabited {
                    return None;
                }
                if l.size == 0 {
                    return Some((l.align.max(1) as u64).to_le_bytes().to_vec());
                }
                // Interior-mutable promoted values need writable memory, as in lowering.
                let a = self.u.rw_data_obj(l.size as u64, l.align.max(8) as u64, crate::syms::DataKind::Typed(xt));
                unsafe { std::ptr::copy_nonoverlapping(v.as_ptr(), a as *mut u8, v.len()) };
                Some(a.to_le_bytes().to_vec())
            }
            ExprKind::ConstBlock(bl) => self.offset_of(b, e, bl).or_else(|| self.block(b, env, bl)),
            ExprKind::Block(bl, l) if l.is_none() => self.block(b, env, bl),
            ExprKind::Unsafe(bl) => self.block(b, env, bl),
            ExprKind::While(row) => {
                // `while cond { body }` (no label, no `while let`): body blocks without break or
                // continue (those are outside the subset and end the evaluation)
                let crate::ast::WhileRow { cond, body, label } = *self.u.prog.files[file].ast.row(row);
                if label.is_some() || matches!(self.u.prog.files[file].ast.expr(cond).kind, ExprKind::Let(..)) {
                    return None;
                }
                loop {
                    self.steps += 1;
                    if self.steps > STEP_BUDGET {
                        return None;
                    }
                    let cv = self.eval(b, env, cond)?;
                    if *cv.first()? & 1 == 0 {
                        break;
                    }
                    self.block(b, env, body)?;
                }
                Some(Vec::new())
            }
            ExprKind::If(c, then, els) => {
                let cv = self.eval(b, env, c)?;
                if *cv.first()? & 1 == 1 {
                    self.block(b, env, then)
                } else {
                    match els.get() {
                        Some(x) => self.eval(b, env, x),
                        None => Some(Vec::new()),
                    }
                }
            }
            ExprKind::TupleField(x, i) => {
                if b.field_idx.get(&e.0).map_or(false, |f| f.0 != 0) {
                    return None;
                }
                let xt = b.ty(x);
                let v = self.eval(b, env, x)?;
                let off = match self.kind(xt) {
                    TyKind::Tuple(_) => *self.layout(xt).fields.get(i as usize)?,
                    TyKind::Adt(..) => self.struct_field_off(xt, i)?,
                    _ => return None,
                } as usize;
                let n = self.size(t) as usize;
                Some(v.get(off..off + n)?.to_vec())
            }
            ExprKind::Field(x, _) => {
                let (derefs, fi) = b.field_idx.get(&e.0).map(|&(d, f)| (d as usize, f))?;
                if derefs != 0 {
                    return None;
                }
                let xt = b.ty(x);
                let v = self.eval(b, env, x)?;
                let off = self.struct_field_off(xt, fi)? as usize;
                let n = self.size(t) as usize;
                Some(v.get(off..off + n)?.to_vec())
            }
            ExprKind::Index(a, i) => {
                if b.ov_index.contains_key(&e.0) {
                    return None;
                }
                let derefs = b.index_derefs(e.0);
                let av = self.eval(b, env, a)?;
                let iv = self.eval(b, env, i)?;
                let idx = rd_u64(&iv, 0)? as usize;
                let es = self.size(t) as usize;
                let at = b.ty(a);
                match (derefs, self.kind(at)) {
                    (0, TyKind::Array(_, n)) if n.try_to_target_usize(&self.u.tcx.tys).is_some() => {
                        let n = n.try_to_target_usize(&self.u.tcx.tys).unwrap();
                        if idx >= n as usize {
                            return None;
                        }
                        Some(av.get(idx * es..(idx + 1) * es)?.to_vec())
                    }
                    (1, TyKind::Ref(_, inner, _)) => {
                        let (p, n) = match self.kind(inner) {
                            TyKind::Array(_, n) if n.try_to_target_usize(&self.u.tcx.tys).is_some() => (rd_u64(&av, 0)?, n.try_to_target_usize(&self.u.tcx.tys).unwrap()),
                            TyKind::Slice(_) => (rd_u64(&av, 0)?, rd_u64(&av, 8)?),
                            _ => return None,
                        };
                        if idx as u64 >= n || p == 0 {
                            return None;
                        }
                        let mut out = vec![0u8; es];
                        unsafe { std::ptr::copy_nonoverlapping((p as usize + idx * es) as *const u8, out.as_mut_ptr(), es) };
                        Some(out)
                    }
                    _ => None,
                }
            }
            _ => None,
        }
    }

    fn block(&mut self, b: &Body, env: &mut Vec<Option<Vec<u8>>>, bl: crate::ast::BlockId) -> Option<Vec<u8>> {
        let file = b.file as usize;
        let stmts = self.u.prog.files[file].ast.stmts(bl).to_vec();
        let n = stmts.len();
        for (k, s) in stmts.iter().enumerate() {
            match s {
                Stmt::Let(r) if { let l = self.u.prog.files[file].ast.row(*r); l.init.is_some() && l.else_.is_none() } => {
                    let crate::ast::LetStmt { pat, init, .. } = self.u.prog.files[file].ast.row(*r).clone();
                    let (pat, x) = (&pat, init.as_ref().unwrap());
                    if matches!(self.u.prog.files[file].ast.pat(*pat), crate::ast::Pat::Wild) {
                        self.eval(b, env, *x)?;
                        continue;
                    }
                    let li = *b.pat_local.get(&pat.0)?;
                    if !matches!(self.u.prog.files[file].ast.pat(*pat), crate::ast::Pat::Ident { by_ref: false, sub: None, .. }) {
                        return None;
                    }
                    let v = self.eval(b, env, *x)?;
                    if env.len() <= li as usize {
                        env.resize(li as usize + 1, None);
                    }
                    env[li as usize] = Some(v);
                }
                Stmt::Expr(x, false) if k + 1 == n => return self.eval(b, env, *x),
                Stmt::Expr(x, _) => { self.eval(b, env, *x)?; }
                Stmt::Item(_) => {}
                _ => return None,
            }
        }
        // no tail: unit
        Some(Vec::new())
    }

    fn owned_local_place(&self, b: &Body, e: ExprId) -> bool {
        match self.u.prog.files[b.file as usize].ast.expr(e).kind {
            ExprKind::Path(_) => matches!(b.res.get(&e.0), Some(Res::Local(_))),
            ExprKind::Paren(x) => self.owned_local_place(b, x),
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => b.field_idx.get(&e.0).is_none_or(|f| f.0 == 0) && self.owned_local_place(b, x),
            ExprKind::Index(x, _) => b.index_derefs(e.0) == 0 && self.owned_local_place(b, x),
            _ => false,
        }
    }

    /// A shared receiver's address, retaining slice metadata through field projections.
    /// Owned locals (whose pointers could escape), overloaded derefs and raw-pointer
    /// derefs stay outside the subset.
    fn borrow(&mut self, b: &Body, env: &mut Vec<Option<Vec<u8>>>, e: ExprId) -> Option<Vec<u8>> {
        if self.owned_local_place(b, e) { return None; }
        match self.u.prog.files[b.file as usize].ast.expr(e).kind {
            ExprKind::Paren(x) => return self.borrow(b, env, x),
            ExprKind::Unary(UnOp::Deref, x) if matches!(self.kind(b.ty(x)), TyKind::Ref(..)) => return self.eval(b, env, x),
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (derefs, fi) = b.field_idx.get(&e.0).map(|&(d, f)| (d as usize, f))?;
                let (mut ptr, t) = match (derefs, self.kind(b.ty(x))) {
                    (0, _) => (self.borrow(b, env, x)?, b.ty(x)),
                    (1, TyKind::Ref(_, inner, _)) if !b.ov_deref(x.0, 0).is_some() => (self.eval(b, env, x)?, inner),
                    _ => return None,
                };
                let off = match self.kind(t) {
                    TyKind::Tuple(_) => *self.layout(t).fields.get(fi as usize)?,
                    TyKind::Adt(..) => self.struct_field_off(t, fi)?,
                    _ => return None,
                };
                let a = rd_u64(&ptr, 0)?.checked_add(off as u64)?;
                ptr.get_mut(..8)?.copy_from_slice(&a.to_le_bytes());
                let et = b.ty(e);
                if self.u.lay.is_unsized(&crate::layout::LayoutCx::new(&self.u.prog), &mut self.u.tcx, et) {
                    if !matches!(self.kind(et), TyKind::Slice(_) | TyKind::Str) || ptr.len() != 16 { return None; }
                } else {
                    ptr.truncate(8);
                }
                return Some(ptr);
            }
            ExprKind::Path(_) => {
                if let Some(Res::Def(d)) = b.res.get(&e.0).copied() {
                    if self.u.prog.def(d).kind == DefKind::Static {
                        let a = self.u.ctfe_static_addr(d)?;
                        return Some(a.to_le_bytes().to_vec());
                    }
                }
            }
            ExprKind::Index(x, index) => {
                if b.ov_index.contains_key(&e.0) { return None; }
                let derefs = b.index_derefs(e.0);
                let (ptr, t) = match (derefs, self.kind(b.ty(x))) {
                    (0, _) => (self.borrow(b, env, x)?, b.ty(x)),
                    (1, TyKind::Ref(_, inner, _)) => (self.eval(b, env, x)?, inner),
                    _ => return None,
                };
                let (et, n) = match self.kind(t) {
                    TyKind::Array(et, n) => (et, n.try_to_target_usize(&self.u.tcx.tys)?),
                    TyKind::Slice(et) => (et, rd_u64(&ptr, 8)?),
                    _ => return None,
                };
                let i = rd_u64(&self.eval(b, env, index)?, 0)?;
                if i >= n { return None; }
                let offset = i.checked_mul(self.size(et) as u64)?;
                let a = rd_u64(&ptr, 0)?.checked_add(offset)?;
                return Some(a.to_le_bytes().to_vec());
            }
            _ => {}
        }
        let t = b.ty(e);
        if self.u.lay.is_unsized(&crate::layout::LayoutCx::new(&self.u.prog), &mut self.u.tcx, t)
            || !self.u.tcx.is_freeze(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), t) { return None; }
        let v = self.eval(b, env, e)?;
        let l = self.layout(t);
        if l.uninhabited || v.len() != l.size as usize { return None; }
        let a = if l.size == 0 { l.align.max(1) as u64 }
            else { self.u.data(&v, l.align.max(8) as u64, crate::syms::DataKind::Typed(t)) };
        Some(a.to_le_bytes().to_vec())
    }

    fn local_place(&mut self, b: &Body, env: &mut Vec<Option<Vec<u8>>>, e: ExprId) -> Option<(usize, usize)> {
        match self.u.prog.files[b.file as usize].ast.expr(e).kind {
            ExprKind::Paren(x) => self.local_place(b, env, x),
            ExprKind::Path(_) => match b.res.get(&e.0) {
                Some(Res::Local(li)) => Some((*li as usize, 0)),
                _ => None,
            },
            ExprKind::Field(x, _) | ExprKind::TupleField(x, _) => {
                let (derefs, fi) = b.field_idx.get(&e.0).map(|&(d, f)| (d as usize, f))?;
                if derefs != 0 { return None; }
                let t = b.ty(x);
                let off = match self.kind(t) {
                    TyKind::Tuple(_) => *self.layout(t).fields.get(fi as usize)?,
                    TyKind::Adt(..) => self.struct_field_off(t, fi)?,
                    _ => return None,
                };
                let (li, base) = self.local_place(b, env, x)?;
                Some((li, base.checked_add(off as usize)?))
            }
            ExprKind::Index(x, index) => {
                if b.ov_index.contains_key(&e.0) || b.index_derefs(e.0) != 0 { return None; }
                let TyKind::Array(et, n) = self.kind(b.ty(x)) else { return None };
                let n = n.try_to_target_usize(&self.u.tcx.tys)?;
                let (li, base) = self.local_place(b, env, x)?;
                let i = rd_u64(&self.eval(b, env, index)?, 0)?;
                if i >= n { return None; }
                Some((li, base.checked_add((i as usize).checked_mul(self.size(et) as usize)?)?))
            }
            _ => None,
        }
    }

    /// Relax's offset_of! expands to an inline const taking a raw field address of a
    /// MaybeUninit<T>, then subtracting its base address. Recognize that entire typed
    /// computation and answer from the layout. Do not materialize uninitialized bytes or
    /// make general pointer-to-integer casts legal in CTFE (rustc's offset_of is a builtin).
    fn offset_of(&mut self, b: &Body, e: ExprId, bl: crate::ast::BlockId) -> Option<Vec<u8>> {
        let file = &self.u.prog.files[b.file as usize];
        let ast = &file.ast;
        // Only the macro expansion, never a user-written pointer-to-integer const.
        let pos = ast.expr(e).lo;
        let expansions = file.part_at(pos).map_or(&file.exp_map, |p| &p.exp_map);
        let (_, (_, end, call)) = crate::origin_table::OSegRef::from(expansions).floor(pos)?;
        if pos >= end { return None; }
        let invocation = file.bytes(call, u32::MAX);
        let bang = invocation.iter().position(|c| *c == b'!')?;
        if std::str::from_utf8(&invocation[..bang]).ok()?.rsplit("::").next()?.trim() != "offset_of" { return None; }
        let [Stmt::Let(r1), Stmt::Let(r2), Stmt::Let(r3), Stmt::Expr(tail, false)] = ast.stmts(bl) else { return None };
        let (crate::ast::LetStmt { pat: up, init: Some(ui), else_: None, .. },
             crate::ast::LetStmt { pat: bp, init: Some(bi), else_: None, .. },
             crate::ast::LetStmt { pat: fp, init: Some(fi), else_: None, .. }) = (ast.row(*r1), ast.row(*r2), ast.row(*r3)) else { return None };
        for pat in [up, bp, fp] {
            if !matches!(ast.pat(*pat), crate::ast::Pat::Ident { by_ref: false, mutbl: false, sub: None, .. }) { return None; }
        }
        let (ul, base, field) = (*b.pat_local.get(&up.0)?, *b.pat_local.get(&bp.0)?, *b.pat_local.get(&fp.0)?);
        let TyKind::Adt(mu, _, _) = self.kind(b.ty(*ui)) else { return None };
        if self.u.tcx.lang_items.get(crate::lang_items::LangItem::MaybeUninit) != Some(mu) { return None; }
        let ExprKind::Call(f, args) = ast.expr(*ui).kind else { return None };
        let TyKind::FnDef(d, _, _) = self.kind(b.ty(f)) else { return None };
        if !args.is_empty() || self.u.prog.def(d).krate != self.u.core_crate || self.u.prog.name(d) != "uninit" { return None; }
        let ExprKind::MethodCall(recv, mrow) = ast.expr(*bi).kind else { return None };
        let args = ast.row(mrow).args;
        let m = b.methods.get(&bi.0)?;
        if !args.is_empty() || b.res.get(&recv.0) != Some(&Res::Local(ul)) || m.derefs != 0 || m.autoref != 1
            || self.u.prog.def(m.def).krate != self.u.core_crate || self.u.prog.name(m.def) != "as_ptr" { return None; }
        let ExprKind::Unsafe(fb) = ast.expr(*fi).kind else { return None };
        let [Stmt::Expr(addr, false)] = ast.stmts(fb) else { return None };
        let ExprKind::AddrOf(true, false, place) = ast.expr(*addr).kind else { return None };
        let ExprKind::Binary(BinOp::Sub, lhs, rhs) = ast.expr(*tail).kind else { return None };
        // Both operands are exactly `local as *const u8 as usize`.
        for (e, local) in [(lhs, field), (rhs, base)] {
            if b.ty(e) != self.u.tcx.tys.usize_ { return None; }
            let ExprKind::Cast(ptr, _) = ast.expr(e).kind else { return None };
            if !matches!(self.kind(b.ty(ptr)), TyKind::Ptr(false, p) if matches!(self.kind(p), TyKind::Int(crate::program::Prim::U8))) { return None; }
            let ExprKind::Cast(value, _) = ast.expr(ptr).kind else { return None };
            if b.res.get(&value.0) != Some(&Res::Local(local)) { return None; }
        }
        let offset = self.place_offset(b, place, base)?;
        Some((offset as u64).to_le_bytes().to_vec())
    }

    fn place_offset(&mut self, b: &Body, e: ExprId, base: u32) -> Option<u32> {
        let kind = self.u.prog.files[b.file as usize].ast.expr(e).kind;
        let (x, fi) = match kind {
            ExprKind::Paren(x) => return self.place_offset(b, x, base),
            ExprKind::Unary(UnOp::Deref, x) if b.res.get(&x.0) == Some(&Res::Local(base)) => return Some(0),
            ExprKind::Field(x, _) => {
                let (derefs, fi) = b.field_idx.get(&e.0).map(|&(d, f)| (d as usize, f))?;
                if derefs != 0 { return None; }
                (x, fi)
            }
            ExprKind::TupleField(x, fi) => {
                if b.field_idx.get(&e.0).is_some_and(|f| f.0 != 0) { return None; }
                (x, fi)
            }
            _ => return None,
        };
        let t = b.ty(x);
        if self.u.lay.is_unsized(&crate::layout::LayoutCx::new(&self.u.prog), &mut self.u.tcx, t) { return None; }
        let offset = match self.kind(t) {
            TyKind::Tuple(_) => *self.layout(t).fields.get(fi as usize)?,
            TyKind::Adt(..) => self.struct_field_off(t, fi)?,
            _ => return None,
        };
        self.place_offset(b, x, base)?.checked_add(offset)
    }

    fn fn_pointer(&mut self, t: TyId) -> Option<Vec<u8>> {
        let TyKind::FnDef(d, args, _) = self.kind(t) else { return None };
        let instance = self.u.tcx.resolve_instance_raw(&self.u.prog, crate::tcx::TypingEnv::fully_monomorphized(&self.u.prog), d, self.u.tcx.tys.list(args)).ok()?;
        let crate::instance::InstanceKind::Item(d) = instance.def else { return None; };
        let args = instance.compact_args(&self.u.tcx);
        // Foreign symbols and track_caller shims have different entry conventions.
        if self.u.prog.def(d).kind == DefKind::ForeignFn || self.u.tcx.fn_sig(&self.u.prog, d).is_some_and(|s| s.track_caller) { return None; }
        let id = self.u.fn_id(crate::jit::FnKey::Inst(d, args));
        Some(self.u.entry(id).to_le_bytes().to_vec())
    }

    fn lit(&mut self, b: &Body, e: ExprId, k: LitKind, t: TyId) -> Option<Vec<u8>> {
        let file = b.file as usize;
        let ex = self.u.prog.files[file].ast.expr(e);
        let text = std::str::from_utf8(self.u.prog.files[file].bytes(ex.lo, ex.hi)).ok()?.to_string();
        match k {
            LitKind::Int | LitKind::Float => match self.kind(t) {
                TyKind::Float(p) => {
                    let num: String = text.chars().filter(|c| *c != '_').collect();
                    let num = num.trim_end_matches("f32").trim_end_matches("f64");
                    if num.starts_with("0x") || num.starts_with("0o") || num.starts_with("0b") {
                        return None;
                    }
                    if p == crate::program::Prim::F32 {
                        Some(num.parse::<f32>().ok()?.to_bits().to_le_bytes().to_vec())
                    } else {
                        Some(num.parse::<f64>().ok()?.to_bits().to_le_bytes().to_vec())
                    }
                }
                TyKind::Int(_) => {
                    let (n, s) = self.int_of(t)?;
                    let v = crate::const_fold::parse_int_lit(&text)?;
                    // A positive literal must fit. Negated signed minimum literals are
                    // handled at the unary expression, before evaluating the operand.
                    let max = if s { u128::MAX >> (129 - n * 8) } else { u128::MAX >> (128 - n * 8) };
                    if v > max {
                        return None;
                    }
                    Some(v.to_le_bytes()[..n].to_vec())
                }
                _ => None,
            },
            LitKind::Bool(x) => Some(vec![x as u8]),
            LitKind::Char => {
                let s = crate::lit::unescape_str(&text);
                let mut it = s.chars();
                let c = it.next()?;
                if it.next().is_some() {
                    return None;
                }
                Some((c as u32).to_le_bytes().to_vec())
            }
            LitKind::Byte => {
                let v = crate::lit::unescape_bytes(&text);
                if v.len() != 1 {
                    return None;
                }
                Some(v)
            }
            LitKind::Str | LitKind::RawStr => {
                if !matches!(self.kind(t), TyKind::Ref(_, s, _) if matches!(self.kind(s), TyKind::Str)) {
                    return None;
                }
                let s = crate::lit::unescape_str(&text);
                let a = self.u.lit_data(s.as_bytes());
                let mut out = a.to_le_bytes().to_vec();
                out.extend_from_slice(&(s.len() as u64).to_le_bytes());
                Some(out)
            }
            LitKind::ByteStr => {
                let v = crate::lit::unescape_bytes(&text);
                match self.kind(t) {
                    TyKind::Ref(_, a, _) if matches!(self.kind(a), TyKind::Array(_, n) if n.try_to_target_usize(&self.u.tcx.tys) == Some(v.len() as u64)) => {}
                    _ => return None,
                }
                let a = self.u.lit_data(&v);
                Some(a.to_le_bytes().to_vec())
            }
            LitKind::CStr => {
                let mut v = crate::lit::unescape_bytes(&text);
                if v.contains(&0) { return None; }
                v.push(0);
                let mut out = self.u.lit_data(&v).to_le_bytes().to_vec();
                out.extend_from_slice(&(v.len() as u64).to_le_bytes());
                Some(out)
            }
        }
    }

    fn binary(&mut self, b: &Body, env: &mut Vec<Option<Vec<u8>>>, op: BinOp, x: ExprId, y: ExprId) -> Option<Vec<u8>> {
        // short-circuit logic
        if matches!(op, BinOp::And | BinOp::Or) {
            let a = *self.eval(b, env, x)?.first()? & 1;
            if (op == BinOp::And && a == 0) || (op == BinOp::Or && a == 1) {
                return Some(vec![a]);
            }
            let c = *self.eval(b, env, y)?.first()? & 1;
            return Some(vec![c]);
        }
        let xt = b.ty(x);
        let av = self.eval(b, env, x)?;
        let bv = self.eval(b, env, y)?;
        self.binary_values(op, &av, xt, &bv, b.ty(y))
    }

    fn binary_values(&mut self, op: BinOp, av: &[u8], xt: TyId, bv: &[u8], yt: TyId) -> Option<Vec<u8>> {
        if let TyKind::Float(_) = self.kind(xt) {
            return None;
        }
        let (n, s) = self.int_of(xt)?;
        if n >= 16 {
            return None;
        }
        let (nq, sq) = self.int_of(yt)?;
        if av.len() != n || bv.len() != nq { return None; }
        let p = load_int(av, n, s);
        let q = load_int(bv, nq, sq);
        let cmp = |r: bool| Some(vec![r as u8]);
        let (pi, qi) = (p as i128, q as i128);
        // N:rustc_mir_build/src/builder/expr/as_rvalue.rs:533-589: both / and %
        // reject MIN/-1 regardless of the body's overflow policy.
        if s && matches!(op, BinOp::Div | BinOp::Rem) && qi == -1 && pi == -(1i128 << (n * 8 - 1)) { return None; }
        let r: i128 = match op {
            BinOp::Eq => return cmp(p == q),
            BinOp::Ne => return cmp(p != q),
            BinOp::Lt => return cmp(if s { pi < qi } else { p < q }),
            BinOp::Le => return cmp(if s { pi <= qi } else { p <= q }),
            BinOp::Gt => return cmp(if s { pi > qi } else { p > q }),
            BinOp::Ge => return cmp(if s { pi >= qi } else { p >= q }),
            BinOp::Add => pi.checked_add(qi)?,
            BinOp::Sub => pi.checked_sub(qi)?,
            BinOp::Mul => pi.checked_mul(qi)?,
            BinOp::Div => {
                if qi == 0 {
                    return None;
                }
                pi.checked_div(qi)?
            }
            BinOp::Rem => {
                if qi == 0 || (s && pi == -(1i128 << (n * 8 - 1)) && qi == -1) {
                    return None;
                }
                pi.checked_rem(qi)?
            }
            BinOp::BitAnd => pi & qi,
            BinOp::BitOr => pi | qi,
            BinOp::BitXor => pi ^ qi,
            BinOp::Shl | BinOp::Shr => {
                if qi < 0 || qi >= (n * 8) as i128 {
                    return None;
                }
                if op == BinOp::Shl {
                    // bits shifted out are dropped (only the shift amount can overflow)
                    let m = if n >= 16 { u128::MAX } else { (1u128 << (n * 8)) - 1 };
                    (((p as u128) << qi) & m) as i128
                } else if s {
                    pi >> qi
                } else {
                    ((p as u128) >> qi) as i128
                }
            }
            BinOp::And | BinOp::Or => unreachable!(),
        };
        let r = if matches!(op, BinOp::BitAnd | BinOp::BitOr | BinOp::BitXor | BinOp::Shl | BinOp::Shr) { wrap(r, n, s) } else { r };
        if !fits(r, n, s) {
            return None;
        }
        Some(store_int(r, n))
    }

    fn cast(&mut self, v: &[u8], from: TyId, to: TyId) -> Option<Vec<u8>> {
        let (fk, tk) = (self.kind(from), self.kind(to));
        match (&fk, &tk) {
            (TyKind::FnDef(..), TyKind::FnPtr(..) | TyKind::Ptr(..)) => self.fn_pointer(from),
            (TyKind::Int(_), TyKind::Ptr(_, inner)) if !self.u.lay.is_unsized(&crate::layout::LayoutCx::new(&self.u.prog), &mut self.u.tcx, *inner) => {
                let (n, s) = self.int_of(from)?;
                Some(store_int(load_int(v, n, s), 8))
            }
            (TyKind::Ptr(..) | TyKind::Ref(..) | TyKind::FnPtr(..), TyKind::Ptr(..)) if self.size(from) == self.size(to) => Some(v.to_vec()),
            (TyKind::Ptr(..), TyKind::Ptr(_, inner)) if v.len() == 16 && !self.u.lay.is_unsized(&crate::layout::LayoutCx::new(&self.u.prog), &mut self.u.tcx, *inner) => Some(v.get(..8)?.to_vec()),
            (TyKind::Int(_) | TyKind::Bool | TyKind::Char, TyKind::Int(_)) => {
                let (n, s) = self.int_of(from)?;
                let x = load_int(v, n, s);
                let (m, _) = self.int_of(to)?;
                Some(store_int(x as i128, m))
            }
            (TyKind::Int(_), TyKind::Char) => {
                // only `u8 as char` is a valid cast
                let (n, s) = self.int_of(from)?;
                if n != 1 || s {
                    return None;
                }
                Some((v[0] as u32).to_le_bytes().to_vec())
            }
            (TyKind::Adt(d, _, _), TyKind::Int(_)) => {
                // a fieldless enum's discriminant
                let adt = self.u.tcx.adts.get(d).cloned()?;
                if !adt.is_enum || adt.variants.iter().any(|x| !x.fields.is_empty()) {
                    return None;
                }
                let l = self.layout(from);
                if l.uninhabited { return None; }
                let disc = match l.tag {
                    Some((toff, relax_backend::rir::Mem::Int(tn, ts))) => {
                        let raw = load_int(v.get(toff as usize..(toff + tn as u32) as usize)?, tn as usize, ts);
                        crate::lower::decode_tag_const(self.u, raw, &l, &adt)?
                    }
                    None => match l.enc {
                        crate::layout::TagEnc::Single(vi) => adt.discriminant_for_variant(&self.u.tcx, &self.u.prog, vi)?.signed_value(),
                        _ => return None,
                    },
                    _ => return None,
                };
                let (m, _) = self.int_of(to)?;
                Some(store_int(disc, m))
            }
            _ if fk == tk => Some(v.to_vec()),
            _ => None,
        }
    }

    /// The address of constant `d`'s value: already evaluated, or evaluated here. Never
    /// compiles anything: a constant outside the subset fails the evaluation that uses it,
    /// which then runs as code, the way it did before (its own const uses resolved there).
    fn const_data(&mut self, d: crate::program::DefId, args: Vec<TyId>) -> Option<u64> {
        let u = &mut *self.u;
        let (d, args) = if u.prog.def(d).kind == DefKind::AssocConst && u.prog.def(u.prog.def(d).parent).kind == DefKind::Trait {
            let instance = u.tcx.resolve_instance_raw(&u.prog, crate::tcx::TypingEnv::fully_monomorphized(&u.prog), d, &args).expect("established instance dispatch");
            let relax_types::instance::InstanceKind::Item(d) = instance.def else { panic!("non-item associated const") };
            (d, instance.compact_args(&u.tcx))
        } else {
            (d, args)
        };
        if let Some(&a) = u.consts.get(&(d, args.clone())) {
            return Some(a);
        }
        if u.const_failed.contains(&(d, args.clone())) || self.depth >= CALL_DEPTH {
            return None;
        }
        // Nested CTFE used to bypass the persistent value cache even when the
        // outer initializer came from a warm program.
        if let Some(a) = unsafe { crate::cache::const_value(u as *mut Unit, d, &args) } {
            u.put_const(d, args, a);
            return Some(a);
        }
        let t = u.tcx.tys.subst_erasing_regions(u.tcx.const_ty(&u.prog, d)?, &args);
        let l = u.layout_of(t);
        let buf = u.data(&vec![0u8; l.size.max(1) as usize], l.align.max(8) as u64, crate::syms::DataKind::Typed(t));
        u.syms.set_data_name(buf, crate::jit::const_name(u, d, &args));
        // a constant that reads itself, directly or through others, is outside the subset (the
        // code path reports the cycle); each nested evaluation has a Ctx of its own, so
        // `depth` alone does not bound this recursion
        // (and nesting is bounded: each nested evaluation has a Ctx of its own, so `depth` alone does not
        // stop `C<T>` reading `C<Option<T>>` reading ...)
        let limit = u.prog.crates[u.prog.def(d).krate as usize].recursion_limit as usize;
        if CONSTS_READING.try_with(|c| c.borrow().len() >= limit || c.borrow().iter().any(|(cd, ca)| *cd == d && *ca == args)).unwrap_or(false) {
            return None;
        }
        struct Pop;
        impl Drop for Pop { fn drop(&mut self) { let _ = CONSTS_READING.try_with(|c| { c.borrow_mut().pop(); }); } }
        let _ = CONSTS_READING.try_with(|c| c.borrow_mut().push((d, args.clone())));
        self.depth += 1;
        let ok = { let _pop = Pop; eval_into(self.u, d, &args, buf) };
        self.depth -= 1;
        if !ok {
            return None;
        }
        zero_undefined(self.u, t, buf);
        self.u.syms.set_data_kind(buf, crate::syms::DataKind::Typed(t));
        if let Some(mut cache) = self.u.cache.take() {
            cache.record_const(self.u, d, &args, buf);
            self.u.cache = Some(cache);
        }
        self.u.put_const(d, args, buf);
        Some(buf)
    }

    fn field_index(&self, t: TyId, vi: u32, name: u32) -> Option<u32> {
        if let TyKind::Adt(d, _, _) = self.u.tcx.tys.kind(t) {
            let a = self.u.tcx.adts.get(d)?;
            let v = a.variants.get(vi as usize)?;
            for (i, f) in v.fields.iter().enumerate() {
                if f.name == name {
                    return Some(i as u32);
                }
            }
        }
        None
    }

    fn struct_field_off(&mut self, t: TyId, fi: u32) -> Option<u32> {
        let adt = match self.kind(t) {
            TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).cloned()?,
            _ => return None,
        };
        if adt.is_enum {
            return None;
        }
        if adt.is_union {
            return Some(0);
        }
        self.layout(t).fields.get(fi as usize).copied()
    }

    /// A struct / enum value of variant `vi` with the given field values (unset bytes zero).
    fn build_adt(&mut self, t: TyId, vi: u32, vals: Vec<(u32, Vec<u8>)>) -> Option<Vec<u8>> {
        let adt = match self.kind(t) {
            TyKind::Adt(d, _, _) => self.u.tcx.adts.get(&d).cloned()?,
            _ => return None,
        };
        if adt.is_union {
            return None;
        }
        let l = self.layout(t);
        if l.uninhabited {
            return None;
        }
        let mut out = vec![0u8; l.size as usize];
        let nf = adt.variants.get(vi as usize)?.fields.len();
        if vals.len() != nf {
            // every field must be given (no `..base`, no partial values)
            return None;
        }
        for (fi, v) in vals {
            let off = if adt.is_enum { *l.variant_fields.get(vi as usize)?.get(fi as usize)? } else { *l.fields.get(fi as usize)? };
            put(&mut out, off as usize, &v)?;
        }
        if adt.is_enum {
            // the tag (lower::emit_store_disc)
            if let Some((toff, relax_backend::rir::Mem::Int(tn, _))) = l.tag {
                let tv = match l.enc {
                    crate::layout::TagEnc::Direct => Some(adt.discriminant_for_variant(&self.u.tcx, &self.u.prog, vi)?.val),
                    crate::layout::TagEnc::Single(_) => None,
                    crate::layout::TagEnc::Niche { untagged, lo, start, .. } => {
                        if vi == untagged {
                            None
                        } else {
                            Some(start.wrapping_add((vi - lo) as u128))
                        }
                    }
                };
                if let Some(tv) = tv {
                    put(&mut out, toff as usize, &tv.to_le_bytes()[..tn as usize])?;
                }
            } else if l.tag.is_some() {
                return None;
            }
        }
        Some(out)
    }

    /// A call of a `const fn` over the subset, including by-value and shared receivers.
    fn call_const_fn(&mut self, d: crate::program::DefId, mut gargs: Vec<TyId>, args: Vec<Vec<u8>>, t: TyId) -> Option<Vec<u8>> {
        if self.depth >= CALL_DEPTH {
            if self.trace { self.reason = Some(format!("const call depth exhausted in {}", self.u.prog.def_path(d))); }
            return None;
        }
        let def = self.u.prog.def(d);
        if def.kind == DefKind::ForeignFn && self.u.is_intrinsic(d) {
            match self.u.prog.name(d) {
                "slice_len" if args.len() == 1 => return Some(args[0].get(8..16)?.to_vec()),
                "slice_ptr" if args.len() == 1 => return Some(args[0].get(..8)?.to_vec()),
                "str_as_bytes" if args.len() == 1 && args[0].len() == 16 => return Some(args[0].clone()),
                _ => {}
            }
            if matches!(self.u.prog.name(d), "from_bits_f32" | "from_bits_f64" | "to_bits_f32" | "to_bits_f64") && args.len() == 1 && args[0].len() == self.size(t) as usize {
                return Some(args[0].clone());
            }
            let value = match self.u.prog.name(d) {
                "size_of" if args.is_empty() => {
                    let l = self.layout(*gargs.first()?);
                    crate::layout::round_up(l.size, l.align) as u64
                }
                "align_of" if args.is_empty() => self.layout(*gargs.first()?).align as u64,
                _ => return None,
            };
            return Some(value.to_le_bytes().to_vec());
        }
        if !matches!(def.kind, DefKind::Fn | DefKind::AssocFn) {
            return None;
        }
        let ast = &self.u.prog.files[def.file as usize].ast;
        match &ast.item(def.item).kind {
            ItemKind::Fn(sig, _, _) if ast.fn_sig(*sig).konst && !ast.fn_sig(*sig).variadic && ast.fn_sig(*sig).abi.is_none() => {}
            _ => return None,
        }
        // const fn bodies are checked once per unit (each constant's evaluation used to check
        // them again: AtomicU32::new 1,187 times in uizoo); a check that failed or waited for a
        // constant is not kept
        gargs.iter_mut().for_each(|t| *t = self.u.tcx.tys.erase_regions(*t));
        let key = (d, gargs.clone());
        let epoch = self.u.tcx.collect_epoch.load(std::sync::atomic::Ordering::SeqCst);
        let cached = self.u.ctfe_fn_bodies.lock().unwrap_or_else(|e| e.into_inner()).get(&key).filter(|x| x.0 == epoch).map(|x| x.1.clone());
        let body = match cached {
            Some(b) => b,
            None => {
                let b = check_quiet(self.u, d, &gargs)?;
                self.u.ctfe_fn_bodies.lock().unwrap_or_else(|e| e.into_inner()).insert(key, (epoch, b.clone()));
                b
            }
        };
        let bl = body.body?;
        let has_self = body.locals.first().is_some_and(|l| l.pat == u32::MAX && self.u.prog.syms.str(l.name) == "self");
        if body.param_pats.len() + usize::from(has_self) != args.len() {
            return None;
        }
        let mut env: Vec<Option<Vec<u8>>> = vec![None; body.locals.len()];
        let mut args = args.into_iter();
        if has_self {
            let v = args.next()?;
            if v.len() != self.size(body.locals[0].ty) as usize { return None; }
            env[0] = Some(v);
        }
        for (p, v) in body.param_pats.iter().zip(args) {
            if !matches!(self.u.prog.files[body.file as usize].ast.pat(*p), crate::ast::Pat::Ident { by_ref: false, sub: None, .. }) {
                return None;
            }
            let li = *body.pat_local.get(&p.0)? as usize;
            if v.len() != self.size(body.locals.get(li)?.ty) as usize { return None; }
            *env.get_mut(li)? = Some(v);
        }
        if let Some(f) = self.u.prog.exec_profile_file(body.file) { f.note(crate::exec_profile::CTFE); }
        self.depth += 1;
        let r = self.block(&body, &mut env, bl);
        self.depth -= 1;
        let r = r?;
        if r.len() != self.size(t) as usize {
            return None;
        }
        Some(r)
    }
}

fn load_int(v: &[u8], n: usize, signed: bool) -> i128 {
    let mut b = [0u8; 16];
    b[..n.min(v.len())].copy_from_slice(&v[..n.min(v.len())]);
    let raw = u128::from_le_bytes(b);
    if signed && n < 16 && (raw >> (n * 8 - 1)) & 1 == 1 {
        (raw | (u128::MAX << (n * 8))) as i128
    } else {
        raw as i128
    }
}

fn store_int(x: i128, n: usize) -> Vec<u8> {
    x.to_le_bytes()[..n].to_vec()
}

/// x reduced to n bytes, read back with the signedness (bit operations never overflow)
fn wrap(x: i128, n: usize, signed: bool) -> i128 {
    let b = store_int(x, n);
    load_int(&b, n, signed)
}

fn fits(x: i128, n: usize, signed: bool) -> bool {
    if n >= 16 {
        return signed || x >= 0;
    }
    let bits = (n * 8) as u32;
    if signed {
        x >= -(1i128 << (bits - 1)) && x < (1i128 << (bits - 1))
    } else {
        x >= 0 && x < (1i128 << bits)
    }
}
