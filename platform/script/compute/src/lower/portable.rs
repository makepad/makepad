//! `math: portable`: the fdlibm kernels of `makepad_csg_math::portable`,
//! lowered to AIR f64 ops. Every function here gives the same bits as its
//! Rust original (tested bit for bit in tests/portable.rs), so a kernel
//! and a Rust generator using `PortableFloat` agree exactly.
//!
//! The Rust code branches; AIR selects. Each early `return` of the original
//! becomes a select, nested in the original's order (the first condition
//! that holds wins), and every path is computed. Paths computed with
//! out-of-range inputs are never selected, and no AIR op traps. Bounded
//! loops of the original (`scalbn`, integer powers) are unrolled to their
//! reachable trip counts.

use super::*;

impl Builder {
    fn dc(&mut self, x: f64) -> Val {
        self.cd(x)
    }
    fn db(&mut self, b: Bin, x: Val, y: Val) -> Val {
        self.def(Ty::F64, Op::Bin(b, x, y))
    }
    fn dadd(&mut self, x: Val, y: Val) -> Val {
        self.db(Bin::AddD, x, y)
    }
    fn dsub(&mut self, x: Val, y: Val) -> Val {
        self.db(Bin::SubD, x, y)
    }
    fn dmul(&mut self, x: Val, y: Val) -> Val {
        self.db(Bin::MulD, x, y)
    }
    fn ddiv(&mut self, x: Val, y: Val) -> Val {
        self.db(Bin::DivD, x, y)
    }
    fn dun(&mut self, u: Un, x: Val) -> Val {
        self.def(Ty::F64, Op::Un(u, x))
    }
    fn dcmp(&mut self, cc: Cmp, x: Val, y: Val) -> Val {
        self.def(Ty::Bool, Op::CmpD(cc, x, y))
    }
    fn dcmpc(&mut self, cc: Cmp, x: Val, c: f64) -> Val {
        let c = self.dc(c);
        self.dcmp(cc, x, c)
    }
    fn band(&mut self, a: Val, b: Val) -> Val {
        self.def(Ty::Bool, Op::Bin(Bin::AndB, a, b))
    }
    fn bor(&mut self, a: Val, b: Val) -> Val {
        self.def(Ty::Bool, Op::Bin(Bin::OrB, a, b))
    }
    fn bnot(&mut self, a: Val) -> Val {
        self.def(Ty::Bool, Op::Un(Un::NotB, a))
    }
    pub(super) fn f2d(&mut self, x: Val) -> Val {
        self.dun(Un::F2D, x)
    }
    pub(super) fn d2f(&mut self, x: Val) -> Val {
        self.def(Ty::F32, Op::Un(Un::D2F, x))
    }
    fn is_nan_d(&mut self, x: Val) -> Val {
        self.dcmp(Cmp::Ne, x, x)
    }
    /// `x.is_finite()`: |x| < inf (false for NaN).
    fn is_finite_d(&mut self, x: Val) -> Val {
        let a = self.dun(Un::AbsD, x);
        self.dcmpc(Cmp::Lt, a, f64::INFINITY)
    }
    /// `x.is_sign_negative()`: the sign bit.
    fn sign_neg_d(&mut self, x: Val) -> Val {
        let hi = self.def(Ty::I32, Op::Un(Un::HiD, x));
        let z = self.ci(0);
        self.cmpi(Cmp::Lt, hi, z)
    }
    fn dneg(&mut self, x: Val) -> Val {
        self.dun(Un::NegD, x)
    }
    /// `if c { -v } else { v }`.
    fn dneg_if(&mut self, c: Val, v: Val) -> Val {
        let n = self.dneg(v);
        self.sel(c, n, v)
    }

    fn k_sin(&mut self, x: Val) -> Val {
        const S1: f64 = -1.66666666666666324348e-01;
        const S2: f64 = 8.33333333332248946124e-03;
        const S3: f64 = -1.98412698298579493134e-04;
        const S4: f64 = 2.75573137070700676789e-06;
        const S5: f64 = -2.50507602534068634195e-08;
        const S6: f64 = 1.58969099521155010221e-10;
        let z = self.dmul(x, x);
        let v = self.dmul(z, x);
        // S2 + z * (S3 + z * (S4 + z * (S5 + z * S6)))
        let r = self.horner_d(z, &[S6, S5, S4, S3, S2]);
        let zr = self.dmul(z, r);
        let s1 = self.dc(S1);
        let t = self.dadd(s1, zr);
        let t = self.dmul(v, t);
        self.dadd(x, t)
    }

