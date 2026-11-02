//! Geometry and boundary curves for translational sweeps.
//!
//! Upstream: `source/SMLib/inc/SmTransSweepGeometry.h`,
//! `source/SMLib/src/SmTransSweepGeometry.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`TranslationalSweepGeometry`] | `SmTranslationalSweepGeometry` |
//! | [`TranslationalSweepGeometry::start_coord_system`] | `GetStartCoordSystem` |
//! | [`TranslationalSweepGeometry::end_coord_system`] | `GetEndCoordSystem` |
//! | [`TranslationalSweepGeometry::sweep_transform`] | `GetSweepTransform` |
//! | [`TranslationalSweepGeometry::vertex_sweep_higher`] | `VertexSweepHigher` |
//! | [`TranslationalSweepGeometry::vertex_sweep_same`] | `VertexSweepSame` |
//! | [`TranslationalSweepGeometry::edge_sweep_higher`] | `EdgeSweepHigher` |
//! | [`TranslationalSweepGeometry::edge_sweep_same`] | `EdgeSweepSame` |
//! | [`TranslationalSweepGeometry::face_sweep_same`] | `FaceSweepSame` |
//! | [`TranslationalSweepGeometry::check_curve_sweep`] | `CheckCurveSweep` |
//!
//! Deviations: arena ids replace pointers; owned return values replace output
//! pointers (failed calls expose no partially constructed geometry). `Clone` is
//! the upstream `Copy` method, including the shared base configuration. Only code
//! compiled without `SM_SWEEP_CHECKS` is
//! ported, matching the reference build; the disabled self-intersection experiment
//! and debug drawing are omitted. Ignored statuses still propagate `Unported`.
//! There is no input-driven recursion or allocation from file counts here.

