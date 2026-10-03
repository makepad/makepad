//! C ABI classification: where each argument and the result of a C call (or of an
//! `extern "C"` body) live, for SysV x86-64 and AAPCS64 as Apple uses it. The
//! backends turn a `Plan` into moves; lower only says which values are aggregates
//! (`CSig`). `c_sig` builds a `CSig` from Rust types.

use crate::layout::Layout;
use crate::rir::{CAgg, CArg, CSig, Mem};
use crate::types::{TyId, TyKind};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// SysV x86-64 (Linux)
    SysV,
    /// AAPCS64 with Apple's rules: variadic arguments on the stack in 8-byte slots,
    /// fixed stack arguments packed by natural size and alignment
    Darwin64,
}

#[cfg(target_arch = "aarch64")]
pub const HOST: Target = Target::Darwin64;
#[cfg(not(target_arch = "aarch64"))]
pub const HOST: Target = Target::SysV;

/// Where a value (or one piece of an aggregate) is passed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PLoc {
    /// n-th integer argument / result register
    Int(u8),
    /// n-th float/vector argument / result register
    Flt(u8),
    /// byte offset in the outgoing (caller) / incoming (callee) stack argument area
    Stack(u32),
}

/// A piece of an aggregate: `size` bytes at `off` in the aggregate's memory.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Part {
    pub off: u32,
    pub size: u32,
    pub loc: PLoc,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum ArgPlan {
    /// scalar value; `size` bytes on the stack (Apple packs fixed stack arguments)
    Scalar(PLoc, u32),
    /// aggregate split into register pieces
    Parts(Vec<Part>),
    /// aggregate passed as the address of a copy (AAPCS64 > 16 bytes)
    ByRef(PLoc),
    /// aggregate copied by value into the stack argument area: (offset, size)
    Stack(u32, u32),
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RetPlan {
    /// no aggregate result (scalar results use the backend's normal result registers)
    Scalar,
    /// aggregate returned in registers
    Parts(Vec<Part>),
    /// aggregate returned through a caller-provided address: x8 on AAPCS64, the first
    /// integer argument (and rax on return) on SysV
    Indirect,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Plan {
    pub args: Vec<ArgPlan>,
    pub ret: RetPlan,
    /// bytes of stack arguments (16-byte rounded)
    pub stack_bytes: u32,
    /// float registers used (SysV variadic calls pass it in al)
    pub n_flt: u8,
}

fn mem_size(m: Mem) -> u32 {
    match m {
        Mem::Int(b, _) => b as u32,
        Mem::F32 => 4,
        Mem::F64 => 8,
    }
}

fn is_float(m: Mem) -> bool {
    matches!(m, Mem::F32 | Mem::F64)
}

fn round_up(x: u32, a: u32) -> u32 {
    if a <= 1 {
        x
    } else {
        (x + a - 1) / a * a
    }
}

/// Homogeneous float aggregate: 1-4 fields of one float type covering the whole size.
fn hfa(a: &CAgg) -> Option<(u32, Mem)> {
    let n = a.fields.len();
    if n == 0 || n > 4 {
        return None;
    }
    let m = a.fields[0].1;
    if !is_float(m) {
        return None;
    }
    let es = mem_size(m);
    for (k, (off, fm)) in a.fields.iter().enumerate() {
        if *fm != m || *off != k as u32 * es {
            return None;
        }
    }
    if a.size != n as u32 * es {
        return None;
    }
    Some((n as u32, m))
}

/// SysV class of each eightbyte of an aggregate of <= 16 bytes: Some(true) = SSE.
/// None = MEMORY.
fn sysv_classes(a: &CAgg) -> Option<Vec<bool>> {
    if a.size == 0 || a.size > 16 || a.fields.is_empty() {
        return None;
    }
    let n = ((a.size + 7) / 8) as usize;
    let mut sse = vec![true; n];
    let mut seen = vec![false; n];
    for (off, m) in &a.fields {
        let sz = mem_size(*m);
        if off % sz != 0 {
            return None; // unaligned field
        }
        let e = (*off / 8) as usize;
        if e >= n {
            return None;
        }
        seen[e] = true;
        if !is_float(*m) {
            sse[e] = false;
        }
    }
    for k in 0..n {
        if !seen[k] {
            // padding-only eightbyte: SysV classifies it NO_CLASS -> merged; treat as INTEGER
            sse[k] = false;
        }
    }
    Some(sse)
}

fn piece_size(a: &CAgg, k: u32) -> u32 {
    (a.size - 8 * k).min(8)
}

struct StackAlloc {
    off: u32,
}

impl StackAlloc {
    fn take(&mut self, size: u32, align: u32) -> u32 {
        let o = round_up(self.off, align.max(1));
        self.off = o + size;
        o
    }
}

/// Classifies a C signature for `t`.
pub fn plan(sig: &CSig, t: Target) -> Plan {
    let (n_int, n_flt) = match t {
        Target::SysV => (6u8, 8u8),
        Target::Darwin64 => (8u8, 8u8),
    };
    let mut gi = 0u8;
    let mut fi = 0u8;
    let mut st = StackAlloc { off: 0 };
    // result
    let ret = match &sig.ret {
        None => RetPlan::Scalar,
        Some(a) => match t {
            Target::Darwin64 => {
                if let Some((n, m)) = hfa(a) {
                    let es = mem_size(m);
                    let mut parts = Vec::new();
                    for k in 0..n {
                        parts.push(Part { off: k * es, size: es, loc: PLoc::Flt(k as u8) });
                    }
                    RetPlan::Parts(parts)
                } else if a.size <= 16 && a.size > 0 {
                    let mut parts = Vec::new();
                    for k in 0..(a.size + 7) / 8 {
                        parts.push(Part { off: 8 * k, size: piece_size(a, k), loc: PLoc::Int(k as u8) });
                    }
                    RetPlan::Parts(parts)
                } else {
                    RetPlan::Indirect
                }
            }
            Target::SysV => match sysv_classes(a) {
                Some(cls) => {
                    let mut parts = Vec::new();
                    let (mut ri, mut rf) = (0u8, 0u8);
                    for (k, sse) in cls.iter().enumerate() {
                        let loc = if *sse {
                            rf += 1;
                            PLoc::Flt(rf - 1)
                        } else {
                            ri += 1;
                            PLoc::Int(ri - 1)
                        };
                        parts.push(Part { off: 8 * k as u32, size: piece_size(a, k as u32), loc });
                    }
                    RetPlan::Parts(parts)
                }
                None => {
                    gi = 1; // hidden result pointer in rdi
                    RetPlan::Indirect
                }
            },
        },
    };
    let mut args = Vec::new();
    for (k, arg) in sig.args.iter().enumerate() {
        let variadic = (k as u32) >= sig.n_fixed;
        let p = match (t, arg) {
            (Target::Darwin64, CArg::Scalar(m)) => {
                if variadic {
                    ArgPlan::Scalar(PLoc::Stack(st.take(8, 8)), 8)
                } else if is_float(*m) && fi < n_flt {
                    fi += 1;
                    ArgPlan::Scalar(PLoc::Flt(fi - 1), 8)
                } else if !is_float(*m) && gi < n_int {
                    gi += 1;
                    ArgPlan::Scalar(PLoc::Int(gi - 1), 8)
                } else {
                    let sz = mem_size(*m);
                    ArgPlan::Scalar(PLoc::Stack(st.take(sz, sz)), sz)
                }
            }
            (Target::Darwin64, CArg::Agg(a)) => {
                let h = hfa(a);
                if a.size > 16 && h.is_none() {
                    // pointer to the (private) copy
                    if !variadic && gi < n_int {
                        gi += 1;
                        ArgPlan::ByRef(PLoc::Int(gi - 1))
                    } else {
                        ArgPlan::ByRef(PLoc::Stack(st.take(8, 8)))
                    }
                } else if let (Some((n, m)), false) = (h, variadic) {
                    let es = mem_size(m);
                    if fi as u32 + n <= n_flt as u32 {
                        let mut parts = Vec::new();
                        for j in 0..n {
                            parts.push(Part { off: j * es, size: es, loc: PLoc::Flt(fi + j as u8) });
                        }
                        fi += n as u8;
                        ArgPlan::Parts(parts)
                    } else {
                        fi = n_flt;
                        ArgPlan::Stack(st.take(round_up(a.size, 8), a.align.max(8)), a.size)
                    }
                } else {
                    let words = (a.size + 7) / 8;
                    if !variadic && gi as u32 + words <= n_int as u32 && words > 0 {
                        let mut parts = Vec::new();
                        for j in 0..words {
                            parts.push(Part { off: 8 * j, size: piece_size(a, j), loc: PLoc::Int(gi + j as u8) });
                        }
                        gi += words as u8;
                        ArgPlan::Parts(parts)
                    } else {
                        if !variadic {
                            gi = n_int;
                        }
                        ArgPlan::Stack(st.take(round_up(a.size, 8), a.align.max(8)), a.size)
                    }
                }
            }
            (Target::SysV, CArg::Scalar(m)) => {
                if is_float(*m) && fi < n_flt {
                    fi += 1;
                    ArgPlan::Scalar(PLoc::Flt(fi - 1), 8)
                } else if !is_float(*m) && gi < n_int {
                    gi += 1;
                    ArgPlan::Scalar(PLoc::Int(gi - 1), 8)
                } else {
                    ArgPlan::Scalar(PLoc::Stack(st.take(8, 8)), 8)
                }
            }
            (Target::SysV, CArg::Agg(a)) => {
                let mut placed = None;
                if let Some(cls) = sysv_classes(a) {
                    let ni = cls.iter().filter(|s| !**s).count() as u8;
                    let nf = cls.iter().filter(|s| **s).count() as u8;
                    if gi + ni <= n_int && fi + nf <= n_flt {
                        let mut parts = Vec::new();
                        for (j, sse) in cls.iter().enumerate() {
                            let loc = if *sse {
                                fi += 1;
                                PLoc::Flt(fi - 1)
                            } else {
                                gi += 1;
                                PLoc::Int(gi - 1)
                            };
                            parts.push(Part { off: 8 * j as u32, size: piece_size(a, j as u32), loc });
                        }
                        placed = Some(ArgPlan::Parts(parts));
                    }
                }
                match placed {
                    Some(p) => p,
                    None => ArgPlan::Stack(st.take(round_up(a.size, 8), a.align.max(8).min(16)), a.size),
                }
            }
        };
        args.push(p);
    }
    Plan { args, ret, stack_bytes: round_up(st.off, 16), n_flt: fi }
}

// ------------------------------------------------------------ CSig from Rust types

/// Flattens the scalar fields of `t` (structs, tuples, arrays; at most 64 fields).
fn flatten(u: &mut crate::jit::Unit, t: TyId, base: u32, out: &mut Vec<(u32, Mem)>) -> bool {
    if out.len() > 64 {
        return false;
    }
    let l: Layout = u.lay.of(&mut u.tcx, t);
    let k = u.tcx.tys.kind(t).clone();
    match k {
        TyKind::Array(e, n) => {
            let el = u.lay.of(&mut u.tcx, e);
            let stride = round_up(el.size, el.align);
            for i in 0..n as u32 {
                if !flatten(u, e, base + i * stride, out) {
                    return false;
                }
            }
            true
        }
        TyKind::Tuple(v) => {
            for (i, ft) in v.iter().enumerate() {
                if !flatten(u, *ft, base + l.fields[i], out) {
                    return false;
                }
            }
            true
        }
        TyKind::Adt(d, args) => {
            let adt = match u.tcx.adts.get(&d) {
                Some(a) => a.clone(),
                None => return true,
            };
            if adt.is_enum {
                // fieldless (C-like) enums are their tag
                if l.variant_fields.iter().all(|v| v.is_empty()) {
                    if let Some((_, m)) = l.tag {
                        out.push((base, m));
                        return true;
                    }
                }
                return false;
            }
            if adt.is_union {
                return false;
            }
            for (i, f) in adt.variants[0].fields.iter().enumerate() {
                let ft = u.tcx.tys.subst(f.ty, &args);
                if !flatten(u, ft, base + l.fields[i], out) {
                    return false;
                }
            }
            true
        }
        _ => match &l.leaves {
            Some(lv) => {
                for x in lv {
                    out.push((base + x.off, x.mem));
                }
                true
            }
            None => false,
        },
    }
}

/// How `t` crosses a C boundary: a scalar (one register-sized leaf) or an aggregate.
pub fn c_arg(u: &mut crate::jit::Unit, t: TyId) -> CArg {
    let l = u.lay.of(&mut u.tcx, t);
    if let Some(lv) = &l.leaves {
        if lv.len() == 1 && lv[0].off == 0 && l.size == mem_size(lv[0].mem) {
            return CArg::Scalar(lv[0].mem);
        }
    }
    let mut fields = Vec::new();
    if !flatten(u, t, 0, &mut fields) {
        fields.clear();
    }
    CArg::Agg(CAgg { size: l.size, align: l.align, fields })
}

/// C signature of a call or body with Rust parameter types `params` and result `ret`.
/// `n_fixed`: Some(n) for a variadic call with n fixed parameters (the rest of `params`
/// are the variadic arguments' types).
pub fn c_sig(u: &mut crate::jit::Unit, params: &[TyId], ret: TyId, n_fixed: Option<usize>) -> CSig {
    let mut args = Vec::new();
    for p in params {
        let l = u.lay.of(&mut u.tcx, *p);
        if l.size == 0 {
            continue; // zero-sized arguments are not passed
        }
        args.push(c_arg(u, *p));
    }
    let rl = u.lay.of(&mut u.tcx, ret);
    let ret = if rl.size == 0 {
        None
    } else {
        match c_arg(u, ret) {
            CArg::Scalar(_) => None,
            CArg::Agg(a) => Some(a),
        }
    };
    CSig { args, ret, n_fixed: match n_fixed {
        Some(n) => n as u32,
        None => u32::MAX,
    } }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn agg(size: u32, align: u32, f: &[(u32, Mem)]) -> CArg {
        CArg::Agg(CAgg { size, align, fields: f.to_vec() })
    }
    const I32: Mem = Mem::Int(4, true);
    const I64: Mem = Mem::Int(8, true);

    #[test]
    fn darwin_rect_and_mixed() {
        let rect = agg(32, 8, &[(0, Mem::F64), (8, Mem::F64), (16, Mem::F64), (24, Mem::F64)]);
        let pair = agg(8, 4, &[(0, I32), (4, I32)]);
        let big = agg(24, 8, &[(0, I64), (8, Mem::F64), (16, I64)]);
        let sig = CSig { args: vec![CArg::Scalar(I64), rect, pair, big], ret: Some(CAgg { size: 16, align: 8, fields: vec![(0, Mem::F64), (8, Mem::F64)] }), n_fixed: u32::MAX };
        let p = plan(&sig, Target::Darwin64);
        assert_eq!(p.ret, RetPlan::Parts(vec![Part { off: 0, size: 8, loc: PLoc::Flt(0) }, Part { off: 8, size: 8, loc: PLoc::Flt(1) }]));
        assert_eq!(p.args[0], ArgPlan::Scalar(PLoc::Int(0), 8));
        assert!(matches!(&p.args[1], ArgPlan::Parts(v) if v.len() == 4 && v[3].loc == PLoc::Flt(3)));
        assert_eq!(p.args[2], ArgPlan::Parts(vec![Part { off: 0, size: 8, loc: PLoc::Int(1) }]));
        assert_eq!(p.args[3], ArgPlan::ByRef(PLoc::Int(2)));
        let p = plan(&sig, Target::SysV);
        assert_eq!(p.args[1], ArgPlan::Stack(0, 32));
        assert_eq!(p.args[3], ArgPlan::Stack(32, 24));
        assert_eq!(p.args[2], ArgPlan::Parts(vec![Part { off: 0, size: 8, loc: PLoc::Int(1) }]));
        assert_eq!(p.stack_bytes, 64);
    }

    #[test]
    fn darwin_variadic_and_packing() {
        let mut args = Vec::new();
        for _ in 0..8 {
            args.push(CArg::Scalar(I64));
        }
        args.push(CArg::Scalar(Mem::Int(1, false)));
        args.push(CArg::Scalar(I32));
        let p = plan(&CSig { args: args.clone(), ret: None, n_fixed: u32::MAX }, Target::Darwin64);
        assert_eq!(p.args[8], ArgPlan::Scalar(PLoc::Stack(0), 1));
        assert_eq!(p.args[9], ArgPlan::Scalar(PLoc::Stack(4), 4));
        let p = plan(&CSig { args: vec![CArg::Scalar(I64), CArg::Scalar(I32), CArg::Scalar(Mem::F64)], ret: None, n_fixed: 1 }, Target::Darwin64);
        assert_eq!(p.args[1], ArgPlan::Scalar(PLoc::Stack(0), 8));
        assert_eq!(p.args[2], ArgPlan::Scalar(PLoc::Stack(8), 8));
    }
}