    /// `c[n-1] + z * (c[n-2] + ... + z * c[0])` evaluated innermost first
    /// (the nesting of the fdlibm source): coefficients innermost first.
    fn horner_d(&mut self, z: Val, inner_first: &[f64]) -> Val {
        let mut acc = self.dc(inner_first[0]);
        for c in &inner_first[1..] {
            let m = self.dmul(z, acc);
            let c = self.dc(*c);
            acc = self.dadd(c, m);
        }
        acc
    }

    fn k_cos(&mut self, x: Val) -> Val {
        const C1: f64 = 4.16666666666666019037e-02;
        const C2: f64 = -1.38888888888741095749e-03;
        const C3: f64 = 2.48015872894767294178e-05;
        const C4: f64 = -2.75573143513906633035e-07;
        const C5: f64 = 2.08757232129817482790e-09;
        const C6: f64 = -1.13596475577881948265e-11;
        let z = self.dmul(x, x);
        let w = self.dmul(z, z);
        // z * (C1 + z * (C2 + z * C3)) + w * w * (C4 + z * (C5 + z * C6))
        let a = self.horner_d(z, &[C3, C2, C1]);
        let a = self.dmul(z, a);
        let b = self.horner_d(z, &[C6, C5, C4]);
        let ww = self.dmul(w, w);
        let b = self.dmul(ww, b);
        let r = self.dadd(a, b);
        let hz = self.dmulc(z, 0.5);
        let one = self.dc(1.0);
        let w = self.dsub(one, hz);
        // w + (((1.0 - w) - hz) + z * r)
        let t = self.dsub(one, w);
        let t = self.dsub(t, hz);
        let zr = self.dmul(z, r);
        let t = self.dadd(t, zr);
        self.dadd(w, t)
    }

    fn dmulc(&mut self, x: Val, c: f64) -> Val {
        let c = self.dc(c);
        // Constant on the left, as the source writes `0.5 * z`.
        self.dmul(c, x)
    }

    /// x = n·π/2 + r (three-part Cody-Waite): (n mod 4 as i32, r).
    fn reduce_d(&mut self, x: Val) -> (Val, Val) {
        const INVPIO2: f64 = 6.36619772367581382433e-01;
        const PIO2_1: f64 = 1.57079632673412561417e+00;
        const PIO2_2: f64 = 6.07710050630396597660e-11;
        const PIO2_2T: f64 = 2.02226624879595063154e-21;
        let ax = self.dun(Un::AbsD, x);
        let small = self.dcmpc(Cmp::Le, ax, core::f64::consts::FRAC_PI_4);
        let inv = self.dc(INVPIO2);
        let n = self.dmul(x, inv);
        let n = self.dun(Un::RoundD, n);
        let p1 = self.dc(PIO2_1);
        let t = self.dmul(n, p1);
        let y = self.dsub(x, t);
        let p2 = self.dc(PIO2_2);
        let w = self.dmul(n, p2);
        let r = self.dsub(y, w);
        let p2t = self.dc(PIO2_2T);
        let a = self.dmul(n, p2t);
        let b = self.dsub(y, r);
        let b = self.dsub(b, w);
        let w2 = self.dsub(a, b);
        // n.rem_euclid(4.0): n % 4 (exact for integral n), + 4 if negative.
        let q4 = self.dmulc(n, 0.25);
        let q4 = self.dun(Un::TruncD, q4);
        let four = self.dc(4.0);
        let m = self.dmul(four, q4);
        let rem = self.dsub(n, m);
        let neg = self.dcmpc(Cmp::Lt, rem, 0.0);
        let rp = self.dadd(rem, four);
        let rem = self.sel(neg, rp, rem);
        let q = self.def(Ty::I32, Op::Un(Un::D2I, rem));
        let rr = self.dsub(r, w2);
        let zero = self.ci(0);
        let q = self.sel(small, zero, q);
        let rr = self.sel(small, x, rr);
        (q, rr)
    }

