//! Curve sweeps of edges and face boundaries, and circular pipe sweeps.
//!
//! Upstream: `source/SMLib/src/SmPrimitiveCreation.cpp`.
//!
//! | Rust | Upstream |
//! |------|----------|
//! | [`PrimitiveCreation::create_curve_sweep_from_edges`] | `CreateCurveSweepFromEdges` |
//! | [`PrimitiveCreation::create_curve_sweep_from_faces`] | `CreateCurveSweepFromFaces` |
//! | [`PrimitiveCreation::create_pipe_sweep`] | `CreatePipeSweep` |
//!
//! Deviations: input edges and faces are typed ids, possibly of another B-rep.
//! A missing edge curve (a null dereference upstream) and a failed curve copy
//! (a null `Trim` target upstream) are errors. The pipe's circle stays a null
//! profile slot when it cannot be built, as upstream ignores that status. The
//! path's box uses the B-spline override, as the typed path is a B-spline; for a
//! line path that override evaluates the interval's end points, as
//! `SmLine::CalculateBoundingBox` does (`SmLine.cpp:605-657`), so the box is the
//! same. No recursion.

use super::PrimitiveCreationAccess;
use crate::curve::curve::Curve;
use crate::curve::ellipse::Ellipse;
use crate::error::{ignore_status, ignored, Result, SolidError};
use crate::geom::axis2_placement::Axis2Placement;
use crate::geom::extent1d::Extent1d;
use crate::geom::extent3d::Extent3d;
use crate::geom::tol::ZONE_TOL_3D;
use crate::nurbs::b_spline_curve::p1::BSplineCurve;
use crate::nurbs::b_spline_curve::p2::BoundingBoxRequest;
use crate::nurbs::b_spline_curve_n_lib::TrimMode;
use crate::primitive::primitive_creation::{p1::PrimitiveCreation, SweepOptions};
use crate::sweep::topology_sweep::access::{EdgeRef, FaceRef};
use crate::topology::{BrepId, FaceId};

const OP: &str = "primitive::primitive_creation::p2::curve_sweeps";

/// Copy an edge's curve in the target context and trim the copy to the edge's
/// interval, ignoring the trim status (`Trim(sInterval, FALSE)`).
fn trimmed_edge_curve(
    a: &impl PrimitiveCreationAccess,
    target: BrepId,
    edge: EdgeRef,
) -> Result<Curve> {
    let k = a.kernel(target)?;
    let source = a.brep(edge.0)?;
    let curve = crate::topology::edge::curve(source, edge.1)?
        .ok_or_else(|| SolidError::null_pointer(OP))?;
    let mut copy = curve.copy(k)?;
    let mut interval = crate::topology::edge::interval(source, edge.1)?;
    ignore_status(copy.trim(k, &mut interval, TrimMode::Silent))?;
    Ok(copy)
}

impl PrimitiveCreation {
    /// Sweep copies of the edges' curves, each trimmed to its edge, along `path`
    /// (`CreateCurveSweepFromEdges`), with p3's `create_curve_sweep` and its
    /// default continuity test. The edges and their curves are unchanged.
    ///
    /// # Errors
    ///
    /// A null pointer for no target region or a missing edge curve; those of the
    /// curve copy; those of `create_curve_sweep` (`SM_ERR` for no edges).
    pub fn create_curve_sweep_from_edges(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        edges: &[EdgeRef],
        path: &BSplineCurve,
        scale_reference: Option<&BSplineCurve>,
        scale_curve: Option<&BSplineCurve>,
        approx_tol: f64,
        options: SweepOptions,
        start_faces: &mut Vec<FaceId>,
        side_faces: &mut Vec<FaceId>,
        end_faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        // Get Profile curves from input edges (in the target region's context)
        if self.region().is_none() {
            return Err(SolidError::null_pointer(OP));
        }
        let mut curves = Vec::new();
        for &edge in edges {
            curves.push(Some(trimmed_edge_curve(a, brep, edge)?));
        }

        // Now call Sweep with profile curves (the B-spline reference as an SmCurve)
        let scale_reference = scale_reference.cloned().map(Curve::from);
        self.create_curve_sweep(
            a,
            brep,
            &curves,
            path,
            scale_reference.as_ref(),
            scale_curve,
            approx_tol,
            options,
            true,
            start_faces,
            side_faces,
            end_faces,
        )
    }

