//! Topology gaps and gap functions: the cached 3D gap records between topology objects
//! (`SmGap` and its derived classes), lists of them (`SmGapArray`), sampled gaps
//! (`SmGapSample`), and the sampled gap functions from a curve or a surface to a point,
//! a curve or a surface (`SmGapFunction` and its derived classes).
//!
//! Upstream: `source/SMLib/src/SmGap.cpp`, `source/SMLib/inc/SmGap.h`.
//!
//! | Rust                                         | Upstream                                         |
//! |----------------------------------------------|--------------------------------------------------|
//! | [`Gap`]                                      | `SmGap` (the virtual interface over the derived classes) |
//! | [`Gap::length`], [`Gap::is_greater`], [`Gap::is_less`] | `GetLength`, `operator>`, `operator<`    |
//! | [`Gap::assign`]                              | `operator=(const SmGap&)` of every class         |
//! | [`Gap::cache_gap`], [`Gap::gap_end_points`], [`Gap::end_points`], [`GapEndPoints`] | `CacheGap`, `GetGapEndPoints` |
//! | [`Gap::vertex`], [`Gap::vertexuse`], [`Gap::edge`], [`Gap::edgeuse`], [`Gap::face`] | `GetVertex`, `GetVertexuse`, `GetEdge`, `GetEdgeuse`, `GetFace` (base and overrides) |
//! | [`Gap::edge_t`], [`Gap::uv_trim_curve_t`], [`Gap::face_uv`] | `GetEdgeT`, `GetUVTrimCurveT`, `GetFaceUV` (base and overrides) |
//! | [`Gap::type_code`], [`Gap::is_kind_of`], [`types`] | `GetType`, `IsKindOf`, the `Sm*Gap_TYPE` codes |
//! | [`XSECT_TOL_3D`]                             | `SM_XSECT_TOL_3D`                                |
//! | [`VertexEdgeGap`]                            | `SmVertexEdgeGap` (`Set`, `ReSet`, `IsInit`, `GetEdgeT`) |
//! | [`VertexFaceTrimCurveGap`]                   | `SmVertexFaceTrimCurveGap` (`Set`, `ReSet`, `IsInit`, `GetUVTrimCurveT`, `GetFaceUV`) |
//! | [`VertexFaceGap`]                            | `SmVertexFaceGap` (`Set`, `ReSet`, `IsInit`, `GetFaceUV`) |
//! | [`EdgeEdgeGap`]                              | `SmEdgeEdgeGap` (`SetEdgeEdgeGap`, `ReSet`, `IsInit`, `GetEdgeT`) |
//! | [`EdgeFaceTrimCurveGap`]                     | `SmEdgeFaceTrimCurveGap` (`Set`, `GetEdgeT`, `GetUVTrimCurveT`, `GetFaceUV`) |
//! | [`EdgeFaceGap`]                              | `SmEdgeFaceGap` (constructor, `Set`, `SetEdgeuse`, `ReSet`, `IsInit`, `GetEdgeT`, `GetFaceUV`, destructor) |
//! | [`GapArray`]                                 | `SmGapArray` (constructor, `operator[]`, `ReSet`, `GetSize`, `Add`, `Append`, destructor) |
//! | [`GapSample`], [`GapDropType`]               | `SmGapSample` (`GetThisParam`, `GetOtherParam`, `GetLength`, `GetGapDropType`, `SetCanonical`, `SetTolPoint`), `SmGapDropType` |
//! | [`GapFunction`], [`GapSampleType`], [`GapGeometry`] | `SmGapFunction` (constructor, destructor, `SetSampleCnt`, `BuildSampleSet`, `GetMaxGapSample`, `GetMaxIndexSample`, `GetTol`, `GetXSectTol3d`, `GetThis*` / `GetOther*`), `SmGapSampleType` |
//! | [`GapFunction::crv_pt`] ... [`GapFunction::srf_srf`] | the derived class constructors (`SmCrvSrfGapFunction` ...) |
//! | [`GapFunction::evaluate`]                    | `Evaluate` of `SmCrvPtGapFunction`, `SmCrvCrvGapFunction`, `SmCrvSrfGapFunction`, `SmSrfPtGapFunction`, `SmSrfCrvGapFunction`, `SmSrfSrfGapFunction` |
//! | [`add_unique_get_ordered_index`]             | `sm_AddUniqueGetOrderedIndex`                    |
//!
//! Deviations:
//! - The class hierarchy is an enum ([`Gap`]) over plain records; the topology pointers
//!   are ids, so every accessor that follows a pointer takes the [`Brep`]. The setters
//!   (`Set`, `SetEdgeEdgeGap`, the argument constructors) evaluate the gap at once
//!   (`CacheGap`) and so take the B-rep too. `SmGap` used as a bare `double`, the
//!   graphics, dumps, `RefineGeometry` and the base-class `Evaluate` (`SE(SM_ERR)`) have
//!   no counterpart (the enums have no base variant).
//! - Where upstream dereferences null (a vertex edge gap whose vertex use has no edge
//!   use, an edge without curve, a face without surface, an edge parameter without
//!   edge, a UV trim curve parameter without edge use) the port returns
//!   [`SolidError::InvalidInput`]. Statuses upstream ignores (failed evaluations leave
//!   the `SM_UNDEF_DOUBLE` point) are ignored; [`SolidError::Unported`] propagates.
//! - `GetGapEndPoints` reads an existing UV trim curve through `GetOrCreateUVTrimCurve`;
//!   a missing one is created (`topology.edgeuse_p3`), which needs the kernel and the
//!   B-rep mutably: the gap constructors and setters take `k` and `&mut Brep`.
//! - The gap functions borrow their geometry ([`GapGeometry`]); the local
//!   neighbourhoods (`SmCurveClassification`, `BuildLocalNeighborhood`,
//!   `EvaluateNeighborhoodBoundaries`) are not part of this unit and are left out.
//!   `SmBoolean` tri-states are enums / `Option<bool>`.
//! - Sample counts: a count of zero (upstream indexes before the array) and more than
//!   [`MAX_SAMPLES`] samples (upstream allocates whatever the counts say) return
//!   [`SolidError::InvalidInput`] from [`GapFunction::build_sample_set`]. No recursion.

use crate::curve::curve::p2::BSPLINE_CURVE_TYPE;
use crate::curve::curve::p6::DroppedPoint;
use crate::curve::curve::Curve;
use crate::error::{ignored, Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::extent2d::Extent2d;
use crate::geom::global_solver::SolverOperation;
use crate::geom::tol::{are_same_to_tol, eff_zero_param};
use crate::geom::vector2d::Point2;
use crate::geom::vector3d::{Point3, Vec3};
use crate::kernel::Kernel;
use crate::math::{BIG_DOUBLE, EFF_ZERO, EFF_ZERO_SQ, UNDEF_DOUBLE};
use crate::nurbs::b_spline_surface::SurfParamType;
use crate::nurbs::b_spline_surface_n_lib::p2::EvaluateFlags;
use crate::nurbs::knots::Side;
use crate::nurbs::math::NOZ;
use crate::surface::surface::p2::B_SPLINE_SURFACE_TYPE;
use crate::surface::surface::Surface;
use crate::topology::edgeuse::p1 as eu1;
use crate::topology::vertexuse as vu1;
use crate::topology::{Brep, EdgeId, EdgeuseId, FaceId, VertexId, VertexuseId};

const OP: &str = "heal::gap";
const OP_GF: &str = "heal::gap::GapFunction";

/// `SM_ERR_ASSERT_FAILURE`.
const SM_ERR_ASSERT_FAILURE: i32 = 1005;

/// `SM_XSECT_TOL_3D` (`2 * SM_ZONE_TOL_3D`): the default gap tolerance.
pub const XSECT_TOL_3D: f64 = 2.0 * 1.0e-5;

/// Largest number of samples a gap function builds (`SampleCntU * SampleCntV` after
/// the B-spline adjustment); more is [`SolidError::InvalidInput`].
pub const MAX_SAMPLES: usize = 1 << 22;

/// `SmTopoTypes.h` type codes of the gap classes.
pub mod types {
    /// `SmGap_TYPE`.
    pub const GAP: i64 = 16640;
    /// `SmVertexEdgeGap_TYPE`.
    pub const VERTEX_EDGE_GAP: i64 = 16642;
    /// `SmVertexFaceTrimCurveGap_TYPE`.
    pub const VERTEX_FACE_TRIM_CURVE_GAP: i64 = 16644;
    /// `SmVertexFaceGap_TYPE`.
    pub const VERTEX_FACE_GAP: i64 = 16646;
    /// `SmEdgeEdgeGap_TYPE`.
    pub const EDGE_EDGE_GAP: i64 = 16647;
    /// `SmEdgeFaceTrimCurveGap_TYPE`.
    pub const EDGE_FACE_TRIM_CURVE_GAP: i64 = 16648;
    /// `SmEdgeFaceGap_TYPE`.
    pub const EDGE_FACE_GAP: i64 = 16650;
}

fn null(reason: &'static str) -> SolidError {
    SolidError::invalid_input(OP, reason)
}

/// A point whose evaluation status upstream ignores: `SM_UNDEF_DOUBLE` on failure.
fn point_or_undef(r: Result<Point3>) -> Result<Point3> {
    Ok(ignored(r)?.unwrap_or(Vec3::UNDEF))
}

// ---- gap records ----------------------------------------------------------------------

/// A gap between a vertex and the end of an edge's curve it bounds
/// (`SmVertexEdgeGap`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexEdgeGap {
    /// `m_dGap3d`: the gap (`-SM_UNDEF_DOUBLE` when undefined).
    pub gap_3d: f64,
    /// `m_pVertexuse`: the vertex use connecting the vertex to the edge.
    pub vertexuse: Option<VertexuseId>,
    /// `m_dEdgeT`: the edge curve parameter bounding the gap.
    pub edge_t: f64,
}

impl Default for VertexEdgeGap {
    fn default() -> Self {
        Self::new()
    }
}

impl VertexEdgeGap {
    /// `SmVertexEdgeGap()`: the reset state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gap_3d: -UNDEF_DOUBLE,
            vertexuse: None,
            edge_t: UNDEF_DOUBLE,
        }
    }

    /// `SmVertexEdgeGap(pVertexuse, dEdgeT)`: [`VertexEdgeGap::set`] on a new record.
    ///
    /// # Errors
    ///
    /// As [`VertexEdgeGap::set`].
    pub fn with(
        k: &Kernel,
        b: &mut Brep,
        vertexuse: Option<VertexuseId>,
        edge_t: f64,
    ) -> Result<Self> {
        let mut g = Self::new();
        g.set(k, b, vertexuse, edge_t)?;
        Ok(g)
    }

    /// `IsInit`: a vertex use is set.
    #[must_use]
    pub const fn is_init(&self) -> bool {
        self.vertexuse.is_some()
    }

    /// `ReSet`: back to the uninitialised state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `Set`: records the vertex use and parameter and evaluates the gap
    /// (`CacheGap`) unless the parameter is `SM_UNDEF_DOUBLE`.
    ///
    /// # Errors
    ///
    /// Those of [`Gap::cache_gap`].
    pub fn set(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        vertexuse: Option<VertexuseId>,
        edge_t: f64,
    ) -> Result<()> {
        self.vertexuse = vertexuse;
        self.edge_t = edge_t;
        // (upstream: if(m_dEdgeT != SM_UNDEF_DOUBLE) CacheGap() else undefined)
        if self.edge_t == UNDEF_DOUBLE {
            self.gap_3d = -UNDEF_DOUBLE;
        } else {
            self.gap_3d = Gap::from(*self).end_point_distance(k, b)?;
        }
        Ok(())
    }
}

/// A gap between a vertex and the face's surface point of the UV trim curve end it
/// bounds (`SmVertexFaceTrimCurveGap`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexFaceTrimCurveGap {
    /// `m_dGap3d`: the gap (`-SM_UNDEF_DOUBLE` when undefined).
    pub gap_3d: f64,
    /// `m_pVertexuse`: the vertex use connecting the vertex to the face through an
    /// edge.
    pub vertexuse: Option<VertexuseId>,
    /// `m_dUVTrimCurveT`: the UV trim curve parameter bounding the gap.
    pub uv_trim_curve_t: f64,
}

impl Default for VertexFaceTrimCurveGap {
    fn default() -> Self {
        Self::new()
    }
}

