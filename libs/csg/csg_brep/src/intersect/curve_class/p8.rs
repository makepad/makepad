//! Natural-boundary and surface/volume discontinuity curve classifications.
//!
//! Upstream: `source/SMLib/src/SmCurveClass.cpp`, `source/SMLib/inc/SmCurveClass.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`CurveClassification::intersect_natural_bounds`] | `IntersectNaturalBounds` |
//! | [`CurveClassification::intersect_natural_uv_bounds`] | `IntersectNaturalUVBounds` |
//! | [`CurveClassification::intersect_uv_discontinuities`] | `IntersectUVDiscontinuties` |
//! | [`CurveClassification::intersect_3d_discontinuities`] | `Intersect3DDiscontinuties` (surface) |
//! | [`CurveClassification::intersect_volume_discontinuities`] | `Intersect3DDiscontinuities` (volume) |
//! | `insert_solutions` (private) | `sm_InsertSolutions` call adapter (part 5) |
//! | `plane_intersections` (private) | `SmSurface::GlobalCurveIntersect` call on a boundary plane |
//! | [`uv_discontinuity_intervals`] | the classification in `SmCrvOnSurf::GetKnots` |
//! | [`volume_discontinuity_intervals`] | the classification in `SmCrvInVolume::GetKnots` |
//! | [`FreeHandle`] | the stack classification's objects (a curve and its surface or volume) |
//!
//! Deviations: handles and caller-supplied geometry replace pointers; Kernel is explicit.
//! Temporary boundaries have local ownership; a detached surface copy replaces the
//! temporary native owner clearing. Debug-only drawing is
//! omitted. Ignored ordinary statuses use initialized results where available;
//! evaluation failures without defined outputs return errors, and Unported always propagates.
//! Empty knot lists/interval arrays return errors instead of native undefined
//! behaviour. The native U-only unaligned test is retained.
//! Volume slots 4 and 5 share one 20-solution borrowed buffer natively (WR-155): slot
//! 5's final solutions overwrite slot 4's leading entries while slot 4 fits the buffer;
//! an overflowing slot 5 over a borrowed slot 4 returns an error (native buffer state
//! then depends on intermediate writes). Plane intersection keeps its ignored status but
//! an ordinary failure yields no solutions (native may keep partial output).
//! Temporary boundary curves/planes are named by [`ClassObject::temporary`] handles
//! (native stores the temporaries' pointers, dangling after return) and resolved by a
//! geometry wrapper for the rest of the call; a handle type without temporaries gets an
//! invalid-input error at its first insertion. Solutions passed to part 5 name the
//! classification curve for its curve (and analytic replacement) and the temporary for
//! the boundary (and its analytic replacement); collapsed ranges are copied back.
//! Curve-on-surface construction preserves concrete surface dispatch.
//! No input-driven recursion: dimension forwarding dispatches once through shared helpers.

use crate::curve::crv_on_surf::CrvOnSurf;
use crate::curve::curve::p8::CurveIntersectOptions;
use crate::curve::curve::{Curve, CurveKind};
use crate::curve::line::Line;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::solution_array::{ObjectKind, SolutionObject};
use crate::geom::solution_array::{Solution, SolutionType};
use crate::geom::tol::{
    are_parallel, are_perpendicular, eff_zero_param, xsect_tol_3d_from_zones, zone_tol_3d,
    SmallTopoType,
};
use crate::intersect::curve_class::{
    ClassificationInit, CurveClassification, FoundInterval, IntervalPosition, NO_INTERVAL,
};
use crate::intersect::point_class::{ClassGeometry, ClassObject, ClassObjectKind, PointClassType};
use crate::kernel::Kernel;
use crate::math::{Point2, Point3, Vec2, Vec3, BIG_DOUBLE, EFF_ZERO};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::plane::Plane;
use crate::surface::surface::p1::zone_tol_3d as surface_zone;
use crate::surface::surface::p5::DropPointOptions;
use crate::surface::surface::{Surface, SurfaceKind};
use crate::surface::volume::{Discontinuities3d, Volume};

/// Solution-object tag: 0/2 the classification curve (and its analytic replacement),
/// 1/3 the boundary curve (and its analytic replacement), [`PLANE_TAG`] a boundary
/// plane; with the natural interval of the curve or the natural UV domain of the
/// plane (upstream's objects are the curves and the plane, whose parameter intervals
/// `IdenticalSolutions` reads).
#[derive(Clone, Copy, PartialEq)]
struct BoundaryObject(u8, Extent1d, Extent2d);
impl BoundaryObject {
    /// Curve `tag` with natural interval `ivl`.
    fn curve(tag: u8, ivl: Extent1d) -> Self {
        Self(tag, ivl, Extent2d::new())
    }

    /// The boundary plane with natural UV domain `domain`.
    fn plane(domain: Extent2d) -> Self {
        Self(PLANE_TAG, Extent1d::new(), domain)
    }
}
impl SolutionObject for BoundaryObject {
    fn kind(&self) -> ObjectKind {
        if self.0 == PLANE_TAG {
            ObjectKind::Surface
        } else {
            ObjectKind::Curve
        }
    }
    fn natural_interval(&self) -> Option<Extent1d> {
        (self.0 != PLANE_TAG).then_some(self.1)
    }
    fn natural_uv_domain(&self) -> Option<Extent2d> {
        (self.0 == PLANE_TAG).then_some(self.2)
    }
}

const PLANE_TAG: u8 = 4;

/// Native volume solution slots 4 and 5 borrow one 20-solution buffer (WR-155).
const BORROWED_SOLUTIONS: usize = 20;

