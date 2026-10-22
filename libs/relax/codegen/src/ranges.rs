//! Value ranges (O3): integer intervals and compare facts carried forward through the CFG,
//! used to decide compares -- bounds checks above all -- whose outcome the ranges fix.
//!
//! RIR keeps an integer extended to 64 bits by its type; an interval here is over that 64-bit
//! value read as u64 (`lo <= hi`, never wrapping). RIR is not SSA, so every fact is flow
//! sensitive: a definition of a vreg ends every fact that mentions it. Facts come from
//! - definitions: constants, masks, zero-extending loads and conversions, non-wrapping
//!   add / sub / shifts / multiplication / division / remainder by constants, `d = c ? a : b`;
//! - branch edges: `c = x < k` (any condition, against a constant or a vreg) refines x on the
//!   edge it holds on, and through `x = y + k` also y;
//! - loop headers keep only the facts on vregs the loop never defines, plus a hypothesis for each
//!   counting-up loop variable (defined in the loop only by `v = v + k`, k >= 1, or a copy of
//!   such a sum) and compared in the loop with a bound known on entry: v stays in [its entry
//!   minimum, its type's natural maximum minus k]. The pass runs with the hypotheses on a copy
//!   and keeps the result only when every back edge carries v inside its hypothesis (induction
//!   over the loop's iterations); failed ones are dropped and the pass reruns (at most three
//!   rounds, then none).
//! A non-negative dividend makes a signed division / remainder by 2^k (expanded by
//! `div_by_const` into a sign-bias sequence) a plain shift / mask: `(i % 1024) as f32` over
//! `0..n` is one `and`, as rustc's.
//! A compare the facts decide becomes a constant (`simplify` then folds its branch), and
//! `d = (b + k) - b` becomes `k`. rustc's LLVM removes these checks the same way (correlated
//! value propagation + instcombine); without it every masked index into a fixed array keeps its
//! bounds check (deflate: four per hash-chain probe). MAKEPAD_RUN_NO_RANGES=1 turns it off.
use relax_backend::regalloc::{for_defs, reachable};
use relax_backend::rir::*;
/// Maps keyed by vreg numbers and block pairs: a multiply-rotate hash (SipHash's per-key cost
/// showed in O3 profiles: State is cloned and rebuilt per edge)
type HashMap<K, V> = std::collections::HashMap<K, V, std::hash::BuildHasherDefault<VHash>>;

#[derive(Default)]
struct VHash(u64);

impl std::hash::Hasher for VHash {
    fn finish(&self) -> u64 {
        self.0
    }
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.write_u64(*b as u64);
        }
    }
    fn write_u32(&mut self, x: u32) {
        self.write_u64(x as u64);
    }
    fn write_u64(&mut self, x: u64) {
        self.0 = (self.0.rotate_left(5) ^ x).wrapping_mul(0x517c_c1b7_2722_0a95);
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct R {
    lo: u64,
    hi: u64,
}

/// A compare's operand: a vreg or a constant.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Opnd {
    V(u32),
    K(i64),
}

#[derive(Clone, Default)]
struct State {
    /// known intervals
    rng: HashMap<u32, R>,
    /// d = (x cond y) of the compare that last defined d
    cmp: Vec<(u32, Cond, bool, u32, Opnd)>,
    /// d = x + k exactly (mod 2^bits of its type; `wide`: 64-bit, or no wrap shown)
    lin: Vec<(u32, u32, i64, u8)>,
    /// x cond y known true (from branch edges)
    rel: Vec<(Cond, bool, u32, Opnd)>,
    /// d <= x (signed?, d, x): d is a select of two values and a compare of them, `min(x, y)`
    le: Vec<(bool, u32, u32)>,
}

impl State {
    fn kill(&mut self, v: u32) {
        self.rng.remove(&v);
        self.cmp.retain(|(d, _, _, x, y)| *d != v && *x != v && *y != Opnd::V(v));
        self.lin.retain(|(d, x, _, _)| *d != v && *x != v);
        self.rel.retain(|(_, _, x, y)| *x != v && *y != Opnd::V(v));
        self.le.retain(|(_, d, x)| *d != v && *x != v);
    }
    fn get(&self, v: u32) -> Option<R> {
        self.rng.get(&v).copied()
    }
    /// x's interval narrowed to r (intersection; an empty one means the path cannot run --
    /// kept as is, nothing is concluded from it)
    fn narrow(&mut self, x: u32, r: R) {
        let n = match self.get(x) {
            Some(o) => R { lo: o.lo.max(r.lo), hi: o.hi.min(r.hi) },
            None => r,
        };
        if n.lo <= n.hi {
            self.rng.insert(x, n);
        }
    }
    /// the meet of two states at a join: facts both hold (intervals: their hull)
    fn join(&self, o: &State) -> State {
        let mut rng = HashMap::default();
        for (v, a) in &self.rng {
            if let Some(b) = o.rng.get(v) {
                rng.insert(*v, R { lo: a.lo.min(b.lo), hi: a.hi.max(b.hi) });
            }
        }
        State {
            rng,
            cmp: self.cmp.iter().filter(|x| o.cmp.contains(x)).copied().collect(),
            lin: self.lin.iter().filter(|x| o.lin.contains(x)).copied().collect(),
            rel: self.rel.iter().filter(|x| o.rel.contains(x)).copied().collect(),
            le: self.le.iter().filter(|x| o.le.contains(x)).copied().collect(),
        }
    }
    /// only facts on vregs outside `defd` (a loop's header: the facts its body cannot change)
    fn keep_outside(&mut self, defd: &[bool]) {
        let out = |v: u32| !defd.get(v as usize).copied().unwrap_or(false);
        let opo = |o: &Opnd| match o {
            Opnd::V(v) => out(*v),
            Opnd::K(_) => true,
        };
        self.rng.retain(|v, _| out(*v));
        self.cmp.retain(|(d, _, _, x, y)| out(*d) && out(*x) && opo(y));
        self.lin.retain(|(d, x, _, _)| out(*d) && out(*x));
        self.rel.retain(|(_, _, x, y)| out(*x) && opo(y));
        self.le.retain(|(_, d, x)| out(*d) && out(*x));
    }
}