impl VertexFaceTrimCurveGap {
    /// `SmVertexFaceTrimCurveGap()`: the reset state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gap_3d: -UNDEF_DOUBLE,
            vertexuse: None,
            uv_trim_curve_t: UNDEF_DOUBLE,
        }
    }

    /// `SmVertexFaceTrimCurveGap(pVertexuse, dUVTrimCurveT)`.
    ///
    /// # Errors
    ///
    /// As [`VertexFaceTrimCurveGap::set`].
    pub fn with(
        k: &Kernel,
        b: &mut Brep,
        vertexuse: Option<VertexuseId>,
        uv_trim_curve_t: f64,
    ) -> Result<Self> {
        let mut g = Self::new();
        g.set(k, b, vertexuse, uv_trim_curve_t)?;
        Ok(g)
    }

    /// `IsInit`: a vertex use is set.
    #[must_use]
    pub const fn is_init(&self) -> bool {
        self.vertexuse.is_some()
    }

    /// `ReSet`: back to the uninitialised state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `Set`: records the vertex use and parameter and evaluates the gap when both are
    /// set.
    ///
    /// # Errors
    ///
    /// Those of [`Gap::cache_gap`].
    pub fn set(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        vertexuse: Option<VertexuseId>,
        uv_trim_curve_t: f64,
    ) -> Result<()> {
        self.vertexuse = vertexuse;
        self.uv_trim_curve_t = uv_trim_curve_t;
        if self.vertexuse.is_some() && self.uv_trim_curve_t != UNDEF_DOUBLE {
            self.gap_3d = Gap::from(*self).end_point_distance(k, b)?;
        } else {
            self.gap_3d = -UNDEF_DOUBLE;
        }
        Ok(())
    }
}

/// A gap between a vertex and the surface of a face it bounds (`SmVertexFaceGap`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VertexFaceGap {
    /// `m_dGap3d`: the gap (`-SM_UNDEF_DOUBLE` when undefined).
    pub gap_3d: f64,
    /// `m_pVertexuse`: the vertex use connecting the vertex to the face.
    pub vertexuse: Option<VertexuseId>,
    /// `m_sFaceUV`: the surface parameter bounding the gap.
    pub face_uv: Point2,
}

impl Default for VertexFaceGap {
    fn default() -> Self {
        Self::new()
    }
}

impl VertexFaceGap {
    /// `SmVertexFaceGap()`: the reset state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gap_3d: -UNDEF_DOUBLE,
            vertexuse: None,
            face_uv: Point2::UNDEF,
        }
    }

    /// `SmVertexFaceGap(pVertexuse, pFaceUV)`.
    ///
    /// # Errors
    ///
    /// As [`VertexFaceGap::set`].
    pub fn with(
        k: &Kernel,
        b: &mut Brep,
        vertexuse: Option<VertexuseId>,
        face_uv: Option<Point2>,
    ) -> Result<Self> {
        let mut g = Self::new();
        g.set(k, b, vertexuse, face_uv)?;
        Ok(g)
    }

    /// `IsInit`: a vertex use is set.
    #[must_use]
    pub const fn is_init(&self) -> bool {
        self.vertexuse.is_some()
    }

    /// `ReSet`: back to the uninitialised state.
    pub fn reset(&mut self) {
        self.gap_3d = -UNDEF_DOUBLE;
        self.vertexuse = None;
        self.face_uv.set_uninitialized();
    }

    /// `Set`: records the vertex use and, when it and an initialised `face_uv` are
    /// given, the surface parameter and the gap; else the parameter and gap are reset.
    ///
    /// # Errors
    ///
    /// Those of [`Gap::cache_gap`].
    pub fn set(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        vertexuse: Option<VertexuseId>,
        face_uv: Option<Point2>,
    ) -> Result<()> {
        self.vertexuse = vertexuse;
        match face_uv {
            Some(uv) if vertexuse.is_some() && uv.is_initialized() => {
                self.face_uv = uv;
                self.gap_3d = Gap::from(*self).end_point_distance(k, b)?;
            }
            _ => {
                self.face_uv.set_uninitialized();
                self.gap_3d = -UNDEF_DOUBLE;
            }
        }
        Ok(())
    }
}

/// A gap between an edge use's end and the next edge use's start
/// (`SmEdgeEdgeGap`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeEdgeGap {
    /// `m_dGap3d`: the gap (`-SM_UNDEF_DOUBLE` when undefined).
    pub gap_3d: f64,
    /// `m_pEdgeuse0`.
    pub edgeuse0: Option<EdgeuseId>,
    /// `m_pEdgeuse1`.
    pub edgeuse1: Option<EdgeuseId>,
    /// `m_dEdgeT0`: the first edge's curve parameter bounding the gap.
    pub edge_t0: f64,
    /// `m_dEdgeT1`: the second edge's curve parameter bounding the gap.
    pub edge_t1: f64,
}

impl Default for EdgeEdgeGap {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeEdgeGap {
    /// `SmEdgeEdgeGap()`: the reset state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gap_3d: -UNDEF_DOUBLE,
            edgeuse0: None,
            edgeuse1: None,
            edge_t0: UNDEF_DOUBLE,
            edge_t1: UNDEF_DOUBLE,
        }
    }

    /// `SmEdgeEdgeGap(pEdgeuse0, pEdgeuse1, dEdgeT0, dEdgeT1)`.
    ///
    /// # Errors
    ///
    /// As [`EdgeEdgeGap::set_edge_edge_gap`].
    pub fn with(
        k: &Kernel,
        b: &mut Brep,
        edgeuse0: Option<EdgeuseId>,
        edgeuse1: Option<EdgeuseId>,
        edge_t0: f64,
        edge_t1: f64,
    ) -> Result<Self> {
        let mut g = Self::new();
        g.set_edge_edge_gap(k, b, edgeuse0, edgeuse1, edge_t0, edge_t1)?;
        Ok(g)
    }

    /// `IsInit`: both edge uses are set.
    #[must_use]
    pub const fn is_init(&self) -> bool {
        self.edgeuse0.is_some() && self.edgeuse1.is_some()
    }

    /// `ReSet`: back to the uninitialised state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `SetEdgeuse(pEdgeuse, lIndx)`: names edge use `0` or (any other index) `1`.
    pub fn set_edgeuse(&mut self, edgeuse: Option<EdgeuseId>, index: usize) {
        if index == 0 {
            self.edgeuse0 = edgeuse;
        } else {
            self.edgeuse1 = edgeuse;
        }
    }

    /// `SetEdgeEdgeGap`: records both edge uses and parameters and evaluates the gap
    /// unless a parameter is `SM_UNDEF_DOUBLE`.
    ///
    /// # Errors
    ///
    /// Those of [`Gap::cache_gap`].
    pub fn set_edge_edge_gap(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        edgeuse0: Option<EdgeuseId>,
        edgeuse1: Option<EdgeuseId>,
        edge_t0: f64,
        edge_t1: f64,
    ) -> Result<()> {
        self.edgeuse0 = edgeuse0;
        self.edgeuse1 = edgeuse1;
        self.edge_t0 = edge_t0;
        self.edge_t1 = edge_t1;
        if self.edge_t0 != UNDEF_DOUBLE && self.edge_t1 != UNDEF_DOUBLE {
            self.gap_3d = Gap::from(*self).end_point_distance(k, b)?;
        } else {
            self.gap_3d = -UNDEF_DOUBLE;
        }
        Ok(())
    }
}

/// A gap between an edge's curve and the face's surface image of its UV trim curve
/// (`SmEdgeFaceTrimCurveGap`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeFaceTrimCurveGap {
    /// `m_dGap3d`: the gap (`-SM_UNDEF_DOUBLE` when undefined).
    pub gap_3d: f64,
    /// `m_pEdgeuse`: the edge use connecting the edge to the face.
    pub edgeuse: Option<EdgeuseId>,
    /// `m_dEdgeT`: the edge curve parameter bounding the gap.
    pub edge_t: f64,
    /// `m_dUVTrimCurveT`: the UV trim curve parameter bounding the gap.
    pub uv_trim_curve_t: f64,
}

impl Default for EdgeFaceTrimCurveGap {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeFaceTrimCurveGap {
    /// `SmEdgeFaceTrimCurveGap()`: the reset state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gap_3d: -UNDEF_DOUBLE,
            edgeuse: None,
            edge_t: UNDEF_DOUBLE,
            uv_trim_curve_t: UNDEF_DOUBLE,
        }
    }

    /// `SmEdgeFaceTrimCurveGap(pEdgeuse, dEdgeT, dUVTrimCurveT)`.
    ///
    /// # Errors
    ///
    /// As [`EdgeFaceTrimCurveGap::set`].
    pub fn with(
        k: &Kernel,
        b: &mut Brep,
        edgeuse: Option<EdgeuseId>,
        edge_t: f64,
        uv_trim_curve_t: f64,
    ) -> Result<Self> {
        let mut g = Self::new();
        g.set(k, b, edgeuse, edge_t, uv_trim_curve_t)?;
        Ok(g)
    }

    /// `IsInit`: an edge use is set.
    #[must_use]
    pub const fn is_init(&self) -> bool {
        self.edgeuse.is_some()
    }

    /// `ReSet`: back to the uninitialised state.
    pub fn reset(&mut self) {
        *self = Self::new();
    }

    /// `Set`: records the edge use and parameters and evaluates the gap when all are
    /// set.
    ///
    /// # Errors
    ///
    /// Those of [`Gap::cache_gap`].
    pub fn set(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        edgeuse: Option<EdgeuseId>,
        edge_t: f64,
        uv_trim_curve_t: f64,
    ) -> Result<()> {
        self.edgeuse = edgeuse;
        self.edge_t = edge_t;
        self.uv_trim_curve_t = uv_trim_curve_t;
        if self.edgeuse.is_some()
            && self.edge_t != UNDEF_DOUBLE
            && self.uv_trim_curve_t != UNDEF_DOUBLE
        {
            self.gap_3d = Gap::from(*self).end_point_distance(k, b)?;
        } else {
            self.gap_3d = -UNDEF_DOUBLE;
        }
        Ok(())
    }
}

/// A gap between an edge's curve and its face's surface (`SmEdgeFaceGap`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct EdgeFaceGap {
    /// `m_dGap3d`: the gap (`-SM_UNDEF_DOUBLE` when undefined).
    pub gap_3d: f64,
    /// `m_pEdgeuse`: the edge use connecting the edge to the face (`None`: not set).
    pub edgeuse: Option<EdgeuseId>,
    /// `m_dEdgeT`: the edge curve parameter bounding the gap.
    pub edge_t: f64,
    /// `m_sFaceUV`: the surface parameter bounding the gap.
    pub face_uv: Point2,
}

impl Default for EdgeFaceGap {
    fn default() -> Self {
        Self::new()
    }
}

impl EdgeFaceGap {
    /// `SmEdgeFaceGap()`: the reset state.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            gap_3d: -UNDEF_DOUBLE,
            edgeuse: None,
            edge_t: UNDEF_DOUBLE,
            face_uv: Point2::UNDEF,
        }
    }

    /// `SmEdgeFaceGap(pEdgeuse, dEdgeT, pFaceUV)`.
    ///
    /// # Errors
    ///
    /// As [`EdgeFaceGap::set`].
    pub fn with(
        k: &Kernel,
        b: &mut Brep,
        edgeuse: Option<EdgeuseId>,
        edge_t: f64,
        face_uv: Option<Point2>,
    ) -> Result<Self> {
        let mut g = Self::new();
        g.set(k, b, edgeuse, edge_t, face_uv)?;
        Ok(g)
    }

    /// `IsInit`: an edge use is set.
    #[must_use]
    pub const fn is_init(&self) -> bool {
        self.edgeuse.is_some()
    }

    /// `ReSet`: back to the uninitialised state.
    pub fn reset(&mut self) {
        self.gap_3d = -UNDEF_DOUBLE;
        self.edgeuse = None;
        self.edge_t = UNDEF_DOUBLE;
        self.face_uv.set_uninitialized();
    }

    /// `SetEdgeuse`: names a different edge use (a mate sharing the gap).
    pub fn set_edgeuse(&mut self, edgeuse: Option<EdgeuseId>) {
        self.edgeuse = edgeuse;
    }

    /// `Set`: records the edge use and parameter and, when both and an initialised
    /// `face_uv` are given, the surface parameter and the gap; else the surface
    /// parameter and gap are reset.
    ///
    /// # Errors
    ///
    /// Those of [`Gap::cache_gap`].
    pub fn set(
        &mut self,
        k: &Kernel,
        b: &mut Brep,
        edgeuse: Option<EdgeuseId>,
        edge_t: f64,
        face_uv: Option<Point2>,
    ) -> Result<()> {
        self.edgeuse = edgeuse;
        self.edge_t = edge_t;
        match face_uv {
            Some(uv) if edgeuse.is_some() && edge_t != UNDEF_DOUBLE && uv.is_initialized() => {
                self.face_uv = uv;
                self.gap_3d = Gap::from(*self).end_point_distance(k, b)?;
            }
            _ => {
                self.face_uv.set_uninitialized();
                self.gap_3d = -UNDEF_DOUBLE;
            }
        }
        Ok(())
    }
}