const OP: &str = "intersect::curve_class::p8";

/// The caller's geometry plus the temporary boundaries of one p8 call: native keeps
/// every temporary alive (and resolvable through the end objects that insertion
/// records) until the member returns.
struct WithTemporaries<'a, G, O> {
    inner: &'a G,
    curves: Vec<(O, &'a Curve)>,
    surfaces: Vec<(O, &'a Surface)>,
}

impl<'a, G: ClassGeometry<O>, O: ClassObject> WithTemporaries<'a, G, O> {
    fn curves(inner: &'a G, curves: &'a [Curve]) -> Self {
        Self {
            inner,
            curves: curves
                .iter()
                .enumerate()
                .filter_map(|(i, c)| O::temporary(i, ClassObjectKind::Curve).map(|h| (h, c)))
                .collect(),
            surfaces: Vec::new(),
        }
    }
    fn surfaces(inner: &'a G, surfaces: &'a [Surface]) -> Self {
        Self {
            inner,
            curves: Vec::new(),
            surfaces: surfaces
                .iter()
                .enumerate()
                .filter_map(|(i, s)| O::temporary(i, ClassObjectKind::Surface).map(|h| (h, s)))
                .collect(),
        }
    }
}

impl<G: ClassGeometry<O>, O: ClassObject> ClassGeometry<O> for WithTemporaries<'_, G, O> {
    fn curve(&self, o: O) -> Option<&Curve> {
        match self.curves.iter().find(|(h, _)| *h == o) {
            Some((_, c)) => Some(*c),
            None => self.inner.curve(o),
        }
    }
    fn surface(&self, o: O) -> Option<&Surface> {
        match self.surfaces.iter().find(|(h, _)| *h == o) {
            Some((_, s)) => Some(*s),
            None => self.inner.surface(o),
        }
    }
    fn volume(&self, o: O) -> Option<&Volume> {
        self.inner.volume(o)
    }
    fn point(&self, o: O) -> Option<Point3> {
        self.inner.point(o)
    }
    fn classification_vertex_point(&self, vertex: O) -> Option<Point3> {
        self.inner.classification_vertex_point(vertex)
    }
    fn topology_zone_tol_3d(&self, object: O) -> Option<f64> {
        self.inner.topology_zone_tol_3d(object)
    }
    fn vertex_face_gap(&self, vertex: O, surface: &Surface, face: Option<O>) -> Option<f64> {
        self.inner.vertex_face_gap(vertex, surface, face)
    }
    fn owner_of(
        &self,
        owner: crate::geom::object::ObjectRef,
    ) -> Option<crate::geom::object::ObjectRef> {
        self.inner.owner_of(owner)
    }
}

/// `sm_InsertSolutions(this, sols, pTemporary, &tol)`: the solutions name the
/// classification curve and the temporary boundary `index` of `kind`; they are
/// updated in place as native (short ranges collapse to points).
fn insert_solutions<O: ClassObject>(
    k: &Kernel,
    geometry: &impl ClassGeometry<O>,
    cc: &mut CurveClassification<O>,
    solutions: &mut [Solution<BoundaryObject>],
    index: usize,
    kind: ClassObjectKind,
    tol: f64,
) -> Result<()> {
    let target = O::temporary(index, kind).ok_or(SolidError::invalid_input(
        OP,
        "classification handle type cannot name temporary boundaries",
    ))?;
    let curve = cc.curve();
    let mut mapped: Vec<Solution<O>> = solutions
        .iter()
        .map(|s| Solution {
            solution_type: s.solution_type,
            num_variables: s.num_variables,
            start: s.start,
            end: s.end,
            num_objects: s.num_objects,
            objects: s.objects.map(|o| {
                o.and_then(|b| {
                    if b.0 == 0 || b.0 == 2 {
                        curve
                    } else {
                        Some(target)
                    }
                })
            }),
            nodes: s.nodes,
        })
        .collect();
    super::p5::insert_solutions(k, geometry, cc, &mut mapped, target, Some(tol))?;
    for (s, m) in solutions.iter_mut().zip(&mapped) {
        s.solution_type = m.solution_type;
        s.num_variables = m.num_variables;
        s.start = m.start;
        s.end = m.end;
    }
    Ok(())
}

/// `pBdryPlane->GlobalCurveIntersect(pBdryPlane->GetNaturalUVDomain(), *m_cpCurve,
/// m_cpCurve->GetNaturalInterval(), tol, sols)`: `[curve t, plane u, plane v]`.
fn plane_intersections(
    k: &Kernel,
    plane: &Surface,
    curve: &Curve,
    tol: f64,
) -> Result<Vec<Solution<BoundaryObject>>> {
    let natural = curve.natural_interval();
    Ok(plane
        .global_curve_intersect(
            k,
            [
                BoundaryObject::curve(0, natural),
                BoundaryObject::plane(plane.natural_uv_domain_with_nurb(k)?),
            ],
            &plane.natural_uv_domain_with_nurb(k)?,
            curve,
            &natural,
            tol,
        )?
        .as_slice()
        .to_vec())
}

fn pv(k: &Kernel, curve: &Curve, t: f64) -> Result<[Vec3; 2]> {
    let v = curve.evaluate(k, t, 1, true, true)?;
    Ok([
        *v.first().ok_or_else(|| SolidError::sm_err(OP))?,
        *v.get(1).ok_or_else(|| SolidError::sm_err(OP))?,
    ])
}

fn iso_curve(k: &Kernel, surface: &Surface, line: Line) -> Result<Curve> {
    Ok(CrvOnSurf::new(k, line.into(), surface.clone_for_evaluation()?, None, 0)?.into())
}