    /// Picks `vals[q]` for q in 0..4.
    fn quadrant(&mut self, q: Val, vals: [Val; 4]) -> Val {
        let one = self.ci(1);
        let two = self.ci(2);
        let is1 = self.cmpi(Cmp::Eq, q, one);
        let is2 = self.cmpi(Cmp::Eq, q, two);
        let zero = self.ci(0);
        let is0 = self.cmpi(Cmp::Eq, q, zero);
        // match q { 0 => .., 1 => .., 2 => .., _ => .. }
        let v = self.sel(is2, vals[2], vals[3]);
        let v = self.sel(is1, vals[1], v);
        self.sel(is0, vals[0], v)
    }

    fn nan_unless_finite(&mut self, x: Val, v: Val) -> Val {
        let fin = self.is_finite_d(x);
        let nan = self.dc(f64::NAN);
        self.sel(fin, v, nan)
    }

    pub(super) fn p_sin(&mut self, x: Val) -> Val {
        let (q, r) = self.reduce_d(x);
        let s = self.k_sin(r);
        let c = self.k_cos(r);
        let ns = self.dneg(s);
        let nc = self.dneg(c);
        let v = self.quadrant(q, [s, c, ns, nc]);
        self.nan_unless_finite(x, v)
    }

    pub(super) fn p_cos(&mut self, x: Val) -> Val {
        let (q, r) = self.reduce_d(x);
        let s = self.k_sin(r);
        let c = self.k_cos(r);
        let ns = self.dneg(s);
        let nc = self.dneg(c);
        let v = self.quadrant(q, [c, ns, nc, s]);
        self.nan_unless_finite(x, v)
    }

    pub(super) fn p_tan(&mut self, x: Val) -> Val {
        // sin_cos then s / c (both NaN when x is not finite).
        let (q, r) = self.reduce_d(x);
        let s = self.k_sin(r);
        let c = self.k_cos(r);
        let ns = self.dneg(s);
        let nc = self.dneg(c);
        let sv = self.quadrant(q, [s, c, ns, nc]);
        let cv = self.quadrant(q, [c, ns, nc, s]);
        let sv = self.nan_unless_finite(x, sv);
        let cv = self.nan_unless_finite(x, cv);
        self.ddiv(sv, cv)
    }

