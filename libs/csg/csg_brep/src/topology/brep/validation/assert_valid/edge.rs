//! The edge and edge-use checks, and the virtual curve check.
//!
//! Upstream: `source/SMLib/src/SmEdge.cpp`, `SmEdgeuse.cpp`, `SmCurve.cpp`,
//! `SmBSplineCurve.cpp`, `SmLine.cpp`, `SmEllipse.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | `edge` | `SmEdge::AssertValid` |
//! | `assert_test_edge14` | `sm_AssertTestEdge14` |
//! | `edgeuse` | `SmEdgeuse::AssertValid` |
//! | `curve` | the virtual `SmCurve::AssertValid` (`m_pCurve->AssertValid`, `m_pUVTrimCurve->AssertValid`) |
//!
//! Deviations: the radial edge-use walks are bounded by the edge's use count (upstream
//! loops forever on a non-loop edge use) and return
//! [`crate::SolidError::InvalidInput`] past it or at a missing mate. Statuses upstream
//! ignores leave uninitialised points there; here a failed binormal or curve
//! evaluation propagates its error. Loop membership of an edge use (test 14) and wire
//! membership (test 13) read sets built once per loop use / shell. `SmHyperbola` and
//! `SmParabola` use the inherited `SmBSplineCurve` check with their own `MakeNurb`,
//! `IsAnalytic` and `IsBounded`. Opt-in (WR-540,
//! `Kernel::fixes.edge_check_orders_tangent_faces`, #383): where two faces round an edge
//! leave it within `SM_ANG_TOL_DEG` of each other, test 9 orders every face by the
//! direction to its point 20 edge tolerances into the face, square to the edge (the
//! fork's source fork@5533bbb).

use std::collections::{BTreeMap, BTreeSet};

use super::{base, class, geometry_key, seams, Cx};
use crate::curve::curve::{Curve, CurveKind};
use crate::error::{EntityRef, Result};
use crate::geom::assert_array::{AssertArray, ObjectKey};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::tol::{is_zero, ScaledZero, ANG_TOL_DEG, ZONE_TOL_3D};
use crate::kernel::Kernel;
use crate::math::{rad_to_deg, Point3, EFF_ZERO, PI, UNDEF_DOUBLE};
use crate::nurbs::b_spline_curve::p3::DerivedOverrides;
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::edgeuse::p3::{evaluate_binormal, BinormalOptions};
use crate::topology::{
    edge as tedge, vertexuse as tvu, Brep, EdgeId, EdgeuseId, EdgeuseOwner, LoopuseId, ShellId,
};

/// `m_pCurve->AssertValid(pAList, eTestLevel, SM_NO_WALK)` dispatched on the curve's
/// class: the curve object reports as `key`, an ellipse's polar converter as
/// `polar_key`. `trim_surface` is the face's surface of a UV trim (WR-560, read by the
/// `SmBSplineCurve` checks with `Kernel::fixes.uv_trim_checks_on_surface`).
pub(super) fn curve(
    k: &Kernel,
    c: &mut Curve,
    list: Option<&mut AssertArray>,
    level: u8,
    key: ObjectKey,
    polar_key: ObjectKey,
    trim_surface: Option<&Surface>,
) -> Result<bool> {
    let given = level == super::LEVEL_GIVEN;
    let owner = c.owner();
    let mut list = list;
    match c.kind_mut() {
        CurveKind::BSpline(x) => x.assert_valid_on(k, list, given, key, trim_surface),
        CurveKind::Line(x) => x.assert_valid_on(k, list, given, key, trim_surface),
        CurveKind::Ellipse(x) => x.assert_valid_on(k, list, given, key, polar_key, trim_surface),
        CurveKind::Parabola(x) => {
            // SmBSplineCurve::AssertValid builds a missing NURB (virtual MakeNurb,
            // status ignored) after the SmCurve base check, which does not read it
            if x.base().nurb.is_none() {
                crate::error::ignored(x.make_nurb(owner))?;
            }
            let analytic = x.is_analytic();
            let bounded = x.is_bounded();
            let base = x.base();
            let is_degenerate = |tol: f64| base.is_degenerate(k, tol, None);
            base.assert_valid_with(
                k,
                list.as_deref_mut(),
                given,
                key,
                &DerivedOverrides {
                    degree: None,
                    analytic,
                    bounded,
                    is_degenerate: &is_degenerate,
                    trim_surface,
                },
            )
        }
        CurveKind::Hyperbola(x) => {
            if x.base().nurb.is_none() {
                crate::error::ignored(x.make_nurb(owner))?;
            }
            let analytic = x.is_analytic();
            let bounded = x.is_bounded();
            let base = x.base();
            let is_degenerate = |tol: f64| base.is_degenerate(k, tol, None);
            base.assert_valid_with(
                k,
                list.as_deref_mut(),
                given,
                key,
                &DerivedOverrides {
                    degree: None,
                    analytic,
                    bounded,
                    is_degenerate: &is_degenerate,
                    trim_surface,
                },
            )
        }
        CurveKind::Hermite(_) => seams::hermite_curve_assert_valid(k, c, list, level, key),
        CurveKind::Projected(_) => seams::projected_curve_assert_valid(k, c, list, level, key),
        CurveKind::Offset(_) => seams::offset_curve_assert_valid(k, c, list, level, key),
        CurveKind::CrvOnSurf(_) | CurveKind::TangentField(_) => {
            seams::crv_on_surf_assert_valid(k, c, list, level, key)
        }
        CurveKind::CrvInVolume(_) => seams::crv_in_volume_assert_valid(k, c, list, level, key),
        // no override: SmCurve::AssertValid
        CurveKind::Iso(_) | CurveKind::Conic(_) | CurveKind::Composite(_) => {
            c.assert_valid(list, given, key)
        }
    }
}

