//! Advanced operations on scalar B-spline curve functions: knot refinement, the
//! derivative function, and the symbolic products function * function and
//! function * curve.
//!
//! Upstream: `source/SMLib/src/NL_FuncsAdv.cpp` (declarations in
//! `source/SMLib/inc/NL_FuncsAdv.h`).
//!
//! | Rust                                  | Upstream                     |
//! |---------------------------------------|------------------------------|
//! | [`CurveFunction::refine`]             | `N_CrvFuncRefine`            |
//! | [`CurveFunction::deriv`]              | `N_CrvFuncDeriv`             |
//! | [`CurveFunction::multiply_func`]      | `N_CrvFuncMultiplyCrvFunc`   |
//! | [`CurveFunction::multiply_crv`]       | `N_CrvFuncMultiplyCrv`       |
//! | [`CurveFunction::multiply_crv_4d`]    | `N_CrvFuncMultiplyCrv4d`     |
//! | `multiply` (private)                  | the shared body of the three products |
//!
//! Deviations:
//! - Upstream writes into an output object that is either NULL (allocated), an
//!   existing object (checked by `N_CFuncSizeArrays` / `N_CrvSizeArrays`, `NL_STO_ERR`
//!   when too small) or, for `N_CrvFuncRefine`, the input itself (in place). The Rust
//!   methods take the inputs by reference and return a new object sized as upstream
//!   sizes a NULL output (and compacts it), so the storage error path does not exist.
//! - The products rescale the knot vectors of **both** inputs to a common range, as
//!   upstream (`N_MakeKnotsCompatible`); the inputs are therefore `&mut`. Upstream
//!   callers square a function by passing it twice (`N_CrvFuncMultiplyCrvFunc(&d, &d,
//!   ..)`); Rust callers pass a clone as the second operand, which gives the same
//!   result (equal knot vectors are neither rescaled nor refined).
//! - The three upstream products are copies of one loop that differ only in the type
//!   of the second operand and the Bezier product routine; they share one generic
//!   body here, with the upstream operation order on every value.
//! - Out-params become return values; errors are [`SolidError::Nlib`] values with the
//!   upstream `NL_*_ERR` number instead of a flag plus the global `NL_ERROR` record.
//!   The `NL_STACKS` arguments, the routine names and the memory error paths have no
//!   counterpart.
//! - Every array access is bounds checked: where upstream would read or write outside
//!   its arrays (malformed functions or curves: knot vectors that do not match the
//!   values or are not clamped), the methods return [`SolidError::InvalidInput`]
//!   instead. For every input upstream handles without undefined behaviour the results
//!   are the same, bit for bit.
//! - A degree above [`MAX_SHORT_DEGREE`](crate::nurbs::bezier::MAX_SHORT_DEGREE)
//!   (impossible upstream, where `NL_DEGREE` is a `short`) is
//!   [`SolidError::InvalidInput`], so index arithmetic and allocations sized from the
//!   degree cannot overflow.

// Index arithmetic follows upstream's signed `NL_INDEX` (`isize` here); every access
// goes through `nurbs::index::{get, set}`, which reject negative or too-large indices.

use crate::error::{NlibCode, Result, SolidError};
use crate::nurbs::basis_basic::{make_knots_compatible, PTOL};
use crate::nurbs::bezier::{
    check_short_degree, func_multiply_crv, func_multiply_crv_4d, func_multiply_func, product_matrix,
};
use crate::nurbs::crv_basic::Curve;
use crate::nurbs::funcs_basic::CurveFunction;
use crate::nurbs::index::{get, oob, set, storage, sx, ux};
use crate::nurbs::knots::{Side, MAX_DEGREE};
use crate::nurbs::math::{float_op_is_bad, CPoint, FloatOp, RealMatrix};

/// A function over whole arrays `fq[0..=n]`, `uq[0..=m]`.
fn function(fq: Vec<f64>, p: usize, uq: Vec<f64>, op: &'static str) -> Result<CurveFunction> {
    CurveFunction::new(fq, p, uq).map_err(|_| oob(op))
}

// ---- refinement and derivative ------------------------------------------------------