/// Any gap record (`SmGap*`): the virtual interface of the gap classes.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Gap {
    /// `SmVertexEdgeGap`.
    VertexEdge(VertexEdgeGap),
    /// `SmVertexFaceTrimCurveGap`.
    VertexFaceTrimCurve(VertexFaceTrimCurveGap),
    /// `SmVertexFaceGap`.
    VertexFace(VertexFaceGap),
    /// `SmEdgeEdgeGap`.
    EdgeEdge(EdgeEdgeGap),
    /// `SmEdgeFaceTrimCurveGap`.
    EdgeFaceTrimCurve(EdgeFaceTrimCurveGap),
    /// `SmEdgeFaceGap`.
    EdgeFace(EdgeFaceGap),
}

impl From<VertexEdgeGap> for Gap {
    fn from(g: VertexEdgeGap) -> Self {
        Self::VertexEdge(g)
    }
}

impl From<VertexFaceTrimCurveGap> for Gap {
    fn from(g: VertexFaceTrimCurveGap) -> Self {
        Self::VertexFaceTrimCurve(g)
    }
}

impl From<VertexFaceGap> for Gap {
    fn from(g: VertexFaceGap) -> Self {
        Self::VertexFace(g)
    }
}

impl From<EdgeEdgeGap> for Gap {
    fn from(g: EdgeEdgeGap) -> Self {
        Self::EdgeEdge(g)
    }
}

impl From<EdgeFaceTrimCurveGap> for Gap {
    fn from(g: EdgeFaceTrimCurveGap) -> Self {
        Self::EdgeFaceTrimCurve(g)
    }
}

impl From<EdgeFaceGap> for Gap {
    fn from(g: EdgeFaceGap) -> Self {
        Self::EdgeFace(g)
    }
}

/// The edge of a vertex use's edge use (`m_pVertexuse->GetEdgeuse()->GetEdge()`; the
/// edge use is dereferenced).
fn vertexuse_edge(b: &Brep, vu: VertexuseId) -> Result<Option<EdgeId>> {
    let eu = vu1::edgeuse(b, vu)?.ok_or_else(|| null("GetEdge: vertex use without edge use"))?;
    eu1::edge(b, eu)
}

/// `m_pVertexuse->GetFaceuse() ? ->GetFace() : NULL`.
fn vertexuse_face(b: &Brep, vu: Option<VertexuseId>) -> Result<Option<FaceId>> {
    let Some(vu) = vu else {
        return Ok(None);
    };
    match vu1::faceuse(b, vu)? {
        Some(fu) => eu1::faceuse_face(b, fu),
        None => Ok(None),
    }
}

/// `pEdgeuse->GetUVTrimCurve()->EvaluatePoint(t)` as a UV point, `[SM_UNDEF_DOUBLE,
/// SM_UNDEF_DOUBLE]` without edge use or curve.
fn trim_curve_uv(k: &Kernel, b: &Brep, eu: Option<EdgeuseId>, t: f64) -> Result<Point2> {
    let Some(eu) = eu else {
        return Ok(Point2::UNDEF);
    };
    match eu1::uv_trim_curve(b, eu)?.0 {
        Some(c) => {
            let p = point_or_undef(c.evaluate_point(k, t))?;
            Ok(Point2::new(p.x, p.y))
        }
        None => Ok(Point2::UNDEF),
    }
}

/// `pEdgeuse->GetOrCreateUVTrimCurve(pUVTrimCurve)` (both optional outputs `NULL`,
/// `bUVSpaceOkay` `TRUE`; the returned status is not tested by `GetGapEndPoints`): the
/// attached UV trim curve of the edge use or its mate, built first when missing. A
/// failed build leaves `NULL` (`None`). [`SolidError::Unported`] and the null
/// dereferences propagate.
fn get_or_create_uv_trim_curve(k: &Kernel, b: &mut Brep, eu: EdgeuseId) -> Result<Option<Curve>> {
    let _ =
        crate::topology::edgeuse::p3::get_or_create_uv_trim_curve(k, b, eu, false, false, true)?;
    Ok(eu1::uv_trim_curve(b, eu)?.0.cloned())
}

/// The end points `GetGapEndPoints` found: the first two it set (`SM_UNDEF_DOUBLE`
/// where it set none) and how many it found (upstream fails unless two).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GapEndPoints {
    /// `rPoint1`.
    pub point1: Point3,
    /// `rPoint2`.
    pub point2: Point3,
    /// The number of end points found (`lCnt`).
    pub count: usize,
}

impl Gap {
    /// `GetLength` (`m_dGap3d`).
    #[must_use]
    pub const fn length(&self) -> f64 {
        match self {
            Self::VertexEdge(g) => g.gap_3d,
            Self::VertexFaceTrimCurve(g) => g.gap_3d,
            Self::VertexFace(g) => g.gap_3d,
            Self::EdgeEdge(g) => g.gap_3d,
            Self::EdgeFaceTrimCurve(g) => g.gap_3d,
            Self::EdgeFace(g) => g.gap_3d,
        }
    }

    fn length_mut(&mut self) -> &mut f64 {
        match self {
            Self::VertexEdge(g) => &mut g.gap_3d,
            Self::VertexFaceTrimCurve(g) => &mut g.gap_3d,
            Self::VertexFace(g) => &mut g.gap_3d,
            Self::EdgeEdge(g) => &mut g.gap_3d,
            Self::EdgeFaceTrimCurve(g) => &mut g.gap_3d,
            Self::EdgeFace(g) => &mut g.gap_3d,
        }
    }

    /// `operator>`: this gap is longer.
    #[must_use]
    pub fn is_greater(&self, other: &Self) -> bool {
        self.length() > other.length()
    }

    /// `operator<`: this gap is shorter.
    #[must_use]
    pub fn is_less(&self, other: &Self) -> bool {
        self.length() < other.length()
    }

    /// `operator=(const SmGap&)`: copies the gap length, and the topology and
    /// parameters when `other` is of the same class.
    pub fn assign(&mut self, other: &Self) {
        *self.length_mut() = other.length();
        match (self, other) {
            (Self::VertexEdge(a), Self::VertexEdge(o)) => {
                a.vertexuse = o.vertexuse;
                a.edge_t = o.edge_t;
            }
            (Self::VertexFaceTrimCurve(a), Self::VertexFaceTrimCurve(o)) => {
                a.vertexuse = o.vertexuse;
                a.uv_trim_curve_t = o.uv_trim_curve_t;
            }
            (Self::VertexFace(a), Self::VertexFace(o)) => {
                a.vertexuse = o.vertexuse;
                a.face_uv = o.face_uv;
            }
            (Self::EdgeEdge(a), Self::EdgeEdge(o)) => {
                a.edgeuse0 = o.edgeuse0;
                a.edgeuse1 = o.edgeuse1;
                a.edge_t0 = o.edge_t0;
                a.edge_t1 = o.edge_t1;
            }
            (Self::EdgeFaceTrimCurve(a), Self::EdgeFaceTrimCurve(o)) => {
                a.edgeuse = o.edgeuse;
                a.edge_t = o.edge_t;
                a.uv_trim_curve_t = o.uv_trim_curve_t;
            }
            (Self::EdgeFace(a), Self::EdgeFace(o)) => {
                a.edgeuse = o.edgeuse;
                a.edge_t = o.edge_t;
                a.face_uv = o.face_uv;
            }
            _ => {}
        }
    }

    /// `IsInit`.
    #[must_use]
    pub const fn is_init(&self) -> bool {
        match self {
            Self::VertexEdge(g) => g.is_init(),
            Self::VertexFaceTrimCurve(g) => g.is_init(),
            Self::VertexFace(g) => g.is_init(),
            Self::EdgeEdge(g) => g.is_init(),
            Self::EdgeFaceTrimCurve(g) => g.is_init(),
            Self::EdgeFace(g) => g.is_init(),
        }
    }

