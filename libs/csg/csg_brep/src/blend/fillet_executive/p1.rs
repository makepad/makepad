//! Part 1 of `SmFilletExecutive`: the fillet laws (the radius and its derivatives
//! along a filleted edge, constant, linear or given by a B-spline), the error info and
//! the executive object.
//!
//! Upstream: `source/SMLib/src/SmFilletExecutive.cpp` (lines 30-292, 488-523,
//! 1601-1881, 3979-4120, 4909-4929, 5126-6132), `source/SMLib/inc/SmFilletExecutive.h`.
//!
//! The law type itself ([`FilletLaw`], [`FilletLawKind`]) was CO-PORTED by unit
//! `blend.fillet_standard_solver_p1` into
//! [`crate::blend::fillet_standard_solver::p1`]; it is extended there (edge map, arc
//! length, cached extension) and re-exported here. This module holds its behaviour.
//!
//! | Rust                                   | Upstream                                              |
//! |----------------------------------------|-------------------------------------------------------|
//! | [`FilletLaw::new`] (co-port)           | `SmFilletLaw::SmFilletLaw`, `~SmFilletLaw`, `~SmBSplineFilletLaw` (`Drop`) |
//! | [`FilletLaw::evaluate`]                | `SmFilletLaw::Evaluate`, `SmLinearFilletLaw::Evaluate`, `SmBSplineFilletLaw::Evaluate` |
//! | [`FilletLaw::is_linear_law`] (co-port) | `SmFilletLaw::IsLinearLaw`, `SmLinearFilletLaw::IsLinearLaw`, `SmBSplineFilletLaw::IsLinearLaw` |
//! | [`FilletLaw::law_curve`]               | `SmBSplineFilletLaw::GetLawCurve`                     |
//! | [`FilletLaw::extended_curve`]          | `SmBSplineFilletLaw::GetExtendedCurve`                |
//! | field `FilletLawKind::BSpline::law_curve` / `extended_law` | `SetLawCurve`, `SetExtendedCurve`     |
//! | field `FilletLawKind::Linear::start_radius` / `end_radius` | `Get/SetStartRadius`, `Get/SetEndRadius` |
//! | field `FilletLaw::edge_arc_length`     | `SmFilletLaw::SetEdgeArcLength`                       |
//! | [`FilletLaw::extended_edge_curve`], [`FilletLaw::set_extended_edge_curve`] (co-port) | `GetExtendedEdgeCurve`, `SetExtendedEdgeCurve` |
//! | [`FilletLaw::type_code`], [`FilletLaw::type_string`] | `GetType`, `GetTypeString` of the three classes |
//! | [`FilletLaw::is_kind_of`]              | `IsKindOf` of the three classes                       |
//! | [`FILLET_LAW_TYPE`], [`LINEAR_FILLET_LAW_TYPE`], [`BSPLINE_FILLET_LAW_TYPE`] | `SmFilletLaw_TYPE`, `SmLinearFilletLaw_TYPE`, `SmBSplineFilletLaw_TYPE` |
//!
//! The `SmLinearFilletLaw` and `SmBSplineFilletLaw` constructors are unit
//! `blend.fillet_executive_p2`'s.
//!
//! The executive's member data ([`FilletExecutive`], [`ErrorInfo`]) was CO-PORTED by
//! unit `blend.fillet_executive_p3` and is extended there; this module holds the
//! constructor and the members of this part.
//!
//! | Rust                                   | Upstream                                              |
//! |----------------------------------------|-------------------------------------------------------|
//! | [`ErrorInfo::new`], `ErrorInfo::default` (co-port) | `SmFilletErrorInfo(pFilletExec)`, `SmFilletErrorInfo()`, `set()` |
//! | [`ErrorInfo::note_error`], [`ErrorInfo::note_edge_error`], [`ErrorInfo::note_vertex_error`], [`ErrorInfo::note_merge_error`], [`ErrorInfo::note_corner_error`], [`ErrorInfo::note_fillet_edge_error`], [`ErrorInfo::note_intersection_error`] | the seven `SmFilletErrorInfo::NoteError` overloads (cpp:117, 131, 150, 173, 199, 231, 256) |
//! | `ErrorInfo::assign` (co-port)          | `SmFilletErrorInfo::operator=`                        |
//! | [`FilletExecutive::new`], [`ExecutiveObjects`] | `SmFilletExecutive::SmFilletExecutive`        |
//! | [`FilletExecutive::load_fillet_solver`] | `LoadFilletSolver` |
//! | [`FilletExecutive::create_fillet_corners`] | `CreateFilletCorners` |
//! | [`FilletExecutive::create_smooth_edge_sequence`] | `CreateSmoothEdgeSequence` |
//! | [`remove_topo_edges`]                  | `RemoveTopoEdges`                                     |
//! | [`FilletExecutive::set_bevel_corner`]  | `SetBevelCorner`                                      |
//! | [`FilletExecutive::add_fillet_surface_generator`] | `AddFilletSurfaceGenerator`                |
//! | [`FilletExecutive::remove_from_topo_edges`] | `RemoveFromTopoEdges`                            |
//! | [`FilletExecutive::set_do_global_merge`], [`FilletExecutive::set_do_trimming`], [`FilletExecutive::set_self_intersection_handler`] | `SetDoGlobalMerge`, `SetDoTrimming`, `SetSelfIntersectionHandler` |
//! | [`FilletExecutive::set_blend_curve_at_thirds`], [`FilletExecutive::set_mark_corner_patches`] | the fork's `SetBlendCurveAtThirds` (source fork `fd3b6a7`), `SetMarkCornerPatches` (`60af574`); off by default |
//! | [`is_radius_supported`]                | `SmFilletExecutive::IsRadiusSupported` (static)       |
//! | fields `fillet`, `pseudo_brep`, `corners`, `solvers`, `error` | `GetFilletBrep`, `GetPseudoBrep`, `GetFilletCorners`, `GetFilletSolvers`, `GetFilletErrorInfo` |
//! | [`FilletExecutive::fillet_corner_of_vertex`] | `GetFilletCornerOfVertex`                       |
//! | [`FilletExecutive::fillet_solver_of_edgeuse`], [`FilletExecutive::fillet_solver_of_edge`] | `GetFilletSolverOfEdgeuse`, `GetFilletSolverOfEdge` |
//! | [`FilletExecutive::note_fillet_error`], [`FilletExecutive::note_fillet_merge_error`], [`FilletExecutive::note_fillet_corner_error`], [`FilletExecutive::note_fillet_intersection_error`] | the four `NoteFilletError` overloads |
//!
//! Deviations:
//! - The class hierarchy is the enum [`FilletLawKind`]; the trivial getters and setters
//!   are public fields.
//! - `Evaluate` is `const` upstream and caches the B-spline extension through a cast
//!   away from `const`; here [`FilletLaw::evaluate`] takes `&mut self` and stores it in
//!   `FilletLawKind::BSpline::extended_law`. The context passed to
//!   `CreateExtendedCurve` only allocates, so no `Kernel` is needed.
//! - The "unable to extend non-B-spline laws" branch is unreachable (both the law curve
//!   and its extension are B-splines) and not ported.
//! - A G1-closed law curve is not extended: `CreateExtendedCurve` hands back an
//!   unextended copy (NLib evaluates such a curve periodically) and the evaluation
//!   clamps, as upstream. `CreateExtendedCurve` returning no curve at all (its
//!   `Ok(None)`: the parameter lies inside the NURB's own bounds, possible only if they
//!   disagreed with `natural_interval`) is a `NULL` dereference upstream and
//!   `SM_ERR_NULL_POINTER` ([`SolidError::null_pointer`]) here.
//! - `GetClassType` / `GetClassTypeString` (the `SM_COMMON` statics) are not ported.
//! - The out-parameter `dValues[3]` is the returned `[f64; 3]`.
//! - Error info: the overloads of `NoteError` and `NoteFilletError` have their own
//!   names. Comments and names are `Option<&str>` (null upstream is `None`); a null one
//!   is printed as glibc's `(null)` (`"%s"` with a null pointer is undefined in C) and
//!   text is cut to 1023 bytes as `snprintf` into the `SM_TBLOCK_SIZE` buffers does, at
//!   a character boundary (upstream may cut inside a UTF-8 sequence; that case is not
//!   compared). The vertex overload of `NoteError` (cpp:131) reads the vertex's number
//!   but stores nothing in the pin; [`ErrorInfo::note_vertex_error`] ports the fork's
//!   recording behaviour, and the executive consults it only under
//!   [`crate::kernel::UpstreamFixes::fillet_vertex_error_diagnostics`] (WR-513, #1469).
//!   The caller supplies the vertex's point (the C++ overload reads it from the vertex).
//!   The codes are typed
//!   ([`FilletErrorType`], [`FilStatus`], [`FilletEdgeType`]) and stored as their
//!   upstream values. `NoteFilletError` on a fillet edge whose type `GetName` writes
//!   no name for is [`SolidError::InvalidInput`]: upstream prints an uninitialised
//!   buffer.
//! - The executive's objects (pseudo B-rep, the fillet-edges attribute, its own
//!   identity) are handles the owner allocates ([`ExecutiveObjects`]). The constructor
//!   leaves `m_bIsInitialized` unset; it is `false` here, the value the first upstream
//!   write (`SetFilletParameters`) gives it. A null surface generator is not
//!   representable (no upstream caller adds one). `GetContext` is
//!   not ported (no context object). The destructor (deleting the owned objects)
//!   waits for the owner's object model. `SetBevelCorner` reaches the corner through
//!   the owner's closure.
//! - `RemoveTopoEdges` is the free function [`remove_topo_edges`] over the target
//!   B-rep and the topo edges (the executive holds handles; its owner resolves them and
//!   calls it from `ExecutiveAccess::remove_topo_edges`). Faces own their surfaces, so
//!   two faces' surfaces differ unless both are null or the faces are the same face; a
//!   face with a surface beside one without is [`SolidError::InvalidInput`] (upstream
//!   dereferences null). The copy replacing a deleted surface keeps the identity that
//!   surface named its face by. An edge or vertex freed by an earlier deletion is
//!   [`SolidError::NotFound`] (upstream reads freed memory). A failed
//!   `DeleteTopologicalVertex` is ignored as upstream does, except
//!   [`SolidError::Unported`] and the port's own [`SolidError::InvalidInput`] /
//!   [`SolidError::NotFound`]. The deletions run with local notification
//!   environments, and the replaced surface is dropped without a destruction
//!   notification: an owner whose attributes (the fillet-edges attribute the topo
//!   edges are users of) must follow the deletions keeps them itself.
//! - No recursion.

use std::collections::BTreeMap;