impl CurveFunction {
    /// Inserts the knots `x` (non-decreasing, strictly inside the knot range) into the
    /// function (`N_CrvFuncRefine`); the result has `x.len()` more values and knots.
    ///
    /// Errors: [`NlibCode::Input`] when `x` is empty; [`NlibCode::Parameter`] when
    /// `x[0]` or its last knot is on or outside the ends of the knot vector; the errors
    /// of [`crate::nurbs::knots::KnotVector::find_span`]; `InvalidInput` where upstream
    /// would index outside its arrays.
    pub fn refine(&self, x: &[f64]) -> Result<CurveFunction> {
        const OP: &str = "nurbs::funcs_adv::CurveFunction::refine";
        let fp = self.values();
        let up = self.knots();
        let pu = self.degree();
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        check_short_degree(pu, OP)?;
        let p = sx(pu, OP)?;
        let knt = self.knot_vector()?;
        let r = sx(x.len(), OP)? - 1;

        // Check input parameters.
        if r < 0 {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }
        let x0 = get(x, 0, OP)?;
        let xr = get(x, r, OP)?;
        if knt.is_end_param(x0) || knt.is_end_param(xr) {
            return Err(SolidError::nlib(OP, NlibCode::Parameter));
        }

        let mut fq: Vec<f64> = storage(n + r + 1, OP)?;
        let mut uq: Vec<f64> = storage(m + r + 1, OP)?;

        // Find knot spans.
        let a = sx(knt.find_span(pu, x0, Side::Left)?, OP)?;
        let b = sx(knt.find_span(pu, xr, Side::Left)?, OP)? + 1;

        // Initialize output knot vector.
        for j in 0..=a {
            set(&mut uq, j, get(up, j, OP)?, OP)?;
        }
        for j in b + p..=m {
            set(&mut uq, j + r + 1, get(up, j, OP)?, OP)?;
        }

        // Save unaltered control values.
        for j in 0..=a - p {
            set(&mut fq, j, get(fp, j, OP)?, OP)?;
        }
        for j in b - 1..=n {
            set(&mut fq, j + r + 1, get(fp, j, OP)?, OP)?;
        }

        // Now refine the knot vector.
        let mut i = b + p - 1;
        let mut k = b + p + r;
        let mut j = r;
        while j >= 0 {
            let xj = get(x, j, OP)?;
            while xj <= get(up, i, OP)? && i > a {
                set(&mut fq, k - p - 1, get(fp, i - p - 1, OP)?, OP)?;
                set(&mut uq, k, get(up, i, OP)?, OP)?;
                k -= 1;
                i -= 1;
            }

            let v = get(&fq, k - p, OP)?;
            set(&mut fq, k - p - 1, v, OP)?;

            for l in 1..=p {
                let t = k - p + l;
                let mut alf = get(&uq, k + l, OP)? - xj;
                if alf.abs() <= 0.0 {
                    let v = get(&fq, t, OP)?;
                    set(&mut fq, t - 1, v, OP)?;
                } else {
                    alf /= get(&uq, k + l, OP)? - get(up, i - p + l, OP)?;
                    let oma = 1.0 - alf;
                    let v = oma * get(&fq, t, OP)? + alf * get(&fq, t - 1, OP)?;
                    set(&mut fq, t - 1, v, OP)?;
                }
            }
            set(&mut uq, k, xj, OP)?;
            k -= 1;
            j -= 1;
        }

        function(fq, pu, uq, OP)
    }