fn bits(it: IntTy) -> u32 {
    (it.bits as u32).min(64)
}

/// The largest canonical value of type `it` that reads as the same number unsigned and as its
/// type (unsigned: 2^bits - 1; signed: 2^(bits-1) - 1): arithmetic below it behaves as on
/// natural numbers.
fn nat_max(it: IntTy) -> u64 {
    let b = bits(it);
    if it.signed {
        (1u64 << (b - 1)) - 1
    } else if b == 64 {
        u64::MAX
    } else {
        (1u64 << b) - 1
    }
}

fn mk(lo: u128, hi: u128, max: u64) -> Option<R> {
    if lo <= hi && hi <= max as u128 {
        Some(R { lo: lo as u64, hi: hi as u64 })
    } else {
        None
    }
}

fn konst(k: i64) -> R {
    R { lo: k as u64, hi: k as u64 }
}

/// The interval of `a op b` of type it, a and b as natural numbers within it (else None).
fn arith(op: IOp, it: IntTy, a: R, b: R) -> Option<R> {
    let m = nat_max(it);
    if a.hi > m || b.hi > m {
        // a negative signed value or beyond the type: only masks still say something
        return match op {
            IOp::And if b.hi <= m => Some(R { lo: 0, hi: b.hi }),
            IOp::And if a.hi <= m => Some(R { lo: 0, hi: a.hi }),
            _ => None,
        };
    }
    let (al, ah, bl, bh) = (a.lo as u128, a.hi as u128, b.lo as u128, b.hi as u128);
    match op {
        IOp::Add => mk(al + bl, ah + bh, m),
        IOp::Sub => {
            if al >= bh {
                mk(al - bh, ah - bl, m)
            } else {
                None
            }
        }
        IOp::Mul => mk(al * bl, ah * bh, m),
        IOp::And => Some(R { lo: 0, hi: a.hi.min(b.hi) }),
        IOp::Or | IOp::Xor => {
            let top = a.hi.max(b.hi);
            let p = if top == 0 { 0 } else { u64::MAX >> top.leading_zeros() };
            mk(if op == IOp::Or { al.max(bl) } else { 0 }, p as u128, m)
        }
        IOp::Shr if b.lo == b.hi && b.lo < 64 => mk(al >> bl, ah >> bl, m),
        IOp::Shl if b.lo == b.hi && b.lo < 64 => {
            let s = b.lo as u32;
            if ah.leading_zeros() as u32 >= s + 64 {
                mk(al << s, ah << s, m)
            } else {
                None
            }
        }
        IOp::Div if b.lo > 0 => mk(al / bh, ah / bl, m),
        IOp::Rem if b.lo > 0 => mk(0, ah.min(bh - 1), m),
        _ => None,
    }
}

/// Is `x c y` true for every x in a, y in b (Some(true)), false for every one (Some(false))?
/// Signed compares only of natural-number intervals (below 2^63).
fn decide(c: Cond, signed: bool, a: R, b: R) -> Option<bool> {
    if signed && (a.hi > i64::MAX as u64 || b.hi > i64::MAX as u64) {
        return None;
    }
    match c {
        Cond::Lt => {
            if a.hi < b.lo {
                Some(true)
            } else if a.lo >= b.hi {
                Some(false)
            } else {
                None
            }
        }
        Cond::Le => {
            if a.hi <= b.lo {
                Some(true)
            } else if a.lo > b.hi {
                Some(false)
            } else {
                None
            }
        }
        Cond::Gt => decide(Cond::Lt, signed, b, a),
        Cond::Ge => decide(Cond::Le, signed, b, a),
        Cond::Eq => {
            if a.lo == a.hi && b.lo == b.hi && a.lo == b.lo {
                Some(true)
            } else if a.hi < b.lo || b.hi < a.lo {
                Some(false)
            } else {
                None
            }
        }
        Cond::Ne => decide(Cond::Eq, signed, a, b).map(|x| !x),
    }
}

fn negate(c: Cond) -> Cond {
    match c {
        Cond::Eq => Cond::Ne,
        Cond::Ne => Cond::Eq,
        Cond::Lt => Cond::Ge,
        Cond::Le => Cond::Gt,
        Cond::Gt => Cond::Le,
        Cond::Ge => Cond::Lt,
    }
}

/// The values x may take given `x c k` holds (an interval of the u64 view; None: no single
/// interval). Signed facts give one only for k >= 0 bounds that exclude negative x.
fn bound(c: Cond, signed: bool, k: u64) -> Option<R> {
    if signed {
        let k = k as i64;
        return match c {
            Cond::Ge if k >= 0 => Some(R { lo: k as u64, hi: i64::MAX as u64 }),
            Cond::Gt if k >= -1 => Some(R { lo: (k + 1) as u64, hi: i64::MAX as u64 }),
            Cond::Eq => Some(konst(k)),
            _ => None,
        };
    }
    match c {
        Cond::Lt if k > 0 => Some(R { lo: 0, hi: k - 1 }),
        Cond::Le => Some(R { lo: 0, hi: k }),
        Cond::Gt if k < u64::MAX => Some(R { lo: k + 1, hi: u64::MAX }),
        Cond::Ge => Some(R { lo: k, hi: u64::MAX }),
        Cond::Eq => Some(konst(k as i64)),
        _ => None,
    }
}

/// signed `x c k` narrowed further by what is known of x: with x known non-negative a signed
/// upper bound is an unsigned one
fn bound_with(c: Cond, signed: bool, k: i64, have: Option<R>) -> Option<R> {
    if signed {
        if let Some(h) = have {
            if h.hi <= i64::MAX as u64 && k >= 0 {
                return bound(c, false, k as u64);
            }
        }
    }
    bound(c, signed, k as u64)
}

