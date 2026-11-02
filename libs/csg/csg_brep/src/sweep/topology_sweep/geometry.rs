//! Static dispatch for the upstream sweep geometry base and its two subclasses.
//!
//! Upstream: `source/SMLib/inc/SmSweepGeometryCreation.h`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`SweepGeometry`] | `SmSweepGeometryCreation*` |
//!
//! Deviations: enum dispatch and owned results replace virtual calls and out-parameters.

use super::{SweepGeometryCreation, OP};
use crate::curve::curve::Curve;
use crate::error::{Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::kernel::Kernel;
use crate::math::Point3;
use crate::surface::surface::Surface;
use crate::sweep::rotational_sweep_geometry::{EdgeSweepHigher, RotationalSweepGeometry};
use crate::sweep::trans_sweep_geometry::TranslationalSweepGeometry;
use crate::topology::{Brep, EdgeId, FaceId, VertexId};

/// The base geometry generator or either concrete upstream sweep generator.
#[derive(Debug, Clone)]
pub enum SweepGeometry {
    /// Base defaults: geometric calls return `SM_ERR`, closure returns false.
    Base(SweepGeometryCreation),
    /// Translational geometry.
    Translation(TranslationalSweepGeometry),
    /// Rotational geometry.
    Rotation(RotationalSweepGeometry),
}

impl SweepGeometry {
    /// The base `Copy()` returns null; concrete creators copy all their state.
    #[must_use]
    pub fn copy_for_sweep(&self) -> Option<Self> {
        match self {
            Self::Base(_) => None,
            _ => Some(self.clone()),
        }
    }

    /// `GetEndCoordSystem`.
    pub fn end_coord_system(&self, point: Point3) -> Result<Axis2Placement> {
        match self {
            Self::Translation(g) => g.end_coord_system(point),
            Self::Rotation(g) => g.end_coord_system(point),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
        }
    }

    /// `IsSweepClosed`: the base and translational sweep are open.
    pub fn is_sweep_closed(&self, repeats: u64) -> Result<bool> {
        match self {
            Self::Rotation(g) => g.is_sweep_closed(repeats),
            _ => Ok(false),
        }
    }

    /// `GetStartCoordSystem`.
    pub fn start_coord_system(&self, point: Point3) -> Result<Axis2Placement> {
        match self {
            Self::Translation(g) => g.start_coord_system(point),
            Self::Rotation(g) => g.start_coord_system(point),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
        }
    }

    /// `GetSweepTransform`.
    pub fn sweep_transform(&self) -> Result<Axis2Placement> {
        match self {
            Self::Translation(g) => Ok(g.sweep_transform()),
            Self::Rotation(g) => Ok(g.sweep_transform()),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
        }
    }

    /// `VertexSweepSame`.
    pub fn vertex_sweep_same(&self, b: &Brep, v: VertexId) -> Result<(Point3, f64)> {
        match self {
            Self::Translation(g) => g.vertex_sweep_same(b, v),
            Self::Rotation(g) => g.vertex_sweep_same(b, v),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
        }
    }

    /// `VertexSweepHigher`: an absent curve leaves the vertex on the rotation axis.
    pub fn vertex_sweep_higher(&self, b: &Brep, v: VertexId) -> Result<Option<(Curve, f64)>> {
        match self {
            Self::Translation(g) => {
                let (c, t) = g.vertex_sweep_higher(b, v)?;
                Ok(Some((
                    Curve::new(crate::curve::curve::CurveKind::Line(c)),
                    t,
                )))
            }
            Self::Rotation(g) => g.vertex_sweep_higher(b, v),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
        }
    }

    /// `EdgeSweepSame`.
    pub fn edge_sweep_same(&self, k: &Kernel, b: &Brep, e: EdgeId) -> Result<(Curve, f64)> {
        match self {
            Self::Translation(g) => g.edge_sweep_same(k, b, e),
            Self::Rotation(g) => g.edge_sweep_same(k, b, e),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
        }
    }

    /// `FaceSweepSame`.
    pub fn face_sweep_same(
        &self,
        k: &Kernel,
        b: &Brep,
        f: FaceId,
    ) -> Result<Option<(Surface, f64)>> {
        match self {
            Self::Translation(g) => g.face_sweep_same(k, b, f).map(Some),
            Self::Rotation(g) => g.face_sweep_same(k, b, f),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
        }
    }

    /// `EdgeSweepHigher`, normalising the two existing owned result layouts.
    pub fn edge_sweep_higher(&self, k: &Kernel, b: &Brep, e: EdgeId) -> Result<EdgeSweepHigher> {
        match self {
            Self::Rotation(g) => g.edge_sweep_higher(k, b, e),
            Self::Base(_) => Err(SolidError::sm_err(OP)),
            Self::Translation(g) => {
                let r = g.edge_sweep_higher(k, b, e)?;
                Ok(EdgeSweepHigher {
                    surface: Some(r.surface),
                    face_tol: r.face_tolerance,
                    start_curve: Some(0),
                    start_curve_tol: r
                        .curve_tolerances
                        .first()
                        .copied()
                        .ok_or(SolidError::sm_err(OP))?,
                    far_end_curve: Some(2),
                    edge_tol: r
                        .curve_tolerances
                        .get(2)
                        .copied()
                        .ok_or(SolidError::sm_err(OP))?,
                    start_vert_curve: Some(r.start_vertex_curve),
                    start_vert_edge_tol: r
                        .curve_tolerances
                        .get(r.start_vertex_curve)
                        .copied()
                        .ok_or(SolidError::sm_err(OP))?,
                    end_vert_curve: r.end_vertex_curve,
                    end_vert_edge_tol: r
                        .end_vertex_curve
                        .and_then(|i| r.curve_tolerances.get(i).copied())
                        .unwrap_or(crate::math::BIG_DOUBLE),
                    trim_curves: r.curves.into_iter().collect(),
                    uv_trim_curves: r.uv_curves.into_iter().collect(),
                    orients: r.orientations.into_iter().collect(),
                    normal: Some(r.start_normal),
                })
            }
        }
    }
}