    pub(super) fn p_atan(&mut self, x: Val) -> Val {
        const ATANHI: [f64; 4] = [4.63647609000806093515e-01, 7.85398163397448278999e-01, 9.82793723247329054082e-01, 1.57079632679489655800e+00];
        const ATANLO: [f64; 4] = [2.26987774529616870924e-17, 3.06161699786838301793e-17, 1.39033110312309984516e-17, 6.12323399573676603587e-17];
        const AT: [f64; 11] = [
            3.33333333333329318027e-01, -1.99999999998764832476e-01, 1.42857142725034663711e-01, -1.11111104054623557880e-01,
            9.09088713343650656196e-02, -7.69187620504482999495e-02, 6.66107313738753120669e-02, -5.83357013379057348645e-02,
            4.97687799461593236017e-02, -3.65315727442169155270e-02, 1.62858201153657823623e-02,
        ];
        let sign = self.sign_neg_d(x);
        let ax = self.dun(Un::AbsD, x);
        let c0 = self.dcmpc(Cmp::Lt, ax, 0.4375);
        let c1 = self.dcmpc(Cmp::Lt, ax, 0.6875);
        let c2 = self.dcmpc(Cmp::Lt, ax, 1.1875);
        let c3 = self.dcmpc(Cmp::Lt, ax, 2.4375);
        let c4 = self.dcmpc(Cmp::Lt, ax, 2f64.powi(66));
        let one = self.dc(1.0);
        let two = self.dc(2.0);
        // id 0: (2ax - 1) / (2 + ax)
        let t = self.dmul(two, ax);
        let t = self.dsub(t, one);
        let d = self.dadd(two, ax);
        let x0 = self.ddiv(t, d);
        // id 1: (ax - 1) / (ax + 1)
        let t = self.dsub(ax, one);
        let d = self.dadd(ax, one);
        let x1 = self.ddiv(t, d);
        // id 2: (ax - 1.5) / (1 + 1.5 ax)
        let h = self.dc(1.5);
        let t = self.dsub(ax, h);
        let d = self.dmul(h, ax);
        let d = self.dadd(one, d);
        let x2 = self.ddiv(t, d);
        // id 3: -1 / ax
        let m1 = self.dc(-1.0);
        let x3 = self.ddiv(m1, ax);
        let xr = self.sel(c3, x2, x3);
        let xr = self.sel(c2, x1, xr);
        let xr = self.sel(c1, x0, xr);
        let xr = self.sel(c0, x, xr);
        let i0 = self.ci(0);
        let i1 = self.ci(1);
        let i2 = self.ci(2);
        let i3 = self.ci(3);
        let im = self.ci(-1);
        let id = self.sel(c3, i2, i3);
        let id = self.sel(c2, i1, id);
        let id = self.sel(c1, i0, id);
        let id = self.sel(c0, im, id);
        let z = self.dmul(xr, xr);
        let w = self.dmul(z, z);
        let s1 = self.horner_d(w, &[AT[10], AT[8], AT[6], AT[4], AT[2], AT[0]]);
        let s1 = self.dmul(z, s1);
        let s2 = self.horner_d(w, &[AT[9], AT[7], AT[5], AT[3], AT[1]]);
        let s2 = self.dmul(w, s2);
        let ss = self.dadd(s1, s2);
        let xs = self.dmul(xr, ss);
        let small = self.dsub(xr, xs);
        let hi = self.pick4(id, ATANHI);
        let lo = self.pick4(id, ATANLO);
        let t = self.dsub(xs, lo);
        let t = self.dsub(t, xr);
        let v = self.dsub(hi, t);
        let v = self.dneg_if(sign, v);
        let zero = self.ci(0);
        let neg_id = self.cmpi(Cmp::Lt, id, zero);
        let r = self.sel(neg_id, small, v);
        // |x| >= 2^66: ±π/2.
        let big = self.dc(ATANHI[3]);
        let big = self.dneg_if(sign, big);
        let r = self.sel(c4, r, big);
        // Tiny: x itself.
        let tiny = self.dcmpc(Cmp::Lt, ax, 1e-27);
        let r = self.sel(tiny, x, r);
        let nan = self.is_nan_d(x);
        let qn = self.dc(f64::NAN);
        self.sel(nan, qn, r)
    }

    /// One of four constants by an index in 0..4 (anything else: the last).
    fn pick4(&mut self, id: Val, c: [f64; 4]) -> Val {
        let v: [Val; 4] = [self.dc(c[0]), self.dc(c[1]), self.dc(c[2]), self.dc(c[3])];
        self.quadrant(id, v)
    }