/// Facts that hold on the edge where `c != 0` is `taken`.
fn refine(st: &mut State, c: u32, taken: bool) {
    let Some(&(_, cond, signed, x, y)) = st.cmp.iter().find(|f| f.0 == c) else { return };
    let cond = if taken { cond } else { negate(cond) };
    push_capped(&mut st.rel, (cond, signed, x, y));
    match y {
        Opnd::K(k) => {
            if let Some(b) = bound_with(cond, signed, k, st.get(x)) {
                narrow_lin(st, x, b);
            }
        }
        Opnd::V(yv) => {
            // x < y: x below y's top, y above x's bottom (unsigned, or both natural)
            let (ax, ay) = (st.get(x), st.get(yv));
            let nat = |r: Option<R>| r.map_or(false, |r| r.hi <= i64::MAX as u64);
            if !signed || (nat(ax) && nat(ay)) {
                if let Some(ry) = ay {
                    let hx = match cond {
                        Cond::Lt => ry.hi.checked_sub(1),
                        Cond::Le | Cond::Eq => Some(ry.hi),
                        _ => None,
                    };
                    if let Some(h) = hx {
                        narrow_lin(st, x, R { lo: 0, hi: h });
                    }
                }
                if let Some(rx) = ax {
                    let ly = match cond {
                        Cond::Lt => rx.lo.checked_add(1),
                        Cond::Le | Cond::Eq => Some(rx.lo),
                        _ => None,
                    };
                    if let Some(l) = ly {
                        narrow_lin(st, yv, R { lo: l, hi: u64::MAX });
                    }
                }
            }
        }
    }
}

/// x narrowed to b, and through `x = y + k` (no wrap over the interval) y too.
fn narrow_lin(st: &mut State, x: u32, b: R) {
    st.narrow(x, b);
    let Some(r) = st.get(x) else { return };
    let links: Vec<(u32, i64, u8)> = st.lin.iter().filter(|l| l.0 == x).map(|l| (l.1, l.2, l.3)).collect();
    for (y, k, bits) in links {
        // y = x - k over [r.lo, r.hi], within y's type's natural range
        let lo = r.lo as i128 - k as i128;
        let hi = r.hi as i128 - k as i128;
        let max = if bits >= 64 { u64::MAX as i128 } else { (1i128 << bits) - 1 };
        if lo >= 0 && hi <= max {
            st.narrow(y, R { lo: lo as u64, hi: hi as u64 });
        }
    }
}

/// The interval of a definition's result, from the state before it.
fn def_range(st: &State, i: &Inst) -> Option<R> {
    let g = |v: VReg| st.get(v.0);
    match i {
        Inst::Iconst(_, k) => Some(konst(*k)),
        Inst::Mov(_, s) => g(*s),
        Inst::IBin(op, it, _, a, b) => arith(*op, *it, g(*a)?, g(*b)?).or_else(|| match op {
            IOp::And => {
                let m = nat_max(*it);
                [g(*a), g(*b)].iter().flatten().filter(|r| r.hi <= m).map(|r| R { lo: 0, hi: r.hi }).min_by_key(|r| r.hi)
            }
            _ => None,
        }),
        Inst::IBinI(op, it, _, a, k) => {
            let kr = if *k >= 0 { Some(konst(*k)) } else { None };
            match (op, g(*a), kr) {
                (IOp::Add, Some(ra), None) => arith(IOp::Sub, *it, ra, konst(k.wrapping_neg())),
                (IOp::Sub, Some(ra), None) => arith(IOp::Add, *it, ra, konst(k.wrapping_neg())),
                (_, Some(ra), Some(kr)) => arith(*op, *it, ra, kr),
                (IOp::And, None, Some(kr)) if kr.hi <= nat_max(*it) => Some(R { lo: 0, hi: kr.hi }),
                (IOp::Rem, None, Some(kr)) if !it.signed && kr.lo > 0 => Some(R { lo: 0, hi: kr.lo - 1 }),
                (IOp::Shr, None, Some(kr)) if !it.signed && kr.lo > 0 && kr.lo < 64 => Some(R { lo: 0, hi: nat_max(*it) >> kr.lo }),
                _ => None,
            }
        }
        Inst::ICmp(..) | Inst::ICmpI(..) | Inst::FCmp(..) | Inst::VAny(..) => Some(R { lo: 0, hi: 1 }),
        Inst::Load(Mem::Int(n, false), ..) | Inst::LoadX(Mem::Int(n, false), ..) | Inst::AtomicLoad(Mem::Int(n, false), ..) if *n < 8 => Some(R { lo: 0, hi: (1u64 << (8 * *n as u32)) - 1 }),
        Inst::Conv(Conv::IntToInt(it), _, s) => {
            let b = bits(*it);
            if b >= 64 {
                // the canonical 64-bit value is unchanged
                g(*s)
            } else {
                let m = nat_max(*it);
                match g(*s) {
                    Some(r) if r.hi <= m => Some(r),
                    _ if !it.signed => Some(R { lo: 0, hi: m }),
                    _ => None,
                }
            }
        }
        Inst::Sel(_, _, a, b) => {
            let (ra, rb) = (g(*a)?, g(*b)?);
            Some(R { lo: ra.lo.min(rb.lo), hi: ra.hi.max(rb.hi) })
        }
        _ => None,
    }
}

/// Is `x c y` (x, y vregs) fixed by a relation fact or by `y = x + k` without wrap?
/// The vregs holding the same value as `v` right now (copies of it, copies of those, ...).
fn same_as(st: &State, v: u32) -> Vec<u32> {
    let mut out = vec![v];
    let mut k = 0;
    while k < out.len() && out.len() < 16 {
        let w = out[k];
        k += 1;
        for &(d, s, off, b) in &st.lin {
            if off != 0 || b < 64 {
                continue;
            }
            let o = if d == w { s } else if s == w { d } else { continue };
            if !out.contains(&o) {
                out.push(o);
            }
        }
    }
    out
}