fn ends(knots: &[f64]) -> Result<(f64, f64)> {
    Ok((
        *knots.first().ok_or_else(|| SolidError::sm_err(OP))?,
        *knots.last().ok_or_else(|| SolidError::sm_err(OP))?,
    ))
}

fn intersections(
    k: &Kernel,
    curve: &Curve,
    interval: &Extent1d,
    boundary: &Curve,
    tol: f64,
    fast: bool,
) -> Result<Vec<Solution<BoundaryObject>>> {
    let this = BoundaryObject::curve(0, curve.natural_interval());
    let other = BoundaryObject::curve(1, boundary.natural_interval());
    if fast {
        if let CurveKind::Line(line) = boundary.kind() {
            let r =
                curve.intersect_with_line(k, interval, line, &boundary.natural_interval(), tol)?;
            if !r.needs_more_intersections {
                return Ok(r
                    .solutions
                    .into_iter()
                    .map(|s| Solution {
                        solution_type: s.solution_type,
                        num_variables: s.num_variables,
                        start: s.start,
                        end: s.end,
                        num_objects: s.num_objects,
                        objects: [Some(this), Some(other), None, None, None],
                        nodes: s.nodes,
                    })
                    .collect());
            }
        }
    }
    Ok(curve
        .global_curve_intersect(
            k,
            interval,
            boundary,
            &boundary.natural_interval(),
            tol,
            &CurveIntersectOptions {
                handles: [Some(this), Some(other)],
                analytic_handles: [
                    Some(BoundaryObject::curve(2, this.1)),
                    Some(BoundaryObject::curve(3, other.1)),
                ],
                ..Default::default()
            },
        )?
        .as_slice()
        .to_vec())
}

impl<O: ClassObject> CurveClassification<O> {
    /// Intersect natural boundaries, forwarding UV curves to the UV algorithm.
    /// Errors from geometry, intersection and solution insertion propagate.
    pub fn intersect_natural_bounds(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        surface_handle: O,
    ) -> Result<()> {
        let uv = self.curve_geometry(geometry)?.dim() == 2;
        self.natural_bounds(k, geometry, surface, surface_handle, uv)
    }

    /// Intersect natural UV boundaries, forwarding 3D curves to the 3D algorithm.
    /// Errors from geometry, intersection and solution insertion propagate.
    pub fn intersect_natural_uv_bounds(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        surface_handle: O,
    ) -> Result<()> {
        let uv = self.curve_geometry(geometry)?.dim() != 3;
        self.natural_bounds(k, geometry, surface, surface_handle, uv)
    }