    /// `GetType`: the class's `SmTopoTypes.h` code ([`types`]).
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        match self {
            Self::VertexEdge(_) => types::VERTEX_EDGE_GAP,
            Self::VertexFaceTrimCurve(_) => types::VERTEX_FACE_TRIM_CURVE_GAP,
            Self::VertexFace(_) => types::VERTEX_FACE_GAP,
            Self::EdgeEdge(_) => types::EDGE_EDGE_GAP,
            Self::EdgeFaceTrimCurve(_) => types::EDGE_FACE_TRIM_CURVE_GAP,
            Self::EdgeFace(_) => types::EDGE_FACE_GAP,
        }
    }

    /// `IsKindOf`: the class's own code or `SmGap_TYPE`.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == self.type_code() || t == types::GAP
    }

    /// `GetVertexuse`: the vertex use of a vertex gap.
    #[must_use]
    pub const fn vertexuse(&self) -> Option<VertexuseId> {
        match self {
            Self::VertexEdge(g) => g.vertexuse,
            Self::VertexFaceTrimCurve(g) => g.vertexuse,
            Self::VertexFace(g) => g.vertexuse,
            Self::EdgeEdge(_) | Self::EdgeFaceTrimCurve(_) | Self::EdgeFace(_) => None,
        }
    }

    /// `GetVertex`: the vertex of a vertex gap's vertex use.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id.
    pub fn vertex(&self, b: &Brep) -> Result<Option<VertexId>> {
        match self.vertexuse() {
            Some(vu) => vu1::vertex(b, vu),
            None => Ok(None),
        }
    }

    /// `GetEdgeuse(lIndx)`: the edge use bounding the gap (index `0`; an edge/edge gap
    /// also index `1`).
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id.
    pub fn edgeuse(&self, b: &Brep, index: usize) -> Result<Option<EdgeuseId>> {
        Ok(match self {
            Self::VertexEdge(VertexEdgeGap { vertexuse, .. })
            | Self::VertexFaceTrimCurve(VertexFaceTrimCurveGap { vertexuse, .. }) => {
                match (index, vertexuse) {
                    (0, Some(vu)) => vu1::edgeuse(b, *vu)?,
                    _ => None,
                }
            }
            Self::VertexFace(_) => None,
            Self::EdgeEdge(g) => match index {
                0 => g.edgeuse0,
                1 => g.edgeuse1,
                _ => None,
            },
            Self::EdgeFaceTrimCurve(g) => {
                if index == 0 {
                    g.edgeuse
                } else {
                    None
                }
            }
            Self::EdgeFace(g) => {
                if index == 0 {
                    g.edgeuse
                } else {
                    None
                }
            }
        })
    }

    /// `GetEdge(lIndx)`: the edge bounding the gap.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] for a
    /// vertex/edge gap whose vertex use has no edge use (upstream dereferences it).
    pub fn edge(&self, b: &Brep, index: usize) -> Result<Option<EdgeId>> {
        match self {
            Self::VertexEdge(g) => match (index, g.vertexuse) {
                (0, Some(vu)) => vertexuse_edge(b, vu),
                _ => Ok(None),
            },
            // (not overridden: the base class NULL)
            Self::VertexFaceTrimCurve(_) | Self::VertexFace(_) => Ok(None),
            Self::EdgeEdge(g) => {
                let eu = match index {
                    0 => g.edgeuse0,
                    1 => g.edgeuse1,
                    _ => None,
                };
                match eu {
                    Some(eu) => eu1::edge(b, eu),
                    None => Ok(None),
                }
            }
            Self::EdgeFaceTrimCurve(EdgeFaceTrimCurveGap { edgeuse, .. })
            | Self::EdgeFace(EdgeFaceGap { edgeuse, .. }) => match (index, edgeuse) {
                (0, Some(eu)) => eu1::edge(b, *eu),
                _ => Ok(None),
            },
        }
    }

    /// `GetFace`: the face bounding the gap.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id; [`SolidError::InvalidInput`] where
    /// upstream dereferences null (`GetFaceuse` / `GetFace` of the use).
    pub fn face(&self, b: &Brep) -> Result<Option<FaceId>> {
        match self {
            Self::VertexEdge(_) | Self::EdgeEdge(_) => Ok(None),
            Self::VertexFaceTrimCurve(VertexFaceTrimCurveGap { vertexuse, .. })
            | Self::VertexFace(VertexFaceGap { vertexuse, .. }) => vertexuse_face(b, *vertexuse),
            Self::EdgeFaceTrimCurve(EdgeFaceTrimCurveGap { edgeuse, .. })
            | Self::EdgeFace(EdgeFaceGap { edgeuse, .. }) => match edgeuse {
                Some(eu) => eu1::face(b, *eu),
                None => Ok(None),
            },
        }
    }

    /// `GetEdgeT(lIndx)`: the edge curve parameter bounding the gap,
    /// `SM_UNDEF_DOUBLE` when not bounded by an edge.
    #[must_use]
    pub const fn edge_t(&self, index: usize) -> f64 {
        match self {
            Self::VertexEdge(VertexEdgeGap { edge_t, .. })
            | Self::EdgeFaceTrimCurve(EdgeFaceTrimCurveGap { edge_t, .. })
            | Self::EdgeFace(EdgeFaceGap { edge_t, .. }) => {
                if index == 0 {
                    *edge_t
                } else {
                    UNDEF_DOUBLE
                }
            }
            Self::EdgeEdge(g) => {
                if index == 0 {
                    g.edge_t0
                } else {
                    g.edge_t1
                }
            }
            Self::VertexFaceTrimCurve(_) | Self::VertexFace(_) => UNDEF_DOUBLE,
        }
    }

    /// `GetUVTrimCurveT`: the UV trim curve parameter bounding the gap,
    /// `SM_UNDEF_DOUBLE` when not bounded by a face trim curve.
    #[must_use]
    pub const fn uv_trim_curve_t(&self) -> f64 {
        match self {
            Self::VertexFaceTrimCurve(g) => g.uv_trim_curve_t,
            Self::EdgeFaceTrimCurve(g) => g.uv_trim_curve_t,
            _ => UNDEF_DOUBLE,
        }
    }

    /// `GetFaceUV`: the surface parameter bounding the gap (for a trim curve gap the
    /// UV trim curve's point), `[SM_UNDEF_DOUBLE, SM_UNDEF_DOUBLE]` when not bounded by
    /// a face.
    ///
    /// # Errors
    ///
    /// [`SolidError::NotFound`] for a dangling id; [`SolidError::Unported`] from the
    /// trim curve evaluation.
    pub fn face_uv(&self, k: &Kernel, b: &Brep) -> Result<Point2> {
        match self {
            Self::VertexFace(g) => Ok(g.face_uv),
            Self::EdgeFace(g) => Ok(g.face_uv),
            Self::VertexFaceTrimCurve(g) => {
                let eu = match g.vertexuse {
                    Some(vu) => vu1::edgeuse(b, vu)?,
                    None => None,
                };
                trim_curve_uv(k, b, eu, g.uv_trim_curve_t)
            }
            Self::EdgeFaceTrimCurve(g) => trim_curve_uv(k, b, g.edgeuse, g.uv_trim_curve_t),
            Self::VertexEdge(_) | Self::EdgeEdge(_) => Ok(Point2::UNDEF),
        }
    }

    /// `GetGapEndPoints` without the final count check: the points as upstream leaves
    /// them and their count.
    ///
    /// # Errors
    ///
    /// As [`Gap::gap_end_points`] but `SM_ERR`.
    pub fn end_points(&self, k: &Kernel, b: &mut Brep) -> Result<GapEndPoints> {
        // rather than a bunch of derived methods - build the logic here to get end points
        let vertex = self.vertex(b)?;
        let edge0 = self.edge(b, 0)?;
        let edgeuse0 = self.edgeuse(b, 0)?;
        let edge1 = self.edge(b, 1)?;
        let face = self.face(b)?;

        let edge_t0 = self.edge_t(0);
        let edge_t1 = self.edge_t(1);
        let uv_trim_curve_t = self.uv_trim_curve_t();
        let face_uv = self.face_uv(k, b)?;

        // locals
        let mut out = GapEndPoints {
            point1: Vec3::UNDEF,
            point2: Vec3::UNDEF,
            count: 0,
        };
        // (upstream's pTarget: the first point until one is set, then the second)
        let mut second = false;
        let put = |o: &mut GapEndPoints, second: bool, p: Point3| {
            if second {
                o.point2 = p;
            } else {
                o.point1 = p;
            }
        };

        // Point = Vertex->Point
        if let Some(v) = vertex {
            put(&mut out, second, b.vertex(v)?.point);
            second = true;
            out.count += 1;
        }

        // Point = Edge0->Curve(T0)
        if edge_t0 != UNDEF_DOUBLE {
            let e =
                edge0.ok_or_else(|| null("GetGapEndPoints: an EdgeT0 value without an Edge0"))?;
            let c = b
                .edge(e)?
                .curve
                .as_ref()
                .ok_or_else(|| null("GetGapEndPoints: edge without curve"))?;
            if let Some(p) = ignored(c.evaluate_point(k, edge_t0))? {
                put(&mut out, second, p);
            }
            second = true;
            out.count += 1;
        }

        // Point = Edge1->Curve(T1)
        if edge_t1 != UNDEF_DOUBLE {
            let e =
                edge1.ok_or_else(|| null("GetGapEndPoints: an EdgeT1 value without an Edge1"))?;
            let c = b
                .edge(e)?
                .curve
                .as_ref()
                .ok_or_else(|| null("GetGapEndPoints: edge without curve"))?;
            if let Some(p) = ignored(c.evaluate_point(k, edge_t1))? {
                put(&mut out, second, p);
            }
            second = true;
            out.count += 1;
        }

        // Point = Face->Surface->Evaluate(UVTrimCurve(T))
        if uv_trim_curve_t != UNDEF_DOUBLE {
            let eu = edgeuse0
                .ok_or_else(|| null("GetGapEndPoints: a UVTrimCurve without an Edgeuse"))?;
            let uv_curve = get_or_create_uv_trim_curve(k, b, eu)?;
            let surface = match face {
                Some(f) => b.face(f)?.surface.as_ref(),
                None => None,
            };
            // (a missing curve leaves `sPtUV` at its constructed `SM_UNDEF_DOUBLE`s)
            let pt_uv = match uv_curve.as_ref() {
                Some(c) => point_or_undef(c.evaluate_point(k, uv_trim_curve_t))?,
                None => Vec3::UNDEF,
            };
            if let Some(s) = surface {
                if let Some(p) = ignored(s.evaluate_point(k, Point2::new(pt_uv.x, pt_uv.y)))? {
                    put(&mut out, second, p);
                }
                // (upstream then moves pTarget to the second point; nothing follows)
            }
            out.count += 1;
        }
        // Point = Face->Surface(U,V)
        // note: the following else is needed: the trim curve gaps return both
        //       dUVTrimCurveT and sFaceUV values - only evaluate one
        else if face_uv.is_initialized() {
            let f = face.ok_or_else(|| null("GetGapEndPoints: a FaceUV value without a Face"))?;
            let s = b
                .face(f)?
                .surface
                .as_ref()
                .ok_or_else(|| null("GetGapEndPoints: face without surface"))?;
            if let Some(p) = ignored(s.evaluate_point(k, face_uv))? {
                put(&mut out, second, p);
            }
            out.count += 1;
        }
        Ok(out)
    }

    /// `GetGapEndPoints`: the two points bounding the gap (vertex point, edge curve
    /// points, face surface point, in that order).
    ///
    /// # Errors
    ///
    /// `SM_ERR` unless exactly two points were found; [`SolidError::InvalidInput`]
    /// where upstream dereferences null; [`SolidError::NotFound`] for a dangling id;
    /// [`SolidError::Unported`] for a missing UV trim curve.
    pub fn gap_end_points(&self, k: &Kernel, b: &mut Brep) -> Result<(Point3, Point3)> {
        let e = self.end_points(k, b)?;
        if e.count == 2 {
            Ok((e.point1, e.point2))
        } else {
            Err(SolidError::sm_err(OP))
        }
    }

    /// The distance `CacheGap` stores: between the end points, whatever their count.
    fn end_point_distance(&self, k: &Kernel, b: &mut Brep) -> Result<f64> {
        let e = self.end_points(k, b)?;
        Ok(e.point1.distance(e.point2))
    }

    /// `CacheGap`: evaluates and stores the gap length (the end point status is
    /// ignored, as upstream).
    ///
    /// # Errors
    ///
    /// Those of [`Gap::gap_end_points`] but `SM_ERR`.
    pub fn cache_gap(&mut self, k: &Kernel, b: &mut Brep) -> Result<()> {
        let d = self.end_point_distance(k, b)?;
        *self.length_mut() = d;
        Ok(())
    }
}

// ---- gap array ------------------------------------------------------------------------

/// A list of gaps and the index of the longest one added (`SmGapArray`).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GapArray {
    /// `m_lMaxGap3d`: the index of the longest gap added (`SM_BIG_ULONG` = `None`).
    max_gap_3d: Option<usize>,
    /// `m_sGaps`.
    gaps: Vec<Gap>,
}

impl GapArray {
    /// `SmGapArray()`: empty.
    #[must_use]
    pub const fn new() -> Self {
        Self {
            max_gap_3d: None,
            gaps: Vec::new(),
        }
    }

    /// `operator[]`: the gap at `index`.
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Gap> {
        self.gaps.get(index)
    }

    /// `ReSet`: empties the list.
    pub fn reset(&mut self) {
        self.gaps.clear();
        self.max_gap_3d = None;
    }

    /// `GetSize`.
    #[must_use]
    pub fn len(&self) -> usize {
        self.gaps.len()
    }

    /// True when the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.gaps.is_empty()
    }

    /// `GetMaxGap`: the longest gap added.
    #[must_use]
    pub fn max_gap(&self) -> Option<&Gap> {
        self.max_gap_3d.and_then(|i| self.gaps.get(i))
    }

    /// `GetMaxGapLength`: its length, `0` without one.
    #[must_use]
    pub fn max_gap_length(&self) -> f64 {
        self.max_gap().map_or(0.0, Gap::length)
    }

    /// The index of the longest gap added (`m_lMaxGap3d`).
    #[must_use]
    pub const fn max_gap_index(&self) -> Option<usize> {
        self.max_gap_3d
    }

    /// `Add`: appends `gap`, noting it as the longest when it is longer than the
    /// current longest (or the first).
    pub fn add(&mut self, gap: Gap) {
        let longer = match self.max_gap() {
            None => true,
            Some(m) => gap.length() > m.length(),
        };
        if longer {
            self.max_gap_3d = Some(self.gaps.len());
        }
        self.gaps.push(gap);
    }

    /// `Append`: appends every gap of `other` (the longest-gap index is not updated,
    /// as upstream).
    pub fn append(&mut self, other: &Self) {
        self.gaps.extend(other.gaps.iter().copied());
    }
}

// ---- gap samples ----------------------------------------------------------------------

/// The classification of a gap sample's drop (`SmGapDropType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum GapDropType {
    /// `SM_GD_UNKNOWN`: uninitialised.
    #[default]
    Unknown,
    /// `SM_GD_NORMAL`: the gap runs along the other geometry's normal.
    Normal,
    /// `SM_GD_BOUNDARY`: the gap runs to the nearest other-geometry boundary point.
    Boundary,
    /// `SM_GD_NO_DROP`: no drop point was found.
    NoDrop,
}

impl GapDropType {
    /// The upstream enum value.
    #[must_use]
    pub const fn code(self) -> i32 {
        match self {
            Self::Unknown => 0,
            Self::Normal => 1,
            Self::Boundary => 2,
            Self::NoDrop => 3,
        }
    }
}

/// One sampled gap between two geometries (`SmGapSample`).
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct GapSample {
    /// `m_eGapDropType`.
    drop_type: GapDropType,
    /// `m_sThisPos`.
    this_pos: Point3,
    /// `m_sOtherPos`.
    other_pos: Point3,
    /// `m_sTolPoint`.
    tol_point: Point3,
    /// `m_sThisParam`.
    this_param: Point2,
    /// `m_sOtherParam`.
    other_param: Point2,
    /// `m_sXSectTol3d`.
    xsect_tol_3d: f64,
    /// `m_bIsInTol`.
    is_in_tol: bool,
}

impl Default for GapSample {
    fn default() -> Self {
        Self::new(XSECT_TOL_3D)
    }
}

impl GapSample {
    /// A sample slot of a freshly sized `SmTArray<SmGapSample>` (memory set to `0`).
    pub const ZERO: Self = Self {
        drop_type: GapDropType::Unknown,
        this_pos: Vec3::new(0.0, 0.0, 0.0),
        other_pos: Vec3::new(0.0, 0.0, 0.0),
        tol_point: Vec3::new(0.0, 0.0, 0.0),
        this_param: Point2::new(0.0, 0.0),
        other_param: Point2::new(0.0, 0.0),
        xsect_tol_3d: 0.0,
        is_in_tol: false,
    };