/// `d` and the values it is known to be at most (through chains of selects of minima)
fn above(st: &State, signed: bool, d: u32) -> Vec<u32> {
    let mut out = vec![d];
    let mut k = 0;
    while k < out.len() && out.len() < 12 {
        let w = out[k];
        k += 1;
        for &(ls, ld, lx) in &st.le {
            if ls == signed && ld == w && !out.contains(&lx) {
                out.push(lx);
            }
        }
    }
    out
}

/// the comparison that holds exactly when `c` does not, for the orders only (`x < y` against
/// `x >= y`): decide_rel reads a derived `x < y` as the negation of `y <= x` and so on
fn flip(c: Cond) -> Option<Cond> {
    // c false <=> (c's negation) true, and the negation of Lt is Ge, of Le is Gt: the derivations
    // above only prove Lt / Le, so a Ge / Gt query is false exactly when Lt / Le is implied
    match c {
        Cond::Ge => Some(Cond::Lt),
        Cond::Gt => Some(Cond::Le),
        _ => None,
    }
}

fn decide_rel(st: &State, c: Cond, signed: bool, x: u32, y: Opnd) -> Option<bool> {
    let xs = same_as(st, x);
    let ys: Vec<Opnd> = match y {
        Opnd::V(v) => same_as(st, v).into_iter().map(Opnd::V).collect(),
        k => vec![k],
    };
    for &(rc, rs, rx, ry) in &st.rel {
        if rs == signed && xs.contains(&rx) && ys.contains(&ry) {
            if rc == c {
                return Some(true);
            }
            if rc == negate(c) {
                return Some(false);
            }
        }
    }
    let Opnd::V(yv) = y else { return None };
    // x < d (or <=) and d <= y: d = min(y, ..) after `n = a.len().min(b.len())` and `i < n`
    {
        let ys = same_as(st, yv);
        let implied = |c: Cond| -> bool {
            if !matches!(c, Cond::Lt | Cond::Le) {
                return false;
            }
            st.rel.iter().any(|&(rc, rs, rx, ry)| {
                rs == signed
                    && xs.contains(&rx)
                    && (rc == Cond::Lt || (rc == Cond::Le && c == Cond::Le))
                    && matches!(ry, Opnd::V(d) if above(st, signed, d).iter().any(|u| ys.contains(u)))
            })
        };
        if implied(c) {
            return Some(true);
        }
        if let Some(nc) = flip(c) {
            if implied(nc) {
                return Some(false);
            }
        }
    }
    // y = x + k or x = y + k, the sum not wrapping over x's (y's) interval
    for &(d, s, k, b) in &st.lin {
        let (lhs_is_x, base) = if d == yv && s == x {
            (true, x)
        } else if d == x && s == yv {
            (false, yv)
        } else {
            continue;
        };
        let Some(r) = st.get(base) else { continue };
        let max = if b >= 64 { u64::MAX as i128 } else { (1i128 << b) - 1 };
        if r.lo as i128 + (k as i128) < 0 || r.hi as i128 + (k as i128) > max {
            continue;
        }
        if signed && (r.hi > i64::MAX as u64 || r.hi as i128 + k as i128 > i64::MAX as i128) {
            continue;
        }
        // compare x with x + k (lhs_is_x: y = x + k) or x + k with x
        let diff = if lhs_is_x { k } else { -k }; // y - x
        let t = match c {
            Cond::Lt => diff > 0,
            Cond::Le => diff >= 0,
            Cond::Gt => diff < 0,
            Cond::Ge => diff <= 0,
            Cond::Eq => diff == 0,
            Cond::Ne => diff != 0,
        };
        let f = match c {
            Cond::Lt => diff <= 0,
            Cond::Le => diff < 0,
            Cond::Gt => diff >= 0,
            Cond::Ge => diff > 0,
            Cond::Eq => diff != 0,
            Cond::Ne => diff == 0,
        };
        if t {
            return Some(true);
        }
        if f {
            return Some(false);
        }
    }
    None
}

fn ranges_off(options: &relax_session::Options) -> bool {
    options.no_ranges
}

/// The pass (see the module doc). Returns whether anything changed.
pub fn fold_ranges(options: &relax_session::Options, f: &mut Func) -> bool {
    if ranges_off(options) || f.blocks.len() < 2 {
        return false;
    }
    // a counting-up loop variable anywhere (v = v + k, or v = t after t = v + k)?
    let mut any_iv = false;
    for b in &f.blocks {
        for (k, i) in b.insts.iter().enumerate() {
            match *i {
                Inst::IBinI(IOp::Add, _, d, x, c) if d == x && c >= 1 => any_iv = true,
                Inst::Mov(d, t) if k > 0 && matches!(b.insts[k - 1], Inst::IBinI(IOp::Add, _, t2, x, c) if t2 == t && x == d && c >= 1) => any_iv = true,
                _ => {}
            }
        }
    }
    let mut res = Vec::new();
    if !any_iv || iv_off(options) {
        let mut ed = Edges::default();
        let ch = run(f, &HashMap::default(), false, &mut res, Some(&mut ed));
        return thread_decided(f, &ed) | ch;
    }
    // hypotheses found not to hold, per (header, vreg): left out of the next round
    let mut off: HashMap<(u32, u32), u8> = HashMap::default();
    for round in 0..3 {
        let mut g = f.clone();
        res.clear();
        let mut ed = Edges::default();
        let ch = run(&mut g, &off, true, &mut res, Some(&mut ed));
        if iv_trace() {
            eprintln!("iv: round {} hyps {} failed {}", round, res.len(), res.iter().filter(|r| !r.1).count());
        }
        if res.iter().all(|r| r.1) {
            *f = g;
            return thread_decided(f, &ed) | ch;
        }
        for (key, ok) in &res {
            if !ok {
                off.insert(*key, 1);
            }
        }
        if res.iter().all(|r| !r.1) {
            break;
        }
    }
    res.clear();
    let mut ed = Edges::default();
    let ch = run(f, &HashMap::default(), false, &mut res, Some(&mut ed));
    thread_decided(f, &ed) | ch
}