use crate::blend::fillet_executive::p3::{
    ErrorInfo, ExecutionOptions, ExecutiveAccess, FilletExecutive,
};
use crate::blend::fillet_geom::p1::{FilStatus, FilletCornerType, FilletEdgeType, FilletErrorType};
pub use crate::blend::fillet_standard_solver::p1::{FilletLaw, FilletLawKind};
use crate::boolean::topology_intersector::p1::TopologyIntersector;
use crate::error::EntityRef;
use crate::error::{Result, SolidError};
use crate::geom::extent1d::Extent1d;
use crate::geom::geom_utility::p2::ContinuityType;
use crate::geom::object;
use crate::geom::solution_array::SolutionObject;
use crate::intersect::point_class::ClassObject;
use crate::kernel::Kernel;
use crate::math::{Point3, EFF_ZERO, EFF_ZERO_SQ, EFF_ZERO_SQRT, PI};
use crate::nurbs::b_spline_curve::BSplineCurve;
use crate::surface::surface::Surface;
use crate::topology::brep::{p4 as brep_p4, p5 as brep_p5};
use crate::topology::edge as topo_edge;
use crate::topology::face::{p1 as face_p1, p2 as face_p2};
use crate::topology::{loopuse, vertex, Brep, EdgeId};

const OP_EVAL: &str = "blend::fillet_executive::p1::FilletLaw::evaluate";
const OP_NOTE: &str = "blend::fillet_executive::p1::FilletExecutive::note_fillet_error";

/// `TOPO_BASE_TYPE`.
const TOPO_BASE_TYPE: i64 = 16000;
/// `SmFilletLaw_TYPE` (`TOPO_BASE_TYPE + 39`).
pub const FILLET_LAW_TYPE: i64 = TOPO_BASE_TYPE + 39;
/// `SmLinearFilletLaw_TYPE` (`TOPO_BASE_TYPE + 40`).
pub const LINEAR_FILLET_LAW_TYPE: i64 = TOPO_BASE_TYPE + 40;
/// `SmBSplineFilletLaw_TYPE` (`TOPO_BASE_TYPE + 41`).
pub const BSPLINE_FILLET_LAW_TYPE: i64 = TOPO_BASE_TYPE + 41;

impl FilletLaw {
    /// `GetLawCurve`: the B-spline radius function (`None` for the other kinds).
    #[must_use]
    pub const fn law_curve(&self) -> Option<&BSplineCurve> {
        match &self.kind {
            FilletLawKind::BSpline { law_curve, .. } => Some(law_curve),
            _ => None,
        }
    }

    /// `GetExtendedCurve`: the extension of the law curve the last out-of-domain
    /// evaluation created (`None` before that, and for the other kinds).
    #[must_use]
    pub const fn extended_curve(&self) -> Option<&BSplineCurve> {
        match &self.kind {
            FilletLawKind::BSpline { extended_law, .. } => extended_law.as_ref(),
            _ => None,
        }
    }

    /// `GetType`: the run-time type code of the law's class.
    #[must_use]
    pub const fn type_code(&self) -> i64 {
        match &self.kind {
            FilletLawKind::Base => FILLET_LAW_TYPE,
            FilletLawKind::Linear { .. } => LINEAR_FILLET_LAW_TYPE,
            FilletLawKind::BSpline { .. } => BSPLINE_FILLET_LAW_TYPE,
        }
    }

    /// `GetTypeString`.
    #[must_use]
    pub const fn type_string(&self) -> &'static str {
        match &self.kind {
            FilletLawKind::Base => "SmFilletLaw_TYPE",
            FilletLawKind::Linear { .. } => "SmLinearFilletLaw_TYPE",
            FilletLawKind::BSpline { .. } => "SmBSplineFilletLaw_TYPE",
        }
    }

    /// `IsKindOf`: the law's own type, `SmFilletLaw_TYPE` or `SmObject_TYPE`.
    #[must_use]
    pub const fn is_kind_of(&self, t: i64) -> bool {
        t == self.type_code() || t == FILLET_LAW_TYPE || object::is_kind_of(t)
    }

    /// `Evaluate(dParameter, crCurveInterval, bReverseOrientation, dValues)`: the
    /// fillet radius at `parameter` of the edge interval `interval` and its first two
    /// derivatives with respect to that parameter (`reverse`: the law runs from the
    /// interval's end to its start).
    ///
    /// A linear law evaluates anywhere. A B-spline law maps the parameter through the
    /// edge map onto the law curve; a parameter within `SM_EFF_ZERO * (1 + |min| +
    /// |max|)` outside the curve's domain snaps to the end, a parameter further out
    /// extends the curve (`C-infinity`, to twice the overshoot and at least a tenth of
    /// the domain) and keeps the extension for later evaluations.
    ///
    /// # Errors
    ///
    /// `SM_ERR` for the base law and for a zero-length `interval` of a B-spline law;
    /// the errors of [`BSplineCurve::create_extended_curve`] and
    /// [`BSplineCurve::evaluate`].
    pub fn evaluate(
        &mut self,
        parameter: f64,
        interval: &Extent1d,
        reverse: bool,
    ) -> Result<[f64; 3]> {
        let edge_map = self.edge_map;
        match &mut self.kind {
            FilletLawKind::Base => Err(SolidError::sm_err(OP_EVAL)),
            FilletLawKind::Linear {
                start_radius,
                end_radius,
            } => Ok(linear_evaluate(
                *start_radius,
                *end_radius,
                &edge_map,
                parameter,
                interval,
                reverse,
            )),
            FilletLawKind::BSpline {
                law_curve,
                extended_law,
            } => bspline_evaluate(
                law_curve,
                extended_law,
                &edge_map,
                parameter,
                interval,
                reverse,
            ),
        }
    }
}

/// `SmLinearFilletLaw::Evaluate`.
fn linear_evaluate(
    start_value: f64,
    end_value: f64,
    edge_map: &Extent1d,
    parameter: f64,
    interval: &Extent1d,
    reverse: bool,
) -> [f64; 3] {
    // Map dParameter into domain of Law which is contained in the edge map.
    let d_min = interval.min();
    let d_max = interval.max();
    let t_norm = if reverse {
        (d_max - parameter) / (d_max - d_min)
    } else {
        (parameter - d_min) / (d_max - d_min)
    };
    let t = edge_map.min() + t_norm * (edge_map.max() - edge_map.min());

    // evaluate function, 1st, and 2nd derivatives at param = dParameter
    let value = start_value + t * (end_value - start_value);
    let end_map_value = start_value + edge_map.max() * (end_value - start_value);
    let start_map_value = start_value + edge_map.min() * (end_value - start_value);
    let mut deriv1 = (end_map_value - start_map_value) / interval.length();
    let deriv2 = 0.0;

    // negate 1st derivatives for reversed orientations
    if reverse {
        deriv1 = -deriv1;
    }

    [value, deriv1, deriv2]
}

/// `SmBSplineFilletLaw::Evaluate`.
fn bspline_evaluate(
    law_curve: &BSplineCurve,
    extended_law: &mut Option<BSplineCurve>,
    edge_map: &Extent1d,
    parameter: f64,
    interval: &Extent1d,
    reverse: bool,
) -> Result<[f64; 3]> {
    if interval.length() == 0.0 {
        return Err(SolidError::sm_err(OP_EVAL));
    }

    // select function to evaluate
    let law = extended_law.as_ref().unwrap_or(law_curve);

    // Compute the scale difference between the parameters of the map and the curve.
    let ivl = law.natural_interval();
    let crv_ivl_size = interval.length();
    let map_ivl_size = edge_map.length();
    let param_scale = crv_ivl_size / map_ivl_size;

    // Map parameter to unitized range accounting for bReverseOrientation
    let d_min = interval.min();
    let d_max = interval.max();

    let t = if reverse {
        (d_max - parameter) / (d_max - d_min)
    } else {
        (parameter - d_min) / (d_max - d_min)
    };

    // Map parameter back to interval of the curve.
    let mut law_t = edge_map.evaluate(t);

    // when parameter is beyond current range
    let mut new_extension = None;
    if !ivl.contains_value(law_t, 0.0) {
        // select param space tolerance
        let scaled_tol = EFF_ZERO * (1.0 + ivl.min().abs() + ivl.max().abs());

        // Try to snap to begin point.
        if law_t < ivl.min() && law_t + scaled_tol > ivl.min() {
            law_t = ivl.min();
        }

        // Try to snap to end point.
        if law_t > ivl.max() && law_t - scaled_tol < ivl.max() {
            law_t = ivl.max();
        }

        // when parameter is still beyond current range
        if !ivl.contains_value(law_t, 0.0) {
            // create an extension of the curve to the requested param point
            // But extend farther than requested, so that we won't have to do this too often.
            // This doubles the extension distance:
            let mut ext_param = if law_t < ivl.min() {
                law_t - (ivl.min() - law_t)
            } else {
                law_t + (law_t - ivl.max())
            };
            // But never do a really tiny extension: it might get snapped to the end,
            // and therefor not extended at all.  [Fillet 308]
            let len = ivl.length();
            if ext_param < ivl.min() {
                if ext_param > ivl.min() - len / 10.0 {
                    ext_param = ivl.min() - len / 10.0;
                }
            } else if ext_param < ivl.max() + len / 10.0 {
                ext_param = ivl.max() + len / 10.0;
            }

            // precise extension (otherwise it adds another length of sIvl);
            // C-infinity works better than G1_G2.  [B551]
            new_extension =
                Some(law.create_extended_curve(ext_param, ContinuityType::CInfinity, true)?);
        } // end need to extend fillet-radius function check after snapping to endPoints
    } // end need to extend fillet-radius function check

    if let Some(extension) = new_extension {
        // save and use the extended curve (the previous one is dropped)
        *extended_law = extension;
        if extended_law.is_none() {
            // create_extended_curve found the parameter inside the NURB's own bounds
            // (Ok(None)), which needs natural_interval and those bounds to disagree;
            // upstream would dereference the NULL curve here
            return Err(SolidError::null_pointer(OP_EVAL));
        }
    }
    let law = extended_law.as_ref().unwrap_or(law_curve);

    let pv = law.evaluate(law_t, 2, true, true)?;
    let x = |i: usize| {
        pv.get(i)
            .map(|v| v.x)
            .ok_or_else(|| SolidError::sm_err(OP_EVAL))
    };

    let value = x(0)?;
    let mut deriv1 = x(1)? / param_scale;
    if reverse {
        deriv1 = -deriv1;
    }
    let deriv2 = x(2)? / (param_scale * param_scale);

    Ok([value, deriv1, deriv2])
}

// ---- SmFilletErrorInfo ---------------------------------------------------------------------

/// `SM_TBLOCK_SIZE`: the size of the error-info text buffers.
const TBLOCK_SIZE: usize = 1024;