    /// `SmGapSample(sXSectTol3d)`: uninitialised points, not in tolerance.
    #[must_use]
    pub const fn new(xsect_tol_3d: f64) -> Self {
        Self {
            drop_type: GapDropType::Unknown,
            this_pos: Vec3::UNDEF,
            other_pos: Vec3::UNDEF,
            tol_point: Vec3::UNDEF,
            this_param: Point2::UNDEF,
            other_param: Point2::UNDEF,
            xsect_tol_3d,
            is_in_tol: false,
        }
    }

    /// `GetGapDropType`.
    #[must_use]
    pub const fn drop_type(&self) -> GapDropType {
        self.drop_type
    }

    /// `GetThisPos`.
    #[must_use]
    pub const fn this_pos(&self) -> Point3 {
        self.this_pos
    }

    /// `GetOtherPos`.
    #[must_use]
    pub const fn other_pos(&self) -> Point3 {
        self.other_pos
    }

    /// `GetTolPoint`.
    #[must_use]
    pub const fn tol_point(&self) -> Point3 {
        self.tol_point
    }

    /// `GetThisParam()`.
    #[must_use]
    pub const fn this_param(&self) -> Point2 {
        self.this_param
    }

    /// `GetOtherParam()`.
    #[must_use]
    pub const fn other_param(&self) -> Point2 {
        self.other_param
    }

    /// `GetThisParam(lIndx)`: `x` for index `0`, else `y`.
    #[must_use]
    pub const fn this_param_at(&self, index: usize) -> f64 {
        if index == 0 {
            self.this_param.x
        } else {
            self.this_param.y
        }
    }

    /// `GetOtherParam(lIndx)`: `x` for index `0`, else `y`.
    #[must_use]
    pub const fn other_param_at(&self, index: usize) -> f64 {
        if index == 0 {
            self.other_param.x
        } else {
            self.other_param.y
        }
    }

    /// `GetXSectTol3d`.
    #[must_use]
    pub const fn xsect_tol_3d(&self) -> f64 {
        self.xsect_tol_3d
    }

    /// `IsInTol`.
    #[must_use]
    pub const fn is_in_tol(&self) -> bool {
        self.is_in_tol
    }

    /// `GetLength`: the distance between the end positions.
    #[must_use]
    pub fn length(&self) -> f64 {
        self.this_pos.distance(self.other_pos)
    }

    /// `GetLengthSquared`.
    #[must_use]
    pub fn length_squared(&self) -> f64 {
        self.this_pos.distance_squared(self.other_pos)
    }

    /// `SetCanonical`: sets every value and recomputes the tolerance state.
    #[allow(clippy::too_many_arguments)]
    pub fn set_canonical(
        &mut self,
        this_pos: Point3,
        this_u: f64,
        this_v: f64,
        other_pos: Point3,
        other_u: f64,
        other_v: f64,
        drop_type: GapDropType,
        xsect_tol_3d: f64,
    ) {
        self.drop_type = drop_type;
        self.this_pos = this_pos;
        self.other_pos = other_pos;
        self.this_param = Point2::new(this_u, this_v);
        self.other_param = Point2::new(other_u, other_v);
        self.xsect_tol_3d = xsect_tol_3d;
        self.is_in_tol = false;
        self.set_tol_point();
    }

    /// `SetTolPoint`: when both positions are defined, whether the gap is within the
    /// tolerance and, when not, the point at the tolerance distance from this
    /// position towards the other.
    fn set_tol_point(&mut self) {
        let defined = |p: Point3| {
            p.x != UNDEF_DOUBLE && p.y != UNDEF_DOUBLE && p.z != UNDEF_DOUBLE && p.z != NOZ
        };
        // no work - m_sThisPos or m_sOtherPos still undefined
        if !(defined(self.this_pos) && defined(self.other_pos)) {
            return;
        }

        // locals
        if self.xsect_tol_3d <= 0.0 {
            self.xsect_tol_3d = XSECT_TOL_3D;
        }
        let tol_sq = (self.xsect_tol_3d * self.xsect_tol_3d).abs();
        let gap_sq = self.this_pos.distance_squared(self.other_pos);

        // check for in/out tolerance gaps
        self.is_in_tol = gap_sq <= tol_sq + self.xsect_tol_3d * EFF_ZERO;

        // Point at the tolerance bound
        if self.is_in_tol {
            self.tol_point.set_uninitialized();
        } else {
            let param = (tol_sq / gap_sq).sqrt();
            self.tol_point = (1.0 - param) * self.this_pos + param * self.other_pos;
        }
    }
}

// ---- gap functions --------------------------------------------------------------------

/// What a gap function samples from and to (`SmGapSampleType`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapSampleType {
    /// `SM_GS_CRV_PT`.
    CrvPt,
    /// `SM_GS_CRV_CRV`.
    CrvCrv,
    /// `SM_GS_CRV_SRF`.
    CrvSrf,
    /// `SM_GS_SRF_PT`.
    SrfPt,
    /// `SM_GS_SRF_CRV`.
    SrfCrv,
    /// `SM_GS_SRF_SRF`.
    SrfSrf,
}

impl GapSampleType {
    /// True for the types sampling a curve (`SampleCntV` is always `1`).
    #[must_use]
    pub const fn is_curve(self) -> bool {
        matches!(self, Self::CrvPt | Self::CrvCrv | Self::CrvSrf)
    }
}

/// The geometry pair of a gap function (the members of its derived class).
#[derive(Debug, Clone, Copy)]
pub enum GapGeometry<'a> {
    /// `SmCrvPtGapFunction`: `m_pThisCurve`, `m_sOtherPoint`.
    CrvPt {
        /// This curve.
        curve: &'a Curve,
        /// The other point.
        point: Point3,
    },
    /// `SmCrvCrvGapFunction`: `m_pThisCurve`, `m_pOtherCurve`.
    CrvCrv {
        /// This curve.
        curve: &'a Curve,
        /// The other curve.
        other: &'a Curve,
    },
    /// `SmCrvSrfGapFunction`: `m_pThisCurve`, `m_pOtherSurface`.
    CrvSrf {
        /// This curve.
        curve: &'a Curve,
        /// The other surface.
        other: &'a Surface,
    },
    /// `SmSrfPtGapFunction`: `m_pThisSurface`, `m_sOtherPoint`.
    SrfPt {
        /// This surface.
        surface: &'a Surface,
        /// The other point.
        point: Point3,
    },
    /// `SmSrfCrvGapFunction`: `m_pThisSurface`, `m_pOtherCurve`.
    SrfCrv {
        /// This surface.
        surface: &'a Surface,
        /// The other curve.
        other: &'a Curve,
    },
    /// `SmSrfSrfGapFunction`: `m_pThisSurface`, `m_pOtherSurface`.
    SrfSrf {
        /// This surface.
        surface: &'a Surface,
        /// The other surface.
        other: &'a Surface,
    },
}

impl GapGeometry<'_> {
    /// The sample type of the pair.
    #[must_use]
    pub const fn sample_type(&self) -> GapSampleType {
        match self {
            Self::CrvPt { .. } => GapSampleType::CrvPt,
            Self::CrvCrv { .. } => GapSampleType::CrvCrv,
            Self::CrvSrf { .. } => GapSampleType::CrvSrf,
            Self::SrfPt { .. } => GapSampleType::SrfPt,
            Self::SrfCrv { .. } => GapSampleType::SrfCrv,
            Self::SrfSrf { .. } => GapSampleType::SrfSrf,
        }
    }
}

/// How far the samples of a gap function are computed (`m_bAreSamplesSet`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SamplesState {
    /// `FALSE`: not computed.
    NotSet,
    /// `TRUE`: all computed.
    Set,
    /// `UNSURE`: computed up to the first gap larger than the tolerance.
    UntilBigGap,
}

/// The gap function from one shape to another, approximated by a grid of gap samples
/// (`SmGapFunction` and its derived classes).
#[derive(Debug, Clone)]
pub struct GapFunction<'a> {
    /// The shapes (the derived class and its members; `m_eSampleType`).
    geometry: GapGeometry<'a>,
    /// `m_sXSectTol3d`.
    xsect_tol_3d: f64,
    /// `m_sSampleIvlU`.
    sample_ivl_u: Extent1d,
    /// `m_sSampleIvlV`.
    sample_ivl_v: Extent1d,
    /// `m_bAreSamplesSet`.
    samples_set: SamplesState,
    /// `m_bIsLocalNeighborhoodSet`.
    local_neighborhood_set: bool,
    /// `m_lSampleCntU`.
    sample_cnt_u: usize,
    /// `m_lSampleCntV`.
    sample_cnt_v: usize,
    /// `m_sSampleGaps`, row major.
    samples: Vec<GapSample>,
    /// `m_lMaxSampleIndex`.
    max_sample_index: usize,
    /// `m_lMinSampleIndex`.
    min_sample_index: usize,
    /// `m_bAllNormalGaps` (`None` = `UNSURE`).
    all_normal_gaps: Option<bool>,
    /// `m_sOtherIvlU`.
    other_ivl_u: Extent1d,
    /// `m_sOtherIvlV`.
    other_ivl_v: Extent1d,
}

/// `sm_AddUniqueGetOrderedIndex`: the index of `value` in the sorted `array`, inserting
/// it in order when no entry is within `SmTol::GetEffZeroParam()` of it.
pub fn add_unique_get_ordered_index(array: &mut Vec<f64>, value: f64) -> usize {
    let tol = eff_zero_param();
    // low work - no elements
    let (Some(&first), Some(&last)) = (array.first(), array.last()) else {
        array.push(value);
        return 0;
    };

    // low Work - element equal to 1st entry
    if are_same_to_tol(value, first, tol) {
        return 0;
    }

    // low Work - element less than 1st entry
    if value < first {
        array.insert(0, value);
        return 0;
    }

    // low Work - element greater than last entry (needed for 1 entry cases)
    if value > last {
        array.push(value);
        return array.len() - 1;
    }

    // low Work - element equal to last entry
    if are_same_to_tol(value, last, tol) {
        return array.len() - 1;
    }

    // 2 or more entries with the new element inside the entry range
    for ii in 1..array.len() {
        let Some(&a) = array.get(ii) else { break };
        // when new element already exists - return its index
        if are_same_to_tol(value, a, tol) {
            return ii;
        }
        // else if new element < rArray[ii] - insert value and return index value
        if value < a {
            array.insert(ii, value);
            return ii;
        }
    }

    // should never arrive here (upstream reports a bug and returns 0)
    0
}

/// The unique knots of a `GetKnots(rUniqueKnots, NULL, NULL)` call whose status is
/// ignored (empty on failure).
fn unique_knots(r: Result<(Vec<f64>, Vec<usize>)>) -> Result<Vec<f64>> {
    Ok(ignored(r)?.map(|(k, _)| k).unwrap_or_default())
}

/// The samples per knot interval that give at least `count - 1` samples over
/// `intervals` intervals, an even number of at least 2.
fn samples_per_interval(count: usize, intervals: usize) -> Result<usize> {
    let want = count
        .checked_sub(1)
        .ok_or_else(|| SolidError::invalid_input(OP_GF, "zero sample count"))?;
    // integer number of samples per interval
    let mut per = want / intervals;
    // when rounded down (fewer samples than requested)
    if per * intervals < want {
        per += 1;
    }
    // at least 2 - guarantees interior samples of each interval
    if per < 2 {
        per += 1;
    }
    // even: one sample at the interval center and one at its end
    if per % 2 != 0 {
        per += 1;
    }
    Ok(per)
}

fn too_many() -> SolidError {
    SolidError::invalid_input(OP_GF, "too many gap samples")
}

fn at(v: &[f64], i: usize) -> Result<f64> {
    v.get(i)
        .copied()
        .ok_or_else(|| SolidError::invalid_input(OP_GF, "sample index out of range"))
}

fn set_at(v: &mut [f64], i: usize, x: f64) -> Result<()> {
    *v.get_mut(i)
        .ok_or_else(|| SolidError::invalid_input(OP_GF, "sample index out of range"))? = x;
    Ok(())
}

/// A curve drop whose failing status upstream ignores (no success).
fn curve_drop(
    k: &Kernel,
    curve: &Curve,
    domain: &Extent1d,
    point: Point3,
    dist_tol: f64,
    guess: Option<f64>,
) -> Result<DroppedPoint> {
    Ok(ignored(curve.drop_point(
        k,
        domain,
        point,
        None,
        dist_tol,
        guess,
        SolverOperation::Minimize,
    ))?
    .unwrap_or_default())
}