fn iv_trace() -> bool {
    static E: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *E.get_or_init(|| std::env::var("MAKEPAD_RUN_IV_TRACE").is_ok())
}

fn iv_off(options: &relax_session::Options) -> bool {
    options.no_iv_ranges
}

/// One forward pass. `spec`: loop variables get the hypotheses `lv` allows; each one used is
/// reported in `res` as ((header, vreg), holds on every back edge).
/// `out`: the states on the edges and the blocks' reverse-postorder numbers (thread_decided).
fn run(f: &mut Func, lv: &HashMap<(u32, u32), u8>, spec: bool, res: &mut Vec<((u32, u32), bool)>, out: Option<&mut Edges>) -> bool {
    let n = f.blocks.len();
    let reach = reachable(f);
    let idom = relax_backend::dom::idoms(f);
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (b, blk) in f.blocks.iter().enumerate() {
        if reach[b] {
            for s in blk.term.succs() {
                if !preds[s as usize].contains(&(b as u32)) {
                    preds[s as usize].push(b as u32);
                }
            }
        }
    }
    // reverse postorder
    let mut rpo: Vec<u32> = Vec::with_capacity(n);
    {
        let mut seen = vec![false; n];
        let mut stack: Vec<(u32, Vec<u32>)> = vec![(0, f.blocks[0].term.succs().collect())];
        seen[0] = true;
        while let Some((b, rest)) = stack.last_mut() {
            if let Some(s) = rest.pop() {
                if !seen[s as usize] {
                    seen[s as usize] = true;
                    let ss: Vec<u32> = f.blocks[s as usize].term.succs().collect();
                    stack.push((s, ss));
                }
            } else {
                rpo.push(*b);
                stack.pop();
            }
        }
        rpo.reverse();
    }
    // loads whose type limits their values (Func::ranged): (width, range) of the defining Load
    let marks: HashMap<u32, (u8, R)> = f.ranged.iter().map(|&(v, b, lo, hi)| (v.0, (b, R { lo, hi }))).collect();
    let mut order = vec![u32::MAX; n];
    for (i, b) in rpo.iter().enumerate() {
        order[*b as usize] = i as u32;
    }
    let dominates = |a: u32, mut b: u32| -> bool {
        loop {
            if a == b {
                return true;
            }
            let p = idom[b as usize];
            if p == b || p as usize >= n {
                return false;
            }
            b = p;
        }
    };
    let nv = f.vregs.len();
    // uses per vreg (the div/rem rewrite needs its temporaries used once)
    let mut nuse = vec![0u32; nv];
    for b in &f.blocks {
        for i in &b.insts {
            relax_backend::regalloc::for_uses(i, |u| {
                if (u.0 as usize) < nv {
                    nuse[u.0 as usize] += 1
                }
            });
        }
        match &b.term {
            Term::Branch(c, _, _) | Term::Switch(c, _) => nuse[c.0 as usize] += 1,
            Term::Ret(vs) => vs.iter().for_each(|v| nuse[v.0 as usize] += 1),
            _ => {}
        }
    }
    // hypotheses in use: (header, vreg, interval, back-edge preds)
    let mut hyps: Vec<(u32, u32, R, Vec<u32>)> = Vec::new();
    // per edge (from, to): the state on it; filled as blocks are processed in RPO
    let mut edge: HashMap<(u32, u32), State> = HashMap::default();
    let mut changed = false;
    for &b in &rpo {
        let bi = b as usize;
        // in-state: the meet of processed predecessors; a loop header keeps what its body
        // never defines; an unprocessed predecessor that is not a back edge (irreducible
        // flow) leaves no facts
        let mut st: Option<State> = None;
        let mut back: Vec<u32> = Vec::new();
        let mut irreducible = false;
        for &p in &preds[bi] {
            if order[p as usize] >= order[bi] {
                if dominates(b, p) {
                    back.push(p);
                } else {
                    irreducible = true;
                }
                continue;
            }
            let e = edge.get(&(p, b)).cloned().unwrap_or_default();
            st = Some(match st {
                None => e,
                Some(s) => s.join(&e),
            });
        }
        let mut st = if irreducible || bi == 0 { State::default() } else { st.unwrap_or_default() };
        if !back.is_empty() && !irreducible {
            // the natural loop of these back edges: blocks reaching them without passing b
            let mut inl = vec![false; n];
            inl[bi] = true;
            let mut work: Vec<u32> = back.clone();
            while let Some(x) = work.pop() {
                if inl[x as usize] {
                    continue;
                }
                inl[x as usize] = true;
                for &p in &preds[x as usize] {
                    if !inl[p as usize] {
                        work.push(p);
                    }
                }
            }
            let mut defd = vec![false; nv];
            // loop variables: v -> (type, largest step); `bad`: some other definition
            let mut ivk: HashMap<u32, (IntTy, i64)> = HashMap::default();
            let mut bad: Vec<u32> = Vec::new();
            for (x, blk) in f.blocks.iter().enumerate() {
                if inl[x] {
                    for (k, i) in blk.insts.iter().enumerate() {
                        for_defs(i, |d| {
                            if (d.0 as usize) < nv {
                                defd[d.0 as usize] = true;
                            }
                        });
                        if !spec {
                            continue;
                        }
                        match *i {
                            Inst::IBinI(IOp::Add, it, d, x2, c) if d == x2 && c >= 1 => {
                                let e = ivk.entry(d.0).or_insert((it, c));
                                e.1 = e.1.max(c);
                            }
                            Inst::Mov(d, t) if k > 0 && matches!(blk.insts[k - 1], Inst::IBinI(IOp::Add, _, t2, x2, c) if t2 == t && x2 == d && c >= 1) => {
                                if let Inst::IBinI(IOp::Add, it, _, _, c) = blk.insts[k - 1] {
                                    let e = ivk.entry(d.0).or_insert((it, c));
                                    e.1 = e.1.max(c);
                                }
                            }
                            _ => for_defs(i, |d| bad.push(d.0)),
                        }
                    }
                }
            }
            let mut add: Vec<(u32, R)> = Vec::new();
            if spec && !ivk.is_empty() {
                // a loop variable is bounded when the loop compares it (or its sum v + k) with a
                // constant or with a value the loop does not define and whose interval is known
                // on entry; others would fail the check and cost a round
                let mut bounded: Vec<u32> = Vec::new();
                for (x, blk) in f.blocks.iter().enumerate() {
                    if !inl[x] {
                        continue;
                    }
                    for (k, i) in blk.insts.iter().enumerate() {
                        let (a, ok) = match *i {
                            Inst::ICmpI(_, _, _, a, _) => (a, true),
                            Inst::ICmp(_, _, _, a, y) => (a, !defd.get(y.0 as usize).copied().unwrap_or(true) && st.get(y.0).is_some()),
                            _ => continue,
                        };
                        if !ok {
                            continue;
                        }
                        if ivk.contains_key(&a.0) {
                            bounded.push(a.0);
                        } else if let Some(Inst::IBinI(IOp::Add, _, t, v, c)) = blk.insts[..k].iter().rev().find(|j| {
                            let mut d = false;
                            for_defs(j, |w| d |= w == a);
                            d
                        }) {
                            if *t == a && *c >= 1 && ivk.contains_key(&v.0) {
                                bounded.push(v.0);
                            }
                        }
                    }
                }
                for (&v, &(it, kmax)) in &ivk {
                    if bad.contains(&v) || !bounded.contains(&v) || lv.contains_key(&(b, v)) || f.vregs.get(v as usize) != Some(&Cls::I) {
                        continue;
                    }
                    let Some(e) = st.get(v) else { continue };
                    let m = nat_max(it);
                    if m < kmax as u64 {
                        continue;
                    }
                    let hi = m - kmax as u64;
                    if e.hi > hi {
                        continue;
                    }
                    add.push((v, R { lo: e.lo, hi }));
                }
            }
            st.keep_outside(&defd);
            for (v, h) in add {
                st.rng.insert(v, h);
                hyps.push((b, v, h, back.clone()));
            }
        }
        // the instructions
        let blk = &mut f.blocks[bi];
        for k in 0..blk.insts.len() {
            nonneg_div(&mut blk.insts, k, &st, &nuse);
            changed |= step(&mut st, &mut blk.insts[k], &marks);
        }
        // out-edges
        match blk.term {
            Term::Branch(c, t, e) if t != e => {
                let mut a = st.clone();
                refine(&mut a, c.0, true);
                edge.insert((b, t), a);
                let mut z = st;
                refine(&mut z, c.0, false);
                edge.insert((b, e), z);
            }
            _ => {
                let ss: Vec<u32> = blk.term.succs().collect();
                for s in ss {
                    edge.insert((b, s), st.clone());
                }
            }
        }
    }
    for (h, v, r, back) in &hyps {
        let ok = back.iter().all(|p| match edge.get(&(*p, *h)).and_then(|s| s.get(*v)) {
            Some(x) => x.lo >= r.lo && x.hi <= r.hi,
            None => false,
        });
        res.push(((*h, *v), ok));
    }
    if let Some(o) = out {
        o.edge = edge;
        o.order = order;
        o.marks = marks;
    }
    changed
}

