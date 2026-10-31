//! C ABI classification: where each argument and the result of a C call (or of an
//! `extern "C"` body) live, for SysV x86-64, Win64 (x64 Windows) and AAPCS64 as Apple uses it. The
//! backends turn a `Plan` into moves; a front end only says which values are aggregates
//! (`CSig`; Relax builds them from Rust types).

use crate::rir::{CAgg, CArg, CSig, Mem};

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Target {
    /// SysV x86-64 (the C convention on Linux/macOS, also explicit on Windows)
    SysV,
    /// Microsoft x64 (Windows): four register slots by position (rcx rdx r8 r9 / xmm0-3), a
    /// 32-byte shadow area below the stack arguments, aggregates of 1/2/4/8 bytes by value and
    /// the rest by reference to a caller copy, results of 1/2/4/8 bytes in rax, else through a
    /// hidden pointer in the first slot (returned in rax)
    Win64,
    /// AAPCS64 with Apple's rules: variadic arguments on the stack in 8-byte slots,
    /// fixed stack arguments packed by natural size and alignment
    Darwin64,
    /// standard AAPCS64 (Linux, Android): variadic arguments like fixed ones (registers, then the
    /// stack), every stack argument in its own 8-byte-aligned slot of at least 8 bytes
    Aapcs64,
}

#[cfg(all(target_arch = "aarch64", target_vendor = "apple"))]
pub const HOST: Target = Target::Darwin64;
#[cfg(all(target_arch = "aarch64", not(target_vendor = "apple")))]
pub const HOST: Target = Target::Aapcs64;
#[cfg(all(not(target_arch = "aarch64"), not(windows)))]
pub const HOST: Target = Target::SysV;
#[cfg(all(not(target_arch = "aarch64"), windows))]
pub const HOST: Target = Target::Win64;

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

/// Win64 (Microsoft x64 calling convention docs, "Parameter passing" / "Return values"):
/// argument k takes slot k whatever its class -- rcx/xmm0, rdx/xmm1, r8/xmm2, r9/xmm3, then the
/// stack at [rsp + 32 + 8 (k - 4)] at the call (the caller reserves 32 bytes of shadow space
/// even with fewer than four arguments); aggregates of 1, 2, 4 or 8 bytes travel as that integer,
/// every other aggregate as the address of a caller-made copy; variadic floating-point arguments
/// go in the integer slot (the callee's va_arg reads the integer registers' home area); results:
/// a scalar float in xmm0, an aggregate of 1/2/4/8 bytes in rax, any other aggregate through a
/// caller pointer passed in slot 0 and returned in rax.
fn plan_win64(sig: &CSig) -> Plan {
    let by_value = |a: &CAgg| matches!(a.size, 1 | 2 | 4 | 8);
    let mut slot = 0u32;
    let ret = match &sig.ret {
        None => RetPlan::Scalar,
        Some(a) if by_value(a) => RetPlan::Parts(vec![Part { off: 0, size: a.size, loc: PLoc::Int(0) }]),
        Some(_) => {
            slot = 1; // the hidden result pointer
            RetPlan::Indirect
        }
    };
    let stack_off = |slot: u32| 32 + 8 * (slot - 4);
    let mut args = Vec::new();
    let mut n_flt = 0u8;
    for (k, arg) in sig.args.iter().enumerate() {
        let variadic = (k as u32) >= sig.n_fixed;
        let p = match arg {
            CArg::Scalar(m) => {
                if slot < 4 {
                    if is_float(*m) && !variadic {
                        n_flt = n_flt.max(slot as u8 + 1);
                        ArgPlan::Scalar(PLoc::Flt(slot as u8), 8)
                    } else {
                        ArgPlan::Scalar(PLoc::Int(slot as u8), 8)
                    }
                } else {
                    ArgPlan::Scalar(PLoc::Stack(stack_off(slot)), 8)
                }
            }
            CArg::Agg(a) if by_value(a) => {
                if slot < 4 {
                    ArgPlan::Parts(vec![Part { off: 0, size: a.size, loc: PLoc::Int(slot as u8) }])
                } else {
                    ArgPlan::Stack(stack_off(slot), a.size)
                }
            }
            CArg::Agg(_) => {
                if slot < 4 {
                    ArgPlan::ByRef(PLoc::Int(slot as u8))
                } else {
                    ArgPlan::ByRef(PLoc::Stack(stack_off(slot)))
                }
            }
        };
        args.push(p);
        slot += 1;
    }
    let stack_bytes = round_up(32 + 8 * slot.saturating_sub(4), 16);
    Plan { args, ret, stack_bytes, n_flt }
}