/// `smos_sprintf(buffer, "%s", text)`: `snprintf` into an `SM_TBLOCK_SIZE` buffer keeps
/// at most 1023 bytes (cut back to a character boundary here); a null `text` prints
/// glibc's `(null)`.
fn tblock_text(text: Option<&str>) -> String {
    let text = text.unwrap_or("(null)");
    let mut end = text.len().min(TBLOCK_SIZE - 1);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.get(..end).unwrap_or_default().to_owned()
}

/// `SmTArray::AddUnique`.
fn add_unique<T: PartialEq>(list: &mut Vec<T>, item: T) {
    if !list.contains(&item) {
        list.push(item);
    }
}

impl<O: Copy + PartialEq> ErrorInfo<O> {
    /// `SmFilletErrorInfo(pFilletExec)`: the `set()` defaults with the executive.
    #[must_use]
    pub fn new(executive: Option<O>) -> Self {
        Self {
            executive,
            ..Self::default()
        }
    }

    /// `NoteError(eErrorCode, cComment)`: a generic error; a null comment keeps the old
    /// one.
    pub fn note_error(&mut self, code: FilletErrorType, comment: Option<&str>) {
        self.code = code as i32;
        if comment.is_some() {
            self.comment = tblock_text(comment);
        }
    }

    /// `NoteError(pBadEdge, eStatus, eErrorCode, cComment)`: an error at a filleted edge;
    /// nothing for a null edge.
    pub fn note_edge_error(
        &mut self,
        edge: Option<O>,
        status: FilStatus,
        code: FilletErrorType,
        comment: Option<&str>,
    ) {
        if edge.is_none() {
            return;
        }
        self.edge = edge;
        self.code = code as i32;
        self.status = status as i32;
        self.comment = tblock_text(comment);
    }

    /// `NoteError(pBadVert, eVertexStatus, eErrorCode, cComment)`: an error at a filleted
    /// vertex; nothing for a null vertex.
    ///
    /// The pinned overload reads the vertex number and records nothing (the issue #1469
    /// defect). This stores the vertex, its point, the status, the code and the comment;
    /// the executive calls it only when
    /// [`crate::kernel::UpstreamFixes::fillet_vertex_error_diagnostics`] is on.
    /// `location` is the vertex point the caller resolved (the fork's corrected overload
    /// reads it from `pBadVert->GetPoint()`); a null comment keeps the old one, as its
    /// `if (cComment != NULL)` guard does.
    pub fn note_vertex_error(
        &mut self,
        vertex: Option<O>,
        location: Option<Point3>,
        status: FilStatus,
        code: FilletErrorType,
        comment: Option<&str>,
    ) {
        if vertex.is_none() {
            return;
        }
        self.vertex = vertex;
        self.location = location;
        self.code = code as i32;
        self.status = status as i32;
        if comment.is_some() {
            self.comment = tblock_text(comment);
        }
    }

    /// `NoteError(eErrorCode, eEdgeStatus, pFilletedEdge, pSideFace, pFilletEdge,
    /// pFilletSurface, cComment)`: merging a fillet edge into a target face failed.
    #[allow(clippy::too_many_arguments)] // upstream's overload
    pub fn note_merge_error(
        &mut self,
        code: FilletErrorType,
        status: FilStatus,
        filleted_edge: Option<O>,
        side_face: Option<O>,
        fillet_edge: Option<O>,
        surface: Option<O>,
        comment: Option<&str>,
    ) {
        self.code = code as i32;
        self.status = status as i32;
        self.edge = filleted_edge;
        self.side_face = side_face;
        add_unique(&mut self.edges, fillet_edge);
        add_unique(&mut self.surfaces, surface);
        self.comment = tblock_text(comment);
    }

    /// `NoteError(eErrorCode, eCornerType, eFilletStatus, cCornerName, pFilletedVertex,
    /// pFilletSurface1, pFilletSurface2, pStartFV, pEndFV, cComment)`: an error at a
    /// fillet corner (two fillet surfaces failed to intersect, for one).
    #[allow(clippy::too_many_arguments)] // upstream's overload
    pub fn note_corner_error(
        &mut self,
        code: FilletErrorType,
        corner_type: FilletCornerType,
        status: FilStatus,
        corner_name: Option<&str>,
        vertex: Option<O>,
        surfaces: [Option<O>; 2],
        fillet_vertices: [Option<O>; 2],
        comment: Option<&str>,
    ) {
        self.code = code as i32;
        self.status = status as i32;
        self.corner_type = corner_type;
        self.vertex = vertex;
        let [surface1, surface2] = surfaces;
        add_unique(&mut self.surfaces, surface1);
        add_unique(&mut self.surfaces, surface2);
        self.fillet_vertices = fillet_vertices;
        self.comment = tblock_text(comment);
        self.entity_name = tblock_text(corner_name);
    }

    /// `NoteError(eErrorCode, eFilletStat, eFEType, pFilletEdge, cEdgeName, cComment)`:
    /// an error at a fillet edge (of the fillet B-rep, not the target).
    pub fn note_fillet_edge_error(
        &mut self,
        code: FilletErrorType,
        status: FilStatus,
        edge_type: FilletEdgeType,
        fillet_edge: Option<O>,
        edge_name: Option<&str>,
        comment: Option<&str>,
    ) {
        self.code = code as i32;
        self.status = status as i32;
        self.edge_type = edge_type as i32;
        add_unique(&mut self.edges, fillet_edge);
        self.comment = tblock_text(comment);
        self.entity_name = tblock_text(edge_name);
    }

    /// `NoteError(eErrorCode, eFilletStatus, pFilletedEdge, rapFilSurfs, rapFilCurvs,
    /// rapFilEdges, rTsectPos, cComment)`: an error along a filleted edge (the base
    /// surfaces failed to intersect, for one).
    #[allow(clippy::too_many_arguments)] // upstream's overload
    pub fn note_intersection_error(
        &mut self,
        code: FilletErrorType,
        status: FilStatus,
        filleted_edge: Option<O>,
        surfaces: &[Option<O>],
        curves: &[Option<O>],
        fillet_edges: &[Option<O>],
        location: Point3,
        comment: Option<&str>,
    ) {
        self.code = code as i32;
        self.status = status as i32;
        self.edge_type = FilletEdgeType::Unknown as i32; // (it's not a fillet edge)
        self.edge = filleted_edge;
        for &surface in surfaces {
            add_unique(&mut self.surfaces, surface);
        }
        for &curve in curves {
            add_unique(&mut self.curves, curve);
        }
        for &edge in fillet_edges {
            add_unique(&mut self.edges, edge);
        }
        self.location = Some(location);
        self.comment = tblock_text(comment);
    }
}

// ---- SmFilletExecutive: the object ---------------------------------------------------------

/// The live objects read or written by `LoadFilletSolver`.
///
/// A production session can load its concrete solvers without implementing unrelated
/// executive operations. Full adapters retain their behavior through delegation.
pub trait SolverLoadingAccess<O: ClassObject + SolutionObject + Ord> {
    /// The retained concrete solver's mutable base.
    ///
    /// # Errors
    /// Caller object resolution or the delegated native query failure.
    fn load_solver_base_mut(
        &mut self,
        solver: O,
    ) -> Result<&mut crate::blend::fillet_solver::p1::FilletSolver>;
    /// Actual `GetSurface(rail)`, including geometry-backed surfaces.
    ///
    /// # Errors
    /// Caller object resolution or the delegated native query failure.
    fn load_solver_has_surface(&self, solver: O, rail: usize) -> Result<bool>;
    /// Actual edge knots whose curve-cache continuity is G1, in order.
    ///
    /// # Errors
    /// Caller object resolution or the delegated native query failure.
    fn load_edge_g1_knots(&mut self, k: &Kernel, edge: O) -> Result<Vec<f64>>;
    /// Actual `GetEdgeuse(rail)` of the retained solver.
    ///
    /// # Errors
    /// Caller object resolution or the delegated native query failure.
    fn load_solver_edgeuse(&self, solver: O, rail: usize) -> Result<Option<O>>;
    /// The edge use's owning edge.
    ///
    /// # Errors
    /// Caller object resolution or the delegated native query failure.
    fn load_edgeuse_edge(&self, edgeuse: O) -> Result<Option<O>>;
    /// The edge use's start vertex (a null vertex stays null).
    ///
    /// # Errors
    /// Caller object resolution or the delegated native query failure.
    fn load_edgeuse_vertex(&self, edgeuse: O) -> Result<Option<O>>;
}

impl<O, A> SolverLoadingAccess<O> for A
where
    O: ClassObject + SolutionObject + Ord,
    A: ExecutiveAccess<O> + ?Sized,
{
    fn load_solver_base_mut(
        &mut self,
        solver: O,
    ) -> Result<&mut crate::blend::fillet_solver::p1::FilletSolver> {
        ExecutiveAccess::solver_base_mut(self, solver)
    }
    fn load_solver_has_surface(&self, solver: O, rail: usize) -> Result<bool> {
        ExecutiveAccess::solver_has_surface(self, solver, rail)
    }
    fn load_edge_g1_knots(&mut self, k: &Kernel, edge: O) -> Result<Vec<f64>> {
        ExecutiveAccess::edge_g1_knots(self, k, edge)
    }
    fn load_solver_edgeuse(&self, solver: O, rail: usize) -> Result<Option<O>> {
        ExecutiveAccess::solver_edgeuse(self, solver, rail)
    }
    fn load_edgeuse_edge(&self, edgeuse: O) -> Result<Option<O>> {
        ExecutiveAccess::edgeuse_edge(self, edgeuse)
    }
    fn load_edgeuse_vertex(&self, edgeuse: O) -> Result<Option<O>> {
        ExecutiveAccess::edgeuse_vertex(self, edgeuse)
    }
}

/// The objects `SmFilletExecutive(crContext, pTargetBrep)` allocates, made by the owner of
/// the handles ([`ExecutiveAccess::new_executive_objects`]).
#[derive(Debug, Clone, Copy)]
pub struct ExecutiveObjects<O> {
    /// `this`, as `m_sErrorInfo.m_pFilExec` records it.
    pub executive: O,
    /// `m_pPseudoBrep`: a new `SmFilletBrep` with editing enabled.
    pub pseudo_brep: O,
    /// `m_pAttributeE`: a new `SmAttribute(SM_AI_ALL_FILLET_EDGES,
    /// SM_AB_STANDALONE_REFERENCE)`.
    pub edge_attribute: O,
}