/// What `run` knew on each edge, for threading.
#[derive(Default)]
struct Edges {
    edge: HashMap<(u32, u32), State>,
    order: Vec<u32>,
    marks: HashMap<u32, (u8, R)>,
}

/// Blocks this small are copied onto the edges that decide their branch (thread_decided).
const THREAD_INSTS: usize = 8;

/// Jump threading by the facts on each edge (LLVM's JumpThreading over LazyValueInfo): a small
/// block B ending in a branch whose condition the state on an incoming edge P -> B decides --
/// while the meet of all of B's edges does not -- gets a copy B' (B's instructions, then a jump to
/// the decided successor) that P enters instead. Every path still runs B's instructions once
/// (the copy defines the same vregs), only the branch is gone. `for c in s.chars()` after
/// inlining is the case: the iterator's `Option<char>` is tested twice per character (the
/// decoder's `next`, then the loop's match), once against a constant None on the end path and
/// once against a byte the decoder just loaded -- both decided on their edges, joined at the test.
/// B is never a loop header (nor entered by irreducible flow): a copy of a header would enter its
/// loop past it.
fn thread_decided(f: &mut Func, ed: &Edges) -> bool {
    // MAKEPAD_RUN_OPT=-threadedge: off (bisection)
    if relax_backend::stats::opt_env_off("-threadedge") {
        return false;
    }
    let n = f.blocks.len();
    let reach = reachable(f);
    let mut preds: Vec<Vec<u32>> = vec![Vec::new(); n];
    for (b, blk) in f.blocks.iter().enumerate() {
        if reach[b] {
            for s in blk.term.succs() {
                if !preds[s as usize].contains(&(b as u32)) {
                    preds[s as usize].push(b as u32);
                }
            }
        }
    }
    let mut plan: Vec<(u32, u32, u32)> = Vec::new();
    for b in 1..n {
        let blk = &f.blocks[b];
        let Term::Branch(c, t, e) = blk.term else { continue };
        if !reach[b] || t == e || preds[b].len() < 2 || blk.insts.len() > THREAD_INSTS || blk.insts.iter().any(|i| matches!(i, Inst::Call(..))) {
            continue;
        }
        let ob = ed.order.get(b).copied().unwrap_or(u32::MAX);
        // (nor a landing pad: an invoke's unwind edge cannot be retargeted to a copy)
        if ob == u32::MAX || preds[b].iter().any(|&p| ed.order.get(p as usize).map_or(true, |&o| o >= ob) || matches!(f.blocks[p as usize].term, Term::Invoke(..))) {
            continue;
        }
        for &p in &preds[b] {
            if !matches!(f.blocks[p as usize].term, Term::Jump(_) | Term::Branch(..)) {
                continue;
            }
            let Some(st0) = ed.edge.get(&(p, b as u32)) else { continue };
            let mut st = st0.clone();
            for i in &blk.insts {
                let mut j = i.clone();
                step(&mut st, &mut j, &ed.marks);
            }
            if let Some(R { lo, hi }) = st.get(c.0) {
                if lo == hi {
                    let to = if lo != 0 { t } else { e };
                    if to as usize != b {
                        plan.push((p, b as u32, to));
                    }
                }
            }
        }
    }
    if plan.is_empty() {
        return false;
    }
    for (p, b, to) in plan {
        let src = &f.blocks[b as usize];
        let nb = Block { insts: src.insts.clone(), pos: src.pos.clone(), term: Term::Jump(to), term_pos: src.term_pos, count: src.count };
        let id = f.blocks.len() as u32;
        f.blocks.push(nb);
        f.blocks[p as usize].term.map_succs(|s| if s == b { id } else { s });
    }
    true
}