    /// The `d`-th derivative function, of degree `p - d` (`N_CrvFuncDeriv`).
    ///
    /// Errors: [`NlibCode::Input`] unless `d <= p`; [`NlibCode::Derivative`] when a
    /// knot difference `U[i+p+1] - U[i+k]` is zero; [`NlibCode::Numerical`] when a
    /// quotient `(p-k+1) / (U[i+p+1] - U[i+k])` would overflow or underflow;
    /// `InvalidInput` where upstream would index outside its arrays (`n < d` or knots
    /// that do not match the values).
    pub fn deriv(&self, d: usize) -> Result<CurveFunction> {
        const OP: &str = "nurbs::funcs_adv::CurveFunction::deriv";
        let fu = self.values();
        let u = self.knots();
        let n = sx(self.n(), OP)?;
        let m = sx(self.m(), OP)?;
        check_short_degree(self.degree(), OP)?;
        let p = sx(self.degree(), OP)?;

        // Check error.
        let d = isize::try_from(d).map_err(|_| SolidError::nlib(OP, NlibCode::Input))?;
        if d > p {
            return Err(SolidError::nlib(OP, NlibCode::Input));
        }

        // Output sizes.
        let nd = n - d;
        let pd = p - d;
        let md = nd + pd + 1;
        let mut fd: Vec<f64> = storage(nd, OP)?;
        let mut v: Vec<f64> = storage(md, OP)?;

        // Initialize control values.
        let mut ft = fu.to_vec();

        // Compute control values of derivative function.
        let pf = p as f64;
        for k in 1..=d {
            let kf = k as f64;
            for i in 0..=n - k {
                let hi = get(u, i + p + 1, OP)?;
                let lo = get(u, i + k, OP)?;
                if hi == lo {
                    return Err(SolidError::nlib(OP, NlibCode::Derivative));
                }
                if float_op_is_bad(pf - kf + 1.0, hi - lo, FloatOp::Division) {
                    return Err(SolidError::nlib(OP, NlibCode::Numerical));
                }
                let alf = (pf - kf + 1.0) / (hi - lo);
                let t = alf * (get(&ft, i + 1, OP)? - get(&ft, i, OP)?);
                set(&mut ft, i, t, OP)?;
            }
        }
        for i in 0..=n - d {
            set(&mut fd, i, get(&ft, i, OP)?, OP)?;
        }

        // Compute the knot vector.
        let mut j: isize = -1;
        let u0 = get(u, 0, OP)?;
        let um = get(u, m, OP)?;
        for _ in 0..=p - d {
            j += 1;
            set(&mut v, j, u0, OP)?;
        }
        for i in p + 1..=n {
            j += 1;
            set(&mut v, j, get(u, i, OP)?, OP)?;
        }
        for _ in 0..=p - d {
            j += 1;
            set(&mut v, j, um, OP)?;
        }

        function(fd, ux(pd, OP)?, v, OP)
    }
}

// ---- products -----------------------------------------------------------------------

/// A coefficient of the second product operand: a function value or a control point.
trait Coef: Copy + Default {
    /// `alpha*a + beta*b` (`N_Combine2CPts` for points).
    fn combine2(alpha: f64, a: Self, beta: f64, b: Self) -> Self;
}

impl Coef for f64 {
    fn combine2(alpha: f64, a: Self, beta: f64, b: Self) -> Self {
        alpha * a + beta * b
    }
}

impl Coef for CPoint {
    fn combine2(alpha: f64, a: Self, beta: f64, b: Self) -> Self {
        CPoint::combine2(alpha, a, beta, b)
    }
}

/// The second operand of a product: a curve function or a curve.
trait Operand: Sized {
    type C: Coef;
    fn degree(&self) -> usize;
    fn n(&self) -> usize;
    fn coefs(&self) -> &[Self::C];
    fn knots(&self) -> &[f64];
    fn knots_mut(&mut self) -> &mut [f64];
    fn span_count(&self) -> Result<usize>;
    fn refine(&self, x: &[f64]) -> Result<Self>;
}

impl Operand for CurveFunction {
    type C = f64;
    fn degree(&self) -> usize {
        CurveFunction::degree(self)
    }
    fn n(&self) -> usize {
        CurveFunction::n(self)
    }
    fn coefs(&self) -> &[f64] {
        self.values()
    }
    fn knots(&self) -> &[f64] {
        CurveFunction::knots(self)
    }
    fn knots_mut(&mut self) -> &mut [f64] {
        CurveFunction::knots_mut(self)
    }
    fn span_count(&self) -> Result<usize> {
        Ok(self.knot_vector()?.span_count(self.degree()))
    }
    fn refine(&self, x: &[f64]) -> Result<Self> {
        CurveFunction::refine(self, x)
    }
}

impl Operand for Curve {
    type C = CPoint;
    fn degree(&self) -> usize {
        Curve::degree(self)
    }
    fn n(&self) -> usize {
        Curve::n(self)
    }
    fn coefs(&self) -> &[CPoint] {
        self.cpts()
    }
    fn knots(&self) -> &[f64] {
        Curve::knots(self)
    }
    fn knots_mut(&mut self) -> &mut [f64] {
        Curve::knots_mut(self)
    }
    fn span_count(&self) -> Result<usize> {
        Ok(self.knot_vector()?.span_count(self.degree()))
    }
    fn refine(&self, x: &[f64]) -> Result<Self> {
        Curve::refine(self, x)
    }
}

/// Bezier product `fg[s..=e]` of `f` (degree `p`) and `g` (degree `q`).
type BezProduct<C> =
    fn(&[f64], usize, &[C], usize, &RealMatrix, usize, usize, &mut [C]) -> Result<()>;

/// The product's coefficients and knots with the indices in use.
struct Product<C> {
    coefs: Vec<C>,
    n: usize,
    p: usize,
    knots: Vec<f64>,
    m: usize,
}

