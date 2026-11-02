//! The solver core of the source fork's tangential surface/surface intersection
//! curve tracer: contact points (the surfaces meet with parallel normals) solved by
//! Levenberg-Marquardt, marched along tangential curves and along crossing branches
//! through tangent contact nodes, and probed for the directions a contact continues in.
//!
//! Not upstream: the fork's `SmTangentCurveTracer.cpp` (source fork@ef4ab25,
//! @e9fde8f, @d8c61db, @1693c9d), its anonymous namespace. The pass that uses it,
//! `SmSurfaceIntersector::FindTangentCurves`, runs only with
//! `Kernel::fixes.ssi_tangent_curve_tracer` (WR-75).
//!
//! | Rust                          | Fork                         |
//! |-------------------------------|------------------------------|
//! | [`TcSrfEval`]                 | `TcSrfEval`                  |
//! | [`TcPt`]                      | `TcPt`                       |
//! | [`SolveMode`]                 | `TC_FREE` .. `TC_PLANEFIX`   |
//! | [`TangentTracer`]             | `TcTracer`                   |
//! | [`TangentTracer::eval`]       | `TcTracer::Eval`             |
//! | `clamp` (private)             | `TcTracer::Clamp`            |
//! | `residual` (private)          | `TcTracer::Residual`         |
//! | `solve4` (private)            | `TcTracer::Solve4`           |
//! | [`TangentTracer::solve`]      | `TcTracer::Solve`            |
//! | [`TangentTracer::drop_dir`]   | `TcTracer::DropDir`          |
//! | `exit_fraction` (private)     | `TcTracer::ExitFraction`     |
//! | `step` (private)              | `TcTracer::Step`             |
//! | `solve_node` (private)        | `TcTracer::SolveNode`        |
//! | [`TangentTracer::march`]      | `TcTracer::March`            |
//! | [`TangentTracer::probe`]      | `TcTracer::Probe`            |
//! | [`tc_uv_der`]                 | `sm_TcUVDer`                 |
//! | [`tc_poly_dist`]              | `sm_TcPolyDist`              |
//! | [`tc_near_poly`]              | `sm_TcNearPoly`              |
//! | [`tc_tangents`]               | `sm_TcTangents`              |
//!
//! Deviations: the fork's `TcV` is [`Vec3`] (the same component order in `dot` and
//! `cross`); its `len` and `unit` are `len` and `unit` here (`std::sqrt`, a reciprocal
//! multiply). `m_pNodes` is the `nodes` argument of [`TangentTracer::march`];
//! `m_iFix2` is a [`Cell`] reset when `Step` returns, as the fork's guard does.
//! Outputs are return values (`None` for the fork's `FALSE`; a failed surface
//! evaluation is `None`, [`SolidError::Unported`] is passed on). The solver's
//! residual has at most 10 rows and is kept in `Vec`s. Every loop is bounded as in
//! the fork (200 iterations of 12 tries, 20000 march steps or 5000 points, 20 node
//! passes, 24 probe directions); there is no recursion.

use std::cell::Cell;

use crate::error::{Result, SolidError};
use crate::geom::extent2d::Extent2d;
use crate::kernel::Kernel;
use crate::math::{Point2, Vec3, BIG_DOUBLE, PI};
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::surface::surface::Surface;

const OP: &str = "intersect::surface_intersector::tangent_tracer";

/// The directions `Probe` tries on its circle.
const PROBE_DIRS: i32 = 24;

fn oob() -> SolidError {
    SolidError::invalid_input(OP, "index out of range")
}

fn at<T: Copy>(a: &[T], i: usize) -> Result<T> {
    a.get(i).copied().ok_or_else(oob)
}

fn at_mut<T>(a: &mut [T], i: usize) -> Result<&mut T> {
    a.get_mut(i).ok_or_else(oob)
}

/// `TcV::len`.
fn len(v: Vec3) -> f64 {
    v.dot(v).sqrt()
}

/// `TcV::unit`: `v` scaled by the reciprocal of its length, `v` itself when it is zero.
fn unit(v: Vec3) -> Vec3 {
    let l = len(v);
    if l > 0.0 {
        v * (1.0 / l)
    } else {
        v
    }
}

/// `SM_DEG2RAD`.
fn deg2rad(d: f64) -> f64 {
    d * (PI / 180.0)
}

/// `smos_Max`.
fn smax(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `smos_Min`.
fn smin(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// Component `k` of `v` (0 x, 1 y, else z).
fn comp(v: Vec3, k: usize) -> f64 {
    match k {
        0 => v.x,
        1 => v.y,
        _ => v.z,
    }
}

/// One surface evaluation with what the solver needs (`TcSrfEval`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TcSrfEval {
    /// The point.
    pub p: Vec3,
    /// First partial in u.
    pub su: Vec3,
    /// First partial in v.
    pub sv: Vec3,
    /// Second partial in u.
    pub suu: Vec3,
    /// Mixed partial.
    pub suv: Vec3,
    /// Second partial in v.
    pub svv: Vec3,
    /// The unit normal.
    pub n: Vec3,
    /// The unit normal's derivative in u.
    pub nu: Vec3,
    /// The unit normal's derivative in v.
    pub nv: Vec3,
}

/// A point of a tangential curve (`TcPt`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct TcPt {
    /// `u1 v1 u2 v2`.
    pub x: [f64; 4],
    /// The 3D point: the mean of the two surface points.
    pub p: Vec3,
    /// The unit normal of surface 1.
    pub n: Vec3,
    /// The unit normal of surface 2.
    pub n2: Vec3,
    /// `|S1 - S2|`.
    pub gap: f64,
    /// The angle between the tangent planes.
    pub ang: f64,
}