/// `SmFilletExecutive::IsRadiusSupported`: `false` for a radius or chamfer width below
/// 8 times the B-rep tolerance `brep_tol` (below that, some fillets come out
/// non-manifold, crash or hang), and for NaN.
#[must_use]
pub fn is_radius_supported(radius: f64, brep_tol: f64) -> bool {
    radius >= 8.0 * brep_tol * (1.0 - EFF_ZERO_SQRT)
}

impl<O: ClassObject + SolutionObject + Ord> FilletExecutive<O> {
    /// `SmFilletExecutive(crContext, pTargetBrep)`: no fillet B-rep, empty lists and
    /// maps, the upstream switch defaults, the `SmMerge(crContext)` intersector
    /// (`SM_EFF_ZERO_SQ`, 30 degrees) ignoring tangency, and the error info pointing
    /// back at the executive.
    #[must_use]
    pub fn new(target: Option<O>, objects: ExecutiveObjects<O>) -> Self {
        let mut intersector =
            TopologyIntersector::new(None, None, EFF_ZERO_SQ, 30.0 * (PI / 180.0));
        intersector.ignore_tangency = true;
        Self {
            target,
            fillet: None,
            corners: Vec::new(),
            solvers: Vec::new(),
            topo_edges: Vec::new(),
            edge_attribute: Some(objects.edge_attribute),
            initialized: false,
            options: ExecutionOptions::default(),
            error: ErrorInfo::new(Some(objects.executive)),
            intersector,
            intersection_edges: Vec::new(),
            intersection_vertices: Vec::new(),
            law_sequence_edges: Vec::new(),
            pseudo_brep: Some(objects.pseudo_brep),
            surface_generators: Vec::new(),
            filleted_vertices: Vec::new(),
            vertex_corner_map: BTreeMap::new(),
            edgeuse_solver_map: BTreeMap::new(),
            self_intersection_handler: None,
            blend_curve_at_thirds: false,
            mark_corner_patches: false,
        }
    }

    /// `LoadFilletSolver`: retain the solver, its actual executive binding and the
    /// incident edge uses of its vertices, preserving native insertion order.
    ///
    /// # Errors
    /// Missing offset surfaces, unresolved caller objects, or a native undefined
    /// vertex-array read when an absent rail reaches an existing vertex entry.
    /// Earlier native mutations remain visible on a later error.
    pub fn load_fillet_solver<A: SolverLoadingAccess<O> + ?Sized>(
        &mut self,
        k: &Kernel,
        access: &mut A,
        solver: O,
    ) -> Result<()> {
        const OP: &str = "blend::fillet_executive::p1::load_fillet_solver";
        for rail in 0..2 {
            if !access.load_solver_has_surface(solver, rail)? {
                return Err(SolidError::null_pointer(OP));
            }
        }
        if let Some(edgeuse) = access.load_solver_edgeuse(solver, 0)? {
            let edge = access
                .load_edgeuse_edge(edgeuse)?
                .ok_or_else(|| SolidError::invalid_input(OP, "edge use without edge"))?;
            for knot in access.load_edge_g1_knots(k, edge)? {
                access.load_solver_base_mut(solver)?.g1_knots.push(knot);
            }
        }
        self.solvers.push(solver);
        access
            .load_solver_base_mut(solver)?
            .set_fillet_executive(Some(crate::blend::fillet_solver::p1::FilletExecutiveRef));
        let mut edgeuses = [None; 2];
        // Outer None is an unwritten native pVerts entry; inner None is a null vertex.
        let mut vertices = [None; 2];
        for (rail, (edgeuse_slot, vertex_slot)) in
            edgeuses.iter_mut().zip(vertices.iter_mut()).enumerate()
        {
            let edgeuse = access.load_solver_edgeuse(solver, rail)?;
            if let Some(edgeuse) = edgeuse {
                self.edgeuse_solver_map.insert(edgeuse, solver);
                *vertex_slot = Some(access.load_edgeuse_vertex(edgeuse)?);
            }
            *edgeuse_slot = edgeuse;
        }
        let first_edgeuse_present = edgeuses.first().is_some_and(Option::is_some);
        for (edgeuse, vertex_entry) in edgeuses.into_iter().zip(vertices) {
            let mut found = false;
            for &vertex in &self.filleted_vertices {
                let current = vertex_entry.ok_or_else(|| {
                    SolidError::invalid_input(
                        OP,
                        "native pVerts entry is uninitialized (absent solver rail)",
                    )
                })?;
                if current == vertex {
                    found = true;
                    let entry = self
                        .vertex_corner_map
                        .get_mut(&vertex)
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    let super::p3::VertexEntry::SolverEdgeuses(pending) = entry else {
                        return Err(SolidError::invalid_input(
                            OP,
                            "native pending list has already been replaced by a corner",
                        ));
                    };
                    let edgeuse = edgeuse.ok_or_else(|| {
                        SolidError::invalid_input(
                            OP,
                            "native pending array would read an absent edge use",
                        )
                    })?;
                    let edge = access.load_edgeuse_edge(edgeuse)?;
                    let mut coincident = false;
                    for &pending_edgeuse in pending.iter() {
                        if edge == access.load_edgeuse_edge(pending_edgeuse)? {
                            coincident = true;
                            break;
                        }
                    }
                    if !coincident {
                        pending.push(edgeuse);
                    }
                    break;
                }
            }
            if !found && first_edgeuse_present {
                let vertex = vertex_entry.ok_or_else(|| {
                    SolidError::invalid_input(
                        OP,
                        "native pVerts entry is uninitialized (absent solver rail)",
                    )
                })?;
                let edgeuse = edgeuse.ok_or_else(|| {
                    SolidError::invalid_input(
                        OP,
                        "native pending array would store an absent edge use",
                    )
                })?;
                self.filleted_vertices.push(vertex);
                self.vertex_corner_map.insert(
                    vertex,
                    super::p3::VertexEntry::SolverEdgeuses(vec![edgeuse]),
                );
            }
        }
        Ok(())
    }

    /// `AddFilletSurfaceGenerator(pFSG)`: the executive owns the generator from now on.
    pub fn add_fillet_surface_generator(&mut self, generator: O) {
        self.surface_generators.push(generator);
    }

    /// `RemoveFromTopoEdges(pTopoEdge)`: drop the first entry equal to `topo_edge`.
    pub fn remove_from_topo_edges(&mut self, topo_edge: Option<O>) {
        if let Some(index) = self.topo_edges.iter().position(|&e| e == topo_edge) {
            self.topo_edges.remove(index);
        }
    }

    /// `SetDoGlobalMerge`.
    pub fn set_do_global_merge(&mut self, value: bool) {
        self.options.global_merge = value;
    }

    /// `SetDoTrimming`.
    pub fn set_do_trimming(&mut self, value: bool) {
        self.options.trimming = value;
    }

    /// `SetBlendCurveAtThirds` (the fork's, source fork `fd3b6a7`): before
    /// `SetFilletParameters`, which creates the section generators with it.
    pub fn set_blend_curve_at_thirds(&mut self, value: bool) {
        self.blend_curve_at_thirds = value;
    }

    /// `SetMarkCornerPatches` (the fork's, source fork `60af574`): before
    /// `DoFilleting`.
    pub fn set_mark_corner_patches(&mut self, value: bool) {
        self.mark_corner_patches = value;
    }

    /// `SetSelfIntersectionHandler`.
    pub fn set_self_intersection_handler(&mut self, handler: Option<O>) {
        self.self_intersection_handler = handler;
    }

    /// `GetFilletCornerOfVertex(pVertex)`: the corner at `vertex`, `None` when there is
    /// none.
    ///
    /// # Errors
    /// A pending solver list is not a completed corner (native UB-209).
    pub fn fillet_corner_of_vertex(&self, vertex: O) -> Result<Option<O>> {
        match self.vertex_corner_map.get(&Some(vertex)) {
            None => Ok(None),
            Some(super::p3::VertexEntry::Corner(corner)) => Ok(Some(*corner)),
            Some(super::p3::VertexEntry::SolverEdgeuses(_)) => Err(SolidError::invalid_input(
                "blend::fillet_executive::p1::fillet_corner_of_vertex",
                "vertex still contains a pending solver list",
            )),
        }
    }

    /// `GetFilletSolverOfEdgeuse(pEdgeuse)`: the solver of `edgeuse`, `None` when there
    /// is none.
    #[must_use]
    pub fn fillet_solver_of_edgeuse(&self, edgeuse: O) -> Option<O> {
        self.edgeuse_solver_map.get(&edgeuse).copied()
    }

    /// `GetFilletSolverOfEdge(pEdge)`: the solver of the first edge use of `edge` that
    /// has one.
    ///
    /// # Errors
    ///
    /// Those of [`ExecutiveAccess::edge_edgeuses`].
    pub fn fillet_solver_of_edge<A>(&self, access: &A, edge: O) -> Result<Option<O>>
    where
        A: ExecutiveAccess<O> + ?Sized,
    {
        self.fillet_solver_of_edge_with(edge, |e| access.edge_edgeuses(e))
    }

    /// Closure-backed form of [`FilletExecutive::fillet_solver_of_edge`].
    ///
    /// # Errors
    ///
    /// Those of `edgeuses_of`.
    pub fn fillet_solver_of_edge_with(
        &self,
        edge: O,
        mut edgeuses_of: impl FnMut(O) -> Result<Vec<O>>,
    ) -> Result<Option<O>> {
        Ok(edgeuses_of(edge)?
            .into_iter()
            .find_map(|eu| self.fillet_solver_of_edgeuse(eu)))
    }

    /// `NoteFilletError(eErrorType, pFilletEdge, cComment)`: an error at a fillet edge,
    /// with the edge's name, type and status from the adapter.
    ///
    /// # Errors
    ///
    /// Those of [`ExecutiveAccess::fillet_edge_note`] and
    /// [`FilletExecutive::note_fillet_edge`].
    pub fn note_fillet_error<A>(
        &mut self,
        access: &A,
        code: FilletErrorType,
        fillet_edge: O,
        comment: Option<&str>,
    ) -> Result<()>
    where
        A: ExecutiveAccess<O> + ?Sized,
    {
        let note = access.fillet_edge_note(fillet_edge)?;
        self.note_fillet_edge(code, fillet_edge, note, comment)
    }