    pub(super) fn p_atan2(&mut self, y: Val, x: Val) -> Val {
        use core::f64::consts::{FRAC_PI_2, FRAC_PI_4, PI};
        let neg_y = self.sign_neg_d(y);
        let signed = |b: &mut Self, v: f64| {
            let c = b.dc(v);
            b.dneg_if(neg_y, c)
        };
        // General case: atan(|y / x|) by quadrant.
        let q = self.ddiv(y, x);
        let q = self.dun(Un::AbsD, q);
        let a = self.p_atan(q);
        let xpos = self.dcmpc(Cmp::Gt, x, 0.0);
        let na = self.dneg(a);
        let pi = self.dc(PI);
        let pa = self.dsub(pi, a);
        let ap = self.dsub(a, pi);
        let pos_branch = self.sel(neg_y, na, a);
        let neg_branch = self.sel(neg_y, ap, pa);
        let r = self.sel(xpos, pos_branch, neg_branch);
        // y infinite: ±π/2.
        let yinf = {
            let f = self.is_finite_d(y);
            let n = self.is_nan_d(y);
            let fin_or_nan = self.bor(f, n);
            self.bnot(fin_or_nan)
        };
        let h = signed(self, FRAC_PI_2);
        let r = self.sel(yinf, h, r);
        // x infinite.
        let xinf = {
            let f = self.is_finite_d(x);
            let n = self.is_nan_d(x);
            let fin_or_nan = self.bor(f, n);
            self.bnot(fin_or_nan)
        };
        let yinf2 = yinf;
        let q1 = signed(self, FRAC_PI_4);
        let q3 = signed(self, 3.0 * FRAC_PI_4);
        let z0 = signed(self, 0.0);
        let zp = signed(self, PI);
        let when_pos = self.sel(yinf2, q1, z0);
        let when_neg = self.sel(yinf2, q3, zp);
        let xinf_v = self.sel(xpos, when_pos, when_neg);
        let r = self.sel(xinf, xinf_v, r);
        // x == 0: ±π/2.
        let x0 = self.dcmpc(Cmp::Eq, x, 0.0);
        let r = self.sel(x0, h, r);
        // y == 0: y, or ±π when x is negative (by sign bit).
        let y0 = self.dcmpc(Cmp::Eq, y, 0.0);
        let xneg = self.sign_neg_d(x);
        let sp = signed(self, PI);
        let yz = self.sel(xneg, sp, y);
        let r = self.sel(y0, yz, r);
        let nx = self.is_nan_d(x);
        let ny = self.is_nan_d(y);
        let any = self.bor(nx, ny);
        let qn = self.dc(f64::NAN);
        self.sel(any, qn, r)
    }

    /// asin (`acos == false`) or acos through atan2.
    pub(super) fn p_asin_acos(&mut self, x: Val, acos: bool) -> Val {
        let one = self.dc(1.0);
        let a = self.dsub(one, x);
        let b = self.dadd(one, x);
        let p = self.dmul(a, b);
        let s = self.dun(Un::SqrtD, p);
        let r = if acos { self.p_atan2(s, x) } else { self.p_atan2(x, s) };
        let ge = self.dcmpc(Cmp::Ge, x, -1.0);
        let le = self.dcmpc(Cmp::Le, x, 1.0);
        let inside = self.band(ge, le);
        let qn = self.dc(f64::NAN);
        self.sel(inside, r, qn)
    }

    /// 2^e as an f64 (e in -1022..=1023).
    fn pow2_d(&mut self, e: Val) -> Val {
        let bias = self.ci(1023);
        let x = self.ib(Bin::AddI, e, bias);
        let s = self.ci(20);
        let hi = self.ib(Bin::ShlI, x, s);
        let lo = self.ci(0);
        self.def(Ty::F64, Op::Bin(Bin::MakeD, hi, lo))
    }

    /// y·2^k by exact power-of-two steps (the source's loops run at most
    /// once for every k `exp` produces; two steps each are kept).
    fn scalbn_d(&mut self, y: Val, k: Val) -> Val {
        let (mut y, mut k) = (y, k);
        for _ in 0..2 {
            let lim = self.ci(1023);
            let c = self.cmpi(Cmp::Gt, k, lim);
            let st = self.pow2_d(lim);
            let ys = self.dmul(y, st);
            y = self.sel(c, ys, y);
            let ks = self.ib(Bin::SubI, k, lim);
            k = self.sel(c, ks, k);
        }
        for _ in 0..2 {
            let lim = self.ci(-1022);
            let c = self.cmpi(Cmp::Lt, k, lim);
            let st = self.pow2_d(lim);
            let ys = self.dmul(y, st);
            y = self.sel(c, ys, y);
            let add = self.ci(1022);
            let ks = self.ib(Bin::AddI, k, add);
            k = self.sel(c, ks, k);
        }
        let st = self.pow2_d(k);
        self.dmul(y, st)
    }