    fn natural_bounds(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        handle: O,
        uv: bool,
    ) -> Result<()> {
        let mut detached = surface.clone_for_evaluation()?;
        let _ = detached.set_owner(None);
        let surface = &detached;
        let curve = self.curve_geometry(geometry)?;
        let domain = surface.natural_uv_domain_with_nurb(k)?;
        let surf_tol = if uv {
            eff_zero_param()
        } else {
            surface_zone(k, surface)?
        };
        let tol = if uv {
            surf_tol
        } else {
            xsect_tol_3d_from_zones(k, surf_tol, self.src_zone_tol_3d())
        };
        let (boundaries, orients) = if uv {
            let corners = [
                domain.min(),
                Point2::new(domain.max().x, domain.min().y),
                domain.max(),
                Point2::new(domain.min().x, domain.max().y),
                domain.min(),
            ];
            let mut b = Vec::new();
            for pair in corners.windows(2) {
                let a = pair.first().ok_or_else(|| SolidError::sm_err(OP))?;
                let z = pair.get(1).ok_or_else(|| SolidError::sm_err(OP))?;
                b.push(Curve::from(Line::create_line_segment(
                    2,
                    Point3::new(a.x, a.y, 0.0),
                    Point3::new(z.x, z.y, 0.0),
                    None,
                )?));
            }
            (b, vec![OrientType::Same; 4])
        } else {
            let b = surface.create_natural_uv_trim_curves(k, &domain, SurfParamType::V, false)?;
            if b.curves_3d.len() != b.orients.len() {
                return Err(SolidError::sm_err(OP));
            }
            (b.curves_3d, b.orients)
        };
        let geometry = &WithTemporaries::curves(geometry, &boundaries);
        let mut solutions = Vec::new();
        let mut degenerate = Vec::new();
        for (ii, boundary) in boundaries.iter().enumerate() {
            let deg = !uv && boundary.is_degenerate(k, EFF_ZERO, None)?;
            degenerate.push(deg);
            let mut sols = if deg {
                Vec::new()
            } else {
                intersections(k, curve, &self.interval(), boundary, tol, uv)?
            };
            if !sols.is_empty() {
                insert_solutions(
                    k,
                    geometry,
                    self,
                    &mut sols,
                    ii,
                    ClassObjectKind::Curve,
                    tol,
                )?;
            }
            solutions.push(sols);
        }
        for (ii, ((boundary, sols), deg)) in boundaries
            .iter()
            .zip(&solutions)
            .zip(&degenerate)
            .enumerate()
        {
            if *deg {
                continue;
            }
            for sol in sols {
                if sol.solution_type != SolutionType::SingleValue {
                    continue;
                }
                let t = sol.start.parameters[0];
                let cpv = pv(k, curve, t)?;
                let mut bpv = pv(k, boundary, sol.start.parameters[1])?;
                if uv {
                    if are_parallel(bpv[1], cpv[1], None) {
                        continue;
                    }
                    let angle = Vec3::Z.ccw_angle_between(bpv[1], cpv[1])?;
                    self.classify_crossing(
                        k,
                        geometry,
                        t,
                        surf_tol,
                        tol,
                        angle > 0.0,
                        handle,
                        PointClassType::Surface,
                        false,
                    )?;
                } else {
                    let found = self.boundary_interval(k, geometry, t, surf_tol)?;
                    if !matches!(
                        found.position,
                        IntervalPosition::Start | IntervalPosition::End
                    ) || found.end_dist_3d > tol
                    {
                        continue;
                    }
                    let (next, prev) = self.neighbors(found.index, found.position);
                    if self.mid_class(next)? != PointClassType::Unknown
                        && self.mid_class(prev)? != PointClassType::Unknown
                    {
                        continue;
                    }
                    // Native uses the classification parameter here, not the boundary parameter.
                    let p = match ii {
                        0 => Point2::new(t, domain.min().y),
                        1 => Point2::new(domain.max().x, t),
                        2 => Point2::new(t, domain.max().y),
                        _ => Point2::new(domain.min().x, t),
                    };
                    let normal = surface.evaluate_normal(k, p, Side::Left, Side::Left)?;
                    if !are_perpendicular(cpv[1], normal, None) {
                        continue;
                    }
                    if orients.get(ii) == Some(&OrientType::Opposite) {
                        bpv[1] = -bpv[1];
                    }
                    if are_parallel(bpv[1], cpv[1], None) {
                        continue;
                    }
                    let angle = normal.ccw_angle_between(bpv[1], cpv[1])?;
                    self.set_crossing(
                        next,
                        prev,
                        angle > 0.0,
                        handle,
                        PointClassType::Surface,
                        true,
                    )?;
                }
            }
        }
        for i in 0..self.size() {
            let pc = self.at(i)?.mid.point_class();
            if pc != PointClassType::Unknown {
                if pc == PointClassType::Nothing {
                    self.at_mut(i)?
                        .mid
                        .set_class_object(PointClassType::Unknown, None);
                }
                continue;
            }
            let t = self.at(i)?.interval().evaluate(0.5);
            let cpv = pv(k, curve, t)?;
            if uv {
                let inside = domain.contains_point(Point2::new(cpv[0].x, cpv[0].y), 0.0);
                self.at_mut(i)?.mid.set_class_object(
                    if inside {
                        PointClassType::Surface
                    } else {
                        PointClassType::Unknown
                    },
                    if inside { Some(handle) } else { None },
                );
            } else if let Some(drop) = ignored(surface.drop_point::<BoundaryObject>(
                k,
                cpv[0],
                &domain,
                &DropPointOptions {
                    operation: SolverOperation::Normalize,
                    ..Default::default()
                },
            ))? {
                if drop.success && drop.gap < tol {
                    if let Some(normal) =
                        ignored(surface.evaluate_normal(k, drop.result_uv, Side::Left, Side::Left))?
                    {
                        if are_perpendicular(cpv[1], normal, None) {
                            self.at_mut(i)?
                                .mid
                                .set_class_object(PointClassType::Surface, Some(handle));
                        }
                    }
                }
            }
        }
        Ok(())
    }

    /// Intersect UV knot lines, forwarding 3D curves to the surface's 3D algorithm.
    /// Missing geometry adapters and deeper insertion callees return `Unported`.
    pub fn intersect_uv_discontinuities(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        surface_handle: O,
    ) -> Result<()> {
        let uv = self.curve_geometry(geometry)?.dim() != 3;
        self.surface_discontinuities(k, geometry, surface, surface_handle, uv)
    }

    /// Intersect surface knot iso-curves, forwarding UV curves to the UV algorithm.
    /// Missing geometry adapters and deeper insertion callees return `Unported`.
    pub fn intersect_3d_discontinuities(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        surface_handle: O,
    ) -> Result<()> {
        let uv = self.curve_geometry(geometry)?.dim() == 2;
        self.surface_discontinuities(k, geometry, surface, surface_handle, uv)
    }