/// `pSurface->DropPoint(pt, domain, pOptGuess, bSuccess, uv, dist, bIsMulti)`: success,
/// the drop parameters and distance; the status is ignored as upstream does.
fn surface_drop(
    k: &Kernel,
    surface: &Surface,
    point: Point3,
    domain: &Extent2d,
    guess: Option<Point2>,
) -> Result<(bool, Point2, f64)> {
    let opts = crate::surface::surface::p5::DropPointOptions {
        guess_uv: guess,
        ..Default::default()
    };
    // Upstream ignores the DropPoint status (no `SER`) and reads `bSuccess`/`dDist`
    // either way; on failure they keep their initial values (`FALSE`, the guess, `BIG_DOUBLE`).
    Ok(
        match ignored(
            surface.drop_point::<crate::surface::surface::p4::NoObject>(k, point, domain, &opts),
        )? {
            Some(d) => (d.success, d.result_uv, d.gap),
            None => (false, guess.unwrap_or(Point2::UNDEF), BIG_DOUBLE),
        },
    )
}

/// `Evaluate(dOtherU, 1, TRUE, sOtherPD, TRUE)` of a curve, status ignored: the point
/// and tangent, `SM_UNDEF_DOUBLE` on failure.
fn curve_point_tangent(k: &Kernel, curve: &Curve, t: f64) -> Result<[Vec3; 2]> {
    let d = ignored(curve.evaluate(k, t, 1, true, true))?.unwrap_or_default();
    Ok([
        d.first().copied().unwrap_or(Vec3::UNDEF),
        d.get(1).copied().unwrap_or(Vec3::UNDEF),
    ])
}

/// `Evaluate(uv, 1, 1, TRUE, TRUE, TRUE, sOtherPD[0], TRUE)` of a surface, status
/// ignored: the point and the two first partials, `SM_UNDEF_DOUBLE` on failure.
fn surface_point_partials(k: &Kernel, surface: &Surface, uv: Point2) -> Result<[Vec3; 3]> {
    let flags = EvaluateFlags {
        u_side: Side::Left,
        v_side: Side::Left,
        only_upper_half: true,
        non_zero_tangents: true,
    };
    let d = ignored(surface.evaluate(k, uv, 1, 1, flags))?.unwrap_or_default();
    let get = |i: usize, j: usize| {
        d.get(i)
            .and_then(|r| r.get(j))
            .copied()
            .unwrap_or(Vec3::UNDEF)
    };
    Ok([get(0, 0), get(1, 0), get(0, 1)])
}

impl<'a> GapFunction<'a> {
    /// `SmGapFunction(eType, rXSectTol3d, rSampleIvlU, rSampleIvlV, lSampleCntU,
    /// lSampleCntV)`.
    fn base(
        geometry: GapGeometry<'a>,
        xsect_tol_3d: f64,
        sample_ivl_u: Extent1d,
        sample_ivl_v: Extent1d,
        sample_cnt_u: usize,
        sample_cnt_v: usize,
    ) -> Self {
        let mut g = Self {
            geometry,
            xsect_tol_3d,
            sample_ivl_u,
            sample_ivl_v,
            samples_set: SamplesState::NotSet,
            local_neighborhood_set: false,
            sample_cnt_u: 0,
            sample_cnt_v: 0,
            samples: Vec::new(),
            max_sample_index: 0,
            min_sample_index: 0,
            all_normal_gaps: None,
            other_ivl_u: Extent1d::new(),
            other_ivl_v: Extent1d::new(),
        };
        g.set_sample_cnt(sample_cnt_u, sample_cnt_v);
        g
    }

    /// A curve-type gap function over `this_ivl` (the second interval is `[0, 1]`).
    fn curve_based(
        geometry: GapGeometry<'a>,
        xsect_tol_3d: f64,
        this_ivl: &Extent1d,
        sample_cnt_u: usize,
    ) -> Self {
        let mut g = Self::base(
            geometry,
            xsect_tol_3d,
            *this_ivl,
            *this_ivl,
            sample_cnt_u,
            1,
        );
        g.sample_ivl_v = Extent1d::from_raw(0.0, 1.0);
        g
    }

    /// `SmCrvPtGapFunction(rXSectTol3d, pThisCurve, rThisIvl, pOtherPoint, lSampleCntU)`
    /// (upstream default count `20`).
    #[must_use]
    pub fn crv_pt(
        xsect_tol_3d: f64,
        curve: &'a Curve,
        this_ivl: &Extent1d,
        point: Point3,
        sample_cnt_u: usize,
    ) -> Self {
        Self::curve_based(
            GapGeometry::CrvPt { curve, point },
            xsect_tol_3d,
            this_ivl,
            sample_cnt_u,
        )
    }

    /// `SmCrvCrvGapFunction(rXSectTol3d, pThisCurve, rThisIvl, pOtherCurve,
    /// lSampleCntU)` (upstream default count `20`).
    #[must_use]
    pub fn crv_crv(
        xsect_tol_3d: f64,
        curve: &'a Curve,
        this_ivl: &Extent1d,
        other: &'a Curve,
        sample_cnt_u: usize,
    ) -> Self {
        Self::curve_based(
            GapGeometry::CrvCrv { curve, other },
            xsect_tol_3d,
            this_ivl,
            sample_cnt_u,
        )
    }

    /// `SmCrvSrfGapFunction(rXSectTol3d, pThisCurve, rThisIvl, pOtherSurface,
    /// lSampleCntU)` (upstream default count `20`).
    #[must_use]
    pub fn crv_srf(
        xsect_tol_3d: f64,
        curve: &'a Curve,
        this_ivl: &Extent1d,
        other: &'a Surface,
        sample_cnt_u: usize,
    ) -> Self {
        Self::curve_based(
            GapGeometry::CrvSrf { curve, other },
            xsect_tol_3d,
            this_ivl,
            sample_cnt_u,
        )
    }

    /// `SmSrfPtGapFunction(rXSectTol3d, pThisSurface, rThisDomain, pOtherPoint,
    /// lSampleCntU)` (one sample in `v`; upstream default count `20`).
    #[must_use]
    pub fn srf_pt(
        xsect_tol_3d: f64,
        surface: &'a Surface,
        this_domain: &Extent2d,
        point: Point3,
        sample_cnt_u: usize,
    ) -> Self {
        Self::base(
            GapGeometry::SrfPt { surface, point },
            xsect_tol_3d,
            this_domain.u_interval(),
            this_domain.v_interval(),
            sample_cnt_u,
            1,
        )
    }

    /// `SmSrfCrvGapFunction(rXSectTol3d, pThisSurface, rThisDomain, pOtherCurve,
    /// lSampleCntU)` (one sample in `v`; upstream default count `20`).
    #[must_use]
    pub fn srf_crv(
        xsect_tol_3d: f64,
        surface: &'a Surface,
        this_domain: &Extent2d,
        other: &'a Curve,
        sample_cnt_u: usize,
    ) -> Self {
        Self::base(
            GapGeometry::SrfCrv { surface, other },
            xsect_tol_3d,
            this_domain.u_interval(),
            this_domain.v_interval(),
            sample_cnt_u,
            1,
        )
    }

    /// `SmSrfSrfGapFunction(rXSectTol3d, pThisSurface, rThisDomain, pOtherSurface,
    /// lSampleCntU, lSampleCntV)` (upstream default counts `20`, `20`).
    #[must_use]
    pub fn srf_srf(
        xsect_tol_3d: f64,
        surface: &'a Surface,
        this_domain: &Extent2d,
        other: &'a Surface,
        sample_cnt_u: usize,
        sample_cnt_v: usize,
    ) -> Self {
        Self::base(
            GapGeometry::SrfSrf { surface, other },
            xsect_tol_3d,
            this_domain.u_interval(),
            this_domain.v_interval(),
            sample_cnt_u,
            sample_cnt_v,
        )
    }

    // ---- simple access ------------------------------------------------------------