    pub(super) fn p_exp(&mut self, x: Val) -> Val {
        const P1: f64 = 1.66666666666666019037e-01;
        const P2: f64 = -2.77777777770155933842e-03;
        const P3: f64 = 6.61375632143793436117e-05;
        const P4: f64 = -1.65339022054652515390e-06;
        const P5: f64 = 4.13813679705723846039e-08;
        const LN2_HI: f64 = 6.93147180369123816490e-01;
        const LN2_LO: f64 = 1.90821492927058770002e-10;
        const INV_LN2: f64 = 1.44269504088896338700e+00;
        let il = self.dc(INV_LN2);
        let k = self.dmul(x, il);
        let k = self.dun(Un::RoundD, k);
        let lh = self.dc(LN2_HI);
        let t = self.dmul(k, lh);
        let hi = self.dsub(x, t);
        let ll = self.dc(LN2_LO);
        let lo = self.dmul(k, ll);
        let r = self.dsub(hi, lo);
        let t = self.dmul(r, r);
        // r - t * (P1 + t * (P2 + t * (P3 + t * (P4 + t * P5))))
        let p = self.horner_d(t, &[P5, P4, P3, P2, P1]);
        let tp = self.dmul(t, p);
        let c = self.dsub(r, tp);
        // 1.0 - ((lo - (r * c) / (2.0 - c)) - hi)
        let rc = self.dmul(r, c);
        let two = self.dc(2.0);
        let d = self.dsub(two, c);
        let q = self.ddiv(rc, d);
        let u = self.dsub(lo, q);
        let u = self.dsub(u, hi);
        let one = self.dc(1.0);
        let y = self.dsub(one, u);
        let ki = self.def(Ty::I32, Op::Un(Un::D2I, k));
        let v = self.scalbn_d(y, ki);
        // |x| < 2^-28: 1 + x.
        let ax = self.dun(Un::AbsD, x);
        let tiny = self.dcmpc(Cmp::Lt, ax, 2f64.powi(-28));
        let ox = self.dadd(one, x);
        let v = self.sel(tiny, ox, v);
        let under = self.dcmpc(Cmp::Lt, x, -745.13321910194110842);
        let z = self.dc(0.0);
        let v = self.sel(under, z, v);
        let over = self.dcmpc(Cmp::Gt, x, 709.782712893383973096);
        let inf = self.dc(f64::INFINITY);
        let v = self.sel(over, inf, v);
        let nan = self.is_nan_d(x);
        let qn = self.dc(f64::NAN);
        self.sel(nan, qn, v)
    }

    pub(super) fn p_ln(&mut self, x: Val) -> Val {
        const LG: [f64; 7] = [
            6.666666666666735130e-01, 3.999999999940941908e-01, 2.857142874366239149e-01, 2.222219843214978396e-01,
            1.818357216161805012e-01, 1.531383769920937332e-01, 1.479819860511658591e-01,
        ];
        const LN2_HI: f64 = 6.93147180369123816490e-01;
        const LN2_LO: f64 = 1.90821492927058770002e-10;
        // Subnormals: scale into the normal range first (exact).
        let sub = self.dcmpc(Cmp::Lt, x, f64::MIN_POSITIVE);
        let xs = self.dmulc(x, 2f64.powi(54));
        let xs = self.sel(sub, xs, x);
        let m54 = self.ci(-54);
        let z0 = self.ci(0);
        let bias = self.sel(sub, m54, z0);
        let hi = self.def(Ty::I32, Op::Un(Un::HiD, xs));
        let lo = self.def(Ty::I32, Op::Un(Un::LoD, xs));
        let s20 = self.ci(20);
        let e = self.ib(Bin::ShrUI, hi, s20);
        let mask = self.ci(0x7ff);
        let e = self.ib(Bin::AndI, e, mask);
        let c1023 = self.ci(1023);
        let k = self.ib(Bin::SubI, e, c1023);
        let k = self.ib(Bin::AddI, k, bias);
        let mm = self.ci(0x000f_ffff);
        let mh = self.ib(Bin::AndI, hi, mm);
        let one_hi = self.ci(0x3ff0_0000);
        let mh = self.ib(Bin::OrI, mh, one_hi);
        let m = self.def(Ty::F64, Op::Bin(Bin::MakeD, mh, lo));
        let big = self.dcmpc(Cmp::Gt, m, core::f64::consts::SQRT_2);
        // `m *= 0.5`.
        let half = self.dc(0.5);
        let mhalf = self.dmul(m, half);
        let m = self.sel(big, mhalf, m);
        let i1 = self.ci(1);
        let k1 = self.ib(Bin::AddI, k, i1);
        let k = self.sel(big, k1, k);
        let one = self.dc(1.0);
        let f = self.dsub(m, one);
        let two = self.dc(2.0);
        let d = self.dadd(two, f);
        let s = self.ddiv(f, d);
        let z = self.dmul(s, s);
        let w = self.dmul(z, z);
        // t1 = w * (LG[1] + w * (LG[3] + w * LG[5]))
        let t1 = self.horner_d(w, &[LG[5], LG[3], LG[1]]);
        let t1 = self.dmul(w, t1);
        // t2 = z * (LG[0] + w * (LG[2] + w * (LG[4] + w * LG[6])))
        let t2 = self.horner_d(w, &[LG[6], LG[4], LG[2], LG[0]]);
        let t2 = self.dmul(z, t2);
        let r = self.dadd(t1, t2);
        // hfsq = 0.5 * f * f
        let hf = self.dmul(half, f);
        let hfsq = self.dmul(hf, f);
        let kd = self.dun(Un::I2D, k);
        // kd * LN2_HI - ((hfsq - (s * (hfsq + r) + kd * LN2_LO)) - f)
        let lh = self.dc(LN2_HI);
        let a = self.dmul(kd, lh);
        let hr = self.dadd(hfsq, r);
        let shr = self.dmul(s, hr);
        let ll = self.dc(LN2_LO);
        let kl = self.dmul(kd, ll);
        let inner = self.dadd(shr, kl);
        let b = self.dsub(hfsq, inner);
        let b = self.dsub(b, f);
        let v = self.dsub(a, b);
        let inf = self.dcmpc(Cmp::Eq, x, f64::INFINITY);
        let pinf = self.dc(f64::INFINITY);
        let v = self.sel(inf, pinf, v);
        let zero = self.dcmpc(Cmp::Eq, x, 0.0);
        let ninf = self.dc(f64::NEG_INFINITY);
        let v = self.sel(zero, ninf, v);
        let nan = self.is_nan_d(x);
        let negx = self.dcmpc(Cmp::Lt, x, 0.0);
        let bad = self.bor(nan, negx);
        let qn = self.dc(f64::NAN);
        self.sel(bad, qn, v)
    }