/// The constraints of [`TangentTracer::solve`] besides the contact rows
/// (`TC_FREE`, `TC_PLANE`, `TC_FIX`, `TC_PLANEFIX`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum SolveMode {
    /// No constraint.
    Free,
    /// Both surface points on the plane through `q` normal to `t`.
    Plane,
    /// One parameter held at a value.
    Fix,
    /// Both.
    PlaneFix,
}

impl SolveMode {
    const fn plane(self) -> bool {
        matches!(self, Self::Plane | Self::PlaneFix)
    }

    const fn fix(self) -> bool {
        matches!(self, Self::Fix | Self::PlaneFix)
    }
}

/// The residual rows and Jacobian of one solver evaluation, and the two surface
/// evaluations they come from.
struct ResidualEval {
    r: Vec<f64>,
    j: Vec<[f64; 4]>,
    e: [TcSrfEval; 2],
}

/// The tracer (`TcTracer`) over two surfaces and their domains.
#[derive(Debug, Clone)]
pub(crate) struct TangentTracer<'a> {
    surfaces: [&'a Surface; 2],
    domains: [Extent2d; 2],
    /// `m_dTol`: the 3D approximation tolerance.
    tol: f64,
    /// `m_dSize`: the model size.
    size: f64,
    /// `m_dL`: the weight of the normal residual (a length).
    l: f64,
    /// `m_dSign`: the orientation of `n2` relative to `n1` on the current curve.
    pub sign: f64,
    /// `m_bTangent`: tangential curves (normals parallel), else crossing curves.
    pub tangent: bool,
    /// `m_iFix2` / `m_dFix2`: a second parameter held on a domain boundary.
    fix2: Cell<Option<(usize, f64)>>,
}

/// Resets the second held parameter when a step returns (the fork's `TcFix2Guard`).
struct Fix2Guard<'t> {
    cell: &'t Cell<Option<(usize, f64)>>,
}

impl Drop for Fix2Guard<'_> {
    fn drop(&mut self) {
        self.cell.set(None);
    }
}

impl<'a> TangentTracer<'a> {
    /// `TcTracer(apSrf, aDom, dTol, dSize)`.
    pub fn new(surfaces: [&'a Surface; 2], domains: [Extent2d; 2], tol: f64, size: f64) -> Self {
        Self {
            surfaces,
            domains,
            tol,
            size,
            l: 0.05 * size,
            sign: 1.0,
            tangent: true,
            fix2: Cell::new(None),
        }
    }

    /// `Eval`: surface `s` at `(u, v)` with its second derivatives, unit normal and the
    /// normal's derivatives; at a degenerate point (a pole, a collapsed or folded edge)
    /// the limit normal with zero derivatives. `None` where the surface cannot be
    /// evaluated.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn eval(&self, k: &Kernel, s: usize, u: f64, v: f64) -> Result<Option<TcSrfEval>> {
        let srf = *self.surfaces.get(s).ok_or_else(oob)?;
        let uv = Point2::new(u, v);
        let flags = EvaluateFlags {
            u_side: Side::Left,
            v_side: Side::Left,
            only_upper_half: true,
            non_zero_tangents: true,
        };
        let d = match srf.evaluate(k, uv, 2, 2, flags) {
            Ok(d) => d,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Ok(None),
        };
        let g = |i: usize, j: usize| -> Result<Vec3> {
            d.get(i).and_then(|r| r.get(j)).copied().ok_or_else(oob)
        };
        let p = g(0, 0)?;
        let sv = g(0, 1)?;
        let svv = g(0, 2)?;
        let su = g(1, 0)?;
        let suv = g(1, 1)?;
        let suu = g(2, 0)?;
        let big_n = su.cross(sv);
        let d_len = len(big_n);
        let regular = d_len > 1e-12 * (su.dot(su) + sv.dot(sv)) && d_len > 1e-300;
        if !regular {
            // degenerate point (pole, collapsed or folded edge): the limit normal
            let sn = match srf.evaluate_normal(k, uv, Side::Left, Side::Left) {
                Ok(n) => n,
                Err(e) if e.is_unported() => return Err(e),
                Err(_) => return Ok(None),
            };
            let n = unit(sn);
            if len(n) <= 0.5 || len(n).is_nan() {
                return Ok(None);
            }
            return Ok(Some(TcSrfEval {
                p,
                su,
                sv,
                suu,
                suv,
                svv,
                n,
                nu: Vec3::ZERO,
                nv: Vec3::ZERO,
            }));
        }
        let n = big_n * (1.0 / d_len);
        let n_u = suu.cross(sv) + su.cross(suv);
        let n_v = suv.cross(sv) + su.cross(svv);
        // d(N/|N|) = (I - n n^T) dN / |N|
        let nu = (n_u - n * n.dot(n_u)) * (1.0 / d_len);
        let nv = (n_v - n * n.dot(n_v)) * (1.0 / d_len);
        Ok(Some(TcSrfEval {
            p,
            su,
            sv,
            suu,
            suv,
            svv,
            n,
            nu,
            nv,
        }))
    }

