//! The tangential intersection curve pass of the surface/surface intersector: traces the
//! curves along which the two surfaces touch, and the crossing branches through the
//! contact points where crossing curves meet (opt-in, WR-75).
//!
//! Upstream: the source fork's `source/SMLib/src/SmTangentCurveTracer.cpp`
//! (source fork ef4ab25, e9fde8f, d8c61db, 1693c9d; not in the pinned SMLib).
//! The solver core (`TcTracer` and its helpers) is
//! [`crate::intersect::surface_intersector::tangent_tracer`].
//!
//! | Rust                    | Upstream (fork)                               |
//! |-------------------------|-----------------------------------------------|
//! | [`find_tangent_curves`] | `SmSurfaceIntersector::FindTangentCurves`     |
//! | [`add_tangent_nodes`]   | `SmSurfaceIntersector::AddTangentNodes`       |
//!
//! Deviations: `m_vTangentSeeds` (filled by `FindInteriorCurves`) and `m_vTangentNodes`
//! (filled by `FindTangentCurves`, read by `AddTangentNodes`) are passed in and returned
//! instead of being intersector members, and the model size `FindTangentCurves` reads
//! from the two decomposition trees' top boxes is the caller's (the trees live with the
//! surface caches). A curve's UV curves are never `NULL` here. `SmTList::GetAllNodes`
//! of the through points is their list order. Loops are bounded as the fork's are (50
//! nodes, 12 refinement passes, 6000 points per curve); no recursion.
//! `IsDegenerate` is a plain bool in the fork: an evaluation error there counts as not
//! degenerate (`Unported` still stops the pass).

use crate::curve::curve::Curve;
use crate::error::{ignored, Result};
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::vector2d::Vec2;
use crate::intersect::surface_intersector::p1::{
    IntersectionPointType, SrfIndex, SurfaceIntersector, TsectPnt,
};
use crate::intersect::surface_intersector::p2::TsectCurves;
use crate::intersect::surface_intersector::tangent_tracer::{
    tc_near_poly, tc_poly_dist, tc_tangents, tc_uv_der, SolveMode, TangentTracer, TcPt,
};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec3, EFF_ZERO, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::surface::surface::p2::TsectCurveType;

/// `SM_DEG2RAD`.
fn deg2rad(d: f64) -> f64 {
    d * (PI / 180.0)
}

/// `TcV::unit()`: the vector scaled to length one, or as it is when of length zero.
fn unit(v: Vec3) -> Vec3 {
    let l = v.dot(v).sqrt();
    if l > 0.0 {
        v * (1.0 / l)
    } else {
        v
    }
}

/// `TcV::len()`.
fn len(v: Vec3) -> f64 {
    v.dot(v).sqrt()
}

/// `smos_Max` / `smos_Min`.
fn smax(a: f64, b: f64) -> f64 {
    if a > b {
        a
    } else {
        b
    }
}

/// `x[i]` of a parameter quadruple (`i < 4` at every call).
fn at4(x: &[f64; 4], i: usize) -> f64 {
    x.get(i).copied().unwrap_or(0.0)
}

fn smin(a: f64, b: f64) -> f64 {
    if a < b {
        a
    } else {
        b
    }
}

/// `pC->DropPoint(ivl, X, NULL, tol, NULL, bOk, dParam, dDist, SM_SO_MINIMIZE)`: the
/// distance of a found drop, `None` when the drop fails or finds nothing
/// ([`crate::SolidError::Unported`] propagates).
fn drop_distance(k: &Kernel, c: &Curve, x: Vec3, tol: f64) -> Result<Option<f64>> {
    match c.drop_point(
        k,
        &c.natural_interval(),
        x,
        None,
        tol,
        None,
        SolverOperation::Minimize,
    ) {
        Ok(d) if d.success => Ok(Some(d.distance)),
        Err(e) if e.is_unported() => Err(e),
        Ok(_) | Err(_) => Ok(None),
    }
}