    /// `y.fract() == 0.0` (false for inf and NaN).
    fn is_integral_d(&mut self, y: Val) -> Val {
        let t = self.dun(Un::TruncD, y);
        let f = self.dsub(y, t);
        self.dcmpc(Cmp::Eq, f, 0.0)
    }

    pub(super) fn p_pow(&mut self, x: Val, y: Val) -> Val {
        let one = self.dc(1.0);
        let integral = self.is_integral_d(y);
        let ay = self.dun(Un::AbsD, y);
        // Integer exponents up to 64: square and multiply (7 bits).
        let n = self.def(Ty::I32, Op::Un(Un::D2I, ay));
        let mut base = x;
        let mut acc = one;
        let i1 = self.ci(1);
        for bit in 0..7 {
            let sh = self.ci(bit);
            let nb = self.ib(Bin::ShrI, n, sh);
            let nb = self.ib(Bin::AndI, nb, i1);
            let set = self.cmpi(Cmp::Eq, nb, i1);
            let m = self.dmul(acc, base);
            acc = self.sel(set, m, acc);
            base = self.dmul(base, base);
        }
        let inv = self.ddiv(one, acc);
        let yneg = self.dcmpc(Cmp::Lt, y, 0.0);
        let ipow = self.sel(yneg, inv, acc);
        // exp(y * ln(x)), and for x < 0 exp(y * ln(-x)) with the sign of an
        // odd integer y.
        let nx = self.dneg(x);
        let ax_ln = self.p_ln(nx);
        let t = self.dmul(y, ax_ln);
        let m = self.p_exp(t);
        let half = self.dc(0.5);
        let yh = self.dmul(y, half);
        let yh = self.dun(Un::TruncD, yh);
        let two = self.dc(2.0);
        let y2 = self.dmul(two, yh);
        let par = self.dsub(y, y2);
        let odd = self.dcmpc(Cmp::Ne, par, 0.0);
        let lim = self.dcmpc(Cmp::Lt, ay, 2f64.powi(53));
        let odd = self.band(lim, odd);
        let neg_pow = self.dneg_if(odd, m);
        let qn = self.dc(f64::NAN);
        let neg_pow = self.sel(integral, neg_pow, qn);
        let lx = self.p_ln(x);
        let t = self.dmul(y, lx);
        let pos_pow = self.p_exp(t);
        let xneg = self.dcmpc(Cmp::Lt, x, 0.0);
        let r = self.sel(xneg, neg_pow, pos_pow);
        // x == 0: 0 for y > 0, else inf.
        let ypos = self.dcmpc(Cmp::Gt, y, 0.0);
        let z = self.dc(0.0);
        let inf = self.dc(f64::INFINITY);
        let zr = self.sel(ypos, z, inf);
        let x0 = self.dcmpc(Cmp::Eq, x, 0.0);
        let r = self.sel(x0, zr, r);
        let le64 = self.dcmpc(Cmp::Le, ay, 64.0);
        let small_int = self.band(integral, le64);
        let r = self.sel(small_int, ipow, r);
        let xn = self.is_nan_d(x);
        let yn = self.is_nan_d(y);
        let any = self.bor(xn, yn);
        let r = self.sel(any, qn, r);
        let y0 = self.dcmpc(Cmp::Eq, y, 0.0);
        let x1 = self.dcmpc(Cmp::Eq, x, 1.0);
        let unit = self.bor(y0, x1);
        self.sel(unit, one, r)
    }