    /// `Clamp`: both parameter pairs clamped to their domains.
    fn clamp(&self, x: &mut [f64; 4]) {
        for (s, dom) in self.domains.iter().enumerate() {
            let (i, j) = (2 * s, 2 * s + 1);
            let (Some(&u), Some(&v)) = (x.get(i), x.get(j)) else {
                continue;
            };
            let p = dom.clamp_point(Point2::new(u, v));
            if let Some(xu) = x.get_mut(i) {
                *xu = p.x;
            }
            if let Some(xv) = x.get_mut(j) {
                *xv = p.y;
            }
        }
    }

    /// The partial a held parameter is weighted by: 0 `S1_u`, 1 `S1_v`, 2 `S2_u`,
    /// else `S2_v`.
    fn fix_partial(e: &[TcSrfEval; 2], i: usize) -> Vec3 {
        match i {
            0 => e[0].su,
            1 => e[0].sv,
            2 => e[1].su,
            _ => e[1].sv,
        }
    }

    /// `Residual`: the residual rows and Jacobian at `x`; `None` (the fork's 0 rows)
    /// when a surface cannot be evaluated.
    fn residual(
        &self,
        k: &Kernel,
        x: &[f64; 4],
        mode: SolveMode,
        q: Vec3,
        t: Vec3,
        i_fix: Option<usize>,
        d_fix: f64,
    ) -> Result<Option<ResidualEval>> {
        let Some(e0) = self.eval(k, 0, x[0], x[1])? else {
            return Ok(None);
        };
        let Some(e1) = self.eval(k, 1, x[2], x[3])? else {
            return Ok(None);
        };
        let e = [e0, e1];
        let mut r: Vec<f64> = Vec::with_capacity(10);
        let mut j: Vec<[f64; 4]> = Vec::with_capacity(10);
        let d = e0.p - e1.p;
        for kk in 0..3 {
            r.push(comp(d, kk));
            j.push([
                comp(e0.su, kk),
                comp(e0.sv, kk),
                -comp(e1.su, kk),
                -comp(e1.sv, kk),
            ]);
        }
        let dn = e0.n - e1.n * self.sign;
        if self.tangent {
            for kk in 0..3 {
                r.push(self.l * comp(dn, kk));
                j.push([
                    self.l * comp(e0.nu, kk),
                    self.l * comp(e0.nv, kk),
                    -self.sign * self.l * comp(e1.nu, kk),
                    -self.sign * self.l * comp(e1.nv, kk),
                ]);
            }
        }
        if mode.plane() {
            r.push((e0.p - q).dot(t));
            j.push([e0.su.dot(t), e0.sv.dot(t), 0.0, 0.0]);
            r.push((e1.p - q).dot(t));
            j.push([0.0, 0.0, e1.su.dot(t), e1.sv.dot(t)]);
        }
        let mut fix_row = |i: usize, value: f64| -> Result<()> {
            // weight the parameter residual to a length
            let w = smax(len(Self::fix_partial(&e, i)), 1e-12);
            r.push(w * (at(x, i)? - value));
            let mut row = [0.0; 4];
            *at_mut(&mut row, i)? = w;
            j.push(row);
            Ok(())
        };
        if mode.fix() {
            fix_row(i_fix.ok_or_else(oob)?, d_fix)?;
        }
        if let Some((i2, d2)) = self.fix2.get() {
            fix_row(i2, d2)?;
        }
        Ok(Some(ResidualEval { r, j, e }))
    }

    /// `Solve4`: `A z = b` by Gaussian elimination with partial pivoting; `None` for a
    /// singular pivot.
    fn solve4(a: &[[f64; 4]; 4], b: &[f64; 4]) -> Result<Option<[f64; 4]>> {
        let mut m = [[0.0_f64; 5]; 4];
        for ((row, ar), &bi) in m.iter_mut().zip(a).zip(b) {
            for (dst, &src) in row.iter_mut().zip(ar) {
                *dst = src;
            }
            row[4] = bi;
        }
        for c in 0..4 {
            let mut p = c;
            for i in c + 1..4 {
                if at(&at(&m, i)?, c)?.abs() > at(&at(&m, p)?, c)?.abs() {
                    p = i;
                }
            }
            let pivot = at(&at(&m, p)?, c)?;
            if pivot.abs() <= 1e-300 || pivot.is_nan() {
                return Ok(None);
            }
            if p != c {
                m.swap(c, p);
            }
            let mc = at(&m, c)?;
            let mcc = at(&mc, c)?;
            for i in c + 1..4 {
                let mi = at_mut(&mut m, i)?;
                let f = at(mi, c)? / mcc;
                for jj in c..5 {
                    *at_mut(mi, jj)? -= f * at(&mc, jj)?;
                }
            }
        }
        let mut z = [0.0_f64; 4];
        for i in (0..4).rev() {
            let mi = at(&m, i)?;
            let mut s = mi[4];
            for jj in i + 1..4 {
                s -= at(&mi, jj)? * at(&z, jj)?;
            }
            *at_mut(&mut z, i)? = s / at(&mi, i)?;
        }
        Ok(Some(z))
    }