/// The membership sets the edge-use checks share across a run.
#[derive(Default)]
pub(super) struct EdgeSets {
    loopuse_edgeuses: BTreeMap<LoopuseId, BTreeSet<EdgeuseId>>,
    wire_edges: BTreeMap<ShellId, BTreeSet<EdgeId>>,
}

/// Runs the edge's curve check through [`curve`] on the curve held by the edge.
fn edge_curve_check(k: &Kernel, b: &mut Brep, cx: &mut Cx<'_>, e: EdgeId) -> Result<bool> {
    let level = cx.rep.level;
    let list = cx.rep.array.as_deref_mut();
    let Some(c) = b.edge_mut(e)?.curve.as_mut() else {
        return Ok(true);
    };
    curve(
        k,
        c,
        list,
        level,
        geometry_key(20001, e.index()),
        geometry_key(20004, e.index()),
        None,
    )
}

/// `SmEdge::AssertValid(SM_NO_WALK)`.
#[allow(clippy::too_many_lines)]
pub(super) fn edge(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>, e: EdgeId) -> Result<bool> {
    const C: i64 = class::EDGE;
    let this = EntityRef::Edge(e);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::owning(b, cx, this)?;
    }
    let need = cx.rep.need(0);

    // 0: Edge has a curve
    let interval = b.edge(e)?.interval;
    let has_curve = b.edge(e)?.curve.is_some();
    rtn &= cx.rep.this_bool(b, C, this, 0, !need || has_curve)?;
    if let Some(c) = b.edge(e)?.curve.as_ref() {
        // 1: Edge and Curve both have the same context
        let natural = c.natural_interval();
        rtn &= cx.rep.this_bool(b, C, this, 1, true)?;
        // 2: Edge's interval is contained within the curve's Natural interval
        let ok = !need || interval.is_contained_by(&natural, EFF_ZERO);
        rtn &= cx.rep.this_bool(b, C, this, 2, ok)?;
    }

    // Check Edge->Curve
    if has_curve {
        rtn &= edge_curve_check(k, b, cx, e)?;
    }

    // Check all connected Edgeuses
    let edgeuses = b.edge(e)?.edgeuses.clone();

    // 3: Edges must have uses
    rtn &= cx
        .rep
        .this_bool(b, C, this, 3, !need || !edgeuses.is_empty())?;
    for &eu in &edgeuses {
        rtn &= edgeuse(k, b, cx, eu)?;
    }

    let edge_tol = tedge::tolerance(b, e)?;
    let curve_length = {
        let c = super::super::required(b.edge(e)?.curve.as_ref())?;
        c.approximate_length(k, &interval, 5, false)?.0
    };

    // 7: Edge Length must be larger than its tolerance
    let ok = !need || curve_length >= edge_tol - EFF_ZERO;
    rtn &= cx
        .rep
        .this_value(b, C, this, 7, ok, edge_tol, || curve_length - edge_tol)?;

    // 13: Edge->Curve must be within SmTol::GetXSectTol3d(Edge, GenSurface) of all
    // Face->Surfaces (level 2)
    let xsect_tol = edge_tol + edge_tol;
    let mut max_gap = 0.0;
    let ok = !cx.rep.need(2) || {
        let (passed, gap) = seams::assert_test_edge13(k, b, e)?;
        max_gap = gap;
        passed
    };
    rtn &= cx
        .rep
        .this_value(b, C, this, 13, ok, xsect_tol, || max_gap)?;

    // 14: Edge->Edgeuse orientations must alternate, SAME-OPPO-SAME-OPPO
    let ok = !need || assert_test_edge14(b, e)?;
    rtn &= cx.rep.this_bool(b, C, this, 14, ok)?;

    // Vertex cardinality and endpoint gaps
    let vertices = tedge::vertices(b, e, None)?;
    let num_verts = vertices.len();

    // 15: closed edges can only have one vert, open edges 2
    rtn &= cx
        .rep
        .this_bool(b, C, this, 15, !need || num_verts == 1 || num_verts == 2)?;
    if (num_verts == 1 || num_verts == 2) && has_curve {
        if let Some(start) = tedge::start_vertex(b, e)? {
            let (vtx_pt, vtx_tol) = (b.vertex(start)?.point, b.vertex(start)?.tolerance);
            let edge_pt = edge_point(k, b, e, interval.min())?;
            let dist = vtx_pt.distance(edge_pt);
            // 16: start edge/vertex gap must be less than edge + vertex tolerance
            rtn &= cx
                .rep
                .this_bool(b, C, this, 16, !need || dist <= edge_tol + vtx_tol)?;
            if let Some(other) = tedge::other_vertex(b, e, Some(start))? {
                let (vtx_pt, vtx_tol) = (b.vertex(other)?.point, b.vertex(other)?.tolerance);
                let edge_pt = edge_point(k, b, e, interval.max())?;
                let dist = vtx_pt.distance(edge_pt);
                // 17: end edge/vertex gap must be less than edge + vertex tolerance
                rtn &= cx
                    .rep
                    .this_bool(b, C, this, 17, !need || dist <= edge_tol + vtx_tol)?;
            }
        }
    }

    // radial ordering of the faceuses round a manifold or spine edge
    if tedge::is_manifold(b, e)? || tedge::is_spine(b, e)? {
        rtn &= radial_order(k, b, cx, e, &vertices)?;
    }

    // all done
    Ok(rtn)
}