    /// The shapes.
    #[must_use]
    pub const fn geometry(&self) -> &GapGeometry<'a> {
        &self.geometry
    }

    /// `GetSampleType`.
    #[must_use]
    pub const fn sample_type(&self) -> GapSampleType {
        self.geometry.sample_type()
    }

    /// `GetXSectTol3d` (and the obsolete `GetTol`).
    #[must_use]
    pub const fn xsect_tol_3d(&self) -> f64 {
        self.xsect_tol_3d
    }

    /// `SetXSectTol3d`: a different tolerance invalidates the samples.
    pub fn set_xsect_tol_3d(&mut self, xsect_tol_3d: f64) {
        if xsect_tol_3d != self.xsect_tol_3d {
            self.xsect_tol_3d = xsect_tol_3d;
            self.samples_set = SamplesState::NotSet;
            self.local_neighborhood_set = false;
        }
    }

    /// `GetSampleCntU`.
    #[must_use]
    pub const fn sample_cnt_u(&self) -> usize {
        self.sample_cnt_u
    }

    /// `GetSampleCntV`.
    #[must_use]
    pub const fn sample_cnt_v(&self) -> usize {
        self.sample_cnt_v
    }

    /// `AreSamplesSet`.
    #[must_use]
    pub const fn samples_state(&self) -> SamplesState {
        self.samples_set
    }

    /// The samples computed so far (`m_sSampleGaps`, row major; slots not yet
    /// evaluated are [`GapSample::ZERO`]).
    #[must_use]
    pub fn samples(&self) -> &[GapSample] {
        &self.samples
    }

    /// `m_lMaxSampleIndex`.
    #[must_use]
    pub const fn max_sample_index(&self) -> usize {
        self.max_sample_index
    }

    /// `m_lMinSampleIndex`.
    #[must_use]
    pub const fn min_sample_index(&self) -> usize {
        self.min_sample_index
    }

    /// `GetAllNormalGaps` (`None` = `UNSURE`, before any build).
    #[must_use]
    pub const fn all_normal_gaps(&self) -> Option<bool> {
        self.all_normal_gaps
    }

    /// `GetOtherIvlU`: the other parameters' `u` range over the samples.
    #[must_use]
    pub const fn other_ivl_u(&self) -> &Extent1d {
        &self.other_ivl_u
    }

    /// `GetOtherIvlV`.
    #[must_use]
    pub const fn other_ivl_v(&self) -> &Extent1d {
        &self.other_ivl_v
    }

    /// `GetThisIvlU`.
    #[must_use]
    pub const fn this_ivl_u(&self) -> &Extent1d {
        &self.sample_ivl_u
    }

    /// `GetThisIvlV`.
    #[must_use]
    pub const fn this_ivl_v(&self) -> &Extent1d {
        &self.sample_ivl_v
    }

    /// `GetThisCurve`.
    #[must_use]
    pub const fn this_curve(&self) -> Option<&'a Curve> {
        match self.geometry {
            GapGeometry::CrvPt { curve, .. }
            | GapGeometry::CrvCrv { curve, .. }
            | GapGeometry::CrvSrf { curve, .. } => Some(curve),
            _ => None,
        }
    }

    /// `GetThisSurface`.
    #[must_use]
    pub const fn this_surface(&self) -> Option<&'a Surface> {
        match self.geometry {
            GapGeometry::SrfPt { surface, .. }
            | GapGeometry::SrfCrv { surface, .. }
            | GapGeometry::SrfSrf { surface, .. } => Some(surface),
            _ => None,
        }
    }

    /// `GetOtherCurve`.
    #[must_use]
    pub const fn other_curve(&self) -> Option<&'a Curve> {
        match self.geometry {
            GapGeometry::CrvCrv { other, .. } | GapGeometry::SrfCrv { other, .. } => Some(other),
            _ => None,
        }
    }

    /// `GetOtherSurface`.
    #[must_use]
    pub const fn other_surface(&self) -> Option<&'a Surface> {
        match self.geometry {
            GapGeometry::CrvSrf { other, .. } | GapGeometry::SrfSrf { other, .. } => Some(other),
            _ => None,
        }
    }

    /// `GetOtherPoint`.
    #[must_use]
    pub const fn other_point(&self) -> Option<Point3> {
        match self.geometry {
            GapGeometry::CrvPt { point, .. } | GapGeometry::SrfPt { point, .. } => Some(point),
            _ => None,
        }
    }

    /// `SetSampleCnt(lSampleCntU, lSampleCntV)` (upstream defaults `20`, `1`): a
    /// changed count invalidates the samples; curve-type functions keep one sample in
    /// `v`.
    pub fn set_sample_cnt(&mut self, sample_cnt_u: usize, sample_cnt_v: usize) {
        // when CntU needs changing
        if self.sample_cnt_u != sample_cnt_u {
            self.sample_cnt_u = sample_cnt_u;
            self.samples_set = SamplesState::NotSet;
        }

        // When GapFunction has a Curve ThisGeometry - set CntV = 1
        if self.sample_type().is_curve() {
            self.sample_cnt_v = 1;
        }
        // else when CntV needs changing
        else if self.sample_cnt_v != sample_cnt_v {
            self.sample_cnt_v = sample_cnt_v;
            self.samples_set = SamplesState::NotSet;
        }
    }

    // ---- samples ------------------------------------------------------------------

    /// `GetMaxIndexSample`: builds the samples (up to the first gap larger than the
    /// tolerance when `stop_after_big_gap`) and returns the index of the longest.
    ///
    /// # Errors
    ///
    /// Those of [`GapFunction::build_sample_set`].
    pub fn max_index_sample(&mut self, k: &Kernel, stop_after_big_gap: bool) -> Result<usize> {
        self.build_sample_set(k, stop_after_big_gap)?;
        Ok(self.max_sample_index)
    }

    /// `GetMaxGapSample`: builds the samples and returns the longest.
    ///
    /// # Errors
    ///
    /// Those of [`GapFunction::build_sample_set`].
    pub fn max_gap_sample(&mut self, k: &Kernel, stop_after_big_gap: bool) -> Result<&GapSample> {
        let i = self.max_index_sample(k, stop_after_big_gap)?;
        self.samples
            .get(i)
            .ok_or_else(|| SolidError::invalid_input(OP_GF, "no gap samples"))
    }

    /// The normalised sample parameters `[u0 .. un v0 .. vm]` of `BuildSampleSet`,
    /// adjusting the sample counts of B-spline geometry so that the ends and center of
    /// every knot interval are sampled.
    fn sample_normalized_values(&mut self, k: &Kernel) -> Result<Vec<f64>> {
        let this_curve = self.this_curve();
        let this_surface = self.this_surface();
        let is_bspline = this_curve.is_some_and(|c| c.is_kind_of(BSPLINE_CURVE_TYPE))
            || match this_surface {
                Some(s) => s.is_kind_of(B_SPLINE_SURFACE_TYPE)?,
                None => false,
            };
        if self.sample_cnt_u == 0 || self.sample_cnt_v == 0 {
            return Err(SolidError::invalid_input(OP_GF, "zero sample count"));
        }

        let cnt = |u: usize, v: usize| -> Result<usize> {
            let n = u.checked_add(v).ok_or_else(too_many)?;
            if u.checked_mul(v).is_none_or(|t| t > MAX_SAMPLES) {
                return Err(too_many());
            }
            Ok(n)
        };

        // When ThisGeometry is BSpline geometry adjust sample counts and sample UV
        // values to make sure the ends and center of every knot span are sampled
        if !is_bspline {
            // This Geometry is not a BSpline - even stride sampling
            let (cu, cv) = (self.sample_cnt_u, self.sample_cnt_v);
            let mut vals = vec![0.0; cnt(cu, cv)?];

            // boundary values - set to avoid numerical tolerances
            set_at(&mut vals, 0, 0.0)?;
            set_at(&mut vals, cu - 1, 1.0)?;
            set_at(&mut vals, cu, 0.0)?;
            if cv > 1 {
                set_at(&mut vals, cu + cv - 1, 1.0)?;
            }

            // for every internal U value - NaturalInterval even stride sampling
            // (upstream: for(ii=2,jj=1;ii<CntU;ii++,jj++))
            for jj in 1..cu.saturating_sub(1) {
                set_at(&mut vals, jj, jj as f64 / (cu - 1) as f64)?;
            }

            // for every internal V value (upstream divides the array index, which
            // includes the U count)
            // (upstream: for(ii=2,jj=CntU+1;ii<CntV;ii++,jj++))
            for jj in cu + 1..(cu + cv).saturating_sub(1) {
                set_at(&mut vals, jj, jj as f64 / (cv - 1) as f64)?;
            }
            return Ok(vals);
        }

        // Get Spline U (and V) Knots
        let (mut knots_u, mut knots_v) = match (this_curve, this_surface) {
            (Some(c), _) => (unique_knots(c.knots(k, None))?, vec![0.0]),
            (None, Some(s)) => (
                unique_knots(s.knots_with_nurb(k, SurfParamType::U, None))?,
                unique_knots(s.knots_with_nurb(k, SurfParamType::V, None))?,
            ),
            (None, None) => return Err(SolidError::invalid_input(OP_GF, "no this geometry")),
        };

        // Get UniqueKnots index values for the sample interval end points
        let start_u = add_unique_get_ordered_index(&mut knots_u, self.sample_ivl_u.min());
        let end_u = add_unique_get_ordered_index(&mut knots_u, self.sample_ivl_u.max());
        let (mut start_v, mut end_v) = (0, 0);
        if this_surface.is_some() {
            start_v = add_unique_get_ordered_index(&mut knots_v, self.sample_ivl_v.min());
            end_v = add_unique_get_ordered_index(&mut knots_v, self.sample_ivl_v.max());
        }

        // There should always be 1 or more intervals
        if !(end_u > start_u && (this_surface.is_none() || end_v > start_v)) {
            return Err(SolidError::Upstream {
                operation: OP_GF,
                code: SM_ERR_ASSERT_FAILURE,
                entity: None,
            });
        }
        let ivl_cnt_u = end_u - start_u;
        let ivl_cnt_v = end_v - start_v;

        // U dir interval sample counts
        let per_u = samples_per_interval(self.sample_cnt_u, ivl_cnt_u)?;
        self.sample_cnt_u = per_u
            .checked_mul(ivl_cnt_u)
            .and_then(|n| n.checked_add(1))
            .ok_or_else(too_many)?;

        // V dir interval sample counts (surfaces)
        let mut per_v = 1;
        if this_surface.is_some() {
            per_v = samples_per_interval(self.sample_cnt_v, ivl_cnt_v)?;
            self.sample_cnt_v = per_v
                .checked_mul(ivl_cnt_v)
                .and_then(|n| n.checked_add(1))
                .ok_or_else(too_many)?;
        }
        let (cu, cv) = (self.sample_cnt_u, self.sample_cnt_v);
        let mut vals = vec![0.0; cnt(cu, cv)?];

        // ParamLength of U and V domains
        let ku0 = at(&knots_u, 0)?;
        let len_u = knots_u.last().copied().unwrap_or(0.0) - ku0;
        let mut len_v = 1.0;

        // boundary normalized values - set to avoid numerical tolerances
        set_at(&mut vals, 0, (at(&knots_u, start_u)? - ku0) / len_u)?;
        set_at(&mut vals, cu - 1, (at(&knots_u, end_u)? - ku0) / len_u)?;
        set_at(&mut vals, cu, 0.0)?;
        let kv0 = at(&knots_v, 0)?;
        if this_surface.is_some() {
            len_v = knots_v.last().copied().unwrap_or(0.0) - kv0;
            set_at(&mut vals, cu, (at(&knots_v, start_v)? - kv0) / len_v)?;
            set_at(&mut vals, cu + cv - 1, (at(&knots_v, end_v)? - kv0) / len_v)?;
        }

        // for every U BSpline Span (upstream walks the spans from the first knot)
        let mut jcnt = 1usize;
        for ii in 0..ivl_cnt_u {
            let start_p = at(&knots_u, ii)?;
            let end_p = at(&knots_u, ii + 1)?;
            let inc = (end_p - start_p) / per_u as f64;
            let mut val = start_p + inc;
            for _ in 0..per_u {
                set_at(&mut vals, jcnt, (val - ku0) / len_u)?;
                jcnt += 1;
                val += inc;
            }
        }

        // when working with a surface
        if this_surface.is_some() {
            // skip V Start Point value
            jcnt += 1;
            for ii in 0..ivl_cnt_v {
                let start_p = at(&knots_v, ii)?;
                let end_p = at(&knots_v, ii + 1)?;
                let inc = (end_p - start_p) / per_v as f64;
                let mut val = start_p + inc;
                for _ in 0..per_v {
                    set_at(&mut vals, jcnt, (val - kv0) / len_v)?;
                    jcnt += 1;
                    val += inc;
                }
            }
        }
        Ok(vals)
    }

    /// `BuildSampleSet`: evaluates the grid of gap samples (row major, `v` rows of
    /// `u` samples, each drop guessed from the previous sample), the longest and
    /// shortest, whether all gaps are normal and the range of the other parameters.
    /// Does nothing when the samples are already (partly) set. With
    /// `stop_after_big_gap` it stops at the first gap larger than the tolerance.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a zero sample count or more than
    /// [`MAX_SAMPLES`] samples; `SM_ERR_ASSERT_FAILURE` when the sample interval of
    /// B-spline geometry spans no knot interval; [`SolidError::Unported`] from
    /// [`GapFunction::evaluate`].
    pub fn build_sample_set(&mut self, k: &Kernel, stop_after_big_gap: bool) -> Result<()> {
        // no work - samples already set or already have a gap bigger than tol
        if self.samples_set != SamplesState::NotSet {
            return Ok(());
        }

        let vals = self.sample_normalized_values(k)?;
        let (cu, cv) = (self.sample_cnt_u, self.sample_cnt_v);

        // Set memory in m_sSampleGaps (SetSize: new slots zeroed)
        let n = cu.checked_mul(cv).ok_or_else(too_many)?;
        let mut samples = core::mem::take(&mut self.samples);
        samples.truncate(n);
        samples.resize(n, GapSample::ZERO);

        let r = self.evaluate_samples(k, &vals, &mut samples, stop_after_big_gap);
        self.samples = samples;
        r
    }

    /// The sample loop of `BuildSampleSet`.
    fn evaluate_samples(
        &mut self,
        k: &Kernel,
        vals: &[f64],
        samples: &mut [GapSample],
        stop_after_big_gap: bool,
    ) -> Result<()> {
        let (cu, cv) = (self.sample_cnt_u, self.sample_cnt_v);
        let use_v = matches!(
            self.sample_type(),
            GapSampleType::CrvSrf | GapSampleType::SrfSrf
        );
        let mut last: Option<usize> = None;

        // iteration statistics
        let mut min_gap = BIG_DOUBLE;
        let mut max_gap = -BIG_DOUBLE;
        self.all_normal_gaps = Some(true);
        self.other_ivl_u.init();
        self.other_ivl_v.init();

        // for every sample point row
        for jj in 0..cv {
            if jj > 0 {
                last = Some((jj - 1) * cu);
            }
            let this_v = self.sample_ivl_v.evaluate(at(vals, cu + jj)?);
            let row = jj * cu;

            // for every sample point col entry
            for ii in 0..cu {
                if ii > 0 {
                    last = Some(row + ii - 1);
                }
                let this_u = self.sample_ivl_u.evaluate(at(vals, ii)?);

                // the previous sample's other parameters guess this drop
                let prev = match last {
                    Some(i) => Some(
                        *samples
                            .get(i)
                            .ok_or_else(|| SolidError::invalid_input(OP_GF, "bad sample"))?,
                    ),
                    None => None,
                };
                let guess_u = prev.map(|s| s.other_param.x);
                let guess_v = if use_v {
                    prev.map(|s| s.other_param.y)
                } else {
                    None
                };

                let idx = row + ii;
                let slot = samples
                    .get_mut(idx)
                    .ok_or_else(|| SolidError::invalid_input(OP_GF, "bad sample"))?;
                self.evaluate(k, this_u, this_v, slot, guess_u, guess_v)?;
                let s = *slot;

                // note when a NonNormal Gap was found
                if s.drop_type != GapDropType::Normal {
                    self.all_normal_gaps = Some(false);
                }

                // accumulate the drop Domain when the other object is a curve or surface
                if self.other_point().is_none() {
                    self.other_ivl_u.add_value(s.other_param.x);
                    if self.other_surface().is_some() {
                        self.other_ivl_v.add_value(s.other_param.y);
                    }
                }

                // save min and max indices
                let gap = s.length();
                if gap > max_gap {
                    max_gap = gap;
                    self.max_sample_index = idx;
                }
                if gap < min_gap {
                    min_gap = gap;
                    self.min_sample_index = idx;
                }

                // when asked - quit at the 1st out of tol gap
                if stop_after_big_gap && gap > self.xsect_tol_3d {
                    self.samples_set = SamplesState::UntilBigGap;
                    return Ok(());
                }
            }
        }

        // remember the work
        self.samples_set = SamplesState::Set;
        Ok(())
    }

    /// `Evaluate`: the gap from this shape at `(this_u, this_v)` (`this_v` only for
    /// surfaces) to the other shape, written into `sample` (`SetCanonical`). Drops to
    /// a curve or surface start from the guesses when given (a surface needs both)
    /// and fall back to a global drop when that fails or lands too far away or on the
    /// boundary out of tolerance; a failed drop gives a zero gap of type
    /// [`GapDropType::NoDrop`].
    ///
    /// # Errors
    ///
    /// Those of the geometry evaluations and drops. Unported calls propagate; other
    /// failures are ignored, as upstream.
    pub fn evaluate(
        &self,
        k: &Kernel,
        this_u: f64,
        this_v: f64,
        sample: &mut GapSample,
        guess_u: Option<f64>,
        guess_v: Option<f64>,
    ) -> Result<()> {
        let tol = self.xsect_tol_3d;
        match self.geometry {
            GapGeometry::CrvPt { curve, point } => {
                let this_pos = point_or_undef(curve.evaluate_point(k, this_u))?;
                sample.set_canonical(
                    this_pos,
                    this_u,
                    0.0,
                    point,
                    0.0,
                    0.0,
                    GapDropType::Normal,
                    tol,
                );
            }
            GapGeometry::SrfPt { surface, point } => {
                let this_pos =
                    point_or_undef(surface.evaluate_point(k, Point2::new(this_u, this_v)))?;
                sample.set_canonical(
                    this_pos,
                    this_u,
                    this_v,
                    point,
                    0.0,
                    0.0,
                    GapDropType::Normal,
                    tol,
                );
            }
            GapGeometry::CrvCrv { curve, other } => {
                let this_pos = point_or_undef(curve.evaluate_point(k, this_u))?;
                self.evaluate_to_curve(k, this_pos, this_u, 0.0, other, sample, guess_u)?;
            }
            GapGeometry::SrfCrv { surface, other } => {
                let this_pos =
                    point_or_undef(surface.evaluate_point(k, Point2::new(this_u, this_v)))?;
                self.evaluate_to_curve(k, this_pos, this_u, this_v, other, sample, guess_u)?;
            }
            GapGeometry::CrvSrf { curve, other } => {
                let this_pos = point_or_undef(curve.evaluate_point(k, this_u))?;
                self.evaluate_to_surface(
                    k, this_pos, this_u, 0.0, other, sample, guess_u, guess_v,
                )?;
            }
            GapGeometry::SrfSrf { surface, other } => {
                let this_pos =
                    point_or_undef(surface.evaluate_point(k, Point2::new(this_u, this_v)))?;
                self.evaluate_to_surface(
                    k, this_pos, this_u, this_v, other, sample, guess_u, guess_v,
                )?;
            }
        }
        Ok(())
    }

    /// The drop to the other curve of `SmCrvCrvGapFunction::Evaluate` and
    /// `SmSrfCrvGapFunction::Evaluate`.
    #[allow(clippy::too_many_arguments)]
    fn evaluate_to_curve(
        &self,
        k: &Kernel,
        this_pos: Point3,
        this_u: f64,
        this_v: f64,
        other: &Curve,
        sample: &mut GapSample,
        guess_u: Option<f64>,
    ) -> Result<()> {
        let tol = self.xsect_tol_3d;
        // Drop point to ToGeometry (GetTol(): distance between close solutions)
        let dist_tol = self.xsect_tol_3d;
        let domain = other.natural_interval();
        let mut d = curve_drop(k, other, &domain, this_pos, dist_tol, guess_u)?;

        // If we don't like the result of the drop with a guess, try without a guess
        let try_global = guess_u.is_some()
            && (!d.success
                || d.distance > tol * 100.0
                || (d.distance > tol && domain.is_value_on_boundary(d.parameter, EFF_ZERO).0));
        if try_global {
            d = curve_drop(k, other, &domain, this_pos, dist_tol, None)?;
        }

        // set output
        if d.success {
            // Other Curve dropped point Eval
            let pd = curve_point_tangent(k, other, d.parameter)?;
            let [p0, p1] = pd;
            let gap = p0 - this_pos;
            let drop_type = if !domain.is_value_on_boundary(d.parameter, EFF_ZERO).0
                || gap.length_squared() < EFF_ZERO_SQ
                || p1.is_perpendicular_to(gap, 0.5)
            {
                GapDropType::Normal
            } else {
                GapDropType::Boundary
            };
            sample.set_canonical(
                this_pos,
                this_u,
                this_v,
                p0,
                d.parameter,
                0.0,
                drop_type,
                tol,
            );
        } else {
            // failed to drop - load a zero length gap vector
            sample.set_canonical(
                this_pos,
                this_u,
                this_v,
                this_pos,
                other.natural_interval().min(),
                0.0,
                GapDropType::NoDrop,
                tol,
            );
        }
        Ok(())
    }

    /// The drop to the other surface of `SmCrvSrfGapFunction::Evaluate` and
    /// `SmSrfSrfGapFunction::Evaluate`.
    #[allow(clippy::too_many_arguments)]
    fn evaluate_to_surface(
        &self,
        k: &Kernel,
        this_pos: Point3,
        this_u: f64,
        this_v: f64,
        other: &Surface,
        sample: &mut GapSample,
        guess_u: Option<f64>,
        guess_v: Option<f64>,
    ) -> Result<()> {
        let tol = self.xsect_tol_3d;
        // Drop point to ToGeometry
        let guess = match (guess_u, guess_v) {
            (Some(u), Some(v)) => Some(Point2::new(u, v)),
            _ => None,
        };
        let domain = other.natural_uv_domain_with_nurb(k)?;
        let (mut success, mut uv, mut dist) = surface_drop(k, other, this_pos, &domain, guess)?;

        // If we don't like the result of the drop with a guess, try without a guess
        let try_global = guess.is_some()
            && (!success
                || dist > tol * 100.0
                || (dist > tol && domain.is_point_on_boundary(uv, EFF_ZERO).0));
        if try_global {
            (success, uv, dist) = surface_drop(k, other, this_pos, &domain, None)?;
        }
        let _ = dist;

        // set output
        if success {
            // Other Surface dropped point Eval
            let [p, du, dv] = surface_point_partials(k, other, uv)?;
            let gap = p - this_pos;
            let drop_type = if !domain.is_point_on_boundary(uv, EFF_ZERO).0
                || gap.length_squared() < EFF_ZERO_SQ
                || (du.is_perpendicular_to(gap, 0.5) && dv.is_perpendicular_to(gap, 0.5))
            {
                GapDropType::Normal
            } else {
                GapDropType::Boundary
            };
            sample.set_canonical(this_pos, this_u, this_v, p, uv.x, uv.y, drop_type, tol);
        } else {
            // failed to drop - load a zero length gap vector
            let min = other.natural_uv_domain_with_nurb(k)?.min();
            sample.set_canonical(
                this_pos,
                this_u,
                this_v,
                this_pos,
                min.x,
                min.y,
                GapDropType::NoDrop,
                tol,
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::curve::line::Line;

    #[test]
    fn reset_states() {
        let mut g = EdgeFaceGap::new();
        assert!(!g.is_init());
        g.set_edgeuse(Some(EdgeuseId::from_index(3)));
        g.gap_3d = 1.0;
        assert!(g.is_init());
        g.reset();
        assert_eq!(g, EdgeFaceGap::default());

        let mut e = EdgeEdgeGap::new();
        e.edgeuse0 = Some(EdgeuseId::from_index(0));
        assert!(!e.is_init());
        e.set_edgeuse(Some(EdgeuseId::from_index(1)), 1);
        assert!(e.is_init());
        e.reset();
        assert!(!e.is_init());

        let mut v = VertexFaceGap::new();
        v.vertexuse = Some(VertexuseId::from_index(2));
        assert!(v.is_init());
        v.reset();
        assert_eq!(v, VertexFaceGap::default());

        let mut ve = VertexEdgeGap::new();
        ve.vertexuse = Some(VertexuseId::from_index(1));
        ve.edge_t = 0.5;
        assert!(ve.is_init());
        ve.reset();
        assert_eq!(ve, VertexEdgeGap::default());
    }

    #[test]
    fn ordered_index() {
        let mut a = Vec::new();
        assert_eq!(add_unique_get_ordered_index(&mut a, 1.0), 0);
        assert_eq!(add_unique_get_ordered_index(&mut a, 0.0), 0);
        assert_eq!(add_unique_get_ordered_index(&mut a, 3.0), 2);
        assert_eq!(add_unique_get_ordered_index(&mut a, 2.0), 2);
        assert_eq!(add_unique_get_ordered_index(&mut a, 2.0 + 1e-9), 2);
        assert_eq!(add_unique_get_ordered_index(&mut a, 3.0 - 1e-9), 3);
        assert_eq!(a, vec![0.0, 1.0, 2.0, 3.0]);
        assert_eq!(add_unique_get_ordered_index(&mut a, f64::NAN), 0);
    }

    #[test]
    fn tol_point() {
        let mut s = GapSample::default();
        s.set_canonical(
            Vec3::ZERO,
            0.0,
            0.0,
            Vec3::new(1.0, 0.0, 0.0),
            0.0,
            0.0,
            GapDropType::Normal,
            0.25,
        );
        assert!(!s.is_in_tol());
        assert!((s.tol_point().x - 0.25).abs() < 1e-15);
        assert_eq!(s.length(), 1.0);
        s.set_canonical(
            Vec3::ZERO,
            0.0,
            0.0,
            Vec3::ZERO,
            0.0,
            0.0,
            GapDropType::Normal,
            0.0,
        );
        assert!(s.is_in_tol());
        assert_eq!(s.xsect_tol_3d(), XSECT_TOL_3D);
        assert!(!s.tol_point().is_initialized());
    }

    #[test]
    fn gap_array_tracks_longest_added() {
        let g = |l: f64| {
            Gap::from(VertexEdgeGap {
                gap_3d: l,
                ..VertexEdgeGap::new()
            })
        };
        let mut a = GapArray::new();
        assert_eq!(a.max_gap_length(), 0.0);
        a.add(g(1.0));
        a.add(g(3.0));
        a.add(g(2.0));
        assert_eq!(a.max_gap_index(), Some(1));
        let mut b = GapArray::new();
        b.add(g(0.5));
        b.append(&a);
        assert_eq!(b.len(), 4);
        assert_eq!(b.max_gap_index(), Some(0));
        b.reset();
        assert!(b.is_empty());
        assert!(g(1.0).is_less(&g(2.0)));
        let mut x = g(1.0);
        x.assign(&Gap::from(EdgeFaceGap::new()));
        assert_eq!(x.length(), -UNDEF_DOUBLE);
        assert!(x.is_kind_of(types::GAP) && x.is_kind_of(types::VERTEX_EDGE_GAP));
    }

    #[test]
    fn curve_point_samples() {
        let k = Kernel::default();
        let line = Curve::from(Line::from_points(Vec3::ZERO, Vec3::new(1.0, 0.0, 0.0), 3));
        let ivl = line.natural_interval();
        let mut f = GapFunction::crv_pt(1e-3, &line, &ivl, Vec3::new(0.0, 1.0, 0.0), 5);
        let max = *f.max_gap_sample(&k, false).unwrap();
        assert_eq!(f.samples_state(), SamplesState::Set);
        assert!(f.samples().len() >= 5);
        assert!(max.length() >= 1.0);
        assert_eq!(f.all_normal_gaps(), Some(true));
    }

    #[test]
    fn vertex_edge_gap_on_a_wire() {
        let mut b = Brep::new();
        let v0 = b.add_vertex(Vec3::new(0.0, 0.0, 1e-3)).unwrap();
        let v1 = b.add_vertex(Vec3::new(1.0, 0.0, 0.0)).unwrap();
        let e = b.add_edge(v0, v1).unwrap();
        b.edge_mut(e).unwrap().curve = Some(Curve::from(Line::from_points(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            3,
        )));
        let eu = b
            .add_edgeuse_record(crate::topology::Edgeuse::default())
            .unwrap();
        b.edgeuse_mut(eu).unwrap().edge = Some(e);
        let vu = b
            .add_vertexuse_record(crate::topology::Vertexuse {
                vertex: Some(v0),
                owner: crate::topology::VertexuseOwner::Edgeuse(Some(eu)),
                ..crate::topology::Vertexuse::default()
            })
            .unwrap();
        let k = Kernel::default();
        let g = VertexEdgeGap::with(&k, &mut b, Some(vu), 0.0).unwrap();
        assert!((g.gap_3d - 1e-3).abs() < 1e-15);
        let (p, q) = Gap::from(g).gap_end_points(&k, &mut b).unwrap();
        assert_eq!(p, Vec3::new(0.0, 0.0, 1e-3));
        assert_eq!(q, Vec3::ZERO);
    }
}