/// The shared body of `N_CrvFuncMultiplyCrvFunc`, `N_CrvFuncMultiplyCrv` and
/// `N_CrvFuncMultiplyCrv4d`: the product of the function `fun` and `opg`. `scale(c, a)`
/// is `a * c` (the scaling of the special case and the first coefficient).
fn multiply<G: Operand>(
    fun: &mut CurveFunction,
    opg: &mut G,
    bez: BezProduct<G::C>,
    scale: fn(G::C, f64) -> G::C,
    op: &'static str,
) -> Result<Product<G::C>> {
    // Get new degree.
    let p = sx(fun.degree(), op)?;
    let q = sx(opg.degree(), op)?;
    // A sum past `isize::MAX` exceeds `NL_MAXDEG` too.
    let pq = match p.checked_add(q) {
        Some(pq) if pq <= sx(MAX_DEGREE, op)? => pq,
        _ => return Err(SolidError::nlib(op, NlibCode::Degree)),
    };
    let pqu = ux(pq, op)?;

    // Handle special case p=0 or q=0.
    if p == 0 || q == 0 {
        if sx(fun.n(), op)? != p || sx(opg.n(), op)? != q {
            return Err(SolidError::nlib(op, NlibCode::Input));
        }
        let mut h: Vec<G::C> = storage(pq, op)?;
        let mut t: Vec<f64> = storage(pq + pq + 1, op)?;
        let f = fun.values();
        let r = CurveFunction::knots(fun);
        let g = opg.coefs();
        let s = opg.knots();
        if p == 0 {
            let f0 = get(f, 0, op)?;
            for j in 0..=q {
                set(&mut h, j, scale(get(g, j, op)?, f0), op)?;
                set(&mut t, j, get(s, j, op)?, op)?;
                set(&mut t, q + j + 1, get(s, q + j + 1, op)?, op)?;
            }
        } else {
            let g0 = get(g, 0, op)?;
            for i in 0..=p {
                set(&mut h, i, scale(g0, get(f, i, op)?), op)?;
                set(&mut t, i, get(r, i, op)?, op)?;
                set(&mut t, p + i + 1, get(r, p + i + 1, op)?, op)?;
            }
        }
        return Ok(Product {
            coefs: h,
            n: pqu,
            p: pqu,
            knots: t,
            m: ux(pq + pq + 1, op)?,
        });
    }

    // Merge knots (rescales the knot vector with the shorter range).
    let pu = fun.degree();
    let qu = opg.degree();
    let (xr, xs) = make_knots_compatible(fun.knots_mut(), opg.knots_mut(), pu, qu)?;

    // Refine functions.
    let mxr = sx(xr.len(), op)? - 1;
    let mxs = sx(xs.len(), op)? - 1;
    let fr;
    let ffun: &CurveFunction = if mxr >= 0 {
        fr = fun.refine(&xr)?;
        &fr
    } else {
        fun
    };
    let gr;
    let gop: &G = if mxs >= 0 {
        gr = opg.refine(&xs)?;
        &gr
    } else {
        opg
    };
    let f = ffun.values();
    let rk = CurveFunction::knots(ffun);
    let mr = sx(ffun.m(), op)?;
    let nsr = ffun.knot_vector()?.span_count(pu);
    let g = gop.coefs();
    let sk = gop.knots();
    let ms = sx(sk.len(), op)? - 1;
    let nss = gop.span_count()?;

    if nsr != nss {
        return Err(SolidError::nlib(op, NlibCode::Knot));
    }

    // See if memory is needed.
    let nh = sx(nsr, op)? * pq;
    let mt = nh + pq + 1;
    let mut h: Vec<G::C> = storage(nh, op)?;
    let mut t: Vec<f64> = storage(mt, op)?;

    // Get local memory.
    let mut fb: Vec<f64> = storage(p, op)?;
    let mut fnx: Vec<f64> = storage(p, op)?;
    let mut gb: Vec<G::C> = storage(q, op)?;
    let mut gn: Vec<G::C> = storage(q, op)?;
    let mut fg: Vec<G::C> = storage(pq, op)?;
    let mut alfr: Vec<f64> = storage(p, op)?;
    let mut omar: Vec<f64> = storage(p, op)?;
    let mut alfs: Vec<f64> = storage(q, op)?;
    let mut omas: Vec<f64> = storage(q, op)?;

    // Initialize.
    let mut ar = p;
    let mut br = p + 1;
    let mut as_ = q;
    let mut bs = q + 1;
    let mut ixr: isize = 0;
    let mut ixs: isize = 0;

    let mut find: isize = 1;
    let mut kind = pq + 1;
    let mut rem: isize = -1;

    for i in 0..=pq {
        set(&mut t, i, get(sk, as_, op)?, op)?;
    }
    for i in 0..=p {
        set(&mut fb, i, get(f, i, op)?, op)?;
    }
    for j in 0..=q {
        set(&mut gb, j, get(g, j, op)?, op)?;
    }
    set(&mut h, 0, scale(get(g, 0, op)?, get(f, 0, op)?), op)?;

    let pm = product_matrix(pu, qu)?;

    // Loop through the knot vectors and do the following:
    //   (1) Extract the i-th Bezier entities.
    //   (2) Compute their product.
    //   (3) Remove the knot(s) between the i-th and the (i-1)-th segments.
    while br < mr && bs < ms {
        // Get multiplicities of the knots.
        let i0 = br;
        while br < mr && get(rk, br, op)? == get(rk, br + 1, op)? {
            br += 1;
        }
        let mlr = br - i0 + 1;
        let mut mir = mlr;

        let j0 = bs;
        while bs < ms && get(sk, bs, op)? == get(sk, bs + 1, op)? {
            bs += 1;
        }
        let mls = bs - j0 + 1;
        let mut mis = mls;

        // Check multiplicities of internal knots.
        if br < mr && bs < ms && (mlr > p || mls > q) {
            return Err(SolidError::nlib(op, NlibCode::Knot));
        }

        // Adjust multiplicities and compute multiplicity of output knot.
        if mir == 1 && ixr <= mxr && (get(rk, br, op)? - get(&xr, ixr, op)?).abs() < PTOL {
            mir = 0;
            ixr += 1;
        }
        if mis == 1 && ixs <= mxs && (get(sk, bs, op)? - get(&xs, ixs, op)?).abs() < PTOL {
            mis = 0;
            ixs += 1;
        }

        let mi = if mis == 0 {
            q + mir
        } else if mir == 0 {
            p + mis
        } else {
            (q + mir).max(p + mis)
        };

        // Insert knot to get Bezier entities.
        let rr = p - mlr;
        let rs = q - mls;
        let tt = pq - mi;

        let lbz = if rem > 0 { (rem + 2) / 2 } else { 1 };
        let rbz = if tt > 0 { pq - (tt + 1) / 2 } else { pq };

        if rr > 0 {
            let num = get(rk, br, op)? - get(rk, ar, op)?;
            let mut k = p;
            while k > mlr {
                let a = num / (get(rk, ar + k, op)? - get(rk, ar, op)?);
                set(&mut alfr, k - mlr - 1, a, op)?;
                set(&mut omar, k - mlr - 1, 1.0 - a, op)?;
                k -= 1;
            }
            for j in 1..=rr {
                let save = rr - j;
                let s = mlr + j;
                let mut k = p;
                while k >= s {
                    let v = get(&alfr, k - s, op)? * get(&fb, k, op)?
                        + get(&omar, k - s, op)? * get(&fb, k - 1, op)?;
                    set(&mut fb, k, v, op)?;
                    k -= 1;
                }
                set(&mut fnx, save, get(&fb, p, op)?, op)?;
            }
        }

        if rs > 0 {
            let num = get(sk, bs, op)? - get(sk, as_, op)?;
            let mut k = q;
            while k > mls {
                let a = num / (get(sk, as_ + k, op)? - get(sk, as_, op)?);
                set(&mut alfs, k - mls - 1, a, op)?;
                set(&mut omas, k - mls - 1, 1.0 - a, op)?;
                k -= 1;
            }
            for j in 1..=rs {
                let save = rs - j;
                let s = mls + j;
                let mut k = q;
                while k >= s {
                    let v = G::C::combine2(
                        get(&alfs, k - s, op)?,
                        get(&gb, k, op)?,
                        get(&omas, k - s, op)?,
                        get(&gb, k - 1, op)?,
                    );
                    set(&mut gb, k, v, op)?;
                    k -= 1;
                }
                set(&mut gn, save, get(&gb, q, op)?, op)?;
            }
        }

        // Compute product of Bezier entities.
        bez(&fb, pu, &gb, qu, &pm, ux(lbz, op)?, pqu, &mut fg)?;

        // Remove the knot R[ar] = S[as].
        if rem > 1 {
            let mut first = kind - 2;
            let den = get(sk, bs, op)? - get(sk, as_, op)?;
            let bet = (get(sk, bs, op)? - get(&t, kind - 1, op)?) / den;
            let omb = 1.0 - bet;

            // `last` starts at `kind` and grows by one per pass.
            for (k, last) in (1..rem).zip(kind..) {
                let mut i = first;
                let mut j = last;
                let mut l = j - kind + 1;

                while j - i > k {
                    if i < find {
                        let ti = get(&t, i, op)?;
                        let alf = (get(sk, bs, op)? - ti) / (get(sk, as_, op)? - ti);
                        let oma = 1.0 - alf;
                        let v = G::C::combine2(alf, get(&h, i, op)?, oma, get(&h, i - 1, op)?);
                        set(&mut h, i, v, op)?;
                    }
                    if j >= lbz {
                        let v = if j - k <= kind - pq + rem {
                            let gam = (get(sk, bs, op)? - get(&t, j - k, op)?) / den;
                            let omg = 1.0 - gam;
                            G::C::combine2(gam, get(&fg, l, op)?, omg, get(&fg, l + 1, op)?)
                        } else {
                            G::C::combine2(bet, get(&fg, l, op)?, omb, get(&fg, l + 1, op)?)
                        };
                        set(&mut fg, l, v, op)?;
                    }
                    i += 1;
                    j -= 1;
                    l -= 1;
                }
                first -= 1;
            }
        }

        // Load knots and control values.
        if ar != p && as_ != q {
            let sa = get(sk, as_, op)?;
            for _ in 0..pq - rem {
                set(&mut t, kind, sa, op)?;
                kind += 1;
            }
        }
        for i in lbz..=rbz {
            set(&mut h, find, get(&fg, i, op)?, op)?;
            find += 1;
        }

        // Initialize for next pass through.
        if br < mr && bs < ms {
            for i in 0..rr {
                set(&mut fb, i, get(&fnx, i, op)?, op)?;
            }
            for i in rr..=p {
                set(&mut fb, i, get(f, br - p + i, op)?, op)?;
            }
            for j in 0..rs {
                set(&mut gb, j, get(&gn, j, op)?, op)?;
            }
            for j in rs..=q {
                set(&mut gb, j, get(g, bs - q + j, op)?, op)?;
            }
            ar = br;
            br += 1;
            rem = pq - mi;
            as_ = bs;
            bs += 1;
        } else {
            // Get end knots.
            let sb = get(sk, bs, op)?;
            for _ in 0..=pq {
                set(&mut t, kind, sb, op)?;
                kind += 1;
            }
        }
    }

    // Compact output (the caller shrinks the storage to the indices in use).
    let (n, m) = if find < nh + 1 {
        (find - 1, kind - 1)
    } else {
        (nh, mt)
    };
    let (n, m) = (ux(n, op)?, ux(m, op)?);
    if h.len() <= n || t.len() <= m {
        return Err(oob(op));
    }
    h.truncate(n + 1);
    t.truncate(m + 1);
    Ok(Product {
        coefs: h,
        n,
        p: pqu,
        knots: t,
        m,
    })
}