/// The edge curve's point at `t` (`m_pCurve->EvaluatePoint`).
fn edge_point(k: &Kernel, b: &Brep, e: EdgeId, t: f64) -> Result<Point3> {
    let c = super::super::required(b.edge(e)?.curve.as_ref())?;
    c.evaluate_point(k, t)
}

/// The direction edge use `eu`'s face leaves the edge in at `t` (with its point,
/// tangent and face normal): its binormal, or with `step_off` (WR-540) the direction to
/// its point `step_off_dist` into the face, square to the edge (kept as the binormal when
/// the step or a unitisation fails).
fn sector_direction(
    k: &Kernel,
    b: &mut Brep,
    eu: EdgeuseId,
    t: f64,
    options: BinormalOptions,
    step_off: bool,
    step_off_dist: f64,
) -> Result<crate::topology::edgeuse::p2::BinormalEval> {
    use crate::topology::edgeuse::p2::{evaluate_binormal_step_off, StepOffOptions};
    let mut eval = evaluate_binormal(k, b, eu, t, true, &options)?;
    if !step_off {
        return Ok(eval);
    }
    let Some(stepped) = crate::error::ignored(evaluate_binormal_step_off(
        k,
        b,
        eu,
        t,
        step_off_dist,
        &StepOffOptions::default(),
    ))?
    else {
        return Ok(eval);
    };
    let Ok(unit_tan) = eval.tangent.unitize() else {
        return Ok(eval);
    };
    let mut away = stepped.point - eval.point;
    away = away - away.dot(unit_tan) * unit_tan;
    if let Ok(away) = away.unitize() {
        eval.binormal = away;
    }
    Ok(eval)
}