    fn surface_discontinuities(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        surface: &Surface,
        handle: O,
        uv: bool,
    ) -> Result<()> {
        let mut detached = surface.clone_for_evaluation()?;
        let _ = detached.set_owner(None);
        let surface = &detached;
        let curve = self.curve_geometry(geometry)?;
        let domain = surface.natural_uv_domain_with_nurb(k)?;
        let surf_tol = if uv {
            eff_zero_param()
        } else {
            surface_zone(k, surface)?
        };
        let tol = if uv {
            surf_tol
        } else {
            xsect_tol_3d_from_zones(k, surf_tol, self.src_zone_tol_3d())
        };
        // Native ignores the status; absent output leads to invalid indexing below.
        let u = ignored(surface.knots_with_nurb(k, SurfParamType::U, None))?
            .map(|r| r.0)
            .unwrap_or_default();
        let v = ignored(surface.knots_with_nurb(k, SurfParamType::V, None))?
            .map(|r| r.0)
            .unwrap_or_default();
        let (umin, umax) = ends(&u)?;
        let (vmin, vmax) = ends(&v)?;
        let mut boundaries = Vec::new();
        let mut uv_lines = Vec::new();
        for &x in &u {
            uv_lines.push(Line::create_line_segment(
                2,
                Point3::new(x, vmin, 0.0),
                Point3::new(x, vmax, 0.0),
                None,
            )?);
        }
        for &y in &v {
            uv_lines.push(Line::create_line_segment(
                2,
                Point3::new(umin, y, 0.0),
                Point3::new(umax, y, 0.0),
                None,
            )?);
        }
        for line in &uv_lines {
            boundaries.push(if uv {
                line.clone().into()
            } else {
                iso_curve(k, surface, line.clone())?
            });
        }
        let geometry = &WithTemporaries::curves(geometry, &boundaries);
        let mut solutions: [Vec<Solution<BoundaryObject>>; 4] =
            core::array::from_fn(|_| Vec::new());
        for (ii, boundary) in boundaries.iter().enumerate() {
            let mut sols = intersections(k, curve, &self.interval(), boundary, tol, uv)?;
            let slot = if ii == 0 {
                Some(0)
            } else if ii == u.len() - 1 {
                Some(1)
            } else if ii == u.len() {
                Some(2)
            } else if ii == u.len() + v.len() - 1 {
                Some(3)
            } else {
                None
            };
            if !sols.is_empty() {
                insert_solutions(
                    k,
                    geometry,
                    self,
                    &mut sols,
                    ii,
                    ClassObjectKind::Curve,
                    tol,
                )?;
            }
            if let Some(s) = slot {
                *solutions.get_mut(s).ok_or_else(|| SolidError::sm_err(OP))? = sols;
            }
        }
        let indices = [0, u.len() - 1, u.len(), u.len() + v.len() - 1];
        let mut classified = false;
        for (ii, (index, sols)) in indices.into_iter().zip(&solutions).enumerate() {
            let boundary = boundaries
                .get(index)
                .ok_or_else(|| SolidError::sm_err(OP))?;
            for sol in sols {
                if sol.solution_type != SolutionType::SingleValue {
                    continue;
                }
                let t = sol.start.parameters[0];
                let bt = sol.start.parameters[1];
                let cpv = pv(k, curve, t)?;
                let bpv = pv(k, boundary, bt)?;
                let normal = if uv {
                    Vec3::Z
                } else {
                    let CurveKind::CrvOnSurf(iso) = boundary.kind() else {
                        return Err(SolidError::sm_err(OP));
                    };
                    iso.evaluate_surface_normal(k, bt, true)?.normal
                };
                if are_parallel(bpv[1], cpv[1], None) {
                    continue;
                }
                let angle = normal.ccw_angle_between(bpv[1], cpv[1])?;
                let left = ii == 1 || ii == 2;
                classified |= self.classify_crossing(
                    k,
                    geometry,
                    t,
                    surf_tol,
                    tol,
                    (angle > 0.0 && left) || (angle < 0.0 && !left),
                    handle,
                    PointClassType::Surface,
                    false,
                )?;
            }
        }
        if !classified {
            let first = self.at(0)?;
            let t = first.interval().evaluate(0.5);
            let inside = if uv {
                let p = pv(k, curve, t)?[0];
                domain.contains_point(Point2::new(p.x, p.y), 0.0)
            } else {
                let guess = first
                    .start
                    .curve_object(geometry)?
                    .and_then(|c| match c.kind() {
                        CurveKind::CrvOnSurf(c) => Some(c),
                        _ => None,
                    })
                    .map(|c| c.evaluate_uv_point(k, first.start.t_param()))
                    .transpose()?;
                let drop = ignored(surface.drop_point::<BoundaryObject>(
                    k,
                    curve.evaluate_point(k, t)?,
                    &domain,
                    &DropPointOptions {
                        guess_uv: guess,
                        operation: SolverOperation::Normalize,
                        ..Default::default()
                    },
                ))?;
                drop.is_some_and(|d| d.success)
            };
            self.at_mut(0)?.mid.set_class_object(
                if inside {
                    PointClassType::Surface
                } else {
                    PointClassType::Nothing
                },
                if inside { Some(handle) } else { None },
            );
        }
        self.propagate()
    }