    /// `Solve`: Levenberg-Marquardt on the residual from `x` (in/out), then the
    /// acceptance tests; the contact point, or `None` (the fork's `FALSE`).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn solve(
        &self,
        k: &Kernel,
        x: &mut [f64; 4],
        mode: SolveMode,
        q: Vec3,
        t: Vec3,
        i_fix: Option<usize>,
        d_fix: f64,
    ) -> Result<Option<TcPt>> {
        self.clamp(x);
        let Some(mut cur) = self.residual(k, x, mode, q, t, i_fix, d_fix)? else {
            return Ok(None);
        };
        let mut cost = 0.0;
        for &ri in &cur.r {
            cost += ri * ri;
        }
        let mut lambda = 1e-3;
        let mut it = 0;
        while it < 200 && cost > 1e-34 {
            let mut a = [[0.0_f64; 4]; 4];
            let mut g = [0.0_f64; 4];
            for (aa, (arow, ga)) in a.iter_mut().zip(g.iter_mut()).enumerate() {
                for (ji, &ri) in cur.j.iter().zip(&cur.r) {
                    *ga += at(ji, aa)? * ri;
                }
                for (bb, ab) in arow.iter_mut().enumerate() {
                    for ji in &cur.j {
                        *ab += at(ji, aa)? * at(ji, bb)?;
                    }
                }
            }
            let mut diag_max = 0.0;
            for (aa, arow) in a.iter().enumerate() {
                diag_max = smax(diag_max, at(arow, aa)?);
            }
            let mut stepped = false;
            let mut converged = false;
            for _tries in 0..12 {
                let mut bm = a;
                let mut nb = [0.0_f64; 4];
                for (aa, (brow, nba)) in bm.iter_mut().zip(nb.iter_mut()).enumerate() {
                    let ad = at(&at(&a, aa)?, aa)?;
                    *at_mut(brow, aa)? += lambda * (ad + 1e-12 * diag_max) + 1e-30;
                    *nba = -at(&g, aa)?;
                }
                let Some(z) = Self::solve4(&bm, &nb)? else {
                    lambda *= 10.0;
                    continue;
                };
                let mut xt = [x[0] + z[0], x[1] + z[1], x[2] + z[2], x[3] + z[3]];
                self.clamp(&mut xt);
                let Some(trial) = self.residual(k, &xt, mode, q, t, i_fix, d_fix)? else {
                    lambda *= 10.0;
                    continue;
                };
                let mut cost_t = 0.0;
                for &ri in &trial.r {
                    cost_t += ri * ri;
                }
                if cost_t < cost {
                    let step = (xt[0] - x[0]).abs()
                        + (xt[1] - x[1]).abs()
                        + (xt[2] - x[2]).abs()
                        + (xt[3] - x[3]).abs();
                    *x = xt;
                    cur = trial;
                    cost = cost_t;
                    lambda = smax(lambda / 4.0, 1e-12);
                    stepped = true;
                    if step < 1e-15 {
                        converged = true;
                    }
                    break;
                }
                lambda *= 8.0;
            }
            if !stepped || converged {
                break;
            }
            it += 1;
        }

        // judge
        let [e0, e1] = cur.e;
        let gap = len(e0.p - e1.p);
        let dn = e0.n - e1.n * self.sign;
        if gap > 0.1 * self.tol {
            return Ok(None);
        }
        if self.tangent && len(dn) > 1e-8 {
            return Ok(None);
        }
        if mode.plane() && (e0.p - q).dot(t).abs() > 0.1 * self.tol {
            return Ok(None);
        }
        if mode.fix() {
            let i = i_fix.ok_or_else(oob)?;
            if (at(x, i)? - d_fix).abs() > 1e-9 * (1.0 + d_fix.abs()) {
                return Ok(None);
            }
        }
        if let Some((i2, d2)) = self.fix2.get() {
            if (at(x, i2)? - d2).abs() > 1e-9 * (1.0 + d2.abs()) {
                return Ok(None);
            }
        }
        Ok(Some(TcPt {
            x: *x,
            p: (e0.p + e1.p) * 0.5,
            n: e0.n,
            n2: e1.n,
            gap,
            ang: smin(1.0, len(e0.n.cross(e1.n))).asin(),
        }))
    }

    /// `DropDir`: a 3D tangent vector dropped into both parameter spaces; `None` where
    /// a surface cannot be evaluated or its first fundamental form is singular.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn drop_dir(&self, k: &Kernel, x: &[f64; 4], t: Vec3) -> Result<Option<[f64; 4]>> {
        let mut duv = [0.0_f64; 4];
        for s in 0..2 {
            let Some(e) = self.eval(k, s, at(x, 2 * s)?, at(x, 2 * s + 1)?)? else {
                return Ok(None);
            };
            let a = e.su.dot(e.su);
            let b = e.su.dot(e.sv);
            let c = e.sv.dot(e.sv);
            let p = e.su.dot(t);
            let q = e.sv.dot(t);
            let det = a * c - b * b;
            if det.abs() <= 1e-300 || det.is_nan() {
                return Ok(None);
            }
            *at_mut(&mut duv, 2 * s)? = (c * p - b * q) / det;
            *at_mut(&mut duv, 2 * s + 1)? = (-b * p + a * q) / det;
        }
        Ok(Some(duv))
    }

    /// `ExitFraction`: the largest `t` in `[0, 1]` with `x + t dx` inside both domains,
    /// and the parameter and bound that limit it.
    fn exit_fraction(&self, x: &[f64; 4], dx: &[f64; 4]) -> Result<(f64, Option<(usize, f64)>)> {
        let mut t = 1.0;
        let mut fix = None;
        for (s, dom) in self.domains.iter().enumerate() {
            let lo = [dom.min().x, dom.min().y];
            let hi = [dom.max().x, dom.max().y];
            for kk in 0..2 {
                let i = 2 * s + kk;
                let (xi, dxi) = (at(x, i)?, at(dx, i)?);
                let (lok, hik) = (at(&lo, kk)?, at(&hi, kk)?);
                if xi + dxi > hik && dxi > 0.0 {
                    let f = (hik - xi) / dxi;
                    if f < t {
                        t = smax(f, 0.0);
                        fix = Some((i, hik));
                    }
                }
                if xi + dxi < lok && dxi < 0.0 {
                    let f = (lok - xi) / dxi;
                    if f < t {
                        t = smax(f, 0.0);
                        fix = Some((i, lok));
                    }
                }
            }
        }
        Ok((t, fix))
    }

    /// `Step`: one predictor/corrector step from `from` along `t` with step `h`:
    /// 1 a point found, 2 an end point on a domain boundary found, 3 on the boundary
    /// heading out, 0 failed.
    fn step(&self, k: &Kernel, from: &TcPt, t: Vec3, h: f64) -> Result<(i32, Option<TcPt>)> {
        let Some(duv) = self.drop_dir(k, &from.x, t)? else {
            return Ok((0, None));
        };
        let mut dx = [h * duv[0], h * duv[1], h * duv[2], h * duv[3]];
        // a curve running along a domain boundary (an edge of one surface lying
        // on the other) keeps that parameter on the boundary
        let mut along: Option<(usize, f64)> = None;
        let mut along2: Option<(usize, f64)> = None;
        for i in 0..4 {
            let ls = i / 2;
            let dom = self.domains.get(ls).ok_or_else(oob)?;
            let (lo, hi) = if i % 2 == 1 {
                (dom.min().y, dom.max().y)
            } else {
                (dom.min().x, dom.max().x)
            };
            let eps = 1e-10 * (1.0 + hi - lo);
            let (xi, dxi) = (at(&from.x, i)?, at(&dx, i)?);
            let out = (xi >= hi - eps && dxi > 0.0) || (xi <= lo + eps && dxi < 0.0);
            if !out {
                continue;
            }
            let (d0, d1) = (at(&dx, 2 * ls)?, at(&dx, 2 * ls + 1)?);
            let dl = (d0 * d0 + d1 * d1).sqrt();
            if dxi.abs() > 0.2 * dl || along2.is_some() {
                return Ok((3, None)); // on the boundary heading out
            }
            let bound = if dxi > 0.0 { hi } else { lo };
            if along.is_some() {
                along2 = Some((i, bound));
            } else {
                along = Some((i, bound));
            }
            *at_mut(&mut dx, i)? = 0.0;
        }
        // holds the second boundary parameter in the solves below
        self.fix2.set(along2);
        let _guard = Fix2Guard { cell: &self.fix2 };
        let (f, fix) = self.exit_fraction(&from.x, &dx)?;
        if let Some((i_fix, d_fix)) = fix {
            if f * h < 1e-3 * self.tol {
                return Ok((3, None)); // already on the boundary heading out
            }
            let mut xg = [
                from.x[0] + f * dx[0],
                from.x[1] + f * dx[1],
                from.x[2] + f * dx[2],
                from.x[3] + f * dx[3],
            ];
            *at_mut(&mut xg, i_fix)? = d_fix;
            if along.is_some() && along2.is_none() {
                self.fix2.set(along);
            }
            if let Some(end) = self.solve(
                k,
                &mut xg,
                SolveMode::Fix,
                Vec3::ZERO,
                Vec3::ZERO,
                Some(i_fix),
                d_fix,
            )? {
                let dp = end.p - from.p;
                let d_len = len(dp);
                if d_len < 1e-3 * self.tol {
                    return Ok((3, None));
                }
                if dp.dot(t) > 0.7 * d_len && d_len < 2.0 * f * h + 10.0 * self.tol {
                    return Ok((2, Some(end)));
                }
            }
            return Ok((0, None));
        }
        let mut xg = [
            from.x[0] + dx[0],
            from.x[1] + dx[1],
            from.x[2] + dx[2],
            from.x[3] + dx[3],
        ];
        let q = from.p + t * h;
        let next = match along {
            Some((i_along, d_along)) => self.solve(
                k,
                &mut xg,
                SolveMode::PlaneFix,
                q,
                t,
                Some(i_along),
                d_along,
            )?,
            None => self.solve(k, &mut xg, SolveMode::Plane, q, t, None, 0.0)?,
        };
        let Some(next) = next else {
            return Ok((0, None));
        };
        let dp = next.p - from.p;
        if len(next.p - q) > 0.3 * h {
            return Ok((0, None));
        }
        if dp.dot(t) < 0.9 * len(dp) {
            return Ok((0, None));
        }
        Ok((1, Some(next)))
    }

    /// `SolveNode`: the contact point (normals parallel) near a point of a crossing
    /// curve.
    fn solve_node(&self, k: &Kernel, from: &TcPt) -> Result<Option<TcPt>> {
        let mut tr = self.clone();
        tr.tangent = true;
        tr.sign = if from.n.dot(from.n2) < 0.0 { -1.0 } else { 1.0 };
        let mut x = from.x;
        tr.solve(
            k,
            &mut x,
            SolveMode::Free,
            Vec3::ZERO,
            Vec3::ZERO,
            None,
            0.0,
        )
    }

    /// `March`: marches from `p0` in direction `t0`, appending the points (not `p0`)
    /// to `pts`; `true` when the curve closed back onto `start`. In crossing mode
    /// (`tangent` false) with `nodes` (the fork's `m_pNodes`) the curve runs on
    /// through the contact points where it meets other branches, adding the ones it
    /// finds.
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn march(
        &self,
        k: &Kernel,
        p0: &TcPt,
        t0: Vec3,
        start: &TcPt,
        pts: &mut Vec<TcPt>,
        mut nodes: Option<&mut Vec<TcPt>>,
    ) -> Result<bool> {
        let h0 = 2e-3 * self.size;
        let h_max = 0.05 * self.size;
        let h_min = smax(1e-7 * self.size, 10.0 * self.tol);
        let mut h = h0;
        let mut cur = *p0;
        let mut t = t0;
        let mut travel = 0.0;
        let mut node_passes = 0;
        let mut cur_is_node = p0.ang < deg2rad(0.01);
        let mut iter = 0;
        while iter < 20000 && pts.len() < 5000 {
            iter += 1;
            let (res, nxt) = self.step(k, &cur, t, h)?;
            if res == 3 {
                return Ok(false);
            }
            let Some(nxt) = nxt.filter(|_| res != 0) else {
                h *= 0.5;
                if h < h_min {
                    return Ok(false);
                }
                continue;
            };
            let dp = nxt.p - cur.p;
            let d_len = len(dp);
            if d_len < 1e-3 * self.tol {
                return Ok(false);
            }
            let c = dp * (1.0 / d_len);
            // the chord direction is close to the mean of the end tangents
            let tn = c * 2.0 - t;
            let tn = unit(tn - nxt.n * nxt.n.dot(tn));
            let d_cos = tn.dot(t);
            if d_cos < deg2rad(6.0).cos() && h > h_min {
                h *= 0.5;
                continue;
            }
            // crossing curves pass through the contact points where they meet other
            // branches: the curve continues along the branch leaving the node in the
            // arrival direction (and ends there when no branch does)
            if let (false, Some(nodes)) = (self.tangent, nodes.as_deref_mut()) {
                let mut at_node: Option<TcPt> = None;
                for r_m in nodes.iter() {
                    if at_node.is_some() {
                        break;
                    }
                    if len(r_m.p - cur.p) < 10.0 * self.tol {
                        continue; // just left it
                    }
                    let dm = r_m.p - cur.p;
                    let dd = len(dm);
                    if dd <= 1.05 * d_len + 10.0 * self.tol && dm.dot(t) > 0.5 * dd {
                        // the node in this curve's parameter chart (periodic seams)
                        let Some(m) = self.solve_node(k, &cur)? else {
                            return Ok(false);
                        };
                        if len(m.p - r_m.p) > 100.0 * self.tol {
                            return Ok(false);
                        }
                        at_node = Some(m);
                        pts.push(m);
                    }
                }
                if at_node.is_none() && !cur_is_node && pts.len() >= 2 {
                    let a = at(pts, pts.len() - 2)?.ang;
                    let b = cur.ang;
                    let cc = nxt.ang;
                    if b < a && b <= cc && b < deg2rad(3.0) {
                        if let Some(m) = self.solve_node(k, &cur)? {
                            if len(m.p - cur.p) < 2.0 * d_len {
                                if (m.p - cur.p).dot(t) > 0.0 {
                                    pts.push(m);
                                } else if let Some(last) = pts.last_mut() {
                                    *last = m;
                                }
                                let mut known = false;
                                for node in nodes.iter() {
                                    if len(node.p - m.p) < 100.0 * self.tol {
                                        known = true;
                                    }
                                }
                                if !known {
                                    nodes.push(m);
                                }
                                at_node = Some(m);
                            }
                        }
                    }
                }
                if let Some(m) = at_node {
                    let back = if pts.len() >= 2 { pts.len() - 2 } else { 0 };
                    let mut t_in = unit(m.p - at(pts, back)?.p);
                    if pts.len() < 2 {
                        t_in = t;
                    }
                    if len(m.p - start.p) < 100.0 * self.tol && t_in.dot(t0) > 0.5 {
                        // closed through the start node
                        if let Some(last) = pts.last_mut() {
                            *last = *start;
                        }
                        return Ok(true);
                    }
                    node_passes += 1;
                    if node_passes > 20 {
                        return Ok(false);
                    }
                    let mut out = Vec::new();
                    if self.probe(k, &m, &mut out)? < 0 {
                        return Ok(false);
                    }
                    let mut best = deg2rad(45.0).cos();
                    let mut cont = false;
                    for o in &out {
                        if o.dot(t_in) > best {
                            best = o.dot(t_in);
                            t = *o;
                            cont = true;
                        }
                    }
                    if !cont {
                        return Ok(false);
                    }
                    travel += len(m.p - cur.p);
                    cur = m;
                    h = h0;
                    cur_is_node = true;
                    continue;
                }
            }
            // closing back onto the start point
            if res == 1 && pts.len() >= 3 && travel > 4.0 * h {
                let ds = start.p - cur.p;
                let dist = len(ds);
                if dist < 1.5 * h && ds.dot(t) > 0.5 * dist {
                    pts.push(*start);
                    return Ok(true);
                }
            }
            pts.push(nxt);
            travel += d_len;
            if res == 2 {
                return Ok(false);
            }
            cur = nxt;
            cur_is_node = false;
            t = tn;
            if d_cos > deg2rad(1.5).cos() {
                h = smin(h * 1.5, h_max);
            }
        }
        Ok(false)
    }

    /// `Probe`: the tangent-plane directions in which the contact continues from `p`
    /// (24 directions on a circle of radius `2e-3` model sizes, clustered within 30
    /// degrees) into `dirs`; their number, or -1 when the contact holds in (almost)
    /// every direction (coincident surfaces).
    ///
    /// # Errors
    ///
    /// [`SolidError::Unported`] from the evaluations.
    pub fn probe(&self, k: &Kernel, p: &TcPt, dirs: &mut Vec<Vec3>) -> Result<i32> {
        dirs.clear();
        let n = p.n;
        let a = if n.x.abs() < 0.6 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0)
        };
        let e1 = unit(a - n * n.dot(a));
        let e2 = n.cross(e1);
        let r = 2e-3 * self.size;
        let mut hits: Vec<Vec3> = Vec::new();
        let mut num_exact = 0;
        for jj in 0..PROBE_DIRS {
            let th = 2.0 * PI * f64::from(jj) / f64::from(PROBE_DIRS);
            let d = e1 * th.cos() + e2 * th.sin();
            let Some(duv) = self.drop_dir(k, &p.x, d)? else {
                continue;
            };
            let mut xg = [
                p.x[0] + r * duv[0],
                p.x[1] + r * duv[1],
                p.x[2] + r * duv[2],
                p.x[3] + r * duv[3],
            ];
            let q_pt = p.p + d * r;
            let Some(q) = self.solve(k, &mut xg, SolveMode::Plane, q_pt, d, None, 0.0)? else {
                continue;
            };
            let off = len(q.p - q_pt);
            if off > 0.35 * r {
                continue;
            }
            // contact right at the probe point: the surfaces coincide there
            // (on a tangential curve this happens only when probing along it)
            if off < 0.01 * r {
                num_exact += 1;
            }
            hits.push(unit(q.p - p.p));
        }
        if num_exact >= 5 || hits.len() > (PROBE_DIRS / 2) as usize {
            return Ok(-1);
        }
        // cluster
        let cos30 = deg2rad(30.0).cos();
        for hit in &hits {
            if !dirs.iter().any(|d| d.dot(*hit) > cos30) {
                dirs.push(*hit);
            }
        }
        i32::try_from(dirs.len()).map_err(|_| oob())
    }
}