    /// Sweep copies of every edge curve of `faces`, in face then edge order, along
    /// `path` (`CreateCurveSweepFromFaces`). The faces and their curves are unchanged.
    ///
    /// # Errors
    ///
    /// As [`PrimitiveCreation::create_curve_sweep_from_edges`], plus those of the
    /// face edge walk.
    pub fn create_curve_sweep_from_faces(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        input_faces: &[FaceRef],
        path: &BSplineCurve,
        scale_reference: Option<&BSplineCurve>,
        scale_curve: Option<&BSplineCurve>,
        approx_tol: f64,
        options: SweepOptions,
        start_faces: &mut Vec<FaceId>,
        side_faces: &mut Vec<FaceId>,
        end_faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        // Get Profile curves from input faces (in the target region's context)
        if self.region().is_none() {
            return Err(SolidError::null_pointer(OP));
        }
        let mut curves = Vec::new();
        for &face in input_faces {
            // GetEdges resets its output for every face
            for edge in a.face_edges(face)? {
                curves.push(Some(trimmed_edge_curve(a, brep, (face.0, edge))?));
            }
        }

        // Now call Sweep with profile curves (the B-spline reference as an SmCurve)
        let scale_reference = scale_reference.cloned().map(Curve::from);
        self.create_curve_sweep(
            a,
            brep,
            &curves,
            path,
            scale_reference.as_ref(),
            scale_curve,
            approx_tol,
            options,
            true,
            start_faces,
            side_faces,
            end_faces,
        )
    }

    /// Sweep a circle of `radius`, centred and oriented perpendicular to the path
    /// start, along `path` (`CreatePipeSweep`). `_approx_tol` is unused upstream;
    /// the sweep's tolerance is `100 * ZONE_TOL_3D` times half the path box's
    /// diagonal. The three output lists are reset first.
    ///
    /// # Errors
    ///
    /// A null pointer for no target region; those of `create_curve_sweep`.
    pub fn create_pipe_sweep(
        &mut self,
        a: &mut impl PrimitiveCreationAccess,
        brep: BrepId,
        radius: f64,
        path: &BSplineCurve,
        _approx_tol: f64,
        cap_ends: bool,
        translational_sweep: bool,
        start_faces: &mut Vec<FaceId>,
        side_faces: &mut Vec<FaceId>,
        end_faces: &mut Vec<FaceId>,
    ) -> Result<()> {
        self.init_entity_maps();
        start_faces.clear();
        side_faces.clear();
        end_faces.clear();
        if self.region().is_none() {
            return Err(SolidError::null_pointer(OP));
        }

        // Get the Shape from the first pipe (its status is ignored upstream)
        let place = Axis2Placement::IDENTITY;
        let domain = Extent1d::from_min_max(0.0, 360.0);
        let circle = ignored(Ellipse::create_canonical_circle(
            a.kernel(brep)?,
            &place,
            radius,
            Some(&domain),
            None,
        ))?;
        let shape = vec![circle.map(Curve::from)];

        let mut bounds = Extent3d::new();
        if let Some(boxes) = ignored(path.calculate_bounding_box(
            &path.natural_interval(),
            BoundingBoxRequest {
                normal: true,
                ..BoundingBoxRequest::default()
            },
        ))? {
            if let Some(normal) = boxes.normal {
                bounds = normal;
            }
        }
        let mut size = bounds.size().length();
        size /= 2.0;
        let tolerance = size * ZONE_TOL_3D;

        let options = SweepOptions {
            cap_ends,
            translational_sweep,
            move_profile_center_to_path: true,
            orient_profile_perpendicular_to_path: true,
            move_to_profile: false,
            profile_at_path_start: false,
        };
        self.create_curve_sweep(
            a,
            brep,
            &shape,
            path,
            None,
            None,
            tolerance * 100.0,
            options,
            true,
            start_faces,
            side_faces,
            end_faces,
        )
    }
}