/// Classifies a C signature for `t`.
pub fn plan(sig: &CSig, t: Target) -> Plan {
    let (n_int, n_flt) = match t {
        Target::SysV => (6u8, 8u8),
        Target::Darwin64 | Target::Aapcs64 => (8u8, 8u8),
        Target::Win64 => return plan_win64(sig),
    };
    let mut gi = 0u8;
    let mut fi = 0u8;
    let mut st = StackAlloc { off: 0 };
    // result
    let ret = match &sig.ret {
        None => RetPlan::Scalar,
        Some(a) => match t {
            Target::Darwin64 | Target::Aapcs64 => {
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
            Target::Win64 => unreachable!("plan_win64"),
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
    // Apple's AAPCS64 variant: variadic arguments on the stack, fixed stack arguments packed
    let apple = t == Target::Darwin64;
    let mut args = Vec::new();
    for (k, arg) in sig.args.iter().enumerate() {
        let variadic = (k as u32) >= sig.n_fixed;
        let p = match (t, arg) {
            (Target::Darwin64 | Target::Aapcs64, CArg::Scalar(m)) => {
                if variadic && apple {
                    ArgPlan::Scalar(PLoc::Stack(st.take(8, 8)), 8)
                } else if is_float(*m) && fi < n_flt {
                    fi += 1;
                    ArgPlan::Scalar(PLoc::Flt(fi - 1), 8)
                } else if !is_float(*m) && gi < n_int {
                    gi += 1;
                    ArgPlan::Scalar(PLoc::Int(gi - 1), 8)
                } else if apple {
                    let sz = mem_size(*m);
                    ArgPlan::Scalar(PLoc::Stack(st.take(sz, sz)), sz)
                } else {
                    ArgPlan::Scalar(PLoc::Stack(st.take(8, 8)), 8)
                }
            }
            (Target::Darwin64 | Target::Aapcs64, CArg::Agg(a)) => {
                let h = hfa(a);
                if a.size > 16 && h.is_none() {
                    // pointer to the (private) copy
                    if !(variadic && apple) && gi < n_int {
                        gi += 1;
                        ArgPlan::ByRef(PLoc::Int(gi - 1))
                    } else {
                        ArgPlan::ByRef(PLoc::Stack(st.take(8, 8)))
                    }
                } else if let (Some((n, m)), false) = (h, variadic && apple) {
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
                    if !(variadic && apple) && gi as u32 + words <= n_int as u32 && words > 0 {
                        let mut parts = Vec::new();
                        for j in 0..words {
                            parts.push(Part { off: 8 * j, size: piece_size(a, j), loc: PLoc::Int(gi + j as u8) });
                        }
                        gi += words as u8;
                        ArgPlan::Parts(parts)
                    } else {
                        if !(variadic && apple) {
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
            (Target::Win64, _) => unreachable!("plan_win64"),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rir::CConv;

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
        let sig = CSig { conv: CConv::Host, args: vec![CArg::Scalar(I64), rect, pair, big], ret: Some(CAgg { size: 16, align: 8, fields: vec![(0, Mem::F64), (8, Mem::F64)] }), n_fixed: u32::MAX, ret_narrow: None, unwinds: false };
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
        let p = plan(&CSig { conv: CConv::Host, args: args.clone(), ret: None, n_fixed: u32::MAX, ret_narrow: None, unwinds: false }, Target::Darwin64);
        assert_eq!(p.args[8], ArgPlan::Scalar(PLoc::Stack(0), 1));
        assert_eq!(p.args[9], ArgPlan::Scalar(PLoc::Stack(4), 4));
        let p = plan(&CSig { conv: CConv::Host, args: vec![CArg::Scalar(I64), CArg::Scalar(I32), CArg::Scalar(Mem::F64)], ret: None, n_fixed: 1, ret_narrow: None, unwinds: false }, Target::Darwin64);
        assert_eq!(p.args[1], ArgPlan::Scalar(PLoc::Stack(0), 8));
        assert_eq!(p.args[2], ArgPlan::Scalar(PLoc::Stack(8), 8));
    }

    /// Standard AAPCS64 (Linux/Android, e.g. JNI's variadic Call*Method): variadic arguments
    /// take the next registers like fixed ones (int in x, double in v), stack arguments
    /// (here the 9th and 10th integers) one 8-byte slot each.
    #[test]
    fn aapcs64_variadic_in_registers_and_slots() {
        let mut args = Vec::new();
        for _ in 0..8 {
            args.push(CArg::Scalar(I64));
        }
        args.push(CArg::Scalar(Mem::Int(1, false)));
        args.push(CArg::Scalar(I32));
        let p = plan(&CSig { conv: CConv::Host, args, ret: None, n_fixed: u32::MAX, ret_narrow: None, unwinds: false }, Target::Aapcs64);
        assert_eq!(p.args[8], ArgPlan::Scalar(PLoc::Stack(0), 8));
        assert_eq!(p.args[9], ArgPlan::Scalar(PLoc::Stack(8), 8));
        assert_eq!(p.stack_bytes, 16);
        let p = plan(&CSig { conv: CConv::Host, args: vec![CArg::Scalar(I64), CArg::Scalar(I32), CArg::Scalar(Mem::F64)], ret: None, n_fixed: 1, ret_narrow: None, unwinds: false }, Target::Aapcs64);
        assert_eq!(p.args[1], ArgPlan::Scalar(PLoc::Int(1), 8));
        assert_eq!(p.args[2], ArgPlan::Scalar(PLoc::Flt(0), 8));
        assert_eq!(p.stack_bytes, 0);
        let hfa = agg(8, 4, &[(0, Mem::F32), (4, Mem::F32)]);
        let p = plan(&CSig { conv: CConv::Host, args: vec![CArg::Scalar(I64), hfa], ret: None, n_fixed: 1, ret_narrow: None, unwinds: false }, Target::Aapcs64);
        assert!(matches!(&p.args[1], ArgPlan::Parts(v) if v.len() == 2 && v[1].loc == PLoc::Flt(1)));
    }

    /// The Microsoft x64 docs' examples: `func1(int a, int b, int c, int d, int e, int f)` (e, f
    /// on the stack above the shadow area), `func2(int a, double b, int c, float d, int e, float f)`
    /// (slots by position: b in xmm1, d in xmm3), `func3(__m64, __m128, aligned struct, struct)`
    /// shape (8-byte struct by value, 16-byte struct by reference), a 16-byte result through the
    /// hidden pointer in rcx (shifting every argument one slot), an 8-byte result in rax, and a
    /// variadic double in an integer slot.
    #[test]
    fn win64_slots_by_position() {
        let p = plan(&CSig { conv: CConv::Host, args: vec![CArg::Scalar(I32); 6], ret: None, n_fixed: u32::MAX, ret_narrow: None, unwinds: false }, Target::Win64);
        assert_eq!(p.args[3], ArgPlan::Scalar(PLoc::Int(3), 8));
        assert_eq!(p.args[4], ArgPlan::Scalar(PLoc::Stack(32), 8));
        assert_eq!(p.args[5], ArgPlan::Scalar(PLoc::Stack(40), 8));
        assert_eq!(p.stack_bytes, 48);
        let sig = CSig { conv: CConv::Host, args: vec![CArg::Scalar(I32), CArg::Scalar(Mem::F64), CArg::Scalar(I32), CArg::Scalar(Mem::F32), CArg::Scalar(I32), CArg::Scalar(Mem::F32)], ret: None, n_fixed: u32::MAX, ret_narrow: None, unwinds: false };
        let p = plan(&sig, Target::Win64);
        assert_eq!(p.args[1], ArgPlan::Scalar(PLoc::Flt(1), 8));
        assert_eq!(p.args[2], ArgPlan::Scalar(PLoc::Int(2), 8));
        assert_eq!(p.args[3], ArgPlan::Scalar(PLoc::Flt(3), 8));
        assert_eq!(p.args[5], ArgPlan::Scalar(PLoc::Stack(40), 8));
        let p = plan(&CSig { conv: CConv::Host, args: vec![], ret: None, n_fixed: u32::MAX, ret_narrow: None, unwinds: false }, Target::Win64);
        assert_eq!(p.stack_bytes, 32);
        let small = agg(8, 4, &[(0, Mem::F32), (4, Mem::F32)]);
        let big = agg(16, 8, &[(0, I64), (8, I64)]);
        let odd = agg(3, 1, &[(0, Mem::Int(1, false)), (1, Mem::Int(1, false)), (2, Mem::Int(1, false))]);
        let sig = CSig { conv: CConv::Host, args: vec![small.clone(), big.clone(), odd], ret: Some(CAgg { size: 16, align: 8, fields: vec![(0, I64), (8, I64)] }), n_fixed: u32::MAX, ret_narrow: None, unwinds: false };
        let p = plan(&sig, Target::Win64);
        assert_eq!(p.ret, RetPlan::Indirect);
        assert_eq!(p.args[0], ArgPlan::Parts(vec![Part { off: 0, size: 8, loc: PLoc::Int(1) }]));
        assert_eq!(p.args[1], ArgPlan::ByRef(PLoc::Int(2)));
        assert_eq!(p.args[2], ArgPlan::ByRef(PLoc::Int(3)));
        let p = plan(&CSig { conv: CConv::Host, args: vec![big], ret: Some(CAgg { size: 8, align: 4, fields: vec![(0, Mem::F32), (4, Mem::F32)] }), n_fixed: u32::MAX, ret_narrow: None, unwinds: false }, Target::Win64);
        assert_eq!(p.ret, RetPlan::Parts(vec![Part { off: 0, size: 8, loc: PLoc::Int(0) }]));
        assert_eq!(p.args[0], ArgPlan::ByRef(PLoc::Int(0)));
        let p = plan(&CSig { conv: CConv::Host, args: vec![CArg::Scalar(I64), CArg::Scalar(Mem::F64)], ret: None, n_fixed: 1, ret_narrow: None, unwinds: false }, Target::Win64);
        assert_eq!(p.args[1], ArgPlan::Scalar(PLoc::Int(1), 8));
    }
}