    /// Intersect volume discontinuity planes, using parameter or input space as requested.
    /// UV curves return `SM_ERR`; no-discontinuity volumes leave all intervals unchanged.
    /// Plane intersections and part-5 insertion may return `Unported` from deeper callees.
    pub fn intersect_volume_discontinuities(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        volume: &Volume,
        handle: O,
        in_param_space: bool,
    ) -> Result<()> {
        let curve = self.curve_geometry(geometry)?;
        if curve.dim() == 2 {
            return Err(SolidError::sm_err(OP));
        }
        let mut d = Discontinuities3d::new();
        if !volume.has_discontinuities(k, Some(&mut d), false)? {
            return Ok(());
        }
        let domain = volume.natural_param_domain()?;
        // Native tests U three times, not U/V/W.
        if d.u.unaligned {
            return Err(SolidError::sm_err(OP));
        }
        let (umin, umax) = ends(&d.u.params)?;
        let (vmin, vmax) = ends(&d.v.params)?;
        let (wmin, wmax) = ends(&d.w.params)?;
        let tol = xsect_tol_3d_from_zones(
            k,
            zone_tol_3d(k, SmallTopoType::NormalSize),
            self.src_zone_tol_3d(),
        );
        let mut planes = Vec::new();
        for axis in 0..3 {
            let params = match axis {
                0 => &d.u.params,
                1 => &d.v.params,
                _ => &d.w.params,
            };
            for (j, &value) in params.iter().enumerate() {
                let (mut origin, mut x, mut y, scale) = match axis {
                    0 => (
                        Point3::new(value, vmin, wmin),
                        Vec3::Y,
                        Vec3::Z,
                        Vec2::new(vmax - vmin, wmax - wmin),
                    ),
                    1 => (
                        Point3::new(umin, value, wmin),
                        Vec3::Z,
                        Vec3::X,
                        Vec2::new(wmax - wmin, umax - umin),
                    ),
                    _ => (
                        Point3::new(
                            umin,
                            vmin,
                            *d.w.params.get(j).ok_or_else(|| SolidError::sm_err(OP))?,
                        ),
                        Vec3::X,
                        Vec3::Y,
                        Vec2::new(umax - umin, vmax - vmin),
                    ),
                };
                if !in_param_space {
                    x = ignored(volume.orient_vector(origin, x, [true; 3]))?.unwrap_or(x);
                    y = ignored(volume.orient_vector(origin, y, [true; 3]))?.unwrap_or(y);
                    origin = ignored(volume.orient_point(origin))?.unwrap_or(origin);
                }
                planes.push(Surface::from(Plane::new(
                    origin,
                    x,
                    y,
                    scale,
                    &Extent2d::from_points(Point2::new(0.0, 0.0), Point2::new(1.0, 1.0)),
                )));
            }
        }
        let nu = d.u.params.len();
        let nv = d.v.params.len();
        let nw = d.w.params.len();
        let geometry = &WithTemporaries::surfaces(geometry, &planes);
        let mut solutions: [Vec<Solution<BoundaryObject>>; 6] =
            core::array::from_fn(|_| Vec::new());
        for (i, plane) in planes.iter().enumerate() {
            // Native uses the curve's natural interval (not the classification interval).
            let mut sols = ignored(plane_intersections(k, plane, curve, tol))?.unwrap_or_default();
            let slot = if i == 0 {
                Some(0)
            } else if i == nu - 1 {
                Some(1)
            } else if i == nu {
                Some(2)
            } else if i == nu + nv - 1 {
                Some(3)
            } else if i == nu + nv {
                Some(4)
            } else if i == nu + nv + nw - 1 {
                Some(5)
            } else {
                None
            };
            if !sols.is_empty() {
                insert_solutions(
                    k,
                    geometry,
                    self,
                    &mut sols,
                    i,
                    ClassObjectKind::Surface,
                    tol,
                )?;
            }
            if let Some(s) = slot {
                if s == 5 {
                    // WR-155: slot 4 still borrows the shared buffer when it fits, so
                    // slot 5's (inserted) solutions overwrite its leading entries.
                    let shared = solutions.get_mut(4).ok_or_else(|| SolidError::sm_err(OP))?;
                    if !shared.is_empty() && shared.len() <= BORROWED_SOLUTIONS {
                        if sols.len() > BORROWED_SOLUTIONS {
                            return Err(SolidError::invalid_input(
                                OP,
                                "volume W-plane solutions overflow the native shared buffer",
                            ));
                        }
                        for (aliased, sol) in shared.iter_mut().zip(&sols) {
                            *aliased = *sol;
                        }
                    }
                }
                *solutions.get_mut(s).ok_or_else(|| SolidError::sm_err(OP))? = sols;
            }
        }
        let mut classified = false;
        if domain.is_bounded() {
            for (i, (index, sols)) in [0, nu - 1, nu, nu + nv - 1, nu + nv, nu + nv + nw - 1]
                .into_iter()
                .zip(&solutions)
                .enumerate()
            {
                let SurfaceKind::Plane(plane) = planes
                    .get(index)
                    .ok_or_else(|| SolidError::sm_err(OP))?
                    .kind()
                else {
                    return Err(SolidError::sm_err(OP));
                };
                for sol in sols {
                    if sol.solution_type != SolutionType::SingleValue {
                        continue;
                    }
                    let t = sol.start.parameters[0];
                    let cpv = pv(k, curve, t)?;
                    let normal = plane.evaluate_normal();
                    if normal.is_perpendicular_to(cpv[1], 0.5) {
                        continue;
                    }
                    let dot = normal.dot(cpv[1]);
                    let inward = i == 0 || i == 2 || i == 4;
                    classified |= self.classify_crossing(
                        k,
                        geometry,
                        t,
                        zone_tol_3d(k, SmallTopoType::NormalSize),
                        tol,
                        (dot > 0.0 && inward) || (dot < 0.0 && !inward),
                        handle,
                        PointClassType::Volume,
                        false,
                    )?;
                }
            }
            if !classified {
                let p = curve.evaluate_point(k, self.at(0)?.interval().evaluate(0.5))?;
                let inside = domain.contains_point(p, 0.0);
                self.at_mut(0)?.mid.set_class_object(
                    if inside {
                        PointClassType::Volume
                    } else {
                        PointClassType::Nothing
                    },
                    if inside { Some(handle) } else { None },
                );
            }
        } else {
            self.at_mut(0)?
                .mid
                .set_class_object(PointClassType::Edge, Some(handle));
        }
        self.propagate()
    }

    fn boundary_interval(
        &self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        t: f64,
        obj_tol: f64,
    ) -> Result<FoundInterval> {
        let mut out = FoundInterval {
            index: NO_INTERVAL,
            end_dist_3d: 0.0,
            position: IntervalPosition::Outside,
        };
        // Native ignores this status and inspects the initialized classification.
        ignore_status(self.find_interval_inner(k, geometry, t, BIG_DOUBLE, obj_tol, &mut out))?;
        Ok(out)
    }