/// The radial edge-use walk of `SmEdge::AssertValid` (tests 8-11).
fn radial_order(
    k: &mut Kernel,
    b: &mut Brep,
    cx: &mut Cx<'_>,
    e: EdgeId,
    vertices: &[Option<crate::topology::VertexId>],
) -> Result<bool> {
    const C: i64 = class::EDGE;
    let this = EntityRef::Edge(e);
    let mut rtn = true;
    let need = cx.rep.need(0);

    // Check consistency of contained Edgeuses
    let mut target = super::super::required(eu1::primary_edgeuse(b, e)?)?;
    let mut radial = super::super::required(eu1::radial(b, target)?)?;
    let mut angles: Vec<f64> = Vec::new();
    let extent = b.edge(e)?.interval;
    let mut primary: Option<EdgeuseId> = None;
    let mut primary_bivec = Point3::UNDEF;
    let mut primary_tangent = Point3::UNDEF;
    let circle = Extent1d::from_min_max(0.0, 360.0);
    let bound = b.edge(e)?.edgeuses.len() + 1;
    let options = BinormalOptions::default();

    // Must watch out for zero sectors: coincident faces.
    let mut have_nonzero_angle = false;

    // Not upstream (WR-540), opt-in `Kernel::fixes.edge_check_orders_tangent_faces` (the
    // fork's source fork@5533bbb, #383): a sector between faces tangent along the
    // edge (a cylinder touching a plane it rests on, left by a boolean as a non-manifold
    // edge) has binormals that agree to first order, so the binormal angles tie (or differ
    // by rounding, either way round) and cannot show the order. Where any sector is that
    // narrow, every face is measured by the direction from the edge to its point a short
    // step into the face (20 edge tolerances, as IsConvexRadialSector steps).
    let seq_param = extent.evaluate(0.456);
    let mut step_off = false;
    let mut step_off_dist = 0.0;
    if k.fixes.edge_check_orders_tangent_faces {
        step_off_dist = 20.0 * tedge::tolerance(b, e)?;
        let tie_dot = (ANG_TOL_DEG * PI / 180.0).cos();
        let num_eus = b.edge(e)?.edgeuses.len();
        let mut eu = target;
        let mut sectors = 0usize;
        while let Some(rad) = eu1::radial(b, eu)? {
            if !eu1::is_loop_edgeuse(b, eu)? || !eu1::is_loop_edgeuse(b, rad)? {
                break;
            }
            let b0 = crate::error::ignored(evaluate_binormal(k, b, eu, seq_param, true, &options))?;
            let b1 =
                crate::error::ignored(evaluate_binormal(k, b, rad, seq_param, true, &options))?;
            if let (Some(b0), Some(b1)) = (b0, b1) {
                if let (Ok(u0), Ok(u1)) = (b0.binormal.unitize(), b1.binormal.unitize()) {
                    if u0.dot(u1) > tie_dot {
                        step_off = true;
                    }
                }
            }
            sectors += 1;
            match eu1::mate(b, rad)? {
                Some(next) if !step_off && next != target && sectors < num_eus => eu = next,
                _ => break,
            }
        }
    }

    let mut steps = 0;
    while Some(target) != primary {
        steps += 1;
        if steps > bound {
            return Err(super::super::invalid("radial edge-use walk does not close"));
        }
        // 8: Edgeuses of Manifold and Spine edges must be of Type SmLoopuse_TYPE
        let target_loop = eu1::is_loop_edgeuse(b, target)?;
        let radial_loop = eu1::is_loop_edgeuse(b, radial)?;
        rtn &= cx.rep.this_bool(b, C, this, 8, !need || target_loop)?;
        rtn &= cx.rep.this_bool(b, C, this, 8, !need || radial_loop)?;
        if !(target_loop && radial_loop) {
            // upstream does not advance and loops forever
            return Err(super::super::invalid(
                "manifold edge with a non-loop edge use",
            ));
        }
        let t = extent.evaluate(0.456);
        let target_eval = sector_direction(k, b, target, t, options, step_off, step_off_dist)?;
        let radial_eval = sector_direction(k, b, radial, t, options, step_off, step_off_dist)?;

        // 1st iteration save variables
        if primary.is_none() {
            primary = Some(target);
            primary_bivec = target_eval.binormal;
            primary_tangent = target_eval.tangent;
        }

        // Primary BiVec to current Radial BiVec Angle (status ignored)
        let angle_rad = primary_tangent
            .ccw_angle_between(primary_bivec, radial_eval.binormal)
            .unwrap_or(0.0);
        let mut angle_deg = rad_to_deg(angle_rad);
        if angle_deg < -EFF_ZERO {
            angle_deg += 360.0;
        }
        if have_nonzero_angle && angle_deg <= 0.0 + EFF_ZERO {
            angle_deg += 360.0;
        }
        if !have_nonzero_angle && angle_deg > 360.0 + EFF_ZERO {
            angle_deg -= 360.0;
            have_nonzero_angle = angle_deg.abs() > EFF_ZERO;
        }

        // After the first sector
        if let Some(&last) = angles.last() {
            if is_zero(angle_deg) && primary == eu1::mate(b, radial)? {
                angle_deg = 360.0;
            }
            // 9: increasing angles between ordered RadialEUs and PrimaryEdgeuse
            let ok = !need || angle_deg > last - EFF_ZERO;
            rtn &= cx
                .rep
                .this_value(b, C, this, 9, ok, EFF_ZERO, || angle_deg - last)?;
            // 10: angle between every RadialEU and the PrimaryEdgeuse is within [0 360]
            let ok = !need || circle.contains_value(angle_deg, EFF_ZERO);
            rtn &= cx
                .rep
                .this_value(b, C, this, 10, ok, EFF_ZERO, || UNDEF_DOUBLE)?;
            // 11: all Edge->Edgeuse->Vertexuse->Vertices map to the same two vertices
            for eu in [target, radial] {
                let v = match eu1::vertexuse(b, eu)? {
                    Some(vu) => tvu::vertex(b, vu)?,
                    None => None,
                };
                let ok = !need || vertices.contains(&v);
                rtn &= cx.rep.this_bool(b, C, this, 11, ok)?;
            }
        }
        angles.push(angle_deg);

        target = super::super::required(eu1::mate(b, radial)?)?;
        radial = super::super::required(eu1::radial(b, target)?)?;
    }
    Ok(rtn)
}