fn scale_value(g: f64, f: f64) -> f64 {
    f * g
}

impl CurveFunction {
    /// The product `self * g` of two curve functions, of degree `p + q`
    /// (`N_CrvFuncMultiplyCrvFunc`).
    ///
    /// The knot vector of the operand with the shorter parameter range is first
    /// rescaled in place to the range of the other (the functions do not change
    /// otherwise). When either degree is zero, both operands must be single Bezier
    /// segments (`n == p`); the result then takes the knots of the other operand.
    ///
    /// Errors: [`NlibCode::Degree`] when `p + q > MAX_DEGREE`; [`NlibCode::Input`] for
    /// a zero degree with more than one segment; [`NlibCode::Knot`] when the refined
    /// knot vectors have different span counts or an interior knot multiplicity above
    /// the degree; the errors of [`CurveFunction::refine`], `make_knots_compatible`
    /// and the Bezier product; `InvalidInput` where upstream would index outside its
    /// arrays.
    pub fn multiply_func(&mut self, g: &mut CurveFunction) -> Result<CurveFunction> {
        let op = "nurbs::funcs_adv::CurveFunction::multiply_func";
        let r = multiply(self, g, func_multiply_func, scale_value, op)?;
        CurveFunction::with_indices(r.coefs, r.n, r.p, r.knots, r.m).map_err(|_| oob(op))
    }