/// One instruction's transfer: decides the compares (and `(b + k) - b`) the state fixes, rewriting
/// `i` into a constant, then applies its definitions to the state. Returns whether `i` changed.
fn step(st: &mut State, i: &mut Inst, marks: &HashMap<u32, (u8, R)>) -> bool {
    let mut changed = false;
    // decide compares first (the state before the instruction)
    let fold = match &*i {
        Inst::ICmpI(c, s, d, x, k) => {
            let by_rng = st.get(x.0).and_then(|rx| if *s && *k < 0 { if rx.hi <= i64::MAX as u64 { decide_neg(*c) } else { None } } else { decide(*c, *s, rx, konst(*k)) });
            by_rng.or_else(|| decide_rel(st, *c, *s, x.0, Opnd::K(*k))).map(|t| (*d, t))
        }
        Inst::ICmp(c, s, d, x, y) => {
            let by_rng = match (st.get(x.0), st.get(y.0)) {
                (Some(a), Some(b)) => decide(*c, *s, a, b),
                _ => None,
            };
            by_rng.or_else(|| decide_rel(st, *c, *s, x.0, Opnd::V(y.0))).map(|t| (*d, t))
        }
        _ => None,
    };
    // (b + k) - b
    let sub_k = match &*i {
        Inst::IBin(IOp::Sub, it, d, p, q) => st.lin.iter().find(|l| l.0 == p.0 && l.1 == q.0 && (l.3 as u32) == bits(*it)).map(|l| (*d, extend_const(l.2, *it))),
        _ => None,
    };
    let mut r = def_range(&st, i);
    if let Inst::Load(Mem::Int(w, false), d, ..) | Inst::LoadX(Mem::Int(w, false), d, ..) = &*i {
        if let Some(&(mw, m)) = marks.get(&d.0) {
            if mw == *w {
                r = Some(match r {
                    Some(x) if x.lo.max(m.lo) <= x.hi.min(m.hi) => R { lo: x.lo.max(m.lo), hi: x.hi.min(m.hi) },
                    _ => m,
                });
            }
        }
    }
    let cmpf = match &*i {
        Inst::ICmpI(c, s, d, x, k) if d != x => Some((d.0, *c, *s, x.0, Opnd::K(*k))),
        Inst::ICmp(c, s, d, x, y) if d != x && d != y => Some((d.0, *c, *s, x.0, Opnd::V(y.0))),
        _ => None,
    };
    // d = c ? x : y with c = (x < y): d = min(x, y), d <= x and d <= y
    let sel_min = match &*i {
        Inst::Sel(d, c, a, b2) if d != a && d != b2 => st.cmp.iter().find(|f| f.0 == c.0).and_then(|&(_, cond, sg, x, y)| {
            if matches!(cond, Cond::Lt | Cond::Le) && x == a.0 && y == Opnd::V(b2.0) {
                Some((sg, a.0, b2.0))
            } else {
                None
            }
        }),
        _ => None,
    };
    let linf = match &*i {
        // exact as u64 intervals only for 64-bit or unsigned types (a narrow signed
        // value is sign-extended)
        Inst::IBinI(IOp::Add, it, d, x, k) if d != x && (bits(*it) == 64 || !it.signed) => Some((d.0, x.0, *k, bits(*it) as u8)),
        Inst::IBinI(IOp::Sub, it, d, x, k) if d != x && *k != i64::MIN && (bits(*it) == 64 || !it.signed) => Some((d.0, x.0, -*k, bits(*it) as u8)),
        Inst::Mov(d, x) if d != x => Some((d.0, x.0, 0, 64)),
        _ => None,
    };
    if let Some((d, t)) = fold {
        *i = Inst::Iconst(d, t as i64);
        changed = true;
    } else if let Some((d, k)) = sub_k {
        *i = Inst::Iconst(d, k);
        changed = true;
    }
    let mut ds: Vec<u32> = Vec::new();
    for_defs(i, |d| ds.push(d.0));
    // a copy `d = s` holds what is known of s's order against other values: a later
    // redefinition of s (the counter's increment) leaves the facts about the copy, which
    // is the value the bounds check after the step reads
    let copied: Vec<(Cond, bool, u32, Opnd)> = match &*i {
        Inst::Mov(d, x) if d != x => st
            .rel
            .iter()
            .filter_map(|&(c, sg, a, b)| {
                if a == x.0 && b != Opnd::V(d.0) {
                    Some((c, sg, d.0, b))
                } else if b == Opnd::V(x.0) && a != d.0 {
                    Some((c, sg, a, Opnd::V(d.0)))
                } else {
                    None
                }
            })
            .collect(),
        _ => Vec::new(),
    };
    for d in &ds {
        st.kill(*d);
    }
    for c in copied {
        push_capped(&mut st.rel, c);
    }
    if let [d] = ds[..] {
        if let Some(l) = sel_min {
            push_capped(&mut st.le, (l.0, d, l.1));
            push_capped(&mut st.le, (l.0, d, l.2));
        }
        let r = match &*i {
            Inst::Iconst(_, k) => Some(konst(*k)),
            _ => r,
        };
        if let Some(r) = r {
            st.rng.insert(d, r);
        }
        if !matches!(i, Inst::Iconst(..)) {
            if let Some(c) = cmpf {
                push_capped(&mut st.cmp, c);
            }
            if let Some(l) = linf {
                push_capped(&mut st.lin, l);
            }
        }
    }
    changed
}