/// `sm_TcUVDer`: the uv derivatives (per unit 3D arc length) at point `i` of a traced
/// curve: the exact tangent `tan[i]` dropped into both parameter planes, or the
/// finite difference one `fd[4 i ..]` where that is unreliable (degenerate
/// parametrisation at poles and corners).
///
/// # Errors
///
/// `InvalidInput` for an index past the arrays; [`SolidError::Unported`] from the
/// evaluations.
pub(crate) fn tc_uv_der(
    tr: &TangentTracer<'_>,
    k: &Kernel,
    c: &[TcPt],
    tan: &[Vec3],
    fd: &[f64],
    i: usize,
) -> Result<[f64; 4]> {
    let dropped = tr.drop_dir(k, &at(c, i)?.x, at(tan, i)?)?;
    let mut ok = dropped.is_some();
    let d = dropped.unwrap_or([0.0; 4]);
    for ls in 0..2 {
        if !ok {
            break;
        }
        let (d0, d1) = (at(&d, 2 * ls)?, at(&d, 2 * ls + 1)?);
        let de = (d0 * d0 + d1 * d1).sqrt();
        let (f0, f1) = (at(fd, 4 * i + 2 * ls)?, at(fd, 4 * i + 2 * ls + 1)?);
        let df = (f0 * f0 + f1 * f1).sqrt();
        let fine = de <= 2.0 * df + 1e-12;
        if !fine {
            ok = false;
        }
    }
    let mut duv = [0.0; 4];
    for (a, slot) in duv.iter_mut().enumerate() {
        *slot = if ok { at(&d, a)? } else { at(fd, 4 * i + a)? };
    }
    Ok(duv)
}