/// `pC->EvaluatePoint(t, P)`: `None` on a failed evaluation
/// ([`crate::SolidError::Unported`] propagates).
fn curve_point(k: &Kernel, c: &Curve, t: f64) -> Result<Option<Point3>> {
    match c.evaluate_point(k, t) {
        Ok(p) => Ok(Some(p)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(None),
    }
}

/// Whether the regular marcher's curve `c` covers `rc` within `cov_tol` at every
/// fortieth point (`pC->DropPoint` of each).
fn covers(k: &Kernel, c: &Curve, rc: &[TcPt], tol: f64, cov_tol: f64) -> Result<bool> {
    let stride = (rc.len() / 40).max(1);
    let mut i = 0;
    while i < rc.len() {
        let Some(p) = rc.get(i) else { break };
        match drop_distance(k, c, p.p, tol * 10.0)? {
            Some(d) if d <= cov_tol => {}
            _ => return Ok(false),
        }
        i += stride;
    }
    Ok(true)
}

/// Removes curve `ii` from the five output arrays (`delete pC; RemoveAt(ii)` and the
/// guarded `RemoveAt`s of the associated arrays).
fn remove_curve(out: &mut TsectCurves, ii: usize) {
    if ii < out.curves_3d.len() {
        out.curves_3d.remove(ii);
    }
    for uv in &mut out.uv_curves {
        if ii < uv.len() {
            uv.remove(ii);
        }
    }
    if ii < out.curve_types.len() {
        out.curve_types.remove(ii);
    }
    if ii < out.deviations.len() {
        out.deviations.remove(ii);
    }
}

/// A traced curve and how it was traced.
struct NewCurve {
    pts: Vec<TcPt>,
    closed: bool,
    /// `n2`'s orientation relative to `n1` along it.
    sign: f64,
    /// A crossing branch between contact points (else a tangential curve).
    cross: bool,
}

/// `FindTangentCurves`: finds and traces the tangential intersection curves and the
/// crossing branches through contact points, and appends them to `out` through
/// [`SurfaceIntersector::flush_curve`], replacing the curves and points the regular
/// marcher produced along them; returns the contact points where crossing branches
/// meet as `(uv1, uv2)` pairs (`m_vTangentNodes`, for [`add_tangent_nodes`]).
///
/// Seeds are the ends of `out`'s curves where the normals are within 1 degree of
/// parallel (and its degenerate curves), the through points and `tangent_seeds` (the
/// node pairs `FindInteriorCurves` skipped for parallel normals). Each seed is refined
/// to a contact point; one from which the contact continues in two opposite directions
/// is traced both ways; a contact point with no tangential curve but two or more
/// crossing directions is a node whose crossing branches are traced through it and the
/// further nodes they meet. `size` is the model size (the smaller of the two trees' top
/// box diagonals).
///
/// # Errors
///
/// Those of `FlushCurve` and of the evaluations upstream checks;
/// [`crate::SolidError::Unported`] from any callee.
#[allow(clippy::too_many_lines)] // one fork routine
pub(crate) fn find_tangent_curves(
    k: &Kernel,
    si: &mut SurfaceIntersector<'_>,
    size: f64,
    tangent_seeds: &[[Point2; 2]],
    out: &mut TsectCurves,
) -> Result<Vec<[Point2; 2]>> {
    let mut tangent_nodes = Vec::new();
    let sized = size > 0.0;
    if !sized {
        return Ok(tangent_nodes);
    }
    let tol = si.this_approx_tol_3d;
    let uv_domains: [Extent2d; 2] = si.uv_domains;
    let mut tr = TangentTracer::new(
        [si.surface(SrfIndex::First), si.surface(SrfIndex::Second)],
        uv_domains,
        tol,
        size,
    );

    // --- collect seeds (pairs: uv1, uv2)
    let mut seeds: Vec<[Point2; 2]> = Vec::new();
    let num_orig_curves = out.curves_3d.len();
    for ii in 0..num_orig_curves {
        let (Some(uv1), Some(uv2)) = (
            out.uv_curves[0].get(ii).cloned(),
            out.uv_curves[1].get(ii).cloned(),
        ) else {
            break;
        };
        let i1 = uv1.natural_interval();
        let i2 = uv2.natural_interval();
        let degen = match out.curves_3d.get(ii) {
            Some(c) => ignored(c.is_degenerate(k, EFF_ZERO, None))?.unwrap_or(false),
            None => break,
        };
        for end in 0..(if degen { 1 } else { 2 }) {
            let t1 = if end == 1 { i1.max() } else { i1.min() };
            let t2 = if end == 1 { i2.max() } else { i2.min() };
            let Some(sa) = curve_point(k, &uv1, t1)? else {
                continue;
            };
            let Some(sb) = curve_point(k, &uv2, t2)? else {
                continue;
            };
            let a = Point2::new(sa.x, sa.y);
            let b = Point2::new(sb.x, sb.y);
            if !degen {
                // only curve ends where the surfaces are (nearly) tangent
                let Some(e1) = tr.eval(k, 0, a.x, a.y)? else {
                    continue;
                };
                let Some(e2) = tr.eval(k, 1, b.x, b.y)? else {
                    continue;
                };
                if e1.n.dot(e2.n).abs() < deg2rad(1.0).cos() {
                    continue;
                }
            }
            seeds.push([a, b]);
        }
    }
    for &id in si.through_points() {
        let p = si.point(id)?;
        seeds.push([p.uv_pos(SrfIndex::First), p.uv_pos(SrfIndex::Second)]);
    }
    seeds.extend_from_slice(tangent_seeds);
    if seeds.is_empty() {
        return Ok(tangent_nodes);
    }

    // --- trace
    let mut new_curves: Vec<NewCurve> = Vec::new();
    let mut nodes: Vec<TcPt> = Vec::new(); // contact points where crossing branches meet
    let mut node_dirs: Vec<Vec<Vec3>> = Vec::new();
    let probe_r = 2e-3 * size;
    let mut contacts: Vec<Vec3> = Vec::new(); // contact points already classified
    let near_tol = smax(100.0 * tol, 1e-4 * size);
    for seed in &seeds {
        let mut x = [seed[0].x, seed[0].y, seed[1].x, seed[1].y];
        let Some(e1) = tr.eval(k, 0, x[0], x[1])? else {
            continue;
        };
        let Some(e2) = tr.eval(k, 1, x[2], x[3])? else {
            continue;
        };
        if len(e1.p - e2.p) > 0.01 * size {
            continue;
        }
        if e1.n.dot(e2.n).abs() < deg2rad(10.0).cos() {
            continue;
        }
        tr.sign = if e1.n.dot(e2.n) < 0.0 { -1.0 } else { 1.0 };

        // skip seeds on tangential curves found already
        let mid = (e1.p + e2.p) * 0.5;
        if new_curves
            .iter()
            .any(|c| tc_near_poly(mid, &c.pts, near_tol))
        {
            continue;
        }

        let Some(p) = tr.solve(
            k,
            &mut x,
            SolveMode::Free,
            Vec3::ZERO,
            Vec3::ZERO,
            None,
            0.0,
        )?
        else {
            continue;
        };
        if new_curves
            .iter()
            .any(|c| tc_near_poly(p.p, &c.pts, near_tol))
            || contacts.iter().any(|&c| len(c - p.p) < near_tol)
        {
            continue;
        }
        contacts.push(p.p);

        let mut dirs = Vec::new();
        let num_dirs = tr.probe(k, &p, &mut dirs)?;
        // the surfaces coincide around p: that region is not a tangential curve, so this
        // seed gives none, but seeds elsewhere on a partly coincident pair still can
        if num_dirs < 0 {
            continue;
        }
        if num_dirs == 0 {
            // no tangential curve here: a touching point, or a point where crossing
            // branches meet (a saddle-like contact)
            tr.tangent = false;
            let mut cross_dirs = Vec::new();
            let num_cross = tr.probe(k, &p, &mut cross_dirs)?;
            tr.tangent = true;
            let node_known = nodes.iter().any(|n| len(n.p - p.p) < near_tol);
            if num_cross >= 2 && !node_known {
                nodes.push(p);
                node_dirs.push(cross_dirs);
            }
            continue;
        }

        let (Some(&d0), Some(&d1)) = (dirs.first(), dirs.get(1).or(dirs.first())) else {
            continue;
        };
        if num_dirs <= 2 {
            // one curve through p (or ending at p on a boundary)
            let mut fwd = Vec::new();
            let mut bwd = Vec::new();
            let closed = tr.march(k, &p, d0, &p, &mut fwd, None)?;
            if !closed {
                let t1 = if num_dirs == 2 { d1 } else { d0 * -1.0 };
                tr.march(k, &p, t1, &p, &mut bwd, None)?;
            }
            let mut all: Vec<TcPt> = bwd.into_iter().rev().collect();
            all.push(p);
            all.extend(fwd);
            if all.len() < 3 {
                continue;
            }
            new_curves.push(NewCurve {
                pts: all,
                closed,
                sign: tr.sign,
                cross: false,
            });
        } else {
            // a node of several tangential branches: trace each branch from it
            for &dir in &dirs {
                let mut fwd = Vec::new();
                tr.march(k, &p, dir, &p, &mut fwd, None)?;
                if fwd.len() < 2 {
                    continue;
                }
                let mut all = vec![p];
                all.extend(fwd);
                new_curves.push(NewCurve {
                    pts: all,
                    closed: false,
                    sign: tr.sign,
                    cross: false,
                });
            }
        }
    }

    // --- crossing branches through contact points (nodes): trace every branch through
    //     a node, on through the other nodes it meets (found on the way and added), to
    //     the domain boundaries or back to itself
    tr.tangent = false;
    let mut in_ = 0;
    while in_ < nodes.len() && in_ < 50 {
        let node = *nodes
            .get(in_)
            .ok_or_else(|| crate::SolidError::invalid_input(OP, "node index"))?;
        // the node, and the node in other parameter charts: a surface corner can map to
        // the same point as another corner (folded/collapsed edges)
        let mut charts = vec![node];
        {
            let mut alt: [Vec<Point2>; 2] = [Vec::new(), Vec::new()];
            for (s, (alt_s, &dom)) in alt.iter_mut().zip(uv_domains.iter()).enumerate() {
                let own = Point2::new(at4(&node.x, 2 * s), at4(&node.x, 2 * s + 1));
                alt_s.push(own);
                let corners = [
                    dom.min(),
                    dom.max(),
                    Point2::new(dom.min().x, dom.max().y),
                    Point2::new(dom.max().x, dom.min().y),
                ];
                for c in corners {
                    if c.distance(own) < 1e-9 * (1.0 + dom.max_dimension()) {
                        continue;
                    }
                    if let Some(ec) = tr.eval(k, s, c.x, c.y)? {
                        if len(ec.p - node.p) < 10.0 * tol {
                            alt_s.push(c);
                        }
                    }
                }
            }
            for (i0, a0) in alt[0].iter().enumerate() {
                for (i1, a1) in alt[1].iter().enumerate() {
                    if i0 == 0 && i1 == 0 {
                        continue;
                    }
                    let mut c = node;
                    c.x = [a0.x, a0.y, a1.x, a1.y];
                    let Some(e1) = tr.eval(k, 0, c.x[0], c.x[1])? else {
                        continue;
                    };
                    let Some(e2) = tr.eval(k, 1, c.x[2], c.x[3])? else {
                        continue;
                    };
                    c.p = (e1.p + e2.p) * 0.5;
                    c.n = e1.n;
                    c.n2 = e2.n;
                    c.gap = len(e1.p - e2.p);
                    c.ang = smin(1.0, len(e1.n.cross(e2.n))).asin();
                    if c.gap < 10.0 * tol {
                        charts.push(c);
                    }
                }
            }
        }
        if in_ >= node_dirs.len() {
            let mut dirs = Vec::new();
            if tr.probe(k, &node, &mut dirs)? < 0 {
                dirs.clear();
            }
            node_dirs.push(dirs);
        }
        for (ic, &n) in charts.iter().enumerate() {
            // the node's own chart has its directions already (node_dirs grows only above)
            let dirs: Vec<Vec3> = if ic == 0 {
                node_dirs.get(in_).cloned().unwrap_or_default()
            } else {
                let mut chart_dirs = Vec::new();
                if tr.probe(k, &n, &mut chart_dirs)? < 0 {
                    chart_dirs.clear();
                }
                chart_dirs
            };
            for &dir in &dirs {
                let pr = n.p + dir * probe_r;
                if new_curves
                    .iter()
                    .any(|c| tc_poly_dist(pr, &c.pts).0 < 0.05 * probe_r)
                {
                    continue;
                }
                let mut fwd = Vec::new();
                let mut bwd = Vec::new();
                let closed = tr.march(k, &n, dir, &n, &mut fwd, Some(&mut nodes))?;
                if !closed {
                    // the smooth continuation backwards through N
                    let mut best = deg2rad(45.0).cos();
                    let mut back: Option<usize> = None;
                    for (j, d) in dirs.iter().enumerate() {
                        let c = -d.dot(dir);
                        if c > best {
                            best = c;
                            back = Some(j);
                        }
                    }
                    if let Some(bd) = back.and_then(|j| dirs.get(j).copied()) {
                        tr.march(k, &n, bd, &n, &mut bwd, Some(&mut nodes))?;
                    }
                }
                if fwd.is_empty() && bwd.is_empty() {
                    continue;
                }
                let mut all: Vec<TcPt> = bwd.into_iter().rev().collect();
                all.push(n);
                all.extend(fwd);
                new_curves.push(NewCurve {
                    pts: all,
                    closed,
                    sign: 1.0,
                    cross: true,
                });
            }
        }
        in_ += 1;
    }
    for n in &nodes {
        tangent_nodes.push([Point2::new(n.x[0], n.x[1]), Point2::new(n.x[2], n.x[3])]);
    }

    // --- refine spans until the cubic Hermite spans FlushCurve() builds (3d and uv)
    //     are within tolerance; keep a dense sampling of each curve for the comparisons
    //     with the curves the regular marcher produced
    let mut new_tan: Vec<Vec<Vec3>> = Vec::with_capacity(new_curves.len());
    let mut new_uv_der: Vec<Vec<f64>> = Vec::with_capacity(new_curves.len());
    let mut dense: Vec<Vec<TcPt>> = Vec::with_capacity(new_curves.len());
    for nc in &mut new_curves {
        tr.tangent = !nc.cross;
        tr.sign = nc.sign;
        let rc = &mut nc.pts;
        for _pass in 0..12 {
            let (tan, fd) = tc_tangents(rc, nc.closed)?;
            let mut refined: Vec<TcPt> = Vec::new();
            let mut inserted = false;
            for i in 0..rc.len().saturating_sub(1) {
                let (Some(&a), Some(&b)) = (rc.get(i), rc.get(i + 1)) else {
                    break;
                };
                refined.push(a);
                if rc.len() + refined.len() > 6000 {
                    continue;
                }
                let h = len(b.p - a.p);
                if h < 10.0 * tol {
                    continue;
                }
                let (Some(&ta), Some(&tb)) = (tan.get(i), tan.get(i + 1)) else {
                    break;
                };
                let hm = (a.p + b.p) * 0.5 + (ta - tb) * (h / 8.0);
                let t = unit(b.p - a.p);
                let mut xg = [0.0; 4];
                for ((g, &xa), &xb) in xg.iter_mut().zip(&a.x).zip(&b.x) {
                    *g = 0.5 * (xa + xb);
                }
                let Some(q) = tr.solve(k, &mut xg, SolveMode::Plane, hm, t, None, 0.0)? else {
                    continue;
                };
                if len(q.p - hm) > 0.25 * h {
                    continue;
                }
                let mut err = len(q.p - hm);
                {
                    // the uv Hermite spans too
                    let d0 = tc_uv_der(&tr, k, rc, &tan, &fd, i)?;
                    let d1 = tc_uv_der(&tr, k, rc, &tan, &fd, i + 1)?;
                    for s in 0..2 {
                        let (iu, iv) = (2 * s, 2 * s + 1);
                        let um = 0.5 * (at4(&a.x, iu) + at4(&b.x, iu))
                            + (at4(&d0, iu) - at4(&d1, iu)) * (h / 8.0);
                        let vm = 0.5 * (at4(&a.x, iv) + at4(&b.x, iv))
                            + (at4(&d0, iv) - at4(&d1, iv)) * (h / 8.0);
                        if let Some(em) = tr.eval(k, s, um, vm)? {
                            err = smax(err, len(em.p - q.p));
                        }
                    }
                }
                if err <= 0.5 * tol {
                    continue;
                }
                refined.push(q);
                inserted = true;
            }
            if let Some(&last) = rc.last() {
                refined.push(last);
            }
            *rc = refined;
            if !inserted {
                break;
            }
        }
        let (tan, uv_der) = tc_tangents(rc, nc.closed)?;

        // dense sampling of the Hermite spans
        let mut d = Vec::new();
        for i in 0..rc.len() {
            let Some(&a) = rc.get(i) else { break };
            d.push(a);
            let Some(&b) = rc.get(i + 1) else { break };
            let (Some(&ta), Some(&tb)) = (tan.get(i), tan.get(i + 1)) else {
                break;
            };
            let h = len(b.p - a.p);
            for j in 1..8 {
                let t = f64::from(j) / 8.0;
                let t2 = t * t;
                let t3 = t2 * t;
                let mut q = a;
                q.p = a.p * (2.0 * t3 - 3.0 * t2 + 1.0)
                    + ta * (h * (t3 - 2.0 * t2 + t))
                    + b.p * (-2.0 * t3 + 3.0 * t2)
                    + tb * (h * (t3 - t2));
                q.ang = (1.0 - t) * a.ang + t * b.ang;
                d.push(q);
            }
        }
        new_tan.push(tan);
        new_uv_der.push(uv_der);
        dense.push(d);
    }

    // a crossing branch an existing curve already covers is dropped (the regular
    // marcher's curve is kept); existing curves lying along the remaining crossing
    // branches are replaced by them
    let mut keep = vec![true; new_curves.len()];
    for (nc, kp) in new_curves.iter().zip(keep.iter_mut()) {
        if !nc.cross {
            continue;
        }
        for c in &out.curves_3d {
            if !*kp {
                break;
            }
            if ignored(c.is_degenerate(k, EFF_ZERO, None))?.unwrap_or(false) {
                continue;
            }
            if covers(k, c, &nc.pts, tol, near_tol)? {
                *kp = false;
            }
        }
    }
    if !nodes.is_empty() {
        let mut ii = 0;
        while ii < out.curves_3d.len() {
            let Some(c) = out.curves_3d.get(ii) else {
                break;
            };
            let ivl = c.natural_interval();
            let mut on = true;
            for j in 0..=32 {
                let Some(sp) = curve_point(k, c, ivl.evaluate(f64::from(j) / 32.0))? else {
                    on = false;
                    break;
                };
                let mut near = false;
                for (kk, rb) in dense.iter().enumerate() {
                    let (Some(nc), Some(&kp)) = (new_curves.get(kk), keep.get(kk)) else {
                        break;
                    };
                    if !nc.cross || !kp {
                        continue;
                    }
                    let (dist, seg, _) = tc_poly_dist(sp, rb);
                    // where the surfaces cross at a small angle a curve traced to
                    // tolerance can stray across by tolerance / sin(angle)
                    let a0 = rb.get(seg).map_or(0.0, |p| p.ang);
                    let a1 = rb
                        .get(seg.saturating_add(1).min(rb.len().saturating_sub(1)))
                        .map_or(0.0, |p| p.ang);
                    let sin = smin(a0, a1).sin();
                    let band = smax(near_tol, 20.0 * tol / smax(sin, 1e-12));
                    if dist < smin(band, 0.02 * size) {
                        near = true;
                        break;
                    }
                }
                if !near {
                    on = false;
                    break;
                }
            }
            if on {
                remove_curve(out, ii);
                continue;
            }
            ii += 1;
        }
    }

    // --- reconcile with what the regular marcher produced and flush
    for (kk, nc) in new_curves.iter().enumerate() {
        if !keep.get(kk).copied().unwrap_or(false) {
            continue;
        }
        let (Some(tan), Some(uv_der), Some(dn)) =
            (new_tan.get(kk), new_uv_der.get(kk), dense.get(kk))
        else {
            break;
        };
        tr.tangent = !nc.cross;
        tr.sign = nc.sign;
        let rc = &nc.pts;
        let length: f64 = rc
            .windows(2)
            .map(|w| match w {
                [a, b] => len(b.p - a.p),
                _ => 0.0,
            })
            .sum();
        if length < 10.0 * tol {
            continue;
        }

        // existing curve covering this one: keep the existing one
        let mut covered = false;
        for (ii, c) in out.curves_3d.iter().enumerate() {
            if ignored(c.is_degenerate(k, EFF_ZERO, None))?.unwrap_or(false) {
                continue;
            }
            // a tangential curve traced by the regular marcher can lie across anywhere
            // in the band where the surfaces are within tolerance of each other, which
            // is wide for high order contact
            let cov_tol = if !nc.cross && out.curve_types.get(ii) == Some(&TsectCurveType::Tangent)
            {
                0.02 * size
            } else {
                near_tol
            };
            if covers(k, c, rc, tol, cov_tol)? {
                covered = true;
                break;
            }
        }
        if covered {
            continue;
        }

        // existing curves (and points) lying along this tangential curve: remove them
        if !nc.cross {
            let mut ii = 0;
            while ii < out.curves_3d.len() {
                let Some(c) = out.curves_3d.get(ii) else {
                    break;
                };
                let ivl = c.natural_interval();
                let mut on = true;
                for j in 0..=32 {
                    let Some(sp) = curve_point(k, c, ivl.evaluate(f64::from(j) / 32.0))? else {
                        on = false;
                        break;
                    };
                    if tc_poly_dist(sp, dn).0 > 0.02 * size {
                        on = false;
                        break;
                    }
                }
                if on {
                    remove_curve(out, ii);
                    continue;
                }
                ii += 1;
            }
        }

        let mut param = 0.0;
        for i in 0..rc.len() {
            let Some(&p) = rc.get(i) else { break };
            if let Some(&prev) = i.checked_sub(1).and_then(|j| rc.get(j)) {
                param += len(p.p - prev.p);
            }
            let t = tan.get(i).copied().unwrap_or(Vec3::ZERO);
            let duv = tc_uv_der(&tr, k, rc, tan, uv_der, i)?;
            let mut tp = TsectPnt::zeroed();
            tp.point_type = if nc.cross {
                IntersectionPointType::Crossing
            } else {
                IntersectionPointType::TangentCurve
            };
            *tp.uv_pos_mut(SrfIndex::First) = Point2::new(p.x[0], p.x[1]);
            *tp.uv_pos_mut(SrfIndex::Second) = Point2::new(p.x[2], p.x[3]);
            *tp.uv_deriv_mut(SrfIndex::First) = Vec2::new(duv[0], duv[1]);
            *tp.uv_deriv_mut(SrfIndex::Second) = Vec2::new(duv[2], duv[3]);
            *tp.crv_pos_mut() = p.p;
            *tp.crv_deriv_mut() = t;
            tp.curve_parameter = param;
            tp.deviation = p.gap;
            tp.tangent_plane_angle_rad = if nc.cross { p.ang } else { 0.0 };
            let id = si.alloc_point(tp);
            si.add_point_to_curve(id)?;
        }
        si.flush_curve(k, out)?;
    }

    Ok(tangent_nodes)
}

/// `AddTangentNodes`: appends each contact point [`find_tangent_curves`] found where
/// crossing branches meet as a touching point (degenerate 3D and UV curves at it, type
/// `SM_TC_TOUCHING`, deviation the distance between the two surface points).
///
/// # Errors
///
/// Those of the surface evaluations and of `CreateDegenerateCurve` (`SER`).
#[allow(clippy::too_many_arguments)] // the fork's five output arrays
pub(crate) fn add_tangent_nodes(
    k: &Kernel,
    si: &SurfaceIntersector<'_>,
    tangent_nodes: &[[Point2; 2]],
    curves_3d: &mut Vec<Curve>,
    surface1_uv_curves: &mut Vec<Curve>,
    surface2_uv_curves: &mut Vec<Curve>,
    curve_types: &mut Vec<TsectCurveType>,
    deviations: &mut Vec<f64>,
) -> Result<()> {
    for &[uv1, uv2] in tangent_nodes {
        let p1 = si.surface(SrfIndex::First).evaluate_point(k, uv1)?;
        let p2 = si.surface(SrfIndex::Second).evaluate_point(k, uv2)?;
        let p = Point3::new(
            (p1.x + p2.x) / 2.0,
            (p1.y + p2.y) / 2.0,
            (p1.z + p2.z) / 2.0,
        );
        curves_3d.push(Curve::from(BSplineCurve::create_degenerate_curve(3, p)?));
        surface1_uv_curves.push(Curve::from(BSplineCurve::create_degenerate_curve(
            2,
            Point3::new(uv1.x, uv1.y, 0.0),
        )?));
        surface2_uv_curves.push(Curve::from(BSplineCurve::create_degenerate_curve(
            2,
            Point3::new(uv2.x, uv2.y, 0.0),
        )?));
        deviations.push(p1.distance(p2));
        curve_types.push(TsectCurveType::Touching);
    }
    Ok(())
}

const OP: &str = "intersect::surface_intersector::tangent_curves::find_tangent_curves";