/// `div_by_const`'s signed division / remainder by c = 2^sh at insts[k] (sg = x >> 63;
/// bias = sg >>> (64 - sh); t = x + bias; then d = t >> sh, or m = t & -c; d = x - m) with x
/// known non-negative: bias is 0, so d = x >>> sh, or d = x & (c - 1). The temporaries become
/// dead constants (simplify removes them).
fn nonneg_div(insts: &mut [Inst], k: usize, st: &State, nuse: &[u32]) {
    let Inst::IBinI(IOp::Shr, it0, sg, x, 63) = insts[k] else { return };
    if it0 != I64 || k + 3 >= insts.len() {
        return;
    }
    match st.get(x.0) {
        Some(r) if r.hi <= i64::MAX as u64 => {}
        _ => return,
    }
    let once = |v: VReg| nuse.get(v.0 as usize) == Some(&1);
    let Inst::IBinI(IOp::Shr, it1, bias, sg2, s2) = insts[k + 1] else { return };
    let Inst::IBin(IOp::Add, it2, t, x2, bias2) = insts[k + 2] else { return };
    if it1 != U64 || it2 != I64 || sg2 != sg || bias2 != bias || x2 != x || !(1..63).contains(&s2) || !once(sg) || !once(bias) || !once(t) || t == x || sg == x || bias == x {
        return;
    }
    let sh = 64 - s2;
    match insts[k + 3] {
        Inst::IBinI(IOp::Shr, it, d, t2, sh2) if t2 == t && sh2 == sh => {
            insts[k] = Inst::Iconst(sg, 0);
            insts[k + 1] = Inst::Iconst(bias, 0);
            insts[k + 2] = Inst::Iconst(t, 0);
            insts[k + 3] = Inst::IBinI(IOp::Shr, it, d, x, sh);
        }
        Inst::IBinI(IOp::And, it3, m, t2, mk) if k + 4 < insts.len() && t2 == t && it3 == I64 && mk == -(1i64 << sh) && once(m) && m != x => {
            if let Inst::IBin(IOp::Sub, it, d, x3, m2) = insts[k + 4] {
                if x3 == x && m2 == m {
                    insts[k] = Inst::Iconst(sg, 0);
                    insts[k + 1] = Inst::Iconst(bias, 0);
                    insts[k + 2] = Inst::Iconst(t, 0);
                    insts[k + 3] = Inst::Iconst(m, 0);
                    insts[k + 4] = Inst::IBinI(IOp::And, it, d, x, (1i64 << sh) - 1);
                }
            }
        }
        _ => {}
    }
}

/// Fact lists keep their newest MAX_FACTS entries (joins compare them pairwise).
const MAX_FACTS: usize = 48;

fn push_capped<T>(v: &mut Vec<T>, x: T) {
    if v.len() >= MAX_FACTS {
        v.remove(0);
    }
    v.push(x);
}

/// A signed compare of a non-negative x with a negative constant.
fn decide_neg(c: Cond) -> Option<bool> {
    match c {
        Cond::Gt | Cond::Ge | Cond::Ne => Some(true),
        Cond::Lt | Cond::Le | Cond::Eq => Some(false),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_intervals() {
        let a = R { lo: 0, hi: 32767 };
        assert_eq!(decide(Cond::Lt, false, a, konst(33026)), Some(true));
        assert_eq!(decide(Cond::Ge, false, a, konst(33026)), Some(false));
        assert_eq!(decide(Cond::Lt, false, a, konst(100)), None);
        assert_eq!(decide(Cond::Eq, false, a, konst(40000)), Some(false));
        // signed compares only of natural intervals
        assert_eq!(decide(Cond::Lt, true, R { lo: 0, hi: u64::MAX }, konst(3)), None);
    }

    #[test]
    fn arith_no_wrap() {
        let m = R { lo: 0, hi: 32767 };
        assert_eq!(arith(IOp::Add, U64, m, R { lo: 1, hi: 258 }), Some(R { lo: 1, hi: 33025 }));
        assert_eq!(arith(IOp::Sub, U64, R { lo: 1, hi: 33025 }, konst(1)), Some(R { lo: 0, hi: 33024 }));
        // u8 + u8 may wrap at 8 bits: no interval
        assert_eq!(arith(IOp::Add, IntTy { bits: 8, signed: false }, R { lo: 0, hi: 200 }, R { lo: 0, hi: 100 }), None);
        // sub that may go below zero: none
        assert_eq!(arith(IOp::Sub, U64, R { lo: 0, hi: 5 }, konst(1)), None);
    }

    #[test]
    fn bounds() {
        assert_eq!(bound(Cond::Lt, false, 258), Some(R { lo: 0, hi: 257 }));
        assert_eq!(bound(Cond::Lt, true, 3), None);
        assert_eq!(bound_with(Cond::Lt, true, 3, Some(R { lo: 0, hi: 10 })), Some(R { lo: 0, hi: 2 }));
    }
}