use crate::curve::curve::p7::{PropertyAnalysis, PropertyAnalysisOptions};
use crate::curve::curve::{Curve, CurvePropertyType};
use crate::curve::line::Line;
use crate::error::{ignore_status, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::solution_array::SolutionArray;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::nurbs::b_spline_surface::{BSplineSurface, SurfParamType};
use crate::nurbs::b_spline_surface_n_lib::p2::OrientType;
use crate::nurbs::knots::Side;
use crate::surface::surf_of_revolution::p1::{gen_base, gen_base_mut};
use crate::surface::surface::Surface;
use crate::sweep::topology_sweep::{SweepCheckType, SweepGeometryCreation};
use crate::topology::{edge, face, vertex, Brep, EdgeId, FaceId, ObjectHandle, VertexId};

const OP: &str = "sweep::trans_sweep_geometry";

/// Geometry creation for a sweep by one translation vector.
#[derive(Debug, Clone)]
pub struct TranslationalSweepGeometry {
    /// `m_sSweepVector`: both direction and length.
    pub sweep_vector: Vec3,
    /// Shared tolerance and analytic-recognition configuration.
    pub base: SweepGeometryCreation,
}

/// A swept edge's surface and its four boundaries, in upstream traversal order.
#[derive(Debug, Clone)]
pub struct EdgeSweep {
    /// The swept surface.
    pub surface: Surface,
    /// The original edge tolerance, assigned to the new face.
    pub face_tolerance: f64,
    /// Start edge, minimum-v side, far edge, maximum-v side (no duplicated owners).
    pub curves: [Curve; 4],
    /// Matching two-dimensional straight UV boundaries.
    pub uv_curves: [Line; 4],
    /// Opposite, same, same, opposite, respectively.
    pub orientations: [OrientType; 4],
    /// Tolerances of the four boundary curves, in the same order.
    pub curve_tolerances: [f64; 4],
    /// Index of the start vertex's swept curve (1 or 3).
    pub start_vertex_curve: usize,
    /// Index of the end vertex's swept curve; absent for a closed edge.
    pub end_vertex_curve: Option<usize>,
    /// Unit normal at the original start vertex.
    pub start_normal: Vec3,
}

impl TranslationalSweepGeometry {
    /// Construct with the upstream defaults (`SM_ZONE_TOL_3D`, analytics enabled).
    pub fn new(sweep_vector: Vec3) -> Self {
        Self {
            sweep_vector,
            base: SweepGeometryCreation::default(),
        }
    }

    /// Starting coordinate frame, choosing the least-aligned global axis.
    ///
    /// # Errors
    /// The `Unitize` error for a zero or extremely short sweep vector.
    pub fn start_coord_system(&self, point: Point3) -> Result<Axis2Placement> {
        let v = self.sweep_vector;
        let mut x = if v.x.abs() <= v.y.abs() && v.x.abs() <= v.z.abs() {
            Vec3::X
        } else if v.y.abs() <= v.z.abs() {
            Vec3::Y
        } else {
            Vec3::Z
        };
        let y = v.cross(x).unitize()?;
        x = y.cross(v).unitize()?;
        let mut frame = Axis2Placement::IDENTITY;
        // SetCanonical's status is ignored upstream.
        let _ = frame.set_canonical(point, x, y);
        Ok(frame)
    }

    /// Ending coordinate frame (`GetEndCoordSystem`).
    ///
    /// # Errors
    /// Those of [`Self::start_coord_system`].
    pub fn end_coord_system(&self, point: Point3) -> Result<Axis2Placement> {
        let mut frame = self.start_coord_system(point)?;
        frame.translate(self.sweep_vector);
        Ok(frame)
    }

    /// The pure translation from the start to the end of the sweep.
    pub fn sweep_transform(&self) -> Axis2Placement {
        let mut frame = Axis2Placement::IDENTITY;
        frame.translate(self.sweep_vector);
        frame
    }

    /// Translate a vertex, carrying its tolerance (`VertexSweepSame`).
    ///
    /// # Errors
    /// `SM_ERR` for a sweep shorter than twice the vertex tolerance, or a bad id.
    pub fn vertex_sweep_same(&self, b: &Brep, v: VertexId) -> Result<(Point3, f64)> {
        let original = vertex::point(b, v)?;
        let swept = original + self.sweep_vector;
        let tol = vertex::tolerance(b, v)?;
        if self.sweep_vector.length() < 2.0 * tol {
            return Err(SolidError::sm_err(OP));
        }
        Ok((swept, tol))
    }

    /// The line joining a vertex to its translated position (`VertexSweepHigher`).
    ///
    /// # Errors
    /// Those of [`Self::vertex_sweep_same`] and the line-segment constructor.
    pub fn vertex_sweep_higher(&self, b: &Brep, v: VertexId) -> Result<(Line, f64)> {
        let (swept, tol) = self.vertex_sweep_same(b, v)?;
        Ok((
            Line::create_line_segment(3, vertex::point(b, v)?, swept, None)?,
            tol,
        ))
    }

    /// Copy, trim and translate an edge curve (`EdgeSweepSame`).
    ///
    /// # Errors
    /// `SM_ERR` for a diminishing sweep; otherwise those of lookup/copy/transform.
    /// The trim status is ignored upstream and here, except `Unported`.
    pub fn edge_sweep_same(&self, k: &Kernel, b: &Brep, e: EdgeId) -> Result<(Curve, f64)> {
        let original = edge::curve(b, e)?.ok_or(SolidError::null_pointer(OP))?;
        let tol = edge::tolerance(b, e)?;
        if self.sweep_vector.length() < 2.0 * tol {
            return Err(SolidError::sm_err(OP));
        }
        let mut curve = original.copy(k)?;
        let mut ivl = edge::interval(b, e)?;
        ignore_status(curve.trim(k, &mut ivl, TrimMode::Notify))?;
        curve.transform(k, &self.sweep_transform(), None)?;
        Ok((curve, tol))
    }

    /// Copy with analytic recognition, then translate a face's whole surface.
    ///
    /// # Errors
    /// `SM_ERR` for a diminishing sweep; otherwise those of lookup/copy/transform.
    pub fn face_sweep_same(&self, k: &Kernel, b: &Brep, f: FaceId) -> Result<(Surface, f64)> {
        let original = face::p2::surface(b, f)?.ok_or(SolidError::null_pointer(OP))?;
        let tol = face::p2::tolerance(b, f)?;
        if self.sweep_vector.length() < 2.0 * tol {
            return Err(SolidError::sm_err(OP));
        }
        let mut surface = original.copy_and_add_analytics(k)?;
        surface.transform(k, &self.sweep_transform(), None)?;
        Ok((surface, tol))
    }

    /// Find tangencies parallel to `sweep_vector` over `interval` (`CheckCurveSweep`).
    /// The sweep point and amount are unused in the compiled upstream body.
    ///
    /// # Errors
    /// `Unported` from property-analysis callees; other analysis statuses are ignored.
    pub fn check_curve_sweep(
        &self,
        k: &Kernel,
        curve: &Curve,
        interval: &Extent1d,
        dist_tol: f64,
        sweep_vector: Vec3,
    ) -> Result<SweepCheckType> {
        let vectors = [sweep_vector];
        let options = PropertyAnalysisOptions::<ObjectHandle> {
            opt_vectors: &vectors,
            ..Default::default()
        };
        let mut analysis = PropertyAnalysis {
            solutions: SolutionArray::new(),
            comb_points: Vec::new(),
            comb_vecs: Vec::new(),
        };
        ignore_status(curve.global_property_analysis_into(
            k,
            interval,
            CurvePropertyType::ParallelToVector,
            dist_tol,
            &options,
            &mut analysis,
        ))?;
        Ok(if analysis.solutions.is_empty() {
            SweepCheckType::Okay
        } else {
            SweepCheckType::SweepAlongLength
        })
    }

    /// Sweep an edge into a surface, with ordered 3D/UV boundaries and tolerances.
    ///
    /// # Errors
    /// `SM_ERR` for tangencies, diminishing sweeps or a mismatching start point;
    /// `SM_ERR_NULL_POINTER` without a B-spline-derived curve; callee errors propagate.
    pub fn edge_sweep_higher(&self, k: &Kernel, b: &Brep, e: EdgeId) -> Result<EdgeSweep> {
        let original = edge::curve(b, e)?.ok_or(SolidError::null_pointer(OP))?;
        let mut interval = edge::interval(b, e)?;
        let length = self.sweep_vector.length();
        let direction = self.sweep_vector / length;
        gen_base(original).map_err(|_| SolidError::null_pointer(OP))?;
        let brep_tol = b.tolerance();
        if self.check_curve_sweep(k, original, &interval, self.base.eps, direction)?
            != SweepCheckType::Okay
        {
            return Err(SolidError::sm_err(OP));
        }
        let mut start = original.copy(k)?;
        start.trim(k, &mut interval, TrimMode::Notify)?;
        let start_id = edge::start_vertex(b, e)?.ok_or(SolidError::null_pointer(OP))?;
        let end_id =
            edge::other_vertex(b, e, Some(start_id))?.ok_or(SolidError::null_pointer(OP))?;
        let start_tol = vertex::tolerance(b, start_id)?;
        let end_tol = if end_id == start_id {
            0.0
        } else {
            vertex::tolerance(b, end_id)?
        };
        let tol = edge::tolerance(b, e)?;
        if !(length > 2.0 * start_tol && length > 2.0 * end_tol && length > 2.0 * tol) {
            return Err(SolidError::sm_err(OP));
        }
        let base = gen_base_mut(&mut start).ok_or(SolidError::null_pointer(OP))?;
        let surface = BSplineSurface::create_linear_sweep(base, self.sweep_vector)?;
        let output_surface = if self.base.make_analytics {
            surface.copy_and_add_analytics(k)?
        } else {
            Surface::from(surface)
        };
        let domain = output_surface.natural_uv_domain_with_nurb(k)?;
        let p00 = output_surface.evaluate_point(k, domain.min())?;
        let p01 = output_surface.evaluate_point(k, domain.evaluate(0.0, 1.0))?;
        let start_point = vertex::point(b, start_id)?;
        let d00 = (p00 - start_point).length();
        let d01 = (p01 - start_point).length();
        let closed = edge::is_closed(b, e)?;
        let swap = d01 < d00 && !closed;
        let (distance, uv) = if swap {
            (d01, domain.evaluate(0.0, 1.0))
        } else {
            (d00, domain.min())
        };
        if distance > brep_tol {
            return Err(SolidError::sm_err(OP));
        }
        let normal = output_surface.evaluate_normal(k, uv, Side::Left, Side::Left)?;
        let iso = |param, value| {
            output_surface
                .create_iso_parametric_curve(k, param, value, tol, None, false)
                .map(|i| i.curve)
        };
        let uv_line = |u0, v0, u1, v1| {
            let a = domain.evaluate(u0, v0);
            let z = domain.evaluate(u1, v1);
            Line::create_line_segment(2, Vec3::new(a.x, a.y, 0.0), Vec3::new(z.x, z.y, 0.0), None)
        };
        let uv_start = uv_line(0.0, 0.0, 0.0, 1.0)?;
        let side0 = iso(SurfParamType::V, domain.min().y)?;
        let uv_side0 = uv_line(0.0, 0.0, 1.0, 0.0)?;
        let far = iso(SurfParamType::U, domain.max().x)?;
        let uv_far = uv_line(1.0, 0.0, 1.0, 1.0)?;
        let side1 = iso(SurfParamType::V, domain.max().y)?;
        let uv_side1 = uv_line(0.0, 1.0, 1.0, 1.0)?;
        Ok(EdgeSweep {
            surface: output_surface,
            face_tolerance: tol,
            curves: [start, side0, far, side1],
            uv_curves: [uv_start, uv_side0, uv_far, uv_side1],
            orientations: [
                OrientType::Opposite,
                OrientType::Same,
                OrientType::Same,
                OrientType::Opposite,
            ],
            curve_tolerances: [
                tol,
                if swap { end_tol } else { start_tol },
                tol,
                if swap { start_tol } else { end_tol },
            ],
            start_vertex_curve: if swap { 3 } else { 1 },
            end_vertex_curve: if closed {
                None
            } else {
                Some(if swap { 1 } else { 3 })
            },
            start_normal: normal,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vertex_tolerance_threshold_is_inclusive() {
        let mut b = Brep::new();
        let v = b.add_vertex(Vec3::new(3.0, -2.0, 1.0)).unwrap();
        b.vertex_mut(v).unwrap().tolerance = 0.5;
        let short = TranslationalSweepGeometry::new(Vec3::new(0.0, 0.0, 1.0 - 1e-12));
        assert!(short.vertex_sweep_same(&b, v).is_err());
        let exact = TranslationalSweepGeometry::new(Vec3::Z);
        assert_eq!(
            exact.vertex_sweep_same(&b, v).unwrap(),
            (Vec3::new(3.0, -2.0, 2.0), 0.5)
        );
        let (line, tolerance) = exact.vertex_sweep_higher(&b, v).unwrap();
        assert_eq!(line.evaluate(0.0, 0)[0], b.vertex(v).unwrap().point);
        assert_eq!(line.evaluate(1.0, 0)[0], Vec3::new(3.0, -2.0, 2.0));
        assert_eq!(tolerance, 0.5);
    }

    #[test]
    fn zero_sweep_transforms_but_cannot_define_a_frame() {
        let sweep = TranslationalSweepGeometry::new(Vec3::ZERO);
        assert!(sweep.start_coord_system(Vec3::ZERO).is_err());
        assert!(sweep.end_coord_system(Vec3::ZERO).is_err());
        assert_eq!(sweep.sweep_transform(), Axis2Placement::IDENTITY);
    }

    #[test]
    fn incomplete_topology_and_unknown_ids_return_errors() {
        let k = Kernel::default();
        let sweep = TranslationalSweepGeometry::new(Vec3::Z);
        let mut b = Brep::new();
        let v = b.add_vertex(Vec3::ZERO).unwrap();
        let z = b.add_vertex(Vec3::X).unwrap();
        let e = b.add_edge(v, z).unwrap();
        assert!(sweep.edge_sweep_same(&k, &b, e).is_err());
        assert!(sweep.edge_sweep_higher(&k, &b, e).is_err());
        b.edge_mut(e).unwrap().curve = Some(Curve::from(
            Line::create_line_segment(3, Vec3::ZERO, Vec3::X, None).unwrap(),
        ));
        b.edge_mut(e).unwrap().interval = Extent1d::from_min_max(0.0, 1.0);
        // Geometry exists, but there are no edgeuses / vertexuses to traverse.
        assert!(sweep.edge_sweep_higher(&k, &b, e).is_err());
        let shell = b.add_shell().unwrap();
        let f = b.add_face(shell).unwrap();
        assert!(sweep.face_sweep_same(&k, &b, f).is_err());
        let bad_vertex = VertexId::from_index(u32::MAX);
        assert!(sweep.vertex_sweep_same(&b, bad_vertex).is_err());
        assert!(sweep.vertex_sweep_higher(&b, bad_vertex).is_err());
        let bad_edge = EdgeId::from_index(u32::MAX);
        assert!(sweep.edge_sweep_same(&k, &b, bad_edge).is_err());
        assert!(sweep.edge_sweep_higher(&k, &b, bad_edge).is_err());
        assert!(sweep
            .face_sweep_same(&k, &b, FaceId::from_index(u32::MAX))
            .is_err());
    }

    #[test]
    fn copy_retains_configuration_and_frames_translate() {
        let mut sweep = TranslationalSweepGeometry::new(Vec3::new(-1.0, 2.0, -3.0));
        sweep.base.make_analytics = false;
        sweep.base.eps = 2e-7;
        let copied = sweep.clone();
        assert!(!copied.base.make_analytics);
        assert_eq!(copied.base.eps, 2e-7);
        let origin = Vec3::new(3.0, 4.0, 5.0);
        let a = copied.start_coord_system(origin).unwrap();
        let z = copied.end_coord_system(origin).unwrap();
        assert_eq!(z.origin(), origin + sweep.sweep_vector);
        assert_eq!(a.x_axis(), z.x_axis());
        assert_eq!(a.y_axis(), z.y_axis());
        assert!(a.z_axis().dot(sweep.sweep_vector) > 0.0);
    }
}