/// `sm_AssertTestEdge14`: the edge-use orientations alternate round the edge.
fn assert_test_edge14(b: &Brep, e: EdgeId) -> Result<bool> {
    let mut rtn = true;
    let Some(first) = eu1::primary_edgeuse(b, e)? else {
        return Ok(true);
    };
    let bound = b.edge(e)?.edgeuses.len() + 1;
    let mut this = Some(first);
    let mut mate = eu1::mate(b, first)?;
    let mut mate_radial = match mate {
        Some(m) => eu1::radial(b, m)?,
        None => None,
    };
    let mut steps = 0;
    while let (Some(t), Some(m), Some(r)) = (this, mate, mate_radial) {
        steps += 1;
        if steps > bound {
            return Err(super::super::invalid("radial edge-use walk does not close"));
        }
        rtn &= eu1::orientation(b, t)? != eu1::orientation(b, m)?;
        rtn &= eu1::orientation(b, m)? != eu1::orientation(b, r)?;

        // Get next set of EUs to compare
        this = Some(r);
        mate = eu1::mate(b, r)?;
        mate_radial = match mate {
            Some(m) => eu1::radial(b, m)?,
            None => None,
        };
        if this == Some(first) || !rtn {
            break;
        }
    }
    Ok(rtn)
}

/// Whether `eu` is in its loop use's edge-use walk (cached per loop use).
fn in_loopuse(b: &Brep, cx: &mut Cx<'_>, lu: LoopuseId, eu: EdgeuseId) -> Result<bool> {
    if let std::collections::btree_map::Entry::Vacant(slot) =
        cx.edge_sets.loopuse_edgeuses.entry(lu)
    {
        let set: BTreeSet<EdgeuseId> = eu1::loopuse_edgeuses(b, lu)?.into_iter().collect();
        slot.insert(set);
    }
    Ok(cx
        .edge_sets
        .loopuse_edgeuses
        .get(&lu)
        .is_some_and(|s| s.contains(&eu)))
}

/// Whether `e` is a wire edge of shell `s` (cached per shell).
fn in_wire_shell(
    k: &mut Kernel,
    b: &Brep,
    cx: &mut Cx<'_>,
    s: ShellId,
    e: Option<EdgeId>,
) -> Result<bool> {
    if let std::collections::btree_map::Entry::Vacant(slot) = cx.edge_sets.wire_edges.entry(s) {
        let set: BTreeSet<EdgeId> = crate::topology::shell::wire_edges(k, b, s)?
            .into_iter()
            .collect();
        slot.insert(set);
    }
    Ok(e.is_some_and(|e| {
        cx.edge_sets
            .wire_edges
            .get(&s)
            .is_some_and(|w| w.contains(&e))
    }))
}