    fn neighbors(
        &self,
        index: usize,
        position: IntervalPosition,
    ) -> (Option<usize>, Option<usize>) {
        if position == IntervalPosition::Start {
            (Some(index), index.checked_sub(1))
        } else {
            (
                index.checked_add(1).filter(|i| *i < self.size()),
                Some(index),
            )
        }
    }
    fn mid_class(&self, index: Option<usize>) -> Result<PointClassType> {
        index
            .map(|i| self.at(i).map(|v| v.mid.point_class()))
            .transpose()
            .map(|v| v.unwrap_or(PointClassType::Nothing))
    }
    fn set_crossing(
        &mut self,
        next: Option<usize>,
        prev: Option<usize>,
        enter: bool,
        handle: O,
        inside: PointClassType,
        preserve: bool,
    ) -> Result<()> {
        for (index, enter) in [(next, enter), (prev, !enter)] {
            if let Some(i) = index {
                let pc = self.at(i)?.mid.point_class();
                let class = if preserve && pc != PointClassType::Unknown {
                    pc
                } else if enter {
                    inside
                } else {
                    PointClassType::Nothing
                };
                self.at_mut(i)?
                    .mid
                    .set_class_object(class, if class == inside { Some(handle) } else { None });
            }
        }
        Ok(())
    }
    fn classify_crossing(
        &mut self,
        k: &Kernel,
        geometry: &impl ClassGeometry<O>,
        t: f64,
        obj_tol: f64,
        tol: f64,
        enter: bool,
        handle: O,
        inside: PointClassType,
        preserve: bool,
    ) -> Result<bool> {
        let found = self.boundary_interval(k, geometry, t, obj_tol)?;
        if !matches!(
            found.position,
            IntervalPosition::Start | IntervalPosition::End
        ) || found.end_dist_3d > tol
        {
            return Ok(false);
        }
        let (next, prev) = self.neighbors(found.index, found.position);
        self.set_crossing(next, prev, enter, handle, inside, preserve)?;
        Ok(true)
    }
    fn propagate(&mut self) -> Result<()> {
        let mut class = PointClassType::Unknown;
        let mut object = None;
        for i in self.intervals() {
            if i.mid.point_class() != PointClassType::Unknown {
                class = i.mid.point_class();
                object = i.mid.object();
                break;
            }
        }
        for index in 0..self.size() {
            let i = self.at_mut(index)?;
            if i.mid.point_class() == PointClassType::Unknown {
                i.mid.set_class_object(class, object);
            } else {
                class = i.mid.point_class();
                object = i.mid.object();
            }
            if i.mid.point_class() == PointClassType::Nothing {
                i.mid.set_class_object(PointClassType::Unknown, None);
            }
        }
        Ok(())
    }
}

// ---- free-standing classifications ------------------------------------------------------

/// The objects of a classification of one free-standing curve against one surface or
/// volume: the stack `SmCurveClassification` of `SmCrvOnSurf::GetKnots` and
/// `SmCrvInVolume::GetKnots`, plus the temporary boundaries this part records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FreeHandle {
    /// The classified curve.
    Curve,
    /// The surface or volume it is classified against.
    Target,
    /// The `index`-th temporary of `kind` ([`ClassObject::temporary`]).
    Temporary(usize, ClassObjectKind),
}

impl ClassObject for FreeHandle {
    fn class_kind(&self) -> ClassObjectKind {
        match self {
            Self::Curve => ClassObjectKind::Curve,
            // only read through ClassGeometry::surface / volume
            Self::Target => ClassObjectKind::Other,
            Self::Temporary(_, kind) => *kind,
        }
    }
    fn temporary(index: usize, kind: ClassObjectKind) -> Option<Self> {
        Some(Self::Temporary(index, kind))
    }
}

/// What a free-standing curve is classified against.
#[derive(Clone, Copy)]
enum FreeTarget<'a> {
    Surface(&'a Surface),
    Volume(&'a Volume),
}

/// The geometry behind [`FreeHandle`]s (temporaries are resolved by this part's own
/// wrapper).
struct FreeGeometry<'a> {
    curve: &'a Curve,
    target: FreeTarget<'a>,
}

impl ClassGeometry<FreeHandle> for FreeGeometry<'_> {
    fn curve(&self, o: FreeHandle) -> Option<&Curve> {
        (o == FreeHandle::Curve).then_some(self.curve)
    }
    fn surface(&self, o: FreeHandle) -> Option<&Surface> {
        match (o, self.target) {
            (FreeHandle::Target, FreeTarget::Surface(s)) => Some(s),
            _ => None,
        }
    }
    fn volume(&self, o: FreeHandle) -> Option<&Volume> {
        match (o, self.target) {
            (FreeHandle::Target, FreeTarget::Volume(v)) => Some(v),
            _ => None,
        }
    }
    fn point(&self, _o: FreeHandle) -> Option<Point3> {
        None
    }
}

/// The interval parameter ranges of `cc`, in order (`GetSize` / `GetAt(ii).m_vInterval`).
fn interval_ranges(cc: &CurveClassification<FreeHandle>) -> Vec<Extent1d> {
    cc.intervals().iter().map(|i| i.interval).collect()
}

/// The classification `SmCrvOnSurf::GetKnots` makes of its UV curve `uv_curve` over
/// `interval` (zone tolerance `tol`) on `surface`, cut at the surface's knot lines
/// (`SmCurveClassification(uv, ivl, NULL, tol, 0, NULL, FALSE, TRUE, surface)` then
/// `IntersectUVDiscontinuties`): the parameter range of every interval, in order.
///
/// # Errors
///
/// Those of [`CurveClassification::intersect_uv_discontinuities`]; the constructor's
/// status is ignored, as upstream.
pub fn uv_discontinuity_intervals(
    k: &Kernel,
    uv_curve: &Curve,
    interval: Extent1d,
    tol: f64,
    surface: &Surface,
) -> Result<Vec<Extent1d>> {
    let geometry = FreeGeometry {
        curve: uv_curve,
        target: FreeTarget::Surface(surface),
    };
    let mut cc = CurveClassification::with_curve(
        k,
        &ClassificationInit {
            curve: Some(FreeHandle::Curve),
            interval,
            uv_curve: None,
            src_zone_tol_3d: tol,
            opt_data_size: 0,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: Some(FreeHandle::Target),
            surface2: None,
            uv_curve2: None,
            ti: None,
        },
    )?;
    cc.intersect_uv_discontinuities(k, &geometry, surface, FreeHandle::Target)?;
    Ok(interval_ranges(&cc))
}