    /// The body of [`FilletExecutive::note_fillet_error`] given the fillet edge's
    /// `GetName`, `GetFilletEdgeType()` and `GetStatus()`.
    ///
    /// # Errors
    ///
    /// [`SolidError::InvalidInput`] for a fillet edge type `GetName` writes no name for
    /// (upstream then prints an uninitialised buffer).
    pub fn note_fillet_edge(
        &mut self,
        code: FilletErrorType,
        fillet_edge: O,
        (name, edge_type, status): (Option<&'static str>, FilletEdgeType, FilStatus),
        comment: Option<&str>,
    ) -> Result<()> {
        let name = name.ok_or(SolidError::InvalidInput {
            operation: OP_NOTE,
            reason: "fillet edge type without a name (uninitialised name upstream)",
        })?;
        self.error.note_fillet_edge_error(
            code,
            status,
            edge_type,
            Some(fillet_edge),
            Some(name),
            comment,
        );
        Ok(())
    }

    /// `NoteFilletError(eErrorCode, eFilletStatus, pFilletedEdge, pTargetFace,
    /// pFilletEdge, pFilletSurface, cComment)`: [`ErrorInfo::note_merge_error`].
    #[allow(clippy::too_many_arguments)] // upstream's overload
    pub fn note_fillet_merge_error(
        &mut self,
        code: FilletErrorType,
        status: FilStatus,
        filleted_edge: Option<O>,
        target_face: Option<O>,
        fillet_edge: Option<O>,
        surface: Option<O>,
        comment: Option<&str>,
    ) {
        self.error.note_merge_error(
            code,
            status,
            filleted_edge,
            target_face,
            fillet_edge,
            surface,
            comment,
        );
    }

    /// `NoteFilletError(eErrorType, eCornerType, eFilletStat, sCornerName, ...)`:
    /// [`ErrorInfo::note_corner_error`].
    #[allow(clippy::too_many_arguments)] // upstream's overload
    pub fn note_fillet_corner_error(
        &mut self,
        code: FilletErrorType,
        corner_type: FilletCornerType,
        status: FilStatus,
        corner_name: Option<&str>,
        vertex: Option<O>,
        surfaces: [Option<O>; 2],
        fillet_vertices: [Option<O>; 2],
        comment: Option<&str>,
    ) {
        self.error.note_corner_error(
            code,
            corner_type,
            status,
            corner_name,
            vertex,
            surfaces,
            fillet_vertices,
            comment,
        );
    }

    /// `NoteFilletError(eErrorType, eFilletStat, pFilletedEdge, rapFilSurfs, ...)`:
    /// [`ErrorInfo::note_intersection_error`].
    #[allow(clippy::too_many_arguments)] // upstream's overload
    pub fn note_fillet_intersection_error(
        &mut self,
        code: FilletErrorType,
        status: FilStatus,
        filleted_edge: Option<O>,
        surfaces: &[Option<O>],
        curves: &[Option<O>],
        fillet_edges: &[Option<O>],
        location: Point3,
        comment: Option<&str>,
    ) {
        self.error.note_intersection_error(
            code,
            status,
            filleted_edge,
            surfaces,
            curves,
            fillet_edges,
            location,
            comment,
        );
    }
}

/// What the owner's `SmFilletCorner::Create` factory did for one vertex.
///
/// `SmFilletCorner::Create` returns a corner pointer or null; the null case is split
/// here so an owner that can resolve the vertex's point can feed the opt-in vertex
/// error note without changing the existing factory signature.
#[derive(Debug, Clone, Copy)]
pub enum CornerCreation<O> {
    /// The factory created a corner of this identity.
    Created(O),
    /// No corner type handles the vertex. `location` is its point when the owner could
    /// resolve it; the fork's corrected overload reads it from the vertex.
    Unsupported {
        /// The vertex's point when the owner resolved it.
        location: Option<Point3>,
    },
}

impl<O: ClassObject + SolutionObject + Ord> FilletExecutive<O> {
    /// `SmFilletExecutive::CreateFilletCorners`: create corners in the native
    /// filleted-vertex order using the owner's actual corner factory and store.
    ///
    /// `create` borrows the live executive and pending edge-use list. It returns
    /// the identity of the corner it allocated, or `None` for an unsupported
    /// corner. Successful corners replace pending entries; unsupported corners
    /// remain pending. The optional error info is assigned only after the loop.
    ///
    /// This keeps the original factory shape. An owner that can resolve a missing
    /// vertex's point uses [`Self::create_fillet_corners_recording_vertex_errors`].
    ///
    /// # Errors
    ///
    /// A missing pending entry is `SM_ERR_NULL_POINTER`, matching native `NER`.
    /// An already completed entry is `InvalidInput`: native would cast a corner
    /// to the pending edge-use array. Factory errors preserve completed prefixes
    /// and leave the optional error info untouched. Null vertices are passed to
    /// the owner, whose actual factory must reject the invalid topology.
    pub fn create_fillet_corners(
        &mut self,
        k: &Kernel,
        mut create: impl FnMut(&Kernel, &Self, Option<O>, &[O]) -> Result<Option<O>>,
        error_info: Option<&mut ErrorInfo<O>>,
    ) -> Result<()> {
        self.create_fillet_corners_recording_vertex_errors(
            k,
            |k, executive, vertex, pending| {
                Ok(match create(k, executive, vertex, pending)? {
                    Some(corner) => CornerCreation::Created(corner),
                    None => CornerCreation::Unsupported { location: None },
                })
            },
            error_info,
        )
    }

    /// [`Self::create_fillet_corners`] with the vertex's point supplied on an
    /// unsupported corner, so the opt-in `fixes.fillet_vertex_error_diagnostics` can
    /// retain the fields written by the fork's corrected vertex `NoteError` overload.
    ///
    /// With the fix off (the default), an unsupported corner records nothing and keeps
    /// its pending entry, exactly as [`Self::create_fillet_corners`] and the pinned
    /// `CreateFilletCorners` do. With it on, the executive records the vertex, its
    /// location, `SM_FIL_FAILURE`, `FilletErrorType::CornerUnsupported` and
    /// `"Corner case not implemented"` (the fork's note) and still keeps the pending
    /// entry: retaining the note does not change the operation's result or topology.
    ///
    /// # Errors
    ///
    /// Those of [`Self::create_fillet_corners`].
    pub fn create_fillet_corners_recording_vertex_errors(
        &mut self,
        k: &Kernel,
        mut create: impl FnMut(&Kernel, &Self, Option<O>, &[O]) -> Result<CornerCreation<O>>,
        error_info: Option<&mut ErrorInfo<O>>,
    ) -> Result<()> {
        use crate::blend::fillet_executive::p3::VertexEntry;

        const OP: &str = "blend::fillet_executive::p1::FilletExecutive::create_fillet_corners";
        for &vertex in &self.filleted_vertices {
            let pending = match self.vertex_corner_map.get(&vertex) {
                Some(VertexEntry::SolverEdgeuses(edgeuses)) => edgeuses,
                Some(VertexEntry::Corner(_)) => {
                    return Err(SolidError::invalid_input(OP, "corner already created"));
                }
                None => return Err(SolidError::null_pointer(OP)),
            };
            match create(k, self, vertex, pending)? {
                CornerCreation::Created(corner) => {
                    self.corners.push(corner);
                    self.vertex_corner_map
                        .insert(vertex, VertexEntry::Corner(corner));
                }
                CornerCreation::Unsupported { location } => {
                    if k.fixes.fillet_vertex_error_diagnostics {
                        self.error.note_vertex_error(
                            vertex,
                            location,
                            FilStatus::Failure,
                            FilletErrorType::CornerUnsupported,
                            Some("Corner case not implemented"),
                        );
                    }
                }
            }
        }
        if let Some(info) = error_info {
            info.assign(&self.error);
        }
        Ok(())
    }

    /// `SmFilletExecutive::SetBevelCorner`: makes the corner of `vertex` a bevel
    /// (`SetBevel(TRUE)`) through `set_bevel`, the owner's access to its corner
    /// objects.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` ([`SolidError::null_pointer`]) when `vertex` has no entry
    /// (`NER`); [`SolidError::InvalidInput`] when its entry is still the pending
    /// edge-use list (upstream casts the list to a corner); those of `set_bevel`.
    pub fn set_bevel_corner(
        &self,
        vertex: Option<O>,
        set_bevel: impl FnOnce(O) -> Result<()>,
    ) -> Result<()> {
        use crate::blend::fillet_executive::p3::VertexEntry;

        const OP: &str = "blend::fillet_executive::p1::FilletExecutive::set_bevel_corner";
        match self.vertex_corner_map.get(&vertex) {
            Some(VertexEntry::Corner(corner)) => set_bevel(*corner),
            Some(VertexEntry::SolverEdgeuses(_)) => Err(SolidError::invalid_input(
                OP,
                "no corner created for the vertex",
            )),
            None => Err(SolidError::null_pointer(OP)),
        }
    }