/// The UV trim checks of `SmEdgeuse::AssertValid` (tests 11, 6, 7, 25, 26) for an edge
/// use holding a UV trim curve.
fn uv_trim_checks(k: &mut Kernel, b: &mut Brep, cx: &mut Cx<'_>, eu: EdgeuseId) -> Result<bool> {
    const C: i64 = class::EDGEUSE;
    let this = EntityRef::Edgeuse(eu);
    let mut rtn = true;
    let need = cx.rep.need(0);

    // get connected surface and its closed/open state in U and V directions
    let face = eu1::face(b, eu)?;
    let e = b.edgeuse(eu)?.edge;
    let trim = super::super::required(b.edgeuse(eu)?.uv_trim_curve.as_ref())?;
    let uv_ivl = trim.natural_interval();
    let edge_ivl = match e {
        Some(e) => b.edge(e)?.interval,
        None => uv_ivl,
    };
    let ec_ivl = match e.and_then(|e| b.edges().get(e).and_then(|x| x.curve.as_ref())) {
        Some(c) => c.natural_interval(),
        None => uv_ivl,
    };
    let surface = match face {
        Some(f) => b.face(f)?.surface.as_ref(),
        None => None,
    };
    let mut domain = Extent2d::new();
    let (mut closed_u, mut closed_v) = (false, false);
    if let Some(s) = surface {
        domain = s.natural_uv_domain_with_nurb(k)?;
        closed_u = s.is_closed(k, &domain, SurfParamType::U, None, false)?.0;
        closed_v = s.is_closed(k, &domain, SurfParamType::V, None, false)?.0;
    }
    let tol_3d = match (e, face) {
        (Some(e), _) => b.edge(e)?.tolerance,
        (None, Some(f)) => b.face(f)?.tolerance,
        (None, None) => match eu1::brep(b, eu)? {
            Some(x) => x.tolerance(),
            None => ZONE_TOL_3D / 10.0,
        },
    };

    // UVTrimCurve->Owner pointer must equal this Edgeuse pointer
    let check = trim.owner() == Some((cx.keys)(this));
    if check {
        let s = super::super::required(surface)?;
        let on_surface = crate::curve::crv_on_surf::CrvOnSurf::new(
            k,
            trim.clone_for_evaluation()?,
            s.clone_for_evaluation()?,
            None,
            0,
        )?;
        let length =
            crate::curve::curve::p2::approximate_length(k, &on_surface, &edge_ivl, 5, false)?.0;
        // 11: UVTrimCurve cannot have zero length
        let ok = !need || length >= tol_3d;
        rtn &= cx.rep.this_value(b, C, this, 11, ok, tol_3d, || length)?;
    }

    // 6
    rtn &= cx.rep.this_bool(b, C, this, 6, !need || check)?;

    // 7: see if the UVTrimCurve jumps across a periodic boundary inappropriately
    let ok = !need || {
        let trim = super::super::required(b.edgeuse(eu)?.uv_trim_curve.as_ref())?;
        seams::assert_no_periodic_jump(k, trim, closed_u, closed_v, &domain)?
    };
    rtn &= cx.rep.this_bool(b, C, this, 7, ok)?;

    // Check UVTrimCurve parameterization. Should be equal to Edge interval and
    // contained by Curve interval
    let zero_min = 100.0 * ec_ivl.min().scaled_zero();
    let zero_max = 100.0 * ec_ivl.max().scaled_zero();
    let equality = !((uv_ivl.min() - edge_ivl.min()).abs() > zero_min
        || (uv_ivl.max() - edge_ivl.max()).abs() > zero_max);
    let containment =
        !(uv_ivl.min() < ec_ivl.min() - zero_min || uv_ivl.max() > ec_ivl.max() + zero_max);
    rtn &= cx.rep.this_bool(b, C, this, 25, !need || equality)?;
    rtn &= cx.rep.this_bool(b, C, this, 26, !need || containment)?;
    Ok(rtn)
}