    /// The product `self * C` of the function and the curve `g`, of degree `p + q`,
    /// scaling only `x, y, z` of the control points (`N_CrvFuncMultiplyCrv`).
    ///
    /// Knot rescaling, special cases and errors as [`CurveFunction::multiply_func`]
    /// (with [`Curve::refine`] for the curve).
    pub fn multiply_crv(&mut self, g: &mut Curve) -> Result<Curve> {
        let op = "nurbs::funcs_adv::CurveFunction::multiply_crv";
        let r = multiply(self, g, func_multiply_crv, CPoint::scale_xyz, op)?;
        Curve::with_indices(r.coefs, r.n, r.p, r.knots, r.m).map_err(|_| oob(op))
    }

    /// As [`CurveFunction::multiply_crv`], scaling all four homogeneous coordinates
    /// (`N_CrvFuncMultiplyCrv4d`).
    pub fn multiply_crv_4d(&mut self, g: &mut Curve) -> Result<Curve> {
        let op = "nurbs::funcs_adv::CurveFunction::multiply_crv_4d";
        let r = multiply(self, g, func_multiply_crv_4d, CPoint::scale, op)?;
        Curve::with_indices(r.coefs, r.n, r.p, r.knots, r.m).map_err(|_| oob(op))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfn(fu: &[f64], p: usize, u: &[f64]) -> CurveFunction {
        CurveFunction::new(fu.to_vec(), p, u.to_vec()).unwrap()
    }

    fn eval(f: &CurveFunction, u: f64) -> f64 {
        f.eval(u, Side::Left).unwrap()
    }

    #[test]
    fn refine_keeps_function() {
        let f = cfn(
            &[1.0, -2.0, 3.0, 0.5, 4.0],
            3,
            &[0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0],
        );
        let g = f.refine(&[0.2, 0.4, 0.7, 0.7]).unwrap();
        assert_eq!(g.n(), 8);
        assert_eq!(
            g.knots(),
            &[0.0, 0.0, 0.0, 0.0, 0.2, 0.4, 0.4, 0.7, 0.7, 1.0, 1.0, 1.0, 1.0]
        );
        for k in 0..=20 {
            let u = f64::from(k) / 20.0;
            assert!((eval(&f, u) - eval(&g, u)).abs() < 1e-12);
        }
    }

    #[test]
    fn refine_errors() {
        let f = cfn(&[1.0, 2.0, 3.0], 2, &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        assert!(matches!(
            f.refine(&[]),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        assert!(matches!(
            f.refine(&[0.0, 0.5]),
            Err(SolidError::Nlib {
                code: NlibCode::Parameter,
                ..
            })
        ));
    }

    #[test]
    fn deriv_matches_evaluator() {
        let f = cfn(
            &[1.0, -2.0, 3.0, 0.5, 4.0],
            3,
            &[0.0, 0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0, 1.0],
        );
        let d1 = f.deriv(1).unwrap();
        let d2 = f.deriv(2).unwrap();
        assert_eq!(d1.degree(), 2);
        assert_eq!(d1.knots(), &[0.0, 0.0, 0.0, 0.4, 1.0, 1.0, 1.0]);
        for k in 0..=10 {
            let u = f64::from(k) / 10.0;
            let ds = f.derivs(u, Side::Left, 2).unwrap();
            assert!((ds[1] - eval(&d1, u)).abs() < 1e-9);
            assert!((ds[2] - eval(&d2, u)).abs() < 1e-9);
        }
        assert!(matches!(
            f.deriv(4),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
        assert_eq!(f.deriv(0).unwrap(), f);
    }

    #[test]
    fn product_of_functions_evaluates_to_product() {
        let mut f = cfn(
            &[1.0, -2.0, 3.0, 0.5],
            2,
            &[0.0, 0.0, 0.0, 0.3, 1.0, 1.0, 1.0],
        );
        let mut g = cfn(
            &[2.0, 1.0, -1.0, 0.25, 3.0],
            3,
            &[0.0, 0.0, 0.0, 0.0, 0.6, 1.0, 1.0, 1.0, 1.0],
        );
        let h = f.multiply_func(&mut g).unwrap();
        assert_eq!(h.degree(), 5);
        for k in 0..=20 {
            let u = f64::from(k) / 20.0;
            assert!(
                (eval(&f, u) * eval(&g, u) - eval(&h, u)).abs() < 1e-10,
                "u={u}"
            );
        }
    }

    #[test]
    fn product_rescales_shorter_range() {
        let mut f = cfn(&[1.0, 2.0, 0.0], 2, &[0.0, 0.0, 0.0, 2.0, 2.0, 2.0]);
        let mut g = cfn(&[1.0, 3.0], 1, &[0.0, 0.0, 1.0, 1.0]);
        let h = f.multiply_func(&mut g).unwrap();
        assert_eq!(g.knots(), &[0.0, 0.0, 2.0, 2.0]);
        assert_eq!(h.knots().first(), Some(&0.0));
        assert_eq!(h.knots().last(), Some(&2.0));
        for k in 0..=8 {
            let u = f64::from(k) / 4.0;
            assert!((eval(&f, u) * eval(&g, u) - eval(&h, u)).abs() < 1e-12);
        }
    }

    #[test]
    fn product_zero_degree() {
        let mut f = cfn(&[2.0], 0, &[0.0, 1.0]);
        let mut g = cfn(&[1.0, 3.0], 1, &[5.0, 5.0, 6.0, 6.0]);
        let h = f.multiply_func(&mut g).unwrap();
        assert_eq!(h.values(), &[2.0, 6.0]);
        assert_eq!(h.knots(), &[5.0, 5.0, 6.0, 6.0]);
        let mut f2 = cfn(&[2.0, 3.0], 0, &[0.0, 0.5, 1.0]);
        assert!(matches!(
            f2.multiply_func(&mut g),
            Err(SolidError::Nlib {
                code: NlibCode::Input,
                ..
            })
        ));
    }

    #[test]
    fn product_degree_limit() {
        let k: Vec<f64> = [vec![0.0; 10], vec![1.0; 10]].concat();
        let mut f = cfn(&[1.0; 10], 9, &k);
        let mut g = f.clone();
        assert!(matches!(
            f.multiply_func(&mut g),
            Err(SolidError::Nlib {
                code: NlibCode::Degree,
                ..
            })
        ));
    }

    #[test]
    fn product_with_curve() {
        let mut f = cfn(&[1.0, 2.0, 0.5], 2, &[0.0, 0.0, 0.0, 1.0, 1.0, 1.0]);
        let pw = vec![
            CPoint::new(0.0, 0.0, 0.0, 1.0),
            CPoint::new(2.0, 2.0, 2.0, 2.0),
            CPoint::new(3.0, 0.0, 1.0, 1.0),
            CPoint::new(4.0, 1.0, 0.0, 1.0),
        ];
        let mut c = Curve::new(pw, 2, vec![0.0, 0.0, 0.0, 0.5, 1.0, 1.0, 1.0]).unwrap();
        let h = f.multiply_crv(&mut c.clone()).unwrap();
        let h4 = f.multiply_crv_4d(&mut c).unwrap();
        assert_eq!(h.degree(), 4);
        assert_eq!(h.knots(), h4.knots());
        // The 4d product scales the weights, the xyz product keeps the weights' function.
        let wf = cfn(
            &c.cpts().iter().map(|p| p.w).collect::<Vec<_>>(),
            2,
            c.knots(),
        );
        let wprod = f.multiply_func(&mut wf.clone()).unwrap();
        for (a, b) in h4.cpts().iter().zip(wprod.values()) {
            assert!((a.w - b).abs() < 1e-12);
        }
    }

    /// Fuzz regression: `p + q` overflowed for huge degrees; the sum is above
    /// `NL_MAXDEG` either way.
    #[test]
    fn huge_degree_product_is_a_degree_error() {
        let k = [0.0, 0.0, 1.0, 1.0];
        // (`usize::MAX` does not fit upstream's signed index at all: `InvalidInput`.)
        for p in [1usize << 31, i64::MAX as usize] {
            let mut f = cfn(&[1.0, 2.0], p, &k);
            let mut g = cfn(&[1.0, 2.0], 1, &k);
            let r = f.multiply_func(&mut g);
            assert!(
                matches!(
                    r,
                    Err(SolidError::Nlib {
                        code: NlibCode::Degree,
                        ..
                    })
                ),
                "{r:?}"
            );
            let r = g.multiply_func(&mut f);
            assert!(
                matches!(
                    r,
                    Err(SolidError::Nlib {
                        code: NlibCode::Degree,
                        ..
                    })
                ),
                "{r:?}"
            );
            let r = f.deriv(1);
            assert!(matches!(r, Err(SolidError::InvalidInput { .. })), "{r:?}");
        }
    }
}