/// `sm_TcPolyDist`: the distance from `x` to the polyline of `poly`, the segment it
/// is nearest and the parameter on that segment.
#[must_use]
pub(crate) fn tc_poly_dist(x: Vec3, poly: &[TcPt]) -> (f64, usize, f64) {
    if let [only] = poly {
        return (len(x - only.p), 0, 0.0);
    }
    let mut best = (BIG_DOUBLE, 0, 0.0);
    for (i, seg) in poly.windows(2).enumerate() {
        let [p0, p1] = seg else {
            continue;
        };
        let a = p0.p;
        let d = p1.p - a;
        let dd = d.dot(d);
        let t = if dd > 0.0 { (x - a).dot(d) / dd } else { 0.0 };
        let t = smax(0.0, smin(1.0, t));
        let dist = len(x - (a + d * t));
        if dist < best.0 {
            best = (dist, i, t);
        }
    }
    best
}

/// `sm_TcNearPoly`: whether `x` is on a traced curve: within `tol` of its polyline,
/// allowing for the chord error of its spans (a span turns by a few degrees at most).
#[must_use]
pub(crate) fn tc_near_poly(x: Vec3, poly: &[TcPt], tol: f64) -> bool {
    if let [only] = poly {
        return len(x - only.p) < tol;
    }
    for seg in poly.windows(2) {
        let [p0, p1] = seg else {
            continue;
        };
        let a = p0.p;
        let d = p1.p - a;
        let dd = d.dot(d);
        let t = if dd > 0.0 { (x - a).dot(d) / dd } else { 0.0 };
        let t = smax(0.0, smin(1.0, t));
        if len(x - (a + d * t)) < smax(tol, 0.02 * dd.sqrt()) {
            return true;
        }
    }
    false
}