/// `SmEdgeuse::AssertValid(SM_NO_WALK)`.
#[allow(clippy::too_many_lines)]
pub(super) fn edgeuse(
    k: &mut Kernel,
    b: &mut Brep,
    cx: &mut Cx<'_>,
    eu: EdgeuseId,
) -> Result<bool> {
    const C: i64 = class::EDGEUSE;
    let this = EntityRef::Edgeuse(eu);
    let mut rtn = true;

    // call the base class AssertValid
    if cx.bases() {
        rtn &= base::topology(b, cx, this)?;
    }
    let need = cx.rep.need(0);

    // Edgeuse and the objects it attaches to need to share common contexts
    let record = b.edgeuse(eu)?;
    let owner = record.owner;
    let has_owner = matches!(
        owner,
        EdgeuseOwner::Loopuse(Some(_)) | EdgeuseOwner::Shell(Some(_))
    );
    let (ccw, cw, vu) = (record.ccw, record.cw, record.vertexuse);
    let has_trim = record.uv_trim_curve.is_some();
    if has_owner {
        rtn &= cx.rep.this_bool(b, C, this, 0, true)?;
    }
    if ccw.is_some() {
        rtn &= cx.rep.this_bool(b, C, this, 1, true)?;
    }
    if cw.is_some() {
        rtn &= cx.rep.this_bool(b, C, this, 2, true)?;
    }
    if vu.is_some() {
        rtn &= cx.rep.this_bool(b, C, this, 3, true)?;
    }
    if has_trim {
        rtn &= cx.rep.this_bool(b, C, this, 4, true)?;
    }

    // m_tEdgeuseType and m_pSorLU have consistent types (intrinsic: one enum)
    if has_owner {
        rtn &= cx.rep.this_bool(b, C, this, 5, true)?;
    }

    // for edgeuses with UVTrimCurves on closed surfaces
    if has_trim {
        rtn &= uv_trim_checks(k, b, cx, eu)?;
    }

    // 17: Edgeuse type is SmLoopuse_TYPE only when m_pSorLU points to a Loopuse
    let is_loop = eu1::is_loop_edgeuse(b, eu)?;
    let owner_loopuse = match owner {
        EdgeuseOwner::Loopuse(lu) => lu,
        _ => None,
    };
    rtn &= cx
        .rep
        .this_bool(b, C, this, 17, !need || !is_loop || owner_loopuse.is_some())?;

    // 14: Edgeuse->GetLoopuse() obj contains this Edgeuse
    if let (true, Some(lu)) = (is_loop, owner_loopuse) {
        let has = in_loopuse(b, cx, lu, eu)?;
        rtn &= cx.rep.this_bool(b, C, this, 14, !need || has)?;
    }

    // 18: Edgeuse type is SmShell_TYPE only when m_pSorLU points to a Shell
    let is_shell = eu1::is_shell_edgeuse(b, eu)?;
    let owner_shell = match owner {
        EdgeuseOwner::Shell(s) => s,
        _ => None,
    };
    rtn &= cx
        .rep
        .this_bool(b, C, this, 18, !need || !is_shell || owner_shell.is_some())?;

    // 13: Edgeuse->GetShell() obj contains this Edgeuse
    if let (true, Some(s)) = (is_shell, owner_shell) {
        let has = if b.shell(s)?.is_wire_shell() {
            let e = b.edgeuse(eu)?.edge;
            in_wire_shell(k, b, cx, s, e)?
        } else {
            // an edgeuse of a wire edge attached to a FaceuseShell has no back pointer
            b.shell(s)?.is_faceuse_shell()
        };
        rtn &= cx.rep.this_bool(b, C, this, 13, !need || has)?;
    }

    // 14: Edgeuse->m_pCCW->m_pCW == Edgeuse
    let ok = !need
        || match ccw {
            None => true,
            Some(x) => b.edgeuse(x)?.cw == Some(eu),
        };
    rtn &= cx.rep.this_bool(b, C, this, 14, ok)?;

    // 15: Edgeuse->m_pCW->m_pCCW == Edgeuse
    let ok = !need
        || match cw {
            None => true,
            Some(x) => b.edgeuse(x)?.ccw == Some(eu),
        };
    rtn &= cx.rep.this_bool(b, C, this, 15, ok)?;

    // 16: Edgeuse->m_pVU is an edge vertexuse of this Edgeuse
    let ok = !need
        || match vu {
            None => true,
            Some(x) => tvu::is_edge_vertexuse(b, x)? && tvu::edgeuse(b, x)? == Some(eu),
        };
    rtn &= cx.rep.this_bool(b, C, this, 16, ok)?;

    // 19: loop neighbour connection made through a single vertex
    let ok = !need || test19(b, eu, vu, cw)?;
    rtn &= cx.rep.this_bool(b, C, this, 19, ok)?;

    // for SmLoopuse_TYPE edgeuses make sure of radial ordering
    if is_loop {
        let e = super::super::required(b.edgeuse(eu)?.edge)?;
        let extent = b.edge(e)?.interval;
        if tedge::is_manifold(b, e)? || tedge::is_spine(b, e)? {
            let radial = eu1::radial(b, eu)?;
            let mate = eu1::mate(b, eu)?;

            // SAME-oriented edgeuses own the mate/radial pair tests
            if eu1::orientation(b, eu)? == OrientType::Same {
                let shell = eu1::shell(b, eu)?;
                let orient = eu1::orientation(b, eu)?;
                // 8: radial pair has consistent shell membership
                let ok = !need
                    || match radial {
                        Some(r) => shell == eu1::shell(b, r)?,
                        None => false,
                    };
                rtn &= cx.rep.this_bool(b, C, this, 8, ok)?;
                // 22: mate pair has consistent face membership
                let ok = !need
                    || match mate {
                        Some(m) => eu1::face(b, eu)? == eu1::face(b, m)?,
                        None => false,
                    };
                rtn &= cx.rep.this_bool(b, C, this, 22, ok)?;
                // 23, 24: Edge->Edgeuse List Orientation order alternates
                let ok = !need
                    || match radial {
                        Some(r) => orient != eu1::orientation(b, r)?,
                        None => false,
                    };
                rtn &= cx.rep.this_bool(b, C, this, 23, ok)?;
                let ok = !need
                    || match mate {
                        Some(m) => orient != eu1::orientation(b, m)?,
                        None => false,
                    };
                rtn &= cx.rep.this_bool(b, C, this, 24, ok)?;

                if let (Some(r), Some(_)) = (radial, mate) {
                    let options = BinormalOptions::default();
                    let t = extent.evaluate(0.456);
                    let target = evaluate_binormal(k, b, eu, t, true, &options)?;
                    let radial_eval = evaluate_binormal(k, b, r, t, true, &options)?;

                    // tangent sector orientation consistency (no new UV trim curves)
                    let tangent_sector = crate::topology::edgeuse::p2::is_tangent_sector(
                        k, b, eu, 5.0, false, false,
                    )?;

                    // 9: in a TangentSector both Faceuse Normals point the same way
                    let dot = target.face_normal.dot(radial_eval.face_normal);
                    let ok = !need || !tangent_sector || dot > -EFF_ZERO;
                    rtn &= cx.rep.this_value(b, C, this, 9, ok, EFF_ZERO, || dot)?;

                    // 10: in a nonTangentSector both Faceuse Normals point into the sector
                    let triple = target
                        .binormal
                        .cross(radial_eval.binormal)
                        .dot(target.face_normal.cross(radial_eval.face_normal));
                    let ok = !need || tangent_sector || triple < EFF_ZERO;
                    rtn &= cx.rep.this_value(b, C, this, 10, ok, EFF_ZERO, || triple)?;

                    // 27: consistent binormal location
                    let ok =
                        !need || target.point.distance(radial_eval.point) < tedge::tolerance(b, e)?;
                    rtn &= cx.rep.this_bool(b, C, this, 27, ok)?;
                }
            }
        }
    }

    // when Edgeuse has a UVTrimCurve - Check that it's valid
    if has_trim {
        let level = cx.rep.level;
        // not upstream (WR-560, opt-in): the trim is measured on its face's surface
        // (`GetEdgeuse()->GetFace()->GetSurface()`)
        let trim_surface = if k.fixes.uv_trim_checks_on_surface {
            match eu1::face(b, eu)? {
                Some(f) => b
                    .face(f)?
                    .surface
                    .as_ref()
                    .map(Surface::clone_for_evaluation)
                    .transpose()?,
                None => None,
            }
        } else {
            None
        };
        let list = cx.rep.array.as_deref_mut();
        if let Some(c) = b.edgeuse_mut(eu)?.uv_trim_curve.as_mut() {
            rtn &= curve(
                k,
                c,
                list,
                level,
                geometry_key(20003, eu.index()),
                geometry_key(20005, eu.index()),
                trim_surface.as_ref(),
            )?;
        }
    }

    // 20: The Control Polygon turns to follow a singular surface boundary
    let dog_leg = seams::has_dog_leg_near_pole(k, b, eu)?;
    rtn &= cx.rep.this_bool(b, C, this, 20, !need || !dog_leg)?;

    // all done
    Ok(rtn)
}