    pub(super) fn p_hypot(&mut self, x: Val, y: Val) -> Val {
        let a = self.dun(Un::AbsD, x);
        let b = self.dun(Un::AbsD, y);
        // a.max(b): the other one when one is NaN.
        let gt = self.dcmp(Cmp::Gt, a, b);
        let mx = self.sel(gt, a, b);
        let an = self.is_nan_d(a);
        let bn = self.is_nan_d(b);
        let mx = self.sel(bn, a, mx);
        let big = self.sel(an, b, mx);
        let over = self.dcmpc(Cmp::Gt, big, 2f64.powi(500));
        let under = self.dcmpc(Cmp::Lt, big, 2f64.powi(-500));
        let pos = self.dcmpc(Cmp::Gt, big, 0.0);
        let under = self.band(under, pos);
        let one = self.dc(1.0);
        let up = self.dc(2f64.powi(600));
        let down = self.dc(2f64.powi(-600));
        let scale = self.sel(under, up, one);
        let scale = self.sel(over, down, scale);
        let a = self.dmul(a, scale);
        let b = self.dmul(b, scale);
        let aa = self.dmul(a, a);
        let bb = self.dmul(b, b);
        let s = self.dadd(aa, bb);
        let s = self.dun(Un::SqrtD, s);
        let v = self.ddiv(s, scale);
        // is_infinite on the unscaled |x|, |y|.
        let xa = self.dun(Un::AbsD, x);
        let ya = self.dun(Un::AbsD, y);
        let xi = self.dcmpc(Cmp::Eq, xa, f64::INFINITY);
        let yi = self.dcmpc(Cmp::Eq, ya, f64::INFINITY);
        let inf = self.bor(xi, yi);
        let pinf = self.dc(f64::INFINITY);
        self.sel(inf, pinf, v)
    }

    pub(super) fn p_cbrt(&mut self, x: Val) -> Val {
        let a = self.dun(Un::AbsD, x);
        let l = self.p_ln(a);
        let three = self.dc(3.0);
        let q = self.ddiv(l, three);
        let y = self.p_exp(q);
        // y -= (y * y * y - a) / (3.0 * y * y)
        let yy = self.dmul(y, y);
        let yyy = self.dmul(yy, y);
        let n = self.dsub(yyy, a);
        let t = self.dmul(three, y);
        let d = self.dmul(t, y);
        let c = self.ddiv(n, d);
        let y = self.dsub(y, c);
        let neg = self.dcmpc(Cmp::Lt, x, 0.0);
        let v = self.dneg_if(neg, y);
        let z = self.dcmpc(Cmp::Eq, x, 0.0);
        let fin = self.is_finite_d(x);
        let nf = self.bnot(fin);
        let keep = self.bor(z, nf);
        self.sel(keep, x, v)
    }
}