/// `sm_TcTangents`: the unit tangents of the polyline of `c` by the derivative of the
/// parabola through three neighbours (across the closure for a closed curve), and
/// the uv derivatives the same way (`pOptUVDer`, four per point).
///
/// # Errors
///
/// `InvalidInput` for a curve of fewer than three points that is not two (the fork
/// reads past its points).
pub(crate) fn tc_tangents(c: &[TcPt], closed: bool) -> Result<(Vec<Vec3>, Vec<f64>)> {
    let n = c.len();
    let mut tan = vec![Vec3::ZERO; n];
    let mut fd = vec![0.0; 4 * n];
    let mut s = vec![0.0; n];
    for i in 1..n {
        *at_mut(&mut s, i)? = at(&s, i - 1)? + len(at(c, i)?.p - at(c, i - 1)?.p);
    }
    if n == 2 {
        let t = unit(at(c, 1)?.p - at(c, 0)?.p);
        *at_mut(&mut tan, 0)? = t;
        *at_mut(&mut tan, 1)? = t;
        let s1 = at(&s, 1)?;
        if s1 > 0.0 {
            for a in 0..4 {
                let v = (at(&at(c, 1)?.x, a)? - at(&at(c, 0)?.x, a)?) / s1;
                *at_mut(&mut fd, a)? = v;
                *at_mut(&mut fd, 4 + a)? = v;
            }
        }
        return Ok((tan, fd));
    }
    for i in 0..n {
        let (ja, jb, jc, ta, tb, tc, t);
        if closed && n > 3 && (i == 0 || i == n - 1) {
            ja = n - 2;
            jb = 0;
            jc = 1;
            ta = -(at(&s, n - 1)? - at(&s, n - 2)?);
            tb = 0.0;
            tc = at(&s, 1)?;
            t = 0.0;
        } else {
            let i0 = if i == 0 {
                0
            } else if i == n - 1 {
                n.checked_sub(3).ok_or_else(oob)?
            } else {
                i - 1
            };
            ja = i0;
            jb = i0 + 1;
            jc = i0 + 2;
            ta = at(&s, i0)?;
            tb = at(&s, i0 + 1)?;
            tc = at(&s, i0 + 2)?;
            t = at(&s, i)?;
        }
        let (pa, pb, pc) = (at(c, ja)?, at(c, jb)?, at(c, jc)?);
        let w0 = ((t - tb) + (t - tc)) / ((ta - tb) * (ta - tc));
        let w1 = ((t - ta) + (t - tc)) / ((tb - ta) * (tb - tc));
        let w2 = ((t - ta) + (t - tb)) / ((tc - ta) * (tc - tb));
        *at_mut(&mut tan, i)? = unit(pa.p * w0 + pb.p * w1 + pc.p * w2);
        for a in 0..4 {
            *at_mut(&mut fd, 4 * i + a)? =
                at(&pa.x, a)? * w0 + at(&pb.x, a)? * w1 + at(&pc.x, a)? * w2;
        }
    }
    Ok((tan, fd))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::axis2_placement::Axis2Placement;
    use crate::nurbs::b_spline_surface::BSplineSurface;
    use crate::surface::sphere::Sphere;

    fn plane_z(z: f64) -> Surface {
        Surface::from(
            BSplineSurface::create_bilinear_surface(
                Vec3::new(-1.0, -1.0, z),
                Vec3::new(1.0, -1.0, z),
                Vec3::new(-1.0, 1.0, z),
                Vec3::new(1.0, 1.0, z),
            )
            .unwrap(),
        )
    }

    /// A unit sphere resting on the plane z = -1 touches it at its south pole only:
    /// the contact point is found, and no direction continues from it.
    #[test]
    fn sphere_on_plane_is_a_touching_point() {
        let k = Kernel::default();
        let sphere =
            Surface::from(Sphere::create_canonical(&k, &Axis2Placement::default(), 1.0).unwrap());
        let plane = plane_z(-1.0);
        let doms = [
            sphere.natural_uv_domain_with_nurb(&k).unwrap(),
            plane.natural_uv_domain_with_nurb(&k).unwrap(),
        ];
        let mut tr = TangentTracer::new([&sphere, &plane], doms, 1e-6, 2.0);
        tr.sign = -1.0;
        // near the south pole (v = -90 on the sphere) and the plane's middle
        let mut x = [10.0, -85.0, 0.5, 0.5];
        let p = tr
            .solve(
                &k,
                &mut x,
                SolveMode::Free,
                Vec3::ZERO,
                Vec3::ZERO,
                None,
                0.0,
            )
            .unwrap()
            .unwrap();
        assert!(len(p.p - Vec3::new(0.0, 0.0, -1.0)) < 1e-6, "{:?}", p.p);
        let mut dirs = Vec::new();
        assert_eq!(tr.probe(&k, &p, &mut dirs).unwrap(), 0);
    }

    /// The parabola tangents of three points, the polyline distance and the near test.
    #[test]
    fn tangent_tangents_of_three_points() {
        let pt = |x: f64, y: f64| TcPt {
            x: [x, y, x, y],
            p: Vec3::new(x, y, 0.0),
            n: Vec3::new(0.0, 0.0, 1.0),
            n2: Vec3::new(0.0, 0.0, 1.0),
            gap: 0.0,
            ang: 0.0,
        };
        let c = [pt(0.0, 0.0), pt(1.0, 1.0), pt(2.0, 0.0)];
        let (tan, fd) = tc_tangents(&c, false).unwrap();
        assert!(len(tan[1] - Vec3::new(1.0, 0.0, 0.0)) < 1e-12);
        assert_eq!(fd.len(), 12);
        let (d, seg, _) = tc_poly_dist(Vec3::new(1.5, 0.5, 0.0), &c);
        assert!(d < 1e-12 && seg == 1);
        assert!(tc_near_poly(Vec3::new(0.5, 0.5, 0.0), &c, 1e-9));
    }
}