/// The predicate of Edgeuse test 19: the loop neighbour connection is made through a
/// single vertex (or one of its preconditions does not hold).
fn test19(
    b: &Brep,
    eu: EdgeuseId,
    vu: Option<crate::topology::VertexuseId>,
    cw: Option<EdgeuseId>,
) -> Result<bool> {
    let Some(vu) = vu else { return Ok(true) };
    let Some(v) = tvu::vertex(b, vu)? else {
        return Ok(true);
    };
    let e = super::super::required(b.edgeuse(eu)?.edge)?;
    if eu1::edge_is_lamina(b, e)? {
        return Ok(true);
    }
    let Some(cw) = cw else { return Ok(true) };
    let Some(cw_mate) = eu1::mate(b, cw)? else {
        return Ok(true);
    };
    let Some(cw_mate_edge) = b.edgeuse(cw_mate)?.edge else {
        return Ok(true);
    };
    let Some(cw_mate_vu) = eu1::vertexuse(b, cw_mate)? else {
        return Ok(true);
    };
    let Some(cw_mate_v) = tvu::vertex(b, cw_mate_vu)? else {
        return Ok(true);
    };
    if eu1::edge_is_lamina(b, cw_mate_edge)? {
        return Ok(true);
    }
    Ok(v == cw_mate_v)
}