    /// `SmFilletExecutive::CreateSmoothEdgeSequence(rSmoothSequence)` (`SmFilletExecutive.cpp:4704`):
    /// orients the edges of `sequence` into sequence order, trims each edge curve to its
    /// edge interval, rescales and translates intervals after the first so consecutive
    /// segments meet with C1 derivative continuity, joins all segments into one
    /// [`BSplineCurve`], updates any `CurveBasedFs` original curve / `VariableRadiusFs`
    /// law interval, and installs copies of the joined curve on every edge.
    ///
    /// # Errors
    ///
    /// `SM_ERR_NULL_POINTER` when a solver has no primary edge use, an edge has no curve
    /// derived from `SmBSplineCurve`, or `CreateByJoining` fails (`NER(pCompCurve)`);
    /// those of the adapter and B-rep operations.
    pub fn create_smooth_edge_sequence<A>(
        &mut self,
        k: &mut Kernel,
        access: &mut A,
        sequence: &[O],
    ) -> Result<()>
    where
        A: SmoothSequenceAccess<O> + ?Sized,
    {
        const OP: &str =
            "blend::fillet_executive::p1::FilletExecutive::create_smooth_edge_sequence";
        if sequence.len() < 2 {
            return Ok(());
        }

        let mut edges = Vec::new();
        let mut shares_edge_curve = Vec::new();
        for &fs in sequence {
            let edge_id = access.smooth_solver_edge0_id(fs)?;
            let shares = match (
                access.smooth_solver_original_curve(fs)?,
                access.smooth_target_brep()?.edge(edge_id)?.curve.as_ref(),
            ) {
                (Some(orig), Some(edge_crv)) => orig.approx_eq(k, edge_crv)?,
                _ => false,
            };
            edges.push(edge_id);
            shares_edge_curve.push(shares);
        }

        let mut bs_curves = access.smooth_with_target_mut(&edges, |target| {
            crate::error::ignore_status(topo_edge::orient_edge_sequence(k, target, &edges))?;

            let mut prev_ivl = Extent1d::new();
            let mut curves = Vec::new();
            for (ii, &edge_id) in edges.iter().enumerate() {
                let mut edge_ivl = topo_edge::interval(target, edge_id)?;
                let edge_tol = topo_edge::tolerance(target, edge_id)?;
                let mut tk = Kernel::new(&crate::kernel::KernelOptions {
                    tolerances: crate::kernel::Tolerances {
                        zone_tol_3d: edge_tol,
                        ..k.tolerances
                    },
                    ..crate::kernel::KernelOptions::default()
                });
                tk.rng = k.rng.clone();
                let mut trim_ivl = edge_ivl;
                let crv = target
                    .edge_mut(edge_id)?
                    .curve
                    .as_mut()
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                crate::error::ignore_status(crv.trim(
                    &tk,
                    &mut trim_ivl,
                    crate::nurbs::b_spline_curve_n_lib::TrimMode::Notify,
                ))?;

                if ii > 0 {
                    let prev_edge_id = *edges
                        .get(ii - 1)
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    let prev_param1 = prev_ivl.max();
                    let curr_param0 = edge_ivl.min();
                    let prev_crv = target
                        .edge(prev_edge_id)?
                        .curve
                        .as_ref()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    let curr_crv = target
                        .edge(edge_id)?
                        .curve
                        .as_ref()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    let prev_deriv = eval_first_deriv_ignored(k, prev_crv, prev_param1)?;
                    let curr_deriv = eval_first_deriv_ignored(k, curr_crv, curr_param0)?;
                    let prev_len = prev_deriv.length();
                    let curr_len = curr_deriv.length();

                    // Keep the Min of the current interval the same, scale its Max by the deriv length ratio.
                    let scale = curr_len / prev_len;
                    let scaled_end = edge_ivl.min() + scale * edge_ivl.length();
                    crate::error::ignore_status(edge_ivl.set_min_max(edge_ivl.min(), scaled_end))?;

                    // Domains are the right lengths, make sure they're consecutive.
                    if !crate::geom::tol::are_same(edge_ivl.min(), prev_ivl.max()) {
                        edge_ivl.translate(prev_ivl.max() - edge_ivl.min());
                    }

                    let crv = target
                        .edge_mut(edge_id)?
                        .curve
                        .as_mut()
                        .ok_or_else(|| SolidError::null_pointer(OP))?;
                    crate::error::ignore_status(crv.edit_parameterization(k, &edge_ivl))?;
                    target.edge_mut(edge_id)?.interval = edge_ivl;
                }

                let curr_crv = target
                    .edge(edge_id)?
                    .curve
                    .as_ref()
                    .ok_or_else(|| SolidError::null_pointer(OP))?;
                curves.push(curve_as_bspline(curr_crv, OP)?);
                prev_ivl = edge_ivl;
            }
            Ok(curves)
        })?;

        for ((&fs, &edge_id), &shares) in sequence.iter().zip(&edges).zip(&shares_edge_curve) {
            if shares {
                if let Some(edited) = access.smooth_target_brep()?.edge(edge_id)?.curve.clone() {
                    access.smooth_set_solver_original_curve(fs, edited)?;
                }
            }
        }

        let comp_curve = match BSplineCurve::create_by_joining(&mut bs_curves, None) {
            Ok(c) => c,
            Err(e) if e.is_unported() => return Err(e),
            Err(_) => return Err(SolidError::null_pointer(OP)),
        };

        for (ii, ((&fs, &edge_id), &shares)) in sequence
            .iter()
            .zip(&edges)
            .zip(&shares_edge_curve)
            .enumerate()
        {
            let new_curve = if ii == 0 {
                crate::curve::curve::Curve::from(comp_curve.clone())
            } else {
                crate::curve::curve::Curve::from(comp_curve.copy())
            };
            if shares {
                access.smooth_set_solver_original_curve(fs, new_curve.clone())?;
            }
            let ivl = topo_edge::interval(access.smooth_target_brep()?, edge_id)?;
            access.smooth_set_solver_law_interval(fs, ivl)?;
            access.smooth_set_edge_curve(k, edge_id, new_curve)?;
        }

        Ok(())
    }
}

fn eval_first_deriv_ignored(
    k: &Kernel,
    crv: &crate::curve::curve::Curve,
    param: f64,
) -> Result<crate::math::Vec3> {
    match crv.evaluate(k, param, 1, false, true) {
        Ok(vals) => Ok(vals.get(1).copied().unwrap_or(crate::math::Vec3::UNDEF)),
        Err(e) if e.is_unported() => Err(e),
        Err(_) => Ok(crate::math::Vec3::UNDEF),
    }
}

fn curve_as_bspline(crv: &crate::curve::curve::Curve, op: &'static str) -> Result<BSplineCurve> {
    use crate::curve::curve::CurveKind;
    match crv.kind() {
        CurveKind::BSpline(b) => Ok(b.clone()),
        CurveKind::Line(l) => Ok(l.base_with_nurb()?.into_owned()),
        CurveKind::Ellipse(e) => Ok(e.base().clone()),
        CurveKind::Parabola(p) => Ok(p.base().clone()),
        CurveKind::Hyperbola(h) => Ok(h.base.clone()),
        _ => Err(SolidError::null_pointer(op)),
    }
}

/// Live adapter operations used by `CollectTangentEdgeSequence` and
/// `CreateSmoothEdgeSequence`.
pub trait SmoothSequenceAccess<O: ClassObject + SolutionObject + Ord> {
    /// `pEdge->GetEdgeuses(sEdgeuses)` (used by `GetFilletSolverOfEdge`).
    ///
    /// # Errors
    /// Object resolution or topology query failures.
    fn smooth_edge_edgeuses(&self, edge: O) -> Result<Vec<O>>;
    /// `pFS->GetEdge()`: the target edge handle of `solver`.
    ///
    /// # Errors
    /// Object resolution or missing solver edge (`SM_ERR_NULL_POINTER`).
    fn smooth_solver_edge(&self, solver: O) -> Result<O>;
    /// `pFS->GetFilletCorner(index)`: corner of `solver`'s start (`0`) or end (`1`) vertex.
    ///
    /// # Errors
    /// Object resolution or pending corner-map failures.
    fn smooth_solver_corner(
        &self,
        executive: &FilletExecutive<O>,
        solver: O,
        index: usize,
    ) -> Result<Option<O>>;
    /// `pFS->GetOtherFilletCorner(corner)`: the corner at the opposite end of `solver`.
    ///
    /// # Errors
    /// Those of [`SmoothSequenceAccess::smooth_solver_corner`].
    fn smooth_solver_other_corner(
        &self,
        executive: &FilletExecutive<O>,
        solver: O,
        corner: O,
    ) -> Result<Option<O>> {
        let c0 = self.smooth_solver_corner(executive, solver, 0)?;
        let c1 = self.smooth_solver_corner(executive, solver, 1)?;
        if c0 == Some(corner) {
            Ok(c1)
        } else if c1 == Some(corner) {
            Ok(c0)
        } else {
            Ok(None)
        }
    }
    /// `pCorner->GetTangentFilletSolver(solver)`.
    ///
    /// # Errors
    /// Object resolution or continuity evaluation failures.
    fn smooth_corner_tangent_solver(
        &self,
        k: &Kernel,
        executive: &FilletExecutive<O>,
        corner: O,
        solver: O,
    ) -> Result<Option<O>>;
    /// Target `EdgeId` of `pFS->GetEdgeuse(0)->GetEdge()` for `CreateSmoothEdgeSequence`.
    ///
    /// # Errors
    /// Object resolution or missing edge use / edge (`SM_ERR_NULL_POINTER`).
    fn smooth_solver_edge0_id(&self, solver: O) -> Result<EdgeId>;
    /// Borrow the target `Brep` (`CreateSmoothEdgeSequence`).
    ///
    /// # Errors
    /// Missing target B-rep.
    fn smooth_target_brep(&self) -> Result<&Brep>;
    /// Mutate the target `Brep` during `OrientEdgeSequence`, `Trim`, `EditParameterization`,
    /// and `SetInterval`, invalidating any cached `CurveCache` for `edges`.
    ///
    /// # Errors
    /// Missing target B-rep or callback errors.
    fn smooth_with_target_mut<T>(
        &mut self,
        edges: &[EdgeId],
        f: impl FnOnce(&mut Brep) -> Result<T>,
    ) -> Result<T>;
    /// Set a joined curve on `edge` (`pE->SetCurve(pNewCurve, TRUE)`) and invalidate caches.
    ///
    /// # Errors
    /// Topology notification or B-rep mutation failures.
    fn smooth_set_edge_curve(
        &mut self,
        k: &mut Kernel,
        edge: EdgeId,
        curve: crate::curve::curve::Curve,
    ) -> Result<()>;
    /// `SM_CAST_PTR(SmCurveBasedFS, solver)->GetOriginalCurve()`.
    ///
    /// # Errors
    /// Solver lookup failure.
    fn smooth_solver_original_curve(
        &self,
        solver: O,
    ) -> Result<Option<&crate::curve::curve::Curve>>;
    /// `SM_CAST_PTR(SmCurveBasedFS, solver)->SetOriginalCurve(curve)`.
    ///
    /// # Errors
    /// Solver lookup failure.
    fn smooth_set_solver_original_curve(
        &mut self,
        solver: O,
        curve: crate::curve::curve::Curve,
    ) -> Result<()>;
    /// `SM_CAST_PTR(SmVariableRadiusFS, solver)->SetFilletLawInterval(interval)`.
    ///
    /// # Errors
    /// Solver lookup failure.
    fn smooth_set_solver_law_interval(&mut self, solver: O, interval: Extent1d) -> Result<()>;
}

// ---- RemoveTopoEdges -----------------------------------------------------------------------

/// `SmFilletExecutive::RemoveTopoEdges`: deletes the target B-rep's `m_vTopoEdges`
/// (`edges`, in order), edges that can go without changing the modelled shape, such
/// as struts.
///
/// Where an edge joins two faces with different surfaces, the face whose surface's
/// natural domain lies inside the other's (to `SM_EFF_ZERO`; otherwise the second
/// face) takes a `CopyAndAddAnalytics` copy of the other surface and its own is
/// deleted. Then the edge is deleted, its vertices left without edges are deleted,
/// and a vertex left between two different manifold edges (four vertex uses) is
/// removed by `DeleteTopologicalVertex`, merging the edges; when that fails the
/// vertex stays, as upstream ignores the status. The faces of `b` own their surfaces
/// (as every face here does); the edits notify through local environments only.
///
/// # Errors
///
/// The statuses of `DeleteEdge`, `DeleteVertex` and `CopyAndAddAnalytics`; the port's
/// own errors from `DeleteTopologicalVertex`;
/// [`SolidError::InvalidInput`] for a null edge or vertex, or for one face with a
/// surface and one without, where upstream dereferences null;
/// [`SolidError::NotFound`] for an edge or vertex an earlier deletion removed
/// (upstream reads freed memory).
pub fn remove_topo_edges(k: &mut Kernel, b: &mut Brep, edges: &[Option<EdgeId>]) -> Result<()> {
    const OP: &str = "blend::fillet_executive::p1::remove_topo_edges";

    // for every Edge in the m_vTopoEdges list
    for &edge in edges {
        let edge = edge.ok_or_else(|| SolidError::invalid_input(OP, "null topo edge"))?;
        let verts = topo_edge::vertices(b, edge, None)?;
        let faces = topo_edge::faces(b, edge, None)?;

        // for manifold edges, when faces are defined by different surfaces:
        // copy one surface, delete the other, and map its face to the copy
        if let [f0, f1] = faces[..] {
            let domains = match (face_p2::surface(b, f0)?, face_p2::surface(b, f1)?) {
                (None, None) => None,
                (Some(s0), Some(s1)) => Some((
                    s0.natural_uv_domain_with_nurb(k)?,
                    s1.natural_uv_domain_with_nurb(k)?,
                )),
                _ => return Err(SolidError::invalid_input(OP, "face without surface")),
            };
            if let Some((domain0, domain1)) = domains {
                // pick a surface to delete
                let (kept_face, delete_face) = if domain0.is_contained_by(&domain1, EFF_ZERO) {
                    (f1, f0)
                } else {
                    (f0, f1)
                };

                // Copy the KeptSurface, when possible as an analytic surface
                let new_surface = face_p2::surface(b, kept_face)?
                    .ok_or_else(|| SolidError::invalid_input(OP, "face without surface"))?
                    .copy_and_add_analytics(k)?;
                let face_domain = new_surface.natural_uv_domain_with_nurb(k)?;
                face_p2::set_uv_domain(b, delete_face, &face_domain)?;
                // the face owns the copy (under the identity the DeleteSurface names it
                // by); the DeleteSurface is deleted
                let this = face_p2::surface(b, delete_face)?
                    .and_then(Surface::owner)
                    .unwrap_or_else(|| loopuse::notify_ref(EntityRef::Face(delete_face)));
                face_p1::set_surface(k, b, delete_face, this, new_surface, true)?;
            }
        }

        // Now delete the edge
        brep_p4::with_local_env(|d| brep_p4::delete_edge(k, b, d, edge, None))?;

        // Delete topological vertex and combine two adjacent edges
        for vertex in verts {
            let vertex = vertex.ok_or_else(|| SolidError::invalid_input(OP, "null vertex"))?;
            match vertex::edges(b, vertex, None)?[..] {
                [] => brep_p4::with_local_env(|d| brep_p4::delete_vertex(k, b, d, vertex))?,
                // when vertex is connected to just two different manifold edges
                [e1, e2]
                    if vertex::vertexuses(b, vertex)?.len() == 4
                        && topo_edge::is_manifold(b, e1)?
                        && topo_edge::is_manifold(b, e2)?
                        && e1 != e2 =>
                {
                    // Not passing in an edge to delete will remove the shorter edge;
                    // when unable to really combine edges, go on
                    let editing = b.editing_enabled();
                    if let Err(
                        e @ (SolidError::Unported { .. }
                        | SolidError::InvalidInput { .. }
                        | SolidError::PortInvariant { .. }
                        | SolidError::NotFound { .. }),
                    ) = brep_p5::delete_topological_vertex(k, b, editing, Some(vertex), None)
                    {
                        // the port's own failures (a dangling id, a modelled null
                        // dereference) are not upstream statuses
                        return Err(e);
                    }
                }
                _ => {}
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::nurbs::crv_basic::Curve as NurbCurve;
    use crate::nurbs::math::{CPoint, NOW};

    /// A clamped degree-`p` curve with the radii `xs` as x coordinates over `[a, b]`.
    fn radius_curve(xs: &[f64], p: usize, a: f64, b: f64) -> BSplineCurve {
        let pw = xs.iter().map(|&x| CPoint::new(x, 0.0, 0.0, NOW)).collect();
        let spans = xs.len() - p;
        let mut u = vec![a; p + 1];
        for i in 1..spans {
            u.push(a + (b - a) * i as f64 / spans as f64);
        }
        u.extend(std::iter::repeat_n(b, p + 1));
        BSplineCurve::from_nurb(NurbCurve::new(pw, p, u).expect("curve"), 3, false)
    }

    fn close(a: f64, b: f64) -> bool {
        (a - b).abs() <= 1e-12 * (1.0 + a.abs().max(b.abs()))
    }

    fn linear(r0: f64, r1: f64) -> FilletLaw {
        FilletLaw::new(FilletLawKind::Linear {
            start_radius: r0,
            end_radius: r1,
        })
    }

    fn bspline(law_curve: BSplineCurve, map: Extent1d) -> FilletLaw {
        let mut law = FilletLaw::new(FilletLawKind::BSpline {
            law_curve,
            extended_law: None,
        });
        law.edge_map = map;
        law
    }

    #[test]
    fn linear_law_values_and_reverse() {
        let mut law = linear(2.0, 0.0);
        assert_eq!(law.edge_map.min(), 0.0);
        assert_eq!(law.edge_map.max(), 1.0);
        assert_eq!(law.edge_arc_length, 0.0);
        let ivl = Extent1d::from_min_max(0.0, 10.0);
        assert_eq!(
            law.evaluate(2.5, &ivl, false).expect("eval"),
            [1.5, -0.2, 0.0]
        );
        assert_eq!(
            law.evaluate(2.5, &ivl, true).expect("eval"),
            [0.5, 0.2, 0.0]
        );
        // a sub-map: only [0.5, 1] of the function maps to the edge
        let mut law = linear(2.0, 0.0);
        law.edge_map = Extent1d::from_min_max(0.5, 1.0);
        assert_eq!(
            law.evaluate(0.0, &ivl, false).expect("eval"),
            [1.0, -0.1, 0.0]
        );
        // a zero-length map means none
        assert!(law.is_linear_law().expect("linear"));
        assert_eq!(law.type_code(), LINEAR_FILLET_LAW_TYPE);
        assert!(law.is_kind_of(FILLET_LAW_TYPE));
        assert!(law.is_kind_of(11001));
        assert!(!law.is_kind_of(BSPLINE_FILLET_LAW_TYPE));
    }

    #[test]
    fn base_law_errors() {
        let mut law = FilletLaw::new(FilletLawKind::Base);
        assert!(law
            .evaluate(0.5, &Extent1d::from_min_max(0.0, 1.0), false)
            .is_err());
        assert_eq!(law.type_string(), "SmFilletLaw_TYPE");
        assert!(law.is_kind_of(FILLET_LAW_TYPE));
        assert!(!law.is_linear_law().expect("linear"));
    }

    #[test]
    fn bspline_law_inside_snap_and_extension() {
        // a degree-1 law from 1 to 3 over [0, 2], mapped onto the edge interval [10, 14]
        let mut law = bspline(
            radius_curve(&[1.0, 3.0], 1, 0.0, 2.0),
            Extent1d::from_min_max(0.0, 2.0),
        );
        assert!(law.is_linear_law().expect("linear"));
        assert_eq!(law.type_code(), BSPLINE_FILLET_LAW_TYPE);
        let ivl = Extent1d::from_min_max(10.0, 14.0);
        let v = law.evaluate(11.0, &ivl, false).expect("eval");
        assert!(
            close(v[0], 1.5) && close(v[1], 0.5) && close(v[2], 0.0),
            "{v:?}"
        );
        let v = law.evaluate(11.0, &ivl, true).expect("eval");
        assert!(close(v[0], 2.5) && close(v[1], -0.5), "{v:?}");
        assert!(law.extended_curve().is_none());
        // just outside: snaps to the end
        let v = law.evaluate(14.0 + 1e-13, &ivl, false).expect("eval");
        assert!(close(v[0], 3.0), "{v:?}");
        assert!(law.extended_curve().is_none());
        // further outside: extends the line (to at least a tenth of the domain)
        let v = law.evaluate(15.0, &ivl, false).expect("eval");
        assert!(close(v[0], 3.5) && close(v[1], 0.5), "{v:?}");
        let ext = law.extended_curve().expect("extended").natural_interval();
        assert!(ext.max() >= 3.0 && ext.min() <= 0.0, "{ext:?}");
        // zero-length interval
        assert!(law
            .evaluate(1.0, &Extent1d::from_min_max(3.0, 3.0), false)
            .is_err());
    }

    #[test]
    fn closed_law_curve_extension_is_a_copy() {
        // smFilletTest.cpp:307: the cubic r, 1.2r, 0.8r, r is G1-closed, so NLib hands
        // back an unextended copy and the evaluation clamps to the end
        let mut law = bspline(
            radius_curve(&[1.0, 1.2, 0.8, 1.0], 3, 0.0, 10.0),
            Extent1d::from_min_max(0.0, 10.0),
        );
        let ivl = Extent1d::from_min_max(0.0, 10.0);
        let v = law.evaluate(5.0, &ivl, false).expect("eval");
        assert!(close(v[0], 1.0), "{v:?}");
        let v = law.evaluate(12.0, &ivl, false).expect("eval");
        assert!(close(v[0], 1.0), "{v:?}");
        let ext = law.extended_curve().expect("copy").natural_interval();
        assert_eq!((ext.min(), ext.max()), (0.0, 10.0));
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
    struct H(u32);
    impl ClassObject for H {
        fn class_kind(&self) -> crate::intersect::point_class::ClassObjectKind {
            crate::intersect::point_class::ClassObjectKind::Other
        }
    }
    impl SolutionObject for H {
        fn kind(&self) -> crate::geom::solution_array::ObjectKind {
            crate::geom::solution_array::ObjectKind::Other
        }
    }

    #[test]
    fn error_text_is_snprintf_into_the_tblock_buffer() {
        assert_eq!(tblock_text(None), "(null)");
        assert_eq!(tblock_text(Some(&"x".repeat(2000))).len(), 1023);
        // 'é' is two bytes: byte 1023 falls inside the 512th one.
        assert_eq!(tblock_text(Some(&"é".repeat(600))).len(), 1022);
    }

    #[test]
    fn note_errors_follow_the_overloads() {
        let mut e = ErrorInfo::<H>::new(Some(H(9)));
        e.note_error(FilletErrorType::BadInt, None);
        assert_eq!((e.code, e.comment.as_str()), (5, "<no comment>\n"));
        e.note_edge_error(
            None,
            FilStatus::Failure,
            FilletErrorType::BadMerge,
            Some("ignored"),
        );
        assert_eq!((e.code, e.edge), (5, None));
        e.note_merge_error(
            FilletErrorType::EdgeProblem,
            FilStatus::Failure,
            Some(H(1)),
            Some(H(2)),
            None,
            Some(H(3)),
            None,
        );
        e.note_merge_error(
            FilletErrorType::EdgeProblem,
            FilStatus::Failure,
            Some(H(1)),
            Some(H(2)),
            None,
            Some(H(3)),
            Some("again"),
        );
        assert_eq!(e.edges, vec![None]);
        assert_eq!(e.surfaces, vec![Some(H(3))]);
        assert_eq!(e.comment, "again");
        let objects = ExecutiveObjects {
            executive: H(9),
            pseudo_brep: H(10),
            edge_attribute: H(11),
        };
        let x = FilletExecutive::new(Some(H(1)), objects);
        assert!(x.intersector.ignore_tangency && !x.initialized);
        assert_eq!(
            (x.error.executive, x.pseudo_brep),
            (Some(H(9)), Some(H(10)))
        );
    }

    #[test]
    fn vertex_error_note_retains_all_fields_and_keeps_a_null_comment() {
        let mut e = ErrorInfo::<H>::new(Some(H(9)));
        let first = Point3::new(1.0, 2.0, 3.0);
        e.note_vertex_error(
            Some(H(4)),
            Some(first),
            FilStatus::Failure,
            FilletErrorType::CornerUnsupported,
            Some("Corner case not implemented"),
        );
        assert_eq!(e.vertex, Some(H(4)));
        assert_eq!(e.location, Some(first));
        assert_eq!((e.status, e.code), (2, 9));
        assert_eq!(e.comment, "Corner case not implemented");
        // The port keeps its existing codes 0..=8 (pinned 0..=7 plus `NoRoom` = 8)
        // and gives the new code 9.
        assert_eq!(FilletErrorType::CornerUnsupported as i32, 9);
        assert_eq!(FilletErrorType::NoRoom as i32, 8);
        // No edge field is touched by the vertex overload.
        assert_eq!((e.edge, e.edges.len(), e.surfaces.len()), (None, 0, 0));

        // A null comment keeps the old text (the fork's `if (cComment != NULL)`), but
        // every other field is replaced.
        let second = Point3::new(-1.0, 0.5, 7.0);
        e.note_vertex_error(
            Some(H(5)),
            Some(second),
            FilStatus::Processed,
            FilletErrorType::BadTrim,
            None,
        );
        assert_eq!(e.vertex, Some(H(5)));
        assert_eq!(e.location, Some(second));
        assert_eq!((e.status, e.code), (1, 6));
        assert_eq!(e.comment, "Corner case not implemented");

        // A null vertex records nothing, as the pinned overload returns early.
        e.note_vertex_error(
            None,
            Some(first),
            FilStatus::Failure,
            FilletErrorType::BadMerge,
            Some("ignored"),
        );
        assert_eq!(e.vertex, Some(H(5)));
        assert_eq!(e.location, Some(second));
        assert_eq!((e.status, e.code), (1, 6));
        assert_eq!(e.comment, "Corner case not implemented");
    }

    #[test]
    fn create_corners_notes_an_unsupported_vertex_only_when_opted_in() {
        use crate::blend::fillet_executive::p3::VertexEntry;

        let point = Point3::new(4.0, 5.0, 6.0);
        for (fix, expected) in [(false, (0, None, None)), (true, (9, Some(H(3)), None))] {
            let mut k = Kernel::default();
            k.fixes.fillet_vertex_error_diagnostics = fix;
            let mut executive = corner_creation_executive();
            executive.filleted_vertices = vec![Some(H(3))];
            executive
                .vertex_corner_map
                .insert(Some(H(3)), VertexEntry::SolverEdgeuses(vec![H(20)]));
            let mut info = ErrorInfo::new(None);
            info.code = 99;
            executive
                .create_fillet_corners_recording_vertex_errors(
                    &k,
                    |_, _, vertex, _| {
                        assert_eq!(vertex, Some(H(3)));
                        Ok(CornerCreation::Unsupported {
                            location: Some(point),
                        })
                    },
                    Some(&mut info),
                )
                .unwrap();
            // The optional out-parameter copy follows upstream `operator=`, which does
            // not copy the location, comment or entity name.
            assert_eq!((info.code, info.vertex, info.location), expected);
            assert_eq!(info.comment, "<no comment>\n");
            if fix {
                assert_eq!(info.status, 2);
                assert_eq!(executive.error.status, 2);
                assert_eq!(executive.error.location, Some(point));
                assert_eq!(executive.error.comment, "Corner case not implemented");
            } else {
                // The pinned default: no note, so the executive's own info is untouched.
                assert_eq!(
                    (
                        executive.error.code,
                        executive.error.status,
                        executive.error.vertex,
                        executive.error.location
                    ),
                    (0, 0, None, None)
                );
                assert_eq!(executive.error.comment, "<no comment>\n");
            }
            // Diagnostics only: the pending entry and the corner list are unchanged.
            assert!(executive.corners.is_empty());
            assert_eq!(
                executive.vertex_corner_map.get(&Some(H(3))),
                Some(&VertexEntry::SolverEdgeuses(vec![H(20)]))
            );
        }
    }

    #[test]
    fn create_corners_original_factory_shape_still_notes_without_a_location() {
        use crate::blend::fillet_executive::p3::VertexEntry;

        let mut k = Kernel::default();
        k.fixes.fillet_vertex_error_diagnostics = true;
        let mut executive = corner_creation_executive();
        executive.filleted_vertices = vec![Some(H(3))];
        executive
            .vertex_corner_map
            .insert(Some(H(3)), VertexEntry::SolverEdgeuses(vec![H(20)]));
        executive
            .create_fillet_corners(&k, |_, _, _, _| Ok(None), None)
            .unwrap();
        assert_eq!(
            (
                executive.error.code,
                executive.error.vertex,
                executive.error.location
            ),
            (9, Some(H(3)), None)
        );
        // The `None` error-info argument still leaves the executive's own record
        // readable, with the fork's status and comment.
        assert_eq!(executive.error.status, 2);
        assert_eq!(executive.error.comment, "Corner case not implemented");
    }

    fn corner_creation_executive() -> FilletExecutive<H> {
        FilletExecutive::new(
            Some(H(1)),
            ExecutiveObjects {
                executive: H(9),
                pseudo_brep: H(10),
                edge_attribute: H(11),
            },
        )
    }

    #[test]
    fn create_corners_preserves_order_pending_and_final_error_assignment() {
        use crate::blend::fillet_executive::p3::VertexEntry;

        let k = Kernel::default();
        let mut executive = corner_creation_executive();
        executive.filleted_vertices = vec![Some(H(3)), Some(H(2)), Some(H(4))];
        for &vertex in &executive.filleted_vertices {
            executive
                .vertex_corner_map
                .insert(vertex, VertexEntry::SolverEdgeuses(vec![H(20), H(21)]));
        }
        executive.error.code = 17;
        let mut info = ErrorInfo::new(None);
        let mut seen = Vec::new();
        executive
            .create_fillet_corners(
                &k,
                |_, live, vertex, edgeuses| {
                    seen.push(vertex);
                    assert_eq!(edgeuses, &[H(20), H(21)]);
                    assert_eq!(live.corners.len(), usize::from(seen.len() > 1));
                    Ok(match vertex {
                        Some(H(3)) => Some(H(30)),
                        Some(H(4)) => Some(H(40)),
                        _ => None,
                    })
                },
                Some(&mut info),
            )
            .unwrap();
        assert_eq!(seen, executive.filleted_vertices);
        assert_eq!(executive.corners, vec![H(30), H(40)]);
        assert_eq!(
            executive.vertex_corner_map.get(&Some(H(2))),
            Some(&VertexEntry::SolverEdgeuses(vec![H(20), H(21)]))
        );
        assert_eq!(
            executive.vertex_corner_map.get(&Some(H(3))),
            Some(&VertexEntry::Corner(H(30)))
        );
        assert_eq!(info.code, 17);
    }

    #[test]
    fn create_corners_failure_keeps_completed_prefix_and_error_output() {
        use crate::blend::fillet_executive::p3::VertexEntry;

        let k = Kernel::default();
        let mut executive = corner_creation_executive();
        executive.filleted_vertices = vec![Some(H(2)), Some(H(3)), Some(H(4))];
        for &vertex in &executive.filleted_vertices {
            executive
                .vertex_corner_map
                .insert(vertex, VertexEntry::SolverEdgeuses(vec![H(20)]));
        }
        executive.error.code = 17;
        let mut info = ErrorInfo::new(None);
        info.code = 99;
        let mut seen = Vec::new();
        let failure = SolidError::invalid_input("corner_creation_test", "factory failure");
        let result = executive.create_fillet_corners(
            &k,
            |_, _, vertex, _| {
                seen.push(vertex);
                if seen.len() == 2 {
                    Err(failure.clone())
                } else {
                    Ok(Some(H(30)))
                }
            },
            Some(&mut info),
        );
        assert_eq!(result.unwrap_err(), failure);
        assert_eq!(seen, vec![Some(H(2)), Some(H(3))]);
        assert_eq!(executive.corners, vec![H(30)]);
        assert_eq!(
            executive.vertex_corner_map.get(&Some(H(2))),
            Some(&VertexEntry::Corner(H(30)))
        );
        assert_eq!(
            executive.vertex_corner_map.get(&Some(H(3))),
            Some(&VertexEntry::SolverEdgeuses(vec![H(20)]))
        );
        assert_eq!(info.code, 99);
    }

    #[test]
    fn create_corners_missing_completed_and_null_context_guards() {
        use crate::blend::fillet_executive::p3::VertexEntry;

        let k = Kernel::default();
        let mut executive = corner_creation_executive();
        executive.filleted_vertices.push(Some(H(2)));
        let result = executive.create_fillet_corners(&k, |_, _, _, _| unreachable!(), None);
        assert!(matches!(
            result,
            Err(SolidError::Upstream { code: 1006, .. })
        ));
        executive
            .vertex_corner_map
            .insert(Some(H(2)), VertexEntry::Corner(H(30)));
        let result = executive.create_fillet_corners(&k, |_, _, _, _| unreachable!(), None);
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));

        // Rust-only lifecycle guard: never pass null vertices to the native factory.
        executive.filleted_vertices = vec![None];
        executive
            .vertex_corner_map
            .insert(None, VertexEntry::SolverEdgeuses(vec![H(20)]));
        let result = executive.create_fillet_corners(
            &k,
            |_, _, vertex, _| {
                assert!(vertex.is_none());
                Err(SolidError::invalid_input(
                    "corner_creation_test",
                    "null vertex",
                ))
            },
            None,
        );
        assert!(matches!(result, Err(SolidError::InvalidInput { .. })));
        assert!(executive.corners.is_empty());
        assert!(matches!(
            executive.vertex_corner_map.get(&None),
            Some(VertexEntry::SolverEdgeuses(_))
        ));
    }
}