/// The classification `SmCrvInVolume::GetKnots` makes of its base curve `curve` over
/// `interval` (zone tolerance `tol`) against `volume`'s discontinuity planes
/// (`SmCurveClassification(curve, ivl, NULL, tol)` then
/// `Intersect3DDiscontinuities(volume, in_param_space)`): the parameter range of every
/// interval, in order.
///
/// # Errors
///
/// Those of [`CurveClassification::intersect_volume_discontinuities`]; the
/// constructor's status is ignored, as upstream.
pub fn volume_discontinuity_intervals(
    k: &Kernel,
    curve: &Curve,
    interval: Extent1d,
    tol: f64,
    volume: &Volume,
    in_param_space: bool,
) -> Result<Vec<Extent1d>> {
    let geometry = FreeGeometry {
        curve,
        target: FreeTarget::Volume(volume),
    };
    let mut cc = CurveClassification::with_curve(
        k,
        &ClassificationInit {
            curve: Some(FreeHandle::Curve),
            interval,
            uv_curve: None,
            src_zone_tol_3d: tol,
            opt_data_size: 0,
            is_curve_owned: false,
            is_curve_current: true,
            surface1: None,
            surface2: None,
            uv_curve2: None,
            ti: None,
        },
    )?;
    cc.intersect_volume_discontinuities(k, &geometry, volume, FreeHandle::Target, in_param_space)?;
    Ok(interval_ranges(&cc))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geom::assert_array::labels::FACE_TYPE;
    use crate::geom::attribute::ObjectKey;
    use crate::geom::extent1d::Extent1d;
    use crate::geom::object::ObjectRef;
    use crate::geom::transform::Transform;
    use crate::intersect::point_class::ClassObjectKind;
    use crate::surface::volume::VolumeKind;

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum H {
        Curve,
        Surface,
        Volume,
    }
    impl ClassObject for H {
        fn class_kind(&self) -> ClassObjectKind {
            match self {
                Self::Curve => ClassObjectKind::Curve,
                Self::Surface => ClassObjectKind::Surface,
                Self::Volume => ClassObjectKind::Volume,
            }
        }
    }
    struct G(Curve);
    impl ClassGeometry<H> for G {
        fn curve(&self, o: H) -> Option<&Curve> {
            (o == H::Curve).then_some(&self.0)
        }
        fn surface(&self, _: H) -> Option<&Surface> {
            None
        }
        fn volume(&self, _: H) -> Option<&Volume> {
            None
        }
        fn point(&self, _: H) -> Option<Point3> {
            None
        }
    }
    fn classification(dim: u64) -> (Kernel, G, CurveClassification<H>) {
        let k = Kernel::default();
        let g = G(Line::create_line_segment(
            dim,
            Vec3::new(0.2, 0.2, 0.),
            Vec3::new(0.8, 0.8, 0.),
            None,
        )
        .unwrap()
        .into());
        let mut cc = CurveClassification::new(&k);
        cc.set_curve(Some(H::Curve), false);
        cc.set_size(&k, 1).unwrap();
        cc.at_mut(0)
            .unwrap()
            .set_interval(&Extent1d::from_min_max(0., 1.));
        cc.set_src_zone_tol_3d(1e-7);
        (k, g, cc)
    }
    #[test]
    fn no_discontinuities_preserves_state_but_still_rejects_uv_curve() {
        let (k, g, mut cc) = classification(3);
        cc.at_mut(0)
            .unwrap()
            .mid
            .set_class_object(PointClassType::Nothing, None);
        let before = cc.intervals().to_vec();
        let volume =
            Volume::with_maps(VolumeKind::Transform(Transform::default()), None, None).unwrap();
        cc.intersect_volume_discontinuities(&k, &g, &volume, H::Volume, true)
            .unwrap();
        assert_eq!(before, cc.intervals());
        let (k, g, mut cc) = classification(2);
        assert!(cc
            .intersect_volume_discontinuities(&k, &g, &volume, H::Volume, true)
            .is_err());
    }
    #[test]
    fn natural_boundary_evaluation_detaches_owner_without_mutating_input() {
        let (k, g, mut cc) = classification(3);
        let mut surface = Surface::from(Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec2::new(1., 1.),
            &Extent2d::from_bounds(0., 0., 1., 1.),
        ));
        let owner = ObjectRef {
            type_code: FACE_TYPE,
            key: ObjectKey(42),
        };
        *surface.owner_mut() = Some(owner);
        cc.intersect_natural_bounds(&k, &g, &surface, H::Surface)
            .unwrap();
        assert_eq!(cc.at(0).unwrap().mid.point_class(), PointClassType::Surface);
        assert_eq!(*surface.owner_mut(), Some(owner));
    }
    #[test]
    fn empty_discontinuity_classification_returns_error() {
        let (k, g, mut cc) = classification(2);
        cc.set_size(&k, 0).unwrap();
        let mut plane = Plane::new(
            Vec3::ZERO,
            Vec3::X,
            Vec3::Y,
            Vec2::new(1., 1.),
            &Extent2d::from_bounds(0., 0., 1., 1.),
        );
        plane.make_nurb().unwrap();
        assert!(cc
            .intersect_uv_discontinuities(&k, &g, &plane.into(), H::Surface)
            .is_err());
    }
}
